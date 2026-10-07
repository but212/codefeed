//! Single source of truth for ingestion policies, limits, and conventions.

/// Maximum individual file size accepted (1 MiB in bytes). Larger files are skipped.
pub const MAX_FILE_SIZE: u64 = 1024 * 1024;
/// Maximum rendered digest size (20 MiB in bytes). Overflows return [`IngestError::OutputTooLarge`](crate::core::error::IngestError::OutputTooLarge).
pub const MAX_OUTPUT_SIZE: u64 = 20 * 1024 * 1024;
/// Maximum candidate files evaluated before candidate budget overflows (10,000 files).
pub const MAX_FILE_COUNT: usize = 10_000;
/// Maximum GitHub API JSON response body size (8 MiB in bytes).
pub const MAX_JSON_SIZE: u64 = 8 * 1024 * 1024;
/// Number of concurrent remote download worker threads.
pub const FETCH_WINDOW: usize = 16;

/// Conventional lockfiles excluded before candidate budget accounting.
pub const NOISE: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "go.sum",
];

/// Known sensitive file extensions (case-insensitive) excluded prior to reading or download.
pub const SENSITIVE_EXT: &[&str] = &["pem", "key", "p12", "pkcs12", "credentials"];

/// Known sensitive exact file names excluded prior to reading or download.
pub const SENSITIVE_EXACT: &[&str] = &["id_rsa", "id_ed25519", "credentials.json"];

/// Known binary file extensions filtered prior to remote download.
pub const BINARY_EXT: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "ico", "webp", "woff", "woff2", "ttf", "pdf", "zip",
];

/// Maximum total HTTP request attempts (1 initial + up to 2 retries).
pub const MAX_RETRIES: usize = 3;
/// Base initial delay in milliseconds for exponential backoff.
pub const RETRY_INITIAL_DELAY_MS: u64 = 250;
/// Maximum delay cap in milliseconds for exponential backoff.
pub const RETRY_MAX_DELAY_MS: u64 = 5_000;
/// Maximum random jitter in milliseconds added to backoff delay.
pub const RETRY_JITTER_MAX_MS: u64 = 50;

/// Returns `true` if `path` matches any conventional noise lockfile.
pub(crate) fn is_noise_path(path: &str) -> bool {
    let file_name = path.rsplit('/').next().unwrap_or(path);
    NOISE.contains(&file_name)
}

/// Returns `true` if `path` matches conventional sensitive files (.env, keys, credentials).
pub(crate) fn is_sensitive_path(path: &str) -> bool {
    let file_name = path.rsplit('/').next().unwrap_or(path);
    if file_name.starts_with(".env") {
        return true;
    }
    if SENSITIVE_EXACT.contains(&file_name) {
        return true;
    }
    if let Some((_, ext)) = file_name.rsplit_once('.') {
        SENSITIVE_EXT.iter().any(|&s| s.eq_ignore_ascii_case(ext))
    } else {
        false
    }
}

/// Returns `true` if `path` has a known binary file extension (case-insensitive).
pub(crate) fn has_binary_extension(path: &str) -> bool {
    if let Some((_, ext)) = path.rsplit_once('.') {
        BINARY_EXT.iter().any(|&b| b.eq_ignore_ascii_case(ext))
    } else {
        false
    }
}

/// Returns `true` if HTTP status code is transient/retryable (HTTP 429 or 5xx).
pub(crate) fn is_retryable_status(status_code: u16) -> bool {
    status_code == 429 || (500..=599).contains(&status_code)
}

/// Calculates bounded exponential backoff duration with jitter for the given attempt.
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
    fn classifies_sensitive_paths() {
        assert!(is_sensitive_path(".env"));
        assert!(is_sensitive_path("config/.env.local"));
        assert!(is_sensitive_path("certs/server.pem"));
        assert!(is_sensitive_path("certs/privkey.key"));
        assert!(is_sensitive_path("id_rsa"));
        assert!(is_sensitive_path(".ssh/id_ed25519"));
        assert!(is_sensitive_path("secrets/credentials.json"));
        assert!(is_sensitive_path("auth/aws.credentials"));
        assert!(!is_sensitive_path("src/main.rs"));
        assert!(!is_sensitive_path("Cargo.toml"));
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
