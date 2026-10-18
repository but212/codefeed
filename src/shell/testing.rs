//! Test fixtures and temporary directory guard for shell unit tests.

#![cfg(test)]

use std::error::Error;
use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub(super) struct TempDirGuard(PathBuf);

impl TempDirGuard {
    pub(super) fn new(label: &str) -> Result<Self, Box<dyn Error>> {
        let pid = std::process::id();
        let path = loop {
            let counter = TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
            let candidate =
                std::env::temp_dir().join(format!("local_ingest_{label}_{pid}_{counter}"));
            match fs::create_dir(&candidate) {
                Ok(()) => break candidate,
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(err) => return Err(Box::new(err)),
            }
        };
        Ok(Self(fs::canonicalize(&path)?))
    }
}

impl Deref for TempDirGuard {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<Path> for TempDirGuard {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(super) fn temp_dir(label: &str) -> Result<TempDirGuard, Box<dyn Error>> {
    TempDirGuard::new(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_dir_guard_cleans_up_on_drop_and_panic() -> Result<(), Box<dyn Error>> {
        // 1. Drop cleanup
        let drop_path = {
            let guard = temp_dir("test_guard_drop")?;
            assert!(guard.is_dir());
            guard.to_path_buf()
        };
        assert!(
            !drop_path.exists(),
            "directory should be removed on guard drop"
        );

        // 2. Panic cleanup
        let mut panic_path = PathBuf::new();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let guard = temp_dir("test_guard_panic").unwrap();
            panic_path = guard.to_path_buf();
            assert!(guard.is_dir());
            panic!("intentional panic inside test");
        }));
        assert!(result.is_err());
        assert!(
            !panic_path.exists(),
            "directory should be removed even on panic"
        );
        Ok(())
    }
}
