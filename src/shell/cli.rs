use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use crate::core::error::IngestError;
use crate::core::model::{DigestFormat, Options, OutputTarget, Source};
use crate::core::source::parse_source_os;

const HELP: &str = "Usage: codefeed <SOURCE> [--output <PATH|->] [--ref <REF>] [--format <FORMAT>]";

pub fn help_text() -> &'static str {
    HELP
}

#[derive(Debug, PartialEq, Eq)]
pub enum CliAction {
    Help,
    Run { source: Source, options: Options },
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

/// Returns `true` if the argument begins with a hyphen.
fn starts_with_hyphen(arg: &OsStr) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        arg.as_bytes().starts_with(b"-")
    }
    #[cfg(not(unix))]
    {
        arg.to_str().is_some_and(|s| s.starts_with('-'))
    }
}

fn next_val<I: Iterator<Item = OsString>>(
    args: &mut I,
    inline_val: Option<&str>,
    missing_msg: &str,
) -> Result<OsString, IngestError> {
    if let Some(v) = inline_val {
        return Ok(OsString::from(v));
    }
    let Some(next_arg) = args.next() else {
        return Err(usage(missing_msg));
    };
    if matches!(
        next_arg.to_str(),
        Some("--help" | "-h" | "--output" | "-o" | "--ref" | "-r" | "--format" | "-f")
    ) {
        return Err(usage(missing_msg));
    }
    Ok(next_arg)
}

pub fn parse_cli<I, T>(args: I) -> Result<CliAction, IngestError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let mut args = args.into_iter().map(Into::into);
    let _program = args.next(); // skip binary name

    let mut source_slot: Option<OsString> = None;
    let mut parsed_reference: Option<String> = None;
    let mut options = Options::default();
    let mut after_delimiter = false;

    while let Some(arg) = args.next() {
        if !after_delimiter {
            if arg == OsStr::new("--") {
                after_delimiter = true;
                continue;
            }

            if starts_with_hyphen(&arg) && arg != OsStr::new("-") {
                let Some(arg_str) = arg.to_str() else {
                    return Err(usage("unknown option contains non-UTF-8 bytes"));
                };

                if arg_str == "--help" || arg_str == "-h" {
                    return Ok(CliAction::Help);
                }

                let (key, inline_val) = arg_str
                    .split_once('=')
                    .map_or((arg_str, None), |(k, v)| (k, Some(v)));

                match key {
                    "--output" | "-o" => {
                        let val = next_val(&mut args, inline_val, "--output requires a path")?;
                        options.output = parse_output_target(&val);
                    }
                    "--ref" | "-r" => {
                        let val = next_val(
                            &mut args,
                            inline_val,
                            "--ref requires a branch, tag, or commit",
                        )?;
                        let Some(s) = val.to_str() else {
                            return Err(usage("--ref requires a UTF-8 value"));
                        };
                        parsed_reference = Some(s.to_owned());
                    }
                    "--format" | "-f" => {
                        let val = next_val(
                            &mut args,
                            inline_val,
                            "--format requires a format (text, xml)",
                        )?;
                        let Some(s) = val.to_str() else {
                            return Err(usage("--format requires a UTF-8 value"));
                        };
                        match s.to_ascii_lowercase().as_str() {
                            "text" => options.format = DigestFormat::Text,
                            "xml" => options.format = DigestFormat::Xml,
                            other => {
                                return Err(usage(&format!(
                                    "invalid format: {other} (expected text, xml)"
                                )));
                            }
                        }
                    }
                    unknown => return Err(usage(&format!("unknown option: {unknown}"))),
                }
                continue;
            }
        }

        if source_slot.is_some() {
            return Err(usage(&format!(
                "unexpected argument: {}",
                arg.to_string_lossy()
            )));
        }
        source_slot = Some(arg);
    }

    if parsed_reference
        .as_ref()
        .is_some_and(|r| r.trim().is_empty())
    {
        return Err(IngestError::InvalidSource("--ref cannot be empty".into()));
    }

    let Some(source_value) = source_slot else {
        return Err(usage("missing source"));
    };
    let mut source = parse_source_os(&source_value)?;

    if let Some(reference) = parsed_reference {
        match &mut source {
            Source::GitHub {
                reference: ref_slot,
                ..
            } => {
                *ref_slot = Some(reference);
            }
            Source::Local(_) => {
                return Err(IngestError::InvalidSource(
                    "--ref is only valid for GitHub sources".into(),
                ));
            }
        }
    }

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
    fn cli_parses_valid_invocations() {
        assert_eq!(parse_cli(["codefeed", "--help"]).unwrap(), CliAction::Help);
        assert_eq!(parse_cli(["codefeed", ".", "-h"]).unwrap(), CliAction::Help);

        let run_standard =
            parse_cli(["codefeed", ".", "--format", "xml", "--output", "-"]).unwrap();
        match run_standard {
            CliAction::Run { source, options } => {
                assert!(matches!(source, Source::Local(_)));
                assert_eq!(options.format, DigestFormat::Xml);
                assert_eq!(options.output, OutputTarget::Stdout);
            }
            CliAction::Help => panic!("expected Run action"),
        }

        let run_short = parse_cli(["codefeed", "-f", "xml", "-o", "-", "."]).unwrap();
        match run_short {
            CliAction::Run { source, options } => {
                assert!(matches!(source, Source::Local(_)));
                assert_eq!(options.format, DigestFormat::Xml);
                assert_eq!(options.output, OutputTarget::Stdout);
            }
            CliAction::Help => panic!("expected Run action"),
        }

        let run_eq = parse_cli(["codefeed", "--format=xml", "-o=-", "."]).unwrap();
        match run_eq {
            CliAction::Run { source, options } => {
                assert!(matches!(source, Source::Local(_)));
                assert_eq!(options.format, DigestFormat::Xml);
                assert_eq!(options.output, OutputTarget::Stdout);
            }
            CliAction::Help => panic!("expected Run action"),
        }

        let run_delimiter = parse_cli(["codefeed", "--", "-weird-path"]).unwrap();
        match run_delimiter {
            CliAction::Run { source, .. } => {
                assert_eq!(source, Source::Local(PathBuf::from("-weird-path")));
            }
            CliAction::Help => panic!("expected Run action"),
        }

        let run_gh = parse_cli([
            "codefeed",
            "https://github.com/rust-lang/cargo",
            "--ref",
            "main",
        ])
        .unwrap();
        match run_gh {
            CliAction::Run { source, .. } => {
                assert_eq!(
                    source,
                    Source::GitHub {
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
        ];
        for args in invalid_cases {
            assert!(
                parse_cli(args.iter().copied()).is_err(),
                "expected error for: {args:?}"
            );
        }
    }
}
