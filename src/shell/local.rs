//! Local directory collection orchestration: walk, map, select.

use std::env;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use ignore::WalkBuilder;

use super::local_io::read_local_file;
use crate::core::decide::{Item, ItemPath, Probe, Selector};
use crate::core::error::IngestError;
use crate::core::model::{CollectionResult, OutputTarget};

pub(crate) fn collect_local(
    root: &Path,
    output: &OutputTarget,
) -> Result<CollectionResult, IngestError> {
    let root = fs::canonicalize(root)
        .map_err(|error| IngestError::InvalidSource(format!("{} ({error})", root.display())))?;
    if !root.is_dir() {
        return Err(IngestError::InvalidSource(format!(
            "{} is not a directory",
            root.display()
        )));
    }
    // Standard output has no filesystem path, so there is nothing to exclude
    // from the walk.
    let exclude = match output {
        OutputTarget::File(path) => Some(output_identity(path)?),
        OutputTarget::Stdout => None,
    };
    let label = root.display().to_string();
    let mut selector = Selector::new(&label, None)?;

    let walker = WalkBuilder::new(&root)
        .hidden(true)
        .ignore(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(false)
        .require_git(false)
        .parents(false)
        .follow_links(false)
        .build();

    for entry in walker {
        let entry = entry.map_err(|error| IngestError::Io(io::Error::other(error)))?;
        let entry_path = entry.path();
        if entry_path == root
            || exclude
                .as_ref()
                .is_some_and(|excluded| entry_path == excluded)
            || !entry.file_type().is_some_and(|kind| kind.is_file())
        {
            continue;
        }
        let relative = entry_path
            .strip_prefix(&root)
            .map_err(|error| IngestError::InvalidMetadata(error.to_string()))?;
        let item_path = match relative
            .components()
            .map(|part| part.as_os_str().to_str())
            .collect::<Option<Vec<_>>>()
        {
            Some(parts) => ItemPath::Utf8(parts.join("/")),
            None => ItemPath::NonUtf8,
        };
        let probe = match item_path {
            ItemPath::NonUtf8 => Probe::Missing,
            ItemPath::Utf8(_) => read_local_file(&root, entry_path)?,
        };
        selector.decide(Item {
            path: item_path,
            probe,
        })?;
    }

    let (files, skipped) = selector.into_parts();
    Ok(CollectionResult::Local {
        label,
        files,
        skipped,
    })
}

fn output_identity(output: &Path) -> Result<PathBuf, IngestError> {
    let absolute = if output.is_absolute() {
        output.to_path_buf()
    } else {
        env::current_dir()?.join(output)
    };
    let file_name = absolute.file_name().unwrap_or_default().to_os_string();
    let mut parent = absolute
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let mut unresolved = Vec::new();
    let canonical_parent = loop {
        match fs::canonicalize(&parent) {
            Ok(parent) => break parent,
            Err(_) => {
                let Some(component) = parent.file_name() else {
                    break parent;
                };
                unresolved.push(component.to_os_string());
                parent = parent
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .to_path_buf();
            }
        }
    };

    let mut resolved = canonical_parent;
    for component in unresolved.iter().rev() {
        resolved.push(component);
    }
    resolved.push(file_name);
    Ok(normalize_path(&resolved))
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{Options, Source};
    use crate::ingest;
    use std::time::{SystemTime, UNIX_EPOCH};

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
            },
        )?;
        let digest = fs::read_to_string(root.join("digest.txt"))?;

        assert_eq!(report.included_files, 1);
        assert!(!digest.contains("previous output"));
        assert!(digest.contains("FILE: source.txt"));
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
            },
        )?;
        let digest = fs::read_to_string(output)?;

        assert_eq!(report.included_files, 1);
        assert!(digest.contains("FILE: important.txt"));
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[cfg(unix)]
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
            },
        )?;
        assert_eq!(report.included_files, 0);
        assert_eq!(report.skipped_files, 2);
        fs::remove_dir_all(root)?;
        Ok(())
    }

    fn temp_dir(label: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        Ok(env::temp_dir().join(format!("local_ingest_{label}_{nanos}")))
    }
}
