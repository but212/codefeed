//! GitHub collection orchestration: single-request streaming tarball ingestion.

use std::io::Read;

use flate2::read::GzDecoder;
use tar::{Archive, EntryType};

use crate::core::archive::{parse_pax_comment, strip_archive_root};
use crate::core::decide::{Item, ItemPath, Probe};
use crate::core::error::IngestError;
use crate::core::policy::{MAX_FILE_SIZE, PathClassification};

use super::orchestrate::SourceStream;

pub struct GitHubStream<'a, R: Read> {
    label: String,
    reference: Option<String>,
    parsed_revision: Option<String>,
    entries: tar::Entries<'a, GzDecoder<R>>,
    seen_items: bool,
}

impl<'a, R: Read> GitHubStream<'a, R> {
    pub fn new(
        archive: &'a mut Archive<GzDecoder<R>>,
        owner: &str,
        repo: &str,
        reference: Option<&str>,
    ) -> Result<Self, IngestError> {
        let entries = archive.entries().map_err(IngestError::Io)?;
        Ok(Self {
            label: format!("https://github.com/{owner}/{repo}"),
            reference: reference.map(str::to_string),
            parsed_revision: None,
            entries,
            seen_items: false,
        })
    }
}

impl<'a, R: Read> Iterator for GitHubStream<'a, R> {
    type Item = Result<Item, IngestError>;

    fn next(&mut self) -> Option<Self::Item> {
        for entry_res in self.entries.by_ref() {
            let mut entry = match entry_res {
                Ok(e) => e,
                Err(err) => return Some(Err(IngestError::Io(err))),
            };
            match probe_tar_entry(&mut entry) {
                Ok(ArchiveEntryOutcome::PaxHeader(sha)) => {
                    if self.seen_items {
                        return Some(Err(IngestError::InvalidMetadata(
                            "pax header after archive entries".into(),
                        )));
                    }
                    self.parsed_revision = Some(sha);
                }
                Ok(ArchiveEntryOutcome::Item(item)) => {
                    self.seen_items = true;
                    return Some(Ok(item));
                }
                Ok(ArchiveEntryOutcome::Ignored) => {}
                Err(err) => return Some(Err(err)),
            }
        }
        None
    }
}

impl<'a, R: Read> SourceStream for GitHubStream<'a, R> {
    fn label(&self) -> &str {
        &self.label
    }

    fn revision(&self) -> Option<&str> {
        self.parsed_revision
            .as_deref()
            .or(self.reference.as_deref())
    }
}

enum ArchiveEntryOutcome {
    PaxHeader(String),
    Item(Item),
    Ignored,
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
    if !matches!(entry_type, EntryType::Regular | EntryType::Continuous) {
        return Ok(ArchiveEntryOutcome::Ignored);
    }

    // 3. Path decoding & root stripping
    let path_bytes = entry.path_bytes();
    let Some(rel_bytes) = strip_archive_root(&path_bytes) else {
        // Root directory entry itself (e.g. "{owner}-{repo}-{sha}/")
        return Ok(ArchiveEntryOutcome::Ignored);
    };

    if rel_bytes.is_empty()
        || rel_bytes
            .split(|&b| b == b'/')
            .filter(|part| !part.is_empty())
            .any(|part| part.starts_with(b"."))
    {
        return Ok(ArchiveEntryOutcome::Ignored);
    }

    let Some(rel_str) = crate::core::decide::text_str(rel_bytes) else {
        return Ok(ArchiveEntryOutcome::Item(Item {
            path: ItemPath::NonUtf8,
            probe: Probe::Missing,
        }));
    };
    let rel_path = rel_str.to_owned();

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

        let mut archive = Archive::new(GzDecoder::new(mock_data.as_slice()));
        let stream = GitHubStream::new(&mut archive, "but212", "codefeed", None)
            .expect("stream should open");
        let result =
            crate::shell::orchestrate::collect_stream(stream).expect("collection should succeed");

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

        let mut archive = Archive::new(GzDecoder::new(mock_data.as_slice()));
        let stream = GitHubStream::new(&mut archive, "owner", "repo", Some("main"))
            .expect("stream should open");
        let result =
            crate::shell::orchestrate::collect_stream(stream).expect("collection should succeed");

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

        let mut archive = Archive::new(GzDecoder::new(mock_data.as_slice()));
        let stream =
            GitHubStream::new(&mut archive, "owner", "repo", None).expect("stream should open");
        let result =
            crate::shell::orchestrate::collect_stream(stream).expect("collection should succeed");

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

        let mut archive = Archive::new(GzDecoder::new(mock_data.as_slice()));
        let stream =
            GitHubStream::new(&mut archive, "owner", "repo", None).expect("stream should open");
        let result =
            crate::shell::orchestrate::collect_stream(stream).expect("collection should succeed");

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

        let mut archive = Archive::new(GzDecoder::new(mock_data.as_slice()));
        let stream =
            GitHubStream::new(&mut archive, "owner", "repo", None).expect("stream should open");
        let result =
            crate::shell::orchestrate::collect_stream(stream).expect("collection should succeed");

        assert_eq!(result.files.len(), 0);
        assert_eq!(
            result.skipped, 0,
            "hidden entries must never be accounted as candidates or skipped"
        );
    }

    #[test]
    fn rejects_out_of_order_pax_header() {
        let pax_comment = b"52 comment=c1863c8b05c5d2db4030bc8c8a7d2f10bda4a950\n";
        let mock_data = create_mock_tarball(&[
            ("repo-root/file.txt", b"content", EntryType::Regular),
            ("pax_global_header", pax_comment, EntryType::XGlobalHeader),
        ]);

        let mut archive = Archive::new(GzDecoder::new(mock_data.as_slice()));
        let stream =
            GitHubStream::new(&mut archive, "owner", "repo", None).expect("stream should open");
        let result = crate::shell::orchestrate::collect_stream(stream);

        assert!(matches!(
            result,
            Err(IngestError::InvalidMetadata(msg)) if msg.contains("pax header after archive entries")
        ));
    }
}
