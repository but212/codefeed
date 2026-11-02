//! User exclusion patterns (`--exclude`) in `.gitignore` syntax, matched against source-relative paths.

use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::core::error::DomainError;

/// Validated user exclusion patterns, matched with `.gitignore` semantics.
///
/// Patterns are relative to the source root and case-sensitive. They are independent of the source's
/// own `.gitignore` files: a path is excluded when either source drops it.
#[derive(Clone, Debug)]
pub struct ExcludePatterns {
    patterns: Vec<String>,
    rules: Gitignore,
}

impl Default for ExcludePatterns {
    fn default() -> Self {
        Self {
            patterns: Vec::new(),
            rules: Gitignore::empty(),
        }
    }
}

impl PartialEq for ExcludePatterns {
    fn eq(&self, other: &Self) -> bool {
        self.patterns == other.patterns
    }
}

impl Eq for ExcludePatterns {}

impl ExcludePatterns {
    /// Compiles `patterns` in the given order. Each pattern must yield exactly one rule.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::Usage`] naming the first pattern that is not a valid glob or that
    /// yields no rule, such as an empty or whitespace-only pattern, a comment, or a bare `!` (with or
    /// without trailing whitespace).
    pub fn new<I, S>(patterns: I) -> Result<Self, DomainError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let patterns: Vec<String> = patterns.into_iter().map(Into::into).collect();
        let mut builder = user_builder();
        for pattern in &patterns {
            check_one_rule(pattern)?;
            // Every line was validated above, so this adds exactly the rule that was checked.
            let _ = builder.add_line(None, pattern);
        }
        let rules = builder
            .build()
            .map_err(|err| DomainError::usage(format!("invalid exclude patterns: {err}")))?;
        Ok(Self { patterns, rules })
    }

    /// Whether the patterns exclude `prefix`, a match prefix built by [`push_match_segment`].
    ///
    /// Each prefix is matched on its own. Callers test a directory as it is admitted, so an excluded
    /// directory excludes everything below it, and a `!` pattern cannot re-include such an entry.
    #[must_use]
    pub(crate) fn excludes_prefix(&self, prefix: &str, is_dir: bool) -> bool {
        !self.rules.is_empty() && self.rules.matched(prefix, is_dir).is_ignore()
    }
}

/// Appends one path segment to a match prefix, joined with `/` when the prefix is not empty.
///
/// A `\` inside a segment is matched as U+FFFD on every platform, so it is never a separator and never
/// matches a `\` in a pattern. Invalid UTF-8 is matched by its lossy form.
pub(super) fn push_match_segment(prefix: &mut String, segment: &[u8]) {
    if !prefix.is_empty() {
        prefix.push('/');
    }
    prefix.extend(
        String::from_utf8_lossy(segment)
            .chars()
            .map(|c| if c == '\\' { '\u{FFFD}' } else { c }),
    );
}

/// Compiles one pattern on its own and requires exactly one rule from it. A pattern whose body is
/// empty once trailing whitespace, `!`, and the surrounding `/` are removed (such as `!` or `! `)
/// would compile to a glob that matches everything or nothing, so it is rejected before compilation.
/// The body is taken after the trailing whitespace that `add_line` trims, so the check and the
/// compiled rule agree. An escaped `\ ` keeps its backslash, so it never empties the body.
fn check_one_rule(pattern: &str) -> Result<(), DomainError> {
    let trimmed = pattern.trim_end();
    let body = trimmed
        .strip_prefix('!')
        .unwrap_or(trimmed)
        .trim_matches('/');
    if body.is_empty() {
        return Err(no_rule(pattern));
    }
    let single = user_builder()
        .add_line(None, pattern)
        .and_then(|line| line.build())
        .map_err(|err| invalid_pattern(pattern, &err.to_string()))?;
    if single.len() == 1 {
        Ok(())
    } else {
        Err(no_rule(pattern))
    }
}

/// Builder for user patterns, rooted at the source root. Unlike a `.gitignore` file, an unclosed `[`
/// is an error here, since on the command line it is almost always a typo.
fn user_builder() -> GitignoreBuilder {
    let mut builder = GitignoreBuilder::new("");
    builder.allow_unclosed_class(false);
    builder
}

fn no_rule(pattern: &str) -> DomainError {
    invalid_pattern(
        pattern,
        "it matches nothing: blank lines, comments, and a bare '!' are not allowed",
    )
}

fn invalid_pattern(pattern: &str, reason: &str) -> DomainError {
    DomainError::usage(format!(
        "invalid value '{pattern}' for '--exclude <PATTERN>': {reason}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::error::sanitize_error_detail;
    use crate::core::policy::{
        AdmittedDir, MAX_PATH_BYTES_USIZE, MAX_PATH_DEPTH, PathStatus, admit_dir,
        classify_path_segments,
    };

    fn patterns(list: &[&str]) -> ExcludePatterns {
        ExcludePatterns::new(list.iter().copied()).expect("patterns are valid")
    }

    /// Splits a `/`-separated path into the segments that `classify_path_segments` receives.
    fn segs(path: &str) -> Vec<&[u8]> {
        path.split('/').map(str::as_bytes).collect()
    }

    /// Whether `patterns` reject the path as the walk and the classifier apply it: a directory is
    /// rejected when admitting it fails, and a file when it classifies as `Ignored`.
    fn rejected(patterns: &ExcludePatterns, segments: &[&[u8]], is_dir: bool) -> bool {
        if !is_dir {
            return classify_path_segments(segments.iter().copied(), patterns)
                == Ok(PathStatus::Ignored);
        }
        let mut dir = AdmittedDir::root();
        for name in segments {
            match admit_dir(dir, name, patterns) {
                Some(admitted) => dir = admitted,
                None => return true,
            }
        }
        false
    }

    impl ExcludePatterns {
        /// Whether the user patterns exclude the path. It must be rejected with these patterns but not
        /// with none, so hidden and noise names are never reported as user-excluded.
        fn excludes(&self, segments: &[&[u8]], is_dir: bool) -> bool {
            rejected(self, segments, is_dir)
                && !rejected(&Self::default(), segments, is_dir)
        }
    }

    #[test]
    fn new_rejects_patterns_that_are_invalid_or_yield_no_rule() {
        for pattern in ["", "   ", "#note", "!", "! ", "!\t", "/ ", "!/ ", "["] {
            let Err(err) = ExcludePatterns::new([pattern]) else {
                panic!("expected rejection for {pattern:?}");
            };
            let expected = format!(
                "invalid value '{}' for '--exclude <PATTERN>': ",
                sanitize_error_detail(pattern)
            );
            assert!(
                matches!(&err, DomainError::Usage(msg) if msg.starts_with(&expected)),
                "{pattern:?}: {err}"
            );
        }
    }

    #[test]
    fn rejected_pattern_echo_is_sanitized() {
        let Err(err) = ExcludePatterns::new(["[\x1b[2J"]) else {
            panic!("expected rejection for a malformed glob");
        };
        let rendered = err.to_string();
        assert!(!rendered.contains('\x1b'), "ESC leaked: {rendered:?}");
        assert!(
            rendered.starts_with("invalid value '[\u{FFFD}[2J' for '--exclude <PATTERN>': "),
            "{rendered:?}"
        );
    }

    #[test]
    fn unescaped_trailing_whitespace_is_trimmed_as_in_gitignore() {
        // `.gitignore` trims trailing whitespace before parsing, so `*.log ` is `*.log`. A bare `!`
        // with trailing whitespace must therefore be rejected, not compiled to a match-all rule.
        let ex = patterns(&["*.log "]);
        assert!(ex.excludes(&segs("a.log"), false));
    }

    #[test]
    fn escaped_trailing_space_is_a_literal_space() {
        // `\ ` keeps its space, so the pattern names a file ending in a space.
        let ex = patterns(&["a\\ "]);
        assert!(ex.excludes(&segs("a "), false));
        assert!(!ex.excludes(&segs("a"), false));
    }

    #[test]
    fn new_accepts_gitignore_syntax() {
        for pattern in ["*.log", "/top", "build/", "!keep.log", "-foo", "\\#literal"] {
            assert!(ExcludePatterns::new([pattern]).is_ok(), "{pattern}");
        }
    }

    #[test]
    fn empty_set_excludes_nothing() {
        let none = ExcludePatterns::default();
        assert!(!none.excludes(&segs("a/b.txt"), false));
        assert!(!none.excludes(&segs("build"), true));
        assert_eq!(none, ExcludePatterns::new(Vec::<String>::new()).unwrap());
    }

    #[test]
    fn unanchored_pattern_matches_at_any_depth() {
        let ex = patterns(&["*.log"]);
        assert!(ex.excludes(&segs("a.log"), false));
        assert!(ex.excludes(&segs("sub/deeper/a.log"), false));
        assert!(!ex.excludes(&segs("sub/a.txt"), false));

        // A matching directory excludes the files below it.
        let ex = patterns(&["target"]);
        assert!(ex.excludes(&segs("target/debug/x.rs"), false));
        assert!(ex.excludes(&segs("sub/target"), true));
    }

    #[test]
    fn anchored_pattern_matches_from_source_root_only() {
        let ex = patterns(&["/top"]);
        assert!(ex.excludes(&segs("top"), false));
        assert!(!ex.excludes(&segs("sub/top"), false));
    }

    #[test]
    fn directory_only_pattern_needs_a_directory() {
        let ex = patterns(&["build/"]);
        assert!(ex.excludes(&segs("build"), true));
        assert!(!ex.excludes(&segs("build"), false));
        assert!(ex.excludes(&segs("build/out.txt"), false));
        assert!(ex.excludes(&segs("sub/build/out.txt"), false));
    }

    #[test]
    fn user_negation_reincludes_only_within_user_rules() {
        let ex = patterns(&["*.log", "!keep.log"]);
        assert!(ex.excludes(&segs("a.log"), false));
        assert!(!ex.excludes(&segs("keep.log"), false));
        assert!(!ex.excludes(&segs("sub/keep.log"), false));
    }

    #[test]
    fn excluded_directory_cannot_be_reincluded() {
        // Git semantics: a file under an excluded directory stays excluded whatever `!` says.
        let ex = patterns(&["build/", "!build/keep.txt"]);
        assert!(ex.excludes(&segs("build"), true));
        assert!(ex.excludes(&segs("build/keep.txt"), false));
    }

    #[test]
    fn matching_is_case_sensitive() {
        let ex = patterns(&["*.LOG"]);
        assert!(ex.excludes(&segs("a.LOG"), false));
        assert!(!ex.excludes(&segs("a.log"), false));
    }

    #[test]
    fn non_utf8_segments_match_by_lossy_form() {
        let ex = patterns(&["*.txt"]);
        let segments: [&[u8]; 2] = [b"dir", b"\xff.txt"];
        assert!(ex.excludes(&segments, false));
    }

    #[test]
    fn match_window_is_the_first_depth_limit_segments() {
        let ex = patterns(&["file.txt"]);

        let shallow: Vec<&[u8]> = std::iter::repeat_n(b"d".as_slice(), MAX_PATH_DEPTH)
            .chain(std::iter::once(b"file.txt".as_slice()))
            .collect();
        assert!(ex.excludes(&shallow, false));

        // The match sits past the first `MAX_PATH_DEPTH + 1` segments, so it is not tested.
        let deep: Vec<&[u8]> = std::iter::repeat_n(b"d".as_slice(), MAX_PATH_DEPTH + 1)
            .chain(std::iter::once(b"file.txt".as_slice()))
            .collect();
        assert!(!ex.excludes(&deep, false));

        // Byte length does not end the window: a match inside it is found however long the prefix is.
        let head = "a".repeat(MAX_PATH_BYTES_USIZE + 1);
        let long: [&[u8]; 2] = [head.as_bytes(), b"file.txt"];
        assert!(ex.excludes(&long, false));
    }

    #[test]
    fn backslash_in_segment_matches_the_same_on_every_platform() {
        // A `\` in a segment is not a separator on any platform. The segment is matched with its
        // `\` replaced by U+FFFD, so no pattern matches it by name or by a `\` escape.
        let segments: [&[u8]; 1] = [b"docs\\secret.md"];
        assert!(!patterns(&["secret.md"]).excludes(&segments, false));
        assert!(!patterns(&["/docs/secret.md"]).excludes(&segments, false));
        assert!(!patterns(&["docs\\\\secret.md"]).excludes(&segments, false));
        assert!(patterns(&["*.md"]).excludes(&segments, false));
    }

    #[test]
    fn excludes_reports_only_user_rules() {
        let none = ExcludePatterns::default();
        assert!(!none.excludes(&segs(".git"), true));
        assert!(!none.excludes(&segs("Cargo.lock"), false));
        assert!(!patterns(&["*.log"]).excludes(&segs(".git"), true));
    }

    #[test]
    fn equality_compares_patterns() {
        assert_eq!(patterns(&["a", "b"]), patterns(&["a", "b"]));
        assert_ne!(patterns(&["a"]), patterns(&["a", "b"]));
    }
}
