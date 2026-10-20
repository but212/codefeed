use crate::core::error::IngestError;
use crate::core::policy::{PathStatus, classify_path_segments};
use memchr::{memchr, memmem};

/// Normalizes a raw tar entry path, stripping the root directory prefix,
/// collapsing multiple slashes, removing `.` segments, and classifying against policy.
///
/// Returns `Err(IngestError)` if the path contains directory traversal, invalid root prefixes, or NUL bytes.
/// Returns `Ok(PathStatus::Ignored)` if the entry is the root directory itself, empty, hidden, noise, or sensitive.
/// Returns `Ok(PathStatus::NonUtf8Candidate(prefix))` if the entry path contains non-UTF-8 bytes and is not hidden.
/// Returns `Ok(PathStatus::Valid(path))` on success.
pub(crate) fn normalize_tar_entry_path(raw_bytes: &[u8]) -> Result<PathStatus, IngestError> {
    if memchr(0, raw_bytes).is_some() {
        return Err(IngestError::invalid_metadata(
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
        return Err(IngestError::invalid_metadata(
            "Tar entry contains directory traversal (..)",
        ));
    }

    let Some(slash_index) = memchr(b'/', trimmed) else {
        return Ok(PathStatus::Ignored);
    };

    if trimmed[..slash_index] == *b"." {
        return Err(IngestError::invalid_metadata(
            "Tar entry root segment is '.'; expected a single repository root directory",
        ));
    }

    let rel = &trimmed[slash_index + 1..];

    let filtered_segments = rel
        .split(|&b| b == b'/')
        .filter(|seg| !seg.is_empty() && *seg != b".");

    classify_path_segments(filtered_segments).map_err(|_| {
        IngestError::invalid_metadata("Tar entry path exceeds maximum directory depth")
    })
}

/// Parses a 40-character commit SHA from PAX global header comment bytes.
///
/// `git archive` embeds `comment = <commit_sha>` or `comment=<commit_sha>`
/// in the PAX extended header.
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
        assert_eq!(
            normalize_tar_entry_path(b"but212-codefeed-c1863c8/src/main.rs").unwrap(),
            PathStatus::Valid("src/main.rs".into())
        );

        assert_eq!(
            normalize_tar_entry_path(b"repo-root//src/main.rs").unwrap(),
            PathStatus::Valid("src/main.rs".into())
        );

        assert_eq!(
            normalize_tar_entry_path(b"repo-root///deep//nested/./file.txt").unwrap(),
            PathStatus::Valid("deep/nested/file.txt".into())
        );

        assert_eq!(
            normalize_tar_entry_path(b"repo-root/././a.rs").unwrap(),
            PathStatus::Valid("a.rs".into())
        );

        assert_eq!(
            normalize_tar_entry_path(b"but212-codefeed-c1863c8/").unwrap(),
            PathStatus::Ignored
        );
        assert_eq!(
            normalize_tar_entry_path(b"but212-codefeed-c1863c8///").unwrap(),
            PathStatus::Ignored
        );
        assert_eq!(
            normalize_tar_entry_path(b"but212-codefeed-c1863c8").unwrap(),
            PathStatus::Ignored
        );
        assert_eq!(normalize_tar_entry_path(b"").unwrap(), PathStatus::Ignored);

        // Policy exclusion: noise lockfile and sensitive credentials
        assert_eq!(
            normalize_tar_entry_path(b"repo-root/Cargo.lock").unwrap(),
            PathStatus::Ignored
        );
        assert_eq!(
            normalize_tar_entry_path(b"repo-root/id_rsa").unwrap(),
            PathStatus::Ignored
        );
        assert_eq!(
            normalize_tar_entry_path(b"repo-root/secret.pem").unwrap(),
            PathStatus::Ignored
        );

        // Path traversal rejection
        assert!(normalize_tar_entry_path(b"../etc/passwd").is_err());
        assert!(normalize_tar_entry_path(b"./etc/passwd").is_err());
        assert!(normalize_tar_entry_path(b"repo-root/../etc/passwd").is_err());
        assert!(normalize_tar_entry_path(b"repo-root/src/../../root").is_err());
        assert!(normalize_tar_entry_path(b"repo-root/.hidden/../etc/passwd").is_err());

        // Embedded NUL rejection
        assert!(normalize_tar_entry_path(b"repo-root/certs/privkey.key\0").is_err());
        assert!(normalize_tar_entry_path(b"repo-root\0/src/main.rs").is_err());

        // Depth limit rejection (256 directory levels permitted, 257 rejected)
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

        // Non-UTF8 handling
        assert_eq!(
            normalize_tar_entry_path(b"repo-root/invalid/\xff\xfe").unwrap(),
            PathStatus::NonUtf8Candidate("invalid/".into())
        );

        assert_eq!(
            normalize_tar_entry_path(b"repo-root/\xff\xfe").unwrap(),
            PathStatus::NonUtf8Candidate(String::new())
        );

        assert_eq!(
            normalize_tar_entry_path(b"repo-root/.hidden/\xff\xfe").unwrap(),
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
