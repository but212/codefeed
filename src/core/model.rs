use std::path::PathBuf;

/// Ingestion source location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Local filesystem directory.
    Local(PathBuf),
    /// Public or authenticated GitHub repository.
    GitHub {
        /// Repository owner (user or organization).
        owner: String,
        /// Repository name.
        repo: String,
    },
}

/// Where the digest is delivered. Distinguishes a filesystem path from the
/// standard output stream at the type level; there is no sentinel path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputTarget {
    /// Writes the digest atomically to a filesystem path.
    File(PathBuf),
    /// Writes the digest directly to standard output without file creation.
    Stdout,
}

/// Ingestion options controlling destination and remote revision.
#[derive(Debug, Clone)]
pub struct Options {
    /// Target destination for the rendered digest. Defaults to `digest.txt`.
    pub output: OutputTarget,
    /// Optional GitHub branch, tag, or commit SHA. Invalid for local sources.
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

/// Summary report emitted upon successful digest generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestReport {
    /// Target destination where the digest was delivered.
    pub output: OutputTarget,
    /// Number of candidate files included in the rendered digest.
    pub included_files: usize,
    /// Number of candidate files rejected due to size, encoding, or binary content.
    pub skipped_files: usize,
}

/// One digest entry. Owned by the domain core; rendered by `digest`.
#[derive(Debug)]
pub(crate) struct FileRecord {
    pub(crate) path: String,
    pub(crate) content: String,
}

/// Raw files and metadata collected by the shell before digest rendering.
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
