//! Candidate discovery: the sorted walker and per-entry path classification.

use std::io;
use std::path::Path;

use ignore::WalkBuilder;

use crate::core::error::DomainError;
use crate::core::policy::{PathStatus, classify_path_segments, is_hidden_segment};
use crate::shell::IngestError;

/// Builds the walker. Entries arrive in file-name order, which matches the `cmp_path` order that
/// `SortedFiles` uses, so the producer never needs to re-sort them.
pub(super) fn sorted_walker(root: &Path) -> ignore::Walk {
    WalkBuilder::new(root)
        .hidden(false)
        .filter_entry(|entry| {
            entry.depth() == 0 || !is_hidden_segment(entry.file_name().as_encoded_bytes())
        })
        .ignore(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(false)
        .require_git(false)
        .parents(false)
        .follow_links(false)
        .sort_by_file_name(std::cmp::Ord::cmp)
        .build()
}

/// Classifies one walk entry on the producer thread. `Ignored` covers the root, the output file,
/// directories, and non-files.
pub(super) fn classify_entry(
    root: &Path,
    output: Option<&Path>,
    entry: &ignore::DirEntry,
) -> Result<PathStatus, IngestError> {
    let entry_path = entry.path();
    if entry_path == root || output.is_some_and(|target| entry_path == target) {
        return Ok(PathStatus::Ignored);
    }

    if !entry.file_type().is_some_and(|kind| kind.is_file()) {
        return Ok(PathStatus::Ignored);
    }

    let relative = entry_path
        .strip_prefix(root)
        .map_err(|err| DomainError::invalid_metadata(err.to_string()))?;
    classify_local_path(relative).map_err(IngestError::from)
}

fn classify_local_path(relative: &Path) -> Result<PathStatus, DomainError> {
    let segments = relative
        .components()
        .map(|c| c.as_os_str().as_encoded_bytes());
    classify_path_segments(segments)
        .map_err(|_| DomainError::invalid_metadata("local path exceeds maximum directory depth"))
}

pub(super) fn format_ignore_error(err: &ignore::Error) -> io::Error {
    let mut current = err;
    let mut path = None;
    loop {
        match current {
            ignore::Error::WithPath {
                path: p,
                err: inner,
            } => {
                if path.is_none() {
                    path = Some(p.as_path());
                }
                current = inner;
            }
            ignore::Error::WithDepth { err: inner, .. } => {
                current = inner;
            }
            _ => break,
        }
    }
    let (kind, msg) = match (path, current) {
        (Some(p), ignore::Error::Io(inner)) => (inner.kind(), format!("{}: {inner}", p.display())),
        (None, ignore::Error::Io(inner)) => (inner.kind(), inner.to_string()),
        (Some(p), _) => (io::ErrorKind::Other, format!("{}: {current}", p.display())),
        (None, _) => (io::ErrorKind::Other, current.to_string()),
    };
    crate::shell::error::sanitized_io_error(kind, &msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn rejects_excessive_path_depth() {
        let mut excessive = PathBuf::new();
        for _ in 0..257 {
            excessive.push("d");
        }
        excessive.push("file.txt");
        let result = classify_local_path(&excessive);
        assert!(result.is_err());

        let mut normal = PathBuf::new();
        for _ in 0..256 {
            normal.push("d");
        }
        normal.push("file.txt");
        let result = classify_local_path(&normal);
        assert!(result.is_ok());
    }

    #[test]
    fn format_ignore_error_covers_all_wrapping_and_kind_variants() {
        let path = PathBuf::from("dir/file.txt");

        // 1. WithPath wrapping WithDepth wrapping Io error
        let io_err = io::Error::new(io::ErrorKind::PermissionDenied, "access denied");
        let depth_err = ignore::Error::WithDepth {
            depth: 2,
            err: Box::new(ignore::Error::Io(io_err)),
        };
        let with_path = ignore::Error::WithPath {
            path: path.clone(),
            err: Box::new(depth_err),
        };
        let res = format_ignore_error(&with_path);
        assert_eq!(res.kind(), io::ErrorKind::PermissionDenied);
        assert!(res.to_string().contains("dir/file.txt: access denied"));

        // 2. Direct Io error with None path
        let io_not_found = io::Error::new(io::ErrorKind::NotFound, "file not found");
        let direct_io = ignore::Error::Io(io_not_found);
        let res = format_ignore_error(&direct_io);
        assert_eq!(res.kind(), io::ErrorKind::NotFound);
        assert_eq!(res.to_string(), "file not found");

        // 3. WithDepth without path wrapping non-Io error (None, _)
        let loop_err = ignore::Error::Loop {
            ancestor: PathBuf::from("a"),
            child: PathBuf::from("b"),
        };
        let depth_loop = ignore::Error::WithDepth {
            depth: 3,
            err: Box::new(loop_err.clone()),
        };
        let res = format_ignore_error(&depth_loop);
        assert_eq!(res.kind(), io::ErrorKind::Other);
        assert_ne!(res.to_string(), "");

        // 4. WithPath wrapping non-Io error (Some(p), _)
        let with_path_loop = ignore::Error::WithPath {
            path: path.clone(),
            err: Box::new(loop_err),
        };
        let res = format_ignore_error(&with_path_loop);
        assert_eq!(res.kind(), io::ErrorKind::Other);
        assert!(res.to_string().contains("dir/file.txt"));

        // 5. Control character sanitization in path
        let ctrl_path = PathBuf::from("dir\x1b[31m\nname/file\x07.txt");
        let with_ctrl_path = ignore::Error::WithPath {
            path: ctrl_path,
            err: Box::new(ignore::Error::Io(io::Error::new(
                io::ErrorKind::NotFound,
                "missing",
            ))),
        };
        let res = format_ignore_error(&with_ctrl_path);
        let s = res.to_string();
        assert!(!s.contains('\x1b'));
        assert!(!s.contains('\n'));
        assert!(!s.contains('\x07'));
        assert!(s.contains("dir\u{FFFD}[31m name/file\u{FFFD}.txt: missing"));
    }
}
