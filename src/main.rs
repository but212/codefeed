use std::ffi::OsString;
use std::process;

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    if requests_help(&args) {
        println!("{}", rustingest::help_text());
        return;
    }

    match rustingest::run_cli(args) {
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

fn requests_help(args: &[OsString]) -> bool {
    if matches!(
        args.get(1).and_then(|argument| argument.to_str()),
        Some("--help" | "-h")
    ) {
        return true;
    }
    if args.get(1).and_then(|argument| argument.to_str()) != Some("ingest") {
        return false;
    }

    let mut index = 2;
    while let Some(argument) = args.get(index).and_then(|argument| argument.to_str()) {
        match argument {
            "--help" | "-h" => return true,
            "--output" | "--ref" => index += 2,
            _ => index += 1,
        }
    }
    false
}
