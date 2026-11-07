//! In-memory bounded buffer and token estimation heuristic.

use crate::core::policy::ASCII_BYTES_PER_TOKEN;

/// Token estimation heuristic calculating ASCII bytes and non-ASCII character counts.
#[must_use]
pub(super) fn estimate_tokens_from_bytes(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }
    // Chunks of 4096 bytes keep the per-chunk `u32` counters far below overflow.
    let mut ascii_bytes = 0usize;
    let mut non_ascii_chars = 0usize;
    for chunk in bytes.chunks(4096) {
        let mut ascii = 0u32;
        let mut lead = 0u32;
        for &b in chunk {
            ascii += u32::from(b < 0x80);
            lead += u32::from(b >= 0xC0);
        }
        ascii_bytes += ascii as usize;
        non_ascii_chars += lead as usize;
    }

    let ascii_tokens = ascii_bytes / ASCII_BYTES_PER_TOKEN;
    let total = ascii_tokens + non_ascii_chars;
    total.max(1)
}

/// Error returned when rendered content exceeds the buffer's limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OutputLimitExceeded;

/// Rendered digest: the bytes to deliver and their token estimate.
#[derive(Debug)]
#[must_use]
pub(crate) struct RenderedDigest {
    pub(crate) bytes: Vec<u8>,
    pub(crate) estimated_tokens: usize,
}

/// In-memory buffer with value-passing operations, bounded by a byte `limit`.
///
/// Invariant I-B1 (`runtime`): `bytes.len() <= limit`. `push_bytes` is the only growth path and checks it.
/// Outside tests the limit is always [`MAX_OUTPUT_SIZE`](crate::core::policy::MAX_OUTPUT_SIZE): [`BoundedBuffer::with_capacity`]
/// is the only constructor, and the arbitrary-limit constructor exists only under `cfg(test)` so tests can sweep every boundary.
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq, Eq))]
#[must_use]
pub(crate) struct BoundedBuffer {
    bytes: Vec<u8>,
    limit: u64,
}

#[cfg(test)]
impl Default for BoundedBuffer {
    fn default() -> Self {
        Self::with_capacity(0)
    }
}

impl BoundedBuffer {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(capacity),
            limit: crate::core::policy::MAX_OUTPUT_SIZE,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_limit(capacity: usize, limit: u64) -> Self {
        Self {
            limit,
            ..Self::with_capacity(capacity)
        }
    }

    pub(crate) fn push_bytes(mut self, slice: &[u8]) -> Result<Self, OutputLimitExceeded> {
        let new_len = self.bytes.len() + slice.len();
        if new_len as u64 > self.limit {
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

    pub(crate) fn finish(self) -> RenderedDigest {
        let estimated_tokens = estimate_tokens_from_bytes(&self.bytes);
        RenderedDigest {
            bytes: self.bytes,
            estimated_tokens,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_buffer_limit_is_exact_at_every_boundary() {
        for limit in 0..=8u64 {
            let full = BoundedBuffer::with_limit(0, limit)
                .push_bytes(&vec![
                    b'x';
                    usize::try_from(limit).expect("limit fits in usize")
                ])
                .expect("content at the limit fits");
            assert_eq!(
                full.push_bytes(b"x").err(),
                Some(OutputLimitExceeded),
                "limit {limit}"
            );
        }
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
    fn estimate_tokens_matches_reference_definition() {
        // Reference: the per-byte definition from context.md sec. 3.5, written as a single loop.
        fn reference(bytes: &[u8]) -> usize {
            if bytes.is_empty() {
                return 0;
            }
            let mut ascii = 0usize;
            let mut lead = 0usize;
            for &b in bytes {
                if b < 0x80 {
                    ascii += 1;
                }
                if b >= 0xC0 {
                    lead += 1;
                }
            }
            (ascii / ASCII_BYTES_PER_TOKEN + lead).max(1)
        }

        let lengths = [
            1, 2, 3, 4, 5, 7, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 65, 254, 255, 256, 510, 511,
            512, 4095, 4096, 4097, 8191, 8192, 8193,
        ];
        for len in lengths {
            let cycled: Vec<u8> = (0..=255u8).cycle().take(len).collect();
            assert_eq!(
                estimate_tokens_from_bytes(&cycled),
                reference(&cycled),
                "cycled bytes, len {len}"
            );
        }
        for value in 0..=255u8 {
            for len in [1, 2, 3, 4, 5, 64, 4097] {
                let uniform = vec![value; len];
                assert_eq!(
                    estimate_tokens_from_bytes(&uniform),
                    reference(&uniform),
                    "byte {value:#04x}, len {len}"
                );
            }
        }
    }

    #[test]
    fn finish_reports_bytes_and_tokens() {
        let digest = BoundedBuffer::default()
            .push_str("test output")
            .expect("fixture fits the output limit")
            .finish();
        assert_eq!(digest.bytes, b"test output");
        assert_eq!(
            digest.estimated_tokens,
            estimate_tokens_from_bytes(b"test output")
        );
    }
}
