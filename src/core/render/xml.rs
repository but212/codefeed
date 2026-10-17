//! XML format specification, entity escaping, and layout rendering.

use std::io::Write;

use crate::core::error::IngestError;
use crate::core::model::CollectionResult;

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

#[inline(always)]
fn xml_special_at(bytes: &[u8], i: usize) -> Option<(usize, &'static str)> {
    match bytes[i] {
        0..=8 | 11 | 12 | 14..=31 => Some((1, "\u{FFFD}")),
        b'"' => Some((1, "&quot;")),
        b'&' => Some((1, "&amp;")),
        b'\'' => Some((1, "&apos;")),
        b'<' => Some((1, "&lt;")),
        b'>' => Some((1, "&gt;")),
        0xEF if i + 2 < bytes.len()
            && bytes[i + 1] == 0xBF
            && (bytes[i + 2] == 0xBE || bytes[i + 2] == 0xBF) =>
        {
            Some((3, "\u{FFFD}"))
        }
        _ => None,
    }
}

#[inline(always)]
fn xml_tree_special_at(bytes: &[u8], i: usize) -> Option<(usize, &'static str)> {
    match bytes[i] {
        b'\\' => Some((1, "\\\\")),
        b'\r' => Some((1, "\\r")),
        b'\n' => Some((1, "\\n")),
        0..=8 | 11 | 12 | 14..=31 => Some((1, "\u{FFFD}")),
        b'"' => Some((1, "&quot;")),
        b'&' => Some((1, "&amp;")),
        b'\'' => Some((1, "&apos;")),
        b'<' => Some((1, "&lt;")),
        b'>' => Some((1, "&gt;")),
        0xEF if i + 2 < bytes.len()
            && bytes[i + 1] == 0xBF
            && (bytes[i + 2] == 0xBE || bytes[i + 2] == 0xBF) =>
        {
            Some((3, "\u{FFFD}"))
        }
        _ => None,
    }
}

fn write_stream_escaped<W, F>(writer: &mut W, value: &str, special_at: F) -> std::io::Result<()>
where
    W: Write + ?Sized,
    F: Fn(&[u8], usize) -> Option<(usize, &'static str)>,
{
    let bytes = value.as_bytes();
    let Some(first) = (0..bytes.len()).find(|&i| special_at(bytes, i).is_some()) else {
        return writer.write_all(bytes);
    };

    writer.write_all(&bytes[..first])?;
    let mut last = first;
    let mut i = first;
    while i < bytes.len() {
        if let Some((consumed, esc)) = special_at(bytes, i) {
            if last < i {
                writer.write_all(&bytes[last..i])?;
            }
            writer.write_all(esc.as_bytes())?;
            i += consumed;
            last = i;
        } else {
            i += 1;
        }
    }
    if last < bytes.len() {
        writer.write_all(&bytes[last..])?;
    }
    Ok(())
}

/// Escapes standard XML characters and control bytes while streaming to `writer`.
pub(crate) fn write_escaped<W: Write + ?Sized>(writer: &mut W, value: &str) -> std::io::Result<()> {
    write_stream_escaped(writer, value, xml_special_at)
}

/// Escapes newlines (`\n`, `\r`, `\`) and XML specials for tree node names in `<directory_structure>`.
pub(crate) fn write_xml_tree_escaped<W: Write + ?Sized>(
    writer: &mut W,
    value: &str,
) -> std::io::Result<()> {
    write_stream_escaped(writer, value, xml_tree_special_at)
}

pub(crate) fn render_xml<W: Write + ?Sized>(
    writer: &mut W,
    collection: &CollectionResult,
) -> Result<(), IngestError> {
    writer.write_all(XML_REPO_START.as_bytes())?;
    write_escaped(writer, &collection.label)?;
    if let Some(ref revision) = collection.revision {
        writer.write_all(XML_REVISION_ATTR.as_bytes())?;
        write_escaped(writer, revision)?;
    }
    writer.write_all(XML_TAG_CLOSE.as_bytes())?;
    writer.write_all(XML_DIR_START.as_bytes())?;
    crate::core::tree::render_tree(writer, &collection.files, write_xml_tree_escaped)?;
    writer.write_all(XML_DIR_END.as_bytes())?;
    writer.write_all(XML_FILES_START.as_bytes())?;
    for file in collection.files.iter() {
        writer.write_all(XML_FILE_START.as_bytes())?;
        write_escaped(writer, file.path)?;
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

    fn xml_escape(value: &str) -> String {
        let mut buf = Vec::new();
        write_escaped(&mut buf, value).unwrap();
        String::from_utf8(buf).unwrap()
    }

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
                escaped.as_str(),
                expected_escaped,
                "escape mismatch for {text:?}"
            );

            let mut streamed = Vec::new();
            write_escaped(&mut streamed, text).unwrap();
            assert_eq!(
                String::from_utf8(streamed).unwrap(),
                escaped.as_str(),
                "streamed content mismatch for {text:?}"
            );
        }
    }

    #[test]
    fn test_xml_escape_and_streaming_handles_non_escaping_ef_lead_bytes() {
        let input = "prefix \u{FFFD} & <\u{FEFF}> middle \u{FF01} suffix \u{FFFE} and \u{FFFF}";
        let escaped = xml_escape(input);
        assert_eq!(
            escaped.as_str(),
            "prefix \u{FFFD} &amp; &lt;\u{FEFF}&gt; middle \u{FF01} suffix \u{FFFD} and \u{FFFD}"
        );
        let mut streamed = Vec::new();
        write_escaped(&mut streamed, input).unwrap();
        assert_eq!(String::from_utf8(streamed).unwrap(), escaped.as_str());
    }

    #[test]
    fn test_write_xml_tree_escaped_preserves_newline_injection_defense() {
        let input = "dir\nname\r<test>\\file";
        let mut out = Vec::new();
        write_xml_tree_escaped(&mut out, input).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "dir\\nname\\r&lt;test&gt;\\\\file"
        );
    }
}
