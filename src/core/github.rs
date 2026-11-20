//! GitHub remote-source domain: API/tarball URL helpers, PAX revision parsing, retry policy,
//! and API error types.

use std::fmt::{Display, Formatter, Result, Write as _};
use std::time::Duration;

use crate::core::error::ErrorDetail;
use crate::core::policy::{
    MAX_ATTEMPTS, RETRY_INITIAL_DELAY_MS, RETRY_MAX_DELAY_MS, TOKEN_ENV_VARS,
};

pub(crate) const GITHUB_API_BASE_URL: &str = "https://api.github.com";

/// Exact-origin match: returns `true` if `url == GITHUB_API_BASE_URL` or starts with `GITHUB_API_BASE_URL + "/"`.
#[must_use]
pub(crate) fn is_github_api_origin(url: &str) -> bool {
    url.strip_prefix(GITHUB_API_BASE_URL)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// Canonical public web URL for a repository, used as the digest label for remote sources.
#[must_use]
pub(crate) fn repository_url(owner: &str, repo: &str) -> String {
    format!("https://github.com/{owner}/{repo}")
}

/// Constructs the tarball download URL under `base` for the given repository and optional Git reference.
#[must_use]
pub(crate) fn tarball_url(base: &str, owner: &str, repo: &str, reference: Option<&str>) -> String {
    let mut url = String::from(base);
    for segment in ["repos", owner, repo, "tarball"] {
        url = append_encoded_segment(url, segment);
    }
    if let Some(reference) = reference {
        url = append_encoded_segment(url, reference);
    }
    url
}

/// RFC 3986 unreserved bytes: ASCII alphanumerics plus `-`, `.`, `_`, and `~`.
const fn is_unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~')
}

fn append_encoded_segment(mut url: String, segment: &str) -> String {
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
    url
}

/// Parses a 40- or 64-hex-character commit SHA from a `comment=<sha>` PAX record.
#[must_use]
pub(crate) fn parse_pax_comment(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    text.lines().find_map(|line| {
        let trimmed = line.trim_ascii();
        let payload = match trimmed.split_once(' ') {
            Some((len, rest)) if len.bytes().all(|b| b.is_ascii_digit()) => rest,
            _ => trimmed,
        };
        payload
            .strip_prefix("comment=")
            .map(str::trim_ascii)
            .filter(|sha| is_valid_commit_sha(sha))
            .map(ToOwned::to_owned)
    })
}

#[must_use]
fn is_valid_commit_sha(sha: &str) -> bool {
    matches!(sha.len(), 40 | 64) && sha.bytes().all(|b| b.is_ascii_hexdigit())
}

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
    let detail = ErrorDetail::new(String::from_utf8_lossy(body));
    let mut message = format!("HTTP {status}: {detail}");
    if let Some(d) = retry_after {
        let _ = write!(message, " (retry available in {}s)", d.as_secs());
    }
    if auth == Auth::Anonymous && matches!(status, 429 | 403) {
        let _ = write!(
            message,
            " (set {} to authenticate or increase rate limits)",
            TOKEN_ENV_VARS.join(" or ")
        );
    }

    GithubApiError {
        status,
        retry_after,
        message,
    }
}

fn retry_warning(reason: impl Display, attempt: usize, url: &str, delay: Duration) -> String {
    format!(
        "warning: GitHub {reason} on attempt {attempt}/{MAX_ATTEMPTS} for {url}; retrying in {delay:?}"
    )
}

/// Warning printed before retrying a non-success response.
#[must_use]
pub(crate) fn status_retry_warning(
    status: u16,
    attempt: usize,
    url: &str,
    delay: Duration,
) -> String {
    retry_warning(format_args!("HTTP status {status}"), attempt, url, delay)
}

/// Warning printed before retrying a transport failure. `error_text` is the transport error's `Display`.
#[must_use]
pub(crate) fn transport_retry_warning(
    error_text: &str,
    attempt: usize,
    url: &str,
    delay: Duration,
) -> String {
    retry_warning(
        format_args!("transport error: {}", ErrorDetail::new(error_text)),
        attempt,
        url,
        delay,
    )
}

/// Returns `true` if HTTP status code is transient/retryable (HTTP 429, 5xx, or rate-limited 403).
#[must_use]
const fn is_retryable_http_status(status_code: u16, is_rate_limit: bool) -> bool {
    matches!(status_code, 429 | 500..=599) || (status_code == 403 && is_rate_limit)
}

/// Calculates bounded exponential backoff duration for the given attempt.
#[must_use]
const fn retry_delay(attempt: usize) -> Duration {
    let bounded_attempt = if attempt < 16 { attempt } else { 16 };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "bounded_attempt is clamped to <= 16 which fits in u32"
    )]
    let shift = bounded_attempt as u32;
    let exponential = RETRY_INITIAL_DELAY_MS.saturating_mul(1_u64 << shift);
    let millis = if exponential < RETRY_MAX_DELAY_MS {
        exponential
    } else {
        RETRY_MAX_DELAY_MS
    };

    Duration::from_millis(millis)
}

const _: () = {
    assert!(MAX_ATTEMPTS >= 1);
    assert!(RETRY_INITIAL_DELAY_MS > 0);
    assert!(RETRY_INITIAL_DELAY_MS <= RETRY_MAX_DELAY_MS);
    assert!(retry_delay(0).as_millis() == RETRY_INITIAL_DELAY_MS as u128);
    assert!(retry_delay(usize::MAX).as_millis() == RETRY_MAX_DELAY_MS as u128);
};

/// Rate-limit facts from one response's headers. The shell reads the raw values; this type decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RateLimitHeaders {
    /// No `Retry-After` header and quota is not exhausted.
    NotLimited,
    /// `Retry-After` was present or quota was exhausted, with an optional resolved wait.
    Limited { retry_after: Option<Duration> },
}

impl RateLimitHeaders {
    /// Parses numeric header values against `now_secs`, where `None` means the header was absent.
    /// The shell resolves valid HTTP-date values before numeric parsing.
    pub(crate) fn parse(
        retry_after: Option<&str>,
        remaining: Option<&str>,
        reset: Option<&str>,
        now_secs: u64,
    ) -> Self {
        if let Some(secs) = retry_after.and_then(|value| {
            let value = value.trim();
            value.parse().ok().or_else(|| {
                // All-digit overflow is still a delay above the retry cap, not a missing hint.
                (!value.is_empty() && value.bytes().all(|b| b.is_ascii_digit())).then_some(u64::MAX)
            })
        }) {
            Self::Limited {
                retry_after: Some(Duration::from_secs(secs)),
            }
        } else if remaining.is_some_and(|v| v.trim() == "0") {
            let retry_after = reset
                .and_then(|v| v.trim().parse::<u64>().ok())
                .filter(|&reset| reset > now_secs)
                .map(|reset| Duration::from_secs(reset - now_secs));
            Self::Limited { retry_after }
        } else if retry_after.is_some() {
            Self::Limited { retry_after: None }
        } else {
            Self::NotLimited
        }
    }

    /// Delay before retrying. `Retry-After` wins; otherwise the reset epoch counts only once quota is exhausted.
    #[must_use]
    pub(crate) const fn retry_after(self) -> Option<Duration> {
        match self {
            Self::Limited { retry_after } => retry_after,
            Self::NotLimited => None,
        }
    }

    /// Rate-limited when `Retry-After` is present or quota is exhausted.
    #[must_use]
    const fn is_rate_limited(self) -> bool {
        !matches!(self, Self::NotLimited)
    }
}

/// Action outcome from a network / HTTP attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestOutcome {
    HttpStatus {
        status: u16,
        rate_limit: RateLimitHeaders,
    },
    TransportError {
        retryable: bool,
    },
}

/// Transition outcome from an HTTP attempt state transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Transition {
    Retry { next: AttemptState, delay: Duration },
    Fail,
}

/// Finite state machine managing HTTP attempt state and retry transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AttemptState {
    attempt: usize,
}

impl AttemptState {
    pub(crate) const fn new() -> Self {
        Self { attempt: 0 }
    }

    /// Number of retries scheduled so far, used in the retry warning.
    #[must_use]
    pub(crate) const fn retry_count(self) -> usize {
        self.attempt
    }

    /// Advances state only when another attempt is scheduled.
    #[must_use]
    pub(crate) const fn step(self, outcome: &RequestOutcome) -> Transition {
        if self.attempt >= MAX_ATTEMPTS - 1 {
            return Transition::Fail;
        }
        match outcome {
            RequestOutcome::HttpStatus { status, rate_limit } => {
                if !is_retryable_http_status(*status, rate_limit.is_rate_limited()) {
                    return Transition::Fail;
                }
                match rate_limit.retry_after() {
                    Some(d) if d.as_millis() > RETRY_MAX_DELAY_MS as u128 => Transition::Fail,
                    Some(d) => self.retry(d),
                    None => self.retry(retry_delay(self.attempt)),
                }
            }
            RequestOutcome::TransportError { retryable: true } => {
                self.retry(retry_delay(self.attempt))
            }
            RequestOutcome::TransportError { retryable: false } => Transition::Fail,
        }
    }

    /// Consumes one attempt and schedules the next after `delay`.
    const fn retry(self, delay: Duration) -> Transition {
        Transition::Retry {
            next: Self {
                attempt: self.attempt + 1,
            },
            delay,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            ("http://127.0.0.1:8080/repos/owner/repo", false),
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
    fn builds_repository_url() {
        assert_eq!(
            repository_url("but212", "codefeed"),
            "https://github.com/but212/codefeed"
        );
    }

    #[test]
    fn builds_tarball_urls() {
        assert_eq!(
            tarball_url(GITHUB_API_BASE_URL, "rust-lang", "cargo", None),
            "https://api.github.com/repos/rust-lang/cargo/tarball"
        );
        assert_eq!(
            tarball_url(GITHUB_API_BASE_URL, "rust-lang", "cargo", Some("main")),
            "https://api.github.com/repos/rust-lang/cargo/tarball/main"
        );
        assert_eq!(
            tarball_url(GITHUB_API_BASE_URL, "owner", "repo", Some("feature/test")),
            "https://api.github.com/repos/owner/repo/tarball/feature%2Ftest"
        );
        assert_eq!(
            tarball_url("http://127.0.0.1:8080", "owner", "repo", Some("main")),
            "http://127.0.0.1:8080/repos/owner/repo/tarball/main"
        );
    }

    #[test]
    fn tarball_url_encodes_every_byte_outside_the_unreserved_set() {
        const PREFIX: &str = "https://api.github.com/repos/o/r/tarball/";
        for byte in 0..128u8 {
            let c = char::from(byte);
            let expected = if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~') {
                c.to_string()
            } else {
                format!("%{byte:02X}")
            };
            assert_eq!(
                tarball_url(GITHUB_API_BASE_URL, "o", "r", Some(&c.to_string())),
                format!("{PREFIX}{expected}"),
                "{c:?}"
            );
        }
        assert_eq!(
            tarball_url(GITHUB_API_BASE_URL, "o", "r", Some("\u{e9}")),
            format!("{PREFIX}%C3%A9")
        );
    }

    #[test]
    fn parses_git_archive_pax_comment() {
        let pax_header = b"52 comment=c1863c8b05c5d2db4030bc8c8a7d2f10bda4a950\n";
        assert_eq!(
            parse_pax_comment(pax_header),
            Some("c1863c8b05c5d2db4030bc8c8a7d2f10bda4a950".into())
        );

        let direct_comment = b"comment=c1863c8b05c5d2db4030bc8c8a7d2f10bda4a950\n";
        assert_eq!(
            parse_pax_comment(direct_comment),
            Some("c1863c8b05c5d2db4030bc8c8a7d2f10bda4a950".into())
        );

        let invalid = b"52 comment=not-a-sha\n";
        assert_eq!(parse_pax_comment(invalid), None);

        // A non-UTF-8 PAX record is not a commit SHA, and the parser must not panic.
        let non_utf8 = b"comment=\xFF\xFEc1863c8b05c5d2db4030bc8c8a7d2f10bda4a950\n";
        assert_eq!(parse_pax_comment(non_utf8), None);

        // Only exactly 40 or 64 hex characters are a commit SHA.
        let boundary = [
            ("a".repeat(39), false),
            ("a".repeat(40), true),
            ("a".repeat(41), false),
            ("a".repeat(63), false),
            ("a".repeat(64), true),
            ("a".repeat(65), false),
        ];
        for (sha, valid) in boundary {
            let line = format!("comment={sha}\n");
            assert_eq!(parse_pax_comment(line.as_bytes()).is_some(), valid, "{sha}");
        }
        let non_hex = format!("comment={}g\n", "a".repeat(39));
        assert_eq!(parse_pax_comment(non_hex.as_bytes()), None);

        let empty = b"";
        assert_eq!(parse_pax_comment(empty), None);

        // Only a decimal length may precede the record; any other prefix word voids it.
        let prefixed = format!("ab comment={}\n", "a".repeat(40));
        assert_eq!(parse_pax_comment(prefixed.as_bytes()), None);
    }

    #[test]
    fn api_error_message_keeps_literal_contract_text() {
        // Literal anchors for the oracle in `api_error_message_is_exact_for_every_http_status_code_and_auth`.
        const TOKEN_HINT: &str =
            "(set GITHUB_TOKEN or GH_TOKEN to authenticate or increase rate limits)";
        let err = api_error(
            429,
            b"rate limit",
            Some(Duration::from_secs(7)),
            Auth::Anonymous,
        );
        assert_eq!(
            err.to_string(),
            format!("HTTP 429: rate limit (retry available in 7s) {TOKEN_HINT}")
        );

        let err = api_error(
            400,
            b"error\twith\nnewlines\r\nand\x1b[31mANSI\x07bell\x7fdel\x80c1",
            None,
            Auth::Anonymous,
        );
        assert_eq!(
            err.to_string(),
            "HTTP 400: error with newlines  and\u{FFFD}[31mANSI\u{FFFD}bell\u{FFFD}del\u{FFFD}c1"
        );
    }

    #[test]
    fn token_hint_names_every_token_env_var_in_lookup_order() {
        assert_eq!(TOKEN_ENV_VARS, ["GITHUB_TOKEN", "GH_TOKEN"]);
        let message = api_error(403, b"x", None, Auth::Anonymous).to_string();
        for var in TOKEN_ENV_VARS {
            assert!(message.contains(var), "{var} missing from: {message}");
        }
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

    #[test]
    fn classifies_retryable_status_codes() {
        // Retryable: 429, every 5xx, and 403 only when it signals a rate limit.
        for code in 100..=599_u16 {
            for is_rate_limit in [false, true] {
                let expected =
                    code == 429 || (500..=599).contains(&code) || (code == 403 && is_rate_limit);
                assert_eq!(
                    is_retryable_http_status(code, is_rate_limit),
                    expected,
                    "code {code} rate_limit {is_rate_limit}"
                );
            }
        }
        // Literal anchors for the boundaries of the model above.
        assert!(is_retryable_http_status(403, true));
        assert!(!is_retryable_http_status(403, false));
        assert!(is_retryable_http_status(599, false));
        assert!(!is_retryable_http_status(499, true));
    }

    #[test]
    fn calculates_bounded_exponential_backoff() {
        assert_eq!(retry_delay(0).as_millis(), 250);
        assert_eq!(retry_delay(1).as_millis(), 500);
        assert_eq!(retry_delay(2).as_millis(), 1000);
        assert_eq!(retry_delay(3).as_millis(), 2000);
        assert_eq!(retry_delay(4).as_millis(), 4000);
        assert_eq!(retry_delay(5).as_millis(), 5000);
        assert_eq!(retry_delay(10).as_millis(), 5000);
    }

    #[test]
    fn retry_after_honors_quota_state() {
        const NOW: u64 = 1_000_000;
        // (retry-after, x-ratelimit-remaining, x-ratelimit-reset, now, expected delay in seconds)
        type Row<'a> = (
            Option<&'a str>,
            Option<&'a str>,
            Option<&'a str>,
            u64,
            Option<u64>,
        );
        let cases: [Row<'_>; 9] = [
            (None, Some("59"), Some("1002400"), NOW, None),
            (None, Some("0"), Some("1002400"), NOW, Some(2400)),
            // A reset at `now` is already due: no wait.
            (None, Some("0"), Some("1000000"), NOW, None),
            (Some("2"), Some("0"), Some("1002400"), NOW, Some(2)),
            (Some("2"), Some("59"), Some("1002400"), NOW, Some(2)),
            (None, None, Some("1002400"), NOW, None),
            (None, Some("0"), Some("999"), NOW, None),
            (None, Some("0"), Some("1002400"), u64::MAX, None),
            // An invalid hint falls back to backoff; HTTP-dates are resolved by the shell.
            (Some("soon"), None, None, NOW, None),
        ];

        for (retry_after, remaining, reset, now, expected) in cases {
            let got = RateLimitHeaders::parse(retry_after, remaining, reset, now)
                .retry_after()
                .map(|d| d.as_secs());
            assert_eq!(
                got, expected,
                "retry-after={retry_after:?} remaining={remaining:?} reset={reset:?} now={now}"
            );
        }
    }

    #[test]
    fn equivalent_rate_limit_signals_parse_to_the_same_facts() {
        let parse = RateLimitHeaders::parse;
        assert_eq!(
            parse(Some("soon"), None, None, 100),
            parse(None, Some("0"), None, 100)
        );
        assert_eq!(
            parse(Some("2"), None, None, 100),
            parse(None, Some("0"), Some("102"), 100)
        );
    }

    #[test]
    fn overflowing_retry_after_seconds_exceed_the_retry_cap() {
        for value in ["18446744073709551616", "999999999999999999999999999999"] {
            let facts = RateLimitHeaders::parse(Some(value), Some("0"), Some("1000001"), 1_000_000);
            let delay = facts.retry_after();
            assert_eq!(delay, Some(Duration::from_secs(u64::MAX)), "{value}");
            let transition = AttemptState::new().step(&RequestOutcome::HttpStatus {
                status: 429,
                rate_limit: facts,
            });
            assert_eq!(transition, Transition::Fail, "{value}");
        }
    }

    #[test]
    fn rate_limited_when_retry_after_present_or_quota_exhausted() {
        // (retry-after, x-ratelimit-remaining, expected)
        type Row<'a> = (Option<&'a str>, Option<&'a str>, bool);
        let cases: [Row<'_>; 5] = [
            (Some("1"), None, true),
            (Some("soon"), Some("59"), true),
            (None, Some("0"), true),
            (None, Some("59"), false),
            (None, None, false),
        ];
        for (retry_after, remaining, expected) in cases {
            let got = RateLimitHeaders::parse(retry_after, remaining, None, 0).is_rate_limited();
            assert_eq!(
                got, expected,
                "retry-after={retry_after:?} remaining={remaining:?}"
            );
        }
    }

    #[test]
    #[expect(clippy::too_many_lines, reason = "one table of retry transitions")]
    fn decides_retry_correctly() {
        let retry = |attempt, delay| Transition::Retry {
            next: AttemptState { attempt },
            delay,
        };
        let limited = |secs| RateLimitHeaders::Limited {
            retry_after: Some(Duration::from_secs(secs)),
        };
        let max_secs = RETRY_MAX_DELAY_MS / 1000;
        let cases = [
            // (attempt, outcome, expected)
            (
                0,
                RequestOutcome::HttpStatus {
                    status: 500,
                    rate_limit: RateLimitHeaders::NotLimited,
                },
                retry(1, retry_delay(0)),
            ),
            (
                0,
                RequestOutcome::HttpStatus {
                    status: 429,
                    rate_limit: limited(2),
                },
                retry(1, Duration::from_secs(2)),
            ),
            (
                0,
                RequestOutcome::HttpStatus {
                    status: 429,
                    rate_limit: limited(10),
                },
                Transition::Fail,
            ),
            (
                0,
                RequestOutcome::HttpStatus {
                    status: 429,
                    rate_limit: limited(max_secs),
                },
                retry(1, Duration::from_secs(max_secs)),
            ),
            (
                0,
                RequestOutcome::HttpStatus {
                    status: 429,
                    rate_limit: limited(max_secs + 1),
                },
                Transition::Fail,
            ),
            (
                0,
                RequestOutcome::HttpStatus {
                    status: 403,
                    rate_limit: RateLimitHeaders::Limited { retry_after: None },
                },
                retry(1, retry_delay(0)),
            ),
            (
                0,
                RequestOutcome::HttpStatus {
                    status: 403,
                    rate_limit: RateLimitHeaders::NotLimited,
                },
                Transition::Fail,
            ),
            (
                1,
                RequestOutcome::HttpStatus {
                    status: 429,
                    rate_limit: RateLimitHeaders::Limited { retry_after: None },
                },
                retry(2, retry_delay(1)),
            ),
            (
                2,
                RequestOutcome::HttpStatus {
                    status: 500,
                    rate_limit: RateLimitHeaders::NotLimited,
                },
                Transition::Fail,
            ),
            (
                0,
                RequestOutcome::HttpStatus {
                    status: 404,
                    rate_limit: RateLimitHeaders::NotLimited,
                },
                Transition::Fail,
            ),
            (
                0,
                RequestOutcome::TransportError { retryable: true },
                retry(1, retry_delay(0)),
            ),
            (
                0,
                RequestOutcome::TransportError { retryable: false },
                Transition::Fail,
            ),
        ];
        for (attempt, outcome, expected) in cases {
            let got = AttemptState { attempt }.step(&outcome);
            assert_eq!(got, expected, "attempt={attempt} outcome={outcome:?}");
        }
    }
}
