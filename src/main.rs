//! Command-line binary entry point for rustingest.

mod core;
mod shell;

use std::env::args_os;
use std::process;

use crate::core::error::IngestError;
use crate::core::model::{IngestReport, Options, OutputTarget, Source};
use crate::shell::cli::{help_text, requests_help, run_cli};

pub(crate) fn ingest(source: &Source, options: &Options) -> Result<IngestReport, IngestError> {
    if options
        .reference
        .as_ref()
        .is_some_and(|reference| reference.trim().is_empty())
    {
        return Err(IngestError::InvalidSource("--ref cannot be empty".into()));
    }
    if matches!(source, Source::Local(_)) && options.reference.is_some() {
        return Err(IngestError::InvalidSource(
            "--ref is only valid for GitHub sources".into(),
        ));
    }

    let collection = match source {
        Source::Local(root) => shell::local::collect_local(root, &options.output, options.format)?,
        Source::GitHub { owner, repo } => shell::github::collect_github(
            owner,
            repo,
            options.reference.as_deref(),
            options.format,
        )?,
    };

    let (label, revision, files, skipped) = match collection {
        core::model::CollectionResult::Local {
            label,
            files,
            skipped,
        } => (label, None, files, skipped),
        core::model::CollectionResult::GitHub {
            label,
            revision,
            files,
            skipped,
        } => (label, Some(revision), files, skipped),
    };

    let mut estimated_tokens = 0;
    shell::output::emit(&options.output, |writer| {
        estimated_tokens = core::digest::render_digest_to_writer(
            writer,
            &label,
            revision.as_deref(),
            &files,
            skipped,
            options.format,
        )?;
        Ok(())
    })?;

    Ok(IngestReport {
        output: options.output.clone(),
        included_files: files.len(),
        skipped_files: skipped,
        estimated_tokens,
    })
}

fn main() {
    let args: Vec<_> = args_os().collect();
    if requests_help(&args) {
        println!("{}", help_text());
        return;
    }

    match run_cli(args) {
        // With `--output -` the digest owns stdout, so the completion message
        // goes to stderr there and stays on stdout for a file target.
        Ok(report) => match &report.output {
            OutputTarget::File(path) => println!(
                "Wrote {} ({} files, {} skipped, ~{} tokens)",
                path.display(),
                report.included_files,
                report.skipped_files,
                report.estimated_tokens
            ),
            OutputTarget::Stdout => eprintln!(
                "Wrote to stdout ({} files, {} skipped, ~{} tokens)",
                report.included_files, report.skipped_files, report.estimated_tokens
            ),
        },
        Err(error) => {
            eprintln!("error: {error}");
            process::exit(1);
        }
    }
}
