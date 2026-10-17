//! Selector accumulation, file storage feeding, and digest output limit enforcement.

use super::budget::CandidateBudget;
use super::item::{Item, ItemDecision};
use crate::core::error::IngestError;
use crate::core::model::{CollectionResult, FileRecord, FileStorage};
use crate::core::policy::{MAX_FILE_COUNT, MAX_OUTPUT_SIZE};
use crate::core::tree::BudgetTrie;

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
        let file_bytes = (path.len() + content.len()) as u64;
        let new_total = self.content_bytes.saturating_add(file_bytes);
        if new_total > MAX_OUTPUT_SIZE {
            return Err(self.output_too_large_error(path, content));
        }
        self.content_bytes = new_total;
        self.storage.push(path, content);
        Ok(())
    }

    #[cold]
    #[inline(never)]
    fn output_too_large_error(&self, path: &str, content: &str) -> IngestError {
        let record = FileRecord { path, content };
        IngestError::output_too_large(largest_files(
            self.storage.iter().chain(std::iter::once(record)),
        ))
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
    use std::borrow::Cow;

    use super::*;
    use crate::core::decide::item::{ItemPath, Probe};
    use crate::core::policy::MAX_FILE_SIZE;

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
        selector.decide(decide_item("kept.txt", bytes_probe(b"kept".to_vec())))?;
        let collection = selector.finish("test".into(), None);
        assert_eq!(collection.files.len(), 1);
        assert_eq!(collection.skipped, 1);
        Ok(())
    }

    #[test]
    fn decide_counts_candidate_before_content() -> Result<(), Box<dyn std::error::Error>> {
        let mut selector = Selector::new();
        for _ in 0..MAX_FILE_COUNT {
            selector.decide(decide_item("bin.txt", bytes_probe(vec![0])))?;
        }
        assert!(matches!(
            selector.decide(decide_item("extra.txt", bytes_probe(b"ok".to_vec()))),
            Err(IngestError::CandidateLimitExceeded(..))
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

    #[test]
    fn selector_decide_and_finish_accepts_overlong_path_component() {
        let mut selector = Selector::new();
        let long_len = 70_000;
        let path = format!("{}/file.rs", "a".repeat(long_len));
        selector
            .accumulate(ItemDecision::Included {
                path: &path,
                content: "fn main() {}",
            })
            .unwrap();
        let res = selector.finish("test".to_string(), None);
        assert_eq!(res.files.len(), 1);
        let child = res.trie.nodes[0].first_child;
        assert_eq!(res.trie.node_name(child).len(), long_len);
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
