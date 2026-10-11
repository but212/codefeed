//! Command-line binary entry point for rustingest.

mod core;
mod shell;

use std::env::args_os;
use std::process;

use crate::core::model::OutputTarget;
use crate::shell::cli::{CliAction, help_text, parse_cli};

pub(crate) use shell::orchestrate::ingest;

fn main() {
    let action = parse_cli(args_os()).unwrap_or_else(|error| {
        eprintln!("error: {error}");
        process::exit(1);
    });

    match action {
        CliAction::Help => {
            println!("{}", help_text());
        }
        CliAction::Run { source, options } => match ingest(&source, &options) {
            // With `--output -` the digest owns stdout, so the completion message
            // goes to stderr there and stays on stdout for a file target.
            Ok(report) => {
                let included = report.included_files;
                let skipped = report.skipped_files;
                let tokens = report.estimated_tokens;
                match &report.output {
                    OutputTarget::File(path) => {
                        let path_display = path.display();
                        println!(
                            "Wrote {path_display} ({included} files, {skipped} skipped, ~{tokens} tokens)"
                        );
                    }
                    OutputTarget::Stdout => {
                        eprintln!(
                            "Wrote to stdout ({included} files, {skipped} skipped, ~{tokens} tokens)"
                        );
                    }
                }
            }
            Err(error) => {
                eprintln!("error: {error}");
                process::exit(1);
            }
        },
    }
}
