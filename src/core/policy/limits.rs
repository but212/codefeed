//! Numerical limits, timeouts, and compile-time boundary assertions.

/// Maximum individual file size accepted (1 MiB in bytes). Larger files are skipped.
pub(crate) const MAX_FILE_SIZE: u64 = 1024 * 1024;
/// Maximum rendered digest size (20 MiB in bytes). Overflows return [`DomainError::OutputTooLarge`](crate::core::error::DomainError::OutputTooLarge).
pub(crate) const MAX_OUTPUT_SIZE: u64 = 20 * 1024 * 1024;
/// Maximum candidate files evaluated before candidate budget overflows (10,000 files).
pub(crate) const MAX_FILE_COUNT: usize = 10_000;
/// Maximum directory nesting depth permitted for archive entry paths (256 levels).
pub(crate) const MAX_PATH_DEPTH: usize = 256;
/// Maximum allowable path length in bytes (4,096 bytes). Paths exceeding this limit are skipped.
pub(crate) const MAX_PATH_BYTES: u64 = 4096;
/// Capacity in bytes for buffering incoming archive decompression streams (64 KiB).
pub(crate) const ARCHIVE_BUFFER_BYTES: usize = 64 * 1024;
/// Capacity in messages of each local pipeline channel (jobs and slots). A full channel blocks its
/// sender, which bounds the file bodies held at once to this value plus two (security §5.5, §8).
pub(crate) const LOCAL_CHANNEL_CAPACITY: usize = 64;
/// Maximum local read workers. Fewer available cores use fewer workers (security §5.5, §8).
pub(crate) const MAX_LOCAL_WORKERS: usize = 12;

/// Maximum duration in seconds allowed for whole-archive Tarball transfer (5 minutes).
pub(crate) const ARCHIVE_MAX_SECS: u64 = 300;
/// Maximum total HTTP request attempts (1 initial + up to 2 retries).
pub(crate) const MAX_ATTEMPTS: usize = 3;
/// Fast byte-level token heuristic constant for English text and source code.
pub(crate) const ASCII_BYTES_PER_TOKEN: u64 = 4;
/// Base initial delay in milliseconds for exponential backoff.
pub(crate) const RETRY_INITIAL_DELAY_MS: u64 = 250;
/// Maximum delay cap in milliseconds for exponential backoff.
pub(crate) const RETRY_MAX_DELAY_MS: u64 = 5_000;
/// HTTP response headers timeout in seconds for GitHub API requests (30 seconds).
pub(crate) const GITHUB_TIMEOUT_SECS: u64 = 30;

const _: () = {
    assert!(MAX_OUTPUT_SIZE + MAX_FILE_SIZE + MAX_PATH_BYTES <= u32::MAX as u64);
    assert!(MAX_FILE_SIZE <= MAX_OUTPUT_SIZE);
    assert!(MAX_PATH_BYTES <= MAX_FILE_SIZE);
    assert!(MAX_FILE_COUNT > 0);
    assert!(MAX_PATH_DEPTH > 0);
    assert!(ASCII_BYTES_PER_TOKEN > 0);
    assert!(GITHUB_TIMEOUT_SECS <= ARCHIVE_MAX_SECS);
    assert!(ARCHIVE_BUFFER_BYTES > 0);
    assert!(LOCAL_CHANNEL_CAPACITY > 0);
    assert!(MAX_LOCAL_WORKERS > 0);
};
