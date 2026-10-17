//! Single source of truth for ingestion policies, limits, and conventions.

/// Maximum individual file size accepted (1 MiB in bytes). Larger files are skipped.
pub(crate) const MAX_FILE_SIZE: u64 = 1024 * 1024;
/// Maximum rendered digest size (20 MiB in bytes). Overflows return [`IngestError::OutputTooLarge`](crate::core::error::IngestError::OutputTooLarge).
pub(crate) const MAX_OUTPUT_SIZE: u64 = 20 * 1024 * 1024;
/// Maximum candidate files evaluated before candidate budget overflows (10,000 files).
pub(crate) const MAX_FILE_COUNT: usize = 10_000;
/// Maximum duration in seconds allowed for whole-archive Tarball transfer (5 minutes).
pub(crate) const ARCHIVE_MAX_SECS: u64 = 300;

/// Conventional lockfiles excluded before candidate budget accounting.
const NOISE: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "go.sum",
];

/// Known sensitive file extensions (case-insensitive) excluded prior to reading or download.
///
/// Note: `credentials` is matched as an extension here (`*.credentials`, e.g., `aws.credentials`),
/// and is also matched as a filename prefix in [`is_sensitive_file`] (`credentials*`, e.g., `credentials.json`).
const SENSITIVE_EXTENSIONS: &[&str] = &["pem", "key", "p12", "pkcs12", "credentials"];

/// Maximum total HTTP request attempts (1 initial + up to 2 retries).
pub(crate) const MAX_RETRIES: usize = 3;
/// Base initial delay in milliseconds for exponential backoff.
const RETRY_INITIAL_DELAY_MS: u64 = 250;
/// Maximum delay cap in milliseconds for exponential backoff.
const RETRY_MAX_DELAY_MS: u64 = 5_000;

/// Returns `true` if `file_name` matches a known noise file.
#[inline]
pub(crate) fn is_noise_file(file_name: &str) -> bool {
    NOISE.contains(&file_name)
}

fn starts_with_ignore_ascii_case(value: &str, prefix: &str) -> bool {
    value
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

/// Returns `true` if `file_name` or its `extension` indicates a sensitive credentials file.
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

/// Returns `true` if `path` contains hidden directory or file components (starting with `.`).
#[inline]
pub(crate) fn is_hidden_path(path: &str) -> bool {
    path.split('/').any(|s| s.starts_with('.'))
}

/// Returns `true` if `path` is an ignored noise, sensitive, or hidden file.
pub(crate) fn is_ignored_path(path: &str) -> bool {
    if is_hidden_path(path) {
        return true;
    }

    let file_name = path.rsplit('/').next().unwrap_or(path);
    let extension = file_name.rsplit_once('.').map(|(_, e)| e);

    is_noise_file(file_name) || is_sensitive_file(file_name, extension)
}

/// Returns `true` if HTTP status code is transient/retryable (HTTP 429 or 5xx).
#[inline]
pub(crate) const fn is_retryable_status(status_code: u16) -> bool {
    matches!(status_code, 429 | 500..=599)
}

/// Calculates bounded exponential backoff duration for the given attempt.
#[inline]
pub(crate) const fn retry_delay(attempt: usize) -> std::time::Duration {
    let shift = if attempt < 5 { attempt as u32 } else { 5 };
    let exponential = RETRY_INITIAL_DELAY_MS.saturating_mul(1_u64 << shift);
    let millis = if exponential < RETRY_MAX_DELAY_MS {
        exponential
    } else {
        RETRY_MAX_DELAY_MS
    };

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
            assert!(retry_delay(0).as_millis() == 250);
            assert!(retry_delay(1).as_millis() == 500);
            assert!(retry_delay(2).as_millis() == 1000);
            assert!(retry_delay(5).as_millis() == 5000);
            assert!(retry_delay(10).as_millis() == 5000);
        }
        assert_eq!(retry_delay(0).as_millis(), 250);
        assert_eq!(retry_delay(1).as_millis(), 500);
        assert_eq!(retry_delay(2).as_millis(), 1000);
        assert_eq!(retry_delay(3).as_millis(), 2000);
        assert_eq!(retry_delay(4).as_millis(), 4000);
        assert_eq!(retry_delay(5).as_millis(), 5000);
        assert_eq!(retry_delay(10).as_millis(), 5000);
    }

    #[test]
    fn test_is_ignored_path() {
        let ignored = [
            "Cargo.lock",
            "nested/dir/package-lock.json",
            ".env",
            "config/.env.local",
            "certs/server.pem",
            "certs/privkey.key",
            "id_rsa",
            ".ssh/id_rsa_deploy",
            "id_ed25519",
            ".ssh/id_ed25519_backup",
            "secrets/credentials.json",
            "auth/aws.credentials",
            "aws/credentials",
            "config/credentials.yaml",
            "Credentials.json",
            "keys/ID_RSA",
            "ID_ED25519_backup",
            "config/.ENV.prod",
            ".git/config",
            "src/.hidden.rs",
            ".env.production",
            ".envrc",
        ];
        for path in ignored {
            assert!(is_ignored_path(path), "expected {path} to be ignored");
        }

        let kept = [
            "assets/logo.png",
            "image.PNG",
            "assets/font.woff2",
            "src/main.rs",
            "src//main.rs",
            "/src/main.rs",
            "Cargo.toml",
            "Makefile",
            "environment.rs",
        ];
        for path in kept {
            assert!(!is_ignored_path(path), "expected {path} not to be ignored");
        }
    }

    #[test]
    fn test_is_hidden_path() {
        let cases = [
            (".hidden", true),
            ("dir/.hidden", true),
            ("a/b/.c/d", true),
            ("not_hidden", false),
            ("dir/not_hidden.txt", false),
            ("ends_with_dot.", false),
            ("a/b", false),
        ];
        for (path, expected) in cases {
            assert_eq!(is_hidden_path(path), expected, "path: {path}");
        }
    }
}
