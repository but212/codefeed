use std::collections::HashMap;

use crate::core::digest::DigestBudget;
use crate::core::error::IngestError;
use crate::core::model::{DigestFormat, FileRecord};
use crate::core::policy::MAX_FILE_COUNT;

/// Path text as observed by the imperative shell. Non-UTF-8 paths are still
/// candidates and must be counted, then skipped.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ItemPath {
    Utf8(String),
    NonUtf8,
}

/// Result of the shell's bounded resource acquisition. `Ignored`, `TooLarge`,
/// and `Missing` carry no content. `TooLarge` and `Missing` still consume a
/// candidate slot; `Ignored` is excluded before candidate accounting.
#[derive(Clone)]
pub enum Probe {
    Bytes(Vec<u8>),
    Missing,
    TooLarge,
    Ignored,
}

pub struct Item {
    pub path: ItemPath,
    pub probe: Probe,
}

enum ItemResolution {
    Ignored,
    Included(FileRecord),
    Skipped(Option<String>),
}

impl Item {
    fn resolve(self) -> ItemResolution {
        match (self.path, self.probe) {
            (_, Probe::Ignored) => ItemResolution::Ignored,
            (ItemPath::Utf8(path), Probe::Bytes(bytes)) => match text_content(bytes) {
                Some(content) => ItemResolution::Included(FileRecord { path, content }),
                None => ItemResolution::Skipped(Some(path)),
            },
            (ItemPath::Utf8(path), _) => ItemResolution::Skipped(Some(path)),
            (ItemPath::NonUtf8, _) => ItemResolution::Skipped(None),
        }
    }
}

/// Pure per-item selection and budget accounting. Owns no I/O; the shell feeds
/// observations and reads the outcome through [`Selector::into_parts`].
pub struct Selector {
    files: Vec<FileRecord>,
    candidates: CandidateBudget,
    digest: DigestBudget,
}

impl Selector {
    pub fn new(
        label: &str,
        revision: Option<&str>,
        format: DigestFormat,
    ) -> Result<Self, IngestError> {
        Ok(Self {
            files: Vec::new(),
            candidates: CandidateBudget::default(),
            digest: DigestBudget::new(label, revision, format)?,
        })
    }

    pub fn decide(&mut self, item: Item) -> Result<(), IngestError> {
        match item.resolve() {
            ItemResolution::Ignored => Ok(()),
            ItemResolution::Included(file) => {
                self.candidates.add(Some(file.path.as_str()))?;
                self.add_file(file)
            }
            ItemResolution::Skipped(path) => {
                self.candidates.add(path.as_deref())?;
                self.digest.skip()
            }
        }
    }

    fn add_file(&mut self, file: FileRecord) -> Result<(), IngestError> {
        if let Err(err) = self.digest.add(&file) {
            if matches!(err, IngestError::OutputTooLarge { .. }) {
                return Err(IngestError::OutputTooLarge {
                    largest_files: crate::core::digest::largest_files(
                        self.files.iter().chain(std::iter::once(&file)),
                    ),
                });
            }
            return Err(err);
        }
        self.files.push(file);
        Ok(())
    }

    pub fn into_parts(
        self,
    ) -> (
        Vec<FileRecord>,
        usize,
        crate::core::digest::BudgetTrie,
        DigestFormat,
    ) {
        (
            self.files,
            self.digest.skipped(),
            self.digest.trie,
            self.digest.format,
        )
    }
}

/// Candidate-count budget owned by the selector that consumes it.
#[derive(Default)]
pub(crate) struct CandidateBudget {
    count: usize,
    dir_counts: HashMap<String, usize>,
}

impl CandidateBudget {
    pub(crate) fn add(&mut self, path: Option<&str>) -> Result<(), IngestError> {
        self.count = self
            .count
            .checked_add(1)
            .ok_or_else(|| IngestError::InvalidMetadata("file count overflow".into()))?;
        let dir_slice = extract_dir_slice(path);
        if let Some(count) = self.dir_counts.get_mut(dir_slice) {
            *count += 1;
        } else {
            self.dir_counts.insert(dir_slice.to_owned(), 1);
        }

        if self.count > MAX_FILE_COUNT {
            return Err(IngestError::CandidateLimitExceeded(
                format_overflow_diagnostic(&self.dir_counts),
            ));
        }
        Ok(())
    }

    /// Validates that total candidates do not exceed `MAX_FILE_COUNT`.
    /// When exceeded, returns `CandidateLimitExceeded` with diagnostic details.
    pub(crate) fn verify_count<'a>(
        paths: impl IntoIterator<Item = Option<&'a str>>,
        count: usize,
    ) -> Result<(), IngestError> {
        if count <= MAX_FILE_COUNT {
            return Ok(());
        }
        let mut budget = Self::default();
        paths.into_iter().try_for_each(|path| budget.add(path))
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

fn format_overflow_diagnostic(dir_counts: &HashMap<String, usize>) -> String {
    let mut dirs: Vec<(&str, usize)> = dir_counts.iter().map(|(d, c)| (d.as_str(), *c)).collect();
    dirs.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));

    let mut msg =
        format!("repository contains more than {MAX_FILE_COUNT} files\nlargest directories:");
    for (dir, count) in dirs.into_iter().take(3) {
        use std::fmt::Write;
        let _ = write!(msg, "\n  - {dir} ({count} files)");
    }
    msg
}

/// UTF-8 text predicate: NUL-free bytes only.
pub fn text_content(bytes: Vec<u8>) -> Option<String> {
    let s = String::from_utf8(bytes).ok()?;
    (!s.as_bytes().contains(&0)).then_some(s)
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
    fn text_content_detects_binary_and_handles_probe_boundary() {
        let mut bin = vec![b'a'; 1024];
        bin[12] = 0;
        assert!(text_content(bin).is_none());

        let mut bin_tail = vec![b'b'; 5000];
        bin_tail[4500] = 0;
        assert!(text_content(bin_tail).is_none());

        let mut bytes = vec![b'a'; 4095];
        bytes.extend_from_slice("€tail\n".as_bytes());
        let content = text_content(bytes);
        assert!(content.is_some());
        assert!(content.unwrap().ends_with("€tail\n"));

        let mut bytes_with_nul = vec![b'a'; 4095];
        bytes_with_nul.extend_from_slice("€\0tail\n".as_bytes());
        assert!(text_content(bytes_with_nul).is_none());
    }

    #[test]
    fn candidate_budget_enforces_limit_and_batch_verification() {
        let mut candidates = CandidateBudget::default();
        for _ in 0..MAX_FILE_COUNT {
            candidates.add(None).unwrap();
        }
        assert!(candidates.add(None).is_err());

        let items: Vec<Option<&str>> = (0..MAX_FILE_COUNT).map(|_| Some("src/file.rs")).collect();
        assert!(CandidateBudget::verify_count(items.iter().copied(), items.len()).is_ok());

        let overflowing: Vec<Option<&str>> =
            (0..=MAX_FILE_COUNT).map(|_| Some("src/file.rs")).collect();
        assert!(
            CandidateBudget::verify_count(overflowing.iter().copied(), overflowing.len()).is_err()
        );
    }

    #[test]
    fn decide_classifies_text_binary_and_size_boundaries() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut selector = Selector::new("repo", None, DigestFormat::Text)?;
        selector.decide(decide_item("a.txt", Probe::Bytes(b"hello".to_vec())))?;
        selector.decide(decide_item("bin.bin", Probe::Bytes(vec![0, 1, 2])))?;
        selector.decide(decide_item("bad_utf8", Probe::Bytes(vec![0xff])))?;
        selector.decide(decide_item("big.txt", Probe::TooLarge))?;
        selector.decide(decide_item("missing.txt", Probe::Missing))?;
        selector.decide(decide_item(
            "exact.txt",
            Probe::Bytes(vec![b'x'; MAX_FILE_SIZE as usize]),
        ))?;

        let (files, skipped, _, format) = selector.into_parts();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "a.txt");
        assert_eq!(files[1].path, "exact.txt");
        assert_eq!(skipped, 4);
        assert_eq!(format, DigestFormat::Text);
        Ok(())
    }

    #[test]
    fn github_ignored_is_not_skipped() -> Result<(), Box<dyn std::error::Error>> {
        let mut selector = Selector::new("repo", None, DigestFormat::Text)?;
        for _ in 0..MAX_FILE_COUNT {
            selector.decide(decide_item("ignored.txt", Probe::Ignored))?;
        }
        selector.decide(decide_item("kept.txt", Probe::Bytes(b"kept".to_vec())))?;
        let (files, skipped, _, _) = selector.into_parts();
        assert_eq!(files.len(), 1);
        assert_eq!(skipped, 0);
        Ok(())
    }

    #[test]
    fn item_resolve_classifies_all_variants() {
        let ignored = Item {
            path: ItemPath::Utf8("ignored.txt".into()),
            probe: Probe::Ignored,
        };
        assert!(matches!(ignored.resolve(), ItemResolution::Ignored));

        let included = Item {
            path: ItemPath::Utf8("included.txt".into()),
            probe: Probe::Bytes(b"hello".to_vec()),
        };
        match included.resolve() {
            ItemResolution::Included(file) => {
                assert_eq!(file.path, "included.txt");
                assert_eq!(file.content, "hello");
            }
            _ => panic!("expected Included"),
        }

        let binary = Item {
            path: ItemPath::Utf8("binary.bin".into()),
            probe: Probe::Bytes(vec![0, 1, 2]),
        };
        match binary.resolve() {
            ItemResolution::Skipped(Some(path)) => assert_eq!(path, "binary.bin"),
            _ => panic!("expected Skipped(Some)"),
        }

        let non_utf8 = Item {
            path: ItemPath::NonUtf8,
            probe: Probe::Bytes(b"text".to_vec()),
        };
        assert!(matches!(non_utf8.resolve(), ItemResolution::Skipped(None)));
    }

    #[test]
    fn decide_counts_candidate_before_content() -> Result<(), Box<dyn std::error::Error>> {
        let mut selector = Selector::new("repo", None, DigestFormat::Text)?;
        for _ in 0..MAX_FILE_COUNT {
            selector.decide(decide_item("bin.txt", Probe::Bytes(vec![0])))?;
        }
        assert!(matches!(
            selector.decide(decide_item("extra.txt", Probe::Bytes(b"ok".to_vec()))),
            Err(IngestError::CandidateLimitExceeded(_))
        ));
        Ok(())
    }

    #[test]
    fn output_overflow_includes_largest_files_diagnostic() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut selector = Selector::new("repo", None, DigestFormat::Text)?;
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
        let mut selector = Selector::new("repo", None, DigestFormat::Text)?;
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
}
