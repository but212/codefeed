//! Single source of truth for ingestion policies, limits, and conventions.

/// Maximum individual file size accepted (1 MiB in bytes). Larger files are skipped.
pub const MAX_FILE_SIZE: u64 = 1024 * 1024;
/// Maximum rendered digest size (20 MiB in bytes). Overflows return [`IngestError::OutputTooLarge`](crate::core::error::IngestError::OutputTooLarge).
pub const MAX_OUTPUT_SIZE: u64 = 20 * 1024 * 1024;
/// Maximum candidate files evaluated before candidate budget overflows (10,000 files).
pub const MAX_FILE_COUNT: usize = 10_000;
/// Maximum duration in seconds allowed for whole-archive Tarball transfer (5 minutes).
pub const ARCHIVE_MAX_SECS: u64 = 300;

/// Conventional lockfiles excluded before candidate budget accounting.
pub const NOISE: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "go.sum",
];

/// Known sensitive file extensions (case-insensitive) excluded prior to reading or download.
///
/// Note: `credentials` is matched as an extension here (`*.credentials`, e.g., `aws.credentials`),
/// and is also matched as a filename prefix in [`is_sensitive_path`] (`credentials*`, e.g., `credentials.json`).
pub const SENSITIVE_EXTENSIONS: &[&str] = &["pem", "key", "p12", "pkcs12", "credentials"];

/// Known binary file extensions filtered prior to remote download.
pub const BINARY_EXTENSIONS: &[&str] = &[
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

#[inline]
pub(crate) fn is_noise_file(file_name: &str) -> bool {
    NOISE.contains(&file_name)
}

fn starts_with_ignore_ascii_case(value: &str, prefix: &str) -> bool {
    value
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

#[inline]
pub(crate) fn is_sensitive_file(file_name: &str, extension: Option<&str>) -> bool {
    if starts_with_ignore_ascii_case(file_name, ".env")
        || starts_with_ignore_ascii_case(file_name, "id_rsa")
        || starts_with_ignore_ascii_case(file_name, "id_ed25519")
        || starts_with_ignore_ascii_case(file_name, "credentials")
    {
        return true;
    }
    let Some(extension) = extension else {
        return false;
    };
    SENSITIVE_EXTENSIONS
        .iter()
        .any(|&s| s.eq_ignore_ascii_case(extension))
}

#[inline]
pub(crate) fn is_binary_extension(extension: Option<&str>) -> bool {
    let Some(extension) = extension else {
        return false;
    };
    BINARY_EXTENSIONS
        .iter()
        .any(|&b| b.eq_ignore_ascii_case(extension))
}

/// Returns `true` if `path` contains hidden directory or file components (starting with `.`).
#[inline]
pub(crate) fn is_hidden_path(path: &str) -> bool {
    path.starts_with('.') || path.contains("/.")
}

/// Returns `true` if raw byte path contains hidden directory or file components.
#[inline]
pub(crate) fn is_hidden_bytes(path: &[u8]) -> bool {
    path.starts_with(b".") || path.windows(2).any(|w| w == b"/.")
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
        if is_hidden_path(path) {
            return Self::Ignored;
        }

        let file_name = path.rsplit('/').next().unwrap_or(path);
        let extension = file_name.rsplit_once('.').map(|(_, e)| e);

        if is_noise_file(file_name) || is_sensitive_file(file_name, extension) {
            Self::Ignored
        } else if is_binary_extension(extension) {
            Self::PreclassifiedBinary
        } else {
            Self::NeedsContent
        }
    }
}

/// Returns `true` if HTTP status code is transient/retryable (HTTP 429 or 5xx).
#[inline]
pub(crate) const fn is_retryable_status(status_code: u16) -> bool {
    (status_code == 429) | (status_code.wrapping_sub(500) <= 99)
}

/// Calculates bounded exponential backoff duration with jitter for the given attempt.
#[inline]
pub(crate) const fn retry_delay(attempt: usize, entropy: u64) -> std::time::Duration {
    let shift = if attempt < 5 { attempt as u32 } else { 5 };
    let exponential = RETRY_INITIAL_DELAY_MS.saturating_mul(1_u64 << shift);
    let capped_exponential = if exponential < RETRY_MAX_DELAY_MS - RETRY_JITTER_MAX_MS {
        exponential
    } else {
        RETRY_MAX_DELAY_MS - RETRY_JITTER_MAX_MS
    };
    let jitter = entropy % (RETRY_JITTER_MAX_MS + 1);
    let millis = capped_exponential + jitter;

    std::time::Duration::from_millis(millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_retryable_status_codes() {
        const {
            assert!(is_retryable_status(429));
            assert!(is_retryable_status(500));
            assert!(!is_retryable_status(200));
        }
        for code in [429, 500, 502, 503, 504] {
            assert!(is_retryable_status(code), "code {code} should be retryable");
        }
        for code in [200, 400, 401, 403, 404] {
            assert!(
                !is_retryable_status(code),
                "code {code} should not be retryable"
            );
        }
    }

    #[test]
    fn calculates_bounded_exponential_backoff() {
        const {
            assert!(retry_delay(0, 0).as_millis() == 250);
            assert!(retry_delay(10, 0).as_millis() == 4950);
            assert!(retry_delay(10, 50).as_millis() == 5000);
        }
        assert_eq!(retry_delay(0, 0).as_millis(), 250);
        assert_eq!(retry_delay(1, 0).as_millis(), 500);
        assert_eq!(retry_delay(2, 0).as_millis(), 1000);
        assert_eq!(retry_delay(10, 0).as_millis(), 4950);
        assert_eq!(retry_delay(10, 25).as_millis(), 4975);
        assert_eq!(retry_delay(10, 50).as_millis(), 5000);

        assert_eq!(retry_delay(0, 42).as_millis(), 292);
        assert_eq!(retry_delay(0, 50).as_millis(), 300);
    }

    #[test]
    fn classifies_candidate_paths() {
        let cases = [
            ("Cargo.lock", PathClassification::Ignored),
            ("nested/dir/package-lock.json", PathClassification::Ignored),
            (".env", PathClassification::Ignored),
            ("config/.env.local", PathClassification::Ignored),
            ("certs/server.pem", PathClassification::Ignored),
            ("certs/privkey.key", PathClassification::Ignored),
            ("id_rsa", PathClassification::Ignored),
            (".ssh/id_rsa_deploy", PathClassification::Ignored),
            ("id_ed25519", PathClassification::Ignored),
            (".ssh/id_ed25519_backup", PathClassification::Ignored),
            ("secrets/credentials.json", PathClassification::Ignored),
            ("auth/aws.credentials", PathClassification::Ignored),
            ("aws/credentials", PathClassification::Ignored),
            ("config/credentials.yaml", PathClassification::Ignored),
            ("Credentials.json", PathClassification::Ignored),
            ("keys/ID_RSA", PathClassification::Ignored),
            ("ID_ED25519_backup", PathClassification::Ignored),
            ("config/.ENV.prod", PathClassification::Ignored),
            (".git/config", PathClassification::Ignored),
            ("src/.hidden.rs", PathClassification::Ignored),
            ("assets/logo.png", PathClassification::PreclassifiedBinary),
            ("image.PNG", PathClassification::PreclassifiedBinary),
            ("assets/font.woff2", PathClassification::PreclassifiedBinary),
            ("src/main.rs", PathClassification::NeedsContent),
            ("src//main.rs", PathClassification::NeedsContent),
            ("/src/main.rs", PathClassification::NeedsContent),
            ("Cargo.toml", PathClassification::NeedsContent),
            ("Makefile", PathClassification::NeedsContent),
        ];
        for (path, expected) in cases {
            assert_eq!(
                PathClassification::classify(path),
                expected,
                "classification mismatch for {path}"
            );
        }
    }

    #[test]
    fn is_sensitive_path_matches_env_prefixes() {
        assert_eq!(
            PathClassification::classify(".env"),
            PathClassification::Ignored
        );
        assert_eq!(
            PathClassification::classify(".env.local"),
            PathClassification::Ignored
        );
        assert_eq!(
            PathClassification::classify("nested/.ENV.production"),
            PathClassification::Ignored
        );
        assert_eq!(
            PathClassification::classify(".envrc"),
            PathClassification::Ignored
        );
        assert_eq!(
            PathClassification::classify("environment.rs"),
            PathClassification::NeedsContent
        );
    }

    #[test]
    fn test_is_hidden_bytes_matches_is_hidden_path() {
        let cases = [
            (".hidden", true),
            ("dir/.hidden", true),
            ("a/b/.c/d", true),
            ("not_hidden", false),
            ("dir/not_hidden.txt", false),
            ("ends_with_dot.", false),
        ];
        for (path, expected) in cases {
            assert_eq!(is_hidden_path(path), expected);
            assert_eq!(is_hidden_bytes(path.as_bytes()), expected);
        }
    }
}
