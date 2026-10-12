//! Plain text format specification and escaping.

use std::borrow::Cow;
use std::io::Write;

use super::FormatFrameMetrics;
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

const TEXT_FILE_BLOCK_BYTES: u64 = (FILE_BLOCK_PREFIX.len() + FILE_BLOCK_SUFFIX.len()) as u64;

const TEXT_FIXED_FRAME_BYTES: u64 = (REPOSITORY_PREFIX.len()
    + NEWLINE.len()
    + FILES_PREFIX.len()
    + SKIPPED_PREFIX.len()
    + METADATA_SUFFIX.len()
    + DIRECTORY_HEADER.len()
    + FILES_HEADER.len()) as u64;

const TEXT_REVISION_OVERHEAD_BYTES: u64 = (REVISION_PREFIX.len() + NEWLINE.len()) as u64;

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

pub(crate) const FRAME_METRICS: FormatFrameMetrics = FormatFrameMetrics {
    fixed_frame: TEXT_FIXED_FRAME_BYTES,
    revision_overhead: TEXT_REVISION_OVERHEAD_BYTES,
    per_file: TEXT_FILE_BLOCK_BYTES,
};

pub(crate) const fn num_digits(mut n: usize) -> u64 {
    let mut digits = 1;
    while n >= 10 {
        digits += 1;
        n /= 10;
    }
    digits
}

pub(crate) const fn extra_metadata_bytes(file_count: usize, skipped: usize) -> Option<u64> {
    let file_digits = num_digits(file_count);
    let skipped_digits = num_digits(skipped);
    file_digits.checked_add(skipped_digits)
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
    let mut ordered_files: Vec<&FileRecord> = files.into_iter().collect();
    ordered_files.sort_by(|left, right| left.path.cmp(&right.path));

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

    #[test]
    fn text_static_frame_constants_match_individual_prefixes() {
        let expected_file_block = (FILE_BLOCK_PREFIX.len() + FILE_BLOCK_SUFFIX.len()) as u64;
        assert_eq!(TEXT_FILE_BLOCK_BYTES, expected_file_block);

        let expected_fixed_frame = (REPOSITORY_PREFIX.len()
            + NEWLINE.len()
            + FILES_PREFIX.len()
            + SKIPPED_PREFIX.len()
            + METADATA_SUFFIX.len()
            + DIRECTORY_HEADER.len()
            + FILES_HEADER.len()) as u64;
        assert_eq!(TEXT_FIXED_FRAME_BYTES, expected_fixed_frame);

        let expected_rev_overhead = (REVISION_PREFIX.len() + NEWLINE.len()) as u64;
        assert_eq!(TEXT_REVISION_OVERHEAD_BYTES, expected_rev_overhead);
    }

    #[test]
    fn num_digits_matches_to_string_and_evaluates_in_const() {
        const {
            assert!(matches!(extra_metadata_bytes(100, 5), Some(4)));
        }

        for n in [0, 1, 9, 10, 99, 100, 999, 1000, 1234567, usize::MAX] {
            assert_eq!(
                num_digits(n),
                n.to_string().len() as u64,
                "digit count mismatch for {n}"
            );
        }
    }
}
