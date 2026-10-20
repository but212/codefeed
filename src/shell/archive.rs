//! Tarball archive streaming ingestion and entry admission.

use std::io::{BufReader, Read};

use flate2::read::GzDecoder;
use tar::{Archive, EntryType};
use zeroize::Zeroizing;

use crate::core::archive::{normalize_tar_entry_path, parse_pax_comment};
use crate::core::error::IngestError;
use crate::core::model::CollectionResult;
use crate::core::policy::{ARCHIVE_BUFFER_BYTES, MAX_FILE_SIZE};
use crate::core::selector::{Admission, Selector};

/// Collects files from a tarball reader directly into a [`CollectionResult`].
///
/// Decompresses and streams entries through a tuned [`ARCHIVE_BUFFER_BYTES`] buffer,
/// extracts commit revision from PAX global headers if present (falling back to `default_reference`),
/// filters non-regular or sensitive entries via core policies, and feeds valid candidates to [`Selector`].
pub(super) fn collect<R: Read>(
    reader: R,
    label: &str,
    default_reference: Option<&str>,
) -> Result<CollectionResult, IngestError> {
    let mut archive = Archive::new(GzDecoder::new(BufReader::with_capacity(
        ARCHIVE_BUFFER_BYTES,
        reader,
    )));
    let entries = archive.entries().map_err(IngestError::Io)?;
    let mut parsed_revision = None;
    let mut selector = Selector::new();
    let mut content_scratch = Zeroizing::new(Vec::with_capacity(32 * 1024));

    for entry_res in entries {
        let mut entry = entry_res.map_err(IngestError::Io)?;
        content_scratch.clear();
        let (next_selector, sha) = process_tar_entry(&mut entry, selector, &mut content_scratch)?;
        selector = next_selector;
        if let Some(sha) = sha
            && parsed_revision.is_none()
        {
            parsed_revision = Some(sha);
        }
    }

    let revision = parsed_revision.or_else(|| default_reference.map(str::to_string));
    Ok(selector.finish(label.to_string(), revision))
}

fn process_tar_entry<R: Read>(
    entry: &mut tar::Entry<R>,
    selector: Selector,
    content_scratch: &mut Vec<u8>,
) -> Result<(Selector, Option<String>), IngestError> {
    let entry_type = entry.header().entry_type();

    // 1. PAX Global Header: Extract git commit SHA from comment
    if entry_type == EntryType::XGlobalHeader {
        entry
            .take(1024)
            .read_to_end(content_scratch)
            .map_err(IngestError::Io)?;
        return Ok((selector, parse_pax_comment(content_scratch)));
    }

    // 2. Skip directories, symlinks, and special entries before path decoding
    if !matches!(entry_type, EntryType::Regular | EntryType::Continuous) {
        return Ok((selector, None));
    }

    // 3. Path decoding, root stripping, normalization & policy classification
    let path_bytes = entry.path_bytes();
    let status = normalize_tar_entry_path(&path_bytes)?;

    let size_hint = entry.header().size().ok();

    // 4. Pre-I/O candidate admission before reading entry body
    match selector.admit(status, size_hint)? {
        Admission::Settled(s) => Ok((s, None)),
        Admission::Pending(pending) => {
            read_tar_entry_into(entry, MAX_FILE_SIZE, content_scratch)?;
            let selector = pending.accept(content_scratch)?;
            Ok((selector, None))
        }
    }
}

fn read_tar_entry_into<R: Read>(
    entry: &mut tar::Entry<R>,
    limit: u64,
    buf: &mut Vec<u8>,
) -> Result<(), IngestError> {
    entry
        .take(limit.saturating_add(1))
        .read_to_end(buf)
        .map_err(IngestError::Io)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use tar::Builder;

    fn create_mock_tarball(entries: &[(&str, &[u8], EntryType)]) -> Vec<u8> {
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        {
            let mut tar = Builder::new(&mut gz);
            for &(name, content, ref entry_type) in entries {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(*entry_type);
                header.set_size(content.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                tar.append_data(&mut header, name, content).unwrap();
            }
            tar.finish().unwrap();
        }
        gz.finish().unwrap()
    }

    #[test]
    fn collects_files_and_extracts_pax_sha() {
        let pax_comment = b"52 comment=c1863c8b05c5d2db4030bc8c8a7d2f10bda4a950\n";
        let mock_data = create_mock_tarball(&[
            ("pax_global_header", pax_comment, EntryType::XGlobalHeader),
            ("repo-root/", b"", EntryType::Directory),
            (
                "repo-root/Cargo.toml",
                b"[package]\nname = \"test\"",
                EntryType::Regular,
            ),
            ("repo-root/src/main.rs", b"fn main() {}", EntryType::Regular),
            ("repo-root/Cargo.lock", b"lockfile", EntryType::Regular),
            ("repo-root/poetry.lock", b"lockfile", EntryType::Regular),
            ("repo-root/id_ecdsa", b"PRIVATE KEY", EntryType::Regular),
            (
                "repo-root/image.png",
                b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR",
                EntryType::Regular,
            ),
        ]);

        let result = collect(
            mock_data.as_slice(),
            "https://github.com/but212/codefeed",
            None,
        )
        .expect("collection should succeed");

        assert_eq!(
            result.revision.as_deref(),
            Some("c1863c8b05c5d2db4030bc8c8a7d2f10bda4a950")
        );
        assert_eq!(result.label, "https://github.com/but212/codefeed");

        let paths: Vec<&str> = result.files.iter().map(|f| f.path).collect();
        assert_eq!(paths, vec!["Cargo.toml", "src/main.rs"]);

        // Cargo.lock is ignored (neither candidate nor skipped)
        // image.png is binary (contains NUL byte -> skipped = 1)
        assert_eq!(result.skipped, 1);
    }

    #[test]
    fn skips_too_large_files() {
        use crate::core::policy::MAX_FILE_SIZE;

        let mock_data = create_mock_tarball(&[
            ("repo-root/small.txt", b"small", EntryType::Regular),
            (
                "repo-root/large.txt",
                &vec![b'x'; (MAX_FILE_SIZE + 10) as usize],
                EntryType::Regular,
            ),
        ]);

        let result = collect(
            mock_data.as_slice(),
            "https://github.com/owner/repo",
            Some("main"),
        )
        .expect("collection should succeed");

        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files.iter().next().unwrap().path, "small.txt");
        assert_eq!(result.skipped, 1);
    }

    #[test]
    fn regular_file_named_pax_global_header_is_not_dropped() {
        let mock_data = create_mock_tarball(&[(
            "repo-root/pax_global_header",
            b"hello content",
            EntryType::Regular,
        )]);

        let result = collect(mock_data.as_slice(), "https://github.com/owner/repo", None)
            .expect("collection should succeed");

        assert_eq!(result.files.len(), 1);
        let first = result.files.iter().next().unwrap();
        assert_eq!(first.path, "pax_global_header");
        assert_eq!(first.content, "hello content");
        assert_eq!(result.revision, None);
    }

    #[test]
    fn non_utf8_symlink_or_dir_is_ignored() {
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        {
            let mut tar = Builder::new(&mut gz);
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(EntryType::Symlink);
            header.set_size(0);
            header.set_mode(0o777);
            let name1 = b"repo-root/\xff_symlink";
            header.as_mut_bytes()[..name1.len()].copy_from_slice(name1);
            header.set_cksum();
            tar.append(&header, &b""[..]).unwrap();

            let mut header2 = tar::Header::new_gnu();
            header2.set_entry_type(EntryType::Directory);
            header2.set_size(0);
            header2.set_mode(0o755);
            let name2 = b"repo-root/\xff_dir/";
            header2.as_mut_bytes()[..name2.len()].copy_from_slice(name2);
            header2.set_cksum();
            tar.append(&header2, &b""[..]).unwrap();

            tar.finish().unwrap();
        }
        let mock_data = gz.finish().unwrap();

        let result = collect(mock_data.as_slice(), "https://github.com/owner/repo", None)
            .expect("collection should succeed");

        assert_eq!(result.files.len(), 0);
        assert_eq!(result.skipped, 0);
    }

    #[test]
    fn non_utf8_hidden_regular_file_is_ignored() {
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        {
            let mut tar = Builder::new(&mut gz);
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(EntryType::Regular);
            header.set_size(5);
            header.set_mode(0o644);
            let name = b"repo-root/.git/\xff";
            header.as_mut_bytes()[..name.len()].copy_from_slice(name);
            header.set_cksum();
            tar.append(&header, &b"dummy"[..]).unwrap();
            tar.finish().unwrap();
        }
        let mock_data = gz.finish().unwrap();

        let result = collect(mock_data.as_slice(), "https://github.com/owner/repo", None)
            .expect("collection should succeed");

        assert_eq!(result.files.len(), 0);
        assert_eq!(
            result.skipped, 0,
            "hidden entries must never be accounted as candidates or skipped"
        );
    }

    #[test]
    fn accepts_pax_header_after_archive_entries() {
        let pax_comment = b"52 comment=c1863c8b05c5d2db4030bc8c8a7d2f10bda4a950\n";
        let mock_data = create_mock_tarball(&[
            ("repo-root/file.txt", b"content", EntryType::Regular),
            ("pax_global_header", pax_comment, EntryType::XGlobalHeader),
        ]);

        let result = collect(mock_data.as_slice(), "https://github.com/owner/repo", None)
            .expect("collection should succeed");

        assert_eq!(
            result.revision.as_deref(),
            Some("c1863c8b05c5d2db4030bc8c8a7d2f10bda4a950")
        );
        assert_eq!(result.files.len(), 1);
    }
}
