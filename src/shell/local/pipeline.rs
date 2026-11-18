//! Order-preserving parallel collection: a sorted producer, read workers, in-order settlement.
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
//!   further reads and the walk stops between entries. Dropping the receiver unblocks slot sends.

use std::fs;
use std::io::{self, Read};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Mutex, PoisonError};

use super::walk::{RegularFile, Walk, classify_entry};
use crate::core::path::{Candidate, NormalizedPath};
use crate::core::policy::{
    ExcludePatterns, LOCAL_CHANNEL_CAPACITY, MAX_FILE_SIZE, MAX_LOCAL_WORKERS,
};
use crate::core::selector::{Body, exceeds_file_size};
use crate::shell::error::{IngestError, io_error_at};

/// Upper bound on buffer capacity retained in [`BufferPool`], preventing large files from
/// pinning up to `LOCAL_CHANNEL_CAPACITY * MAX_FILE_SIZE` bytes of RSS.
const LOCAL_BUFFER_RETAIN_BYTES: usize = 128 * 1024;
const LOCAL_BUFFER_POOL_CAP: usize = LOCAL_CHANNEL_CAPACITY + MAX_LOCAL_WORKERS;

/// Bounded pool of reusable `Vec<u8>` read buffers scoped to a single local collection run.
#[derive(Debug)]
struct BufferPool(Mutex<Vec<Vec<u8>>>);

impl BufferPool {
    fn new() -> Self {
        Self(Mutex::new(Vec::new()))
    }

    fn take(&self, min_capacity: usize) -> PooledBuf<'_> {
        let mut buf = if min_capacity <= LOCAL_BUFFER_RETAIN_BYTES {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pop()
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        if buf.capacity() < min_capacity {
            buf.reserve(min_capacity - buf.len());
        }
        PooledBuf { buf, pool: self }
    }

    fn put(&self, mut buf: Vec<u8>) {
        if buf.capacity() == 0 || buf.capacity() > LOCAL_BUFFER_RETAIN_BYTES {
            return;
        }
        buf.clear();
        let mut pool = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if pool.len() < LOCAL_BUFFER_POOL_CAP {
            pool.push(buf);
        }
    }
}

/// Read buffer borrowed from [`BufferPool`] and automatically returned on drop.
#[derive(Debug)]
pub(super) struct PooledBuf<'a> {
    buf: Vec<u8>,
    pool: &'a BufferPool,
}

impl AsRef<[u8]> for PooledBuf<'_> {
    fn as_ref(&self) -> &[u8] {
        &self.buf
    }
}

impl Drop for PooledBuf<'_> {
    fn drop(&mut self) {
        self.pool.put(std::mem::take(&mut self.buf));
    }
}

/// One regular file queued for a worker. The worker answers once on `reply`.
struct Job<'a> {
    path: NormalizedPath,
    entry_path: PathBuf,
    reply: SyncSender<Settled<'a>>,
}

/// The outcome for one walk entry: `None` when it is not a candidate after all.
type Settled<'a> = Result<Option<Candidate<Body<PooledBuf<'a>>>>, IngestError>;

/// One walk entry in walk order: decided on the producer thread, or waiting for a worker.
enum Slot<'a> {
    Ready(Settled<'a>),
    Pending(Receiver<Settled<'a>>),
}

/// Walks `root` in sorted order and folds each candidate through `step` in that order.
pub(super) fn fold_candidates<T>(
    root: &Path,
    output: Option<&Path>,
    exclude: &ExcludePatterns,
    init: T,
    mut step: impl FnMut(T, Candidate<Body<PooledBuf<'_>>>) -> Result<T, IngestError>,
) -> Result<T, IngestError> {
    let aborted = AtomicBool::new(false);
    let walker = Walk::new(root, exclude, &aborted)?;
    let pool = BufferPool::new();
    let (job_tx, job_rx) = sync_channel::<Job<'_>>(LOCAL_CHANNEL_CAPACITY);
    let jobs = Mutex::new(job_rx);
    let (slot_tx, slot_rx) = sync_channel::<Slot<'_>>(LOCAL_CHANNEL_CAPACITY);
    std::thread::scope(|s| {
        for _ in 0..local_worker_count(std::thread::available_parallelism()) {
            s.spawn(|| run_worker(&jobs, &aborted, &pool));
        }
        // Senders move into the producer, so they drop before the workers are joined.
        s.spawn(move || produce_slots(walker, output, exclude, job_tx, slot_tx));

        let result = settle_slots(slot_rx, init, &mut step);
        if result.is_err() {
            aborted.store(true, Ordering::Relaxed);
        }
        result
    })
}

/// Routes one walk entry: `None` when it is not a candidate, a ready reply when it is decided on
/// the producer thread, or a pending reply when a worker reads its body.
fn dispatch<'a>(
    entry: Result<RegularFile, IngestError>,
    output: Option<&Path>,
    exclude: &ExcludePatterns,
    jobs: &SyncSender<Job<'a>>,
) -> Option<Slot<'a>> {
    let file = match entry {
        Ok(file) => file,
        Err(err) => return Some(Slot::Ready(Err(err))),
    };
    let path = match classify_entry(output, exclude, &file) {
        Ok(None) => return None,
        Ok(Some(Candidate::Unstorable(prefix))) => {
            return Some(Slot::Ready(Ok(Some(Candidate::Unstorable(prefix)))));
        }
        Ok(Some(Candidate::Valid(path, ()))) => path,
        Err(err) => return Some(Slot::Ready(Err(err))),
    };

    let (reply, pending) = sync_channel(1);
    let job = Job {
        path,
        entry_path: file.into_path(),
        reply,
    };
    if jobs.send(job).is_err() {
        return Some(Slot::Ready(Err(worker_stopped())));
    }
    Some(Slot::Pending(pending))
}

/// Answers jobs until the job channel closes. After an abort, workers answer without touching the
/// filesystem, so the remaining queue drains quickly.
fn run_worker<'a>(jobs: &Mutex<Receiver<Job<'a>>>, aborted: &AtomicBool, pool: &'a BufferPool) {
    while let Some(job) = next_job(jobs) {
        let msg = if aborted.load(Ordering::Relaxed) {
            Ok(None)
        } else {
            read_candidate(&job.entry_path, job.path, pool)
        };
        // The settling side may already have stopped after an error; the reply is then unused.
        let _ = job.reply.send(msg);
    }
}

/// Takes the next job. The lock covers only the receive, so workers do not serialize on the reads.
fn next_job<'a>(jobs: &Mutex<Receiver<Job<'a>>>) -> Option<Job<'a>> {
    jobs.lock().ok()?.recv().ok()
}

fn worker_stopped() -> IngestError {
    IngestError::Io(io::Error::other("local read worker stopped"))
}

/// Waits for one slot's reply and folds its candidate into `acc`.
fn settle<'a, T>(
    slot: Slot<'a>,
    acc: T,
    step: &mut impl FnMut(T, Candidate<Body<PooledBuf<'a>>>) -> Result<T, IngestError>,
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
#[expect(
    clippy::needless_pass_by_value,
    reason = "dropping the senders when the producer exits disconnects both channels"
)]
fn produce_slots<'a>(
    walker: Walk<'_>,
    output: Option<&Path>,
    exclude: &ExcludePatterns,
    jobs: SyncSender<Job<'a>>,
    slots: SyncSender<Slot<'a>>,
) {
    for entry in walker {
        let Some(slot) = dispatch(entry, output, exclude, &jobs) else {
            continue;
        };
        let fatal = matches!(slot, Slot::Ready(Err(_)));
        if slots.send(slot).is_err() || fatal {
            break;
        }
    }
}

/// Settles slots in the order they arrive. The first failure in walk order stops the fold.
#[expect(
    clippy::needless_pass_by_value,
    reason = "dropping the receiver on early error disconnects the slot channel and unblocks the producer"
)]
fn settle_slots<'a, T>(
    slots: Receiver<Slot<'a>>,
    init: T,
    step: &mut impl FnMut(T, Candidate<Body<PooledBuf<'a>>>) -> Result<T, IngestError>,
) -> Result<T, IngestError> {
    slots
        .iter()
        .try_fold(init, |acc, slot| settle(slot, acc, step))
}

/// Read worker count: available cores capped at `MAX_LOCAL_WORKERS`, or 1 when unknown.
fn local_worker_count(available: io::Result<NonZeroUsize>) -> usize {
    available.map_or(1, |cores| cores.get().min(MAX_LOCAL_WORKERS))
}

/// Reads one regular file's metadata and body. `Ok(None)` when the entry is a symlink or non-file
/// (no longer a regular file); `Err` aborts the run. A file unavailable at metadata, open, or read
/// (`is_unavailable_file`) is [`Body::Unavailable`].
fn read_candidate<'a>(
    entry_path: &Path,
    path: NormalizedPath,
    pool: &'a BufferPool,
) -> Result<Option<Candidate<Body<PooledBuf<'a>>>>, IngestError> {
    let meta = match fs::symlink_metadata(entry_path) {
        Ok(m) => m,
        // Unlinked or locked after discovery: the selector counts it as skipped.
        Err(err) if is_unavailable_file(&err) => {
            return Ok(Some(Candidate::Valid(path, Body::Unavailable)));
        }
        Err(err) => return Err(io_error_at(entry_path, &err)),
    };

    if meta.is_symlink() || !meta.is_file() {
        return Ok(None);
    }

    let file_len = meta.len();
    if exceeds_file_size(file_len) {
        return Ok(Some(Candidate::Valid(path, Body::OverLimit)));
    }
    let body = read_bounded(entry_path, file_len, pool)?.map_or(Body::Unavailable, Body::Bytes);
    Ok(Some(Candidate::Valid(path, body)))
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
/// `file_len` is the metadata length, which the caller has already checked against `MAX_FILE_SIZE`.
fn read_bounded<'a>(
    path: &Path,
    file_len: u64,
    pool: &'a BufferPool,
) -> Result<Option<PooledBuf<'a>>, IngestError> {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(err) if is_unavailable_file(&err) => return Ok(None),
        Err(err) => return Err(io_error_at(path, &err)),
    };
    let capacity =
        usize::try_from(file_len).expect("invariant: caller checked file_len <= MAX_FILE_SIZE");
    let mut pooled = pool.take(capacity);
    match file.take(MAX_FILE_SIZE + 1).read_to_end(&mut pooled.buf) {
        Ok(_) => Ok(Some(pooled)),
        Err(err) if is_unavailable_file(&err) => Ok(None),
        Err(err) => Err(io_error_at(path, &err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::policy::MAX_FILE_SIZE_USIZE;
    use crate::core::selector::Selector;
    #[cfg(unix)]
    use crate::shell::testing::run_with_timeout;

    #[test]
    fn read_bounded_caps_and_reports_unavailable() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("read_bounded")?;
        let exact = root.join("exact.txt");
        let over = root.join("over.txt");
        let kib = root.join("kib.txt");
        let missing = root.join("missing.txt");
        let kib_payload = vec![b'y'; 64 * 1024];
        fs::write(&exact, vec![b'x'; MAX_FILE_SIZE_USIZE])?;
        fs::write(&over, vec![b'x'; MAX_FILE_SIZE_USIZE + 1])?;
        fs::write(&kib, &kib_payload)?;

        let cases = [
            (
                &exact,
                MAX_FILE_SIZE,
                Some(MAX_FILE_SIZE_USIZE),
                "exact limit",
            ),
            (
                &over,
                MAX_FILE_SIZE + 1,
                Some(MAX_FILE_SIZE_USIZE + 1),
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
        let pool = BufferPool::new();
        for (path, file_len, expected_len, label) in cases {
            let read = read_bounded(path, file_len, &pool)?;
            assert_eq!(
                read.as_ref().map(|buf| buf.as_ref().len()),
                expected_len,
                "{label}"
            );
        }
        assert_eq!(
            read_bounded(&kib, kib_payload.len() as u64, &pool)?
                .as_ref()
                .map(AsRef::as_ref),
            Some(kib_payload.as_slice())
        );
        Ok(())
    }

    #[test]
    fn buffer_pool_reuses_within_cap_and_drops_oversized() {
        let pool = BufferPool::new();
        let small = pool.take(4 * 1024);
        let small_ptr = small.buf.as_ptr();
        drop(small);
        assert_eq!(pool.0.lock().unwrap().len(), 1);

        let reused = pool.take(1024);
        assert_eq!(reused.buf.as_ptr(), small_ptr);
        assert_eq!(reused.buf, [] as [u8; 0]);
        drop(reused);

        let oversized = pool.take(LOCAL_BUFFER_RETAIN_BYTES + 1);
        drop(oversized);
        // Oversized buffer is dropped rather than retained alongside the small one.
        assert_eq!(pool.0.lock().unwrap().len(), 1);
    }

    #[test]
    fn unlinked_before_metadata_is_counted_as_skipped() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("unlinked_after_discovery")?;
        let file = root.join("a.txt");
        fs::write(&file, "content")?;
        fs::remove_file(&file)?;

        let pool = BufferPool::new();
        let observation = read_candidate(&file, NormalizedPath::from("a.txt"), &pool)?
            .ok_or("unlinked file must reach the selector as a candidate")?;
        assert!(
            matches!(&observation, Candidate::Valid(_, Body::Unavailable)),
            "unlinked file must be an unavailable candidate"
        );
        let selector = Selector::new().observe(observation)?;

        let result = selector.finish("test".to_string(), None)?;
        assert_eq!(result.files.len(), 0);
        assert_eq!(result.skipped, 1);
        Ok(())
    }

    #[test]
    fn oversized_file_is_over_limit_without_a_read() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("oversized_over_limit")?;
        let file = root.join("large.txt");
        fs::write(&file, vec![b'x'; MAX_FILE_SIZE_USIZE + 1])?;

        let pool = BufferPool::new();
        let observation = read_candidate(&file, NormalizedPath::from("large.txt"), &pool)?
            .ok_or("oversized file must reach the selector as a candidate")?;
        assert!(
            matches!(&observation, Candidate::Valid(_, Body::OverLimit)),
            "oversized file must be over the limit and carry no read"
        );
        let selector = Selector::new().observe(observation)?;

        let result = selector.finish("test".to_string(), None)?;
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

    #[test]
    fn read_candidate_skips_directory_entry() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("read_candidate_dir")?;
        let sub = root.join("subdir");
        fs::create_dir(&sub)?;

        let pool = BufferPool::new();
        let result = read_candidate(&sub, NormalizedPath::from("subdir"), &pool)?;
        assert!(result.is_none());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn read_candidate_skips_fifo_without_opening() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("read_candidate_fifo")?;
        let fifo = root.join("pipe");
        crate::shell::testing::make_fifo(&fifo)?;

        // Opening a FIFO with no writer blocks, so an `open` regression surfaces as a timeout.
        let is_none = run_with_timeout(move || {
            let pool = BufferPool::new();
            read_candidate(&fifo, NormalizedPath::from("pipe"), &pool).map(|obs| obs.is_none())
        });
        assert!(is_none?, "a FIFO is not a candidate");
        Ok(())
    }

    use crate::core::storage::cmp_path;
    use crate::shell::local::validate_root;
    use crate::shell::testing::temp_dir;
    use std::fs;

    #[test]
    fn candidates_arrive_in_path_order() -> Result<(), Box<dyn std::error::Error>> {
        let temp = temp_dir("candidate_order")?;
        let root = validate_root(&temp)?;
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

        let arrived = fold_candidates(
            &root,
            None,
            &ExcludePatterns::default(),
            Vec::new(),
            |mut paths, candidate| {
                if let Candidate::Valid(path, _) = candidate {
                    paths.push(path.to_string());
                }
                Ok(paths)
            },
        )?;

        let mut expected = arrived.clone();
        expected.sort_by(|a, b| cmp_path(a, b));
        assert_eq!(arrived.len(), 52);
        assert_eq!(arrived, expected);
        Ok(())
    }

    #[test]
    fn early_abort_disconnects_and_joins_when_queue_overflows()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = temp_dir("early_abort")?;
        let root = validate_root(&temp)?;
        for file_index in 0..(LOCAL_CHANNEL_CAPACITY + 10) {
            fs::write(root.join(format!("file_{file_index:03}.txt")), "content")?;
        }

        let err = fold_candidates(
            &root,
            None,
            &ExcludePatterns::default(),
            0,
            |_acc, _candidate| {
                Err(IngestError::Io(io::Error::other(
                    "abort on first candidate",
                )))
            },
        );
        assert!(err.is_err());
        Ok(())
    }

    #[test]
    fn local_worker_count_caps_and_defaults() {
        let cores = |n: usize| NonZeroUsize::new(n).unwrap();
        assert_eq!(local_worker_count(Ok(cores(64))), MAX_LOCAL_WORKERS);
        assert_eq!(local_worker_count(Ok(cores(4))), 4);
        assert_eq!(local_worker_count(Err(io::Error::other("unknown"))), 1);
    }

    #[test]
    fn dispatch_and_settle_handle_errors_and_disconnections() {
        let (job_tx, job_rx) = sync_channel(1);
        drop(job_rx); // Disconnect worker side

        // 1. dispatch when walk entry is Err
        let walk_err = IngestError::Io(io::Error::other("walk failed"));
        let slot = dispatch(Err(walk_err), None, &ExcludePatterns::default(), &job_tx);
        assert!(matches!(slot, Some(Slot::Ready(Err(_)))));

        // settle with Slot::Ready(Err)
        let res = settle(slot.unwrap(), 0, &mut |_acc, _c| Ok(1));
        assert!(res.is_err());

        // 2. dispatch when job_tx is disconnected
        let temp = temp_dir("dispatch_disconnect").unwrap();
        let file = temp.join("test.txt");
        fs::write(&file, "data").unwrap();
        let regular = RegularFile::from_parts(file, Path::new("test.txt"));
        let slot = dispatch(Ok(regular), None, &ExcludePatterns::default(), &job_tx);
        // jobs.send fails, returning Slot::Ready(Err(worker_stopped()))
        assert!(matches!(slot, Some(Slot::Ready(Err(_)))));
        let res = settle(slot.unwrap(), 0, &mut |_acc, _c| Ok(1));
        assert!(res.is_err());

        // 3. settle with Slot::Pending where reply sender is dropped
        let (reply_tx, reply_rx) = sync_channel(1);
        drop(reply_tx);
        let res = settle(Slot::Pending(reply_rx), 0, &mut |_acc, _c| Ok(1));
        assert!(res.is_err());

        // 4. settle with Slot::Pending where candidate is None
        let (reply_tx, reply_rx) = sync_channel(1);
        reply_tx.send(Ok(None)).unwrap();
        let res = settle(Slot::Pending(reply_rx), 42, &mut |_acc, _c| Ok(1)).unwrap();
        assert_eq!(res, 42);

        // 5. settle with Slot::Pending where candidate is Some
        let (reply_tx, reply_rx) = sync_channel(1);
        reply_tx
            .send(Ok(Some(Candidate::Unstorable(
                ("", b"\xff".as_slice()).into(),
            ))))
            .unwrap();
        let res = settle(Slot::Pending(reply_rx), 42, &mut |acc, _c| Ok(acc + 1)).unwrap();
        assert_eq!(res, 43);
    }
}
