//! Single-pass escaping utilities for text and XML output formats.

use super::budget::{BoundedBuffer, OutputLimitExceeded};

#[inline(always)]
pub(crate) fn escape_header(bytes: &[u8], i: usize) -> Option<(usize, &'static str)> {
    match bytes[i] {
        b'\\' => Some((1, "\\\\")),
        b'\r' => Some((1, "\\r")),
        b'\n' => Some((1, "\\n")),
        0x00..=0x08 | 0x0B | 0x0C | 0x0E..=0x1F | 0x7F => Some((1, "\u{FFFD}")),
        _ => None,
    }
}

#[inline(always)]
pub(crate) fn escape_xml_body(bytes: &[u8], i: usize) -> Option<(usize, &'static str)> {
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
pub(crate) fn escape_xml_single_line(bytes: &[u8], i: usize) -> Option<(usize, &'static str)> {
    match bytes[i] {
        b'\t' => Some((1, "&#9;")),
        b'\n' => Some((1, "&#10;")),
        b'\r' => Some((1, "&#13;")),
        _ => escape_xml_body(bytes, i),
    }
}

pub(crate) fn write_escaped<F>(
    mut buf: BoundedBuffer,
    value: &str,
    special_at: F,
) -> Result<BoundedBuffer, OutputLimitExceeded>
where
    F: Fn(&[u8], usize) -> Option<(usize, &'static str)>,
{
    let bytes = value.as_bytes();
    let mut last = 0;
    let mut i = 0;
    while i < bytes.len() {
        if let Some((consumed, esc)) = special_at(bytes, i) {
            buf = buf.push_bytes(&bytes[last..i])?;
            buf = buf.push_bytes(esc.as_bytes())?;
            i += consumed;
            last = i;
        } else {
            i += 1;
        }
    }
    buf.push_bytes(&bytes[last..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_header_escaped_preserves_clean_input() {
        let buf = write_escaped(BoundedBuffer::default(), "clean_path", escape_header).unwrap();
        assert_eq!(
            std::str::from_utf8(&buf.finish().bytes).unwrap(),
            "clean_path"
        );

        let buf = write_escaped(BoundedBuffer::default(), "", escape_header).unwrap();
        assert_eq!(buf.finish().bytes.len(), 0);
    }

    #[test]
    fn write_header_escaped_replaces_all_target_characters() {
        let input = "a\\b\rc\nd\\\\e\r\n\x1b]0;title\x07\x7f\t";
        let expected = "a\\\\b\\rc\\nd\\\\\\\\e\\r\\n\u{FFFD}]0;title\u{FFFD}\u{FFFD}\t";
        let buf = write_escaped(BoundedBuffer::default(), input, escape_header).unwrap();
        assert_eq!(std::str::from_utf8(&buf.finish().bytes).unwrap(), expected);
    }

    fn xml_escape(value: &str) -> String {
        let buf = write_escaped(BoundedBuffer::default(), value, escape_xml_body).unwrap();
        String::from_utf8(buf.finish().bytes).unwrap()
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

            let streamed = write_escaped(BoundedBuffer::default(), text, escape_xml_body).unwrap();
            assert_eq!(
                String::from_utf8(streamed.finish().bytes).unwrap(),
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
        let streamed = write_escaped(BoundedBuffer::default(), input, escape_xml_body).unwrap();
        assert_eq!(
            String::from_utf8(streamed.finish().bytes).unwrap(),
            escaped.as_str()
        );
    }

    #[test]
    fn test_write_xml_attr_escaped_preserves_whitespace_char_refs() {
        let input = "tab\tnewline\r\nquote\"amp&apos'<tag>";
        let out = write_escaped(BoundedBuffer::default(), input, escape_xml_single_line).unwrap();
        assert_eq!(
            String::from_utf8(out.finish().bytes).unwrap(),
            "tab&#9;newline&#13;&#10;quote&quot;amp&amp;apos&apos;&lt;tag&gt;"
        );
    }
}
