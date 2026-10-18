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
    /// Parses an OS-native string argument into a [`Source`].
    pub(crate) fn parse_os(value: &std::ffi::OsStr) -> Result<Self, IngestError> {
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
    pub(crate) fn parse(value: &str) -> Result<Self, IngestError> {
        if value.starts_with("http://") || value.starts_with("https://") {
            return parse_github_url(value);
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
    /// Returns [`IngestError::InvalidSource`] if `reference` is empty/whitespace or
    /// if `reference` is [`Some`] when called on a [`Source::Local`] variant.
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

fn parse_github_url(value: &str) -> Result<Source, IngestError> {
    let remainder = value.strip_prefix("https://").ok_or_else(|| {
        IngestError::unsupported_source("only https://github.com/{owner}/{repo} is supported")
    })?;
    let (authority, path) = match remainder.find('/') {
        Some(index) => (&remainder[..index], &remainder[index..]),
        None => (remainder, ""),
    };
    if authority.is_empty() {
        return Err(IngestError::invalid_source(value));
    }
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let host = match host_port.split_once(':') {
        Some((host, port)) => {
            if !port.is_empty() && port.parse::<u16>().is_err() {
                return Err(IngestError::invalid_source(value));
            }
            host
        }
        None => host_port,
    };
    if !host.eq_ignore_ascii_case("github.com") {
        return Err(IngestError::unsupported_source(
            "only https://github.com/{owner}/{repo} is supported",
        ));
    }
    if path.contains('?') || path.contains('#') {
        return Err(IngestError::invalid_source(
            "repository URLs cannot contain a query or fragment",
        ));
    }

    let mut parts = path.split('/').filter(|part| !part.is_empty());
    let (Some(owner), Some(repo_segment), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(IngestError::invalid_source(
            "expected https://github.com/{owner}/{repo}",
        ));
    };

    let repo = repo_segment.strip_suffix(".git").unwrap_or(repo_segment);
    if repo.is_empty() || !valid_github_name(owner) || !valid_github_name(repo) {
        return Err(IngestError::invalid_source(
            "owner and repository names contain invalid characters",
        ));
    }

    Ok(Source::Github {
        owner: (*owner).to_owned(),
        repo: repo.to_owned(),
        reference: None,
    })
}

fn valid_github_name(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value.bytes().all(|byte| {
            (byte.wrapping_sub(b'0') < 10)
                | ((byte | 32).wrapping_sub(b'a') < 26)
                | (byte == b'-')
                | (byte == b'_')
                | (byte == b'.')
        })
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
}
