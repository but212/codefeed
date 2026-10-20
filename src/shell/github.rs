//! GitHub collection orchestration: single-request streaming tarball ingestion and HTTP transport.

use std::io::{BufReader, Read};
use std::time::Duration;

use flate2::read::GzDecoder;
use tar::{Archive, EntryType};
use ureq::Agent;
use ureq::config::RedirectAuthHeaders;
use ureq::http::StatusCode;
use ureq::tls::{RootCerts, TlsConfig};
use zeroize::Zeroizing;

use crate::core::archive::{normalize_tar_entry_path, parse_pax_comment};
use crate::core::error::IngestError;
use crate::core::github::{
    AttemptState, RequestOutcome, Transition, TransportKind, is_github_api_origin, tarball_url,
};
use crate::core::model::CollectionResult;
use crate::core::policy::{ARCHIVE_MAX_SECS, GITHUB_TIMEOUT_SECS, MAX_ATTEMPTS, MAX_FILE_SIZE};
use crate::core::selector::{Admission, Selector};

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
    collect_archive(reader, owner, repo, reference)
}

struct GithubClient {
    agent: Agent,
    auth_header: Option<Zeroizing<String>>,
}

impl GithubClient {
    fn new() -> Self {
        let tls = TlsConfig::builder()
            .root_certs(RootCerts::PlatformVerifier)
            .build();
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
                    return Ok(Box::new(BufReader::with_capacity(
                        64 * 1024,
                        resp.into_body().into_reader(),
                    )));
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

/// Collects files from an uncompressed or compressed tarball reader directly into a CollectionResult.
fn collect_archive<R: Read>(
    reader: R,
    owner: &str,
    repo: &str,
    reference: Option<&str>,
) -> Result<CollectionResult, IngestError> {
    let mut archive = Archive::new(GzDecoder::new(reader));
    let entries = archive.entries().map_err(IngestError::Io)?;
    let label = format!("https://github.com/{owner}/{repo}");
    let mut parsed_revision = None;
    let mut selector = Selector::new();
    let mut content_scratch = Zeroizing::new(Vec::with_capacity(32 * 1024));

    for entry_res in entries {
        let mut entry = entry_res.map_err(IngestError::Io)?;
        content_scratch.clear();
        let (next_selector, sha) = process_tar_entry(&mut entry, selector, &mut content_scratch)?;
        selector = next_selector;
        if let Some(sha) = sha
            && parsed_revision.is_none()
        {
            parsed_revision = Some(sha);
        }
    }

    let revision = parsed_revision.or_else(|| reference.map(str::to_string));
    Ok(selector.finish(label, revision))
}

fn process_tar_entry<R: Read>(
    entry: &mut tar::Entry<R>,
    selector: Selector,
    content_scratch: &mut Vec<u8>,
) -> Result<(Selector, Option<String>), IngestError> {
    let entry_type = entry.header().entry_type();

    // 1. PAX Global Header: Extract git commit SHA from comment
    if entry_type == EntryType::XGlobalHeader {
        entry
            .take(1024)
            .read_to_end(content_scratch)
            .map_err(IngestError::Io)?;
        return Ok((selector, parse_pax_comment(content_scratch)));
    }

    // 2. Skip directories, symlinks, and special entries before path decoding
    if !matches!(entry_type, EntryType::Regular | EntryType::Continuous) {
        return Ok((selector, None));
    }

    // 3. Path decoding, root stripping, normalization & policy classification
    let path_bytes = entry.path_bytes();
    let status = normalize_tar_entry_path(&path_bytes)?;

    let size_hint = entry.header().size().ok();

    // 4. Pre-I/O candidate admission before reading entry body
    match selector.admit(status, size_hint)? {
        Admission::Settled(s) => Ok((s, None)),
        Admission::Pending(pending) => {
            read_tar_entry_into(entry, MAX_FILE_SIZE, content_scratch)?;
            let selector = pending.accept(content_scratch)?;
            Ok((selector, None))
        }
    }
}

fn read_tar_entry_into<R: Read>(
    entry: &mut tar::Entry<R>,
    limit: u64,
    buf: &mut Vec<u8>,
) -> Result<(), IngestError> {
    entry
        .take(limit.saturating_add(1))
        .read_to_end(buf)
        .map_err(IngestError::Io)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::collections::HashMap;
    use std::env::VarError;
    use std::io::Write;
    use std::net::TcpListener;
    use std::thread;
    use tar::Builder;

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

    fn create_mock_tarball(entries: &[(&str, &[u8], EntryType)]) -> Vec<u8> {
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        {
            let mut tar = Builder::new(&mut gz);
            for &(name, content, ref entry_type) in entries {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(*entry_type);
                header.set_size(content.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                tar.append_data(&mut header, name, content).unwrap();
            }
            tar.finish().unwrap();
        }
        gz.finish().unwrap()
    }

    #[test]
    fn collects_files_and_extracts_pax_sha() {
        let pax_comment = b"52 comment=c1863c8b05c5d2db4030bc8c8a7d2f10bda4a950\n";
        let mock_data = create_mock_tarball(&[
            ("pax_global_header", pax_comment, EntryType::XGlobalHeader),
            ("repo-root/", b"", EntryType::Directory),
            (
                "repo-root/Cargo.toml",
                b"[package]\nname = \"test\"",
                EntryType::Regular,
            ),
            ("repo-root/src/main.rs", b"fn main() {}", EntryType::Regular),
            ("repo-root/Cargo.lock", b"lockfile", EntryType::Regular),
            ("repo-root/poetry.lock", b"lockfile", EntryType::Regular),
            ("repo-root/id_ecdsa", b"PRIVATE KEY", EntryType::Regular),
            (
                "repo-root/image.png",
                b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR",
                EntryType::Regular,
            ),
        ]);

        let result = collect_archive(mock_data.as_slice(), "but212", "codefeed", None)
            .expect("collection should succeed");

        assert_eq!(
            result.revision.as_deref(),
            Some("c1863c8b05c5d2db4030bc8c8a7d2f10bda4a950")
        );
        assert_eq!(result.label, "https://github.com/but212/codefeed");

        let paths: Vec<&str> = result.files.iter().map(|f| f.path).collect();
        assert_eq!(paths, vec!["Cargo.toml", "src/main.rs"]);

        // Cargo.lock is ignored (neither candidate nor skipped)
        // image.png is binary (contains NUL byte -> skipped = 1)
        assert_eq!(result.skipped, 1);
    }

    #[test]
    fn skips_too_large_files() {
        use crate::core::policy::MAX_FILE_SIZE;

        let mock_data = create_mock_tarball(&[
            ("repo-root/small.txt", b"small", EntryType::Regular),
            (
                "repo-root/large.txt",
                &vec![b'x'; (MAX_FILE_SIZE + 10) as usize],
                EntryType::Regular,
            ),
        ]);

        let result = collect_archive(mock_data.as_slice(), "owner", "repo", Some("main"))
            .expect("collection should succeed");

        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files.iter().next().unwrap().path, "small.txt");
        assert_eq!(result.skipped, 1);
    }

    #[test]
    fn regular_file_named_pax_global_header_is_not_dropped() {
        let mock_data = create_mock_tarball(&[(
            "repo-root/pax_global_header",
            b"hello content",
            EntryType::Regular,
        )]);

        let result = collect_archive(mock_data.as_slice(), "owner", "repo", None)
            .expect("collection should succeed");

        assert_eq!(result.files.len(), 1);
        let first = result.files.iter().next().unwrap();
        assert_eq!(first.path, "pax_global_header");
        assert_eq!(first.content, "hello content");
        assert_eq!(result.revision, None);
    }

    #[test]
    fn non_utf8_symlink_or_dir_is_ignored() {
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        {
            let mut tar = Builder::new(&mut gz);
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(EntryType::Symlink);
            header.set_size(0);
            header.set_mode(0o777);
            let name1 = b"repo-root/\xff_symlink";
            header.as_mut_bytes()[..name1.len()].copy_from_slice(name1);
            header.set_cksum();
            tar.append(&header, &b""[..]).unwrap();

            let mut header2 = tar::Header::new_gnu();
            header2.set_entry_type(EntryType::Directory);
            header2.set_size(0);
            header2.set_mode(0o755);
            let name2 = b"repo-root/\xff_dir/";
            header2.as_mut_bytes()[..name2.len()].copy_from_slice(name2);
            header2.set_cksum();
            tar.append(&header2, &b""[..]).unwrap();

            tar.finish().unwrap();
        }
        let mock_data = gz.finish().unwrap();

        let result = collect_archive(mock_data.as_slice(), "owner", "repo", None)
            .expect("collection should succeed");

        assert_eq!(result.files.len(), 0);
        assert_eq!(result.skipped, 0);
    }

    #[test]
    fn non_utf8_hidden_regular_file_is_ignored() {
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        {
            let mut tar = Builder::new(&mut gz);
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(EntryType::Regular);
            header.set_size(5);
            header.set_mode(0o644);
            let name = b"repo-root/.git/\xff";
            header.as_mut_bytes()[..name.len()].copy_from_slice(name);
            header.set_cksum();
            tar.append(&header, &b"dummy"[..]).unwrap();
            tar.finish().unwrap();
        }
        let mock_data = gz.finish().unwrap();

        let result = collect_archive(mock_data.as_slice(), "owner", "repo", None)
            .expect("collection should succeed");

        assert_eq!(result.files.len(), 0);
        assert_eq!(
            result.skipped, 0,
            "hidden entries must never be accounted as candidates or skipped"
        );
    }

    #[test]
    fn accepts_pax_header_after_archive_entries() {
        let pax_comment = b"52 comment=c1863c8b05c5d2db4030bc8c8a7d2f10bda4a950\n";
        let mock_data = create_mock_tarball(&[
            ("repo-root/file.txt", b"content", EntryType::Regular),
            ("pax_global_header", pax_comment, EntryType::XGlobalHeader),
        ]);

        let result = collect_archive(mock_data.as_slice(), "owner", "repo", None)
            .expect("collection should succeed");

        assert_eq!(
            result.revision.as_deref(),
            Some("c1863c8b05c5d2db4030bc8c8a7d2f10bda4a950")
        );
        assert_eq!(result.files.len(), 1);
    }
}
