//! Candidate observation, budget accounting, and storage accumulation.

use std::collections::HashMap;

use crate::core::error::IngestError;
use crate::core::model::CollectionResult;
use crate::core::policy::{MAX_FILE_COUNT, MAX_OUTPUT_SIZE};
use crate::core::storage::{FileRecord, FileStorage, largest_files};

/// Linear proof token demonstrating a candidate slot was reserved in [`CandidateBudget`].
#[derive(Debug)]
#[must_use = "candidate slot must be recorded into the selector"]
pub(crate) struct CandidateSlot {
    _priv: (),
}

/// UTF-8 text predicate: NUL-free bytes only.
#[inline]
fn text_str(bytes: &[u8]) -> Option<&str> {
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

    /// Reserves a candidate slot against budget limits before reading content bytes.
    ///
    /// # Errors
    ///
    /// Returns [`IngestError::CandidateLimitExceeded`] if file count overflows the policy limit.
    pub(crate) fn reserve_candidate(
        &mut self,
        path: Option<&str>,
    ) -> Result<CandidateSlot, IngestError> {
        self.candidates.add(path)?;
        Ok(CandidateSlot { _priv: () })
    }

    /// Consumes a reserved candidate slot and accumulates the file if UTF-8 text and within output limits.
    ///
    /// # Errors
    ///
    /// Returns [`IngestError::OutputTooLarge`] if output size exceeds the policy limit.
    pub(crate) fn record_file(
        &mut self,
        _slot: CandidateSlot,
        path: &str,
        bytes: &[u8],
    ) -> Result<(), IngestError> {
        if let Some(content) = text_str(bytes) {
            self.add_file(path, content)
        } else {
            self.skipped = self.skipped.saturating_add(1);
            Ok(())
        }
    }

    /// Consumes a reserved candidate slot for a skipped candidate.
    pub(crate) fn record_skipped(&mut self, _slot: CandidateSlot) {
        self.skipped = self.skipped.saturating_add(1);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::policy::MAX_FILE_SIZE;

    fn feed_file(selector: &mut Selector, path: &str, bytes: &[u8]) -> Result<(), IngestError> {
        let slot = selector.reserve_candidate(Some(path))?;
        selector.record_file(slot, path, bytes)
    }

    fn feed_skipped(selector: &mut Selector, path: Option<&str>) -> Result<(), IngestError> {
        let slot = selector.reserve_candidate(path)?;
        selector.record_skipped(slot);
        Ok(())
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
        feed_file(&mut selector, "a.txt", b"hello")?;
        feed_file(&mut selector, "bin.bin", &[0, 1, 2])?;
        feed_file(&mut selector, "bad_utf8", &[0xff])?;
        feed_skipped(&mut selector, Some("big.txt"))?;
        feed_skipped(&mut selector, Some("missing.txt"))?;
        feed_file(&mut selector, "exact.txt", &exact_bytes)?;

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
        feed_skipped(&mut selector, Some("missing.png"))?;
        feed_file(&mut selector, "kept.txt", b"kept")?;
        let collection = selector.finish("test".into(), None);
        assert_eq!(collection.files.len(), 1);
        assert_eq!(collection.skipped, 1);
        Ok(())
    }

    #[test]
    fn feed_counts_candidate_before_content() -> Result<(), Box<dyn std::error::Error>> {
        let mut selector = Selector::new();
        for _ in 0..MAX_FILE_COUNT {
            feed_file(&mut selector, "bin.txt", &[0])?;
        }
        assert!(matches!(
            feed_file(&mut selector, "extra.txt", b"ok"),
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
        feed_file(&mut selector, "small.txt", &small)?;
        feed_file(&mut selector, "medium.txt", &medium)?;
        let err = feed_file(&mut selector, "large.txt", &large).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("digest exceeds the 20 MiB output limit"));
        assert!(message.contains("largest files:"));
        assert!(message.contains("large.txt"));
        assert!(message.contains("medium.txt"));
        Ok(())
    }

    #[test]
    fn selector_accumulates_decisions() {
        let mut selector = Selector::new();
        feed_file(&mut selector, "a.txt", b"hello").unwrap();
        feed_skipped(&mut selector, Some("b.bin")).unwrap();
        let res = selector.finish("test".to_string(), None);
        assert_eq!(res.files.len(), 1);
        assert_eq!(res.skipped, 1);
    }

    #[test]
    fn selector_decide_and_finish_accepts_overlong_path_component() {
        let mut selector = Selector::new();
        let long_len = 70_000;
        let path = format!("{}/file.rs", "a".repeat(long_len));
        feed_file(&mut selector, &path, b"fn main() {}").unwrap();
        let res = selector.finish("test".to_string(), None);
        assert_eq!(res.files.len(), 1);
        assert_eq!(
            res.files.iter().next().unwrap().path.len(),
            long_len + "/file.rs".len()
        );
    }

    #[test]
    fn reserve_candidate_enforces_budget_before_recording() {
        let mut selector = Selector::new();
        for i in 0..MAX_FILE_COUNT {
            let path = format!("file_{i}.txt");
            let slot = selector.reserve_candidate(Some(&path)).unwrap();
            selector.record_file(slot, &path, b"content").unwrap();
        }

        // 10,001st candidate must fail during reservation before recording any bytes
        let overflow_err = selector
            .reserve_candidate(Some("overflow.txt"))
            .unwrap_err();
        match overflow_err {
            IngestError::CandidateLimitExceeded(..) => {}
            other => panic!("expected CandidateLimitExceeded, got {other:?}"),
        }
    }
}
