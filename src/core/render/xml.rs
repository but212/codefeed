//! XML format specification, entity escaping, and layout rendering.

use std::borrow::Cow;
use std::io::Write;

use super::text::escape_header;
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

fn first_special_index(bytes: &[u8]) -> Option<usize> {
    (0..bytes.len()).find(|&i| xml_special_at(bytes, i).is_some())
}

fn xml_escape(value: &str) -> Cow<'_, str> {
    let bytes = value.as_bytes();
    let Some(first) = first_special_index(bytes) else {
        return Cow::Borrowed(value);
    };

    let mut result = String::with_capacity(value.len() + 16);
    result.push_str(&value[..first]);
    let mut last = first;
    let mut i = first;
    while i < bytes.len() {
        if let Some((consumed, esc)) = xml_special_at(bytes, i) {
            result.push_str(&value[last..i]);
            result.push_str(esc);
            i += consumed;
            last = i;
        } else {
            i += 1;
        }
    }
    result.push_str(&value[last..]);
    Cow::Owned(result)
}

fn write_escaped<W: Write + ?Sized>(writer: &mut W, value: &str) -> std::io::Result<()> {
    let bytes = value.as_bytes();
    let Some(first) = first_special_index(bytes) else {
        return writer.write_all(bytes);
    };

    writer.write_all(&bytes[..first])?;
    let mut last = first;
    let mut i = first;
    while i < bytes.len() {
        if let Some((consumed, esc)) = xml_special_at(bytes, i) {
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

pub(crate) fn render_xml<'a, W, I>(
    writer: &mut W,
    label: &str,
    revision: Option<&str>,
    ordered_files: I,
    paths: &[&str],
) -> Result<(), IngestError>
where
    W: Write + ?Sized,
    I: IntoIterator<Item = FileRecord<'a>>,
{
    writer.write_all(XML_REPO_START.as_bytes())?;
    write_escaped(writer, label)?;
    if let Some(revision) = revision {
        writer.write_all(XML_REVISION_ATTR.as_bytes())?;
        write_escaped(writer, revision)?;
    }
    writer.write_all(XML_TAG_CLOSE.as_bytes())?;
    writer.write_all(XML_DIR_START.as_bytes())?;
    crate::core::tree::render_tree(writer, paths, |name| {
        let escaped = escape_header(name);
        match escaped {
            Cow::Borrowed(b) => xml_escape(b),
            Cow::Owned(o) => Cow::Owned(xml_escape(&o).into_owned()),
        }
    })?;
    writer.write_all(XML_DIR_END.as_bytes())?;
    writer.write_all(XML_FILES_START.as_bytes())?;
    for file in ordered_files {
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
    fn test_xml_escape_and_streaming_handles_non_escaping_ef_lead_bytes() {
        let input = "prefix \u{FFFD} & <\u{FEFF}> middle \u{FF01} suffix \u{FFFE} and \u{FFFF}";
        let escaped = xml_escape(input);
        assert_eq!(
            escaped.as_ref(),
            "prefix \u{FFFD} &amp; &lt;\u{FEFF}&gt; middle \u{FF01} suffix \u{FFFD} and \u{FFFD}"
        );
        let mut streamed = Vec::new();
        write_escaped(&mut streamed, input).unwrap();
        assert_eq!(String::from_utf8(streamed).unwrap(), escaped.as_ref());
    }

    #[test]
    fn test_first_special_index_boundary_alignment() {
        for len in 0..=48 {
            let clean = "a".repeat(len);
            assert_eq!(first_special_index(clean.as_bytes()), None);
            assert!(matches!(xml_escape(&clean), Cow::Borrowed(_)));

            for special in ['&', '<', '>', '"', '\''] {
                for pos in 0..len {
                    let mut text = clean.clone();
                    text.replace_range(pos..pos + 1, &special.to_string());
                    assert_eq!(
                        first_special_index(text.as_bytes()),
                        Some(pos),
                        "mismatch at len {len}, pos {pos}, special {special}"
                    );
                    assert!(matches!(xml_escape(&text), Cow::Owned(_)));
                }
            }
        }
    }
}
