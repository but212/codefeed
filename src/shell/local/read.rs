//! Reads one walk candidate: metadata, the size gate, and a bounded body.

use std::fs;
use std::io::{self, Read};
use std::path::Path;

use crate::core::path::NormalizedPath;
use crate::core::policy::MAX_FILE_SIZE;
use crate::core::selector::{Body, Observation, exceeds_file_size};
use crate::shell::error::{IngestError, io_error_at};

/// Reads one regular file's metadata and body. `Ok(None)` when the entry is a symlink or non-file
/// (no longer a regular file); `Err` aborts the run. A file unavailable at metadata, open, or read
/// (`is_unavailable_file`) is [`Body::Unavailable`].
pub(super) fn read_candidate(
    entry_path: &Path,
    path: NormalizedPath,
) -> Result<Option<Observation<Vec<u8>>>, IngestError> {
    let meta = match fs::symlink_metadata(entry_path) {
        Ok(m) => m,
        // Unlinked or locked after discovery: the selector counts it as skipped.
        Err(err) if is_unavailable_file(&err) => {
            return Ok(Some(Observation::File(path, Body::Unavailable)));
        }
        Err(err) => return Err(io_error_at(entry_path, &err)),
    };

    if meta.is_symlink() || !meta.is_file() {
        return Ok(None);
    }

    let file_len = meta.len();
    if exceeds_file_size(file_len) {
        return Ok(Some(Observation::File(path, Body::OverLimit)));
    }
    let body = read_bounded(entry_path, file_len)?.map_or(Body::Unavailable, Body::Bytes);
    Ok(Some(Observation::File(path, body)))
}

/// Errors that make one file unavailable without invalidating the walk. Applied at metadata, open,
/// and read; any other error fails the run.
fn is_unavailable_file(err: &io::Error) -> bool {
    const ERROR_SHARING_VIOLATION: i32 = 32;
    const ERROR_LOCK_VIOLATION: i32 = 33;
    matches!(
        err.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
    ) || (cfg!(windows)
        && matches!(
            err.raw_os_error(),
            Some(ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION)
        ))
}

/// Reads at most `MAX_FILE_SIZE + 1` bytes; the extra byte lets the selector detect overflow.
/// Returns `Ok(None)` when the file is unavailable at open or read (`is_unavailable_file`).
/// `file_len` is the metadata length, which the caller has already checked against `MAX_FILE_SIZE`.
///
/// # Panics
///
/// Panics if `file_len` does not fit in `usize`, which the caller's size check rules out.
fn read_bounded(path: &Path, file_len: u64) -> Result<Option<Vec<u8>>, IngestError> {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(err) if is_unavailable_file(&err) => return Ok(None),
        Err(err) => return Err(io_error_at(path, &err)),
    };
    let capacity =
        usize::try_from(file_len).expect("invariant: caller checked file_len <= MAX_FILE_SIZE");
    let mut buf = Vec::with_capacity(capacity);
    match file.take(MAX_FILE_SIZE + 1).read_to_end(&mut buf) {
        Ok(_) => Ok(Some(buf)),
        Err(err) if is_unavailable_file(&err) => Ok(None),
        Err(err) => Err(io_error_at(path, &err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::policy::MAX_FILE_SIZE_USIZE;
    use crate::core::selector::Selector;
    #[cfg(unix)]
    use crate::shell::testing::run_with_timeout;
    use crate::shell::testing::temp_dir;

    #[test]
    fn read_bounded_caps_and_reports_unavailable() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("read_bounded")?;
        let exact = root.join("exact.txt");
        let over = root.join("over.txt");
        let kib = root.join("kib.txt");
        let missing = root.join("missing.txt");
        let kib_payload = vec![b'y'; 64 * 1024];
        fs::write(&exact, vec![b'x'; MAX_FILE_SIZE_USIZE])?;
        fs::write(&over, vec![b'x'; MAX_FILE_SIZE_USIZE + 1])?;
        fs::write(&kib, &kib_payload)?;

        let cases = [
            (
                &exact,
                MAX_FILE_SIZE,
                Some(MAX_FILE_SIZE_USIZE),
                "exact limit",
            ),
            (
                &over,
                MAX_FILE_SIZE + 1,
                Some(MAX_FILE_SIZE_USIZE + 1),
                "one byte over",
            ),
            (
                &kib,
                kib_payload.len() as u64,
                Some(kib_payload.len()),
                "64 KiB",
            ),
            (&missing, 0, None, "unlinked before open"),
        ];
        for (path, file_len, expected_len, label) in cases {
            let read = read_bounded(path, file_len)?;
            assert_eq!(read.as_ref().map(Vec::len), expected_len, "{label}");
        }
        assert_eq!(
            read_bounded(&kib, kib_payload.len() as u64)?,
            Some(kib_payload)
        );
        Ok(())
    }

    #[test]
    fn unlinked_before_metadata_is_counted_as_skipped() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("unlinked_after_discovery")?;
        let file = root.join("a.txt");
        fs::write(&file, "content")?;
        fs::remove_file(&file)?;

        let observation = read_candidate(&file, NormalizedPath::from("a.txt"))?
            .ok_or("unlinked file must reach the selector as a candidate")?;
        assert!(
            matches!(&observation, Observation::File(_, Body::Unavailable)),
            "unlinked file must be an unavailable candidate"
        );
        let selector = Selector::new().observe(observation)?;

        let result = selector.finish("test".to_string(), None)?;
        assert_eq!(result.files.len(), 0);
        assert_eq!(result.skipped, 1);
        Ok(())
    }

    #[test]
    fn oversized_file_is_over_limit_without_a_read() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("oversized_over_limit")?;
        let file = root.join("large.txt");
        fs::write(&file, vec![b'x'; MAX_FILE_SIZE_USIZE + 1])?;

        let observation = read_candidate(&file, NormalizedPath::from("large.txt"))?
            .ok_or("oversized file must reach the selector as a candidate")?;
        assert!(
            matches!(&observation, Observation::File(_, Body::OverLimit)),
            "oversized file must be over the limit and carry no read"
        );
        let selector = Selector::new().observe(observation)?;

        let result = selector.finish("test".to_string(), None)?;
        assert_eq!(result.files.len(), 0);
        assert_eq!(result.skipped, 1);
        Ok(())
    }

    #[test]
    fn unavailable_errors_skip_others_fail() {
        // Windows delete-pending and lock-violation codes are unavailable; other errors are fatal.
        let cases = [
            (io::Error::from(io::ErrorKind::NotFound), true),
            (io::Error::from(io::ErrorKind::PermissionDenied), true),
            (io::Error::from_raw_os_error(32), cfg!(windows)),
            (io::Error::from_raw_os_error(33), cfg!(windows)),
            (io::Error::other("disk failure"), false),
        ];
        for (err, expected) in cases {
            assert_eq!(is_unavailable_file(&err), expected, "{err:?}");
        }
    }

    #[test]
    fn read_candidate_skips_directory_entry() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("read_candidate_dir")?;
        let sub = root.join("subdir");
        fs::create_dir(&sub)?;

        let result = read_candidate(&sub, NormalizedPath::from("subdir"))?;
        assert!(result.is_none());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn read_candidate_skips_fifo_without_opening() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("read_candidate_fifo")?;
        let fifo = root.join("pipe");
        crate::shell::testing::make_fifo(&fifo)?;

        // Opening a FIFO with no writer blocks, so an `open` regression surfaces as a timeout.
        let msg = run_with_timeout(move || read_candidate(&fifo, NormalizedPath::from("pipe")));
        assert!(msg?.is_none(), "a FIFO is not a candidate");
        Ok(())
    }
}
