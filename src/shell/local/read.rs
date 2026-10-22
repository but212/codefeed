//! Reads one walk candidate: metadata, the size gate, and a bounded body.

use std::fs;
use std::io::{self, Read};
use std::path::Path;

use crate::core::policy::{MAX_FILE_SIZE, NormalizedPath};
use crate::core::selector::exceeds_size_limits;
use crate::shell::IngestError;
use crate::shell::error::io_error_at;

/// What a worker learned about one regular-file candidate.
pub(super) enum Body {
    /// Unavailable at metadata, open, or read (`is_unavailable_file`).
    Unavailable,
    /// Path or declared length over the size limits; not read. Carries the declared length.
    OverLimit(u64),
    /// Bounded read of at most `MAX_FILE_SIZE + 1` bytes.
    Read(Vec<u8>),
}

/// One walk result: a non-UTF-8 path prefix, or a regular file with its outcome.
pub(super) enum Candidate {
    NonUtf8(String),
    File(NormalizedPath, Body),
}

/// Worker reply for one job: a candidate, `None` when the entry is no longer a regular file, or the
/// error that aborts the run.
pub(super) type WorkerMsg = Result<Option<Candidate>, IngestError>;

/// Reads one regular file's metadata and body. `Ok(None)` when the entry is a symlink or non-file.
pub(super) fn read_candidate(entry_path: &Path, path: NormalizedPath) -> WorkerMsg {
    let meta = match fs::symlink_metadata(entry_path) {
        Ok(m) => m,
        // Unlinked or locked after discovery: the selector counts it as skipped.
        Err(err) if is_unavailable_file(&err) => {
            return Ok(Some(Candidate::File(path, Body::Unavailable)));
        }
        Err(err) => return Err(io_error_at(entry_path, err)),
    };

    if meta.is_symlink() || !meta.is_file() {
        return Ok(None);
    }

    let file_len = meta.len();
    if exceeds_size_limits(path.len(), Some(file_len)) {
        return Ok(Some(Candidate::File(path, Body::OverLimit(file_len))));
    }
    let body = match read_bounded(entry_path, file_len)? {
        Some(bytes) => Body::Read(bytes),
        None => Body::Unavailable,
    };
    Ok(Some(Candidate::File(path, body)))
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
fn read_bounded(path: &Path, file_len: u64) -> Result<Option<Vec<u8>>, IngestError> {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(err) if is_unavailable_file(&err) => return Ok(None),
        Err(err) => return Err(io_error_at(path, err)),
    };
    let mut buf = Vec::with_capacity(file_len.min(MAX_FILE_SIZE + 1) as usize);
    match file.take(MAX_FILE_SIZE + 1).read_to_end(&mut buf) {
        Ok(_) => Ok(Some(buf)),
        Err(err) if is_unavailable_file(&err) => Ok(None),
        Err(err) => Err(io_error_at(path, err)),
    }
}

#[cfg(test)]
mod tests {
    use super::super::observe_candidate;
    use super::*;
    use crate::core::selector::Selector;
    use crate::shell::testing::temp_dir;

    #[test]
    fn read_bounded_caps_and_reports_unavailable() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("read_bounded")?;
        let exact = root.join("exact.txt");
        let over = root.join("over.txt");
        let kib = root.join("kib.txt");
        let missing = root.join("missing.txt");
        let kib_payload = vec![b'y'; 64 * 1024];
        fs::write(&exact, vec![b'x'; MAX_FILE_SIZE as usize])?;
        fs::write(&over, vec![b'x'; MAX_FILE_SIZE as usize + 1])?;
        fs::write(&kib, &kib_payload)?;

        let cases = [
            (
                &exact,
                MAX_FILE_SIZE,
                Some(MAX_FILE_SIZE as usize),
                "exact limit",
            ),
            (
                &over,
                MAX_FILE_SIZE + 1,
                Some(MAX_FILE_SIZE as usize + 1),
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

        let candidate = read_candidate(&file, NormalizedPath::from("a.txt"))?
            .ok_or("unlinked file must reach the selector as a candidate")?;
        assert!(
            matches!(&candidate, Candidate::File(_, Body::Unavailable)),
            "unlinked file must be an unavailable candidate"
        );
        let selector = observe_candidate(Selector::new(), candidate)?;

        let result = selector.finish("test".to_string(), None);
        assert_eq!(result.files.len(), 0);
        assert_eq!(result.skipped, 1);
        Ok(())
    }

    #[test]
    fn oversized_file_carries_size_hint() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("oversized_size_hint")?;
        let file = root.join("large.txt");
        fs::write(&file, vec![b'x'; MAX_FILE_SIZE as usize + 1])?;

        let candidate = read_candidate(&file, NormalizedPath::from("large.txt"))?
            .ok_or("oversized file must reach the selector as a candidate")?;
        assert!(
            matches!(
                &candidate,
                Candidate::File(_, Body::OverLimit(len)) if *len == MAX_FILE_SIZE + 1
            ),
            "oversized file must carry its declared size and no read"
        );
        let selector = observe_candidate(Selector::new(), candidate)?;

        let result = selector.finish("test".to_string(), None);
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
}
