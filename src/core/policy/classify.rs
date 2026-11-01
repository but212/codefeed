//! Relative path normalization, depth enforcement, and segment classification.

use super::exclude::{ExcludePatterns, push_match_segment};
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

/// Error indicating that a path's directory nesting depth exceeds [`MAX_PATH_DEPTH`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DepthExceededError;

/// Classifies a sequence of raw path segment bytes into [`PathStatus`].
///
/// Checks run in this order: hidden segment, user exclusion, noise or sensitive name, depth. Each
/// check that matches returns `Ignored` before the later ones run, so an excluded path is never
/// reported as a depth error. Builds a normalized UTF-8 path string for valid candidates or prefix
/// strings for non-UTF-8 candidates. Empty and `.` segments are dropped, so a [`NormalizedPath`]
/// never contains them.
pub(crate) fn classify_path_segments<'a, I>(
    segments: I,
    exclude: &ExcludePatterns,
) -> Result<PathStatus, DepthExceededError>
where
    I: IntoIterator<Item = &'a [u8]>,
{
    let segments: Vec<&'a [u8]> = segments
        .into_iter()
        .filter(|seg| !seg.is_empty() && *seg != b".")
        .collect();
    let Some((leaf, ancestors)) = segments.split_last() else {
        return Ok(PathStatus::Ignored);
    };
    let mut dir = AdmittedDir::root();
    for name in ancestors {
        let Some(admitted) = admit_dir(&dir, name, exclude) else {
            return Ok(PathStatus::Ignored);
        };
        dir = admitted;
    }
    classify_leaf(&dir, leaf, exclude)
}

/// A directory that passed the hidden and user-exclusion checks, with the prefixes a file below it needs.
///
/// Built only by [`AdmittedDir::root`] and [`admit_dir`], so holding one means every ancestor was checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AdmittedDir {
    depth: usize,
    match_prefix: String,
    output: DirPrefix,
}

/// Output prefix of an admitted directory. `NonUtf8` holds the valid prefix before the first non-UTF-8
/// segment, which every path below reports as its candidate prefix.
#[derive(Clone, Debug, PartialEq, Eq)]
enum DirPrefix {
    Utf8(String),
    NonUtf8(String),
}

impl AdmittedDir {
    /// The source root, which has no ancestors to check.
    pub(crate) fn root() -> Self {
        Self {
            depth: 0,
            match_prefix: String::new(),
            output: DirPrefix::Utf8(String::new()),
        }
    }
}

/// Admits directory `name` under `parent`. Returns `None` when it is hidden or user-excluded.
///
/// Only the new prefix is matched. The prefixes of `parent`'s ancestors were matched when `parent` was
/// admitted, and a match there would have returned `None` already. Matching stops after the first
/// `MAX_PATH_DEPTH + 1` segments, so a directory past that window is not matched.
pub(crate) fn admit_dir(
    parent: &AdmittedDir,
    name: &[u8],
    exclude: &ExcludePatterns,
) -> Option<AdmittedDir> {
    if is_hidden_segment(name) {
        return None;
    }
    let mut match_prefix = parent.match_prefix.clone();
    push_match_segment(&mut match_prefix, name);
    if parent.depth <= MAX_PATH_DEPTH && exclude.excludes_prefix(&match_prefix, true) {
        return None;
    }
    let output = match &parent.output {
        DirPrefix::NonUtf8(prefix) => DirPrefix::NonUtf8(prefix.clone()),
        DirPrefix::Utf8(prefix) => match std::str::from_utf8(name) {
            Ok(segment) => DirPrefix::Utf8(join_segment(prefix, segment)),
            Err(_) => DirPrefix::NonUtf8(with_separator(prefix)),
        },
    };
    Some(AdmittedDir {
        depth: parent.depth + 1,
        match_prefix,
        output,
    })
}

/// Classifies file `name` directly under `parent`. The checks on `parent`'s ancestors already ran in
/// [`admit_dir`], so only the file itself is matched here.
///
/// Check order: hidden segment, user exclusion, noise or sensitive name, depth, then non-UTF-8 prefixing.
pub(crate) fn classify_leaf(
    parent: &AdmittedDir,
    name: &[u8],
    exclude: &ExcludePatterns,
) -> Result<PathStatus, DepthExceededError> {
    if is_hidden_segment(name) {
        return Ok(PathStatus::Ignored);
    }
    if parent.depth <= MAX_PATH_DEPTH {
        let mut full = parent.match_prefix.clone();
        push_match_segment(&mut full, name);
        if exclude.excludes_prefix(&full, false) {
            return Ok(PathStatus::Ignored);
        }
    }

    // Lossy decoding keeps the ASCII name rules exact: invalid bytes become U+FFFD, which never
    // equals a noise name, and prefix and extension matches see the same ASCII bytes.
    if is_ignored_filename(&String::from_utf8_lossy(name)) {
        return Ok(PathStatus::Ignored);
    }

    if parent.depth > MAX_PATH_DEPTH {
        return Err(DepthExceededError);
    }

    match (&parent.output, std::str::from_utf8(name)) {
        (DirPrefix::NonUtf8(prefix), _) => Ok(PathStatus::NonUtf8Candidate(prefix.clone())),
        (DirPrefix::Utf8(prefix), Err(_)) => {
            Ok(PathStatus::NonUtf8Candidate(with_separator(prefix)))
        }
        (DirPrefix::Utf8(prefix), Ok(segment)) => Ok(PathStatus::Valid(NormalizedPath(
            join_segment(prefix, segment),
        ))),
    }
}

/// The prefix followed by its `/` separator, or empty when the prefix is empty. This is the output a
/// non-UTF-8 segment reports, so the trailing separator is part of the contract.
fn with_separator(prefix: &str) -> String {
    if prefix.is_empty() {
        String::new()
    } else {
        format!("{prefix}/")
    }
}

/// Appends `segment` to a `/`-joined prefix, with no leading separator when the prefix is empty.
fn join_segment(prefix: &str, segment: &str) -> String {
    let mut joined = with_separator(prefix);
    joined.push_str(segment);
    joined
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_path_segments_non_utf8_with_ignored_filename() {
        // non-UTF-8 directory segment with ignored filename (id_rsa) -> Ignored
        let segments: &[&[u8]] = &[b"\xff\xfe", b"id_rsa"];
        assert_eq!(
            classify_path_segments(segments.iter().copied(), &ExcludePatterns::default()),
            Ok(PathStatus::Ignored)
        );

        // non-UTF-8 directory segment with valid candidate filename (main.rs) -> NonUtf8Candidate
        let segments: &[&[u8]] = &[b"\xff\xfe", b"main.rs"];
        assert_eq!(
            classify_path_segments(segments.iter().copied(), &ExcludePatterns::default()),
            Ok(PathStatus::NonUtf8Candidate(String::new()))
        );

        // non-UTF-8 directory segment + valid dir + valid candidate filename -> NonUtf8Candidate with prefix
        let segments: &[&[u8]] = &[b"src", b"\xff\xfe", b"main.rs"];
        assert_eq!(
            classify_path_segments(segments.iter().copied(), &ExcludePatterns::default()),
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
                classify_path_segments(segments.iter().copied(), &ExcludePatterns::default()),
                Ok(PathStatus::Ignored),
                "segments: {segments:?}"
            );
        }

        // C-02: invalid-UTF-8 final segment with no sensitive match -> NonUtf8Candidate
        let plain: [&[&[u8]]; 2] = [&[b"notes\xff.txt"], &[b"src", b"\xff.rs"]];
        for segments in plain {
            assert!(
                matches!(
                    classify_path_segments(segments.iter().copied(), &ExcludePatterns::default()),
                    Ok(PathStatus::NonUtf8Candidate(_))
                ),
                "segments: {segments:?}"
            );
        }

        // C-03: noise names match exactly; invalid bytes make Cargo.lock differ
        let noise_suffix: &[&[u8]] = &[b"Cargo.lock\xff"];
        assert!(matches!(
            classify_path_segments(noise_suffix.iter().copied(), &ExcludePatterns::default()),
            Ok(PathStatus::NonUtf8Candidate(_))
        ));
    }

    #[test]
    fn test_classify_path_segments_depth_limit() {
        let normal: Vec<&[u8]> = std::iter::repeat_n(b"d".as_slice(), MAX_PATH_DEPTH)
            .chain(std::iter::once(b"file.txt".as_slice()))
            .collect();
        assert!(classify_path_segments(normal, &ExcludePatterns::default()).is_ok());

        let excessive: Vec<&[u8]> = std::iter::repeat_n(b"d".as_slice(), MAX_PATH_DEPTH + 1)
            .chain(std::iter::once(b"file.txt".as_slice()))
            .collect();
        assert_eq!(
            classify_path_segments(excessive, &ExcludePatterns::default()),
            Err(DepthExceededError)
        );

        let deep_hidden: Vec<&[u8]> = std::iter::once(b".hidden".as_slice())
            .chain(std::iter::repeat_n(b"d".as_slice(), 300))
            .chain(std::iter::once(b"file.txt".as_slice()))
            .collect();
        assert_eq!(
            classify_path_segments(deep_hidden, &ExcludePatterns::default()),
            Ok(PathStatus::Ignored)
        );

        let deep_noise: Vec<&[u8]> = std::iter::repeat_n(b"d".as_slice(), MAX_PATH_DEPTH + 1)
            .chain(std::iter::once(b"Cargo.lock".as_slice()))
            .collect();
        assert_eq!(
            classify_path_segments(deep_noise, &ExcludePatterns::default()),
            Ok(PathStatus::Ignored)
        );
    }

    #[test]
    fn test_normalized_path_invariants_and_classification() {
        let segments: &[&[u8]] = &[b"src", b"core", b"model.rs"];
        let status =
            classify_path_segments(segments.iter().copied(), &ExcludePatterns::default()).unwrap();
        let PathStatus::Valid(path) = status else {
            panic!("expected PathStatus::Valid");
        };
        assert_eq!(path.as_str(), "src/core/model.rs");
        assert_eq!(&*path, "src/core/model.rs");
    }

    #[test]
    fn user_exclusions_drop_matching_paths_before_other_checks() {
        let exclude = ExcludePatterns::new(["build/", "*.log"]).unwrap();

        // An excluded ancestor, an excluded file name, and an excluded non-UTF-8 final segment are all
        // Ignored, so none of them is counted as a skipped candidate.
        let excluded: [&[&[u8]]; 4] = [
            &[b"build", b"main.rs"],
            &[b"src", b"build", b"x.txt"],
            &[b"src", b"app.log"],
            &[b"build", b"\xff"],
        ];
        for segments in excluded {
            assert_eq!(
                classify_path_segments(segments.iter().copied(), &exclude),
                Ok(PathStatus::Ignored),
                "segments: {segments:?}"
            );
        }

        let kept: &[&[u8]] = &[b"src", b"main.rs"];
        assert_eq!(
            classify_path_segments(kept.iter().copied(), &exclude),
            Ok(PathStatus::Valid("src/main.rs".into()))
        );
    }

    #[test]
    fn excluded_path_past_depth_limit_is_ignored_not_an_error() {
        let deep: Vec<&[u8]> = std::iter::once(b"build".as_slice())
            .chain(std::iter::repeat_n(b"d".as_slice(), MAX_PATH_DEPTH + 1))
            .chain(std::iter::once(b"file.txt".as_slice()))
            .collect();

        let exclude = ExcludePatterns::new(["build/"]).unwrap();
        assert_eq!(
            classify_path_segments(deep.iter().copied(), &exclude),
            Ok(PathStatus::Ignored)
        );
        assert_eq!(
            classify_path_segments(deep, &ExcludePatterns::default()),
            Err(DepthExceededError)
        );
    }

    #[test]
    fn match_beyond_the_depth_window_still_fails_the_depth_limit() {
        // The `build` match is past the first `MAX_PATH_DEPTH + 1` segments, so it is not seen.
        let deep: Vec<&[u8]> = std::iter::repeat_n(b"d".as_slice(), MAX_PATH_DEPTH + 1)
            .chain(std::iter::once(b"build".as_slice()))
            .chain(std::iter::once(b"file.txt".as_slice()))
            .collect();

        let exclude = ExcludePatterns::new(["build/"]).unwrap();
        assert_eq!(
            classify_path_segments(deep, &exclude),
            Err(DepthExceededError)
        );
    }

    #[test]
    fn user_exclusion_applies_to_paths_longer_than_max_path_bytes() {
        // Twenty-one bytes per directory: the prefix passes `MAX_PATH_BYTES` long before the final segment.
        let dir = "d".repeat(20);
        let mut segments: Vec<&[u8]> = (0..220).map(|_| dir.as_bytes()).collect();
        segments.push(b"x.log".as_slice());

        let exclude = ExcludePatterns::new(["*.log"]).unwrap();
        assert_eq!(
            classify_path_segments(segments.iter().copied(), &exclude),
            Ok(PathStatus::Ignored)
        );
        assert!(matches!(
            classify_path_segments(segments.iter().copied(), &ExcludePatterns::default()),
            Ok(PathStatus::Valid(_))
        ));
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
                classify_path_segments(segments.iter().copied(), &ExcludePatterns::default()),
                Ok(expected),
                "segments: {segments:?}"
            );
        }
    }

    #[test]
    fn admit_dir_checks_only_the_match_window_and_hidden_names() {
        let exclude = ExcludePatterns::new(["build/"]).unwrap();
        let root = AdmittedDir::root();
        assert!(admit_dir(&root, b"build", &exclude).is_none());
        assert!(admit_dir(&root, b".git", &exclude).is_none());
        assert!(admit_dir(&root, b"src", &exclude).is_some());

        // A match past the first `MAX_PATH_DEPTH + 1` directories is not seen, as in the file check.
        let mut deep = AdmittedDir::root();
        for _ in 0..=MAX_PATH_DEPTH {
            deep = admit_dir(&deep, b"d", &exclude).expect("d is not excluded");
        }
        assert!(admit_dir(&deep, b"build", &exclude).is_some());
        assert!(
            admit_dir(&deep, b".git", &exclude).is_none(),
            "hidden names are checked at every depth"
        );
    }

    #[test]
    fn classification_corpus_is_stable() {
        // Pins the observable outcome of every check, including check order, the depth window, and the
        // trailing separator of non-UTF-8 prefixes. Both the archive and the local walk rely on it.
        fn segs<'a>(parts: &[&'a [u8]]) -> Vec<&'a [u8]> {
            parts.to_vec()
        }
        fn deep(n: usize, tail: &[u8]) -> Vec<&[u8]> {
            std::iter::repeat_n(b"d".as_slice(), n)
                .chain(std::iter::once(tail))
                .collect()
        }
        let valid =
            |p: &str| -> Result<PathStatus, DepthExceededError> { Ok(PathStatus::Valid(p.into())) };
        let nonutf8 = |p: &str| -> Result<PathStatus, DepthExceededError> {
            Ok(PathStatus::NonUtf8Candidate(p.into()))
        };
        let ignored = || -> Result<PathStatus, DepthExceededError> { Ok(PathStatus::Ignored) };
        let depth_err = || -> Result<PathStatus, DepthExceededError> { Err(DepthExceededError) };

        let exclude = ExcludePatterns::new(["build/", "*.log", "/top"]).unwrap();
        let none = ExcludePatterns::default();
        let max = MAX_PATH_DEPTH;

        let mut build_past_window = deep(max + 1, b"build");
        build_past_window.push(b"x.txt");
        let mut hidden_past_window = deep(max + 1, b".h");
        hidden_past_window.push(b"x");
        let max_valid = format!("{}file.txt", "d/".repeat(max));

        let cases = vec![
            (segs(&[b"src", b"main.rs"]), &exclude, valid("src/main.rs")),
            (segs(&[b".git", b"config"]), &exclude, ignored()),
            (segs(&[b"src", b".hid", b"x.rs"]), &exclude, ignored()),
            (segs(&[b"build", b"main.rs"]), &exclude, ignored()),
            (segs(&[b"src", b"build", b"x.txt"]), &exclude, ignored()),
            (segs(&[b"src", b"a.log"]), &exclude, ignored()),
            (segs(&[b"top", b"x.rs"]), &exclude, ignored()),
            (
                segs(&[b"sub", b"top", b"x.rs"]),
                &exclude,
                valid("sub/top/x.rs"),
            ),
            (segs(&[b"src", b"build"]), &exclude, valid("src/build")),
            (segs(&[b"src", b"Cargo.lock"]), &exclude, ignored()),
            (
                segs(&[b"src", b"\xff\xfe", b"main.rs"]),
                &exclude,
                nonutf8("src/"),
            ),
            (segs(&[b"\xff", b"x.rs"]), &exclude, nonutf8("")),
            (segs(&[b"src", b"\xff.rs"]), &exclude, nonutf8("src/")),
            (segs(&[b"\xff", b"build", b"x"]), &exclude, ignored()),
            (segs(&[b"build", b"\xff"]), &exclude, ignored()),
            (segs(&[b"\xff", b"a.log"]), &exclude, ignored()),
            (segs(&[b"\xff.log"]), &exclude, ignored()),
            (segs(&[b"\xff", b"id_rsa"]), &none, ignored()),
            (
                segs(&[b"docs\\secret.md"]),
                &exclude,
                valid("docs\\secret.md"),
            ),
            (deep(max, b"file.txt"), &none, valid(&max_valid)),
            (deep(max, b"a.log"), &exclude, ignored()),
            (deep(max + 1, b"x.rs"), &none, depth_err()),
            (deep(max + 1, b"a.log"), &exclude, depth_err()),
            (build_past_window, &exclude, depth_err()),
            (hidden_past_window, &none, ignored()),
        ];
        for (segments, exclude, expected) in cases {
            assert_eq!(
                classify_path_segments(segments.iter().copied(), exclude),
                expected,
                "segments: {segments:?}"
            );
        }
    }
}
