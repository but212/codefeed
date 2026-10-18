use crate::core::error::IngestError;
use crate::core::policy::{PathStatus, classify_path_segments};

/// Normalizes a raw tar entry path into `out`, stripping the root directory prefix,
/// collapsing multiple slashes, removing `.` segments, and classifying against policy.
///
/// Returns `Err(IngestError)` if the path contains directory traversal, invalid root prefixes, or NUL bytes.
/// Returns `Ok(PathStatus::Ignored)` if the entry is the root directory itself, empty, hidden, noise, or sensitive.
/// Returns `Ok(PathStatus::NonUtf8Candidate)` if the entry path contains non-UTF-8 bytes and is not hidden.
/// Returns `Ok(PathStatus::Valid)` on success.
pub(crate) fn normalize_tar_entry_path(
    raw_bytes: &[u8],
    out: &mut String,
) -> Result<PathStatus, IngestError> {
    out.clear();
    if raw_bytes.contains(&0) {
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

    if trimmed.split(|&b| b == b'/').any(|seg| seg == b"..") {
        return Err(IngestError::invalid_metadata(
            "Tar entry contains directory traversal (..)",
        ));
    }

    let Some(slash_index) = trimmed.iter().position(|&b| b == b'/') else {
        return Ok(PathStatus::Ignored);
    };

    if trimmed[..slash_index] == *b"." {
        return Err(IngestError::invalid_metadata(
            "Tar entry contains directory traversal (..)",
        ));
    }

    let rel = &trimmed[slash_index + 1..];

    if rel
        .split(|&b| b == b'/')
        .filter(|seg| !seg.is_empty() && *seg != b".")
        .count()
        > crate::core::policy::MAX_PATH_DEPTH
    {
        return Err(IngestError::invalid_metadata(
            "Tar entry path exceeds maximum directory depth",
        ));
    }

    let filtered_segments = rel
        .split(|&b| b == b'/')
        .filter(|seg| !seg.is_empty() && *seg != b".");

    let status = classify_path_segments(filtered_segments, out);
    Ok(status)
}

/// Parses a 40-character commit SHA from PAX global header comment bytes.
///
/// `git archive` embeds `comment = <commit_sha>` or `comment=<commit_sha>`
/// in the PAX extended header.
pub(crate) fn parse_pax_comment(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    text.lines().find_map(|line| {
        let trimmed = line.trim_ascii();
        let payload = match trimmed.find(' ') {
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

fn is_valid_commit_sha(sha: &str) -> bool {
    (sha.len() == 40 || sha.len() == 64) && sha.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_archive_entry_path_correctly() {
        let mut out = String::new();

        assert_eq!(
            normalize_tar_entry_path(b"but212-codefeed-c1863c8/src/main.rs", &mut out).unwrap(),
            PathStatus::Valid
        );
        assert_eq!(out, "src/main.rs");

        assert_eq!(
            normalize_tar_entry_path(b"repo-root//src/main.rs", &mut out).unwrap(),
            PathStatus::Valid
        );
        assert_eq!(out, "src/main.rs");

        assert_eq!(
            normalize_tar_entry_path(b"repo-root///deep//nested/./file.txt", &mut out).unwrap(),
            PathStatus::Valid
        );
        assert_eq!(out, "deep/nested/file.txt");

        assert_eq!(
            normalize_tar_entry_path(b"repo-root/././a.rs", &mut out).unwrap(),
            PathStatus::Valid
        );
        assert_eq!(out, "a.rs");

        assert_eq!(
            normalize_tar_entry_path(b"but212-codefeed-c1863c8/", &mut out).unwrap(),
            PathStatus::Ignored
        );
        assert_eq!(
            normalize_tar_entry_path(b"but212-codefeed-c1863c8///", &mut out).unwrap(),
            PathStatus::Ignored
        );
        assert_eq!(
            normalize_tar_entry_path(b"but212-codefeed-c1863c8", &mut out).unwrap(),
            PathStatus::Ignored
        );
        assert_eq!(
            normalize_tar_entry_path(b"", &mut out).unwrap(),
            PathStatus::Ignored
        );

        // Policy exclusion: noise lockfile and sensitive credentials
        assert_eq!(
            normalize_tar_entry_path(b"repo-root/Cargo.lock", &mut out).unwrap(),
            PathStatus::Ignored
        );
        assert_eq!(
            normalize_tar_entry_path(b"repo-root/id_rsa", &mut out).unwrap(),
            PathStatus::Ignored
        );
        assert_eq!(
            normalize_tar_entry_path(b"repo-root/secret.pem", &mut out).unwrap(),
            PathStatus::Ignored
        );

        // Path traversal rejection
        assert!(normalize_tar_entry_path(b"../etc/passwd", &mut out).is_err());
        assert!(normalize_tar_entry_path(b"./etc/passwd", &mut out).is_err());
        assert!(normalize_tar_entry_path(b"repo-root/../etc/passwd", &mut out).is_err());
        assert!(normalize_tar_entry_path(b"repo-root/src/../../root", &mut out).is_err());
        assert!(normalize_tar_entry_path(b"repo-root/.hidden/../etc/passwd", &mut out).is_err());

        // Embedded NUL rejection
        assert!(normalize_tar_entry_path(b"repo-root/certs/privkey.key\0", &mut out).is_err());
        assert!(normalize_tar_entry_path(b"repo-root\0/src/main.rs", &mut out).is_err());

        // Depth limit rejection
        let normal_depth_path = format!("repo-root/{}/file.txt", vec!["d"; 255].join("/"));
        assert!(normalize_tar_entry_path(normal_depth_path.as_bytes(), &mut out).is_ok());

        let excessive_depth_path = format!("repo-root/{}/file.txt", vec!["d"; 256].join("/"));
        assert!(normalize_tar_entry_path(excessive_depth_path.as_bytes(), &mut out).is_err());

        // Non-UTF8 handling
        assert_eq!(
            normalize_tar_entry_path(b"repo-root/invalid/\xff\xfe", &mut out).unwrap(),
            PathStatus::NonUtf8Candidate
        );
        assert_eq!(out, "invalid/");

        assert_eq!(
            normalize_tar_entry_path(b"repo-root/\xff\xfe", &mut out).unwrap(),
            PathStatus::NonUtf8Candidate
        );
        assert_eq!(out, "");

        assert_eq!(
            normalize_tar_entry_path(b"repo-root/.hidden/\xff\xfe", &mut out).unwrap(),
            PathStatus::Ignored
        );
        assert_eq!(out, "");
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
