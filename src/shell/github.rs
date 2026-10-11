//! GitHub collection orchestration: tree walk, per-entry probe, selection.

use crate::core::decide::{CandidateBudget, Item, ItemPath, Probe, Selector};
use crate::core::error::IngestError;
use crate::core::github_tree::{TreeEntry, is_hidden_path, is_regular_blob};
use crate::core::model::{CollectionResult, DigestFormat};
use crate::core::policy::{
    FETCH_WINDOW, MAX_FILE_COUNT, MAX_FILE_SIZE, has_binary_extension, is_noise_path,
    is_sensitive_path,
};

use super::github_client::{GithubClient, LimitedBody};

pub(crate) fn collect_github(
    owner: &str,
    repo: &str,
    reference: Option<&str>,
    format: DigestFormat,
) -> Result<CollectionResult, IngestError> {
    let client = GithubClient::new();
    let reference = match reference {
        Some(reference) => reference.to_owned(),
        None => client.repository(owner, repo)?.default_branch,
    };
    let revision = client.commit_sha(owner, repo, &reference)?;
    let tree = client.tree(owner, repo, &revision)?;
    if tree.truncated {
        return Err(IngestError::Api(
            "repository tree is too large for the GitHub API response".into(),
        ));
    }

    let label = format!("https://github.com/{owner}/{repo}");
    let mut selector = Selector::new(&label, Some(&revision), format)?;

    let mut entries: Vec<_> = tree
        .tree
        .into_iter()
        .filter(|entry| {
            is_regular_blob(entry)
                && !is_hidden_path(&entry.path)
                && !is_noise_path(&entry.path)
                && !is_sensitive_path(&entry.path)
        })
        .collect();

    sort_entries(&mut entries);

    if entries.len() > MAX_FILE_COUNT {
        let mut budget = CandidateBudget::default();
        for entry in &entries {
            budget.add(Some(&entry.path))?;
        }
    }

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

    let (files, skipped, trie) = selector.into_parts();
    Ok(CollectionResult {
        label,
        revision: Some(revision),
        files,
        skipped,
        trie,
    })
}

fn sort_entries(entries: &mut [TreeEntry]) {
    entries.sort_by(|a, b| a.path.cmp(&b.path));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_tree_entries_lexicographically() {
        let mut entries = vec![
            TreeEntry {
                path: "src/main.rs".into(),
                mode: "100644".into(),
                entry_type: "blob".into(),
                size: Some(10),
            },
            TreeEntry {
                path: "Cargo.toml".into(),
                mode: "100644".into(),
                entry_type: "blob".into(),
                size: Some(10),
            },
            TreeEntry {
                path: "src/core/mod.rs".into(),
                mode: "100644".into(),
                entry_type: "blob".into(),
                size: Some(10),
            },
        ];

        sort_entries(&mut entries);

        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["Cargo.toml", "src/core/mod.rs", "src/main.rs"]);
    }
}
