use crate::digest::{DigestBudget, FileRecord};
use crate::{CandidateBudget, IngestError, text_content};

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
