//! Single source of truth for ingestion policies, limits, and conventions.

/// Maximum individual file size accepted (1 MiB in bytes). Larger files are skipped.
pub(crate) const MAX_FILE_SIZE: u64 = 1024 * 1024;
/// Maximum rendered digest size (20 MiB in bytes). Overflows return [`IngestError::OutputTooLarge`](crate::core::error::IngestError::OutputTooLarge).
pub(crate) const MAX_OUTPUT_SIZE: u64 = 20 * 1024 * 1024;
/// Maximum candidate files evaluated before candidate budget overflows (10,000 files).
pub(crate) const MAX_FILE_COUNT: usize = 10_000;
/// Maximum duration in seconds allowed for whole-archive Tarball transfer (5 minutes).
pub(crate) const ARCHIVE_MAX_SECS: u64 = 300;

/// Conventional lockfiles excluded before candidate budget accounting.
const NOISE: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "go.sum",
];

/// Known sensitive file extensions (case-insensitive) excluded prior to reading or download.
///
/// Note: `credentials` is matched as an extension here (`*.credentials`, e.g., `aws.credentials`),
/// and is also matched as a filename prefix in [`is_sensitive_file`] (`credentials*`, e.g., `credentials.json`).
const SENSITIVE_EXTENSIONS: &[&str] = &["pem", "key", "p12", "pkcs12", "credentials"];

/// Maximum total HTTP request attempts (1 initial + up to 2 retries).
pub(crate) const MAX_RETRIES: usize = 3;
/// Base initial delay in milliseconds for exponential backoff.
const RETRY_INITIAL_DELAY_MS: u64 = 250;
/// Maximum delay cap in milliseconds for exponential backoff.
const RETRY_MAX_DELAY_MS: u64 = 5_000;

/// Path classification outcome from a single-pass policy evaluation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PathStatus {
    /// Normalized valid relative UTF-8 path.
    Valid,
    /// Ignored path (hidden component, conventional noise, sensitive pattern, or root).
    Ignored,
    /// Non-UTF-8 path that is not hidden and must be counted against candidate budget as skipped.
    NonUtf8Candidate,
}

/// Returns `true` if `file_name` matches a known noise file.
#[inline]
fn is_noise_file(file_name: &str) -> bool {
    NOISE.contains(&file_name)
}

fn starts_with_ignore_ascii_case(value: &str, prefix: &str) -> bool {
    value
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

/// Returns `true` if `file_name` or its `extension` indicates a sensitive credentials file.
#[inline]
fn is_sensitive_file(file_name: &str, extension: Option<&str>) -> bool {
    if starts_with_ignore_ascii_case(file_name, "id_rsa")
        || starts_with_ignore_ascii_case(file_name, "id_ed25519")
        || starts_with_ignore_ascii_case(file_name, "credentials")
    {
        return true;
    }
    let Some(extension) = extension else {
        return false;
    };
    SENSITIVE_EXTENSIONS
        .iter()
        .any(|&s| s.eq_ignore_ascii_case(extension))
}

/// Returns `true` if raw path segment bytes start with a dot (`.`), marking a hidden file or directory.
#[inline]
pub(crate) fn is_hidden_segment(segment: &[u8]) -> bool {
    segment.first() == Some(&b'.')
}

/// Returns `true` if `file_name` matches a known noise file or a sensitive credentials pattern.
#[inline]
pub(crate) fn is_ignored_filename(file_name: &str) -> bool {
    let extension = file_name.rsplit_once('.').map(|(_, e)| e);
    is_noise_file(file_name) || is_sensitive_file(file_name, extension)
}

/// Classifies a sequence of raw path segment bytes into [`PathStatus`].
///
/// Builds a normalized UTF-8 path string in `scratch` for valid candidates or prefix strings for non-UTF-8 candidates.
pub(crate) fn classify_path_segments<'a, I>(segments: I, scratch: &mut String) -> PathStatus
where
    I: IntoIterator<Item = &'a [u8]>,
{
    scratch.clear();
    let mut has_hidden = false;
    let mut has_non_utf8 = false;
    let mut last_segment_bytes: Option<&'a [u8]> = None;

    for segment in segments {
        if is_hidden_segment(segment) {
            has_hidden = true;
        }
        last_segment_bytes = Some(segment);
        if has_non_utf8 {
            continue;
        }
        match std::str::from_utf8(segment) {
            Ok(s) => {
                if !scratch.is_empty() {
                    scratch.push('/');
                }
                scratch.push_str(s);
            }
            Err(_) => {
                has_non_utf8 = true;
                if !scratch.is_empty() {
                    scratch.push('/');
                }
            }
        }
    }

    if has_hidden {
        scratch.clear();
        return PathStatus::Ignored;
    }

    if let Some(last) = last_segment_bytes {
        if last.is_empty() {
            scratch.clear();
            return PathStatus::Ignored;
        }
        if let Ok(filename) = std::str::from_utf8(last) {
            if is_ignored_filename(filename) {
                scratch.clear();
                return PathStatus::Ignored;
            }
        }
    }

    if has_non_utf8 {
        return PathStatus::NonUtf8Candidate;
    }

    if scratch.is_empty() {
        return PathStatus::Ignored;
    }

    PathStatus::Valid
}

/// Returns `true` if HTTP status code is transient/retryable (HTTP 429 or 5xx).
#[inline]
pub(crate) const fn is_retryable_status(status_code: u16) -> bool {
    matches!(status_code, 429 | 500..=599)
}

/// Calculates bounded exponential backoff duration for the given attempt.
#[inline]
pub(crate) const fn retry_delay(attempt: usize) -> std::time::Duration {
    let shift = if attempt < 5 { attempt as u32 } else { 5 };
    let exponential = RETRY_INITIAL_DELAY_MS.saturating_mul(1_u64 << shift);
    let millis = if exponential < RETRY_MAX_DELAY_MS {
        exponential
    } else {
        RETRY_MAX_DELAY_MS
    };

    std::time::Duration::from_millis(millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_retryable_status_codes() {
        const {
            assert!(is_retryable_status(429));
            assert!(is_retryable_status(500));
            assert!(!is_retryable_status(200));
        }
        for code in [429, 500, 502, 503, 504] {
            assert!(is_retryable_status(code), "code {code} should be retryable");
        }
        for code in [200, 400, 401, 403, 404] {
            assert!(
                !is_retryable_status(code),
                "code {code} should not be retryable"
            );
        }
    }

    #[test]
    fn calculates_bounded_exponential_backoff() {
        const {
            assert!(retry_delay(0).as_millis() == 250);
            assert!(retry_delay(1).as_millis() == 500);
            assert!(retry_delay(2).as_millis() == 1000);
            assert!(retry_delay(5).as_millis() == 5000);
            assert!(retry_delay(10).as_millis() == 5000);
        }
        assert_eq!(retry_delay(0).as_millis(), 250);
        assert_eq!(retry_delay(1).as_millis(), 500);
        assert_eq!(retry_delay(2).as_millis(), 1000);
        assert_eq!(retry_delay(3).as_millis(), 2000);
        assert_eq!(retry_delay(4).as_millis(), 4000);
        assert_eq!(retry_delay(5).as_millis(), 5000);
        assert_eq!(retry_delay(10).as_millis(), 5000);
    }

    #[test]
    fn test_is_hidden_segment() {
        assert!(is_hidden_segment(b".git"));
        assert!(is_hidden_segment(b".env"));
        assert!(is_hidden_segment(b"."));
        assert!(is_hidden_segment(b".."));
        assert!(!is_hidden_segment(b"src"));
        assert!(!is_hidden_segment(b""));
        assert!(!is_hidden_segment(b"file.txt"));
    }

    #[test]
    fn test_classify_path_segments_non_utf8_with_ignored_filename() {
        let mut scratch = String::new();

        // non-UTF-8 directory segment with ignored filename (id_rsa) -> Ignored
        let segments: &[&[u8]] = &[b"\xff\xfe", b"id_rsa"];
        assert_eq!(
            classify_path_segments(segments, &mut scratch),
            PathStatus::Ignored
        );

        // non-UTF-8 directory segment with valid candidate filename (main.rs) -> NonUtf8Candidate
        let segments: &[&[u8]] = &[b"\xff\xfe", b"main.rs"];
        assert_eq!(
            classify_path_segments(segments, &mut scratch),
            PathStatus::NonUtf8Candidate
        );
        assert_eq!(scratch, "");

        // non-UTF-8 directory segment + valid dir + valid candidate filename -> NonUtf8Candidate with prefix
        let segments: &[&[u8]] = &[b"src", b"\xff\xfe", b"main.rs"];
        assert_eq!(
            classify_path_segments(segments, &mut scratch),
            PathStatus::NonUtf8Candidate
        );
        assert_eq!(scratch, "src/");
    }

    #[test]
    fn test_is_ignored_filename() {
        let ignored = [
            "Cargo.lock",
            "package-lock.json",
            "yarn.lock",
            "pnpm-lock.yaml",
            "go.sum",
            "server.key",
            "cert.pem",
            "client.p12",
            "cert.pkcs12",
            "credentials.json",
            "aws.credentials",
            "credentials",
            "id_rsa",
            "id_rsa.pub",
            "id_ed25519",
        ];
        for filename in ignored {
            assert!(
                is_ignored_filename(filename),
                "expected {filename} to be ignored"
            );
        }

        let kept = ["Cargo.toml", "main.rs", "Makefile", "environment.rs"];
        for filename in kept {
            assert!(
                !is_ignored_filename(filename),
                "expected {filename} not to be ignored"
            );
        }
    }
}
