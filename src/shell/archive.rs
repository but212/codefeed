//! Tarball archive streaming ingestion and entry admission.

use std::io::{BufReader, Read};

use flate2::read::GzDecoder;
use tar::{Archive, EntryType};

use super::IngestError;
use super::error::transport_error;
use crate::core::archive::{normalize_tar_entry_path, parse_pax_comment};
use crate::core::model::CollectionResult;
use crate::core::policy::{ARCHIVE_BUFFER_BYTES, MAX_FILE_SIZE};
use crate::core::selector::Selector;

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
    let entries = archive
        .entries()
        .map_err(|err: std::io::Error| stream_error(&err))?;
    let mut parsed_revision = None;
    let mut selector = Selector::new();
    let mut content_scratch = Vec::with_capacity(32 * 1024);

    for entry_res in entries {
        let mut entry = entry_res.map_err(|err: std::io::Error| stream_error(&err))?;
        if entry.header().entry_type() == EntryType::XGlobalHeader {
            content_scratch.clear();
            entry
                .take(1024)
                .read_to_end(&mut content_scratch)
                .map_err(|err: std::io::Error| stream_error(&err))?;
            parsed_revision = parsed_revision.or_else(|| parse_pax_comment(&content_scratch));
            continue;
        }
        content_scratch.clear();
        selector = process_tar_entry(&mut entry, selector, &mut content_scratch)?;
    }

    let revision = parsed_revision.or_else(|| default_reference.map(str::to_string));
    Ok(selector.finish(label.to_string(), revision))
}

fn process_tar_entry<R: Read>(
    entry: &mut tar::Entry<R>,
    selector: Selector,
    content_scratch: &mut Vec<u8>,
) -> Result<Selector, IngestError> {
    // 1. Skip directories, symlinks, and special entries before path decoding
    if !matches!(
        entry.header().entry_type(),
        EntryType::Regular | EntryType::Continuous
    ) {
        return Ok(selector);
    }

    // 2. Path decoding, root stripping, normalization & policy classification
    let path_bytes = entry.path_bytes();
    let status = normalize_tar_entry_path(&path_bytes)?;

    let size_hint = Some(entry.size());

    // 3. Pre-I/O candidate admission: the body is read only if the candidate is admitted
    selector.observe(status, size_hint, move || {
        entry
            .take(MAX_FILE_SIZE + 1)
            .read_to_end(content_scratch)
            .map_err(|err: std::io::Error| stream_error(&err))?;
        Ok(Some(content_scratch))
    })
}

/// Maps any failure while streaming the archive to a request error. Every `io::Error` here comes from the
/// network body, gzip, or tar decoding, so it never surfaces as a filesystem error.
fn stream_error(err: &std::io::Error) -> IngestError {
    transport_error(&err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::error::DomainError;
    use crate::shell::testing::{gzip_archive, gzip_tarball};

    #[test]
    fn collects_files_and_extracts_pax_sha() {
        let pax_comment = b"52 comment=c1863c8b05c5d2db4030bc8c8a7d2f10bda4a950\n";
        let mock_data = gzip_tarball(&[
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
        use crate::core::policy::MAX_FILE_SIZE_USIZE;

        let mock_data = gzip_tarball(&[
            ("repo-root/small.txt", b"small", EntryType::Regular),
            (
                "repo-root/large.txt",
                &vec![b'x'; MAX_FILE_SIZE_USIZE + 10],
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
    fn skips_too_large_files_with_pax_size_override() {
        use crate::core::policy::{MAX_FILE_SIZE, MAX_FILE_SIZE_USIZE};

        let mock_data = gzip_archive(|tar| {
            let mut pax_header = tar::Header::new_gnu();
            pax_header.set_entry_type(EntryType::XHeader);
            let pax_data = format!("16 size={}\n", MAX_FILE_SIZE + 10);
            pax_header.set_size(pax_data.len() as u64);
            pax_header.set_mode(0o644);
            pax_header.set_cksum();
            tar.append_data(
                &mut pax_header,
                "repo-root/PaxHeaders/pax_large.txt",
                pax_data.as_bytes(),
            )
            .unwrap();

            let mut file_header = tar::Header::new_gnu();
            file_header.set_entry_type(EntryType::Regular);
            let name = b"repo-root/pax_large.txt";
            file_header.as_mut_bytes()[..name.len()].copy_from_slice(name);
            file_header.set_size(0);
            file_header.set_mode(0o644);
            file_header.set_cksum();
            let large_payload = vec![b'a'; MAX_FILE_SIZE_USIZE + 10];
            tar.append(&file_header, large_payload.as_slice()).unwrap();
        });

        let result = collect(
            mock_data.as_slice(),
            "https://github.com/owner/repo",
            Some("main"),
        )
        .expect("collection should succeed");

        assert_eq!(result.files.len(), 0);
        assert_eq!(result.skipped, 1);
    }

    #[test]
    fn regular_file_named_pax_global_header_is_not_dropped() {
        let mock_data = gzip_tarball(&[(
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
        let mock_data = gzip_archive(|tar| {
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
        });

        let result = collect(mock_data.as_slice(), "https://github.com/owner/repo", None)
            .expect("collection should succeed");

        assert_eq!(result.files.len(), 0);
        assert_eq!(result.skipped, 0);
    }

    #[test]
    fn non_utf8_hidden_regular_file_is_ignored() {
        let mock_data = gzip_archive(|tar| {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(EntryType::Regular);
            header.set_size(5);
            header.set_mode(0o644);
            let name = b"repo-root/.git/\xff";
            header.as_mut_bytes()[..name.len()].copy_from_slice(name);
            header.set_cksum();
            tar.append(&header, &b"dummy"[..]).unwrap();
        });

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
        let mock_data = gzip_tarball(&[
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

    #[test]
    fn duplicate_paths_across_repo_roots_fail_the_run() {
        let mock_data = gzip_tarball(&[
            ("r1/a.txt", b"first", EntryType::Regular),
            ("r2/a.txt", b"second", EntryType::Regular),
        ]);

        let err = collect(mock_data.as_slice(), "https://github.com/owner/repo", None).unwrap_err();
        assert!(
            matches!(&err, IngestError::Domain(DomainError::InvalidMetadata(_))),
            "expected InvalidMetadata, got: {err:?}"
        );
    }

    #[test]
    fn duplicate_paths_after_normalization_fail_the_run() {
        let mock_data = gzip_tarball(&[
            ("repo/a//b.txt", b"first", EntryType::Regular),
            ("repo/a/b.txt", b"second", EntryType::Regular),
        ]);

        let err = collect(mock_data.as_slice(), "https://github.com/owner/repo", None).unwrap_err();
        assert!(
            matches!(&err, IngestError::Domain(DomainError::InvalidMetadata(_))),
            "expected InvalidMetadata, got: {err:?}"
        );
    }

    #[test]
    fn duplicate_skipped_candidate_fails_the_run() {
        // The oversized second entry is skipped for size, but its path still collides with the stored first entry.
        let oversized = vec![b'x'; crate::core::policy::MAX_FILE_SIZE_USIZE + 1];
        let mock_data = gzip_archive(|tar| {
            for body in [&b"first"[..], oversized.as_slice()] {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(EntryType::Regular);
                header.set_size(body.len() as u64);
                header.set_mode(0o644);
                let name = b"repo/a.txt";
                header.as_mut_bytes()[..name.len()].copy_from_slice(name);
                header.set_cksum();
                tar.append(&header, body).unwrap();
            }
        });

        let err = collect(mock_data.as_slice(), "https://github.com/owner/repo", None).unwrap_err();
        assert!(
            matches!(&err, IngestError::Domain(DomainError::InvalidMetadata(_))),
            "expected InvalidMetadata, got: {err:?}"
        );
    }

    #[test]
    fn non_regular_entries_are_dropped_before_path_normalization() {
        // Pins the documented order: entry type filtering runs before tar path normalization, so
        // a non-regular entry with a traversal path is ignored rather than rejected.
        let mock_data = gzip_archive(|tar| {
            for (kind, name) in [
                (EntryType::Directory, &b"repo-root/../x/"[..]),
                (EntryType::Symlink, &b"repo-root/../y"[..]),
            ] {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(kind);
                header.set_size(0);
                header.set_mode(0o755);
                header.as_mut_bytes()[..name.len()].copy_from_slice(name);
                header.set_cksum();
                tar.append(&header, &b""[..]).unwrap();
            }
        });

        let result = collect(mock_data.as_slice(), "https://github.com/owner/repo", None)
            .expect("collection should succeed");

        assert_eq!(result.files.len(), 0);
        assert_eq!(result.skipped, 0);
    }

    #[test]
    fn stream_decode_failure_is_http() {
        let Err(err) = collect(&b"not gzip"[..], "x", None) else {
            panic!("expected error");
        };
        assert!(matches!(err, IngestError::Http(_)), "{err}");
        assert!(err.to_string().starts_with("GitHub request failed:"));
    }
}
