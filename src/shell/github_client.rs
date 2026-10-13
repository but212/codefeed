//! GitHub HTTP transport with streaming response bodies.

use std::io::{BufReader, Read};
use std::time::Duration;

use ureq::Agent;
use ureq::http::StatusCode;
use ureq::tls::{RootCerts, TlsConfig};

use crate::core::error::IngestError;
use crate::core::github_url::{RequestTarget, request_target, tarball_url};
use crate::core::policy::{ARCHIVE_MAX_SECS, MAX_RETRIES, is_retryable_status, retry_delay};

const GITHUB_USER_AGENT: &str = "codefeed/0.1";
const GITHUB_TIMEOUT_SECS: u64 = 30;

pub(crate) struct GithubClient {
    agent: Agent,
    auth_header: Option<String>,
}

impl GithubClient {
    pub(crate) fn new() -> Self {
        let tls = TlsConfig::builder()
            .root_certs(RootCerts::PlatformVerifier)
            .build();
        let config = Agent::config_builder()
            .user_agent(GITHUB_USER_AGENT)
            .timeout_recv_response(Some(Duration::from_secs(GITHUB_TIMEOUT_SECS)))
            .timeout_recv_body(Some(Duration::from_secs(ARCHIVE_MAX_SECS)))
            .http_status_as_error(false)
            .tls_config(tls)
            .build();

        let auth_header = resolve_token_from_env().map(|token| format!("Bearer {token}"));

        Self {
            agent: Agent::new_with_config(config),
            auth_header,
        }
    }

    pub(crate) fn tarball_stream(
        &self,
        owner: &str,
        repo: &str,
        reference: Option<&str>,
    ) -> Result<Box<dyn Read + Send>, IngestError> {
        let url = tarball_url(owner, repo, reference);
        self.stream_url(&url)
    }

    pub(crate) fn stream_url(&self, url: &str) -> Result<Box<dyn Read + Send>, IngestError> {
        let mut attempt = 0;
        loop {
            let mut request = self.agent.get(url);
            let target = request_target(url);

            if target == RequestTarget::Api {
                if let Some(auth) = &self.auth_header {
                    request = request.header("Authorization", auth);
                }
                request = request.header("X-Github-Api-Version", "2022-11-28");
            }

            let retry = |attempt: &mut usize,
                         reason: &dyn std::fmt::Display|
             -> std::time::Duration {
                *attempt += 1;
                let delay = retry_delay(*attempt - 1, current_entropy());
                eprintln!(
                    "warning: {reason} on attempt {attempt}/{MAX_RETRIES} for {url}; retrying in {delay:?}"
                );
                std::thread::sleep(delay);
                delay
            };

            let response = match request.call() {
                Ok(resp) => resp,
                Err(error) => {
                    if is_retryable_transport_error(&error) && attempt + 1 < MAX_RETRIES {
                        retry(
                            &mut attempt,
                            &format_args!("GitHub transport error: {error}"),
                        );
                        continue;
                    }
                    return Err(IngestError::Http(error.to_string()));
                }
            };

            let status = response.status();
            if is_retryable_status(status.as_u16()) && attempt + 1 < MAX_RETRIES {
                retry(&mut attempt, &format_args!("GitHub HTTP status {status}"));
                continue;
            }

            if !status.is_success() {
                let mut body_bytes = Vec::new();
                let _ = response
                    .into_body()
                    .into_reader()
                    .take(4096)
                    .read_to_end(&mut body_bytes);
                return Err(api_error(status, &body_bytes));
            }

            return Ok(Box::new(BufReader::with_capacity(
                64 * 1024,
                response.into_body().into_reader(),
            )));
        }
    }
}

fn is_retryable_transport_error(error: &ureq::Error) -> bool {
    matches!(
        error,
        ureq::Error::Io(_) | ureq::Error::Timeout(_) | ureq::Error::ConnectionFailed
    )
}

fn api_error(status: StatusCode, body_bytes: &[u8]) -> IngestError {
    let detail = String::from_utf8_lossy(body_bytes).replace('\n', " ");
    if status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::FORBIDDEN {
        IngestError::Api(format!(
            "HTTP {status}: {detail} (set GITHUB_TOKEN or GH_TOKEN to authenticate or increase rate limits)"
        ))
    } else {
        IngestError::Api(format!("HTTP {status}: {detail}"))
    }
}

fn resolve_token_with<F>(mut lookup: F) -> Option<String>
where
    F: FnMut(&str) -> Result<String, std::env::VarError>,
{
    ["GITHUB_TOKEN", "GH_TOKEN"].into_iter().find_map(|key| {
        lookup(key)
            .ok()
            .map(|t| t.trim_ascii().to_string())
            .filter(|t| !t.is_empty())
    })
}

fn resolve_token_from_env() -> Option<String> {
    resolve_token_with(|k| std::env::var(k))
}

fn current_entropy() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::env::VarError;

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
        assert_eq!(resolve_token_with(mock_env(env)), Some("token_a".into()));

        let env = HashMap::from([("GH_TOKEN", "token_b")]);
        assert_eq!(resolve_token_with(mock_env(env)), Some("token_b".into()));

        let env = HashMap::from([("GITHUB_TOKEN", "   "), ("GH_TOKEN", "valid_token")]);
        assert_eq!(
            resolve_token_with(mock_env(env)),
            Some("valid_token".into())
        );

        let env = HashMap::from([("GITHUB_TOKEN", "")]);
        assert_eq!(resolve_token_with(mock_env(env)), None);

        let env = HashMap::new();
        assert_eq!(resolve_token_with(mock_env(env)), None);
    }

    #[test]
    fn api_error_suggests_token_on_rate_limit_and_forbidden() {
        let err_429 = api_error(StatusCode::TOO_MANY_REQUESTS, b"rate limit");
        assert!(err_429.to_string().contains("GITHUB_TOKEN"));

        let err_403 = api_error(StatusCode::FORBIDDEN, b"forbidden");
        assert!(err_403.to_string().contains("GITHUB_TOKEN"));

        let err_404 = api_error(StatusCode::NOT_FOUND, b"not found");
        assert!(!err_404.to_string().contains("GITHUB_TOKEN"));
    }

    #[test]
    fn retries_on_server_error_and_succeeds() {
        use std::io::Write;
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let port = listener.local_addr().expect("local addr").port();

        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept 1");
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let response = "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 5\r\nConnection: close\r\n\r\nerror";
            let _ = stream.write_all(response.as_bytes());

            let (mut stream, _) = listener.accept().expect("accept 2");
            let _ = stream.read(&mut buf);
            let response =
                "HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nsuccess";
            let _ = stream.write_all(response.as_bytes());
        });

        let client = GithubClient::new();
        let url = format!("http://127.0.0.1:{port}/test");
        let mut reader = client
            .stream_url(&url)
            .expect("stream_url should succeed on retry");
        let mut body = String::new();
        reader.read_to_string(&mut body).expect("read body");
        assert_eq!(body, "success");

        handle.join().expect("server thread finish");
    }

    #[test]
    fn does_not_retry_on_client_error() {
        use std::io::Write;
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let port = listener.local_addr().expect("local addr").port();

        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept 1");
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let response =
                "HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nnot found";
            let _ = stream.write_all(response.as_bytes());
        });

        let client = GithubClient::new();
        let url = format!("http://127.0.0.1:{port}/test");
        let Err(err) = client.stream_url(&url) else {
            panic!("expected error");
        };
        assert!(err.to_string().contains("404"));

        handle.join().expect("server thread finish");
    }

    #[test]
    fn retries_up_to_max_budget_and_returns_final_error() {
        use std::io::Write;
        use std::net::TcpListener;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let port = listener.local_addr().expect("local addr").port();
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = Arc::clone(&attempts);

        let handle = thread::spawn(move || {
            for _ in 0..MAX_RETRIES {
                let (mut stream, _) = listener.accept().expect("accept");
                attempts_clone.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let response = "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 11\r\nConnection: close\r\n\r\nunavailable";
                let _ = stream.write_all(response.as_bytes());
            }
        });

        let client = GithubClient::new();
        let url = format!("http://127.0.0.1:{port}/test");
        let Err(err) = client.stream_url(&url) else {
            panic!("expected error");
        };
        assert!(err.to_string().contains("503"));

        handle.join().expect("server thread finish");
        assert_eq!(attempts.load(Ordering::SeqCst), MAX_RETRIES);
    }

    #[test]
    fn does_not_retry_non_transient_transport_error() {
        use std::io::Write;
        use std::net::TcpListener;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::thread;

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

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let port = listener.local_addr().expect("local addr").port();
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = Arc::clone(&attempts);

        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept 1");
            attempts_clone.fetch_add(1, Ordering::SeqCst);
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(b"NOT_HTTP/1.1\r\n\r\n");
        });

        let client = GithubClient::new();
        let url = format!("http://127.0.0.1:{port}/test");
        assert!(client.stream_url(&url).is_err());

        handle.join().expect("server thread finish");
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }
}
