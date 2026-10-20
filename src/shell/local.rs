//! Local directory collection orchestration: walk, map, select.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use zeroize::Zeroizing;

use crate::core::error::IngestError;
use crate::core::model::CollectionResult;
use crate::core::policy::{MAX_FILE_SIZE, PathStatus, classify_path_segments, is_hidden_segment};
use crate::core::selector::{Admission, Selector};

/// Collects files from a local directory root, excluding the resolved output file if located inside it.
pub(super) fn collect(
    root: &Path,
    resolved_output: Option<&Path>,
) -> Result<CollectionResult, IngestError> {
    let canonical_root = validate_root(root)?;
    let label = local_label(&canonical_root);

    let walker = WalkBuilder::new(&canonical_root)
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
        .sort_by_file_name(|a, b| a.cmp(b))
        .build();

    let mut selector = Selector::new();
    let mut scratch = Zeroizing::new(Vec::with_capacity(32 * 1024));

    for entry in walker {
        let entry = entry.map_err(|err| IngestError::Io(format_ignore_error(&err)))?;
        let entry_path = entry.path();
        if entry_path == canonical_root
            || !entry.file_type().is_some_and(|kind| kind.is_file())
            || resolved_output.is_some_and(|target| entry_path == target)
        {
            continue;
        }

        let relative = entry_path
            .strip_prefix(&canonical_root)
            .map_err(|err| IngestError::invalid_metadata(err.to_string()))?;

        let status = classify_local_path(relative)?;
        let size_hint = match status {
            PathStatus::Valid(_) => entry.metadata().ok().map(|m| m.len()),
            PathStatus::Ignored | PathStatus::NonUtf8Candidate(_) => None,
        };
        match selector.admit(status, size_hint)? {
            Admission::Settled(s) => selector = s,
            Admission::Pending(pending) => {
                scratch.clear();
                match read_local_file_into(entry_path, MAX_FILE_SIZE, &mut scratch)? {
                    Some(bytes) => selector = pending.accept(bytes)?,
                    None => selector = pending.skip(),
                }
            }
        }
    }

    Ok(selector.finish(label, None))
}

fn read_local_file_into<'b>(
    path: &Path,
    limit: u64,
    buf: &'b mut Vec<u8>,
) -> Result<Option<&'b [u8]>, IngestError> {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(IngestError::Io(err)),
    };
    if meta.is_symlink() || !meta.is_file() {
        return Ok(None);
    }
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(IngestError::Io(err)),
    };
    read_bounded_content_into(file, limit, buf).map_err(Into::into)
}

fn read_bounded_content_into(
    file: fs::File,
    limit: u64,
    buf: &mut Vec<u8>,
) -> io::Result<Option<&[u8]>> {
    if !file.metadata()?.is_file() {
        return Ok(None);
    }
    let mut handle = file.take(limit.saturating_add(1));
    handle.read_to_end(buf)?;
    Ok(Some(buf.as_slice()))
}

fn validate_root(root: &Path) -> Result<PathBuf, IngestError> {
    let root_display = root.display();
    let canonical = fs::canonicalize(root)
        .map_err(|error| IngestError::invalid_source(format!("{root_display} ({error})")))?;
    if !canonical.is_dir() {
        let display = canonical.display();
        return Err(IngestError::invalid_source(format!(
            "{display} is not a directory"
        )));
    }
    Ok(canonical)
}

fn classify_local_path(relative: &Path) -> Result<PathStatus, IngestError> {
    let segments = relative
        .components()
        .map(|c| c.as_os_str().as_encoded_bytes());
    classify_path_segments(segments)
        .map_err(|_| IngestError::invalid_metadata("local path exceeds maximum directory depth"))
}

fn format_ignore_error(err: &ignore::Error) -> io::Error {
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
    io::Error::new(kind, msg)
}

fn local_label(root: &Path) -> String {
    root.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("root")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::super::testing::temp_dir;
    use super::*;
    use crate::core::model::{DigestFormat, OutputTarget, RunConfig, Source};
    use crate::core::storage::{FileRecord, cmp_path};
    use crate::ingest;

    #[test]
    fn read_local_file_enforces_size_boundaries() -> Result<(), Box<dyn std::error::Error>> {
        use crate::core::policy::MAX_FILE_SIZE;

        let root = temp_dir("size_boundaries")?;
        let exact_path = root.join("exact.txt");
        let too_large_path = root.join("large.txt");
        fs::write(&exact_path, vec![b'x'; MAX_FILE_SIZE as usize])?;
        fs::write(&too_large_path, vec![b'x'; MAX_FILE_SIZE as usize + 1])?;

        let mut buf = Vec::new();
        assert!(matches!(
            read_local_file_into(&exact_path, MAX_FILE_SIZE, &mut buf)?,
            Some(b) if b.len() == MAX_FILE_SIZE as usize
        ));
        buf.clear();
        assert!(matches!(
            read_local_file_into(&too_large_path, MAX_FILE_SIZE, &mut buf)?,
            Some(b) if b.len() == MAX_FILE_SIZE as usize + 1
        ));
        Ok(())
    }

    #[test]
    fn handles_unlinked_file_as_missing() -> Result<(), Box<dyn std::error::Error>> {
        use crate::core::policy::MAX_FILE_SIZE;

        let root = temp_dir("missing_file")?;
        let path = root.join("nonexistent.txt");

        let mut buf = Vec::new();
        assert_eq!(read_local_file_into(&path, MAX_FILE_SIZE, &mut buf)?, None);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_file() -> Result<(), Box<dyn std::error::Error>> {
        use crate::core::policy::MAX_FILE_SIZE;
        use std::os::unix::fs::symlink;

        let root = temp_dir("symlink_test")?;
        let target = root.join("target.txt");
        let link = root.join("link.txt");
        fs::write(&target, "content")?;
        symlink(&target, &link)?;

        let mut buf = Vec::new();
        assert_eq!(read_local_file_into(&link, MAX_FILE_SIZE, &mut buf)?, None);
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn rejects_windows_symlink_file() -> Result<(), Box<dyn std::error::Error>> {
        use crate::core::policy::MAX_FILE_SIZE;
        use std::os::windows::fs::symlink_file;

        let root = temp_dir("win_symlink_test")?;
        let target = root.join("target.txt");
        let link = root.join("link.txt");
        fs::write(&target, "content")?;

        if symlink_file(&target, &link).is_ok() {
            let mut buf = Vec::new();
            assert_eq!(read_local_file_into(&link, MAX_FILE_SIZE, &mut buf)?, None);
            let _ = fs::remove_file(&link);
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn preserves_distinct_unix_paths() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("path_encoding")?;
        fs::create_dir_all(root.join("a"))?;
        fs::write(root.join("a\\b.txt"), "backslash")?;
        fs::write(root.join("a").join("b.txt"), "slash")?;
        let output = root.join("digest.txt");

        let report = ingest(&RunConfig::new(
            Source::Local(root.to_path_buf()),
            OutputTarget::File(output.clone()),
            DigestFormat::Text,
        ))?;
        let digest = fs::read_to_string(output)?;

        assert_eq!(report.included_files, 2);
        assert!(digest.contains("FILE: a\\\\b.txt"));
        assert!(digest.contains("FILE: a/b.txt"));
        Ok(())
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn excludes_output_case_insensitively() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("output_case")?;
        fs::write(root.join("source.txt"), "source")?;
        fs::write(root.join("digest.txt"), "previous output")?;
        let output = root.join("DIGEST.TXT");

        let report = ingest(&RunConfig::new(
            Source::Local(root.to_path_buf()),
            OutputTarget::File(output),
            DigestFormat::Text,
        ))?;
        let digest = fs::read_to_string(root.join("DIGEST.TXT"))?;

        assert_eq!(report.included_files, 1);
        assert!(!digest.contains("previous output"));
        assert!(digest.contains("FILE: source.txt"));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_directory_fails_fast_with_path_context() -> Result<(), Box<dyn std::error::Error>>
    {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_dir("unreadable_dir")?;
        let locked = root.join("locked");
        fs::create_dir(&locked)?;
        fs::write(locked.join("file.txt"), "content")?;
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o0000))?;

        if fs::read_dir(&locked).is_ok() {
            let _ = fs::set_permissions(&locked, fs::Permissions::from_mode(0o0755));
            return Ok(());
        }

        let res = collect(&root, None);

        // Restore permissions so TempDirGuard can clean up
        let _ = fs::set_permissions(&locked, fs::Permissions::from_mode(0o0755));

        let err = res.unwrap_err();
        assert!(
            matches!(&err, IngestError::Io(io_err) if io_err.kind() == std::io::ErrorKind::PermissionDenied),
            "expected IngestError::Io with ErrorKind::PermissionDenied, got: {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("locked"),
            "error message should contain path context 'locked', got: {msg}"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn accounting_rules_non_regular_files_and_unlinked_files()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        let root = temp_dir("accounting_non_regular")?;
        let target = root.join("regular.txt");
        let link = root.join("symlink.txt");

        fs::write(&target, "hello")?;
        symlink(&target, &link)?;

        // Symlink is filtered out before candidate reservation and should not increase skipped
        let res = collect(&root, None)?;
        assert_eq!(res.files.len(), 1);
        assert_eq!(res.skipped, 0);

        Ok(())
    }

    #[test]
    fn ingests_local_directory_respects_all_exclusion_policies()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("comprehensive_local_policies")?;
        fs::create_dir_all(root.join(".git"))?;
        fs::create_dir_all(root.join("certs"))?;
        fs::create_dir_all(root.join("sub"))?;

        // 1. Text file: included
        fs::write(root.join("README.md"), "hello")?;

        // 2. Binary file: skipped (increments skipped count)
        fs::write(root.join("binary.bin"), [0, 1, 2])?;

        // 3. Noise files: ignored without incrementing skipped
        fs::write(root.join("Cargo.lock"), "[lockfile]")?;
        fs::write(root.join("sub").join("package-lock.json"), "{}")?;

        // 4. Sensitive files: ignored without incrementing skipped
        fs::write(root.join(".env"), "SECRET=true")?;
        fs::write(root.join(".env.production"), "KEY=123")?;
        fs::write(root.join("certs").join("privkey.key"), "PRIVATE")?;
        fs::write(root.join("id_rsa"), "SSH")?;

        // 5. Git metadata: ignored
        fs::write(root.join(".git/config"), "secret")?;

        // 6. Gitignore: ignored without git directory
        fs::write(root.join(".gitignore"), "ignored.txt\n")?;
        fs::write(root.join("ignored.txt"), "ignored")?;

        let output = root.join("digest.txt");
        let report = ingest(&RunConfig::new(
            Source::Local(root.to_path_buf()),
            OutputTarget::File(output.clone()),
            DigestFormat::Text,
        ))?;
        let digest = fs::read_to_string(output)?;

        assert_eq!(report.included_files, 1);
        assert_eq!(report.skipped_files, 1);
        assert!(digest.contains("FILE: README.md"));
        assert!(!digest.contains("binary.bin"));
        assert!(!digest.contains("Cargo.lock"));
        assert!(!digest.contains("package-lock.json"));
        assert!(!digest.contains(".env"));
        assert!(!digest.contains("privkey.key"));
        assert!(!digest.contains("id_rsa"));
        assert!(!digest.contains(".git/config"));
        assert!(!digest.contains("ignored.txt"));
        Ok(())
    }

    #[test]
    fn does_not_apply_dot_ignore_files() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("dot_ignore")?;
        fs::write(root.join(".ignore"), "important.txt\n")?;
        fs::write(root.join("important.txt"), "important")?;
        let output = root.join("digest.txt");

        let report = ingest(&RunConfig::new(
            Source::Local(root.to_path_buf()),
            OutputTarget::File(output.clone()),
            DigestFormat::Text,
        ))?;
        let digest = fs::read_to_string(output)?;

        assert_eq!(report.included_files, 1);
        assert!(digest.contains("FILE: important.txt"));
        Ok(())
    }

    #[test]
    fn label_uses_directory_basename() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("custom_repo_name")?;
        fs::write(root.join("file.txt"), "hello")?;
        let output = root.join("digest.txt");
        let _ = ingest(&RunConfig::new(
            Source::Local(root.to_path_buf()),
            OutputTarget::File(output.clone()),
            DigestFormat::Text,
        ))?;
        let digest = fs::read_to_string(&output)?;
        let expected_basename = root.file_name().unwrap().to_str().unwrap();
        assert!(digest.starts_with(&format!("Repository: {expected_basename}\n")));
        Ok(())
    }

    #[test]
    fn label_handles_filesystem_root() {
        assert_eq!(local_label(Path::new("/")), "root");
        #[cfg(windows)]
        assert_eq!(local_label(Path::new("C:\\")), "root");
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn skips_non_utf8_paths() -> Result<(), Box<dyn std::error::Error>> {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let root = temp_dir("invalid_paths")?;
        fs::write(root.join(".gitignore"), "!*\n")?;
        fs::create_dir(root.join(".hidden"))?;
        fs::write(
            root.join(".hidden")
                .join(OsString::from_vec(b"c\x82".to_vec())),
            "c",
        )?;
        fs::write(root.join(OsString::from_vec(b"a\x80".to_vec())), "a")?;
        fs::write(root.join(OsString::from_vec(b"b\x81".to_vec())), "b")?;
        let output = root.join("digest.txt");
        let report = ingest(&RunConfig::new(
            Source::Local(root.to_path_buf()),
            OutputTarget::File(output),
            DigestFormat::Text,
        ))?;
        assert_eq!(report.included_files, 0);
        assert_eq!(report.skipped_files, 2);
        Ok(())
    }

    #[test]
    fn walk_preserves_deterministic_order() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("parallel_order")?;
        for dir in ["b_dir", "a_dir", "c_dir", "src-test", "src"] {
            fs::create_dir_all(root.join(dir))?;
            for file_index in 0..10 {
                fs::write(
                    root.join(dir).join(format!("file_{file_index:02}.txt")),
                    "content",
                )?;
            }
        }
        let result = collect(&root, None)?;
        let files = result.files;
        assert_eq!(files.len(), 50);
        let records: Vec<FileRecord<'_>> = files.iter().collect();
        for window in records.windows(2) {
            assert!(
                cmp_path(window[0].path, window[1].path).is_lt(),
                "ordering violated: {} >= {}",
                window[0].path,
                window[1].path
            );
        }
        Ok(())
    }

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
    fn hidden_directories_and_files_are_pruned_by_policy() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = temp_dir("hidden_pruning")?;
        fs::create_dir_all(root.join(".hidden_dir"))?;
        fs::write(root.join(".hidden_dir").join("nested.txt"), "nested")?;
        fs::write(root.join(".dotfile"), "secret")?;
        fs::write(root.join("visible.txt"), "visible")?;

        let result = collect(&root, None)?;
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files.iter().next().unwrap().path, "visible.txt");
        assert_eq!(result.skipped, 0);
        Ok(())
    }
}
