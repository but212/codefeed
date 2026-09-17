//! Single source of truth for numeric ingestion limits.

pub(crate) const MAX_FILE_SIZE: u64 = 1024 * 1024;
pub(crate) const MAX_OUTPUT_SIZE: u64 = 20 * 1024 * 1024;
pub(crate) const MAX_FILE_COUNT: usize = 10_000;
pub(crate) const MAX_JSON_SIZE: u64 = 8 * 1024 * 1024;
pub(crate) const MAX_REMOTE_IGNORE_FILES: usize = 256;
pub(crate) const MAX_REMOTE_IGNORE_BYTES: u64 = 8 * 1024 * 1024;
