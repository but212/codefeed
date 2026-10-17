use std::ffi::OsString;
use std::path::PathBuf;

use crate::core::error::IngestError;
use crate::core::model::{DigestFormat, OutputTarget, RunConfig};
use crate::core::source::parse_os;

const HELP: &str =
    "Usage: codefeed <SOURCE> [--output <PATH|->] [--ref <REF>] [--format <text|xml>]";

/// Returns the static command-line help message.
pub const fn help_text() -> &'static str {
    HELP
}

/// Action determined by parsing command-line arguments.
#[derive(Debug, PartialEq, Eq)]
pub enum CliAction {
    /// Print help text and exit successfully.
    Help,
    /// Execute ingestion using the resolved [`RunConfig`].
    Run(RunConfig),
}

/// Parses command-line arguments into a [`CliAction`].
///
/// Skips the first element (the binary name) and interprets remaining arguments
/// according to the CLI specification.
///
/// # Errors
///
/// Returns [`IngestError::Usage`] if required arguments are missing, unknown options
/// are passed, or flags receive invalid values.
pub fn parse<I, T>(args: I) -> Result<CliAction, IngestError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let mut args_iter = args.into_iter().map(Into::into);
    let _bin = args_iter.next(); // Skip binary name

    let mut source: Option<OsString> = None;
    let mut output: Option<OsString> = None;
    let mut reference: Option<String> = None;
    let mut format = DigestFormat::Text;
    let mut positional_only = false;

    while let Some(arg) = args_iter.next() {
        if positional_only {
            if source.is_some() {
                return Err(IngestError::usage(format!(
                    "unexpected argument '{}' found",
                    arg.to_string_lossy()
                )));
            }
            source = Some(arg);
            continue;
        }

        let arg_str = arg.to_str();
        if arg_str == Some("--") {
            positional_only = true;
            continue;
        }

        if let Some(s) = arg_str {
            if s == "-h" || s == "--help" {
                return Ok(CliAction::Help);
            }
            if s == "-o" || s == "--output" || s.starts_with("-o=") || s.starts_with("--output=") {
                let inline = s.split_once('=').map(|(_, v)| v);
                let flag = inline.map_or(s, |_| {
                    if s.starts_with("-o=") {
                        "-o"
                    } else {
                        "--output"
                    }
                });
                output = Some(extract_opt_val(flag, &mut args_iter, inline, true)?);
                continue;
            }
            if s == "-r" || s == "--ref" || s.starts_with("-r=") || s.starts_with("--ref=") {
                let inline = s.split_once('=').map(|(_, v)| v);
                let flag = inline.map_or(s, |_| if s.starts_with("-r=") { "-r" } else { "--ref" });
                let val = extract_opt_val(flag, &mut args_iter, inline, false)?;
                let val_str = val
                    .into_string()
                    .map_err(|_| IngestError::usage("git reference must be valid utf-8"))?;
                reference = Some(val_str);
                continue;
            }
            if s == "-f" || s == "--format" || s.starts_with("-f=") || s.starts_with("--format=") {
                let inline = s.split_once('=').map(|(_, v)| v);
                let flag = inline.map_or(s, |_| {
                    if s.starts_with("-f=") {
                        "-f"
                    } else {
                        "--format"
                    }
                });
                let val = extract_opt_val(flag, &mut args_iter, inline, false)?;
                format = parse_format(val.to_str().unwrap_or(""))?;
                continue;
            }
            if s.starts_with('-') && s != "-" {
                return Err(IngestError::usage(format!("unknown option: {s}")));
            }
        }

        if source.is_some() {
            return Err(IngestError::usage(format!(
                "unexpected argument '{}' found",
                arg.to_string_lossy()
            )));
        }
        source = Some(arg);
    }

    let source_os = source.ok_or_else(|| {
        IngestError::usage("the following required arguments were not provided:\n  <SOURCE>")
    })?;

    let output_target = match output {
        Some(o) if o == "-" => OutputTarget::Stdout,
        Some(o) => OutputTarget::File(PathBuf::from(o)),
        None => OutputTarget::File(PathBuf::from("digest.txt")),
    };

    let resolved_source = parse_os(&source_os)?.with_reference(reference)?;
    Ok(CliAction::Run(RunConfig::new(
        resolved_source,
        output_target,
        format,
    )))
}

fn extract_opt_val<I>(
    flag: &str,
    args: &mut I,
    inline_val: Option<&str>,
    allow_dash: bool,
) -> Result<OsString, IngestError>
where
    I: Iterator<Item = OsString>,
{
    let val = match inline_val {
        Some(v) => OsString::from(v),
        None => match args.next() {
            Some(v) => v,
            None => {
                return Err(IngestError::usage(format!(
                    "a value is required for '{flag}' but none was supplied"
                )));
            }
        },
    };

    if let Some(s) = val.to_str()
        && s.starts_with('-')
        && !(allow_dash && s == "-")
    {
        return Err(IngestError::usage(format!(
            "a value is required for '{flag}' but none was supplied"
        )));
    }
    Ok(val)
}

fn parse_format(val: &str) -> Result<DigestFormat, IngestError> {
    match val {
        "text" => Ok(DigestFormat::Text),
        "xml" => Ok(DigestFormat::Xml),
        _ => Err(IngestError::usage(format!(
            "invalid value '{val}' for '--format <text|xml>'"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::Source;

    #[test]
    fn cli_parses_valid_invocations() {
        assert_eq!(parse(["codefeed", "--help"]).unwrap(), CliAction::Help);
        assert_eq!(parse(["codefeed", ".", "-h"]).unwrap(), CliAction::Help);

        let run_standard = parse(["codefeed", ".", "--format", "xml", "--output", "-"]).unwrap();
        match run_standard {
            CliAction::Run(config) => {
                assert!(matches!(config.source, Source::Local(_)));
                assert_eq!(config.format, DigestFormat::Xml);
                assert_eq!(config.output, OutputTarget::Stdout);
            }
            CliAction::Help => panic!("expected Run action"),
        }

        let run_short = parse(["codefeed", "-f", "xml", "-o", "-", "."]).unwrap();
        match run_short {
            CliAction::Run(config) => {
                assert!(matches!(config.source, Source::Local(_)));
                assert_eq!(config.format, DigestFormat::Xml);
                assert_eq!(config.output, OutputTarget::Stdout);
            }
            CliAction::Help => panic!("expected Run action"),
        }

        let run_eq = parse(["codefeed", "--format=xml", "-o=-", "."]).unwrap();
        match run_eq {
            CliAction::Run(config) => {
                assert!(matches!(config.source, Source::Local(_)));
                assert_eq!(config.format, DigestFormat::Xml);
                assert_eq!(config.output, OutputTarget::Stdout);
            }
            CliAction::Help => panic!("expected Run action"),
        }

        let run_delimiter = parse(["codefeed", "--", "-weird-path"]).unwrap();
        match run_delimiter {
            CliAction::Run(config) => {
                assert_eq!(config.source, Source::Local(PathBuf::from("-weird-path")));
            }
            CliAction::Help => panic!("expected Run action"),
        }

        let run_dash = parse(["codefeed", "-o", "-", "--", "-"]).unwrap();
        match run_dash {
            CliAction::Run(config) => {
                assert_eq!(config.source, Source::Local(PathBuf::from("-")));
                assert_eq!(config.output, OutputTarget::Stdout);
            }
            CliAction::Help => panic!("expected Run action"),
        }

        let run_gh = parse([
            "codefeed",
            "https://github.com/rust-lang/cargo",
            "--ref",
            "main",
        ])
        .unwrap();
        match run_gh {
            CliAction::Run(config) => {
                assert_eq!(
                    config.source,
                    Source::Github {
                        owner: "rust-lang".into(),
                        repo: "cargo".into(),
                        reference: Some("main".into()),
                    }
                );
            }
            CliAction::Help => panic!("expected Run action"),
        }
    }

    #[test]
    fn cli_rejects_invalid_invocations() {
        let invalid_cases: &[&[&str]] = &[
            &["codefeed"],
            &["codefeed", "src", "tests"],
            &["codefeed", ".", "--ref", "main"],
            &[
                "codefeed",
                "https://github.com/rust-lang/cargo",
                "--ref",
                "",
            ],
            &[
                "codefeed",
                "https://github.com/rust-lang/cargo",
                "--ref",
                "   ",
            ],
            &["codefeed", "https://github.com/rust-lang/cargo", "-r", "-"],
            &[
                "codefeed",
                "https://github.com/rust-lang/cargo",
                "--ref",
                "-",
            ],
            &["codefeed", "src", "-f", "-"],
            &["codefeed", "src", "--format", "-"],
        ];
        for args in invalid_cases {
            assert!(
                parse(args.iter().copied()).is_err(),
                "expected error for: {args:?}"
            );
        }
    }
}
