//! Candidate observation, budget accounting, and storage accumulation.

use std::collections::HashMap;

use crate::core::error::IngestError;
use crate::core::model::CollectionResult;
use crate::core::policy::{MAX_FILE_COUNT, MAX_FILE_SIZE, MAX_OUTPUT_SIZE, PathStatus};
use crate::core::storage::{FileStorage, SortedFiles};

/// Shell observation of a candidate file content pass.
pub(crate) enum Observation<'b> {
    /// File was missing/unlinked/unreadable after candidate reservation.
    Unavailable,
    /// Bounded content bytes read up to `limit + 1`.
    Bytes(&'b [u8]),
}

/// Linear proof token demonstrating a candidate slot was reserved in [`CandidateBudget`].
#[derive(Debug)]
#[must_use = "candidate slot must be recorded into the selector"]
pub(crate) struct CandidateSlot<'a> {
    path: &'a str,
}

enum ContentResult<'a> {
    Valid(&'a str),
    Skipped,
}

/// UTF-8 text predicate: NUL-free bytes only, returning classified result.
#[inline]
fn classify_content(bytes: &[u8]) -> ContentResult<'_> {
    if bytes.len() as u64 > MAX_FILE_SIZE {
        return ContentResult::Skipped;
    }
    if bytes.contains(&0) {
        return ContentResult::Skipped;
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => ContentResult::Valid(s),
        Err(_) => ContentResult::Skipped,
    }
}

/// Candidate-count budget owned by the selector that consumes it.
#[derive(Default, Debug)]
struct CandidateBudget {
    count: usize,
    dir_counts: HashMap<String, usize>,
}

impl CandidateBudget {
    /// Adds a single observed candidate path to the budget accounting.
    pub(crate) fn add(&mut self, path: &str) -> Result<(), IngestError> {
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
        Ok(())
    }

    fn overflow_error(&self) -> IngestError {
        IngestError::candidate_limit_exceeded(
            MAX_FILE_COUNT,
            top_overflow_directories(&self.dir_counts),
        )
    }
}

fn extract_dir_slice(path: &str) -> &str {
    if path.is_empty() {
        "<unknown>/"
    } else {
        match path.find('/') {
            Some(index) => &path[..=index],
            None => "<root>/",
        }
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
        Self {
            storage: FileStorage::new(),
            candidates: CandidateBudget {
                count: 0,
                dir_counts: HashMap::with_capacity(32),
            },
            skipped: 0,
        }
    }

    /// Reserves a candidate slot against budget limits before reading content bytes.
    ///
    /// # Errors
    ///
    /// Returns [`IngestError::CandidateLimitExceeded`] if file count overflows the policy limit.
    pub(crate) fn reserve_candidate<'a>(
        &mut self,
        path: &'a str,
    ) -> Result<CandidateSlot<'a>, IngestError> {
        self.candidates.add(path)?;
        Ok(CandidateSlot { path })
    }

    /// Consumes a reserved candidate slot and accumulates the file if content is available,
    /// valid UTF-8 text, and within output limits; otherwise increments skipped count.
    pub(crate) fn record_observation<'b>(
        &mut self,
        slot: CandidateSlot<'_>,
        observation: Observation<'b>,
    ) -> Result<(), IngestError> {
        let content_res = match observation {
            Observation::Unavailable => ContentResult::Skipped,
            Observation::Bytes(bytes) => classify_content(bytes),
        };

        match content_res {
            ContentResult::Valid(content) => self.add_file(slot.path, content),
            ContentResult::Skipped => {
                self.skipped = self.skipped.saturating_add(1);
                Ok(())
            }
        }
    }

    /// Reserves a candidate slot and immediately records it as skipped without reading content.
    fn record_skipped(&mut self, path: &str) -> Result<(), IngestError> {
        let _slot = self.reserve_candidate(path)?;
        self.skipped = self.skipped.saturating_add(1);
        Ok(())
    }

    /// Handles entry classification state (`PathStatus`), reserving candidate slots,
    /// evaluating pre-I/O size hints lazily after slot reservation, and executing content reading only when valid.
    pub(crate) fn record_entry<'b, H, F>(
        &mut self,
        status: PathStatus,
        path: &str,
        size_hint: H,
        read: F,
    ) -> Result<(), IngestError>
    where
        H: FnOnce() -> Option<u64>,
        F: FnOnce(u64) -> Result<Observation<'b>, IngestError>,
    {
        match status {
            PathStatus::Ignored => Ok(()),
            PathStatus::NonUtf8Candidate => self.record_skipped(path),
            PathStatus::Valid => self.record_with(path, size_hint, read),
        }
    }

    /// Reserves a candidate slot, evaluates optional pre-I/O size hint against limits after reservation,
    /// passes the file size limit to the reader callback if within budget,
    /// and accumulates the file or increments skipped count based on core policy decisions.
    ///
    /// Per-file size enforcement occurs in two stages:
    /// 1. Pre-I/O size hint evaluation (`size_hint() > MAX_FILE_SIZE`) skips reading the file body entirely.
    /// 2. Body inspection in [`record_observation`] verifies actual read bytes against [`MAX_FILE_SIZE`].
    fn record_with<'b, H, F>(
        &mut self,
        path: &str,
        size_hint: H,
        read: F,
    ) -> Result<(), IngestError>
    where
        H: FnOnce() -> Option<u64>,
        F: FnOnce(u64) -> Result<Observation<'b>, IngestError>,
    {
        let slot = self.reserve_candidate(path)?;
        if let Some(hint) = size_hint()
            && hint > MAX_FILE_SIZE
        {
            self.skipped = self.skipped.saturating_add(1);
            return Ok(());
        }
        let observation = read(MAX_FILE_SIZE)?;
        self.record_observation(slot, observation)
    }

    fn add_file(&mut self, path: &str, content: &str) -> Result<(), IngestError> {
        self.storage.push(path, content);
        // Early fail output bounding check:
        // Accumulated raw arena bytes (path_arena + content_arena) serve as a strict lower
        // bound for final rendered digest size (rendered_bytes >= raw_bytes). Checking here
        // aborts ingestion early to save memory and I/O before final rendering pass.
        if self.storage.total_bytes() > MAX_OUTPUT_SIZE {
            return Err(IngestError::output_too_large(
                crate::core::storage::largest_files(self.storage.iter()),
            ));
        }
        Ok(())
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
        let slot = selector.reserve_candidate(path)?;
        selector.record_observation(slot, Observation::Bytes(bytes))
    }

    fn feed_skipped(selector: &mut Selector, path: &str) -> Result<(), IngestError> {
        selector.record_skipped(path)
    }

    #[test]
    fn candidate_budget_enforces_limit() {
        let mut candidates = CandidateBudget::default();
        for _ in 0..MAX_FILE_COUNT {
            candidates.add("").unwrap();
        }
        assert!(candidates.add("").is_err());
    }

    #[test]
    fn candidate_overflow_includes_largest_directories_diagnostic()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut budget = CandidateBudget::default();
        for index in 0..8_000 {
            budget.add(&format!("vendor/pkg_{index}.rs"))?;
        }
        for index in 0..2_000 {
            budget.add(&format!("src/file_{index}.rs"))?;
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
            budget.add(&path).unwrap();
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
        assert_eq!(extract_dir_slice(""), "<unknown>/");
        assert_eq!(extract_dir_slice("vendor/"), "vendor/");
    }

    #[test]
    fn classify_content_detects_binary_nul_anywhere() {
        let mut bin = vec![b'a'; 1024];
        bin[12] = 0;
        assert!(matches!(classify_content(&bin), ContentResult::Skipped));

        let mut bin_tail = vec![b'b'; 5000];
        bin_tail[4500] = 0;
        assert!(matches!(
            classify_content(&bin_tail),
            ContentResult::Skipped
        ));

        let mut bytes = vec![b'a'; 4095];
        bytes.extend_from_slice("€tail\n".as_bytes());
        let res = classify_content(&bytes);
        assert!(matches!(res, ContentResult::Valid(content) if content.ends_with("€tail\n")));

        let mut bytes_with_nul = vec![b'a'; 4095];
        bytes_with_nul.extend_from_slice("€\0tail\n".as_bytes());
        assert!(matches!(
            classify_content(&bytes_with_nul),
            ContentResult::Skipped
        ));
    }

    #[test]
    fn feed_classifies_text_binary_and_size_boundaries() -> Result<(), Box<dyn std::error::Error>> {
        let mut selector = Selector::new();
        let exact_bytes = vec![b'x'; MAX_FILE_SIZE as usize];
        feed_file(&mut selector, "a.txt", b"hello")?;
        feed_file(&mut selector, "bin.bin", &[0, 1, 2])?;
        feed_file(&mut selector, "bad_utf8", &[0xff])?;
        feed_skipped(&mut selector, "big.txt")?;
        feed_skipped(&mut selector, "missing.txt")?;
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
        feed_skipped(&mut selector, "missing.png")?;
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
        let slot = selector.reserve_candidate("large.txt")?;
        let err = selector
            .add_file(slot.path, std::str::from_utf8(&large).unwrap())
            .unwrap_err();
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
        feed_skipped(&mut selector, "b.bin").unwrap();
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
    fn non_utf8_candidate_is_recorded_and_counted_as_skipped() {
        let mut selector = Selector::new();
        let slot = selector.reserve_candidate("").unwrap();
        selector
            .record_observation(slot, Observation::Unavailable)
            .unwrap();
        let res = selector.finish("test".to_string(), None);
        assert_eq!(res.files.len(), 0);
        assert_eq!(res.skipped, 1);
    }

    #[test]
    fn reserve_candidate_enforces_budget_before_recording() {
        let mut selector = Selector::new();
        for i in 0..MAX_FILE_COUNT {
            let path = format!("file_{i}.txt");
            let slot = selector.reserve_candidate(&path).unwrap();
            selector
                .record_observation(slot, Observation::Bytes(b"content"))
                .unwrap();
        }

        // 10,001st candidate must fail during reservation before recording any bytes
        let overflow_err = selector.reserve_candidate("overflow.txt").unwrap_err();
        match overflow_err {
            IngestError::CandidateLimitExceeded(..) => {}
            other => panic!("expected CandidateLimitExceeded, got {other:?}"),
        }
    }

    #[test]
    fn record_skipped_records_slot_and_increments_skipped_count() {
        let mut selector = Selector::new();
        selector.record_skipped("skipped.bin").unwrap();
        let res = selector.finish("test".to_string(), None);
        assert_eq!(res.files.len(), 0);
        assert_eq!(res.skipped, 1);
    }

    #[test]
    fn record_observation_accumulates_file_on_success() {
        let mut selector = Selector::new();
        feed_file(&mut selector, "valid.txt", b"hello").unwrap();
        let collection = selector.finish("test".into(), None);
        assert_eq!(collection.files.len(), 1);
    }

    #[test]
    fn non_utf8_prefix_skipped_correctly() {
        let mut selector = Selector::new();
        selector.record_skipped("dir/sub/").unwrap();
        let collection = selector.finish("test".into(), None);
        assert_eq!(collection.skipped, 1);
    }

    #[test]
    fn record_with_skips_io_when_size_hint_exceeds_limit() {
        let mut selector = Selector::new();
        let mut reader_called = false;
        selector
            .record_with(
                "big.bin",
                || Some(MAX_FILE_SIZE + 1),
                |_| {
                    reader_called = true;
                    Ok(Observation::Unavailable)
                },
            )
            .unwrap();
        assert!(
            !reader_called,
            "reader must not be called when size_hint > MAX_FILE_SIZE"
        );
        let collection = selector.finish("test".into(), None);
        assert_eq!(collection.skipped, 1);
    }

    #[test]
    fn record_with_executes_reader_only_when_within_budget() {
        let mut selector = Selector::new();
        let mut reader_called = false;
        selector
            .record_with(
                "valid.rs",
                || None,
                |_limit| {
                    reader_called = true;
                    Ok(Observation::Bytes(b"pub fn test() {}"))
                },
            )
            .unwrap();
        assert!(reader_called);
        let res = selector.finish("test".to_string(), None);
        assert_eq!(res.files.len(), 1);
        assert_eq!(res.skipped, 0);

        let mut overflow_selector = Selector::new();
        for i in 0..MAX_FILE_COUNT {
            let path = format!("file_{i}.txt");
            overflow_selector.record_skipped(&path).unwrap();
        }
        let mut overflow_reader_called = false;
        let err = overflow_selector.record_with(
            "overflow.rs",
            || None,
            |_limit| {
                overflow_reader_called = true;
                Ok(Observation::Bytes(b"content"))
            },
        );
        assert!(err.is_err());
        assert!(
            !overflow_reader_called,
            "reader callback must not be invoked when candidate budget is exceeded"
        );
    }
}
