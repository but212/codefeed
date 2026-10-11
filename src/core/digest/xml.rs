//! XML structured format specification and escaping.

use std::borrow::Cow;
use std::io::Write;

use crate::core::error::IngestError;
use crate::core::model::FileRecord;
use super::byte_count;
use super::format::FormatSpec;
use super::trie::BudgetTrie;
use super::DigestBudget;

const XML_REPO_START: &str = "<repository name=\"";
const XML_REVISION_ATTR: &str = "\" revision=\"";
const XML_TAG_CLOSE: &str = "\">\n";
const XML_DIR_START: &str = "<directory_structure>\n";
const XML_DIR_END: &str = "</directory_structure>\n";
const XML_FILES_START: &str = "<files>\n";
const XML_FILES_END: &str = "</files>\n";
const XML_REPO_END: &str = "</repository>\n";
const XML_FILE_START: &str = "<file path=\"";
const XML_FILE_END: &str = "</file>\n";
const NEWLINE: &str = "\n";

pub(crate) fn is_disallowed_xml_char(c: char) -> bool {
    matches!(
        c,
        '\u{1}'..='\u{8}' | '\u{B}' | '\u{C}' | '\u{E}'..='\u{1F}' | '\u{FFFE}' | '\u{FFFF}'
    )
}

pub(crate) fn xml_escape(value: &str) -> Cow<'_, str> {
    let needs_escape = value.chars().any(|c| {
        matches!(c, '&' | '<' | '>' | '"' | '\'') || is_disallowed_xml_char(c)
    });
    if !needs_escape {
        return Cow::Borrowed(value);
    }

    let mut result = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => result.push_str("&amp;"),
            '<' => result.push_str("&lt;"),
            '>' => result.push_str("&gt;"),
            '"' => result.push_str("&quot;"),
            '\'' => result.push_str("&apos;"),
            c if is_disallowed_xml_char(c) => result.push('\u{FFFD}'),
            c => result.push(c),
        }
    }
    Cow::Owned(result)
}

pub(crate) struct XmlFormat;

impl FormatSpec for XmlFormat {
    fn escape_label_or_revision<'a>(&self, text: &'a str) -> Cow<'a, str> {
        xml_escape(text)
    }

    fn escape_tree_xml(&self) -> bool {
        true
    }

    fn calculate_file_bytes(&self, file: &FileRecord) -> Result<(u64, u64, u64), IngestError> {
        let path_b = byte_count(&xml_escape(&file.path))?;
        let content_escaped = xml_escape(&file.content);
        let content_b = byte_count(&content_escaped)?;
        let trailing_b = usize::from(!content_escaped.ends_with('\n')) as u64;
        Ok((path_b, content_b, trailing_b))
    }

    fn rendered_base_size(&self, budget: &DigestBudget) -> Option<u64> {
        let count = u64::try_from(budget.file_count).ok()?;
        let mut size = 0_u64;
        size = size.checked_add(XML_REPO_START.len() as u64)?;
        size = size.checked_add(budget.label_bytes)?;
        if let Some(rev) = budget.revision_bytes {
            size = size.checked_add(XML_REVISION_ATTR.len() as u64)?;
            size = size.checked_add(rev)?;
        }
        size = size.checked_add(XML_TAG_CLOSE.len() as u64)?;
        size = size.checked_add(XML_DIR_START.len() as u64)?;
        size = size.checked_add(XML_DIR_END.len() as u64)?;
        size = size.checked_add(XML_FILES_START.len() as u64)?;
        let per_file_tag_bytes = XML_FILE_START.len() + XML_TAG_CLOSE.len() + XML_FILE_END.len();
        size = size.checked_add(count.checked_mul(per_file_tag_bytes as u64)?)?;
        size = size.checked_add(budget.path_bytes)?;
        size = size.checked_add(budget.content_bytes)?;
        size = size.checked_add(budget.trailing_bytes)?;
        size = size.checked_add(XML_FILES_END.len() as u64)?;
        size.checked_add(XML_REPO_END.len() as u64)
    }

    fn render(
        &self,
        writer: &mut dyn Write,
        label: &str,
        revision: Option<&str>,
        files: &[FileRecord],
        _skipped: usize,
        trie: &BudgetTrie,
    ) -> Result<(), IngestError> {
        writer.write_all(XML_REPO_START.as_bytes())?;
        writer.write_all(xml_escape(label).as_bytes())?;
        if let Some(revision) = revision {
            writer.write_all(XML_REVISION_ATTR.as_bytes())?;
            writer.write_all(xml_escape(revision).as_bytes())?;
        }
        writer.write_all(XML_TAG_CLOSE.as_bytes())?;
        writer.write_all(XML_DIR_START.as_bytes())?;
        trie.render_to_writer(writer)?;
        writer.write_all(XML_DIR_END.as_bytes())?;
        writer.write_all(XML_FILES_START.as_bytes())?;
        let mut ordered_files: Vec<_> = files.iter().collect();
        ordered_files.sort_by(|left, right| left.path.cmp(&right.path));
        for file in ordered_files {
            writer.write_all(XML_FILE_START.as_bytes())?;
            writer.write_all(xml_escape(&file.path).as_bytes())?;
            writer.write_all(XML_TAG_CLOSE.as_bytes())?;
            let escaped_content = xml_escape(&file.content);
            writer.write_all(escaped_content.as_bytes())?;
            if !escaped_content.ends_with('\n') {
                writer.write_all(NEWLINE.as_bytes())?;
            }
            writer.write_all(XML_FILE_END.as_bytes())?;
        }
        writer.write_all(XML_FILES_END.as_bytes())?;
        writer.write_all(XML_REPO_END.as_bytes())?;
        Ok(())
    }
}
