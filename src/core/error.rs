use std::fmt::{Display, Formatter, Result};
use std::io;

use crate::core::policy::MAX_OUTPUT_SIZE;

/// Domain error encountered during ingestion or CLI execution.
#[derive(Debug)]
pub enum IngestError {
    /// CLI usage error or missing argument.
    Usage(String),
    /// Candidate file budget exceeded.
    CandidateLimitExceeded {
        /// The candidate limit that was exceeded.
        limit: usize,
        /// Top directories contributing to the candidate overflow.
        largest_directories: Vec<(String, usize)>,
    },
    /// Source string or CLI argument is syntactically invalid.
    InvalidSource(String),
    /// Source scheme or host is unsupported (e.g. non-GitHub remote or SSH).
    UnsupportedSource(String),
    /// Local filesystem I/O failure.
    Io(io::Error),
    /// Network transport or connection failure contacting GitHub.
    Http(String),
    /// GitHub HTTP error response or rate limit exceeded.
    Api(String),
    /// Repository metadata or API response payload is malformed.
    InvalidMetadata(String),
    /// Rendered digest exceeds the maximum allowed output size.
    OutputTooLarge {
        /// Up to five largest files contributing to the digest overflow.
        largest_files: Vec<(String, usize)>,
    },
    /// Path component length exceeds maximum allowed limit.
    PathComponentTooLong {
        /// File path containing the oversized component.
        path: String,
        /// Actual byte length of the oversized component.
        len: usize,
        /// Maximum allowed component byte length.
        max: usize,
    },
}

impl Display for IngestError {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        match self {
            Self::Usage(message) => write!(f, "{message}"),
            Self::CandidateLimitExceeded {
                limit,
                largest_directories,
            } => {
                write!(
                    f,
                    "repository contains more than {limit} files\nlargest directories:"
                )?;
                for (dir, count) in largest_directories {
                    write!(f, "\n  - {dir} ({count} files)")?;
                }
                Ok(())
            }
            Self::InvalidSource(message) => write!(f, "invalid source: {message}"),
            Self::UnsupportedSource(message) => write!(f, "unsupported source: {message}"),
            Self::Io(error) => write!(f, "filesystem error: {error}"),
            Self::Http(message) => write!(f, "GitHub request failed: {message}"),
            Self::Api(message) => write!(f, "GitHub API error: {message}"),
            Self::InvalidMetadata(message) => write!(f, "invalid repository metadata: {message}"),
            Self::OutputTooLarge { largest_files } => {
                let limit_mib = MAX_OUTPUT_SIZE / (1024 * 1024);
                write!(f, "digest exceeds the {limit_mib} MiB output limit")?;
                if !largest_files.is_empty() {
                    write!(f, "\nlargest files:")?;
                    for (path, size) in largest_files {
                        let formatted_size = format_size(*size);
                        write!(f, "\n  - {path} ({formatted_size})")?;
                    }
                }
                Ok(())
            }
            Self::PathComponentTooLong { path, len, max } => {
                write!(
                    f,
                    "path component in '{path}' is {len} bytes; maximum is {max}"
                )
            }
        }
    }
}

const KIB: f64 = 1024.0;
const MIB: f64 = 1024.0 * 1024.0;
// With "{:.1}", any value >= 1023.95 KiB rounds to "1024.0 KiB", colliding with MiB.
// 1023.95 * 1024 = 1_048_524.8, so values >= 1_048_525 must be formatted as MiB.
const MIB_ROUNDING_THRESHOLD: usize = 1_048_525;

fn format_size(bytes: usize) -> String {
    match bytes {
        b @ MIB_ROUNDING_THRESHOLD.. => format!("{:.1} MiB", b as f64 / MIB),
        b @ 1024.. => format!("{:.1} KiB", b as f64 / KIB),
        b => format!("{b} B"),
    }
}

impl std::error::Error for IngestError {}

impl From<io::Error> for IngestError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_too_large_message_derives_from_limit() {
        let mib = MAX_OUTPUT_SIZE / (1024 * 1024);
        assert_eq!(
            IngestError::OutputTooLarge {
                largest_files: Vec::new()
            }
            .to_string(),
            format!("digest exceeds the {mib} MiB output limit")
        );
    }

    #[test]
    fn format_size_boundary_cases() {
        let cases = [
            (0, "0 B"),
            (1023, "1023 B"),
            (1024, "1.0 KiB"),
            (1_048_524, "1023.9 KiB"),
            (1_048_525, "1.0 MiB"),
            (1_048_575, "1.0 MiB"),
            (1_048_576, "1.0 MiB"),
            (2_097_152, "2.0 MiB"),
        ];

        for (bytes, expected) in cases {
            assert_eq!(format_size(bytes), expected, "failed for {bytes} bytes");
        }
    }
}
