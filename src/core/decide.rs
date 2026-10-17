use std::borrow::Cow;
use std::collections::HashMap;

use crate::core::error::IngestError;
use crate::core::model::{CollectionResult, FileRecord, FileStorage};
use crate::core::policy::{
    MAX_COMPONENT_LEN, MAX_FILE_COUNT, MAX_OUTPUT_SIZE, find_overlong_component,
};
use crate::core::tree::{BudgetTrie, InsertError};

/// Path text as observed by the imperative shell. Non-UTF-8 paths are still
/// candidates and must be counted, then skipped.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ItemPath<'a> {
    Utf8(Cow<'a, str>),
    NonUtf8,
}

impl<'a> ItemPath<'a> {
    #[inline]
    pub fn as_utf8_str(&self) -> Option<&str> {
        match self {
            Self::Utf8(s) => Some(s.as_ref()),
            Self::NonUtf8 => None,
        }
    }
}

impl<'a> From<&'a str> for ItemPath<'a> {
    fn from(s: &'a str) -> Self {
        Self::Utf8(Cow::Borrowed(s))
    }
}

impl<'a> From<String> for ItemPath<'a> {
    fn from(s: String) -> Self {
        Self::Utf8(Cow::Owned(s))
    }
}

/// Result of the shell's bounded resource acquisition. `TooLarge` and `Missing`
/// carry no content and consume a candidate slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probe<'a> {
    Bytes(Cow<'a, [u8]>),
    Missing,
    TooLarge,
}

impl<'a> From<&'a [u8]> for Probe<'a> {
    fn from(b: &'a [u8]) -> Self {
        Self::Bytes(Cow::Borrowed(b))
    }
}

impl<'a> From<Vec<u8>> for Probe<'a> {
    fn from(v: Vec<u8>) -> Self {
        Self::Bytes(Cow::Owned(v))
    }
}

#[derive(Clone, Debug)]
pub struct Item<'a> {
    pub path: ItemPath<'a>,
    pub probe: Probe<'a>,
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
    pub(crate) const fn path(&self) -> Option<&'a str> {
        match self {
            Self::Included { path, .. } => Some(path),
            Self::Skipped { path } => *path,
        }
    }
}

impl<'a> Item<'a> {
    #[inline]
    pub(crate) fn evaluate(&self) -> ItemDecision<'_> {
        match (&self.path, &self.probe) {
            (ItemPath::Utf8(path), Probe::Bytes(bytes)) => match text_str(bytes.as_ref()) {
                Some(content) => ItemDecision::Included {
                    path: path.as_ref(),
                    content,
                },
                None => ItemDecision::Skipped {
                    path: Some(path.as_ref()),
                },
            },
            (ItemPath::Utf8(path), Probe::Missing | Probe::TooLarge) => ItemDecision::Skipped {
                path: Some(path.as_ref()),
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
        let dir_runs_cap = (spans_cap / 16).clamp(16, 256);
        Self {
            storage: FileStorage::with_capacity(spans_cap, path_bytes, content_bytes),
            candidates: CandidateBudget::with_capacity(dir_runs_cap),
            content_bytes: 0,
            skipped: 0,
        }
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

    pub fn decide(&mut self, item: Item<'_>) -> Result<(), IngestError> {
        self.accumulate(item.evaluate())
    }

    fn add_file(&mut self, path: &str, content: &str) -> Result<(), IngestError> {
        if let Some(len) = find_overlong_component(path) {
            return Err(IngestError::PathComponentTooLong {
                path: path.to_string(),
                len,
                max: MAX_COMPONENT_LEN,
            });
        }

        let file_bytes = (path.len() + content.len()) as u64;
        let new_total = self.content_bytes.saturating_add(file_bytes);
        if new_total > MAX_OUTPUT_SIZE {
            let record = FileRecord { path, content };
            return Err(IngestError::OutputTooLarge {
                largest_files: largest_files(self.storage.iter().chain(std::iter::once(record))),
            });
        }
        self.content_bytes = new_total;
        self.storage.push(path, content);
        Ok(())
    }

    pub fn finish(
        mut self,
        label: String,
        revision: Option<String>,
    ) -> Result<CollectionResult, IngestError> {
        self.storage.sort_by_path();
        let mut trie = BudgetTrie::new();
        self.storage.iter().try_for_each(|file| {
            trie.insert(file.path).map_err(|err| match err {
                InsertError::ComponentTooLong { len } => IngestError::PathComponentTooLong {
                    path: file.path.to_string(),
                    len,
                    max: MAX_COMPONENT_LEN,
                },
            })
        })?;
        Ok(CollectionResult {
            label,
            revision,
            files: self.storage,
            skipped: self.skipped,
            trie,
        })
    }
}

impl Default for Selector {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug)]
pub(crate) enum CompactDir {
    Inline { len: u8, buf: [u8; 22] },
    Heap(Box<str>),
}

impl CompactDir {
    #[inline]
    pub(crate) fn as_str(&self) -> &str {
        match self {
            Self::Inline { len, buf } => std::str::from_utf8(&buf[..*len as usize]).unwrap_or(""),
            Self::Heap(s) => s.as_ref(),
        }
    }

    #[inline]
    pub(crate) fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Inline { len, buf } => &buf[..*len as usize],
            Self::Heap(s) => s.as_bytes(),
        }
    }
}

impl From<&str> for CompactDir {
    fn from(s: &str) -> Self {
        if s.len() <= 22 {
            let mut buf = [0u8; 22];
            buf[..s.len()].copy_from_slice(s.as_bytes());
            Self::Inline {
                len: s.len() as u8,
                buf,
            }
        } else {
            Self::Heap(s.into())
        }
    }
}

/// Candidate-count budget owned by the selector that consumes it.
#[derive(Default, Debug)]
pub struct CandidateBudget {
    count: usize,
    dir_runs: Vec<(CompactDir, usize)>,
}

impl CandidateBudget {
    pub const fn new() -> Self {
        Self {
            count: 0,
            dir_runs: Vec::new(),
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            count: 0,
            dir_runs: Vec::with_capacity(capacity),
        }
    }

    pub fn check_limit<'a, I>(paths: I) -> Result<(), IngestError>
    where
        I: IntoIterator<Item = Option<&'a str>>,
    {
        let mut iter = paths.into_iter();
        if matches!(iter.size_hint().1, Some(upper) if upper <= MAX_FILE_COUNT) {
            return Ok(());
        }
        let mut budget = Self::default();
        iter.try_for_each(|path| budget.add(path))
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
        let dir_bytes = dir_slice.as_bytes();
        if let Some(entry) = self
            .dir_runs
            .iter_mut()
            .rev()
            .take(16)
            .find(|(dir, _)| dir.as_bytes() == dir_bytes)
        {
            entry.1 += 1;
        } else {
            self.dir_runs.push((CompactDir::from(dir_slice), 1));
        }
    }

    fn overflow_error(&self) -> IngestError {
        let mut aggregated: HashMap<&str, usize> = HashMap::new();
        for (dir, count) in &self.dir_runs {
            *aggregated.entry(dir.as_str()).or_insert(0) += count;
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
            Some(index) => &p[..=index],
            None => "<root>/",
        },
        None => "<unknown>/",
    }
}

fn top_overflow_directories(dir_counts: &HashMap<&str, usize>) -> Vec<(String, usize)> {
    let mut dirs: Vec<(&str, usize)> = dir_counts.iter().map(|(&d, &c)| (d, c)).collect();
    dirs.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    dirs.truncate(3);
    dirs.into_iter().map(|(d, c)| (d.to_string(), c)).collect()
}

/// UTF-8 text predicate: NUL-free bytes only.
#[inline]
pub fn text_str(bytes: &[u8]) -> Option<&str> {
    if bytes.contains(&0) {
        return None;
    }
    std::str::from_utf8(bytes).ok()
}

// ---------------------------------------------------------------------------
// Diagnostic ranking for digest size
// ---------------------------------------------------------------------------

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
    use crate::core::policy::{MAX_FILE_COUNT, MAX_FILE_SIZE, MAX_OUTPUT_SIZE};

    fn bytes_probe<'a>(bytes: impl Into<Cow<'a, [u8]>>) -> Probe<'a> {
        Probe::Bytes(bytes.into())
    }

    fn decide_item<'a>(path: &'a str, probe: Probe<'a>) -> Item<'a> {
        Item {
            path: ItemPath::Utf8(Cow::Borrowed(path)),
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
        selector.decide(decide_item("a.txt", bytes_probe(b"hello".to_vec())))?;
        selector.decide(decide_item("bin.bin", bytes_probe(vec![0, 1, 2])))?;
        selector.decide(decide_item("bad_utf8", bytes_probe(vec![0xff])))?;
        selector.decide(decide_item("big.txt", Probe::TooLarge))?;
        selector.decide(decide_item("missing.txt", Probe::Missing))?;
        selector.decide(decide_item(
            "exact.txt",
            bytes_probe(vec![b'x'; MAX_FILE_SIZE as usize]),
        ))?;

        let collection = selector.finish("test".into(), None)?;
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
        selector.decide(decide_item("kept.txt", bytes_probe(b"kept".to_vec())))?;
        let collection = selector.finish("test".into(), None)?;
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
            probe: bytes_probe(b"hello".to_vec()),
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
            probe: bytes_probe(vec![0, 1, 2]),
        };
        match binary.evaluate() {
            ItemDecision::Skipped { path: Some(path) } => assert_eq!(path, "binary.bin"),
            _ => panic!("expected Skipped(Some)"),
        }

        let non_utf8 = Item {
            path: ItemPath::NonUtf8,
            probe: bytes_probe(b"text".to_vec()),
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
            selector.decide(decide_item("bin.txt", bytes_probe(vec![0])))?;
        }
        assert!(matches!(
            selector.decide(decide_item("extra.txt", bytes_probe(b"ok".to_vec()))),
            Err(IngestError::CandidateLimitExceeded { .. })
        ));
        Ok(())
    }

    #[test]
    fn output_overflow_includes_largest_files_diagnostic() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut selector = Selector::new();
        selector.decide(decide_item("small.txt", bytes_probe(vec![b'a'; 100])))?;
        selector.decide(decide_item("medium.txt", bytes_probe(vec![b'b'; 50_000])))?;
        let err = selector
            .decide(decide_item(
                "large.txt",
                bytes_probe(vec![b'x'; MAX_OUTPUT_SIZE as usize]),
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
                bytes_probe(b"x".to_vec()),
            ))?;
        }
        for index in 0..2_000 {
            selector.decide(decide_item(
                &format!("src/file_{index}.rs"),
                bytes_probe(b"x".to_vec()),
            ))?;
        }
        let err = selector
            .decide(decide_item("extra/one.rs", bytes_probe(b"x".to_vec())))
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
            path: ItemPath::Utf8(Cow::Borrowed("src/lib.rs")),
            probe: bytes_probe(b"pub fn run() {}".to_vec()),
        };
        assert_eq!(
            utf8_item.evaluate(),
            ItemDecision::Included {
                path: "src/lib.rs",
                content: "pub fn run() {}",
            }
        );

        let binary_item = Item {
            path: ItemPath::Utf8(Cow::Borrowed("assets/image.png")),
            probe: bytes_probe(vec![0x89, 0x50, 0x00, 0x47]),
        };
        assert_eq!(
            binary_item.evaluate(),
            ItemDecision::Skipped {
                path: Some("assets/image.png"),
            }
        );

        let missing_item = Item {
            path: ItemPath::Utf8(Cow::Borrowed("missing.txt")),
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
        let res = selector.finish("test".to_string(), None).unwrap();
        assert_eq!(res.files.len(), 1);
        assert_eq!(res.skipped, 1);
    }

    #[test]
    fn selector_decide_fails_fast_on_overlong_path_component() {
        let mut selector = Selector::new();
        let too_long = MAX_COMPONENT_LEN + 1;
        let path = format!("{}/file.rs", "a".repeat(too_long));
        let err = selector
            .accumulate(ItemDecision::Included {
                path: &path,
                content: "fn main() {}",
            })
            .unwrap_err();
        match err {
            IngestError::PathComponentTooLong {
                path: err_path,
                len,
                max,
            } => {
                assert_eq!(err_path, path);
                assert_eq!(len, too_long);
                assert_eq!(max, MAX_COMPONENT_LEN);
            }
            other => panic!("expected PathComponentTooLong, got {other:?}"),
        }
    }

    #[test]
    fn selector_finish_rejects_overlong_path_in_storage() {
        let mut selector = Selector::new();
        let too_long = MAX_COMPONENT_LEN + 1;
        let path = format!("{}/file.rs", "a".repeat(too_long));
        selector.storage.push(&path, "fn main() {}");
        let err = selector.finish("test".to_string(), None).unwrap_err();
        match err {
            IngestError::PathComponentTooLong {
                path: err_path,
                len,
                max,
            } => {
                assert_eq!(err_path, path);
                assert_eq!(len, too_long);
                assert_eq!(max, MAX_COMPONENT_LEN);
            }
            other => panic!("expected PathComponentTooLong, got {other:?}"),
        }
    }

    #[test]
    fn test_compact_dir_layout_and_parity() {
        assert!(std::mem::size_of::<CompactDir>() <= 24);
        assert!(std::mem::size_of::<(CompactDir, usize)>() <= 32);

        let short_dir = CompactDir::from("src/");
        assert_eq!(short_dir.as_str(), "src/");

        let long_str = "deeply/nested/directory/structure/path/";
        let long_dir = CompactDir::from(long_str);
        assert_eq!(long_dir.as_str(), long_str);
    }

    #[test]
    fn test_compact_dir_as_bytes() {
        let short_dir = CompactDir::from("src/");
        assert_eq!(short_dir.as_bytes(), b"src/");

        let long_str = "deeply/nested/directory/structure/path/";
        let long_dir = CompactDir::from(long_str);
        assert_eq!(long_dir.as_bytes(), long_str.as_bytes());
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
