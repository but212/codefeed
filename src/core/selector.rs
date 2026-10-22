//! Candidate observation, budget accounting, and storage accumulation.

use std::collections::HashMap;

use memchr::memchr;

use crate::core::error::DomainError;
use crate::core::model::CollectionResult;
use crate::core::policy::{MAX_FILE_COUNT, MAX_FILE_SIZE, MAX_PATH_BYTES, PathStatus};
use crate::core::storage::{FileStorage, SortedFiles};

/// UTF-8 text predicate: NUL-free bytes within size limit, returning string slice.
fn classify_content(bytes: &[u8]) -> Option<&str> {
    if bytes.len() as u64 > MAX_FILE_SIZE || memchr(0, bytes).is_some() {
        return None;
    }
    std::str::from_utf8(bytes).ok()
}

/// Path-length and declared-size gate. Shared by [`Selector::observe`] and the shell worker that decides whether to read content.
#[must_use]
pub(crate) fn exceeds_size_limits(path_len: usize, size_hint: Option<u64>) -> bool {
    path_len as u64 > MAX_PATH_BYTES || size_hint.is_some_and(|hint| hint > MAX_FILE_SIZE)
}

/// Candidate-count budget owned by the selector that consumes it.
#[derive(Default, Debug)]
struct CandidateBudget {
    count: usize,
    dir_counts: HashMap<String, usize>,
}

impl CandidateBudget {
    /// Adds a single observed candidate path to the budget accounting.
    pub(crate) fn add(mut self, path: &str) -> Result<Self, DomainError> {
        self.count = self.count.saturating_add(1);
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

fn extract_dir_slice(path: &str) -> &str {
    match memchr(b'/', path.as_bytes()) {
        Some(index) => &path[..=index],
        None => "<root>/",
    }
}

fn top_overflow_directories(dir_counts: &HashMap<String, usize>) -> Vec<(String, usize)> {
    let mut dirs: Vec<(&str, usize)> = dir_counts.iter().map(|(d, &c)| (d.as_str(), c)).collect();
    dirs.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    dirs.truncate(3);
    dirs.into_iter().map(|(d, c)| (d.to_string(), c)).collect()
}

/// Pure per-item selection and budget accounting. Owns no I/O; the shell feeds
/// observations and reads the outcome through [`Selector::finish`].
#[derive(Debug)]
#[must_use]
pub(crate) struct Selector {
    storage: FileStorage,
    candidates: CandidateBudget,
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
            skipped: 0,
        }
    }

    /// Observes one candidate entry and settles it.
    ///
    /// - [`PathStatus::Ignored`] is dropped without consuming candidate budget.
    /// - [`PathStatus::NonUtf8Candidate`] and paths over the size limits consume candidate budget and are skipped.
    /// - [`PathStatus::Valid`] within the size limits consumes candidate budget, then `read` runs; its bytes
    ///   are stored when they are UTF-8 text, and skipped otherwise or when `None` (unavailable file).
    ///
    /// # Errors
    ///
    /// Returns budget or output-limit errors as `E`, and any error from `read` unchanged.
    pub(crate) fn observe<B: AsRef<[u8]>, E: From<DomainError>>(
        mut self,
        status: PathStatus,
        size_hint: Option<u64>,
        read: impl FnOnce() -> Result<Option<B>, E>,
    ) -> Result<Self, E> {
        let path = match status {
            PathStatus::Ignored => return Ok(self),
            PathStatus::NonUtf8Candidate(prefix) => {
                self.candidates = self.candidates.add(&prefix)?;
                return Ok(self.skip());
            }
            PathStatus::Valid(path) => path,
        };
        self.candidates = self.candidates.add(&path)?;
        if exceeds_size_limits(path.len(), size_hint) {
            return Ok(self.skip());
        }
        let bytes = read()?;
        match bytes.as_ref().and_then(|b| classify_content(b.as_ref())) {
            Some(content) => {
                // Raw arena bytes are a strict lower bound for the rendered digest size, so
                // `push` rejects over-limit content before the final rendering pass.
                self.storage = self.storage.push(&path, content)?;
                Ok(self)
            }
            None => Ok(self.skip()),
        }
    }

    fn skip(mut self) -> Self {
        self.skipped = self.skipped.saturating_add(1);
        self
    }

    /// Finalizes selection, sorting files lexicographically in-place, and returns a [`CollectionResult`].
    pub(crate) fn finish(self, label: String, revision: Option<String>) -> CollectionResult {
        CollectionResult {
            label,
            revision,
            files: SortedFiles::new(self.storage),
            skipped: self.skipped,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::policy::MAX_FILE_SIZE;

    #[test]
    fn candidate_budget_enforces_limit() {
        let mut candidates = CandidateBudget::default();
        for _ in 0..MAX_FILE_COUNT {
            candidates = candidates.add("").unwrap();
        }
        assert!(candidates.add("").is_err());
    }

    #[test]
    fn candidate_overflow_includes_largest_directories_diagnostic()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut budget = CandidateBudget::default();
        for index in 0..8_000 {
            budget = budget.add(&format!("vendor/pkg_{index}.rs"))?;
        }
        for index in 0..2_000 {
            budget = budget.add(&format!("src/file_{index}.rs"))?;
        }
        let err = budget.add("extra/one.rs").unwrap_err();
        let message = err.to_string();
        assert!(message.contains("repository contains more than 10000 files"));
        assert!(message.contains("largest directories:"));
        assert!(message.contains("vendor/ (8000 files)"));
        assert!(message.contains("src/ (2000 files)"));
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
        assert!(msg.contains("repository contains more than 10000 files"));
        assert!(msg.contains("largest directories:"));
        assert!(msg.contains("- dir_00000/ (2 files)"));
    }

    #[test]
    fn extracts_dir_slice_from_paths() {
        assert_eq!(extract_dir_slice("src/core/decide.rs"), "src/");
        assert_eq!(extract_dir_slice("root_file.txt"), "<root>/");
        assert_eq!(extract_dir_slice(""), "<root>/");
        assert_eq!(extract_dir_slice("vendor/"), "vendor/");
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

    #[test]
    fn output_overflow_includes_largest_files_diagnostic() -> Result<(), Box<dyn std::error::Error>>
    {
        let chunk = vec![b'x'; MAX_FILE_SIZE as usize];
        let mut selector = Selector::new();
        for i in 0..19 {
            selector = selector.observe(
                PathStatus::Valid(format!("file_{i:02}.txt").into()),
                None,
                || Ok::<_, DomainError>(Some(chunk.as_slice())),
            )?;
        }
        let err = selector
            .observe(PathStatus::Valid("overflow.txt".into()), None, || {
                Ok::<_, DomainError>(Some(chunk.as_slice()))
            })
            .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("digest exceeds the 20 MiB output limit"));
        assert!(message.contains("largest files:"));
        assert!(message.contains("file_00.txt"));
        Ok(())
    }

    #[test]
    fn observe_enforces_candidate_budget_before_read() -> Result<(), Box<dyn std::error::Error>> {
        let mut selector = Selector::new();
        for i in 0..MAX_FILE_COUNT {
            let path = PathStatus::Valid(format!("file_{i}.txt").into());
            selector =
                selector.observe(path, None, || Ok::<_, DomainError>(Some(&b"content"[..])))?;
        }

        // The 10,001st candidate fails at admission, so its read closure must never run.
        let err = selector
            .observe(
                PathStatus::Valid("overflow.txt".into()),
                None,
                || -> Result<Option<Vec<u8>>, DomainError> {
                    panic!("read must not run past the candidate budget")
                },
            )
            .unwrap_err();
        assert!(matches!(err, DomainError::CandidateLimitExceeded(..)));
        Ok(())
    }

    #[test]
    fn observe_counts_candidates_by_content_outcome() -> Result<(), Box<dyn std::error::Error>> {
        // (path, content, included, skipped); `None` content means the file was unavailable.
        let cases: [(&str, Option<Vec<u8>>, usize, usize); 6] = [
            ("a.txt", Some(b"hello".to_vec()), 1, 0),
            ("bin.bin", Some(vec![0, 1]), 0, 1),
            ("bad_utf8", Some(vec![0xff]), 0, 1),
            ("exact.txt", Some(vec![b'x'; MAX_FILE_SIZE as usize]), 1, 0),
            (
                "over.txt",
                Some(vec![b'x'; MAX_FILE_SIZE as usize + 1]),
                0,
                1,
            ),
            ("gone.txt", None, 0, 1),
        ];
        for (path, content, included, skipped) in cases {
            let selector = Selector::new().observe(PathStatus::Valid(path.into()), None, || {
                Ok::<_, DomainError>(content)
            })?;
            let collection = selector.finish("test".into(), None);
            assert_eq!(collection.files.len(), included, "{path}");
            assert_eq!(collection.skipped, skipped, "{path}");
        }
        Ok(())
    }

    #[test]
    fn observe_does_not_read_settled_candidates() -> Result<(), Box<dyn std::error::Error>> {
        let overlong = format!("{}/file.rs", "a".repeat(MAX_PATH_BYTES as usize));
        // (status, size_hint, candidates counted, skipped)
        let cases: [(PathStatus, Option<u64>, usize, usize); 5] = [
            (PathStatus::Ignored, None, 0, 0),
            (PathStatus::NonUtf8Candidate("".into()), None, 1, 1),
            (PathStatus::NonUtf8Candidate("dir/sub/".into()), None, 1, 1),
            (
                PathStatus::Valid("big.bin".into()),
                Some(MAX_FILE_SIZE + 1),
                1,
                1,
            ),
            (PathStatus::Valid(overlong.into()), None, 1, 1),
        ];
        for (status, size_hint, candidates, skipped) in cases {
            let selector = Selector::new().observe(
                status,
                size_hint,
                || -> Result<Option<Vec<u8>>, DomainError> {
                    panic!("read must not run for settled candidates")
                },
            )?;
            assert_eq!(selector.candidates.count, candidates);
            let collection = selector.finish("test".into(), None);
            assert_eq!(collection.files.len(), 0);
            assert_eq!(collection.skipped, skipped);
        }
        Ok(())
    }

    #[test]
    fn observe_propagates_read_errors() {
        #[derive(Debug, PartialEq)]
        enum TestError {
            Read,
            Domain,
        }
        impl From<DomainError> for TestError {
            fn from(_: DomainError) -> Self {
                Self::Domain
            }
        }

        let err = Selector::new()
            .observe(PathStatus::Valid("a.txt".into()), None, || {
                Err::<Option<Vec<u8>>, _>(TestError::Read)
            })
            .unwrap_err();
        assert_eq!(err, TestError::Read);
    }
}
