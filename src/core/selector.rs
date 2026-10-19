//! Candidate observation, budget accounting, and storage accumulation.

use std::collections::HashMap;

use crate::core::error::IngestError;
use crate::core::model::CollectionResult;
use crate::core::policy::{MAX_FILE_COUNT, MAX_FILE_SIZE, MAX_OUTPUT_SIZE, PathStatus};
use crate::core::storage::{FileStorage, SortedFiles};

/// Outcome of evaluating an entry candidate against admission policy.
#[derive(Debug)]
pub(crate) enum Admission {
    /// Path is explicitly ignored by policy (e.g., hidden files, lockfiles, sensitive credentials).
    Ignored(Selector),
    /// Path was admitted as a candidate but skipped (e.g., non-UTF-8 path or pre-I/O size limit exceeded).
    Skipped(Selector),
    /// Path is valid and ready for content evaluation.
    Pending(PendingEntry),
}

/// An admitted candidate entry requiring linear completion via [`accept`](Self::accept) or [`skip`](Self::skip).
#[derive(Debug)]
#[must_use = "pending entry must be completed via accept or skip"]
pub(crate) struct PendingEntry {
    selector: Selector,
    path: String,
}

impl PendingEntry {
    /// Evaluates content bytes and either accumulates valid UTF-8 text or marks file as skipped.
    pub(crate) fn accept(mut self, bytes: &[u8]) -> Result<Selector, IngestError> {
        match classify_content(bytes) {
            Some(content) => self.selector.add_file(&self.path, content),
            None => {
                self.selector.skipped = self.selector.skipped.saturating_add(1);
                Ok(self.selector)
            }
        }
    }

    /// Skips the pending entry when file cannot be read or is unavailable.
    pub(crate) fn skip(mut self) -> Selector {
        self.selector.skipped = self.selector.skipped.saturating_add(1);
        self.selector
    }
}

/// UTF-8 text predicate: NUL-free bytes within size limit, returning string slice.
#[inline]
fn classify_content(bytes: &[u8]) -> Option<&str> {
    if bytes.len() as u64 > MAX_FILE_SIZE || bytes.contains(&0) {
        return None;
    }
    std::str::from_utf8(bytes).ok()
}

/// Candidate-count budget owned by the selector that consumes it.
#[derive(Default, Debug)]
struct CandidateBudget {
    count: usize,
    dir_counts: HashMap<String, usize>,
}

impl CandidateBudget {
    /// Adds a single observed candidate path to the budget accounting.
    pub(crate) fn add(mut self, path: &str) -> Result<Self, IngestError> {
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
#[derive(Debug)]
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

    /// Evaluates entry admission against policy and budget limits.
    ///
    /// - [`PathStatus::Ignored`] is ignored without consuming candidate budget.
    /// - [`PathStatus::NonUtf8Candidate`] or paths exceeding `size_hint` consume candidate budget and increment skipped count.
    /// - [`PathStatus::Valid`] within size limit consumes candidate budget and yields a [`PendingEntry`].
    pub(crate) fn admit(
        mut self,
        status: PathStatus,
        size_hint: Option<u64>,
    ) -> Result<Admission, IngestError> {
        match status {
            PathStatus::Ignored => Ok(Admission::Ignored(self)),
            PathStatus::NonUtf8Candidate(path) => {
                self.candidates = self.candidates.add(&path)?;
                self.skipped = self.skipped.saturating_add(1);
                Ok(Admission::Skipped(self))
            }
            PathStatus::Valid(path) => {
                self.candidates = self.candidates.add(&path)?;
                if let Some(hint) = size_hint
                    && hint > MAX_FILE_SIZE
                {
                    self.skipped = self.skipped.saturating_add(1);
                    Ok(Admission::Skipped(self))
                } else {
                    Ok(Admission::Pending(PendingEntry {
                        selector: self,
                        path,
                    }))
                }
            }
        }
    }

    fn add_file(mut self, path: &str, content: &str) -> Result<Self, IngestError> {
        self.storage = self.storage.push(path, content);
        // Early fail output bounding check:
        // Accumulated raw arena bytes (path_arena + content_arena) serve as a strict lower
        // bound for final rendered digest size (rendered_bytes >= raw_bytes). Checking here
        // aborts ingestion early to save memory and I/O before final rendering pass.
        if self.storage.total_bytes() > MAX_OUTPUT_SIZE {
            return Err(IngestError::output_too_large(
                crate::core::storage::largest_files(self.storage.iter()),
            ));
        }
        Ok(self)
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

    fn feed_file(selector: Selector, path: &str, bytes: &[u8]) -> Result<Selector, IngestError> {
        match selector.admit(PathStatus::Valid(path.to_string()), None)? {
            Admission::Pending(pending) => pending.accept(bytes),
            Admission::Ignored(s) | Admission::Skipped(s) => Ok(s),
        }
    }

    fn feed_skipped(selector: Selector, path: &str) -> Result<Selector, IngestError> {
        match selector.admit(PathStatus::Valid(path.to_string()), None)? {
            Admission::Pending(pending) => Ok(pending.skip()),
            Admission::Ignored(s) | Admission::Skipped(s) => Ok(s),
        }
    }

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
        assert_eq!(extract_dir_slice(""), "<unknown>/");
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
    fn feed_classifies_text_binary_and_size_boundaries() -> Result<(), Box<dyn std::error::Error>> {
        let selector = Selector::new();
        let exact_bytes = vec![b'x'; MAX_FILE_SIZE as usize];
        let selector = feed_file(selector, "a.txt", b"hello")?;
        let selector = feed_file(selector, "bin.bin", &[0, 1, 2])?;
        let selector = feed_file(selector, "bad_utf8", &[0xff])?;
        let selector = feed_skipped(selector, "big.txt")?;
        let selector = feed_skipped(selector, "missing.txt")?;
        let selector = feed_file(selector, "exact.txt", &exact_bytes)?;

        let collection = selector.finish("test".into(), None);
        assert_eq!(collection.files.len(), 2);
        assert_eq!(collection.files.iter().next().unwrap().path, "a.txt");
        assert_eq!(collection.files.iter().nth(1).unwrap().path, "exact.txt");
        assert_eq!(collection.skipped, 4);
        Ok(())
    }

    #[test]
    fn skipped_candidate_is_counted() -> Result<(), Box<dyn std::error::Error>> {
        let selector = Selector::new();
        let selector = feed_skipped(selector, "missing.png")?;
        let selector = feed_file(selector, "kept.txt", b"kept")?;
        let collection = selector.finish("test".into(), None);
        assert_eq!(collection.files.len(), 1);
        assert_eq!(collection.skipped, 1);
        Ok(())
    }

    #[test]
    fn feed_counts_candidate_before_content() -> Result<(), Box<dyn std::error::Error>> {
        let mut selector = Selector::new();
        for _ in 0..MAX_FILE_COUNT {
            selector = feed_file(selector, "bin.txt", &[0])?;
        }
        assert!(matches!(
            feed_file(selector, "extra.txt", b"ok"),
            Err(IngestError::CandidateLimitExceeded(..))
        ));
        Ok(())
    }

    #[test]
    fn output_overflow_includes_largest_files_diagnostic() -> Result<(), Box<dyn std::error::Error>>
    {
        let selector = Selector::new();
        let chunk = vec![b'x'; MAX_FILE_SIZE as usize];
        let mut selector = selector;
        for i in 0..19 {
            selector = feed_file(selector, &format!("file_{i:02}.txt"), &chunk)?;
        }
        let pending = match selector.admit(PathStatus::Valid("overflow.txt".into()), None)? {
            Admission::Pending(p) => p,
            _ => panic!("expected pending"),
        };
        let err = pending.accept(&chunk).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("digest exceeds the 20 MiB output limit"));
        assert!(message.contains("largest files:"));
        assert!(message.contains("file_00.txt"));
        Ok(())
    }

    #[test]
    fn selector_accumulates_decisions() {
        let selector = Selector::new();
        let selector = feed_file(selector, "a.txt", b"hello").unwrap();
        let selector = feed_skipped(selector, "b.bin").unwrap();
        let res = selector.finish("test".to_string(), None);
        assert_eq!(res.files.len(), 1);
        assert_eq!(res.skipped, 1);
    }

    #[test]
    fn selector_decide_and_finish_accepts_overlong_path_component() {
        let selector = Selector::new();
        let long_len = 70_000;
        let path = format!("{}/file.rs", "a".repeat(long_len));
        let selector = feed_file(selector, &path, b"fn main() {}").unwrap();
        let res = selector.finish("test".to_string(), None);
        assert_eq!(res.files.len(), 1);
        assert_eq!(
            res.files.iter().next().unwrap().path.len(),
            long_len + "/file.rs".len()
        );
    }

    #[test]
    fn non_utf8_candidate_is_recorded_and_counted_as_skipped() {
        let selector = Selector::new();
        let admission = selector
            .admit(PathStatus::NonUtf8Candidate("".into()), None)
            .unwrap();
        let selector = match admission {
            Admission::Skipped(s) => s,
            _ => panic!("expected skipped"),
        };
        let res = selector.finish("test".to_string(), None);
        assert_eq!(res.files.len(), 0);
        assert_eq!(res.skipped, 1);
    }

    #[test]
    fn admit_enforces_budget_before_accept() {
        let mut selector = Selector::new();
        for i in 0..MAX_FILE_COUNT {
            let path = format!("file_{i}.txt");
            match selector.admit(PathStatus::Valid(path), None).unwrap() {
                Admission::Pending(pending) => {
                    selector = pending.accept(b"content").unwrap();
                }
                _ => panic!("expected pending"),
            }
        }

        // 10,001st candidate must fail during admit before reading bytes
        let overflow_err = selector
            .admit(PathStatus::Valid("overflow.txt".to_string()), None)
            .unwrap_err();
        match overflow_err {
            IngestError::CandidateLimitExceeded(..) => {}
            other => panic!("expected CandidateLimitExceeded, got {other:?}"),
        }
    }

    #[test]
    fn pending_entry_skip_increments_skipped_count() {
        let selector = Selector::new();
        let pending = match selector
            .admit(PathStatus::Valid("skip.txt".to_string()), None)
            .unwrap()
        {
            Admission::Pending(p) => p,
            _ => panic!("expected pending"),
        };
        let selector = pending.skip();
        let res = selector.finish("test".to_string(), None);
        assert_eq!(res.files.len(), 0);
        assert_eq!(res.skipped, 1);
    }

    #[test]
    fn ignored_status_leaves_state_unchanged() {
        let selector = Selector::new();
        let admission = selector.admit(PathStatus::Ignored, None).unwrap();
        let selector = match admission {
            Admission::Ignored(s) => s,
            _ => panic!("expected ignored"),
        };
        let res = selector.finish("test".to_string(), None);
        assert_eq!(res.files.len(), 0);
        assert_eq!(res.skipped, 0);
    }

    #[test]
    fn non_utf8_prefix_skipped_correctly() {
        let selector = Selector::new();
        let admission = selector
            .admit(PathStatus::NonUtf8Candidate("dir/sub/".to_string()), None)
            .unwrap();
        let selector = match admission {
            Admission::Skipped(s) => s,
            _ => panic!("expected skipped"),
        };
        let collection = selector.finish("test".into(), None);
        assert_eq!(collection.skipped, 1);
    }

    #[test]
    fn admit_skips_when_size_hint_exceeds_limit() {
        let selector = Selector::new();
        let admission = selector
            .admit(
                PathStatus::Valid("big.bin".to_string()),
                Some(MAX_FILE_SIZE + 1),
            )
            .unwrap();
        let selector = match admission {
            Admission::Skipped(s) => s,
            _ => panic!("expected skipped admission"),
        };
        let collection = selector.finish("test".into(), None);
        assert_eq!(collection.skipped, 1);
    }
}
