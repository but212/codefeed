use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

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
        let Some(raw_name) = target.file_name() else {
            return Err(IngestError::Usage("output path has no filename".into()));
        };
        if target.is_dir() {
            return Err(IngestError::Io(io::Error::new(
                io::ErrorKind::IsADirectory,
                "output path is a directory",
            )));
        }
        let parent = target
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let normalized_parent = normalize_path(parent);
        fs::create_dir_all(&normalized_parent)?;
        let file_name = raw_name.to_string_lossy();
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

#[derive(Debug, Clone)]
pub(crate) struct OutputFilter {
    canonical: PathBuf,
    #[cfg(unix)]
    dev_ino: Option<(u64, u64)>,
}

impl OutputFilter {
    pub(crate) fn from_target(target: &OutputTarget) -> Result<Option<Self>, IngestError> {
        match target {
            OutputTarget::File(path) => Self::new(path),
            OutputTarget::Stdout => Ok(None),
        }
    }

    pub(crate) fn new(output: &Path) -> Result<Option<Self>, IngestError> {
        let canonical = match fs::canonicalize(output) {
            Ok(c) => c,
            Err(_) => {
                let absolute = if output.is_absolute() {
                    output.to_path_buf()
                } else {
                    env::current_dir()?.join(output)
                };
                let Ok(c) = fs::canonicalize(normalize_path(&absolute)) else {
                    // An output file that does not exist on disk prior to ingest cannot be visited during walk
                    return Ok(None);
                };
                c
            }
        };

        #[cfg(unix)]
        let dev_ino = fs::symlink_metadata(&canonical).ok().map(|m| {
            use std::os::unix::fs::MetadataExt;
            (m.dev(), m.ino())
        });

        Ok(Some(Self {
            canonical,
            #[cfg(unix)]
            dev_ino,
        }))
    }

    pub(crate) fn is_excluded(&self, candidate: &Path) -> bool {
        if candidate == self.canonical {
            return true;
        }

        #[cfg(unix)]
        if let (Some(target_id), Ok(meta)) = (self.dev_ino, fs::symlink_metadata(candidate)) {
            use std::os::unix::fs::MetadataExt;
            return (meta.dev(), meta.ino()) == target_id;
        }

        #[cfg(not(unix))]
        {
            let Some(cand_name) = candidate.file_name() else {
                return false;
            };
            let Some(target_name) = self.canonical.file_name() else {
                return false;
            };

            if !names_match(cand_name, target_name) {
                return false;
            }

            fs::canonicalize(candidate).is_ok_and(|c| c == self.canonical)
        }

        #[cfg(unix)]
        false
    }
}

#[cfg(not(unix))]
#[inline]
fn names_match(candidate: &std::ffi::OsStr, target: &std::ffi::OsStr) -> bool {
    #[cfg(any(windows, target_os = "macos"))]
    {
        if candidate.is_ascii() && target.is_ascii() {
            candidate
                .to_string_lossy()
                .eq_ignore_ascii_case(&target.to_string_lossy())
        } else {
            candidate
                .to_string_lossy()
                .chars()
                .flat_map(char::to_lowercase)
                .eq(target
                    .to_string_lossy()
                    .chars()
                    .flat_map(char::to_lowercase))
        }
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        candidate == target
    }
}

pub(crate) fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
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
    fn normalizes_parent_and_current_dir_components() {
        let path = Path::new("/a/b/../c/./d");
        let normalized = normalize_path(path);
        assert_eq!(normalized, Path::new("/a/c/d"));
    }

    #[test]
    fn nonexistent_output_yields_none() -> Result<(), Box<dyn std::error::Error>> {
        let filter = OutputFilter::new(Path::new("nonexistent_output_definitely_missing.txt"))?;
        assert!(filter.is_none());
        Ok(())
    }

    #[test]
    fn output_filter_from_target_handles_stdout_and_file() -> Result<(), Box<dyn std::error::Error>>
    {
        let filter = OutputFilter::from_target(&OutputTarget::Stdout)?;
        assert!(filter.is_none());

        let missing = OutputFilter::from_target(&OutputTarget::File(PathBuf::from(
            "nonexistent_output_definitely_missing.txt",
        )))?;
        assert!(missing.is_none());
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
        let filter = OutputFilter::new(&file_a)?.expect("filter exists");
        let same = filter.is_excluded(&file_b);
        assert!(same);
        Ok(())
    }

    #[test]
    fn filter_skips_different_filename() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("output_filter_diff")?;
        let output = root.join("digest.txt");
        fs::write(&output, "output")?;
        let filter = OutputFilter::new(&output)?.expect("filter exists");
        assert!(!filter.is_excluded(&root.join("src/main.rs")));
        assert!(!filter.is_excluded(&root.join("digest_other.txt")));
        Ok(())
    }

    #[test]
    fn filter_identifies_output_file() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("output_filter_test")?;
        fs::create_dir_all(root.join("subdir"))?;
        let output = root.join("digest.txt");
        fs::write(&output, "output")?;

        let filter = OutputFilter::new(&output)?.expect("filter exists");
        assert!(filter.is_excluded(&output));
        assert!(filter.is_excluded(&root.join("subdir").join("..").join("digest.txt")));

        #[cfg(any(windows, target_os = "macos"))]
        assert!(filter.is_excluded(&root.join("DIGEST.TXT")));

        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn excludes_hard_link_to_output_file() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("output_hard_link")?;
        let output = root.join("digest.txt");
        let hardlink = root.join("notes.txt");
        fs::write(&output, "output content")?;
        fs::hard_link(&output, &hardlink)?;

        let filter = OutputFilter::new(&output)?.expect("filter exists");
        assert!(filter.is_excluded(&hardlink));

        Ok(())
    }
}
