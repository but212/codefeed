//! Command-line interface execution and entry point.
//!
//! Parses CLI arguments, formats completion summaries or error messages,
//! and determines process exit codes.

use std::fmt::Display;

use crate::core::cli::{CliAction, HELP, VERSION, completion_message, parse};
use crate::core::model::OutputTarget;
use crate::shell::{ingest, stdio};

/// Executes the command-line interface entry point.
///
/// Parses arguments, renders help text or performs ingestion, formats user-facing
/// status messages, and returns the appropriate process exit code (`0` on success, `1` on error).
pub fn run_cli<I, T>(args: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString>,
{
    let action = match parse(args) {
        Ok(action) => action,
        Err(error) => return report_error(&error),
    };

    let config = match action {
        CliAction::Help => {
            stdio::write_stdout_line(HELP);
            return 0;
        }
        CliAction::Version => {
            stdio::write_stdout_line(VERSION);
            return 0;
        }
        CliAction::Run(config) => config,
    };

    let report = match ingest(&config) {
        Ok(report) => report,
        Err(error) => return report_error(&error),
    };
    let message = completion_message(&report);
    match &report.output {
        OutputTarget::File(_) => stdio::write_stdout_line(&message),
        OutputTarget::Stdout => stdio::write_stderr_line(&message),
    }
    0
}

/// Writes one `error:` line to stderr and returns the failure exit code.
fn report_error(error: &impl Display) -> i32 {
    stdio::write_stderr_line(&format!("error: {error}"));
    1
}
