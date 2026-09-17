use rustingest::{help_text, requests_help, run_cli};
use std::env::args_os;
use std::process;

fn main() {
    let args: Vec<_> = args_os().collect();
    if requests_help(&args) {
        println!("{}", help_text());
        return;
    }

    match run_cli(args) {
        Ok(report) => {
            println!(
                "Wrote {} ({} files, {} skipped)",
                report.output.display(),
                report.included_files,
                report.skipped_files
            );
        }
        Err(error) => {
            eprintln!("error: {error}");
            process::exit(1);
        }
    }
}
