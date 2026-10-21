//! Digest rendering and token estimation.
//!
//! Coordinates layout rendering across text and XML formats,
//! token estimation, and output size cap enforcement.

use crate::core::error::DomainError;
use crate::core::model::{CollectionResult, DigestFormat};

mod budget;
mod escape;
mod formats;
mod tree;

pub(crate) use budget::{BoundedBuffer, OutputLimitExceeded, RenderedDigest};
pub(crate) use formats::{FormatRenderer, TextRenderer, XmlRenderer};

pub(crate) fn render_with<R: FormatRenderer>(
    mut buf: BoundedBuffer,
    collection: &CollectionResult,
    renderer: R,
) -> Result<BoundedBuffer, OutputLimitExceeded> {
    buf = renderer.write_header(
        buf,
        &collection.label,
        collection.revision.as_deref(),
        collection.files.len(),
        collection.skipped,
    )?;
    buf = tree::render_tree(buf, &collection.files, |b, name| {
        renderer.write_tree_name(b, name)
    })?;
    buf = renderer.close_tree_section(buf)?;
    for file in collection.files.iter() {
        buf = renderer.write_file_entry(buf, file.path, file.content)?;
    }
    renderer.write_footer(buf)
}

impl CollectionResult {
    /// Renders the collected files into [`RenderedDigest`] using the specified format.
    ///
    /// Binds output generation to [`MAX_OUTPUT_SIZE`] and calculates the estimated token count.
    /// Consumes `self` as part of the linear ingestion pipeline.
    ///
    /// # Output Size Invariant
    ///
    /// Final output size bounding relies on a two-stage guarantee:
    /// 1. `FileStorage::push` performs early-fail bounding against `MAX_OUTPUT_SIZE` using raw arena byte totals.
    /// 2. `render` via [`BoundedBuffer`] enforces `MAX_OUTPUT_SIZE` during rendering passes.
    ///
    /// For early-fail checks to be sound (never rejecting a collection that would pass final rendering),
    /// the invariant `rendered_bytes >= raw_bytes` (`files.total_bytes()`) must hold for both Text and XML formats.
    /// Text rendering appends framing, headers, and tree structure while preserving raw content bytes.
    /// XML rendering escapes entities and sanitizes control characters, which strictly increases or preserves length.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::OutputTooLarge`] if output exceeds the maximum size limit.
    pub(crate) fn render(self, format: DigestFormat) -> Result<RenderedDigest, DomainError> {
        let capacity = (self.files.total_bytes() as usize).saturating_add(4096);
        let buf = BoundedBuffer::with_capacity(capacity);
        let included = self.files.len();
        let skipped = self.skipped;
        let rendered = match format {
            DigestFormat::Text => render_with(buf, &self, TextRenderer),
            DigestFormat::Xml => render_with(buf, &self, XmlRenderer),
        };
        match rendered {
            Ok(buf) => Ok(buf.finish_with_counts(included, skipped)),
            Err(OutputLimitExceeded) => Err(self.files.output_too_large_error()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::error::DomainError;
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
            let storage = records.into_iter().fold(FileStorage::new(), |s, r| {
                s.push(
                    &crate::core::policy::NormalizedPath::from(r.path),
                    r.content,
                )
                .expect("test fixture fits the output limit")
            });
            let raw_bytes = storage.total_bytes() as usize;

            let collection = CollectionResult {
                label: label.to_string(),
                revision: None,
                files: SortedFiles::new(storage),
                skipped: 0,
            };

            for format in [DigestFormat::Text, DigestFormat::Xml] {
                let digest = collection.clone().render(format)?;
                assert!(
                    digest.bytes.len() >= raw_bytes,
                    "Format {format:?} output length ({}) must be >= raw arena bytes ({raw_bytes}) for {label}",
                    digest.bytes.len()
                );
            }
        }
        Ok(())
    }

    #[test]
    fn render_enforces_output_size_cap() -> Result<(), DomainError> {
        // Raw bytes fill the storage cap exactly, so `push` accepts them and render framing exceeds it.
        let path = "large.txt";
        let content = "x".repeat(MAX_OUTPUT_SIZE as usize - path.len());
        let storage =
            FileStorage::new().push(&crate::core::policy::NormalizedPath::from(path), &content)?;
        let collection = CollectionResult {
            label: "repo".into(),
            revision: None,
            files: SortedFiles::new(storage),
            skipped: 0,
        };
        match collection.render(DigestFormat::Text) {
            Err(DomainError::OutputTooLarge(largest_files)) => {
                assert_eq!(largest_files.len(), 1);
                assert_eq!(largest_files[0].0, "large.txt");
            }
            other => panic!("expected OutputTooLarge, got {other:?}"),
        }
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

        let collection = CollectionResult {
            label: "repo".into(),
            revision: None,
            files: SortedFiles::new(files.into_iter().collect()),
            skipped: 0,
        };

        let text_digest = collection.clone().render(DigestFormat::Text)?;
        let text_rendered = std::str::from_utf8(&text_digest.bytes)?;
        assert!(
            text_rendered.contains(raw_ansi),
            "Text format must preserve raw ANSI/control characters for LLM fidelity"
        );

        let xml_digest = collection.render(DigestFormat::Xml)?;
        let xml_str = std::str::from_utf8(&xml_digest.bytes)?;
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
