use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::blocking::{Client, Response};
use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use url::Url;

use crate::digest::{DigestBudget, FileRecord};
use crate::local::CandidateBudget;
use crate::{
    text_content, IngestError, Options, MAX_FILE_SIZE, MAX_JSON_SIZE, MAX_REMOTE_IGNORE_BYTES,
    MAX_REMOTE_IGNORE_FILES, REMOTE_ROOT,
};

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

pub(crate) struct RemoteIgnoreRule {
    pub(crate) base: PathBuf,
    pub(crate) matcher: ignore::gitignore::Gitignore,
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
    client: Client,
}

impl GithubClient {
    fn new() -> Result<Self, IngestError> {
        let client = Client::builder()
            .user_agent("rustingest/0.1")
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|error| IngestError::Http(error.to_string()))?;
        Ok(Self { client })
    }

    fn repository(&self, owner: &str, repo: &str) -> Result<RepositoryResponse, IngestError> {
        self.json(api_url(&["repos", owner, repo])?)
    }

    fn commit_sha(&self, owner: &str, repo: &str, reference: &str) -> Result<String, IngestError> {
        let url = api_url(&["repos", owner, repo, "commits", reference])?;
        let commit: CommitResponse = self.json(url)?;
        if commit.sha.is_empty() {
            return Err(IngestError::InvalidMetadata(
                "GitHub returned an empty commit SHA".into(),
            ));
        }
        Ok(commit.sha)
    }

    fn tree(&self, owner: &str, repo: &str, revision: &str) -> Result<TreeResponse, IngestError> {
        let mut url = api_url(&["repos", owner, repo, "git", "trees", revision])?;
        url.query_pairs_mut().append_pair("recursive", "1");
        self.json(url)
    }

    fn raw_file(
        &self,
        owner: &str,
        repo: &str,
        revision: &str,
        path: &str,
        limit: u64,
    ) -> Result<LimitedBody, IngestError> {
        let mut url = Url::parse("https://raw.githubusercontent.com")
            .map_err(|error| IngestError::InvalidMetadata(error.to_string()))?;
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| IngestError::InvalidMetadata("invalid raw URL".into()))?;
            segments.push(owner).push(repo).push(revision);
            for part in path.split('/') {
                segments.push(part);
            }
        }
        let (status, body) = self.response(url, limit)?;
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
                    && (entry.path == ".gitignore" || entry.path.ends_with("/.gitignore"))
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
            if ignored_remote_path(&entry.path, &rules) {
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

    fn response(&self, url: Url, limit: u64) -> Result<(StatusCode, LimitedBody), IngestError> {
        let response = self
            .client
            .get(url)
            .send()
            .map_err(|error| IngestError::Http(error.to_string()))?;
        let status = response.status();
        let body = read_response(response, limit)?;
        Ok((status, body))
    }

    fn json<T: DeserializeOwned>(&self, url: Url) -> Result<T, IngestError> {
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
) -> Result<(String, Option<String>, Vec<FileRecord>, usize), IngestError> {
    let client = GithubClient::new()?;
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
    let mut files = Vec::new();
    let mut budget = DigestBudget::new(&label, Some(&revision))?;
    let mut candidates = CandidateBudget::default();
    for entry in tree.tree {
        if !is_regular_blob(&entry) || is_hidden_path(&entry.path) {
            continue;
        }
        if ignored_remote_path(&entry.path, &ignore_rules) {
            budget.skip()?;
            continue;
        }
        candidates.add()?;
        if entry.size.unwrap_or(0) > MAX_FILE_SIZE {
            budget.skip()?;
            continue;
        }
        let bytes = match client.raw_file(owner, repo, &revision, &entry.path, MAX_FILE_SIZE)? {
            LimitedBody::Complete(bytes) => bytes,
            LimitedBody::Exceeded => {
                budget.skip()?;
                continue;
            }
        };
        if let Some(content) = text_content(bytes) {
            let candidate = FileRecord {
                path: entry.path,
                content,
            };
            budget.add(&candidate)?;
            files.push(candidate);
        } else {
            budget.skip()?;
        }
    }

    Ok((label, Some(revision), files, budget.skipped()))
}

fn is_hidden_path(path: &str) -> bool {
    path.split('/')
        .any(|part| part.starts_with('.') || part.is_empty())
}

pub(crate) fn is_regular_blob(entry: &TreeEntry) -> bool {
    entry.entry_type == "blob" && matches!(entry.mode.as_str(), "100644" | "100755")
}

pub(crate) fn ignored_remote_path(path: &str, rules: &[RemoteIgnoreRule]) -> bool {
    let absolute = Path::new(REMOTE_ROOT).join(path);
    let mut directories = Vec::new();
    let mut current = absolute.parent();
    while let Some(directory) = current {
        if directory == Path::new(REMOTE_ROOT) {
            break;
        }
        directories.push(directory.to_path_buf());
        current = directory.parent();
    }
    directories.reverse();

    let mut ignored_directories = Vec::new();
    for directory in directories {
        if remote_match(&directory, true, rules, &ignored_directories) {
            ignored_directories.push(directory);
        }
    }
    if ignored_directories
        .iter()
        .any(|directory| absolute.starts_with(directory))
    {
        return true;
    }
    remote_match(&absolute, false, rules, &ignored_directories)
}

fn remote_match(
    path: &Path,
    is_dir: bool,
    rules: &[RemoteIgnoreRule],
    ignored_directories: &[PathBuf],
) -> bool {
    let mut ignored = false;
    for rule in rules {
        if !path.starts_with(&rule.base)
            || ignored_directories
                .iter()
                .any(|directory| rule.base.starts_with(directory))
        {
            continue;
        }
        match rule.matcher.matched_path_or_any_parents(path, is_dir) {
            result if result.is_ignore() => ignored = true,
            result if result.is_whitelist() => ignored = false,
            _ => {}
        }
    }
    ignored
}

fn api_url(segments: &[&str]) -> Result<Url, IngestError> {
    let mut url = Url::parse("https://api.github.com")
        .map_err(|error| IngestError::InvalidMetadata(error.to_string()))?;
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|_| IngestError::InvalidMetadata("invalid GitHub API URL".into()))?;
        for segment in segments {
            path.push(segment);
        }
    }
    Ok(url)
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

fn read_response(response: Response, limit: u64) -> Result<LimitedBody, IngestError> {
    let mut bytes = Vec::new();
    response
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| IngestError::Http(error.to_string()))?;
    Ok(classify_body(bytes, limit))
}
