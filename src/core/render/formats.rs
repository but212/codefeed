use super::budget::{BoundedBuffer, OutputLimitExceeded};

use crate::core::model::DigestFormat;

// Text format markers
const REPOSITORY_PREFIX: &str = "Repository: ";
const REVISION_PREFIX: &str = "Revision: ";
const FILES_PREFIX: &str = "Files: ";
const SKIPPED_PREFIX: &str = "\nSkipped: ";
const DIRECTORY_HEADER: &str = "Directory structure:\n";
const FILES_HEADER: &str = "\nFiles:\n\n";
const FILE_BLOCK_PREFIX: &str = "==================================================\nFILE: ";
const FILE_BLOCK_SUFFIX: &str = "\n==================================================\n\n";
const NEWLINE: &str = "\n";
const METADATA_SUFFIX: &str = "\n\n";

// XML format markers
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

pub(super) fn write_header(
    format: DigestFormat,
    mut buf: BoundedBuffer,
    label: &str,
    revision: Option<&str>,
    file_count: usize,
    skipped: usize,
) -> Result<BoundedBuffer, OutputLimitExceeded> {
    match format {
        DigestFormat::Text => {
            buf = buf.push_bytes(REPOSITORY_PREFIX.as_bytes())?;
            buf = write_escaped(buf, label, escape_header)?;
            buf = buf.push_bytes(NEWLINE.as_bytes())?;
            if let Some(rev) = revision {
                buf = buf.push_bytes(REVISION_PREFIX.as_bytes())?;
                buf = write_escaped(buf, rev, escape_header)?;
                buf = buf.push_bytes(NEWLINE.as_bytes())?;
            }
            buf = buf.push_bytes(FILES_PREFIX.as_bytes())?;
            buf = buf.push_usize(file_count)?;
            buf = buf.push_bytes(SKIPPED_PREFIX.as_bytes())?;
            buf = buf.push_usize(skipped)?;
            buf = buf.push_bytes(METADATA_SUFFIX.as_bytes())?;
            buf = buf.push_bytes(DIRECTORY_HEADER.as_bytes())?;
            Ok(buf)
        }
        DigestFormat::Xml => {
            buf = buf.push_bytes(XML_REPO_START.as_bytes())?;
            buf = write_escaped(buf, label, escape_xml_single_line)?;
            if let Some(rev) = revision {
                buf = buf.push_bytes(XML_REVISION_ATTR.as_bytes())?;
                buf = write_escaped(buf, rev, escape_xml_single_line)?;
            }
            buf = buf.push_bytes(XML_TAG_CLOSE.as_bytes())?;
            buf = buf.push_bytes(XML_DIR_START.as_bytes())?;
            Ok(buf)
        }
    }
}

pub(super) fn write_tree_name(
    format: DigestFormat,
    buf: BoundedBuffer,
    name: &str,
) -> Result<BoundedBuffer, OutputLimitExceeded> {
    match format {
        DigestFormat::Text => write_escaped(buf, name, escape_header),
        DigestFormat::Xml => write_escaped(buf, name, escape_xml_single_line),
    }
}

pub(super) fn close_tree_section(
    format: DigestFormat,
    mut buf: BoundedBuffer,
) -> Result<BoundedBuffer, OutputLimitExceeded> {
    match format {
        DigestFormat::Text => buf.push_bytes(FILES_HEADER.as_bytes()),
        DigestFormat::Xml => {
            buf = buf.push_bytes(XML_DIR_END.as_bytes())?;
            buf = buf.push_bytes(XML_FILES_START.as_bytes())?;
            Ok(buf)
        }
    }
}

pub(super) fn write_file_entry(
    format: DigestFormat,
    mut buf: BoundedBuffer,
    path: &str,
    content: &str,
) -> Result<BoundedBuffer, OutputLimitExceeded> {
    match format {
        DigestFormat::Text => {
            const TAIL: [&[u8]; 2] = [b"\n", b"\n\n"];
            let needs_extra_nl = usize::from(!content.ends_with('\n'));
            buf = buf.push_bytes(FILE_BLOCK_PREFIX.as_bytes())?;
            buf = write_escaped(buf, path, escape_header)?;
            buf = buf.push_bytes(FILE_BLOCK_SUFFIX.as_bytes())?;
            buf = buf.push_bytes(content.as_bytes())?;
            buf = buf.push_bytes(TAIL[needs_extra_nl])?;
            Ok(buf)
        }
        DigestFormat::Xml => {
            buf = buf.push_bytes(XML_FILE_START.as_bytes())?;
            buf = write_escaped(buf, path, escape_xml_single_line)?;
            buf = buf.push_bytes(XML_TAG_CLOSE.as_bytes())?;
            buf = write_escaped(buf, content, escape_xml_body)?;
            let needs_nl = usize::from(!content.ends_with('\n'));
            buf = buf.push_bytes(FILE_TAILS[needs_nl])?;
            Ok(buf)
        }
    }
}

pub(super) fn write_footer(
    format: DigestFormat,
    mut buf: BoundedBuffer,
) -> Result<BoundedBuffer, OutputLimitExceeded> {
    match format {
        DigestFormat::Text => Ok(buf),
        DigestFormat::Xml => {
            buf = buf.push_bytes(XML_FILES_END.as_bytes())?;
            buf = buf.push_bytes(XML_REPO_END.as_bytes())?;
            Ok(buf)
        }
    }
}

#[must_use]
const fn escape_header(bytes: &[u8], i: usize) -> Option<(usize, &'static str)> {
    match bytes[i] {
        b'\\' => Some((1, "\\\\")),
        b'\r' => Some((1, "\\r")),
        b'\n' => Some((1, "\\n")),
        0x00..=0x08 | 0x0B | 0x0C | 0x0E..=0x1F | 0x7F => Some((1, "\u{FFFD}")),
        _ => None,
    }
}

#[must_use]
const fn escape_xml_body(bytes: &[u8], i: usize) -> Option<(usize, &'static str)> {
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
const fn escape_xml_single_line(bytes: &[u8], i: usize) -> Option<(usize, &'static str)> {
    match bytes[i] {
        b'\t' => Some((1, "&#9;")),
        b'\n' => Some((1, "&#10;")),
        b'\r' => Some((1, "&#13;")),
        _ => escape_xml_body(bytes, i),
    }
}

fn write_escaped<F>(
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

    type SpecialAt = fn(&[u8], usize) -> Option<(usize, &'static str)>;

    fn escape_with(special_at: SpecialAt, value: &str) -> String {
        let buf = write_escaped(BoundedBuffer::default(), value, special_at).unwrap();
        String::from_utf8(buf.finish().bytes).unwrap()
    }

    fn xml_escape(value: &str) -> String {
        escape_with(escape_xml_body, value)
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
    fn header_and_attribute_escaping_examples() {
        // Literal anchors for the per-character models in `properties`.
        assert_eq!(escape_with(escape_header, "clean_path"), "clean_path");
        assert_eq!(escape_with(escape_header, ""), "");
        assert_eq!(
            escape_with(escape_header, "a\\b\rc\nd\\\\e\r\n\x1b]0;title\x07\x7f\t"),
            "a\\\\b\\rc\\nd\\\\\\\\e\\r\\n\u{FFFD}]0;title\u{FFFD}\u{FFFD}\t"
        );
        assert_eq!(
            escape_with(
                escape_xml_single_line,
                "tab\tnewline\r\nquote\"amp&apos'<tag>"
            ),
            "tab&#9;newline&#13;&#10;quote&quot;amp&amp;apos&apos;&lt;tag&gt;"
        );
    }

    mod properties {
        use super::*;
        use proptest::prelude::*;

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

    use crate::core::render::render_digest;
    use crate::core::selector::CollectionResult;
    use crate::core::storage::{FileRecord, FileStorage, SortedFiles};

    fn render_digest_str(
        format: DigestFormat,
        label: &str,
        revision: Option<&str>,
        files: &[FileRecord<'_>],
        skipped: usize,
    ) -> Result<String, OutputLimitExceeded> {
        let storage: FileStorage = files.iter().copied().collect();
        let collection = CollectionResult {
            label: label.to_string(),
            revision: revision.map(str::to_string),
            files: SortedFiles::new(storage).expect("test fixture has no file/directory collision"),
            skipped,
        };
        let buf = render_digest(BoundedBuffer::default(), &collection, format)?;
        Ok(String::from_utf8(buf.finish().bytes).unwrap())
    }

    fn render_text_digest(
        label: &str,
        revision: Option<&str>,
        files: &[FileRecord<'_>],
        skipped: usize,
    ) -> Result<String, OutputLimitExceeded> {
        render_digest_str(DigestFormat::Text, label, revision, files, skipped)
    }

    fn render_xml_digest(
        label: &str,
        revision: Option<&str>,
        files: &[FileRecord<'_>],
        skipped: usize,
    ) -> Result<String, OutputLimitExceeded> {
        render_digest_str(DigestFormat::Xml, label, revision, files, skipped)
    }

    #[test]
    fn text_escapes_revision_header() {
        let revision = "feature\\x\r\nFiles: 9";
        let rendered = render_text_digest("repo", Some(revision), &[], 0).unwrap();

        assert_eq!(
            rendered.lines().nth(1),
            Some("Revision: feature\\\\x\\r\\nFiles: 9")
        );
        assert!(!rendered.contains(revision));
    }

    #[test]
    fn text_streaming_matches_string_render() {
        let files = [
            FileRecord {
                path: "b.txt",
                content: "hello\n",
            },
            FileRecord {
                path: "a.txt",
                content: "world",
            },
        ];
        let storage: FileStorage = files.into_iter().collect();
        let collection = CollectionResult {
            label: "my-repo".into(),
            revision: Some("v1.0".into()),
            files: SortedFiles::new(storage).expect("test fixture has no file/directory collision"),
            skipped: 2,
        };
        let buf = render_digest(BoundedBuffer::default(), &collection, DigestFormat::Text).unwrap();
        let digest = buf.finish();
        let streamed_str = std::str::from_utf8(&digest.bytes).unwrap();

        let expected = "\
Repository: my-repo\n\
Revision: v1.0\n\
Files: 2\n\
Skipped: 2\n\
\n\
Directory structure:\n\
├── a.txt\n\
└── b.txt\n\
\n\
Files:\n\
\n\
==================================================\n\
FILE: a.txt\n\
==================================================\n\
\n\
world\n\
\n\
==================================================\n\
FILE: b.txt\n\
==================================================\n\
\n\
hello\n\
\n";
        assert_eq!(streamed_str, expected);
    }

    #[test]
    fn text_escapes_control_characters_in_filename_headers_and_tree() {
        let filename = "x\x1b]0;title\x07.md";
        let files = [FileRecord {
            path: filename,
            content: "content\n",
        }];
        let text_rendered = render_text_digest("repo", None, &files, 0).unwrap();
        assert!(!text_rendered.contains('\x1b'));
        assert!(!text_rendered.contains('\x07'));
        assert!(text_rendered.contains("x\u{FFFD}]0;title\u{FFFD}.md"));
    }

    #[test]
    fn xml_renders_in_deterministic_order() {
        let files = [
            FileRecord {
                path: "z.txt",
                content: "z < 10",
            },
            FileRecord {
                path: "a.txt",
                content: "a & b",
            },
        ];
        let xml_str = render_xml_digest("my-repo", Some("v1.0"), &files, 1).unwrap();
        let expected = "\
<repository name=\"my-repo\" revision=\"v1.0\">
<directory_structure>
├── a.txt
└── z.txt
</directory_structure>
<files>
<file path=\"a.txt\">
a &amp; b
</file>
<file path=\"z.txt\">
z &lt; 10
</file>
</files>
</repository>
";
        assert_eq!(xml_str, expected);
    }

    #[test]
    fn xml_attr_escapes_whitespace_and_preserves_body() {
        let files = [FileRecord {
            path: "dir\nname\r\t\\back/file.rs",
            content: "fn main() {\n\tprintln!(\"hello\");\r\n}\n",
        }];
        let xml_str = render_xml_digest("my\trepo\n", Some("rev\r\n1"), &files, 0).unwrap();

        let expected = "\
<repository name=\"my&#9;repo&#10;\" revision=\"rev&#13;&#10;1\">
<directory_structure>
└── dir&#10;name&#13;&#9;\\back/
    └── file.rs
</directory_structure>
<files>
<file path=\"dir&#10;name&#13;&#9;\\back/file.rs\">
fn main() {\n\tprintln!(&quot;hello&quot;);\r\n}
</file>
</files>
</repository>
";
        assert_eq!(xml_str, expected);
    }
}
