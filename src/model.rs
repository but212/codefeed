use std::path::PathBuf;

use crate::digest::FileRecord;
use crate::error::IngestError;

pub(crate) const MAX_FILE_SIZE: u64 = 1024 * 1024;
pub(crate) const MAX_OUTPUT_SIZE: u64 = 20 * 1024 * 1024;
pub(crate) const MAX_FILE_COUNT: usize = 10_000;
pub(crate) const MAX_JSON_SIZE: u64 = 8 * 1024 * 1024;
pub(crate) const MAX_REMOTE_IGNORE_FILES: usize = 256;
pub(crate) const MAX_REMOTE_IGNORE_BYTES: u64 = 8 * 1024 * 1024;
pub(crate) const REMOTE_ROOT: &str = "/__local_ingest_repo";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Local(PathBuf),
    GitHub { owner: String, repo: String },
}

#[derive(Debug, Clone)]
pub struct Options {
    pub output: PathBuf,
    pub reference: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            output: PathBuf::from("digest.txt"),
            reference: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestReport {
    pub output: PathBuf,
    pub included_files: usize,
    pub skipped_files: usize,
}

pub(crate) enum CollectionResult {
    Local {
        label: String,
        files: Vec<FileRecord>,
        skipped: usize,
    },
    GitHub {
        label: String,
        revision: String,
        files: Vec<FileRecord>,
        skipped: usize,
    },
}

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

pub(crate) fn text_content(bytes: Vec<u8>) -> Option<String> {
    if bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}
