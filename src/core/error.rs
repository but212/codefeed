use std::fmt::{Display, Formatter, Result};
use std::io;

use crate::core::policy::MAX_OUTPUT_SIZE;

/// Domain error encountered during ingestion or CLI execution.
#[derive(Debug)]
pub enum IngestError {
    /// CLI usage error or missing argument.
    Usage(String),
    /// Candidate file budget exceeded.
    CandidateLimitExceeded(String),
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
}

impl Display for IngestError {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        match self {
            Self::Usage(message) => write!(f, "{message}"),
            Self::CandidateLimitExceeded(message) => write!(f, "{message}"),
            Self::InvalidSource(message) => write!(f, "invalid source: {message}"),
            Self::UnsupportedSource(message) => write!(f, "unsupported source: {message}"),
            Self::Io(error) => write!(f, "filesystem error: {error}"),
            Self::Http(message) => write!(f, "GitHub request failed: {message}"),
            Self::Api(message) => write!(f, "GitHub API error: {message}"),
            Self::InvalidMetadata(message) => write!(f, "invalid repository metadata: {message}"),
            Self::OutputTooLarge { largest_files } => {
                write!(
                    f,
                    "digest exceeds the {} MiB output limit",
                    MAX_OUTPUT_SIZE / (1024 * 1024)
                )?;
                if !largest_files.is_empty() {
                    write!(f, "\nlargest files:")?;
                    for (path, size) in largest_files {
                        write!(f, "\n  - {path} ({})", format_size(*size))?;
                    }
                }
                Ok(())
            }
        }
    }
}

fn format_size(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
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
}
