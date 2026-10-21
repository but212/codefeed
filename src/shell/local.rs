//! Local directory collection orchestration: parallel walk, bounded I/O, linear selection.
//!
//! # Concurrency & Performance Architecture
//! - **Producer-Consumer Pipeline**: Directory traversal and file reads run concurrently
//!   via `ignore::WalkParallel` worker threads, piped through a bounded `sync_channel(128)`.
//!   Eliminates single-threaded OS kernel `CreateFileW` latency without unbound memory growth.
//! - **FCIS Invariant**: Thread workers perform side-effecting I/O and pre-open symlink
//!   checks (`fs::symlink_metadata`), delegating pure linear accumulation to `Selector` on
//!   the main thread inside `std::thread::scope`.
//! - **Determinism & Cancellation**: Storage ordering is guaranteed post-collection via
//!   `SortedFiles` (`cmp_path`). Errors or budget overflows abort workers immediately via
//!   `AtomicBool` and receiver disconnection.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::sync_channel;

use ignore::{WalkBuilder, WalkState};

use crate::core::error::IngestError;
use crate::core::model::CollectionResult;
use crate::core::policy::{MAX_FILE_SIZE, PathStatus, classify_path_segments, is_hidden_segment};
use crate::core::selector::{Admission, Selector, exceeds_size_limits};

enum WorkerMsg {
    /// `content` is `None` when the worker skipped the read, or the file was unlinked after discovery.
    Candidate {
        status: PathStatus,
        size_hint: Option<u64>,
        content: Option<Vec<u8>>,
    },
    Error(IngestError),
}

/// Collects files from a local directory root, excluding the resolved output file if located inside it.
///
/// Uses parallel workers to concurrently evaluate path policy, inspect metadata (rejecting symlinks
/// pre-open), and read file contents into pre-allocated buffers. Results are streamed through a
/// bounded channel to maintain backpressure and preserve pure sequential accumulation in `Selector`.
pub(super) fn collect(
    root: &Path,
    resolved_output: Option<&Path>,
) -> Result<CollectionResult, IngestError> {
    let canonical_root = validate_root(root)?;
    let label = local_label(&canonical_root);

    let walker = WalkBuilder::new(&canonical_root)
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
        .build_parallel();

    let (tx, rx) = sync_channel::<WorkerMsg>(128);
    let aborted = AtomicBool::new(false);

    let selector = std::thread::scope(|s| {
        s.spawn(|| {
            let mut visitor = LocalVisitor {
                root: &canonical_root,
                resolved_output,
                tx,
                aborted: &aborted,
            };
            walker.visit(&mut visitor);
        });

        drain_channel(rx, &aborted)
    })?;

    Ok(selector.finish(label, None))
}

/// Walker visitor. `build` clones it per worker thread; every clone shares the channel sender and abort flag.
#[derive(Clone)]
struct LocalVisitor<'a> {
    root: &'a Path,
    resolved_output: Option<&'a Path>,
    tx: std::sync::mpsc::SyncSender<WorkerMsg>,
    aborted: &'a AtomicBool,
}

impl<'s> ignore::ParallelVisitorBuilder<'s> for LocalVisitor<'s> {
    fn build(&mut self) -> Box<dyn ignore::ParallelVisitor + 's> {
        Box::new(self.clone())
    }
}

impl LocalVisitor<'_> {
    fn fail(&self, err: IngestError) -> WalkState {
        self.aborted.store(true, Ordering::Relaxed);
        let _ = self.tx.send(WorkerMsg::Error(err));
        WalkState::Quit
    }

    fn process_entry(&self, entry: ignore::DirEntry) -> Result<Option<WorkerMsg>, IngestError> {
        let entry_path = entry.path();
        if entry_path == self.root
            || self
                .resolved_output
                .is_some_and(|target| entry_path == target)
        {
            return Ok(None);
        }

        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            return Ok(None);
        }

        let relative = entry_path
            .strip_prefix(self.root)
            .map_err(|err| IngestError::invalid_metadata(err.to_string()))?;

        let path = match classify_local_path(relative)? {
            PathStatus::Ignored => return Ok(None),
            status @ PathStatus::NonUtf8Candidate(_) => {
                return Ok(Some(WorkerMsg::Candidate {
                    status,
                    size_hint: None,
                    content: None,
                }));
            }
            PathStatus::Valid(path) => path,
        };

        let meta = match fs::symlink_metadata(entry_path) {
            Ok(m) => m,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(IngestError::Io(err)),
        };

        if meta.is_symlink() || !meta.is_file() {
            return Ok(None);
        }

        let file_len = meta.len();
        let mut content = None;
        if !exceeds_size_limits(path.len(), Some(file_len)) {
            let mut bytes = Vec::new();
            if read_file_content_bounded(entry_path, file_len, MAX_FILE_SIZE, &mut bytes)? {
                content = Some(bytes);
            }
        }
        Ok(Some(WorkerMsg::Candidate {
            status: PathStatus::Valid(path),
            size_hint: Some(file_len),
            content,
        }))
    }
}

impl ignore::ParallelVisitor for LocalVisitor<'_> {
    fn visit(&mut self, entry: Result<ignore::DirEntry, ignore::Error>) -> WalkState {
        if self.aborted.load(Ordering::Relaxed) {
            return WalkState::Quit;
        }

        let entry = match entry {
            Ok(e) => e,
            Err(err) => return self.fail(IngestError::Io(format_ignore_error(&err))),
        };

        match self.process_entry(entry) {
            Ok(Some(msg)) => {
                if self.tx.send(msg).is_err() {
                    WalkState::Quit
                } else {
                    WalkState::Continue
                }
            }
            Ok(None) => WalkState::Continue,
            Err(err) => self.fail(err),
        }
    }
}

fn apply_msg(selector: Selector, msg: WorkerMsg) -> Result<Selector, IngestError> {
    match msg {
        WorkerMsg::Candidate {
            status,
            size_hint,
            content,
        } => match selector.admit(status, size_hint)? {
            Admission::Settled(s) => Ok(s),
            Admission::Pending(pending) => match content {
                Some(bytes) => pending.accept(&bytes),
                None => Ok(pending.skip()),
            },
        },
        WorkerMsg::Error(err) => Err(err),
    }
}

fn drain_channel(
    rx: std::sync::mpsc::Receiver<WorkerMsg>,
    aborted: &AtomicBool,
) -> Result<Selector, IngestError> {
    let mut selector = Selector::new();
    while let Ok(msg) = rx.recv() {
        match apply_msg(selector, msg) {
            Ok(next) => selector = next,
            Err(err) => {
                aborted.store(true, Ordering::Relaxed);
                drop(rx);
                return Err(err);
            }
        }
    }
    Ok(selector)
}

/// Reads up to `limit + 1` bytes of `path` into `buf`; the extra byte lets the caller detect overflow.
/// Returns `Ok(false)` when the file was unlinked after discovery.
fn read_file_content_bounded(
    path: &Path,
    file_len: u64,
    limit: u64,
    buf: &mut Vec<u8>,
) -> Result<bool, IngestError> {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(IngestError::Io(err)),
    };
    read_bounded_content_into(file, file_len, limit, buf)?;
    Ok(true)
}

fn read_bounded_content_into(
    file: fs::File,
    file_len: u64,
    limit: u64,
    buf: &mut Vec<u8>,
) -> io::Result<()> {
    let to_read = file_len.min(limit.saturating_add(1)) as usize;
    let needed = buf
        .len()
        .saturating_add(to_read)
        .saturating_sub(buf.capacity());
    buf.reserve(needed);
    let mut handle = file.take(limit.saturating_add(1));
    handle.read_to_end(buf)?;
    Ok(())
}

fn validate_root(root: &Path) -> Result<PathBuf, IngestError> {
    let root_display = root.display();
    let canonical = fs::canonicalize(root)
        .map_err(|error| IngestError::invalid_source(format!("{root_display} ({error})")))?;
    if !canonical.is_dir() {
        let display = canonical.display();
        return Err(IngestError::invalid_source(format!(
            "{display} is not a directory"
        )));
    }
    Ok(canonical)
}

fn classify_local_path(relative: &Path) -> Result<PathStatus, IngestError> {
    let segments = relative
        .components()
        .map(|c| c.as_os_str().as_encoded_bytes());
    classify_path_segments(segments)
        .map_err(|_| IngestError::invalid_metadata("local path exceeds maximum directory depth"))
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
    root.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("root")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::super::testing::temp_dir;
    use super::*;
    use crate::core::model::{DigestFormat, OutputTarget, RunConfig, Source};
    use crate::core::storage::{FileRecord, cmp_path};
    use crate::ingest;

    #[test]
    fn read_local_file_enforces_size_boundaries() -> Result<(), Box<dyn std::error::Error>> {
        use crate::core::policy::MAX_FILE_SIZE;

        let root = temp_dir("size_boundaries")?;
        let exact_path = root.join("exact.txt");
        let too_large_path = root.join("large.txt");
        fs::write(&exact_path, vec![b'x'; MAX_FILE_SIZE as usize])?;
        fs::write(&too_large_path, vec![b'x'; MAX_FILE_SIZE as usize + 1])?;

        let mut buf = Vec::new();
        assert!(read_file_content_bounded(
            &exact_path,
            MAX_FILE_SIZE,
            MAX_FILE_SIZE,
            &mut buf
        )?);
        assert_eq!(buf.len(), MAX_FILE_SIZE as usize);

        buf.clear();
        assert!(read_file_content_bounded(
            &too_large_path,
            MAX_FILE_SIZE + 1,
            MAX_FILE_SIZE,
            &mut buf
        )?);
        assert_eq!(
            buf.len(),
            MAX_FILE_SIZE as usize + 1,
            "one sentinel byte past the limit"
        );

        let res = collect(&root, None)?;
        assert_eq!(res.files.len(), 1, "exact-size file is included");
        assert_eq!(res.skipped, 1, "one byte over the limit is skipped");
        Ok(())
    }

    #[test]
    fn read_bounded_content_preallocates_capacity() -> Result<(), Box<dyn std::error::Error>> {
        use crate::core::policy::MAX_FILE_SIZE;

        let root = temp_dir("preallocate_test")?;
        let file_path = root.join("test.txt");
        let payload = vec![b'y'; 64 * 1024];
        fs::write(&file_path, &payload)?;

        let mut buf = Vec::new();
        assert!(read_file_content_bounded(
            &file_path,
            payload.len() as u64,
            MAX_FILE_SIZE,
            &mut buf
        )?);
        assert_eq!(buf, payload);
        assert!(buf.capacity() >= 64 * 1024);
        Ok(())
    }

    #[test]
    fn handles_unlinked_file_as_missing() -> Result<(), Box<dyn std::error::Error>> {
        use crate::core::policy::MAX_FILE_SIZE;

        let root = temp_dir("missing_file")?;
        let path = root.join("nonexistent.txt");

        let mut buf = Vec::new();
        assert!(!read_file_content_bounded(
            &path,
            0,
            MAX_FILE_SIZE,
            &mut buf
        )?);
        assert!(buf.is_empty());
        Ok(())
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

        let res = collect(&root, None)?;
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
            let res = collect(&root, None)?;
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
        let locked = root.join("locked");
        fs::create_dir(&locked)?;
        fs::write(locked.join("file.txt"), "content")?;
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o0000))?;

        if fs::read_dir(&locked).is_ok() {
            let _ = fs::set_permissions(&locked, fs::Permissions::from_mode(0o0755));
            return Ok(());
        }

        let res = collect(&root, None);

        // Restore permissions so TempDirGuard can clean up
        let _ = fs::set_permissions(&locked, fs::Permissions::from_mode(0o0755));

        let err = res.unwrap_err();
        assert!(
            matches!(&err, IngestError::Io(io_err) if io_err.kind() == std::io::ErrorKind::PermissionDenied),
            "expected IngestError::Io with ErrorKind::PermissionDenied, got: {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("locked"),
            "error message should contain path context 'locked', got: {msg}"
        );
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
        let res = collect(&root, None)?;
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
    fn walk_preserves_deterministic_order() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("parallel_order")?;
        for dir in ["b_dir", "a_dir", "c_dir", "src-test", "src"] {
            fs::create_dir_all(root.join(dir))?;
            for file_index in 0..10 {
                fs::write(
                    root.join(dir).join(format!("file_{file_index:02}.txt")),
                    "content",
                )?;
            }
        }
        let result = collect(&root, None)?;
        let files = result.files;
        assert_eq!(files.len(), 50);
        let records: Vec<FileRecord<'_>> = files.iter().collect();
        for window in records.windows(2) {
            assert!(
                cmp_path(window[0].path, window[1].path).is_lt(),
                "ordering violated: {} >= {}",
                window[0].path,
                window[1].path
            );
        }
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

        let result = collect(&root, None)?;
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files.iter().next().unwrap().path, "visible.txt");
        assert_eq!(result.skipped, 0);
        Ok(())
    }
}
