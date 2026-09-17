//! Local directory collection orchestration: walk, map, select.

use std::fs;
use std::io;
use std::path::Path;

use ignore::WalkBuilder;

use super::local_io::read_local_file;
use crate::decide::{Item, ItemPath, Probe, Selector};
use crate::error::IngestError;
use crate::model::CollectionResult;
use crate::output::output_identity;

pub(crate) fn collect_local(root: &Path, output: &Path) -> Result<CollectionResult, IngestError> {
    let root = fs::canonicalize(root)
        .map_err(|error| IngestError::InvalidSource(format!("{} ({error})", root.display())))?;
    if !root.is_dir() {
        return Err(IngestError::InvalidSource(format!(
            "{} is not a directory",
            root.display()
        )));
    }
    let exclude = output_identity(output)?;
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
            || entry_path == exclude
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
