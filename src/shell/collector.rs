//! Collector abstraction and unified source ingestion dispatch.

use super::github::GitHubCollector;
use super::local::LocalCollector;
use crate::core::error::IngestError;
use crate::core::model::{CollectionResult, RunConfig, Source};

/// Common collector trait for directory or remote repository ingestion.
pub trait Collector {
    fn collect(&self) -> Result<CollectionResult, IngestError>;
}

/// Static dispatch enum covering all supported ingestion source types.
pub enum AnyCollector {
    Local(LocalCollector),
    GitHub(GitHubCollector),
}

impl AnyCollector {
    pub fn from_config(config: &RunConfig) -> Self {
        match &config.source {
            Source::Local(path) => Self::Local(LocalCollector::new(
                path.clone(),
                config.output.clone(),
                config.format,
            )),
            Source::GitHub {
                owner,
                repo,
                reference,
            } => Self::GitHub(GitHubCollector::new(
                owner.clone(),
                repo.clone(),
                reference.clone(),
                config.format,
            )),
        }
    }
}

impl Collector for AnyCollector {
    fn collect(&self) -> Result<CollectionResult, IngestError> {
        match self {
            Self::Local(collector) => collector.collect(),
            Self::GitHub(collector) => collector.collect(),
        }
    }
}
