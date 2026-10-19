use std::ffi::OsString;
use std::path::PathBuf;

use crate::core::error::IngestError;
use crate::core::model::{DigestFormat, OutputTarget, RunConfig, Source};

const HELP: &str =
    "Usage: codefeed <SOURCE> [--output <PATH|->] [--ref <REF>] [--format <text|xml>]";

/// Returns the static command-line help message.
pub(crate) const fn help_text() -> &'static str {
    HELP
}

/// Action determined by parsing command-line arguments.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CliAction {
    /// Print help text and exit successfully.
    Help,
    /// Execute ingestion using the resolved [`RunConfig`].
    Run(RunConfig),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flag {
    Output,
    Reference,
    Format,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParseState {
    Normal,
    ExpectValue(Flag, &'static str),
    PositionalOnly,
}

fn match_flag(s: &str) -> Option<(Flag, &'static str, Option<&str>)> {
    for (flag, flag_str) in [
        (Flag::Output, "-o"),
        (Flag::Output, "--output"),
        (Flag::Reference, "-r"),
        (Flag::Reference, "--ref"),
        (Flag::Format, "-f"),
        (Flag::Format, "--format"),
    ] {
        if s == flag_str {
            return Some((flag, flag_str, None));
        }
        if let Some(rest) = s.strip_prefix(flag_str)
            && let Some(inline) = rest.strip_prefix('=')
        {
            return Some((flag, flag_str, Some(inline)));
        }
    }
    None
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
pub(crate) fn parse<I, T>(args: I) -> Result<CliAction, IngestError>
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
    let mut state = ParseState::Normal;

    for arg in args_iter {
        match state {
            ParseState::ExpectValue(flag, flag_str) => {
                match parse_flag_value(flag, flag_str, arg)? {
                    FlagValue::Output(val) => output = Some(val),
                    FlagValue::Reference(val) => reference = Some(val),
                    FlagValue::Format(val) => format = val,
                }
                state = ParseState::Normal;
            }
            ParseState::PositionalOnly => {
                if source.is_some() {
                    return Err(IngestError::usage(format!(
                        "unexpected argument '{}' found",
                        arg.to_string_lossy()
                    )));
                }
                source = Some(arg);
            }
            ParseState::Normal => {
                let arg_str = arg.to_str();
                if arg_str == Some("--") {
                    state = ParseState::PositionalOnly;
                    continue;
                }

                if let Some(s) = arg_str {
                    if s == "-h" || s == "--help" {
                        return Ok(CliAction::Help);
                    }
                    if let Some((flag, flag_str, inline_val)) = match_flag(s) {
                        if let Some(inline) = inline_val {
                            match parse_flag_value(flag, flag_str, OsString::from(inline))? {
                                FlagValue::Output(val) => output = Some(val),
                                FlagValue::Reference(val) => reference = Some(val),
                                FlagValue::Format(val) => format = val,
                            }
                        } else {
                            state = ParseState::ExpectValue(flag, flag_str);
                        }
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
        }
    }

    if let ParseState::ExpectValue(_, flag_str) = state {
        return Err(IngestError::usage(format!(
            "a value is required for '{flag_str}' but none was supplied"
        )));
    }

    let source_os = source.ok_or_else(|| {
        IngestError::usage("the following required arguments were not provided:\n  <SOURCE>")
    })?;

    let output_target = match output {
        Some(o) if o == "-" => OutputTarget::Stdout,
        Some(o) => OutputTarget::File(PathBuf::from(o)),
        None => OutputTarget::File(PathBuf::from("digest.txt")),
    };

    let resolved_source = Source::parse_os(&source_os)?.with_reference(reference)?;
    Ok(CliAction::Run(RunConfig::new(
        resolved_source,
        output_target,
        format,
    )))
}

enum FlagValue {
    Output(OsString),
    Reference(String),
    Format(DigestFormat),
}

fn parse_flag_value(flag: Flag, flag_str: &str, val: OsString) -> Result<FlagValue, IngestError> {
    let allow_dash = matches!(flag, Flag::Output);
    if let Some(s) = val.to_str()
        && s.starts_with('-')
        && !(allow_dash && s == "-")
    {
        return Err(IngestError::usage(format!(
            "a value is required for '{flag_str}' but none was supplied"
        )));
    }
    match flag {
        Flag::Output => Ok(FlagValue::Output(val)),
        Flag::Reference => {
            let val_str = val
                .into_string()
                .map_err(|_| IngestError::usage("git reference must be valid utf-8"))?;
            Ok(FlagValue::Reference(val_str))
        }
        Flag::Format => {
            let format = parse_format(&val.to_string_lossy())?;
            Ok(FlagValue::Format(format))
        }
    }
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
    use crate::core::model::{GitRef, Source};

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
                        reference: Some(GitRef::new("main").unwrap()),
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

    #[test]
    fn cli_non_utf8_format_reports_lossy_value() {
        #[cfg(unix)]
        let raw_val = {
            use std::os::unix::ffi::OsStringExt;
            std::ffi::OsString::from_vec(vec![0xff])
        };
        #[cfg(windows)]
        let raw_val = {
            use std::os::windows::ffi::OsStringExt;
            std::ffi::OsString::from_wide(&[0xD800])
        };

        #[cfg(any(unix, windows))]
        {
            let args = [
                std::ffi::OsString::from("codefeed"),
                std::ffi::OsString::from("."),
                std::ffi::OsString::from("-f"),
                raw_val,
            ];
            let err = parse(args.iter()).unwrap_err();
            assert!(
                err.to_string()
                    .contains("invalid value '\u{FFFD}' for '--format <text|xml>'"),
                "unexpected error: {err}"
            );
        }
    }

    #[test]
    fn cli_ref_precedence_and_validation() {
        assert!(matches!(
            parse(["codefeed", "--ref", "", "--help"]),
            Ok(CliAction::Help)
        ));
        assert!(matches!(
            parse(["codefeed", "--help", "--ref", ""]),
            Ok(CliAction::Help)
        ));

        let missing_source = parse(["codefeed", "--ref", ""]).unwrap_err();
        assert!(
            missing_source
                .to_string()
                .contains("required arguments were not provided"),
            "unexpected error: {missing_source}"
        );

        let local_ref = parse(["codefeed", ".", "--ref", ""]).unwrap_err();
        assert!(
            local_ref
                .to_string()
                .contains("--ref is only valid for GitHub sources"),
            "unexpected error: {local_ref}"
        );

        let github_ref = parse([
            "codefeed",
            "https://github.com/rust-lang/cargo",
            "--ref",
            "",
        ])
        .unwrap_err();
        assert!(
            github_ref.to_string().contains("--ref cannot be empty"),
            "unexpected error: {github_ref}"
        );
    }
}
