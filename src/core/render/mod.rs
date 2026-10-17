//! Digest rendering and streaming token estimation.
//!
//! Coordinates layout rendering across text and XML formats, streaming
//! token estimation, and output size cap enforcement.

use std::io::Write;

pub(crate) mod text;
pub(crate) mod xml;

use crate::core::error::IngestError;
use crate::core::model::{CollectionResult, DigestFormat, FileRecord};
use crate::core::policy::MAX_OUTPUT_SIZE;

#[inline]
pub fn estimate_tokens(bytes: u64) -> usize {
    if bytes == 0 {
        0
    } else {
        (bytes / 4).max(1) as usize
    }
}

impl DigestFormat {
    pub(crate) fn render<'a, W, I>(
        self,
        writer: &mut W,
        label: &str,
        revision: Option<&str>,
        files: I,
        skipped: usize,
        paths: &[&str],
    ) -> Result<(), IngestError>
    where
        W: Write + ?Sized,
        I: IntoIterator<Item = FileRecord<'a>>,
        I::IntoIter: ExactSizeIterator,
    {
        match self {
            Self::Text => text::render_text(writer, label, revision, files, skipped, paths),
            Self::Xml => xml::render_xml(writer, label, revision, files, paths),
        }
    }
}

struct BoundedEstimatingWriter<'a, W: Write + ?Sized> {
    inner: &'a mut W,
    written: u64,
    overflowed: bool,
}

impl<'a, W: Write + ?Sized> BoundedEstimatingWriter<'a, W> {
    fn new(inner: &'a mut W) -> Self {
        Self {
            inner,
            written: 0,
            overflowed: false,
        }
    }

    fn finish(self) -> std::io::Result<usize> {
        Ok(estimate_tokens(self.written))
    }
}

impl<W: Write + ?Sized> Write for BoundedEstimatingWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let new_written = self.written.saturating_add(buf.len() as u64);
        if new_written > MAX_OUTPUT_SIZE {
            self.overflowed = true;
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

pub fn render_to_writer<W: Write + ?Sized>(
    writer: &mut W,
    collection: &CollectionResult,
    format: DigestFormat,
) -> Result<usize, IngestError> {
    let mut estimating_writer = BoundedEstimatingWriter::new(writer);
    let paths: Vec<&str> = collection.files.iter().map(|f| f.path).collect();
    let render_res = format.render(
        &mut estimating_writer,
        &collection.label,
        collection.revision.as_deref(),
        collection.files.iter(),
        collection.skipped,
        &paths,
    );

    if estimating_writer.overflowed {
        return Err(output_too_large_error(collection));
    }

    render_res?;
    estimating_writer.finish().map_err(Into::into)
}

fn output_too_large_error(collection: &CollectionResult) -> IngestError {
    IngestError::output_too_large(crate::core::selector::largest_files(
        collection.files.iter(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::error::IngestError;
    use crate::core::model::{FileRecord, FileStorage};
    use crate::core::policy::MAX_OUTPUT_SIZE;

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
        let mut buf = Vec::new();
        let filtered: Vec<_> = files
            .into_iter()
            .filter(|f| f.path != "excluded.txt")
            .collect();
        DigestFormat::Text.render(&mut buf, "repo", None, filtered, 1, &["a.txt", "b.txt"])?;
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
        let tokens = writer
            .finish()
            .expect("finish must succeed even if inner flush fails");
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
}
