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

#[inline]
fn disallowed_control_byte(b: u8) -> bool {
    matches!(b, 1..=8 | 0x0B | 0x0C | 0x0E..=0x1F)
}

#[inline]
fn classify_xml_byte(bytes: &[u8], idx: usize) -> Option<(&'static str, usize)> {
    let b = bytes[idx];
    match b {
        b'&' => Some(("&amp;", 1)),
        b'<' => Some(("&lt;", 1)),
        b'>' => Some(("&gt;", 1)),
        b'"' => Some(("&quot;", 1)),
        b'\'' => Some(("&apos;", 1)),
        b if disallowed_control_byte(b) => Some(("\u{FFFD}", 1)),
        0xEF if idx + 2 < bytes.len() => {
            // \u{FFFE} is [0xEF, 0xBF, 0xBE], \u{FFFF} is [0xEF, 0xBF, 0xBF]
            if bytes[idx + 1] == 0xBF && (bytes[idx + 2] == 0xBE || bytes[idx + 2] == 0xBF) {
                Some(("\u{FFFD}", 3))
            } else {
                None
            }
        }
        _ => None,
    }
}

enum XmlChunk<'a> {
    Raw(&'a str),
    Escaped(&'static str),
}

struct XmlScanner<'a> {
    value: &'a str,
    bytes: &'a [u8],
    idx: usize,
}

impl<'a> XmlScanner<'a> {
    fn new(value: &'a str) -> Self {
        Self {
            value,
            bytes: value.as_bytes(),
            idx: 0,
        }
    }
}

impl<'a> Iterator for XmlScanner<'a> {
    type Item = XmlChunk<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.idx >= self.bytes.len() {
            return None;
        }

        if let Some((rep, len)) = classify_xml_byte(self.bytes, self.idx) {
            self.idx += len;
            return Some(XmlChunk::Escaped(rep));
        }

        let start = self.idx;
        while self.idx < self.bytes.len() {
            if classify_xml_byte(self.bytes, self.idx).is_some() {
                break;
            }
            if self.bytes[self.idx] < 0x80 {
                self.idx += 1;
            } else {
                let rest = &self.value[self.idx..];
                if let Some(c) = rest.chars().next() {
                    self.idx += c.len_utf8();
                } else {
                    self.idx += 1;
                }
            }
        }
        Some(XmlChunk::Raw(&self.value[start..self.idx]))
    }
}

pub(crate) fn xml_escape(value: &str) -> Cow<'_, str> {
    let bytes = value.as_bytes();
    let has_escapes = (0..bytes.len()).any(|i| classify_xml_byte(bytes, i).is_some());
    if !has_escapes {
        return Cow::Borrowed(value);
    }

    let mut result = String::with_capacity(value.len() + 16);
    for chunk in XmlScanner::new(value) {
        match chunk {
            XmlChunk::Raw(s) => result.push_str(s),
            XmlChunk::Escaped(rep) => result.push_str(rep),
        }
    }
    Cow::Owned(result)
}

pub(crate) fn escaped_metrics(value: &str) -> (u64, bool) {
    let mut len: u64 = 0;
    let mut last_char = None;

    for chunk in XmlScanner::new(value) {
        match chunk {
            XmlChunk::Raw(s) => {
                len += s.len() as u64;
                last_char = s.chars().last();
            }
            XmlChunk::Escaped(rep) => {
                len += rep.len() as u64;
                last_char = rep.chars().last();
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
    let mut last_char = None;

    for chunk in XmlScanner::new(value) {
        match chunk {
            XmlChunk::Raw(s) => {
                writer.write_all(s.as_bytes())?;
                last_char = s.chars().last();
            }
            XmlChunk::Escaped(rep) => {
                writer.write_all(rep.as_bytes())?;
                last_char = rep.chars().last();
            }
        }
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
