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

pub(crate) fn escape_header(value: &str) -> Cow<'_, str> {
    if memchr::memchr3(b'\\', b'\r', b'\n', value.as_bytes()).is_none() {
        return Cow::Borrowed(value);
    }
    let mut out = String::with_capacity(value.len() + 8);
    for b in value.bytes() {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'\r' => out.push_str("\\r"),
            b'\n' => out.push_str("\\n"),
            _ => out.push(b as char),
        }
    }
    Cow::Owned(out)
}

pub(crate) fn render<'a, W, I>(
    writer: &mut W,
    label: &str,
    revision: Option<&str>,
    files: I,
    skipped: usize,
    trie: &BudgetTrie,
) -> Result<(), IngestError>
where
    W: Write + ?Sized,
    I: IntoIterator<Item = &'a FileRecord>,
{
    let mut ordered_files: Vec<(&str, &FileRecord)> =
        files.into_iter().map(|f| (f.path.as_str(), f)).collect();
    ordered_files.sort_unstable_by_key(|(path, _)| *path);

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

    for (_, file) in ordered_files {
        writer.write_all(FILE_BLOCK_PREFIX.as_bytes())?;
        writer.write_all(escape_header(&file.path).as_bytes())?;
        writer.write_all(FILE_BLOCK_SUFFIX.as_bytes())?;
        writer.write_all(file.content.as_bytes())?;
        if !file.content.ends_with('\n') {
            writer.write_all(NEWLINE.as_bytes())?;
        }
        writer.write_all(NEWLINE.as_bytes())?;
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
