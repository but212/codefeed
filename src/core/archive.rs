//! Archive parsing pure utilities: Pax header comment extraction and root stripping.

/// Strips the top-level directory prefix from raw archive entry path bytes.
///
/// GitHub tarballs wrap all entries in a root directory `{owner}-{repo}-{hash}/`.
/// Returns `None` for the root directory itself or empty paths.
pub(crate) fn strip_archive_root(raw_bytes: &[u8]) -> Option<&[u8]> {
    let start = raw_bytes
        .iter()
        .position(|&b| b != b'/')
        .unwrap_or(raw_bytes.len());
    let end = raw_bytes
        .iter()
        .rposition(|&b| b != b'/')
        .map_or(start, |i| i + 1);
    let normalized = &raw_bytes[start..end];
    let slash_index = normalized.iter().position(|&b| b == b'/')?;
    let rest = &normalized[slash_index + 1..];
    if rest.is_empty() { None } else { Some(rest) }
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
    ((sha.len() == 40) | (sha.len() == 64))
        && sha
            .bytes()
            .all(|b| (b.wrapping_sub(b'0') < 10) | ((b | 32).wrapping_sub(b'a') < 6))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_archive_root_correctly() {
        assert_eq!(
            strip_archive_root(b"but212-codefeed-c1863c8/src/main.rs"),
            Some(&b"src/main.rs"[..])
        );
        assert_eq!(
            strip_archive_root(b"repo-root/deep/nested/file.txt"),
            Some(&b"deep/nested/file.txt"[..])
        );
        assert_eq!(strip_archive_root(b"but212-codefeed-c1863c8/"), None);
        assert_eq!(strip_archive_root(b"but212-codefeed-c1863c8"), None);
        assert_eq!(strip_archive_root(b""), None);
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
