use std::borrow::Cow;
use std::cmp::Ordering;
use std::fmt::{Display, Formatter, Result};

/// Error text that may echo user input or a remote payload. It holds no control character (I-04):
/// the only constructor sanitizes, so every error that stores one renders safely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorDetail(String);

impl ErrorDetail {
    /// Creates a detail from `text`, replacing line breaks with spaces and other control characters
    /// with `U+FFFD` (unlike digest header escaping, keeping stderr diagnostics single-line).
    #[must_use]
    pub fn new<'a>(text: impl Into<Cow<'a, str>>) -> Self {
        let cow = text.into();
        if !cow.chars().any(char::is_control) {
            return Self(cow.into_owned());
        }
        Self(
            cow.chars()
                .map(|c| match c {
                    '\t' | '\r' | '\n' => ' ',
                    c if c.is_control() => '\u{FFFD}',
                    c => c,
                })
                .collect(),
        )
    }

    /// Returns the sanitized error string, consuming the wrapper.
    #[must_use]
    pub(crate) fn into_inner(self) -> String {
        self.0
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

/// Diagnostic details for an exceeded candidate count or output size limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitExceededError {
    /// The limit that was exceeded (file count or output bytes).
    pub limit: u64,
    /// Top entries contributing to the overflow, as `(name/path, count/size)`.
    pub largest: Vec<(ErrorDetail, usize)>,
}

/// Pure domain rule failure. Core returns this type only; effect failures belong to the shell.
#[derive(Debug)]
#[non_exhaustive]
pub enum DomainError {
    /// CLI usage error or missing argument.
    Usage(ErrorDetail),
    /// Candidate file budget exceeded.
    CandidateLimitExceeded(LimitExceededError),
    /// Source string or CLI argument is syntactically invalid.
    InvalidSource(ErrorDetail),
    /// Source scheme or host is unsupported (e.g. non-GitHub remote or SSH).
    UnsupportedSource(ErrorDetail),
    /// Repository metadata or API response payload is malformed.
    InvalidMetadata(ErrorDetail),
    /// Rendered digest exceeds the maximum allowed output size.
    OutputTooLarge(LimitExceededError),
}

impl DomainError {
    /// Constructs a [`Usage`](Self::Usage) error. The message is sanitized because it may echo user input.
    #[cold]
    pub(crate) fn usage<'a>(message: impl Into<Cow<'a, str>>) -> Self {
        Self::Usage(ErrorDetail::new(message))
    }

    /// Constructs a [`CandidateLimitExceeded`](Self::CandidateLimitExceeded) error with diagnostic directory counts.
    /// The directory names are sanitized.
    #[cold]
    pub(crate) fn candidate_limit_exceeded(
        limit: usize,
        largest_directories: &[(&str, usize)],
    ) -> Self {
        Self::CandidateLimitExceeded(LimitExceededError {
            limit: limit as u64,
            largest: sanitized_counts(largest_directories),
        })
    }

    /// Constructs an [`InvalidSource`](Self::InvalidSource) error. The message is sanitized because it may echo user input.
    #[cold]
    pub(crate) fn invalid_source<'a>(message: impl Into<Cow<'a, str>>) -> Self {
        Self::InvalidSource(ErrorDetail::new(message))
    }

    /// Constructs an [`UnsupportedSource`](Self::UnsupportedSource) error. The message is sanitized because it may echo user input.
    #[cold]
    pub(crate) fn unsupported_source<'a>(message: impl Into<Cow<'a, str>>) -> Self {
        Self::UnsupportedSource(ErrorDetail::new(message))
    }

    /// Constructs an [`InvalidMetadata`](Self::InvalidMetadata) error. The message is sanitized because it may echo remote payload.
    #[cold]
    pub(crate) fn invalid_metadata<'a>(message: impl Into<Cow<'a, str>>) -> Self {
        Self::InvalidMetadata(ErrorDetail::new(message))
    }

    /// Constructs an [`OutputTooLarge`](Self::OutputTooLarge) error with diagnostic largest file entries.
    /// The file names are sanitized.
    #[cold]
    pub(crate) fn output_too_large(limit: u64, largest_files: &[(&str, usize)]) -> Self {
        Self::OutputTooLarge(LimitExceededError {
            limit,
            largest: sanitized_counts(largest_files),
        })
    }
}

fn sanitized_counts(entries: &[(&str, usize)]) -> Vec<(ErrorDetail, usize)> {
    entries
        .iter()
        .map(|&(name, count)| (ErrorDetail::new(name), count))
        .collect()
}

/// Selects up to `limit` entries with the largest counts, tie-breaking equal counts with `tie_break`.
#[must_use]
pub(crate) fn top_largest<'a>(
    entries: impl IntoIterator<Item = (&'a str, usize)>,
    limit: usize,
    tie_break: impl Fn(&str, &str) -> Ordering,
) -> Vec<(&'a str, usize)> {
    if limit == 0 {
        return Vec::new();
    }
    let cmp =
        |a: (&'a str, usize), b: (&'a str, usize)| b.1.cmp(&a.1).then_with(|| tie_break(a.0, b.0));
    let mut top: Vec<(&'a str, usize)> = Vec::with_capacity(limit);
    for item in entries {
        if top.len() == limit && cmp(top[limit - 1], item).is_le() {
            continue;
        }
        let pos = top.partition_point(|&existing| cmp(existing, item).is_le());
        if top.len() == limit {
            top.pop();
        }
        top.insert(pos, item);
    }
    top
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
                for (dir, count) in &err.largest {
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
                if !err.largest.is_empty() {
                    write!(f, "\nlargest files:")?;
                    for (path, size) in &err.largest {
                        write!(f, "\n  - {path} ({})", FormattedSize(*size))?;
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

struct FormattedSize(usize);

impl Display for FormattedSize {
    #[expect(
        clippy::cast_precision_loss,
        reason = "diagnostic byte sizes in error messages are well below f64 53-bit mantissa limit"
    )]
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        match self.0 {
            b @ MIB_ROUNDING_THRESHOLD.. => write!(f, "{:.1} MiB", b as f64 / MIB),
            b @ 1024.. => write!(f, "{:.1} KiB", b as f64 / KIB),
            b => write!(f, "{b} B"),
        }
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
            DomainError::output_too_large(7 * 1024 * 1024, &[]).to_string(),
            "digest exceeds the 7 MiB output limit"
        );
    }

    #[test]
    fn top_largest_selects_and_truncates_across_boundary_sizes() {
        type Entry<'a> = (&'a str, usize);
        type TopCase<'a> = (&'a [Entry<'a>], usize, &'a [Entry<'a>]);
        let less = [("a", 1), ("b", 3)];
        let exact = [("a", 1), ("b", 3), ("c", 2)];
        let more = [("a", 1), ("b", 5), ("c", 2), ("d", 5), ("e", 3)];
        let tie_evict = [("m", 5), ("z", 5), ("a", 5), ("c", 5), ("b", 5)];
        let cases: [TopCase<'_>; 7] = [
            (&[], 0, &[]),
            (&more, 0, &[]),
            (&[], 3, &[]),
            (&less, 3, &[("b", 3), ("a", 1)]),
            (&exact, 3, &[("b", 3), ("c", 2), ("a", 1)]),
            (&more, 3, &[("b", 5), ("d", 5), ("e", 3)]),
            (&tie_evict, 2, &[("a", 5), ("b", 5)]),
        ];
        for (entries, limit, expected) in cases {
            let got = top_largest(entries.iter().copied(), limit, str::cmp);
            assert_eq!(got, expected, "limit={limit}");
        }
    }

    #[test]
    fn top_largest_uses_caller_tie_break() {
        let entries = [("src/", 4), ("src-test/", 4)];
        let by_component = |a: &str, b: &str| a.split('/').cmp(b.split('/'));

        // Byte order puts '-' (0x2D) before '/' (0x2F); component order has "src" < "src-test".
        let by_byte = top_largest(entries, 2, str::cmp);
        assert_eq!(by_byte, [("src-test/", 4), ("src/", 4)]);
        let by_comp = top_largest(entries, 2, by_component);
        assert_eq!(by_comp, [("src/", 4), ("src-test/", 4)]);
        assert_eq!(top_largest(entries, 1, str::cmp), [("src-test/", 4)]);
        assert_eq!(top_largest(entries, 1, by_component), [("src/", 4)]);
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
            assert_eq!(
                FormattedSize(bytes).to_string(),
                expected,
                "failed for {bytes} bytes"
            );
        }
    }

    #[test]
    fn error_detail_is_sanitized_at_construction() {
        let clean = ErrorDetail::new("plain error message");
        assert_eq!(&*clean, "plain error message");

        let input = "hello\tworld\r\nfoo\x1b[31mbar\x07baz\x7fdel\u{80}c1";
        let sanitized = ErrorDetail::new(input);
        assert_eq!(
            &*sanitized,
            "hello world  foo\u{FFFD}[31mbar\u{FFFD}baz\u{FFFD}del\u{FFFD}c1"
        );

        let raw = "a\x1b[2J\nb";
        let detail = ErrorDetail::new(raw);
        // The stored text, the `Deref` view, and the rendering are all the sanitized text.
        assert_eq!(&*detail, "a\u{FFFD}[2J b");
        assert_eq!(detail, *"a\u{FFFD}[2J b");
        assert_eq!(detail.to_string(), "a\u{FFFD}[2J b");
        assert_eq!(detail.into_inner(), "a\u{FFFD}[2J b");
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
            DomainError::candidate_limit_exceeded(MAX_FILE_COUNT, &[("dir_a", 3), ("dir_b", 2)]),
            DomainError::output_too_large(MAX_OUTPUT_SIZE, &[("big.txt", 2_000_000)]),
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
        let candidates =
            DomainError::candidate_limit_exceeded(MAX_FILE_COUNT, &[("d\x1b]0;t\x07/", 3)])
                .to_string();
        assert_eq!(
            candidates,
            format!(
                "repository contains more than {MAX_FILE_COUNT} files\nlargest directories:\n  - d\u{FFFD}]0;t\u{FFFD}/ (3 files)"
            )
        );

        let mib = MAX_OUTPUT_SIZE / (1024 * 1024);
        let output =
            DomainError::output_too_large(MAX_OUTPUT_SIZE, &[("f\x1b[31m\n.txt", 1)]).to_string();
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
                prop_assert_eq!(ErrorDetail::new(detail.to_string()), detail.clone());
                prop_assert!(!detail.chars().any(char::is_control), "{:?}", detail.clone());
                prop_assert_eq!(detail.chars().count(), input.chars().count());
                for (before, after) in input.chars().zip(detail.chars()) {
                    if !before.is_control() {
                        prop_assert_eq!(before, after);
                    }
                }
            }

            #[test]
            fn sanitizing_is_idempotent(input in text()) {
                let once = ErrorDetail::new(input.as_str());
                prop_assert_eq!(ErrorDetail::new(&*once), once);
            }
        }
    }
}
