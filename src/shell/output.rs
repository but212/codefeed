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
/// (`rustingest . --output - | head -1`), not an I/O failure: the consumer took
/// what it asked for, and stdout emission leaves no partial state behind.
pub(crate) fn emit<F>(target: &OutputTarget, write_fn: F) -> Result<(), IngestError>
where
    F: FnOnce(&mut dyn io::Write) -> Result<(), IngestError>,
{
    match target {
        OutputTarget::File(path) => write_atomic(path, write_fn),
        OutputTarget::Stdout => write_stdout(write_fn),
    }
}

fn write_stdout<F>(write_fn: F) -> Result<(), IngestError>
where
    F: FnOnce(&mut dyn io::Write) -> Result<(), IngestError>,
{
    let stdout = io::stdout();
    let mut writer = io::BufWriter::new(stdout.lock());
    match write_fn(&mut writer).and_then(|()| writer.flush().map_err(IngestError::Io)) {
        Ok(()) => Ok(()),
        Err(IngestError::Io(error)) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(error),
    }
}

fn write_atomic<F>(output: &Path, write_fn: F) -> Result<(), IngestError>
where
    F: FnOnce(&mut dyn io::Write) -> Result<(), IngestError>,
{
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let file_name = output
        .file_name()
        .ok_or_else(|| IngestError::InvalidMetadata("output path has no filename".into()))?
        .to_string_lossy();
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
    let result = (|| -> Result<(), IngestError> {
        let mut writer = io::BufWriter::new(file);
        write_fn(&mut writer)?;
        let file = writer
            .into_inner()
            .map_err(|e| IngestError::Io(e.into_error()))?;
        file.sync_all().map_err(IngestError::Io)?;
        fs::rename(&temporary, output).map_err(IngestError::Io)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::super::temp_dir;
    use super::*;

    #[test]
    fn atomic_write_ignores_stale_temporary_file() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("atomic")?;
        fs::create_dir_all(&root)?;
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
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn preserves_previous_output_on_atomic_failure() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("atomic_failure")?;
        fs::create_dir_all(&root)?;
        let result = write_atomic(&root, |w| {
            w.write_all(b"new")?;
            Ok(())
        });
        assert!(result.is_err());
        assert!(root.is_dir());
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn streaming_atomic_write_cleans_up_on_closure_error() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = temp_dir("atomic_closure_failure")?;
        fs::create_dir_all(&root)?;
        let output = root.join("digest.txt");
        let result = write_atomic(&output, |_w| {
            Err(IngestError::InvalidMetadata("test failure".into()))
        });
        assert!(result.is_err());
        assert!(!output.exists());
        let temp_files = fs::read_dir(&root)?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".digest.txt."))
            .count();
        assert_eq!(
            temp_files, 0,
            "temporary file should be cleaned up on error"
        );
        fs::remove_dir_all(root)?;
        Ok(())
    }
}
