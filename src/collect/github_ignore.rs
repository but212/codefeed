//! Remote `.gitignore` budgeting and rule loading.

use std::path::Path;

use crate::error::IngestError;
use crate::limits::{MAX_FILE_SIZE, MAX_REMOTE_IGNORE_BYTES, MAX_REMOTE_IGNORE_FILES};
use crate::model::REMOTE_ROOT;

use super::github_client::{GithubClient, LimitedBody};
use super::github_tree::{TreeEntry, is_regular_blob};
use super::ignore::{RemoteIgnoreRule, remote_ignore_file_is_reachable};

const GITIGNORE_FILE_NAME: &str = ".gitignore";
const NESTED_GITIGNORE_SUFFIX: &str = "/.gitignore";

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

pub(crate) fn load_ignore_rules(
    client: &GithubClient,
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
            client.raw_file(owner, repo, revision, &entry.path, limit)?,
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
