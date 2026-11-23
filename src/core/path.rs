//! Relative path normalization, depth enforcement, and segment classification.

use memchr::memchr;

use crate::core::error::DomainError;
use crate::core::policy::{
    ExcludePatterns, MAX_PATH_BYTES, MAX_PATH_DEPTH, is_hidden_segment, is_ignored_filename,
};

/// Non-empty, normalized slash-separated relative UTF-8 path of at most `MAX_PATH_BYTES` bytes (I-10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NormalizedPath(String);

impl NormalizedPath {
    /// Consumes the path and returns its underlying UTF-8 bytes without allocation.
    #[must_use]
    pub(crate) fn into_bytes(self) -> Vec<u8> {
        self.0.into_bytes()
    }

    /// Budget directory key for `self` (I-02, I-05): `NormalizedPath` is already UTF-8 and
    /// `<= MAX_PATH_BYTES` (I-10), so only the first segment or `"./"` is needed.
    #[must_use]
    pub(crate) fn budget_dir(&self) -> BudgetDirKey<'_> {
        let key = memchr(b'/', self.0.as_bytes()).map_or("./", |index| &self.0[..=index]);
        BudgetDirKey(key)
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

/// Budget directory key: first path segment with trailing `/` capped at `MAX_PATH_BYTES` on a
/// UTF-8 char boundary (I-05), or `"./"` for root-level or non-UTF-8-first-segment entries (I-02).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BudgetDirKey<'a>(&'a str);

impl<'a> BudgetDirKey<'a> {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'a str {
        self.0
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn from_raw(path: &'a str) -> Self {
        budget_dir_from_bytes(path.as_bytes())
    }
}

impl std::ops::Deref for BudgetDirKey<'_> {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

/// Extracts a bounded UTF-8 first-segment key from normalized raw path bytes, falling back to `"./"`.
fn budget_dir_from_bytes(bytes: &[u8]) -> BudgetDirKey<'_> {
    let Some(slash) = memchr(b'/', bytes) else {
        return BudgetDirKey("./");
    };
    let Ok(dir) = std::str::from_utf8(&bytes[..=slash]) else {
        return BudgetDirKey("./");
    };
    let mut end = dir.len().min(MAX_PATH_BYTES);
    while !dir.is_char_boundary(end) {
        end -= 1;
    }
    BudgetDirKey(&dir[..end])
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

/// Unstorable candidate carrying its complete normalized raw-byte identity.
///
/// Only classification constructs it, preserving non-UTF-8 and overlong paths without lossy collisions.
/// Its accounting prefix for candidate budgeting is derived from the first segment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UnstorablePath(Vec<u8>);

impl UnstorablePath {
    /// Bounded first-segment UTF-8 prefix for candidate budget accounting (I-02, I-05).
    #[must_use]
    pub(crate) fn budget_dir(&self) -> BudgetDirKey<'_> {
        budget_dir_from_bytes(&self.0)
    }

    #[must_use]
    pub(crate) fn into_identity(self) -> Vec<u8> {
        self.0
    }
}

#[cfg(test)]
impl From<&[u8]> for UnstorablePath {
    fn from(identity: &[u8]) -> Self {
        Self(identity.to_vec())
    }
}

/// Normalizes a raw tar entry path (strips root prefix, collapses slashes/`.`s) and classifies it.
///
/// User exclusions match relative to the stripped root. Returns `Err(DomainError)` on traversal,
/// invalid root prefix, or NUL; `Ok(None)` if root/empty/ignored; `Candidate::Unstorable` if
/// non-UTF-8 or over `MAX_PATH_BYTES` (and not hidden); `Ok(Some(Candidate::Valid(path, ())))` otherwise.
pub(crate) fn normalize_tar_entry_path(
    raw_bytes: &[u8],
    exclude: &ExcludePatterns,
) -> Result<Option<Candidate>, DomainError> {
    if memchr(0, raw_bytes).is_some() {
        return Err(DomainError::invalid_metadata(
            "Tar entry path contains NUL byte",
        ));
    }
    let start = raw_bytes
        .iter()
        .position(|&b| b != b'/')
        .unwrap_or(raw_bytes.len());
    let end = raw_bytes
        .iter()
        .rposition(|&b| b != b'/')
        .map_or(start, |i| i + 1);
    let trimmed = &raw_bytes[start..end];

    if trimmed.split(|&b| b == b'/').any(|seg| seg == b"..") {
        return Err(DomainError::invalid_metadata(
            "Tar entry contains directory traversal (..)",
        ));
    }

    let Some(slash_index) = memchr(b'/', trimmed) else {
        return Ok(None);
    };

    if trimmed[..slash_index] == *b"." {
        return Err(DomainError::invalid_metadata(
            "Tar entry root segment cannot be '.'",
        ));
    }

    let rel = &trimmed[slash_index + 1..];

    classify_path_segments(rel.split(|&b| b == b'/'), exclude, "Tar entry path")
}

/// Classifies raw path segment bytes into [`Option<Candidate>`] after dropping empty and `.` segments.
///
/// Check order per segment: hidden, user exclusion, depth (`MAX_PATH_DEPTH`), then noise/sensitive
/// on the leaf. Produces a [`NormalizedPath`] or an [`UnstorablePath`].
fn classify_path_segments<'a, I>(
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
        let Some(admitted) = admit_dir(dir, pending, exclude, subject)? else {
            return Ok(None);
        };
        dir = admitted;
        pending = next;
    }
    Ok(classify_leaf(&dir, pending, exclude))
}

/// A directory that passed hidden, user-exclusion, and depth (`<= MAX_PATH_DEPTH`) checks.
///
/// Built only by [`AdmittedDir::root`] and [`admit_dir`], so holding one guarantees every ancestor
/// was checked and `depth <= MAX_PATH_DEPTH`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AdmittedDir {
    depth: usize,
    match_prefix: String,
    output: DirPrefix,
}

/// Output prefix of an admitted directory, ending in `/` or empty at the root.
///
/// Transitions monotonically to `Unstorable` on the first non-UTF-8 segment or when exceeding
/// `MAX_PATH_BYTES`, retaining raw bytes for lossless candidate identity.
#[derive(Clone, Debug, PartialEq, Eq)]
enum DirPrefix {
    Utf8(String),
    Unstorable(Vec<u8>),
}

impl DirPrefix {
    /// Extends the prefix with directory segment `name`, transitioning to `Unstorable` on invalid
    /// UTF-8 or when the prefix would exceed `MAX_PATH_BYTES`.
    #[must_use]
    fn advance(self, name: &[u8]) -> Self {
        let mut bytes = match self {
            Self::Utf8(mut prefix) => {
                if let Ok(segment) = std::str::from_utf8(name)
                    && prefix.len() + segment.len() < MAX_PATH_BYTES
                {
                    prefix.push_str(segment);
                    prefix.push('/');
                    return Self::Utf8(prefix);
                }
                prefix.into_bytes()
            }
            Self::Unstorable(bytes) => bytes,
        };
        bytes.extend_from_slice(name);
        bytes.push(b'/');
        Self::Unstorable(bytes)
    }

    /// Converts the prefix into an [`UnstorablePath`] terminated by `name`.
    #[must_use]
    fn into_unstorable(self, name: &[u8]) -> UnstorablePath {
        let mut identity = match self {
            Self::Utf8(prefix) => prefix.into_bytes(),
            Self::Unstorable(bytes) => bytes,
        };
        identity.extend_from_slice(name);
        UnstorablePath(identity)
    }
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

/// Admits directory `name` under `dir`, returning `Ok(None)` if hidden or user-excluded, or
/// `Err(DomainError::InvalidMetadata)` if entering it would exceed `MAX_PATH_DEPTH`.
///
/// Extends `dir`'s prefixes in place (O(len(name))).
pub(crate) fn admit_dir(
    dir: AdmittedDir,
    name: &[u8],
    exclude: &ExcludePatterns,
    subject: &str,
) -> Result<Option<AdmittedDir>, DomainError> {
    let AdmittedDir {
        depth,
        match_prefix,
        output,
    } = dir;
    if is_hidden_segment(name) {
        return Ok(None);
    }
    let Some(match_prefix) = exclude.advance(match_prefix, name) else {
        return Ok(None);
    };
    let next_depth = depth + 1;
    if next_depth > MAX_PATH_DEPTH {
        return Err(DomainError::invalid_metadata(format!(
            "{subject} exceeds maximum directory depth"
        )));
    }
    Ok(Some(AdmittedDir {
        depth: next_depth,
        match_prefix,
        output: output.advance(name),
    }))
}

/// Classifies file `name` directly under `parent`. The checks on `parent`'s ancestors (including
/// `MAX_PATH_DEPTH`) already ran in [`admit_dir`], so only the file itself is classified here.
///
/// Check order: hidden segment, user exclusion, noise or sensitive name, then UTF-8 and
/// `MAX_PATH_BYTES` check.
#[must_use]
pub(crate) fn classify_leaf(
    parent: &AdmittedDir,
    name: &[u8],
    exclude: &ExcludePatterns,
) -> Option<Candidate> {
    if is_hidden_segment(name) {
        return None;
    }
    if exclude.excludes_leaf(&parent.match_prefix, name) {
        return None;
    }
    if is_ignored_filename(name) {
        return None;
    }

    if let (DirPrefix::Utf8(prefix), Ok(segment)) = (&parent.output, std::str::from_utf8(name))
        && prefix.len() + segment.len() <= MAX_PATH_BYTES
    {
        return Some(Candidate::Valid(
            NormalizedPath(format!("{prefix}{segment}")),
            (),
        ));
    }

    Some(Candidate::Unstorable(
        parent.output.clone().into_unstorable(name),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

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
            let Some(Candidate::Unstorable(over)) = classify(MAX_PATH_BYTES + 1).unwrap() else {
                panic!("prefix {prefix:?}: one byte over the limit should be unstorable");
            };
            let expected_budget = if prefix.is_empty() { "./" } else { prefix };
            assert_eq!(over.budget_dir().as_str(), expected_budget);
            let expected_identity =
                format!("{prefix}{}", "f".repeat(MAX_PATH_BYTES + 1 - prefix.len()));
            assert_eq!(over.into_identity(), expected_identity.into_bytes());
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
    fn admit_dir_enforces_exclusions_before_depth_limit() {
        let exclude = ExcludePatterns::new(["build/"]).unwrap();
        let root = AdmittedDir::root();
        assert!(matches!(
            admit_dir(root.clone(), b"build", &exclude, "test path"),
            Ok(None)
        ));
        assert!(matches!(
            admit_dir(root.clone(), b".git", &exclude, "test path"),
            Ok(None)
        ));
        assert!(matches!(
            admit_dir(root, b"src", &exclude, "test path"),
            Ok(Some(_))
        ));

        // At depth MAX_PATH_DEPTH, hidden and excluded directories are still ignored before the
        // depth limit fails, while any admitted directory is rejected.
        let mut deep = AdmittedDir::root();
        for _ in 0..MAX_PATH_DEPTH {
            deep = admit_dir(deep, b"d", &exclude, "test path")
                .expect("within depth limit")
                .expect("d is not excluded");
        }
        assert!(matches!(
            admit_dir(deep.clone(), b"build", &exclude, "test path"),
            Ok(None)
        ));
        assert!(matches!(
            admit_dir(deep.clone(), b".git", &exclude, "test path"),
            Ok(None)
        ));
        assert!(matches!(
            admit_dir(deep, b"extra", &exclude, "test path"),
            Err(DomainError::InvalidMetadata(_))
        ));
    }

    #[test]
    fn dir_prefix_transitions_to_unstorable_past_max_path_bytes() {
        let exclude = ExcludePatterns::default();
        let long_dir = "d".repeat(MAX_PATH_BYTES);
        let dir = admit_dir(
            AdmittedDir::root(),
            long_dir.as_bytes(),
            &exclude,
            "test path",
        )
        .expect("within depth limit")
        .expect("not excluded");
        assert!(matches!(dir.output, DirPrefix::Unstorable(_)));
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
        let nonutf8 = |identity: &[u8]| -> Result<Option<Candidate>, ()> {
            Ok(Some(Candidate::Unstorable(identity.into())))
        };
        let ignored = || -> Result<Option<Candidate>, ()> { Ok(None) };
        let depth_err = || -> Result<Option<Candidate>, ()> { Err(()) };

        let exclude = ExcludePatterns::new(["build/", "*.log", "/top"]).unwrap();
        let none = ExcludePatterns::default();
        let max = MAX_PATH_DEPTH;

        let mut build_past_depth = deep(max, b"build");
        build_past_depth.push(b"x.txt");
        let mut hidden_past_depth = deep(max, b".h");
        hidden_past_depth.push(b"x");
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
                nonutf8(b"src/\xff\xfe/main.rs"),
            ),
            (segs(&[b"\xff", b"x.rs"]), &exclude, nonutf8(b"\xff/x.rs")),
            (
                segs(&[b"src", b"\xff.rs"]),
                &exclude,
                nonutf8(b"src/\xff.rs"),
            ),
            (segs(&[b"a", b"b", b"c.rs"]), &none, valid("a/b/c.rs")),
            (
                segs(&[b"a", b"b", b"\xff", b"x.rs"]),
                &none,
                nonutf8(b"a/b/\xff/x.rs"),
            ),
            (
                segs(&[b"a", b"\xff", b"b", b"c.rs"]),
                &none,
                nonutf8(b"a/\xff/b/c.rs"),
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
            (deep(max + 1, b"Cargo.lock"), &none, depth_err()),
            (build_past_depth, &exclude, ignored()),
            (hidden_past_depth, &none, ignored()),
            (hidden_deep, &none, ignored()),
            (excluded_past_depth.clone(), &exclude, ignored()),
            (excluded_past_depth, &none, depth_err()),
            // A sensitive name is ignored even when its final segment is not UTF-8.
            (segs(&[b"dir", b"id_rsa\xff.key"]), &none, ignored()),
            (segs(&[b"secret\xff.key"]), &none, ignored()),
            (segs(&[b"dir", b"credentials\xff"]), &none, ignored()),
            (segs(&[b"notes\xff.txt"]), &none, nonutf8(b"notes\xff.txt")),
            // Noise names match exactly: an invalid byte makes `Cargo.lock` a different name.
            (
                segs(&[b"Cargo.lock\xff"]),
                &none,
                nonutf8(b"Cargo.lock\xff"),
            ),
        ];
        for (segments, exclude, expected) in cases {
            let actual = classify_path_segments(segments.iter().copied(), exclude, "test path")
                .map_err(|err| assert!(matches!(err, DomainError::InvalidMetadata(_))));
            assert_eq!(actual, expected, "segments: {segments:?}");
        }
    }

    fn normalize(raw: &[u8]) -> Result<Option<Candidate>, DomainError> {
        normalize_tar_entry_path(raw, &ExcludePatterns::default())
    }

    #[test]
    fn user_exclusions_are_relative_to_the_archive_root() {
        let exclude = ExcludePatterns::new(["build/", "/src/", "repo-root"]).unwrap();
        let cases: [(&[u8], Option<Candidate>); 4] = [
            (b"repo-root/build/x.txt", None),
            (b"repo-root/src/main.rs", None),
            (
                b"repo-root/lib/src/x.rs",
                Some(Candidate::Valid("lib/src/x.rs".into(), ())),
            ),
            // The root directory itself is never matched, so the `repo-root` pattern keeps nothing out.
            (
                b"repo-root/lib/main.rs",
                Some(Candidate::Valid("lib/main.rs".into(), ())),
            ),
        ];
        for (raw, expected) in cases {
            assert_eq!(
                normalize_tar_entry_path(raw, &exclude).unwrap(),
                expected,
                "{raw:?}"
            );
        }
    }

    #[test]
    fn trailing_separators_never_change_the_classification() {
        let cases: [(&[u8], Option<Candidate>); 6] = [
            (
                b"repo-root/src/a.rs/",
                Some(Candidate::Valid("src/a.rs".into(), ())),
            ),
            (
                b"repo-root/src/a.rs///",
                Some(Candidate::Valid("src/a.rs".into(), ())),
            ),
            (b"repo-root/", None),
            (b"repo-root//", None),
            (b"///", None),
            (b"./", None),
        ];
        for (raw, expected) in cases {
            assert_eq!(normalize(raw).unwrap(), expected, "{raw:?}");
        }
    }

    #[test]
    fn normalizes_archive_entry_path_correctly() {
        // (raw entry path, expected status). Covers root stripping, slash and `.` collapsing,
        // policy exclusion (noise lockfile, sensitive credential), and non-UTF-8 prefixes.
        let ok_cases: [(&[u8], Option<Candidate>); 10] = [
            (
                b"but212-codefeed-c1863c8/src/main.rs",
                Some(Candidate::Valid("src/main.rs".into(), ())),
            ),
            (
                b"repo-root//src/main.rs",
                Some(Candidate::Valid("src/main.rs".into(), ())),
            ),
            (
                b"repo-root///deep//nested/./file.txt",
                Some(Candidate::Valid("deep/nested/file.txt".into(), ())),
            ),
            (
                b"repo-root/././a.rs",
                Some(Candidate::Valid("a.rs".into(), ())),
            ),
            (b"but212-codefeed-c1863c8/", None),
            (b"but212-codefeed-c1863c8///", None),
            (b"but212-codefeed-c1863c8", None),
            (b"", None),
            // Classification itself is pinned by `path::classification_corpus_is_stable`; one
            // ignored and one unstorable row show that the archive delegates to it.
            (b"repo-root/Cargo.lock", None),
            (
                b"repo-root/\xff\xfe",
                Some(Candidate::Unstorable(b"\xff\xfe".as_slice().into())),
            ),
        ];
        for (raw, expected) in ok_cases {
            assert_eq!(normalize(raw).unwrap(), expected, "{raw:?}");
        }

        // Rejected: path traversal (`..`, `.` root) and embedded NUL
        let err_cases: [&[u8]; 7] = [
            b"../etc/passwd",
            b"./etc/passwd",
            b"repo-root/../etc/passwd",
            b"repo-root/src/../../root",
            b"repo-root/.hidden/../etc/passwd",
            b"repo-root/certs/privkey.key\0",
            b"repo-root\0/src/main.rs",
        ];
        for raw in err_cases {
            assert!(normalize(raw).is_err(), "{raw:?}");
        }

        let dot_root_err = normalize(b"./etc/passwd").unwrap_err();
        assert_eq!(
            dot_root_err.to_string(),
            "invalid repository metadata: Tar entry root segment cannot be '.'"
        );
    }

    #[test]
    fn normalizes_archive_entry_depth_limit() {
        // The limit itself is pinned by `path::classification_corpus_is_stable`; this checks that
        // the archive reports a depth failure as invalid metadata naming the entry kind.
        let excessive_depth_path = format!(
            "repo-root/{}/file.txt",
            vec!["d"; MAX_PATH_DEPTH + 1].join("/")
        );
        let err = normalize(excessive_depth_path.as_bytes())
            .expect_err("one level past the limit must be rejected");
        assert!(
            err.to_string()
                .contains("Tar entry path exceeds maximum directory depth"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn dots_inside_a_name_are_not_traversal() {
        for (raw, expected) in [
            (&b"repo-root/a..b.txt"[..], "a..b.txt"),
            (b"repo-root/dir../x", "dir../x"),
        ] {
            assert_eq!(
                normalize(raw).unwrap(),
                Some(Candidate::Valid(expected.into(), ())),
                "{raw:?}"
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
                        let budget_dir = path.budget_dir();
                        prop_assert!(budget_dir.ends_with('/'));
                        prop_assert!(budget_dir.len() <= MAX_PATH_BYTES);
                        prop_assert!(!budget_dir.contains("//") && !budget_dir.starts_with('/'));
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
