//! Digest rendering and streaming token estimation.
//!
//! Coordinates layout rendering across text and XML formats, streaming
//! token estimation, and output size cap enforcement.

use std::io::Write;

use crate::core::error::IngestError;
use crate::core::model::{CollectionResult, DigestFormat};
use crate::core::policy::MAX_OUTPUT_SIZE;
use crate::core::storage::{FileSpan, FileStorage};

const TREE_BRANCH: &str = "├── ";
const TREE_LAST: &str = "└── ";
const TREE_VERT: &str = "│   ";
const TREE_SPACE: &str = "    ";

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

/// Heuristic token estimation (~4 bytes per token for typical ASCII code).
///
/// Returns 0 for empty digests, and at least 1 token when non-empty.
///
/// Note: This is a fast byte-level heuristic calibrated for English text and source code.
/// Multibyte UTF-8 sequences (such as CJK characters, which consume 3 bytes each)
/// are underestimated by this formula.
#[inline]
fn estimate_tokens(bytes: u64) -> usize {
    if bytes == 0 {
        0
    } else {
        (bytes / 4).max(1) as usize
    }
}

impl DigestFormat {
    pub(crate) fn render<W: Write + ?Sized>(
        self,
        writer: &mut W,
        collection: &CollectionResult,
    ) -> std::io::Result<()> {
        match self {
            Self::Text => render_text(writer, collection),
            Self::Xml => render_xml(writer, collection),
        }
    }
}

struct BoundedEstimatingWriter<'a, W: Write + ?Sized> {
    inner: &'a mut W,
    written: u64,
}

impl<'a, W: Write + ?Sized> BoundedEstimatingWriter<'a, W> {
    fn new(inner: &'a mut W) -> Self {
        Self { inner, written: 0 }
    }

    fn finish(self) -> usize {
        estimate_tokens(self.written)
    }
}

impl<W: Write + ?Sized> Write for BoundedEstimatingWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let new_written = self.written.saturating_add(buf.len() as u64);
        if new_written > MAX_OUTPUT_SIZE {
            self.written = new_written;
            return Err(output_limit_io_error());
        }
        self.inner.write_all(buf)?;
        self.written = new_written;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn output_limit_io_error() -> std::io::Error {
    std::io::Error::other("output size limit exceeded")
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
        if estimating_writer.written > MAX_OUTPUT_SIZE {
            return Err(collection.output_too_large_error());
        }
        return Err(IngestError::Io(err));
    }
    Ok(estimating_writer.finish())
}

#[inline(always)]
fn escape_header(bytes: &[u8], i: usize) -> Option<(usize, &'static str)> {
    match bytes[i] {
        b'\\' => Some((1, "\\\\")),
        b'\r' => Some((1, "\\r")),
        b'\n' => Some((1, "\\n")),
        0x00..=0x08 | 0x0B | 0x0C | 0x0E..=0x1F | 0x7F => Some((1, "\u{FFFD}")),
        _ => None,
    }
}

fn render_text<W: Write + ?Sized>(
    writer: &mut W,
    collection: &CollectionResult,
) -> std::io::Result<()> {
    writer.write_all(REPOSITORY_PREFIX.as_bytes())?;
    write_escaped(writer, &collection.label, escape_header)?;
    writer.write_all(NEWLINE.as_bytes())?;
    if let Some(ref revision) = collection.revision {
        writer.write_all(REVISION_PREFIX.as_bytes())?;
        write_escaped(writer, revision, escape_header)?;
        writer.write_all(NEWLINE.as_bytes())?;
    }
    writer.write_all(FILES_PREFIX.as_bytes())?;
    write!(writer, "{}", collection.files.len())?;
    writer.write_all(SKIPPED_PREFIX.as_bytes())?;
    write!(writer, "{}", collection.skipped)?;
    writer.write_all(METADATA_SUFFIX.as_bytes())?;
    writer.write_all(DIRECTORY_HEADER.as_bytes())?;
    render_tree(writer, &collection.files, |w, name| {
        write_escaped(w, name, escape_header)
    })?;
    writer.write_all(FILES_HEADER.as_bytes())?;
    for file in collection.files.iter() {
        const TAIL: [&[u8]; 2] = [b"\n", b"\n\n"];
        let needs_extra_nl = (!file.content.ends_with('\n')) as usize;
        writer.write_all(FILE_BLOCK_PREFIX.as_bytes())?;
        write_escaped(writer, file.path, escape_header)?;
        writer.write_all(FILE_BLOCK_SUFFIX.as_bytes())?;
        writer.write_all(file.content.as_bytes())?;
        writer.write_all(TAIL[needs_extra_nl])?;
    }

    Ok(())
}

#[inline(always)]
fn escape_xml_body(bytes: &[u8], i: usize) -> Option<(usize, &'static str)> {
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
fn escape_xml_single_line(bytes: &[u8], i: usize) -> Option<(usize, &'static str)> {
    match bytes[i] {
        b'\t' => Some((1, "&#9;")),
        b'\n' => Some((1, "&#10;")),
        b'\r' => Some((1, "&#13;")),
        _ => escape_xml_body(bytes, i),
    }
}

fn write_escaped<W, F>(writer: &mut W, value: &str, special_at: F) -> std::io::Result<()>
where
    W: Write + ?Sized,
    F: Fn(&[u8], usize) -> Option<(usize, &'static str)>,
{
    let bytes = value.as_bytes();
    let mut last = 0;
    let mut i = 0;
    while i < bytes.len() {
        if let Some((consumed, esc)) = special_at(bytes, i) {
            writer.write_all(&bytes[last..i])?;
            writer.write_all(esc.as_bytes())?;
            i += consumed;
            last = i;
        } else {
            i += 1;
        }
    }
    writer.write_all(&bytes[last..])
}

fn render_xml<W: Write + ?Sized>(
    writer: &mut W,
    collection: &CollectionResult,
) -> std::io::Result<()> {
    writer.write_all(XML_REPO_START.as_bytes())?;
    write_escaped(writer, &collection.label, escape_xml_single_line)?;
    if let Some(ref revision) = collection.revision {
        writer.write_all(XML_REVISION_ATTR.as_bytes())?;
        write_escaped(writer, revision, escape_xml_single_line)?;
    }
    writer.write_all(XML_TAG_CLOSE.as_bytes())?;
    writer.write_all(XML_DIR_START.as_bytes())?;
    render_tree(writer, &collection.files, |w, name| {
        write_escaped(w, name, escape_xml_single_line)
    })?;
    writer.write_all(XML_DIR_END.as_bytes())?;
    writer.write_all(XML_FILES_START.as_bytes())?;
    for file in collection.files.iter() {
        writer.write_all(XML_FILE_START.as_bytes())?;
        write_escaped(writer, file.path, escape_xml_single_line)?;
        writer.write_all(XML_TAG_CLOSE.as_bytes())?;
        write_escaped(writer, file.content, escape_xml_body)?;
        let needs_nl = (!file.content.ends_with('\n')) as usize;
        writer.write_all(FILE_TAILS[needs_nl])?;
    }
    writer.write_all(XML_FILES_END.as_bytes())?;
    writer.write_all(XML_REPO_END.as_bytes())?;
    Ok(())
}

/// Renders an ASCII tree representation of the provided pre-sorted [`FileStorage`] to the writer.
fn render_tree<W: Write + ?Sized, F>(
    writer: &mut W,
    storage: &FileStorage,
    mut format_name: F,
) -> std::io::Result<()>
where
    F: FnMut(&mut W, &str) -> std::io::Result<()>,
{
    let mut prefix = String::new();
    render_level(
        writer,
        storage.spans(),
        storage.path_arena(),
        0,
        &mut prefix,
        &mut format_name,
    )
}

fn render_level<W: Write + ?Sized, F>(
    writer: &mut W,
    spans: &[FileSpan],
    path_arena: &str,
    offset: usize,
    prefix: &mut String,
    format_name: &mut F,
) -> std::io::Result<()>
where
    F: FnMut(&mut W, &str) -> std::io::Result<()>,
{
    let mut i = 0;
    while let Some((name, is_dir)) = spans
        .get(i)
        .and_then(|s| component_at_offset(s.path(path_arena), offset))
    {
        let start = i;
        i += 1;
        if is_dir {
            let dir_prefix = &spans[start].path(path_arena)[offset..offset + name.len() + 1];
            while spans
                .get(i)
                .and_then(|s| s.path(path_arena).get(offset..))
                .is_some_and(|rem| rem.starts_with(dir_prefix))
            {
                i += 1;
            }
        }
        let end = i;
        let is_last = end == spans.len();

        writer.write_all(prefix.as_bytes())?;
        writer.write_all(if is_last { TREE_LAST } else { TREE_BRANCH }.as_bytes())?;
        format_name(writer, name)?;

        if is_dir {
            writer.write_all(b"/\n")?;
            let prev_len = prefix.len();
            prefix.push_str(if is_last { TREE_SPACE } else { TREE_VERT });
            render_level(
                writer,
                &spans[start..end],
                path_arena,
                offset + name.len() + 1,
                prefix,
                format_name,
            )?;
            prefix.truncate(prev_len);
        } else {
            writer.write_all(b"\n")?;
        }
    }

    Ok(())
}

#[inline]
fn component_at_offset(path: &str, offset: usize) -> Option<(&str, bool)> {
    let remainder = path.get(offset..)?;
    if remainder.is_empty() {
        return None;
    }
    match remainder.find('/') {
        Some(idx) => Some((&remainder[..idx], true)),
        None => Some((remainder, false)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::error::IngestError;
    use crate::core::policy::MAX_OUTPUT_SIZE;
    use crate::core::storage::{FileRecord, FileStorage};

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
            files: storage,
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
            files: storage,
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
            files: storage,
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
            files: storage,
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
            files: storage,
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
            files: files.into_iter().collect(),
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
            files: storage,
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
    fn escapes_control_characters_in_filename_headers_and_tree() -> Result<(), Box<dyn std::error::Error>> {
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
        storage.sort_by_path();

        let mut buf = Vec::new();
        render_tree(&mut buf, &storage, |w, s| w.write_all(s.as_bytes()))?;
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

        let mut out = Vec::new();
        render_tree(&mut out, &storage, |w, name| {
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
        storage.sort_by_path();

        let mut buf = Vec::new();
        render_tree(&mut buf, &storage, |w, s| w.write_all(s.as_bytes()))?;
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
        storage.sort_by_path();

        let mut buf = Vec::new();
        render_tree(&mut buf, &storage, |w, s| w.write_all(s.as_bytes()))?;
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
