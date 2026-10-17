//! XML format specification, entity escaping, and layout rendering.

use std::borrow::Cow;
use std::io::Write;

use super::text::escape_header;
use crate::core::error::IngestError;
use crate::core::model::FileRecord;
use crate::core::tree::BudgetTrie;

const XML_REPO_START: &str = "<repository name=\"";
const XML_REVISION_ATTR: &str = "\" revision=\"";
const XML_TAG_CLOSE: &str = "\">\n";
const XML_DIR_START: &str = "<directory_structure>\n";
const XML_DIR_END: &str = "</directory_structure>\n";
const XML_FILES_START: &str = "<files>\n";
const XML_FILES_END: &str = "</files>\n";
const XML_REPO_END: &str = "</repository>\n";
const XML_FILE_START: &str = "<file path=\"";
const FILE_TAILS: [&[u8]; 2] = [b"</file>\n", b"\n</file>\n"];

const SPECIAL_BITSET: [u64; 4] = {
    let mut mask = [0u64; 4];
    let mut i = 0;
    while i <= 8 {
        mask[0] |= 1 << i;
        i += 1;
    }
    mask[0] |= (1 << 0x0B) | (1 << 0x0C);
    let mut j = 0x0E;
    while j <= 0x1F {
        mask[0] |= 1 << j;
        j += 1;
    }
    mask[b'&' as usize / 64] |= 1 << (b'&' as usize % 64);
    mask[b'<' as usize / 64] |= 1 << (b'<' as usize % 64);
    mask[b'>' as usize / 64] |= 1 << (b'>' as usize % 64);
    mask[b'"' as usize / 64] |= 1 << (b'"' as usize % 64);
    mask[b'\'' as usize / 64] |= 1 << (b'\'' as usize % 64);
    mask[0xEF / 64] |= 1 << (0xEF % 64);
    mask
};

#[inline(always)]
const fn is_special_byte(b: u8) -> bool {
    let word = SPECIAL_BITSET[(b >> 6) as usize];
    (word & (1u64 << (b & 63))) != 0
}

const XML_ESCAPES: [&str; 6] = ["\u{FFFD}", "&quot;", "&amp;", "&apos;", "&lt;", "&gt;"];
const ASCII_ESC_IDX: [u8; 64] = {
    let mut t = [0u8; 64];
    t[b'"' as usize] = 1;
    t[b'&' as usize] = 2;
    t[b'\'' as usize] = 3;
    t[b'<' as usize] = 4;
    t[b'>' as usize] = 5;
    t
};

#[inline(always)]
fn xml_special(bytes: &[u8], i: usize) -> Option<(usize, &'static str)> {
    let b = bytes[i];
    if b < 64 {
        return Some((1, XML_ESCAPES[ASCII_ESC_IDX[b as usize] as usize]));
    }
    if b == 0xEF && i + 2 < bytes.len() {
        let tail = ((bytes[i + 1] as u32) << 8) | (bytes[i + 2] as u32);
        return ((tail & !1) == 0xBFBE).then_some((3, "\u{FFFD}"));
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XmlChunk<'a> {
    Raw(&'a str),
    Escaped(&'static str),
}

impl<'a> XmlChunk<'a> {
    #[inline]
    pub const fn as_str(&self) -> &'a str {
        match *self {
            XmlChunk::Raw(s) => s,
            XmlChunk::Escaped(s) => s,
        }
    }

    #[inline]
    pub const fn as_bytes(&self) -> &'a [u8] {
        self.as_str().as_bytes()
    }
}

pub struct XmlChunks<'a> {
    value: &'a str,
    bytes: &'a [u8],
    cursor: usize,
    pending: Option<&'static str>,
}

impl<'a> XmlChunks<'a> {
    #[inline]
    pub const fn from_offset(value: &'a str, offset: usize) -> Self {
        Self {
            value,
            bytes: value.as_bytes(),
            cursor: offset,
            pending: None,
        }
    }
}

impl<'a> Iterator for XmlChunks<'a> {
    type Item = XmlChunk<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(esc) = self.pending.take() {
            return Some(XmlChunk::Escaped(esc));
        }

        if self.cursor >= self.bytes.len() {
            return None;
        }

        let start = self.cursor;
        while self.cursor < self.bytes.len() {
            if is_special_byte(self.bytes[self.cursor])
                && let Some((consumed, esc)) = xml_special(self.bytes, self.cursor)
            {
                if self.cursor > start {
                    let raw = &self.value[start..self.cursor];
                    self.cursor += consumed;
                    self.pending = Some(esc);
                    return Some(XmlChunk::Raw(raw));
                }
                self.cursor += consumed;
                return Some(XmlChunk::Escaped(esc));
            }
            self.cursor += 1;
        }

        if self.cursor > start {
            Some(XmlChunk::Raw(&self.value[start..]))
        } else {
            None
        }
    }
}

#[inline(always)]
pub(crate) fn first_special_index(bytes: &[u8]) -> Option<usize> {
    bytes
        .iter()
        .enumerate()
        .position(|(i, &b)| is_special_byte(b) && (b != 0xEF || xml_special(bytes, i).is_some()))
}

pub fn xml_escape(value: &str) -> Cow<'_, str> {
    let bytes = value.as_bytes();
    let Some(first) = first_special_index(bytes) else {
        return Cow::Borrowed(value);
    };

    let mut result = String::with_capacity(value.len() + 16);
    result.push_str(&value[..first]);
    for chunk in XmlChunks::from_offset(value, first) {
        result.push_str(chunk.as_str());
    }
    Cow::Owned(result)
}

pub fn write_escaped<W: Write + ?Sized>(writer: &mut W, value: &str) -> std::io::Result<()> {
    let bytes = value.as_bytes();
    let Some(first) = first_special_index(bytes) else {
        return writer.write_all(bytes);
    };

    if first > 0 {
        writer.write_all(&bytes[..first])?;
    }
    for chunk in XmlChunks::from_offset(value, first) {
        writer.write_all(chunk.as_bytes())?;
    }
    Ok(())
}

pub(crate) fn render_xml<'a, W, I>(
    writer: &mut W,
    label: &str,
    revision: Option<&str>,
    ordered_files: I,
    trie: &BudgetTrie,
) -> Result<(), IngestError>
where
    W: Write + ?Sized,
    I: IntoIterator<Item = FileRecord<'a>>,
{
    writer.write_all(XML_REPO_START.as_bytes())?;
    writer.write_all(xml_escape(label).as_bytes())?;
    if let Some(revision) = revision {
        writer.write_all(XML_REVISION_ATTR.as_bytes())?;
        writer.write_all(xml_escape(revision).as_bytes())?;
    }
    writer.write_all(XML_TAG_CLOSE.as_bytes())?;
    writer.write_all(XML_DIR_START.as_bytes())?;
    trie.render_to_writer(writer, |name| {
        let escaped = escape_header(name);
        match escaped {
            Cow::Borrowed(b) => xml_escape(b),
            Cow::Owned(o) => match xml_escape(&o) {
                Cow::Borrowed(_) => Cow::Owned(o),
                Cow::Owned(xml_str) => Cow::Owned(xml_str),
            },
        }
    })?;
    writer.write_all(XML_DIR_END.as_bytes())?;
    writer.write_all(XML_FILES_START.as_bytes())?;
    for file in ordered_files {
        writer.write_all(XML_FILE_START.as_bytes())?;
        writer.write_all(xml_escape(file.path).as_bytes())?;
        writer.write_all(XML_TAG_CLOSE.as_bytes())?;
        write_escaped(writer, file.content)?;
        let needs_nl = (!file.content.ends_with('\n')) as usize;
        writer.write_all(FILE_TAILS[needs_nl])?;
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
                "code\u{0000}\u{0001}\u{0008}\u{000B}\u{001F}",
                "code\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}",
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

            let mut streamed = Vec::new();
            write_escaped(&mut streamed, text).unwrap();
            assert_eq!(
                String::from_utf8(streamed).unwrap(),
                escaped.as_ref(),
                "streamed content mismatch for {text:?}"
            );
        }
    }

    #[test]
    fn xml_chunks_iterator_yields_expected_sequence() {
        let input = "hello <world> & 'test'";
        let chunks: Vec<_> = XmlChunks::from_offset(input, 0).collect();
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
    fn test_xml_chunks_handles_non_escaping_ef_lead_bytes() {
        let input = "prefix \u{FFFD} & <\u{FEFF}> middle \u{FF01} suffix";
        let chunks: Vec<_> = XmlChunks::from_offset(input, 0).collect();
        assert_eq!(
            chunks,
            vec![
                XmlChunk::Raw("prefix \u{FFFD} "),
                XmlChunk::Escaped("&amp;"),
                XmlChunk::Raw(" "),
                XmlChunk::Escaped("&lt;"),
                XmlChunk::Raw("\u{FEFF}"),
                XmlChunk::Escaped("&gt;"),
                XmlChunk::Raw(" middle \u{FF01} suffix"),
            ]
        );
        let escaped = xml_escape(input);
        assert_eq!(
            escaped.as_ref(),
            "prefix \u{FFFD} &amp; &lt;\u{FEFF}&gt; middle \u{FF01} suffix"
        );
    }
}
