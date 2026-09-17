//! GitHub collection orchestration: tree walk, per-entry probe, selection.

use crate::decide::{Item, ItemPath, Probe, Selector};
use crate::error::IngestError;
use crate::limits::MAX_FILE_SIZE;
use crate::model::{CollectionResult, Options};

use super::github_client::{GithubClient, LimitedBody};
use super::github_ignore::load_ignore_rules;
use super::github_tree::{is_hidden_path, is_regular_blob};
use super::ignore::ignored_remote_path;

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

    let ignore_rules = load_ignore_rules(&client, owner, repo, &revision, &tree.tree)?;
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
