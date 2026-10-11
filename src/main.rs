//! Command-line binary entry point for rustingest.

mod core;
mod shell;

use std::env::args_os;
use std::process;

use crate::core::model::OutputTarget;
use crate::shell::cli::{CliAction, help_text, parse_cli};

pub(crate) use shell::orchestrate::ingest;

fn main() {
    let action = match parse_cli(args_os()) {
        Ok(action) => action,
        Err(error) => {
            eprintln!("error: {error}");
            process::exit(1);
        }
    };

    match action {
        CliAction::Help => {
            println!("{}", help_text());
        }
        CliAction::Run { source, options } => match ingest(&source, &options) {
            // With `--output -` the digest owns stdout, so the completion message
            // goes to stderr there and stays on stdout for a file target.
            Ok(report) => match &report.output {
                OutputTarget::File(path) => println!(
                    "Wrote {} ({} files, {} skipped, ~{} tokens)",
                    path.display(),
                    report.included_files,
                    report.skipped_files,
                    report.estimated_tokens
                ),
                OutputTarget::Stdout => eprintln!(
                    "Wrote to stdout ({} files, {} skipped, ~{} tokens)",
                    report.included_files, report.skipped_files, report.estimated_tokens
                ),
            },
            Err(error) => {
                eprintln!("error: {error}");
                process::exit(1);
            }
        },
    }
}
