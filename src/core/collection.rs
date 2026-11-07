use std::ffi::OsStr;
use std::path::Path;

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
/// missing or not UTF-8. `Path::file_name` never yields an empty name.
#[must_use]
pub(crate) fn repository_name(root: &Path) -> &str {
    root.file_name().and_then(OsStr::to_str).unwrap_or("root")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_name_falls_back_to_root() {
        assert_eq!(repository_name(Path::new("a/codefeed")), "codefeed");
        assert_eq!(repository_name(Path::new("/")), "root");
        #[cfg(windows)]
        assert_eq!(repository_name(Path::new("C:\\")), "root");
        assert_eq!(repository_name(Path::new("a/..")), "root");
        assert_eq!(repository_name(Path::new("")), "root");
    }

    #[cfg(unix)]
    #[test]
    fn repository_name_falls_back_to_root_for_non_utf8_name() {
        use std::os::unix::ffi::OsStrExt;

        let path = Path::new(std::ffi::OsStr::from_bytes(b"a/\xff"));
        assert_eq!(repository_name(path), "root");
    }
}
