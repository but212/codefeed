//! Plain text format specification and escaping.

use std::borrow::Cow;
use std::io::Write;

use super::trie::BudgetTrie;
use crate::core::error::IngestError;
use crate::core::model::FileRecord;

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

const fn make_header_escape_table() -> [Option<&'static str>; 256] {
    let mut table = [None; 256];
    table[b'\\' as usize] = Some("\\\\");
    table[b'\r' as usize] = Some("\\r");
    table[b'\n' as usize] = Some("\\n");
    table
}

const HEADER_ESCAPE_TABLE: [Option<&'static str>; 256] = make_header_escape_table();

pub(crate) fn escape_header(value: &str) -> Cow<'_, str> {
    let bytes = value.as_bytes();
    if !bytes
        .iter()
        .any(|&b| HEADER_ESCAPE_TABLE[b as usize].is_some())
    {
        return Cow::Borrowed(value);
    }

    let mut out = String::with_capacity(value.len() + 8);
    let mut last = 0;
    for (i, &b) in bytes.iter().enumerate() {
        if let Some(escaped) = HEADER_ESCAPE_TABLE[b as usize] {
            out.push_str(&value[last..i]);
            out.push_str(escaped);
            last = i + 1;
        }
    }
    out.push_str(&value[last..]);
    Cow::Owned(out)
}

pub(crate) fn render<W>(
    writer: &mut W,
    label: &str,
    revision: Option<&str>,
    ordered_files: &[FileRecord<'_>],
    skipped: usize,
    trie: &BudgetTrie,
) -> Result<(), IngestError>
where
    W: Write + ?Sized,
{
    writer.write_all(REPOSITORY_PREFIX.as_bytes())?;
    writer.write_all(escape_header(label).as_bytes())?;
    writer.write_all(NEWLINE.as_bytes())?;
    if let Some(revision) = revision {
        writer.write_all(REVISION_PREFIX.as_bytes())?;
        writer.write_all(escape_header(revision).as_bytes())?;
        writer.write_all(NEWLINE.as_bytes())?;
    }
    writer.write_all(FILES_PREFIX.as_bytes())?;
    writer.write_all(ordered_files.len().to_string().as_bytes())?;
    writer.write_all(SKIPPED_PREFIX.as_bytes())?;
    writer.write_all(skipped.to_string().as_bytes())?;
    writer.write_all(METADATA_SUFFIX.as_bytes())?;
    writer.write_all(DIRECTORY_HEADER.as_bytes())?;
    trie.render_to_writer(writer)?;
    writer.write_all(FILES_HEADER.as_bytes())?;

    for file in ordered_files {
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
pub(crate) mod tests {
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
