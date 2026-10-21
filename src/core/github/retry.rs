//! Pure retry policy: bounded backoff, rate-limit header interpretation, and the attempt state machine.

use std::time::Duration;

use crate::core::policy::{MAX_ATTEMPTS, RETRY_INITIAL_DELAY_MS, RETRY_MAX_DELAY_MS};

/// Returns `true` if HTTP status code is transient/retryable (HTTP 429, 5xx, or rate-limited 403).
#[must_use]
pub(crate) const fn is_retryable_http_status(status_code: u16, is_rate_limit: bool) -> bool {
    matches!(status_code, 429 | 500..=599) || (status_code == 403 && is_rate_limit)
}

/// Calculates bounded exponential backoff duration for the given attempt.
#[must_use]
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

const _: () = {
    assert!(MAX_ATTEMPTS >= 1);
    assert!(RETRY_INITIAL_DELAY_MS > 0);
    assert!(RETRY_INITIAL_DELAY_MS <= RETRY_MAX_DELAY_MS);
    assert!(retry_delay(0).as_millis() == RETRY_INITIAL_DELAY_MS as u128);
    assert!(retry_delay(usize::MAX).as_millis() == RETRY_MAX_DELAY_MS as u128);
};

/// Rate-limit facts from one response's headers. The shell reads the raw values; this type decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct RateLimitHeaders {
    retry_after_present: bool,
    retry_after_secs: Option<u64>,
    quota_exhausted: bool,
    reset_epoch_secs: Option<u64>,
}

impl RateLimitHeaders {
    /// Parses raw header values, where `None` means the header was absent.
    pub(crate) fn parse(
        retry_after: Option<&str>,
        remaining: Option<&str>,
        reset: Option<&str>,
    ) -> Self {
        Self {
            retry_after_present: retry_after.is_some(),
            retry_after_secs: retry_after.and_then(|v| v.trim().parse().ok()),
            quota_exhausted: remaining.is_some_and(|v| v.trim() == "0"),
            reset_epoch_secs: reset.and_then(|v| v.trim().parse().ok()),
        }
    }

    /// Delay before retrying. `Retry-After` wins; otherwise the reset epoch counts only once quota is exhausted.
    #[must_use]
    pub(crate) fn retry_after(self, now_secs: u64) -> Option<Duration> {
        if let Some(secs) = self.retry_after_secs {
            return Some(Duration::from_secs(secs));
        }
        if !self.quota_exhausted {
            return None;
        }
        self.reset_epoch_secs
            .filter(|&reset| reset > now_secs)
            .map(|reset| Duration::from_secs(reset - now_secs))
    }

    /// Rate-limited when `Retry-After` is present or quota is exhausted.
    #[must_use]
    pub(crate) const fn is_rate_limited(self) -> bool {
        self.retry_after_present || self.quota_exhausted
    }
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
    #[must_use]
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
                match retry_after {
                    Some(d) if d.as_millis() > RETRY_MAX_DELAY_MS as u128 => {
                        (self, Transition::Fail)
                    }
                    Some(d) => self.retry(*d),
                    None => self.retry(retry_delay(self.attempt)),
                }
            }
            RequestOutcome::TransportError(TransportKind::Retryable) => {
                self.retry(retry_delay(self.attempt))
            }
            RequestOutcome::TransportError(TransportKind::Permanent) => (self, Transition::Fail),
        }
    }

    /// Consumes one attempt and schedules the next after `delay`.
    const fn retry(self, delay: Duration) -> (Self, Transition) {
        let attempt = self.attempt + 1;
        (Self { attempt }, Transition::Retry { attempt, delay })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let cases: [Row<'_>; 7] = [
            (None, Some("59"), Some("1002400"), NOW, None),
            (None, Some("0"), Some("1002400"), NOW, Some(2400)),
            (Some("2"), Some("0"), Some("1002400"), NOW, Some(2)),
            (Some("2"), Some("59"), Some("1002400"), NOW, Some(2)),
            (None, None, Some("1002400"), NOW, None),
            (None, Some("0"), Some("999"), NOW, None),
            (None, Some("0"), Some("1002400"), u64::MAX, None),
        ];

        for (retry_after, remaining, reset, now, expected) in cases {
            let got = RateLimitHeaders::parse(retry_after, remaining, reset)
                .retry_after(now)
                .map(|d| d.as_secs());
            assert_eq!(
                got, expected,
                "retry-after={retry_after:?} remaining={remaining:?} reset={reset:?} now={now}"
            );
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
            let got = RateLimitHeaders::parse(retry_after, remaining, None).is_rate_limited();
            assert_eq!(
                got, expected,
                "retry-after={retry_after:?} remaining={remaining:?}"
            );
        }
    }

    #[test]
    fn decides_retry_correctly() {
        let retry = |attempt, delay| Transition::Retry { attempt, delay };
        let cases = [
            // (attempt, outcome, expected)
            (
                0,
                RequestOutcome::HttpStatus {
                    status: 500,
                    retry_after: None,
                    is_rate_limit: false,
                },
                retry(1, retry_delay(0)),
            ),
            (
                0,
                RequestOutcome::HttpStatus {
                    status: 429,
                    retry_after: Some(Duration::from_secs(2)),
                    is_rate_limit: true,
                },
                retry(1, Duration::from_secs(2)),
            ),
            (
                0,
                RequestOutcome::HttpStatus {
                    status: 429,
                    retry_after: Some(Duration::from_secs(10)),
                    is_rate_limit: true,
                },
                Transition::Fail,
            ),
            (
                0,
                RequestOutcome::HttpStatus {
                    status: 403,
                    retry_after: None,
                    is_rate_limit: true,
                },
                retry(1, retry_delay(0)),
            ),
            (
                0,
                RequestOutcome::HttpStatus {
                    status: 403,
                    retry_after: None,
                    is_rate_limit: false,
                },
                Transition::Fail,
            ),
            (
                1,
                RequestOutcome::HttpStatus {
                    status: 429,
                    retry_after: None,
                    is_rate_limit: true,
                },
                retry(2, retry_delay(1)),
            ),
            (
                2,
                RequestOutcome::HttpStatus {
                    status: 500,
                    retry_after: None,
                    is_rate_limit: false,
                },
                Transition::Fail,
            ),
            (
                0,
                RequestOutcome::HttpStatus {
                    status: 404,
                    retry_after: None,
                    is_rate_limit: false,
                },
                Transition::Fail,
            ),
            (
                0,
                RequestOutcome::TransportError(TransportKind::Retryable),
                retry(1, retry_delay(0)),
            ),
            (
                0,
                RequestOutcome::TransportError(TransportKind::Permanent),
                Transition::Fail,
            ),
        ];
        for (attempt, outcome, expected) in cases {
            let (_, got) = AttemptState { attempt }.step(&outcome);
            assert_eq!(got, expected, "attempt={attempt} outcome={outcome:?}");
        }
    }
}
