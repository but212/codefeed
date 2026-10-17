//! Local directory collection orchestration: walk, map, select.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use crate::core::error::IngestError;
use crate::core::policy::MAX_FILE_SIZE;
use crate::core::selector::{CandidateBudget, CandidateInput, Selector};

#[derive(Default, Debug)]
struct PathArena {
    data: String,
    spans: Vec<Option<(u32, u32)>>,
}

impl PathArena {
    fn with_capacity(spans_cap: usize, bytes_cap: usize) -> Self {
        Self {
            data: String::with_capacity(bytes_cap),
            spans: Vec::with_capacity(spans_cap),
        }
    }

    fn push_some(&mut self, path: &str) {
        let offset = self.data.len() as u32;
        let len = path.len() as u32;
        self.data.push_str(path);
        self.spans.push(Some((offset, len)));
    }

    fn push_none(&mut self) {
        self.spans.push(None);
    }

    fn len(&self) -> usize {
        self.spans.len()
    }

    fn iter(&self) -> impl ExactSizeIterator<Item = Option<&str>> {
        self.spans.iter().map(move |&span| {
            span.map(|(offset, len)| {
                let start = offset as usize;
                let end = start + len as usize;
                &self.data[start..end]
            })
        })
    }
}

/// Collects files from a local directory root, excluding the resolved output file if located inside it.
pub(crate) fn collect(
    root: &Path,
    resolved_output: Option<&Path>,
) -> Result<crate::core::model::CollectionResult, IngestError> {
    let canonical_root = validate_root(root)?;
    let label = local_label(&canonical_root);
    let candidates = walk_candidates(&canonical_root, resolved_output)?;

    let mut selector = Selector::from_size_hint((candidates.len(), Some(candidates.len())));
    let mut work_path = PathBuf::new();
    let mut scratch = Vec::with_capacity(32 * 1024);

    for item_path in candidates.iter() {
        let input = match item_path {
            Some(rel) => {
                work_path.clear();
                work_path.push(&canonical_root);
                work_path.push(rel);
                scratch.clear();
                match read_local_file_into(&work_path, &mut scratch)? {
                    LocalFileRead::Bytes(bytes) => CandidateInput::File { path: rel, bytes },
                    LocalFileRead::TooLarge => CandidateInput::TooLarge { path: rel },
                    LocalFileRead::Missing => CandidateInput::Skipped { path: Some(rel) },
                }
            }
            None => CandidateInput::Skipped { path: None },
        };
        selector.feed(input)?;
    }

    Ok(selector.finish(label, None))
}

#[derive(Debug, PartialEq, Eq)]
enum LocalFileRead<'b> {
    Bytes(&'b [u8]),
    Missing,
    TooLarge,
}

fn read_local_file_into<'b>(
    path: &Path,
    buf: &'b mut Vec<u8>,
) -> Result<LocalFileRead<'b>, IngestError> {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(LocalFileRead::Missing),
        Err(err) => return Err(IngestError::Io(err)),
    };
    if meta.is_symlink() || !meta.is_file() {
        return Ok(LocalFileRead::Missing);
    }
    if meta.len() > MAX_FILE_SIZE {
        return Ok(LocalFileRead::TooLarge);
    }
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(LocalFileRead::Missing),
        Err(err) => return Err(IngestError::Io(err)),
    };
    read_bounded_content_into(file, buf).map_err(Into::into)
}

fn read_bounded_content_into<'b>(
    file: fs::File,
    buf: &'b mut Vec<u8>,
) -> io::Result<LocalFileRead<'b>> {
    if !file.metadata()?.is_file() {
        return Ok(LocalFileRead::Missing);
    }
    let mut handle = file.take(MAX_FILE_SIZE.saturating_add(1));
    handle.read_to_end(buf)?;
    if buf.len() as u64 > MAX_FILE_SIZE {
        return Ok(LocalFileRead::TooLarge);
    }
    Ok(LocalFileRead::Bytes(buf.as_slice()))
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

fn walk_candidates(root: &Path, resolved_output: Option<&Path>) -> Result<PathArena, IngestError> {
    let walker = WalkBuilder::new(root)
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

    let mut candidates = PathArena::with_capacity(512, 16 * 1024);
    let mut budget = CandidateBudget::with_capacity(64);
    let mut path_scratch = String::with_capacity(256);

    for entry in walker {
        let entry = entry.map_err(|err| IngestError::Io(io::Error::other(err)))?;
        let entry_path = entry.path();
        if entry_path == root
            || !entry.file_type().is_some_and(|kind| kind.is_file())
            || resolved_output.is_some_and(|target| entry_path == target)
        {
            continue;
        }

        let relative = entry_path
            .strip_prefix(root)
            .map_err(|err| IngestError::invalid_metadata(err.to_string()))?;

        if normalize_relative_path_into(relative, &mut path_scratch) {
            if crate::core::policy::is_ignored_path(&path_scratch) {
                continue;
            }
            budget.add(Some(&path_scratch))?;
            candidates.push_some(&path_scratch);
        } else {
            budget.add(None)?;
            candidates.push_none();
        }
    }

    Ok(candidates)
}

fn normalize_relative_path_into(relative: &Path, scratch: &mut String) -> bool {
    scratch.clear();
    let mut parts = relative.components();
    let Some(first) = parts.next() else {
        return false;
    };
    let Some(first_str) = first.as_os_str().to_str() else {
        return false;
    };
    scratch.push_str(first_str);
    for part in parts {
        let Some(s) = part.as_os_str().to_str() else {
            return false;
        };
        scratch.push('/');
        scratch.push_str(s);
    }
    true
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
    use crate::core::model::{DigestFormat, FileRecord, OutputTarget, RunConfig, Source};
    use crate::ingest;

    #[test]
    fn read_local_file_enforces_size_boundaries() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("size_boundaries")?;
        let exact_path = root.join("exact.txt");
        let too_large_path = root.join("large.txt");
        fs::write(&exact_path, vec![b'x'; MAX_FILE_SIZE as usize])?;
        fs::write(&too_large_path, vec![b'x'; MAX_FILE_SIZE as usize + 1])?;

        let mut buf = Vec::new();
        assert!(matches!(
            read_local_file_into(&exact_path, &mut buf)?,
            LocalFileRead::Bytes(_)
        ));
        buf.clear();
        assert!(matches!(
            read_local_file_into(&too_large_path, &mut buf)?,
            LocalFileRead::TooLarge
        ));
        Ok(())
    }

    #[test]
    fn handles_unlinked_file_as_missing() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("missing_file")?;
        let path = root.join("nonexistent.txt");

        let mut buf = Vec::new();
        assert!(matches!(
            read_local_file_into(&path, &mut buf)?,
            LocalFileRead::Missing
        ));
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
        assert!(matches!(
            read_local_file_into(&link, &mut buf)?,
            LocalFileRead::Missing
        ));
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
            assert!(matches!(
                read_local_file_into(&link, &mut buf)?,
                LocalFileRead::Missing
            ));
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
    fn candidate_overflow_does_not_read_file_bodies() -> Result<(), Box<dyn std::error::Error>> {
        use crate::core::policy::MAX_FILE_COUNT;

        #[cfg(windows)]
        use std::os::windows::fs::OpenOptionsExt;

        let root = temp_dir("candidate_overflow_no_read")?;
        for i in 0..=MAX_FILE_COUNT {
            fs::write(root.join(format!("file_{i:05}.txt")), b"")?;
        }

        #[cfg(windows)]
        let _lock = fs::OpenOptions::new()
            .write(true)
            .share_mode(0)
            .open(root.join("file_00000.txt"))?;

        let err = collect(&root, None).unwrap_err();
        match err {
            IngestError::CandidateLimitExceeded(..) => {
                let msg = err.to_string();
                assert!(msg.contains("repository contains more than 10000 files"));
            }
            other => panic!("expected CandidateLimitExceeded, got {other:?}"),
        }
        Ok(())
    }
}
