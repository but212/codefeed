//! Exclusion lists for noise files and sensitive credential patterns.

use memchr::memrchr;

/// Conventional lockfiles excluded before candidate budget accounting.
const NOISE: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lock",
    "bun.lockb",
    "go.sum",
    "poetry.lock",
    "Pipfile.lock",
    "composer.lock",
    "Gemfile.lock",
];

/// Known sensitive file extensions (case-insensitive) excluded prior to reading or download.
///
/// Note: `credentials` is matched as an extension here (`*.credentials`, e.g., `aws.credentials`),
/// and is also matched as a filename prefix in [`SENSITIVE_PREFIXES`] (`credentials*`, e.g., `credentials.json`).
const SENSITIVE_EXTENSIONS: &[&str] = &["pem", "key", "p12", "pkcs12", "pfx", "credentials"];

/// Known sensitive filename prefixes (case-insensitive) excluded prior to reading or download.
const SENSITIVE_PREFIXES: &[&str] = &["id_rsa", "id_ed25519", "id_ecdsa", "id_dsa", "credentials"];

fn starts_with_ignore_ascii_case(value: &str, prefix: &str) -> bool {
    value
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

/// Returns `true` if `file_name` indicates a sensitive credentials file.
fn is_sensitive_file(file_name: &str) -> bool {
    if SENSITIVE_PREFIXES
        .iter()
        .any(|&prefix| starts_with_ignore_ascii_case(file_name, prefix))
    {
        return true;
    }
    let Some(dot_idx) = memrchr(b'.', file_name.as_bytes()) else {
        return false;
    };
    let extension = &file_name[dot_idx + 1..];
    SENSITIVE_EXTENSIONS
        .iter()
        .any(|&s| s.eq_ignore_ascii_case(extension))
}

/// Returns `true` if raw path segment bytes start with a dot (`.`), marking a hidden file or directory.
#[must_use]
pub(crate) const fn is_hidden_segment(segment: &[u8]) -> bool {
    matches!(segment, [b'.', ..])
}

/// Returns `true` if `file_name` matches a known noise file or a sensitive credentials pattern.
#[must_use]
pub(crate) fn is_ignored_filename(file_name: &str) -> bool {
    NOISE.contains(&file_name) || is_sensitive_file(file_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_hidden_segment() {
        assert!(is_hidden_segment(b".git"));
        assert!(is_hidden_segment(b".env"));
        assert!(is_hidden_segment(b"."));
        assert!(is_hidden_segment(b".."));
        assert!(!is_hidden_segment(b"src"));
        assert!(!is_hidden_segment(b""));
        assert!(!is_hidden_segment(b"file.txt"));
    }

    #[test]
    fn test_is_ignored_filename() {
        let ignored = [
            "Cargo.lock",
            "package-lock.json",
            "yarn.lock",
            "pnpm-lock.yaml",
            "go.sum",
            "poetry.lock",
            "Pipfile.lock",
            "composer.lock",
            "Gemfile.lock",
            "bun.lock",
            "bun.lockb",
            "server.key",
            "cert.pem",
            "client.p12",
            "cert.pkcs12",
            "identity.pfx",
            "credentials.json",
            "aws.credentials",
            "credentials",
            "id_rsa",
            "id_rsa.pub",
            "id_ed25519",
            "id_ed25519.pub",
            "id_ecdsa",
            "id_ecdsa.pub",
            "id_dsa",
            "id_dsa.pub",
        ];
        for filename in ignored {
            assert!(
                is_ignored_filename(filename),
                "expected {filename} to be ignored"
            );
        }

        let kept = ["Cargo.toml", "main.rs", "Makefile", "environment.rs"];
        for filename in kept {
            assert!(
                !is_ignored_filename(filename),
                "expected {filename} not to be ignored"
            );
        }
    }
}
