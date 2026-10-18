//! GitHub collection orchestration: single-request streaming tarball ingestion and HTTP transport.

use std::io::{BufReader, Read};
use std::time::Duration;

use flate2::read::GzDecoder;
use tar::{Archive, EntryType};
use ureq::Agent;
use ureq::config::RedirectAuthHeaders;
use ureq::http::StatusCode;
use ureq::tls::{RootCerts, TlsConfig};

use crate::core::archive::{normalize_tar_entry_path, parse_pax_comment};
use crate::core::error::IngestError;
use crate::core::model::CollectionResult;
use crate::core::policy::{
    ARCHIVE_MAX_SECS, MAX_FILE_SIZE, MAX_RETRIES, PathStatus, is_retryable_status, retry_delay,
};
use crate::core::selector::Selector;

const GITHUB_API_BASE_URL: &str = "https://api.github.com";

/// Exact-origin match: returns `true` if `url == GITHUB_API_BASE_URL` or starts with `GITHUB_API_BASE_URL + "/"`.
fn is_github_api_origin(url: &str) -> bool {
    url.strip_prefix(GITHUB_API_BASE_URL)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// Constructs the tarball download URL for the given repository and optional Git reference.
fn tarball_url(owner: &str, repo: &str, reference: Option<&str>) -> String {
    let mut url = String::from(GITHUB_API_BASE_URL);
    for segment in ["repos", owner, repo, "tarball"] {
        push_encoded_segment(&mut url, segment);
    }
    if let Some(reference) = reference {
        push_encoded_segment(&mut url, reference);
    }
    url
}

#[inline(always)]
const fn is_unreserved(b: u8) -> bool {
    ((b | 32).wrapping_sub(b'a') < 26)
        | (b.wrapping_sub(b'0') < 10)
        | (b == b'-')
        | (b == b'.')
        | (b == b'_')
        | (b == b'~')
}

fn push_encoded_segment(url: &mut String, segment: &str) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    url.push('/');
    for &b in segment.as_bytes() {
        if is_unreserved(b) {
            url.push(b as char);
        } else {
            url.push('%');
            url.push(HEX[(b >> 4) as usize] as char);
            url.push(HEX[(b & 0xF) as usize] as char);
        }
    }
}

const GITHUB_USER_AGENT: &str = concat!("codefeed/", env!("CARGO_PKG_VERSION"));
const GITHUB_TIMEOUT_SECS: u64 = 30;

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
    auth_header: Option<String>,
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

        let auth_header = resolve_token_from_env().map(|token| format!("Bearer {token}"));

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
        let mut attempt = 0;
        loop {
            let mut request = self.agent.get(url);

            if is_github_api_origin(url) {
                if let Some(auth) = &self.auth_header {
                    request = request.header("Authorization", auth);
                }
                request = request.header("X-Github-Api-Version", "2022-11-28");
            }

            let retry = |attempt: &mut usize, reason: &dyn std::fmt::Display| {
                *attempt += 1;
                let delay = retry_delay(*attempt - 1);
                eprintln!(
                    "warning: {reason} on attempt {attempt}/{MAX_RETRIES} for {url}; retrying in {delay:?}"
                );
                std::thread::sleep(delay);
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
                    return Err(IngestError::http(error.to_string()));
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

const fn is_retryable_transport_error(error: &ureq::Error) -> bool {
    matches!(
        error,
        ureq::Error::Io(_) | ureq::Error::Timeout(_) | ureq::Error::ConnectionFailed
    )
}

fn api_error(status: StatusCode, body_bytes: &[u8]) -> IngestError {
    let detail = String::from_utf8_lossy(body_bytes).replace('\n', " ");
    if status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::FORBIDDEN {
        IngestError::api(format!(
            "HTTP {status}: {detail} (set GITHUB_TOKEN or GH_TOKEN to authenticate or increase rate limits)"
        ))
    } else {
        IngestError::api(format!("HTTP {status}: {detail}"))
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
    let mut path_scratch = String::with_capacity(256);
    let mut content_scratch = Vec::with_capacity(32 * 1024);

    for entry_res in entries {
        let mut entry = match entry_res {
            Ok(e) => e,
            Err(err) => return Err(IngestError::Io(err)),
        };
        path_scratch.clear();
        content_scratch.clear();
        if let Some(sha) = process_tar_entry(
            &mut entry,
            &mut selector,
            &mut path_scratch,
            &mut content_scratch,
        )? && parsed_revision.is_none()
        {
            parsed_revision = Some(sha);
        }
    }

    let revision = parsed_revision.or_else(|| reference.map(str::to_string));
    Ok(selector.finish(label, revision))
}

fn process_tar_entry<R: Read>(
    entry: &mut tar::Entry<R>,
    selector: &mut Selector,
    path_scratch: &mut String,
    content_scratch: &mut Vec<u8>,
) -> Result<Option<String>, IngestError> {
    let entry_type = entry.header().entry_type();

    // 1. PAX Global Header: Extract git commit SHA from comment
    if entry_type == EntryType::XGlobalHeader {
        entry
            .take(1024)
            .read_to_end(content_scratch)
            .map_err(IngestError::Io)?;
        return Ok(parse_pax_comment(content_scratch));
    }

    // 2. Skip directories, symlinks, and special entries before path decoding
    if !matches!(entry_type, EntryType::Regular | EntryType::Continuous) {
        return Ok(None);
    }

    // 3. Path decoding, root stripping, normalization & policy classification
    let path_bytes = entry.path_bytes();
    match normalize_tar_entry_path(&path_bytes, path_scratch)? {
        PathStatus::Ignored => return Ok(None),
        PathStatus::NonUtf8Candidate => {
            selector.record_skipped(path_scratch)?;
            return Ok(None);
        }
        PathStatus::Valid => {}
    }

    let rel_path = path_scratch.as_str();

    // 4. Pre-I/O candidate reservation before reading entry body
    selector.record_with(rel_path, || read_tar_entry_into(entry, content_scratch))?;

    Ok(None)
}

fn read_tar_entry_into<'b, R: Read>(
    entry: &mut tar::Entry<R>,
    buf: &'b mut Vec<u8>,
) -> Result<Option<&'b [u8]>, IngestError> {
    let header_size = entry.header().size().unwrap_or(0);
    if header_size > MAX_FILE_SIZE {
        return Ok(None);
    }

    entry
        .take(MAX_FILE_SIZE.saturating_add(1))
        .read_to_end(buf)
        .map_err(IngestError::Io)?;

    if buf.len() as u64 > MAX_FILE_SIZE {
        return Ok(None);
    }
    Ok(Some(buf.as_slice()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::collections::HashMap;
    use std::env::VarError;
    use tar::Builder;

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

    #[test]
    fn user_agent_matches_cargo_package_version() {
        assert_eq!(
            GITHUB_USER_AGENT,
            concat!("codefeed/", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn strips_authorization_header_on_redirect() {
        use std::io::Write;
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let port = listener.local_addr().expect("local addr").port();

        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept 1");
            let mut buf = [0u8; 2048];
            let n = stream.read(&mut buf).expect("read 1");
            let req1 = String::from_utf8_lossy(&buf[..n]);
            assert!(
                req1.to_ascii_lowercase()
                    .contains("authorization: bearer test-token"),
                "first request must contain authorization header: {req1}"
            );
            let response = format!(
                "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{port}/dest\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            let _ = stream.write_all(response.as_bytes());

            let (mut stream, _) = listener.accept().expect("accept 2");
            let mut buf = [0u8; 2048];
            let n = stream.read(&mut buf).expect("read 2");
            let req2 = String::from_utf8_lossy(&buf[..n]);
            assert!(
                !req2.to_ascii_lowercase().contains("authorization"),
                "redirected request must not contain authorization header: {req2}"
            );
            let response =
                "HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nsuccess";
            let _ = stream.write_all(response.as_bytes());
        });

        let client = GithubClient::new();
        let url = format!("http://127.0.0.1:{port}/start");
        let resp = client
            .agent
            .get(&url)
            .header("Authorization", "Bearer test-token")
            .call()
            .expect("call should follow redirect and succeed");
        assert_eq!(resp.status(), StatusCode::OK);

        handle.join().expect("server thread finish");
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

    fn encode_segment(segment: &str) -> String {
        let mut out = String::new();
        push_encoded_segment(&mut out, segment);
        out.strip_prefix('/').unwrap().to_string()
    }

    #[test]
    fn percent_encodes_url_path_segments() {
        assert_eq!(encode_segment("AZaz09-._~"), "AZaz09-._~");
        assert_eq!(encode_segment("feature/x"), "feature%2Fx");
        assert_eq!(encode_segment("a b"), "a%20b");
        assert_eq!(encode_segment("100%"), "100%25");
        assert_eq!(encode_segment("a#b?c"), "a%23b%3Fc");
        assert_eq!(encode_segment("café"), "caf%C3%A9");
    }

    #[test]
    fn origin_classification_identifies_github_api_correctly() {
        let cases = [
            ("https://api.github.com/repos/owner/repo", true),
            ("https://api.github.com", true),
            (
                "https://raw.githubusercontent.com/owner/repo/main/lib.rs",
                false,
            ),
            ("https://raw.githubusercontent.com", false),
            ("https://api.github.com.evil.example/x", false),
            ("https://api.github.com@evil.example/x", false),
            ("https://raw.githubusercontent.com.evil.example/x", false),
            ("https://raw.githubusercontent.com@evil.example/x", false),
            ("http://127.0.0.1:8080/repos/owner/repo", false),
            ("https://evil.example/", false),
        ];
        for (url, expected) in cases {
            assert_eq!(
                is_github_api_origin(url),
                expected,
                "unexpected auth scope for: {url}"
            );
        }
    }

    #[test]
    fn builds_tarball_urls() {
        assert_eq!(
            tarball_url("rust-lang", "cargo", None),
            "https://api.github.com/repos/rust-lang/cargo/tarball"
        );
        assert_eq!(
            tarball_url("rust-lang", "cargo", Some("main")),
            "https://api.github.com/repos/rust-lang/cargo/tarball/main"
        );
        assert_eq!(
            tarball_url("owner", "repo", Some("feature/test")),
            "https://api.github.com/repos/owner/repo/tarball/feature%2Ftest"
        );
    }
}
