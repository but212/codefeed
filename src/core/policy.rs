//! Single source of truth for ingestion policies, limits, and conventions.

pub const MAX_FILE_SIZE: u64 = 1024 * 1024;
pub const MAX_OUTPUT_SIZE: u64 = 20 * 1024 * 1024;
pub const MAX_FILE_COUNT: usize = 10_000;
pub const MAX_JSON_SIZE: u64 = 8 * 1024 * 1024;
pub const FETCH_WINDOW: usize = 16;

pub const NOISE: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "go.sum",
];

pub const BINARY_EXT: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "ico", "webp", "woff", "woff2", "ttf", "pdf", "zip",
];

pub const MAX_RETRIES: usize = 3;
pub const RETRY_INITIAL_DELAY_MS: u64 = 250;
pub const RETRY_MAX_DELAY_MS: u64 = 5_000;
pub const RETRY_JITTER_MAX_MS: u64 = 50;

pub(crate) fn is_noise_path(path: &str) -> bool {
    let file_name = path.rsplit('/').next().unwrap_or(path);
    NOISE.contains(&file_name)
}

pub(crate) fn has_binary_extension(path: &str) -> bool {
    if let Some((_, ext)) = path.rsplit_once('.') {
        BINARY_EXT.iter().any(|&b| b.eq_ignore_ascii_case(ext))
    } else {
        false
    }
}

pub(crate) fn is_retryable_status(status_code: u16) -> bool {
    status_code == 429 || (500..=599).contains(&status_code)
}

pub(crate) fn retry_delay(attempt: usize, entropy: u64) -> std::time::Duration {
    let multiplier = 1_u64.checked_shl(attempt as u32).unwrap_or(u64::MAX);
    let exponential = RETRY_INITIAL_DELAY_MS.saturating_mul(multiplier);
    let jitter = entropy % (RETRY_JITTER_MAX_MS + 1);
    let base = exponential.saturating_add(jitter);
    std::time::Duration::from_millis(base.min(RETRY_MAX_DELAY_MS))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_noise_filenames() {
        assert!(is_noise_path("Cargo.lock"));
        assert!(is_noise_path("nested/dir/package-lock.json"));
        assert!(!is_noise_path("Cargo.toml"));
    }

    #[test]
    fn classifies_binary_extensions() {
        assert!(has_binary_extension("image.PNG"));
        assert!(has_binary_extension("assets/font.woff2"));
        assert!(!has_binary_extension("src/main.rs"));
        assert!(!has_binary_extension("Makefile"));
    }

    #[test]
    fn classifies_retryable_status_codes() {
        assert!(is_retryable_status(429));
        assert!(is_retryable_status(500));
        assert!(is_retryable_status(502));
        assert!(is_retryable_status(503));
        assert!(is_retryable_status(504));
        assert!(!is_retryable_status(200));
        assert!(!is_retryable_status(400));
        assert!(!is_retryable_status(401));
        assert!(!is_retryable_status(403));
        assert!(!is_retryable_status(404));
    }

    #[test]
    fn calculates_bounded_exponential_backoff() {
        let d0 = retry_delay(0, 0);
        let d1 = retry_delay(1, 0);
        let d2 = retry_delay(2, 0);
        let d10 = retry_delay(10, 0);

        assert_eq!(d0.as_millis(), 250);
        assert_eq!(d1.as_millis(), 500);
        assert_eq!(d2.as_millis(), 1000);
        assert_eq!(d10.as_millis(), 5000);

        let d_jitter = retry_delay(0, 42);
        assert_eq!(d_jitter.as_millis(), 292);
        let d_jitter_max = retry_delay(0, 50);
        assert_eq!(d_jitter_max.as_millis(), 300);

        let d10_jitter = retry_delay(10, 50);
        assert_eq!(d10_jitter.as_millis(), 5000);
    }
}
