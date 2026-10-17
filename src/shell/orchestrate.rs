//! Pipeline orchestration: coordinates collection, emission, and digest rendering.

use crate::core::error::IngestError;
use crate::core::model::{IngestReport, RunConfig, Source};
use crate::shell;

/// Executes the end-to-end repository ingestion pipeline.
///
/// Dispatches collection across local filesystem or GitHub remote sources, renders
/// the digest into the target format (plain text or XML), and atomically emits the
/// result to standard output or a destination file.
///
/// # Errors
///
/// Returns [`IngestError`] if input sources are invalid, candidate budgets or output size
/// limits are exceeded, filesystem operations fail, or network requests error.
pub fn ingest(config: &RunConfig) -> Result<IngestReport, IngestError> {
    let resolved_output = shell::output::resolve_output_target(&config.output)?;

    let collection = match &config.source {
        Source::Local(path) => shell::local::collect(path, resolved_output.as_deref())?,
        Source::Github {
            owner,
            repo,
            reference,
        } => shell::github::collect(owner, repo, reference.as_deref())?,
    };

    let estimated_tokens =
        shell::output::emit(resolved_output.as_deref(), &collection, config.format)?;

    Ok(IngestReport {
        output: config.output.clone(),
        included_files: collection.files.len(),
        skipped_files: collection.skipped,
        estimated_tokens,
    })
}
