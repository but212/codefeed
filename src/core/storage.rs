//! Continuous arena storage and zero-copy views for collected files.
//!
//! Eliminates per-file heap allocations by maintaining contiguous string arenas
//! for paths and contents, indexed by compact 16-byte span records kept private to this module.

use std::cmp::Ordering;
use std::iter::once;
use std::ops::Deref;

use crate::core::error::DomainError;
use crate::core::policy::{MAX_OUTPUT_SIZE, NormalizedPath, OVERFLOW_LARGEST_FILES};

/// 16-byte POD indexing a file path and content slice in [`FileStorage`].
///
/// Private to this module: a span carries no owner, so it is only resolved inside `FileStorage`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileSpan {
    path_offset: u32,
    path_len: u32,
    content_offset: u32,
    content_len: u32,
}

const _: () = assert!(core::mem::size_of::<FileSpan>() == 16);

impl FileSpan {
    /// Slices this span's path out of `arena`.
    ///
    /// # Panics
    ///
    /// Panics if the span's range is out of bounds for `arena`.
    fn path<'a>(&self, arena: &'a str) -> &'a str {
        let start = self.path_offset as usize;
        let end = start + self.path_len as usize;
        &arena[start..end]
    }

    /// Slices this span's content out of `arena`.
    ///
    /// # Panics
    ///
    /// Panics if the span's range is out of bounds for `arena`.
    fn content<'a>(&self, arena: &'a str) -> &'a str {
        let start = self.content_offset as usize;
        let end = start + self.content_len as usize;
        &arena[start..end]
    }
}

/// Zero-copy borrowed view into a file record stored within [`FileStorage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileRecord<'a> {
    pub(crate) path: &'a str,
    pub(crate) content: &'a str,
}

/// Wrapper guaranteeing that [`FileStorage`] is sorted component-wise lexicographically by path
/// and that no stored path is a directory prefix of another.
#[derive(Debug)]
#[cfg_attr(test, derive(Clone))]
#[must_use]
pub(crate) struct SortedFiles(FileStorage);

impl SortedFiles {
    /// Sorts `storage` by path, then checks that no stored path names a file and a directory at once.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidMetadata`] when one stored path is a directory prefix of another,
    /// since the digest tree would show the same entry as both a file and a directory.
    pub(crate) fn new(storage: FileStorage) -> Result<Self, DomainError> {
        let sorted = storage.sort_by_path();
        check_no_file_directory_collision(&sorted)?;
        Ok(Self(sorted))
    }
}

/// Checks adjacent pairs only. If `p` is a directory prefix of some `p/…` path, the element right
/// after `p` in `cmp_path` order also starts with `p/`, so the first colliding pair is always adjacent.
fn check_no_file_directory_collision(sorted: &FileStorage) -> Result<(), DomainError> {
    let mut paths = sorted.iter().map(|record| record.path);
    let Some(mut previous) = paths.next() else {
        return Ok(());
    };
    for next in paths {
        if next
            .strip_prefix(previous)
            .is_some_and(|rest| rest.starts_with('/'))
        {
            return Err(DomainError::invalid_metadata(format!(
                "entry path is both a file and a directory: {previous}"
            )));
        }
        previous = next;
    }
    Ok(())
}

/// Read-only view. `FileStorage` mutators consume `self`, so sortedness cannot be broken through this view.
impl Deref for SortedFiles {
    type Target = FileStorage;

    fn deref(&self) -> &FileStorage {
        &self.0
    }
}

/// Continuous arena storage for file paths and contents.
/// Eliminates per-file heap allocations and preserves cache locality.
#[derive(Debug)]
#[cfg_attr(test, derive(Clone, PartialEq, Eq))]
pub(crate) struct FileStorage {
    spans: Vec<FileSpan>,
    path_arena: String,
    content_arena: String,
}

impl FileStorage {
    pub(crate) fn new() -> Self {
        Self {
            spans: Vec::with_capacity(512),
            path_arena: String::with_capacity(16 * 1024),
            content_arena: String::with_capacity(256 * 1024),
        }
    }

    #[must_use]
    pub(crate) const fn len(&self) -> usize {
        self.spans.len()
    }

    #[must_use]
    pub(crate) const fn total_bytes(&self) -> u64 {
        (self.path_arena.len() + self.content_arena.len()) as u64
    }

    /// Sorts `spans` (16-byte records) hierarchically by path components in-place, without modifying string arenas.
    #[must_use]
    pub(crate) fn sort_by_path(mut self) -> Self {
        let path_arena = &self.path_arena;
        self.spans
            .sort_by(|a, b| cmp_path(a.path(path_arena), b.path(path_arena)));
        self
    }

    /// Appends path and content to the arenas and records a span for them.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::OutputTooLarge`] without modifying storage when the raw arena bytes
    /// would exceed `MAX_OUTPUT_SIZE`. The rejected candidate is included in the largest-file diagnostics.
    pub(crate) fn push(
        mut self,
        path: &NormalizedPath,
        content: &str,
    ) -> Result<Self, DomainError> {
        let path = path.as_str();
        if self.total_bytes() + path.len() as u64 + content.len() as u64 > MAX_OUTPUT_SIZE {
            let candidate = FileRecord { path, content };
            return Err(DomainError::output_too_large(largest_files(
                self.iter().chain(once(candidate)),
            )));
        }
        // Accepted total is at most `MAX_OUTPUT_SIZE`, which `policy/limits.rs` asserts is `<= u32::MAX`,
        // so every arena offset and length below fits in `u32`.
        let path_offset = u32::try_from(self.path_arena.len())
            .expect("invariant: arena bounded by MAX_OUTPUT_SIZE");
        let path_len =
            u32::try_from(path.len()).expect("invariant: arena bounded by MAX_OUTPUT_SIZE");
        self.path_arena.push_str(path);

        let content_offset = u32::try_from(self.content_arena.len())
            .expect("invariant: arena bounded by MAX_OUTPUT_SIZE");
        let content_len =
            u32::try_from(content.len()).expect("invariant: arena bounded by MAX_OUTPUT_SIZE");
        self.content_arena.push_str(content);

        let span = FileSpan {
            path_offset,
            path_len,
            content_offset,
            content_len,
        };
        self.spans.push(span);
        Ok(self)
    }

    /// Iterator yielding borrowed [`FileRecord`] views over the arena.
    pub(crate) fn iter(&self) -> impl ExactSizeIterator<Item = FileRecord<'_>> {
        self.spans.iter().map(move |span| FileRecord {
            path: span.path(&self.path_arena),
            content: span.content(&self.content_arena),
        })
    }

    /// Error for storage that exceeds the output limit, naming the largest files.
    #[must_use]
    pub(crate) fn output_too_large_error(&self) -> DomainError {
        DomainError::output_too_large(largest_files(self.iter()))
    }
}

#[cfg(test)]
impl<'a> FromIterator<FileRecord<'a>> for FileStorage {
    fn from_iter<T: IntoIterator<Item = FileRecord<'a>>>(iter: T) -> Self {
        iter.into_iter().fold(Self::new(), |storage, record| {
            storage
                .push(&NormalizedPath::from(record.path), record.content)
                .expect("test fixture fits the output limit")
        })
    }
}

/// Compares two normalized slash-separated relative paths hierarchically by path components.
///
/// Equivalent to `a.split('/').cmp(b.split('/'))`, computed on bytes. At the first differing byte,
/// a `/` ends a component that the other path continues, so it sorts first. `/` is ASCII, so it
/// never occurs inside a multi-byte UTF-8 sequence and every `/` byte is a separator.
#[must_use]
pub(crate) fn cmp_path(a: &str, b: &str) -> Ordering {
    let (a_bytes, b_bytes) = (a.as_bytes(), b.as_bytes());
    let common = a_bytes
        .iter()
        .zip(b_bytes)
        .take_while(|(x, y)| x == y)
        .count();
    match (a_bytes.get(common), b_bytes.get(common)) {
        (None, None) => Ordering::Equal,
        (None | Some(&b'/'), Some(_)) => Ordering::Less,
        (Some(_), None | Some(&b'/')) => Ordering::Greater,
        (Some(x), Some(y)) => x.cmp(y),
    }
}

/// Diagnostic ranking for digest size: selects up to `OVERFLOW_LARGEST_FILES` largest files.
///
/// Called only on output limit exceeded error paths to format error diagnostics.
/// Sorts by descending file size, tie-breaking by hierarchical path order (`cmp_path`).
#[must_use]
pub(crate) fn largest_files<'a>(
    files: impl IntoIterator<Item = FileRecord<'a>>,
) -> Vec<(String, usize)> {
    let mut sized: Vec<(&str, usize)> = files
        .into_iter()
        .map(|f| (f.path, f.content.len()))
        .collect();
    sized.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| cmp_path(a.0, b.0)));
    sized.truncate(OVERFLOW_LARGEST_FILES);
    sized.into_iter().map(|(p, s)| (p.to_string(), s)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::policy::MAX_OUTPUT_SIZE_USIZE;

    #[test]
    fn largest_files_selects_largest_with_stability() {
        let files = [
            FileRecord {
                path: "f1.txt",
                content: "1",
            },
            FileRecord {
                path: "f2.txt",
                content: "22",
            },
            FileRecord {
                path: "f3.txt",
                content: "333",
            },
            FileRecord {
                path: "f4.txt",
                content: "4444",
            },
            FileRecord {
                path: "f5_a.txt",
                content: "55555",
            },
            FileRecord {
                path: "f5_b.txt",
                content: "55555",
            },
            FileRecord {
                path: "f6.txt",
                content: "666666",
            },
        ];
        // Full order: descending size, ties by path. The diagnostic keeps the first `OVERFLOW_LARGEST_FILES`.
        let expected: Vec<(String, usize)> = [
            ("f6.txt", 6),
            ("f5_a.txt", 5),
            ("f5_b.txt", 5),
            ("f4.txt", 4),
            ("f3.txt", 3),
            ("f2.txt", 2),
            ("f1.txt", 1),
        ]
        .into_iter()
        .take(OVERFLOW_LARGEST_FILES)
        .map(|(path, size)| (path.to_string(), size))
        .collect();
        assert_eq!(largest_files(files), expected);
    }

    #[test]
    fn largest_files_breaks_ties_by_cmp_path() {
        let files = [
            FileRecord {
                path: "src-test/b.rs",
                content: "same",
            },
            FileRecord {
                path: "src/a.rs",
                content: "same",
            },
            FileRecord {
                path: "z.rs",
                content: "same",
            },
            FileRecord {
                path: "a.rs",
                content: "same",
            },
        ];
        let result = largest_files(files);
        assert_eq!(result[0].0, "a.rs");
        assert_eq!(result[1].0, "src/a.rs");
        assert_eq!(result[2].0, "src-test/b.rs");
        assert_eq!(result[3].0, "z.rs");
    }

    #[test]
    fn file_storage_maintains_continuous_arena_and_zero_copy_views() -> Result<(), DomainError> {
        let storage = FileStorage::new()
            .push(
                &NormalizedPath::from("path/to/first.rs"),
                "pub fn first() {}",
            )?
            .push(&NormalizedPath::from("second.txt"), "second content\n")?;

        assert_eq!(storage.len(), 2);

        let mut iter = storage.iter();
        let rec1 = iter.next().expect("first record");
        assert_eq!(rec1.path, "path/to/first.rs");
        assert_eq!(rec1.content, "pub fn first() {}");

        let rec2 = iter.next().expect("second record");
        assert_eq!(rec2.path, "second.txt");
        assert_eq!(rec2.content, "second content\n");
        assert_eq!(iter.next(), None);

        let collected: Vec<FileRecord<'_>> = storage.iter().collect();
        assert_eq!(collected, vec![rec1, rec2]);

        // Storage byte metrics verification via public contract
        let expected_bytes =
            (rec1.path.len() + rec1.content.len() + rec2.path.len() + rec2.content.len()) as u64;
        assert_eq!(storage.total_bytes(), expected_bytes);

        // FromIterator verification
        let reconstructed: FileStorage = collected.into_iter().collect();
        assert_eq!(reconstructed, storage);
        Ok(())
    }

    #[test]
    fn file_storage_sorts_by_path_in_place() -> Result<(), DomainError> {
        // (pushed paths, expected order): paths compare by component, so `src/a.rs` precedes `src-test/b.rs`.
        let cases: [(&[&str], &[&str]); 2] = [
            (
                &["z/file.rs", "a/file.rs", "m/file.rs"],
                &["a/file.rs", "m/file.rs", "z/file.rs"],
            ),
            (
                &["src-test/b.rs", "src/a.rs"],
                &["src/a.rs", "src-test/b.rs"],
            ),
        ];
        for (pushed, expected) in cases {
            let mut storage = FileStorage::new();
            for path in pushed {
                storage = storage.push(&NormalizedPath::from(*path), "content")?;
            }
            let storage = storage.sort_by_path();

            let paths: Vec<&str> = storage.iter().map(|f| f.path).collect();
            assert_eq!(paths, expected);
        }
        Ok(())
    }

    #[test]
    fn cmp_path_matches_component_order_over_exhaustive_small_alphabet() {
        // Alphabet: separator, a byte below '/', a letter, and a multi-byte char.
        let alphabet = ["+", "/", "a", "é"];
        let mut paths = vec![String::new()];
        let mut frontier = vec![String::new()];
        for _ in 0..5 {
            let next: Vec<String> = frontier
                .iter()
                .flat_map(|prefix| {
                    alphabet
                        .iter()
                        .map(move |symbol| format!("{prefix}{symbol}"))
                })
                .collect();
            paths.extend(next.iter().cloned());
            frontier = next;
        }
        for a in &paths {
            for b in &paths {
                assert_eq!(
                    cmp_path(a, b),
                    a.split('/').cmp(b.split('/')),
                    "{a:?} vs {b:?}"
                );
            }
        }
    }

    fn sorted_from(paths: &[&str]) -> Result<SortedFiles, DomainError> {
        let storage: FileStorage = paths
            .iter()
            .map(|path| FileRecord { path, content: "x" })
            .collect();
        SortedFiles::new(storage)
    }

    #[test]
    fn sorted_files_rejects_path_that_is_also_a_directory() {
        for paths in [&["a", "a/b"][..], &["a/b", "a"], &["x/a", "x/a/b/c"]] {
            let err = sorted_from(paths).unwrap_err();
            assert!(
                matches!(&err, DomainError::InvalidMetadata(msg) if msg.starts_with("entry path is both a file and a directory")),
                "{paths:?}: {err:?}"
            );
        }
    }

    #[test]
    fn sorted_files_allows_names_that_share_only_a_string_prefix() {
        for paths in [&["a.txt", "a/b"][..], &["a-b", "a/b"], &["a", "ab/c"]] {
            assert!(sorted_from(paths).is_ok(), "{paths:?}");
        }
    }

    #[test]
    fn push_enforces_output_limit_at_boundary() {
        let path = NormalizedPath::from("big.txt");
        let fill = MAX_OUTPUT_SIZE_USIZE - path.len();
        // (content length, expected to fit)
        let cases = [(fill, true), (fill + 1, false)];
        for (content_len, fits) in cases {
            let content = "x".repeat(content_len);
            match FileStorage::new().push(&path, &content) {
                Ok(storage) => {
                    assert!(fits, "content length {content_len} must be rejected");
                    assert_eq!(storage.total_bytes(), MAX_OUTPUT_SIZE);
                }
                Err(DomainError::OutputTooLarge(largest)) => {
                    assert!(!fits, "content length {content_len} must fit");
                    assert!(
                        largest.iter().any(|(p, _)| p == "big.txt"),
                        "rejected candidate must appear in diagnostics"
                    );
                }
                Err(other) => panic!("expected OutputTooLarge, got {other:?}"),
            }
        }
    }
}
