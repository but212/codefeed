//! Temporary directory guard shared by shell and CLI tests.

use std::fs;
use std::io;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub(crate) struct TempDirGuard(PathBuf);

impl TempDirGuard {
    pub(super) fn create_in(parent: &Path, label: &str) -> io::Result<Self> {
        let pid = std::process::id();
        loop {
            let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!("codefeed_{label}_{pid}_{counter}"));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
                Err(err) => return Err(err),
            }
        }
    }
}

impl Deref for TempDirGuard {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for TempDirGuard {
    fn as_ref(&self) -> &Path {
        self
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
