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
    let url = resolve_tarball_url(owner, repo, reference);
    let reader = client.stream_url(&url)?;
    let label = repository_url(owner, repo);
    super::archive::collect(reader, &label, reference, exclude)
}

fn resolve_tarball_url(owner: &str, repo: &str, reference: Option<&str>) -> String {
    #[cfg(test)]
    if let Some(base) = super::testing::get_test_github_url() {
        let mut url = base.trim_end_matches('/').to_string();
        for segment in ["repos", owner, repo, "tarball"] {
            url = format!("{url}/{segment}");
        }
        if let Some(reference) = reference {
            url = format!("{url}/{reference}");
        }
        return url;
    }
    tarball_url(owner, repo, reference)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::testing::{MockServer, gzip_tarball, with_test_github_url};
    use tar::EntryType;

    #[test]
    fn collect_fetches_and_parses_remote_tarball() {
        let gz = gzip_tarball(&[("repo-root/file.txt", b"hello github", EntryType::Regular)]);
        let server = MockServer::spawn(1, move |_index, _req, _port| {
            let mut resp = b"HTTP/1.1 200 OK\r\nContent-Type: application/x-gzip\r\nConnection: close\r\nContent-Length: "
                .to_vec();
            resp.extend_from_slice(gz.len().to_string().as_bytes());
            resp.extend_from_slice(b"\r\n\r\n");
            resp.extend_from_slice(&gz);
            resp
        });

        let result = with_test_github_url(server.url(""), || {
            collect(
                "test-owner",
                "test-repo",
                Some("main"),
                &ExcludePatterns::default(),
            )
        })
        .expect("github collection succeeds");

        assert_eq!(result.label, "https://github.com/test-owner/test-repo");
        assert_eq!(result.files.len(), 1);
        let first = result.files.iter().next().unwrap();
        assert_eq!(first.path, "file.txt");
        assert_eq!(first.content, "hello github");

        let (requests, count) = server.join();
        assert_eq!(count, 1);
        assert!(requests[0].contains("/repos/test-owner/test-repo/tarball/main"));
    }
}
