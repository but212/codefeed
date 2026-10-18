use std::path::PathBuf;

use crate::core::error::IngestError;
use crate::core::storage::FileStorage;

/// Ingestion source location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Local filesystem directory.
    Local(PathBuf),
    /// Public or authenticated GitHub repository.
    Github {
        /// Repository owner (user or organization).
        owner: String,
        /// Repository name.
        repo: String,
        /// Optional GitHub branch, tag, or commit SHA.
        reference: Option<String>,
    },
}

impl Source {
    /// Returns a new `Source` instance with the specified Git revision reference.
    ///
    /// # Errors
    ///
    /// Returns [`IngestError::InvalidSource`] if `reference` is empty/whitespace or
    /// if called on a [`Source::Local`] variant.
    pub fn with_reference(self, reference: Option<String>) -> Result<Self, IngestError> {
        if reference
            .as_ref()
            .is_some_and(|r| r.trim_ascii().is_empty())
        {
            return Err(IngestError::invalid_source("--ref cannot be empty"));
        }
        match self {
            Self::Github { owner, repo, .. } => Ok(Self::Github {
                owner,
                repo,
                reference,
            }),
            Self::Local(_) => {
                if reference.is_some() {
                    Err(IngestError::invalid_source(
                        "--ref is only valid for GitHub sources",
                    ))
                } else {
                    Ok(self)
                }
            }
        }
    }
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
    /// Creates a new `RunConfig` with the given source, output target, and format.
    pub const fn new(source: Source, output: OutputTarget, format: DigestFormat) -> Self {
        Self {
            source,
            output,
            format,
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

/// Raw files and metadata collected by the shell before digest rendering.
#[derive(Debug, Clone)]
pub(crate) struct CollectionResult {
    pub(crate) label: String,
    pub(crate) revision: Option<String>,
    pub(crate) files: FileStorage,
    pub(crate) skipped: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_with_reference_validates_and_updates_reference() {
        let gh = Source::Github {
            owner: "rust-lang".into(),
            repo: "rust".into(),
            reference: None,
        };
        let updated = gh.with_reference(Some("main".into())).unwrap();
        assert_eq!(
            updated,
            Source::Github {
                owner: "rust-lang".into(),
                repo: "rust".into(),
                reference: Some("main".into()),
            }
        );

        let cleared = updated.with_reference(None).unwrap();
        assert_eq!(
            cleared,
            Source::Github {
                owner: "rust-lang".into(),
                repo: "rust".into(),
                reference: None,
            }
        );

        let empty_err = cleared
            .clone()
            .with_reference(Some("   ".into()))
            .unwrap_err();
        assert!(
            matches!(empty_err, IngestError::InvalidSource(msg) if msg.contains("--ref cannot be empty"))
        );

        let local = Source::Local(PathBuf::from("."));
        let local_ok = local.clone().with_reference(None).unwrap();
        assert_eq!(local_ok, local);

        let local_err = local.with_reference(Some("main".into())).unwrap_err();
        assert!(
            matches!(local_err, IngestError::InvalidSource(msg) if msg.contains("--ref is only valid for GitHub sources"))
        );
    }
}
