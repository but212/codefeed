use std::fmt::{Display, Formatter, Result};

/// Error text that may echo user input or a remote payload. It holds no control character (I-04):
/// the only constructor sanitizes, so every error that stores one renders safely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorDetail(String);

impl ErrorDetail {
    /// Creates a detail from `text`, replacing line breaks with spaces and other control characters
    /// with `U+FFFD`.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self(sanitize_error_detail(&text.into()))
    }
}

impl std::ops::Deref for ErrorDetail {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl PartialEq<str> for ErrorDetail {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl Display for ErrorDetail {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        f.write_str(&self.0)
    }
}

/// Diagnostic details for exceeded candidate limits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateLimitExceededError {
    /// The candidate limit that was exceeded.
    pub limit: usize,
    /// Top directories contributing to the candidate overflow.
    pub largest_directories: Vec<(ErrorDetail, usize)>,
}

/// Diagnostic details for an exceeded output size limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputTooLargeError {
    /// The output size limit in bytes that was exceeded.
    pub limit: u64,
    /// Largest files contributing to the overflow, as `(path, size in bytes)`.
    pub largest_files: Vec<(ErrorDetail, usize)>,
}

/// Pure domain rule failure. Core returns this type only; effect failures belong to the shell.
#[derive(Debug)]
#[non_exhaustive]
pub enum DomainError {
    /// CLI usage error or missing argument.
    Usage(ErrorDetail),
    /// Candidate file budget exceeded.
    CandidateLimitExceeded(CandidateLimitExceededError),
    /// Source string or CLI argument is syntactically invalid.
    InvalidSource(ErrorDetail),
    /// Source scheme or host is unsupported (e.g. non-GitHub remote or SSH).
    UnsupportedSource(ErrorDetail),
    /// Repository metadata or API response payload is malformed.
    InvalidMetadata(ErrorDetail),
    /// Rendered digest exceeds the maximum allowed output size.
    OutputTooLarge(OutputTooLargeError),
}

impl DomainError {
    /// Constructs a [`Usage`](Self::Usage) error. The message is sanitized because it may echo user input.
    #[cold]
    pub(crate) fn usage(message: impl Into<String>) -> Self {
        Self::Usage(ErrorDetail::new(message))
    }

    /// Constructs a [`CandidateLimitExceeded`](Self::CandidateLimitExceeded) error with diagnostic directory counts.
    /// The directory names are sanitized.
    #[cold]
    pub(crate) fn candidate_limit_exceeded(
        limit: usize,
        largest_directories: Vec<(String, usize)>,
    ) -> Self {
        Self::CandidateLimitExceeded(CandidateLimitExceededError {
            limit,
            largest_directories: sanitized_counts(largest_directories),
        })
    }

    /// Constructs an [`InvalidSource`](Self::InvalidSource) error. The message is sanitized because it may echo user input.
    #[cold]
    pub(crate) fn invalid_source(message: impl Into<String>) -> Self {
        Self::InvalidSource(ErrorDetail::new(message))
    }

    /// Constructs an [`UnsupportedSource`](Self::UnsupportedSource) error. The message is sanitized because it may echo user input.
    #[cold]
    pub(crate) fn unsupported_source(message: impl Into<String>) -> Self {
        Self::UnsupportedSource(ErrorDetail::new(message))
    }

    /// Constructs an [`InvalidMetadata`](Self::InvalidMetadata) error. The message is sanitized because it may echo remote payload.
    #[cold]
    pub(crate) fn invalid_metadata(message: impl Into<String>) -> Self {
        Self::InvalidMetadata(ErrorDetail::new(message))
    }

    /// Constructs an [`OutputTooLarge`](Self::OutputTooLarge) error with diagnostic largest file entries.
    /// The file names are sanitized.
    #[cold]
    pub(crate) fn output_too_large(limit: u64, largest_files: Vec<(String, usize)>) -> Self {
        Self::OutputTooLarge(OutputTooLargeError {
            limit,
            largest_files: sanitized_counts(largest_files),
        })
    }
}

fn sanitized_counts(entries: Vec<(String, usize)>) -> Vec<(ErrorDetail, usize)> {
    entries
        .into_iter()
        .map(|(name, count)| (ErrorDetail::new(name), count))
        .collect()
}

/// Sanitizes control characters in error detail string.
/// Converts `\t`, `\r`, `\n` to space `' '` for single-line output formatting.
/// Converts all other Unicode control characters (`c.is_control()`) to `\u{FFFD}`.
/// Note: Unlike digest header escaping which escapes `\n` and `\r` as `\n` / `\r` literals,
/// error message formatting converts line breaks to spaces for compact single-line stderr diagnostics.
#[must_use]
fn sanitize_error_detail(input: &str) -> String {
    input
        .chars()
        .map(|c| match c {
            '\t' | '\r' | '\n' => ' ',
            c if c.is_control() => '\u{FFFD}',
            c => c,
        })
        .collect()
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
            Self::InvalidMetadata(message) => {
                write!(f, "invalid repository metadata: {message}")
            }
            Self::OutputTooLarge(err) => {
                let limit_mib = err.limit / (1024 * 1024);
                write!(f, "digest exceeds the {limit_mib} MiB output limit")?;
                if !err.largest_files.is_empty() {
                    write!(f, "\nlargest files:")?;
                    for (path, size) in &err.largest_files {
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

#[expect(
    clippy::cast_precision_loss,
    reason = "diagnostic byte sizes in error messages are well below f64 53-bit mantissa limit"
)]
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
    use crate::core::policy::{MAX_FILE_COUNT, MAX_OUTPUT_SIZE};

    #[test]
    fn output_too_large_message_derives_from_payload_limit() {
        assert_eq!(
            DomainError::output_too_large(7 * 1024 * 1024, Vec::new()).to_string(),
            "digest exceeds the 7 MiB output limit"
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

    #[test]
    fn sanitize_error_detail_replaces_newlines_and_controls() {
        let input = "hello\tworld\r\nfoo\x1b[31mbar\x07baz\x7fdel\u{80}c1";
        let sanitized = sanitize_error_detail(input);
        assert_eq!(
            sanitized,
            "hello world  foo\u{FFFD}[31mbar\u{FFFD}baz\u{FFFD}del\u{FFFD}c1"
        );
    }

    #[test]
    fn error_detail_is_sanitized_at_construction() {
        let raw = "a\x1b[2J\nb";
        let detail = ErrorDetail::new(raw);
        // The stored text, the `Deref` view, and the rendering are all the sanitized text.
        assert_eq!(&*detail, "a\u{FFFD}[2J b");
        assert_eq!(detail, *"a\u{FFFD}[2J b");
        assert_eq!(detail.to_string(), "a\u{FFFD}[2J b");
    }

    #[test]
    fn every_message_variant_renders_sanitized_text() {
        let raw = "a\x1b[2J\nb";
        let cases = [
            DomainError::usage(raw),
            DomainError::invalid_source(raw),
            DomainError::unsupported_source(raw),
            DomainError::invalid_metadata(raw),
        ];
        for err in cases {
            let rendered = err.to_string();
            assert!(!rendered.contains('\x1b'), "ESC leaked: {rendered:?}");
            assert!(!rendered.contains('\n'), "newline leaked: {rendered:?}");
        }
        assert_eq!(DomainError::usage(raw).to_string(), "a\u{FFFD}[2J b");
    }

    /// A formatting sink that rejects any write which would exceed `remaining` bytes.
    struct FailingSink {
        remaining: usize,
    }

    impl std::fmt::Write for FailingSink {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            self.remaining = self.remaining.checked_sub(s.len()).ok_or(std::fmt::Error)?;
            Ok(())
        }
    }

    #[test]
    fn display_propagates_sink_failure_at_every_write() {
        use std::fmt::Write as _;

        let errors = [
            DomainError::candidate_limit_exceeded(
                MAX_FILE_COUNT,
                vec![("dir_a".into(), 3), ("dir_b".into(), 2)],
            ),
            DomainError::output_too_large(MAX_OUTPUT_SIZE, vec![("big.txt".into(), 2_000_000)]),
        ];
        for err in errors {
            let full = err.to_string();
            // A sink with exactly `full.len()` bytes accepts every write.
            let mut exact = FailingSink {
                remaining: full.len(),
            };
            write!(exact, "{err}").expect("sink with exact capacity accepts all writes");

            // Any shorter sink must fail, whichever `write!` in `Display` crosses the boundary.
            for capacity in 0..full.len() {
                let mut sink = FailingSink {
                    remaining: capacity,
                };
                assert!(
                    write!(sink, "{err}").is_err(),
                    "capacity {capacity}: {full:?}"
                );
            }
        }
    }

    #[test]
    fn overflow_diagnostics_sanitize_control_characters() {
        let candidates = DomainError::candidate_limit_exceeded(
            MAX_FILE_COUNT,
            vec![("d\x1b]0;t\x07/".into(), 3)],
        )
        .to_string();
        assert_eq!(
            candidates,
            format!(
                "repository contains more than {MAX_FILE_COUNT} files\nlargest directories:\n  - d\u{FFFD}]0;t\u{FFFD}/ (3 files)"
            )
        );

        let mib = MAX_OUTPUT_SIZE / (1024 * 1024);
        let output =
            DomainError::output_too_large(MAX_OUTPUT_SIZE, vec![("f\x1b[31m\n.txt".into(), 1)])
                .to_string();
        assert_eq!(
            output,
            format!(
                "digest exceeds the {mib} MiB output limit\nlargest files:\n  - f\u{FFFD}[31m .txt (1 B)"
            )
        );
    }

    mod properties {
        use super::*;
        use proptest::prelude::*;

        /// Arbitrary characters mixed with the C0 and C1 control ranges.
        fn text() -> impl Strategy<Value = String> {
            prop::collection::vec(
                prop_oneof![
                    3 => any::<char>(),
                    2 => prop::char::range('\0', '\u{9f}'),
                ],
                0..64,
            )
            .prop_map(String::from_iter)
        }

        proptest! {
            #![proptest_config(crate::core::proptest_config())]

            /// The output is one line without control characters, the same length in characters, and
            /// keeps every non-control character in place.
            ///
            /// `ErrorDetail::new` stores exactly that text, and rebuilding one from its rendering
            /// changes nothing.
            #[test]
            fn sanitized_detail_has_no_control_characters(input in text()) {
                let detail = ErrorDetail::new(input.as_str());
                prop_assert_eq!(&*detail, sanitize_error_detail(&input));
                prop_assert_eq!(ErrorDetail::new(detail.to_string()), detail.clone());
                let output = sanitize_error_detail(&input);
                prop_assert!(!output.chars().any(char::is_control), "{:?}", output);
                prop_assert_eq!(output.chars().count(), input.chars().count());
                for (before, after) in input.chars().zip(output.chars()) {
                    if !before.is_control() {
                        prop_assert_eq!(before, after);
                    }
                }
            }

            #[test]
            fn sanitizing_is_idempotent(input in text()) {
                let once = sanitize_error_detail(&input);
                prop_assert_eq!(sanitize_error_detail(&once), once);
            }
        }
    }
}
