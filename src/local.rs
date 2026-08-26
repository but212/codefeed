use std::fs;
use std::io;
use std::path::Path;

use ignore::WalkBuilder;

use crate::digest::{ensure_digest_fits, FileRecord};
use crate::output::output_identity;
use crate::{text_content, IngestError, MAX_FILE_COUNT, MAX_FILE_SIZE};

#[derive(Default)]
pub(crate) struct CandidateBudget {
    count: usize,
}

impl CandidateBudget {
    pub(crate) fn add(&mut self) -> Result<(), IngestError> {
        self.count = self
            .count
            .checked_add(1)
            .ok_or_else(|| IngestError::InvalidMetadata("file count overflow".into()))?;
        if self.count > MAX_FILE_COUNT {
            return Err(IngestError::InvalidMetadata(format!(
                "repository contains more than {MAX_FILE_COUNT} files"
            )));
        }
        Ok(())
    }
}

pub(crate) fn collect_local(
    root: &Path,
    output: &Path,
) -> Result<(String, Option<String>, Vec<FileRecord>, usize), IngestError> {
    let root = fs::canonicalize(root)
        .map_err(|error| IngestError::InvalidSource(format!("{} ({error})", root.display())))?;
    if !root.is_dir() {
        return Err(IngestError::InvalidSource(format!(
            "{} is not a directory",
            root.display()
        )));
    }
    let output = output_identity(output)?;
    let label = root.display().to_string();
    let mut files = Vec::new();
    let mut skipped = 0;
    let mut candidates = CandidateBudget::default();

    let walker = WalkBuilder::new(&root)
        .hidden(true)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(false)
        .require_git(false)
        .parents(false)
        .follow_links(false)
        .build();

    for entry in walker {
        let entry = entry.map_err(|error| IngestError::Io(io::Error::other(error)))?;
        let path = entry.path();
        if path == root || path == output || !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        candidates.add()?;
        let relative = path
            .strip_prefix(&root)
            .map_err(|error| IngestError::InvalidMetadata(error.to_string()))?;
        let Some(relative) = relative.to_str() else {
            skipped += 1;
            continue;
        };
        let relative = relative.replace('\\', "/");
        let metadata = fs::metadata(path)?;
        if metadata.len() > MAX_FILE_SIZE {
            skipped += 1;
            continue;
        }
        let bytes = fs::read(path)?;
        if let Some(content) = text_content(bytes) {
            let candidate = FileRecord {
                path: relative,
                content,
            };
            ensure_digest_fits(&label, None, &files, skipped, Some(&candidate))?;
            files.push(candidate);
        } else {
            skipped += 1;
        }
    }

    Ok((label, None, files, skipped))
}
