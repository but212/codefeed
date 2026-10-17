//! Text format specification, escaping, and layout rendering.

use std::borrow::Cow;
use std::io::Write;

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

#[inline(always)]
const fn needs_header_escape(b: u8) -> bool {
    matches!(b, b'\\' | b'\r' | b'\n')
}

#[inline(always)]
const fn header_escape_str(b: u8) -> &'static str {
    match b {
        b'\\' => "\\\\",
        b'\r' => "\\r",
        b'\n' => "\\n",
        _ => "",
    }
}

#[inline(always)]
fn first_header_escape_index(bytes: &[u8]) -> Option<usize> {
    bytes.iter().position(|&b| needs_header_escape(b))
}

/// Escapes backslashes, carriage returns, and newlines in metadata header strings.
pub(crate) fn escape_header(value: &str) -> Cow<'_, str> {
    let bytes = value.as_bytes();
    let Some(first) = first_header_escape_index(bytes) else {
        return Cow::Borrowed(value);
    };

    let mut out = String::with_capacity(value.len() + 8);
    out.push_str(&value[..first]);
    let mut last = first;
    for (i, &b) in bytes[first..].iter().enumerate() {
        let abs_i = first + i;
        if needs_header_escape(b) {
            out.push_str(&value[last..abs_i]);
            out.push_str(header_escape_str(b));
            last = abs_i + 1;
        }
    }
    out.push_str(&value[last..]);
    Cow::Owned(out)
}

pub(crate) fn render_text<'a, W, I>(
    writer: &mut W,
    label: &str,
    revision: Option<&str>,
    ordered_files: I,
    skipped: usize,
    paths: &[&str],
) -> Result<(), IngestError>
where
    W: Write + ?Sized,
    I: IntoIterator<Item = FileRecord<'a>>,
    I::IntoIter: ExactSizeIterator,
{
    writer.write_all(REPOSITORY_PREFIX.as_bytes())?;
    writer.write_all(escape_header(label).as_bytes())?;
    writer.write_all(NEWLINE.as_bytes())?;
    if let Some(revision) = revision {
        writer.write_all(REVISION_PREFIX.as_bytes())?;
        writer.write_all(escape_header(revision).as_bytes())?;
        writer.write_all(NEWLINE.as_bytes())?;
    }
    let iter = ordered_files.into_iter();
    writer.write_all(FILES_PREFIX.as_bytes())?;
    write!(writer, "{}", iter.len())?;
    writer.write_all(SKIPPED_PREFIX.as_bytes())?;
    write!(writer, "{}", skipped)?;
    writer.write_all(METADATA_SUFFIX.as_bytes())?;
    writer.write_all(DIRECTORY_HEADER.as_bytes())?;
    crate::core::tree::render_tree(writer, paths, escape_header)?;
    writer.write_all(FILES_HEADER.as_bytes())?;
    for file in iter {
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

    #[test]
    fn test_escape_header_boundary_alignment() {
        for len in 0..=48 {
            let clean = "a".repeat(len);
            assert_eq!(first_header_escape_index(clean.as_bytes()), None);
            assert!(matches!(escape_header(&clean), Cow::Borrowed(_)));

            for special in ['\\', '\r', '\n'] {
                for pos in 0..len {
                    let mut text = clean.clone();
                    text.replace_range(pos..pos + 1, &special.to_string());
                    assert_eq!(
                        first_header_escape_index(text.as_bytes()),
                        Some(pos),
                        "mismatch at len {len}, pos {pos}, special {special:?}"
                    );
                    assert!(matches!(escape_header(&text), Cow::Owned(_)));
                }
            }
        }
    }
}
