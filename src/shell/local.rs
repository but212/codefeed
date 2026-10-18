//! Local directory collection orchestration: walk, map, select.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use crate::core::error::IngestError;
use crate::core::model::CollectionResult;
use crate::core::policy::{MAX_FILE_SIZE, PathStatus, classify_path, is_hidden_segment};
use crate::core::selector::Selector;

/// Collects files from a local directory root, excluding the resolved output file if located inside it.
pub(super) fn collect(
    root: &Path,
    resolved_output: Option<&Path>,
) -> Result<CollectionResult, IngestError> {
    let canonical_root = validate_root(root)?;
    let label = local_label(&canonical_root);

    let walker = WalkBuilder::new(&canonical_root)
        .hidden(true)
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
    let mut path_scratch = String::with_capacity(256);
    let mut scratch = Vec::with_capacity(32 * 1024);

    for entry in walker {
        let entry = entry.map_err(|err| IngestError::Io(io::Error::other(err)))?;
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

        match classify_local_path(relative, &mut path_scratch) {
            PathStatus::Ignored => continue,
            PathStatus::NonUtf8Candidate => {
                selector.record_non_utf8()?;
            }
            PathStatus::Valid => {
                let slot = selector.reserve_candidate(&path_scratch)?;
                scratch.clear();
                match read_local_file_into(entry_path, &mut scratch)? {
                    Some(bytes) => selector.record_file(slot, bytes)?,
                    None => selector.record_skipped(slot),
                }
            }
        }
    }

    Ok(selector.finish(label, None))
}

#[cfg(test)]
thread_local! {
    static BODY_READ_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn read_local_file_into<'b>(
    path: &Path,
    buf: &'b mut Vec<u8>,
) -> Result<Option<&'b [u8]>, IngestError> {
    #[cfg(test)]
    BODY_READ_COUNT.with(|c| c.set(c.get() + 1));

    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(IngestError::Io(err)),
    };
    if meta.is_symlink() || !meta.is_file() || meta.len() > MAX_FILE_SIZE {
        return Ok(None);
    }
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(IngestError::Io(err)),
    };
    read_bounded_content_into(file, buf).map_err(Into::into)
}

fn read_bounded_content_into(file: fs::File, buf: &mut Vec<u8>) -> io::Result<Option<&[u8]>> {
    if !file.metadata()?.is_file() {
        return Ok(None);
    }
    let mut handle = file.take(MAX_FILE_SIZE.saturating_add(1));
    handle.read_to_end(buf)?;
    if buf.len() as u64 > MAX_FILE_SIZE {
        return Ok(None);
    }
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

fn classify_local_path(relative: &Path, scratch: &mut String) -> PathStatus {
    scratch.clear();
    let mut has_hidden = false;
    let mut has_non_utf8 = false;

    for part in relative.components() {
        if is_hidden_segment(part.as_os_str().as_encoded_bytes()) {
            has_hidden = true;
        }
        if let Some(s) = part.as_os_str().to_str() {
            if !scratch.is_empty() {
                scratch.push('/');
            }
            scratch.push_str(s);
        } else {
            has_non_utf8 = true;
        }
    }

    if has_hidden {
        scratch.clear();
        return PathStatus::Ignored;
    }
    if has_non_utf8 {
        scratch.clear();
        return PathStatus::NonUtf8Candidate;
    }
    if scratch.is_empty() {
        return PathStatus::Ignored;
    }
    let status = classify_path(scratch);
    if status == PathStatus::Ignored {
        scratch.clear();
    }
    status
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
    use super::super::temp_dir;
    use super::*;
    use crate::core::model::{DigestFormat, OutputTarget, RunConfig, Source};
    use crate::core::storage::FileRecord;
    use crate::ingest;

    #[test]
    fn read_local_file_enforces_size_boundaries() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("size_boundaries")?;
        let exact_path = root.join("exact.txt");
        let too_large_path = root.join("large.txt");
        fs::write(&exact_path, vec![b'x'; MAX_FILE_SIZE as usize])?;
        fs::write(&too_large_path, vec![b'x'; MAX_FILE_SIZE as usize + 1])?;

        let mut buf = Vec::new();
        assert!(read_local_file_into(&exact_path, &mut buf)?.is_some());
        buf.clear();
        assert!(read_local_file_into(&too_large_path, &mut buf)?.is_none());
        Ok(())
    }

    #[test]
    fn handles_unlinked_file_as_missing() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("missing_file")?;
        let path = root.join("nonexistent.txt");

        let mut buf = Vec::new();
        assert!(read_local_file_into(&path, &mut buf)?.is_none());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_file() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        let root = temp_dir("symlink_test")?;
        let target = root.join("target.txt");
        let link = root.join("link.txt");
        fs::write(&target, "content")?;
        symlink(&target, &link)?;

        let mut buf = Vec::new();
        assert!(read_local_file_into(&link, &mut buf)?.is_none());
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn rejects_windows_symlink_file() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::windows::fs::symlink_file;

        let root = temp_dir("win_symlink_test")?;
        let target = root.join("target.txt");
        let link = root.join("link.txt");
        fs::write(&target, "content")?;

        if symlink_file(&target, &link).is_ok() {
            let mut buf = Vec::new();
            assert!(read_local_file_into(&link, &mut buf)?.is_none());
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
    fn parallel_walk_preserves_deterministic_order() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("parallel_order")?;
        for dir in ["b_dir", "a_dir", "c_dir"] {
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
        assert_eq!(files.len(), 30);
        let records: Vec<FileRecord<'_>> = files.iter().collect();
        for window in records.windows(2) {
            assert!(
                window[0].path < window[1].path,
                "ordering violated: {} >= {}",
                window[0].path,
                window[1].path
            );
        }
        Ok(())
    }

    #[test]
    fn candidate_overflow_aborts_immediately_at_limit() -> Result<(), Box<dyn std::error::Error>> {
        use crate::core::policy::MAX_FILE_COUNT;

        let root = temp_dir("candidate_overflow_abort")?;
        for i in 0..(MAX_FILE_COUNT + 50) {
            fs::write(root.join(format!("file_{i:05}.txt")), b"")?;
        }

        BODY_READ_COUNT.with(|c| c.set(0));

        let err = collect(&root, None).unwrap_err();
        match err {
            IngestError::CandidateLimitExceeded(..) => {
                let msg = err.to_string();
                assert!(msg.contains("repository contains more than 10000 files"));
            }
            other => panic!("expected CandidateLimitExceeded, got {other:?}"),
        }

        assert_eq!(
            BODY_READ_COUNT.with(|c| c.get()),
            MAX_FILE_COUNT,
            "must abort traversal immediately upon exceeding limit without reading remaining files"
        );
        Ok(())
    }
}
