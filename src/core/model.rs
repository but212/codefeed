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
        /// Optional GitHub branch, tag, or commit SHA.
        reference: Option<String>,
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

/// Ingestion options controlling destination and serialization format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Target destination for the rendered digest. Defaults to `digest.txt`.
    pub output: OutputTarget,
    /// Serialization format for the digest. Defaults to plain text.
    pub format: DigestFormat,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            output: OutputTarget::File(PathBuf::from("digest.txt")),
            format: DigestFormat::Text,
        }
    }
}

/// Execution configuration unifying source, output destination, and format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunConfig {
    /// Ingestion source location.
    pub source: Source,
    /// Target destination for the rendered digest.
    pub output: OutputTarget,
    /// Serialization format for the digest.
    pub format: DigestFormat,
}

impl RunConfig {
    pub fn new(source: Source, output: OutputTarget, format: DigestFormat) -> Self {
        Self {
            source,
            output,
            format,
        }
    }

    pub fn from_source_and_options(source: Source, options: Options) -> Self {
        Self {
            source,
            output: options.output,
            format: options.format,
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
pub struct FileRecord {
    pub path: String,
    pub content: String,
}

/// Raw files and metadata collected by the shell before digest rendering.
#[derive(Debug, Clone)]
pub struct CollectionResult {
    pub label: String,
    pub revision: Option<String>,
    pub files: Vec<FileRecord>,
    pub skipped: usize,
    pub trie: crate::core::digest::BudgetTrie,
    pub format: DigestFormat,
}
