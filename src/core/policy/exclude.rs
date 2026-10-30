//! User exclusion patterns (`--exclude`) in `.gitignore` syntax, matched against source-relative paths.

use ignore::gitignore::{Gitignore, GitignoreBuilder};

use super::limits::MAX_PATH_DEPTH;
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
    /// yields no rule, such as an empty or whitespace-only pattern, a comment, or a bare `!`.
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

    /// Whether the patterns exclude the path given by `segments`, relative to the source root.
    ///
    /// Segments must be non-empty and must not be `.`. Ancestors are tested as directories, from the
    /// top down, so an excluded directory excludes everything below it, and a `!` pattern cannot
    /// re-include such an entry. Only the first `MAX_PATH_DEPTH + 1` segments are tested, so the
    /// cost is at most that many matches, each linear in its prefix. A match beyond that window is
    /// not seen, and the entry then fails its depth limit as any other deep entry does.
    #[must_use]
    pub(crate) fn excludes(&self, segments: &[&[u8]], is_dir: bool) -> bool {
        if self.rules.is_empty() {
            return false;
        }
        let last = segments.len().saturating_sub(1);
        let mut prefix = String::new();
        for (index, segment) in segments.iter().take(MAX_PATH_DEPTH + 1).enumerate() {
            if index > 0 {
                prefix.push('/');
            }
            prefix.push_str(&String::from_utf8_lossy(segment));
            let as_dir = index < last || is_dir;
            if self.rules.matched(&prefix, as_dir).is_ignore() {
                return true;
            }
        }
        false
    }
}

/// Compiles one pattern on its own and requires exactly one rule from it. A pattern whose body is
/// empty once `!` and the surrounding `/` are removed (such as `!`) would compile to a glob that
/// matches everything or nothing, so it is rejected before compilation.
fn check_one_rule(pattern: &str) -> Result<(), DomainError> {
    let body = pattern
        .strip_prefix('!')
        .unwrap_or(pattern)
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
    use crate::core::policy::MAX_PATH_BYTES_USIZE;

    fn patterns(list: &[&str]) -> ExcludePatterns {
        ExcludePatterns::new(list.iter().copied()).expect("patterns are valid")
    }

    /// Splits a `/`-separated path into the segments that `classify_path_segments` receives.
    fn segs(path: &str) -> Vec<&[u8]> {
        path.split('/').map(str::as_bytes).collect()
    }

    #[test]
    fn new_rejects_patterns_that_are_invalid_or_yield_no_rule() {
        for pattern in ["", "   ", "#note", "!", "["] {
            let Err(err) = ExcludePatterns::new([pattern]) else {
                panic!("expected rejection for {pattern:?}");
            };
            let expected = format!("invalid value '{pattern}' for '--exclude <PATTERN>': ");
            assert!(
                matches!(&err, DomainError::Usage(msg) if msg.starts_with(&expected)),
                "{pattern:?}: {err}"
            );
        }
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
    fn equality_compares_patterns() {
        assert_eq!(patterns(&["a", "b"]), patterns(&["a", "b"]));
        assert_ne!(patterns(&["a"]), patterns(&["a", "b"]));
    }
}
