//! CLI execution: argument parsing, output formatting, and exit code mapping.

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
    stdio::write_stderr_line(format_args!("error: {error}"));
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::testing::temp_dir;

    #[test]
    fn run_cli_returns_expected_exit_codes() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("run_cli_codes")?;
        let missing = root.join("does_not_exist");

        assert_eq!(run_cli(["codefeed", "--help"]), 0);
        assert_eq!(run_cli(["codefeed", "--version"]), 0);
        assert_eq!(run_cli(["codefeed", "--unknown"]), 1);
        assert_eq!(
            run_cli(["codefeed", missing.to_str().ok_or("temp path is UTF-8")?]),
            1
        );
        Ok(())
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn accepts_non_utf8_local_source_path() -> Result<(), Box<dyn std::error::Error>> {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let parent = temp_dir("non_utf8")?;
        let source = parent.join(OsString::from_vec(b"source-\xff".to_vec()));
        let output = source.join("digest.txt");
        std::fs::create_dir_all(&source)?;
        std::fs::write(source.join("visible.txt"), "visible")?;

        assert_eq!(
            run_cli([
                OsString::from("codefeed"),
                source.into_os_string(),
                OsString::from("--output"),
                output.clone().into_os_string(),
            ]),
            0
        );
        assert!(output.is_file());
        Ok(())
    }
}
