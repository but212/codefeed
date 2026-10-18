//! Bounded writer and token estimation heuristic.

use crate::core::policy::MAX_OUTPUT_SIZE;
use std::io::Write;

/// Fast byte-level heuristic calibrated for English text and source code.
#[inline]
pub(crate) fn estimate_tokens(bytes: u64) -> usize {
    if bytes == 0 {
        0
    } else {
        (bytes / 4).max(1) as usize
    }
}

pub(crate) struct BoundedEstimatingWriter<'a, W: Write + ?Sized> {
    inner: &'a mut W,
    written: u64,
}

impl<'a, W: Write + ?Sized> BoundedEstimatingWriter<'a, W> {
    pub(crate) fn new(inner: &'a mut W) -> Self {
        Self { inner, written: 0 }
    }

    pub(crate) fn written(&self) -> u64 {
        self.written
    }

    pub(crate) fn finish(self) -> usize {
        estimate_tokens(self.written)
    }
}

impl<W: Write + ?Sized> Write for BoundedEstimatingWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let new_written = self.written.saturating_add(buf.len() as u64);
        if new_written > MAX_OUTPUT_SIZE {
            self.written = new_written;
            return Err(std::io::Error::other("output size limit exceeded"));
        }
        self.inner.write_all(buf)?;
        self.written = new_written;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
