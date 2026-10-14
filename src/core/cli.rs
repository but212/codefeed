use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Parser, ValueEnum};

use crate::core::error::IngestError;
use crate::core::model::{DigestFormat, OutputTarget, RunConfig};
use crate::core::source::parse_source_os;

const HELP: &str =
    "Usage: codefeed <SOURCE> [--output <PATH|->] [--ref <REF>] [--format <text|xml>]";

pub fn help_text() -> &'static str {
    HELP
}

#[derive(Debug, PartialEq, Eq)]
pub enum CliAction {
    Help,
    Run(RunConfig),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum FormatArg {
    Text,
    Xml,
}

#[derive(Parser, Debug)]
#[command(
    name = "codefeed",
    override_usage = "codefeed <SOURCE> [--output <PATH|->] [--ref <REF>] [--format <text|xml>]",
    disable_help_flag = true
)]
struct Opts {
    #[arg(short = 'h', long = "help", help = "Print help")]
    help: bool,

    #[arg(value_name = "SOURCE")]
    source: Option<OsString>,

    #[arg(
        short = 'o',
        long = "output",
        default_value = "digest.txt",
        value_name = "PATH|->"
    )]
    output: OsString,

    #[arg(short = 'r', long = "ref", value_name = "REF")]
    reference: Option<String>,

    #[arg(
        short = 'f',
        long = "format",
        default_value = "text",
        value_name = "text|xml"
    )]
    format: FormatArg,
}

pub fn parse_cli<I, T>(args: I) -> Result<CliAction, IngestError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let args_vec: Vec<OsString> = args.into_iter().map(Into::into).collect();

    let opts = match Opts::try_parse_from(&args_vec) {
        Ok(opts) => opts,
        Err(err) => return Err(IngestError::Usage(err.to_string())),
    };

    if opts.help {
        return Ok(CliAction::Help);
    }

    let Some(source_value) = opts.source else {
        return Err(IngestError::Usage(format!("missing source\n\n{HELP}")));
    };

    let output = if source_value != "-" && opts.output == "-" {
        OutputTarget::Stdout
    } else {
        OutputTarget::File(PathBuf::from(opts.output))
    };

    let format = match opts.format {
        FormatArg::Text => DigestFormat::Text,
        FormatArg::Xml => DigestFormat::Xml,
    };

    let source = parse_source_os(&source_value)?.with_reference(opts.reference)?;

    Ok(CliAction::Run(RunConfig::new(source, output, format)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::Source;

    #[test]
    fn cli_parses_valid_invocations() {
        assert_eq!(parse_cli(["codefeed", "--help"]).unwrap(), CliAction::Help);
        assert_eq!(parse_cli(["codefeed", ".", "-h"]).unwrap(), CliAction::Help);

        let run_standard =
            parse_cli(["codefeed", ".", "--format", "xml", "--output", "-"]).unwrap();
        match run_standard {
            CliAction::Run(config) => {
                assert!(matches!(config.source, Source::Local(_)));
                assert_eq!(config.format, DigestFormat::Xml);
                assert_eq!(config.output, OutputTarget::Stdout);
            }
            CliAction::Help => panic!("expected Run action"),
        }

        let run_short = parse_cli(["codefeed", "-f", "xml", "-o", "-", "."]).unwrap();
        match run_short {
            CliAction::Run(config) => {
                assert!(matches!(config.source, Source::Local(_)));
                assert_eq!(config.format, DigestFormat::Xml);
                assert_eq!(config.output, OutputTarget::Stdout);
            }
            CliAction::Help => panic!("expected Run action"),
        }

        let run_eq = parse_cli(["codefeed", "--format=xml", "-o=-", "."]).unwrap();
        match run_eq {
            CliAction::Run(config) => {
                assert!(matches!(config.source, Source::Local(_)));
                assert_eq!(config.format, DigestFormat::Xml);
                assert_eq!(config.output, OutputTarget::Stdout);
            }
            CliAction::Help => panic!("expected Run action"),
        }

        let run_delimiter = parse_cli(["codefeed", "--", "-weird-path"]).unwrap();
        match run_delimiter {
            CliAction::Run(config) => {
                assert_eq!(config.source, Source::Local(PathBuf::from("-weird-path")));
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
            CliAction::Run(config) => {
                assert_eq!(
                    config.source,
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
