//! Digest rendering and streaming token estimation.
//!
//! Coordinates layout rendering across text and XML formats, streaming
//! token estimation, and output size cap enforcement.

use std::io::Write;

use crate::core::error::IngestError;
use crate::core::model::{CollectionResult, DigestFormat};
use crate::core::policy::MAX_OUTPUT_SIZE;

mod budget;
mod escape;
mod text;
mod tree;
mod xml;

pub(crate) use budget::BoundedEstimatingWriter;

impl DigestFormat {
    pub(crate) fn render<W: Write + ?Sized>(
        self,
        writer: &mut W,
        collection: &CollectionResult,
    ) -> std::io::Result<()> {
        match self {
            Self::Text => text::render_text(writer, collection),
            Self::Xml => xml::render_xml(writer, collection),
        }
    }
}

/// Renders a collection to the given writer according to the chosen digest format.
///
/// Binds output generation to [`MAX_OUTPUT_SIZE`] and calculates the estimated token count.
///
/// # Errors
///
/// Returns [`IngestError::OutputTooLarge`] if output exceeds the maximum size limit,
/// or [`IngestError::Io`] if writing fails.
pub(crate) fn render_to_writer<W: Write + ?Sized>(
    writer: &mut W,
    collection: &CollectionResult,
    format: DigestFormat,
) -> Result<usize, IngestError> {
    let mut estimating_writer = BoundedEstimatingWriter::new(writer);
    if let Err(err) = format.render(&mut estimating_writer, collection) {
        if estimating_writer.written() > MAX_OUTPUT_SIZE {
            return Err(collection.output_too_large_error());
        }
        return Err(IngestError::Io(err));
    }
    Ok(estimating_writer.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::error::IngestError;
    use crate::core::policy::MAX_OUTPUT_SIZE;
    use crate::core::render::budget::estimate_tokens;
    use crate::core::render::escape::{
        escape_header, escape_xml_body, escape_xml_single_line, write_escaped,
    };
    use crate::core::render::tree::{component_at_offset, render_tree};
    use crate::core::storage::{FileRecord, FileStorage, SortedFiles};

    fn render_digest(
        label: &str,
        revision: Option<&str>,
        files: &[FileRecord<'_>],
        skipped: usize,
    ) -> Result<String, IngestError> {
        let mut storage: FileStorage = files.iter().copied().collect();
        storage.sort_by_path();
        let collection = CollectionResult {
            label: label.to_string(),
            revision: revision.map(str::to_string),
            files: crate::core::storage::SortedFiles::new(storage),
            skipped,
        };
        let mut buffer = Vec::new();
        render_to_writer(&mut buffer, &collection, DigestFormat::Text)?;
        String::from_utf8(buffer).map_err(|error| IngestError::Io(std::io::Error::other(error)))
    }

    #[test]
    fn renders_files_in_deterministic_order() -> Result<(), Box<dyn std::error::Error>> {
        let files = vec![
            FileRecord {
                path: "z.txt",
                content: "z",
            },
            FileRecord {
                path: "a.txt",
                content: "a",
            },
        ];
        let digest = render_digest("repo", Some("abc"), &files, 1)?;
        let first = digest
            .find("FILE: a.txt")
            .ok_or_else(|| std::io::Error::other("a.txt is missing"))?;
        let second = digest
            .find("FILE: z.txt")
            .ok_or_else(|| std::io::Error::other("z.txt is missing"))?;
        assert!(first < second);
        assert!(digest.contains("Revision: abc"));
        assert!(digest.contains("Skipped: 1"));
        Ok(())
    }

    #[test]
    fn escapes_revision_header() -> Result<(), Box<dyn std::error::Error>> {
        let revision = "feature\\x\r\nFiles: 9";
        let rendered = render_digest("repo", Some(revision), &[], 0)?;

        assert_eq!(
            rendered.lines().nth(1),
            Some("Revision: feature\\\\x\\r\\nFiles: 9")
        );
        assert!(!rendered.contains(revision));
        Ok(())
    }

    #[test]
    fn streaming_matches_string_render() -> Result<(), Box<dyn std::error::Error>> {
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
        let mut storage: FileStorage = files.into_iter().collect();
        storage.sort_by_path();
        let collection = CollectionResult {
            label: "my-repo".into(),
            revision: Some("v1.0".into()),
            files: crate::core::storage::SortedFiles::new(storage),
            skipped: 2,
        };
        let mut streamed = Vec::new();
        render_to_writer(&mut streamed, &collection, DigestFormat::Text)?;
        let streamed_str = std::str::from_utf8(&streamed)?;

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
        Ok(())
    }

    #[test]
    fn renders_from_arbitrary_iterator() -> Result<(), Box<dyn std::error::Error>> {
        let files = [
            FileRecord {
                path: "b.txt",
                content: "content_b\n",
            },
            FileRecord {
                path: "excluded.txt",
                content: "skip_me\n",
            },
            FileRecord {
                path: "a.txt",
                content: "content_a\n",
            },
        ];
        let mut storage = FileStorage::new();
        for f in files.into_iter().filter(|f| f.path != "excluded.txt") {
            storage.push(f.path, f.content);
        }
        storage.sort_by_path();
        let collection = CollectionResult {
            label: "repo".into(),
            revision: None,
            files: crate::core::storage::SortedFiles::new(storage),
            skipped: 1,
        };
        let mut buf = Vec::new();
        DigestFormat::Text.render(&mut buf, &collection)?;
        let rendered = std::str::from_utf8(&buf)?;
        assert!(rendered.contains("Files: 2"));
        assert!(rendered.contains("FILE: a.txt"));
        assert!(rendered.contains("FILE: b.txt"));
        assert!(!rendered.contains("excluded.txt"));
        Ok(())
    }

    #[test]
    fn render_to_writer_enforces_output_size_cap() {
        let content = "x".repeat(MAX_OUTPUT_SIZE as usize);
        let candidate = FileRecord {
            path: "large.txt",
            content: &content,
        };
        let mut storage = FileStorage::new();
        storage.push(candidate.path, candidate.content);
        let collection = CollectionResult {
            label: "repo".into(),
            revision: None,
            files: crate::core::storage::SortedFiles::new(storage),
            skipped: 0,
        };
        let mut sink = Vec::new();
        match render_to_writer(&mut sink, &collection, DigestFormat::Text) {
            Err(IngestError::OutputTooLarge(largest_files)) => {
                assert_eq!(largest_files.len(), 1);
                assert_eq!(largest_files[0].0, "large.txt");
            }
            other => panic!("expected OutputTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn renders_xml_in_deterministic_order() -> Result<(), Box<dyn std::error::Error>> {
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
        let mut storage: FileStorage = files.into_iter().collect();
        storage.sort_by_path();
        let collection = CollectionResult {
            label: "my-repo".into(),
            revision: Some("v1.0".into()),
            files: crate::core::storage::SortedFiles::new(storage),
            skipped: 1,
        };
        let mut out = Vec::new();
        render_to_writer(&mut out, &collection, DigestFormat::Xml)?;
        let xml_str = std::str::from_utf8(&out)?;
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
        Ok(())
    }

    #[test]
    fn control_character_policy_preserves_raw_text_and_sanitizes_xml()
    -> Result<(), Box<dyn std::error::Error>> {
        let raw_ansi = "\x1b[31mRed Alert\x1b[0m\tTabbed\r\nNewline";
        let files = [FileRecord {
            path: "ansi.txt",
            content: raw_ansi,
        }];

        let text_rendered = render_digest("repo", None, &files, 0)?;
        assert!(
            text_rendered.contains(raw_ansi),
            "Text format must preserve raw ANSI/control characters for LLM fidelity"
        );

        let collection = CollectionResult {
            label: "repo".into(),
            revision: None,
            files: crate::core::storage::SortedFiles::new(files.into_iter().collect()),
            skipped: 0,
        };
        let mut xml_out = Vec::new();
        render_to_writer(&mut xml_out, &collection, DigestFormat::Xml)?;
        let xml_str = std::str::from_utf8(&xml_out)?;
        assert!(
            !xml_str.contains("\x1b"),
            "XML format must not emit raw ASCII ESC"
        );
        assert!(
            xml_str.contains("\u{FFFD}[31mRed Alert\u{FFFD}[0m\tTabbed\r\nNewline"),
            "XML format must sanitize ESC to replacement char while preserving valid whitespace"
        );

        Ok(())
    }

    #[test]
    fn xml_attr_escapes_whitespace_and_preserves_body() -> Result<(), Box<dyn std::error::Error>> {
        let files = [FileRecord {
            path: "dir\nname\r\t\\back/file.rs",
            content: "fn main() {\n\tprintln!(\"hello\");\r\n}\n",
        }];
        let mut storage: FileStorage = files.into_iter().collect();
        storage.sort_by_path();
        let collection = CollectionResult {
            label: "my\trepo\n".into(),
            revision: Some("rev\r\n1".into()),
            files: crate::core::storage::SortedFiles::new(storage),
            skipped: 0,
        };
        let mut xml_out = Vec::new();
        render_to_writer(&mut xml_out, &collection, DigestFormat::Xml)?;
        let xml_str = std::str::from_utf8(&xml_out)?;

        assert!(
            xml_str.contains("<repository name=\"my&#9;repo&#10;\" revision=\"rev&#13;&#10;1\">")
        );
        assert!(xml_str.contains("<file path=\"dir&#10;name&#13;&#9;\\back/file.rs\">"));
        assert!(xml_str.contains("dir&#10;name&#13;&#9;\\back/"));
        assert!(xml_str.contains("fn main() {\n\tprintln!(&quot;hello&quot;);\r\n}\n"));
        Ok(())
    }

    #[test]
    fn bounded_estimating_writer_finish_does_not_flush_inner_stream() {
        struct BrokenPipeOnFlushWriter {
            buf: Vec<u8>,
        }

        impl Write for BrokenPipeOnFlushWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.buf.extend_from_slice(buf);
                Ok(buf.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "closed pipe",
                ))
            }
        }

        let mut inner = BrokenPipeOnFlushWriter { buf: Vec::new() };
        let mut writer = BoundedEstimatingWriter::new(&mut inner);
        writer.write_all(b"hello world\n").unwrap();
        let tokens = writer.finish();
        assert!(tokens > 0);
        assert_eq!(inner.buf, b"hello world\n");
    }

    #[test]
    fn estimate_tokens_boundaries() {
        assert_eq!(estimate_tokens(0), 0);
        assert_eq!(estimate_tokens(1), 1);
        assert_eq!(estimate_tokens(3), 1);
        assert_eq!(estimate_tokens(4), 1);
        assert_eq!(estimate_tokens(5), 1);
        assert_eq!(estimate_tokens(8), 2);
    }

    #[test]
    fn write_header_escaped_preserves_clean_input() {
        let mut buf = Vec::new();
        write_escaped(&mut buf, "clean_path", escape_header).unwrap();
        assert_eq!(std::str::from_utf8(&buf).unwrap(), "clean_path");

        buf.clear();
        write_escaped(&mut buf, "", escape_header).unwrap();
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn write_header_escaped_replaces_all_target_characters() {
        let input = "a\\b\rc\nd\\\\e\r\n\x1b]0;title\x07\x7f\t";
        let expected = "a\\\\b\\rc\\nd\\\\\\\\e\\r\\n\u{FFFD}]0;title\u{FFFD}\u{FFFD}\t";
        let mut buf = Vec::new();
        write_escaped(&mut buf, input, escape_header).unwrap();
        assert_eq!(std::str::from_utf8(&buf).unwrap(), expected);
    }

    #[test]
    fn escapes_control_characters_in_filename_headers_and_tree()
    -> Result<(), Box<dyn std::error::Error>> {
        let filename = "x\x1b]0;title\x07.md";
        let files = [FileRecord {
            path: filename,
            content: "content\n",
        }];
        let text_rendered = render_digest("repo", None, &files, 0)?;
        assert!(!text_rendered.contains("\x1b"));
        assert!(!text_rendered.contains("\x07"));
        assert!(text_rendered.contains("x\u{FFFD}]0;title\u{FFFD}.md"));
        Ok(())
    }

    fn xml_escape(value: &str) -> String {
        let mut buf = Vec::new();
        write_escaped(&mut buf, value, escape_xml_body).unwrap();
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
            write_escaped(&mut streamed, text, escape_xml_body).unwrap();
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
        write_escaped(&mut streamed, input, escape_xml_body).unwrap();
        assert_eq!(String::from_utf8(streamed).unwrap(), escaped.as_str());
    }

    #[test]
    fn test_write_xml_attr_escaped_preserves_whitespace_char_refs() {
        let input = "tab\tnewline\r\nquote\"amp&apos'<tag>";
        let mut out = Vec::new();
        write_escaped(&mut out, input, escape_xml_single_line).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "tab&#9;newline&#13;&#10;quote&quot;amp&amp;apos&apos;&lt;tag&gt;"
        );
    }

    #[test]
    fn renders_ascii_tree_correctly() -> Result<(), Box<dyn std::error::Error>> {
        let mut storage = FileStorage::new();
        storage.push("Cargo.toml", "");
        storage.push("src/core/a.rs", "");
        storage.push("src/lib.rs", "");
        let files = SortedFiles::new(storage);

        let mut buf = Vec::new();
        render_tree(&mut buf, &files, |w, s| w.write_all(s.as_bytes()))?;
        let tree_str = std::str::from_utf8(&buf)?;
        let expected = "\
├── Cargo.toml
└── src/
    ├── core/
    │   └── a.rs
    └── lib.rs
";
        assert_eq!(tree_str, expected);
        Ok(())
    }

    #[test]
    fn formats_tree_nodes_custom_formatter() -> std::io::Result<()> {
        let mut storage = FileStorage::new();
        storage.push("dir_name/file_name.txt", "");
        let files = SortedFiles::new(storage);

        let mut out = Vec::new();
        render_tree(&mut out, &files, |w, name| {
            w.write_all(name.to_uppercase().as_bytes())
        })?;
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("DIR_NAME/"));
        assert!(s.contains("FILE_NAME.TXT"));
        Ok(())
    }

    #[test]
    fn renders_nested_siblings_in_alphabetical_order() -> Result<(), Box<dyn std::error::Error>> {
        let mut storage = FileStorage::new();
        storage.push("a/x.txt", "");
        storage.push("a/y.txt", "");
        storage.push("b/z.txt", "");
        let files = SortedFiles::new(storage);

        let mut buf = Vec::new();
        render_tree(&mut buf, &files, |w, s| w.write_all(s.as_bytes()))?;
        let tree_str = std::str::from_utf8(&buf)?;
        let expected = "\
├── a/
│   ├── x.txt
│   └── y.txt
└── b/
    └── z.txt
";
        assert_eq!(tree_str, expected);
        Ok(())
    }

    #[test]
    fn component_at_offset_slices_correctly_and_handles_boundaries() {
        let path = "alpha/beta/gamma.rs";
        assert_eq!(component_at_offset(path, 0), Some(("alpha", true)));
        assert_eq!(component_at_offset(path, 6), Some(("beta", true)));
        assert_eq!(component_at_offset(path, 11), Some(("gamma.rs", false)));
        assert_eq!(component_at_offset(path, 19), None);
        assert_eq!(component_at_offset(path, 100), None);

        let single = "root_file.txt";
        assert_eq!(
            component_at_offset(single, 0),
            Some(("root_file.txt", false))
        );
        assert_eq!(component_at_offset(single, 13), None);
    }

    #[test]
    fn renders_deep_multi_level_tree_correctly() -> Result<(), Box<dyn std::error::Error>> {
        let mut storage = FileStorage::new();
        storage.push("a/b/c/d/e.txt", "");
        storage.push("a/b/c/f.txt", "");
        storage.push("a/b/g.txt", "");
        storage.push("root.txt", "");
        let files = SortedFiles::new(storage);

        let mut buf = Vec::new();
        render_tree(&mut buf, &files, |w, s| w.write_all(s.as_bytes()))?;
        let tree_str = std::str::from_utf8(&buf)?;
        let expected = "\
├── a/
│   └── b/
│       ├── c/
│       │   ├── d/
│       │   │   └── e.txt
│       │   └── f.txt
│       └── g.txt
└── root.txt
";
        assert_eq!(tree_str, expected);
        Ok(())
    }
}
