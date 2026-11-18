//! Output delivery: target resolution, atomic file replacement, and stdout streaming.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::core::error::DomainError;
use crate::core::model::OutputTarget;
use crate::shell::error::{IngestError, io_error_at, sanitized_io_error};

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Output target resolved against the filesystem. Only [`resolve_output_target`] constructs it,
/// so collection and delivery never receive an unresolved request.
#[derive(Debug)]
pub(super) struct ResolvedOutput(OutputTarget);

impl ResolvedOutput {
    pub(super) const STDOUT: Self = Self(OutputTarget::Stdout);

    /// Canonical file path to exclude from the local walk, if any.
    pub(super) fn file(&self) -> Option<&Path> {
        match &self.0 {
            OutputTarget::File(path) => Some(path.as_path()),
            OutputTarget::Stdout => None,
        }
    }
}

/// Whether `path` ends with a path separator (`out/`) or a trailing `.` segment (`out/.`),
/// which `Path::file_name()` would otherwise strip.
fn has_trailing_directory_syntax(path: &Path) -> bool {
    let bytes = path.as_os_str().as_encoded_bytes();
    let bytes = bytes
        .strip_suffix(b".")
        .filter(|prefix| {
            prefix
                .last()
                .is_some_and(|&b| std::path::is_separator(char::from(b)))
        })
        .unwrap_or(bytes);
    bytes
        .last()
        .is_some_and(|&b| std::path::is_separator(char::from(b)))
}

/// Parent directory of `path`, or `.` when the path is a bare filename.
fn parent_dir(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// Resolves an [`OutputTarget`] into its delivery destination: stdout passes through, and a
/// file target becomes the canonical path the digest is written to.
pub(super) fn resolve_output_target(target: &OutputTarget) -> Result<ResolvedOutput, IngestError> {
    match target {
        OutputTarget::Stdout => Ok(ResolvedOutput::STDOUT),
        OutputTarget::File(path) => {
            let file_name = (!has_trailing_directory_syntax(path))
                .then(|| path.file_name())
                .flatten()
                .ok_or_else(|| DomainError::usage("output path has no filename"))?;
            let parent = parent_dir(path);
            let canonical_parent = fs::canonicalize(parent).map_err(|e| {
                sanitized_io_error(
                    e.kind(),
                    format!(
                        "failed to resolve output directory '{}': {e}",
                        parent.display()
                    ),
                )
            })?;
            if !canonical_parent.is_dir() {
                return Err(sanitized_io_error(
                    io::ErrorKind::NotADirectory,
                    format!("output directory '{}' is not a directory", parent.display()),
                ));
            }
            let mut resolved = canonical_parent.join(file_name);
            if resolved.is_dir() {
                return Err(IngestError::Io(io::Error::new(
                    io::ErrorKind::IsADirectory,
                    "output path is a directory",
                )));
            }
            // Canonicalize an existing regular file so case-insensitive matches
            // (e.g. Windows/macOS) align with on-disk paths during exclusion checks.
            if fs::symlink_metadata(&resolved).is_ok_and(|m| !m.is_symlink())
                && let Ok(canonical) = fs::canonicalize(&resolved)
            {
                resolved = canonical;
            }
            Ok(ResolvedOutput(OutputTarget::File(resolved)))
        }
    }
}

/// Delivers the rendered bytes to a resolved target.
///
/// A file target is written atomically via temporary file and rename;
/// stdout target streams the buffer directly.
pub(super) fn deliver(resolved_target: &ResolvedOutput, bytes: &[u8]) -> Result<(), IngestError> {
    match &resolved_target.0 {
        OutputTarget::File(path) => write_atomic(path, bytes),
        OutputTarget::Stdout => {
            let stdout = io::stdout();
            render_stdout_to(&mut stdout.lock(), bytes)
        }
    }
}

fn render_stdout_to<W: Write>(out: &mut W, bytes: &[u8]) -> Result<(), IngestError> {
    match out.write_all(bytes).and_then(|()| out.flush()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(IngestError::Io(e)),
    }
}

/// Returns the elapsed duration since the Unix epoch in nanoseconds.
fn current_epoch_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos())
}

/// Path of a temporary file, unlinked on drop unless persisted. It holds no handle. The file it was
/// created with is declared after this guard, so it drops, and closes, first.
struct TempPath(PathBuf);

impl TempPath {
    /// Creates `.codefeed.{nanos}.{counter}.tmp` in `parent`. The name does not depend on the target,
    /// so its length stays within the filename limit for any output name.
    fn create(parent: &Path) -> Result<(Self, fs::File), IngestError> {
        Self::create_internal(parent, current_epoch_nanos(), &TEMP_FILE_COUNTER)
    }

    fn create_internal(
        parent: &Path,
        timestamp: u128,
        next_counter: &AtomicU64,
    ) -> Result<(Self, fs::File), IngestError> {
        loop {
            let counter = next_counter.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(".codefeed.{timestamp}.{counter}.tmp"));
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => return Ok((Self(path), file)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    // Try again with a new counter.
                }
                Err(error) => return Err(io_error_at(&path, &error)),
            }
        }
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// Renames the file over `target`. If the rename fails, the file stays and is unlinked on drop.
    /// On success the guard is forgotten, so the renamed file is never unlinked.
    fn persist(mut self, target: &Path) -> Result<(), IngestError> {
        fs::rename(self.path(), target).map_err(|err| io_error_at(target, &err))?;
        drop(std::mem::take(&mut self.0));
        std::mem::forget(self);
        Ok(())
    }
}

impl Drop for TempPath {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Warning text for a failed parent-directory sync, or `None` when the sync succeeded.
#[cfg(unix)]
fn sync_warning(parent: &Path, synced: io::Result<()>) -> Option<String> {
    synced.err().map(|err| {
        use crate::core::error::ErrorDetail;

        ErrorDetail::new(format!("warning: {}: {err}", parent.display())).into_inner()
    })
}

/// Replaces `output` with `bytes`, so a failed write leaves the previous file in place.
///
/// # Errors
///
/// Returns [`IngestError::Io`] if creating, writing, syncing, or renaming fails. A parent-directory
/// sync failure after the rename is a warning on stderr, not an error, because the target already holds
/// the new digest and the run has succeeded.
fn write_atomic(output: &Path, bytes: &[u8]) -> Result<(), IngestError> {
    let parent = parent_dir(output);
    let (temp, mut file) = TempPath::create(parent)?;

    #[cfg(unix)]
    match fs::metadata(output) {
        Ok(metadata) => {
            file.set_permissions(metadata.permissions())
                .map_err(|error| io_error_at(temp.path(), &error))?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error_at(output, &error)),
    }

    file.write_all(bytes)
        .map_err(|err| io_error_at(output, &err))?;
    file.sync_all()
        .map_err(|err| io_error_at(temp.path(), &err))?;
    drop(file);
    temp.persist(output)?;

    #[cfg(unix)]
    {
        let synced = fs::File::open(parent).and_then(|dir| dir.sync_all());
        if let Some(warning) = sync_warning(parent, synced) {
            super::stdio::write_stderr_line(&warning);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::shell::testing::temp_dir;

    /// Writer whose every operation reports a closed pipe.
    struct BrokenPipeWriter;

    impl io::Write for BrokenPipeWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        }
    }

    /// Resolves a file target and returns the canonical path it will be written to.
    fn resolve_file(path: PathBuf) -> Result<PathBuf, IngestError> {
        let resolved = resolve_output_target(&OutputTarget::File(path))?;
        resolved.file().map_or_else(
            || unreachable!("a file target resolves to a file"),
            |file| Ok(file.to_path_buf()),
        )
    }

    #[test]
    fn resolve_output_target_rejects_non_existent_parent() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = temp_dir("non_existent_parent")?;
        let non_existent_parent = root.join("missing_dir").join("out.txt");

        let result = resolve_output_target(&OutputTarget::File(non_existent_parent));
        match result {
            Err(IngestError::Io(err)) => {
                assert_eq!(err.kind(), io::ErrorKind::NotFound);
                assert!(err.to_string().contains("missing_dir"));
            }
            other => panic!("expected IngestError::Io(NotFound), got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn resolve_output_target_rejects_path_without_filename() {
        let mut cases = vec!["", ".", "..", "out/", "out/."];
        if cfg!(windows) {
            cases.extend(["out\\", "out\\."]);
        }
        for raw in cases {
            let err = resolve_output_target(&OutputTarget::File(PathBuf::from(raw))).unwrap_err();
            assert!(
                matches!(err, IngestError::Domain(DomainError::Usage(ref msg)) if msg.contains("no filename")),
                "expected no-filename error for {raw:?}, got {err:?}"
            );
        }
    }

    #[test]
    fn deliver_to_stdout_handles_empty_slice() -> Result<(), Box<dyn std::error::Error>> {
        let target = resolve_output_target(&OutputTarget::Stdout)?;
        deliver(&target, b"")?;
        Ok(())
    }

    #[test]
    fn render_stdout_to_ignores_broken_pipe() {
        let mut writer = BrokenPipeWriter;
        assert!(render_stdout_to(&mut writer, b"test content").is_ok());
    }

    #[test]
    fn stdout_reports_non_broken_pipe_error_after_writing_a_prefix() {
        let mut written = [0; 3];
        // A fixed-size slice accepts the prefix, then returns zero for the remaining bytes.
        let result = render_stdout_to(&mut written.as_mut_slice(), b"digest");
        assert_eq!(&written, b"dig");
        assert!(
            matches!(result, Err(IngestError::Io(err)) if err.kind() == io::ErrorKind::WriteZero)
        );
    }

    #[test]
    fn resolve_output_target_rejects_directory() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("output_dir_resolve_test")?;
        let existing_dir = root.join("existing_subdir");
        fs::create_dir(&existing_dir)?;

        let result = resolve_output_target(&OutputTarget::File(existing_dir));
        match result {
            Err(IngestError::Io(err)) => {
                assert_eq!(err.kind(), io::ErrorKind::IsADirectory);
            }
            other => panic!("expected IngestError::Io(IsADirectory), got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn resolve_output_target_rejects_file_parent() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("output_file_parent")?;
        let file_parent = root.join("file.txt");
        fs::write(&file_parent, "not a directory")?;

        let result = resolve_output_target(&OutputTarget::File(file_parent.join("out.txt")));
        match result {
            Err(IngestError::Io(err)) => {
                assert_eq!(err.kind(), io::ErrorKind::NotADirectory);
                assert!(
                    err.to_string().contains("file.txt"),
                    "unexpected message: {err}"
                );
            }
            other => panic!("expected IngestError::Io(NotADirectory), got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn resolve_output_target_handles_stdout_and_file() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(resolve_output_target(&OutputTarget::Stdout)?.file(), None);

        let root = temp_dir("output_target_test")?;
        let output = root.join("digest.txt");
        let resolved = resolve_output_target(&OutputTarget::File(output.clone()))?;
        assert_eq!(resolved.file(), Some(output.as_path()));
        Ok(())
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn matches_non_ascii_cased_files_pointing_to_same_file()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("output_non_ascii")?;
        let file_a = root.join("вывод.txt");
        fs::write(&file_a, "test")?;
        let file_b = root.join("ВЫВОД.TXT");
        let resolved = resolve_file(file_b)?;
        let canonical_a = fs::canonicalize(&file_a)?;
        assert_eq!(resolved, canonical_a);
        Ok(())
    }

    #[test]
    fn resolve_output_target_canonicalizes_existing_file() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = temp_dir("output_canonical_test")?;
        fs::create_dir_all(root.join("subdir"))?;
        let output = root.join("digest.txt");
        fs::write(&output, "output")?;
        let canonical_output = fs::canonicalize(&output)?;

        let resolved = resolve_file(output)?;
        assert_eq!(resolved, canonical_output);

        let alias_output = root.join("subdir").join("..").join("digest.txt");
        let alias_resolved = resolve_file(alias_output)?;
        assert_eq!(alias_resolved, canonical_output);

        #[cfg(any(windows, target_os = "macos"))]
        {
            let cased_output = root.join("DIGEST.TXT");
            let cased_resolved = resolve_file(cased_output)?;
            assert_eq!(cased_resolved, canonical_output);
        }

        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn resolve_output_target_resolves_symlinked_parent() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        let root = temp_dir("output_symlink_parent")?;
        let real_dir = root.join("real");
        fs::create_dir_all(&real_dir)?;
        let link_dir = root.join("link");
        symlink(&real_dir, &link_dir)?;

        let output_via_link = link_dir.join("digest.txt");
        let output_via_real = real_dir.join("digest.txt");
        fs::write(&output_via_real, "test")?;

        let resolved = resolve_file(output_via_link)?;
        let canonical_real = fs::canonicalize(&output_via_real)?;
        assert_eq!(resolved, canonical_real);
        Ok(())
    }

    #[cfg(any(unix, windows))]
    #[cfg_attr(
        windows,
        ignore = "creating a symlink on Windows requires Developer Mode or elevation"
    )]
    #[test]
    fn resolve_output_target_preserves_symlink_leaf_and_protects_target()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("output_symlink_leaf")?;
        let src_file = root.join("lib.rs");
        fs::write(&src_file, "original content")?;
        let out_link = root.join("out.txt");

        #[cfg(unix)]
        std::os::unix::fs::symlink(&src_file, &out_link)?;
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&src_file, &out_link)?;

        let resolved = resolve_file(out_link.clone())?;
        let canonical_src = fs::canonicalize(&src_file)?;
        assert_ne!(
            resolved, canonical_src,
            "resolved output must not follow the leaf symlink"
        );
        assert_eq!(resolved, out_link);
        Ok(())
    }

    #[test]
    fn atomic_write_ignores_stale_temporary_file() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("atomic")?;
        let output = root.join("digest.txt");
        fs::write(root.join(".codefeed.123.0.tmp"), "stale")?;
        write_atomic(&output, b"new")?;
        assert_eq!(fs::read(&output)?, b"new");
        assert_eq!(
            fs::read_to_string(root.join(".codefeed.123.0.tmp"))?,
            "stale"
        );
        Ok(())
    }

    #[test]
    fn write_atomic_accepts_long_file_name() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("long_file_name")?;
        let output = root.join("a".repeat(244));
        write_atomic(&output, b"new")?;
        assert_eq!(fs::read(&output)?, b"new");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn preserves_previous_output_on_atomic_failure() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_dir("atomic_failure")?;
        let output = root.join("digest.txt");
        fs::write(&output, "previous")?;
        // A read-only directory rejects the temporary file, so the write fails before the rename.
        fs::set_permissions(&root, fs::Permissions::from_mode(0o500))?;

        let result = write_atomic(&output, b"new");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;

        assert!(
            result.is_err(),
            "write into a read-only directory must fail"
        );
        assert_eq!(fs::read_to_string(&output)?, "previous");
        Ok(())
    }

    #[test]
    fn temp_path_cleans_up_with_open_handle_on_scope_exit() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = temp_dir("temp_path_scope")?;
        let temp_path = {
            let (temp, _file) = TempPath::create(&root)?;
            assert!(temp.path().exists());
            temp.path().to_path_buf()
        };
        assert!(
            !temp_path.exists(),
            "temporary file must be removed when its guard is dropped"
        );
        Ok(())
    }

    #[test]
    fn atomic_write_cleans_up_temporary_file_on_panic() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("atomic_cleanup")?;

        let panic_result = std::panic::catch_unwind(|| {
            let (_temp, mut file) = TempPath::create(&root).unwrap();
            let _ = file.write_all(b"partial");
            panic!("intentional panic during atomic write");
        });
        assert!(panic_result.is_err(), "operation should have panicked");
        let temp_count_after_panic = fs::read_dir(&root)?
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with(".codefeed."))
            .count();
        assert_eq!(
            temp_count_after_panic, 0,
            "temporary file should be cleaned up on panic unwinding"
        );
        Ok(())
    }

    #[test]
    fn write_atomic_fails_and_cleans_up_for_directory_target()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("output_dir_test")?;
        let existing_dir = root.join("existing_subdir");
        fs::create_dir(&existing_dir)?;

        let result = write_atomic(&existing_dir, b"");
        assert!(
            result.is_err(),
            "write_atomic on directory target must fail"
        );

        let temp_count = fs::read_dir(&root)?
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with(".codefeed."))
            .count();
        assert_eq!(
            temp_count, 0,
            "temporary file must be unlinked on write_atomic failure"
        );
        Ok(())
    }

    #[test]
    fn atomic_write_error_names_path() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("error_names_path")?;
        let output = root.join("missing").join("out.txt");

        let Err(err) = write_atomic(&output, b"new") else {
            return Err("write into a missing directory must fail".into());
        };
        assert!(
            err.to_string().contains("missing"),
            "unexpected message: {err}"
        );
        Ok(())
    }

    #[test]
    fn temp_path_create_retries_on_collision() -> Result<(), Box<dyn std::error::Error>> {
        let temp = temp_dir("atomic_collision")?;
        let timestamp = 999_999_999;
        let counter = AtomicU64::new(0);
        let colliding_path = temp.join(format!(".codefeed.{timestamp}.0.tmp"));
        fs::write(&colliding_path, "pre-existing")?;

        let (guard, _file) = TempPath::create_internal(&temp, timestamp, &counter)?;
        let expected = temp.join(format!(".codefeed.{timestamp}.1.tmp"));
        assert_eq!(guard.path(), expected.as_path());
        assert!(guard.path().exists());
        assert_eq!(fs::read_to_string(&colliding_path)?, "pre-existing");
        assert_eq!(counter.load(Ordering::Relaxed), 2);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn unix_atomic_write_preserves_target_permissions() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_dir("output_permissions_test")?;
        let output = root.join("restricted.txt");

        fs::write(&output, "initial content")?;
        fs::set_permissions(&output, fs::Permissions::from_mode(0o600))?;

        write_atomic(&output, b"overwritten content")?;

        let metadata = fs::metadata(&output)?;
        assert_eq!(
            metadata.permissions().mode() & 0o777,
            0o600,
            "permissions must remain 0600 after overwrite"
        );
        assert_eq!(fs::read_to_string(&output)?, "overwritten content");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn unix_atomic_write_creates_non_existent_target() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("output_non_existent_test")?;
        let output = root.join("new_file.txt");

        assert!(!output.exists());

        write_atomic(&output, b"new content")?;

        assert!(output.exists());
        assert_eq!(fs::read_to_string(&output)?, "new content");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_cleans_up_temp_file_on_metadata_error() -> Result<(), Box<dyn std::error::Error>>
    {
        use std::os::unix::fs::symlink;

        let root = temp_dir("create_metadata_error")?;
        let loop_a = root.join("loop_a");
        let loop_b = root.join("loop_b");
        symlink(&loop_b, &loop_a)?;
        symlink(&loop_a, &loop_b)?;

        let result = write_atomic(&loop_a, b"");
        assert!(result.is_err(), "write_atomic on symlink loop must fail");

        let temp_count = fs::read_dir(&root)?
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with(".codefeed."))
            .count();
        assert_eq!(
            temp_count, 0,
            "temporary file must be unlinked when write_atomic fails during metadata check"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn sync_warning_reports_only_failed_directory_syncs() {
        let parent = Path::new("/var/data");
        assert_eq!(sync_warning(parent, Ok(())), None);

        let warning = sync_warning(parent, Err(io::Error::other("fsync rejected")))
            .expect("failed sync yields a warning");
        assert_eq!(warning, "warning: /var/data: fsync rejected");
    }
}
