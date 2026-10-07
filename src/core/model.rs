use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Local(PathBuf),
    GitHub { owner: String, repo: String },
}

/// Where the digest is delivered. Distinguishes a filesystem path from the
/// standard output stream at the type level; there is no sentinel path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputTarget {
    File(PathBuf),
    Stdout,
}

#[derive(Debug, Clone)]
pub struct Options {
    pub output: OutputTarget,
    pub reference: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            output: OutputTarget::File(PathBuf::from("digest.txt")),
            reference: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestReport {
    pub output: OutputTarget,
    pub included_files: usize,
    pub skipped_files: usize,
}

/// One digest entry. Owned by the domain core; rendered by `digest`.
#[derive(Debug)]
pub(crate) struct FileRecord {
    pub(crate) path: String,
    pub(crate) content: String,
}

#[derive(Debug)]
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
