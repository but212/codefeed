use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::blocking::{Client, Response};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use url::Url;

use crate::digest::{ensure_digest_fits, FileRecord};
use crate::local::CandidateBudget;
use crate::{text_content, IngestError, Options, MAX_FILE_SIZE, MAX_JSON_SIZE, REMOTE_ROOT};

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

struct GithubClient {
    client: Client,
}

impl GithubClient {
    fn new() -> Result<Self, IngestError> {
        let client = Client::builder()
            .user_agent("local_ingest/0.1")
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
    ) -> Result<Option<Vec<u8>>, IngestError> {
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
        let response = self
            .client
            .get(url)
            .send()
            .map_err(|error| IngestError::Http(error.to_string()))?;
        let status = response.status();
        let body = read_response(response, MAX_FILE_SIZE)?;
        if !status.is_success() {
            let detail = match body {
                LimitedBody::Complete(body) => String::from_utf8_lossy(&body).replace('\n', " "),
                LimitedBody::Exceeded => "response body exceeds the limit".into(),
            };
            return Err(IngestError::Api(format!("HTTP {status}: {detail}")));
        }
        Ok(match body {
            LimitedBody::Complete(body) => Some(body),
            LimitedBody::Exceeded => None,
        })
    }

    fn ignore_rules(
        &self,
        owner: &str,
        repo: &str,
        revision: &str,
        entries: &[TreeEntry],
    ) -> Result<Vec<RemoteIgnoreRule>, IngestError> {
        let mut rules = Vec::new();
        for entry in entries.iter().filter(|entry| {
            is_regular_blob(entry)
                && (entry.path == ".gitignore" || entry.path.ends_with("/.gitignore"))
        }) {
            let Some(bytes) = self.raw_file(owner, repo, revision, &entry.path)? else {
                continue;
            };
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

    fn json<T: DeserializeOwned>(&self, url: Url) -> Result<T, IngestError> {
        let response = self
            .client
            .get(url)
            .send()
            .map_err(|error| IngestError::Http(error.to_string()))?;
        let status = response.status();
        let body = read_response(response, MAX_JSON_SIZE)?;
        if !status.is_success() {
            let detail = match body {
                LimitedBody::Complete(body) => String::from_utf8_lossy(&body).replace('\n', " "),
                LimitedBody::Exceeded => "response body exceeds the limit".into(),
            };
            return Err(IngestError::Api(format!("HTTP {status}: {detail}")));
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
    let mut skipped = 0;
    let mut candidates = CandidateBudget::default();
    for entry in tree.tree {
        if !is_regular_blob(&entry) || is_hidden_path(&entry.path) {
            continue;
        }
        if ignored_remote_path(&entry.path, &ignore_rules) {
            skipped += 1;
            continue;
        }
        candidates.add()?;
        if entry.size.unwrap_or(0) > MAX_FILE_SIZE {
            skipped += 1;
            continue;
        }
        let Some(bytes) = client.raw_file(owner, repo, &revision, &entry.path)? else {
            skipped += 1;
            continue;
        };
        if let Some(content) = text_content(bytes) {
            let candidate = FileRecord {
                path: entry.path,
                content,
            };
            ensure_digest_fits(&label, Some(&revision), &files, skipped, Some(&candidate))?;
            files.push(candidate);
        } else {
            skipped += 1;
        }
    }

    Ok((label, Some(revision), files, skipped))
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

fn read_response(response: Response, limit: u64) -> Result<LimitedBody, IngestError> {
    let mut bytes = Vec::new();
    response
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| IngestError::Http(error.to_string()))?;
    Ok(classify_body(bytes, limit))
}
