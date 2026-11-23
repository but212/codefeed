//! GitHub HTTP execution: TLS, authentication, retries, and response error handling.

use std::io::Read;
use std::time::Duration;

use ureq::config::RedirectAuthHeaders;
use ureq::tls::{RootCerts, TlsConfig};
use ureq::typestate::WithoutBody;
use ureq::{Agent, RequestBuilder};
use zeroize::Zeroizing;

use crate::core::error::ErrorDetail;
use crate::core::github::{
    AttemptState, Auth, GITHUB_API_VERSION, RateLimitHeaders, RequestOutcome, RequestTarget,
    Transition, api_error, format_bearer_token, status_retry_warning, transport_retry_warning,
};
use crate::core::policy::{
    ARCHIVE_MAX_SECS, GITHUB_ERROR_BODY_BYTES, GITHUB_TIMEOUT_SECS, TOKEN_ENV_VARS,
};
use crate::shell::error::IngestError;
use crate::shell::stdio;

const GITHUB_USER_AGENT: &str = concat!("codefeed/", env!("CARGO_PKG_VERSION"));

pub(super) struct GithubClient {
    agent: Agent,
    auth_header: Option<Zeroizing<String>>,
}

impl GithubClient {
    pub(super) fn new() -> Self {
        let tls = TlsConfig::builder().root_certs(RootCerts::WebPki).build();
        let config = Agent::config_builder()
            .user_agent(GITHUB_USER_AGENT)
            .redirect_auth_headers(RedirectAuthHeaders::Never)
            .timeout_resolve(Some(Duration::from_secs(GITHUB_TIMEOUT_SECS)))
            .timeout_connect(Some(Duration::from_secs(GITHUB_TIMEOUT_SECS)))
            .timeout_recv_response(Some(Duration::from_secs(GITHUB_TIMEOUT_SECS)))
            .timeout_recv_body(Some(Duration::from_secs(ARCHIVE_MAX_SECS)))
            .http_status_as_error(false)
            .tls_config(tls)
            .build();

        let auth_header = resolve_token_with(|k| std::env::var(k));

        Self {
            agent: Agent::new_with_config(config),
            auth_header,
        }
    }

    /// Opens a response stream, retrying request failures according to the core policy.
    pub(super) fn stream_url(&self, url: &str) -> Result<impl Read + Send, IngestError> {
        let mut attempt_state = AttemptState::new();
        let policy = self.policy(url);
        loop {
            let (attempt, outcome) = match policy.apply(self.agent.get(url)).call() {
                Ok(resp) if resp.status().is_success() => {
                    return Ok(resp.into_body().into_reader());
                }
                result => Attempt::from_result(result),
            };

            match attempt_state.step(&outcome) {
                Transition::Retry { next, delay } => {
                    attempt_state = next;
                    attempt.warn(next.retry_count(), url, delay);
                    std::thread::sleep(delay);
                }
                Transition::Fail => return Err(attempt.into_error(policy.auth())),
            }
        }
    }

    /// Classifies `url` once so headers and error-classification `Auth` derive from the same origin check.
    fn policy(&self, url: &str) -> RequestPolicy<'_> {
        RequestPolicy::new(url, self.auth_header.as_ref())
    }
}

/// Origin-bound request policy pairing `Auth` with the headers permitted for that origin (I-07).
enum RequestPolicy<'a> {
    GithubApi(Option<&'a Zeroizing<String>>),
    External,
}

impl<'a> RequestPolicy<'a> {
    fn new(url: &str, configured_token: Option<&'a Zeroizing<String>>) -> Self {
        match RequestTarget::classify(url) {
            RequestTarget::GithubApi => Self::GithubApi(configured_token),
            RequestTarget::External => Self::External,
        }
    }

    const fn auth(&self) -> Auth {
        match self {
            Self::GithubApi(Some(_)) => Auth::Token,
            Self::GithubApi(None) | Self::External => Auth::Anonymous,
        }
    }

    fn apply(&self, mut request: RequestBuilder<WithoutBody>) -> RequestBuilder<WithoutBody> {
        if let Self::GithubApi(token) = self {
            request = request.header("X-Github-Api-Version", GITHUB_API_VERSION);
            if let Some(token) = token {
                request = request.header("Authorization", token.as_str());
            }
        }
        request
    }
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
                let rate_limit = read_rate_limit_headers(response.headers(), unix_now_secs());
                let outcome = RequestOutcome::HttpStatus {
                    status: response.status().as_u16(),
                    rate_limit,
                };
                (
                    Self::Status {
                        response,
                        retry_after: rate_limit.retry_after(),
                    },
                    outcome,
                )
            }
            Err(error) => {
                let retryable = is_retryable_transport(&error);
                (
                    Self::Transport(error),
                    RequestOutcome::TransportError { retryable },
                )
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
            Self::Transport(error) => IngestError::Http(ErrorDetail::new(error.to_string())),
        }
    }
}

const fn is_retryable_transport(error: &ureq::Error) -> bool {
    matches!(
        error,
        ureq::Error::Io(_) | ureq::Error::Timeout(_) | ureq::Error::ConnectionFailed
    )
}

/// Reads raw rate-limit header bytes from the HTTP response for core parsing.
fn read_rate_limit_headers(headers: &ureq::http::HeaderMap, now_secs: u64) -> RateLimitHeaders {
    let raw = |name: &str| headers.get(name).map(ureq::http::HeaderValue::as_bytes);
    RateLimitHeaders::parse(
        raw("retry-after"),
        raw("x-ratelimit-remaining"),
        raw("x-ratelimit-reset"),
        now_secs,
    )
}

/// Current Unix time in seconds. A clock before the epoch maps to `u64::MAX`, so no reset is ever in the future.
fn unix_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(u64::MAX, |now| now.as_secs())
}

fn resolve_token_with<F>(mut lookup: F) -> Option<Zeroizing<String>>
where
    F: FnMut(&str) -> Result<String, std::env::VarError>,
{
    TOKEN_ENV_VARS.into_iter().find_map(|key| {
        let raw = Zeroizing::new(lookup(key).ok()?);
        format_bearer_token(&raw)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::policy::{MAX_ATTEMPTS, RETRY_MAX_DELAY_MS};
    use crate::shell::testing::MockServer;
    use std::collections::HashMap;
    use std::env::VarError;
    use std::io::Read;
    use ureq::http::StatusCode;

    #[test]
    fn reads_rate_limit_headers_from_response() {
        let mut headers = ureq::http::HeaderMap::new();
        headers.insert("retry-after", "30".parse().expect("header value"));
        headers.insert("x-ratelimit-remaining", "0".parse().expect("header value"));
        headers.insert("x-ratelimit-reset", "123".parse().expect("header value"));
        let parsed = read_rate_limit_headers(&headers, 0);
        assert_eq!(
            parsed,
            RateLimitHeaders::parse(Some(b"30"), Some(b"0"), Some(b"123"), 0)
        );
        assert_eq!(parsed.retry_after(), Some(Duration::from_secs(30)));
    }

    #[test]
    fn long_retry_after_values_do_not_send_a_second_request() {
        for (value, body, expected_retry_after, expected_message) in [
            (
                "3600",
                "rate limited",
                Some(Duration::from_secs(3600)),
                Some(
                    "GitHub API error: HTTP 429: rate limited (retry available in 3600s) (set GITHUB_TOKEN or GH_TOKEN to authenticate or increase rate limits)",
                ),
            ),
            ("Fri, 31 Dec 9999 23:59:59 GMT", "", None, None),
            ("18446744073709551616", "", None, None),
        ] {
            let body_len = body.len();
            let server = MockServer::spawn(1, move |_index, _req, _port| {
                format!(
                    "HTTP/1.1 429 Too Many Requests\r\nRetry-After: {value}\r\nContent-Length: {body_len}\r\nConnection: close\r\n\r\n{body}"
                )
                .into_bytes()
            });
            let client = GithubClient::new();
            let Err(err) = client.stream_url(&server.url("/test")) else {
                panic!("{value}: expected the original rate-limit error");
            };
            let IngestError::Api(api) = &err else {
                panic!("{value}: a retry lost the original rate-limit response: {err}");
            };
            assert_eq!(api.status(), 429, "{value}");
            if let Some(exact) = expected_retry_after {
                assert_eq!(api.retry_after(), Some(exact), "{value}");
            }
            assert!(
                api.retry_after()
                    .is_some_and(|delay| delay.as_millis() > u128::from(RETRY_MAX_DELAY_MS)),
                "{value}: requested delay was discarded"
            );
            if let Some(expected) = expected_message {
                assert_eq!(err.to_string(), expected, "{value}");
            }
            assert_eq!(server.join().len(), 1, "{value}");
        }
    }

    #[test]
    fn server_error_is_retried_until_success() {
        let reset = (unix_now_secs() + 2400).to_string();
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

            assert_eq!(server.join().len(), 2, "{case}");
        }
    }

    #[test]
    fn client_error_is_not_retried() {
        let reset = (unix_now_secs() + 2400).to_string();
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

            let requests = server.join();
            assert_eq!(requests.len(), 1, "{case}");
            let expected_ua_header = format!("user-agent: {GITHUB_USER_AGENT}");
            assert!(
                requests[0]
                    .to_ascii_lowercase()
                    .contains(&expected_ua_header),
                "{case}: HTTP request must transmit expected User-Agent header ({expected_ua_header}): {}",
                requests[0]
            );
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

        assert_eq!(server.join().len(), MAX_ATTEMPTS);
    }

    #[test]
    fn does_not_retry_non_transient_transport_error() {
        assert!(is_retryable_transport(&ureq::Error::Timeout(
            ureq::Timeout::Global
        )));
        assert!(is_retryable_transport(&ureq::Error::ConnectionFailed));
        assert!(is_retryable_transport(&ureq::Error::Io(
            std::io::Error::from(std::io::ErrorKind::ConnectionReset)
        )));
        assert!(!is_retryable_transport(&ureq::Error::HostNotFound));
        assert!(!is_retryable_transport(&ureq::Error::Tls("cert error")));

        let server = MockServer::spawn(1, |_index, _req, _port| b"NOT_HTTP/1.1\r\n\r\n".to_vec());

        let client = GithubClient::new();
        let url = server.url("/test");
        assert!(client.stream_url(&url).is_err());

        assert_eq!(server.join().len(), 1);
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

        assert_eq!(server.join().len(), 2);
    }

    #[test]
    fn resolves_token_with_correct_precedence_and_trimming() {
        let mock_env = |map: HashMap<&'static str, &'static str>| {
            move |key: &str| {
                map.get(key)
                    .copied()
                    .map(String::from)
                    .ok_or(VarError::NotPresent)
            }
        };

        let env = HashMap::from([("GITHUB_TOKEN", "token_a"), ("GH_TOKEN", "token_b")]);
        assert_eq!(
            resolve_token_with(mock_env(env)),
            Some(Zeroizing::new("Bearer token_a".into()))
        );

        let env = HashMap::from([("GH_TOKEN", "token_b")]);
        assert_eq!(
            resolve_token_with(mock_env(env)),
            Some(Zeroizing::new("Bearer token_b".into()))
        );

        let env = HashMap::from([("GITHUB_TOKEN", "   "), ("GH_TOKEN", "valid_token")]);
        assert_eq!(
            resolve_token_with(mock_env(env)),
            Some(Zeroizing::new("Bearer valid_token".into()))
        );

        let env = HashMap::from([("GITHUB_TOKEN", "")]);
        assert_eq!(resolve_token_with(mock_env(env)), None);

        let env = HashMap::new();
        assert_eq!(resolve_token_with(mock_env(env)), None);
    }

    #[test]
    fn client_bounds_every_pre_response_phase() {
        let timeouts = GithubClient::new().agent.config().timeouts();
        let bounded = Some(Duration::from_secs(GITHUB_TIMEOUT_SECS));
        assert_eq!(timeouts.resolve, bounded);
        assert_eq!(timeouts.connect, bounded);
        assert_eq!(timeouts.recv_response, bounded);
        assert_eq!(
            timeouts.recv_body,
            Some(Duration::from_secs(ARCHIVE_MAX_SECS))
        );
    }

    #[test]
    fn strips_authorization_header_on_redirect() {
        let server = MockServer::spawn(2, |index, _req, port| {
            match index {
                0 => format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{port}/dest\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .into_bytes(),
                _ => b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nsuccess".to_vec(),
            }
        });

        let client = GithubClient::new();
        let url = server.url("/start");
        let resp = client
            .agent
            .get(&url)
            .header("Authorization", "Bearer test-token")
            .call()
            .expect("call should follow redirect and succeed");
        assert_eq!(resp.status(), StatusCode::OK);

        let requests = server.join();
        assert_eq!(requests.len(), 2);

        let req1 = &requests[0];
        assert!(
            req1.to_ascii_lowercase()
                .contains("authorization: bearer test-token"),
            "first request must contain authorization header: {req1}"
        );

        let req2 = &requests[1];
        assert!(
            !req2.to_ascii_lowercase().contains("authorization"),
            "redirected request must not contain authorization header: {req2}"
        );
    }

    #[test]
    fn github_client_request_headers_for_api_and_non_api_origin() {
        const API_URL: &str = "https://api.github.com/repos/foo/bar/tarball";
        const NON_API_URL: &str = "https://codeload.github.com/foo/bar/tar.gz";

        let mut client = GithubClient::new();
        client.auth_header = Some(Zeroizing::new("Bearer test_token".to_string()));

        assert_eq!(client.policy(NON_API_URL).auth(), Auth::Anonymous);
        let non_api = client
            .policy(NON_API_URL)
            .apply(client.agent.get(NON_API_URL));
        let non_api_headers = non_api.headers_ref();
        assert_eq!(non_api_headers.and_then(|h| h.get("Authorization")), None);
        assert_eq!(
            non_api_headers.and_then(|h| h.get("X-Github-Api-Version")),
            None
        );

        assert_eq!(client.policy(API_URL).auth(), Auth::Token);
        let api = client.policy(API_URL).apply(client.agent.get(API_URL));
        let api_headers = api.headers_ref();
        assert_eq!(
            api_headers
                .and_then(|h| h.get("Authorization"))
                .and_then(|v| v.to_str().ok()),
            Some("Bearer test_token")
        );
        assert_eq!(
            api_headers
                .and_then(|h| h.get("X-Github-Api-Version"))
                .and_then(|v| v.to_str().ok()),
            Some(GITHUB_API_VERSION)
        );

        client.auth_header = None;
        assert_eq!(client.policy(NON_API_URL).auth(), Auth::Anonymous);
        assert_eq!(client.policy(API_URL).auth(), Auth::Anonymous);
        let api_no_auth = client.policy(API_URL).apply(client.agent.get(API_URL));
        let api_no_auth_headers = api_no_auth.headers_ref();
        assert_eq!(
            api_no_auth_headers.and_then(|h| h.get("Authorization")),
            None
        );
        assert_eq!(
            api_no_auth_headers
                .and_then(|h| h.get("X-Github-Api-Version"))
                .and_then(|v| v.to_str().ok()),
            Some(GITHUB_API_VERSION)
        );
    }
}
