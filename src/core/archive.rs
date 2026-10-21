use crate::core::error::DomainError;
use crate::core::policy::{PathStatus, classify_path_segments};
use memchr::{memchr, memmem};

/// Normalizes a raw tar entry path, stripping the root directory prefix,
/// collapsing multiple slashes, removing `.` segments, and classifying against policy.
///
/// Returns `Err(DomainError)` if the path contains directory traversal, invalid root prefixes, or NUL bytes.
/// Returns `Ok(PathStatus::Ignored)` if the entry is the root directory itself, empty, hidden, noise, or sensitive.
/// Returns `Ok(PathStatus::NonUtf8Candidate(prefix))` if the entry path contains non-UTF-8 bytes and is not hidden.
/// Returns `Ok(PathStatus::Valid(path))` on success.
pub(crate) fn normalize_tar_entry_path(raw_bytes: &[u8]) -> Result<PathStatus, DomainError> {
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
        return Ok(PathStatus::Ignored);
    };

    if trimmed[..slash_index] == *b"." {
        return Err(DomainError::invalid_metadata(
            "Tar entry root segment is '.'; expected a single repository root directory",
        ));
    }

    let rel = &trimmed[slash_index + 1..];

    classify_path_segments(rel.split(|&b| b == b'/')).map_err(|_| {
        DomainError::invalid_metadata("Tar entry path exceeds maximum directory depth")
    })
}

/// Parses a 40- or 64-hex-character commit SHA from a `comment=<sha>` PAX record.
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
            .map(|s| s.trim_ascii())
            .filter(|sha| is_valid_commit_sha(sha))
            .map(ToOwned::to_owned)
    })
}

const fn is_valid_commit_sha(sha: &str) -> bool {
    let bytes = sha.as_bytes();
    let len = bytes.len();
    if len != 40 && len != 64 {
        return false;
    }
    let mut i = 0;
    while i < len {
        if !bytes[i].is_ascii_hexdigit() {
            return false;
        }
        i += 1;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_archive_entry_path_correctly() {
        // (raw entry path, expected status). Covers root stripping, slash and `.` collapsing,
        // policy exclusion (noise lockfile, sensitive credential), and non-UTF-8 prefixes.
        let ok_cases: [(&[u8], PathStatus); 14] = [
            (
                b"but212-codefeed-c1863c8/src/main.rs",
                PathStatus::Valid("src/main.rs".into()),
            ),
            (
                b"repo-root//src/main.rs",
                PathStatus::Valid("src/main.rs".into()),
            ),
            (
                b"repo-root///deep//nested/./file.txt",
                PathStatus::Valid("deep/nested/file.txt".into()),
            ),
            (b"repo-root/././a.rs", PathStatus::Valid("a.rs".into())),
            (b"but212-codefeed-c1863c8/", PathStatus::Ignored),
            (b"but212-codefeed-c1863c8///", PathStatus::Ignored),
            (b"but212-codefeed-c1863c8", PathStatus::Ignored),
            (b"", PathStatus::Ignored),
            (b"repo-root/Cargo.lock", PathStatus::Ignored),
            (b"repo-root/id_rsa", PathStatus::Ignored),
            (b"repo-root/secret.pem", PathStatus::Ignored),
            (b"repo-root/.hidden/\xff\xfe", PathStatus::Ignored),
            (
                b"repo-root/invalid/\xff\xfe",
                PathStatus::NonUtf8Candidate("invalid/".into()),
            ),
            (
                b"repo-root/\xff\xfe",
                PathStatus::NonUtf8Candidate(String::new()),
            ),
        ];
        for (raw, expected) in ok_cases {
            assert_eq!(normalize_tar_entry_path(raw).unwrap(), expected, "{raw:?}");
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
            assert!(normalize_tar_entry_path(raw).is_err(), "{raw:?}");
        }
    }

    #[test]
    fn normalizes_archive_entry_depth_limit() {
        // 256 directory levels permitted, 257 rejected
        let normal_depth_path = format!("repo-root/{}/file.txt", vec!["d"; 256].join("/"));
        assert_eq!(
            normalize_tar_entry_path(normal_depth_path.as_bytes()).unwrap(),
            PathStatus::Valid(format!("{}/file.txt", vec!["d"; 256].join("/")).into())
        );

        let excessive_depth_path = format!("repo-root/{}/file.txt", vec!["d"; 257].join("/"));
        assert!(normalize_tar_entry_path(excessive_depth_path.as_bytes()).is_err());

        // Deeply nested hidden path is ignored without triggering depth error
        let deep_hidden_path = format!("repo-root/.hidden/{}/file.txt", vec!["d"; 300].join("/"));
        assert_eq!(
            normalize_tar_entry_path(deep_hidden_path.as_bytes()).unwrap(),
            PathStatus::Ignored
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

        let empty = b"";
        assert_eq!(parse_pax_comment(empty), None);
    }
}
