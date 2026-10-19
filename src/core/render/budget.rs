//! In-memory bounded buffer and token estimation heuristic.

use crate::core::policy::MAX_OUTPUT_SIZE;

/// Fast byte-level heuristic calibrated for English text and source code.
#[inline]
pub(crate) fn estimate_tokens(bytes: u64) -> usize {
    if bytes == 0 {
        0
    } else {
        (bytes / 4).max(1) as usize
    }
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

/// Rendered digest containing accumulated bytes and token estimation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenderedDigest {
    pub(crate) bytes: Vec<u8>,
    pub(crate) estimated_tokens: usize,
}

/// In-memory buffer strictly bounded by [`MAX_OUTPUT_SIZE`] with value-passing operations.
#[derive(Debug, Default, PartialEq, Eq)]
#[repr(transparent)]
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

    pub(crate) fn push_usize(self, mut val: usize) -> Result<Self, OutputLimitExceeded> {
        if val == 0 {
            return self.push_bytes(b"0");
        }
        let mut buf = [0u8; 32];
        let mut i = buf.len();
        while val > 0 {
            i -= 1;
            buf[i] = b'0' + (val % 10) as u8;
            val /= 10;
        }
        self.push_bytes(&buf[i..])
    }

    pub(crate) fn finish(self) -> RenderedDigest {
        let tokens = estimate_tokens(self.bytes.len() as u64);
        RenderedDigest {
            bytes: self.bytes,
            estimated_tokens: tokens,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_buffer_enforces_output_limit() {
        let buf = BoundedBuffer::default();
        let large = vec![b'x'; (MAX_OUTPUT_SIZE + 1) as usize];
        assert_eq!(buf.push_bytes(&large), Err(OutputLimitExceeded));
    }

    #[test]
    fn bounded_buffer_push_usize_formats_correctly() {
        let cases = [(0, "0"), (1, "1"), (42, "42"), (10_000, "10000")];
        for (val, expected) in cases {
            let buf = BoundedBuffer::default().push_usize(val).unwrap();
            let digest = buf.finish();
            assert_eq!(std::str::from_utf8(&digest.bytes).unwrap(), expected);
        }
    }

    #[test]
    fn estimate_tokens_boundaries() {
        let cases = [(0, 0), (1, 1), (3, 1), (4, 1), (5, 1), (8, 2)];
        for (bytes, expected) in cases {
            assert_eq!(
                estimate_tokens(bytes),
                expected,
                "token estimation mismatch for {bytes} bytes"
            );
        }
    }
}
