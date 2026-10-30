//! GitHub collection orchestration: streaming tarball ingestion and HTTP transport.

mod client;
mod stream;

use crate::core::github::{repository_url, tarball_url};
use crate::core::model::CollectionResult;
use crate::core::policy::ExcludePatterns;
use crate::shell::IngestError;
use client::GithubClient;

/// Collects files from a remote GitHub repository via streaming tarball download.
pub(super) fn collect(
    owner: &str,
    repo: &str,
    reference: Option<&str>,
    exclude: &ExcludePatterns,
) -> Result<CollectionResult, IngestError> {
    let client = GithubClient::new();
    let url = tarball_url(owner, repo, reference);
    let reader = client.stream_url(&url)?;
    let label = repository_url(owner, repo);
    super::archive::collect(reader, &label, reference, exclude)
}
