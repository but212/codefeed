//! Tarball archive streaming ingestion and entry admission.

use std::cell::Cell;
use std::io::{self, BufReader, Read};

use flate2::bufread::MultiGzDecoder;
use tar::{Archive, EntryType};

use crate::core::error::{DomainError, ErrorDetail};
use crate::core::github::parse_pax_comment;
use crate::core::model::CollectionResult;
use crate::core::path::{Candidate, normalize_tar_entry_path};
use crate::core::policy::{
    ARCHIVE_BUFFER_BYTES, ARCHIVE_MAX_DECOMPRESSED_BYTES, ARCHIVE_MAX_HEADER_BYTES,
    ARCHIVE_PAX_GLOBAL_BYTES, ExcludePatterns, MAX_FILE_SIZE,
};
use crate::core::selector::{Body, Selector};
use crate::shell::error::IngestError;

/// Collects files from a tarball reader into a [`CollectionResult`].
///
/// Streams entries via [`ARCHIVE_BUFFER_BYTES`], reads PAX global commit revisions (falling back to
/// `default_reference`), filters entries via core policies, and feeds candidates to [`Selector`].
pub(in crate::shell) fn collect<R: Read>(
    reader: R,
    label: impl Into<String>,
    default_reference: Option<&str>,
    exclude: &ExcludePatterns,
) -> Result<CollectionResult, IngestError> {
    let meter = StreamMeter::new();
    let mut archive = Archive::new(Metered {
        inner: MultiGzDecoder::new(BufReader::with_capacity(ARCHIVE_BUFFER_BYTES, reader)),
        meter: &meter,
    });
    let mut entries = archive
        .entries()
        .map_err(|err: io::Error| meter.stream_error(&err))?;
    let mut parsed_revision = None;
    let mut selector = Selector::new();
    let mut content_scratch = Vec::with_capacity(32 * 1024);

    loop {
        meter.arm();
        let next = entries.next();
        meter.disarm();
        let Some(entry_res) = next else { break };
        let mut entry = entry_res.map_err(|err: io::Error| meter.stream_error(&err))?;
        content_scratch.clear();
        match entry.header().entry_type() {
            EntryType::XGlobalHeader => {
                (&mut entry)
                    .take(ARCHIVE_PAX_GLOBAL_BYTES)
                    .read_to_end(&mut content_scratch)
                    .map_err(|err: io::Error| meter.stream_error(&err))?;
                parsed_revision = parsed_revision.or_else(|| parse_pax_comment(&content_scratch));
            }
            EntryType::Regular | EntryType::Continuous => {
                selector = match normalize_tar_entry_path(&entry.path_bytes(), exclude)? {
                    Some(Candidate::Unstorable(path)) => selector.admit_unstorable(path)?,
                    Some(Candidate::Valid(path, ())) => {
                        let pending = selector.admit_file(path)?;
                        let body = if entry.size() > MAX_FILE_SIZE {
                            Body::OverLimit
                        } else {
                            (&mut entry)
                                .take(MAX_FILE_SIZE + 1)
                                .read_to_end(&mut content_scratch)
                                .map_err(|err: io::Error| meter.stream_error(&err))?;
                            Body::Bytes(content_scratch.as_slice())
                        };
                        pending.settle(body)?
                    }
                    None => selector,
                };
            }
            // Skip directories, symlinks, and other special entries before path decoding.
            _ => {}
        }
        drain_entry(&mut entry, &meter)?;
    }
    // Tar stops at its end marker, before gzip necessarily checks every member's checksum and size.
    // Keep all remaining members inside the stream budget while validating their trailers.
    io::copy(&mut archive.into_inner(), &mut io::sink()).map_err(|err| meter.stream_error(&err))?;

    let revision = parsed_revision.or_else(|| default_reference.map(str::to_string));
    Ok(selector.finish(label.into(), revision)?)
}

/// Which stream limit a read overran. Recorded on the [`StreamMeter`] so the failure cannot be mistaken
/// for a transport error once tar or gzip wrap the `io::Error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

    fn into_error(self) -> IngestError {
        DomainError::invalid_metadata(self.message()).into()
    }

    fn into_io_error(self) -> io::Error {
        io::Error::other(self.message())
    }
}

/// Active metering state (`total <= ARCHIVE_MAX_DECOMPRESSED_BYTES`) or a latched overrun.
#[derive(Clone, Copy)]
enum MeterState {
    Active {
        /// Bytes left for the armed call, or `None` while disarmed (unmetered).
        header_remaining: Option<u64>,
        /// Decompressed bytes and reconstructed sparse holes charged so far.
        total: u64,
    },
    Overrun(Overrun),
}

/// Byte meters on the decompressed stream. The header budget (I-03) is armed before an `Entries::next`
/// call and disarmed after it, so only the reads tar makes for headers and extension bodies count
/// against it. The total counts physical reads and reconstructed sparse holes.
struct StreamMeter {
    state: Cell<MeterState>,
}

impl StreamMeter {
    const fn new() -> Self {
        Self {
            state: Cell::new(MeterState::Active {
                header_remaining: None,
                total: 0,
            }),
        }
    }

    fn arm(&self) {
        if let MeterState::Active { total, .. } = self.state.get() {
            self.state.set(MeterState::Active {
                header_remaining: Some(ARCHIVE_MAX_HEADER_BYTES),
                total,
            });
        }
    }

    fn disarm(&self) {
        if let MeterState::Active { total, .. } = self.state.get() {
            self.state.set(MeterState::Active {
                header_remaining: None,
                total,
            });
        }
    }

    /// Records `overrun` and returns it.
    fn fail(&self, overrun: Overrun) -> Overrun {
        self.state.set(MeterState::Overrun(overrun));
        overrun
    }

    fn active_total(&self) -> Result<u64, Overrun> {
        match self.state.get() {
            MeterState::Active { total, .. } => Ok(total),
            MeterState::Overrun(overrun) => Err(overrun),
        }
    }

    /// Maps a failure while streaming the archive to a run error. A limit overrun is a policy violation
    /// (for a header, the entry's path is never parsed, so it cannot be counted as skipped). Any other
    /// `io::Error` here comes from the network body, gzip, or tar decoding, so it never surfaces as a
    /// filesystem error.
    fn stream_error(&self, err: &io::Error) -> IngestError {
        match self.state.get() {
            MeterState::Overrun(overrun) => overrun.into_error(),
            MeterState::Active { .. } => IngestError::Http(ErrorDetail::new(err.to_string())),
        }
    }

    /// Largest read allowed now. The total allows one byte past its limit, so an overrun is detected
    /// rather than mistaken for end of stream.
    fn read_cap(&self, requested: usize) -> Result<usize, Overrun> {
        let (header_remaining, total) = match self.state.get() {
            MeterState::Active {
                header_remaining,
                total,
            } => (header_remaining, total),
            MeterState::Overrun(overrun) => return Err(overrun),
        };
        let total_cap = (ARCHIVE_MAX_DECOMPRESSED_BYTES - total) + 1;
        let cap = match header_remaining {
            Some(0) if requested > 0 => return Err(self.fail(Overrun::Header)),
            Some(remaining) => remaining.min(total_cap),
            None => total_cap,
        };
        let allowed = cap.min(requested as u64);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "allowed <= requested as u64, so it fits in usize"
        )]
        Ok(allowed as usize)
    }

    /// Accounts for physical bytes read or sparse holes reconstructed.
    fn record(&self, read: u64) -> Result<(), Overrun> {
        let (header_remaining, total) = match self.state.get() {
            MeterState::Active {
                header_remaining,
                total,
            } => (header_remaining, total),
            MeterState::Overrun(overrun) => return Err(overrun),
        };
        if read > ARCHIVE_MAX_DECOMPRESSED_BYTES - total {
            return Err(self.fail(Overrun::Decompressed));
        }
        self.state.set(MeterState::Active {
            header_remaining: header_remaining.map(|remaining| remaining - read),
            total: total + read,
        });
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
        let cap = self
            .meter
            .read_cap(buf.len())
            .map_err(Overrun::into_io_error)?;
        let read = self.inner.read(&mut buf[..cap])?;
        self.meter
            .record(read as u64)
            .map_err(Overrun::into_io_error)?;
        Ok(read)
    }
}

/// Discards the unread body. Sparse holes count toward the stream budget even though tar generates
/// them without reading the underlying stream. Sparse entries are never read before this call.
fn drain_entry<R: Read>(entry: &mut tar::Entry<R>, meter: &StreamMeter) -> Result<(), IngestError> {
    let sparse = entry.header().entry_type() == EntryType::GNUSparse;
    let before = meter.active_total().map_err(Overrun::into_error)?;
    if sparse && entry.size() > ARCHIVE_MAX_DECOMPRESSED_BYTES - before {
        return Err(meter.fail(Overrun::Decompressed).into_error());
    }
    let drained =
        io::copy(entry, &mut io::sink()).map_err(|err: io::Error| meter.stream_error(&err))?;
    if sparse {
        // Physical data was already charged by Metered; charge only the synthesized holes.
        let physical = meter.active_total().map_err(Overrun::into_error)? - before;
        meter
            .record(drained - physical)
            .map_err(Overrun::into_error)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::error::DomainError;
    use crate::core::policy::MAX_FILE_SIZE_USIZE;
    use crate::shell::testing::{gzip_archive, gzip_tarball};

    #[test]
    fn read_cap_after_overrun_fails_closed() {
        let meter = StreamMeter::new();
        let cap = meter.read_cap(usize::MAX).unwrap();
        assert_eq!(meter.record(cap as u64), Err(Overrun::Decompressed));
        assert_eq!(meter.read_cap(1), Err(Overrun::Decompressed));
        meter.disarm();
        meter.arm();
        assert_eq!(meter.read_cap(1), Err(Overrun::Decompressed));
        assert!(matches!(
            meter.stream_error(&io::Error::other("wrapped")),
            IngestError::Domain(DomainError::InvalidMetadata(_))
        ));
    }

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
    fn skips_too_large_files_and_reads_exact_limit_in_full() {
        let mock_data = gzip_tarball(&[
            (
                "repo-root/exact.txt",
                &vec![b'x'; MAX_FILE_SIZE_USIZE],
                EntryType::Regular,
            ),
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

        let files: Vec<_> = result.files.iter().collect();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "exact.txt");
        assert_eq!(files[0].content.len(), MAX_FILE_SIZE_USIZE);
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
    fn non_regular_entries_are_dropped_before_path_normalization() {
        // Pins the documented order: entry type filtering runs before tar path decoding and
        // normalization, so non-regular entries with non-UTF-8 or traversal paths are ignored.
        let mock_data = gzip_archive(|tar| {
            for (kind, mode, name) in [
                (EntryType::Symlink, 0o777, &b"repo-root/\xff_symlink"[..]),
                (EntryType::Directory, 0o755, &b"repo-root/\xff_dir/"[..]),
                (EntryType::Directory, 0o755, &b"repo-root/../x/"[..]),
                (EntryType::Symlink, 0o777, &b"repo-root/../y"[..]),
            ] {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(kind);
                header.set_size(0);
                header.set_mode(mode);
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
        let mock_data = gzip_tarball(&[
            ("r1/a.txt", &b"first"[..], EntryType::Regular),
            ("r2/a.txt", &b"second"[..], EntryType::Regular),
        ]);
        let err = collect_default(mock_data.as_slice(), "https://github.com/owner/repo", None)
            .unwrap_err();
        assert!(
            matches!(&err, IngestError::Domain(DomainError::InvalidMetadata(_))),
            "across repo roots: expected InvalidMetadata, got: {err:?}"
        );
    }

    /// Builds GNU long-name records so raw non-UTF-8 paths work on every host OS.
    fn gzip_raw_paths(paths: &[&[u8]]) -> Vec<u8> {
        gzip_archive(|tar| {
            for &path in paths {
                let mut long = tar::Header::new_gnu();
                long.set_entry_type(EntryType::GNULongName);
                long.set_size(path.len() as u64 + 1);
                long.set_cksum();
                tar.append(&long, path.chain(io::repeat(0).take(1)))
                    .unwrap();

                let mut file = tar::Header::new_gnu();
                file.set_size(1);
                file.set_cksum();
                tar.append_data(&mut file, "repo-root/placeholder", &b"x"[..])
                    .unwrap();
            }
        })
    }

    #[test]
    fn unstorable_paths_forward_to_selector() {
        let distinct = gzip_raw_paths(&[b"repo/src/\xff/a", b"repo/src/\xff/b"]);
        let result = collect_default(&distinct, "test", None).unwrap();
        assert_eq!(result.files.len(), 0);
        assert_eq!(result.skipped, 2);

        let duplicate = gzip_raw_paths(&[b"r1/src/\xff/file", b"r2//src/./\xff/file"]);
        let err = collect_default(&duplicate, "test", None).unwrap_err();
        assert!(
            matches!(&err, IngestError::Domain(DomainError::InvalidMetadata(detail)) if detail.contains("duplicate entry path")),
            "{err:?}"
        );
    }

    fn sparse_archive(hole: u64) -> Vec<u8> {
        let mut header = tar::Header::new_gnu();
        header.set_path("repo-root/sparse").unwrap();
        header.set_entry_type(EntryType::GNUSparse);
        header.set_size(1);
        let gnu = header.as_gnu_mut().unwrap();
        gnu.set_real_size(hole + 1);
        gnu.sparse[0].set_offset(hole);
        gnu.sparse[0].set_length(1);
        header.set_cksum();
        let mut raw = header.as_bytes().to_vec();
        raw.push(b'x');
        raw.resize(2048, 0);
        raw
    }

    #[test]
    fn sparse_holes_are_charged_to_and_bounded_by_stream_budget() {
        let hole = MAX_FILE_SIZE;
        let raw = sparse_archive(hole);
        let run = |preset_total: Option<u64>| {
            let meter = StreamMeter::new();
            let mut archive = Archive::new(Metered {
                inner: raw.as_slice(),
                meter: &meter,
            });
            let mut entries = archive.entries().unwrap();
            meter.arm();
            let mut entry = entries.next().unwrap().unwrap();
            meter.disarm();
            if let Some(total) = preset_total {
                meter.state.set(MeterState::Active {
                    header_remaining: None,
                    total,
                });
            }
            drain_entry(&mut entry, &meter).map(|()| meter.active_total().unwrap())
        };

        assert_eq!(run(None).unwrap(), 512 + hole + 1);
        let err = run(Some(ARCHIVE_MAX_DECOMPRESSED_BYTES - hole / 2)).unwrap_err();
        assert!(matches!(
            err,
            IngestError::Domain(DomainError::InvalidMetadata(_))
        ));
    }

    #[test]
    fn sparse_entries_preserve_following_files_or_fail_before_expansion() {
        for hole in [
            0,
            MAX_FILE_SIZE,
            ARCHIVE_MAX_DECOMPRESSED_BYTES,
            1_u64 << 60,
        ] {
            let mut raw = sparse_archive(hole);
            raw.truncate(1024); // Keep the sparse header, physical body, and its padding.
            let mut tail = tar::Builder::new(Vec::new());
            let mut file = tar::Header::new_gnu();
            file.set_size(2);
            file.set_cksum();
            tail.append_data(&mut file, "repo-root/keep.txt", &b"ok"[..])
                .unwrap();
            raw.extend(tail.into_inner().unwrap());
            let gzip = flate2::read::GzEncoder::new(raw.as_slice(), flate2::Compression::fast());
            let result = collect(gzip, "test", None, &ExcludePatterns::default());
            if hole < ARCHIVE_MAX_DECOMPRESSED_BYTES {
                let collection = result.unwrap();
                assert_eq!(collection.skipped, 0);
                let files: Vec<_> = collection.files.iter().collect();
                assert_eq!(files.len(), 1);
                assert_eq!((files[0].path, files[0].content), ("keep.txt", "ok"));
            } else {
                assert!(matches!(
                    result,
                    Err(IngestError::Domain(DomainError::InvalidMetadata(_)))
                ));
            }
        }
    }

    fn gzip_members(parts: &[&[u8]]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for &part in parts {
            flate2::read::GzEncoder::new(part, flate2::Compression::fast())
                .read_to_end(&mut bytes)
                .unwrap();
        }
        bytes
    }

    #[test]
    fn archive_rejects_candidates_before_reading_their_bodies() {
        use crate::core::policy::MAX_FILE_COUNT;

        struct UnavailableBody;
        impl Read for UnavailableBody {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("body unavailable"))
            }
        }

        for (admitted, next) in [(1, 0), (MAX_FILE_COUNT, MAX_FILE_COUNT)] {
            let mut tar = tar::Builder::new(Vec::new());
            for i in 0..admitted {
                let mut header = tar::Header::new_gnu();
                header.set_size(0);
                tar.append_data(&mut header, format!("repo-root/{i}.txt"), &[][..])
                    .unwrap();
            }
            let mut header = tar::Header::new_gnu();
            header.set_size(1);
            tar.append_data(&mut header, format!("repo-root/{next}.txt"), &b"x"[..])
                .unwrap();
            let raw = tar.into_inner().unwrap();
            let headers = gzip_members(&[&raw[..(admitted + 1) * 512]]);
            let result = collect(
                headers.as_slice().chain(UnavailableBody),
                "fixture",
                None,
                &ExcludePatterns::default(),
            );
            let err = result.unwrap_err();
            if admitted == MAX_FILE_COUNT {
                assert!(
                    matches!(
                        err,
                        IngestError::Domain(DomainError::CandidateLimitExceeded(_))
                    ),
                    "{err}"
                );
            } else {
                assert!(
                    matches!(err, IngestError::Domain(DomainError::InvalidMetadata(_))),
                    "{err}"
                );
            }
        }
    }

    #[test]
    fn concatenated_gzip_members_preserve_files() {
        let expected = [("a.txt", "hello"), ("b.txt", "world")];
        let mut tar = tar::Builder::new(Vec::new());
        for (name, content) in expected {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_cksum();
            tar.append_data(&mut header, format!("repo-root/{name}"), content.as_bytes())
                .unwrap();
        }
        let raw = tar.into_inner().unwrap();
        // Cover boundaries between entries and within a header, body, padding, and end marker.
        for split in [1024, 0, 1, 513, 519, raw.len() - 1, raw.len()] {
            let bytes = gzip_members(&[&raw[..split], &raw[split..]]);
            let collection = collect_default(&bytes, "label", None)
                .unwrap_or_else(|error| panic!("split {split}: {error}"));
            let files: Vec<_> = collection
                .files
                .iter()
                .map(|f| (f.path, f.content))
                .collect();
            assert_eq!(files, expected, "split {split}");
            assert_eq!(collection.skipped, 0);
        }
    }

    #[test]
    fn concatenated_gzip_members_validate_later_trailers() {
        let first = gzip_tarball(&[("repo-root/a.txt", b"hello", EntryType::Regular)]);
        let intact = gzip_members(&[b"bytes after the tar end marker"]);
        let mut bad_crc = intact.clone();
        bad_crc[intact.len() - 8] ^= 1;
        let mut bad_size = intact.clone();
        bad_size[intact.len() - 4] ^= 1;
        for (case, later) in [
            ("checksum", bad_crc),
            ("size", bad_size),
            ("truncated", intact[..intact.len() - 1].to_vec()),
        ] {
            let result = collect(
                first.as_slice().chain(later.as_slice()),
                "label",
                None,
                &ExcludePatterns::default(),
            );
            assert!(
                matches!(result, Err(IngestError::Http(_))),
                "{case}: {result:?}"
            );
        }
    }

    #[test]
    fn concatenated_gzip_members_share_the_decompressed_stream_budget() {
        let first_len = ARCHIVE_MAX_DECOMPRESSED_BYTES / 2;
        let mut first = Vec::new();
        flate2::read::GzEncoder::new(io::repeat(0).take(first_len), flate2::Compression::fast())
            .read_to_end(&mut first)
            .unwrap();
        for extra in [0, 1] {
            // Each member fits separately; only their combined decoded size can exceed the limit.
            let later = flate2::read::GzEncoder::new(
                io::repeat(0).take(ARCHIVE_MAX_DECOMPRESSED_BYTES - first_len + extra),
                flate2::Compression::fast(),
            );
            let result = collect(
                first.as_slice().chain(later),
                "label",
                None,
                &ExcludePatterns::default(),
            );
            if extra == 0 {
                let collection = result.expect("members totaling exactly the stream limit fit");
                assert_eq!(collection.files.len(), 0);
                assert_eq!(collection.skipped, 0);
            } else {
                assert!(
                    matches!(&result, Err(IngestError::Domain(DomainError::InvalidMetadata(message)))
                        if message.contains("ARCHIVE_MAX_DECOMPRESSED_BYTES")),
                    "{result:?}"
                );
            }
        }
    }

    #[test]
    fn gzip_payload_and_trailer_corruption_fail_collection() {
        use std::io::Write;

        let mut tar = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(5);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, "repo-root/a.txt", &b"hello"[..])
            .unwrap();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::none());
        encoder.write_all(&tar.into_inner().unwrap()).unwrap();
        let intact = encoder.finish().unwrap();
        let full = collect_default(&intact, "label", None).expect("intact stream collects");
        assert_eq!(full.files.iter().next().unwrap().content, "hello");

        let payload = intact.windows(5).position(|w| w == b"hello").unwrap();
        let mut bad_payload = intact.clone();
        bad_payload[payload] = b'j';
        let mut bad_crc = intact.clone();
        bad_crc[intact.len() - 8] ^= 1;
        let mut bad_size = intact.clone();
        bad_size[intact.len() - 4] ^= 1;
        for (case, bytes) in [
            ("payload", bad_payload),
            ("checksum", bad_crc),
            ("size", bad_size),
        ] {
            let result = collect_default(&bytes, "label", None);
            assert!(
                matches!(result, Err(IngestError::Http(_))),
                "{case}: {result:?}"
            );
        }
        for missing in 1..=8 {
            let result = collect_default(&intact[..intact.len() - missing], "label", None);
            assert!(
                matches!(result, Err(IngestError::Http(_))),
                "missing {missing} trailer bytes: {result:?}"
            );
        }
    }

    #[test]
    fn bytes_after_tar_end_share_the_decompressed_stream_budget() {
        for (length, accepted) in [
            (ARCHIVE_MAX_HEADER_BYTES + 1, true),
            (ARCHIVE_MAX_DECOMPRESSED_BYTES, true),
            (ARCHIVE_MAX_DECOMPRESSED_BYTES + 1, false),
        ] {
            // The first zero block ends tar; the remaining bytes still belong to gzip.
            let stream = flate2::read::GzEncoder::new(
                io::repeat(0).take(length),
                flate2::Compression::fast(),
            );
            let result = collect(stream, "label", None, &ExcludePatterns::default());
            if accepted {
                let collection =
                    result.expect("trailing bytes within the stream budget are accepted");
                assert_eq!(collection.files.len(), 0);
                assert_eq!(collection.skipped, 0);
            } else {
                assert!(
                    matches!(&result, Err(IngestError::Domain(DomainError::InvalidMetadata(message)))
                        if message.contains("ARCHIVE_MAX_DECOMPRESSED_BYTES")),
                    "length {length}: {result:?}"
                );
            }
        }
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

        let _ = collect_default(mock_data.as_slice(), "https://github.com/owner/repo", None)
            .expect("intact stream collects");

        for cut in 0..mock_data.len() {
            let reader = DroppedAfter {
                data: &mock_data[..cut],
            };
            let result = collect(
                reader,
                "https://github.com/owner/repo",
                None,
                &ExcludePatterns::default(),
            );
            assert!(
                matches!(result, Err(IngestError::Http(_))),
                "cut at {cut}: {result:?}"
            );
        }
    }

    #[test]
    fn oversized_header_fails_the_run() {
        let long_name = format!("repo-root/{}", "a".repeat(70_000));
        let hidden_name = format!("repo-root/.cache/{}/f.txt", "a".repeat(70_000));

        let cases = [
            // tar reads the `L` extension body whole before the path reaches policy.
            gzip_tarball(&[(long_name.as_str(), b"content", EntryType::Regular)]),
            // The PAX `x` body is read whole by tar, so an oversized path record fails the run.
            gzip_archive(|tar| {
                tar.append_pax_extensions([("path", long_name.as_bytes())])
                    .unwrap();
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(EntryType::Regular);
                header.set_size(7);
                header.set_mode(0o644);
                header.set_cksum();
                tar.append_data(&mut header, "repo-root/x.txt", &b"content"[..])
                    .unwrap();
            }),
            // The budget runs before classification, so a hidden entry that policy would drop still fails.
            gzip_tarball(&[
                (hidden_name.as_str(), b"content", EntryType::Regular),
                ("repo-root/src/main.rs", b"fn main() {}", EntryType::Regular),
            ]),
        ];

        for mock_data in cases {
            let err =
                collect_default(&mock_data, "https://github.com/owner/repo", None).unwrap_err();
            assert!(
                matches!(&err, IngestError::Domain(DomainError::InvalidMetadata(msg)) if msg.contains("ARCHIVE_MAX_HEADER_BYTES")),
                "expected header budget failure, got: {err:?}"
            );
        }
    }

    /// Gzip stream of a tar holding one regular entry, `repo-root/big.txt`, whose body is `body_len` bytes of
    /// `x`. The decompressed stream is exactly one 512-byte header plus the body, with no padding or end
    /// blocks. Generated on demand, so a limit-sized archive is never held in memory.
    fn gzip_single_entry_stream(body_len: u64) -> impl Read {
        let mut header = tar::Header::new_gnu();
        header.set_path("repo-root/big.txt").unwrap();
        header.set_entry_type(EntryType::Regular);
        header.set_size(body_len);
        header.set_mode(0o644);
        header.set_cksum();
        let raw =
            io::Cursor::new(header.as_bytes().to_vec()).chain(io::repeat(b'x').take(body_len));
        flate2::read::GzEncoder::new(raw, flate2::Compression::fast())
    }

    /// Body length that makes the single-entry stream exactly `extra` bytes past the decompressed limit
    /// (negative `extra` is short of it). The 512-byte tar header counts toward the limit.
    fn body_len_for_stream_len(extra: i64) -> u64 {
        ARCHIVE_MAX_DECOMPRESSED_BYTES
            .checked_add_signed(extra - 512)
            .expect("limit exceeds the header length")
    }

    #[test]
    fn decompressed_stream_enforces_limit_at_boundary() {
        // One oversized entry is skipped, but its body is still drained from the stream.
        let at_limit = gzip_single_entry_stream(body_len_for_stream_len(0));
        let result = collect(at_limit, "label", None, &ExcludePatterns::default())
            .expect("a stream of exactly the limit is accepted");
        assert_eq!(result.files.len(), 0);
        assert_eq!(result.skipped, 1);

        let beyond_limit = gzip_single_entry_stream(body_len_for_stream_len(1));
        let err = collect(beyond_limit, "label", None, &ExcludePatterns::default()).unwrap_err();
        assert!(
            matches!(&err, IngestError::Domain(DomainError::InvalidMetadata(msg)) if msg.contains("ARCHIVE_MAX_DECOMPRESSED_BYTES")),
            "expected decompressed limit failure, got: {err:?}"
        );
    }
}
