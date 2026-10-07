//! Command-line binary entry point for rustingest.

use std::env::args_os;
use std::process;

use rustingest::{OutputTarget, help_text, requests_help, run_cli};

fn main() {
    let args: Vec<_> = args_os().collect();
    if requests_help(&args) {
        println!("{}", help_text());
        return;
    }

    match run_cli(args) {
        // With `--output -` the digest owns stdout, so the completion message
        // goes to stderr there and stays on stdout for a file target.
        Ok(report) => match &report.output {
            OutputTarget::File(path) => println!(
                "Wrote {} ({} files, {} skipped)",
                path.display(),
                report.included_files,
                report.skipped_files
            ),
            OutputTarget::Stdout => eprintln!(
                "Wrote to stdout ({} files, {} skipped)",
                report.included_files, report.skipped_files
            ),
        },
        Err(error) => {
            eprintln!("error: {error}");
            process::exit(1);
        }
    }
}
