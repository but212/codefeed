//! Pipeline orchestration: coordinates collection, emission, and digest rendering.

use crate::core::error::IngestError;
use crate::core::model::{IngestReport, Options, Source};
use crate::shell;

pub fn ingest(source: &Source, options: &Options) -> Result<IngestReport, IngestError> {
    let collection = match source {
        Source::Local(root) => shell::local::collect_local(root, &options.output, options.format)?,
        Source::GitHub {
            owner,
            repo,
            reference,
        } => shell::github::collect_github(owner, repo, reference.as_deref(), options.format)?,
    };

    let estimated_tokens = shell::output::emit(&options.output, |writer| {
        crate::core::digest::render_digest_to_writer(writer, &collection, options.format)
    })?;

    Ok(IngestReport {
        output: options.output.clone(),
        included_files: collection.files.len(),
        skipped_files: collection.skipped,
        estimated_tokens,
    })
}
