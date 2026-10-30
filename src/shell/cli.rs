//! Command-line interface execution and entry point.
//!
//! Parses CLI arguments, formats completion summaries or error messages,
//! and determines process exit codes.

use std::fmt::Display;

use crate::core::cli::{CliAction, HELP, completion_message, parse};
use crate::shell::output;
use crate::{OutputTarget, ingest};

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
            output::write_stdout_line(HELP);
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
        OutputTarget::File(_) => output::write_stdout_line(&message),
        OutputTarget::Stdout => output::write_stderr_line(&message),
    }
    0
}

/// Writes one `error:` line to stderr and returns the failure exit code.
fn report_error(error: &impl Display) -> i32 {
    output::write_stderr_line(&format!("error: {error}"));
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::testing;

    #[test]
    fn run_cli_returns_expected_exit_codes() {
        // Parsing error returns 1
        let exit_code = run_cli(["codefeed", "--invalid-flag"]);
        assert_eq!(exit_code, 1);

        // --help returns 0
        let exit_code = run_cli(["codefeed", "--help"]);
        assert_eq!(exit_code, 0);

        // Non-existent local path returns 1
        let temp_dir = testing::temp_dir("cli_non_existent_path").unwrap();
        let non_existent = temp_dir.join("does_not_exist");
        let exit_code = run_cli(["codefeed", non_existent.to_str().unwrap()]);
        assert_eq!(exit_code, 1);
    }

    #[test]
    fn run_cli_succeeds_with_file_and_stdout_output() {
        let temp_dir = testing::temp_dir("cli_success").unwrap();
        let file_path = temp_dir.join("sample.txt");
        std::fs::write(&file_path, "hello world").unwrap();

        let out_path = temp_dir.join("out.xml");
        let exit_code = run_cli([
            "codefeed",
            temp_dir.to_str().unwrap(),
            "-o",
            out_path.to_str().unwrap(),
        ]);
        assert_eq!(exit_code, 0);
        assert!(out_path.exists());

        let exit_code = run_cli(["codefeed", temp_dir.to_str().unwrap(), "-o", "-"]);
        assert_eq!(exit_code, 0);
    }
}
