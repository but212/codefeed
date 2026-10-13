//! Pipeline orchestration: coordinates collection, emission, and digest rendering.

use crate::core::error::IngestError;
use crate::core::model::{IngestReport, RunConfig, Source};
use crate::shell;

pub fn ingest(config: &RunConfig) -> Result<IngestReport, IngestError> {
    let collection = match &config.source {
        Source::Local(path) => shell::local::collect_local(path, &config.output, config.format)?,
        Source::GitHub {
            owner,
            repo,
            reference,
        } => shell::github::collect_github(owner, repo, reference.as_deref(), config.format)?,
    };

    let estimated_tokens = shell::output::emit(&config.output, |writer| {
        crate::core::digest::render_digest_to_writer(writer, &collection)
    })?;

    Ok(IngestReport {
        output: config.output.clone(),
        included_files: collection.files.len(),
        skipped_files: collection.skipped,
        estimated_tokens,
    })
}
