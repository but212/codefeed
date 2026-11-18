//! Digest rendering (text and XML), token estimation, and output size cap enforcement.

use crate::core::error::DomainError;
use crate::core::model::{CollectionResult, DigestFormat, RenderedDigest};
use crate::core::policy::MAX_OUTPUT_SIZE;

mod budget;
mod formats;
mod tree;

use budget::{BoundedBuffer, OutputLimitExceeded};
use formats::render_digest;

impl CollectionResult {
    /// Renders collected files into [`RenderedDigest`], enforcing [`MAX_OUTPUT_SIZE`](crate::core::policy::MAX_OUTPUT_SIZE) and estimating tokens.
    ///
    /// # Output Size Invariant
    ///
    /// 1. `FileStorage::push` checks raw arena bytes against `MAX_OUTPUT_SIZE` early.
    /// 2. [`BoundedBuffer`] enforces `MAX_OUTPUT_SIZE` during rendering.
    ///
    /// Soundness requires `rendered_bytes >= raw_bytes` (`files.total_bytes()`): text adds framing/headers/tree,
    /// and XML escapes or sanitizes bytes without shrinking length.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::OutputTooLarge`] if output exceeds `MAX_OUTPUT_SIZE`.
    pub(crate) fn render(&self, format: DigestFormat) -> Result<RenderedDigest, DomainError> {
        let extra = match format {
            DigestFormat::Text => self.files.len() * 128 + 4096,
            DigestFormat::Xml => self.files.total_bytes() / 16 + self.files.len() * 64 + 4096,
        };
        let capacity = self
            .files
            .total_bytes()
            .saturating_add(extra)
            .min(usize::try_from(MAX_OUTPUT_SIZE).unwrap_or(usize::MAX));
        let buf = BoundedBuffer::with_capacity(capacity);
        match render_digest(buf, self, format) {
            Ok(buf) => Ok(buf.finish()),
            Err(OutputLimitExceeded) => Err(self.files.output_too_large_error()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::error::DomainError;
    use crate::core::path::NormalizedPath;
    use crate::core::policy::MAX_OUTPUT_SIZE_USIZE;
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
            let storage: FileStorage = records.into_iter().collect();
            let raw_bytes = storage.total_bytes();

            let collection = CollectionResult {
                label: label.to_string(),
                revision: None,
                files: SortedFiles::new(storage)
                    .expect("test fixture has no file/directory collision"),
                skipped: 0,
            };

            for format in [DigestFormat::Text, DigestFormat::Xml] {
                let digest = collection.render(format)?;
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
    fn render_fails_at_every_write_position_within_its_limit() -> Result<(), DomainError> {
        // Every write (framing, header, tree, escaped body) must propagate a limit failure.
        let storage = [
            ("src/main.rs", "fn main() {}\r\n"),
            ("src/a&b/c.txt", "a < b & \"c\" 'd'\u{FFFE}\x0C tail"),
            ("README.md", "héllo 한글 🦀\n"),
        ]
        .into_iter()
        .try_fold(FileStorage::new(), |storage, (path, content)| {
            storage.push(&NormalizedPath::from(path), content)
        })?;
        let collection = CollectionResult {
            label: "repo".into(),
            revision: Some("abc123".into()),
            files: SortedFiles::new(storage).expect("test fixture has no file/directory collision"),
            skipped: 1,
        };

        for format in [DigestFormat::Text, DigestFormat::Xml] {
            let render_at = |limit: u64| {
                render_digest(BoundedBuffer::with_limit(0, limit), &collection, format)
            };
            let full = render_at(u64::MAX)
                .expect("unlimited render succeeds")
                .finish()
                .bytes;
            for limit in 0..full.len() as u64 {
                assert_eq!(
                    render_at(limit).err(),
                    Some(OutputLimitExceeded),
                    "{format:?} limit {limit}"
                );
            }
            let exact = render_at(full.len() as u64).expect("exact limit succeeds");
            assert_eq!(exact.finish().bytes, full, "{format:?}");
        }
        Ok(())
    }

    #[test]
    fn render_enforces_output_size_cap() -> Result<(), DomainError> {
        // Raw bytes fill the storage cap exactly, so `push` accepts them and render framing exceeds it.
        let path = "large.txt";
        let content = "x".repeat(MAX_OUTPUT_SIZE_USIZE - path.len());
        let storage = FileStorage::new().push(&NormalizedPath::from(path), &content)?;
        let collection = CollectionResult {
            label: "repo".into(),
            revision: None,
            files: SortedFiles::new(storage).expect("test fixture has no file/directory collision"),
            skipped: 0,
        };
        match collection.render(DigestFormat::Text) {
            Err(DomainError::OutputTooLarge(err)) => {
                assert_eq!(err.largest.len(), 1);
                assert_eq!(&*err.largest[0].0, "large.txt");
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
            files: SortedFiles::new(files.into_iter().collect())
                .expect("test fixture has no file/directory collision"),
            skipped: 0,
        };

        let text_digest = collection.render(DigestFormat::Text)?;
        let text_rendered = std::str::from_utf8(&text_digest.bytes)?;
        assert!(
            text_rendered.contains(raw_ansi),
            "Text format must preserve raw ANSI/control characters for LLM fidelity"
        );

        let xml_digest = collection.render(DigestFormat::Xml)?;
        let xml_str = std::str::from_utf8(&xml_digest.bytes)?;
        assert!(
            !xml_str.contains('\x1b'),
            "XML format must not emit raw ASCII ESC"
        );
        assert!(
            xml_str.contains("\u{FFFD}[31mRed Alert\u{FFFD}[0m\tTabbed\r\nNewline"),
            "XML format must sanitize ESC to replacement char while preserving valid whitespace"
        );

        Ok(())
    }
}
