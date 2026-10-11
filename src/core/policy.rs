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
    if file_name.starts_with(".env")
        || file_name.starts_with("id_rsa")
        || file_name.starts_with("id_ed25519")
        || file_name.starts_with("credentials")
    {
        return true;
    }
    let Some((_, ext)) = file_name.rsplit_once('.') else {
        return false;
    };
    SENSITIVE_EXT.iter().any(|&s| s.eq_ignore_ascii_case(ext))
}

/// Returns `true` if `path` has a known binary file extension (case-insensitive).
pub(crate) fn has_binary_extension(path: &str) -> bool {
    let Some((_, ext)) = path.rsplit_once('.') else {
        return false;
    };
    BINARY_EXT.iter().any(|&b| b.eq_ignore_ascii_case(ext))
}

/// Returns `true` if `path` contains hidden directory or file components (starting with `.`).
pub(crate) fn is_hidden_path(path: &str) -> bool {
    path.split('/')
        .any(|part| part.starts_with('.') || part.is_empty())
}

/// Pre-ingestion classification of a relative path candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathClassification {
    /// Noise, sensitive, or hidden: ignored completely (neither candidates nor skipped).
    Ignored,
    /// Known binary extension: candidate accounted, but body read/fetch is skipped (Probe::Missing).
    PreclassifiedBinary,
    /// Normal candidate: requires reading or downloading content to evaluate encoding and size.
    NeedsContent,
}

impl PathClassification {
    pub(crate) fn classify(path: &str) -> Self {
        if is_hidden_path(path) || is_noise_path(path) || is_sensitive_path(path) {
            Self::Ignored
        } else if has_binary_extension(path) {
            Self::PreclassifiedBinary
        } else {
            Self::NeedsContent
        }
    }
}

/// Returns `true` if HTTP status code is transient/retryable (HTTP 429 or 5xx).
pub(crate) fn is_retryable_status(status_code: u16) -> bool {
    matches!(status_code, 429 | 500..=599)
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
        assert!(is_sensitive_path(".ssh/id_rsa_deploy"));
        assert!(is_sensitive_path("id_ed25519"));
        assert!(is_sensitive_path(".ssh/id_ed25519_backup"));
        assert!(is_sensitive_path("secrets/credentials.json"));
        assert!(is_sensitive_path("auth/aws.credentials"));
        assert!(is_sensitive_path("aws/credentials"));
        assert!(is_sensitive_path("config/credentials.yaml"));
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

    #[test]
    fn classifies_candidate_paths() {
        assert_eq!(
            PathClassification::classify(".git/config"),
            PathClassification::Ignored
        );
        assert_eq!(
            PathClassification::classify("src/.hidden.rs"),
            PathClassification::Ignored
        );
        assert_eq!(
            PathClassification::classify("Cargo.lock"),
            PathClassification::Ignored
        );
        assert_eq!(
            PathClassification::classify("id_rsa"),
            PathClassification::Ignored
        );
        assert_eq!(
            PathClassification::classify("assets/logo.png"),
            PathClassification::PreclassifiedBinary
        );
        assert_eq!(
            PathClassification::classify("src/main.rs"),
            PathClassification::NeedsContent
        );
    }
}
