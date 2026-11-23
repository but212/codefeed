//! Candidate observation, budget accounting, and storage accumulation.

use std::collections::{HashMap, HashSet};

use memchr::memchr;

use crate::core::error::{DomainError, top_largest};
use crate::core::model::CollectionResult;
use crate::core::path::{BudgetDirKey, Candidate, NormalizedPath, UnstorablePath};
use crate::core::policy::{MAX_FILE_COUNT, MAX_FILE_SIZE, OVERFLOW_LARGEST_DIRECTORIES};
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

    /// Adds a single observed candidate directory key to the budget accounting.
    fn add(mut self, dir: BudgetDirKey<'_>) -> Result<Self, DomainError> {
        self.count += 1;
        self = self.increment(dir.as_str());

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

/// Pure per-item selection and budget accounting. Owns no I/O; the shell feeds
/// candidates and reads the outcome through [`Selector::finish`].
#[derive(Debug)]
#[must_use]
pub(crate) struct Selector {
    storage: FileStorage,
    candidates: CandidateBudget,
    seen_paths: HashSet<Vec<u8>>,
}

impl Selector {
    /// Creates a `Selector` with initial arena preallocations.
    pub(crate) fn new() -> Self {
        Self {
            storage: FileStorage::new(),
            candidates: CandidateBudget::with_capacity(32),
            seen_paths: HashSet::with_capacity(512),
        }
    }

    /// Admits and settles one local candidate with its body outcome.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::CandidateLimitExceeded`], [`DomainError::InvalidMetadata`] (duplicate path),
    /// or [`DomainError::OutputTooLarge`].
    pub(crate) fn observe<B: AsRef<[u8]>>(
        self,
        candidate: Candidate<Body<B>>,
    ) -> Result<Self, DomainError> {
        match candidate {
            Candidate::Unstorable(path) => self.admit_unstorable(path),
            Candidate::Valid(path, body) => self.admit_file(path)?.settle(body),
        }
    }

    /// Charges and checks a storable path before its body is available.
    pub(crate) fn admit_file(mut self, path: NormalizedPath) -> Result<PendingFile, DomainError> {
        self.candidates = self.candidates.add(path.budget_dir())?;
        if self.seen_paths.contains(path.as_bytes()) {
            return Err(DomainError::invalid_metadata(format!(
                "duplicate entry path: {}",
                &*path
            )));
        }
        Ok(PendingFile {
            selector: self,
            path,
        })
    }

    /// Charges and records an unstorable path, which has no body to settle.
    pub(crate) fn admit_unstorable(mut self, path: UnstorablePath) -> Result<Self, DomainError> {
        self.candidates = self.candidates.add(path.budget_dir())?;
        if !self.seen_paths.insert(path.into_identity()) {
            return Err(DomainError::invalid_metadata(
                "duplicate entry path (unstorable)",
            ));
        }
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

/// Owns the selector until an admitted, unique file is settled with its body outcome.
#[must_use]
pub(crate) struct PendingFile {
    selector: Selector,
    path: NormalizedPath,
}

impl PendingFile {
    /// Stores bounded UTF-8 text, or records the path alone if its body was skipped.
    pub(crate) fn settle<B: AsRef<[u8]>>(self, body: Body<B>) -> Result<Selector, DomainError> {
        let Self { mut selector, path } = self;
        if let Body::Bytes(bytes) = body
            && let Some(content) = classify_content(bytes.as_ref())
        {
            selector.storage = selector.storage.push(&path, content)?;
        }
        selector.seen_paths.insert(path.into_bytes());
        Ok(selector)
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
        let named_root = MAX_FILE_COUNT * 3 / 5;
        let root_files = MAX_FILE_COUNT - named_root;

        let cases: [(Vec<String>, &str, Vec<String>); 3] = [
            (
                (0..vendor)
                    .map(|index| format!("vendor/pkg_{index}.rs"))
                    .chain((0..src).map(|index| format!("src/file_{index}.rs")))
                    .collect(),
                "extra/one.rs",
                vec![
                    format!("repository contains more than {MAX_FILE_COUNT} files"),
                    "largest directories:".into(),
                    format!("vendor/ ({vendor} files)"),
                    format!("src/ ({src} files)"),
                ],
            ),
            (
                (0..MAX_FILE_COUNT)
                    .map(|i| format!("dir_{i:05}/file.rs"))
                    .collect(),
                "dir_00000/extra.rs",
                vec![
                    format!("repository contains more than {MAX_FILE_COUNT} files"),
                    "largest directories:".into(),
                    "- dir_00000/ (2 files)".into(),
                ],
            ),
            (
                (0..named_root)
                    .map(|index| format!("<root>/pkg_{index}.rs"))
                    .chain((0..root_files).map(|index| format!("f_{index}.rs")))
                    .collect(),
                "extra.rs",
                vec![
                    format!("\n  - <root>/ ({named_root} files)"),
                    // The `extra.rs` entry that overflows the budget is itself a root file.
                    format!("\n  - ./ ({} files)", root_files + 1),
                ],
            ),
        ];

        for (paths, overflow, expected) in cases {
            let mut budget = CandidateBudget::default();
            for path in &paths {
                budget = budget.add(BudgetDirKey::from_raw(path))?;
            }
            let message = budget
                .add(BudgetDirKey::from_raw(overflow))
                .unwrap_err()
                .to_string();
            for snippet in &expected {
                assert!(
                    message.contains(snippet),
                    "missing {snippet:?} in {message}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn budget_dir_key_takes_the_first_segment_capped_at_a_char_boundary() {
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
            assert_eq!(
                BudgetDirKey::from_raw(&path).as_str(),
                expected,
                "{path:.40}"
            );
        }
        assert_eq!(
            BudgetDirKey::from_raw(&format!("{}/file.rs", "é".repeat(MAX_PATH_BYTES))).len(),
            MAX_PATH_BYTES
        );
    }

    #[test]
    fn candidate_budget_merges_overlong_directories_sharing_capped_prefix()
    -> Result<(), DomainError> {
        let head = "a".repeat(MAX_PATH_BYTES + 5);
        let mut budget = CandidateBudget::default();
        for index in 0..3 {
            budget = budget.add(BudgetDirKey::from_raw(&format!("{head}{index}/file.rs")))?;
        }
        assert_eq!(budget.count, 3);
        assert_eq!(budget.dirs.len(), 1);
        assert_eq!(budget.dirs.values().copied().next(), Some(3));
        assert!(budget.dirs.keys().all(|k| k.len() <= MAX_PATH_BYTES));
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
    fn observe_enforces_candidate_budget_for_storable_and_unstorable() -> Result<(), DomainError> {
        for overflow in [
            text("overflow.txt"),
            Candidate::Unstorable(b"dir/\xff".as_slice().into()),
        ] {
            let mut selector = Selector::new();
            for i in 0..MAX_FILE_COUNT {
                selector = selector.observe(text(&format!("file_{i}.txt")))?;
            }
            let err = selector.observe(overflow).unwrap_err();
            assert!(matches!(err, DomainError::CandidateLimitExceeded(..)));
        }
        Ok(())
    }

    #[test]
    fn observe_counts_candidates_by_content_outcome() -> Result<(), Box<dyn std::error::Error>> {
        type ObservationCase = (Candidate<Body<Vec<u8>>>, usize, usize);
        let valid = |path: &str, body: Body<Vec<u8>>| Candidate::Valid(path.into(), body);
        // (candidate, included, skipped)
        let cases: [ObservationCase; 9] = [
            (valid("a.txt", Body::Bytes(b"hello".to_vec())), 1, 0),
            (valid("bin.bin", Body::Bytes(vec![0, 1])), 0, 1),
            (valid("bad_utf8", Body::Bytes(vec![0xff])), 0, 1),
            (
                valid("exact.txt", Body::Bytes(vec![b'x'; MAX_FILE_SIZE_USIZE])),
                1,
                0,
            ),
            (
                valid("over.txt", Body::Bytes(vec![b'x'; MAX_FILE_SIZE_USIZE + 1])),
                0,
                1,
            ),
            (valid("declared_over.txt", Body::OverLimit), 0, 1),
            (valid("gone.txt", Body::Unavailable), 0, 1),
            (valid("empty.txt", Body::Bytes(Vec::new())), 1, 0),
            (Candidate::Unstorable(b"\xff".as_slice().into()), 0, 1),
        ];
        for (candidate, included, skipped) in cases {
            let label = format!("{candidate:?}");
            let selector = Selector::new().observe(candidate)?;
            assert_eq!(selector.candidates.count, 1, "{label}");
            let collection = selector.finish("test".into(), None)?;
            assert_eq!(collection.files.len(), included, "{label}");
            assert_eq!(collection.skipped, skipped, "{label}");
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
            Candidate::Unstorable(b"dir/\xff".as_slice().into()),
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

        let unstorable =
            || Candidate::<Body<&[u8]>>::Unstorable(b"dir/\xff/file.rs".as_slice().into());
        let err = Selector::new()
            .observe(unstorable())
            .and_then(|selector| selector.observe(unstorable()))
            .unwrap_err();
        assert!(
            matches!(&err, DomainError::InvalidMetadata(msg) if msg == "duplicate entry path (unstorable)")
        );
    }

    #[test]
    fn unstorable_and_skipped_paths_preserve_raw_byte_identity_and_prefix_rules()
    -> Result<(), DomainError> {
        // I-09 / L2: exact raw bytes distinguish non-UTF-8 0xFF from valid UTF-8 U+FFFD.
        let collection = Selector::new()
            .observe(Candidate::<Body<&[u8]>>::Unstorable(
                b"\xff.txt".as_slice().into(),
            ))?
            .observe(file("\u{FFFD}.txt", Body::Bytes(b"x")))?
            .finish("test".into(), None)?;
        assert_eq!(collection.files.len(), 1);
        assert_eq!(collection.skipped, 1);

        // L2: distinct overlong unstorable paths sharing a `MAX_PATH_BYTES` prefix do not collide.
        let long = "a/".repeat(MAX_PATH_BYTES / 2 + 1);
        let first = format!("{long}a");
        let second = format!("{long}b");
        let collection = Selector::new()
            .observe(Candidate::<Body<&[u8]>>::Unstorable(
                first.as_bytes().into(),
            ))?
            .observe(Candidate::<Body<&[u8]>>::Unstorable(
                second.as_bytes().into(),
            ))?
            .finish("test".into(), None)?;
        assert_eq!(collection.files.len(), 0);
        assert_eq!(collection.skipped, 2);

        // I-01 applies only to stored `SortedFiles`, so a skipped file does not conflict with a nested stored file.
        for (first, second) in [
            (
                file("a", Body::OverLimit),
                file("a/b", Body::Bytes(b"nested")),
            ),
            (
                file("a/b", Body::Bytes(b"nested")),
                file("a", Body::OverLimit),
            ),
        ] {
            let collection = Selector::new()
                .observe(first)?
                .observe(second)?
                .finish("test".into(), None)?;
            assert_eq!(collection.files.len(), 1);
            assert_eq!(collection.files.iter().next().unwrap().path, "a/b");
            assert_eq!(collection.skipped, 1);
        }
        Ok(())
    }
}
