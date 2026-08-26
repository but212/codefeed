use url::Url;

use crate::{IngestError, Source};

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
    let url = Url::parse(value).map_err(|_| IngestError::InvalidSource(value.into()))?;
    if url.scheme() != "https" || url.host_str() != Some("github.com") {
        return Err(IngestError::UnsupportedSource(
            "only https://github.com/{owner}/{repo} is supported".into(),
        ));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(IngestError::InvalidSource(
            "repository URLs cannot contain a query or fragment".into(),
        ));
    }

    let parts: Vec<_> = url
        .path_segments()
        .ok_or_else(|| IngestError::InvalidSource(value.into()))?
        .filter(|part| !part.is_empty())
        .collect();
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
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}
