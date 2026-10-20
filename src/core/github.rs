//! Pure GitHub domain rules, URL formatting, origin security policy, and retry decisions.

use crate::core::error::IngestError;
use crate::core::policy::{MAX_ATTEMPTS, RETRY_INITIAL_DELAY_MS, RETRY_MAX_DELAY_MS};
use std::time::Duration;

use memchr::{memchr, memchr2};

pub(crate) const GITHUB_API_BASE_URL: &str = "https://api.github.com";

/// Exact-origin match: returns `true` if `url == GITHUB_API_BASE_URL` or starts with `GITHUB_API_BASE_URL + "/"`.
pub(crate) fn is_github_api_origin(url: &str) -> bool {
    url.strip_prefix(GITHUB_API_BASE_URL)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// Validates whether the given string conforms to valid GitHub owner or repository name rules.
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

/// Parses a GitHub repository URL into an `(owner, repo)` string pair.
pub(crate) fn parse_github_url(value: &str) -> Result<(String, String), IngestError> {
    let remainder = value.strip_prefix("https://").ok_or_else(|| {
        IngestError::unsupported_source("only https://github.com/{owner}/{repo} is supported")
    })?;
    let (authority, path) = match memchr(b'/', remainder.as_bytes()) {
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
    if memchr2(b'?', b'#', path.as_bytes()).is_some() {
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
    if !valid_github_name(owner) || !valid_github_name(repo) {
        return Err(IngestError::invalid_source(
            "owner and repository names contain invalid characters",
        ));
    }
    Ok((owner.to_string(), repo.to_string()))
}

/// Constructs the tarball download URL for the given repository and optional Git reference.
pub(crate) fn tarball_url(owner: &str, repo: &str, reference: Option<&str>) -> String {
    let mut url = String::from(GITHUB_API_BASE_URL);
    for segment in ["repos", owner, repo, "tarball"] {
        url = append_encoded_segment(url, segment);
    }
    if let Some(reference) = reference {
        url = append_encoded_segment(url, reference);
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

/// Returns `true` if HTTP status code is transient/retryable (HTTP 429, 5xx, or rate-limited 403).
pub(crate) const fn is_retryable_http_status(status_code: u16, is_rate_limit: bool) -> bool {
    matches!(status_code, 429 | 500..=599) || (status_code == 403 && is_rate_limit)
}

/// Calculates bounded exponential backoff duration for the given attempt.
pub(crate) const fn retry_delay(attempt: usize) -> Duration {
    let bounded_attempt = if attempt < 16 { attempt } else { 16 };
    let shift = bounded_attempt as u32;
    let exponential = RETRY_INITIAL_DELAY_MS.saturating_mul(1_u64 << shift);
    let millis = if exponential < RETRY_MAX_DELAY_MS {
        exponential
    } else {
        RETRY_MAX_DELAY_MS
    };

    Duration::from_millis(millis)
}

/// Classification of transport-level errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransportKind {
    /// Transient network or timeout errors suitable for retry.
    Retryable,
    /// Permanent failures that should not be retried.
    Permanent,
}

/// Action outcome from a network / HTTP attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestOutcome {
    HttpStatus {
        status: u16,
        retry_after: Option<Duration>,
        is_rate_limit: bool,
    },
    TransportError(TransportKind),
}

/// Transition outcome from an HTTP attempt state transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Transition {
    Retry { attempt: usize, delay: Duration },
    Fail,
}

/// Finite state machine managing HTTP attempt state and retry transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(transparent)]
pub(crate) struct AttemptState {
    attempt: usize,
}

impl AttemptState {
    pub(crate) const fn new() -> Self {
        Self { attempt: 0 }
    }

    /// Advances state given an attempt outcome, returning next transition action.
    pub(crate) const fn step(self, outcome: &RequestOutcome) -> (Self, Transition) {
        if self.attempt + 1 >= MAX_ATTEMPTS {
            return (self, Transition::Fail);
        }
        match outcome {
            RequestOutcome::HttpStatus {
                status,
                retry_after,
                is_rate_limit,
            } => {
                if !is_retryable_http_status(*status, *is_rate_limit) {
                    return (self, Transition::Fail);
                }
                let delay = match retry_after {
                    Some(d) => {
                        if d.as_millis() > RETRY_MAX_DELAY_MS as u128 {
                            return (self, Transition::Fail);
                        }
                        *d
                    }
                    None => retry_delay(self.attempt),
                };
                let next_attempt = self.attempt + 1;
                (
                    Self {
                        attempt: next_attempt,
                    },
                    Transition::Retry {
                        attempt: next_attempt,
                        delay,
                    },
                )
            }
            RequestOutcome::TransportError(kind) => {
                if matches!(kind, TransportKind::Retryable) {
                    let next_attempt = self.attempt + 1;
                    let delay = retry_delay(self.attempt);
                    (
                        Self {
                            attempt: next_attempt,
                        },
                        Transition::Retry {
                            attempt: next_attempt,
                            delay,
                        },
                    )
                } else {
                    (self, Transition::Fail)
                }
            }
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
    fn classifies_retryable_status_codes() {
        for code in [429, 500, 502, 503, 504] {
            assert!(
                is_retryable_http_status(code, false),
                "code {code} should be retryable"
            );
        }
        assert!(
            is_retryable_http_status(403, true),
            "403 with rate limit should be retryable"
        );
        assert!(
            !is_retryable_http_status(403, false),
            "403 without rate limit should not be retryable"
        );
        for code in [200, 400, 401, 404] {
            assert!(
                !is_retryable_http_status(code, false),
                "code {code} should not be retryable"
            );
        }
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
    fn parses_owner_and_repo() {
        let (owner, repo) = parse_github_url("https://github.com/rust-lang/cargo").unwrap();
        assert_eq!(owner, "rust-lang");
        assert_eq!(repo, "cargo");

        let (owner, repo) = parse_github_url("https://github.com/owner/repo.git").unwrap();
        assert_eq!(owner, "owner");
        assert_eq!(repo, "repo");

        assert!(parse_github_url("https://notgithub.com/a/b").is_err());
        assert!(parse_github_url("https://github.com/a/b/c").is_err());
        assert!(parse_github_url("https://github.com/../b").is_err());
    }

    #[test]
    fn decides_retry_correctly() {
        // Attempt 0 (1st try), 500 status -> Retry initial delay
        assert_eq!(
            AttemptState { attempt: 0 }
                .step(&RequestOutcome::HttpStatus {
                    status: 500,
                    retry_after: None,
                    is_rate_limit: false,
                })
                .1,
            Transition::Retry {
                attempt: 1,
                delay: retry_delay(0)
            }
        );
        // Attempt 0, 429 status with Retry-After 2s (< 5s) -> Retry 2s delay
        assert_eq!(
            AttemptState { attempt: 0 }
                .step(&RequestOutcome::HttpStatus {
                    status: 429,
                    retry_after: Some(Duration::from_secs(2)),
                    is_rate_limit: true,
                })
                .1,
            Transition::Retry {
                attempt: 1,
                delay: Duration::from_secs(2)
            }
        );
        // Attempt 0, 429 status with Retry-After 10s (> 5s) -> Fail
        assert_eq!(
            AttemptState { attempt: 0 }
                .step(&RequestOutcome::HttpStatus {
                    status: 429,
                    retry_after: Some(Duration::from_secs(10)),
                    is_rate_limit: true,
                })
                .1,
            Transition::Fail
        );
        // Attempt 0, 403 status with rate limit -> Retry
        assert_eq!(
            AttemptState { attempt: 0 }
                .step(&RequestOutcome::HttpStatus {
                    status: 403,
                    retry_after: None,
                    is_rate_limit: true,
                })
                .1,
            Transition::Retry {
                attempt: 1,
                delay: retry_delay(0)
            }
        );
        // Attempt 0, 403 status without rate limit -> Fail
        assert_eq!(
            AttemptState { attempt: 0 }
                .step(&RequestOutcome::HttpStatus {
                    status: 403,
                    retry_after: None,
                    is_rate_limit: false,
                })
                .1,
            Transition::Fail
        );
        // Attempt 1 (2nd try), 429 status -> Retry delay 1
        assert_eq!(
            AttemptState { attempt: 1 }
                .step(&RequestOutcome::HttpStatus {
                    status: 429,
                    retry_after: None,
                    is_rate_limit: true,
                })
                .1,
            Transition::Retry {
                attempt: 2,
                delay: retry_delay(1)
            }
        );
        // Attempt 2 (3rd try) -> Exceeds MAX_ATTEMPTS (3), Fail
        assert_eq!(
            AttemptState { attempt: 2 }
                .step(&RequestOutcome::HttpStatus {
                    status: 500,
                    retry_after: None,
                    is_rate_limit: false,
                })
                .1,
            Transition::Fail
        );
        // Non-retryable status (404) -> Fail
        assert_eq!(
            AttemptState { attempt: 0 }
                .step(&RequestOutcome::HttpStatus {
                    status: 404,
                    retry_after: None,
                    is_rate_limit: false,
                })
                .1,
            Transition::Fail
        );
        // Retryable transport error -> Retry
        assert_eq!(
            AttemptState { attempt: 0 }
                .step(&RequestOutcome::TransportError(TransportKind::Retryable))
                .1,
            Transition::Retry {
                attempt: 1,
                delay: retry_delay(0)
            }
        );
        // Non-retryable transport error -> Fail
        assert_eq!(
            AttemptState { attempt: 0 }
                .step(&RequestOutcome::TransportError(TransportKind::Permanent))
                .1,
            Transition::Fail
        );
    }
}
