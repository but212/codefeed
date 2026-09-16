use std::io::Read;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use ureq::Agent;
use ureq::http::StatusCode;
use ureq::tls::{RootCerts, TlsConfig};

use crate::decide::{Item, ItemPath, Probe, Selector};

pub(crate) use super::ignore::{
    RemoteIgnoreRule, ignored_remote_path, remote_ignore_file_is_reachable,
};

use crate::{
    CollectionResult, IngestError, MAX_FILE_SIZE, MAX_JSON_SIZE, MAX_REMOTE_IGNORE_BYTES,
    MAX_REMOTE_IGNORE_FILES, Options, REMOTE_ROOT,
};

const GITHUB_API_BASE_URL: &str = "https://api.github.com";
const GITHUB_RAW_BASE_URL: &str = "https://raw.githubusercontent.com";
const GITHUB_USER_AGENT: &str = "rustingest/0.1";
const GITHUB_TIMEOUT_SECS: u64 = 30;
const GITIGNORE_FILE_NAME: &str = ".gitignore";
const NESTED_GITIGNORE_SUFFIX: &str = "/.gitignore";
const GIT_BLOB_TYPE: &str = "blob";
const REGULAR_FILE_MODES: &[&str] = &["100644", "100755"];

#[derive(Debug, Deserialize)]
struct RepositoryResponse {
    default_branch: String,
}

#[derive(Debug, Deserialize)]
struct CommitResponse {
    sha: String,
}

#[derive(Debug, Deserialize)]
struct TreeResponse {
    truncated: bool,
    tree: Vec<TreeEntry>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TreeEntry {
    pub(crate) path: String,
    pub(crate) mode: String,
    #[serde(rename = "type")]
    pub(crate) entry_type: String,
    pub(crate) size: Option<u64>,
}

pub(crate) enum LimitedBody {
    Complete(Vec<u8>),
    Exceeded,
}

pub(crate) fn remote_ignore_body(path: &str, body: LimitedBody) -> Result<Vec<u8>, IngestError> {
    match body {
        LimitedBody::Complete(bytes) => Ok(bytes),
        LimitedBody::Exceeded => Err(IngestError::InvalidMetadata(format!(
            "remote .gitignore {path} exceeds the per-file or aggregate byte limit"
        ))),
    }
}

#[derive(Default)]
pub(crate) struct RemoteIgnoreBudget {
    file_count: usize,
    bytes: u64,
}

impl RemoteIgnoreBudget {
    pub(crate) fn reserve_file(&mut self) -> Result<(), IngestError> {
        if self.file_count >= MAX_REMOTE_IGNORE_FILES {
            return Err(IngestError::InvalidMetadata(format!(
                "repository contains more than {MAX_REMOTE_IGNORE_FILES} .gitignore files"
            )));
        }
        self.file_count += 1;
        Ok(())
    }

    pub(crate) fn read_limit(&self) -> Result<u64, IngestError> {
        let remaining = MAX_REMOTE_IGNORE_BYTES
            .checked_sub(self.bytes)
            .ok_or_else(|| IngestError::InvalidMetadata("remote ignore byte overflow".into()))?;
        if remaining == 0 {
            return Err(IngestError::InvalidMetadata(format!(
                "remote .gitignore files exceed the {MAX_REMOTE_IGNORE_BYTES} byte limit"
            )));
        }
        Ok(remaining.min(MAX_FILE_SIZE))
    }

    pub(crate) fn record_bytes(&mut self, bytes: usize) -> Result<(), IngestError> {
        self.bytes = self
            .bytes
            .checked_add(
                u64::try_from(bytes).map_err(|_| {
                    IngestError::InvalidMetadata("remote ignore byte overflow".into())
                })?,
            )
            .ok_or_else(|| IngestError::InvalidMetadata("remote ignore byte overflow".into()))?;
        if self.bytes > MAX_REMOTE_IGNORE_BYTES {
            return Err(IngestError::InvalidMetadata(format!(
                "remote .gitignore files exceed the {MAX_REMOTE_IGNORE_BYTES} byte limit"
            )));
        }
        Ok(())
    }
}

struct GithubClient {
    agent: Agent,
}

impl GithubClient {
    fn new() -> Self {
        let tls = TlsConfig::builder()
            .root_certs(RootCerts::PlatformVerifier)
            .build();
        let config = Agent::config_builder()
            .user_agent(GITHUB_USER_AGENT)
            .timeout_global(Some(Duration::from_secs(GITHUB_TIMEOUT_SECS)))
            .http_status_as_error(false)
            .tls_config(tls)
            .build();
        Self {
            agent: Agent::new_with_config(config),
        }
    }

    fn repository(&self, owner: &str, repo: &str) -> Result<RepositoryResponse, IngestError> {
        self.json(&api_url(&["repos", owner, repo]))
    }

    fn commit_sha(&self, owner: &str, repo: &str, reference: &str) -> Result<String, IngestError> {
        let url = api_url(&["repos", owner, repo, "commits", reference]);
        let commit: CommitResponse = self.json(&url)?;
        if commit.sha.is_empty() {
            return Err(IngestError::InvalidMetadata(
                "GitHub returned an empty commit SHA".into(),
            ));
        }
        Ok(commit.sha)
    }

    fn tree(&self, owner: &str, repo: &str, revision: &str) -> Result<TreeResponse, IngestError> {
        let mut url = api_url(&["repos", owner, repo, "git", "trees", revision]);
        url.push_str("?recursive=1");
        self.json(&url)
    }

    fn raw_file(
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

    fn ignore_rules(
        &self,
        owner: &str,
        repo: &str,
        revision: &str,
        entries: &[TreeEntry],
    ) -> Result<Vec<RemoteIgnoreRule>, IngestError> {
        let mut ignore_entries: Vec<_> = entries
            .iter()
            .filter(|entry| {
                is_regular_blob(entry)
                    && (entry.path == GITIGNORE_FILE_NAME
                        || entry.path.ends_with(NESTED_GITIGNORE_SUFFIX))
            })
            .collect();
        ignore_entries.sort_by(|left, right| {
            left.path
                .matches('/')
                .count()
                .cmp(&right.path.matches('/').count())
                .then_with(|| left.path.cmp(&right.path))
        });

        let mut rules = Vec::new();
        let mut budget = RemoteIgnoreBudget::default();
        for entry in ignore_entries {
            if !remote_ignore_file_is_reachable(&entry.path, &rules) {
                continue;
            }
            budget.reserve_file()?;
            let limit = budget.read_limit()?;
            let bytes = remote_ignore_body(
                &entry.path,
                self.raw_file(owner, repo, revision, &entry.path, limit)?,
            )?;
            budget.record_bytes(bytes.len())?;
            let Ok(contents) = String::from_utf8(bytes) else {
                continue;
            };
            let base = Path::new(REMOTE_ROOT).join(
                Path::new(&entry.path)
                    .parent()
                    .unwrap_or_else(|| Path::new("")),
            );
            let mut builder = ignore::gitignore::GitignoreBuilder::new(&base);
            for line in contents.lines() {
                builder
                    .add_line(Some(Path::new(&entry.path).to_path_buf()), line)
                    .map_err(|error| IngestError::InvalidMetadata(error.to_string()))?;
            }
            rules.push(RemoteIgnoreRule {
                base,
                matcher: builder
                    .build()
                    .map_err(|error| IngestError::InvalidMetadata(error.to_string()))?,
            });
        }
        rules.sort_by_key(|rule| rule.base.components().count());
        Ok(rules)
    }

    fn response(&self, url: &str, limit: u64) -> Result<(StatusCode, LimitedBody), IngestError> {
        let response = self
            .agent
            .get(url)
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

pub(crate) fn collect_github(
    owner: &str,
    repo: &str,
    options: &Options,
) -> Result<CollectionResult, IngestError> {
    let client = GithubClient::new();
    let reference = match &options.reference {
        Some(reference) if !reference.trim().is_empty() => reference.clone(),
        _ => client.repository(owner, repo)?.default_branch,
    };
    let revision = client.commit_sha(owner, repo, &reference)?;
    let tree = client.tree(owner, repo, &revision)?;
    if tree.truncated {
        return Err(IngestError::Api(
            "repository tree is too large for the GitHub API response".into(),
        ));
    }

    let ignore_rules = client.ignore_rules(owner, repo, &revision, &tree.tree)?;
    let label = format!("https://github.com/{owner}/{repo}");
    let mut selector = Selector::new(&label, Some(&revision))?;
    for entry in tree.tree {
        if !is_regular_blob(&entry) || is_hidden_path(&entry.path) {
            continue;
        }
        let ignored = ignored_remote_path(&entry.path, &ignore_rules);
        let probe = if ignored {
            Probe::Ignored
        } else if entry.size.unwrap_or(0) > MAX_FILE_SIZE {
            Probe::TooLarge
        } else {
            match client.raw_file(owner, repo, &revision, &entry.path, MAX_FILE_SIZE)? {
                LimitedBody::Complete(bytes) => Probe::Bytes(bytes),
                LimitedBody::Exceeded => Probe::TooLarge,
            }
        };
        selector.decide(Item {
            path: ItemPath::Utf8(entry.path),
            probe,
        })?;
    }

    let (files, skipped) = selector.into_parts();
    Ok(CollectionResult::GitHub {
        label,
        revision,
        files,
        skipped,
    })
}

fn is_hidden_path(path: &str) -> bool {
    path.split('/')
        .any(|part| part.starts_with('.') || part.is_empty())
}

pub(crate) fn is_regular_blob(entry: &TreeEntry) -> bool {
    entry.entry_type == GIT_BLOB_TYPE && REGULAR_FILE_MODES.contains(&entry.mode.as_str())
}

pub(crate) fn api_url(segments: &[&str]) -> String {
    let mut url = String::from(GITHUB_API_BASE_URL);
    for segment in segments {
        push_encoded_segment(&mut url, segment);
    }
    url
}

pub(crate) fn raw_url(owner: &str, repo: &str, revision: &str, path: &str) -> String {
    let mut url = String::from(GITHUB_RAW_BASE_URL);
    for segment in [owner, repo, revision] {
        push_encoded_segment(&mut url, segment);
    }
    for part in path.split('/') {
        push_encoded_segment(&mut url, part);
    }
    url
}

fn push_encoded_segment(url: &mut String, segment: &str) {
    url.push('/');
    url.push_str(&encode_path_segment(segment));
}

pub(crate) fn encode_path_segment(segment: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push(HEX[(byte >> 4) as usize] as char);
            encoded.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }
    encoded
}

pub(crate) fn classify_body(bytes: Vec<u8>, limit: u64) -> LimitedBody {
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
