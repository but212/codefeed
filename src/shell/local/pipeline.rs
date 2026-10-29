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
//!   further reads, and the main thread drops its receiver, which stops the producer.

use std::io;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

use super::read::{Candidate, WorkerMsg, read_candidate};
use super::walk::{classify_entry, format_ignore_error, sorted_walker};
use crate::core::policy::{LOCAL_CHANNEL_CAPACITY, MAX_LOCAL_WORKERS, NormalizedPath, PathStatus};
use crate::shell::IngestError;

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

/// Walks `root` in sorted order and folds each candidate through `step` in that order.
pub(super) fn fold_candidates<T>(
    root: &Path,
    output: Option<&Path>,
    init: T,
    step: impl FnMut(T, Candidate) -> Result<T, IngestError>,
) -> Result<T, IngestError> {
    let walker = sorted_walker(root);

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

fn worker_stopped() -> IngestError {
    IngestError::Io(io::Error::other("local read worker stopped"))
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
#[expect(
    clippy::needless_pass_by_value,
    reason = "dropping the senders when the producer exits disconnects both channels"
)]
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
#[expect(
    clippy::needless_pass_by_value,
    reason = "dropping the receiver on early error disconnects the slot channel and unblocks the producer"
)]
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

#[cfg(test)]
mod tests {
    use super::super::validate_root;
    use super::*;
    use crate::core::storage::cmp_path;
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
    fn early_abort_disconnects_and_joins_when_queue_overflows()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = temp_dir("early_abort")?;
        let root = validate_root(&temp)?;
        for file_index in 0..(LOCAL_CHANNEL_CAPACITY + 10) {
            fs::write(root.join(format!("file_{file_index:03}.txt")), "content")?;
        }

        let err = fold_candidates(&root, None, 0, |_acc, _candidate| {
            Err(IngestError::Io(io::Error::other(
                "abort on first candidate",
            )))
        });
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
}
