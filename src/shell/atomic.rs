//! Atomic file replacement: the digest is written to a temporary file, synced, and renamed over the target.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::IngestError;
use super::error::io_error_at;
use crate::core::error::DomainError;

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Returns the elapsed duration since the Unix epoch in nanoseconds.
fn current_epoch_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// Parent directory of `path`, or `.` when the path is a bare filename.
pub(super) fn parent_dir(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// Lifecycle of a [`TempFile`]. Only `Writing` holds an open handle, so writes are
/// unrepresentable once the file is closed or committed.
enum TempState {
    Writing(fs::File),
    Closed,
    Committed,
}

/// RAII guard managing a temporary file that cleans up on drop unless committed.
///
/// The handle is closed before the file is unlinked, which keeps deletion Windows-safe
/// (unlinking an open file fails there).
struct TempFile {
    path: PathBuf,
    state: TempState,
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
                        state: TempState::Writing(file),
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(io_error_at(&path, error)),
            }
        }
    }

    #[cfg(test)]
    fn path(&self) -> &Path {
        &self.path
    }

    fn file_mut(&mut self) -> io::Result<&mut fs::File> {
        match &mut self.state {
            TempState::Writing(file) => Ok(file),
            TempState::Closed | TempState::Committed => Err(closed_error()),
        }
    }

    #[cfg(unix)]
    fn set_permissions(&self, permissions: fs::Permissions) -> io::Result<()> {
        match &self.state {
            TempState::Writing(file) => file.set_permissions(permissions),
            TempState::Closed | TempState::Committed => Err(closed_error()),
        }
    }

    fn commit(mut self, target: &Path) -> Result<(), IngestError> {
        let TempState::Writing(file) = mem::replace(&mut self.state, TempState::Closed) else {
            return Err(IngestError::Io(closed_error()));
        };
        // A failure below leaves `Closed` in place, so `Drop` still unlinks the temporary file.
        file.sync_all()
            .map_err(|err| io_error_at(&self.path, err))?;
        drop(file);

        fs::rename(&self.path, target).map_err(|err| io_error_at(target, err))?;
        self.state = TempState::Committed;
        Ok(())
    }
}

fn closed_error() -> io::Error {
    io::Error::other("temporary file is closed")
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let committed = matches!(self.state, TempState::Committed);
        // Assigning drops the previous state, closing any open handle before the unlink below.
        self.state = TempState::Closed;
        if !committed {
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
            Err(error) => return Err(io_error_at(target, error)),
        }

        Ok(Self {
            temp_file,
            target: target.to_path_buf(),
        })
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<(), IngestError> {
        let file = self
            .temp_file
            .file_mut()
            .map_err(|err| io_error_at(&self.target, err))?;
        file.write_all(bytes)
            .map_err(|err| io_error_at(&self.target, err))
    }

    /// Commits the atomic file write by syncing content, renaming to the target path,
    /// and syncing the parent directory on Unix.
    ///
    /// # Errors
    ///
    /// Returns [`IngestError::Io`] if flushing or renaming fails. A parent-directory sync failure
    /// after the rename is a warning on stderr, not an error, because the target already holds the
    /// new digest and the run has succeeded.
    fn commit(self) -> Result<(), IngestError> {
        self.temp_file.commit(&self.target)?;

        #[cfg(unix)]
        {
            let parent = parent_dir(&self.target);
            let synced = fs::File::open(parent).and_then(|dir| dir.sync_all());
            if let Some(warning) = sync_warning(parent, synced) {
                super::output::write_stderr_line(&warning);
            }
        }

        Ok(())
    }
}

/// Warning text for a failed parent-directory sync, or `None` when the sync succeeded.
#[cfg(unix)]
fn sync_warning(parent: &Path, synced: io::Result<()>) -> Option<String> {
    synced
        .err()
        .map(|err| format!("warning: {}: {err}", parent.display()))
}

/// Replaces `output` with `bytes`, so a failed write leaves the previous file in place.
pub(super) fn write_atomic(output: &Path, bytes: &[u8]) -> Result<(), IngestError> {
    let mut file = AtomicFile::create(output)?;
    file.write_all(bytes)?;
    file.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::testing::temp_dir;
    use super::*;

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
    fn atomic_write_error_names_path() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("error_names_path")?;
        let output = root.join("missing").join("out.txt");

        let err = match write_atomic(&output, b"new") {
            Err(err) => err,
            Ok(()) => return Err("write into a missing directory must fail".into()),
        };
        assert!(
            err.to_string().contains("missing"),
            "unexpected message: {err}"
        );
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
