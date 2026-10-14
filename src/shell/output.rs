use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::core::error::IngestError;
use crate::core::model::OutputTarget;

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Delivers the rendered digest to its target. A file target is written
/// atomically; the stdout target writes the buffer to the stream directly,
/// bypassing temporary-file creation and filesystem sync.
///
/// A broken pipe on the stdout target is a normal early close by the consumer
/// (`codefeed . --output - | head -1`), not an I/O failure: the consumer took
/// what it asked for, and stdout emission leaves no partial state behind.
pub(crate) fn emit<R, F>(target: &OutputTarget, write_fn: F) -> Result<R, IngestError>
where
    R: Default,
    F: FnOnce(&mut dyn io::Write) -> Result<R, IngestError>,
{
    match target {
        OutputTarget::File(path) => write_atomic(path, write_fn),
        OutputTarget::Stdout => write_stdout(write_fn),
    }
}

const BUF_WRITER_CAPACITY: usize = 64 * 1024;

fn write_stdout<R, F>(write_fn: F) -> Result<R, IngestError>
where
    R: Default,
    F: FnOnce(&mut dyn io::Write) -> Result<R, IngestError>,
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

struct AtomicFile<'a> {
    target: &'a Path,
    temporary: Option<std::path::PathBuf>,
    writer: Option<io::BufWriter<fs::File>>,
}

impl<'a> AtomicFile<'a> {
    fn create(target: &'a Path) -> Result<Self, IngestError> {
        let parent = target.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let Some(raw_name) = target.file_name() else {
            return Err(IngestError::Usage("output path has no filename".into()));
        };
        let file_name = raw_name.to_string_lossy();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
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
            target,
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
        fs::rename(temp_path, self.target)?;
        self.temporary = None;
        Ok(())
    }
}

impl io::Write for AtomicFile<'_> {
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

impl Drop for AtomicFile<'_> {
    fn drop(&mut self) {
        drop(self.writer.take());
        if let Some(temp_path) = self.temporary.take() {
            let _ = fs::remove_file(temp_path);
        }
    }
}

fn write_atomic<R, F>(output: &Path, write_fn: F) -> Result<R, IngestError>
where
    F: FnOnce(&mut dyn io::Write) -> Result<R, IngestError>,
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
            Err(IngestError::InvalidMetadata("test failure".into()))
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
}
