//! GitHub collection orchestration: streaming tarball ingestion and HTTP transport.

mod client;
mod stream;

use crate::core::github::{GITHUB_API_BASE_URL, repository_url, tarball_url};
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
        return tarball_url(base.trim_end_matches('/'), owner, repo, reference);
    }
    tarball_url(GITHUB_API_BASE_URL, owner, repo, reference)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::testing::{MockServer, gzip_tarball, with_test_github_url};
    use tar::EntryType;

    fn tarball_response(gz: &[u8]) -> Vec<u8> {
        let mut resp = b"HTTP/1.1 200 OK\r\nContent-Type: application/x-gzip\r\nConnection: close\r\nContent-Length: "
            .to_vec();
        resp.extend_from_slice(gz.len().to_string().as_bytes());
        resp.extend_from_slice(b"\r\n\r\n");
        resp.extend_from_slice(gz);
        resp
    }

    #[test]
    fn collect_fetches_and_parses_remote_tarball() {
        let gz = gzip_tarball(&[("repo-root/file.txt", b"hello github", EntryType::Regular)]);
        let server = MockServer::spawn(1, move |_index, _req, _port| tarball_response(&gz));

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

    #[test]
    fn collect_percent_encodes_reference_in_request_path() {
        let gz = gzip_tarball(&[("repo-root/file.txt", b"hello github", EntryType::Regular)]);
        let server = MockServer::spawn(1, move |_index, _req, _port| tarball_response(&gz));

        let result = with_test_github_url(server.url(""), || {
            collect(
                "test-owner",
                "test-repo",
                Some("feature/x"),
                &ExcludePatterns::default(),
            )
        })
        .expect("github collection succeeds");

        assert_eq!(result.files.len(), 1);
        let (requests, _) = server.join();
        assert!(requests[0].contains("/repos/test-owner/test-repo/tarball/feature%2Fx"));
    }

    #[test]
    fn collect_reports_remote_error_status() {
        let server = MockServer::spawn(1, |_index, _req, _port| {
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nnot found"
                .to_vec()
        });

        let Err(err) = with_test_github_url(server.url(""), || {
            collect("test-owner", "test-repo", None, &ExcludePatterns::default())
        }) else {
            panic!("expected 404 to fail collection");
        };
        assert!(matches!(err, IngestError::Api(_)), "{err}");
        server.join();
    }

    #[test]
    fn tarball_url_defaults_to_github_api_origin_without_override() {
        let url = crate::shell::testing::with_default_github_origin(|| {
            resolve_tarball_url("test-owner", "test-repo", Some("main"))
        });
        assert_eq!(
            url,
            tarball_url(GITHUB_API_BASE_URL, "test-owner", "test-repo", Some("main"))
        );
    }
}
