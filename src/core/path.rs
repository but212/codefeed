//! Relative path normalization, depth enforcement, and segment classification.

use crate::core::error::DomainError;
use crate::core::policy::{
    ExcludePatterns, MAX_PATH_BYTES, MAX_PATH_DEPTH, is_hidden_segment, is_ignored_filename,
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
            s.len() <= MAX_PATH_BYTES,
            "NormalizedPath requires at most MAX_PATH_BYTES bytes"
        );
        Self(s)
    }
}

/// Candidate path outcome from classification, optionally carrying a read body (I-11).
///
/// An entry is either a storable path (with `()` at classification, a local path for dispatch, or `Body<B>` at observation)
/// or an unstorable candidate carrying its valid directory prefix and raw byte identity.
/// Ignored entries (hidden, conventional noise, sensitive patterns, or empty/root) are represented by `None`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Candidate<B = ()> {
    /// Normalized relative UTF-8 path within `MAX_PATH_BYTES`.
    Valid(NormalizedPath, B),
    /// Path that is not hidden but cannot be stored, because it is non-UTF-8 or longer than
    /// `MAX_PATH_BYTES`. It is counted against the candidate budget as skipped.
    Unstorable(UnstorablePath),
}

/// Unstorable candidate with an accounting prefix and its complete normalized byte identity.
/// Only classification constructs it, preserving non-UTF-8 segments without lossy collisions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UnstorablePath {
    prefix: String,
    identity: Vec<u8>,
}

impl UnstorablePath {
    #[must_use]
    pub(crate) fn prefix(&self) -> &str {
        &self.prefix
    }

    #[must_use]
    pub(crate) fn into_identity(self) -> Vec<u8> {
        self.identity
    }
}

#[cfg(test)]
impl From<(&str, &[u8])> for UnstorablePath {
    fn from((prefix, identity): (&str, &[u8])) -> Self {
        Self {
            prefix: prefix.into(),
            identity: identity.into(),
        }
    }
}

#[cold]
fn depth_exceeded_error(subject: &str) -> DomainError {
    DomainError::invalid_metadata(format!("{subject} exceeds maximum directory depth"))
}

/// Classifies a sequence of raw path segment bytes into [`Option<Candidate>`].
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
    subject: &str,
) -> Result<Option<Candidate>, DomainError>
where
    I: IntoIterator<Item = &'a [u8]>,
{
    let mut filtered = segments
        .into_iter()
        .filter(|seg| !seg.is_empty() && *seg != b".");
    let Some(mut pending) = filtered.next() else {
        return Ok(None);
    };
    let mut dir = AdmittedDir::root();
    for next in filtered {
        let Some(admitted) = admit_dir(dir, pending, exclude) else {
            return Ok(None);
        };
        dir = admitted;
        pending = next;
    }
    classify_leaf(&dir, pending, exclude, subject)
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

/// Output prefix of an admitted directory, ending in `/` or empty at the root.
/// `NonUtf8` retains both the valid prefix and all raw segments for candidate identity.
#[derive(Clone, Debug, PartialEq, Eq)]
enum DirPrefix {
    Utf8(String),
    NonUtf8(UnstorablePath),
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

    /// Enforces the depth budget after hidden and exclusion checks have run.
    pub(crate) fn check_depth(&self, subject: &str) -> Result<(), DomainError> {
        if self.depth > MAX_PATH_DEPTH {
            return Err(depth_exceeded_error(subject));
        }
        Ok(())
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
    if !exclude.is_empty() && depth <= MAX_PATH_DEPTH {
        match_prefix = push_match_segment(match_prefix, name);
        if exclude.excludes_prefix(&match_prefix, true) {
            return None;
        }
    }
    let output = match output {
        DirPrefix::Utf8(mut prefix) if depth <= MAX_PATH_DEPTH => {
            if let Ok(segment) = std::str::from_utf8(name) {
                prefix.push_str(segment);
                prefix.push('/');
                DirPrefix::Utf8(prefix)
            } else {
                let mut identity = prefix.as_bytes().to_vec();
                identity.extend_from_slice(name);
                identity.push(b'/');
                DirPrefix::NonUtf8(UnstorablePath { prefix, identity })
            }
        }
        DirPrefix::NonUtf8(mut unstorable) if depth <= MAX_PATH_DEPTH => {
            unstorable.identity.extend_from_slice(name);
            unstorable.identity.push(b'/');
            DirPrefix::NonUtf8(unstorable)
        }
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
    subject: &str,
) -> Result<Option<Candidate>, DomainError> {
    if is_hidden_segment(name) {
        return Ok(None);
    }
    if !exclude.is_empty() && parent.depth <= MAX_PATH_DEPTH {
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

    parent.check_depth(subject)?;

    match (&parent.output, std::str::from_utf8(name)) {
        (DirPrefix::Utf8(prefix), Ok(segment))
            if prefix.len() + segment.len() <= MAX_PATH_BYTES =>
        {
            Ok(Some(Candidate::Valid(
                NormalizedPath(format!("{prefix}{segment}")),
                (),
            )))
        }
        (output, _) => {
            let mut unstorable = match output {
                DirPrefix::Utf8(prefix) => UnstorablePath {
                    prefix: prefix.clone(),
                    identity: prefix.as_bytes().to_vec(),
                },
                DirPrefix::NonUtf8(unstorable) => unstorable.clone(),
            };
            unstorable.identity.extend_from_slice(name);
            Ok(Some(Candidate::Unstorable(unstorable)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_exceeded_error_names_its_entry_kind() {
        let err = depth_exceeded_error("Tar entry path");
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
        assert!(matches!(
            classify_path_segments(segments.iter().copied(), &exclude, "test path"),
            Ok(None)
        ));
        // Without the exclusion the path is over `MAX_PATH_BYTES`, so it is unstorable (I-10).
        assert!(matches!(
            classify_path_segments(
                segments.iter().copied(),
                &ExcludePatterns::default(),
                "test path"
            ),
            Ok(Some(Candidate::Unstorable(_)))
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
                classify_path_segments(segments, &ExcludePatterns::default(), "test path")
            };
            assert!(
                matches!(
                    classify(MAX_PATH_BYTES),
                    Ok(Some(Candidate::Valid(path, ()))) if path.len() == MAX_PATH_BYTES
                ),
                "prefix {prefix:?}: at the limit"
            );
            let over = classify(MAX_PATH_BYTES + 1).expect("within depth limit");
            assert_eq!(
                over,
                Some(Candidate::Unstorable(
                    (
                        prefix,
                        format!("{prefix}{}", "f".repeat(MAX_PATH_BYTES + 1 - prefix.len()))
                            .as_bytes(),
                    )
                        .into()
                )),
                "prefix {prefix:?}: one byte over the limit"
            );
        }
    }

    #[test]
    fn classify_path_segments_drops_empty_and_dot_segments() {
        let cases: [(&[&[u8]], Option<Candidate>); 5] = [
            (&[b"a", b"", b"b"], Some(Candidate::Valid("a/b".into(), ()))),
            (
                &[b"a", b".", b"b"],
                Some(Candidate::Valid("a/b".into(), ())),
            ),
            (&[b"", b""], None),
            (&[b"."], None),
            (&[b"a", b"", b""], Some(Candidate::Valid("a".into(), ()))),
        ];
        for (segments, expected) in cases {
            let actual = classify_path_segments(
                segments.iter().copied(),
                &ExcludePatterns::default(),
                "test path",
            )
            .expect("within depth limit");
            assert_eq!(actual, expected, "segments: {segments:?}");
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
    #[allow(clippy::too_many_lines)]
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
            |p: &str| -> Result<Option<Candidate>, ()> { Ok(Some(Candidate::Valid(p.into(), ()))) };
        let nonutf8 = |prefix: &str, identity: &[u8]| -> Result<Option<Candidate>, ()> {
            Ok(Some(Candidate::Unstorable((prefix, identity).into())))
        };
        let ignored = || -> Result<Option<Candidate>, ()> { Ok(None) };
        let depth_err = || -> Result<Option<Candidate>, ()> { Err(()) };

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
                nonutf8("src/", b"src/\xff\xfe/main.rs"),
            ),
            (
                segs(&[b"\xff", b"x.rs"]),
                &exclude,
                nonutf8("", b"\xff/x.rs"),
            ),
            (
                segs(&[b"src", b"\xff.rs"]),
                &exclude,
                nonutf8("src/", b"src/\xff.rs"),
            ),
            (segs(&[b"a", b"b", b"c.rs"]), &none, valid("a/b/c.rs")),
            (
                segs(&[b"a", b"b", b"\xff", b"x.rs"]),
                &none,
                nonutf8("a/b/", b"a/b/\xff/x.rs"),
            ),
            (
                segs(&[b"a", b"\xff", b"b", b"c.rs"]),
                &none,
                nonutf8("a/", b"a/\xff/b/c.rs"),
            ),
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
            (
                segs(&[b"notes\xff.txt"]),
                &none,
                nonutf8("", b"notes\xff.txt"),
            ),
            // Noise names match exactly: an invalid byte makes `Cargo.lock` a different name.
            (
                segs(&[b"Cargo.lock\xff"]),
                &none,
                nonutf8("", b"Cargo.lock\xff"),
            ),
        ];
        for (segments, exclude, expected) in cases {
            let actual = classify_path_segments(segments.iter().copied(), exclude, "test path")
                .map_err(|err| assert!(matches!(err, DomainError::InvalidMetadata(_))));
            assert_eq!(actual, expected, "segments: {segments:?}");
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
                let result = classify_path_segments(
                    segments.iter().copied(),
                    &ExcludePatterns::default(),
                    "test path",
                );
                match result {
                    Ok(Some(Candidate::Valid(path, ()))) => {
                        let expected: Vec<&str> = kept
                            .iter()
                            .map(|seg| std::str::from_utf8(seg).expect("valid candidates are UTF-8"))
                            .collect();
                        prop_assert_eq!(&*path, expected.join("/"));
                        prop_assert!(!path.is_empty());
                        prop_assert!(path.split('/').all(|seg| !seg.is_empty() && seg != "."));
                    }
                    Ok(Some(Candidate::Unstorable(path))) => {
                        let prefix = path.prefix();
                        prop_assert!(prefix.is_empty() || prefix.ends_with('/'));
                        prop_assert!(!prefix.contains("//") && !prefix.starts_with('/'));
                        prop_assert_eq!(path.into_identity(), kept.join(&b'/'));
                    }
                    Ok(None) => {}
                    Err(_) => {
                        prop_assert!(false, "{} segments cannot exceed MAX_PATH_DEPTH", kept.len());
                    }
                }
            }
        }
    }
}
