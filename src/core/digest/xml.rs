//! XML structured format specification and escaping.

use std::borrow::Cow;
use std::io::Write;

use super::FormatFrameMetrics;
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

const XML_PER_FILE_TAG_BYTES: u64 =
    (XML_FILE_START.len() + XML_TAG_CLOSE.len() + XML_FILE_END.len()) as u64;

const XML_FIXED_FRAME_BYTES: u64 = (XML_REPO_START.len()
    + XML_TAG_CLOSE.len()
    + XML_DIR_START.len()
    + XML_DIR_END.len()
    + XML_FILES_START.len()
    + XML_FILES_END.len()
    + XML_REPO_END.len()) as u64;

const XML_REVISION_ATTR_BYTES: u64 = XML_REVISION_ATTR.len() as u64;

const fn make_escape_ascii() -> [Option<&'static str>; 128] {
    let mut table = [None; 128];
    table[b'&' as usize] = Some("&amp;");
    table[b'<' as usize] = Some("&lt;");
    table[b'>' as usize] = Some("&gt;");
    table[b'"' as usize] = Some("&quot;");
    table[b'\'' as usize] = Some("&apos;");
    let mut i = 1;
    while i <= 8 {
        table[i] = Some("\u{FFFD}");
        i += 1;
    }
    table[0x0B] = Some("\u{FFFD}");
    table[0x0C] = Some("\u{FFFD}");
    let mut j = 0x0E;
    while j <= 0x1F {
        table[j] = Some("\u{FFFD}");
        j += 1;
    }
    table
}

const ESCAPE_ASCII: [Option<&'static str>; 128] = make_escape_ascii();

#[inline(always)]
fn xml_special(bytes: &[u8], i: usize) -> Option<(usize, &'static str)> {
    let b = bytes[i];
    if b < 128 {
        ESCAPE_ASCII[b as usize].map(|esc| (1, esc))
    } else if b == 0xEF
        && i + 2 < bytes.len()
        && bytes[i + 1] == 0xBF
        && (bytes[i + 2] == 0xBE || bytes[i + 2] == 0xBF)
    {
        Some((3, "\u{FFFD}"))
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XmlChunk<'a> {
    Raw(&'a str),
    Escaped(&'static str),
}

impl<'a> XmlChunk<'a> {
    #[inline]
    pub fn as_str(&self) -> &'a str {
        match *self {
            XmlChunk::Raw(s) => s,
            XmlChunk::Escaped(s) => s,
        }
    }

    #[inline]
    pub fn as_bytes(&self) -> &'a [u8] {
        self.as_str().as_bytes()
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.as_str().len()
    }

    #[inline]
    pub fn ends_with_nl(&self) -> bool {
        match *self {
            XmlChunk::Raw(s) => s.as_bytes().last() == Some(&b'\n'),
            XmlChunk::Escaped(s) => s.ends_with('\n'),
        }
    }
}

pub struct XmlChunks<'a> {
    value: &'a str,
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> XmlChunks<'a> {
    #[inline]
    pub fn new(value: &'a str) -> Self {
        Self {
            value,
            bytes: value.as_bytes(),
            cursor: 0,
        }
    }
}

impl<'a> Iterator for XmlChunks<'a> {
    type Item = XmlChunk<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor >= self.bytes.len() {
            return None;
        }

        let start = self.cursor;
        while self.cursor < self.bytes.len() {
            if let Some((consumed, esc)) = xml_special(self.bytes, self.cursor) {
                if self.cursor > start {
                    let raw = &self.value[start..self.cursor];
                    return Some(XmlChunk::Raw(raw));
                }
                self.cursor += consumed;
                return Some(XmlChunk::Escaped(esc));
            }
            self.cursor += 1;
        }

        Some(XmlChunk::Raw(&self.value[start..]))
    }
}

pub fn xml_escape(value: &str) -> Cow<'_, str> {
    let bytes = value.as_bytes();
    let needs_escape = (0..bytes.len()).any(|i| xml_special(bytes, i).is_some());

    if !needs_escape {
        return Cow::Borrowed(value);
    }

    let mut result = String::with_capacity(value.len() + 16);
    for chunk in XmlChunks::new(value) {
        result.push_str(chunk.as_str());
    }
    Cow::Owned(result)
}

pub fn escaped_metrics(value: &str) -> (u64, bool) {
    let mut len: u64 = 0;
    let mut ends_with_nl = false;

    for chunk in XmlChunks::new(value) {
        len += chunk.len() as u64;
        ends_with_nl = chunk.ends_with_nl();
    }

    (len, ends_with_nl)
}

pub fn write_xml_escaped<W: Write + ?Sized>(writer: &mut W, value: &str) -> std::io::Result<bool> {
    let mut ends_with_nl = false;

    for chunk in XmlChunks::new(value) {
        writer.write_all(chunk.as_bytes())?;
        ends_with_nl = chunk.ends_with_nl();
    }

    Ok(ends_with_nl)
}

pub(crate) fn calculate_file_bytes(file: &FileRecord) -> Result<(u64, u64, u64), IngestError> {
    let path_b = byte_count(&xml_escape(&file.path))?;
    let (content_b, ends_with_nl) = escaped_metrics(&file.content);
    let trailing_b = usize::from(!ends_with_nl) as u64;
    Ok((path_b, content_b, trailing_b))
}

pub(crate) fn frame_metrics() -> FormatFrameMetrics {
    FormatFrameMetrics {
        fixed_frame: XML_FIXED_FRAME_BYTES,
        revision_overhead: XML_REVISION_ATTR_BYTES,
        per_file: XML_PER_FILE_TAG_BYTES,
    }
}

pub(crate) fn render<'a, W, I>(
    writer: &mut W,
    label: &str,
    revision: Option<&str>,
    files: I,
    trie: &BudgetTrie,
) -> Result<(), IngestError>
where
    W: Write + ?Sized,
    I: IntoIterator<Item = &'a FileRecord>,
{
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
    let mut ordered_files: Vec<_> = files.into_iter().collect();
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

    #[test]
    fn xml_chunks_iterator_yields_expected_sequence() {
        let input = "hello <world> & 'test'";
        let chunks: Vec<_> = XmlChunks::new(input).collect();
        assert_eq!(
            chunks,
            vec![
                XmlChunk::Raw("hello "),
                XmlChunk::Escaped("&lt;"),
                XmlChunk::Raw("world"),
                XmlChunk::Escaped("&gt;"),
                XmlChunk::Raw(" "),
                XmlChunk::Escaped("&amp;"),
                XmlChunk::Raw(" "),
                XmlChunk::Escaped("&apos;"),
                XmlChunk::Raw("test"),
                XmlChunk::Escaped("&apos;"),
            ]
        );
    }

    #[test]
    fn xml_static_frame_constants_match_individual_tags() {
        let expected_per_file =
            (XML_FILE_START.len() + XML_TAG_CLOSE.len() + XML_FILE_END.len()) as u64;
        assert_eq!(XML_PER_FILE_TAG_BYTES, expected_per_file);

        let expected_fixed_frame = (XML_REPO_START.len()
            + XML_TAG_CLOSE.len()
            + XML_DIR_START.len()
            + XML_DIR_END.len()
            + XML_FILES_START.len()
            + XML_FILES_END.len()
            + XML_REPO_END.len()) as u64;
        assert_eq!(XML_FIXED_FRAME_BYTES, expected_fixed_frame);
        assert_eq!(XML_REVISION_ATTR_BYTES, XML_REVISION_ATTR.len() as u64);
    }
}
