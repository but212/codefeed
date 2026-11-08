//! Ingestion error for the imperative shell: domain failures plus effect failures.

use std::fmt::{Display, Formatter, Result};
use std::io;
use std::path::Path;

use crate::core::error::{DomainError, ErrorDetail};
use crate::core::github::GithubApiError;

/// Error returned by ingestion. Domain rule failures stay in [`DomainError`]; effect failures
/// (filesystem, network) are added here by the shell, so core never constructs them.
#[derive(Debug)]
#[non_exhaustive]
pub enum IngestError {
    /// A domain rule rejected the input or the collected repository.
    Domain(DomainError),
    /// Local filesystem I/O failure.
    Io(io::Error),
    /// Network transport or connection failure contacting GitHub.
    Http(ErrorDetail),
    /// GitHub returned a non-success HTTP status.
    Api(GithubApiError),
}

impl Display for IngestError {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        match self {
            Self::Domain(error) => write!(f, "{error}"),
            Self::Io(error) => write!(f, "filesystem error: {error}"),
            Self::Http(message) => write!(f, "GitHub request failed: {message}"),
            Self::Api(error) => write!(f, "GitHub API error: {error}"),
        }
    }
}

impl std::error::Error for IngestError {}

impl From<DomainError> for IngestError {
    fn from(error: DomainError) -> Self {
        Self::Domain(error)
    }
}

impl From<io::Error> for IngestError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Creates an `io::Error` with message sanitized of control characters.
pub(super) fn sanitized_io_error(kind: io::ErrorKind, message: &str) -> io::Error {
    io::Error::new(kind, ErrorDetail::new(message).to_string())
}

/// I/O failure on `path`. Keeps the error kind and names the path, which a bare `io::Error` drops.
pub(super) fn io_error_at(path: &Path, err: &io::Error) -> IngestError {
    IngestError::Io(sanitized_io_error(
        err.kind(),
        &format!("{}: {err}", path.display()),
    ))
}

/// Error for a transport failure. `error_text` is the transport error's `Display`.
pub(super) fn transport_error(error_text: &str) -> IngestError {
    IngestError::Http(ErrorDetail::new(error_text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_wraps_unchanged() {
        let wrapped = IngestError::from(DomainError::usage("test"));
        assert!(
            matches!(&wrapped, IngestError::Domain(DomainError::Usage(message)) if message == "test"),
            "{wrapped:?}"
        );
    }

    #[test]
    fn display_matches_each_variant() {
        let cases: [(IngestError, String); 4] = [
            (
                IngestError::Api(crate::core::github::api_error(
                    500,
                    b"boom",
                    None,
                    crate::core::github::Auth::Anonymous,
                )),
                "GitHub API error: HTTP 500: boom".into(),
            ),
            (DomainError::usage("bad flag").into(), "bad flag".into()),
            (
                IngestError::Io(io::Error::other("disk")),
                "filesystem error: disk".into(),
            ),
            (
                IngestError::Http(ErrorDetail::new("timeout")),
                "GitHub request failed: timeout".into(),
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(error.to_string(), expected);
        }
    }

    #[test]
    fn transport_error_sanitizes_and_wraps_as_http() {
        assert_eq!(
            transport_error("bad\nhost").to_string(),
            IngestError::Http(ErrorDetail::new("bad host")).to_string()
        );
    }

    #[test]
    fn io_error_at_sanitizes_path_with_control_characters() {
        let path = Path::new("test\x1b[31m\nfile\x07.rs");
        let io_err = io::Error::new(io::ErrorKind::NotFound, "not found");
        let err = io_error_at(path, &io_err);
        let msg = err.to_string();
        assert!(!msg.contains('\x1b'));
        assert!(!msg.contains('\n'));
        assert!(!msg.contains('\x07'));
        assert!(msg.contains("test\u{FFFD}[31m file\u{FFFD}.rs: not found"));
    }
}
