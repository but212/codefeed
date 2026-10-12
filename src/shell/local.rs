//! Local directory collection orchestration: walk, map, select.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use ignore::{WalkBuilder, WalkState};

use super::local_io::read_local_file;
use super::output_exclusion::OutputFilter;
use crate::core::decide::{CandidateBudget, Item, ItemPath, Probe, Selector};
use crate::core::error::IngestError;
use crate::core::model::{CollectionResult, DigestFormat, OutputTarget};
use crate::core::policy::PathClassification;

#[derive(Clone)]
struct LocalCandidate {
    path: PathBuf,
    item_path: ItemPath,
    probe: Option<Probe>,
}

pub fn collect_local(
    root: &Path,
    output: &OutputTarget,
    format: DigestFormat,
) -> Result<CollectionResult, IngestError> {
    let canonical_root = validate_root(root)?;
    let output_filter = match output {
        OutputTarget::File(path) => Some(OutputFilter::new(path)?),
        OutputTarget::Stdout => None,
    };
    let label = local_label(&canonical_root);
    let mut candidates = walk_candidates(&canonical_root, output_filter.as_ref())?;
    let mut selector = Selector::new(&label, None, format)?;

    for window in candidates.chunks_mut(crate::core::policy::FETCH_WINDOW) {
        let probes = probe_candidates(&canonical_root, window)?;
        for (candidate, probe) in window.iter_mut().zip(probes) {
            selector.decide(Item {
                path: std::mem::take(&mut candidate.item_path),
                probe,
            })?;
        }
    }

    let (files, skipped, trie) = selector.into_parts();

    Ok(CollectionResult {
        label,
        revision: None,
        files,
        skipped,
        trie,
    })
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
        .build_parallel();

    let (tx, rx) = std::sync::mpsc::channel();
    let root_clone = root.to_path_buf();
    let filter_arc = output_filter.cloned().map(std::sync::Arc::new);
    let candidate_counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

    walker.run(|| {
        let tx = tx.clone();
        let output_filter = filter_arc.clone();
        let root = root_clone.clone();
        let candidate_counter = candidate_counter.clone();
        Box::new(move |entry| {
            if candidate_counter.load(std::sync::atomic::Ordering::Relaxed)
                > crate::core::policy::MAX_FILE_COUNT
            {
                return WalkState::Quit;
            }
            let entry = match entry {
                Ok(e) => e,
                Err(err) => {
                    let _ = tx.send(Err(IngestError::Io(io::Error::other(err))));
                    return WalkState::Continue;
                }
            };
            let entry_path = entry.path();
            if entry_path == root
                || output_filter
                    .as_ref()
                    .is_some_and(|filter| filter.is_excluded(entry_path))
                || !entry.file_type().is_some_and(|kind| kind.is_file())
            {
                return WalkState::Continue;
            }
            let relative = match entry_path.strip_prefix(&root) {
                Ok(rel) => rel,
                Err(err) => {
                    let _ = tx.send(Err(IngestError::InvalidMetadata(err.to_string())));
                    return WalkState::Quit;
                }
            };
            let item_path = match relative
                .components()
                .map(|part| part.as_os_str().to_str())
                .collect::<Option<Vec<_>>>()
            {
                Some(parts) => ItemPath::Utf8(parts.join("/")),
                None => ItemPath::NonUtf8,
            };

            let probe = match &item_path {
                ItemPath::NonUtf8 => Some(Probe::Missing),
                ItemPath::Utf8(path) => match PathClassification::classify(path) {
                    PathClassification::Ignored => Some(Probe::Ignored),
                    PathClassification::PreclassifiedBinary => Some(Probe::Missing),
                    PathClassification::NeedsContent => None,
                },
            };

            if matches!(probe, Some(Probe::Ignored)) {
                return WalkState::Continue;
            }

            if candidate_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                > crate::core::policy::MAX_FILE_COUNT
            {
                return WalkState::Quit;
            }

            let _ = tx.send(Ok(LocalCandidate {
                path: entry_path.to_path_buf(),
                item_path,
                probe,
            }));
            WalkState::Continue
        })
    });

    drop(tx);

    let mut candidates: Vec<LocalCandidate> = rx.into_iter().collect::<Result<_, _>>()?;

    CandidateBudget::verify_count(
        candidates.iter().map(|item| match &item.item_path {
            ItemPath::Utf8(p) => Some(p.as_str()),
            ItemPath::NonUtf8 => None,
        }),
        candidates.len(),
    )?;

    candidates.sort_by(|a, b| match (&a.item_path, &b.item_path) {
        (ItemPath::Utf8(left), ItemPath::Utf8(right)) => left.cmp(right),
        (ItemPath::Utf8(_), ItemPath::NonUtf8) => std::cmp::Ordering::Less,
        (ItemPath::NonUtf8, ItemPath::Utf8(_)) => std::cmp::Ordering::Greater,
        (ItemPath::NonUtf8, ItemPath::NonUtf8) => a.path.cmp(&b.path),
    });

    Ok(candidates)
}

fn probe_candidates(root: &Path, candidates: &[LocalCandidate]) -> Result<Vec<Probe>, IngestError> {
    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    let next_idx = std::sync::atomic::AtomicUsize::new(0);
    let worker_count = crate::core::policy::FETCH_WINDOW
        .min(candidates.len())
        .max(1);

    std::thread::scope(|s| {
        let handles: Vec<_> = (0..worker_count)
            .map(|_| {
                s.spawn(|| {
                    let mut local_results = Vec::new();
                    loop {
                        let idx = next_idx.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if idx >= candidates.len() {
                            break;
                        }
                        let candidate = &candidates[idx];
                        let res = match &candidate.probe {
                            Some(probe) => Ok(probe.clone()),
                            None => read_local_file(root, &candidate.path),
                        };
                        local_results.push((idx, res));
                    }
                    local_results
                })
            })
            .collect();

        let mut results: Vec<_> = handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("reader thread panicked"))
            .collect();
        results.sort_unstable_by_key(|(idx, _)| *idx);
        results.into_iter().map(|(_, res)| res).collect()
    })
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
    use crate::core::model::{Options, Source};
    use crate::ingest;

    #[cfg(unix)]
    #[test]
    fn preserves_distinct_unix_paths() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("path_encoding")?;
        fs::create_dir_all(root.join("a"))?;
        fs::write(root.join("a\\b.txt"), "backslash")?;
        fs::write(root.join("a").join("b.txt"), "slash")?;
        let output = root.join("digest.txt");

        let report = ingest(
            &Source::Local(root.clone()),
            &Options {
                output: OutputTarget::File(output.clone()),
                reference: None,
                ..Default::default()
            },
        )?;
        let digest = fs::read_to_string(output)?;

        assert_eq!(report.included_files, 2);
        assert!(digest.contains("FILE: a\\\\b.txt"));
        assert!(digest.contains("FILE: a/b.txt"));
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn excludes_output_through_nonexistent_parent_alias() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = temp_dir("output_alias")?;
        fs::create_dir_all(&root)?;
        fs::write(root.join("source.txt"), "source")?;
        fs::write(root.join("digest.txt"), "previous output")?;
        let output = root.join("missing").join("..").join("digest.txt");

        let report = ingest(
            &Source::Local(root.clone()),
            &Options {
                output: OutputTarget::File(output),
                reference: None,
                ..Default::default()
            },
        )?;
        let digest = fs::read_to_string(root.join("digest.txt"))?;

        assert_eq!(report.included_files, 1);
        assert!(!digest.contains("previous output"));
        assert!(digest.contains("FILE: source.txt"));
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn excludes_output_case_insensitively() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("output_case")?;
        fs::create_dir_all(&root)?;
        fs::write(root.join("source.txt"), "source")?;
        fs::write(root.join("digest.txt"), "previous output")?;
        let output = root.join("DIGEST.TXT");

        let report = ingest(
            &Source::Local(root.clone()),
            &Options {
                output: OutputTarget::File(output),
                reference: None,
                ..Default::default()
            },
        )?;
        let digest = fs::read_to_string(root.join("DIGEST.TXT"))?;

        assert_eq!(report.included_files, 1);
        assert!(!digest.contains("previous output"));
        assert!(digest.contains("FILE: source.txt"));
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn excludes_noise_files_conventionally() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("noise_files")?;
        fs::create_dir_all(root.join("sub"))?;
        fs::write(root.join("src.rs"), "fn main() {}")?;
        fs::write(root.join("Cargo.lock"), "[lockfile]")?;
        fs::write(root.join("sub").join("package-lock.json"), "{}")?;
        let output = root.join("digest.txt");

        let report = ingest(
            &Source::Local(root.clone()),
            &Options {
                output: OutputTarget::File(output.clone()),
                reference: None,
                ..Default::default()
            },
        )?;
        let digest = fs::read_to_string(output)?;

        assert_eq!(report.included_files, 1);
        assert_eq!(report.skipped_files, 0);
        assert!(digest.contains("FILE: src.rs"));
        assert!(!digest.contains("Cargo.lock"));
        assert!(!digest.contains("package-lock.json"));
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn excludes_sensitive_files_without_skipping() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("sensitive_files")?;
        fs::create_dir_all(root.join("certs"))?;
        fs::write(root.join("main.rs"), "fn main() {}")?;
        fs::write(root.join(".env"), "SECRET=true")?;
        fs::write(root.join(".env.production"), "KEY=123")?;
        fs::write(root.join("certs").join("privkey.key"), "PRIVATE")?;
        fs::write(root.join("id_rsa"), "SSH")?;
        let output = root.join("digest.txt");

        let report = ingest(
            &Source::Local(root.clone()),
            &Options {
                output: OutputTarget::File(output.clone()),
                reference: None,
                ..Default::default()
            },
        )?;
        let digest = fs::read_to_string(output)?;

        assert_eq!(report.included_files, 1);
        assert_eq!(report.skipped_files, 0);
        assert!(digest.contains("FILE: main.rs"));
        assert!(!digest.contains(".env"));
        assert!(!digest.contains("privkey.key"));
        assert!(!digest.contains("id_rsa"));
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn ingests_local_text_and_skips_binary_and_git_directory()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("local")?;
        fs::create_dir_all(root.join(".git"))?;
        fs::write(root.join("README.md"), "hello")?;
        fs::write(root.join("binary.bin"), [0, 1, 2])?;
        fs::write(root.join(".git/config"), "secret")?;
        fs::write(root.join(".gitignore"), "ignored.txt\n")?;
        fs::write(root.join("ignored.txt"), "ignored")?;
        let output = root.join("digest.txt");

        let report = ingest(
            &Source::Local(root.clone()),
            &Options {
                output: OutputTarget::File(output.clone()),
                reference: None,
                ..Default::default()
            },
        )?;
        let digest = fs::read_to_string(output)?;
        assert_eq!(report.included_files, 1);
        assert_eq!(report.skipped_files, 1);
        assert!(digest.contains("FILE: README.md"));
        assert!(!digest.contains("binary.bin"));
        assert!(!digest.contains(".git/config"));
        assert!(!digest.contains("ignored.txt"));
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn applies_gitignore_without_git_directory() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("ignore_without_git")?;
        fs::create_dir_all(&root)?;
        fs::write(root.join(".gitignore"), "ignored.txt\n")?;
        fs::write(root.join("kept.txt"), "kept")?;
        fs::write(root.join("ignored.txt"), "ignored")?;
        let output = root.join("digest.txt");

        let report = ingest(
            &Source::Local(root.clone()),
            &Options {
                output: OutputTarget::File(output.clone()),
                reference: None,
                ..Default::default()
            },
        )?;
        let digest = fs::read_to_string(output)?;
        assert_eq!(report.included_files, 1);
        assert!(digest.contains("FILE: kept.txt"));
        assert!(!digest.contains("ignored.txt"));
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn does_not_apply_dot_ignore_files() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("dot_ignore")?;
        fs::create_dir_all(&root)?;
        fs::write(root.join(".ignore"), "important.txt\n")?;
        fs::write(root.join("important.txt"), "important")?;
        let output = root.join("digest.txt");

        let report = ingest(
            &Source::Local(root.clone()),
            &Options {
                output: OutputTarget::File(output.clone()),
                reference: None,
                ..Default::default()
            },
        )?;
        let digest = fs::read_to_string(output)?;

        assert_eq!(report.included_files, 1);
        assert!(digest.contains("FILE: important.txt"));
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn label_uses_directory_basename() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("custom_repo_name")?;
        fs::create_dir_all(&root)?;
        fs::write(root.join("file.txt"), "hello")?;
        let output = root.join("digest.txt");
        let _ = ingest(
            &Source::Local(root.clone()),
            &Options {
                output: OutputTarget::File(output.clone()),
                reference: None,
                ..Default::default()
            },
        )?;
        let digest = fs::read_to_string(&output)?;
        let expected_basename = root.file_name().unwrap().to_str().unwrap();
        assert!(digest.starts_with(&format!("Repository: {expected_basename}\n")));
        fs::remove_dir_all(root)?;
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
        fs::create_dir_all(&root)?;
        fs::write(root.join(OsString::from_vec(b"a\x80".to_vec())), "a")?;
        fs::write(root.join(OsString::from_vec(b"b\x81".to_vec())), "b")?;
        let output = root.join("digest.txt");
        let report = ingest(
            &Source::Local(root.clone()),
            &Options {
                output: OutputTarget::File(output),
                reference: None,
                ..Default::default()
            },
        )?;
        assert_eq!(report.included_files, 0);
        assert_eq!(report.skipped_files, 2);
        fs::remove_dir_all(root)?;
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
        let result = collect_local(&root, &OutputTarget::Stdout, DigestFormat::Text)?;
        let files = result.files;
        assert_eq!(files.len(), 30);
        for window in files.windows(2) {
            assert!(
                window[0].path < window[1].path,
                "ordering violated: {} >= {}",
                window[0].path,
                window[1].path
            );
        }
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn parallel_walk_excludes_sensitive_files() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("parallel_sensitive")?;
        fs::create_dir_all(&root)?;
        fs::write(root.join(".env"), "SECRET=1")?;
        fs::write(root.join("id_rsa"), "PRIVATE KEY")?;
        fs::write(root.join("public.txt"), "public")?;

        let result = collect_local(&root, &OutputTarget::Stdout, DigestFormat::Text)?;
        let files = result.files;
        let skipped = result.skipped;
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "public.txt");
        assert_eq!(
            skipped, 0,
            "sensitive files must be excluded without consuming skipped budget"
        );
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn parallel_walk_bounds_candidate_memory_early() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("parallel_candidate_bound")?;
        fs::create_dir_all(&root)?;
        for idx in 0..10_005 {
            fs::write(root.join(format!("f_{idx:05}.txt")), "x")?;
        }
        let result = collect_local(&root, &OutputTarget::Stdout, DigestFormat::Text);
        match result {
            Err(IngestError::CandidateLimitExceeded(msg)) => {
                assert!(msg.contains("repository contains more than 10000 files"));
            }
            other => panic!("expected CandidateLimitExceeded candidate overflow, got {other:?}"),
        }
        fs::remove_dir_all(root)?;
        Ok(())
    }
}
