//! In-memory bounded buffer and token estimation heuristic.

use crate::core::model::{IngestReport, OutputTarget};
use crate::core::policy::{ASCII_BYTES_PER_TOKEN, MAX_OUTPUT_SIZE};

/// Token estimation heuristic calculating ASCII bytes and non-ASCII character counts.
#[must_use]
pub(super) const fn estimate_tokens_from_bytes(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }
    let mut ascii_bytes = 0usize;
    let mut non_ascii_chars = 0usize;
    let mut i = 0;

    while i < bytes.len() {
        let b = bytes[i];
        if b < 0x80 {
            ascii_bytes += 1;
        } else if b >= 0xC0 {
            non_ascii_chars += 1;
        }
        i += 1;
    }

    let ascii_tokens = ascii_bytes / ASCII_BYTES_PER_TOKEN;
    let total = ascii_tokens + non_ascii_chars;
    if total > 1 { total } else { 1 }
}

/// Error returned when rendered content exceeds [`MAX_OUTPUT_SIZE`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OutputLimitExceeded;

impl std::fmt::Display for OutputLimitExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "output size limit exceeded")
    }
}

impl std::error::Error for OutputLimitExceeded {}

/// Rendered digest containing accumulated bytes, token estimation, and collection metrics.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub(crate) struct RenderedDigest {
    pub(crate) bytes: Vec<u8>,
    pub(crate) estimated_tokens: usize,
    pub(crate) included_files: usize,
    pub(crate) skipped_files: usize,
}

impl RenderedDigest {
    /// Consumes the rendered digest to generate an [`IngestReport`] for the given output target.
    pub(crate) fn into_report(self, output: OutputTarget) -> IngestReport {
        IngestReport {
            output,
            included_files: self.included_files,
            skipped_files: self.skipped_files,
            estimated_tokens: self.estimated_tokens,
        }
    }
}

/// In-memory buffer strictly bounded by [`MAX_OUTPUT_SIZE`] with value-passing operations.
#[derive(Debug, Default, PartialEq, Eq)]
#[repr(transparent)]
#[must_use]
pub(crate) struct BoundedBuffer {
    bytes: Vec<u8>,
}

impl BoundedBuffer {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(capacity),
        }
    }

    pub(crate) fn push_bytes(mut self, slice: &[u8]) -> Result<Self, OutputLimitExceeded> {
        let new_len = self.bytes.len().saturating_add(slice.len());
        if new_len as u64 > MAX_OUTPUT_SIZE {
            return Err(OutputLimitExceeded);
        }
        self.bytes.extend_from_slice(slice);
        Ok(self)
    }

    pub(crate) fn push_str(self, s: &str) -> Result<Self, OutputLimitExceeded> {
        self.push_bytes(s.as_bytes())
    }

    pub(crate) fn push_usize(self, val: usize) -> Result<Self, OutputLimitExceeded> {
        self.push_str(&val.to_string())
    }

    pub(crate) fn finish_with_counts(
        self,
        included_files: usize,
        skipped_files: usize,
    ) -> RenderedDigest {
        let tokens = estimate_tokens_from_bytes(&self.bytes);
        RenderedDigest {
            bytes: self.bytes,
            estimated_tokens: tokens,
            included_files,
            skipped_files,
        }
    }

    #[cfg(test)]
    pub(crate) fn finish(self) -> RenderedDigest {
        self.finish_with_counts(0, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::policy::MAX_OUTPUT_SIZE_USIZE;

    #[test]
    fn bounded_buffer_enforces_output_limit() {
        let buf = BoundedBuffer::default();
        let large = vec![b'x'; MAX_OUTPUT_SIZE_USIZE + 1];
        assert_eq!(buf.push_bytes(&large), Err(OutputLimitExceeded));
    }

    #[test]
    fn bounded_buffer_push_usize_formats_correctly() {
        let cases = [0, 1, 42, 10_000, usize::MAX];
        for val in cases {
            let buf = BoundedBuffer::default().push_usize(val).unwrap();
            let digest = buf.finish();
            assert_eq!(std::str::from_utf8(&digest.bytes).unwrap(), val.to_string());
        }
    }

    #[test]
    fn estimate_tokens_boundaries() {
        assert_eq!(estimate_tokens_from_bytes(b""), 0);
        assert_eq!(estimate_tokens_from_bytes(b"a"), 1);
        assert_eq!(estimate_tokens_from_bytes(b"abc"), 1);
        assert_eq!(estimate_tokens_from_bytes(b"abcd"), 1);
        assert_eq!(estimate_tokens_from_bytes(b"abcde"), 1);
        assert_eq!(estimate_tokens_from_bytes(b"abcdefgh"), 2);

        // Korean text "안녕하세요" (5 characters, 15 UTF-8 bytes) -> 5 tokens
        assert_eq!(estimate_tokens_from_bytes("안녕하세요".as_bytes()), 5);

        // Mixed ASCII + Korean
        // "Hello 안녕하세요" -> 6 ASCII bytes ('H','e','l','l','o',' ') => 6/4 = 1 token
        // 5 Korean characters => 5 tokens
        // Total = 6 tokens
        assert_eq!(estimate_tokens_from_bytes("Hello 안녕하세요".as_bytes()), 6);
    }

    #[test]
    fn rendered_digest_into_report_preserves_metrics() {
        use crate::core::model::OutputTarget;

        let digest = RenderedDigest {
            bytes: b"test output".to_vec(),
            estimated_tokens: 42,
            included_files: 10,
            skipped_files: 3,
        };
        let report = digest.into_report(OutputTarget::Stdout);
        assert_eq!(report.output, OutputTarget::Stdout);
        assert_eq!(report.included_files, 10);
        assert_eq!(report.skipped_files, 3);
        assert_eq!(report.estimated_tokens, 42);
    }
}
