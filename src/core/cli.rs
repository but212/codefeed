use std::borrow::Cow;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use crate::core::error::{DomainError, ErrorDetail};
use crate::core::model::{DigestFormat, IngestReport, OutputTarget, RunConfig};
use crate::core::policy::ExcludePatterns;
use crate::core::source::Source;

pub(crate) const HELP: &str = "Usage: codefeed <SOURCE> [--output <PATH|->] [--ref <REF>] [--format <text|xml>] [--exclude <PATTERN>]...";

/// Text printed for `-V` and `--version`.
pub(crate) const VERSION: &str = concat!(env!("CARGO_PKG_NAME"), " ", env!("CARGO_PKG_VERSION"));

const STDOUT_ARG: &str = "-";

/// One-line summary of a finished run, naming where the digest went.
#[must_use]
pub(crate) fn completion_message(report: &IngestReport) -> String {
    let included = report.included_files;
    let skipped = report.skipped_files;
    let tokens = report.estimated_tokens;
    let destination = match &report.output {
        OutputTarget::File(path) => {
            Cow::Owned(ErrorDetail::new(path.to_string_lossy()).into_inner())
        }
        OutputTarget::Stdout => Cow::Borrowed("to stdout"),
    };
    format!("Wrote {destination} ({included} files, {skipped} skipped, ~{tokens} tokens)")
}

/// Action determined by parsing command-line arguments.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CliAction {
    /// Print help text and exit successfully.
    Help,
    /// Print the version and exit successfully.
    Version,
    /// Execute ingestion using the resolved [`RunConfig`]. Boxed so that `Help` does not pay for the config's size.
    Run(Box<RunConfig>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flag {
    Output,
    Reference,
    Format,
    Exclude,
}

/// Option values folded in argument order. A repeated flag keeps its last value, except `--exclude`,
/// whose patterns accumulate in the order given.
#[derive(Debug, Default)]
struct Parsed {
    source: Option<OsString>,
    output: Option<OutputTarget>,
    reference: Option<String>,
    format: DigestFormat,
    exclude: Vec<String>,
}

impl Parsed {
    /// Stores one flag value after validating it for that flag.
    fn with_value(mut self, flag: Flag, val: OsString) -> Result<Self, DomainError> {
        match flag {
            Flag::Output => {
                self.output = Some(if val == STDOUT_ARG {
                    OutputTarget::Stdout
                } else {
                    OutputTarget::File(val.into())
                });
            }
            Flag::Reference => {
                let val_str = val
                    .into_string()
                    .map_err(|_| DomainError::usage("git reference must be valid utf-8"))?;
                self.reference = Some(val_str);
            }
            Flag::Format => {
                self.format = match val.as_encoded_bytes() {
                    b"text" => DigestFormat::Text,
                    b"xml" => DigestFormat::Xml,
                    _ => {
                        return Err(DomainError::usage(format!(
                            "invalid value '{}' for '--format <text|xml>'",
                            val.to_string_lossy()
                        )));
                    }
                };
            }
            Flag::Exclude => {
                let pattern = val
                    .into_string()
                    .map_err(|_| DomainError::usage("exclude pattern must be valid utf-8"))?;
                self.exclude.push(pattern);
            }
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

    /// Resolves accumulated options into a [`CliAction::Run`].
    fn finish(self) -> Result<CliAction, DomainError> {
        let source_os = self.source.ok_or_else(|| {
            DomainError::usage("the following required arguments were not provided: <SOURCE>")
        })?;

        let source = Source::parse_os(&source_os)?.with_reference(self.reference)?;
        Ok(CliAction::Run(Box::new(RunConfig {
            source,
            output: self
                .output
                .unwrap_or_else(|| OutputTarget::File(PathBuf::from("digest.txt"))),
            format: self.format,
            exclude: ExcludePatterns::new(self.exclude)?,
        })))
    }
}

/// Meaning of one argument in option position.
enum Arg<'a> {
    /// `--`: every later argument is positional.
    Separator,
    /// `-h` or `--help`.
    Help,
    /// `-V` or `--version`.
    Version,
    /// A value option, preserving its spelling for diagnostics.
    Flag {
        flag: Flag,
        spelling: &'a str,
        inline: Option<Cow<'a, OsStr>>,
    },
    /// An option-like Unicode argument that is not a known flag.
    Unknown(&'a str),
    /// The source positional, including `-` and non-UTF-8 paths without a known option name.
    Positional,
}

/// Matches control tokens exactly and splits value options at their first `=`.
fn classify_arg(arg: &OsStr) -> Arg<'_> {
    let bytes = arg.as_encoded_bytes();
    match bytes {
        b"--" => return Arg::Separator,
        b"-h" | b"--help" => return Arg::Help,
        b"-V" | b"--version" => return Arg::Version,
        b"-" => return Arg::Positional,
        [b'-', ..] => {}
        _ => return Arg::Positional,
    }

    let separator = bytes.iter().position(|&byte| byte == b'=');
    let Ok(spelling) = std::str::from_utf8(&bytes[..separator.unwrap_or(bytes.len())]) else {
        return Arg::Positional;
    };
    let flag = match spelling {
        "-o" | "--output" => Flag::Output,
        "-r" | "--ref" => Flag::Reference,
        "-f" | "--format" => Flag::Format,
        "-x" | "--exclude" => Flag::Exclude,
        _ => return arg.to_str().map_or(Arg::Positional, Arg::Unknown),
    };
    let inline = if let Some(separator) = separator {
        let Some(value) = inline_value(arg, separator + 1) else {
            return Arg::Positional;
        };
        Some(value)
    } else {
        None
    };
    Arg::Flag {
        flag,
        spelling,
        inline,
    }
}

/// Borrows the value after the matched ASCII `flag=` prefix or reconstructs it losslessly.
fn inline_value(arg: &OsStr, value_start: usize) -> Option<Cow<'_, OsStr>> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Some(Cow::Borrowed(OsStr::from_bytes(
            &arg.as_bytes()[value_start..],
        )))
    }
    #[cfg(not(unix))]
    {
        if let Some(text) = arg.to_str() {
            return Some(Cow::Borrowed(OsStr::new(&text[value_start..])));
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::{OsStrExt, OsStringExt};
            // Every byte of the matched ASCII prefix is also one UTF-16 code unit.
            let value: Vec<_> = arg.encode_wide().skip(value_start).collect();
            Some(Cow::Owned(OsString::from_wide(&value)))
        }
        #[cfg(not(windows))]
        None
    }
}

/// Parses `args` (skipping the binary name) into a [`CliAction`].
///
/// # Errors
///
/// Returns [`DomainError::Usage`] for invalid CLI syntax or option values, or a source error
/// for an invalid or unsupported source or revision.
pub(crate) fn parse<I, T>(args: I) -> Result<CliAction, DomainError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let mut args = args.into_iter().map(Into::into);
    let _bin = args.next();

    // A help/version option can preempt the first error until `--` ends option parsing.
    let mut acc = Ok(Parsed::default());
    while let Some(arg) = args.next() {
        match classify_arg(&arg) {
            Arg::Separator => return args.try_fold(acc?, Parsed::with_positional)?.finish(),
            Arg::Help => return Ok(CliAction::Help),
            Arg::Version => return Ok(CliAction::Version),
            Arg::Flag {
                flag,
                inline: Some(value),
                ..
            } => {
                acc = acc.and_then(|parsed| parsed.with_value(flag, value.into_owned()));
            }
            Arg::Flag {
                flag,
                spelling,
                inline: None,
            } => {
                // The value position consumes one token even after an earlier error.
                let Some(value) = args.next() else {
                    return acc.and_then(|_| Err(missing_value(spelling)));
                };
                acc = acc.and_then(|parsed| {
                    reject_option_like(flag, spelling, &value)?;
                    parsed.with_value(flag, value)
                });
            }
            Arg::Unknown(s) => {
                acc = acc.and_then(|_| Err(DomainError::usage(format!("unknown option: {s}"))));
            }
            Arg::Positional => {
                acc = acc.and_then(|parsed| parsed.with_positional(arg));
            }
        }
    }
    acc?.finish()
}

/// Rejects dash-prefixed separate values except the stdout sentinel for output.
fn reject_option_like(flag: Flag, spelling: &str, val: &OsStr) -> Result<(), DomainError> {
    if val.as_encoded_bytes().starts_with(b"-") && !(flag == Flag::Output && val == STDOUT_ARG) {
        return Err(missing_value(spelling));
    }
    Ok(())
}

/// The one "a value is required" message, for a flag with no value or with an option-like value.
fn missing_value(flag_str: &str) -> DomainError {
    DomainError::usage(format!(
        "a value is required for '{flag_str}' but none was supplied"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::policy::ExcludePatterns;
    use crate::core::source::{GitRef, Source};

    /// An argument value that is not valid Unicode on the current platform.
    #[cfg(unix)]
    fn non_utf8_os_value() -> std::ffi::OsString {
        use std::os::unix::ffi::OsStringExt;
        std::ffi::OsString::from_vec(vec![0xff])
    }

    /// An argument value that is not valid Unicode on the current platform.
    #[cfg(windows)]
    fn non_utf8_os_value() -> std::ffi::OsString {
        use std::os::windows::ffi::OsStringExt;
        std::ffi::OsString::from_wide(&[0xD800])
    }

    fn parse_run<'a>(args: impl IntoIterator<Item = &'a str>) -> RunConfig {
        match parse(args).expect("expected parse to succeed") {
            CliAction::Run(config) => *config,
            other => panic!("expected Run action, got {other:?}"),
        }
    }

    #[test]
    fn completion_message_names_destination() {
        // The summary line is a README contract. The destination is echoed to the terminal,
        // so control characters in it must not reach the output.
        let cases = [
            (
                OutputTarget::File(PathBuf::from("digest.txt")),
                10,
                2,
                1500,
                "Wrote digest.txt (10 files, 2 skipped, ~1500 tokens)",
            ),
            (
                OutputTarget::Stdout,
                5,
                0,
                300,
                "Wrote to stdout (5 files, 0 skipped, ~300 tokens)",
            ),
            (
                OutputTarget::File(PathBuf::from("a\x1b[2J\nb.txt")),
                1,
                0,
                1,
                "Wrote a\u{FFFD}[2J b.txt (1 files, 0 skipped, ~1 tokens)",
            ),
        ];
        for (output, included_files, skipped_files, estimated_tokens, expected) in cases {
            let report = IngestReport {
                output,
                included_files,
                skipped_files,
                estimated_tokens,
            };
            assert_eq!(completion_message(&report), expected);
        }
    }

    #[test]
    fn cli_parses_valid_invocations() {
        let standard_cases: [&[&str]; 3] = [
            &["codefeed", ".", "--format", "xml", "--output", "-"],
            &["codefeed", "-f", "xml", "-o", "-", "."],
            &["codefeed", "--format=xml", "-o=-", "."],
        ];
        for args in standard_cases {
            let config = parse_run(args.iter().copied());
            assert!(matches!(config.source, Source::Local(_)));
            assert_eq!(config.format, DigestFormat::Xml);
            assert_eq!(config.output, OutputTarget::Stdout);
        }

        for path in ["-notes.txt", "", "a=b", "-/.", "--help"] {
            let arg = format!("--output={path}");
            let config = parse_run(["codefeed", ".", &arg]);
            assert!(
                matches!(config.output, OutputTarget::File(file) if file.as_os_str() == path),
                "{arg}"
            );
        }

        let config = parse_run(["codefeed", "--", "-weird-path"]);
        assert_eq!(config.source, Source::Local(PathBuf::from("-weird-path")));

        let config = parse_run(["codefeed", "-o", "-", "--", "-"]);
        assert_eq!(config.source, Source::Local(PathBuf::from("-")));
        assert_eq!(config.output, OutputTarget::Stdout);

        let config = parse_run([
            "codefeed",
            "https://github.com/rust-lang/cargo",
            "--ref=.", // Only the final revision is validated against the source.
            "--ref",
            "main",
        ]);
        assert_eq!(
            config.source,
            Source::github("rust-lang", "cargo", Some(GitRef::new("main").unwrap())).unwrap()
        );

        let config = parse_run(["codefeed", "https://github.com/a/b", "--ref=--version"]);
        assert_eq!(
            config.source,
            Source::github("a", "b", Some(GitRef::new("--version").unwrap())).unwrap()
        );
    }

    #[test]
    fn cli_rejects_invalid_invocations() {
        const GH: &str = "https://github.com/rust-lang/cargo";
        // (argv, fragment unique to the failing branch, optional fragment that must be absent).
        // Rows with the same flag but a different value shape exercise `reject_option_like`:
        // only `-o -` may take a bare dash, and inline `=` bypasses option classification.
        let cases: &[(&[&str], &str, Option<&str>)] = &[
            (&["codefeed"], "required arguments were not provided", None),
            (
                &["codefeed", "--ref", ""],
                "required arguments were not provided",
                None,
            ),
            (
                &["codefeed", "--output=--help"],
                "required arguments were not provided",
                None,
            ),
            (
                &["codefeed", "--ref=--version"],
                "required arguments were not provided",
                None,
            ),
            (
                &["codefeed", "src", "tests"],
                "unexpected argument 'tests'",
                None,
            ),
            (
                &["codefeed", "--", ".", "extra"],
                "unexpected argument 'extra'",
                None,
            ),
            (
                &["codefeed", ".", "--bogus"],
                "unknown option: --bogus",
                None,
            ),
            (
                &["codefeed", ".", "--unknown"],
                "unknown option: --unknown",
                Some("invalid source"),
            ),
            (&["codefeed", "--help=x"], "unknown option: --help=x", None),
            (&["codefeed", "-ox="], "unknown option: -ox=", None),
            (
                &["codefeed", "--format=xml=x"],
                "invalid value 'xml=x' for '--format",
                None,
            ),
            (
                &["codefeed", ".", "--exclude=[", "--format=yaml", "-f=text"],
                "invalid value 'yaml' for '--format",
                None,
            ),
            (
                &["codefeed", ".", "--exclude=[", "--ref=main"],
                "--ref is only valid for GitHub sources",
                None,
            ),
            (
                &["codefeed", ".", "--ref", "main"],
                "--ref is only valid for GitHub sources",
                None,
            ),
            (
                &["codefeed", ".", "--ref", ""],
                "--ref is only valid for GitHub sources",
                None,
            ),
            (
                &["codefeed", GH, "--ref", ""],
                "--ref cannot be empty",
                None,
            ),
            (
                &["codefeed", GH, "--ref", "   "],
                "--ref cannot be empty",
                None,
            ),
            (
                &["codefeed", GH, "-r", "-"],
                "a value is required for '-r'",
                None,
            ),
            (
                &["codefeed", GH, "--ref", "-"],
                "a value is required for '--ref'",
                None,
            ),
            (
                &["codefeed", "--ref", "--version"],
                "a value is required for '--ref'",
                None,
            ),
            (
                &["codefeed", ".", "--output"],
                "a value is required for '--output'",
                None,
            ),
            (
                &["codefeed", "--output", "--help"],
                "a value is required for '--output'",
                None,
            ),
            (
                &["codefeed", "src", "-o", "-x"],
                "a value is required for '-o'",
                None,
            ),
            (
                &["codefeed", "src", "-f", "-"],
                "a value is required for '-f'",
                None,
            ),
            (
                &["codefeed", ".", "--format"],
                "a value is required for '--format'",
                Some("invalid source"),
            ),
            (
                &["codefeed", "src", "--format", "-"],
                "a value is required for '--format'",
                None,
            ),
            (
                &["codefeed", ".", "-x"],
                "a value is required for '-x'",
                None,
            ),
            (
                &["codefeed", ".", "-x", "-foo"],
                "a value is required for '-x'",
                None,
            ),
            (
                &["codefeed", ".", "--exclude", "-"],
                "a value is required for '--exclude'",
                None,
            ),
            (
                &["codefeed", ".", "-f", "yaml"],
                "invalid value 'yaml' for '--format",
                None,
            ),
            (
                &["codefeed", "src", "--format=-x"],
                "invalid value '-x' for '--format",
                None,
            ),
            (
                &["codefeed", ".", "-x", "["],
                "invalid value '[' for '--exclude",
                None,
            ),
            (
                &["codefeed", ".", "--exclude", "#note"],
                "invalid value '#note' for '--exclude",
                None,
            ),
        ];
        for &(args, fragment, absent) in cases {
            let err = parse(args.iter().copied()).expect_err(&format!("{args:?}"));
            let rendered = err.to_string();
            assert!(rendered.contains(fragment), "{args:?}: {rendered}");
            if let Some(absent) = absent {
                assert!(!rendered.contains(absent), "{args:?}: {rendered}");
            }
        }
    }

    #[test]
    fn cli_argument_errors_are_single_line_without_control_characters() {
        let cases: [&[&str]; 3] = [
            &["codefeed", ".", "\x1b[2J"],
            &["codefeed", "--\x1b[2J"],
            &["codefeed", ".", "-f", "x\x1b"],
        ];
        for args in cases {
            let rendered = parse(args.iter().copied()).unwrap_err().to_string();
            assert!(
                !rendered.chars().any(char::is_control),
                "{args:?}: {rendered:?}"
            );
        }

        let err = parse(["codefeed", ".", "\x1b[2J"]).unwrap_err();
        assert_eq!(err.to_string(), "unexpected argument '\u{FFFD}[2J' found");
    }

    #[test]
    fn cli_control_flags_take_precedence_respecting_double_dash() {
        let action_cases: &[(&[&str], CliAction)] = &[
            (&["codefeed", "--help"], CliAction::Help),
            (&["codefeed", ".", "-h"], CliAction::Help),
            (&["codefeed", "--ref", "", "--help"], CliAction::Help),
            (&["codefeed", "--help", "--ref", ""], CliAction::Help),
            // Eagerly detected errors before the flag do not mask it either.
            (&["codefeed", "-f", "yaml", "--help"], CliAction::Help),
            (&["codefeed", "--bogus", "--help"], CliAction::Help),
            (&["codefeed", "a", "b", "--help"], CliAction::Help),
            (&["codefeed", "-x", "[", "-h"], CliAction::Help),
            (&["codefeed", "-h", "-V"], CliAction::Help),
            (&["codefeed", "--version"], CliAction::Version),
            (&["codefeed", ".", "-V"], CliAction::Version),
            // Version takes precedence over later and earlier argument errors.
            (&["codefeed", "--ref", "", "-V"], CliAction::Version),
            (&["codefeed", "--bogus", "-V"], CliAction::Version),
            // The first of `-h` and `-V` wins.
            (&["codefeed", "-V", "-h"], CliAction::Version),
        ];
        for &(args, ref expected) in action_cases {
            assert_eq!(parse(args.iter().copied()).unwrap(), *expected, "{args:?}");
        }

        // After `--`, `-h` and `-V` are positionals, so earlier errors or source parsing apply.
        for (args, fragment) in [
            (
                &["codefeed", "--", "a", "-h"][..],
                "unexpected argument '-h'",
            ),
            (
                &["codefeed", "--bogus", "--", "-h"][..],
                "unknown option: --bogus",
            ),
        ] {
            let err = parse(args.iter().copied()).unwrap_err();
            assert!(err.to_string().contains(fragment), "{args:?}: {err}");
        }

        let config = parse_run(["codefeed", "--", "-V"]);
        assert_eq!(config.source, Source::Local(PathBuf::from("-V")));
    }

    #[test]
    fn cli_separate_option_consumes_one_token_even_after_an_error() {
        assert_eq!(
            parse(["codefeed", "--bogus", "--output", "--", "--help"]).unwrap(),
            CliAction::Help
        );
        assert_eq!(
            parse(["codefeed", "--output", "--help", "-V"]).unwrap(),
            CliAction::Version
        );
        for args in [
            &["codefeed", "--bogus", "-o", "-h"][..],
            &["codefeed", "--bogus", "--output"][..],
        ] {
            let err = parse(args.iter().copied()).unwrap_err();
            assert!(err.to_string().contains("unknown option: --bogus"));
        }
    }

    #[test]
    fn cli_exclude_patterns_accumulate_in_argument_order() {
        let config = parse_run([
            "codefeed",
            ".",
            "-x",
            "build/",
            "--exclude",
            "*.log",
            "--exclude=*.tmp",
            "-x=target",
        ]);
        assert_eq!(
            config.exclude,
            ExcludePatterns::new(["build/", "*.log", "*.tmp", "target"]).unwrap()
        );

        let config = parse_run(["codefeed", ".", "--exclude=-foo"]);
        assert_eq!(config.exclude, ExcludePatterns::new(["-foo"]).unwrap());

        // Only the exclusion flag accumulates: a repeated `--format` keeps its last value.
        let config = parse_run(["codefeed", ".", "-f", "xml", "-f", "text"]);
        assert_eq!(config.format, DigestFormat::Text);
        assert_eq!(config.exclude, ExcludePatterns::default());
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn cli_non_utf8_flag_value_is_a_usage_error() {
        const GH: &str = "https://github.com/owner/repo";
        let cases = [
            (".", "-f", "invalid value '\u{FFFD}' for '--format"),
            (GH, "--ref", "git reference must be valid utf-8"),
            (".", "-x", "exclude pattern must be valid utf-8"),
        ];
        for (source, flag, fragment) in cases {
            let mut inline = OsString::from(format!("{flag}="));
            inline.push(non_utf8_os_value());
            let separate = [OsString::from(flag), non_utf8_os_value()];
            for values in [separate.as_slice(), std::slice::from_ref(&inline)] {
                let args = [OsStr::new("codefeed"), OsStr::new(source)]
                    .into_iter()
                    .chain(values.iter().map(OsString::as_os_str));
                let err = parse(args).unwrap_err();
                assert!(err.to_string().contains(fragment), "{flag}: {err}");
            }
        }
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn cli_output_non_utf8_dash_path_inline_vs_separate() {
        let mut path = OsString::from("-notes-");
        path.push(non_utf8_os_value());

        let mut arg = OsString::from("--output=");
        arg.push(&path);
        let CliAction::Run(config) =
            parse([OsStr::new("codefeed"), OsStr::new("."), &arg]).unwrap()
        else {
            panic!("expected Run action");
        };
        assert_eq!(config.output, OutputTarget::File(path.clone().into()));

        let args = [
            OsStr::new("codefeed"),
            OsStr::new("."),
            OsStr::new("-o"),
            &path,
        ];
        assert!(matches!(parse(args), Err(DomainError::Usage(_))));
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn cli_non_utf8_source_is_a_local_path() {
        for prefix in ["", "-", "--unknown="] {
            let mut raw_source = OsString::from(prefix);
            raw_source.push(non_utf8_os_value());
            let args = [OsStr::new("codefeed"), raw_source.as_os_str()];
            let CliAction::Run(config) = parse(args).expect("non-UTF-8 source is a path") else {
                panic!("expected Run action");
            };
            assert!(
                matches!(&config.source, Source::Local(path) if path.as_os_str() == raw_source),
                "expected local path source: {raw_source:?}"
            );
        }
    }
}
