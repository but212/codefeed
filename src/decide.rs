use crate::digest::DigestBudget;
use crate::error::IngestError;
use crate::limits::MAX_FILE_COUNT;
use crate::model::FileRecord;

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
    pub(crate) fn new(label: &str, revision: Option<&str>) -> Result<Self, IngestError> {
        Ok(Self {
            files: Vec::new(),
            candidates: CandidateBudget::default(),
            digest: DigestBudget::new(label, revision)?,
        })
    }

    pub(crate) fn decide(&mut self, item: Item) -> Result<(), IngestError> {
        if let Probe::Ignored = item.probe {
            return Ok(());
        }
        self.candidates.add()?;
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
        self.digest.add(&file)?;
        self.files.push(file);
        Ok(())
    }

    pub(crate) fn into_parts(self) -> (Vec<FileRecord>, usize) {
        (self.files, self.digest.skipped())
    }
}

/// Candidate-count budget owned by the selector that consumes it.
#[derive(Default)]
pub(crate) struct CandidateBudget {
    count: usize,
}

impl CandidateBudget {
    pub(crate) fn add(&mut self) -> Result<(), IngestError> {
        self.count = self
            .count
            .checked_add(1)
            .ok_or_else(|| IngestError::InvalidMetadata("file count overflow".into()))?;
        if self.count > MAX_FILE_COUNT {
            return Err(IngestError::InvalidMetadata(format!(
                "repository contains more than {MAX_FILE_COUNT} files"
            )));
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
