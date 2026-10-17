//! Pipeline orchestration: coordinates collection, emission, and digest rendering.

use crate::core::error::IngestError;
use crate::core::model::{CollectionResult, IngestReport, RunConfig, Source};
use crate::core::selector::{CandidateInput, Selector};
use crate::shell;

/// Identification metadata yielded by a completed source stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StreamInfo {
    /// Display label for the repository or directory root.
    pub(crate) label: String,
    /// Optional commit hash or version tag.
    pub(crate) revision: Option<String>,
}

/// Abstraction for streaming source items into the pure selector.
pub(crate) trait SourceStream {
    /// Returns lower and optional upper bounds on the number of candidate items.
    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, None)
    }

    /// Iterates over all candidate items, passing each to the callback closure.
    fn for_each_item<F>(self, callback: F) -> Result<StreamInfo, IngestError>
    where
        F: FnMut(CandidateInput<'_>) -> Result<(), IngestError>;
}

/// Consumes a [`SourceStream`] into a [`Selector`] and returns the accumulated [`CollectionResult`].
pub(crate) fn collect_stream<S: SourceStream>(stream: S) -> Result<CollectionResult, IngestError> {
    let mut selector = Selector::from_size_hint(stream.size_hint());
    let info = stream.for_each_item(|item| selector.feed(item))?;
    Ok(selector.finish(info.label, info.revision))
}

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
