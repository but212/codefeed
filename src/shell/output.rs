use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::core::error::IngestError;
use crate::core::model::{CollectionResult, DigestFormat, OutputTarget};

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) fn resolve_output_target(target: &OutputTarget) -> Result<Option<PathBuf>, IngestError> {
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
            if resolved.exists()
                && let Ok(canonical) = fs::canonicalize(&resolved)
            {
                resolved = canonical;
            }
            Ok(Some(resolved))
        }
    }
}

/// Delivers the rendered digest to its target. A file target is written
/// atomically; the stdout target writes the buffer to the stream directly,
/// bypassing temporary-file creation and filesystem sync.
pub(crate) fn emit(
    resolved_target: Option<&Path>,
    collection: &CollectionResult,
    format: DigestFormat,
) -> Result<usize, IngestError> {
    match resolved_target {
        Some(path) => write_atomic(path, |file| {
            crate::core::render::render_to_writer(file, collection, format)
        }),
        None => {
            write_stdout(|writer| crate::core::render::render_to_writer(writer, collection, format))
        }
    }
}

const BUF_WRITER_CAPACITY: usize = 64 * 1024;

fn write_stdout<R: Default, F>(write_fn: F) -> Result<R, IngestError>
where
    F: FnOnce(&mut io::BufWriter<io::StdoutLock<'_>>) -> Result<R, IngestError>,
{
    let stdout = io::stdout();
    let mut writer = io::BufWriter::with_capacity(BUF_WRITER_CAPACITY, stdout.lock());
    let result = write_fn(&mut writer);
    match result {
        Ok(val) => match writer.flush() {
            Ok(()) => Ok(val),
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(val),
            Err(error) => Err(IngestError::Io(error)),
        },
        Err(IngestError::Io(error)) if error.kind() == io::ErrorKind::BrokenPipe => {
            Ok(R::default())
        }
        Err(error) => Err(error),
    }
}

struct AtomicFile {
    target: PathBuf,
    temporary: Option<PathBuf>,
    writer: Option<io::BufWriter<fs::File>>,
}

impl AtomicFile {
    fn create(target: &Path) -> Result<Self, IngestError> {
        let file_name = target
            .file_name()
            .ok_or_else(|| IngestError::usage("output path has no filename"))?;
        if target.is_dir() {
            return Err(IngestError::Io(io::Error::new(
                io::ErrorKind::IsADirectory,
                "output path is a directory",
            )));
        }
        let file_name = file_name.to_string_lossy();
        let parent = target
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let timestamp = super::current_epoch_nanos();
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
            target: target.to_path_buf(),
            temporary: Some(temporary),
            writer: Some(io::BufWriter::with_capacity(BUF_WRITER_CAPACITY, file)),
        })
    }

    fn commit(mut self) -> Result<(), IngestError> {
        if let Some(writer) = self.writer.take() {
            let file = writer
                .into_inner()
                .map_err(|e| IngestError::Io(e.into_error()))?;
            file.sync_all()?;
            drop(file);
        }

        let temp_path = self.temporary.as_ref().expect("temporary path exists");
        fs::rename(temp_path, &self.target)?;
        self.temporary = None;
        Ok(())
    }
}

impl io::Write for AtomicFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match &mut self.writer {
            Some(w) => w.write(buf),
            None => Err(io::Error::other("file already committed")),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match &mut self.writer {
            Some(w) => w.flush(),
            None => Ok(()),
        }
    }
}

impl Drop for AtomicFile {
    fn drop(&mut self) {
        drop(self.writer.take());
        if let Some(temp_path) = self.temporary.take() {
            let _ = fs::remove_file(temp_path);
        }
    }
}

fn write_atomic<R, F>(output: &Path, write_fn: F) -> Result<R, IngestError>
where
    F: FnOnce(&mut AtomicFile) -> Result<R, IngestError>,
{
    let mut file = AtomicFile::create(output)?;
    let value = write_fn(&mut file)?;
    file.commit()?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::super::temp_dir;
    use super::*;

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
                assert_eq!(err.kind(), io::ErrorKind::IsADirectory);
            }
            other => panic!("expected IngestError::Io(IsADirectory), got {other:?}"),
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
}
