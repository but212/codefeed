//! GitHub streaming tarball ingestion and HTTP retry loop orchestration.

use std::io::Read;
use std::time::Duration;

use zeroize::Zeroizing;

use super::client::GithubClient;
use crate::core::github::{
    AttemptState, RateLimitHeaders, RequestOutcome, Transition, TransportKind, api_error,
    status_retry_warning, transport_retry_warning,
};
use crate::shell::IngestError;
use crate::shell::error::transport_error;
use crate::shell::output;

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
                let kind = if is_retryable_transport_error(&error) {
                    TransportKind::Retryable
                } else {
                    TransportKind::Permanent
                };
                (Self::Transport(error), RequestOutcome::TransportError(kind))
            }
        }
    }

    fn warn(&self, count: usize, url: &str, delay: Duration) {
        let message = match self {
            Self::Status { response, .. } => {
                status_retry_warning(&response.status().to_string(), count, url, delay)
            }
            Self::Transport(error) => {
                transport_retry_warning(&error.to_string(), count, url, delay)
            }
        };
        output::write_stderr_line(&message);
    }

    fn into_error(self) -> IngestError {
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
                    .take(4096)
                    .read_to_end(&mut body_bytes);
                IngestError::Api(api_error(
                    status.as_u16(),
                    &status.to_string(),
                    &body_bytes,
                    retry_after,
                ))
            }
            Self::Transport(error) => transport_error(&error.to_string()),
        }
    }
}

impl GithubClient {
    pub(super) fn stream_url(&self, url: &str) -> Result<impl Read + Send, IngestError> {
        let mut attempt_state = AttemptState::new();
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
                Transition::Fail => return Err(attempt.into_error()),
            }
        }
    }
}

const fn is_retryable_transport_error(error: &ureq::Error) -> bool {
    matches!(
        error,
        ureq::Error::Io(_) | ureq::Error::Timeout(_) | ureq::Error::ConnectionFailed
    )
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
    use crate::core::policy::MAX_ATTEMPTS;
    use crate::shell::testing::{MockServer, gzip_tarball};
    use std::io::Read;
    use tar::EntryType;

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
    fn api_error_omits_retry_hint_with_remaining_quota() {
        let reset = (epoch_now_secs() + 2400).to_string();
        let server = MockServer::spawn(1, move |_index, _req, _port| {
            format!(
                "HTTP/1.1 404 Not Found\r\nx-ratelimit-remaining: 59\r\nx-ratelimit-reset: {reset}\r\nContent-Length: 9\r\nConnection: close\r\n\r\nnot found"
            )
            .into_bytes()
        });

        let client = GithubClient::new();
        let url = server.url("/test");
        let Err(err) = client.stream_url(&url) else {
            panic!("expected error");
        };
        let msg = err.to_string();
        assert!(msg.contains("404"));
        assert!(!msg.contains("retry available"), "{msg}");

        server.join();
    }

    #[test]
    fn retries_server_error_with_ratelimit_headers() {
        let reset = (epoch_now_secs() + 2400).to_string();
        let server = MockServer::spawn(2, move |index, _req, _port| {
            match index {
            0 => format!(
                "HTTP/1.1 503 Service Unavailable\r\nx-ratelimit-remaining: 59\r\nx-ratelimit-reset: {reset}\r\nContent-Length: 11\r\nConnection: close\r\n\r\nunavailable"
            )
            .into_bytes(),
            _ => b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nsuccess".to_vec(),
        }
        });

        let client = GithubClient::new();
        let url = server.url("/test");
        let mut reader = client
            .stream_url(&url)
            .expect("503 with remaining quota should be retried");
        let mut body = String::new();
        reader.read_to_string(&mut body).expect("read body");
        assert_eq!(body, "success");

        let (_, connections) = server.join();
        assert_eq!(connections, 2);
    }

    #[test]
    fn retries_on_server_error_and_succeeds() {
        let server = MockServer::spawn(2, |index, _req, _port| {
            match index {
            0 => b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 5\r\nConnection: close\r\n\r\nerror".to_vec(),
            _ => b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nsuccess".to_vec(),
        }
        });

        let client = GithubClient::new();
        let url = server.url("/test");
        let mut reader = client
            .stream_url(&url)
            .expect("stream_url should succeed on retry");
        let mut body = String::new();
        reader.read_to_string(&mut body).expect("read body");
        assert_eq!(body, "success");

        let (_, connections) = server.join();
        assert_eq!(connections, 2);
    }

    #[test]
    fn does_not_retry_on_client_error() {
        let server = MockServer::spawn(1, |_index, _req, _port| {
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nnot found"
                .to_vec()
        });

        let client = GithubClient::new();
        let url = server.url("/test");
        let Err(err) = client.stream_url(&url) else {
            panic!("expected error");
        };
        assert!(matches!(err, IngestError::Api(_)), "{err}");
        assert!(err.to_string().contains("404"));

        let (_, connections) = server.join();
        assert_eq!(connections, 1);
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
        assert!(err.to_string().contains("503"));

        let (_, connections) = server.join();
        assert_eq!(connections, MAX_ATTEMPTS);
    }

    #[test]
    fn does_not_retry_non_transient_transport_error() {
        assert!(is_retryable_transport_error(&ureq::Error::Timeout(
            ureq::Timeout::Global
        )));
        assert!(is_retryable_transport_error(&ureq::Error::ConnectionFailed));
        assert!(is_retryable_transport_error(&ureq::Error::Io(
            std::io::Error::from(std::io::ErrorKind::ConnectionReset)
        )));
        assert!(!is_retryable_transport_error(&ureq::Error::HostNotFound));
        assert!(!is_retryable_transport_error(&ureq::Error::Tls(
            "cert error"
        )));

        let server = MockServer::spawn(1, |_index, _req, _port| b"NOT_HTTP/1.1\r\n\r\n".to_vec());

        let client = GithubClient::new();
        let url = server.url("/test");
        assert!(client.stream_url(&url).is_err());

        let (_, connections) = server.join();
        assert_eq!(connections, 1);
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
        let Err(err) = crate::shell::archive::collect(reader, "x", None) else {
            panic!("expected error");
        };
        assert!(matches!(err, IngestError::Http(_)), "{err}");

        server.join();
    }

    #[test]
    fn attempt_warn_handles_transport_error() {
        let attempt = Attempt::Transport(ureq::Error::ConnectionFailed);
        attempt.warn(1, "https://api.github.com/test", Duration::from_millis(50));
    }
}
