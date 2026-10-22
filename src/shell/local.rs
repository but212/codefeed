//! Local directory collection orchestration: sorted walk, bounded parallel reads, ordered selection.
//!
//! # Concurrency & Performance Architecture
//! - **Ordered Pipeline**: A producer thread walks in sorted order and sends one slot per entry.
//!   Workers read regular files concurrently and answer on their slot. The main thread settles
//!   slots in walk order, so `Selector` observes candidates in `cmp_path` order.
//!   Bounded `sync_channel`s of `LOCAL_CHANNEL_CAPACITY` messages keep memory bounded.
//! - **FCIS Invariant**: Workers perform side-effecting I/O and pre-open symlink checks
//!   (`fs::symlink_metadata`), delegating pure linear accumulation to `Selector` on the main
//!   thread inside `std::thread::scope`.
//! - **Determinism & Cancellation**: The first failure in walk order is reported, so the result
//!   is the same on every run. An error or budget overflow sets `AtomicBool`, so workers skip
//!   further reads, and the main thread drops its receiver, which stops the producer.

use std::fs;
use std::io::{self, Read};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

use ignore::WalkBuilder;

use super::IngestError;
use super::error::io_error_at;
use super::output::ResolvedOutput;
use crate::core::error::DomainError;
use crate::core::model::{CollectionResult, repository_name};
use crate::core::policy::{
    LOCAL_CHANNEL_CAPACITY, MAX_FILE_SIZE, MAX_LOCAL_WORKERS, NormalizedPath, PathStatus,
    classify_path_segments, is_hidden_segment,
};
use crate::core::selector::{Selector, exceeds_size_limits};

/// What a worker learned about one regular-file candidate.
enum Body {
    /// Unavailable at metadata, open, or read (`is_unavailable_file`).
    Unavailable,
    /// Path or declared length over the size limits; not read. Carries the declared length.
    OverLimit(u64),
    /// Bounded read of at most `MAX_FILE_SIZE + 1` bytes.
    Read(Vec<u8>),
}

/// One walk result: a non-UTF-8 path prefix, or a regular file with its outcome.
enum Candidate {
    NonUtf8(String),
    File(NormalizedPath, Body),
}

/// Worker reply for one job: a candidate, `None` when the entry is no longer a regular file, or the
/// error that aborts the run.
type WorkerMsg = Result<Option<Candidate>, IngestError>;

/// One regular file queued for a worker. The worker answers once on `reply`.
struct Job {
    path: NormalizedPath,
    entry_path: PathBuf,
    reply: SyncSender<WorkerMsg>,
}

/// One walk entry in walk order: decided on the producer thread, or waiting for a worker.
enum Slot {
    Ready(WorkerMsg),
    Pending(Receiver<WorkerMsg>),
}

/// Collects files from a local directory root, excluding the output file if located inside it.
///
/// A producer thread walks in sorted order, workers read file bodies concurrently, and the calling
/// thread settles results in that same order, so `Selector` observes candidates in `cmp_path` order.
pub(super) fn collect(
    root: &Path,
    output: &ResolvedOutput,
) -> Result<CollectionResult, IngestError> {
    let canonical_root = validate_root(root)?;
    let label = local_label(&canonical_root);
    let selector = fold_candidates(
        &canonical_root,
        output.file(),
        Selector::new(),
        observe_candidate,
    )?;

    Ok(selector.finish(label, None))
}

/// Walks `root` in sorted order and folds each candidate through `step` in that order.
fn fold_candidates<T>(
    root: &Path,
    output: Option<&Path>,
    init: T,
    step: impl FnMut(T, Candidate) -> Result<T, IngestError>,
) -> Result<T, IngestError> {
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .filter_entry(|entry| {
            entry.depth() == 0 || !is_hidden_segment(entry.file_name().as_encoded_bytes())
        })
        .ignore(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(false)
        .require_git(false)
        .parents(false)
        .follow_links(false)
        .sort_by_file_name(|a, b| a.cmp(b))
        .build();

    let aborted = AtomicBool::new(false);
    let (job_tx, job_rx) = sync_channel::<Job>(LOCAL_CHANNEL_CAPACITY);
    let jobs = Mutex::new(job_rx);
    let (slot_tx, slot_rx) = sync_channel::<Slot>(LOCAL_CHANNEL_CAPACITY);
    std::thread::scope(|s| {
        for _ in 0..local_worker_count(std::thread::available_parallelism()) {
            s.spawn(|| run_worker(&jobs, &aborted));
        }
        // Senders move into the producer, so they drop before the workers are joined.
        s.spawn(move || produce_slots(walker, root, output, job_tx, slot_tx));

        let result = settle_slots(slot_rx, init, step);
        if result.is_err() {
            aborted.store(true, Ordering::Relaxed);
        }
        result
    })
}

/// Classifies one walk entry on the producer thread. `Ignored` covers the root, the output file,
/// directories, and non-files.
fn classify_entry(
    root: &Path,
    output: Option<&Path>,
    entry: &ignore::DirEntry,
) -> Result<PathStatus, IngestError> {
    let entry_path = entry.path();
    if entry_path == root || output.is_some_and(|target| entry_path == target) {
        return Ok(PathStatus::Ignored);
    }

    if !entry.file_type().is_some_and(|kind| kind.is_file()) {
        return Ok(PathStatus::Ignored);
    }

    let relative = entry_path
        .strip_prefix(root)
        .map_err(|err| DomainError::invalid_metadata(err.to_string()))?;
    classify_local_path(relative).map_err(IngestError::from)
}

/// Routes one walk entry: `None` when it is not a candidate, a ready reply when it is decided on
/// the producer thread, or a pending reply when a worker reads its body.
fn dispatch(
    entry: Result<ignore::DirEntry, ignore::Error>,
    root: &Path,
    output: Option<&Path>,
    jobs: &SyncSender<Job>,
) -> Option<Slot> {
    let entry = match entry {
        Ok(entry) => entry,
        Err(err) => return Some(Slot::Ready(Err(IngestError::Io(format_ignore_error(&err))))),
    };
    let path = match classify_entry(root, output, &entry) {
        Ok(PathStatus::Ignored) => return None,
        Ok(PathStatus::NonUtf8Candidate(prefix)) => {
            return Some(Slot::Ready(Ok(Some(Candidate::NonUtf8(prefix)))));
        }
        Ok(PathStatus::Valid(path)) => path,
        Err(err) => return Some(Slot::Ready(Err(err))),
    };

    let (reply, pending) = sync_channel(1);
    let job = Job {
        path,
        entry_path: entry.into_path(),
        reply,
    };
    if jobs.send(job).is_err() {
        return Some(Slot::Ready(Err(worker_stopped())));
    }
    Some(Slot::Pending(pending))
}

/// Answers jobs until the job channel closes. After an abort, workers answer without touching the
/// filesystem, so the remaining queue drains quickly.
fn run_worker(jobs: &Mutex<Receiver<Job>>, aborted: &AtomicBool) {
    while let Some(job) = next_job(jobs) {
        let msg = if aborted.load(Ordering::Relaxed) {
            Ok(None)
        } else {
            read_candidate(&job.entry_path, job.path)
        };
        // The settling side may already have stopped after an error; the reply is then unused.
        let _ = job.reply.send(msg);
    }
}

/// Takes the next job. The lock covers only the receive, so workers do not serialize on the reads.
fn next_job(jobs: &Mutex<Receiver<Job>>) -> Option<Job> {
    jobs.lock().ok()?.recv().ok()
}

/// Reads one regular file's metadata and body. `Ok(None)` when the entry is a symlink or non-file.
fn read_candidate(entry_path: &Path, path: NormalizedPath) -> WorkerMsg {
    let meta = match fs::symlink_metadata(entry_path) {
        Ok(m) => m,
        // Unlinked or locked after discovery: the selector counts it as skipped.
        Err(err) if is_unavailable_file(&err) => {
            return Ok(Some(Candidate::File(path, Body::Unavailable)));
        }
        Err(err) => return Err(io_error_at(entry_path, err)),
    };

    if meta.is_symlink() || !meta.is_file() {
        return Ok(None);
    }

    let file_len = meta.len();
    if exceeds_size_limits(path.len(), Some(file_len)) {
        return Ok(Some(Candidate::File(path, Body::OverLimit(file_len))));
    }
    let body = match read_bounded(entry_path, file_len)? {
        Some(bytes) => Body::Read(bytes),
        None => Body::Unavailable,
    };
    Ok(Some(Candidate::File(path, body)))
}

fn worker_stopped() -> IngestError {
    IngestError::Io(io::Error::other("local read worker stopped"))
}

fn observe_candidate(selector: Selector, candidate: Candidate) -> Result<Selector, IngestError> {
    let (status, size_hint, content) = match candidate {
        Candidate::NonUtf8(prefix) => (PathStatus::NonUtf8Candidate(prefix), None, None),
        Candidate::File(path, Body::Unavailable) => (PathStatus::Valid(path), None, None),
        Candidate::File(path, Body::OverLimit(len)) => (PathStatus::Valid(path), Some(len), None),
        Candidate::File(path, Body::Read(bytes)) => (PathStatus::Valid(path), None, Some(bytes)),
    };
    selector.observe(status, size_hint, || Ok(content))
}

/// Waits for one slot's reply and folds its candidate into `acc`.
fn settle<T>(
    slot: Slot,
    acc: T,
    step: &mut impl FnMut(T, Candidate) -> Result<T, IngestError>,
) -> Result<T, IngestError> {
    let msg = match slot {
        Slot::Ready(msg) => msg,
        Slot::Pending(reply) => reply.recv().unwrap_or_else(|_| Err(worker_stopped())),
    };
    match msg? {
        Some(candidate) => step(acc, candidate),
        None => Ok(acc),
    }
}

/// Walks and dispatches on its own thread, sending slots in walk order. Stops at the first walk or
/// classification error, or when the settling side hangs up.
fn produce_slots(
    walker: ignore::Walk,
    root: &Path,
    output: Option<&Path>,
    jobs: SyncSender<Job>,
    slots: SyncSender<Slot>,
) {
    for entry in walker {
        let Some(slot) = dispatch(entry, root, output, &jobs) else {
            continue;
        };
        let fatal = matches!(slot, Slot::Ready(Err(_)));
        if slots.send(slot).is_err() || fatal {
            break;
        }
    }
}

/// Settles slots in the order they arrive. The first failure in walk order stops the fold.
fn settle_slots<T>(
    slots: Receiver<Slot>,
    init: T,
    mut step: impl FnMut(T, Candidate) -> Result<T, IngestError>,
) -> Result<T, IngestError> {
    slots
        .iter()
        .try_fold(init, |acc, slot| settle(slot, acc, &mut step))
}

/// Read worker count: available cores capped at `MAX_LOCAL_WORKERS`, or 1 when unknown.
fn local_worker_count(available: io::Result<NonZeroUsize>) -> usize {
    available.map_or(1, |cores| cores.get().min(MAX_LOCAL_WORKERS))
}

/// Errors that make one file unavailable without invalidating the walk. Applied at metadata, open,
/// and read; any other error fails the run.
fn is_unavailable_file(err: &io::Error) -> bool {
    const ERROR_SHARING_VIOLATION: i32 = 32;
    const ERROR_LOCK_VIOLATION: i32 = 33;
    matches!(
        err.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
    ) || (cfg!(windows)
        && matches!(
            err.raw_os_error(),
            Some(ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION)
        ))
}

/// Reads at most `MAX_FILE_SIZE + 1` bytes; the extra byte lets the selector detect overflow.
/// Returns `Ok(None)` when the file is unavailable at open or read (`is_unavailable_file`).
fn read_bounded(path: &Path, file_len: u64) -> Result<Option<Vec<u8>>, IngestError> {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(err) if is_unavailable_file(&err) => return Ok(None),
        Err(err) => return Err(io_error_at(path, err)),
    };
    let mut buf = Vec::with_capacity(file_len.min(MAX_FILE_SIZE + 1) as usize);
    match file.take(MAX_FILE_SIZE + 1).read_to_end(&mut buf) {
        Ok(_) => Ok(Some(buf)),
        Err(err) if is_unavailable_file(&err) => Ok(None),
        Err(err) => Err(io_error_at(path, err)),
    }
}

fn validate_root(root: &Path) -> Result<PathBuf, DomainError> {
    let root_display = root.display();
    let canonical = fs::canonicalize(root)
        .map_err(|error| DomainError::invalid_source(format!("{root_display} ({error})")))?;
    if !canonical.is_dir() {
        let display = canonical.display();
        return Err(DomainError::invalid_source(format!(
            "{display} is not a directory"
        )));
    }
    Ok(canonical)
}

fn classify_local_path(relative: &Path) -> Result<PathStatus, DomainError> {
    let segments = relative
        .components()
        .map(|c| c.as_os_str().as_encoded_bytes());
    classify_path_segments(segments)
        .map_err(|_| DomainError::invalid_metadata("local path exceeds maximum directory depth"))
}

fn format_ignore_error(err: &ignore::Error) -> io::Error {
    let mut current = err;
    let mut path = None;
    loop {
        match current {
            ignore::Error::WithPath {
                path: p,
                err: inner,
            } => {
                if path.is_none() {
                    path = Some(p.as_path());
                }
                current = inner;
            }
            ignore::Error::WithDepth { err: inner, .. } => {
                current = inner;
            }
            _ => break,
        }
    }
    let (kind, msg) = match (path, current) {
        (Some(p), ignore::Error::Io(inner)) => (inner.kind(), format!("{}: {inner}", p.display())),
        (None, ignore::Error::Io(inner)) => (inner.kind(), inner.to_string()),
        (Some(p), _) => (io::ErrorKind::Other, format!("{}: {current}", p.display())),
        (None, _) => (io::ErrorKind::Other, current.to_string()),
    };
    io::Error::new(kind, msg)
}

fn local_label(root: &Path) -> String {
    repository_name(root.file_name().and_then(|name| name.to_str())).to_string()
}

#[cfg(test)]
mod tests {
    use super::super::testing::temp_dir;
    use super::*;
    use crate::core::model::{DigestFormat, OutputTarget, RunConfig, Source};
    use crate::core::storage::cmp_path;
    use crate::ingest;

    #[test]
    fn read_bounded_caps_and_reports_unavailable() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("read_bounded")?;
        let exact = root.join("exact.txt");
        let over = root.join("over.txt");
        let kib = root.join("kib.txt");
        let missing = root.join("missing.txt");
        let kib_payload = vec![b'y'; 64 * 1024];
        fs::write(&exact, vec![b'x'; MAX_FILE_SIZE as usize])?;
        fs::write(&over, vec![b'x'; MAX_FILE_SIZE as usize + 1])?;
        fs::write(&kib, &kib_payload)?;

        let cases = [
            (
                &exact,
                MAX_FILE_SIZE,
                Some(MAX_FILE_SIZE as usize),
                "exact limit",
            ),
            (
                &over,
                MAX_FILE_SIZE + 1,
                Some(MAX_FILE_SIZE as usize + 1),
                "one byte over",
            ),
            (
                &kib,
                kib_payload.len() as u64,
                Some(kib_payload.len()),
                "64 KiB",
            ),
            (&missing, 0, None, "unlinked before open"),
        ];
        for (path, file_len, expected_len, label) in cases {
            let read = read_bounded(path, file_len)?;
            assert_eq!(read.as_ref().map(Vec::len), expected_len, "{label}");
        }
        assert_eq!(
            read_bounded(&kib, kib_payload.len() as u64)?,
            Some(kib_payload)
        );
        Ok(())
    }

    #[test]
    fn read_local_file_enforces_size_boundaries() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("size_boundaries")?;
        fs::write(root.join("exact.txt"), vec![b'x'; MAX_FILE_SIZE as usize])?;
        fs::write(
            root.join("large.txt"),
            vec![b'x'; MAX_FILE_SIZE as usize + 1],
        )?;

        let res = collect(&root, &ResolvedOutput::STDOUT)?;
        assert_eq!(res.files.len(), 1, "exact-size file is included");
        assert_eq!(res.skipped, 1, "one byte over the limit is skipped");
        Ok(())
    }

    #[test]
    fn unlinked_before_metadata_is_counted_as_skipped() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("unlinked_after_discovery")?;
        let file = root.join("a.txt");
        fs::write(&file, "content")?;
        fs::remove_file(&file)?;

        let candidate = read_candidate(&file, NormalizedPath::from("a.txt"))?
            .ok_or("unlinked file must reach the selector as a candidate")?;
        assert!(
            matches!(&candidate, Candidate::File(_, Body::Unavailable)),
            "unlinked file must be an unavailable candidate"
        );
        let selector = observe_candidate(Selector::new(), candidate)?;

        let result = selector.finish("test".to_string(), None);
        assert_eq!(result.files.len(), 0);
        assert_eq!(result.skipped, 1);
        Ok(())
    }

    #[test]
    fn oversized_file_carries_size_hint() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("oversized_size_hint")?;
        let file = root.join("large.txt");
        fs::write(&file, vec![b'x'; MAX_FILE_SIZE as usize + 1])?;

        let candidate = read_candidate(&file, NormalizedPath::from("large.txt"))?
            .ok_or("oversized file must reach the selector as a candidate")?;
        assert!(
            matches!(
                &candidate,
                Candidate::File(_, Body::OverLimit(len)) if *len == MAX_FILE_SIZE + 1
            ),
            "oversized file must carry its declared size and no read"
        );
        let selector = observe_candidate(Selector::new(), candidate)?;

        let result = selector.finish("test".to_string(), None);
        assert_eq!(result.files.len(), 0);
        assert_eq!(result.skipped, 1);
        Ok(())
    }

    #[test]
    fn unavailable_errors_skip_others_fail() {
        // Windows delete-pending and lock-violation codes are unavailable; other errors are fatal.
        let cases = [
            (io::Error::from(io::ErrorKind::NotFound), true),
            (io::Error::from(io::ErrorKind::PermissionDenied), true),
            (io::Error::from_raw_os_error(32), cfg!(windows)),
            (io::Error::from_raw_os_error(33), cfg!(windows)),
            (io::Error::other("disk failure"), false),
        ];
        for (err, expected) in cases {
            assert_eq!(is_unavailable_file(&err), expected, "{err:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_file() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        let root = temp_dir("symlink_test")?;
        let target = root.join("target.txt");
        let link = root.join("link.txt");
        fs::write(&target, "content")?;
        symlink(&target, &link)?;

        let res = collect(&root, &ResolvedOutput::STDOUT)?;
        assert_eq!(res.files.len(), 1, "only the regular target is collected");
        assert_eq!(res.skipped, 0);
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn rejects_windows_symlink_file() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::windows::fs::symlink_file;

        let root = temp_dir("win_symlink_test")?;
        let target = root.join("target.txt");
        let link = root.join("link.txt");
        fs::write(&target, "content")?;

        if symlink_file(&target, &link).is_ok() {
            let res = collect(&root, &ResolvedOutput::STDOUT)?;
            assert_eq!(res.files.len(), 1, "only the regular target is collected");
            assert_eq!(res.skipped, 0);
            let _ = fs::remove_file(&link);
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn preserves_distinct_unix_paths() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("path_encoding")?;
        fs::create_dir_all(root.join("a"))?;
        fs::write(root.join("a\\b.txt"), "backslash")?;
        fs::write(root.join("a").join("b.txt"), "slash")?;
        let output = root.join("digest.txt");

        let report = ingest(&RunConfig::new(
            Source::Local(root.to_path_buf()),
            OutputTarget::File(output.clone()),
            DigestFormat::Text,
        ))?;
        let digest = fs::read_to_string(output)?;

        assert_eq!(report.included_files, 2);
        assert!(digest.contains("FILE: a\\\\b.txt"));
        assert!(digest.contains("FILE: a/b.txt"));
        Ok(())
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn excludes_output_case_insensitively() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("output_case")?;
        fs::write(root.join("source.txt"), "source")?;
        fs::write(root.join("digest.txt"), "previous output")?;
        let output = root.join("DIGEST.TXT");

        let report = ingest(&RunConfig::new(
            Source::Local(root.to_path_buf()),
            OutputTarget::File(output),
            DigestFormat::Text,
        ))?;
        let digest = fs::read_to_string(root.join("DIGEST.TXT"))?;

        assert_eq!(report.included_files, 1);
        assert!(!digest.contains("previous output"));
        assert!(digest.contains("FILE: source.txt"));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_directory_fails_fast_with_path_context() -> Result<(), Box<dyn std::error::Error>>
    {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_dir("unreadable_dir")?;
        let locked = ["a_locked", "b_locked"].map(|name| root.join(name));
        for dir in &locked {
            fs::create_dir(dir)?;
            fs::write(dir.join("file.txt"), "content")?;
            fs::set_permissions(dir, fs::Permissions::from_mode(0o0000))?;
        }

        if fs::read_dir(&locked[0]).is_ok() {
            for dir in &locked {
                let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o0755));
            }
            return Ok(());
        }

        let res = collect(&root, &ResolvedOutput::STDOUT);

        // Restore permissions so TempDirGuard can clean up
        for dir in &locked {
            let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o0755));
        }

        let err = res.unwrap_err();
        assert!(
            matches!(&err, IngestError::Io(io_err) if io_err.kind() == std::io::ErrorKind::PermissionDenied),
            "expected IngestError::Io with ErrorKind::PermissionDenied, got: {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("a_locked") && !msg.contains("b_locked"),
            "first directory in walk order must be reported alone, got: {msg}"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_file_is_counted_as_skipped() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_dir("unreadable_file")?;
        let locked = root.join("a.txt");
        fs::write(&locked, "secret")?;
        fs::write(root.join("b.txt"), "visible")?;
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o0000))?;

        if fs::File::open(&locked).is_ok() {
            return Ok(());
        }

        let res = collect(&root, &ResolvedOutput::STDOUT)?;
        assert_eq!(res.files.len(), 1);
        assert_eq!(res.skipped, 1);
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn locked_file_is_counted_as_skipped() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::windows::fs::OpenOptionsExt;

        let root = temp_dir("locked_file")?;
        let locked = root.join("a.txt");
        fs::write(&locked, "secret")?;
        fs::write(root.join("b.txt"), "visible")?;
        let _guard = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&locked)?;

        let res = collect(&root, &ResolvedOutput::STDOUT)?;
        assert_eq!(res.files.len(), 1);
        assert_eq!(res.skipped, 1);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn accounting_rules_non_regular_files_and_unlinked_files()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        let root = temp_dir("accounting_non_regular")?;
        let target = root.join("regular.txt");
        let link = root.join("symlink.txt");

        fs::write(&target, "hello")?;
        symlink(&target, &link)?;

        // Symlink is filtered out before candidate reservation and should not increase skipped
        let res = collect(&root, &ResolvedOutput::STDOUT)?;
        assert_eq!(res.files.len(), 1);
        assert_eq!(res.skipped, 0);

        Ok(())
    }

    #[test]
    fn ingests_local_directory_respects_all_exclusion_policies()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("comprehensive_local_policies")?;
        fs::create_dir_all(root.join(".git"))?;
        fs::create_dir_all(root.join("certs"))?;
        fs::create_dir_all(root.join("sub"))?;

        // 1. Text file: included
        fs::write(root.join("README.md"), "hello")?;

        // 2. Binary file: skipped (increments skipped count)
        fs::write(root.join("binary.bin"), [0, 1, 2])?;

        // 3. Noise files: ignored without incrementing skipped
        fs::write(root.join("Cargo.lock"), "[lockfile]")?;
        fs::write(root.join("poetry.lock"), "[poetry]")?;
        fs::write(root.join("sub").join("package-lock.json"), "{}")?;

        // 4. Sensitive files: ignored without incrementing skipped
        fs::write(root.join(".env"), "SECRET=true")?;
        fs::write(root.join(".env.production"), "KEY=123")?;
        fs::write(root.join("certs").join("privkey.key"), "PRIVATE")?;
        fs::write(root.join("certs").join("cert.pfx"), "CERT")?;
        fs::write(root.join("id_rsa"), "SSH")?;
        fs::write(root.join("id_ecdsa"), "SSH_ECDSA")?;

        // 5. Git metadata: ignored
        fs::write(root.join(".git/config"), "secret")?;

        // 6. Gitignore: ignored without git directory
        fs::write(root.join(".gitignore"), "ignored.txt\n")?;
        fs::write(root.join("ignored.txt"), "ignored")?;

        let output = root.join("digest.txt");
        let report = ingest(&RunConfig::new(
            Source::Local(root.to_path_buf()),
            OutputTarget::File(output.clone()),
            DigestFormat::Text,
        ))?;
        let digest = fs::read_to_string(output)?;

        assert_eq!(report.included_files, 1);
        assert_eq!(report.skipped_files, 1);
        assert!(digest.contains("FILE: README.md"));
        assert!(!digest.contains("binary.bin"));
        assert!(!digest.contains("Cargo.lock"));
        assert!(!digest.contains("poetry.lock"));
        assert!(!digest.contains("package-lock.json"));
        assert!(!digest.contains(".env"));
        assert!(!digest.contains("privkey.key"));
        assert!(!digest.contains("cert.pfx"));
        assert!(!digest.contains("id_rsa"));
        assert!(!digest.contains("id_ecdsa"));
        assert!(!digest.contains(".git/config"));
        assert!(!digest.contains("ignored.txt"));
        Ok(())
    }

    #[test]
    fn does_not_apply_dot_ignore_files() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("dot_ignore")?;
        fs::write(root.join(".ignore"), "important.txt\n")?;
        fs::write(root.join("important.txt"), "important")?;
        let output = root.join("digest.txt");

        let report = ingest(&RunConfig::new(
            Source::Local(root.to_path_buf()),
            OutputTarget::File(output.clone()),
            DigestFormat::Text,
        ))?;
        let digest = fs::read_to_string(output)?;

        assert_eq!(report.included_files, 1);
        assert!(digest.contains("FILE: important.txt"));
        Ok(())
    }

    #[test]
    fn label_uses_directory_basename() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("custom_repo_name")?;
        fs::write(root.join("file.txt"), "hello")?;
        let output = root.join("digest.txt");
        let _ = ingest(&RunConfig::new(
            Source::Local(root.to_path_buf()),
            OutputTarget::File(output.clone()),
            DigestFormat::Text,
        ))?;
        let digest = fs::read_to_string(&output)?;
        let expected_basename = root.file_name().unwrap().to_str().unwrap();
        assert!(digest.starts_with(&format!("Repository: {expected_basename}\n")));
        Ok(())
    }

    #[test]
    fn label_handles_filesystem_root() {
        assert_eq!(local_label(Path::new("/")), "root");
        #[cfg(windows)]
        assert_eq!(local_label(Path::new("C:\\")), "root");
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn skips_non_utf8_paths() -> Result<(), Box<dyn std::error::Error>> {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let root = temp_dir("invalid_paths")?;
        fs::write(root.join(".gitignore"), "!*\n")?;
        fs::create_dir(root.join(".hidden"))?;
        fs::write(
            root.join(".hidden")
                .join(OsString::from_vec(b"c\x82".to_vec())),
            "c",
        )?;
        fs::write(root.join(OsString::from_vec(b"a\x80".to_vec())), "a")?;
        fs::write(root.join(OsString::from_vec(b"b\x81".to_vec())), "b")?;
        let output = root.join("digest.txt");
        let report = ingest(&RunConfig::new(
            Source::Local(root.to_path_buf()),
            OutputTarget::File(output),
            DigestFormat::Text,
        ))?;
        assert_eq!(report.included_files, 0);
        assert_eq!(report.skipped_files, 2);
        Ok(())
    }

    #[test]
    fn candidates_arrive_in_path_order() -> Result<(), Box<dyn std::error::Error>> {
        let root = validate_root(&temp_dir("candidate_order")?)?;
        for dir in ["b_dir", "a_dir", "src-test", "src", "src/nested"] {
            fs::create_dir_all(root.join(dir))?;
            for file_index in 0..10 {
                fs::write(
                    root.join(dir).join(format!("file_{file_index:02}.txt")),
                    "content",
                )?;
            }
        }
        fs::write(root.join("B.txt"), "content")?;
        fs::write(root.join("src.rs"), "content")?;

        let arrived = fold_candidates(&root, None, Vec::new(), |mut paths, candidate| {
            if let Candidate::File(path, _) = candidate {
                paths.push(path.as_str().to_string());
            }
            Ok(paths)
        })?;

        let mut expected = arrived.clone();
        expected.sort_by(|a, b| cmp_path(a, b));
        assert_eq!(arrived.len(), 52);
        assert_eq!(arrived, expected);
        Ok(())
    }

    #[test]
    fn rejects_excessive_path_depth() {
        let mut excessive = PathBuf::new();
        for _ in 0..257 {
            excessive.push("d");
        }
        excessive.push("file.txt");
        let result = classify_local_path(&excessive);
        assert!(result.is_err());

        let mut normal = PathBuf::new();
        for _ in 0..256 {
            normal.push("d");
        }
        normal.push("file.txt");
        let result = classify_local_path(&normal);
        assert!(result.is_ok());
    }

    #[test]
    fn hidden_directories_and_files_are_pruned_by_policy() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = temp_dir("hidden_pruning")?;
        fs::create_dir_all(root.join(".hidden_dir"))?;
        fs::write(root.join(".hidden_dir").join("nested.txt"), "nested")?;
        fs::write(root.join(".dotfile"), "secret")?;
        fs::write(root.join("visible.txt"), "visible")?;

        let result = collect(&root, &ResolvedOutput::STDOUT)?;
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files.iter().next().unwrap().path, "visible.txt");
        assert_eq!(result.skipped, 0);
        Ok(())
    }

    #[test]
    fn local_worker_count_caps_and_defaults() {
        let cores = |n: usize| NonZeroUsize::new(n).unwrap();
        assert_eq!(local_worker_count(Ok(cores(64))), MAX_LOCAL_WORKERS);
        assert_eq!(local_worker_count(Ok(cores(4))), 4);
        assert_eq!(local_worker_count(Err(io::Error::other("unknown"))), 1);
    }
}
