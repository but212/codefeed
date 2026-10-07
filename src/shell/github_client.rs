//! GitHub HTTP transport with bounded response bodies.

use std::io::Read;
use std::time::Duration;

use serde::de::DeserializeOwned;
use ureq::Agent;
use ureq::http::StatusCode;
use ureq::tls::{RootCerts, TlsConfig};

use crate::core::error::IngestError;
use crate::core::github_tree::{CommitResponse, RepositoryResponse, TreeResponse};
use crate::core::github_url::{RequestTarget, api_url, raw_url, request_target};
use crate::core::limits::MAX_JSON_SIZE;

const GITHUB_USER_AGENT: &str = "rustingest/0.1";
const GITHUB_TIMEOUT_SECS: u64 = 30;

pub(crate) enum LimitedBody {
    Complete(Vec<u8>),
    Exceeded,
}

pub(crate) struct GithubClient {
    agent: Agent,
    auth_header: Option<String>,
}

impl GithubClient {
    pub(crate) fn new() -> Self {
        let tls = TlsConfig::builder()
            .root_certs(RootCerts::PlatformVerifier)
            .build();
        let config = Agent::config_builder()
            .user_agent(GITHUB_USER_AGENT)
            .timeout_global(Some(Duration::from_secs(GITHUB_TIMEOUT_SECS)))
            .http_status_as_error(false)
            .tls_config(tls)
            .build();

        let auth_header = resolve_token_from_env().map(|token| format!("Bearer {token}"));

        Self {
            agent: Agent::new_with_config(config),
            auth_header,
        }
    }

    pub(crate) fn repository(
        &self,
        owner: &str,
        repo: &str,
    ) -> Result<RepositoryResponse, IngestError> {
        self.json(&api_url(&["repos", owner, repo]))
    }

    pub(crate) fn commit_sha(
        &self,
        owner: &str,
        repo: &str,
        reference: &str,
    ) -> Result<String, IngestError> {
        let url = api_url(&["repos", owner, repo, "commits", reference]);
        let commit: CommitResponse = self.json(&url)?;
        if commit.sha.is_empty() {
            return Err(IngestError::InvalidMetadata(
                "GitHub returned an empty commit SHA".into(),
            ));
        }
        Ok(commit.sha)
    }

    pub(crate) fn tree(
        &self,
        owner: &str,
        repo: &str,
        revision: &str,
    ) -> Result<TreeResponse, IngestError> {
        let mut url = api_url(&["repos", owner, repo, "git", "trees", revision]);
        url.push_str("?recursive=1");
        self.json(&url)
    }

    pub(crate) fn raw_file(
        &self,
        owner: &str,
        repo: &str,
        revision: &str,
        path: &str,
        limit: u64,
    ) -> Result<LimitedBody, IngestError> {
        let url = raw_url(owner, repo, revision, path);
        let (status, body) = self.response(&url, limit)?;
        if !status.is_success() {
            return Err(api_error(status, body));
        }
        Ok(body)
    }

    fn response(&self, url: &str, limit: u64) -> Result<(StatusCode, LimitedBody), IngestError> {
        let mut request = self.agent.get(url);
        let target = request_target(url);

        if matches!(target, RequestTarget::Api | RequestTarget::Raw)
            && let Some(auth) = &self.auth_header
        {
            request = request.header("Authorization", auth);
        }
        if target == RequestTarget::Api {
            request = request.header("X-Github-Api-Version", "2022-11-28");
        }

        let response = request
            .call()
            .map_err(|error| IngestError::Http(error.to_string()))?;
        let status = response.status();
        let body = read_response(response.into_body(), limit)?;
        Ok((status, body))
    }

    fn json<T: DeserializeOwned>(&self, url: &str) -> Result<T, IngestError> {
        let (status, body) = self.response(url, MAX_JSON_SIZE)?;
        if !status.is_success() {
            return Err(api_error(status, body));
        }
        let LimitedBody::Complete(body) = body else {
            return Err(IngestError::Api(format!(
                "GitHub API response exceeds the {MAX_JSON_SIZE} byte limit"
            )));
        };
        serde_json::from_slice(&body).map_err(|error| IngestError::Api(error.to_string()))
    }
}

fn classify_body(bytes: Vec<u8>, limit: u64) -> LimitedBody {
    if bytes.len() as u64 > limit {
        LimitedBody::Exceeded
    } else {
        LimitedBody::Complete(bytes)
    }
}

fn api_error(status: StatusCode, body: LimitedBody) -> IngestError {
    let detail = match body {
        LimitedBody::Complete(body) => String::from_utf8_lossy(&body).replace('\n', " "),
        LimitedBody::Exceeded => "response body exceeds the limit".into(),
    };
    IngestError::Api(format!("HTTP {status}: {detail}"))
}

fn read_response(body: ureq::Body, limit: u64) -> Result<LimitedBody, IngestError> {
    let mut bytes = Vec::new();
    body.into_reader()
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| IngestError::Http(error.to_string()))?;
    Ok(classify_body(bytes, limit))
}

fn resolve_token_with<F>(mut lookup: F) -> Option<String>
where
    F: FnMut(&str) -> Result<String, std::env::VarError>,
{
    ["GITHUB_TOKEN", "GH_TOKEN"].into_iter().find_map(|key| {
        lookup(key)
            .ok()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
    })
}

fn resolve_token_from_env() -> Option<String> {
    resolve_token_with(|k| std::env::var(k))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::env::VarError;

    #[test]
    fn distinguishes_complete_and_oversized_bodies() {
        assert!(matches!(
            classify_body(vec![0; 8], 8),
            LimitedBody::Complete(body) if body.len() == 8
        ));
        assert!(matches!(
            classify_body(vec![0; 9], 8),
            LimitedBody::Exceeded
        ));
    }

    #[test]
    fn resolves_token_with_correct_precedence_and_trimming() {
        let mock_env = |map: HashMap<&'static str, &'static str>| {
            move |key: &str| {
                map.get(key)
                    .copied()
                    .map(String::from)
                    .ok_or(VarError::NotPresent)
            }
        };

        let env = HashMap::from([("GITHUB_TOKEN", "token_a"), ("GH_TOKEN", "token_b")]);
        assert_eq!(resolve_token_with(mock_env(env)), Some("token_a".into()));

        let env = HashMap::from([("GH_TOKEN", "token_b")]);
        assert_eq!(resolve_token_with(mock_env(env)), Some("token_b".into()));

        let env = HashMap::from([("GITHUB_TOKEN", "   "), ("GH_TOKEN", "valid_token")]);
        assert_eq!(
            resolve_token_with(mock_env(env)),
            Some("valid_token".into())
        );

        let env = HashMap::from([("GITHUB_TOKEN", "")]);
        assert_eq!(resolve_token_with(mock_env(env)), None);

        let env = HashMap::new();
        assert_eq!(resolve_token_with(mock_env(env)), None);
    }
}
