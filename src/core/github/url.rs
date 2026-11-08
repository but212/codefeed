//! GitHub owner/repo validation, source URL parsing, and percent-encoded API URL construction.

use memchr::{memchr, memchr2};

use crate::core::error::DomainError;

pub(crate) const GITHUB_API_BASE_URL: &str = "https://api.github.com";

/// Exact-origin match: returns `true` if `url == GITHUB_API_BASE_URL` or starts with `GITHUB_API_BASE_URL + "/"`.
#[must_use]
pub(crate) fn is_github_api_origin(url: &str) -> bool {
    url.strip_prefix(GITHUB_API_BASE_URL)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

const GITHUB_NAME_CHAR_TABLE: [bool; 256] = {
    let mut table = [false; 256];
    let mut b = 0u8;
    loop {
        table[b as usize] = (b.wrapping_sub(b'0') < 10) | ((b | 32).wrapping_sub(b'a') < 26)
            || (b == b'-')
            || (b == b'.')
            || (b == b'_');
        if b == 255 {
            break;
        }
        b += 1;
    }
    table
};

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
const fn valid_github_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    if bytes.len() == 1 && bytes[0] == b'.' {
        return false;
    }
    if bytes.len() == 2 && bytes[0] == b'.' && bytes[1] == b'.' {
        return false;
    }
    let mut i = 0;
    while i < bytes.len() {
        if !GITHUB_NAME_CHAR_TABLE[bytes[i] as usize] {
            return false;
        }
        i += 1;
    }
    true
}

/// Parses a GitHub repository URL into a validated `(owner, repo)` pair.
pub(crate) fn parse_github_url(value: &str) -> Result<(GithubName, GithubName), DomainError> {
    let remainder = value.strip_prefix("https://").ok_or_else(|| {
        DomainError::unsupported_source("only https://github.com/{owner}/{repo} is supported")
    })?;
    let (authority, path) = memchr(b'/', remainder.as_bytes()).map_or((remainder, ""), |index| {
        (&remainder[..index], &remainder[index..])
    });
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
    if memchr2(b'?', b'#', path.as_bytes()).is_some() {
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
    Ok((GithubName::new(owner)?, GithubName::new(repo)?))
}

/// Canonical public web URL for a repository, used as the digest label for remote sources.
#[must_use]
pub(crate) fn repository_url(owner: &str, repo: &str) -> String {
    format!("https://github.com/{owner}/{repo}")
}

/// Constructs the tarball download URL under `base` for the given repository and optional Git reference.
#[must_use]
pub(crate) fn tarball_url(base: &str, owner: &str, repo: &str, reference: Option<&str>) -> String {
    let mut url = String::from(base);
    for segment in ["repos", owner, repo, "tarball"] {
        url = append_encoded_segment(url, segment);
    }
    if let Some(reference) = reference {
        url = append_encoded_segment(url, reference);
    }
    url
}

/// RFC 3986 unreserved bytes: the name alphabet plus `~`.
const fn is_unreserved(b: u8) -> bool {
    GITHUB_NAME_CHAR_TABLE[b as usize] || b == b'~'
}

fn append_encoded_segment(mut url: String, segment: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    url.push('/');
    for &b in segment.as_bytes() {
        if is_unreserved(b) {
            url.push(b as char);
        } else {
            url.push('%');
            url.push(HEX[(b >> 4) as usize] as char);
            url.push(HEX[(b & 0xF) as usize] as char);
        }
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_classification_identifies_github_api_correctly() {
        let cases = [
            ("https://api.github.com/repos/owner/repo", true),
            ("https://api.github.com", true),
            (
                "https://raw.githubusercontent.com/owner/repo/main/lib.rs",
                false,
            ),
            ("https://raw.githubusercontent.com", false),
            ("https://api.github.com.evil.example/x", false),
            ("https://api.github.com@evil.example/x", false),
            ("http://127.0.0.1:8080/repos/owner/repo", false),
        ];
        for (url, expected) in cases {
            assert_eq!(
                is_github_api_origin(url),
                expected,
                "unexpected auth scope for: {url}"
            );
        }
    }

    #[test]
    fn builds_repository_url() {
        assert_eq!(
            repository_url("but212", "codefeed"),
            "https://github.com/but212/codefeed"
        );
    }

    #[test]
    fn builds_tarball_urls() {
        assert_eq!(
            tarball_url(GITHUB_API_BASE_URL, "rust-lang", "cargo", None),
            "https://api.github.com/repos/rust-lang/cargo/tarball"
        );
        assert_eq!(
            tarball_url(GITHUB_API_BASE_URL, "rust-lang", "cargo", Some("main")),
            "https://api.github.com/repos/rust-lang/cargo/tarball/main"
        );
        assert_eq!(
            tarball_url(GITHUB_API_BASE_URL, "owner", "repo", Some("feature/test")),
            "https://api.github.com/repos/owner/repo/tarball/feature%2Ftest"
        );
        assert_eq!(
            tarball_url("http://127.0.0.1:8080", "owner", "repo", Some("main")),
            "http://127.0.0.1:8080/repos/owner/repo/tarball/main"
        );
    }

    #[test]
    fn github_name_validates_and_provides_slice_access() {
        let name = GithubName::new("rust-lang").unwrap();
        assert_eq!(&*name, "rust-lang");

        let repo = GithubName::new("codefeed-2.0").unwrap();
        assert_eq!(&*repo, "codefeed-2.0");
    }

    #[test]
    fn parses_owner_and_repo() {
        let (owner, repo) = parse_github_url("https://github.com/rust-lang/cargo").unwrap();
        assert_eq!(&*owner, "rust-lang");
        assert_eq!(&*repo, "cargo");

        let (owner, repo) = parse_github_url("https://github.com/owner/repo.git").unwrap();
        assert_eq!(&*owner, "owner");
        assert_eq!(&*repo, "repo");

        assert!(parse_github_url("https://notgithub.com/a/b").is_err());
        assert!(parse_github_url("https://github.com/a/b/c").is_err());
    }

    #[test]
    fn rejects_malformed_repository_urls() {
        for value in [
            "https://github.com/bad name/repo",
            "https://github.com/owner/re%70o",
            "https://github.com/../repo",
            "https://github.com/owner/..",
            "https://github.com/owner/.git",
            "https://github.com/owner/repo?tab=readme",
            "https://github.com/owner/repo#readme",
        ] {
            let err = parse_github_url(value).unwrap_err();
            assert!(matches!(err, DomainError::InvalidSource(_)), "{value}");
        }
    }

    #[test]
    fn rejects_non_https_scheme_and_malformed_authority() {
        let unsupported = parse_github_url("http://github.com/owner/repo").unwrap_err();
        assert!(matches!(unsupported, DomainError::UnsupportedSource(_)));

        let empty_authority = parse_github_url("https:///owner/repo").unwrap_err();
        assert!(matches!(empty_authority, DomainError::InvalidSource(_)));
    }

    #[test]
    fn rejected_url_echo_is_sanitized() {
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

    #[test]
    fn tarball_url_encodes_every_byte_outside_the_unreserved_set() {
        const PREFIX: &str = "https://api.github.com/repos/o/r/tarball/";
        for byte in 0..128u8 {
            let c = char::from(byte);
            let expected = if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~') {
                c.to_string()
            } else {
                format!("%{byte:02X}")
            };
            assert_eq!(
                tarball_url(GITHUB_API_BASE_URL, "o", "r", Some(&c.to_string())),
                format!("{PREFIX}{expected}"),
                "{c:?}"
            );
        }
        assert_eq!(
            tarball_url(GITHUB_API_BASE_URL, "o", "r", Some("\u{e9}")),
            format!("{PREFIX}%C3%A9")
        );
    }
}
