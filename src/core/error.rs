use std::fmt::{Display, Formatter, Result};

use crate::core::policy::MAX_OUTPUT_SIZE;

/// Diagnostic details for exceeded candidate limits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateLimitExceededError {
    /// The candidate limit that was exceeded.
    pub limit: usize,
    /// Top directories contributing to the candidate overflow.
    pub largest_directories: Vec<(String, usize)>,
}

/// Pure domain rule failure. Core returns this type only; effect failures belong to the shell.
#[derive(Debug)]
#[non_exhaustive]
pub enum DomainError {
    /// CLI usage error or missing argument.
    Usage(String),
    /// Candidate file budget exceeded.
    CandidateLimitExceeded(CandidateLimitExceededError),
    /// Source string or CLI argument is syntactically invalid.
    InvalidSource(String),
    /// Source scheme or host is unsupported (e.g. non-GitHub remote or SSH).
    UnsupportedSource(String),
    /// GitHub HTTP error response or rate limit exceeded.
    Api(String),
    /// Repository metadata or API response payload is malformed.
    InvalidMetadata(String),
    /// Rendered digest exceeds the maximum allowed output size.
    OutputTooLarge(Vec<(String, usize)>),
}

impl DomainError {
    /// Constructs a [`Usage`](Self::Usage) error from a descriptive message.
    #[cold]
    pub fn usage(message: impl Into<String>) -> Self {
        Self::Usage(message.into())
    }

    /// Constructs a [`CandidateLimitExceeded`](Self::CandidateLimitExceeded) error with diagnostic directory counts.
    #[cold]
    pub(crate) const fn candidate_limit_exceeded(
        limit: usize,
        largest_directories: Vec<(String, usize)>,
    ) -> Self {
        Self::CandidateLimitExceeded(CandidateLimitExceededError {
            limit,
            largest_directories,
        })
    }

    /// Constructs an [`InvalidSource`](Self::InvalidSource) error from a descriptive message.
    #[cold]
    pub(crate) fn invalid_source(message: impl Into<String>) -> Self {
        Self::InvalidSource(message.into())
    }

    /// Constructs an [`UnsupportedSource`](Self::UnsupportedSource) error from a descriptive message.
    #[cold]
    pub(crate) fn unsupported_source(message: impl Into<String>) -> Self {
        Self::UnsupportedSource(message.into())
    }

    /// Constructs an [`Api`](Self::Api) error from a descriptive message.
    #[cold]
    pub(crate) fn api(message: impl Into<String>) -> Self {
        Self::Api(message.into())
    }

    /// Constructs an [`InvalidMetadata`](Self::InvalidMetadata) error from a descriptive message.
    #[cold]
    pub(crate) fn invalid_metadata(message: impl Into<String>) -> Self {
        Self::InvalidMetadata(message.into())
    }

    /// Constructs an [`OutputTooLarge`](Self::OutputTooLarge) error with diagnostic largest file entries.
    #[cold]
    pub(crate) const fn output_too_large(largest_files: Vec<(String, usize)>) -> Self {
        Self::OutputTooLarge(largest_files)
    }
}

impl Display for DomainError {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        match self {
            Self::Usage(message) => write!(f, "{message}"),
            Self::CandidateLimitExceeded(err) => {
                write!(
                    f,
                    "repository contains more than {} files\nlargest directories:",
                    err.limit
                )?;
                for (dir, count) in &err.largest_directories {
                    write!(f, "\n  - {dir} ({count} files)")?;
                }
                Ok(())
            }
            Self::InvalidSource(message) => write!(f, "invalid source: {message}"),
            Self::UnsupportedSource(message) => write!(f, "unsupported source: {message}"),
            Self::Api(message) => write!(f, "GitHub API error: {message}"),
            Self::InvalidMetadata(message) => write!(f, "invalid repository metadata: {message}"),
            Self::OutputTooLarge(largest_files) => {
                let limit_mib = MAX_OUTPUT_SIZE / (1024 * 1024);
                write!(f, "digest exceeds the {limit_mib} MiB output limit")?;
                if !largest_files.is_empty() {
                    write!(f, "\nlargest files:")?;
                    for (path, size) in largest_files.as_slice() {
                        let formatted_size = format_size(*size);
                        write!(f, "\n  - {path} ({formatted_size})")?;
                    }
                }
                Ok(())
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

impl std::error::Error for DomainError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_too_large_message_derives_from_limit() {
        let mib = MAX_OUTPUT_SIZE / (1024 * 1024);
        assert_eq!(
            DomainError::output_too_large(Vec::new()).to_string(),
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
