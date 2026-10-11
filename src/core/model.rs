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

/// Output serialization format for the LLM digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DigestFormat {
    /// Delimiter-separated plain text format (default).
    #[default]
    Text,
    /// Anthropic and LLM-optimized XML structured format.
    Xml,
}

/// Ingestion options controlling destination, remote revision, and serialization format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Target destination for the rendered digest. Defaults to `digest.txt`.
    pub output: OutputTarget,
    /// Optional GitHub branch, tag, or commit SHA. Invalid for local sources.
    pub reference: Option<String>,
    /// Serialization format for the digest. Defaults to plain text.
    pub format: DigestFormat,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            output: OutputTarget::File(PathBuf::from("digest.txt")),
            reference: None,
            format: DigestFormat::Text,
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
    /// Approximate token count of the rendered digest.
    pub estimated_tokens: usize,
}

/// One digest entry. Owned by the domain core; rendered by `digest`.
#[derive(Debug, Clone)]
pub(crate) struct FileRecord {
    pub(crate) path: String,
    pub(crate) content: String,
}

/// Raw files and metadata collected by the shell before digest rendering.
#[derive(Debug, Clone)]
pub(crate) struct CollectionResult {
    pub(crate) label: String,
    pub(crate) revision: Option<String>,
    pub(crate) files: Vec<FileRecord>,
    pub(crate) skipped: usize,
    pub(crate) trie: crate::core::digest::BudgetTrie,
}
