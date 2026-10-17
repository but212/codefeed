use crate::core::error::IngestError;

/// Status of normalized tar entry path decoding.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TarPathStatus {
    /// Valid normalized relative path written to the output buffer.
    Valid,
    /// Root directory or empty path that should be ignored.
    Skipped,
    /// Path contains invalid UTF-8 bytes.
    NonUtf8 { is_hidden: bool },
}

/// Normalizes a raw tar entry path into `out`, stripping the root directory prefix,
/// collapsing multiple slashes, and removing `.` segments.
///
/// Returns `Err(IngestError)` if the path contains `..` directory traversal.
/// Returns `Ok(TarPathStatus::Skipped)` if the entry is the root directory itself or empty.
/// Returns `Ok(TarPathStatus::NonUtf8)` if the entry path contains non-UTF-8 bytes.
/// Returns `Ok(TarPathStatus::Valid)` on success.
pub(crate) fn normalize_tar_entry_path(
    raw_bytes: &[u8],
    out: &mut String,
) -> Result<TarPathStatus, IngestError> {
    let start = raw_bytes
        .iter()
        .position(|&b| b != b'/')
        .unwrap_or(raw_bytes.len());
    let end = raw_bytes
        .iter()
        .rposition(|&b| b != b'/')
        .map_or(start, |i| i + 1);
    let trimmed = &raw_bytes[start..end];
    let Some(slash_index) = trimmed.iter().position(|&b| b == b'/') else {
        return Ok(TarPathStatus::Skipped);
    };

    let rel = &trimmed[slash_index + 1..];
    out.clear();

    let mut has_hidden = false;
    let mut has_invalid_utf8 = false;

    for segment in rel.split(|&b| b == b'/') {
        if segment.is_empty() || segment == b"." {
            continue;
        }
        if segment == b".." {
            return Err(IngestError::invalid_metadata(
                "Tar entry contains directory traversal (..)",
            ));
        }
        if segment.starts_with(b".") {
            has_hidden = true;
        }
        if has_invalid_utf8 {
            continue;
        }
        match std::str::from_utf8(segment) {
            Ok(seg_str) => {
                if !out.is_empty() {
                    out.push('/');
                }
                out.push_str(seg_str);
            }
            Err(_) => {
                has_invalid_utf8 = true;
            }
        }
    }

    if has_invalid_utf8 {
        out.clear();
        return Ok(TarPathStatus::NonUtf8 {
            is_hidden: has_hidden,
        });
    }

    if out.is_empty() {
        Ok(TarPathStatus::Skipped)
    } else {
        Ok(TarPathStatus::Valid)
    }
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
            TarPathStatus::Valid
        );
        assert_eq!(out, "src/main.rs");

        assert_eq!(
            normalize_tar_entry_path(b"repo-root//src/main.rs", &mut out).unwrap(),
            TarPathStatus::Valid
        );
        assert_eq!(out, "src/main.rs");

        assert_eq!(
            normalize_tar_entry_path(b"repo-root///deep//nested/./file.txt", &mut out).unwrap(),
            TarPathStatus::Valid
        );
        assert_eq!(out, "deep/nested/file.txt");

        assert_eq!(
            normalize_tar_entry_path(b"repo-root/././a.rs", &mut out).unwrap(),
            TarPathStatus::Valid
        );
        assert_eq!(out, "a.rs");

        assert_eq!(
            normalize_tar_entry_path(b"but212-codefeed-c1863c8/", &mut out).unwrap(),
            TarPathStatus::Skipped
        );
        assert_eq!(
            normalize_tar_entry_path(b"but212-codefeed-c1863c8///", &mut out).unwrap(),
            TarPathStatus::Skipped
        );
        assert_eq!(
            normalize_tar_entry_path(b"but212-codefeed-c1863c8", &mut out).unwrap(),
            TarPathStatus::Skipped
        );
        assert_eq!(
            normalize_tar_entry_path(b"", &mut out).unwrap(),
            TarPathStatus::Skipped
        );

        // Path traversal rejection
        assert!(normalize_tar_entry_path(b"repo-root/../etc/passwd", &mut out).is_err());
        assert!(normalize_tar_entry_path(b"repo-root/src/../../root", &mut out).is_err());

        // Non-UTF8 handling
        assert_eq!(
            normalize_tar_entry_path(b"repo-root/invalid/\xff\xfe", &mut out).unwrap(),
            TarPathStatus::NonUtf8 { is_hidden: false }
        );
        assert_eq!(
            normalize_tar_entry_path(b"repo-root/.hidden/\xff\xfe", &mut out).unwrap(),
            TarPathStatus::NonUtf8 { is_hidden: true }
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
