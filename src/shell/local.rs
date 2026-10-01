//! Local directory collection orchestration: walk, map, select.

use std::fs;
use std::io;
use std::path::Path;

use ignore::WalkBuilder;

use super::local_io::read_local_file;
use crate::core::decide::{Item, ItemPath, Probe, Selector};
use crate::core::error::IngestError;
use crate::core::model::{CollectionResult, OutputTarget};
use crate::shell::output::output_identity;

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
