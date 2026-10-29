//! Relative path normalization, depth enforcement, and segment classification.

use super::exclusions::{is_hidden_segment, is_ignored_filename};
use super::limits::MAX_PATH_DEPTH;

/// Non-empty, normalized slash-separated relative UTF-8 path.
#[derive(Debug, Clone, PartialEq, Eq)]
#[repr(transparent)]
pub(crate) struct NormalizedPath(String);

impl NormalizedPath {
    /// Borrows the normalized path as a string slice.
    pub(crate) const fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl std::ops::Deref for NormalizedPath {
    type Target = str;

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
            PathClassifierState::ValidUtf8 => {
                if let Ok(s) = std::str::from_utf8(segment) {
                    if !scratch.is_empty() {
                        scratch.push('/');
                    }
                    scratch.push_str(s);
                } else {
                    state = PathClassifierState::NonUtf8Prefix;
                    if !scratch.is_empty() {
                        scratch.push('/');
                    }
                }
            }
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
}
