use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use crate::core::error::DomainError;
use crate::core::model::{DigestFormat, IngestReport, OutputTarget, RunConfig, Source};

const HELP: &str =
    "Usage: codefeed <SOURCE> [--output <PATH|->] [--ref <REF>] [--format <text|xml>]";

/// Returns the static command-line help message.
pub(crate) const fn help_text() -> &'static str {
    HELP
}

/// One-line summary of a finished run, naming where the digest went.
#[must_use]
pub(crate) fn completion_message(report: &IngestReport) -> String {
    let included = report.included_files;
    let skipped = report.skipped_files;
    let tokens = report.estimated_tokens;
    match &report.output {
        OutputTarget::File(path) => {
            let path_display = path.display();
            format!("Wrote {path_display} ({included} files, {skipped} skipped, ~{tokens} tokens)")
        }
        OutputTarget::Stdout => {
            format!("Wrote to stdout ({included} files, {skipped} skipped, ~{tokens} tokens)")
        }
    }
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

/// Option values folded in argument order. A repeated flag keeps its last value.
#[derive(Debug, Default)]
struct Parsed {
    source: Option<OsString>,
    output: Option<OsString>,
    reference: Option<String>,
    format: DigestFormat,
}

impl Parsed {
    /// Stores one flag value after validating it for that flag.
    fn with_value(mut self, flag: Flag, val: OsString) -> Result<Self, DomainError> {
        match flag {
            Flag::Output => self.output = Some(val),
            Flag::Reference => {
                let val_str = val
                    .into_string()
                    .map_err(|_| DomainError::usage("git reference must be valid utf-8"))?;
                self.reference = Some(val_str);
            }
            Flag::Format => self.format = parse_format(&val.to_string_lossy())?,
        }
        Ok(self)
    }

    /// Stores the source positional. A second one is rejected.
    fn with_positional(mut self, arg: OsString) -> Result<Self, DomainError> {
        if self.source.is_some() {
            return Err(DomainError::usage(format!(
                "unexpected argument '{}' found",
                arg.to_string_lossy()
            )));
        }
        self.source = Some(arg);
        Ok(self)
    }
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

/// Meaning of one argument seen in `ParseState::Normal`.
enum Arg<'a> {
    /// `--`: every later argument is positional.
    Separator,
    /// `-h` or `--help`.
    Help,
    /// A known flag, with its inline `=value` when one is given.
    Flag(Flag, &'static str, Option<&'a str>),
    /// An option-like argument that is not a known flag.
    Unknown(&'a str),
    /// The source positional. Also `-` and any argument that is not valid UTF-8.
    Positional,
}

/// Classifies one argument. Checks run in order: separator, help, known flag, unknown option.
fn classify_arg(arg: &OsStr) -> Arg<'_> {
    let Some(s) = arg.to_str() else {
        return Arg::Positional;
    };
    if s == "--" {
        return Arg::Separator;
    }
    if s == "-h" || s == "--help" {
        return Arg::Help;
    }
    if let Some((flag, flag_str, inline)) = match_flag(s) {
        return Arg::Flag(flag, flag_str, inline);
    }
    if s.starts_with('-') && s != "-" {
        return Arg::Unknown(s);
    }
    Arg::Positional
}

/// Parses command-line arguments into a [`CliAction`].
///
/// Skips the first element (the binary name) and interprets remaining arguments
/// according to the CLI specification.
///
/// # Errors
///
/// Returns [`DomainError::Usage`] if required arguments are missing, unknown options
/// are passed, or flags receive invalid values.
pub(crate) fn parse<I, T>(args: I) -> Result<CliAction, DomainError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let mut args_iter = args.into_iter().map(Into::into);
    let _bin = args_iter.next(); // Skip binary name

    let mut parsed = Parsed::default();
    let mut state = ParseState::Normal;

    for arg in args_iter {
        match state {
            ParseState::ExpectValue(flag, flag_str) => {
                reject_option_like(flag, flag_str, &arg)?;
                parsed = parsed.with_value(flag, arg)?;
                state = ParseState::Normal;
            }
            ParseState::PositionalOnly => {
                parsed = parsed.with_positional(arg)?;
            }
            ParseState::Normal => match classify_arg(&arg) {
                Arg::Separator => state = ParseState::PositionalOnly,
                Arg::Help => return Ok(CliAction::Help),
                Arg::Flag(flag, _, Some(inline)) => {
                    parsed = parsed.with_value(flag, OsString::from(inline))?;
                }
                Arg::Flag(flag, flag_str, None) => {
                    state = ParseState::ExpectValue(flag, flag_str);
                }
                Arg::Unknown(s) => {
                    return Err(DomainError::usage(format!("unknown option: {s}")));
                }
                Arg::Positional => parsed = parsed.with_positional(arg)?,
            },
        }
    }

    if let ParseState::ExpectValue(_, flag_str) = state {
        return Err(missing_value(flag_str));
    }

    let Parsed {
        source,
        output,
        reference,
        format,
    } = parsed;
    let source_os = source.ok_or_else(|| {
        DomainError::usage("the following required arguments were not provided:\n  <SOURCE>")
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

/// Rejects a separate-argument flag value that starts with `-`, so `-o -x` cannot
/// swallow the next option. `-o -` is the stdout sentinel and stays allowed.
/// Inline `--flag=value` forms never reach this check.
fn reject_option_like(flag: Flag, flag_str: &str, val: &OsStr) -> Result<(), DomainError> {
    let allow_dash = matches!(flag, Flag::Output);
    if let Some(s) = val.to_str()
        && s.starts_with('-')
        && !(allow_dash && s == "-")
    {
        return Err(missing_value(flag_str));
    }
    Ok(())
}

/// The one "a value is required" message, for a flag with no value or with an option-like value.
fn missing_value(flag_str: &str) -> DomainError {
    DomainError::usage(format!(
        "a value is required for '{flag_str}' but none was supplied"
    ))
}

fn parse_format(val: &str) -> Result<DigestFormat, DomainError> {
    match val {
        "text" => Ok(DigestFormat::Text),
        "xml" => Ok(DigestFormat::Xml),
        _ => Err(DomainError::usage(format!(
            "invalid value '{val}' for '--format <text|xml>'"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{GitRef, Source};

    #[test]
    fn completion_message_names_destination() {
        let cases = [
            (
                IngestReport {
                    output: OutputTarget::File(PathBuf::from("digest.txt")),
                    included_files: 10,
                    skipped_files: 2,
                    estimated_tokens: 1500,
                },
                "Wrote digest.txt (10 files, 2 skipped, ~1500 tokens)",
            ),
            (
                IngestReport {
                    output: OutputTarget::Stdout,
                    included_files: 5,
                    skipped_files: 0,
                    estimated_tokens: 300,
                },
                "Wrote to stdout (5 files, 0 skipped, ~300 tokens)",
            ),
        ];
        for (report, expected) in cases {
            assert_eq!(completion_message(&report), expected);
        }
    }

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

        let run_inline_dash = parse(["codefeed", ".", "--output=-notes.txt"]).unwrap();
        match run_inline_dash {
            CliAction::Run(config) => {
                assert_eq!(
                    config.output,
                    OutputTarget::File(PathBuf::from("-notes.txt"))
                );
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
            &["codefeed", "src", "--format=-x"],
            &["codefeed", "src", "-o", "-x"],
        ];
        for args in invalid_cases {
            assert!(
                parse(args.iter().copied()).is_err(),
                "expected error for: {args:?}"
            );
        }
    }

    #[test]
    fn cli_usage_messages_are_stable() {
        // (argv, expected message fragment) for each failure path `parse` owns
        let cases: [(&[&str], &str); 8] = [
            (
                &["codefeed", ".", "--output"][..],
                "a value is required for '--output' but none was supplied",
            ),
            (
                &["codefeed", ".", "-o", "-x"][..],
                "a value is required for '-o' but none was supplied",
            ),
            (
                &["codefeed", ".", "--format", "-"][..],
                "a value is required for '--format' but none was supplied",
            ),
            (
                &["codefeed", ".", "extra"][..],
                "unexpected argument 'extra' found",
            ),
            (
                &["codefeed", "--", ".", "extra"][..],
                "unexpected argument 'extra' found",
            ),
            (&["codefeed", ".", "--bogus"][..], "unknown option: --bogus"),
            (
                &["codefeed", ".", "-f", "yaml"][..],
                "invalid value 'yaml' for '--format <text|xml>'",
            ),
            (
                &["codefeed"][..],
                "the following required arguments were not provided",
            ),
        ];
        for (args, expected) in cases {
            let err = parse(args.iter().copied()).unwrap_err();
            assert!(err.to_string().contains(expected), "{args:?}: {err}");
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
