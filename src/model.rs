use std::path::PathBuf;

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

/// One digest entry. Owned by the domain core; rendered by `digest`.
#[derive(Debug)]
pub(crate) struct FileRecord {
    pub(crate) path: String,
    pub(crate) content: String,
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
