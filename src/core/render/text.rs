//! Text format specification, escaping, and layout rendering.

use std::borrow::Cow;
use std::io::Write;

use crate::core::error::IngestError;
use crate::core::model::CollectionResult;

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

/// Escapes backslashes, carriage returns, and newlines in metadata header strings.
fn escape_header(value: &str) -> Cow<'_, str> {
    if !value.contains(['\\', '\r', '\n']) {
        Cow::Borrowed(value)
    } else {
        Cow::Owned(
            value
                .replace('\\', "\\\\")
                .replace('\r', "\\r")
                .replace('\n', "\\n"),
        )
    }
}

pub(crate) fn render_text<W: Write + ?Sized>(
    writer: &mut W,
    collection: &CollectionResult,
) -> Result<(), IngestError> {
    writer.write_all(REPOSITORY_PREFIX.as_bytes())?;
    writer.write_all(escape_header(&collection.label).as_bytes())?;
    writer.write_all(NEWLINE.as_bytes())?;
    if let Some(ref revision) = collection.revision {
        writer.write_all(REVISION_PREFIX.as_bytes())?;
        writer.write_all(escape_header(revision).as_bytes())?;
        writer.write_all(NEWLINE.as_bytes())?;
    }
    writer.write_all(FILES_PREFIX.as_bytes())?;
    write!(writer, "{}", collection.files.len())?;
    writer.write_all(SKIPPED_PREFIX.as_bytes())?;
    write!(writer, "{}", collection.skipped)?;
    writer.write_all(METADATA_SUFFIX.as_bytes())?;
    writer.write_all(DIRECTORY_HEADER.as_bytes())?;
    crate::core::tree::render_tree(writer, &collection.files, |w, name| {
        w.write_all(escape_header(name).as_bytes())
    })?;
    writer.write_all(FILES_HEADER.as_bytes())?;
    for file in collection.files.iter() {
        const TAIL: [&[u8]; 2] = [b"\n", b"\n\n"];
        let needs_extra_nl = (!file.content.ends_with('\n')) as usize;
        writer.write_all(FILE_BLOCK_PREFIX.as_bytes())?;
        writer.write_all(escape_header(file.path).as_bytes())?;
        writer.write_all(FILE_BLOCK_SUFFIX.as_bytes())?;
        writer.write_all(file.content.as_bytes())?;
        writer.write_all(TAIL[needs_extra_nl])?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_header_borrows_clean_input() {
        assert!(matches!(escape_header("clean_path"), Cow::Borrowed(_)));
        assert!(matches!(
            escape_header("path\nwith\nnewline"),
            Cow::Owned(_)
        ));
    }

    #[test]
    fn escape_header_replaces_all_target_characters_in_single_string() {
        let input = "a\\b\rc\nd\\\\e\r\n";
        let expected = "a\\\\b\\rc\\nd\\\\\\\\e\\r\\n";
        assert_eq!(escape_header(input).as_ref(), expected);
    }
}
