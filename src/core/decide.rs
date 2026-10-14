use std::collections::HashMap;

use crate::core::digest::BudgetTrie;
use crate::core::error::IngestError;
use crate::core::model::{CollectionResult, FileRecord, FileStorage};
use crate::core::policy::{MAX_FILE_COUNT, MAX_OUTPUT_SIZE};

/// Path text as observed by the imperative shell. Non-UTF-8 paths are still
/// candidates and must be counted, then skipped.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ItemPath {
    Utf8(String),
    NonUtf8,
}

impl ItemPath {
    #[inline]
    pub fn as_utf8_str(&self) -> Option<&str> {
        match self {
            Self::Utf8(s) => Some(s.as_str()),
            Self::NonUtf8 => None,
        }
    }
}

/// Result of the shell's bounded resource acquisition. `TooLarge` and `Missing`
/// carry no content and consume a candidate slot.
#[derive(Clone)]
pub enum Probe {
    Bytes(Vec<u8>),
    Missing,
    TooLarge,
}

pub struct Item {
    pub path: ItemPath,
    pub probe: Probe,
}

/// Pure decision result evaluating an observed candidate item against content,
/// encoding, and size invariants.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ItemDecision<'a> {
    Included { path: &'a str, content: &'a str },
    Skipped { path: Option<&'a str> },
}

impl<'a> ItemDecision<'a> {
    #[inline]
    pub(crate) fn path(&self) -> Option<&'a str> {
        match self {
            Self::Included { path, .. } => Some(path),
            Self::Skipped { path } => *path,
        }
    }
}

impl Item {
    #[inline]
    pub(crate) fn evaluate(&self) -> ItemDecision<'_> {
        match (&self.path, &self.probe) {
            (ItemPath::Utf8(path), Probe::Bytes(bytes)) => match text_str(bytes) {
                Some(content) => ItemDecision::Included {
                    path: path.as_str(),
                    content,
                },
                None => ItemDecision::Skipped {
                    path: Some(path.as_str()),
                },
            },
            (ItemPath::Utf8(path), Probe::Missing | Probe::TooLarge) => ItemDecision::Skipped {
                path: Some(path.as_str()),
            },
            (ItemPath::NonUtf8, _) => ItemDecision::Skipped { path: None },
        }
    }
}

/// Pure per-item selection and budget accounting. Owns no I/O; the shell feeds
/// observations and reads the outcome through [`Selector::finish`].
pub struct Selector {
    storage: FileStorage,
    candidates: CandidateBudget,
    content_bytes: u64,
    skipped: usize,
}

impl Selector {
    pub fn new() -> Self {
        Self::with_capacity(512, 16 * 1024, 256 * 1024)
    }

    pub fn with_capacity(spans_cap: usize, path_bytes: usize, content_bytes: usize) -> Self {
        Self {
            storage: FileStorage::with_capacity(spans_cap, path_bytes, content_bytes),
            candidates: CandidateBudget::default(),
            content_bytes: 0,
            skipped: 0,
        }
    }

    #[inline]
    pub fn check_candidate_limit<'a, I>(paths: I) -> Result<(), IngestError>
    where
        I: IntoIterator<Item = Option<&'a str>>,
    {
        CandidateBudget::check_limit(paths)
    }

    pub fn from_size_hint(hint: (usize, Option<usize>)) -> Self {
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

    pub(crate) fn accumulate(&mut self, decision: ItemDecision<'_>) -> Result<(), IngestError> {
        self.candidates.add(decision.path())?;
        match decision {
            ItemDecision::Included { path, content } => self.add_file(path, content),
            ItemDecision::Skipped { .. } => {
                self.skipped = self.skipped.saturating_add(1);
                Ok(())
            }
        }
    }

    pub fn decide(&mut self, item: Item) -> Result<(), IngestError> {
        self.accumulate(item.evaluate())
    }

    fn add_file(&mut self, path: &str, content: &str) -> Result<(), IngestError> {
        let file_bytes = (path.len() + content.len()) as u64;
        let new_total = self.content_bytes.saturating_add(file_bytes);
        if new_total > MAX_OUTPUT_SIZE {
            let record = FileRecord { path, content };
            return Err(IngestError::OutputTooLarge {
                largest_files: crate::core::digest::largest_files(
                    self.storage.iter().chain(std::iter::once(record)),
                ),
            });
        }
        self.content_bytes = new_total;
        self.storage.push(path, content);
        Ok(())
    }

    pub fn finish(mut self, label: String, revision: Option<String>) -> CollectionResult {
        self.storage.sort_by_path();
        let mut trie = BudgetTrie::new();
        for file in self.storage.iter() {
            trie.insert(file.path);
        }
        CollectionResult {
            label,
            revision,
            files: self.storage,
            skipped: self.skipped,
            trie,
        }
    }
}

impl Default for Selector {
    fn default() -> Self {
        Self::new()
    }
}

/// Candidate-count budget owned by the selector that consumes it.
#[derive(Default)]
pub struct CandidateBudget {
    count: usize,
    dir_runs: Vec<(String, usize)>,
}

impl CandidateBudget {
    pub fn check_limit<'a, I>(paths: I) -> Result<(), IngestError>
    where
        I: IntoIterator<Item = Option<&'a str>>,
    {
        let iter = paths.into_iter();
        if matches!(iter.size_hint().1, Some(upper) if upper <= MAX_FILE_COUNT) {
            return Ok(());
        }
        let mut budget = Self::default();
        for path in iter {
            budget.add(path)?;
        }
        Ok(())
    }

    pub fn add(&mut self, path: Option<&str>) -> Result<(), IngestError> {
        self.count = self
            .count
            .checked_add(1)
            .ok_or_else(|| IngestError::InvalidMetadata("file count overflow".into()))?;
        self.record_dir_run(extract_dir_slice(path));

        if self.count > MAX_FILE_COUNT {
            return Err(self.overflow_error());
        }
        Ok(())
    }

    fn record_dir_run(&mut self, dir_slice: &str) {
        if let Some(entry) = self
            .dir_runs
            .iter_mut()
            .rev()
            .take(16)
            .find(|(dir, _)| dir == dir_slice)
        {
            entry.1 += 1;
        } else {
            self.dir_runs.push((dir_slice.to_owned(), 1));
        }
    }

    fn overflow_error(&self) -> IngestError {
        let mut aggregated: HashMap<String, usize> = HashMap::new();
        for (dir, count) in &self.dir_runs {
            *aggregated.entry(dir.clone()).or_insert(0) += count;
        }
        IngestError::CandidateLimitExceeded {
            limit: MAX_FILE_COUNT,
            largest_directories: top_overflow_directories(&aggregated),
        }
    }
}

fn extract_dir_slice(path: Option<&str>) -> &str {
    match path {
        Some(p) => match p.find('/') {
            Some(idx) => &p[..=idx],
            None => "<root>/",
        },
        None => "<unknown>/",
    }
}

fn top_overflow_directories(dir_counts: &HashMap<String, usize>) -> Vec<(String, usize)> {
    let mut dirs: Vec<(String, usize)> = dir_counts.iter().map(|(d, c)| (d.clone(), *c)).collect();
    dirs.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    dirs.truncate(3);
    dirs
}

/// UTF-8 text predicate: NUL-free bytes only.
#[inline]
pub fn text_str(bytes: &[u8]) -> Option<&str> {
    if bytes.contains(&0) {
        return None;
    }
    std::str::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::policy::{MAX_FILE_COUNT, MAX_FILE_SIZE, MAX_OUTPUT_SIZE};

    fn decide_item(path: &str, probe: Probe) -> Item {
        Item {
            path: ItemPath::Utf8(path.to_owned()),
            probe,
        }
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
    fn candidate_budget_enforces_limit() {
        let mut candidates = CandidateBudget::default();
        for _ in 0..MAX_FILE_COUNT {
            candidates.add(None).unwrap();
        }
        assert!(candidates.add(None).is_err());
    }

    #[test]
    fn candidate_budget_check_limit_validates_batch() {
        let under_limit = (0..MAX_FILE_COUNT).map(|_| Some("dir/file.txt"));
        assert!(CandidateBudget::check_limit(under_limit).is_ok());

        let over_limit = (0..=MAX_FILE_COUNT).map(|_| Some("dir/file.txt"));
        assert!(CandidateBudget::check_limit(over_limit).is_err());
    }

    #[test]
    fn decide_classifies_text_binary_and_size_boundaries() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut selector = Selector::new();
        selector.decide(decide_item("a.txt", Probe::Bytes(b"hello".to_vec())))?;
        selector.decide(decide_item("bin.bin", Probe::Bytes(vec![0, 1, 2])))?;
        selector.decide(decide_item("bad_utf8", Probe::Bytes(vec![0xff])))?;
        selector.decide(decide_item("big.txt", Probe::TooLarge))?;
        selector.decide(decide_item("missing.txt", Probe::Missing))?;
        selector.decide(decide_item(
            "exact.txt",
            Probe::Bytes(vec![b'x'; MAX_FILE_SIZE as usize]),
        ))?;

        let collection = selector.finish("test".into(), None);
        assert_eq!(collection.files.len(), 2);
        assert_eq!(collection.files.get_at(0).unwrap().path, "a.txt");
        assert_eq!(collection.files.get_at(1).unwrap().path, "exact.txt");
        assert_eq!(collection.skipped, 4);
        Ok(())
    }

    #[test]
    fn missing_probe_is_skipped() -> Result<(), Box<dyn std::error::Error>> {
        let mut selector = Selector::new();
        selector.decide(decide_item("missing.png", Probe::Missing))?;
        selector.decide(decide_item("kept.txt", Probe::Bytes(b"kept".to_vec())))?;
        let collection = selector.finish("test".into(), None);
        assert_eq!(collection.files.len(), 1);
        assert_eq!(collection.skipped, 1);
        Ok(())
    }

    #[test]
    fn item_resolve_classifies_all_variants() {
        let missing = Item {
            path: ItemPath::Utf8("binary.png".into()),
            probe: Probe::Missing,
        };
        match missing.evaluate() {
            ItemDecision::Skipped { path: Some(path) } => assert_eq!(path, "binary.png"),
            _ => panic!("expected Skipped(Some)"),
        }

        let included = Item {
            path: ItemPath::Utf8("included.txt".into()),
            probe: Probe::Bytes(b"hello".to_vec()),
        };
        match included.evaluate() {
            ItemDecision::Included { path, content } => {
                assert_eq!(path, "included.txt");
                assert_eq!(content, "hello");
            }
            _ => panic!("expected Included"),
        }

        let binary = Item {
            path: ItemPath::Utf8("binary.bin".into()),
            probe: Probe::Bytes(vec![0, 1, 2]),
        };
        match binary.evaluate() {
            ItemDecision::Skipped { path: Some(path) } => assert_eq!(path, "binary.bin"),
            _ => panic!("expected Skipped(Some)"),
        }

        let non_utf8 = Item {
            path: ItemPath::NonUtf8,
            probe: Probe::Bytes(b"text".to_vec()),
        };
        assert!(matches!(
            non_utf8.evaluate(),
            ItemDecision::Skipped { path: None }
        ));
    }

    #[test]
    fn decide_counts_candidate_before_content() -> Result<(), Box<dyn std::error::Error>> {
        let mut selector = Selector::new();
        for _ in 0..MAX_FILE_COUNT {
            selector.decide(decide_item("bin.txt", Probe::Bytes(vec![0])))?;
        }
        assert!(matches!(
            selector.decide(decide_item("extra.txt", Probe::Bytes(b"ok".to_vec()))),
            Err(IngestError::CandidateLimitExceeded { .. })
        ));
        Ok(())
    }

    #[test]
    fn output_overflow_includes_largest_files_diagnostic() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut selector = Selector::new();
        selector.decide(decide_item("small.txt", Probe::Bytes(vec![b'a'; 100])))?;
        selector.decide(decide_item("medium.txt", Probe::Bytes(vec![b'b'; 50_000])))?;
        let err = selector
            .decide(decide_item(
                "large.txt",
                Probe::Bytes(vec![b'x'; MAX_OUTPUT_SIZE as usize]),
            ))
            .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("digest exceeds the 20 MiB output limit"));
        assert!(message.contains("largest files:"));
        assert!(message.contains("large.txt"));
        assert!(message.contains("medium.txt"));
        Ok(())
    }

    #[test]
    fn candidate_overflow_includes_largest_directories_diagnostic()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut selector = Selector::new();
        for index in 0..8_000 {
            selector.decide(decide_item(
                &format!("vendor/pkg_{index}.rs"),
                Probe::Bytes(b"x".to_vec()),
            ))?;
        }
        for index in 0..2_000 {
            selector.decide(decide_item(
                &format!("src/file_{index}.rs"),
                Probe::Bytes(b"x".to_vec()),
            ))?;
        }
        let err = selector
            .decide(decide_item("extra/one.rs", Probe::Bytes(b"x".to_vec())))
            .unwrap_err();
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
    fn item_evaluation_separates_decisions_without_heap_allocation() {
        let utf8_item = Item {
            path: ItemPath::Utf8("src/lib.rs".to_string()),
            probe: Probe::Bytes(b"pub fn run() {}".to_vec()),
        };
        assert_eq!(
            utf8_item.evaluate(),
            ItemDecision::Included {
                path: "src/lib.rs",
                content: "pub fn run() {}",
            }
        );

        let binary_item = Item {
            path: ItemPath::Utf8("assets/image.png".to_string()),
            probe: Probe::Bytes(vec![0x89, 0x50, 0x00, 0x47]),
        };
        assert_eq!(
            binary_item.evaluate(),
            ItemDecision::Skipped {
                path: Some("assets/image.png"),
            }
        );

        let missing_item = Item {
            path: ItemPath::Utf8("missing.txt".to_string()),
            probe: Probe::Missing,
        };
        assert_eq!(
            missing_item.evaluate(),
            ItemDecision::Skipped {
                path: Some("missing.txt"),
            }
        );

        let non_utf8_item = Item {
            path: ItemPath::NonUtf8,
            probe: Probe::Missing,
        };
        assert_eq!(
            non_utf8_item.evaluate(),
            ItemDecision::Skipped { path: None }
        );
    }

    #[test]
    fn selector_accumulates_decisions_and_respects_size_hint() {
        let mut selector = Selector::from_size_hint((10, Some(10)));
        selector
            .accumulate(ItemDecision::Included {
                path: "a.txt",
                content: "hello",
            })
            .unwrap();
        selector
            .accumulate(ItemDecision::Skipped {
                path: Some("b.bin"),
            })
            .unwrap();
        let res = selector.finish("test".to_string(), None);
        assert_eq!(res.files.len(), 1);
        assert_eq!(res.skipped, 1);
    }
}
