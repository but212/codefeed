use super::budget::{BoundedBuffer, OutputLimitExceeded};
use super::escape::{escape_header, escape_xml_body, escape_xml_single_line, write_escaped};

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

/// Rendering strategy encapsulating format-specific framing, headers, and entity escaping.
pub(crate) trait FormatRenderer: Copy {
    fn write_header(
        self,
        buf: BoundedBuffer,
        label: &str,
        revision: Option<&str>,
        file_count: usize,
        skipped: usize,
    ) -> Result<BoundedBuffer, OutputLimitExceeded>;

    fn write_tree_name(
        self,
        buf: BoundedBuffer,
        name: &str,
    ) -> Result<BoundedBuffer, OutputLimitExceeded>;

    fn close_tree_section(self, buf: BoundedBuffer) -> Result<BoundedBuffer, OutputLimitExceeded>;

    fn write_file_entry(
        self,
        buf: BoundedBuffer,
        path: &str,
        content: &str,
    ) -> Result<BoundedBuffer, OutputLimitExceeded>;

    fn write_footer(self, buf: BoundedBuffer) -> Result<BoundedBuffer, OutputLimitExceeded>;
}

/// Plaintext delimiter-separated LLM digest renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextRenderer;

impl FormatRenderer for TextRenderer {
    fn write_header(
        self,
        mut buf: BoundedBuffer,
        label: &str,
        revision: Option<&str>,
        file_count: usize,
        skipped: usize,
    ) -> Result<BoundedBuffer, OutputLimitExceeded> {
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

    fn write_tree_name(
        self,
        buf: BoundedBuffer,
        name: &str,
    ) -> Result<BoundedBuffer, OutputLimitExceeded> {
        write_escaped(buf, name, escape_header)
    }

    fn close_tree_section(self, buf: BoundedBuffer) -> Result<BoundedBuffer, OutputLimitExceeded> {
        buf.push_bytes(FILES_HEADER.as_bytes())
    }

    fn write_file_entry(
        self,
        mut buf: BoundedBuffer,
        path: &str,
        content: &str,
    ) -> Result<BoundedBuffer, OutputLimitExceeded> {
        const TAIL: [&[u8]; 2] = [b"\n", b"\n\n"];
        let needs_extra_nl = usize::from(!content.ends_with('\n'));
        buf = buf.push_bytes(FILE_BLOCK_PREFIX.as_bytes())?;
        buf = write_escaped(buf, path, escape_header)?;
        buf = buf.push_bytes(FILE_BLOCK_SUFFIX.as_bytes())?;
        buf = buf.push_bytes(content.as_bytes())?;
        buf = buf.push_bytes(TAIL[needs_extra_nl])?;
        Ok(buf)
    }

    fn write_footer(self, buf: BoundedBuffer) -> Result<BoundedBuffer, OutputLimitExceeded> {
        Ok(buf)
    }
}

/// Anthropic and LLM-optimized XML structured digest renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct XmlRenderer;

impl FormatRenderer for XmlRenderer {
    fn write_header(
        self,
        mut buf: BoundedBuffer,
        label: &str,
        revision: Option<&str>,
        _file_count: usize,
        _skipped: usize,
    ) -> Result<BoundedBuffer, OutputLimitExceeded> {
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

    fn write_tree_name(
        self,
        buf: BoundedBuffer,
        name: &str,
    ) -> Result<BoundedBuffer, OutputLimitExceeded> {
        write_escaped(buf, name, escape_xml_single_line)
    }

    fn close_tree_section(
        self,
        mut buf: BoundedBuffer,
    ) -> Result<BoundedBuffer, OutputLimitExceeded> {
        buf = buf.push_bytes(XML_DIR_END.as_bytes())?;
        buf = buf.push_bytes(XML_FILES_START.as_bytes())?;
        Ok(buf)
    }

    fn write_file_entry(
        self,
        mut buf: BoundedBuffer,
        path: &str,
        content: &str,
    ) -> Result<BoundedBuffer, OutputLimitExceeded> {
        buf = buf.push_bytes(XML_FILE_START.as_bytes())?;
        buf = write_escaped(buf, path, escape_xml_single_line)?;
        buf = buf.push_bytes(XML_TAG_CLOSE.as_bytes())?;
        buf = write_escaped(buf, content, escape_xml_body)?;
        let needs_nl = usize::from(!content.ends_with('\n'));
        buf = buf.push_bytes(FILE_TAILS[needs_nl])?;
        Ok(buf)
    }

    fn write_footer(self, mut buf: BoundedBuffer) -> Result<BoundedBuffer, OutputLimitExceeded> {
        buf = buf.push_bytes(XML_FILES_END.as_bytes())?;
        buf = buf.push_bytes(XML_REPO_END.as_bytes())?;
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::CollectionResult;
    use crate::core::render::render_with;
    use crate::core::storage::{FileRecord, FileStorage, SortedFiles};

    fn render_digest<R: FormatRenderer>(
        renderer: R,
        label: &str,
        revision: Option<&str>,
        files: &[FileRecord<'_>],
        skipped: usize,
    ) -> Result<String, OutputLimitExceeded> {
        let storage: FileStorage = files.iter().copied().collect();
        let collection = CollectionResult {
            label: label.to_string(),
            revision: revision.map(str::to_string),
            files: SortedFiles::new(storage),
            skipped,
        };
        let buf = render_with(BoundedBuffer::default(), &collection, renderer)?;
        Ok(String::from_utf8(buf.finish().bytes).unwrap())
    }

    fn render_text_digest(
        label: &str,
        revision: Option<&str>,
        files: &[FileRecord<'_>],
        skipped: usize,
    ) -> Result<String, OutputLimitExceeded> {
        render_digest(TextRenderer, label, revision, files, skipped)
    }

    fn render_xml_digest(
        label: &str,
        revision: Option<&str>,
        files: &[FileRecord<'_>],
        skipped: usize,
    ) -> Result<String, OutputLimitExceeded> {
        render_digest(XmlRenderer, label, revision, files, skipped)
    }

    #[test]
    fn text_renders_files_in_deterministic_order() -> Result<(), Box<dyn std::error::Error>> {
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
        let digest = render_text_digest("repo", Some("abc"), &files, 1)?;
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
    fn text_escapes_revision_header() -> Result<(), Box<dyn std::error::Error>> {
        let revision = "feature\\x\r\nFiles: 9";
        let rendered = render_text_digest("repo", Some(revision), &[], 0)?;

        assert_eq!(
            rendered.lines().nth(1),
            Some("Revision: feature\\\\x\\r\\nFiles: 9")
        );
        assert!(!rendered.contains(revision));
        Ok(())
    }

    #[test]
    fn text_streaming_matches_string_render() -> Result<(), Box<dyn std::error::Error>> {
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
            files: SortedFiles::new(storage),
            skipped: 2,
        };
        let buf = render_with(BoundedBuffer::default(), &collection, TextRenderer)?;
        let digest = buf.finish();
        let streamed_str = std::str::from_utf8(&digest.bytes)?;

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
    fn text_renders_from_arbitrary_iterator() -> Result<(), Box<dyn std::error::Error>> {
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
        let storage = files.into_iter().filter(|f| f.path != "excluded.txt").fold(
            FileStorage::new(),
            |s, f| {
                s.push(
                    &crate::core::policy::NormalizedPath::from(f.path),
                    f.content,
                )
                .expect("test fixture fits the output limit")
            },
        );
        let collection = CollectionResult {
            label: "repo".into(),
            revision: None,
            files: SortedFiles::new(storage),
            skipped: 1,
        };
        let buf = render_with(BoundedBuffer::default(), &collection, TextRenderer)?;
        let digest = buf.finish();
        let rendered = std::str::from_utf8(&digest.bytes)?;
        assert!(rendered.contains("Files: 2"));
        assert!(rendered.contains("FILE: a.txt"));
        assert!(rendered.contains("FILE: b.txt"));
        assert!(!rendered.contains("excluded.txt"));
        Ok(())
    }

    #[test]
    fn text_escapes_control_characters_in_filename_headers_and_tree()
    -> Result<(), Box<dyn std::error::Error>> {
        let filename = "x\x1b]0;title\x07.md";
        let files = [FileRecord {
            path: filename,
            content: "content\n",
        }];
        let text_rendered = render_text_digest("repo", None, &files, 0)?;
        assert!(!text_rendered.contains('\x1b'));
        assert!(!text_rendered.contains('\x07'));
        assert!(text_rendered.contains("x\u{FFFD}]0;title\u{FFFD}.md"));
        Ok(())
    }

    #[test]
    fn xml_renders_in_deterministic_order() -> Result<(), Box<dyn std::error::Error>> {
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
        let xml_str = render_xml_digest("my-repo", Some("v1.0"), &files, 1)?;
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
    fn xml_attr_escapes_whitespace_and_preserves_body() -> Result<(), Box<dyn std::error::Error>> {
        let files = [FileRecord {
            path: "dir\nname\r\t\\back/file.rs",
            content: "fn main() {\n\tprintln!(\"hello\");\r\n}\n",
        }];
        let xml_str = render_xml_digest("my\trepo\n", Some("rev\r\n1"), &files, 0)?;

        assert!(
            xml_str.contains("<repository name=\"my&#9;repo&#10;\" revision=\"rev&#13;&#10;1\">")
        );
        assert!(xml_str.contains("<file path=\"dir&#10;name&#13;&#9;\\back/file.rs\">"));
        assert!(xml_str.contains("dir&#10;name&#13;&#9;\\back/"));
        assert!(xml_str.contains("fn main() {\n\tprintln!(&quot;hello&quot;);\r\n}\n"));
        Ok(())
    }
}
