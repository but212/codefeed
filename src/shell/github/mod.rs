//! GitHub collection orchestration: tarball URL, HTTP retry loop, and streaming ingestion.

pub(in crate::shell) mod archive;
mod client;

use std::io::Read;
use std::time::Duration;

use zeroize::Zeroizing;

use crate::core::collection::CollectionResult;
use crate::core::github::{
    AttemptState, Auth, RateLimitHeaders, RequestOutcome, Transition, TransportKind, api_error,
    repository_url, status_retry_warning, tarball_url, transport_retry_warning,
};
use crate::core::policy::{ExcludePatterns, GITHUB_ERROR_BODY_BYTES};
use crate::shell::IngestError;
use crate::shell::error::transport_error;
use crate::shell::stdio;
use client::GithubClient;

/// Collects files from a remote GitHub repository via streaming tarball download.
/// `api_base` is the API origin, `GITHUB_API_BASE_URL` in production.
pub(super) fn collect(
    api_base: &str,
    owner: &str,
    repo: &str,
    reference: Option<&str>,
    exclude: &ExcludePatterns,
) -> Result<CollectionResult, IngestError> {
    let client = GithubClient::new();
    let url = tarball_url(api_base, owner, repo, reference);
    let reader = client.stream_url(&url)?;
    let label = repository_url(owner, repo);
    archive::collect(reader, &label, reference, exclude)
}

enum Attempt {
    Status {
        response: ureq::http::Response<ureq::Body>,
        retry_after: Option<Duration>,
    },
    Transport(ureq::Error),
}

impl Attempt {
    /// Classifies a call that did not succeed: `Ok` is a non-2xx response, `Err` a transport failure.
    fn from_result(
        result: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    ) -> (Self, RequestOutcome) {
        match result {
            Ok(response) => {
                let rate_limit = read_rate_limit_headers(response.headers());
                let retry_after = rate_limit.retry_after(unix_now_secs());
                let outcome = RequestOutcome::HttpStatus {
                    status: response.status().as_u16(),
                    retry_after,
                    is_rate_limit: rate_limit.is_rate_limited(),
                };
                (
                    Self::Status {
                        response,
                        retry_after,
                    },
                    outcome,
                )
            }
            Err(error) => {
                let kind = transport_kind(&error);
                (Self::Transport(error), RequestOutcome::TransportError(kind))
            }
        }
    }

    fn warn(&self, count: usize, url: &str, delay: Duration) {
        let message = match self {
            Self::Status { response, .. } => {
                status_retry_warning(response.status().as_u16(), count, url, delay)
            }
            Self::Transport(error) => {
                transport_retry_warning(&error.to_string(), count, url, delay)
            }
        };
        stdio::write_stderr_line(&message);
    }

    fn into_error(self, auth: Auth) -> IngestError {
        match self {
            Self::Status {
                response,
                retry_after,
            } => {
                let status = response.status();
                let mut body_bytes = Zeroizing::new(Vec::new());
                let _ = response
                    .into_body()
                    .into_reader()
                    .take(GITHUB_ERROR_BODY_BYTES)
                    .read_to_end(&mut body_bytes);
                IngestError::Api(api_error(status.as_u16(), &body_bytes, retry_after, auth))
            }
            Self::Transport(error) => transport_error(&error.to_string()),
        }
    }
}

impl GithubClient {
    fn stream_url(&self, url: &str) -> Result<impl Read + Send, IngestError> {
        let mut attempt_state = AttemptState::new();
        let auth = self.auth(url);
        loop {
            let (attempt, outcome) = match self.request(url).call() {
                Ok(resp) if resp.status().is_success() => {
                    return Ok(resp.into_body().into_reader());
                }
                result => Attempt::from_result(result),
            };

            let (next_state, transition) = attempt_state.step(&outcome);
            attempt_state = next_state;
            match transition {
                Transition::Retry {
                    attempt: count,
                    delay,
                } => {
                    attempt.warn(count, url, delay);
                    std::thread::sleep(delay);
                }
                Transition::Fail => return Err(attempt.into_error(auth)),
            }
        }
    }
}

const fn transport_kind(error: &ureq::Error) -> TransportKind {
    match error {
        ureq::Error::Io(_) | ureq::Error::Timeout(_) | ureq::Error::ConnectionFailed => {
            TransportKind::Retryable
        }
        _ => TransportKind::Permanent,
    }
}

/// Reads rate-limit headers from a response. A present but non-UTF-8 header counts as present with no value.
fn read_rate_limit_headers(headers: &ureq::http::HeaderMap) -> RateLimitHeaders {
    let text = |name: &str| headers.get(name).map(|v| v.to_str().unwrap_or(""));
    RateLimitHeaders::parse(
        text("retry-after"),
        text("x-ratelimit-remaining"),
        text("x-ratelimit-reset"),
    )
}

/// Current Unix time in seconds. A clock before the epoch maps to `u64::MAX`, so no reset is ever in the future.
fn unix_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(u64::MAX, |now| now.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::policy::{MAX_ATTEMPTS, TOKEN_ENV_VARS};
    use crate::shell::testing::{MockServer, gzip_tarball};
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

        let result = collect(
            &server.url(""),
            "test-owner",
            "test-repo",
            Some("main"),
            &ExcludePatterns::default(),
        )
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

        let result = collect(
            &server.url(""),
            "test-owner",
            "test-repo",
            Some("feature/x"),
            &ExcludePatterns::default(),
        )
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

        let Err(err) = collect(
            &server.url(""),
            "test-owner",
            "test-repo",
            None,
            &ExcludePatterns::default(),
        ) else {
            panic!("expected 404 to fail collection");
        };
        assert!(matches!(err, IngestError::Api(_)), "{err}");
        server.join();
    }

    fn epoch_now_secs() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_secs()
    }

    #[test]
    fn reads_rate_limit_headers_from_response() {
        let mut headers = ureq::http::HeaderMap::new();
        headers.insert("x-ratelimit-remaining", "0".parse().expect("header value"));
        headers.insert("x-ratelimit-reset", "123".parse().expect("header value"));
        assert_eq!(
            read_rate_limit_headers(&headers),
            RateLimitHeaders::parse(None, Some("0"), Some("123"))
        );
    }

    #[test]
    fn server_error_is_retried_until_success() {
        let reset = (epoch_now_secs() + 2400).to_string();
        // (case, first response): each failure is retried once and the second attempt succeeds.
        let cases = [
            (
                "503 with remaining quota",
                format!(
                    "HTTP/1.1 503 Service Unavailable\r\nx-ratelimit-remaining: 59\r\nx-ratelimit-reset: {reset}\r\nContent-Length: 11\r\nConnection: close\r\n\r\nunavailable"
                )
                .into_bytes(),
            ),
            (
                "500",
                b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 5\r\nConnection: close\r\n\r\nerror".to_vec(),
            ),
        ];
        for (case, failure) in cases {
            let server = MockServer::spawn(2, move |index, _req, _port| match index {
                0 => failure.clone(),
                _ => b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nsuccess"
                    .to_vec(),
            });

            let client = GithubClient::new();
            let url = server.url("/test");
            let mut reader = client
                .stream_url(&url)
                .unwrap_or_else(|err| panic!("{case}: stream_url should succeed on retry: {err}"));
            let mut body = String::new();
            reader.read_to_string(&mut body).expect("read body");
            assert_eq!(body, "success", "{case}");

            let (_, connections) = server.join();
            assert_eq!(connections, 2, "{case}");
        }
    }

    #[test]
    fn client_error_is_not_retried() {
        let reset = (epoch_now_secs() + 2400).to_string();
        // (case, response, status, exact message): a 4xx is terminal, with or without remaining quota.
        let token_hint = format!(
            "(set {} to authenticate or increase rate limits)",
            TOKEN_ENV_VARS.join(" or ")
        );
        let cases = [
            (
                "bare 404",
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nnot found"
                    .to_vec(),
                404,
                "HTTP 404: not found".to_string(),
            ),
            (
                "404 with remaining quota",
                format!(
                    "HTTP/1.1 404 Not Found\r\nx-ratelimit-remaining: 59\r\nx-ratelimit-reset: {reset}\r\nContent-Length: 9\r\nConnection: close\r\n\r\nnot found"
                )
                .into_bytes(),
                404,
                "HTTP 404: not found".to_string(),
            ),
            (
                "403 without rate-limit signal",
                b"HTTP/1.1 403 Forbidden\r\nContent-Length: 9\r\nConnection: close\r\n\r\nforbidden"
                    .to_vec(),
                403,
                format!("HTTP 403: forbidden {token_hint}"),
            ),
        ];
        for (case, response, status, message) in cases {
            let server = MockServer::spawn(1, move |_index, _req, _port| response.clone());

            let client = GithubClient::new();
            let url = server.url("/test");
            let Err(err) = client.stream_url(&url) else {
                panic!("{case}: expected error");
            };
            let IngestError::Api(api) = &err else {
                panic!("{case}: expected Api error, got {err}");
            };
            assert_eq!(api.status(), status, "{case}");
            assert_eq!(api.retry_after(), None, "{case}");
            assert_eq!(
                err.to_string(),
                format!("GitHub API error: {message}"),
                "{case}"
            );

            let (_, connections) = server.join();
            assert_eq!(connections, 1, "{case}");
        }
    }

    #[test]
    fn retries_up_to_max_budget_and_returns_final_error() {
        let server = MockServer::spawn(MAX_ATTEMPTS, |_index, _req, _port| {
            b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 11\r\nConnection: close\r\n\r\nunavailable".to_vec()
        });

        let client = GithubClient::new();
        let url = server.url("/test");
        let Err(err) = client.stream_url(&url) else {
            panic!("expected error");
        };
        let IngestError::Api(api) = &err else {
            panic!("expected Api error, got {err}");
        };
        assert_eq!(api.status(), 503);
        assert_eq!(api.retry_after(), None);
        assert_eq!(err.to_string(), "GitHub API error: HTTP 503: unavailable");

        let (_, connections) = server.join();
        assert_eq!(connections, MAX_ATTEMPTS);
    }

    #[test]
    fn retry_after_beyond_retry_cap_fails_at_once_with_the_delay_in_the_error() {
        // A 3600 s Retry-After exceeds the retry cap, so the first response is final.
        let server = MockServer::spawn(1, |_index, _req, _port| {
            b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 3600\r\nContent-Length: 12\r\nConnection: close\r\n\r\nrate limited"
                .to_vec()
        });

        let client = GithubClient::new();
        let url = server.url("/test");
        let Err(err) = client.stream_url(&url) else {
            panic!("expected error");
        };
        let IngestError::Api(api) = &err else {
            panic!("expected Api error, got {err}");
        };
        assert_eq!(api.status(), 429);
        assert_eq!(api.retry_after(), Some(Duration::from_secs(3600)));
        assert_eq!(
            err.to_string(),
            "GitHub API error: HTTP 429: rate limited (retry available in 3600s) (set GITHUB_TOKEN or GH_TOKEN to authenticate or increase rate limits)"
        );

        let (_, connections) = server.join();
        assert_eq!(connections, 1);
    }

    #[test]
    fn does_not_retry_non_transient_transport_error() {
        let retryable = TransportKind::Retryable;
        let permanent = TransportKind::Permanent;
        assert_eq!(
            transport_kind(&ureq::Error::Timeout(ureq::Timeout::Global)),
            retryable
        );
        assert_eq!(transport_kind(&ureq::Error::ConnectionFailed), retryable);
        assert_eq!(
            transport_kind(&ureq::Error::Io(std::io::Error::from(
                std::io::ErrorKind::ConnectionReset
            ))),
            retryable
        );
        assert_eq!(transport_kind(&ureq::Error::HostNotFound), permanent);
        assert_eq!(transport_kind(&ureq::Error::Tls("cert error")), permanent);

        let server = MockServer::spawn(1, |_index, _req, _port| b"NOT_HTTP/1.1\r\n\r\n".to_vec());

        let client = GithubClient::new();
        let url = server.url("/test");
        assert!(client.stream_url(&url).is_err());

        let (_, connections) = server.join();
        assert_eq!(connections, 1);
    }

    #[test]
    fn dropped_connection_is_retried_as_transport_error() {
        let server = MockServer::spawn(2, |index, _req, _port| match index {
            // An empty response closes the socket before any status line.
            0 => Vec::new(),
            _ => {
                b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nsuccess".to_vec()
            }
        });

        let client = GithubClient::new();
        let url = server.url("/test");
        let mut reader = client
            .stream_url(&url)
            .expect("retry after a dropped connection succeeds");
        let mut body = String::new();
        reader.read_to_string(&mut body).expect("read body");
        assert_eq!(body, "success");

        let (_, connections) = server.join();
        assert_eq!(connections, 2);
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

        let client = GithubClient::new();
        let url = server.url("/archive");
        let reader = client.stream_url(&url).expect("200 headers are accepted");
        let Err(err) = archive::collect(reader, "x", None, &ExcludePatterns::default()) else {
            panic!("expected error");
        };
        assert!(matches!(err, IngestError::Http(_)), "{err}");

        server.join();
    }
}
