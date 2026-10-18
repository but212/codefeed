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
