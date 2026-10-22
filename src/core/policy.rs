//! Single source of truth for ingestion policies, limits, and conventions.

use memchr::memrchr;

/// Maximum individual file size accepted (1 MiB in bytes). Larger files are skipped.
pub(crate) const MAX_FILE_SIZE: u64 = 1024 * 1024;
/// Maximum rendered digest size (20 MiB in bytes). Overflows return [`DomainError::OutputTooLarge`](crate::core::error::DomainError::OutputTooLarge).
pub(crate) const MAX_OUTPUT_SIZE: u64 = 20 * 1024 * 1024;
/// Maximum candidate files evaluated before candidate budget overflows (10,000 files).
pub(crate) const MAX_FILE_COUNT: usize = 10_000;
/// Maximum directory nesting depth permitted for archive entry paths (256 levels).
pub(crate) const MAX_PATH_DEPTH: usize = 256;
/// Maximum allowable path length in bytes (4,096 bytes). Paths exceeding this limit are skipped.
pub(crate) const MAX_PATH_BYTES: u64 = 4096;
/// Capacity in bytes for buffering incoming archive decompression streams (64 KiB).
pub(crate) const ARCHIVE_BUFFER_BYTES: usize = 64 * 1024;
/// Capacity in messages of the local worker queue. Full queues block workers, which bounds read-ahead
/// memory to roughly this many files plus one per worker (security §5, §8).
pub(crate) const LOCAL_CHANNEL_CAPACITY: usize = 64;

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

const _: () = {
    assert!(MAX_OUTPUT_SIZE + MAX_FILE_SIZE + MAX_PATH_BYTES <= u32::MAX as u64);
    assert!(MAX_FILE_SIZE <= MAX_OUTPUT_SIZE);
    assert!(MAX_PATH_BYTES <= MAX_FILE_SIZE);
    assert!(MAX_FILE_COUNT > 0);
    assert!(MAX_PATH_DEPTH > 0);
    assert!(ASCII_BYTES_PER_TOKEN > 0);
    assert!(GITHUB_TIMEOUT_SECS <= ARCHIVE_MAX_SECS);
    assert!(ARCHIVE_BUFFER_BYTES > 0);
    assert!(LOCAL_CHANNEL_CAPACITY > 0);
};

/// Conventional lockfiles excluded before candidate budget accounting.
const NOISE: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lock",
    "bun.lockb",
    "go.sum",
    "poetry.lock",
    "Pipfile.lock",
    "composer.lock",
    "Gemfile.lock",
];

/// Known sensitive file extensions (case-insensitive) excluded prior to reading or download.
///
/// Note: `credentials` is matched as an extension here (`*.credentials`, e.g., `aws.credentials`),
/// and is also matched as a filename prefix in [`SENSITIVE_PREFIXES`] (`credentials*`, e.g., `credentials.json`).
const SENSITIVE_EXTENSIONS: &[&str] = &["pem", "key", "p12", "pkcs12", "pfx", "credentials"];

/// Known sensitive filename prefixes (case-insensitive) excluded prior to reading or download.
const SENSITIVE_PREFIXES: &[&str] = &["id_rsa", "id_ed25519", "id_ecdsa", "id_dsa", "credentials"];

/// Non-empty, normalized slash-separated relative UTF-8 path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub(crate) struct NormalizedPath(String);

impl NormalizedPath {
    /// Borrows the normalized path as a string slice.
    #[inline(always)]
    pub(crate) const fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl std::ops::Deref for NormalizedPath {
    type Target = str;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
impl From<&str> for NormalizedPath {
    fn from(s: &str) -> Self {
        assert!(!s.is_empty(), "NormalizedPath cannot be empty");
        Self(s.to_string())
    }
}

#[cfg(test)]
impl From<String> for NormalizedPath {
    fn from(s: String) -> Self {
        assert!(!s.is_empty(), "NormalizedPath cannot be empty");
        Self(s)
    }
}

/// Path classification outcome from a single-pass policy evaluation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PathStatus {
    /// Normalized valid relative UTF-8 path.
    Valid(NormalizedPath),
    /// Ignored path (hidden component, conventional noise, sensitive pattern, or root).
    Ignored,
    /// Non-UTF-8 path that is not hidden and must be counted against candidate budget as skipped.
    NonUtf8Candidate(String),
}

fn starts_with_ignore_ascii_case(value: &str, prefix: &str) -> bool {
    value
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

/// Returns `true` if `file_name` indicates a sensitive credentials file.
fn is_sensitive_file(file_name: &str) -> bool {
    if SENSITIVE_PREFIXES
        .iter()
        .any(|&prefix| starts_with_ignore_ascii_case(file_name, prefix))
    {
        return true;
    }
    let Some(dot_idx) = memrchr(b'.', file_name.as_bytes()) else {
        return false;
    };
    let extension = &file_name[dot_idx + 1..];
    SENSITIVE_EXTENSIONS
        .iter()
        .any(|&s| s.eq_ignore_ascii_case(extension))
}

/// Returns `true` if raw path segment bytes start with a dot (`.`), marking a hidden file or directory.
#[must_use]
pub(crate) const fn is_hidden_segment(segment: &[u8]) -> bool {
    matches!(segment, [b'.', ..])
}

/// Returns `true` if `file_name` matches a known noise file or a sensitive credentials pattern.
#[must_use]
pub(crate) fn is_ignored_filename(file_name: &str) -> bool {
    NOISE.contains(&file_name) || is_sensitive_file(file_name)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathClassifierState {
    ValidUtf8,
    NonUtf8Prefix,
}

/// Error indicating that a path's directory nesting depth exceeds [`MAX_PATH_DEPTH`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DepthExceededError;

/// Classifies a sequence of raw path segment bytes into [`PathStatus`].
///
/// Builds a normalized UTF-8 path string for valid candidates or prefix strings for non-UTF-8 candidates.
/// Empty and `.` segments are dropped, so a [`NormalizedPath`] never contains them.
pub(crate) fn classify_path_segments<'a, I>(segments: I) -> Result<PathStatus, DepthExceededError>
where
    I: IntoIterator<Item = &'a [u8]>,
{
    let mut scratch = String::new();
    let mut state = PathClassifierState::ValidUtf8;
    let mut last_segment: Option<&'a [u8]> = None;
    let mut segment_count = 0usize;

    for segment in segments
        .into_iter()
        .filter(|seg| !seg.is_empty() && *seg != b".")
    {
        if is_hidden_segment(segment) {
            return Ok(PathStatus::Ignored);
        }

        segment_count = segment_count.saturating_add(1);
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
        return Ok(PathStatus::Ignored);
    };

    // Lossy decoding keeps the ASCII name rules exact: invalid bytes become U+FFFD, which never
    // equals a noise name, and prefix and extension matches see the same ASCII bytes.
    if is_ignored_filename(&String::from_utf8_lossy(last)) {
        return Ok(PathStatus::Ignored);
    }

    if segment_count.saturating_sub(1) > MAX_PATH_DEPTH {
        return Err(DepthExceededError);
    }

    match state {
        PathClassifierState::ValidUtf8 => Ok(PathStatus::Valid(NormalizedPath(scratch))),
        PathClassifierState::NonUtf8Prefix => Ok(PathStatus::NonUtf8Candidate(scratch)),
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
            Ok(PathStatus::Ignored)
        );

        // non-UTF-8 directory segment with valid candidate filename (main.rs) -> NonUtf8Candidate
        let segments: &[&[u8]] = &[b"\xff\xfe", b"main.rs"];
        assert_eq!(
            classify_path_segments(segments.iter().copied()),
            Ok(PathStatus::NonUtf8Candidate(String::new()))
        );

        // non-UTF-8 directory segment + valid dir + valid candidate filename -> NonUtf8Candidate with prefix
        let segments: &[&[u8]] = &[b"src", b"\xff\xfe", b"main.rs"];
        assert_eq!(
            classify_path_segments(segments.iter().copied()),
            Ok(PathStatus::NonUtf8Candidate("src/".to_string()))
        );

        // C-01: sensitive prefix or extension on an invalid-UTF-8 final segment -> Ignored
        let sensitive: [&[&[u8]]; 3] = [
            &[b"dir", b"id_rsa\xff.key"],
            &[b"secret\xff.key"],
            &[b"dir", b"credentials\xff"],
        ];
        for segments in sensitive {
            assert_eq!(
                classify_path_segments(segments.iter().copied()),
                Ok(PathStatus::Ignored),
                "segments: {segments:?}"
            );
        }

        // C-02: invalid-UTF-8 final segment with no sensitive match -> NonUtf8Candidate
        let plain: [&[&[u8]]; 2] = [&[b"notes\xff.txt"], &[b"src", b"\xff.rs"]];
        for segments in plain {
            assert!(
                matches!(
                    classify_path_segments(segments.iter().copied()),
                    Ok(PathStatus::NonUtf8Candidate(_))
                ),
                "segments: {segments:?}"
            );
        }

        // C-03: noise names match exactly; invalid bytes make Cargo.lock differ
        let noise_suffix: &[&[u8]] = &[b"Cargo.lock\xff"];
        assert!(matches!(
            classify_path_segments(noise_suffix.iter().copied()),
            Ok(PathStatus::NonUtf8Candidate(_))
        ));
    }

    #[test]
    fn test_classify_path_segments_depth_limit() {
        let normal: Vec<&[u8]> = std::iter::repeat_n(b"d".as_slice(), 256)
            .chain(std::iter::once(b"file.txt".as_slice()))
            .collect();
        assert!(classify_path_segments(normal).is_ok());

        let excessive: Vec<&[u8]> = std::iter::repeat_n(b"d".as_slice(), 257)
            .chain(std::iter::once(b"file.txt".as_slice()))
            .collect();
        assert_eq!(classify_path_segments(excessive), Err(DepthExceededError));

        let deep_hidden: Vec<&[u8]> = std::iter::once(b".hidden".as_slice())
            .chain(std::iter::repeat_n(b"d".as_slice(), 300))
            .chain(std::iter::once(b"file.txt".as_slice()))
            .collect();
        assert_eq!(classify_path_segments(deep_hidden), Ok(PathStatus::Ignored));

        let deep_noise: Vec<&[u8]> = std::iter::repeat_n(b"d".as_slice(), 257)
            .chain(std::iter::once(b"Cargo.lock".as_slice()))
            .collect();
        assert_eq!(classify_path_segments(deep_noise), Ok(PathStatus::Ignored));
    }

    #[test]
    fn test_is_ignored_filename() {
        let ignored = [
            "Cargo.lock",
            "package-lock.json",
            "yarn.lock",
            "pnpm-lock.yaml",
            "go.sum",
            "poetry.lock",
            "Pipfile.lock",
            "composer.lock",
            "Gemfile.lock",
            "bun.lock",
            "bun.lockb",
            "server.key",
            "cert.pem",
            "client.p12",
            "cert.pkcs12",
            "identity.pfx",
            "credentials.json",
            "aws.credentials",
            "credentials",
            "id_rsa",
            "id_rsa.pub",
            "id_ed25519",
            "id_ed25519.pub",
            "id_ecdsa",
            "id_ecdsa.pub",
            "id_dsa",
            "id_dsa.pub",
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

    #[test]
    fn test_normalized_path_invariants_and_classification() {
        let segments: &[&[u8]] = &[b"src", b"core", b"model.rs"];
        let status = classify_path_segments(segments.iter().copied()).unwrap();
        let PathStatus::Valid(path) = status else {
            panic!("expected PathStatus::Valid");
        };
        assert_eq!(path.as_str(), "src/core/model.rs");
        assert_eq!(&*path, "src/core/model.rs");
    }

    #[test]
    fn classify_path_segments_drops_empty_and_dot_segments() {
        let cases: [(&[&[u8]], PathStatus); 5] = [
            (&[b"a", b"", b"b"], PathStatus::Valid("a/b".into())),
            (&[b"a", b".", b"b"], PathStatus::Valid("a/b".into())),
            (&[b"", b""], PathStatus::Ignored),
            (&[b"."], PathStatus::Ignored),
            (&[b"a", b"", b""], PathStatus::Valid("a".into())),
        ];
        for (segments, expected) in cases {
            assert_eq!(
                classify_path_segments(segments.iter().copied()),
                Ok(expected),
                "segments: {segments:?}"
            );
        }
    }

    #[test]
    #[should_panic(expected = "NormalizedPath cannot be empty")]
    fn test_normalized_path_rejects_empty_in_test_helper() {
        let _ = NormalizedPath::from("");
    }
}
