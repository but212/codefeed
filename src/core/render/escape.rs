//! Single-pass escaping utilities for text and XML output formats.

use super::budget::{BoundedBuffer, OutputLimitExceeded};

#[must_use]
pub(super) const fn escape_header(bytes: &[u8], i: usize) -> Option<(usize, &'static str)> {
    match bytes[i] {
        b'\\' => Some((1, "\\\\")),
        b'\r' => Some((1, "\\r")),
        b'\n' => Some((1, "\\n")),
        0x00..=0x08 | 0x0B | 0x0C | 0x0E..=0x1F | 0x7F => Some((1, "\u{FFFD}")),
        _ => None,
    }
}

// Only the `0xEF` arm reads past its own byte, and it matches for the continuation `0xBF 0xBE`.
// One probe per byte therefore decides whether any continuation can match.
const XML_BODY_START: [bool; 256] = {
    let mut table = [false; 256];
    let mut b = 0u8;
    loop {
        table[b as usize] = xml_body_match(&[b, 0xBF, 0xBE], 0).is_some();
        if b == 255 {
            break;
        }
        b += 1;
    }
    table
};

#[must_use]
pub(super) const fn escape_xml_body(bytes: &[u8], i: usize) -> Option<(usize, &'static str)> {
    if !XML_BODY_START[bytes[i] as usize] {
        return None;
    }
    xml_body_match(bytes, i)
}

const fn xml_body_match(bytes: &[u8], i: usize) -> Option<(usize, &'static str)> {
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

#[must_use]
pub(super) const fn escape_xml_single_line(
    bytes: &[u8],
    i: usize,
) -> Option<(usize, &'static str)> {
    match bytes[i] {
        b'\t' => Some((1, "&#9;")),
        b'\n' => Some((1, "&#10;")),
        b'\r' => Some((1, "&#13;")),
        _ => escape_xml_body(bytes, i),
    }
}

pub(super) fn write_escaped<F>(
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
    fn xml_body_escaping_examples() {
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
            (
                "prefix \u{FFFD} & <\u{FEFF}> middle \u{FF01} suffix \u{FFFE} and \u{FFFF}",
                "prefix \u{FFFD} &amp; &lt;\u{FEFF}&gt; middle \u{FF01} suffix \u{FFFD} and \u{FFFD}",
            ),
        ];

        for &(text, expected_escaped) in cases {
            assert_eq!(
                xml_escape(text),
                expected_escaped,
                "escape mismatch for {text:?}"
            );
        }
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

    #[test]
    fn xml_body_start_table_admits_every_byte_that_matches() {
        for first in 0..=u8::MAX {
            for second in 0..=u8::MAX {
                for third in 0..=u8::MAX {
                    let window = [first, second, third];
                    let matched = xml_body_match(&window, 0);
                    assert_eq!(escape_xml_body(&window, 0), matched);
                    if matched.is_some() {
                        assert!(XML_BODY_START[usize::from(first)], "window {window:?}");
                    }
                }
            }
        }
    }

    mod properties {
        use super::*;
        use proptest::prelude::*;

        type SpecialAt = fn(&[u8], usize) -> Option<(usize, &'static str)>;

        /// Characters that each escaper treats specially, mixed with arbitrary ones.
        fn text() -> impl Strategy<Value = String> {
            const SPECIAL: &[char] = &[
                '<', '>', '&', '"', '\'', '\\', '\t', '\r', '\n', '\0', '\x08', '\x0b', '\x0c',
                '\x0e', '\x1f', '\x7f', '\u{FFFD}', '\u{FFFE}', '\u{FFFF}',
            ];
            prop::collection::vec(
                prop_oneof![
                    3 => any::<char>(),
                    2 => prop::sample::select(SPECIAL),
                ],
                0..48,
            )
            .prop_map(String::from_iter)
        }

        fn escape(value: &str, special_at: SpecialAt) -> String {
            let buf = write_escaped(BoundedBuffer::default(), value, special_at).unwrap();
            String::from_utf8(buf.finish().bytes).unwrap()
        }

        fn model_xml_body(c: char) -> String {
            match c {
                '"' => "&quot;".into(),
                '&' => "&amp;".into(),
                '\'' => "&apos;".into(),
                '<' => "&lt;".into(),
                '>' => "&gt;".into(),
                '\0'..='\x08' | '\x0b' | '\x0c' | '\x0e'..='\x1f' | '\u{FFFE}' | '\u{FFFF}' => {
                    "\u{FFFD}".into()
                }
                c => c.to_string(),
            }
        }

        fn model_xml_single_line(c: char) -> String {
            match c {
                '\t' => "&#9;".into(),
                '\n' => "&#10;".into(),
                '\r' => "&#13;".into(),
                c => model_xml_body(c),
            }
        }

        fn model_header(c: char) -> String {
            match c {
                '\\' => "\\\\".into(),
                '\r' => "\\r".into(),
                '\n' => "\\n".into(),
                '\0'..='\x08' | '\x0b' | '\x0c' | '\x0e'..='\x1f' | '\x7f' => "\u{FFFD}".into(),
                c => c.to_string(),
            }
        }

        type Escaper = (&'static str, SpecialAt, fn(char) -> String);

        fn escapers() -> [Escaper; 3] {
            [
                ("header", escape_header, model_header),
                ("xml body", escape_xml_body, model_xml_body),
                (
                    "xml single line",
                    escape_xml_single_line,
                    model_xml_single_line,
                ),
            ]
        }

        proptest! {
            #![proptest_config(crate::core::proptest_config())]

            /// Each escaper maps every character independently, as its documented table says.
            #[test]
            fn escaping_matches_the_per_character_table(value in text()) {
                for (name, special_at, model) in escapers() {
                    let expected: String = value.chars().map(model).collect();
                    prop_assert_eq!(escape(&value, special_at), expected, "{}", name);
                }
            }

            /// Splitting a value at a character boundary and escaping the parts separately gives the
            /// same bytes as escaping it whole, so the three-byte `U+FFFE`/`U+FFFF` match never
            /// depends on chunking.
            #[test]
            fn escaping_commutes_with_concatenation(a in text(), b in text()) {
                let joined = format!("{a}{b}");
                for (name, special_at, _) in escapers() {
                    prop_assert_eq!(
                        escape(&joined, special_at),
                        format!("{}{}", escape(&a, special_at), escape(&b, special_at)),
                        "{}",
                        name
                    );
                }
            }
        }
    }
}
