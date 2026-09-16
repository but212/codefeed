mod cli;
mod collect;
mod decide;
mod digest;
mod error;
pub(crate) mod model;
mod output;
mod source;

#[cfg(test)]
#[path = "../tests/unit/mod.rs"]
mod tests;

use std::ffi::OsStr;

pub use crate::error::IngestError;
pub(crate) use crate::model::{
    CandidateBudget, CollectionResult, MAX_FILE_SIZE, MAX_JSON_SIZE, MAX_OUTPUT_SIZE,
    MAX_REMOTE_IGNORE_BYTES, MAX_REMOTE_IGNORE_FILES, REMOTE_ROOT, text_content,
};
pub use crate::model::{IngestReport, Options, Source};

pub fn parse_source(value: &str) -> Result<Source, IngestError> {
    source::parse_source(value)
}

pub(crate) fn parse_source_os(value: &OsStr) -> Result<Source, IngestError> {
    source::parse_source_os(value)
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
        Source::Local(root) => collect::local::collect_local(root, &options.output)?,
        Source::GitHub { owner, repo } => collect::github::collect_github(owner, repo, options)?,
    };

    let digest = match &collection {
        CollectionResult::Local {
            label,
            files,
            skipped,
        } => digest::render_digest(label, None, files, *skipped)?,
        CollectionResult::GitHub {
            label,
            revision,
            files,
            skipped,
        } => digest::render_digest(label, Some(revision), files, *skipped)?,
    };
    output::write_atomic(&options.output, digest.as_bytes())?;

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
    cli::help_text()
}

pub fn run_cli<I, T>(args: I) -> Result<IngestReport, IngestError>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString>,
{
    cli::run_cli(args)
}
