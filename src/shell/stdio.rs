//! Diagnostic lines on stdout and stderr. A closed pipe or stream is not an error for these writers.

use std::io::{self, Write};

pub(super) fn ignore_broken_pipe(result: io::Result<()>) -> io::Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(e),
    }
}

pub(super) fn write_stdout_line(line: &str) {
    let stdout = io::stdout();
    let mut lock = stdout.lock();
    let _ = writeln!(lock, "{line}");
}

/// Writes one line to stderr. A closed stderr is not an error, so the failure is dropped.
pub(super) fn write_stderr_line(line: &str) {
    let stderr = io::stderr();
    let mut lock = stderr.lock();
    let _ = writeln!(lock, "{line}");
}

/// Writer whose every operation reports a closed pipe.
#[cfg(test)]
pub(super) struct BrokenPipeWriter;

#[cfg(test)]
impl io::Write for BrokenPipeWriter {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        Err(io::Error::from(io::ErrorKind::BrokenPipe))
    }
    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::from(io::ErrorKind::BrokenPipe))
    }
}
