//! Local directory collection orchestration: walk, map, select.

use std::fs;
use std::io;
use std::path::Path;

use ignore::WalkBuilder;

use super::local_io::read_local_file;
use super::output_exclusion::OutputFilter;
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
    let output_filter = match output {
        OutputTarget::File(path) => Some(OutputFilter::new(path)?),
        OutputTarget::Stdout => None,
    };
    let label = local_label(&root);
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
            || output_filter
                .as_ref()
                .is_some_and(|filter| filter.is_excluded(entry_path))
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
        let probe = match &item_path {
            ItemPath::NonUtf8 => Probe::Missing,
            ItemPath::Utf8(path) if crate::core::policy::is_noise_path(path) => Probe::Ignored,
            ItemPath::Utf8(path) if crate::core::policy::has_binary_extension(path) => {
                Probe::Missing
            }
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
}
