use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use crate::core::error::IngestError;
use crate::core::model::{DigestFormat, Options, OutputTarget, Source};
use crate::core::source::parse_source_os;

const HELP: &str =
    "Usage: rustingest <SOURCE> [--output <PATH|->] [--ref <REF>] [--format <FORMAT>]";

pub(crate) fn help_text() -> &'static str {
    HELP
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CliAction {
    Help,
    Run {
        source: Source,
        options: Options,
    },
}

/// `--output -` selects standard output; every other value is a filesystem
/// path, including names that merely start with a dash.
fn parse_output_target(value: &OsStr) -> OutputTarget {
    if value == OsStr::new("-") {
        OutputTarget::Stdout
    } else {
        OutputTarget::File(PathBuf::from(value))
    }
}

/// Scans for `--help`/`-h` in option position, skipping values of options that
/// take one. Owned here so the binary does not re-implement the CLI grammar.
fn requests_help(args: &[OsString]) -> bool {
    let mut index = 1;
    while let Some(argument) = args.get(index) {
        match argument.to_str() {
            Some("--help" | "-h") => return true,
            Some("--output" | "--ref" | "--format") => index += 2,
            _ => index += 1,
        }
    }
    false
}

pub(crate) fn parse_cli<I, T>(args: I) -> Result<CliAction, IngestError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    if requests_help(&args) {
        return Ok(CliAction::Help);
    }
    let mut iter = args.into_iter();
    let _program = iter.next();
    let source_value = iter.next().ok_or_else(|| usage("missing source"))?;
    let mut options = Options::default();
    while let Some(argument) = iter.next() {
        match argument.to_str() {
            Some("--output") => {
                let value = iter
                    .next()
                    .ok_or_else(|| usage("--output requires a path"))?;
                if matches!(value.to_str(), Some("--help" | "-h")) {
                    return Err(usage("--output requires a path"));
                }
                options.output = parse_output_target(&value);
            }
            Some("--ref") => {
                let value = iter
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
            Some("--format") => {
                let value = iter
                    .next()
                    .ok_or_else(|| usage("--format requires a format (text, xml)"))?;
                if matches!(value.to_str(), Some("--help" | "-h")) {
                    return Err(usage("--format requires a format (text, xml)"));
                }
                let value_str = value
                    .to_str()
                    .ok_or_else(|| usage("--format requires a UTF-8 value"))?;
                match value_str.to_ascii_lowercase().as_str() {
                    "text" => options.format = DigestFormat::Text,
                    "xml" => options.format = DigestFormat::Xml,
                    other => {
                        return Err(usage(&format!(
                            "invalid format: {other} (expected text, xml)"
                        )));
                    }
                }
            }
            Some("--help" | "-h") => return Err(usage(HELP)),
            Some(unknown) => return Err(usage(&format!("unknown option: {unknown}"))),
            None => return Err(usage("unknown option contains non-UTF-8 bytes")),
        }
    }
    let source = parse_source_os(&source_value)?;
    Ok(CliAction::Run { source, options })
}

fn usage(message: &str) -> IngestError {
    IngestError::Usage(format!("{message}\n\n{HELP}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parses_stdout_output_target() {
        assert_eq!(parse_output_target(OsStr::new("-")), OutputTarget::Stdout);
        assert_eq!(
            parse_output_target(OsStr::new("digest.txt")),
            OutputTarget::File(PathBuf::from("digest.txt"))
        );
        assert_eq!(
            parse_output_target(OsStr::new("-digest.txt")),
            OutputTarget::File(PathBuf::from("-digest.txt"))
        );
    }

    #[test]
    fn parses_help_action() {
        assert_eq!(
            parse_cli(["rustingest", "--help"]).unwrap(),
            CliAction::Help
        );
        assert_eq!(
            parse_cli(["rustingest", ".", "-h"]).unwrap(),
            CliAction::Help
        );
    }

    #[test]
    fn parses_run_action() {
        let action = parse_cli([
            "rustingest",
            ".",
            "--format",
            "xml",
            "--output",
            "-",
        ])
        .unwrap();
        match action {
            CliAction::Run { source, options } => {
                assert!(matches!(source, Source::Local(_)));
                assert_eq!(options.format, DigestFormat::Xml);
                assert_eq!(options.output, OutputTarget::Stdout);
            }
            CliAction::Help => panic!("expected Run action"),
        }
    }

    #[test]
    fn returns_error_on_missing_source() {
        assert!(parse_cli(["rustingest"]).is_err());
    }
}
