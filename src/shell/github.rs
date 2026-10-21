//! GitHub collection orchestration: single-request streaming tarball ingestion and HTTP transport.

use std::io::Read;
use std::time::Duration;

use ureq::Agent;
use ureq::config::RedirectAuthHeaders;
use ureq::http::StatusCode;
use ureq::tls::{RootCerts, TlsConfig};
use zeroize::Zeroizing;

use crate::core::error::IngestError;
use crate::core::github::{
    AttemptState, RequestOutcome, Transition, TransportKind, is_github_api_origin, tarball_url,
};
use crate::core::model::CollectionResult;
use crate::core::policy::{ARCHIVE_MAX_SECS, GITHUB_TIMEOUT_SECS, MAX_ATTEMPTS};

const GITHUB_USER_AGENT: &str = concat!("codefeed/", env!("CARGO_PKG_VERSION"));

enum Attempt {
    Status(ureq::http::Response<ureq::Body>),
    Transport(ureq::Error),
}

/// Collects files from a remote GitHub repository via streaming tarball download.
pub(super) fn collect(
    owner: &str,
    repo: &str,
    reference: Option<&str>,
) -> Result<CollectionResult, IngestError> {
    let client = GithubClient::new();
    let reader = client.tarball_stream(owner, repo, reference)?;
    let label = format!("https://github.com/{owner}/{repo}");
    super::archive::collect(reader, &label, reference)
}

struct GithubClient {
    agent: Agent,
    auth_header: Option<Zeroizing<String>>,
}

impl GithubClient {
    fn new() -> Self {
        let tls = TlsConfig::builder().root_certs(RootCerts::WebPki).build();
        let config = Agent::config_builder()
            .user_agent(GITHUB_USER_AGENT)
            .redirect_auth_headers(RedirectAuthHeaders::Never)
            .timeout_recv_response(Some(Duration::from_secs(GITHUB_TIMEOUT_SECS)))
            .timeout_recv_body(Some(Duration::from_secs(ARCHIVE_MAX_SECS)))
            .http_status_as_error(false)
            .tls_config(tls)
            .build();

        let auth_header = resolve_token_from_env()
            .map(|token| Zeroizing::new(format!("Bearer {}", token.as_str())));

        Self {
            agent: Agent::new_with_config(config),
            auth_header,
        }
    }

    fn tarball_stream(
        &self,
        owner: &str,
        repo: &str,
        reference: Option<&str>,
    ) -> Result<Box<dyn Read + Send>, IngestError> {
        let url = tarball_url(owner, repo, reference);
        self.stream_url(&url)
    }

    fn stream_url(&self, url: &str) -> Result<Box<dyn Read + Send>, IngestError> {
        let mut attempt_state = AttemptState::new();
        loop {
            let mut request = self.agent.get(url);

            if is_github_api_origin(url) {
                if let Some(auth) = &self.auth_header {
                    request = request.header("Authorization", auth.as_str());
                }
                request = request.header("X-Github-Api-Version", "2022-11-28");
            }

            let attempt = match request.call() {
                Ok(resp) if resp.status().is_success() => {
                    return Ok(Box::new(resp.into_body().into_reader()));
                }
                Ok(resp) => Attempt::Status(resp),
                Err(error) => Attempt::Transport(error),
            };

            let outcome = match &attempt {
                Attempt::Status(resp) => {
                    let status = resp.status().as_u16();
                    let headers = resp.headers();
                    let retry_after = parse_retry_after(headers);
                    let is_rate_limit = is_rate_limit_403(headers);
                    RequestOutcome::HttpStatus {
                        status,
                        retry_after,
                        is_rate_limit,
                    }
                }
                Attempt::Transport(error) if is_retryable_transport_error(error) => {
                    RequestOutcome::TransportError(TransportKind::Retryable)
                }
                Attempt::Transport(_) => RequestOutcome::TransportError(TransportKind::Permanent),
            };

            let (next_state, transition) = attempt_state.step(&outcome);
            attempt_state = next_state;
            match transition {
                Transition::Retry {
                    attempt: count,
                    delay,
                } => {
                    match &attempt {
                        Attempt::Status(resp) => eprintln!(
                            "warning: GitHub HTTP status {} on attempt {count}/{MAX_ATTEMPTS} for {url}; retrying in {delay:?}",
                            resp.status()
                        ),
                        Attempt::Transport(error) => eprintln!(
                            "warning: GitHub transport error: {} on attempt {count}/{MAX_ATTEMPTS} for {url}; retrying in {delay:?}",
                            sanitize_error_detail(&error.to_string())
                        ),
                    }
                    std::thread::sleep(delay);
                }
                Transition::Fail => match attempt {
                    Attempt::Status(response) => {
                        let status = response.status();
                        let retry_after = parse_retry_after(response.headers());
                        let mut body_bytes = Zeroizing::new(Vec::new());
                        let _ = response
                            .into_body()
                            .into_reader()
                            .take(4096)
                            .read_to_end(&mut body_bytes);
                        return Err(api_error(status, &body_bytes, retry_after));
                    }
                    Attempt::Transport(error) => {
                        return Err(IngestError::http(sanitize_error_detail(&error.to_string())));
                    }
                },
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

/// Sanitizes control characters in error detail string.
/// Converts `\t`, `\r`, `\n` to space `' '` for single-line output formatting.
/// Converts all other Unicode control characters (`c.is_control()`) to `\u{FFFD}`.
/// Note: Unlike digest header escaping which escapes `\n` and `\r` as `\n` / `\r` literals,
/// error message formatting converts line breaks to spaces for compact single-line stderr diagnostics.
fn sanitize_error_detail(input: &str) -> String {
    input
        .chars()
        .map(|c| match c {
            '\t' | '\r' | '\n' => ' ',
            c if c.is_control() => '\u{FFFD}',
            c => c,
        })
        .collect()
}

fn parse_retry_after(headers: &ureq::http::HeaderMap) -> Option<Duration> {
    if let Some(val) = headers.get("retry-after").and_then(|v| v.to_str().ok())
        && let Ok(secs) = val.trim().parse::<u64>()
    {
        return Some(Duration::from_secs(secs));
    }
    let quota_exhausted = headers
        .get("x-ratelimit-remaining")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|rem| rem.trim() == "0");
    if !quota_exhausted {
        return None;
    }
    if let Some(reset) = headers
        .get("x-ratelimit-reset")
        .and_then(|v| v.to_str().ok())
        && let Ok(epoch_secs) = reset.trim().parse::<u64>()
        && let Ok(now) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
    {
        let now_secs = now.as_secs();
        if epoch_secs > now_secs {
            return Some(Duration::from_secs(epoch_secs - now_secs));
        }
    }
    None
}

fn is_rate_limit_403(headers: &ureq::http::HeaderMap) -> bool {
    if headers.get("retry-after").is_some() {
        return true;
    }
    if let Some(rem) = headers
        .get("x-ratelimit-remaining")
        .and_then(|v| v.to_str().ok())
        && rem.trim() == "0"
    {
        return true;
    }
    false
}

fn api_error(status: StatusCode, body_bytes: &[u8], retry_after: Option<Duration>) -> IngestError {
    let raw_detail = String::from_utf8_lossy(body_bytes);
    let detail = sanitize_error_detail(&raw_detail);
    let retry_info = retry_after
        .map(|d| format!(" (retry available in {}s)", d.as_secs()))
        .unwrap_or_default();

    if status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::FORBIDDEN {
        IngestError::api(format!(
            "HTTP {status}: {detail}{retry_info} (set GITHUB_TOKEN or GH_TOKEN to authenticate or increase rate limits)"
        ))
    } else {
        IngestError::api(format!("HTTP {status}: {detail}{retry_info}"))
    }
}

fn resolve_token_with<F>(mut lookup: F) -> Option<Zeroizing<String>>
where
    F: FnMut(&str) -> Result<String, std::env::VarError>,
{
    ["GITHUB_TOKEN", "GH_TOKEN"].into_iter().find_map(|key| {
        let raw = Zeroizing::new(lookup(key).ok()?);
        let trimmed = raw.trim_ascii();
        (!trimmed.is_empty()).then(|| Zeroizing::new(trimmed.to_string()))
    })
}

fn resolve_token_from_env() -> Option<Zeroizing<String>> {
    resolve_token_with(|k| std::env::var(k))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::env::VarError;
    use std::io::Write;
    use std::net::TcpListener;
    use std::thread;

    struct MockServer {
        port: u16,
        handle: thread::JoinHandle<(Vec<String>, usize)>,
    }

    impl MockServer {
        fn spawn<F>(expected_connections: usize, mut handler: F) -> Self
        where
            F: FnMut(usize, &str, u16) -> Vec<u8> + Send + 'static,
        {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
            let port = listener.local_addr().expect("local addr").port();

            let handle = thread::spawn(move || {
                let mut recorded = Vec::new();
                let mut count = 0;

                for index in 0..expected_connections {
                    let (mut stream, _) = match listener.accept() {
                        Ok(res) => res,
                        Err(_) => break,
                    };
                    count += 1;
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));

                    let mut req_buf = Vec::new();
                    let mut temp = [0u8; 512];
                    loop {
                        if req_buf.windows(4).any(|w| w == b"\r\n\r\n") || req_buf.len() >= 8192 {
                            break;
                        }
                        match stream.read(&mut temp) {
                            Ok(0) => break,
                            Ok(n) => req_buf.extend_from_slice(&temp[..n]),
                            Err(ref e)
                                if e.kind() == std::io::ErrorKind::WouldBlock
                                    || e.kind() == std::io::ErrorKind::TimedOut =>
                            {
                                break;
                            }
                            Err(_) => break,
                        }
                    }

                    let req_str = String::from_utf8_lossy(&req_buf).to_string();
                    recorded.push(req_str.clone());

                    let response_bytes = handler(index, &req_str, port);
                    if !response_bytes.is_empty() {
                        let _ = stream.write_all(&response_bytes);
                    }
                }

                (recorded, count)
            });

            Self { port, handle }
        }

        fn join(self) -> (Vec<String>, usize) {
            self.handle.join().expect("mock server thread finish")
        }

        fn url(&self, path: &str) -> String {
            format!("http://127.0.0.1:{}{path}", self.port)
        }
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
            Some(Zeroizing::new("token_a".into()))
        );

        let env = HashMap::from([("GH_TOKEN", "token_b")]);
        assert_eq!(
            resolve_token_with(mock_env(env)),
            Some(Zeroizing::new("token_b".into()))
        );

        let env = HashMap::from([("GITHUB_TOKEN", "   "), ("GH_TOKEN", "valid_token")]);
        assert_eq!(
            resolve_token_with(mock_env(env)),
            Some(Zeroizing::new("valid_token".into()))
        );

        let env = HashMap::from([("GITHUB_TOKEN", "")]);
        assert_eq!(resolve_token_with(mock_env(env)), None);

        let env = HashMap::new();
        assert_eq!(resolve_token_with(mock_env(env)), None);
    }

    #[test]
    fn api_error_suggests_token_on_rate_limit_and_forbidden() {
        let err_429 = api_error(StatusCode::TOO_MANY_REQUESTS, b"rate limit", None);
        assert!(err_429.to_string().contains("GITHUB_TOKEN"));

        let err_403 = api_error(
            StatusCode::FORBIDDEN,
            b"forbidden",
            Some(Duration::from_secs(30)),
        );
        let err_403_msg = err_403.to_string();
        assert!(err_403_msg.contains("GITHUB_TOKEN"));
        assert!(err_403_msg.contains("(retry available in 30s)"));

        let err_404 = api_error(StatusCode::NOT_FOUND, b"not found", None);
        assert!(!err_404.to_string().contains("GITHUB_TOKEN"));
    }

    #[test]
    fn sanitizes_control_characters_in_api_error_detail() {
        let raw = b"error\twith\nnewlines\r\nand\x1b[31mANSI\x07bell\x7fdel\x80c1";
        let err = api_error(StatusCode::BAD_REQUEST, raw, None);
        let err_msg = err.to_string();

        assert!(
            !err_msg.contains('\n') && !err_msg.contains('\r') && !err_msg.contains('\t'),
            "newlines, tabs, and carriage returns should be converted to spaces: {err_msg}"
        );
        assert!(
            err_msg.contains("\u{FFFD}[31mANSI\u{FFFD}bell\u{FFFD}del"),
            "C0, DEL, and C1 control characters should be replaced with replacement char: {err_msg}"
        );
    }

    fn epoch_now_secs() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_secs()
    }

    fn header_map(pairs: &[(&'static str, &str)]) -> ureq::http::HeaderMap {
        let mut headers = ureq::http::HeaderMap::new();
        for (name, value) in pairs {
            let value = ureq::http::HeaderValue::from_str(value).expect("valid header value");
            headers.insert(*name, value);
        }
        headers
    }

    #[test]
    fn retry_after_honors_quota_state() {
        type Row<'a> = (&'a [(&'static str, &'a str)], Option<u64>);
        let reset = (epoch_now_secs() + 2400).to_string();
        let cases: [Row<'_>; 5] = [
            (
                &[
                    ("x-ratelimit-remaining", "59"),
                    ("x-ratelimit-reset", &reset),
                ],
                None,
            ),
            (
                &[
                    ("x-ratelimit-remaining", "0"),
                    ("x-ratelimit-reset", &reset),
                ],
                Some(2400),
            ),
            (
                &[
                    ("retry-after", "2"),
                    ("x-ratelimit-remaining", "0"),
                    ("x-ratelimit-reset", &reset),
                ],
                Some(2),
            ),
            (
                &[
                    ("retry-after", "2"),
                    ("x-ratelimit-remaining", "59"),
                    ("x-ratelimit-reset", &reset),
                ],
                Some(2),
            ),
            (&[("x-ratelimit-reset", &reset)], None),
        ];

        for (pairs, expected) in cases {
            let got = parse_retry_after(&header_map(pairs)).map(|d| d.as_secs());
            // The reset header is wall-clock relative; allow one second of drift between clock reads.
            let matches = match (got, expected) {
                (None, None) => true,
                (Some(g), Some(w)) => g.abs_diff(w) <= 1,
                _ => false,
            };
            assert!(
                matches,
                "headers {pairs:?}: expected {expected:?}, got {got:?}"
            );
        }
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
    fn transmits_user_agent_header_in_http_request() {
        let server = MockServer::spawn(1, |_index, _req, _port| {
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".to_vec()
        });

        let client = GithubClient::new();
        let url = server.url("/user-agent-check");
        let mut reader = client.stream_url(&url).expect("stream_url should succeed");
        let mut body = String::new();
        reader.read_to_string(&mut body).expect("read body");
        assert_eq!(body, "ok");

        let (requests, connections) = server.join();
        assert_eq!(connections, 1);

        let req = &requests[0];
        let expected_ua_header = format!("user-agent: {GITHUB_USER_AGENT}");
        assert!(
            req.to_ascii_lowercase().contains(&expected_ua_header),
            "HTTP request must transmit expected User-Agent header ({expected_ua_header}): {req}"
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

        let (requests, connections) = server.join();
        assert_eq!(connections, 2);

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
}
