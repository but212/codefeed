use std::path::PathBuf;

use crate::{ingest, parse_source, IngestError, IngestReport, Options};

const HELP: &str = "Usage: local_ingest ingest <SOURCE> [--output <PATH>] [--ref <REF>]";

pub(crate) fn help_text() -> &'static str {
    HELP
}

pub(crate) fn run_cli<I>(args: I) -> Result<IngestReport, IngestError>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    let _program = args.next();
    let command = args.next().ok_or_else(|| usage("missing command"))?;
    if command != "ingest" {
        return Err(usage("expected the 'ingest' command"));
    }
    let source_value = args.next().ok_or_else(|| usage("missing source"))?;
    let mut options = Options::default();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--output" => {
                let value = args
                    .next()
                    .ok_or_else(|| usage("--output requires a path"))?;
                if matches!(value.as_str(), "--help" | "-h") {
                    return Err(usage("--output requires a path"));
                }
                options.output = PathBuf::from(value);
            }
            "--ref" => {
                let value = args
                    .next()
                    .ok_or_else(|| usage("--ref requires a branch, tag, or commit"))?;
                if matches!(value.as_str(), "--help" | "-h") {
                    return Err(usage("--ref requires a branch, tag, or commit"));
                }
                options.reference = Some(value);
            }
            "--help" | "-h" => return Err(usage(HELP)),
            unknown => return Err(usage(&format!("unknown option: {unknown}"))),
        }
    }
    let source = parse_source(&source_value)?;
    ingest(&source, &options)
}

fn usage(message: &str) -> IngestError {
    IngestError::InvalidSource(format!("{message}\n\n{HELP}"))
}
