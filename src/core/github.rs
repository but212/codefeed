//! Pure GitHub domain rules, URL formatting, origin security policy, and retry decisions.

use crate::core::error::IngestError;
use crate::core::model::Source;
use crate::core::policy::{MAX_RETRIES, is_retryable_status, retry_delay};
use std::time::Duration;

pub(crate) const GITHUB_API_BASE_URL: &str = "https://api.github.com";

/// Exact-origin match: returns `true` if `url == GITHUB_API_BASE_URL` or starts with `GITHUB_API_BASE_URL + "/"`.
pub(crate) fn is_github_api_origin(url: &str) -> bool {
    url.strip_prefix(GITHUB_API_BASE_URL)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// Constructs the tarball download URL for the given repository and optional Git reference.
pub(crate) fn valid_github_name(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value.bytes().all(|byte| {
            (byte.wrapping_sub(b'0') < 10)
                | ((byte | 32).wrapping_sub(b'a') < 26)
                | (byte == b'-')
                | (byte == b'.')
                | (byte == b'_')
        })
}

pub(crate) fn parse_github_url(value: &str) -> Result<Source, IngestError> {
    let remainder = value.strip_prefix("https://").ok_or_else(|| {
        IngestError::unsupported_source("only https://github.com/{owner}/{repo} is supported")
    })?;
    let (authority, path) = match remainder.find('/') {
        Some(index) => (&remainder[..index], &remainder[index..]),
        None => (remainder, ""),
    };
    if authority.is_empty() {
        return Err(IngestError::invalid_source(value));
    }
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let host = match host_port.split_once(':') {
        Some((host, port)) => {
            if !port.is_empty() && port.parse::<u16>().is_err() {
                return Err(IngestError::invalid_source(value));
            }
            host
        }
        None => host_port,
    };
    if !host.eq_ignore_ascii_case("github.com") {
        return Err(IngestError::unsupported_source(
            "only https://github.com/{owner}/{repo} is supported",
        ));
    }
    if path.contains('?') || path.contains('#') {
        return Err(IngestError::invalid_source(
            "repository URLs cannot contain a query or fragment",
        ));
    }

    let mut parts = path.split('/').filter(|part| !part.is_empty());
    let (Some(owner), Some(repo_segment), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(IngestError::invalid_source(
            "expected https://github.com/{owner}/{repo}",
        ));
    };

    let repo = repo_segment.strip_suffix(".git").unwrap_or(repo_segment);
    Source::github(owner, repo, None)
}

pub(crate) fn tarball_url(owner: &str, repo: &str, reference: Option<&str>) -> String {
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

/// Action outcome from a network / HTTP attempt.
#[derive(Debug)]
pub(crate) enum RequestOutcome {
    HttpStatus(u16),
    TransportError(bool), // true if retryable transport error
}

/// Retry decision derived purely from attempt count and attempt outcome.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RetryDecision {
    Retry(Duration),
    Fail,
}

/// Decides whether to retry an HTTP/transport request based on attempt count and outcome.
pub(crate) fn decide_retry(attempt: usize, outcome: &RequestOutcome) -> RetryDecision {
    if attempt + 1 >= MAX_RETRIES {
        return RetryDecision::Fail;
    }
    let should_retry = match outcome {
        RequestOutcome::HttpStatus(status) => is_retryable_status(*status),
        RequestOutcome::TransportError(retryable) => *retryable,
    };

    if should_retry {
        RetryDecision::Retry(retry_delay(attempt))
    } else {
        RetryDecision::Fail
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

    #[test]
    fn decides_retry_correctly() {
        // Attempt 0 (1st try), 500 status -> Retry initial delay
        assert_eq!(
            decide_retry(0, &RequestOutcome::HttpStatus(500)),
            RetryDecision::Retry(retry_delay(0))
        );
        // Attempt 1 (2nd try), 429 status -> Retry delay 1
        assert_eq!(
            decide_retry(1, &RequestOutcome::HttpStatus(429)),
            RetryDecision::Retry(retry_delay(1))
        );
        // Attempt 2 (3rd try) -> Exceeds MAX_RETRIES (3), Fail
        assert_eq!(
            decide_retry(2, &RequestOutcome::HttpStatus(500)),
            RetryDecision::Fail
        );
        // Non-retryable status (404) -> Fail
        assert_eq!(
            decide_retry(0, &RequestOutcome::HttpStatus(404)),
            RetryDecision::Fail
        );
        // Non-retryable transport error -> Fail
        assert_eq!(
            decide_retry(0, &RequestOutcome::TransportError(false)),
            RetryDecision::Fail
        );
    }
}
