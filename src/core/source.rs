use std::borrow::Cow;
use std::path::PathBuf;

use crate::core::error::DomainError;

const fn is_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_')
}

/// Validated GitHub owner or repository name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
        if !valid_github_name(&string) {
            return Err(DomainError::invalid_source(
                "owner and repository names contain invalid characters",
            ));
        }
        Ok(Self(string))
    }
}

impl std::ops::Deref for GithubName {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Validates whether the given string conforms to valid GitHub owner or repository name rules.
#[must_use]
fn valid_github_name(value: &str) -> bool {
    !matches!(value, "" | "." | "..") && value.bytes().all(is_name_byte)
}

/// Strips an ASCII `prefix` from `value`, ignoring ASCII case (URL schemes are case-insensitive).
#[must_use]
fn strip_prefix_ignore_case<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    let (head, tail) = value.split_at_checked(prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then_some(tail)
}

/// Classifies and parses a remote URL into a validated GitHub `(owner, repo)` pair.
///
/// Returns `Ok(None)` when `value` carries no recognized remote scheme (`https://`, `http://`,
/// `git@`, `ssh://`), allowing the caller to treat it as a local path.
fn parse_github_url(value: &str) -> Result<Option<(GithubName, GithubName)>, DomainError> {
    let Some(remainder) = strip_prefix_ignore_case(value, "https://") else {
        if strip_prefix_ignore_case(value, "http://").is_some() {
            return Err(DomainError::unsupported_source(
                "only https://github.com/{owner}/{repo} is supported",
            ));
        }
        if strip_prefix_ignore_case(value, "git@").is_some()
            || strip_prefix_ignore_case(value, "ssh://").is_some()
        {
            return Err(DomainError::unsupported_source(
                "only local paths and public HTTPS GitHub URLs are supported",
            ));
        }
        return Ok(None);
    };
    let (authority, path) = remainder.split_once('/').unwrap_or((remainder, ""));
    if authority.is_empty() {
        return Err(DomainError::invalid_source(value));
    }
    // Checked before the port, and the URL is not echoed: `user:token@` must never reach stderr.
    if authority.contains('@') {
        return Err(DomainError::invalid_source(
            "repository URLs cannot contain credentials",
        ));
    }
    let host = match authority.split_once(':') {
        Some((host, port)) => {
            if !port.is_empty() && port != "443" {
                return Err(DomainError::invalid_source(value));
            }
            host
        }
        None => authority,
    };
    if !host.eq_ignore_ascii_case("github.com") {
        return Err(DomainError::unsupported_source(
            "only https://github.com/{owner}/{repo} is supported",
        ));
    }
    if path.contains(['?', '#']) {
        return Err(DomainError::invalid_source(
            "repository URLs cannot contain a query or fragment",
        ));
    }

    let mut parts = path.split('/').filter(|part| !part.is_empty());
    let (Some(owner), Some(repo_segment), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(DomainError::invalid_source(
            "expected https://github.com/{owner}/{repo}",
        ));
    };

    let repo = repo_segment.strip_suffix(".git").unwrap_or(repo_segment);
    Ok(Some((GithubName::new(owner)?, GithubName::new(repo)?)))
}

/// Validated non-empty Git revision reference (branch, tag, or commit SHA).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GitRef(String);

impl GitRef {
    /// Creates a new `GitRef`, rejecting empty, whitespace-only, `.`, and `..` strings.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidSource`] if `value` is empty, only whitespace, `.`, or `..`.
    pub fn new<'a>(value: impl Into<Cow<'a, str>>) -> Result<Self, DomainError> {
        let cow = value.into();
        let trimmed = cow.trim_ascii();
        if trimmed.is_empty() {
            return Err(DomainError::invalid_source("--ref cannot be empty"));
        }
        if trimmed == "." || trimmed == ".." {
            return Err(DomainError::invalid_source("--ref cannot be '.' or '..'"));
        }
        let owned = match cow {
            Cow::Owned(s) if trimmed.len() == s.len() => s,
            Cow::Owned(_) | Cow::Borrowed(_) => trimmed.to_owned(),
        };
        Ok(Self(owned))
    }
}

impl std::ops::Deref for GitRef {
    type Target = str;

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
        value
            .to_str()
            .map_or_else(|| Ok(Self::Local(PathBuf::from(value))), Self::parse)
    }

    /// Parses a string slice into either a [`Source::Local`] or [`Source::Github`].
    fn parse(value: &str) -> Result<Self, DomainError> {
        if let Some((owner, repo)) = parse_github_url(value)? {
            return Ok(Self::Github {
                owner,
                repo,
                reference: None,
            });
        }
        Ok(Self::Local(value.into()))
    }

    /// Returns a new `Source` instance with the specified Git revision reference.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidSource`] if `reference` is [`Some`] when called on a [`Source::Local`] variant,
    /// or if the reference string is empty, only whitespace, `.`, or `..`.
    pub fn with_reference<'a>(
        self,
        reference: Option<impl Into<Cow<'a, str>>>,
    ) -> Result<Self, DomainError> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_ref_validates_and_provides_slice_access() {
        let valid = GitRef::new("v1.0.0").unwrap();
        assert_eq!(&*valid, "v1.0.0");

        let untrimmed = GitRef::new("  main\t\n").unwrap();
        assert_eq!(&*untrimmed, "main");

        let owned_clean = GitRef::new(String::from("release/1.0")).unwrap();
        assert_eq!(&*owned_clean, "release/1.0");

        let owned_trimmed = GitRef::new(String::from("  dev  ")).unwrap();
        assert_eq!(&*owned_trimmed, "dev");

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
    fn parse_scheme_is_case_insensitive() {
        let expected = Source::github("owner", "repo", None).unwrap();
        assert_eq!(
            Source::parse("HTTPS://github.com/owner/repo").unwrap(),
            expected
        );
        assert_eq!(
            Source::parse("Https://GitHub.com/owner/repo").unwrap(),
            expected
        );
        for url in [
            "HTTP://github.com/owner/repo",
            "Git@github.com:o/r",
            "SSH://git@github.com/o/r",
        ] {
            assert!(
                matches!(Source::parse(url), Err(DomainError::UnsupportedSource(_))),
                "expected unsupported source for: {url}"
            );
        }
    }

    #[test]
    fn parse_rejects_malformed_urls() {
        let invalid_cases = [
            "git@github.com:owner/repo.git",
            "https://github.com/owner/.",
            "https://github.com/./repo",
            "https://github.com:99999/owner/repo",
            "https://github.com:abc/owner/repo",
            "https://github.com:8080/owner/repo",
        ];
        for url in invalid_cases {
            assert!(Source::parse(url).is_err(), "expected error for: {url}");
        }

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

    #[test]
    fn parses_owner_and_repo() {
        let name = GithubName::new("codefeed-2.0").unwrap();
        assert_eq!(&*name, "codefeed-2.0");

        let (owner, repo) = parse_github_url("https://github.com/rust-lang/cargo")
            .unwrap()
            .unwrap();
        assert_eq!(&*owner, "rust-lang");
        assert_eq!(&*repo, "cargo");

        let (owner, repo) = parse_github_url("https://github.com/owner/repo.git")
            .unwrap()
            .unwrap();
        assert_eq!(&*owner, "owner");
        assert_eq!(&*repo, "repo");

        assert_eq!(parse_github_url("local/path").unwrap(), None);
        assert!(parse_github_url("https://notgithub.com/a/b").is_err());
        assert!(parse_github_url("https://github.com/a/b/c").is_err());
    }

    #[test]
    fn rejects_invalid_repository_urls() {
        for value in [
            "https://github.com/bad name/repo",
            "https://github.com/owner/re%70o",
            "https://github.com/../repo",
            "https://github.com/owner/..",
            "https://github.com/owner/.git",
            "https://github.com/owner/repo?tab=readme",
            "https://github.com/owner/repo#readme",
            "https:///owner/repo",
        ] {
            let err = parse_github_url(value).unwrap_err();
            assert!(matches!(err, DomainError::InvalidSource(_)), "{value}");
        }

        let unsupported = parse_github_url("http://github.com/owner/repo").unwrap_err();
        assert!(matches!(unsupported, DomainError::UnsupportedSource(_)));

        let err = parse_github_url("https://github.com:99999/o/r\x1b[2J").unwrap_err();
        let rendered = err.to_string();
        assert!(!rendered.contains('\x1b'), "ESC leaked: {rendered:?}");
        assert_eq!(
            rendered,
            "invalid source: https://github.com:99999/o/r\u{FFFD}[2J"
        );
    }

    #[test]
    fn valid_github_name_accepts_exactly_the_name_alphabet() {
        for byte in 0..128u8 {
            let c = char::from(byte);
            let in_alphabet = c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_');
            assert_eq!(
                valid_github_name(&format!("a{c}b")),
                in_alphabet,
                "embedded {c:?}"
            );
            assert_eq!(
                valid_github_name(&c.to_string()),
                in_alphabet && c != '.',
                "single {c:?}"
            );
        }
        assert!(!valid_github_name("a\u{e9}"));
    }

    #[test]
    fn valid_github_name_rejects_only_empty_and_dot_names_by_length() {
        for accepted in ["a", ".a", "a.", "ab", "..a", "a..", "...", ".-"] {
            assert!(valid_github_name(accepted), "{accepted:?}");
        }
        for rejected in ["", ".", ".."] {
            assert!(!valid_github_name(rejected), "{rejected:?}");
        }
    }
}
