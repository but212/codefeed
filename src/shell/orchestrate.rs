//! Pipeline orchestration: coordinates collection, emission, and digest rendering.

use crate::core::decide::{Item, Selector};
use crate::core::error::IngestError;
use crate::core::model::{CollectionResult, IngestReport, RunConfig, Source};
use crate::shell;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamInfo {
    pub label: String,
    pub revision: Option<String>,
}

pub trait SourceStream {
    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, None)
    }
    fn for_each_item<F>(self, callback: F) -> Result<StreamInfo, IngestError>
    where
        F: FnMut(Item<'_>) -> Result<(), IngestError>;
}

pub fn collect_stream<S: SourceStream>(stream: S) -> Result<CollectionResult, IngestError> {
    let mut selector = Selector::from_size_hint(stream.size_hint());
    let info = stream.for_each_item(|item| selector.decide(item))?;
    Ok(selector.finish(info.label, info.revision))
}

pub fn ingest(config: &RunConfig) -> Result<IngestReport, IngestError> {
    shell::output::validate_target(&config.output)?;

    let collection = match &config.source {
        Source::Local(path) => shell::local::collect(path, &config.output)?,
        Source::Github {
            owner,
            repo,
            reference,
        } => shell::github::collect(owner, repo, reference.as_deref())?,
    };

    let estimated_tokens = shell::output::emit(&config.output, &collection, config.format)?;

    Ok(IngestReport {
        output: config.output.clone(),
        included_files: collection.files.len(),
        skipped_files: collection.skipped,
        estimated_tokens,
    })
}
