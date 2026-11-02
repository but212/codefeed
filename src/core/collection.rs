use crate::core::storage::SortedFiles;

/// Raw files and metadata collected by the shell before digest rendering.
#[derive(Debug)]
#[cfg_attr(test, derive(Clone))]
#[must_use]
pub(crate) struct CollectionResult {
    pub(crate) label: String,
    pub(crate) revision: Option<String>,
    pub(crate) files: SortedFiles,
    pub(crate) skipped: usize,
}

/// Digest label for a local repository root: its final path segment, or `root` when that is
/// missing, empty, or not UTF-8. The shell extracts `file_name`; this rule decides the fallback.
#[must_use]
pub(crate) fn repository_name(file_name: Option<&str>) -> &str {
    file_name.filter(|name| !name.is_empty()).unwrap_or("root")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_name_falls_back_to_root() {
        assert_eq!(repository_name(Some("codefeed")), "codefeed");
        assert_eq!(repository_name(Some("")), "root");
        assert_eq!(repository_name(None), "root");
    }
}
