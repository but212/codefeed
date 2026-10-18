//! Imperative shell: process, filesystem, network, and orchestration.
//!
//! Shell modules perform all effects and feed observations to the functional
//! core in `crate::core`.

use crate::{
    IngestError, IngestReport, OutputTarget, RunConfig, Source,
    core::cli::{CliAction, help_text, parse},
};

mod github;
mod local;
mod output;
#[cfg(test)]
mod testing;

/// Executes the end-to-end repository ingestion pipeline.
///
/// Dispatches collection across local filesystem or GitHub remote sources, renders
/// the digest into the target format (plain text or XML), and atomically emits the
/// result to standard output or a destination file.
///
/// # Errors
///
/// Returns [`IngestError`] if input sources are invalid, candidate budgets or output size
/// limits are exceeded, filesystem operations fail, or network requests error.
pub fn ingest(config: &RunConfig) -> Result<IngestReport, IngestError> {
    let resolved_output = output::resolve_output_target(&config.output)?;

    let collection = match &config.source {
        Source::Local(path) => local::collect(path, resolved_output.as_deref())?,
        Source::Github {
            owner,
            repo,
            reference,
        } => github::collect(owner, repo, reference.as_deref())?,
    };

    let estimated_tokens = output::emit(resolved_output.as_deref(), &collection, config.format)?;

    Ok(IngestReport {
        output: config.output.clone(),
        included_files: collection.files.len(),
        skipped_files: collection.skipped,
        estimated_tokens,
    })
}

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
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    };

    match action {
        CliAction::Help => {
            output::write_stdout_line(help_text());
            0
        }
        CliAction::Run(config) => match ingest(&config) {
            Ok(report) => {
                let message = format_completion_message(&report);
                match &report.output {
                    OutputTarget::File(_) => {
                        output::write_stdout_line(&message);
                    }
                    OutputTarget::Stdout => {
                        eprintln!("{message}");
                    }
                }
                0
            }
            Err(error) => {
                eprintln!("error: {error}");
                1
            }
        },
    }
}

fn format_completion_message(report: &IngestReport) -> String {
    let included = report.included_files;
    let skipped = report.skipped_files;
    let tokens = report.estimated_tokens;
    match &report.output {
        OutputTarget::File(path) => {
            let path_display = path.display();
            format!("Wrote {path_display} ({included} files, {skipped} skipped, ~{tokens} tokens)")
        }
        OutputTarget::Stdout => {
            format!("Wrote to stdout ({included} files, {skipped} skipped, ~{tokens} tokens)")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn format_completion_message_for_file_target() {
        let report = IngestReport {
            output: OutputTarget::File(PathBuf::from("digest.txt")),
            included_files: 10,
            skipped_files: 2,
            estimated_tokens: 1500,
        };
        let message = format_completion_message(&report);
        assert_eq!(
            message,
            "Wrote digest.txt (10 files, 2 skipped, ~1500 tokens)"
        );
    }

    #[test]
    fn format_completion_message_for_stdout_target() {
        let report = IngestReport {
            output: OutputTarget::Stdout,
            included_files: 5,
            skipped_files: 0,
            estimated_tokens: 300,
        };
        let message = format_completion_message(&report);
        assert_eq!(message, "Wrote to stdout (5 files, 0 skipped, ~300 tokens)");
    }

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
}
