//! Tarball archive streaming ingestion and entry admission.

use std::cell::Cell;
use std::io::{self, BufReader, Read};

use flate2::read::GzDecoder;
use tar::{Archive, EntryType};

use crate::core::collection::CollectionResult;
use crate::core::error::DomainError;
use crate::core::github::{normalize_tar_entry_path, parse_pax_comment};
use crate::core::policy::{
    ARCHIVE_BUFFER_BYTES, ARCHIVE_MAX_DECOMPRESSED_BYTES, ARCHIVE_MAX_HEADER_BYTES,
    ARCHIVE_PAX_GLOBAL_BYTES, ExcludePatterns, MAX_FILE_SIZE,
};
use crate::core::selector::Selector;
use crate::shell::error::{IngestError, transport_error};

/// Collects files from a tarball reader directly into a [`CollectionResult`].
///
/// Decompresses and streams entries through a tuned [`ARCHIVE_BUFFER_BYTES`] buffer,
/// extracts commit revision from PAX global headers if present (falling back to `default_reference`),
/// filters non-regular, user-excluded, or sensitive entries via core policies, and feeds valid candidates to [`Selector`].
pub(in crate::shell) fn collect<R: Read>(
    reader: R,
    label: &str,
    default_reference: Option<&str>,
    exclude: &ExcludePatterns,
) -> Result<CollectionResult, IngestError> {
    let meter = StreamMeter::new();
    let mut archive = Archive::new(Metered {
        inner: GzDecoder::new(BufReader::with_capacity(ARCHIVE_BUFFER_BYTES, reader)),
        meter: &meter,
    });
    let mut entries = archive
        .entries()
        .map_err(|err: io::Error| stream_error(&err, &meter))?;
    let mut parsed_revision = None;
    let mut selector = Selector::new();
    let mut content_scratch = Vec::with_capacity(32 * 1024);

    loop {
        meter.arm();
        let next = entries.next();
        meter.disarm();
        let Some(entry_res) = next else { break };
        let mut entry = entry_res.map_err(|err: io::Error| stream_error(&err, &meter))?;
        content_scratch.clear();
        if entry.header().entry_type() == EntryType::XGlobalHeader {
            (&mut entry)
                .take(ARCHIVE_PAX_GLOBAL_BYTES)
                .read_to_end(&mut content_scratch)
                .map_err(|err: io::Error| stream_error(&err, &meter))?;
            parsed_revision = parsed_revision.or_else(|| parse_pax_comment(&content_scratch));
        } else {
            selector =
                process_tar_entry(&mut entry, selector, &mut content_scratch, exclude, &meter)?;
        }
        drain_entry(&mut entry, &meter)?;
    }

    let revision = parsed_revision.or_else(|| default_reference.map(str::to_string));
    Ok(selector.finish(label.to_string(), revision)?)
}

/// Which stream limit a read overran. Recorded on the [`StreamMeter`] so the failure cannot be mistaken
/// for a transport error once tar or gzip wrap the `io::Error`.
#[derive(Debug, Clone, Copy)]
enum Overrun {
    /// One `Entries::next` call read more than `ARCHIVE_MAX_HEADER_BYTES` (I-03).
    Header,
    /// The decompressed stream is longer than `ARCHIVE_MAX_DECOMPRESSED_BYTES`.
    Decompressed,
}

impl Overrun {
    const fn message(self) -> &'static str {
        match self {
            Self::Header => "tar entry header exceeds ARCHIVE_MAX_HEADER_BYTES",
            Self::Decompressed => "decompressed archive exceeds ARCHIVE_MAX_DECOMPRESSED_BYTES",
        }
    }
}

/// Byte meters on the decompressed stream. The header budget (I-03) is armed before an `Entries::next`
/// call and disarmed after it, so only the reads tar makes for headers and extension bodies count
/// against it. The total counts every read, entry bodies included.
struct StreamMeter {
    /// Bytes left for the armed call, or `None` while disarmed (unmetered).
    header_remaining: Cell<Option<u64>>,
    /// Decompressed bytes read so far.
    total: Cell<u64>,
    overrun: Cell<Option<Overrun>>,
}

impl StreamMeter {
    const fn new() -> Self {
        Self {
            header_remaining: Cell::new(None),
            total: Cell::new(0),
            overrun: Cell::new(None),
        }
    }

    fn arm(&self) {
        self.header_remaining.set(Some(ARCHIVE_MAX_HEADER_BYTES));
    }

    fn disarm(&self) {
        self.header_remaining.set(None);
    }

    /// Records `overrun` and returns the error that stops the read.
    fn fail(&self, overrun: Overrun) -> io::Error {
        self.overrun.set(Some(overrun));
        io::Error::other(overrun.message())
    }

    /// Largest read allowed now. The total allows one byte past its limit, so an overrun is detected
    /// rather than mistaken for end of stream.
    fn read_cap(&self, requested: usize) -> io::Result<usize> {
        let header = match self.header_remaining.get() {
            Some(0) if requested > 0 => return Err(self.fail(Overrun::Header)),
            Some(remaining) => remaining,
            None => u64::MAX,
        };
        let total = ARCHIVE_MAX_DECOMPRESSED_BYTES - self.total.get() + 1;
        let allowed = header.min(total).min(requested as u64);
        // `allowed <= requested`, so it fits in `usize`.
        Ok(usize::try_from(allowed).unwrap_or(requested))
    }

    /// Accounts for `read` bytes just read.
    fn record(&self, read: usize) -> io::Result<()> {
        let read = read as u64;
        let total = self.total.get() + read;
        self.total.set(total);
        if total > ARCHIVE_MAX_DECOMPRESSED_BYTES {
            return Err(self.fail(Overrun::Decompressed));
        }
        if let Some(remaining) = self.header_remaining.get() {
            self.header_remaining.set(Some(remaining - read));
        }
        Ok(())
    }
}

/// Reader that applies a [`StreamMeter`] to the decompressed stream.
struct Metered<'a, R> {
    inner: R,
    meter: &'a StreamMeter,
}

impl<R: Read> Read for Metered<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let cap = self.meter.read_cap(buf.len())?;
        let read = self.inner.read(&mut buf[..cap])?;
        self.meter.record(read)?;
        Ok(read)
    }
}

/// Maps a failure while streaming the archive to a run error. A limit overrun is a policy violation
/// (for a header, the entry's path is never parsed, so it cannot be counted as skipped). Any other
/// `io::Error` here comes from the network body, gzip, or tar decoding, so it never surfaces as a
/// filesystem error.
fn stream_error(err: &io::Error, meter: &StreamMeter) -> IngestError {
    meter.overrun.get().map_or_else(
        || transport_error(&err.to_string()),
        |overrun| DomainError::invalid_metadata(overrun.message()).into(),
    )
}

/// Discards the unread rest of an entry body, so the next header read starts at its padding.
fn drain_entry<R: Read>(entry: &mut tar::Entry<R>, meter: &StreamMeter) -> Result<(), IngestError> {
    io::copy(entry, &mut io::sink()).map_err(|err: io::Error| stream_error(&err, meter))?;
    Ok(())
}

fn process_tar_entry<R: Read>(
    entry: &mut tar::Entry<R>,
    selector: Selector,
    content_scratch: &mut Vec<u8>,
    exclude: &ExcludePatterns,
    meter: &StreamMeter,
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
    let Some(candidate) = normalize_tar_entry_path(&path_bytes, exclude)? else {
        return Ok(selector);
    };

    let size_hint = Some(entry.size());

    // 3. Pre-I/O candidate admission: the body is read only if the candidate is admitted
    selector.observe(candidate, size_hint, move || {
        entry
            .take(MAX_FILE_SIZE + 1)
            .read_to_end(content_scratch)
            .map_err(|err: io::Error| stream_error(&err, meter))?;
        Ok(Some(content_scratch))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::error::DomainError;
    use crate::core::policy::MAX_FILE_SIZE_USIZE;
    use crate::shell::testing::{gzip_archive, gzip_single_entry_stream, gzip_tarball};

    fn collect_default(
        reader: &[u8],
        label: &str,
        default_reference: Option<&str>,
    ) -> Result<CollectionResult, IngestError> {
        collect(
            reader,
            label,
            default_reference,
            &ExcludePatterns::default(),
        )
    }

    #[test]
    fn user_excluded_entries_are_not_counted_as_skipped() {
        // The binary body would count as skipped if the entry were admitted as a candidate.
        let mock_data = gzip_tarball(&[
            ("repo-root/build/out.bin", b"\0\x01\x02", EntryType::Regular),
            ("repo-root/src/main.rs", b"fn main() {}", EntryType::Regular),
        ]);
        let exclude = ExcludePatterns::new(["build/"]).unwrap();

        let result = collect(
            mock_data.as_slice(),
            "https://github.com/owner/repo",
            None,
            &exclude,
        )
        .expect("collection should succeed");

        let paths: Vec<&str> = result.files.iter().map(|f| f.path).collect();
        assert_eq!(paths, vec!["src/main.rs"]);
        assert_eq!(result.skipped, 0);
    }

    #[test]
    fn user_exclusion_beyond_max_path_bytes_is_not_counted_as_skipped() {
        // Twenty-one bytes per directory: the excluded entry is over `MAX_PATH_BYTES`, so without the
        // exclusion it would take a candidate slot and count as skipped.
        let dir = "d".repeat(20);
        let deep = format!("repo-root/{}/x.log", vec![dir.as_str(); 220].join("/"));
        let mock_data = gzip_tarball(&[
            (deep.as_str(), b"log line", EntryType::Regular),
            ("repo-root/src/main.rs", b"fn main() {}", EntryType::Regular),
        ]);
        let exclude = ExcludePatterns::new(["*.log"]).unwrap();

        let result = collect(
            mock_data.as_slice(),
            "https://github.com/owner/repo",
            None,
            &exclude,
        )
        .expect("collection should succeed");

        let paths: Vec<&str> = result.files.iter().map(|f| f.path).collect();
        assert_eq!(paths, vec!["src/main.rs"]);
        assert_eq!(result.skipped, 0);
    }

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

        let result = collect_default(
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
        let mock_data = gzip_tarball(&[
            ("repo-root/small.txt", b"small", EntryType::Regular),
            (
                "repo-root/large.txt",
                &vec![b'x'; MAX_FILE_SIZE_USIZE + 10],
                EntryType::Regular,
            ),
        ]);

        let result = collect_default(
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

        let result = collect_default(
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

        let result = collect_default(mock_data.as_slice(), "https://github.com/owner/repo", None)
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

        let result = collect_default(mock_data.as_slice(), "https://github.com/owner/repo", None)
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

        let result = collect_default(mock_data.as_slice(), "https://github.com/owner/repo", None)
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

        let result = collect_default(mock_data.as_slice(), "https://github.com/owner/repo", None)
            .expect("collection should succeed");

        assert_eq!(
            result.revision.as_deref(),
            Some("c1863c8b05c5d2db4030bc8c8a7d2f10bda4a950")
        );
        assert_eq!(result.files.len(), 1);
    }

    #[test]
    fn colliding_entry_paths_fail_the_run() {
        type TarEntry<'a> = (&'a str, &'a [u8], EntryType);
        // The oversized second entry is skipped for size, but its path still collides with the stored first entry.
        let oversized = vec![b'x'; MAX_FILE_SIZE_USIZE + 1];
        // (case, entries): every case has two entries that collide in the digest tree.
        let cases: [(&str, Vec<TarEntry<'_>>); 4] = [
            (
                "across repo roots",
                vec![
                    ("r1/a.txt", &b"first"[..], EntryType::Regular),
                    ("r2/a.txt", &b"second"[..], EntryType::Regular),
                ],
            ),
            (
                "after normalization",
                vec![
                    ("repo/a//b.txt", &b"first"[..], EntryType::Regular),
                    ("repo/a/b.txt", &b"second"[..], EntryType::Regular),
                ],
            ),
            (
                "skipped candidate",
                vec![
                    ("repo/a.txt", &b"first"[..], EntryType::Regular),
                    ("repo/a.txt", oversized.as_slice(), EntryType::Regular),
                ],
            ),
            (
                "file and directory",
                vec![
                    ("repo-root/a", &b"file"[..], EntryType::Regular),
                    ("repo-root/a/b", &b"nested"[..], EntryType::Regular),
                ],
            ),
        ];
        for (case, entries) in cases {
            let mock_data = gzip_tarball(&entries);
            let err = collect_default(mock_data.as_slice(), "https://github.com/owner/repo", None)
                .unwrap_err();
            assert!(
                matches!(&err, IngestError::Domain(DomainError::InvalidMetadata(_))),
                "{case}: expected InvalidMetadata, got: {err:?}"
            );
        }
    }

    #[test]
    fn skipped_file_with_nested_path_does_not_fail_the_run() {
        // The binary `a` is skipped, so it is not stored and does not collide with `a/b`.
        let mock_data = gzip_tarball(&[
            ("repo-root/a", b"\0\x01\x02", EntryType::Regular),
            ("repo-root/a/b", b"nested", EntryType::Regular),
        ]);

        let result = collect_default(mock_data.as_slice(), "https://github.com/owner/repo", None)
            .expect("collection should succeed");

        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files.iter().next().unwrap().path, "a/b");
        assert_eq!(result.skipped, 1);
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

        let result = collect_default(mock_data.as_slice(), "https://github.com/owner/repo", None)
            .expect("collection should succeed");

        assert_eq!(result.files.len(), 0);
        assert_eq!(result.skipped, 0);
    }

    #[test]
    fn stream_decode_failure_is_http() {
        let Err(err) = collect_default(&b"not gzip"[..], "x", None) else {
            panic!("expected error");
        };
        assert!(matches!(err, IngestError::Http(_)), "{err}");
        assert!(err.to_string().starts_with("GitHub request failed:"));
    }

    #[test]
    fn path_over_max_path_bytes_within_header_budget_is_skipped() {
        // A 5 000-byte path needs a `L` header that fits the budget, so it is skipped rather than failed.
        let long_name = format!("repo-root/{}", "a".repeat(5_000));
        let mock_data = gzip_tarball(&[
            (long_name.as_str(), b"content", EntryType::Regular),
            ("repo-root/src/main.rs", b"fn main() {}", EntryType::Regular),
        ]);

        let result = collect_default(mock_data.as_slice(), "https://github.com/owner/repo", None)
            .expect("collection should succeed");

        let paths: Vec<&str> = result.files.iter().map(|f| f.path).collect();
        assert_eq!(paths, vec!["src/main.rs"]);
        assert_eq!(result.skipped, 1);
    }

    #[test]
    fn entry_path_past_max_depth_fails_the_run() {
        let deep_path = format!(
            "repo-root/{}/file.txt",
            vec!["d"; crate::core::policy::MAX_PATH_DEPTH + 1].join("/")
        );
        let mock_data = gzip_tarball(&[(deep_path.as_str(), b"content", EntryType::Regular)]);

        let err = collect_default(mock_data.as_slice(), "https://github.com/owner/repo", None)
            .unwrap_err();
        assert!(
            matches!(&err, IngestError::Domain(DomainError::InvalidMetadata(msg)) if msg.contains("maximum directory depth")),
            "expected depth failure, got: {err:?}"
        );
    }

    /// Yields `data`, then fails the way a dropped connection does.
    struct DroppedAfter<'a> {
        data: &'a [u8],
    }

    impl Read for DroppedAfter<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.data.is_empty() {
                return Err(io::Error::other("connection reset"));
            }
            let n = buf.len().min(self.data.len());
            buf[..n].copy_from_slice(&self.data[..n]);
            self.data = &self.data[n..];
            Ok(n)
        }
    }

    #[test]
    fn stream_cut_at_any_offset_never_yields_partial_output() {
        // A PAX global header body and a multi-block file body are both read by the loop, so every
        // read site sees the cut.
        let body: Vec<u8> = (0..2048u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13).to_le_bytes()[0])
            .collect();
        let mock_data = gzip_tarball(&[
            (
                "pax_global_header",
                b"52 comment=c1863c8b05c5d2db4030bc8c8a7d2f10bda4a950\n",
                EntryType::XGlobalHeader,
            ),
            ("repo-root/src/main.rs", body.as_slice(), EntryType::Regular),
        ]);

        let full = collect_default(mock_data.as_slice(), "https://github.com/owner/repo", None)
            .expect("intact stream collects");
        let expected_files = full.files.len();

        for cut in 0..mock_data.len() {
            let reader = DroppedAfter {
                data: &mock_data[..cut],
            };
            match collect(
                reader,
                "https://github.com/owner/repo",
                None,
                &ExcludePatterns::default(),
            ) {
                Err(IngestError::Http(_)) => {}
                // The tar end marker can sit before the last gzip bytes, so a cut there completes the run.
                Ok(partial) => {
                    assert_eq!(partial.files.len(), expected_files, "cut at {cut}");
                    assert_eq!(
                        partial.files.total_bytes(),
                        full.files.total_bytes(),
                        "cut at {cut}"
                    );
                }
                Err(other) => panic!("cut at {cut}: unexpected error {other:?}"),
            }
        }
    }

    /// Asserts that the run fails because a header exceeds the `ARCHIVE_MAX_HEADER_BYTES` budget.
    fn assert_header_budget_failure(mock_data: &[u8]) {
        let err = collect_default(mock_data, "https://github.com/owner/repo", None).unwrap_err();
        assert!(
            matches!(&err, IngestError::Domain(DomainError::InvalidMetadata(msg)) if msg.contains("ARCHIVE_MAX_HEADER_BYTES")),
            "expected header budget failure, got: {err:?}"
        );
    }

    #[test]
    fn oversized_gnu_long_name_header_fails_the_run() {
        // tar reads the `L` extension body whole before the path reaches policy, so the header budget bounds it.
        let long_name = format!("repo-root/{}", "a".repeat(70_000));
        let mock_data = gzip_tarball(&[(long_name.as_str(), b"content", EntryType::Regular)]);

        assert_header_budget_failure(&mock_data);
    }

    #[test]
    fn oversized_pax_header_fails_the_run() {
        // The PAX `x` body is read whole by tar, so an oversized path record fails the run.
        let long_path = format!("repo-root/{}", "a".repeat(70_000));
        let mock_data = gzip_archive(|tar| {
            tar.append_pax_extensions([("path", long_path.as_bytes())])
                .unwrap();
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(EntryType::Regular);
            header.set_size(7);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, "repo-root/x.txt", &b"content"[..])
                .unwrap();
        });

        assert_header_budget_failure(&mock_data);
    }

    #[test]
    fn oversized_header_of_hidden_entry_fails_the_run() {
        // The budget runs before classification, so a hidden entry that policy would drop still fails the run.
        let hidden_name = format!("repo-root/.cache/{}/f.txt", "a".repeat(70_000));
        let mock_data = gzip_tarball(&[
            (hidden_name.as_str(), b"content", EntryType::Regular),
            ("repo-root/src/main.rs", b"fn main() {}", EntryType::Regular),
        ]);

        assert_header_budget_failure(&mock_data);
    }

    /// Body length that makes the single-entry stream exactly `extra` bytes past the decompressed limit
    /// (negative `extra` is short of it). The 512-byte tar header counts toward the limit.
    fn body_len_for_stream_len(extra: i64) -> u64 {
        ARCHIVE_MAX_DECOMPRESSED_BYTES
            .checked_add_signed(extra - 512)
            .expect("limit exceeds the header length")
    }

    #[test]
    fn decompressed_stream_at_the_limit_is_accepted() {
        // One oversized entry is skipped, but its body is still drained from the stream.
        let stream = gzip_single_entry_stream(body_len_for_stream_len(0));

        let result = collect(stream, "label", None, &ExcludePatterns::default())
            .expect("a stream of exactly the limit is accepted");

        assert_eq!(result.files.len(), 0);
        assert_eq!(result.skipped, 1);
    }

    #[test]
    fn decompressed_stream_one_byte_beyond_the_limit_fails_the_run() {
        let stream = gzip_single_entry_stream(body_len_for_stream_len(1));

        let err = collect(stream, "label", None, &ExcludePatterns::default()).unwrap_err();
        assert!(
            matches!(&err, IngestError::Domain(DomainError::InvalidMetadata(msg)) if msg.contains("ARCHIVE_MAX_DECOMPRESSED_BYTES")),
            "expected decompressed limit failure, got: {err:?}"
        );
    }

    #[test]
    fn file_of_exactly_the_size_limit_is_read_in_full() {
        let mock_data = gzip_tarball(&[(
            "repo-root/exact.txt",
            &vec![b'x'; MAX_FILE_SIZE_USIZE],
            EntryType::Regular,
        )]);

        let result = collect_default(&mock_data, "label", None).expect("collection succeeds");

        let files: Vec<_> = result.files.iter().collect();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "exact.txt");
        assert_eq!(files[0].content.len(), MAX_FILE_SIZE_USIZE);
        assert_eq!(result.skipped, 0);
    }
}
