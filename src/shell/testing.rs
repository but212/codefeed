//! Test fixtures and temporary directory guard for shell unit tests.

use std::error::Error;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use flate2::Compression;
use flate2::write::GzEncoder;
use tar::{Builder, EntryType};

static TEMP_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

static TEST_GITHUB_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());
static TEST_GITHUB_URL: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

pub(super) fn with_test_github_url<R>(url: String, f: impl FnOnce() -> R) -> R {
    let _lock = TEST_GITHUB_MUTEX.lock().unwrap();
    *TEST_GITHUB_URL.lock().unwrap() = Some(url);
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            *TEST_GITHUB_URL.lock().unwrap() = None;
        }
    }
    let _guard = Guard;
    f()
}

pub(super) fn get_test_github_url() -> Option<String> {
    TEST_GITHUB_URL
        .lock()
        .unwrap()
        .clone()
        .or_else(|| std::env::var("CODEFEED_GITHUB_API_URL").ok())
}

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
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
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

/// Builds a gzipped tarball by letting `fill` append entries, for cases that need raw entry headers.
pub(super) fn gzip_archive(fill: impl FnOnce(&mut Builder<&mut GzEncoder<Vec<u8>>>)) -> Vec<u8> {
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    {
        let mut tar = Builder::new(&mut gz);
        fill(&mut tar);
        tar.finish().unwrap();
    }
    gz.finish().unwrap()
}

/// Builds a gzipped tarball from `(path, content, entry type)` triples.
pub(super) fn gzip_tarball(entries: &[(&str, &[u8], EntryType)]) -> Vec<u8> {
    gzip_archive(|tar| {
        for &(name, content, entry_type) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(entry_type);
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, name, content).unwrap();
        }
    })
}

pub(super) struct MockServer {
    pub(super) port: u16,
    handle: thread::JoinHandle<(Vec<String>, usize)>,
}

impl MockServer {
    pub(super) fn spawn<F>(expected_connections: usize, mut handler: F) -> Self
    where
        F: FnMut(usize, &str, u16) -> Vec<u8> + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let port = listener.local_addr().expect("local addr").port();

        let handle = thread::spawn(move || {
            let mut recorded = Vec::new();
            let mut count = 0;

            for index in 0..expected_connections {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                count += 1;
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));

                let mut req_buf = Vec::new();
                let mut temp = [0u8; 512];
                loop {
                    if req_buf.windows(4).any(|w| w == b"\r\n\r\n") || req_buf.len() > (8192 - 1) {
                        break;
                    }
                    match stream.read(&mut temp) {
                        Ok(0) => break,
                        Ok(n) => req_buf.extend_from_slice(&temp[..n]),
                        Err(ref e)
                            if e.kind() == std::io::ErrorKind::WouldBlock
                                || e.kind() == std::io::ErrorKind::TimedOut =>
                        {
                            break;
                        }
                        Err(_) => break,
                    }
                }

                let req_str = String::from_utf8_lossy(&req_buf).to_string();
                recorded.push(req_str.clone());

                let response_bytes = handler(index, &req_str, port);
                if !response_bytes.is_empty() {
                    let _ = stream.write_all(&response_bytes);
                }
            }

            (recorded, count)
        });

        Self { port, handle }
    }

    pub(super) fn join(self) -> (Vec<String>, usize) {
        self.handle.join().expect("mock server thread finish")
    }

    pub(super) fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }
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
