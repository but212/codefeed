use std::ffi::OsStr;
use std::path::PathBuf;

use crate::core::error::IngestError;
use crate::core::model::Source;

pub(crate) fn parse_source_os(value: &OsStr) -> Result<Source, IngestError> {
    match value.to_str() {
        Some(value) => parse_source(value),
        None => Ok(Source::Local(PathBuf::from(value))),
    }
}

pub(crate) fn parse_source(value: &str) -> Result<Source, IngestError> {
    if value.starts_with("http://") || value.starts_with("https://") {
        return parse_github_url(value);
    }
    if value.starts_with("git@") || value.starts_with("ssh://") {
        return Err(IngestError::UnsupportedSource(
            "only local paths and public HTTPS GitHub URLs are supported".into(),
        ));
    }
    Ok(Source::Local(value.into()))
}

fn parse_github_url(value: &str) -> Result<Source, IngestError> {
    let remainder = value.strip_prefix("https://").ok_or_else(|| {
        IngestError::UnsupportedSource("only https://github.com/{owner}/{repo} is supported".into())
    })?;
    let (authority, path) = match remainder.find('/') {
        Some(index) => (&remainder[..index], &remainder[index..]),
        None => (remainder, ""),
    };
    if authority.is_empty() {
        return Err(IngestError::InvalidSource(value.into()));
    }
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let host = match host_port.split_once(':') {
        Some((host, port)) => {
            if !port.is_empty() && port.parse::<u16>().is_err() {
                return Err(IngestError::InvalidSource(value.into()));
            }
            host
        }
        None => host_port,
    };
    if !host.eq_ignore_ascii_case("github.com") {
        return Err(IngestError::UnsupportedSource(
            "only https://github.com/{owner}/{repo} is supported".into(),
        ));
    }
    if path.contains('?') || path.contains('#') {
        return Err(IngestError::InvalidSource(
            "repository URLs cannot contain a query or fragment".into(),
        ));
    }

    let parts: Vec<_> = path.split('/').filter(|part| !part.is_empty()).collect();
    let [owner, repo_segment] = parts.as_slice() else {
        return Err(IngestError::InvalidSource(
            "expected https://github.com/{owner}/{repo}".into(),
        ));
    };

    let repo = repo_segment.strip_suffix(".git").unwrap_or(repo_segment);
    if repo.is_empty() || !valid_github_name(owner) || !valid_github_name(repo) {
        return Err(IngestError::InvalidSource(
            "owner and repository names contain invalid characters".into(),
        ));
    }

    Ok(Source::GitHub {
        owner: (*owner).to_owned(),
        repo: repo.to_owned(),
        reference: None,
    })
}

fn valid_github_name(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_public_github_repository() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            parse_source("https://github.com/rust-lang/cargo.git")?,
            Source::GitHub {
                owner: "rust-lang".into(),
                repo: "cargo".into(),
                reference: None,
            }
        );
        Ok(())
    }

    #[test]
    fn rejects_unsupported_repository_url() {
        assert!(parse_source("git@github.com:owner/repo.git").is_err());
        assert!(parse_source("https://gitlab.com/owner/repo").is_err());
        assert!(parse_source("https://github.com/owner/repo/issues").is_err());
    }

    #[test]
    fn rejects_dot_path_segments_in_repository_urls() {
        assert!(parse_source("https://github.com/owner/..").is_err());
        assert!(parse_source("https://github.com/owner/.").is_err());
        assert!(parse_source("https://github.com/./repo").is_err());
        assert!(parse_source("https://github.com/../repo").is_err());
    }

    #[test]
    fn rejects_invalid_repository_url_ports() -> Result<(), Box<dyn std::error::Error>> {
        assert!(parse_source("https://github.com:99999/owner/repo").is_err());
        assert!(parse_source("https://github.com:abc/owner/repo").is_err());
        let expected = Source::GitHub {
            owner: "owner".into(),
            repo: "repo".into(),
            reference: None,
        };
        assert_eq!(parse_source("https://github.com:443/owner/repo")?, expected);
        assert_eq!(parse_source("https://github.com:/owner/repo")?, expected);
        Ok(())
    }
}
