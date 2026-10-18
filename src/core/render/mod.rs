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
/// # Output Size Invariant
///
/// Final output size bounding relies on a two-stage guarantee:
/// 1. `Selector::add_file` performs early-fail bounding against `MAX_OUTPUT_SIZE` using raw arena byte totals.
/// 2. `render_to_writer` via [`BoundedEstimatingWriter`] enforces `MAX_OUTPUT_SIZE` during final streaming rendering.
///
/// For early-fail checks to be sound (never rejecting a collection that would pass final rendering),
/// the invariant `rendered_bytes >= raw_bytes` (`files.total_bytes()`) must hold for both Text and XML formats.
/// Text rendering appends framing, headers, and tree structure while preserving raw content bytes.
/// XML rendering escapes entities and sanitizes control characters, which strictly increases or preserves length.
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
    use crate::core::storage::{FileRecord, FileStorage, SortedFiles};

    #[test]
    fn rendered_size_is_at_least_raw_stored_bytes() -> Result<(), Box<dyn std::error::Error>> {
        let test_cases = [
            ("empty", vec![]),
            (
                "single_plain",
                vec![FileRecord {
                    path: "main.rs",
                    content: "fn main() {}",
                }],
            ),
            (
                "xml_special",
                vec![FileRecord {
                    path: "sub/test.xml",
                    content: "a & b < c > d \"quote\" 'single'",
                }],
            ),
            (
                "control_chars",
                vec![FileRecord {
                    path: "ctrl.txt",
                    content: "\x00\x01\x08\x0B\x0C\x0E\x1F",
                }],
            ),
        ];

        for (label, records) in test_cases {
            let mut storage = FileStorage::new();
            for r in records {
                storage.push(r.path, r.content);
            }
            let raw_bytes = storage.total_bytes() as usize;

            let collection = CollectionResult {
                label: label.to_string(),
                revision: None,
                files: SortedFiles::new(storage),
                skipped: 0,
            };

            for format in [DigestFormat::Text, DigestFormat::Xml] {
                let mut buf = Vec::new();
                render_to_writer(&mut buf, &collection, format)?;
                assert!(
                    buf.len() >= raw_bytes,
                    "Format {format:?} output length ({}) must be >= raw arena bytes ({raw_bytes}) for {label}",
                    buf.len()
                );
            }
        }
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
            files: SortedFiles::new(storage),
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
    fn control_character_policy_preserves_raw_text_and_sanitizes_xml()
    -> Result<(), Box<dyn std::error::Error>> {
        let raw_ansi = "\x1b[31mRed Alert\x1b[0m\tTabbed\r\nNewline";
        let files = [FileRecord {
            path: "ansi.txt",
            content: raw_ansi,
        }];

        let collection = CollectionResult {
            label: "repo".into(),
            revision: None,
            files: SortedFiles::new(files.into_iter().collect()),
            skipped: 0,
        };

        let mut text_out = Vec::new();
        render_to_writer(&mut text_out, &collection, DigestFormat::Text)?;
        let text_rendered = std::str::from_utf8(&text_out)?;
        assert!(
            text_rendered.contains(raw_ansi),
            "Text format must preserve raw ANSI/control characters for LLM fidelity"
        );

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
}
