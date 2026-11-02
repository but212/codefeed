use std::path::PathBuf;

use crate::core::policy::ExcludePatterns;
use crate::core::source::Source;

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

/// Execution configuration unifying source, output destination, format, and user exclusions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunConfig {
    /// Ingestion source location.
    pub source: Source,
    /// Target destination for the rendered digest.
    pub output: OutputTarget,
    /// Serialization format for the digest.
    pub format: DigestFormat,
    /// User exclusion patterns, applied on top of the source's own `.gitignore` rules.
    pub exclude: ExcludePatterns,
}

impl RunConfig {
    /// Creates a new `RunConfig` with the given source, output target, and format, and no user exclusions.
    #[must_use]
    pub fn new(source: Source, output: OutputTarget, format: DigestFormat) -> Self {
        Self {
            source,
            output,
            format,
            exclude: ExcludePatterns::default(),
        }
    }
}

/// Summary report emitted upon successful digest generation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct IngestReport {
    /// Output destination as requested in [`RunConfig`], not the canonical path the digest was written to.
    pub output: OutputTarget,
    /// Number of candidate files included in the rendered digest.
    pub included_files: usize,
    /// Number of candidate files skipped: over a size or path limit, non-UTF-8 path, unavailable, binary, or invalid UTF-8.
    pub skipped_files: usize,
    /// Approximate token count of the rendered digest.
    pub estimated_tokens: usize,
}
