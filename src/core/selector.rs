//! Candidate observation, budget accounting, and storage accumulation.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

use memchr::memchr;

use crate::core::error::{DomainError, top_largest};
use crate::core::model::CollectionResult;
use crate::core::path::{Candidate, NormalizedPath};
use crate::core::policy::{
    MAX_FILE_COUNT, MAX_FILE_SIZE, MAX_PATH_BYTES, OVERFLOW_LARGEST_DIRECTORIES,
};
use crate::core::storage::{FileStorage, SortedFiles};

/// UTF-8 text predicate: NUL-free bytes within size limit, returning string slice.
fn classify_content(bytes: &[u8]) -> Option<&str> {
    if bytes.len() as u64 > MAX_FILE_SIZE || memchr(0, bytes).is_some() {
        return None;
    }
    std::str::from_utf8(bytes).ok()
}

/// What a shell reader learned about the content of one storable path.
#[derive(Debug)]
pub(crate) enum Body<B> {
    /// Unavailable at metadata, open, or read.
    Unavailable,
    /// Declared size over `MAX_FILE_SIZE`; not read.
    OverLimit,
    /// Bounded read of at most `MAX_FILE_SIZE + 1` bytes.
    Bytes(B),
}

/// Candidate-count budget and per-directory tallies owned by the selector that consumes it.
#[derive(Debug)]
struct CandidateBudget {
    count: usize,
    dirs: HashMap<String, usize>,
}

#[cfg(test)]
impl Default for CandidateBudget {
    fn default() -> Self {
        Self::with_capacity(16)
    }
}

impl CandidateBudget {
    fn with_capacity(dirs_cap: usize) -> Self {
        Self {
            count: 0,
            dirs: HashMap::with_capacity(dirs_cap),
        }
    }

    /// Increments the count for `key`, inserting `1` if `key` was absent.
    fn increment(mut self, key: &str) -> Self {
        if let Some(count) = self.dirs.get_mut(key) {
            *count += 1;
        } else {
            self.dirs.insert(key.to_owned(), 1);
        }
        self
    }

    /// Adds a single observed candidate path to the budget accounting.
    fn add(mut self, path: &str) -> Result<Self, DomainError> {
        self.count += 1;
        let dir = extract_dir_slice(path);
        self = self.increment(dir);

        if self.count > MAX_FILE_COUNT {
            return Err(self.overflow_error());
        }
        Ok(self)
    }

    fn overflow_error(&self) -> DomainError {
        let dirs = top_largest(
            self.dirs.iter().map(|(dir, &count)| (dir.as_str(), count)),
            OVERFLOW_LARGEST_DIRECTORIES,
            str::cmp,
        );
        DomainError::candidate_limit_exceeded(MAX_FILE_COUNT, &dirs)
    }
}

/// Budget key for `path`: its first segment with a trailing `/`, or `./` for root-level entries.
/// No classified first segment starts with `.` (I-02), so `./` never names a real directory.
/// The key is capped at `MAX_PATH_BYTES` on a char boundary (I-05), so overlong segments cannot
/// grow the key store beyond the candidate count times `MAX_PATH_BYTES`.
fn extract_dir_slice(path: &str) -> &str {
    let dir = memchr(b'/', path.as_bytes()).map_or("./", |index| &path[..=index]);
    let mut end = dir.len().min(MAX_PATH_BYTES);
    while !dir.is_char_boundary(end) {
        end -= 1;
    }
    &dir[..end]
}

/// Pure per-item selection and budget accounting. Owns no I/O; the shell feeds
/// candidates and reads the outcome through [`Selector::finish`].
#[derive(Debug)]
#[must_use]
pub(crate) struct Selector {
    storage: FileStorage,
    candidates: CandidateBudget,
    seen_paths: HashMap<NormalizedPath, ()>,
    unstorable_paths: HashSet<Vec<u8>>,
}

impl Selector {
    /// Creates a `Selector` with initial arena preallocations.
    pub(crate) fn new() -> Self {
        Self {
            storage: FileStorage::new(),
            candidates: CandidateBudget::with_capacity(32),
            seen_paths: HashMap::with_capacity(512),
            unstorable_paths: HashSet::new(),
        }
    }

    /// Observes one candidate entry and settles it.
    ///
    /// - [`Candidate::Unstorable`]: consumes budget, requires unique byte identity, skipped.
    /// - [`Candidate::Valid`]: consumes budget, requires unique path; stores UTF-8 [`Body::Bytes`] within
    ///   size limit, skips non-UTF-8, over-limit, [`Body::Unavailable`], or [`Body::OverLimit`].
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::CandidateLimitExceeded`], [`DomainError::InvalidMetadata`] (duplicate path),
    /// or [`DomainError::OutputTooLarge`].
    pub(crate) fn observe<B: AsRef<[u8]>>(
        mut self,
        candidate: Candidate<Body<B>>,
    ) -> Result<Self, DomainError> {
        let (path, body) = match candidate {
            Candidate::Unstorable(path) => {
                self.candidates = self.candidates.add(path.prefix())?;
                if !self.unstorable_paths.insert(path.into_identity()) {
                    return Err(DomainError::invalid_metadata(
                        "duplicate entry path (unstorable)",
                    ));
                }
                return Ok(self);
            }
            Candidate::Valid(path, body) => (path, body),
        };
        self.candidates = self.candidates.add(&path)?;
        let entry = match self.seen_paths.entry(path) {
            Entry::Occupied(occupied) => {
                return Err(DomainError::invalid_metadata(format!(
                    "duplicate entry path: {}",
                    &**occupied.key()
                )));
            }
            Entry::Vacant(vacant) => vacant,
        };

        if let Body::Bytes(bytes) = body
            && let Some(content) = classify_content(bytes.as_ref())
        {
            // Raw arena bytes are a strict lower bound for the rendered digest size, so
            // `push` rejects over-limit content before the final rendering pass.
            self.storage = self.storage.push(entry.key(), content)?;
        }
        entry.insert(());
        Ok(self)
    }

    /// Finalizes selection, sorting files lexicographically in-place, and returns a [`CollectionResult`].
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidMetadata`] when a stored path is a directory prefix of another stored path.
    pub(crate) fn finish(
        self,
        label: String,
        revision: Option<String>,
    ) -> Result<CollectionResult, DomainError> {
        let skipped = self.candidates.count - self.storage.len();
        Ok(CollectionResult {
            label,
            revision,
            files: SortedFiles::new(self.storage)?,
            skipped,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::policy::{
        MAX_FILE_SIZE_USIZE, MAX_OUTPUT_SIZE, MAX_OUTPUT_SIZE_USIZE, MAX_PATH_BYTES,
    };

    #[test]
    fn candidate_overflow_includes_largest_directories_diagnostic()
    -> Result<(), Box<dyn std::error::Error>> {
        let vendor = MAX_FILE_COUNT * 4 / 5;
        let src = MAX_FILE_COUNT - vendor;
        let mut budget = CandidateBudget::default();
        for index in 0..vendor {
            budget = budget.add(&format!("vendor/pkg_{index}.rs"))?;
        }
        for index in 0..src {
            budget = budget.add(&format!("src/file_{index}.rs"))?;
        }
        let err = budget.add("extra/one.rs").unwrap_err();
        let message = err.to_string();
        assert!(message.contains(&format!(
            "repository contains more than {MAX_FILE_COUNT} files"
        )));
        assert!(message.contains("largest directories:"));
        assert!(message.contains(&format!("vendor/ ({vendor} files)")));
        assert!(message.contains(&format!("src/ ({src} files)")));
        Ok(())
    }

    #[test]
    fn candidate_budget_handles_many_distinct_directories_deterministically() {
        let mut budget = CandidateBudget::default();
        for i in 0..MAX_FILE_COUNT {
            let path = format!("dir_{i:05}/file.rs");
            budget = budget.add(&path).unwrap();
        }
        let err = budget.add("dir_00000/extra.rs").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains(&format!(
            "repository contains more than {MAX_FILE_COUNT} files"
        )));
        assert!(msg.contains("largest directories:"));
        assert!(msg.contains("- dir_00000/ (2 files)"));
    }

    #[test]
    fn extract_dir_slice_takes_the_first_segment_capped_at_a_char_boundary() {
        // `a` followed by two-byte `é` puts every character start at an odd offset, so byte
        // `MAX_PATH_BYTES` lands inside a character and the cut moves back one byte.
        let accented = format!("a{}", "é".repeat(MAX_PATH_BYTES));
        let accented_cut = format!("a{}", "é".repeat((MAX_PATH_BYTES - 2) / 2));
        let overlong = "a".repeat(MAX_PATH_BYTES + 10);

        let cases = [
            ("src/core/decide.rs".to_string(), "src/".to_string()),
            ("root_file.txt".to_string(), "./".to_string()),
            (String::new(), "./".to_string()),
            ("vendor/".to_string(), "vendor/".to_string()),
            ("<root>/x.rs".to_string(), "<root>/".to_string()),
            (format!("{overlong}/file.rs"), "a".repeat(MAX_PATH_BYTES)),
            (format!("{accented}/file.rs"), accented_cut),
        ];
        for (path, expected) in cases {
            assert_eq!(extract_dir_slice(&path), expected, "{path:.40}");
        }
        assert_eq!(
            extract_dir_slice(&format!("{}/file.rs", "é".repeat(MAX_PATH_BYTES))).len(),
            MAX_PATH_BYTES
        );
    }

    #[test]
    fn candidate_budget_merges_overlong_directories_sharing_capped_prefix()
    -> Result<(), DomainError> {
        let head = "a".repeat(MAX_PATH_BYTES + 5);
        let mut budget = CandidateBudget::default();
        for index in 0..3 {
            budget = budget.add(&format!("{head}{index}/file.rs"))?;
        }
        assert_eq!(budget.count, 3);
        assert_eq!(budget.dirs.len(), 1);
        assert_eq!(budget.dirs.values().copied().next(), Some(3));
        assert!(budget.dirs.keys().all(|k| k.len() <= MAX_PATH_BYTES));
        Ok(())
    }

    #[test]
    fn candidate_overflow_separates_root_files_from_directory_named_root()
    -> Result<(), Box<dyn std::error::Error>> {
        let named_root = MAX_FILE_COUNT * 3 / 5;
        let root_files = MAX_FILE_COUNT - named_root;
        let mut budget = CandidateBudget::default();
        for index in 0..named_root {
            budget = budget.add(&format!("<root>/pkg_{index}.rs"))?;
        }
        for index in 0..root_files {
            budget = budget.add(&format!("f_{index}.rs"))?;
        }
        let message = budget.add("extra.rs").unwrap_err().to_string();
        assert!(
            message.contains(&format!("\n  - <root>/ ({named_root} files)")),
            "{message}"
        );
        // The `extra.rs` entry that overflows the budget is itself a root file.
        assert!(
            message.contains(&format!("\n  - ./ ({} files)", root_files + 1)),
            "{message}"
        );
        Ok(())
    }

    #[test]
    fn classify_content_detects_binary_nul_anywhere() {
        let mut bin = vec![b'a'; 1024];
        bin[12] = 0;
        assert_eq!(classify_content(&bin), None);

        let mut bin_tail = vec![b'b'; 5000];
        bin_tail[4500] = 0;
        assert_eq!(classify_content(&bin_tail), None);

        let mut bytes = vec![b'a'; 4095];
        bytes.extend_from_slice("€tail\n".as_bytes());
        let res = classify_content(&bytes);
        assert!(matches!(res, Some(content) if content.ends_with("€tail\n")));

        let mut bytes_with_nul = vec![b'a'; 4095];
        bytes_with_nul.extend_from_slice("€\0tail\n".as_bytes());
        assert_eq!(classify_content(&bytes_with_nul), None);
    }

    fn file<'a>(path: &str, body: Body<&'a [u8]>) -> Candidate<Body<&'a [u8]>> {
        Candidate::Valid(path.into(), body)
    }

    fn text(path: &str) -> Candidate<Body<&'static [u8]>> {
        file(path, Body::Bytes(b"content"))
    }

    #[test]
    fn output_overflow_includes_largest_files_diagnostic() -> Result<(), Box<dyn std::error::Error>>
    {
        let chunk = vec![b'x'; MAX_FILE_SIZE_USIZE];
        let mut selector = Selector::new();
        // Fill the output to one file short of its limit; the next file overflows it.
        for i in 0..MAX_OUTPUT_SIZE_USIZE / MAX_FILE_SIZE_USIZE - 1 {
            selector = selector.observe(file(
                &format!("file_{i:02}.txt"),
                Body::Bytes(chunk.as_slice()),
            ))?;
        }
        let err = selector
            .observe(file("overflow.txt", Body::Bytes(chunk.as_slice())))
            .unwrap_err();
        let message = err.to_string();
        assert!(message.contains(&format!(
            "digest exceeds the {} MiB output limit",
            MAX_OUTPUT_SIZE / (1024 * 1024)
        )));
        assert!(message.contains("largest files:"));
        assert!(message.contains("file_00.txt"));
        Ok(())
    }

    #[test]
    fn observe_enforces_candidate_budget() -> Result<(), DomainError> {
        let mut selector = Selector::new();
        for i in 0..MAX_FILE_COUNT {
            selector = selector.observe(text(&format!("file_{i}.txt")))?;
        }

        let err = selector.observe(text("overflow.txt")).unwrap_err();
        assert!(matches!(err, DomainError::CandidateLimitExceeded(..)));
        Ok(())
    }

    #[test]
    fn unstorable_candidate_consumes_budget_and_fails_when_exhausted() -> Result<(), DomainError> {
        let mut selector = Selector::new();
        for i in 0..MAX_FILE_COUNT {
            selector = selector.observe(text(&format!("file_{i}.txt")))?;
        }

        let err = selector
            .observe(Candidate::<Body<&[u8]>>::Unstorable(
                ("dir/", b"dir/\xff".as_slice()).into(),
            ))
            .unwrap_err();
        assert!(matches!(err, DomainError::CandidateLimitExceeded(..)));
        Ok(())
    }

    #[test]
    fn observe_counts_candidates_by_content_outcome() -> Result<(), Box<dyn std::error::Error>> {
        // (path, body, included, skipped)
        let cases: [(&str, Body<Vec<u8>>, usize, usize); 8] = [
            ("a.txt", Body::Bytes(b"hello".to_vec()), 1, 0),
            ("bin.bin", Body::Bytes(vec![0, 1]), 0, 1),
            ("bad_utf8", Body::Bytes(vec![0xff]), 0, 1),
            (
                "exact.txt",
                Body::Bytes(vec![b'x'; MAX_FILE_SIZE_USIZE]),
                1,
                0,
            ),
            (
                "over.txt",
                Body::Bytes(vec![b'x'; MAX_FILE_SIZE_USIZE + 1]),
                0,
                1,
            ),
            ("declared_over.txt", Body::OverLimit, 0, 1),
            ("gone.txt", Body::Unavailable, 0, 1),
            ("empty.txt", Body::Bytes(Vec::new()), 1, 0),
        ];
        for (path, body, included, skipped) in cases {
            let selector = Selector::new().observe(Candidate::Valid(path.into(), body))?;
            let collection = selector.finish("test".into(), None)?;
            assert_eq!(collection.files.len(), included, "{path}");
            assert_eq!(collection.skipped, skipped, "{path}");
        }
        Ok(())
    }

    #[test]
    fn finish_counts_skipped_across_mixed_observations() -> Result<(), DomainError> {
        let empty = Selector::new().finish("test".into(), None)?;
        assert_eq!(empty.files.len(), 0);
        assert_eq!(empty.skipped, 0);

        let observations = [
            text("a.txt"),
            file("bin.bin", Body::Bytes(b"\0")),
            file("bad_utf8", Body::Bytes(b"\xff")),
            file("over.txt", Body::OverLimit),
            file("gone.txt", Body::Unavailable),
            Candidate::Unstorable(("dir/", b"dir/\xff".as_slice()).into()),
            file("empty.txt", Body::Bytes(b"")),
        ];
        let selector = observations
            .into_iter()
            .try_fold(Selector::new(), Selector::observe)?;
        let collection = selector.finish("test".into(), None)?;
        assert_eq!(collection.files.len(), 2);
        assert_eq!(collection.skipped, 5);
        Ok(())
    }

    #[test]
    fn unstorable_observation_is_counted_and_skipped() -> Result<(), DomainError> {
        for prefix in ["", "dir/sub/"] {
            let mut identity = prefix.as_bytes().to_vec();
            identity.push(0xff);
            let selector = Selector::new().observe(Candidate::<Body<&[u8]>>::Unstorable(
                (prefix, identity.as_slice()).into(),
            ))?;
            assert_eq!(selector.candidates.count, 1);
            let collection = selector.finish("test".into(), None)?;
            assert_eq!(collection.files.len(), 0);
            assert_eq!(collection.skipped, 1);
        }
        Ok(())
    }

    #[test]
    fn observe_rejects_duplicate_paths_including_skipped() {
        let stored = || Body::Bytes(&b"x"[..]);
        // A skipped candidate still names its path, so it collides with a stored one in either order.
        let cases = [
            (stored(), stored()),
            (stored(), Body::OverLimit),
            (Body::OverLimit, stored()),
            (Body::Unavailable, Body::OverLimit),
        ];
        for (first, second) in cases {
            let err = Selector::new()
                .observe(file("a.txt", first))
                .and_then(|selector| selector.observe(file("a.txt", second)))
                .unwrap_err();
            assert!(
                matches!(&err, DomainError::InvalidMetadata(msg) if msg == "duplicate entry path: a.txt")
            );
        }
    }
}
