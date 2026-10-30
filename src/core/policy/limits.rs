//! Numerical limits, timeouts, and compile-time boundary assertions.

/// Maximum individual file size accepted (1 MiB in bytes). Larger files are skipped.
pub(crate) const MAX_FILE_SIZE: u64 = 1024 * 1024;
/// Companion `usize` constant for [`MAX_FILE_SIZE`] used in buffer allocations and tests.
#[cfg(test)]
#[expect(
    clippy::cast_possible_truncation,
    reason = "MAX_FILE_SIZE (1 MiB) fits in usize on all supported targets"
)]
pub(crate) const MAX_FILE_SIZE_USIZE: usize = MAX_FILE_SIZE as usize;
/// Maximum rendered digest size (20 MiB in bytes). Overflows return [`DomainError::OutputTooLarge`](crate::core::error::DomainError::OutputTooLarge).
pub(crate) const MAX_OUTPUT_SIZE: u64 = 20 * 1024 * 1024;
/// Companion `usize` constant for [`MAX_OUTPUT_SIZE`] used in buffer allocations and tests.
#[cfg(test)]
#[expect(
    clippy::cast_possible_truncation,
    reason = "MAX_OUTPUT_SIZE (20 MiB) fits in usize on all supported targets"
)]
pub(crate) const MAX_OUTPUT_SIZE_USIZE: usize = MAX_OUTPUT_SIZE as usize;
/// Maximum candidate files evaluated before candidate budget overflows (10,000 files).
pub(crate) const MAX_FILE_COUNT: usize = 10_000;
/// Maximum directory nesting depth permitted for archive entry paths (256 levels).
pub(crate) const MAX_PATH_DEPTH: usize = 256;
/// Maximum allowable path length in bytes (4,096 bytes). Paths exceeding this limit are skipped.
pub(crate) const MAX_PATH_BYTES: u64 = 4096;
/// Companion `usize` constant for [`MAX_PATH_BYTES`] used in buffer allocations and tests.
#[cfg(test)]
#[expect(
    clippy::cast_possible_truncation,
    reason = "MAX_PATH_BYTES (4 KiB) fits in usize on all supported targets"
)]
pub(crate) const MAX_PATH_BYTES_USIZE: usize = MAX_PATH_BYTES as usize;
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
pub(crate) const ASCII_BYTES_PER_TOKEN: usize = 4;
/// Base initial delay in milliseconds for exponential backoff.
pub(crate) const RETRY_INITIAL_DELAY_MS: u64 = 250;
/// Maximum delay cap in milliseconds for exponential backoff.
pub(crate) const RETRY_MAX_DELAY_MS: u64 = 5_000;
/// HTTP response headers timeout in seconds for GitHub API requests (30 seconds).
pub(crate) const GITHUB_TIMEOUT_SECS: u64 = 30;

#[expect(
    clippy::cast_possible_truncation,
    reason = "compile-time assertion specifically verifies that these limits fit in usize without truncation"
)]
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
    assert!(MAX_FILE_SIZE as usize as u64 == MAX_FILE_SIZE);
    assert!(MAX_OUTPUT_SIZE as usize as u64 == MAX_OUTPUT_SIZE);
    assert!(MAX_PATH_BYTES as usize as u64 == MAX_PATH_BYTES);
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn format_with_commas(mut n: u64) -> String {
        if n == 0 {
            return "0".to_string();
        }
        let mut parts = Vec::new();
        while n > 0 {
            let rem = n % 1000;
            n /= 1000;
            if n > 0 {
                parts.push(format!("{rem:03}"));
            } else {
                parts.push(format!("{rem}"));
            }
        }
        parts.reverse();
        parts.join(",")
    }

    fn parse_limits_table(spec: &str) -> HashMap<&str, &str> {
        let sec8_start = spec
            .split("## 8. Limit Values")
            .nth(1)
            .expect("security_and_filtering.md must contain '## 8. Limit Values'");
        let sec8_body = sec8_start.split("\n## ").next().unwrap_or(sec8_start);

        let mut table = HashMap::new();
        for line in sec8_body.lines().filter(|line| line.starts_with("| `")) {
            let cols: Vec<&str> = line.split('|').map(str::trim).collect();
            assert!(
                cols.len() >= 4,
                "table row does not have at least 4 pipe-delimited columns: {line}"
            );
            let constant = cols[1].trim_matches('`');
            let value = cols[2];
            let previous = table.insert(constant, value);
            assert!(
                previous.is_none(),
                "duplicate constant row detected in table: {constant}"
            );
        }
        table
    }

    #[test]
    fn format_with_commas_formats_numbers_correctly() {
        assert_eq!(format_with_commas(0), "0");
        assert_eq!(format_with_commas(999), "999");
        assert_eq!(format_with_commas(1000), "1,000");
        assert_eq!(format_with_commas(4096), "4,096");
        assert_eq!(format_with_commas(10_000), "10,000");
        assert_eq!(format_with_commas(1_000_000), "1,000,000");
    }

    #[test]
    fn limit_constants_match_security_specification_table() {
        let spec = include_str!("../../../docs/security_and_filtering.md");
        let table = parse_limits_table(spec);

        let expected = [
            (
                "MAX_FILE_SIZE",
                format!("{} MiB", MAX_FILE_SIZE / (1024 * 1024)),
            ),
            (
                "MAX_PATH_BYTES",
                format!("{} B", format_with_commas(MAX_PATH_BYTES)),
            ),
            ("MAX_PATH_DEPTH", format!("{MAX_PATH_DEPTH} levels")),
            (
                "MAX_FILE_COUNT",
                format!("{} files", format_with_commas(MAX_FILE_COUNT as u64)),
            ),
            (
                "MAX_OUTPUT_SIZE",
                format!("{} MiB", MAX_OUTPUT_SIZE / (1024 * 1024)),
            ),
            (
                "ARCHIVE_BUFFER_BYTES",
                format!("{} KiB", ARCHIVE_BUFFER_BYTES / 1024),
            ),
            (
                "LOCAL_CHANNEL_CAPACITY",
                format!("{LOCAL_CHANNEL_CAPACITY} messages"),
            ),
            ("MAX_LOCAL_WORKERS", format!("{MAX_LOCAL_WORKERS} threads")),
            ("ARCHIVE_MAX_SECS", format!("{ARCHIVE_MAX_SECS} s")),
            ("GITHUB_TIMEOUT_SECS", format!("{GITHUB_TIMEOUT_SECS} s")),
            ("MAX_ATTEMPTS", format!("{MAX_ATTEMPTS} attempts")),
            (
                "RETRY_INITIAL_DELAY_MS",
                format!("{RETRY_INITIAL_DELAY_MS} ms"),
            ),
            (
                "RETRY_MAX_DELAY_MS",
                format!("{} ms", format_with_commas(RETRY_MAX_DELAY_MS)),
            ),
            (
                "ASCII_BYTES_PER_TOKEN",
                format!("{ASCII_BYTES_PER_TOKEN} B"),
            ),
        ];

        assert_eq!(
            table.len(),
            expected.len(),
            "number of rows in sec 8 table must match the number of expected constants"
        );

        for (constant_name, expected_value) in &expected {
            let actual_value = table.get(constant_name).unwrap_or_else(|| {
                panic!("security §8 table missing entry for {constant_name}");
            });
            assert_eq!(
                *actual_value, expected_value,
                "mismatch for {constant_name}: code has {expected_value}, but docs §8 has {actual_value}"
            );
        }
    }
}
