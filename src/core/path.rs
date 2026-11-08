//! Relative path normalization, depth enforcement, and segment classification.

use crate::core::error::DomainError;
use crate::core::policy::{
    ExcludePatterns, MAX_PATH_BYTES_USIZE, MAX_PATH_DEPTH, is_hidden_segment, is_ignored_filename,
    push_match_segment,
};

/// Non-empty, normalized slash-separated relative UTF-8 path of at most `MAX_PATH_BYTES` bytes (I-10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NormalizedPath(String);

impl std::ops::Deref for NormalizedPath {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
impl From<&str> for NormalizedPath {
    fn from(s: &str) -> Self {
        Self::from(s.to_string())
    }
}

#[cfg(test)]
impl From<String> for NormalizedPath {
    fn from(s: String) -> Self {
        assert!(
            !s.is_empty() && s.split('/').all(|segment| !segment.is_empty()),
            "NormalizedPath requires non-empty segments and no trailing slash"
        );
        assert!(
            s.len() <= MAX_PATH_BYTES_USIZE,
            "NormalizedPath requires at most MAX_PATH_BYTES bytes"
        );
        Self(s)
    }
}

/// Candidate path outcome from a single-pass policy evaluation.
///
/// An entry is either a storable path or an unstorable candidate carrying its valid directory prefix.
/// Ignored entries (hidden, conventional noise, sensitive patterns, or empty/root) are represented by `None`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CandidatePath {
    /// Normalized relative UTF-8 path within `MAX_PATH_BYTES`.
    Valid(NormalizedPath),
    /// Path that is not hidden but cannot be stored, because it is non-UTF-8 or longer than
    /// `MAX_PATH_BYTES`. It is counted against the candidate budget as skipped.
    Unstorable(String),
}

/// Error indicating that a path's directory nesting depth exceeds [`MAX_PATH_DEPTH`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DepthExceededError;

impl DepthExceededError {
    /// Returns the [`DomainError::InvalidMetadata`] for a path nested past [`MAX_PATH_DEPTH`].
    /// `subject` names the entry kind in the message, such as `local path` or `Tar entry path`.
    #[cold]
    pub(crate) fn domain_error(subject: &str) -> DomainError {
        DomainError::invalid_metadata(format!("{subject} exceeds maximum directory depth"))
    }
}

/// Classifies a sequence of raw path segment bytes into [`Option<CandidatePath>`].
///
/// Checks run in this order: hidden segment, user exclusion, noise or sensitive name, depth. Each
/// check that matches returns `None` before the later ones run, so a path excluded within the first
/// `MAX_PATH_DEPTH + 1` segments is never reported as a depth error. A match past that window is not
/// seen, and the path fails its depth limit. Builds a normalized UTF-8 path string for valid
/// candidates or prefix strings for non-UTF-8 candidates. Empty and `.` segments are dropped, so a
/// [`NormalizedPath`] never contains them.
pub(crate) fn classify_path_segments<'a, I>(
    segments: I,
    exclude: &ExcludePatterns,
) -> Result<Option<CandidatePath>, DepthExceededError>
where
    I: IntoIterator<Item = &'a [u8]>,
{
    let segments: Vec<&'a [u8]> = segments
        .into_iter()
        .filter(|seg| !seg.is_empty() && *seg != b".")
        .collect();
    let Some((leaf, ancestors)) = segments.split_last() else {
        return Ok(None);
    };
    let mut dir = AdmittedDir::root();
    for name in ancestors {
        let Some(admitted) = admit_dir(dir, name, exclude) else {
            return Ok(None);
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

/// Output prefix of an admitted directory. Both variants hold the valid prefix ending in `/`, or empty at
/// the root, so a path below extends it with a bare segment. `NonUtf8` holds the prefix before the first
/// non-UTF-8 segment, which every path below reports as its unstorable prefix.
#[derive(Clone, Debug, PartialEq, Eq)]
enum DirPrefix {
    Utf8(String),
    NonUtf8(String),
}

impl AdmittedDir {
    /// The source root, which has no ancestors to check.
    pub(crate) const fn root() -> Self {
        Self {
            depth: 0,
            match_prefix: String::new(),
            output: DirPrefix::Utf8(String::new()),
        }
    }
}

/// Admits directory `name` under `dir`. Returns `None` when it is hidden or user-excluded.
///
/// Consumes `dir` and returns the admitted child. Each prefix is extended in place, so admitting one
/// directory costs time linear in the bytes it appends. Callers that keep `dir` clone it first.
///
/// Only the new prefix is matched. The prefixes of `dir`'s ancestors were matched when `dir` was
/// admitted, and a match there would have returned `None` already. Matching stops after the first
/// `MAX_PATH_DEPTH + 1` segments: deeper, the match prefix stops growing and is not matched. The
/// output prefix stops growing at the same depth, because [`classify_leaf`] fails the depth limit
/// before it reads it.
#[must_use]
pub(crate) fn admit_dir(
    dir: AdmittedDir,
    name: &[u8],
    exclude: &ExcludePatterns,
) -> Option<AdmittedDir> {
    let AdmittedDir {
        depth,
        mut match_prefix,
        output,
    } = dir;
    if is_hidden_segment(name) {
        return None;
    }
    if depth <= MAX_PATH_DEPTH {
        match_prefix = push_match_segment(match_prefix, name);
        if exclude.excludes_prefix(&match_prefix, true) {
            return None;
        }
    }
    let output = match output {
        DirPrefix::Utf8(mut prefix) if depth <= MAX_PATH_DEPTH => match std::str::from_utf8(name) {
            Ok(segment) => {
                prefix.push_str(segment);
                prefix.push('/');
                DirPrefix::Utf8(prefix)
            }
            Err(_) => DirPrefix::NonUtf8(prefix),
        },
        unchanged => unchanged,
    };
    Some(AdmittedDir {
        depth: depth + 1,
        match_prefix,
        output,
    })
}

/// Classifies file `name` directly under `parent`. The checks on `parent`'s ancestors already ran in
/// [`admit_dir`], so only the file itself is matched here.
///
/// Check order: hidden segment, user exclusion, noise or sensitive name, depth, then non-UTF-8 and
/// length prefixing.
pub(crate) fn classify_leaf(
    parent: &AdmittedDir,
    name: &[u8],
    exclude: &ExcludePatterns,
) -> Result<Option<CandidatePath>, DepthExceededError> {
    if is_hidden_segment(name) {
        return Ok(None);
    }
    if parent.depth <= MAX_PATH_DEPTH {
        let full = push_match_segment(parent.match_prefix.clone(), name);
        if exclude.excludes_prefix(&full, false) {
            return Ok(None);
        }
    }

    // Lossy decoding keeps the ASCII name rules exact: invalid bytes become U+FFFD, which never
    // equals a noise name, and prefix and extension matches see the same ASCII bytes.
    if is_ignored_filename(&String::from_utf8_lossy(name)) {
        return Ok(None);
    }

    if parent.depth > MAX_PATH_DEPTH {
        return Err(DepthExceededError);
    }

    match (&parent.output, std::str::from_utf8(name)) {
        (DirPrefix::Utf8(prefix), Ok(segment))
            if prefix.len() + segment.len() <= MAX_PATH_BYTES_USIZE =>
        {
            Ok(Some(CandidatePath::Valid(NormalizedPath(format!(
                "{prefix}{segment}"
            )))))
        }
        (DirPrefix::Utf8(prefix) | DirPrefix::NonUtf8(prefix), _) => {
            Ok(Some(CandidatePath::Unstorable(prefix.clone())))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_exceeded_error_names_its_entry_kind() {
        let err = DepthExceededError::domain_error("Tar entry path");
        assert!(
            matches!(&err, DomainError::InvalidMetadata(msg) if msg == "Tar entry path exceeds maximum directory depth"),
            "{err:?}"
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
            Ok(None)
        );
        // Without the exclusion the path is over `MAX_PATH_BYTES`, so it is unstorable (I-10).
        assert!(matches!(
            classify_path_segments(segments.iter().copied(), &ExcludePatterns::default()),
            Ok(Some(CandidatePath::Unstorable(_)))
        ));
    }

    #[test]
    fn leaf_over_max_path_bytes_is_unstorable_with_its_directory_prefix() {
        // I-10: a `Valid` path is at most `MAX_PATH_BYTES` bytes. One byte more is `Unstorable`
        // and carries the directory prefix, like a non-UTF-8 path.
        for prefix in ["", "src/"] {
            let classify = |total: usize| {
                let leaf = "f".repeat(total - prefix.len());
                let mut segments: Vec<&[u8]> = Vec::new();
                if !prefix.is_empty() {
                    segments.push(b"src");
                }
                segments.push(leaf.as_bytes());
                classify_path_segments(segments, &ExcludePatterns::default())
            };
            assert!(
                matches!(
                    classify(MAX_PATH_BYTES_USIZE),
                    Ok(Some(CandidatePath::Valid(path))) if path.len() == MAX_PATH_BYTES_USIZE
                ),
                "prefix {prefix:?}: at the limit"
            );
            assert_eq!(
                classify(MAX_PATH_BYTES_USIZE + 1),
                Ok(Some(CandidatePath::Unstorable(prefix.to_string()))),
                "prefix {prefix:?}: one byte over the limit"
            );
        }
    }

    #[test]
    fn classify_path_segments_drops_empty_and_dot_segments() {
        let cases: [(&[&[u8]], Option<CandidatePath>); 5] = [
            (&[b"a", b"", b"b"], Some(CandidatePath::Valid("a/b".into()))),
            (
                &[b"a", b".", b"b"],
                Some(CandidatePath::Valid("a/b".into())),
            ),
            (&[b"", b""], None),
            (&[b"."], None),
            (&[b"a", b"", b""], Some(CandidatePath::Valid("a".into()))),
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
        assert!(admit_dir(root.clone(), b"build", &exclude).is_none());
        assert!(admit_dir(root.clone(), b".git", &exclude).is_none());
        assert!(admit_dir(root, b"src", &exclude).is_some());

        // A match past the first `MAX_PATH_DEPTH + 1` directories is not seen, as in the file check.
        let mut deep = AdmittedDir::root();
        for _ in 0..=MAX_PATH_DEPTH {
            deep = admit_dir(deep, b"d", &exclude).expect("d is not excluded");
        }
        assert!(admit_dir(deep.clone(), b"build", &exclude).is_some());
        assert!(
            admit_dir(deep, b".git", &exclude).is_none(),
            "hidden names are checked at every depth"
        );
    }

    #[test]
    fn admit_dir_stops_growing_match_prefix_past_window() {
        let exclude = ExcludePatterns::default();
        let mut dir = AdmittedDir::root();
        for _ in 0..=MAX_PATH_DEPTH {
            dir = admit_dir(dir, b"d", &exclude).expect("d is not excluded");
        }
        let window_len = dir.match_prefix.len();
        let depth = dir.depth;
        for _ in 0..8 {
            dir = admit_dir(dir, b"d", &exclude).expect("d is not excluded");
        }
        assert_eq!(
            dir.match_prefix.len(),
            window_len,
            "the match prefix grows only inside the window"
        );
        assert_eq!(dir.depth, depth + 8);
    }

    #[test]
    fn admit_dir_stops_growing_output_prefix_past_window() {
        let exclude = ExcludePatterns::default();
        let mut dir = AdmittedDir::root();
        for _ in 0..=MAX_PATH_DEPTH {
            dir = admit_dir(dir, b"d", &exclude).expect("d is not excluded");
        }
        let window_output = dir.output.clone();
        for _ in 0..8 {
            dir = admit_dir(dir, b"d", &exclude).expect("d is not excluded");
        }
        assert_eq!(
            dir.output, window_output,
            "no leaf below the window reads the output prefix"
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
        let valid = |p: &str| -> Result<Option<CandidatePath>, DepthExceededError> {
            Ok(Some(CandidatePath::Valid(p.into())))
        };
        let nonutf8 = |p: &str| -> Result<Option<CandidatePath>, DepthExceededError> {
            Ok(Some(CandidatePath::Unstorable(p.into())))
        };
        let ignored = || -> Result<Option<CandidatePath>, DepthExceededError> { Ok(None) };
        let depth_err =
            || -> Result<Option<CandidatePath>, DepthExceededError> { Err(DepthExceededError) };

        let exclude = ExcludePatterns::new(["build/", "*.log", "/top"]).unwrap();
        let none = ExcludePatterns::default();
        let max = MAX_PATH_DEPTH;

        let mut build_past_window = deep(max + 1, b"build");
        build_past_window.push(b"x.txt");
        let mut hidden_past_window = deep(max + 1, b".h");
        hidden_past_window.push(b"x");
        let max_valid = format!("{}file.txt", "d/".repeat(max));
        let hidden_deep: Vec<&[u8]> = std::iter::once(b".hidden".as_slice())
            .chain(std::iter::repeat_n(b"d".as_slice(), 300))
            .chain(std::iter::once(b"file.txt".as_slice()))
            .collect();
        let mut excluded_past_depth = vec![b"build".as_slice()];
        excluded_past_depth.extend(deep(max + 1, b"file.txt"));

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
            (segs(&[b"a", b"b", b"c.rs"]), &none, valid("a/b/c.rs")),
            (
                segs(&[b"a", b"b", b"\xff", b"x.rs"]),
                &none,
                nonutf8("a/b/"),
            ),
            (segs(&[b"a", b"\xff", b"b", b"c.rs"]), &none, nonutf8("a/")),
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
            (hidden_deep, &none, ignored()),
            (deep(max + 1, b"Cargo.lock"), &none, ignored()),
            (excluded_past_depth.clone(), &exclude, ignored()),
            (excluded_past_depth, &none, depth_err()),
            // A sensitive name is ignored even when its final segment is not UTF-8.
            (segs(&[b"dir", b"id_rsa\xff.key"]), &none, ignored()),
            (segs(&[b"secret\xff.key"]), &none, ignored()),
            (segs(&[b"dir", b"credentials\xff"]), &none, ignored()),
            (segs(&[b"notes\xff.txt"]), &none, nonutf8("")),
            // Noise names match exactly: an invalid byte makes `Cargo.lock` a different name.
            (segs(&[b"Cargo.lock\xff"]), &none, nonutf8("")),
        ];
        for (segments, exclude, expected) in cases {
            assert_eq!(
                classify_path_segments(segments.iter().copied(), exclude),
                expected,
                "segments: {segments:?}"
            );
        }
    }

    mod properties {
        use super::*;
        use proptest::prelude::*;

        const POOL: &[&[u8]] = &[
            b"",
            b".",
            b"..",
            b".hidden",
            b"a",
            b"src",
            b"main.rs",
            b"id_rsa",
            b"Cargo.lock",
            b"\xff",
        ];

        proptest! {
            #![proptest_config(crate::core::proptest_config())]

            /// A valid candidate is the `/`-join of the input segments once empty and `.` segments are
            /// dropped, so it never has an empty segment, a leading slash, or a trailing slash. A
            /// non-UTF-8 candidate carries a prefix that is empty or ends in a single `/`.
            #[test]
            fn classified_paths_are_normalized(
                segments in prop::collection::vec(prop::sample::select(POOL), 0..8)
            ) {
                let kept: Vec<&[u8]> = segments
                    .iter()
                    .copied()
                    .filter(|seg| !seg.is_empty() && *seg != b".")
                    .collect();
                let result =
                    classify_path_segments(segments.iter().copied(), &ExcludePatterns::default());
                match result {
                    Ok(Some(CandidatePath::Valid(path))) => {
                        let expected: Vec<&str> = kept
                            .iter()
                            .map(|seg| std::str::from_utf8(seg).expect("valid candidates are UTF-8"))
                            .collect();
                        prop_assert_eq!(&*path, expected.join("/"));
                        prop_assert!(!path.is_empty());
                        prop_assert!(path.split('/').all(|seg| !seg.is_empty() && seg != "."));
                    }
                    Ok(Some(CandidatePath::Unstorable(prefix))) => {
                        prop_assert!(prefix.is_empty() || prefix.ends_with('/'));
                        prop_assert!(!prefix.contains("//") && !prefix.starts_with('/'));
                    }
                    Ok(None) => {}
                    Err(DepthExceededError) => {
                        prop_assert!(false, "{} segments cannot exceed MAX_PATH_DEPTH", kept.len());
                    }
                }
            }
        }
    }
}
