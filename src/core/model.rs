use std::path::PathBuf;

use crate::core::error::IngestError;
use crate::core::storage::SortedFiles;

/// Validated non-empty Git revision reference (branch, tag, or commit SHA).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GitRef(String);

impl GitRef {
    /// Creates a new `GitRef`, rejecting empty or whitespace-only strings.
    ///
    /// # Errors
    ///
    /// Returns [`IngestError::InvalidSource`] if `value` is empty or only whitespace.
    pub fn new(value: impl Into<String>) -> Result<Self, IngestError> {
        let string = value.into();
        if string.trim_ascii().is_empty() {
            return Err(IngestError::invalid_source("--ref cannot be empty"));
        }
        Ok(Self(string))
    }

    /// Borrows the reference as a string slice.
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::ops::Deref for GitRef {
    type Target = str;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::fmt::Display for GitRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Ingestion source location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Local filesystem directory.
    Local(PathBuf),
    /// Public or authenticated GitHub repository.
    #[non_exhaustive]
    Github {
        /// Repository owner (user or organization).
        owner: String,
        /// Repository name.
        repo: String,
        /// Optional GitHub branch, tag, or commit SHA.
        reference: Option<GitRef>,
    },
}

impl Source {
    /// Creates a new validated [`Source::Github`] instance.
    ///
    /// # Errors
    ///
    /// Returns [`IngestError::InvalidSource`] if `owner` or `repo` contain invalid characters,
    /// empty strings, or path traversals.
    pub fn github(
        owner: impl Into<String>,
        repo: impl Into<String>,
        reference: Option<GitRef>,
    ) -> Result<Self, IngestError> {
        let owner = owner.into();
        let repo = repo.into();
        if !crate::core::github::valid_github_name(&owner)
            || !crate::core::github::valid_github_name(&repo)
        {
            return Err(IngestError::invalid_source(
                "owner and repository names contain invalid characters",
            ));
        }
        Ok(Self::Github {
            owner,
            repo,
            reference,
        })
    }

    /// Parses an OS-native string argument into a [`Source`].
    pub(super) fn parse_os(value: &std::ffi::OsStr) -> Result<Self, IngestError> {
        match value.to_str() {
            Some(value) => Self::parse(value),
            None => Ok(Self::Local(PathBuf::from(value))),
        }
    }

    /// Parses a string slice into either a [`Source::Local`] or [`Source::Github`].
    ///
    /// # Errors
    ///
    /// Returns [`IngestError::UnsupportedSource`] if given an unsupported protocol
    /// (such as SSH) or [`IngestError::InvalidSource`] if the GitHub URL is malformed.
    fn parse(value: &str) -> Result<Self, IngestError> {
        if value.starts_with("http://") || value.starts_with("https://") {
            let (owner, repo) = crate::core::github::parse_github_url(value)?;
            return Self::github(owner, repo, None);
        }
        if value.starts_with("git@") || value.starts_with("ssh://") {
            return Err(IngestError::unsupported_source(
                "only local paths and public HTTPS GitHub URLs are supported",
            ));
        }
        Ok(Self::Local(value.into()))
    }

    /// Returns a new `Source` instance with the specified Git revision reference.
    ///
    /// # Errors
    ///
    /// Returns [`IngestError::InvalidSource`] if `reference` is [`Some`] when called on a [`Source::Local`] variant,
    /// or if the reference string is empty or only whitespace.
    pub fn with_reference(self, reference: Option<impl Into<String>>) -> Result<Self, IngestError> {
        match self {
            Self::Local(_) => {
                if reference.is_some() {
                    Err(IngestError::invalid_source(
                        "--ref is only valid for GitHub sources",
                    ))
                } else {
                    Ok(self)
                }
            }
            Self::Github { owner, repo, .. } => {
                let reference = reference.map(GitRef::new).transpose()?;
                Ok(Self::Github {
                    owner,
                    repo,
                    reference,
                })
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
#[non_exhaustive]
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
    pub(crate) files: SortedFiles,
    pub(crate) skipped: usize,
}

/// Constructs an [`IngestError::OutputTooLarge`] diagnostic error from an iterator of file records.
pub(crate) fn output_too_large_error<'a>(
    records: impl IntoIterator<Item = crate::core::storage::FileRecord<'a>>,
) -> IngestError {
    IngestError::output_too_large(crate::core::storage::largest_files(records))
}

impl CollectionResult {
    pub(crate) fn output_too_large_error(&self) -> IngestError {
        output_too_large_error(self.files.iter())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_ref_validates_and_provides_slice_access() {
        let valid = GitRef::new("v1.0.0").unwrap();
        assert_eq!(valid.as_str(), "v1.0.0");
        assert_eq!(&*valid, "v1.0.0");
        assert_eq!(valid.to_string(), "v1.0.0");

        assert!(GitRef::new("").is_err());
        assert!(GitRef::new("   ").is_err());
        assert!(GitRef::new("\t\n").is_err());
    }

    #[test]
    fn source_with_reference_validates_and_updates_reference() {
        let gh = Source::Github {
            owner: "rust-lang".into(),
            repo: "rust".into(),
            reference: None,
        };
        let updated = gh.clone().with_reference(Some("main")).unwrap();
        assert_eq!(
            updated,
            Source::Github {
                owner: "rust-lang".into(),
                repo: "rust".into(),
                reference: Some(GitRef::new("main").unwrap()),
            }
        );

        let cleared: Source = updated.with_reference(None::<&str>).unwrap();
        assert_eq!(
            cleared,
            Source::Github {
                owner: "rust-lang".into(),
                repo: "rust".into(),
                reference: None,
            }
        );

        let local = Source::Local(PathBuf::from("."));
        let local_ok: Source = local.clone().with_reference(None::<&str>).unwrap();
        assert_eq!(local_ok, local);

        let local_err = local.clone().with_reference(Some("main")).unwrap_err();
        assert!(
            matches!(local_err, IngestError::InvalidSource(msg) if msg.contains("--ref is only valid for GitHub sources"))
        );

        let local_empty_err = local.with_reference(Some("")).unwrap_err();
        assert!(
            matches!(local_empty_err, IngestError::InvalidSource(msg) if msg.contains("--ref is only valid for GitHub sources"))
        );

        let gh_empty_err = gh.with_reference(Some("")).unwrap_err();
        assert!(
            matches!(gh_empty_err, IngestError::InvalidSource(msg) if msg.contains("--ref cannot be empty"))
        );
    }

    #[test]
    fn parses_public_github_repository() -> Result<(), Box<dyn std::error::Error>> {
        let expected = Source::Github {
            owner: "rust-lang".into(),
            repo: "cargo".into(),
            reference: None,
        };
        assert_eq!(
            Source::parse("https://github.com/rust-lang/cargo.git")?,
            expected
        );
        let expected_repo = Source::Github {
            owner: "owner".into(),
            repo: "repo".into(),
            reference: None,
        };
        assert_eq!(
            Source::parse("https://github.com:443/owner/repo")?,
            expected_repo
        );
        assert_eq!(
            Source::parse("https://github.com:/owner/repo")?,
            expected_repo
        );
        Ok(())
    }

    #[test]
    fn parse_rejects_malformed_urls() {
        let invalid_cases = [
            "git@github.com:owner/repo.git",
            "https://gitlab.com/owner/repo",
            "https://github.com/owner/repo/issues",
            "https://github.com/owner/..",
            "https://github.com/owner/.",
            "https://github.com/./repo",
            "https://github.com/../repo",
            "https://github.com:99999/owner/repo",
            "https://github.com:abc/owner/repo",
        ];
        for url in invalid_cases {
            assert!(Source::parse(url).is_err(), "expected error for: {url}");
        }
    }

    #[test]
    fn source_github_constructor_validates_identifiers() {
        let valid = Source::github("rust-lang", "cargo", None).unwrap();
        assert_eq!(
            valid,
            Source::Github {
                owner: "rust-lang".into(),
                repo: "cargo".into(),
                reference: None,
            }
        );

        let invalid = [
            ("", "repo"),
            ("owner", ""),
            (".", "repo"),
            ("owner", "."),
            ("..", "repo"),
            ("owner", ".."),
            ("ow/ner", "repo"),
            ("owner", "re po"),
        ];
        for (owner, repo) in invalid {
            assert!(
                Source::github(owner, repo, None).is_err(),
                "expected error for: {owner}/{repo}"
            );
        }
    }
}
