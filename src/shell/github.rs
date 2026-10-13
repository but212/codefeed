//! GitHub collection orchestration: single-request streaming tarball ingestion.

use std::io::Read;

use flate2::read::GzDecoder;
use tar::{Archive, EntryType};

use crate::core::archive::{parse_pax_comment, strip_archive_root};
use crate::core::decide::{Item, ItemPath, Probe, Selector};
use crate::core::error::IngestError;
use crate::core::model::{CollectionResult, DigestFormat};
use crate::core::policy::{MAX_FILE_SIZE, PathClassification};

use super::github_client::GithubClient;
pub fn collect_github(
    owner: &str,
    repo: &str,
    reference: Option<&str>,
    format: DigestFormat,
) -> Result<CollectionResult, IngestError> {
    let client = GithubClient::new();
    let stream = client.tarball_stream(owner, repo, reference)?;
    collect_from_archive(stream, owner, repo, reference, format)
}

enum ArchiveEntryOutcome {
    PaxHeader(String),
    Item(Item),
    Ignored,
}

pub(crate) fn collect_from_archive<R: Read>(
    reader: R,
    owner: &str,
    repo: &str,
    reference: Option<&str>,
    format: DigestFormat,
) -> Result<CollectionResult, IngestError> {
    let gz = GzDecoder::new(reader);
    let mut archive = Archive::new(gz);

    let label = format!("https://github.com/{owner}/{repo}");
    let mut parsed_revision: Option<String> = None;
    let mut selector = Selector::new(format);

    let entries = archive.entries().map_err(IngestError::Io)?;

    for entry_res in entries {
        let mut entry = entry_res.map_err(IngestError::Io)?;
        match probe_tar_entry(&mut entry)? {
            ArchiveEntryOutcome::PaxHeader(sha) => {
                if !selector.is_empty() {
                    return Err(IngestError::InvalidMetadata(
                        "pax header after archive entries".into(),
                    ));
                }
                parsed_revision = Some(sha);
            }
            ArchiveEntryOutcome::Item(item) => {
                selector.decide(item)?;
            }
            ArchiveEntryOutcome::Ignored => {}
        }
    }

    let final_revision = parsed_revision.or_else(|| reference.map(str::to_string));
    Ok(selector.finish(label, final_revision))
}

fn probe_tar_entry<R: Read>(entry: &mut tar::Entry<R>) -> Result<ArchiveEntryOutcome, IngestError> {
    let entry_type = entry.header().entry_type();

    // 1. PAX Global Header: Extract git commit SHA from comment
    if entry_type == EntryType::XGlobalHeader {
        let mut pax_bytes = Vec::new();
        let _ = entry.take(1024).read_to_end(&mut pax_bytes);
        if let Some(sha) = parse_pax_comment(&pax_bytes) {
            return Ok(ArchiveEntryOutcome::PaxHeader(sha));
        }
        return Ok(ArchiveEntryOutcome::Ignored);
    }

    // 2. Skip directories, symlinks, and special entries before path decoding
    if entry_type.is_dir() || !matches!(entry_type, EntryType::Regular | EntryType::Continuous) {
        return Ok(ArchiveEntryOutcome::Ignored);
    }

    // 3. Path decoding & root stripping
    let path_cow = match entry.path() {
        Ok(p) => p,
        Err(_) => {
            return Ok(ArchiveEntryOutcome::Item(Item {
                path: ItemPath::NonUtf8,
                probe: Probe::Missing,
            }));
        }
    };

    let Some(path_str) = path_cow.to_str() else {
        return Ok(ArchiveEntryOutcome::Item(Item {
            path: ItemPath::NonUtf8,
            probe: Probe::Missing,
        }));
    };

    let Some(rel_path) = strip_archive_root(path_str) else {
        // Root directory entry itself (e.g. "{owner}-{repo}-{sha}/")
        return Ok(ArchiveEntryOutcome::Ignored);
    };
    let rel_path = rel_path.to_owned();

    // 4. Conventional filtering (noise, sensitive, hidden)
    let classification = PathClassification::classify(&rel_path);
    if classification == PathClassification::Ignored {
        return Ok(ArchiveEntryOutcome::Ignored);
    }

    // 5. Known binary extension pre-filter (avoid body read)
    if classification == PathClassification::PreclassifiedBinary {
        return Ok(ArchiveEntryOutcome::Item(Item {
            path: ItemPath::Utf8(rel_path),
            probe: Probe::Missing,
        }));
    }

    // 6. Content reading with MAX_FILE_SIZE limit
    let header_size = entry.header().size().unwrap_or(0);
    if header_size > MAX_FILE_SIZE {
        return Ok(ArchiveEntryOutcome::Item(Item {
            path: ItemPath::Utf8(rel_path),
            probe: Probe::TooLarge,
        }));
    }

    let initial_cap = (header_size as usize).min(8 * 1024);
    let mut bytes = Vec::with_capacity(initial_cap);
    entry
        .take(MAX_FILE_SIZE.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(IngestError::Io)?;

    let probe = if bytes.len() as u64 > MAX_FILE_SIZE {
        Probe::TooLarge
    } else {
        Probe::Bytes(bytes)
    };

    Ok(ArchiveEntryOutcome::Item(Item {
        path: ItemPath::Utf8(rel_path),
        probe,
    }))
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
            ("repo-root/image.png", b"binary-data", EntryType::Regular),
        ]);

        let result = collect_from_archive(
            mock_data.as_slice(),
            "but212",
            "codefeed",
            None,
            DigestFormat::Text,
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
        // image.png is binary (preclassified missing -> skipped = 1)
        assert_eq!(result.skipped, 1);
    }

    #[test]
    fn skips_too_large_files() {
        let mock_data = create_mock_tarball(&[
            ("repo-root/small.txt", b"small", EntryType::Regular),
            (
                "repo-root/large.txt",
                &vec![b'x'; (MAX_FILE_SIZE + 10) as usize],
                EntryType::Regular,
            ),
        ]);

        let result = collect_from_archive(
            mock_data.as_slice(),
            "owner",
            "repo",
            Some("main"),
            DigestFormat::Text,
        )
        .expect("collection should succeed");

        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files.get_at(0).unwrap().path, "small.txt");
        assert_eq!(result.skipped, 1);
    }

    #[test]
    fn regular_file_named_pax_global_header_is_not_dropped() {
        let mock_data = create_mock_tarball(&[(
            "repo-root/pax_global_header",
            b"hello content",
            EntryType::Regular,
        )]);

        let result = collect_from_archive(
            mock_data.as_slice(),
            "owner",
            "repo",
            None,
            DigestFormat::Text,
        )
        .expect("collection should succeed");

        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files.get_at(0).unwrap().path, "pax_global_header");
        assert_eq!(result.files.get_at(0).unwrap().content, "hello content");
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

        let result = collect_from_archive(
            mock_data.as_slice(),
            "owner",
            "repo",
            None,
            DigestFormat::Text,
        )
        .expect("collection should succeed");

        assert_eq!(result.files.len(), 0);
        assert_eq!(result.skipped, 0);
    }

    #[test]
    fn rejects_out_of_order_pax_header() {
        let pax_comment = b"52 comment=c1863c8b05c5d2db4030bc8c8a7d2f10bda4a950\n";
        let mock_data = create_mock_tarball(&[
            ("repo-root/file.txt", b"content", EntryType::Regular),
            ("pax_global_header", pax_comment, EntryType::XGlobalHeader),
        ]);

        let result = collect_from_archive(
            mock_data.as_slice(),
            "owner",
            "repo",
            None,
            DigestFormat::Text,
        );

        assert!(matches!(
            result,
            Err(IngestError::InvalidMetadata(msg)) if msg.contains("pax header after archive entries")
        ));
    }
}
