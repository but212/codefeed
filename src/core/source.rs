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
    if parts.len() != 2 {
        return Err(IngestError::InvalidSource(
            "expected https://github.com/{owner}/{repo}".into(),
        ));
    }

    let repo = parts[1].strip_suffix(".git").unwrap_or(parts[1]);
    if repo.is_empty() || !valid_github_name(parts[0]) || !valid_github_name(repo) {
        return Err(IngestError::InvalidSource(
            "owner and repository names contain invalid characters".into(),
        ));
    }

    Ok(Source::GitHub {
        owner: parts[0].to_owned(),
        repo: repo.to_owned(),
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
