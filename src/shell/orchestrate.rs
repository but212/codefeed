//! Pipeline orchestration: coordinates collection, emission, and digest rendering.

use crate::core::error::IngestError;
use crate::core::model::{CollectionResult, IngestReport, Options, Source};
use crate::shell;

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

    let CollectionResult {
        label,
        revision,
        files,
        skipped,
    } = collection;

    let mut estimated_tokens = 0;
    shell::output::emit(&options.output, |writer| {
        estimated_tokens = crate::core::digest::render_digest_to_writer(
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
