use crate::core::error::DomainError;
use crate::core::path::{CandidatePath, DepthExceededError, classify_path_segments};
use crate::core::policy::ExcludePatterns;
use memchr::{memchr, memmem};

/// Normalizes a raw tar entry path, stripping the root directory prefix,
/// collapsing multiple slashes, removing `.` segments, and classifying against policy.
///
/// User exclusions match the path after the root directory is stripped, so their patterns are relative to the source root.
///
/// Returns `Err(DomainError)` if the path contains directory traversal, invalid root prefixes, or NUL bytes.
/// Returns `Ok(None)` if the entry is the root directory itself, empty, hidden, excluded, noise, or sensitive.
/// Returns `Ok(Some(CandidatePath::Unstorable(prefix)))` if the entry path contains non-UTF-8 bytes or exceeds `MAX_PATH_BYTES`, and is not hidden.
/// Returns `Ok(Some(CandidatePath::Valid(path)))` on success.
pub(crate) fn normalize_tar_entry_path(
    raw_bytes: &[u8],
    exclude: &ExcludePatterns,
) -> Result<Option<CandidatePath>, DomainError> {
    if memchr(0, raw_bytes).is_some() {
        return Err(DomainError::invalid_metadata(
            "Tar entry path contains NUL byte",
        ));
    }
    let start = raw_bytes
        .iter()
        .position(|&b| b != b'/')
        .unwrap_or(raw_bytes.len());
    let end = raw_bytes
        .iter()
        .rposition(|&b| b != b'/')
        .map_or(start, |i| i + 1);
    let trimmed = &raw_bytes[start..end];

    if memmem::find(trimmed, b"..").is_some()
        && trimmed.split(|&b| b == b'/').any(|seg| seg == b"..")
    {
        return Err(DomainError::invalid_metadata(
            "Tar entry contains directory traversal (..)",
        ));
    }

    let Some(slash_index) = memchr(b'/', trimmed) else {
        return Ok(None);
    };

    if trimmed[..slash_index] == *b"." {
        return Err(DomainError::invalid_metadata(
            "Tar entry root segment cannot be '.'",
        ));
    }

    let rel = &trimmed[slash_index + 1..];

    classify_path_segments(rel.split(|&b| b == b'/'), exclude)
        .map_err(|_| DepthExceededError::domain_error("Tar entry path"))
}

/// Parses a 40- or 64-hex-character commit SHA from a `comment=<sha>` PAX record.
#[must_use]
pub(crate) fn parse_pax_comment(bytes: &[u8]) -> Option<String> {
    memmem::find(bytes, b"comment=")?;
    let text = std::str::from_utf8(bytes).ok()?;
    text.lines().find_map(|line| {
        let trimmed = line.trim_ascii();
        let payload = match memchr(b' ', trimmed.as_bytes()) {
            Some(space_index) if trimmed[..space_index].chars().all(|c| c.is_ascii_digit()) => {
                &trimmed[space_index + 1..]
            }
            _ => trimmed,
        };
        payload
            .strip_prefix("comment=")
            .map(str::trim_ascii)
            .filter(|sha| is_valid_commit_sha(sha))
            .map(ToOwned::to_owned)
    })
}

#[must_use]
fn is_valid_commit_sha(sha: &str) -> bool {
    matches!(sha.len(), 40 | 64) && sha.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::policy::MAX_PATH_DEPTH;

    fn normalize(raw: &[u8]) -> Result<Option<CandidatePath>, DomainError> {
        normalize_tar_entry_path(raw, &ExcludePatterns::default())
    }

    #[test]
    fn user_exclusions_are_relative_to_the_archive_root() {
        let exclude = ExcludePatterns::new(["build/", "/src/", "repo-root"]).unwrap();
        let cases: [(&[u8], Option<CandidatePath>); 4] = [
            (b"repo-root/build/x.txt", None),
            (b"repo-root/src/main.rs", None),
            (
                b"repo-root/lib/src/x.rs",
                Some(CandidatePath::Valid("lib/src/x.rs".into())),
            ),
            // The root directory itself is never matched, so the `repo-root` pattern keeps nothing out.
            (
                b"repo-root/lib/main.rs",
                Some(CandidatePath::Valid("lib/main.rs".into())),
            ),
        ];
        for (raw, expected) in cases {
            assert_eq!(
                normalize_tar_entry_path(raw, &exclude).unwrap(),
                expected,
                "{raw:?}"
            );
        }
    }

    #[test]
    fn trailing_separators_never_change_the_classification() {
        let cases: [(&[u8], Option<CandidatePath>); 6] = [
            (
                b"repo-root/src/a.rs/",
                Some(CandidatePath::Valid("src/a.rs".into())),
            ),
            (
                b"repo-root/src/a.rs///",
                Some(CandidatePath::Valid("src/a.rs".into())),
            ),
            (b"repo-root/", None),
            (b"repo-root//", None),
            (b"///", None),
            (b"./", None),
        ];
        for (raw, expected) in cases {
            assert_eq!(normalize(raw).unwrap(), expected, "{raw:?}");
        }
    }

    #[test]
    fn normalizes_archive_entry_path_correctly() {
        // (raw entry path, expected status). Covers root stripping, slash and `.` collapsing,
        // policy exclusion (noise lockfile, sensitive credential), and non-UTF-8 prefixes.
        let ok_cases: [(&[u8], Option<CandidatePath>); 10] = [
            (
                b"but212-codefeed-c1863c8/src/main.rs",
                Some(CandidatePath::Valid("src/main.rs".into())),
            ),
            (
                b"repo-root//src/main.rs",
                Some(CandidatePath::Valid("src/main.rs".into())),
            ),
            (
                b"repo-root///deep//nested/./file.txt",
                Some(CandidatePath::Valid("deep/nested/file.txt".into())),
            ),
            (
                b"repo-root/././a.rs",
                Some(CandidatePath::Valid("a.rs".into())),
            ),
            (b"but212-codefeed-c1863c8/", None),
            (b"but212-codefeed-c1863c8///", None),
            (b"but212-codefeed-c1863c8", None),
            (b"", None),
            // Classification itself is pinned by `path::classification_corpus_is_stable`; one
            // ignored and one unstorable row show that the archive delegates to it.
            (b"repo-root/Cargo.lock", None),
            (
                b"repo-root/\xff\xfe",
                Some(CandidatePath::Unstorable(String::new())),
            ),
        ];
        for (raw, expected) in ok_cases {
            assert_eq!(normalize(raw).unwrap(), expected, "{raw:?}");
        }

        // Rejected: path traversal (`..`, `.` root) and embedded NUL
        let err_cases: [&[u8]; 7] = [
            b"../etc/passwd",
            b"./etc/passwd",
            b"repo-root/../etc/passwd",
            b"repo-root/src/../../root",
            b"repo-root/.hidden/../etc/passwd",
            b"repo-root/certs/privkey.key\0",
            b"repo-root\0/src/main.rs",
        ];
        for raw in err_cases {
            assert!(normalize(raw).is_err(), "{raw:?}");
        }

        let dot_root_err = normalize(b"./etc/passwd").unwrap_err();
        assert_eq!(
            dot_root_err.to_string(),
            "invalid repository metadata: Tar entry root segment cannot be '.'"
        );
    }

    #[test]
    fn normalizes_archive_entry_depth_limit() {
        // The limit itself is pinned by `path::classification_corpus_is_stable`; this checks that
        // the archive reports a depth failure as invalid metadata naming the entry kind.
        let excessive_depth_path = format!(
            "repo-root/{}/file.txt",
            vec!["d"; MAX_PATH_DEPTH + 1].join("/")
        );
        let err = normalize(excessive_depth_path.as_bytes())
            .expect_err("one level past the limit must be rejected");
        assert!(
            err.to_string()
                .contains("Tar entry path exceeds maximum directory depth"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn parses_git_archive_pax_comment() {
        let pax_header = b"52 comment=c1863c8b05c5d2db4030bc8c8a7d2f10bda4a950\n";
        assert_eq!(
            parse_pax_comment(pax_header),
            Some("c1863c8b05c5d2db4030bc8c8a7d2f10bda4a950".into())
        );

        let direct_comment = b"comment=c1863c8b05c5d2db4030bc8c8a7d2f10bda4a950\n";
        assert_eq!(
            parse_pax_comment(direct_comment),
            Some("c1863c8b05c5d2db4030bc8c8a7d2f10bda4a950".into())
        );

        let invalid = b"52 comment=not-a-sha\n";
        assert_eq!(parse_pax_comment(invalid), None);

        // A non-UTF-8 PAX record is not a commit SHA, and the parser must not panic.
        let non_utf8 = b"comment=\xFF\xFEc1863c8b05c5d2db4030bc8c8a7d2f10bda4a950\n";
        assert_eq!(parse_pax_comment(non_utf8), None);

        // Only exactly 40 or 64 hex characters are a commit SHA.
        let boundary = [
            ("a".repeat(39), false),
            ("a".repeat(40), true),
            ("a".repeat(41), false),
            ("a".repeat(63), false),
            ("a".repeat(64), true),
            ("a".repeat(65), false),
        ];
        for (sha, valid) in boundary {
            let line = format!("comment={sha}\n");
            assert_eq!(parse_pax_comment(line.as_bytes()).is_some(), valid, "{sha}");
        }
        let non_hex = format!("comment={}g\n", "a".repeat(39));
        assert_eq!(parse_pax_comment(non_hex.as_bytes()), None);

        let empty = b"";
        assert_eq!(parse_pax_comment(empty), None);

        // Only a decimal length may precede the record; any other prefix word voids it.
        let prefixed = format!("ab comment={}\n", "a".repeat(40));
        assert_eq!(parse_pax_comment(prefixed.as_bytes()), None);
    }

    #[test]
    fn dots_inside_a_name_are_not_traversal() {
        for (raw, expected) in [
            (&b"repo-root/a..b.txt"[..], "a..b.txt"),
            (b"repo-root/dir../x", "dir../x"),
        ] {
            assert_eq!(
                normalize(raw).unwrap(),
                Some(CandidatePath::Valid(expected.into())),
                "{raw:?}"
            );
        }
    }
}
