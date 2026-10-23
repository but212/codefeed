//! Imperative shell: process, filesystem, network, and orchestration.
//!
//! Shell modules perform all effects and feed observations to the functional
//! core in `crate::core`.

use crate::{IngestReport, RunConfig, Source};

mod archive;
mod atomic;
pub(crate) mod cli;
mod error;
mod github;
mod local;
mod output;
#[cfg(test)]
mod testing;

pub use error::IngestError;

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
    let resolved_output = output::resolve_output_target(&config.output)?;

    let collection = match &config.source {
        Source::Local(path) => local::collect(path, &resolved_output)?,
        Source::Github {
            owner,
            repo,
            reference,
        } => github::collect(owner, repo, reference.as_deref())?,
    };

    let rendered = collection.render(config.format)?;
    output::deliver(&resolved_output, &rendered.bytes)?;

    Ok(rendered.into_report(config.output.clone()))
}
