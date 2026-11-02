//! Imperative shell: process, filesystem, network, and orchestration.
//!
//! Shell modules perform all effects and feed observations to the functional
//! core in `crate::core`.

use crate::core::github::GITHUB_API_BASE_URL;
use crate::{IngestReport, RunConfig, Source};

mod atomic;
pub(crate) mod cli;
mod error;
mod github;
mod local;
mod output;
mod stdio;
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
    ingest_from(config, GITHUB_API_BASE_URL)
}

/// [`ingest`] with the GitHub API origin passed in, so tests can serve GitHub sources locally.
fn ingest_from(config: &RunConfig, github_api_base: &str) -> Result<IngestReport, IngestError> {
    let resolved_output = output::resolve_output_target(&config.output)?;

    let collection = match &config.source {
        Source::Local(path) => local::collect(path, &resolved_output, &config.exclude)?,
        Source::Github {
            owner,
            repo,
            reference,
        } => github::collect(
            github_api_base,
            owner,
            repo,
            reference.as_deref(),
            &config.exclude,
        )?,
    };

    let rendered = collection.render(config.format)?;
    output::deliver(&resolved_output, &rendered.bytes)?;

    Ok(IngestReport {
        output: config.output.clone(),
        included_files: rendered.included_files,
        skipped_files: rendered.skipped_files,
        estimated_tokens: rendered.estimated_tokens,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OutputTarget;
    use crate::core::model::DigestFormat;
    use crate::core::source::GitRef;
    use crate::shell::testing::{MockServer, gzip_tarball, temp_dir};
    use std::fs;
    use tar::EntryType;

    #[test]
    fn ingest_report_output_keeps_the_requested_path() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("report_requested_path")?;
        fs::write(root.join("main.rs"), "fn main() {}")?;
        // `.` is not normalized away: the report names the destination exactly as requested.
        let requested = root.join(".").join("digest.txt");
        let config = RunConfig::new(
            Source::Local(root.to_path_buf()),
            OutputTarget::File(requested.clone()),
            DigestFormat::Text,
        );

        let report = ingest(&config)?;

        assert_eq!(report.output, OutputTarget::File(requested));
        Ok(())
    }

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

        let report =
            ingest_from(&config, &server.url("")).expect("ingest should succeed for github source");

        assert_eq!(report.included_files, 1);
        assert_eq!(report.skipped_files, 0);

        let (_, count) = server.join();
        assert_eq!(count, 1);
    }
}
