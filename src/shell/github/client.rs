//! GitHub HTTP client, TLS configuration, and authentication token resolution.

use std::time::Duration;

use ureq::config::RedirectAuthHeaders;
use ureq::tls::{RootCerts, TlsConfig};
use ureq::typestate::WithoutBody;
use ureq::{Agent, RequestBuilder};
use zeroize::Zeroizing;

use crate::core::github::{Auth, is_github_api_origin};
use crate::core::policy::{ARCHIVE_MAX_SECS, GITHUB_TIMEOUT_SECS, TOKEN_ENV_VARS};

pub(super) const GITHUB_USER_AGENT: &str = concat!("codefeed/", env!("CARGO_PKG_VERSION"));

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

        let auth_header = resolve_token_with(|k| std::env::var(k))
            .as_ref()
            .map(bearer_header);

        Self {
            agent: Agent::new_with_config(config),
            auth_header,
        }
    }

    /// Token sent with a request to `url`. Only the GitHub API origin receives it.
    fn credentials(&self, url: &str) -> Option<&Zeroizing<String>> {
        if is_github_api_origin(url) {
            self.auth_header.as_ref()
        } else {
            None
        }
    }

    /// Whether a request to `url` carries a token, for error classification.
    pub(super) fn auth(&self, url: &str) -> Auth {
        match self.credentials(url) {
            Some(_) => Auth::Token,
            None => Auth::Anonymous,
        }
    }

    /// Builds a GET for `url`. Auth and API-version headers are sent only to the GitHub API origin.
    pub(super) fn request(&self, url: &str) -> RequestBuilder<WithoutBody> {
        let request = self.agent.get(url);
        if !is_github_api_origin(url) {
            return request;
        }
        let request = match self.credentials(url) {
            Some(auth) => request.header("Authorization", auth.as_str()),
            None => request,
        };
        request.header("X-Github-Api-Version", "2022-11-28")
    }
}

pub(super) fn resolve_token_with<F>(mut lookup: F) -> Option<Zeroizing<String>>
where
    F: FnMut(&str) -> Result<String, std::env::VarError>,
{
    TOKEN_ENV_VARS.into_iter().find_map(|key| {
        let raw = Zeroizing::new(lookup(key).ok()?);
        let trimmed = raw.trim_ascii();
        (!trimmed.is_empty()).then(|| Zeroizing::new(trimmed.to_string()))
    })
}

fn bearer_header(token: &Zeroizing<String>) -> Zeroizing<String> {
    Zeroizing::new(format!("Bearer {}", token.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::testing::MockServer;
    use std::collections::HashMap;
    use std::env::VarError;
    use std::io::Read;
    use ureq::http::StatusCode;

    #[test]
    fn bearer_header_prefixes_the_token() {
        let header = bearer_header(&Zeroizing::new("ghp_test".into()));
        assert_eq!(header.as_str(), "Bearer ghp_test");
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

    #[test]
    fn auth_is_token_only_for_the_api_origin_with_a_token() {
        const API: &str = "https://api.github.com/repos/foo/bar/tarball";
        const CODELOAD: &str = "https://codeload.github.com/foo/bar/tar.gz";
        let mut client = GithubClient::new();

        client.auth_header = Some(Zeroizing::new("Bearer test_token".to_string()));
        assert_eq!(client.auth(API), Auth::Token);
        assert_eq!(client.auth(CODELOAD), Auth::Anonymous);

        client.auth_header = None;
        assert_eq!(client.auth(API), Auth::Anonymous);
        assert_eq!(client.auth(CODELOAD), Auth::Anonymous);
    }

    #[test]
    fn github_client_request_headers_for_api_and_non_api_origin() {
        let mut client = GithubClient::new();
        client.auth_header = Some(Zeroizing::new("Bearer test_token".to_string()));

        let non_api = client.request("https://codeload.github.com/foo/bar/tar.gz");
        let non_api_headers = non_api.headers_ref();
        assert_eq!(non_api_headers.and_then(|h| h.get("Authorization")), None);
        assert_eq!(
            non_api_headers.and_then(|h| h.get("X-Github-Api-Version")),
            None
        );

        let api = client.request("https://api.github.com/repos/foo/bar/tarball");
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
            Some("2022-11-28")
        );

        client.auth_header = None;
        let api_no_auth = client.request("https://api.github.com/repos/foo/bar/tarball");
        let api_no_auth_headers = api_no_auth.headers_ref();
        assert_eq!(
            api_no_auth_headers.and_then(|h| h.get("Authorization")),
            None
        );
        assert_eq!(
            api_no_auth_headers
                .and_then(|h| h.get("X-Github-Api-Version"))
                .and_then(|v| v.to_str().ok()),
            Some("2022-11-28")
        );
    }
}
