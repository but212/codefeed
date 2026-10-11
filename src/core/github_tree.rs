//! GitHub tree response DTOs and regular-blob classification.

use serde::Deserialize;

const GIT_BLOB_TYPE: &str = "blob";
const REGULAR_FILE_MODES: &[&str] = &["100644", "100755"];

#[derive(Debug, Deserialize)]
pub(crate) struct RepositoryResponse {
    pub(crate) default_branch: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CommitResponse {
    pub(crate) sha: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TreeResponse {
    pub(crate) truncated: bool,
    pub(crate) tree: Vec<TreeEntry>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TreeEntry {
    pub(crate) path: String,
    pub(crate) mode: String,
    #[serde(rename = "type")]
    pub(crate) entry_type: String,
    pub(crate) size: Option<u64>,
}

pub(crate) fn is_regular_blob(entry: &TreeEntry) -> bool {
    entry.entry_type == GIT_BLOB_TYPE && REGULAR_FILE_MODES.contains(&entry.mode.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_regular_git_blobs() {
        let symlink = TreeEntry {
            path: "link".into(),
            mode: "120000".into(),
            entry_type: "blob".into(),
            size: Some(4),
        };
        let executable = TreeEntry {
            path: "run.sh".into(),
            mode: "100755".into(),
            entry_type: "blob".into(),
            size: Some(4),
        };
        assert!(!is_regular_blob(&symlink));
        assert!(is_regular_blob(&executable));
    }
}
