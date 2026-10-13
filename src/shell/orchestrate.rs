//! Pipeline orchestration: coordinates collection, emission, and digest rendering.

use crate::core::error::IngestError;
use crate::core::model::{IngestReport, RunConfig};
use crate::shell;
use crate::shell::collector::{AnyCollector, Collector};

pub fn ingest(config: &RunConfig) -> Result<IngestReport, IngestError> {
    let collector = AnyCollector::from_config(config);
    let collection = collector.collect()?;

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
