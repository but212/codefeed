//! Pipeline orchestration: coordinates collection, emission, and digest rendering.

use crate::core::decide::{Item, Selector};
use crate::core::error::IngestError;
use crate::core::model::{CollectionResult, IngestReport, RunConfig, Source};
use crate::shell;

pub trait SourceStream: Iterator<Item = Result<Item, IngestError>> {
    fn label(&self) -> &str;
    fn revision(&self) -> Option<&str> {
        None
    }
}

pub fn collect_stream<S: SourceStream>(mut stream: S) -> Result<CollectionResult, IngestError> {
    let mut selector = Selector::from_size_hint(stream.size_hint());
    for item_res in stream.by_ref() {
        selector.decide(item_res?)?;
    }
    let label = stream.label().to_string();
    let revision = stream.revision().map(str::to_string);
    Ok(selector.finish(label, revision))
}

pub fn ingest(config: &RunConfig) -> Result<IngestReport, IngestError> {
    let collection = match &config.source {
        Source::Local(path) => {
            let stream = shell::local::LocalStream::new(path, &config.output)?;
            collect_stream(stream)?
        }
        Source::GitHub {
            owner,
            repo,
            reference,
        } => {
            let client = shell::github_client::GithubClient::new();
            let reader = client.tarball_stream(owner, repo, reference.as_deref())?;
            let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(reader));
            let stream =
                shell::github::GitHubStream::new(&mut archive, owner, repo, reference.as_deref())?;
            collect_stream(stream)?
        }
    };

    let estimated_tokens = shell::output::emit(&config.output, |writer| {
        crate::core::digest::render_digest_to_writer(writer, &collection, config.format)
    })?;

    Ok(IngestReport {
        output: config.output.clone(),
        included_files: collection.files.len(),
        skipped_files: collection.skipped,
        estimated_tokens,
    })
}
