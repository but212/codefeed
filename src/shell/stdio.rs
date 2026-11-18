//! Line writers for stdout and stderr. A closed stream is not an error for these writers.

use std::io::{self, Write};

/// Writes one line to stdout. A closed stdout is not an error, so the failure is dropped.
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
