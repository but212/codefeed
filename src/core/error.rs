use std::fmt::{Display, Formatter, Result};
use std::io;

#[derive(Debug)]
pub enum IngestError {
    InvalidSource(String),
    UnsupportedSource(String),
    Io(io::Error),
    Http(String),
    Api(String),
    InvalidMetadata(String),
    OutputTooLarge,
}

impl Display for IngestError {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        match self {
            Self::InvalidSource(message) => write!(f, "invalid source: {message}"),
            Self::UnsupportedSource(message) => write!(f, "unsupported source: {message}"),
            Self::Io(error) => write!(f, "filesystem error: {error}"),
            Self::Http(message) => write!(f, "GitHub request failed: {message}"),
            Self::Api(message) => write!(f, "GitHub API error: {message}"),
            Self::InvalidMetadata(message) => write!(f, "invalid repository metadata: {message}"),
            Self::OutputTooLarge => write!(
                f,
                "digest exceeds the {} MiB output limit",
                crate::core::limits::MAX_OUTPUT_SIZE / (1024 * 1024)
            ),
        }
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
    use crate::core::limits::MAX_OUTPUT_SIZE;

    #[test]
    fn output_too_large_message_derives_from_limit() {
        let mib = MAX_OUTPUT_SIZE / (1024 * 1024);
        assert_eq!(
            IngestError::OutputTooLarge.to_string(),
            format!("digest exceeds the {mib} MiB output limit")
        );
    }
}
