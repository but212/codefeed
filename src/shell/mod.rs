//! Imperative shell: process, filesystem, network, and orchestration.
//!
//! Shell modules perform all effects and feed observations to the functional
//! core in `crate::core`.

pub(crate) mod github;
pub(crate) mod local;
pub(crate) mod orchestrate;
pub(crate) mod output;

pub(crate) fn current_epoch_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
#[derive(Debug)]
pub(crate) struct TempDirGuard(std::path::PathBuf);

#[cfg(test)]
impl TempDirGuard {
    pub(crate) fn new(label: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let nanos = current_epoch_nanos();
        let path = std::env::temp_dir().join(format!("local_ingest_{label}_{nanos}"));
        std::fs::create_dir_all(&path)?;
        Ok(Self(std::fs::canonicalize(&path)?))
    }
}

#[cfg(test)]
impl std::ops::Deref for TempDirGuard {
    type Target = std::path::Path;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
impl AsRef<std::path::Path> for TempDirGuard {
    fn as_ref(&self) -> &std::path::Path {
        &self.0
    }
}

#[cfg(test)]
impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
pub(crate) fn temp_dir(label: &str) -> Result<TempDirGuard, Box<dyn std::error::Error>> {
    TempDirGuard::new(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_dir_guard_cleans_up_on_drop_and_panic() -> Result<(), Box<dyn std::error::Error>> {
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
        let mut panic_path = std::path::PathBuf::new();
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
