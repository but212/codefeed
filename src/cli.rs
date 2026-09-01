use std::ffi::OsString;
use std::path::PathBuf;

use crate::{ingest, parse_source_os, IngestError, IngestReport, Options};

const HELP: &str = "Usage: rustingest <SOURCE> [--output <PATH>] [--ref <REF>]";

pub(crate) fn help_text() -> &'static str {
    HELP
}

pub(crate) fn run_cli<I, T>(args: I) -> Result<IngestReport, IngestError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let mut args = args.into_iter().map(Into::into);
    let _program = args.next();
    let source_value = args.next().ok_or_else(|| usage("missing source"))?;
    let mut options = Options::default();
    while let Some(argument) = args.next() {
        match argument.to_str() {
            Some("--output") => {
                let value = args
                    .next()
                    .ok_or_else(|| usage("--output requires a path"))?;
                if matches!(value.to_str(), Some("--help" | "-h")) {
                    return Err(usage("--output requires a path"));
                }
                options.output = PathBuf::from(value);
            }
            Some("--ref") => {
                let value = args
                    .next()
                    .ok_or_else(|| usage("--ref requires a branch, tag, or commit"))?;
                if matches!(value.to_str(), Some("--help" | "-h")) {
                    return Err(usage("--ref requires a branch, tag, or commit"));
                }
                let value = value
                    .to_str()
                    .ok_or_else(|| usage("--ref requires a UTF-8 value"))?;
                options.reference = Some(value.to_owned());
            }
            Some("--help" | "-h") => return Err(usage(HELP)),
            Some(unknown) => return Err(usage(&format!("unknown option: {unknown}"))),
            None => return Err(usage("unknown option contains non-UTF-8 bytes")),
        }
    }
    let source = parse_source_os(&source_value)?;
    ingest(&source, &options)
}

fn usage(message: &str) -> IngestError {
    IngestError::InvalidSource(format!("{message}\n\n{HELP}"))
}
