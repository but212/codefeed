//! Candidate observation, budget accounting, and storage accumulation.

use std::collections::{HashMap, HashSet};

use memchr::memchr;

use crate::core::collection::CollectionResult;
use crate::core::error::DomainError;
use crate::core::path::NormalizedPath;
use crate::core::policy::{
    MAX_FILE_COUNT, MAX_FILE_SIZE, MAX_PATH_BYTES_USIZE, OVERFLOW_LARGEST_DIRECTORIES,
};
use crate::core::storage::{FileStorage, SortedFiles};

/// UTF-8 text predicate: NUL-free bytes within size limit, returning string slice.
fn classify_content(bytes: &[u8]) -> Option<&str> {
    if bytes.len() as u64 > MAX_FILE_SIZE || memchr(0, bytes).is_some() {
        return None;
    }
    std::str::from_utf8(bytes).ok()
}

/// Declared-size gate. Shared by the shell readers that decide whether to read content, so both
/// report an over-limit declaration as [`Body::OverLimit`].
#[must_use]
pub(crate) const fn exceeds_file_size(declared: u64) -> bool {
    declared > MAX_FILE_SIZE
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

/// One settled candidate, as the shell reports it to [`Selector::observe`] (I-11).
///
/// An unstorable path has no content, so the pair of an unstorable path and a body cannot be built.
#[derive(Debug)]
pub(crate) enum Observation<B> {
    /// Non-UTF-8 or overlong path, identified by its valid directory prefix.
    Unstorable(String),
    /// Storable path with the outcome of reading its content.
    File(NormalizedPath, Body<B>),
}

/// Candidate-count budget owned by the selector that consumes it.
#[derive(Debug)]
#[cfg_attr(test, derive(Default))]
struct CandidateBudget {
    count: usize,
    dir_counts: HashMap<String, usize>,
}

impl CandidateBudget {
    /// Adds a single observed candidate path to the budget accounting.
    pub(crate) fn add(mut self, path: &str) -> Result<Self, DomainError> {
        self.count += 1;
        let dir = extract_dir_slice(path);
        if let Some(count) = self.dir_counts.get_mut(dir) {
            *count += 1;
        } else {
            self.dir_counts.insert(dir.to_string(), 1);
        }

        if self.count > MAX_FILE_COUNT {
            return Err(self.overflow_error());
        }
        Ok(self)
    }

    fn overflow_error(&self) -> DomainError {
        DomainError::candidate_limit_exceeded(
            MAX_FILE_COUNT,
            top_overflow_directories(&self.dir_counts),
        )
    }
}

/// Budget key for `path`: its first segment with a trailing `/`, or `./` for root-level entries.
/// No classified first segment starts with `.` (I-02), so `./` never names a real directory.
/// The key is capped at `MAX_PATH_BYTES` on a char boundary (I-05), so overlong segments cannot
/// grow the key store beyond the candidate count times `MAX_PATH_BYTES`.
fn extract_dir_slice(path: &str) -> &str {
    let dir = memchr(b'/', path.as_bytes()).map_or("./", |index| &path[..=index]);
    let mut end = dir.len().min(MAX_PATH_BYTES_USIZE);
    while !dir.is_char_boundary(end) {
        end -= 1;
    }
    &dir[..end]
}

fn top_overflow_directories(dir_counts: &HashMap<String, usize>) -> Vec<(String, usize)> {
    let mut dirs: Vec<(&str, usize)> = dir_counts.iter().map(|(d, &c)| (d.as_str(), c)).collect();
    dirs.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    dirs.truncate(OVERFLOW_LARGEST_DIRECTORIES);
    dirs.into_iter().map(|(d, c)| (d.to_string(), c)).collect()
}

/// Pure per-item selection and budget accounting. Owns no I/O; the shell feeds
/// observations and reads the outcome through [`Selector::finish`].
#[derive(Debug)]
#[must_use]
pub(crate) struct Selector {
    storage: FileStorage,
    candidates: CandidateBudget,
    paths: HashSet<String>,
    skipped: usize,
}

impl Selector {
    /// Creates a `Selector` with initial arena preallocations.
    pub(crate) fn new() -> Self {
        Self {
            storage: FileStorage::new(),
            candidates: CandidateBudget {
                count: 0,
                dir_counts: HashMap::with_capacity(32),
            },
            paths: HashSet::new(),
            skipped: 0,
        }
    }

    /// Claims `path` for uniqueness. Rejects it when an earlier candidate holds the same path.
    ///
    /// Skipped candidates count: two entries with one path are ambiguous even when one is skipped.
    fn claim_path(mut self, path: &str) -> Result<Self, DomainError> {
        if !self.paths.insert(path.to_string()) {
            return Err(DomainError::invalid_metadata(format!(
                "duplicate entry path: {path}"
            )));
        }
        Ok(self)
    }

    /// Observes one candidate entry and settles it.
    ///
    /// - [`Observation::Unstorable`] consumes candidate budget and is skipped.
    /// - [`Observation::File`] consumes candidate budget and must not repeat an earlier file path.
    ///   [`Body::Bytes`] is stored when it is UTF-8
    ///   text within the size limit, and skipped otherwise. [`Body::Unavailable`] and
    ///   [`Body::OverLimit`] are skipped.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::CandidateLimitExceeded`], [`DomainError::InvalidMetadata`] for a duplicate path, or
    /// [`DomainError::OutputTooLarge`].
    pub(crate) fn observe<B: AsRef<[u8]>>(
        mut self,
        observation: Observation<B>,
    ) -> Result<Self, DomainError> {
        let (path, body) = match observation {
            Observation::Unstorable(prefix) => {
                self.candidates = self.candidates.add(&prefix)?;
                return Ok(self.skip());
            }
            Observation::File(path, body) => (path, body),
        };
        self.candidates = self.candidates.add(&path)?;
        self = self.claim_path(&path)?;
        let Body::Bytes(bytes) = body else {
            return Ok(self.skip());
        };
        match classify_content(bytes.as_ref()) {
            Some(content) => {
                // Raw arena bytes are a strict lower bound for the rendered digest size, so
                // `push` rejects over-limit content before the final rendering pass.
                self.storage = self.storage.push(&path, content)?;
                Ok(self)
            }
            None => Ok(self.skip()),
        }
    }

    const fn skip(mut self) -> Self {
        self.skipped += 1;
        self
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
        Ok(CollectionResult {
            label,
            revision,
            files: SortedFiles::new(self.storage)?,
            skipped: self.skipped,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::policy::{
        MAX_FILE_SIZE_USIZE, MAX_OUTPUT_SIZE, MAX_OUTPUT_SIZE_USIZE, MAX_PATH_BYTES_USIZE,
    };

    #[test]
    fn file_size_gate_admits_exactly_the_limit_and_rejects_one_beyond() {
        assert!(!exceeds_file_size(0));
        assert!(!exceeds_file_size(MAX_FILE_SIZE));
        assert!(exceeds_file_size(MAX_FILE_SIZE + 1));
    }

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
    fn extracts_dir_slice_from_paths() {
        assert_eq!(extract_dir_slice("src/core/decide.rs"), "src/");
        assert_eq!(extract_dir_slice("root_file.txt"), "./");
        assert_eq!(extract_dir_slice(""), "./");
        assert_eq!(extract_dir_slice("vendor/"), "vendor/");
        assert_eq!(extract_dir_slice("<root>/x.rs"), "<root>/");
    }

    #[test]
    fn extract_dir_slice_caps_overlong_first_segment() {
        let overlong = format!("{}/file.rs", "a".repeat(MAX_PATH_BYTES_USIZE + 10));
        assert_eq!(
            extract_dir_slice(&overlong),
            "a".repeat(MAX_PATH_BYTES_USIZE)
        );
    }

    #[test]
    fn extract_dir_slice_cuts_overlong_segment_at_char_boundary() {
        // `a` followed by two-byte `é` puts every character start at an odd offset, so byte
        // `MAX_PATH_BYTES` lands inside a character and the cut moves back one byte.
        let overlong = format!("a{}/file.rs", "é".repeat(MAX_PATH_BYTES_USIZE));
        let dir = extract_dir_slice(&overlong);
        assert_eq!(dir.len(), MAX_PATH_BYTES_USIZE - 1);
        assert!(dir.starts_with('a'));
        assert!(dir.chars().skip(1).all(|c| c == 'é'));
    }

    #[test]
    fn candidate_budget_merges_overlong_directories_sharing_capped_prefix()
    -> Result<(), DomainError> {
        let head = "a".repeat(MAX_PATH_BYTES_USIZE + 5);
        let mut budget = CandidateBudget::default();
        for index in 0..3 {
            budget = budget.add(&format!("{head}{index}/file.rs"))?;
        }
        assert_eq!(budget.count, 3);
        assert_eq!(budget.dir_counts.len(), 1);
        assert_eq!(budget.dir_counts.values().copied().collect::<Vec<_>>(), [3]);
        assert!(
            budget
                .dir_counts
                .keys()
                .all(|key| key.len() <= MAX_PATH_BYTES_USIZE)
        );
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

    fn file<'a>(path: &str, body: Body<&'a [u8]>) -> Observation<&'a [u8]> {
        Observation::File(path.into(), body)
    }

    fn text(path: &str) -> Observation<&'static [u8]> {
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
            .observe(Observation::<&[u8]>::Unstorable("dir/".into()))
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
            let selector = Selector::new().observe(Observation::File(path.into(), body))?;
            let collection = selector.finish("test".into(), None)?;
            assert_eq!(collection.files.len(), included, "{path}");
            assert_eq!(collection.skipped, skipped, "{path}");
        }
        Ok(())
    }

    #[test]
    fn unstorable_observation_is_counted_and_skipped() -> Result<(), DomainError> {
        for prefix in ["", "dir/sub/"] {
            let selector =
                Selector::new().observe(Observation::<&[u8]>::Unstorable(prefix.into()))?;
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
