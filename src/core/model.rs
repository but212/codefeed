use std::path::PathBuf;

use crate::core::error::DomainError;
use crate::core::storage::SortedFiles;

/// Validated non-empty Git revision reference (branch, tag, or commit SHA).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GitRef(String);

impl GitRef {
    /// Creates a new `GitRef`, rejecting empty or whitespace-only strings.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidSource`] if `value` is empty or only whitespace.
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let string = value.into();
        let trimmed = string.trim_ascii();
        if trimmed.is_empty() {
            return Err(DomainError::invalid_source("--ref cannot be empty"));
        }
        if trimmed == "." || trimmed == ".." {
            return Err(DomainError::invalid_source("--ref cannot be '.' or '..'"));
        }
        Ok(Self(trimmed.to_owned()))
    }
}

impl std::ops::Deref for GitRef {
    type Target = str;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Validated GitHub owner or repository name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct GithubName(String);

impl GithubName {
    /// Creates a new `GithubName`, rejecting empty strings, `.`, `..`, and invalid characters.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidSource`] if `value` is empty, `.`, `..`, or contains
    /// characters other than ASCII alphanumerics, `-`, `.`, and `_`.
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let string = value.into();
        if !crate::core::github::valid_github_name(&string) {
            return Err(DomainError::invalid_source(
                "owner and repository names contain invalid characters",
            ));
        }
        Ok(Self(string))
    }
}

impl std::ops::Deref for GithubName {
    type Target = str;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
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
        owner: GithubName,
        /// Repository name.
        repo: GithubName,
        /// Optional GitHub branch, tag, or commit SHA.
        reference: Option<GitRef>,
    },
}

impl Source {
    /// Creates a new validated [`Source::Github`] instance.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidSource`] if `owner` or `repo` contain invalid characters,
    /// empty strings, or path traversals.
    pub fn github(
        owner: impl Into<String>,
        repo: impl Into<String>,
        reference: Option<GitRef>,
    ) -> Result<Self, DomainError> {
        Ok(Self::Github {
            owner: GithubName::new(owner)?,
            repo: GithubName::new(repo)?,
            reference,
        })
    }

    /// Parses an OS-native string argument into a [`Source`].
    pub(super) fn parse_os(value: &std::ffi::OsStr) -> Result<Self, DomainError> {
        match value.to_str() {
            Some(value) => Self::parse(value),
            None => Ok(Self::Local(PathBuf::from(value))),
        }
    }

    /// Parses a string slice into either a [`Source::Local`] or [`Source::Github`].
    fn parse(value: &str) -> Result<Self, DomainError> {
        if value.starts_with("http://") || value.starts_with("https://") {
            let (owner, repo) = crate::core::github::parse_github_url(value)?;
            return Self::github(owner, repo, None);
        }
        if value.starts_with("git@") || value.starts_with("ssh://") {
            return Err(DomainError::unsupported_source(
                "only local paths and public HTTPS GitHub URLs are supported",
            ));
        }
        Ok(Self::Local(value.into()))
    }

    /// Returns a new `Source` instance with the specified Git revision reference.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidSource`] if `reference` is [`Some`] when called on a [`Source::Local`] variant,
    /// or if the reference string is empty or only whitespace.
    pub fn with_reference(self, reference: Option<impl Into<String>>) -> Result<Self, DomainError> {
        match self {
            Self::Local(_) => {
                if reference.is_some() {
                    Err(DomainError::invalid_source(
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
#[must_use]
pub(crate) struct CollectionResult {
    pub(crate) label: String,
    pub(crate) revision: Option<String>,
    pub(crate) files: SortedFiles,
    pub(crate) skipped: usize,
}

/// Digest label for a local repository root: its final path segment, or `root` when that is
/// missing, empty, or not UTF-8. The shell extracts `file_name`; this rule decides the fallback.
#[must_use]
pub(crate) fn repository_name(file_name: Option<&str>) -> &str {
    file_name.filter(|name| !name.is_empty()).unwrap_or("root")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_name_falls_back_to_root() {
        assert_eq!(repository_name(Some("codefeed")), "codefeed");
        assert_eq!(repository_name(Some("")), "root");
        assert_eq!(repository_name(None), "root");
    }

    #[test]
    fn git_ref_validates_and_provides_slice_access() {
        let valid = GitRef::new("v1.0.0").unwrap();
        assert_eq!(&*valid, "v1.0.0");

        let untrimmed = GitRef::new("  main\t\n").unwrap();
        assert_eq!(&*untrimmed, "main");

        assert!(GitRef::new("").is_err());
        assert!(GitRef::new("   ").is_err());
        assert!(GitRef::new("\t\n").is_err());
        assert!(
            matches!(GitRef::new("."), Err(DomainError::InvalidSource(m)) if m.contains("--ref cannot be '.' or '..'"))
        );
        assert!(
            matches!(GitRef::new(".."), Err(DomainError::InvalidSource(m)) if m.contains("--ref cannot be '.' or '..'"))
        );
    }

    #[test]
    fn github_name_validates_and_provides_slice_access() {
        let name = GithubName::new("rust-lang").unwrap();
        assert_eq!(&*name, "rust-lang");

        let repo = GithubName::new("codefeed-2.0").unwrap();
        assert_eq!(&*repo, "codefeed-2.0");

        assert!(GithubName::new("").is_err());
        assert!(GithubName::new(".").is_err());
        assert!(GithubName::new("..").is_err());
        assert!(GithubName::new("bad name").is_err());
        assert!(GithubName::new("bad/name").is_err());
    }

    #[test]
    fn source_with_reference_validates_and_updates_reference() {
        let gh = Source::github("rust-lang", "rust", None).unwrap();
        let updated = gh.clone().with_reference(Some("main")).unwrap();
        assert_eq!(
            updated,
            Source::github("rust-lang", "rust", Some(GitRef::new("main").unwrap())).unwrap()
        );

        let cleared: Source = updated.with_reference(None::<&str>).unwrap();
        assert_eq!(cleared, Source::github("rust-lang", "rust", None).unwrap());

        let local = Source::Local(PathBuf::from("."));
        let local_ok: Source = local.clone().with_reference(None::<&str>).unwrap();
        assert_eq!(local_ok, local);

        let local_err = local.clone().with_reference(Some("main")).unwrap_err();
        assert!(
            matches!(local_err, DomainError::InvalidSource(msg) if msg.contains("--ref is only valid for GitHub sources"))
        );

        let local_empty_err = local.with_reference(Some("")).unwrap_err();
        assert!(
            matches!(local_empty_err, DomainError::InvalidSource(msg) if msg.contains("--ref is only valid for GitHub sources"))
        );

        let gh_empty_err = gh.with_reference(Some("")).unwrap_err();
        assert!(
            matches!(gh_empty_err, DomainError::InvalidSource(msg) if msg.contains("--ref cannot be empty"))
        );
    }

    #[test]
    fn parses_public_github_repository() -> Result<(), Box<dyn std::error::Error>> {
        let expected = Source::github("rust-lang", "cargo", None).unwrap();
        assert_eq!(
            Source::parse("https://github.com/rust-lang/cargo.git")?,
            expected
        );
        let expected_repo = Source::github("owner", "repo", None).unwrap();
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
            "https://github.com:8080/owner/repo",
            "https://tok@github.com/owner/repo",
            "https://tok@github.com:abc/owner/repo",
        ];
        for url in invalid_cases {
            assert!(Source::parse(url).is_err(), "expected error for: {url}");
        }
    }

    #[test]
    fn credential_rejection_does_not_echo_the_url() {
        for url in [
            "https://tok@github.com/owner/repo",
            "https://tok@github.com:abc/owner/repo",
        ] {
            let Err(error) = Source::parse(url) else {
                panic!("expected error for: {url}");
            };
            let message = error.to_string();
            assert!(!message.contains("tok"), "credentials leaked: {message}");
        }
    }

    #[test]
    fn source_github_constructor_validates_identifiers() {
        let valid = Source::github("rust-lang", "cargo", None).unwrap();
        assert_eq!(
            valid,
            Source::Github {
                owner: GithubName::new("rust-lang").unwrap(),
                repo: GithubName::new("cargo").unwrap(),
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
