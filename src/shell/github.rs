//! GitHub collection orchestration: tree walk, per-entry probe, selection.

use crate::core::decide::{CandidateBudget, Item, ItemPath, Probe, Selector};
use crate::core::error::IngestError;
use crate::core::github_tree::{TreeEntry, is_regular_blob};
use crate::core::model::{CollectionResult, DigestFormat};
use crate::core::policy::{FETCH_WINDOW, MAX_FILE_SIZE, PathClassification};

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
                && PathClassification::classify(&entry.path) != PathClassification::Ignored
        })
        .collect();

    sort_entries(&mut entries);

    CandidateBudget::verify_count(
        entries.iter().map(|entry| Some(entry.path.as_str())),
        entries.len(),
    )?;

    BlobFetcher::new(&client, owner, repo, &revision).fetch_all(&entries, &mut selector)?;

    let (files, skipped, trie) = selector.into_parts();
    Ok(CollectionResult {
        label,
        revision: Some(revision),
        files,
        skipped,
        trie,
    })
}

struct BlobFetcher<'a> {
    client: &'a GithubClient,
    owner: &'a str,
    repo: &'a str,
    revision: &'a str,
}

impl<'a> BlobFetcher<'a> {
    fn new(client: &'a GithubClient, owner: &'a str, repo: &'a str, revision: &'a str) -> Self {
        Self {
            client,
            owner,
            repo,
            revision,
        }
    }

    fn probe_entry(&self, entry: &TreeEntry) -> Result<Probe, IngestError> {
        match PathClassification::classify(&entry.path) {
            PathClassification::Ignored => Ok(Probe::Ignored),
            PathClassification::PreclassifiedBinary => Ok(Probe::Missing),
            PathClassification::NeedsContent => {
                if entry.size.unwrap_or(0) > MAX_FILE_SIZE {
                    Ok(Probe::TooLarge)
                } else {
                    match self.client.raw_file(
                        self.owner,
                        self.repo,
                        self.revision,
                        &entry.path,
                        MAX_FILE_SIZE,
                    ) {
                        Ok(LimitedBody::Complete(bytes)) => Ok(Probe::Bytes(bytes)),
                        Ok(LimitedBody::Exceeded) => Ok(Probe::TooLarge),
                        Err(err) => Err(err),
                    }
                }
            }
        }
    }

    fn fetch_all(&self, entries: &[TreeEntry], selector: &mut Selector) -> Result<(), IngestError> {
        for batch in entries.chunks(FETCH_WINDOW) {
            let results = std::thread::scope(|s| {
                let handles: Vec<_> = batch
                    .iter()
                    .map(|entry| s.spawn(|| self.probe_entry(entry)))
                    .collect();
                handles
                    .into_iter()
                    .map(|h| h.join().expect("worker thread panicked"))
                    .collect::<Vec<_>>()
            });

            for (entry, probe_res) in batch.iter().zip(results) {
                selector.decide(Item {
                    path: ItemPath::Utf8(entry.path.clone()),
                    probe: probe_res?,
                })?;
            }
        }
        Ok(())
    }
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
