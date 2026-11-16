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

    const SPEC: &str = include_str!("../../../docs/security_and_filtering.md");

    /// Text between two headings of the specification.
    fn section(from: &str, to: &str) -> &'static str {
        let rest = &SPEC[SPEC.find(from).expect("heading exists")..];
        &rest[..rest.find(to).expect("next heading exists")]
    }

    /// The code spans of `text`, sorted.
    fn code_spans(text: &str) -> Vec<&str> {
        let mut spans: Vec<&str> = text.split('`').skip(1).step_by(2).collect();
        spans.sort_unstable();
        spans
    }

    /// The code spans of the list that follows `label` on its bullet, up to the first `. Matches`.
    fn bullet_list(section: &'static str, label: &str) -> Vec<&'static str> {
        let line = section
            .lines()
            .find(|line| line.starts_with(label))
            .expect("bullet exists");
        let list = line.split_once(":** ").expect("bullet has a list").1;
        code_spans(list.split(". Matches").next().expect("split yields a part"))
    }

    fn sorted(list: &[&'static str]) -> Vec<&'static str> {
        let mut list = list.to_vec();
        list.sort_unstable();
        list
    }

    #[test]
    fn lists_match_security_specification() {
        let mut noise: Vec<&str> = section("### 3.2", "### 3.3")
            .lines()
            .filter(|line| line.starts_with("| `"))
            .flat_map(code_spans)
            .collect();
        noise.sort_unstable();
        assert_eq!(noise, sorted(NOISE), "§ 3.2 noise lockfiles");

        let sensitive = section("### 3.3", "### 3.4");
        assert_eq!(
            bullet_list(sensitive, "- **Prefixes"),
            sorted(SENSITIVE_PREFIXES),
            "§ 3.3 prefixes"
        );
        assert_eq!(
            bullet_list(sensitive, "- **Extensions"),
            sorted(SENSITIVE_EXTENSIONS),
            "§ 3.3 extensions"
        );
    }

    #[test]
    fn is_ignored_filename_applies_each_rule() {
        // Noise names match the whole final segment, case-sensitively.
        for name in NOISE {
            assert!(is_ignored_filename(name), "{name}");
            assert!(!is_ignored_filename(&name.to_ascii_uppercase()), "{name}");
            assert!(!is_ignored_filename(&format!("{name}.orig")), "{name}");
        }
        // Sensitive prefixes match the start of the name, ignoring ASCII case.
        for prefix in SENSITIVE_PREFIXES {
            for name in [
                (*prefix).to_string(),
                prefix.to_ascii_uppercase(),
                format!("{prefix}.pub"),
            ] {
                assert!(is_ignored_filename(&name), "{name}");
            }
            assert!(!is_ignored_filename(&format!("x{prefix}")), "x{prefix}");
        }
        // Sensitive extensions match after the final dot, ignoring ASCII case.
        for ext in SENSITIVE_EXTENSIONS {
            for name in [
                format!("x.{ext}"),
                format!("X.{}", ext.to_ascii_uppercase()),
            ] {
                assert!(is_ignored_filename(&name), "{name}");
            }
            assert!(!is_ignored_filename(&format!("x.{ext}s")), "x.{ext}s");
        }
        for name in ["Cargo.toml", "main.rs", "Makefile", "environment.rs"] {
            assert!(!is_ignored_filename(name), "{name}");
        }
    }

    #[test]
    fn is_hidden_segment_is_leading_dot() {
        assert!(is_hidden_segment(b".git"));
        assert!(is_hidden_segment(b".env"));
        assert!(is_hidden_segment(b"."));
        assert!(is_hidden_segment(b".."));
        assert!(!is_hidden_segment(b"src"));
        assert!(!is_hidden_segment(b""));
        assert!(!is_hidden_segment(b"file.txt"));
    }
}
