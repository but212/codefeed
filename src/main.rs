//! Command-line binary entry point for codefeed.

use std::env::args_os;
use std::process;

fn main() {
    process::exit(codefeed::cli::run_cli(args_os()));
}
