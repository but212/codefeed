//! Local directory collection orchestration: walk, map, select.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use super::local_io::read_local_file;
use super::orchestrate::SourceStream;
use super::output_exclusion::OutputFilter;
use crate::core::decide::{CandidateBudget, Item, ItemPath, Probe};
use crate::core::error::IngestError;
use crate::core::model::OutputTarget;
use crate::core::policy::{MAX_FILE_COUNT, PathClassification};

struct LocalCandidate {
    path: PathBuf,
    item_path: ItemPath,
    skip_read: bool,
}

pub struct LocalStream {
    root: PathBuf,
    label: String,
    candidates: std::vec::IntoIter<LocalCandidate>,
}

impl std::fmt::Debug for LocalStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalStream")
            .field("root", &self.root)
            .field("label", &self.label)
            .finish()
    }
}

impl LocalStream {
    pub fn new(root: &Path, output: &OutputTarget) -> Result<Self, IngestError> {
        let canonical_root = validate_root(root)?;
        let output_filter = match output {
            OutputTarget::File(path) => OutputFilter::new(path)?,
            OutputTarget::Stdout => None,
        };
        let label = local_label(&canonical_root);
        let candidates = walk_candidates(&canonical_root, output_filter.as_ref())?;

        CandidateBudget::check_limit(candidates.iter().map(|c| c.item_path.as_utf8_str()))?;

        Ok(Self {
            root: canonical_root,
            label,
            candidates: candidates.into_iter(),
        })
    }
}

impl Iterator for LocalStream {
    type Item = Result<Item, IngestError>;

    fn next(&mut self) -> Option<Self::Item> {
        let candidate = self.candidates.next()?;
        let probe = if candidate.skip_read {
            Probe::Missing
        } else {
            match read_local_file(&self.root, &candidate.path) {
                Ok(p) => p,
                Err(err) => return Some(Err(err)),
            }
        };
        Some(Ok(Item {
            path: candidate.item_path,
            probe,
        }))
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.candidates.size_hint()
    }
}

impl ExactSizeIterator for LocalStream {}

impl SourceStream for LocalStream {
    fn label(&self) -> &str {
        &self.label
    }
}

fn validate_root(root: &Path) -> Result<PathBuf, IngestError> {
    let root_display = root.display();
    let canonical = fs::canonicalize(root)
        .map_err(|error| IngestError::InvalidSource(format!("{root_display} ({error})")))?;
    if !canonical.is_dir() {
        let display = canonical.display();
        return Err(IngestError::InvalidSource(format!(
            "{display} is not a directory"
        )));
    }
    Ok(canonical)
}

fn walk_candidates(
    root: &Path,
    output_filter: Option<&OutputFilter>,
) -> Result<Vec<LocalCandidate>, IngestError> {
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

    let mut candidates = Vec::new();

    for entry in walker {
        let entry = entry.map_err(|err| IngestError::Io(io::Error::other(err)))?;
        let entry_path = entry.path();
        if entry_path == root
            || !entry.file_type().is_some_and(|kind| kind.is_file())
            || output_filter.is_some_and(|filter| filter.is_excluded(entry_path))
        {
            continue;
        }

        let relative = entry_path
            .strip_prefix(root)
            .map_err(|err| IngestError::InvalidMetadata(err.to_string()))?;
        let item_path = normalize_relative_path(relative);

        let skip_read = match &item_path {
            ItemPath::NonUtf8 => true,
            ItemPath::Utf8(path) => match PathClassification::classify(path) {
                PathClassification::Ignored => continue,
                PathClassification::PreclassifiedBinary => true,
                PathClassification::NeedsContent => false,
            },
        };

        candidates.push(LocalCandidate {
            path: entry_path.to_path_buf(),
            item_path,
            skip_read,
        });

        if candidates.len() > MAX_FILE_COUNT {
            break;
        }
    }

    candidates.sort_unstable_by(|a, b| match (&a.item_path, &b.item_path) {
        (ItemPath::Utf8(left), ItemPath::Utf8(right)) => left.cmp(right),
        (ItemPath::Utf8(_), ItemPath::NonUtf8) => std::cmp::Ordering::Less,
        (ItemPath::NonUtf8, ItemPath::Utf8(_)) => std::cmp::Ordering::Greater,
        (ItemPath::NonUtf8, ItemPath::NonUtf8) => a.path.cmp(&b.path),
    });

    Ok(candidates)
}

fn normalize_relative_path(relative: &Path) -> ItemPath {
    let mut parts = relative.components().peekable();
    let Some(first) = parts.next() else {
        return ItemPath::Utf8(String::new());
    };
    let Some(first_str) = first.as_os_str().to_str() else {
        return ItemPath::NonUtf8;
    };
    if parts.peek().is_none() {
        return ItemPath::Utf8(first_str.to_owned());
    }

    let mut buf = String::with_capacity(relative.as_os_str().len());
    buf.push_str(first_str);
    for part in parts {
        let Some(s) = part.as_os_str().to_str() else {
            return ItemPath::NonUtf8;
        };
        buf.push('/');
        buf.push_str(s);
    }
    ItemPath::Utf8(buf)
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
    use crate::core::model::{DigestFormat, FileRecord, RunConfig, Source};
    use crate::ingest;

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

    #[test]
    fn excludes_output_through_nonexistent_parent_alias() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = temp_dir("output_alias")?;
        fs::write(root.join("source.txt"), "source")?;
        fs::write(root.join("digest.txt"), "previous output")?;
        let output = root.join("missing").join("..").join("digest.txt");

        let report = ingest(&RunConfig::new(
            Source::Local(root.to_path_buf()),
            OutputTarget::File(output),
            DigestFormat::Text,
        ))?;
        let digest = fs::read_to_string(root.join("digest.txt"))?;

        assert_eq!(report.included_files, 1);
        assert!(!digest.contains("previous output"));
        assert!(digest.contains("FILE: source.txt"));
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
            for file_idx in 0..10 {
                fs::write(
                    root.join(dir).join(format!("file_{file_idx:02}.txt")),
                    "content",
                )?;
            }
        }
        let result = crate::shell::orchestrate::collect_stream(LocalStream::new(
            &root,
            &OutputTarget::Stdout,
        )?)?;
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

        let err = LocalStream::new(&root, &OutputTarget::Stdout).unwrap_err();
        match err {
            IngestError::CandidateLimitExceeded { .. } => {
                let msg = err.to_string();
                assert!(msg.contains("repository contains more than 10000 files"));
            }
            other => panic!("expected CandidateLimitExceeded, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn test_local_candidate_size_is_under_cache_line() {
        assert!(std::mem::size_of::<LocalCandidate>() <= 64);
    }
}
