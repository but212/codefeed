//! Output delivery: atomic filesystem writing and stdout streaming.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::IngestError;
use crate::core::error::DomainError;
use crate::core::model::OutputTarget;

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn ignore_broken_pipe(result: io::Result<()>) -> io::Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(e),
    }
}

pub(super) fn write_stdout_line_to<W: io::Write>(out: &mut W, line: &str) -> io::Result<()> {
    ignore_broken_pipe(writeln!(out, "{line}"))
}

pub(super) fn write_stdout_line(line: &str) {
    let stdout = io::stdout();
    let mut lock = stdout.lock();
    let _ = write_stdout_line_to(&mut lock, line);
}

/// Returns the elapsed duration since the Unix epoch in nanoseconds.
fn current_epoch_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// Parent directory of `path`, or `.` when the path is a bare filename.
fn parent_dir(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// Resolves an [`OutputTarget`] into its delivery destination: stdout passes through, and a
/// file target becomes the canonical path the digest is written to.
pub(super) fn resolve_output_target(target: &OutputTarget) -> Result<OutputTarget, IngestError> {
    match target {
        OutputTarget::Stdout => Ok(OutputTarget::Stdout),
        OutputTarget::File(path) => {
            let file_name = path
                .file_name()
                .ok_or_else(|| DomainError::usage("output path has no filename"))?;
            let parent = parent_dir(path);
            let canonical_parent = fs::canonicalize(parent).map_err(|e| {
                IngestError::Io(io::Error::new(
                    e.kind(),
                    format!(
                        "failed to resolve output directory '{}': {e}",
                        parent.display()
                    ),
                ))
            })?;
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
            Ok(OutputTarget::File(resolved))
        }
    }
}

/// Delivers the rendered bytes to a resolved target.
///
/// A file target is written atomically via temporary file and rename;
/// stdout target streams the buffer directly.
pub(super) fn deliver(resolved_target: &OutputTarget, bytes: &[u8]) -> Result<(), IngestError> {
    match resolved_target {
        OutputTarget::File(path) => write_atomic(path, bytes),
        OutputTarget::Stdout => {
            let stdout = io::stdout();
            render_stdout_to(&mut stdout.lock(), bytes)
        }
    }
}

fn render_stdout_to<W: Write>(out: &mut W, bytes: &[u8]) -> Result<(), IngestError> {
    ignore_broken_pipe(out.write_all(bytes).and_then(|()| out.flush())).map_err(IngestError::Io)
}

/// RAII guard managing a temporary file that cleans up on drop unless committed.
///
/// Encapsulates the file handle and path together so that drop explicitly closes
/// the underlying file handle before attempting to unlink the file.
/// This guarantees Windows-safe file deletion without relying on struct field order
/// or pattern let-binding declaration order invariants.
struct TempFile {
    path: PathBuf,
    file: Option<fs::File>,
    active: bool,
}

impl TempFile {
    fn create(parent: &Path, file_name: &str) -> Result<Self, IngestError> {
        let timestamp = current_epoch_nanos();
        loop {
            let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(".{file_name}.{timestamp}.{counter}.tmp"));
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => {
                    return Ok(Self {
                        path,
                        file: Some(file),
                        active: true,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(IngestError::Io(error)),
            }
        }
    }

    #[cfg(test)]
    fn path(&self) -> &Path {
        &self.path
    }

    fn file_mut(&mut self) -> &mut fs::File {
        self.file
            .as_mut()
            .expect("temporary file is active before commit")
    }

    #[cfg(unix)]
    fn set_permissions(&self, permissions: fs::Permissions) -> io::Result<()> {
        self.file
            .as_ref()
            .expect("temporary file is active before commit")
            .set_permissions(permissions)
    }

    fn commit(mut self, target: &Path) -> Result<(), IngestError> {
        let file = self
            .file
            .take()
            .expect("temporary file is active during commit");
        file.sync_all()?;
        drop(file);

        fs::rename(&self.path, target)?;
        self.active = false;
        Ok(())
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        // Drop the file handle first to close the open OS handle before removing the file.
        // On Windows, unlinking an open file fails with an access error.
        drop(self.file.take());
        if self.active {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Manages atomic output writing via a temporary file that is synced and renamed on commit.
struct AtomicFile {
    temp_file: TempFile,
    target: PathBuf,
}

impl AtomicFile {
    fn create(target: &Path) -> Result<Self, IngestError> {
        let file_name = target
            .file_name()
            .ok_or_else(|| DomainError::usage("output path has no filename"))?;
        let file_name = file_name.to_string_lossy();
        let temp_file = TempFile::create(parent_dir(target), &file_name)?;

        #[cfg(unix)]
        match fs::metadata(target) {
            Ok(metadata) => {
                temp_file.set_permissions(metadata.permissions())?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(IngestError::Io(error)),
        }

        Ok(Self {
            temp_file,
            target: target.to_path_buf(),
        })
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<(), IngestError> {
        self.temp_file.file_mut().write_all(bytes)?;
        Ok(())
    }

    /// Commits the atomic file write by syncing content, renaming to the target path,
    /// and syncing the parent directory on Unix.
    ///
    /// # Errors
    ///
    /// Returns [`IngestError::Io`] if flushing, renaming, or directory syncing fails.
    /// If parent directory sync fails on Unix after rename, target replacement has already
    /// completed and durability synchronization failed.
    fn commit(self) -> Result<(), IngestError> {
        self.temp_file.commit(&self.target)?;

        #[cfg(unix)]
        {
            let dir = fs::File::open(parent_dir(&self.target))?;
            dir.sync_all()?;
        }

        Ok(())
    }
}

fn write_atomic(output: &Path, bytes: &[u8]) -> Result<(), IngestError> {
    let mut file = AtomicFile::create(output)?;
    file.write_all(bytes)?;
    file.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::testing::temp_dir;
    use super::*;

    /// Resolves a file target and returns the canonical path it will be written to.
    fn resolve_file(path: PathBuf) -> Result<PathBuf, IngestError> {
        match resolve_output_target(&OutputTarget::File(path))? {
            OutputTarget::File(resolved) => Ok(resolved),
            OutputTarget::Stdout => unreachable!("a file target resolves to a file"),
        }
    }

    #[test]
    fn write_stdout_line_ignores_broken_pipe() {
        struct BrokenPipeWriter;
        impl io::Write for BrokenPipeWriter {
            fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "broken pipe"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "broken pipe"))
            }
        }
        let mut writer = BrokenPipeWriter;
        assert!(write_stdout_line_to(&mut writer, "hello").is_ok());
    }

    #[test]
    fn stdout_overflow_writes_nothing_to_stream() -> Result<(), Box<dyn std::error::Error>> {
        use crate::core::model::{CollectionResult, DigestFormat};
        use crate::core::policy::{MAX_OUTPUT_SIZE, NormalizedPath};
        use crate::core::storage::FileStorage;

        let large_content = "x".repeat(MAX_OUTPUT_SIZE as usize - 16);
        let storage = FileStorage::new()
            .push(&NormalizedPath::from("a.txt"), "hello\n")
            .push(&NormalizedPath::from("b.txt"), &large_content);

        let collection = CollectionResult {
            label: "overflow-repo".into(),
            revision: None,
            files: crate::core::storage::SortedFiles::new(storage),
            skipped: 0,
        };

        let mut sink = Vec::new();
        let rendered_res = collection.render(DigestFormat::Text);
        assert!(matches!(rendered_res, Err(DomainError::OutputTooLarge(_))));
        if let Ok(rendered) = rendered_res {
            let _ = render_stdout_to(&mut sink, &rendered.bytes);
        }
        assert!(sink.is_empty(), "stdout sink must remain empty on overflow");
        Ok(())
    }

    #[test]
    fn atomic_write_ignores_stale_temporary_file() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("atomic")?;
        let output = root.join("digest.txt");
        fs::write(root.join(".digest.txt.123.tmp"), "stale")?;
        write_atomic(&output, b"new")?;
        assert_eq!(fs::read(&output)?, b"new");
        assert_eq!(
            fs::read_to_string(root.join(".digest.txt.123.tmp"))?,
            "stale"
        );
        Ok(())
    }

    #[test]
    fn preserves_previous_output_on_atomic_failure() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("atomic_failure")?;
        let result = write_atomic(&root, b"new");
        assert!(result.is_err());
        assert!(root.is_dir());
        Ok(())
    }

    #[test]
    fn atomic_write_cleans_up_temporary_file_on_error_and_panic()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("atomic_cleanup")?;
        let output = root.join("digest.txt");

        // 1. Cleanup on atomic file drop before commit
        let file = AtomicFile::create(&output)?;
        let temp_path = file.temp_file.path().to_path_buf();
        assert!(temp_path.exists());
        drop(file);
        assert!(
            !temp_path.exists(),
            "temporary file should be cleaned up on drop"
        );

        // 2. Cleanup on panic
        let panic_result = std::panic::catch_unwind(|| {
            let mut file = AtomicFile::create(&output).unwrap();
            let _ = file.write_all(b"partial");
            panic!("intentional panic during atomic write");
        });
        assert!(panic_result.is_err(), "operation should have panicked");
        let temp_count_after_panic = fs::read_dir(&root)?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".digest.txt."))
            .count();
        assert_eq!(
            temp_count_after_panic, 0,
            "temporary file should be cleaned up on panic unwinding"
        );
        Ok(())
    }

    #[test]
    fn missing_output_filename_returns_usage_error() {
        let result = write_atomic(Path::new("."), b"");
        match result {
            Err(IngestError::Domain(DomainError::Usage(msg))) => {
                assert!(msg.contains("output path has no filename"));
            }
            other => panic!("expected DomainError::Usage, got {other:?}"),
        }
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
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(".existing_subdir.")
            })
            .count();
        assert_eq!(
            temp_count, 0,
            "temporary file must be unlinked on write_atomic failure"
        );
        Ok(())
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
    fn resolve_output_target_handles_stdout_and_file() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            resolve_output_target(&OutputTarget::Stdout)?,
            OutputTarget::Stdout
        );

        let root = temp_dir("output_target_test")?;
        let output = root.join("digest.txt");
        assert_eq!(
            resolve_output_target(&OutputTarget::File(output.clone()))?,
            OutputTarget::File(output)
        );
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
    #[test]
    fn resolve_output_target_preserves_symlink_leaf_and_protects_target()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("output_symlink_leaf")?;
        let src_file = root.join("lib.rs");
        fs::write(&src_file, "original content")?;
        let out_link = root.join("out.txt");

        #[cfg(unix)]
        let created = std::os::unix::fs::symlink(&src_file, &out_link).is_ok();
        #[cfg(windows)]
        let created = std::os::windows::fs::symlink_file(&src_file, &out_link).is_ok();

        if created {
            let resolved = resolve_file(out_link.clone())?;
            let canonical_src = fs::canonicalize(&src_file)?;
            assert_ne!(
                resolved, canonical_src,
                "resolved output must not follow the leaf symlink"
            );
            assert_eq!(resolved, out_link);
        }
        Ok(())
    }

    #[test]
    fn temp_file_drop_cleans_up_on_scope_exit() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("temp_file_cleanup")?;
        let temp = TempFile::create(&root, "sample.txt")?;
        let temp_path = temp.path().to_path_buf();
        assert!(temp_path.exists());
        drop(temp);
        assert!(
            !temp_path.exists(),
            "temporary file must be removed when TempFile is dropped"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn atomic_file_create_cleans_up_temp_file_on_metadata_error()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        let root = temp_dir("create_metadata_error")?;
        let loop_a = root.join("loop_a");
        let loop_b = root.join("loop_b");
        symlink(&loop_b, &loop_a)?;
        symlink(&loop_a, &loop_b)?;

        let result = AtomicFile::create(&loop_a);
        assert!(
            result.is_err(),
            "AtomicFile::create on symlink loop must fail"
        );

        let temp_count = fs::read_dir(&root)?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".loop_a."))
            .count();
        assert_eq!(
            temp_count, 0,
            "temporary file must be unlinked when AtomicFile::create fails during metadata check"
        );
        Ok(())
    }
}
