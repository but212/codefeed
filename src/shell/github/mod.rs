//! GitHub collection orchestration: tarball URL, HTTP stream, and archive ingestion.

#[cfg(not(test))]
mod archive;
#[cfg(test)]
pub(in crate::shell) mod archive;
mod client;

use crate::core::github::{repository_url, tarball_url};
use crate::core::model::CollectionResult;
use crate::core::policy::ExcludePatterns;
use crate::core::source::GithubName;
use crate::shell::error::IngestError;
use client::GithubClient;

/// Collects files from a remote GitHub repository via streaming tarball download.
/// `api_base` is the API origin, `GITHUB_API_BASE_URL` in production.
pub(super) fn collect(
    api_base: &str,
    owner: &GithubName,
    repo: &GithubName,
    reference: Option<&str>,
    exclude: &ExcludePatterns,
) -> Result<CollectionResult, IngestError> {
    let client = GithubClient::new();
    let url = tarball_url(api_base, owner, repo, reference);
    let reader = client.stream_url(&url)?;
    let label = repository_url(owner, repo);
    archive::collect(reader, label, reference, exclude)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::testing::{MockServer, gzip_tarball, tarball_response};
    use tar::EntryType;

    #[test]
    fn collect_fetches_and_parses_remote_tarball() {
        let gz_basic = gzip_tarball(&[("repo-root/file.txt", b"hello github", EntryType::Regular)]);
        let gz_pax = gzip_tarball(&[
            (
                "pax_global_header",
                b"52 comment=c1863c8b05c5d2db4030bc8c8a7d2f10bda4a950\n",
                EntryType::XGlobalHeader,
            ),
            ("repo-root/src/main.rs", b"fn main() {}", EntryType::Regular),
            ("repo-root/app.log", b"ignored log", EntryType::Regular),
        ]);
        let server = MockServer::spawn(2, move |index, _req, _port| match index {
            0 => tarball_response(&gz_basic),
            _ => tarball_response(&gz_pax),
        });

        let owner = GithubName::new("test-owner").unwrap();
        let repo = GithubName::new("test-repo").unwrap();
        let basic = collect(
            &server.url(""),
            &owner,
            &repo,
            Some("main"),
            &ExcludePatterns::default(),
        )
        .expect("github collection succeeds");
        assert_eq!(basic.label, "https://github.com/test-owner/test-repo");
        assert_eq!(basic.revision.as_deref(), Some("main"));
        let basic_files: Vec<_> = basic
            .files
            .iter()
            .map(|file| (file.path, file.content))
            .collect();
        assert_eq!(basic_files, [("file.txt", "hello github")]);

        let exclude = ExcludePatterns::new(["*.log"]).expect("valid exclude");
        let with_pax = collect(&server.url(""), &owner, &repo, Some("feature/x"), &exclude)
            .expect("github collection succeeds");
        assert_eq!(
            with_pax.revision.as_deref(),
            Some("c1863c8b05c5d2db4030bc8c8a7d2f10bda4a950")
        );
        let pax_files: Vec<_> = with_pax.files.iter().map(|file| file.path).collect();
        assert_eq!(pax_files, ["src/main.rs"]);

        let requests = server.join();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].contains("/repos/test-owner/test-repo/tarball/main"));
        assert!(requests[1].contains("/repos/test-owner/test-repo/tarball/feature%2Fx"));
    }

    #[test]
    fn collect_reports_remote_error_status() {
        let server = MockServer::spawn(1, |_index, _req, _port| {
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nnot found"
                .to_vec()
        });

        let owner = GithubName::new("test-owner").unwrap();
        let repo = GithubName::new("test-repo").unwrap();
        let Err(err) = collect(
            &server.url(""),
            &owner,
            &repo,
            None,
            &ExcludePatterns::default(),
        ) else {
            panic!("expected 404 to fail collection");
        };
        assert!(matches!(err, IngestError::Api(_)), "{err}");
        let requests = server.join();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].split_ascii_whitespace().nth(1),
            Some("/repos/test-owner/test-repo/tarball")
        );
    }

    #[test]
    fn stream_drop_mid_body_is_http() {
        let gz_bytes = gzip_tarball(&[("repo-root/file.txt", &b"hello"[..], EntryType::Regular)]);
        let truncated = gz_bytes[..16].to_vec();

        let server = MockServer::spawn(1, move |_index, _req, _port| {
            let mut response =
                b"HTTP/1.1 200 OK\r\nContent-Length: 4096\r\nConnection: close\r\n\r\n".to_vec();
            response.extend_from_slice(&truncated);
            response
        });

        let owner = GithubName::new("test-owner").unwrap();
        let repo = GithubName::new("test-repo").unwrap();
        let Err(err) = collect(
            &server.url(""),
            &owner,
            &repo,
            None,
            &ExcludePatterns::default(),
        ) else {
            panic!("expected error");
        };
        assert!(matches!(err, IngestError::Http(_)), "{err}");

        server.join();
    }
}
