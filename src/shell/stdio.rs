//! Diagnostic lines on stderr. A closed stream is not an error for this writer.

use std::io::{self, Write};

/// Writes one line to stderr. A closed stderr is not an error, so the failure is dropped.
pub(super) fn write_stderr_line(line: &str) {
    let stderr = io::stderr();
    let mut lock = stderr.lock();
    let _ = writeln!(lock, "{line}");
}
