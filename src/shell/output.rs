//! Output delivery: atomic filesystem writing and stdout streaming.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::core::error::IngestError;
use crate::core::model::{CollectionResult, DigestFormat, OutputTarget};
use crate::core::render::render_to_writer;

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[inline]
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

/// Resolves an [`OutputTarget`] into an optional canonical file destination.
///
/// Returns `None` when writing directly to [`OutputTarget::Stdout`].
pub(super) fn resolve_output_target(target: &OutputTarget) -> Result<Option<PathBuf>, IngestError> {
    match target {
        OutputTarget::Stdout => Ok(None),
        OutputTarget::File(path) => {
            let file_name = path
                .file_name()
                .ok_or_else(|| IngestError::usage("output path has no filename"))?;
            if path.is_dir() {
                return Err(IngestError::Io(io::Error::new(
                    io::ErrorKind::IsADirectory,
                    "output path is a directory",
                )));
            }
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
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
            Ok(Some(resolved))
        }
    }
}

/// Delivers the rendered digest to its target. A file target is written
/// atomically; the stdout target buffers into memory up to [`MAX_OUTPUT_SIZE`]
/// and writes to the stream only on success, preventing partial output on overflow.
pub(super) fn emit(
    resolved_target: Option<&Path>,
    collection: &CollectionResult,
    format: DigestFormat,
) -> Result<usize, IngestError> {
    match resolved_target {
        Some(path) => write_atomic(path, |writer| render_to_writer(writer, collection, format)),
        None => {
            let stdout = io::stdout();
            render_stdout_to(&mut stdout.lock(), collection, format)
        }
    }
}

fn render_stdout_to<W: Write>(
    out: &mut W,
    collection: &CollectionResult,
    format: DigestFormat,
) -> Result<usize, IngestError> {
    let mut buffer = Vec::new();
    let tokens = render_to_writer(&mut buffer, collection, format)?;
    ignore_broken_pipe(out.write_all(&buffer).and_then(|()| out.flush()))
        .map_err(IngestError::Io)?;
    Ok(tokens)
}

const BUF_WRITER_CAPACITY: usize = 64 * 1024;

struct TempPathGuard {
    path: PathBuf,
    active: bool,
}

impl TempPathGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, active: true }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn disarm(&mut self) {
        self.active = false;
    }
}

impl Drop for TempPathGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = fs::remove_file(&self.path);
        }
    }
}

struct AtomicFile {
    writer: io::BufWriter<fs::File>,
    temp_guard: TempPathGuard,
    target: PathBuf,
}

impl AtomicFile {
    fn create(target: &Path) -> Result<Self, IngestError> {
        let file_name = target
            .file_name()
            .ok_or_else(|| IngestError::usage("output path has no filename"))?;
        let file_name = file_name.to_string_lossy();
        let parent = target
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let timestamp = current_epoch_nanos();
        let (temporary, file) = loop {
            let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let temporary = parent.join(format!(".{file_name}.{timestamp}.{counter}.tmp"));
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
            {
                Ok(file) => break (temporary, file),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(IngestError::Io(error)),
            }
        };

        Ok(Self {
            writer: io::BufWriter::with_capacity(BUF_WRITER_CAPACITY, file),
            temp_guard: TempPathGuard::new(temporary),
            target: target.to_path_buf(),
        })
    }

    fn writer(&mut self) -> &mut io::BufWriter<fs::File> {
        &mut self.writer
    }

    fn commit(mut self) -> Result<(), IngestError> {
        let file = self
            .writer
            .into_inner()
            .map_err(|e| IngestError::Io(e.into_error()))?;
        file.sync_all()?;
        drop(file);

        fs::rename(self.temp_guard.path(), &self.target)?;
        self.temp_guard.disarm();
        Ok(())
    }
}

fn write_atomic<R, F>(output: &Path, write_fn: F) -> Result<R, IngestError>
where
    F: FnOnce(&mut io::BufWriter<fs::File>) -> Result<R, IngestError>,
{
    let mut file = AtomicFile::create(output)?;
    let value = write_fn(file.writer())?;
    file.commit()?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::super::testing::temp_dir;
    use super::*;

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
        use crate::core::policy::MAX_OUTPUT_SIZE;
        use crate::core::storage::FileStorage;

        let mut storage = FileStorage::new();
        storage.push("a.txt", "hello\n");
        let large_content = "x".repeat(MAX_OUTPUT_SIZE as usize - 16);
        storage.push("b.txt", &large_content);

        let collection = CollectionResult {
            label: "overflow-repo".into(),
            revision: None,
            files: crate::core::storage::SortedFiles::new(storage),
            skipped: 0,
        };

        let mut sink = Vec::new();
        let res = render_stdout_to(&mut sink, &collection, DigestFormat::Text);
        assert!(matches!(res, Err(IngestError::OutputTooLarge(_))));
        assert!(sink.is_empty(), "stdout sink must remain empty on overflow");
        Ok(())
    }

    #[test]
    fn atomic_write_ignores_stale_temporary_file() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("atomic")?;
        let output = root.join("digest.txt");
        fs::write(root.join(".digest.txt.123.tmp"), "stale")?;
        write_atomic(&output, |w| {
            w.write_all(b"new")?;
            Ok(())
        })?;
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
        let result = write_atomic(&root, |w| {
            w.write_all(b"new")?;
            Ok(())
        });
        assert!(result.is_err());
        assert!(root.is_dir());
        Ok(())
    }

    #[test]
    fn atomic_write_cleans_up_temporary_file_on_error_and_panic()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("atomic_cleanup")?;
        let output = root.join("digest.txt");

        // 1. Cleanup on closure error
        let err_result: Result<(), _> = write_atomic(&output, |_w| {
            Err(IngestError::invalid_metadata("test failure"))
        });
        assert!(err_result.is_err());
        assert!(!output.exists());
        let temp_count_after_err = fs::read_dir(&root)?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".digest.txt."))
            .count();
        assert_eq!(
            temp_count_after_err, 0,
            "temporary file should be cleaned up on error"
        );

        // 2. Cleanup on panic
        let panic_result = std::panic::catch_unwind(|| {
            let _ = write_atomic::<(), _>(&output, |_w| {
                panic!("intentional panic during streaming write");
            });
        });
        assert!(panic_result.is_err(), "closure should have panicked");
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
        let result = write_atomic(Path::new("."), |_w| Ok(()));
        match result {
            Err(IngestError::Usage(msg)) => {
                assert!(msg.contains("output path has no filename"));
            }
            other => panic!("expected IngestError::Usage, got {other:?}"),
        }
    }

    #[test]
    fn rejects_directory_target_immediately() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("output_dir_test")?;
        let existing_dir = root.join("existing_subdir");
        fs::create_dir(&existing_dir)?;

        let result = write_atomic(&existing_dir, |_w| Ok(()));
        match result {
            Err(IngestError::Io(err)) => {
                assert!(
                    matches!(
                        err.kind(),
                        io::ErrorKind::IsADirectory | io::ErrorKind::PermissionDenied
                    ),
                    "expected IsADirectory or PermissionDenied, got {:?}",
                    err.kind()
                );
            }
            other => panic!("expected IngestError::Io, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn resolve_output_target_handles_stdout_and_file() -> Result<(), Box<dyn std::error::Error>> {
        let stdout_target = resolve_output_target(&OutputTarget::Stdout)?;
        assert!(stdout_target.is_none());

        let root = temp_dir("output_target_test")?;
        let output = root.join("digest.txt");
        let file_target = resolve_output_target(&OutputTarget::File(output))?;
        assert!(file_target.is_some());
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
        let resolved = resolve_output_target(&OutputTarget::File(file_b))?.unwrap();
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

        let resolved = resolve_output_target(&OutputTarget::File(output))?.unwrap();
        assert_eq!(resolved, canonical_output);

        let alias_output = root.join("subdir").join("..").join("digest.txt");
        let alias_resolved = resolve_output_target(&OutputTarget::File(alias_output))?.unwrap();
        assert_eq!(alias_resolved, canonical_output);

        #[cfg(any(windows, target_os = "macos"))]
        {
            let cased_output = root.join("DIGEST.TXT");
            let cased_resolved = resolve_output_target(&OutputTarget::File(cased_output))?.unwrap();
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

        let resolved = resolve_output_target(&OutputTarget::File(output_via_link))?.unwrap();
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
            let resolved = resolve_output_target(&OutputTarget::File(out_link.clone()))?.unwrap();
            let canonical_src = fs::canonicalize(&src_file)?;
            assert_ne!(
                resolved, canonical_src,
                "resolved output must not follow the leaf symlink"
            );
            assert_eq!(resolved, out_link);
        }
        Ok(())
    }
}
