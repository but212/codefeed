//! GitHub remote-source domain: owner/repo URL rules and retry policy live in submodules;
//! this module owns the user-facing error and warning text.

use std::fmt::{Display, Formatter, Result};
use std::time::Duration;

use crate::core::policy::{MAX_ATTEMPTS, TOKEN_ENV_VARS};

mod retry;
mod url;

use crate::core::error::sanitize_error_detail;
pub(crate) use retry::{AttemptState, RateLimitHeaders, RequestOutcome, Transition, TransportKind};
pub(crate) use url::{
    GITHUB_API_BASE_URL, is_github_api_origin, parse_github_url, repository_url, tarball_url,
    valid_github_name,
};

/// Non-success GitHub API response. `Display` renders the message without the error-kind prefix.
///
/// All fields are private: the message is built once from the status and retry delay, so
/// callers read them through accessors and cannot make them disagree with the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubApiError {
    status: u16,
    retry_after: Option<Duration>,
    message: String,
}

impl GithubApiError {
    /// HTTP status code of the response.
    #[must_use]
    pub const fn status(&self) -> u16 {
        self.status
    }

    /// Delay the server requested before a retry, when it gave one.
    #[must_use]
    pub const fn retry_after(&self) -> Option<Duration> {
        self.retry_after
    }
}

impl Display for GithubApiError {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for GithubApiError {}

/// Whether the GitHub API request carried a token. A redirect target never receives it
/// (`RedirectAuthHeaders::Never`), so an error from one is classified by the API request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Auth {
    /// No token was sent.
    Anonymous,
    /// A token was sent to the GitHub API origin.
    Token,
}

/// Error for a non-success response. The message names `status`, so the code is the only source.
/// The token hint appears only for an anonymous 403 or 429: a token that was sent is not the fix.
#[must_use]
pub(crate) fn api_error(
    status: u16,
    body: &[u8],
    retry_after: Option<Duration>,
    auth: Auth,
) -> GithubApiError {
    let raw_detail = String::from_utf8_lossy(body);
    let detail = sanitize_error_detail(&raw_detail);
    let retry_info = retry_after
        .map(|d| format!(" (retry available in {}s)", d.as_secs()))
        .unwrap_or_default();
    let token_hint = match auth {
        Auth::Anonymous if matches!(status, 429 | 403) => format!(
            " (set {} to authenticate or increase rate limits)",
            TOKEN_ENV_VARS.join(" or ")
        ),
        Auth::Anonymous | Auth::Token => String::new(),
    };

    GithubApiError {
        status,
        retry_after,
        message: format!("HTTP {status}: {detail}{retry_info}{token_hint}"),
    }
}

/// Warning printed before retrying a non-success response.
#[must_use]
pub(crate) fn status_retry_warning(
    status: u16,
    attempt: usize,
    url: &str,
    delay: Duration,
) -> String {
    format!(
        "warning: GitHub HTTP status {status} on attempt {attempt}/{MAX_ATTEMPTS} for {url}; retrying in {delay:?}"
    )
}

/// Warning printed before retrying a transport failure. `error_text` is the transport error's `Display`.
#[must_use]
pub(crate) fn transport_retry_warning(
    error_text: &str,
    attempt: usize,
    url: &str,
    delay: Duration,
) -> String {
    format!(
        "warning: GitHub transport error: {} on attempt {attempt}/{MAX_ATTEMPTS} for {url}; retrying in {delay:?}",
        sanitize_error_detail(error_text)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_error_message_keeps_status_detail_and_token_hint() {
        const TOKEN_HINT: &str =
            "(set GITHUB_TOKEN or GH_TOKEN to authenticate or increase rate limits)";
        // (status, body, retry-after, auth, expected message)
        type Row<'a> = (u16, &'a [u8], Option<Duration>, Auth, String);
        let cases: [Row<'_>; 7] = [
            (
                429,
                &b"rate limit"[..],
                None,
                Auth::Anonymous,
                format!("HTTP 429: rate limit {TOKEN_HINT}"),
            ),
            (
                403,
                &b"forbidden"[..],
                Some(Duration::from_secs(30)),
                Auth::Anonymous,
                format!("HTTP 403: forbidden (retry available in 30s) {TOKEN_HINT}"),
            ),
            (
                403,
                &b"forbidden"[..],
                None,
                Auth::Token,
                "HTTP 403: forbidden".into(),
            ),
            (
                429,
                &b"rate limit"[..],
                Some(Duration::from_secs(30)),
                Auth::Token,
                "HTTP 429: rate limit (retry available in 30s)".into(),
            ),
            (
                404,
                &b"not found"[..],
                None,
                Auth::Anonymous,
                "HTTP 404: not found".into(),
            ),
            (
                404,
                &b"not found"[..],
                Some(Duration::from_secs(5)),
                Auth::Anonymous,
                "HTTP 404: not found (retry available in 5s)".into(),
            ),
            (
                400,
                &b"error\twith\nnewlines\r\nand\x1b[31mANSI\x07bell\x7fdel\x80c1"[..],
                None,
                Auth::Anonymous,
                "HTTP 400: error with newlines  and\u{FFFD}[31mANSI\u{FFFD}bell\u{FFFD}del\u{FFFD}c1".into(),
            ),
        ];
        for (status, body, retry_after, auth, expected) in cases {
            let message = api_error(status, body, retry_after, auth);
            assert_eq!(message.to_string(), expected, "status {status} {auth:?}");
        }
    }

    #[test]
    fn token_hint_names_every_token_env_var_in_lookup_order() {
        assert_eq!(TOKEN_ENV_VARS, ["GITHUB_TOKEN", "GH_TOKEN"]);
        let message = api_error(403, b"x", None, Auth::Anonymous).to_string();
        for var in TOKEN_ENV_VARS {
            assert!(message.contains(var), "{var} missing from: {message}");
        }
    }

    #[test]
    fn api_error_exposes_status_and_retry_after_separately_from_message() {
        let err = api_error(
            429,
            b"rate limit",
            Some(Duration::from_secs(7)),
            Auth::Anonymous,
        );
        assert_eq!(err.status(), 429);
        assert_eq!(err.retry_after(), Some(Duration::from_secs(7)));
        assert_eq!(
            err.to_string(),
            "HTTP 429: rate limit (retry available in 7s) (set GITHUB_TOKEN or GH_TOKEN to authenticate or increase rate limits)"
        );
    }

    /// Exact message the contract requires. Only an anonymous 403 or 429 carries the token hint.
    fn expected_message(status: u16, auth: Auth, detail: &str, retry: &str) -> String {
        const TOKEN_HINT: &str =
            "(set GITHUB_TOKEN or GH_TOKEN to authenticate or increase rate limits)";
        if auth == Auth::Anonymous && matches!(status, 403 | 429) {
            format!("HTTP {status}: {detail}{retry} {TOKEN_HINT}")
        } else {
            format!("HTTP {status}: {detail}{retry}")
        }
    }

    #[test]
    fn api_error_message_is_exact_for_every_http_status_code_and_auth() {
        for auth in [Auth::Anonymous, Auth::Token] {
            for status in 100..=599_u16 {
                let err = api_error(status, b"x", None, auth);
                assert_eq!(err.status(), status);
                assert_eq!(err.retry_after(), None, "status {status} {auth:?}");
                assert_eq!(
                    err.to_string(),
                    expected_message(status, auth, "x", ""),
                    "status {status} {auth:?}"
                );

                let err = api_error(status, b"x", Some(Duration::from_millis(1900)), auth);
                assert_eq!(err.retry_after(), Some(Duration::from_millis(1900)));
                assert_eq!(
                    err.to_string(),
                    expected_message(status, auth, "x", " (retry available in 1s)"),
                    "status {status} {auth:?} with retry"
                );
            }
        }
    }

    #[test]
    fn api_error_retry_hint_truncates_to_whole_seconds() {
        let cases = [
            (Duration::ZERO, " (retry available in 0s)"),
            (Duration::from_millis(999), " (retry available in 0s)"),
            (Duration::from_millis(1000), " (retry available in 1s)"),
        ];
        for (retry, hint) in cases {
            assert_eq!(
                api_error(404, b"x", Some(retry), Auth::Anonymous).to_string(),
                expected_message(404, Auth::Anonymous, "x", hint),
                "retry {retry:?}"
            );
        }
    }

    #[test]
    fn retry_messages_match_attempt_delay_and_transport_text() {
        let url = "https://api.github.com/repos/o/r/tarball";
        assert_eq!(
            status_retry_warning(503, 1, url, Duration::from_millis(250)),
            format!(
                "warning: GitHub HTTP status 503 on attempt 1/{MAX_ATTEMPTS} for {url}; retrying in 250ms"
            )
        );
        assert_eq!(
            transport_retry_warning("connection\treset\nby peer", 2, url, Duration::from_secs(1)),
            format!(
                "warning: GitHub transport error: connection reset by peer on attempt 2/{MAX_ATTEMPTS} for {url}; retrying in 1s"
            )
        );
    }
}
