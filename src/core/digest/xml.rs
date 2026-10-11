//! XML structured format specification and escaping.

use std::borrow::Cow;
use std::io::Write;

use super::DigestBudget;
use super::byte_count;
use super::trie::BudgetTrie;
use crate::core::error::IngestError;
use crate::core::model::FileRecord;

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
    let needs_escape = value
        .chars()
        .any(|c| matches!(c, '&' | '<' | '>' | '"' | '\'') || is_disallowed_xml_char(c));
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

pub(crate) fn escaped_metrics(value: &str) -> (u64, bool) {
    let mut len: u64 = 0;
    let mut last_char = None;
    for c in value.chars() {
        match c {
            '&' => {
                len += 5;
                last_char = Some(';');
            }
            '<' => {
                len += 4;
                last_char = Some(';');
            }
            '>' => {
                len += 4;
                last_char = Some(';');
            }
            '"' => {
                len += 6;
                last_char = Some(';');
            }
            '\'' => {
                len += 6;
                last_char = Some(';');
            }
            c if is_disallowed_xml_char(c) => {
                len += '\u{FFFD}'.len_utf8() as u64;
                last_char = Some('\u{FFFD}');
            }
            c => {
                len += c.len_utf8() as u64;
                last_char = Some(c);
            }
        }
    }
    let ends_with_nl = last_char == Some('\n');
    (len, ends_with_nl)
}

pub(crate) fn write_xml_escaped<W: Write + ?Sized>(
    writer: &mut W,
    value: &str,
) -> std::io::Result<bool> {
    let mut last_idx = 0;
    let mut last_char = None;

    let bytes = value.as_bytes();
    for (idx, c) in value.char_indices() {
        let replacement = match c {
            '&' => Some("&amp;"),
            '<' => Some("&lt;"),
            '>' => Some("&gt;"),
            '"' => Some("&quot;"),
            '\'' => Some("&apos;"),
            c if is_disallowed_xml_char(c) => Some("\u{FFFD}"),
            _ => None,
        };

        if let Some(rep) = replacement {
            if idx > last_idx {
                writer.write_all(&bytes[last_idx..idx])?;
            }
            writer.write_all(rep.as_bytes())?;
            last_idx = idx + c.len_utf8();
            last_char = rep.chars().last();
        } else {
            last_char = Some(c);
        }
    }

    if last_idx < value.len() {
        writer.write_all(&bytes[last_idx..])?;
    }

    Ok(last_char == Some('\n'))
}

pub(crate) fn calculate_file_bytes(file: &FileRecord) -> Result<(u64, u64, u64), IngestError> {
    let path_b = byte_count(&xml_escape(&file.path))?;
    let (content_b, ends_with_nl) = escaped_metrics(&file.content);
    let trailing_b = usize::from(!ends_with_nl) as u64;
    Ok((path_b, content_b, trailing_b))
}

pub(crate) fn rendered_base_size(budget: &DigestBudget) -> Option<u64> {
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

pub(crate) fn render<W: Write + ?Sized>(
    writer: &mut W,
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
        let ends_with_nl = write_xml_escaped(writer, &file.content)?;
        if !ends_with_nl {
            writer.write_all(NEWLINE.as_bytes())?;
        }
        writer.write_all(XML_FILE_END.as_bytes())?;
    }
    writer.write_all(XML_FILES_END.as_bytes())?;
    writer.write_all(XML_REPO_END.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_xml_entities() {
        assert_eq!(xml_escape("plain text"), "plain text");
        assert_eq!(xml_escape("a & b < c > d"), "a &amp; b &lt; c &gt; d");
        assert_eq!(xml_escape("let x = a && b;"), "let x = a &amp;&amp; b;");
        assert_eq!(
            xml_escape("<tag attr=\"val\">'hello'</tag>"),
            "&lt;tag attr=&quot;val&quot;&gt;&apos;hello&apos;&lt;/tag&gt;"
        );
    }

    #[test]
    fn sanitizes_xml_control_characters() {
        assert_eq!(xml_escape("hello\x0Cworld"), "hello\u{FFFD}world");
        assert_eq!(xml_escape("valid\t\n\rwhitespace"), "valid\t\n\rwhitespace");
        assert_eq!(
            xml_escape("code\u{0001}\u{0008}\u{000B}\u{001F}"),
            "code\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}"
        );
    }

    #[test]
    fn escaped_metrics_and_streaming_match_xml_escape() {
        let cases = [
            "",
            "plain text",
            "plain text\n",
            "a & b < c > d",
            "<tag attr=\"val\">'hello'</tag>\n",
            "hello\x0Cworld",
            "code\u{0001}\u{0008}\u{000B}\u{001F}\n",
            "한글과 이모지 🦀 & 특수문자\n",
            "끝이 개행이 아닌 &amp",
        ];

        for text in cases {
            let escaped = xml_escape(text);
            let (byte_count, ends_with_nl) = escaped_metrics(text);
            assert_eq!(
                byte_count,
                escaped.len() as u64,
                "length mismatch for {text:?}"
            );
            assert_eq!(
                ends_with_nl,
                escaped.ends_with('\n'),
                "ends_with_nl mismatch for {text:?}"
            );

            let mut streamed = Vec::new();
            let stream_ends_with_nl = write_xml_escaped(&mut streamed, text).unwrap();
            assert_eq!(
                String::from_utf8(streamed).unwrap(),
                escaped.as_ref(),
                "streamed content mismatch for {text:?}"
            );
            assert_eq!(stream_ends_with_nl, ends_with_nl);
        }
    }
}
