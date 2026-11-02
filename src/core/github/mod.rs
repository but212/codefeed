//! GitHub remote-source domain: owner/repo URL rules and retry policy live in submodules;
//! this module owns the user-facing error and warning text.

use std::fmt::{Display, Formatter, Result};
use std::time::Duration;

use crate::core::policy::MAX_ATTEMPTS;

mod retry;
mod url;

use crate::core::error::sanitize_error_detail;
pub(crate) use retry::{AttemptState, RateLimitHeaders, RequestOutcome, Transition, TransportKind};
pub(crate) use url::{
    GITHUB_API_BASE_URL, is_github_api_origin, parse_github_url, repository_url, tarball_url,
    valid_github_name,
};

/// Non-success GitHub API response. `Display` renders the message without the error-kind prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubApiError {
    /// HTTP status code of the response.
    pub status: u16,
    /// Delay the server requested before a retry, when it gave one.
    pub retry_after: Option<Duration>,
    message: String,
}

impl Display for GithubApiError {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for GithubApiError {}

/// Error for a non-success response. `status_text` is the status as `Display` renders it
/// (`429 Too Many Requests`), so the message keeps the reason phrase.
#[must_use]
pub(crate) fn api_error(
    status: u16,
    status_text: &str,
    body: &[u8],
    retry_after: Option<Duration>,
) -> GithubApiError {
    let raw_detail = String::from_utf8_lossy(body);
    let detail = sanitize_error_detail(&raw_detail);
    let retry_info = retry_after
        .map(|d| format!(" (retry available in {}s)", d.as_secs()))
        .unwrap_or_default();

    let message = if matches!(status, 429 | 403) {
        format!(
            "HTTP {status_text}: {detail}{retry_info} (set GITHUB_TOKEN or GH_TOKEN to authenticate or increase rate limits)"
        )
    } else {
        format!("HTTP {status_text}: {detail}{retry_info}")
    };
    GithubApiError {
        status,
        retry_after,
        message,
    }
}

/// Warning printed before retrying a non-success response.
#[must_use]
pub(crate) fn status_retry_warning(
    status_text: &str,
    attempt: usize,
    url: &str,
    delay: Duration,
) -> String {
    format!(
        "warning: GitHub HTTP status {status_text} on attempt {attempt}/{MAX_ATTEMPTS} for {url}; retrying in {delay:?}"
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
        // (status, status text, body, retry-after, expected message)
        type Row<'a> = (u16, &'a str, &'a [u8], Option<Duration>, String);
        let cases: [Row<'_>; 5] = [
            (
                429,
                "429 Too Many Requests",
                &b"rate limit"[..],
                None,
                format!("HTTP 429 Too Many Requests: rate limit {TOKEN_HINT}"),
            ),
            (
                403,
                "403 Forbidden",
                &b"forbidden"[..],
                Some(Duration::from_secs(30)),
                format!("HTTP 403 Forbidden: forbidden (retry available in 30s) {TOKEN_HINT}"),
            ),
            (
                404,
                "404 Not Found",
                &b"not found"[..],
                None,
                "HTTP 404 Not Found: not found".into(),
            ),
            (
                404,
                "404 Not Found",
                &b"not found"[..],
                Some(Duration::from_secs(5)),
                "HTTP 404 Not Found: not found (retry available in 5s)".into(),
            ),
            (
                400,
                "400 Bad Request",
                &b"error\twith\nnewlines\r\nand\x1b[31mANSI\x07bell\x7fdel\x80c1"[..],
                None,
                "HTTP 400 Bad Request: error with newlines  and\u{FFFD}[31mANSI\u{FFFD}bell\u{FFFD}del\u{FFFD}c1".into(),
            ),
        ];
        for (status, status_text, body, retry_after, expected) in cases {
            let message = api_error(status, status_text, body, retry_after);
            assert_eq!(message.to_string(), expected, "status {status}");
        }
    }

    #[test]
    fn api_error_exposes_status_and_retry_after_separately_from_message() {
        let err = api_error(
            429,
            "429 Too Many Requests",
            b"rate limit",
            Some(Duration::from_secs(7)),
        );
        assert_eq!(err.status, 429);
        assert_eq!(err.retry_after, Some(Duration::from_secs(7)));
        assert_eq!(
            err.to_string(),
            "HTTP 429 Too Many Requests: rate limit (retry available in 7s) (set GITHUB_TOKEN or GH_TOKEN to authenticate or increase rate limits)"
        );
    }

    #[test]
    fn retry_messages_match_attempt_delay_and_transport_text() {
        let url = "https://api.github.com/repos/o/r/tarball";
        assert_eq!(
            status_retry_warning(
                "503 Service Unavailable",
                1,
                url,
                Duration::from_millis(250)
            ),
            format!(
                "warning: GitHub HTTP status 503 Service Unavailable on attempt 1/{MAX_ATTEMPTS} for {url}; retrying in 250ms"
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
