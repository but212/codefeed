//! GitHub collection orchestration: tree walk, per-entry probe, selection.

use crate::core::decide::{Item, ItemPath, Probe, Selector};
use crate::core::error::IngestError;
use crate::core::github_tree::{is_hidden_path, is_regular_blob};
use crate::core::model::CollectionResult;
use crate::core::policy::{FETCH_WINDOW, MAX_FILE_SIZE, has_binary_extension, is_noise_path};

use super::github_client::{GithubClient, LimitedBody};

pub(crate) fn collect_github(
    owner: &str,
    repo: &str,
    reference: Option<&str>,
) -> Result<CollectionResult, IngestError> {
    let client = GithubClient::new();
    let reference = match reference {
        Some(reference) if !reference.trim().is_empty() => reference.to_owned(),
        _ => client.repository(owner, repo)?.default_branch,
    };
    let revision = client.commit_sha(owner, repo, &reference)?;
    let tree = client.tree(owner, repo, &revision)?;
    if tree.truncated {
        return Err(IngestError::Api(
            "repository tree is too large for the GitHub API response".into(),
        ));
    }

    let label = format!("https://github.com/{owner}/{repo}");
    let mut selector = Selector::new(&label, Some(&revision))?;

    let entries: Vec<_> = tree
        .tree
        .into_iter()
        .filter(|entry| {
            is_regular_blob(entry)
                && !is_hidden_path(&entry.path)
                && !is_noise_path(&entry.path)
        })
        .collect();

    for batch in entries.chunks(FETCH_WINDOW) {
        let probes: Vec<Result<Probe, IngestError>> = std::thread::scope(|s| {
            let handles: Vec<_> = batch
                .iter()
                .map(|entry| {
                    s.spawn(|| {
                        if has_binary_extension(&entry.path) {
                            Ok(Probe::Missing)
                        } else if entry.size.unwrap_or(0) > MAX_FILE_SIZE {
                            Ok(Probe::TooLarge)
                        } else {
                            match client.raw_file(
                                owner,
                                repo,
                                &revision,
                                &entry.path,
                                MAX_FILE_SIZE,
                            )? {
                                LimitedBody::Complete(bytes) => Ok(Probe::Bytes(bytes)),
                                LimitedBody::Exceeded => Ok(Probe::TooLarge),
                            }
                        }
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("fetch thread panicked"))
                .collect()
        });

        for (entry, probe) in batch.iter().zip(probes) {
            selector.decide(Item {
                path: ItemPath::Utf8(entry.path.clone()),
                probe: probe?,
            })?;
        }
    }

    let (files, skipped) = selector.into_parts();
    Ok(CollectionResult::GitHub {
        label,
        revision,
        files,
        skipped,
    })
}
