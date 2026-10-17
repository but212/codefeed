use std::ffi::OsStr;
use std::path::PathBuf;

use crate::core::error::IngestError;
use crate::core::model::Source;

pub(crate) fn parse_os(value: &OsStr) -> Result<Source, IngestError> {
    match value.to_str() {
        Some(value) => parse(value),
        None => Ok(Source::Local(PathBuf::from(value))),
    }
}

pub(crate) fn parse(value: &str) -> Result<Source, IngestError> {
    if value.starts_with("http://") || value.starts_with("https://") {
        return parse_github_url(value);
    }
    if value.starts_with("git@") || value.starts_with("ssh://") {
        return Err(IngestError::unsupported_source(
            "only local paths and public HTTPS GitHub URLs are supported",
        ));
    }
    Ok(Source::Local(value.into()))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_public_github_repository() -> Result<(), Box<dyn std::error::Error>> {
        let expected = Source::Github {
            owner: "rust-lang".into(),
            repo: "cargo".into(),
            reference: None,
        };
        assert_eq!(parse("https://github.com/rust-lang/cargo.git")?, expected);
        let expected_repo = Source::Github {
            owner: "owner".into(),
            repo: "repo".into(),
            reference: None,
        };
        assert_eq!(parse("https://github.com:443/owner/repo")?, expected_repo);
        assert_eq!(parse("https://github.com:/owner/repo")?, expected_repo);
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
            assert!(parse(url).is_err(), "expected error for: {url}");
        }
    }
}
