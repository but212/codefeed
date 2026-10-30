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
        Source::Local(path) => local::collect(path, &resolved_output, &config.exclude)?,
        Source::Github {
            owner,
            repo,
            reference,
        } => github::collect(owner, repo, reference.as_deref(), &config.exclude)?,
    };

    let rendered = collection.render(config.format)?;
    output::deliver(&resolved_output, &rendered.bytes)?;

    Ok(rendered.into_report(config.output.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OutputTarget;
    use crate::core::model::{DigestFormat, GitRef};
    use crate::shell::testing::{MockServer, gzip_tarball, with_test_github_url};
    use tar::EntryType;

    #[test]
    fn ingest_github_source_succeeds() {
        let gz = gzip_tarball(&[("repo-root/main.rs", b"fn main() {}", EntryType::Regular)]);
        let server = MockServer::spawn(1, move |_index, _req, _port| {
            let mut resp = b"HTTP/1.1 200 OK\r\nContent-Type: application/x-gzip\r\nConnection: close\r\nContent-Length: "
                .to_vec();
            resp.extend_from_slice(gz.len().to_string().as_bytes());
            resp.extend_from_slice(b"\r\n\r\n");
            resp.extend_from_slice(&gz);
            resp
        });

        let source = Source::github("owner", "repo", Some(GitRef::new("v1.0.0").unwrap())).unwrap();

        let config = RunConfig::new(source, OutputTarget::Stdout, DigestFormat::Text);

        let report = with_test_github_url(server.url(""), || ingest(&config))
            .expect("ingest should succeed for github source");

        assert_eq!(report.included_files, 1);
        assert_eq!(report.skipped_files, 0);

        let (_, count) = server.join();
        assert_eq!(count, 1);
    }
}
