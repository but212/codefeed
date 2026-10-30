//! Atomic file replacement: the digest is written to a temporary file, synced, and renamed over the target.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::IngestError;
use super::error::io_error_at;

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Returns the elapsed duration since the Unix epoch in nanoseconds.
fn current_epoch_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos())
}

/// Parent directory of `path`, or `.` when the path is a bare filename.
pub(super) fn parent_dir(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// Path of a temporary file, unlinked on drop unless persisted. It holds no handle. The file it was
/// created with is declared after this guard, so it drops, and closes, first.
struct TempPath(Option<PathBuf>);

impl TempPath {
    /// Creates `.codefeed.{nanos}.{counter}.tmp` in `parent`. The name does not depend on the target,
    /// so its length stays within the filename limit for any output name.
    fn create(parent: &Path) -> Result<(Self, fs::File), IngestError> {
        let timestamp = current_epoch_nanos();
        loop {
            let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(".codefeed.{timestamp}.{counter}.tmp"));
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => return Ok((Self(Some(path)), file)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    // Try again with a new counter.
                }
                Err(error) => return Err(io_error_at(&path, &error)),
            }
        }
    }

    fn path(&self) -> &Path {
        self.0.as_deref().expect("path is present until persist")
    }

    /// Renames the file over `target`. If the rename fails, the file stays and is unlinked on drop.
    fn persist(mut self, target: &Path) -> Result<(), IngestError> {
        fs::rename(self.path(), target).map_err(|err| io_error_at(target, &err))?;
        self.0 = None;
        Ok(())
    }
}

impl Drop for TempPath {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}

/// Warning text for a failed parent-directory sync, or `None` when the sync succeeded.
#[cfg(unix)]
fn sync_warning(parent: &Path, synced: io::Result<()>) -> Option<String> {
    synced.err().map(|err| {
        crate::core::error::sanitize_error_detail(&format!("warning: {}: {err}", parent.display()))
    })
}

/// Replaces `output` with `bytes`, so a failed write leaves the previous file in place.
///
/// # Errors
///
/// Returns [`IngestError::Io`] if creating, writing, syncing, or renaming fails. A parent-directory
/// sync failure after the rename is a warning on stderr, not an error, because the target already holds
/// the new digest and the run has succeeded.
pub(super) fn write_atomic(output: &Path, bytes: &[u8]) -> Result<(), IngestError> {
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
            super::output::write_stderr_line(&warning);
        }
    }

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

    #[test]
    fn preserves_previous_output_on_atomic_failure() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("atomic_failure")?;
        let result = write_atomic(&root, b"new");
        assert!(result.is_err());
        assert!(root.is_dir());
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
