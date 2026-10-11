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
        if has_binary_extension(&entry.path) {
            Ok(Probe::Missing)
        } else if entry.size.unwrap_or(0) > MAX_FILE_SIZE {
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

    fn fetch_all(&self, entries: &[TreeEntry], selector: &mut Selector) -> Result<(), IngestError> {
        std::thread::scope(|s| {
            let mut worker_senders = Vec::with_capacity(FETCH_WINDOW);
            let (res_tx, res_rx) = std::sync::mpsc::channel();

            for _ in 0..FETCH_WINDOW {
                let (w_tx, w_rx) = std::sync::mpsc::channel::<Option<(usize, &TreeEntry)>>();
                worker_senders.push(w_tx);
                let res_tx = res_tx.clone();
                s.spawn(move || {
                    while let Ok(Some((idx, entry))) = w_rx.recv() {
                        let probe = self.probe_entry(entry);
                        if res_tx.send((idx, probe)).is_err() {
                            break;
                        }
                    }
                });
            }

            drop(res_tx);

            for batch in entries.chunks(FETCH_WINDOW) {
                for (idx, entry) in batch.iter().enumerate() {
                    worker_senders[idx]
                        .send(Some((idx, entry)))
                        .expect("worker send failed");
                }

                let mut batch_results: Vec<Option<Result<Probe, IngestError>>> =
                    (0..batch.len()).map(|_| None).collect();
                for _ in 0..batch.len() {
                    let (idx, probe) = res_rx.recv().expect("worker hung up");
                    batch_results[idx] = Some(probe);
                }

                for (entry, probe) in batch.iter().zip(batch_results) {
                    selector.decide(Item {
                        path: ItemPath::Utf8(entry.path.clone()),
                        probe: probe.expect("missing probe result")?,
                    })?;
                }
            }

            // Signal workers to cleanly shut down
            for sender in worker_senders {
                let _ = sender.send(None);
            }
            Ok::<(), IngestError>(())
        })
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
