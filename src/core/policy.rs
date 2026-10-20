//! Single source of truth for ingestion policies, limits, and conventions.

use memchr::memrchr;

/// Maximum individual file size accepted (1 MiB in bytes). Larger files are skipped.
pub(crate) const MAX_FILE_SIZE: u64 = 1024 * 1024;
/// Maximum rendered digest size (20 MiB in bytes). Overflows return [`IngestError::OutputTooLarge`](crate::core::error::IngestError::OutputTooLarge).
pub(crate) const MAX_OUTPUT_SIZE: u64 = 20 * 1024 * 1024;
/// Maximum candidate files evaluated before candidate budget overflows (10,000 files).
pub(crate) const MAX_FILE_COUNT: usize = 10_000;
/// Maximum directory nesting depth permitted for archive entry paths (256 levels).
pub(crate) const MAX_PATH_DEPTH: usize = 256;
/// Maximum allowable path length in bytes (4,096 bytes). Paths exceeding this limit are skipped.
pub(crate) const MAX_PATH_BYTES: u64 = 4096;

const _: () = assert!(MAX_OUTPUT_SIZE + MAX_FILE_SIZE + MAX_PATH_BYTES <= u32::MAX as u64);
/// Maximum duration in seconds allowed for whole-archive Tarball transfer (5 minutes).
pub(crate) const ARCHIVE_MAX_SECS: u64 = 300;
/// Maximum total HTTP request attempts (1 initial + up to 2 retries).
pub(crate) const MAX_ATTEMPTS: usize = 3;
/// Fast byte-level token heuristic constant for English text and source code.
pub(crate) const ASCII_BYTES_PER_TOKEN: u64 = 4;
/// Base initial delay in milliseconds for exponential backoff.
pub(crate) const RETRY_INITIAL_DELAY_MS: u64 = 250;
/// Maximum delay cap in milliseconds for exponential backoff.
pub(crate) const RETRY_MAX_DELAY_MS: u64 = 5_000;
/// HTTP response headers timeout in seconds for GitHub API requests (30 seconds).
pub(crate) const GITHUB_TIMEOUT_SECS: u64 = 30;

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

/// Path classification outcome from a single-pass policy evaluation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PathStatus {
    /// Normalized valid relative UTF-8 path.
    Valid(String),
    /// Ignored path (hidden component, conventional noise, sensitive pattern, or root).
    Ignored,
    /// Non-UTF-8 path that is not hidden and must be counted against candidate budget as skipped.
    NonUtf8Candidate(String),
}

/// Returns `true` if `file_name` matches a known noise file.
fn is_noise_file(file_name: &str) -> bool {
    NOISE.contains(&file_name)
}

fn starts_with_ignore_ascii_case(value: &str, prefix: &str) -> bool {
    value
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

/// Returns `true` if `file_name` or its `extension` indicates a sensitive credentials file.
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
pub(crate) fn is_hidden_segment(segment: &[u8]) -> bool {
    segment.first() == Some(&b'.')
}

/// Returns `true` if `file_name` matches a known noise file or a sensitive credentials pattern.
pub(crate) fn is_ignored_filename(file_name: &str) -> bool {
    let extension = memrchr(b'.', file_name.as_bytes()).map(|i| &file_name[i + 1..]);
    is_noise_file(file_name) || is_sensitive_file(file_name, extension)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathClassifierState {
    ValidUtf8,
    NonUtf8Prefix,
}

/// Classifies a sequence of raw path segment bytes into [`PathStatus`].
///
/// Builds a normalized UTF-8 path string for valid candidates or prefix strings for non-UTF-8 candidates.
pub(crate) fn classify_path_segments<'a, I>(segments: I) -> PathStatus
where
    I: IntoIterator<Item = &'a [u8]>,
{
    let mut scratch = String::new();
    let mut state = PathClassifierState::ValidUtf8;
    let mut last_segment: Option<&'a [u8]> = None;

    for segment in segments {
        if is_hidden_segment(segment) {
            return PathStatus::Ignored;
        }

        last_segment = Some(segment);

        match state {
            PathClassifierState::ValidUtf8 => match std::str::from_utf8(segment) {
                Ok(s) => {
                    if !scratch.is_empty() {
                        scratch.push('/');
                    }
                    scratch.push_str(s);
                }
                Err(_) => {
                    state = PathClassifierState::NonUtf8Prefix;
                    if !scratch.is_empty() {
                        scratch.push('/');
                    }
                }
            },
            PathClassifierState::NonUtf8Prefix => {}
        }
    }

    let Some(last) = last_segment else {
        return PathStatus::Ignored;
    };

    if last.is_empty() {
        return PathStatus::Ignored;
    }
    if matches!(std::str::from_utf8(last), Ok(filename) if is_ignored_filename(filename)) {
        return PathStatus::Ignored;
    }

    match state {
        PathClassifierState::ValidUtf8 => {
            if scratch.is_empty() {
                PathStatus::Ignored
            } else {
                PathStatus::Valid(scratch)
            }
        }
        PathClassifierState::NonUtf8Prefix => PathStatus::NonUtf8Candidate(scratch),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // non-UTF-8 directory segment with ignored filename (id_rsa) -> Ignored
        let segments: &[&[u8]] = &[b"\xff\xfe", b"id_rsa"];
        assert_eq!(
            classify_path_segments(segments.iter().copied()),
            PathStatus::Ignored
        );

        // non-UTF-8 directory segment with valid candidate filename (main.rs) -> NonUtf8Candidate
        let segments: &[&[u8]] = &[b"\xff\xfe", b"main.rs"];
        assert_eq!(
            classify_path_segments(segments.iter().copied()),
            PathStatus::NonUtf8Candidate(String::new())
        );

        // non-UTF-8 directory segment + valid dir + valid candidate filename -> NonUtf8Candidate with prefix
        let segments: &[&[u8]] = &[b"src", b"\xff\xfe", b"main.rs"];
        assert_eq!(
            classify_path_segments(segments.iter().copied()),
            PathStatus::NonUtf8Candidate("src/".to_string())
        );
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
