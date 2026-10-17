//! Candidate observation, budget accounting, and storage accumulation.

use std::collections::HashMap;

use crate::core::error::IngestError;
use crate::core::model::{CollectionResult, FileRecord, FileStorage};
use crate::core::policy::{MAX_FILE_COUNT, MAX_OUTPUT_SIZE};

/// Candidate input observed by the imperative shell. Non-UTF-8 paths or unreadable
/// items are represented as `Skipped` and still counted against the file budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CandidateInput<'a> {
    /// Bounded content bytes successfully acquired from the source.
    File { path: &'a str, bytes: &'a [u8] },
    /// Item exceeded the per-file size limit.
    TooLarge { path: &'a str },
    /// Item skipped due to metadata error, non-UTF-8 path, or read failure.
    Skipped { path: Option<&'a str> },
}

impl<'a> CandidateInput<'a> {
    /// Returns the optional path of the candidate input.
    #[inline]
    pub(crate) const fn path(&self) -> Option<&'a str> {
        match self {
            Self::File { path, .. } | Self::TooLarge { path } => Some(*path),
            Self::Skipped { path } => *path,
        }
    }
}

/// UTF-8 text predicate: NUL-free bytes only.
#[inline]
pub(crate) fn text_str(bytes: &[u8]) -> Option<&str> {
    if bytes.contains(&0) {
        return None;
    }
    std::str::from_utf8(bytes).ok()
}

/// Candidate-count budget owned by the selector that consumes it.
#[derive(Default, Debug)]
pub(crate) struct CandidateBudget {
    count: usize,
    dir_counts: HashMap<String, usize>,
}

impl CandidateBudget {
    /// Creates a new `CandidateBudget` pre-allocating hash map capacity for directory buckets.
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            count: 0,
            dir_counts: HashMap::with_capacity(capacity),
        }
    }

    /// Adds a single observed candidate path to the budget accounting.
    pub(crate) fn add(&mut self, path: Option<&str>) -> Result<(), IngestError> {
        self.count = self.count.checked_add(1).ok_or_else(file_count_overflow)?;
        let dir = extract_dir_slice(path);
        if let Some(count) = self.dir_counts.get_mut(dir) {
            *count += 1;
        } else {
            self.dir_counts.insert(dir.to_string(), 1);
        }

        if self.count > MAX_FILE_COUNT {
            return Err(self.overflow_error());
        }
        Ok(())
    }

    fn overflow_error(&self) -> IngestError {
        IngestError::candidate_limit_exceeded(
            MAX_FILE_COUNT,
            top_overflow_directories(&self.dir_counts),
        )
    }
}

fn file_count_overflow() -> IngestError {
    IngestError::invalid_metadata("file count overflow")
}

fn extract_dir_slice(path: Option<&str>) -> &str {
    match path {
        Some(p) => match p.find('/') {
            Some(index) => &p[..=index],
            None => "<root>/",
        },
        None => "<unknown>/",
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
pub(crate) struct Selector {
    storage: FileStorage,
    candidates: CandidateBudget,
    skipped: usize,
}

impl Selector {
    /// Creates a default `Selector` with initial arena preallocations.
    pub(crate) fn new() -> Self {
        Self::with_capacity(512, 16 * 1024, 256 * 1024)
    }

    /// Creates a `Selector` with explicitly specified arena and directory budget capacities.
    pub(crate) fn with_capacity(spans_cap: usize, path_bytes: usize, content_bytes: usize) -> Self {
        let dir_runs_cap = (spans_cap / 16).clamp(16, 256);
        Self {
            storage: FileStorage::with_capacity(spans_cap, path_bytes, content_bytes),
            candidates: CandidateBudget::with_capacity(dir_runs_cap),
            skipped: 0,
        }
    }

    /// Creates a `Selector` sizing arena capacities based on a source stream size hint.
    pub(crate) fn from_size_hint(hint: (usize, Option<usize>)) -> Self {
        let estimated = hint.1.unwrap_or(hint.0);
        if estimated == 0 {
            Self::new()
        } else {
            let spans_cap = estimated.min(MAX_FILE_COUNT);
            let path_cap = spans_cap.saturating_mul(48);
            let content_cap =
                (spans_cap.saturating_mul(2048)).clamp(256 * 1024, MAX_OUTPUT_SIZE as usize);
            Self::with_capacity(spans_cap, path_cap, content_cap)
        }
    }

    /// Evaluates a candidate input against budget and output limits, accumulating accepted files.
    ///
    /// # Errors
    ///
    /// Returns [`IngestError::CandidateLimitExceeded`] if file count overflows, or
    /// [`IngestError::OutputTooLarge`] if output size exceeds the policy limit.
    pub(crate) fn feed(&mut self, input: CandidateInput<'_>) -> Result<(), IngestError> {
        self.candidates.add(input.path())?;
        match input {
            CandidateInput::File { path, bytes } => {
                if let Some(content) = text_str(bytes) {
                    self.add_file(path, content)
                } else {
                    self.skipped = self.skipped.saturating_add(1);
                    Ok(())
                }
            }
            CandidateInput::TooLarge { .. } | CandidateInput::Skipped { .. } => {
                self.skipped = self.skipped.saturating_add(1);
                Ok(())
            }
        }
    }

    fn add_file(&mut self, path: &str, content: &str) -> Result<(), IngestError> {
        let file_bytes = (path.len() + content.len()) as u64;
        let new_total = self.storage.total_bytes().saturating_add(file_bytes);
        if new_total > MAX_OUTPUT_SIZE {
            return Err(self.output_too_large_error(path, content));
        }
        self.storage.push(path, content);
        Ok(())
    }

    fn output_too_large_error(&self, path: &str, content: &str) -> IngestError {
        let record = FileRecord { path, content };
        IngestError::output_too_large(largest_files(
            self.storage.iter().chain(std::iter::once(record)),
        ))
    }

    /// Finalizes selection, sorting files lexicographically in-place, and returns a [`CollectionResult`].
    pub(crate) fn finish(mut self, label: String, revision: Option<String>) -> CollectionResult {
        self.storage.sort_by_path();
        CollectionResult {
            label,
            revision,
            files: self.storage,
            skipped: self.skipped,
        }
    }
}

impl Default for Selector {
    fn default() -> Self {
        Self::new()
    }
}

/// Diagnostic ranking for digest size: selects up to five largest files.
pub(crate) fn largest_files<'a>(
    files: impl IntoIterator<Item = FileRecord<'a>>,
) -> Vec<(String, usize)> {
    let mut top: Vec<(&'a str, usize)> = Vec::with_capacity(6);

    for file in files {
        let size = file.content.len();
        if top.len() == 5 && size <= top.last().unwrap().1 {
            continue;
        }

        let index = top
            .iter()
            .position(|probe| probe.1 < size)
            .unwrap_or(top.len());
        if index < 5 {
            top.insert(index, (file.path, size));
            top.truncate(5);
        }
    }

    top.into_iter()
        .map(|(path, size)| (path.to_string(), size))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::policy::MAX_FILE_SIZE;

    fn file_candidate<'a>(path: &'a str, bytes: &'a [u8]) -> CandidateInput<'a> {
        CandidateInput::File { path, bytes }
    }

    #[test]
    fn candidate_budget_enforces_limit() {
        let mut candidates = CandidateBudget::default();
        for _ in 0..MAX_FILE_COUNT {
            candidates.add(None).unwrap();
        }
        assert!(candidates.add(None).is_err());
    }

    #[test]
    fn candidate_overflow_includes_largest_directories_diagnostic()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut budget = CandidateBudget::default();
        for index in 0..8_000 {
            budget.add(Some(&format!("vendor/pkg_{index}.rs")))?;
        }
        for index in 0..2_000 {
            budget.add(Some(&format!("src/file_{index}.rs")))?;
        }
        let err = budget.add(Some("extra/one.rs")).unwrap_err();
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
            budget.add(Some(&path)).unwrap();
        }
        let err = budget.add(Some("dir_00000/extra.rs")).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("repository contains more than 10000 files"));
        assert!(msg.contains("largest directories:"));
        assert!(msg.contains("- dir_00000/ (2 files)"));
    }

    #[test]
    fn extracts_dir_slice_from_paths() {
        assert_eq!(extract_dir_slice(Some("src/core/decide.rs")), "src/");
        assert_eq!(extract_dir_slice(Some("root_file.txt")), "<root>/");
        assert_eq!(extract_dir_slice(None), "<unknown>/");
    }

    #[test]
    fn text_str_detects_binary_and_handles_probe_boundary() {
        let mut bin = vec![b'a'; 1024];
        bin[12] = 0;
        assert!(text_str(&bin).is_none());

        let mut bin_tail = vec![b'b'; 5000];
        bin_tail[4500] = 0;
        assert!(text_str(&bin_tail).is_none());

        let mut bytes = vec![b'a'; 4095];
        bytes.extend_from_slice("€tail\n".as_bytes());
        let content = text_str(&bytes);
        assert!(content.is_some());
        assert!(content.unwrap().ends_with("€tail\n"));

        let mut bytes_with_nul = vec![b'a'; 4095];
        bytes_with_nul.extend_from_slice("€\0tail\n".as_bytes());
        assert!(text_str(&bytes_with_nul).is_none());
    }

    #[test]
    fn feed_classifies_text_binary_and_size_boundaries() -> Result<(), Box<dyn std::error::Error>> {
        let mut selector = Selector::new();
        let exact_bytes = vec![b'x'; MAX_FILE_SIZE as usize];
        selector.feed(file_candidate("a.txt", b"hello"))?;
        selector.feed(file_candidate("bin.bin", &[0, 1, 2]))?;
        selector.feed(file_candidate("bad_utf8", &[0xff]))?;
        selector.feed(CandidateInput::TooLarge { path: "big.txt" })?;
        selector.feed(CandidateInput::Skipped {
            path: Some("missing.txt"),
        })?;
        selector.feed(file_candidate("exact.txt", &exact_bytes))?;

        let collection = selector.finish("test".into(), None);
        assert_eq!(collection.files.len(), 2);
        assert_eq!(collection.files.iter().next().unwrap().path, "a.txt");
        assert_eq!(collection.files.iter().nth(1).unwrap().path, "exact.txt");
        assert_eq!(collection.skipped, 4);
        Ok(())
    }

    #[test]
    fn skipped_candidate_is_counted() -> Result<(), Box<dyn std::error::Error>> {
        let mut selector = Selector::new();
        selector.feed(CandidateInput::Skipped {
            path: Some("missing.png"),
        })?;
        selector.feed(file_candidate("kept.txt", b"kept"))?;
        let collection = selector.finish("test".into(), None);
        assert_eq!(collection.files.len(), 1);
        assert_eq!(collection.skipped, 1);
        Ok(())
    }

    #[test]
    fn feed_counts_candidate_before_content() -> Result<(), Box<dyn std::error::Error>> {
        let mut selector = Selector::new();
        for _ in 0..MAX_FILE_COUNT {
            selector.feed(file_candidate("bin.txt", &[0]))?;
        }
        assert!(matches!(
            selector.feed(file_candidate("extra.txt", b"ok")),
            Err(IngestError::CandidateLimitExceeded(..))
        ));
        Ok(())
    }

    #[test]
    fn output_overflow_includes_largest_files_diagnostic() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut selector = Selector::new();
        let small = vec![b'a'; 100];
        let medium = vec![b'b'; 50_000];
        let large = vec![b'x'; MAX_OUTPUT_SIZE as usize];
        selector.feed(file_candidate("small.txt", &small))?;
        selector.feed(file_candidate("medium.txt", &medium))?;
        let err = selector
            .feed(file_candidate("large.txt", &large))
            .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("digest exceeds the 20 MiB output limit"));
        assert!(message.contains("largest files:"));
        assert!(message.contains("large.txt"));
        assert!(message.contains("medium.txt"));
        Ok(())
    }

    #[test]
    fn selector_accumulates_decisions_and_respects_size_hint() {
        let mut selector = Selector::from_size_hint((10, Some(10)));
        selector.feed(file_candidate("a.txt", b"hello")).unwrap();
        selector
            .feed(CandidateInput::Skipped {
                path: Some("b.bin"),
            })
            .unwrap();
        let res = selector.finish("test".to_string(), None);
        assert_eq!(res.files.len(), 1);
        assert_eq!(res.skipped, 1);
    }

    #[test]
    fn selector_decide_and_finish_accepts_overlong_path_component() {
        let mut selector = Selector::new();
        let long_len = 70_000;
        let path = format!("{}/file.rs", "a".repeat(long_len));
        selector
            .feed(CandidateInput::File {
                path: &path,
                bytes: b"fn main() {}",
            })
            .unwrap();
        let res = selector.finish("test".to_string(), None);
        assert_eq!(res.files.len(), 1);
        assert_eq!(
            res.files.iter().next().unwrap().path.len(),
            long_len + "/file.rs".len()
        );
    }

    #[test]
    fn largest_files_selects_top_five_with_stability() {
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
        let result = largest_files(files);
        assert_eq!(result.len(), 5);
        assert_eq!(result[0], ("f6.txt".into(), 6));
        assert_eq!(result[1], ("f5_a.txt".into(), 5));
        assert_eq!(result[2], ("f5_b.txt".into(), 5));
        assert_eq!(result[3], ("f4.txt".into(), 4));
        assert_eq!(result[4], ("f3.txt".into(), 3));
    }
}
