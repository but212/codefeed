use crate::core::digest::DigestBudget;
use crate::core::error::IngestError;
use crate::core::model::{DigestFormat, FileRecord};
use crate::core::policy::MAX_FILE_COUNT;

/// Path text as observed by the imperative shell. Non-UTF-8 paths are still
/// candidates and must be counted, then skipped.
pub(crate) enum ItemPath {
    Utf8(String),
    NonUtf8,
}

/// Result of the shell's bounded resource acquisition. `Ignored`, `TooLarge`,
/// and `Missing` carry no content. `TooLarge` and `Missing` still consume a
/// candidate slot; `Ignored` is excluded before candidate accounting.
pub(crate) enum Probe {
    Bytes(Vec<u8>),
    Missing,
    TooLarge,
    Ignored,
}

pub(crate) struct Item {
    pub(crate) path: ItemPath,
    pub(crate) probe: Probe,
}

/// Pure per-item selection and budget accounting. Owns no I/O; the shell feeds
/// observations and reads the outcome through [`Selector::into_parts`].
pub(crate) struct Selector {
    files: Vec<FileRecord>,
    candidates: CandidateBudget,
    digest: DigestBudget,
}

impl Selector {
    pub(crate) fn new(
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

    pub(crate) fn decide(&mut self, item: Item) -> Result<(), IngestError> {
        if let Probe::Ignored = item.probe {
            return Ok(());
        }
        let path_str = match &item.path {
            ItemPath::Utf8(p) => Some(p.as_str()),
            ItemPath::NonUtf8 => None,
        };
        self.candidates.add(path_str)?;
        let ItemPath::Utf8(path) = item.path else {
            self.digest.skip()?;
            return Ok(());
        };
        let Probe::Bytes(bytes) = item.probe else {
            self.digest.skip()?;
            return Ok(());
        };
        let Some(content) = text_content(bytes) else {
            self.digest.skip()?;
            return Ok(());
        };
        let file = FileRecord { path, content };
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

    pub(crate) fn into_parts(self) -> (Vec<FileRecord>, usize, crate::core::digest::BudgetTrie) {
        (self.files, self.digest.skipped(), self.digest.trie)
    }
}

/// Candidate-count budget owned by the selector that consumes it.
#[derive(Default)]
pub(crate) struct CandidateBudget {
    count: usize,
    dir_counts: std::collections::HashMap<String, usize>,
}

impl CandidateBudget {
    pub(crate) fn add(&mut self, path: Option<&str>) -> Result<(), IngestError> {
        self.count = self
            .count
            .checked_add(1)
            .ok_or_else(|| IngestError::InvalidMetadata("file count overflow".into()))?;
        let dir_slice = match path {
            Some(p) => match p.find('/') {
                Some(idx) => &p[..=idx],
                None => "<root>/",
            },
            None => "<unknown>/",
        };
        if let Some(count) = self.dir_counts.get_mut(dir_slice) {
            *count += 1;
        } else {
            self.dir_counts.insert(dir_slice.to_owned(), 1);
        }

        if self.count > MAX_FILE_COUNT {
            let mut dirs: Vec<(&String, usize)> =
                self.dir_counts.iter().map(|(d, &c)| (d, c)).collect();
            dirs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
            dirs.truncate(3);

            let mut msg = format!(
                "repository contains more than {MAX_FILE_COUNT} files\nlargest directories:"
            );
            for (dir, count) in dirs {
                use std::fmt::Write;
                let _ = write!(msg, "\n  - {dir} ({count} files)");
            }
            return Err(IngestError::CandidateLimitExceeded(msg));
        }
        Ok(())
    }
}

/// UTF-8 text predicate: NUL-free bytes only.
fn text_content(bytes: Vec<u8>) -> Option<String> {
    if bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
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
    fn counts_candidate_before_content_filtering() {
        let mut candidates = CandidateBudget::default();
        for _ in 0..MAX_FILE_COUNT {
            candidates.add(None).unwrap();
        }
        assert!(candidates.add(None).is_err());
    }

    #[test]
    fn decide_includes_text_candidate() -> Result<(), Box<dyn std::error::Error>> {
        let mut selector = Selector::new("repo", None, DigestFormat::Text)?;
        selector.decide(decide_item("a.txt", Probe::Bytes(b"hello".to_vec())))?;
        let (files, skipped, _) = selector.into_parts();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "a.txt");
        assert_eq!(skipped, 0);
        Ok(())
    }

    #[test]
    fn decide_skips_binary_candidate() -> Result<(), Box<dyn std::error::Error>> {
        let mut selector = Selector::new("repo", None, DigestFormat::Text)?;
        selector.decide(decide_item("bin", Probe::Bytes(vec![0, 1, 2])))?;
        selector.decide(decide_item("bad", Probe::Bytes(vec![0xff])))?;
        let (files, skipped, _) = selector.into_parts();
        assert!(files.is_empty());
        assert_eq!(skipped, 2);
        Ok(())
    }

    #[test]
    fn decide_handles_size_boundary() -> Result<(), Box<dyn std::error::Error>> {
        let mut selector = Selector::new("repo", None, DigestFormat::Text)?;
        selector.decide(decide_item("big", Probe::TooLarge))?;
        selector.decide(decide_item("gone", Probe::Missing))?;
        selector.decide(decide_item(
            "exact.txt",
            Probe::Bytes(vec![b'x'; MAX_FILE_SIZE as usize]),
        ))?;
        let (files, skipped, _) = selector.into_parts();
        assert_eq!(files.len(), 1);
        assert_eq!(skipped, 2);
        Ok(())
    }

    #[test]
    fn github_ignored_is_not_skipped() -> Result<(), Box<dyn std::error::Error>> {
        let mut selector = Selector::new("repo", None, DigestFormat::Text)?;
        for _ in 0..MAX_FILE_COUNT {
            selector.decide(decide_item("ignored.txt", Probe::Ignored))?;
        }
        selector.decide(decide_item("kept.txt", Probe::Bytes(b"kept".to_vec())))?;
        let (files, skipped, _) = selector.into_parts();
        assert_eq!(files.len(), 1);
        assert_eq!(skipped, 0);
        Ok(())
    }

    #[test]
    fn decide_counts_candidate_before_content() -> Result<(), Box<dyn std::error::Error>> {
        let mut selector = Selector::new("repo", None, DigestFormat::Text)?;
        for index in 0..MAX_FILE_COUNT {
            selector.decide(decide_item(
                &format!("binary-{index}"),
                Probe::Bytes(vec![0]),
            ))?;
        }
        assert!(matches!(
            selector.decide(decide_item("extra", Probe::Bytes(b"ok".to_vec()))),
            Err(IngestError::CandidateLimitExceeded(_))
        ));
        Ok(())
    }

    #[test]
    fn decide_rejects_output_overflow() -> Result<(), Box<dyn std::error::Error>> {
        let mut selector = Selector::new("repo", None, DigestFormat::Text)?;
        let result = selector.decide(decide_item(
            "large.txt",
            Probe::Bytes(vec![b'x'; MAX_OUTPUT_SIZE as usize]),
        ));
        assert!(matches!(result, Err(IngestError::OutputTooLarge { .. })));
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
}
