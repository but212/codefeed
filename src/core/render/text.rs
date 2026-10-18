//! Plain-text format serializer.

use super::escape::{escape_header, write_escaped};
use super::tree::render_tree;
use crate::core::model::CollectionResult;
use std::io::Write;

const REPOSITORY_PREFIX: &str = "Repository: ";
const REVISION_PREFIX: &str = "Revision: ";
const FILES_PREFIX: &str = "Files: ";
const SKIPPED_PREFIX: &str = "\nSkipped: ";
const DIRECTORY_HEADER: &str = "Directory structure:\n";
const FILES_HEADER: &str = "\nFiles:\n\n";
const FILE_BLOCK_PREFIX: &str = "==================================================\nFILE: ";
const FILE_BLOCK_SUFFIX: &str = "\n==================================================\n\n";
const NEWLINE: &str = "\n";
const METADATA_SUFFIX: &str = "\n\n";

pub(super) fn render_text<W: Write + ?Sized>(
    writer: &mut W,
    collection: &CollectionResult,
) -> std::io::Result<()> {
    writer.write_all(REPOSITORY_PREFIX.as_bytes())?;
    write_escaped(writer, &collection.label, escape_header)?;
    writer.write_all(NEWLINE.as_bytes())?;
    if let Some(ref revision) = collection.revision {
        writer.write_all(REVISION_PREFIX.as_bytes())?;
        write_escaped(writer, revision, escape_header)?;
        writer.write_all(NEWLINE.as_bytes())?;
    }
    writer.write_all(FILES_PREFIX.as_bytes())?;
    write!(writer, "{}", collection.files.len())?;
    writer.write_all(SKIPPED_PREFIX.as_bytes())?;
    write!(writer, "{}", collection.skipped)?;
    writer.write_all(METADATA_SUFFIX.as_bytes())?;
    writer.write_all(DIRECTORY_HEADER.as_bytes())?;
    render_tree(writer, &collection.files, |w, name| {
        write_escaped(w, name, escape_header)
    })?;
    writer.write_all(FILES_HEADER.as_bytes())?;
    for file in collection.files.iter() {
        const TAIL: [&[u8]; 2] = [b"\n", b"\n\n"];
        let needs_extra_nl = (!file.content.ends_with('\n')) as usize;
        writer.write_all(FILE_BLOCK_PREFIX.as_bytes())?;
        write_escaped(writer, file.path, escape_header)?;
        writer.write_all(FILE_BLOCK_SUFFIX.as_bytes())?;
        writer.write_all(file.content.as_bytes())?;
        writer.write_all(TAIL[needs_extra_nl])?;
    }

    Ok(())
}
