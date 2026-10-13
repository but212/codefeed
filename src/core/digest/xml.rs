//! XML structured format specification and escaping.

use std::borrow::Cow;
use std::io::Write;

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
    ordered_files.sort_unstable_by(|left, right| left.path.cmp(&right.path));
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
    fn xml_escape_and_streaming_consistency() {
        let cases: &[(&str, &str)] = &[
            ("", ""),
            ("plain text", "plain text"),
            ("plain text\n", "plain text\n"),
            ("a & b < c > d", "a &amp; b &lt; c &gt; d"),
            ("let x = a && b;", "let x = a &amp;&amp; b;"),
            (
                "<tag attr=\"val\">'hello'</tag>",
                "&lt;tag attr=&quot;val&quot;&gt;&apos;hello&apos;&lt;/tag&gt;",
            ),
            ("hello\x0Cworld", "hello\u{FFFD}world"),
            ("valid\t\n\rwhitespace", "valid\t\n\rwhitespace"),
            (
                "code\u{0001}\u{0008}\u{000B}\u{001F}",
                "code\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}",
            ),
            (
                "한글과 이모지 🦀 & 특수문자\n",
                "한글과 이모지 🦀 &amp; 특수문자\n",
            ),
            ("끝이 개행이 아닌 &amp", "끝이 개행이 아닌 &amp;amp"),
        ];

        for &(text, expected_escaped) in cases {
            let escaped = xml_escape(text);
            assert_eq!(
                escaped.as_ref(),
                expected_escaped,
                "escape mismatch for {text:?}"
            );

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
}
