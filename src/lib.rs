mod core;
mod shell;

use std::ffi::{OsStr, OsString};

use crate::core::model::CollectionResult;

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

    let digest = match &collection {
        CollectionResult::Local {
            label,
            files,
            skipped,
        } => core::digest::render_digest(label, None, files, *skipped)?,
        CollectionResult::GitHub {
            label,
            revision,
            files,
            skipped,
        } => core::digest::render_digest(label, Some(revision), files, *skipped)?,
    };
    shell::output::emit(&options.output, &digest)?;

    let (included_files, skipped_files) = match &collection {
        CollectionResult::Local { files, skipped, .. }
        | CollectionResult::GitHub { files, skipped, .. } => (files.len(), *skipped),
    };
    Ok(IngestReport {
        output: options.output.clone(),
        included_files,
        skipped_files,
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
    T: Into<std::ffi::OsString>,
{
    shell::cli::run_cli(args)
}
