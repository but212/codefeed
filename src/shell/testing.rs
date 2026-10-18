//! Test fixtures and temporary directory guard for shell unit tests.

#![cfg(test)]

use std::error::Error;
use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};

use super::output::current_epoch_nanos;

#[derive(Debug)]
pub(super) struct TempDirGuard(PathBuf);

impl TempDirGuard {
    pub(super) fn new(label: &str) -> Result<Self, Box<dyn Error>> {
        let nanos = current_epoch_nanos();
        let path = std::env::temp_dir().join(format!("local_ingest_{label}_{nanos}"));
        fs::create_dir_all(&path)?;
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
