//! Plain text format specification and escaping.

use std::borrow::Cow;
use std::io::Write;

use super::DigestBudget;
use super::byte_count;
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
    if !value.contains('\\') && !value.contains('\r') && !value.contains('\n') {
        return Cow::Borrowed(value);
    }
    Cow::Owned(
        value
            .replace('\\', "\\\\")
            .replace('\r', "\\r")
            .replace('\n', "\\n"),
    )
}

pub(crate) fn calculate_file_bytes(file: &FileRecord) -> Result<(u64, u64, u64), IngestError> {
    let path_b = byte_count(&escape_header(&file.path))?;
    let content_b = byte_count(&file.content)?;
    let trailing_b = (usize::from(!file.content.ends_with('\n')) + 1) as u64;
    Ok((path_b, content_b, trailing_b))
}

pub(crate) fn rendered_base_size(budget: &DigestBudget) -> Option<u64> {
    let count = u64::try_from(budget.file_count).ok()?;
    let mut size = 0_u64;
    size = size.checked_add(REPOSITORY_PREFIX.len() as u64)?;
    size = size.checked_add(budget.label_bytes)?;
    size = size.checked_add(NEWLINE.len() as u64)?;
    if let Some(rev) = budget.revision_bytes {
        size = size.checked_add(REVISION_PREFIX.len() as u64)?;
        size = size.checked_add(rev)?;
        size = size.checked_add(NEWLINE.len() as u64)?;
    }
    size = size.checked_add(FILES_PREFIX.len() as u64)?;
    size = size.checked_add(budget.file_count.to_string().len() as u64)?;
    size = size.checked_add(SKIPPED_PREFIX.len() as u64)?;
    size = size.checked_add(budget.skipped.to_string().len() as u64)?;
    size = size.checked_add(METADATA_SUFFIX.len() as u64)?;
    size = size.checked_add(DIRECTORY_HEADER.len() as u64)?;
    size = size.checked_add(FILES_HEADER.len() as u64)?;
    let file_block_bytes = FILE_BLOCK_PREFIX.len() + FILE_BLOCK_SUFFIX.len();
    size = size.checked_add(count.checked_mul(file_block_bytes as u64)?)?;
    size = size.checked_add(budget.path_bytes)?;
    size = size.checked_add(budget.content_bytes)?;
    size.checked_add(budget.trailing_bytes)
}

pub(crate) fn render<W: Write + ?Sized>(
    writer: &mut W,
    label: &str,
    revision: Option<&str>,
    files: &[FileRecord],
    skipped: usize,
    trie: &BudgetTrie,
) -> Result<(), IngestError> {
    writer.write_all(REPOSITORY_PREFIX.as_bytes())?;
    writer.write_all(escape_header(label).as_bytes())?;
    writer.write_all(NEWLINE.as_bytes())?;
    if let Some(revision) = revision {
        writer.write_all(REVISION_PREFIX.as_bytes())?;
        writer.write_all(escape_header(revision).as_bytes())?;
        writer.write_all(NEWLINE.as_bytes())?;
    }
    writer.write_all(FILES_PREFIX.as_bytes())?;
    writer.write_all(files.len().to_string().as_bytes())?;
    writer.write_all(SKIPPED_PREFIX.as_bytes())?;
    writer.write_all(skipped.to_string().as_bytes())?;
    writer.write_all(METADATA_SUFFIX.as_bytes())?;

    let mut ordered_files: Vec<_> = files
        .iter()
        .map(|file| (escape_header(&file.path), file))
        .collect();
    ordered_files.sort_by(|left, right| left.1.path.cmp(&right.1.path));

    writer.write_all(DIRECTORY_HEADER.as_bytes())?;
    trie.render_to_writer(writer)?;
    writer.write_all(FILES_HEADER.as_bytes())?;
    for (escaped_path, file) in ordered_files {
        writer.write_all(FILE_BLOCK_PREFIX.as_bytes())?;
        writer.write_all(escaped_path.as_bytes())?;
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
mod tests {
    use super::*;

    #[test]
    fn escape_header_borrows_clean_input() {
        use std::borrow::Cow;
        assert!(matches!(
            escape_header("clean/path/file.rs"),
            Cow::Borrowed(_)
        ));
        assert!(matches!(
            escape_header("path\\with\\backslash"),
            Cow::Owned(_)
        ));
        assert!(matches!(
            escape_header("path\nwith\nnewline"),
            Cow::Owned(_)
        ));
    }
}
