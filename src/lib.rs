mod core;
mod shell;

use std::ffi::{OsStr, OsString};

pub use crate::core::error::IngestError;
pub use crate::core::model::{IngestReport, Options, OutputTarget, Source};

pub fn parse_source(value: &str) -> Result<Source, IngestError> {
    core::source::parse_source(value)
}

pub(crate) fn parse_source_os(value: &OsStr) -> Result<Source, IngestError> {
    core::source::parse_source_os(value)
}

pub fn ingest(source: &Source, options: &Options) -> Result<IngestReport, IngestError> {
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
        Source::Local(root) => shell::local::collect_local(root, &options.output)?,
        Source::GitHub { owner, repo } => {
            shell::github::collect_github(owner, repo, options.reference.as_deref())?
        }
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

    let digest = core::digest::render_digest(&label, revision.as_deref(), &files, skipped)?;
    shell::output::emit(&options.output, &digest)?;

    Ok(IngestReport {
        output: options.output.clone(),
        included_files: files.len(),
        skipped_files: skipped,
    })
}

pub fn help_text() -> &'static str {
    shell::cli::help_text()
}

pub fn requests_help(args: &[OsString]) -> bool {
    shell::cli::requests_help(args)
}

pub fn run_cli<I, T>(args: I) -> Result<IngestReport, IngestError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    shell::cli::run_cli(args)
}
