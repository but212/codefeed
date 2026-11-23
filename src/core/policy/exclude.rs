//! User exclusion patterns (`--exclude`) and `.gitignore` rule compilation.

use std::path::Path;

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
        Self::empty()
    }
}

impl PartialEq for ExcludePatterns {
    fn eq(&self, other: &Self) -> bool {
        self.patterns == other.patterns
    }
}

impl Eq for ExcludePatterns {}

impl ExcludePatterns {
    /// Returns an empty exclusion pattern set that admits every path.
    ///
    /// # Examples
    ///
    /// ```
    /// use codefeed::ExcludePatterns;
    ///
    /// let empty = ExcludePatterns::empty();
    /// assert!(empty.is_empty());
    /// ```
    #[must_use]
    pub fn empty() -> Self {
        Self {
            patterns: Vec::new(),
            rules: Gitignore::empty(),
        }
    }

    /// Returns `true` when no exclusion patterns are configured.
    ///
    /// # Examples
    ///
    /// ```
    /// use codefeed::ExcludePatterns;
    ///
    /// assert!(ExcludePatterns::empty().is_empty());
    /// assert!(!ExcludePatterns::new(["*.log"])?.is_empty());
    /// # Ok::<(), codefeed::DomainError>(())
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Compiles `patterns` in order; each must yield exactly one rule.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::Usage`] for an invalid glob, empty/comment/bare-`!` pattern,
    /// unsupported POSIX character class (`[[:digit:]]`), or rule-set compilation failure.
    pub fn new<I, S>(patterns: I) -> Result<Self, DomainError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let patterns: Vec<String> = patterns.into_iter().map(Into::into).collect();
        for pattern in &patterns {
            check_one_rule(pattern, trim_rule_end(pattern))?;
        }
        Self::compile_validated(patterns)
    }

    /// Combines `self` and `other` in order (`self` rules followed by `other` rules).
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::Usage`] if the combined rules cannot be compiled, even if each
    /// input compiled separately.
    ///
    /// # Examples
    ///
    /// ```
    /// use codefeed::ExcludePatterns;
    ///
    /// let combined = ExcludePatterns::new(["*.log"])?
    ///     .try_combine(ExcludePatterns::new(["!keep.log"])?)?;
    /// assert!(!combined.is_empty());
    /// # Ok::<(), codefeed::DomainError>(())
    /// ```
    pub fn try_combine(mut self, other: Self) -> Result<Self, DomainError> {
        if self.is_empty() {
            return Ok(other);
        }
        if other.is_empty() {
            return Ok(self);
        }
        drop(self.rules);
        drop(other.rules);
        self.patterns.extend(other.patterns);
        Self::compile_validated(self.patterns)
    }

    fn compile_validated(patterns: Vec<String>) -> Result<Self, DomainError> {
        let mut builder = user_builder();
        for pattern in &patterns {
            builder
                .add_line(None, trim_rule_end(pattern))
                .map_err(|err| invalid_pattern(pattern, err))?;
        }
        let rules = builder
            .build()
            .map_err(|err| DomainError::usage(format!("invalid exclude patterns: {err}")))?;
        Ok(Self { patterns, rules })
    }

    /// Advances `prefix` by `segment` and checks user exclusion patterns.
    ///
    /// Returns `Some(prefix)` when empty, `None` when excluded, or `Some(extended)` when admitted.
    /// Excluded directories prune all descendants (`!` cannot re-include them).
    #[must_use]
    pub(crate) fn advance(&self, prefix: String, segment: &[u8]) -> Option<String> {
        if self.patterns.is_empty() {
            return Some(prefix);
        }
        let next = push_match_segment(prefix, segment);
        (!self.rules.matched(&next, true).is_ignore()).then_some(next)
    }

    /// Returns `true` if leaf file `segment` directly under `parent_prefix` is excluded by user patterns.
    #[must_use]
    pub(crate) fn excludes_leaf(&self, parent_prefix: &str, segment: &[u8]) -> bool {
        if self.patterns.is_empty() {
            return false;
        }
        let sep = usize::from(!parent_prefix.is_empty());
        let mut path = String::with_capacity(parent_prefix.len() + sep + segment.len());
        path.push_str(parent_prefix);
        let path = push_match_segment(path, segment);
        self.rules.matched(&path, false).is_ignore()
    }
}

/// Compiles `bytes` as the `.gitignore` rules of `dir` after stripping leading UTF-8 BOMs.
///
/// Invalid UTF-8, unsupported POSIX named character classes, and rule-set compilation failures
/// return an error message. POSIX classes are rejected before other invalid globs are skipped.
pub(crate) fn compile_gitignore(dir: &Path, bytes: &[u8]) -> Result<Gitignore, String> {
    let text = std::str::from_utf8(bytes).map_err(|err| err.to_string())?;
    // Matches `ignore::gitignore`: every leading BOM on the first line is stripped.
    let text = text.trim_start_matches('\u{feff}');
    let mut builder = GitignoreBuilder::new(dir);
    for (index, line) in text.lines().enumerate() {
        if has_posix_class(line) {
            return Err(format!("line {}: {UNSUPPORTED_POSIX_CLASS}", index + 1));
        }
        // An invalid glob line is skipped; the other lines still apply.
        let _ = builder.add_line(None, trim_rule_end(line));
    }
    builder.build().map_err(|err| err.to_string())
}

/// Trims trailing whitespace without discarding a backslash-escaped space.
fn trim_rule_end(pattern: &str) -> &str {
    let mut end = pattern.len();
    for (index, ch) in pattern.char_indices().rev() {
        if !ch.is_whitespace() {
            break;
        }
        if ch == ' '
            && pattern.as_bytes()[..index]
                .iter()
                .rev()
                .take_while(|&&b| b == b'\\')
                .count()
                % 2
                == 1
        {
            break;
        }
        end = index;
    }
    &pattern[..end]
}

/// Appends one path segment to a match prefix, joined with `/` when the prefix is not empty.
///
/// A `\` inside a segment is matched as U+FFFD on every platform, so it is never a separator and never
/// matches a `\` in a pattern. Invalid UTF-8 is matched by its lossy form.
#[must_use]
fn push_match_segment(mut prefix: String, segment: &[u8]) -> String {
    if !prefix.is_empty() {
        prefix.push('/');
    }
    if let Ok(name) = std::str::from_utf8(segment)
        && !name.contains('\\')
    {
        prefix.push_str(name);
    } else {
        prefix.extend(
            String::from_utf8_lossy(segment)
                .chars()
                .map(|c| if c == '\\' { '\u{FFFD}' } else { c }),
        );
    }
    prefix
}

/// Compiles one pattern and requires exactly one rule.
///
/// Rejects rules with an empty body after stripping `!` and `/` (e.g., `!`, `! `).
fn check_one_rule(pattern: &str, rule: &str) -> Result<(), DomainError> {
    let body = rule.strip_prefix('!').unwrap_or(rule).trim_matches('/');
    if body.is_empty() {
        return Err(no_rule(pattern));
    }
    if has_posix_class(pattern) {
        return Err(invalid_pattern(pattern, UNSUPPORTED_POSIX_CLASS));
    }
    let single = user_builder()
        .add_line(None, rule)
        .and_then(|line| line.build())
        .map_err(|err| invalid_pattern(pattern, err))?;
    if single.len() == 1 {
        Ok(())
    } else {
        Err(no_rule(pattern))
    }
}

const UNSUPPORTED_POSIX_CLASS: &str = "POSIX character classes in globs are not supported";

/// Detects named classes inside bracket expressions, which `globset` would match as literal text.
///
/// A complete named class is unsupported even if its enclosing bracket expression is unclosed.
#[must_use]
fn has_posix_class(pattern: &str) -> bool {
    if pattern.starts_with('#') || !pattern.contains("[:") {
        return false;
    }
    let mut tail = pattern.as_bytes();
    while let Some((&byte, rest)) = tail.split_first() {
        tail = rest;
        match byte {
            b'\\' => tail = tail.get(1..).unwrap_or_default(),
            b'[' => {
                if let [b'!' | b'^', rest @ ..] = tail {
                    tail = rest;
                }
                // A closing bracket is literal when it is the first member of a class.
                let leading_literal = tail.strip_prefix(b"]");
                tail = leading_literal.unwrap_or(tail);
                let mut can_start_range = leading_literal.is_some();
                while let Some((&byte, rest)) = tail.split_first() {
                    tail = rest;
                    match (byte, tail) {
                        (b'\\', _) => tail = tail.get(1..).unwrap_or_default(),
                        (b']', _) => break,
                        (b'-', [end, rest @ ..]) if can_start_range && *end != b']' => {
                            // Git consumes the endpoint literally, even if it is `[`. A
                            // completed range cannot start another range.
                            tail = if *end == b'\\' {
                                rest.get(1..).unwrap_or_default()
                            } else {
                                rest
                            };
                            can_start_range = false;
                            continue;
                        }
                        (b'[', [b':', name @ ..]) => {
                            let len = name.iter().take_while(|c| c.is_ascii_alphabetic()).count();
                            if len > 0 && name[len..].starts_with(b":]") {
                                return true;
                            }
                        }
                        _ => {}
                    }
                    can_start_range = true;
                }
            }
            _ => {}
        }
    }
    false
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

fn invalid_pattern(pattern: &str, reason: impl std::fmt::Display) -> DomainError {
    DomainError::usage(format!(
        "invalid value '{pattern}' for '--exclude <PATTERN>': {reason}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::error::ErrorDetail;
    use crate::core::path::{AdmittedDir, admit_dir, classify_leaf};
    use crate::core::policy::{MAX_PATH_BYTES, MAX_PATH_DEPTH};

    fn patterns(list: &[&str]) -> ExcludePatterns {
        ExcludePatterns::new(list.iter().copied()).expect("patterns are valid")
    }

    /// Whether `patterns` reject the path as the walk and the classifier apply it: a directory is
    /// rejected when admitting it returns `Ok(None)`, and a file when it classifies as `None` (Ignored).
    /// Paths exceeding `MAX_PATH_DEPTH` fail admission with `Err(DomainError)`.
    fn rejected(patterns: &ExcludePatterns, segments: &[&[u8]], is_dir: bool) -> bool {
        if !is_dir {
            let mut filtered = segments
                .iter()
                .copied()
                .filter(|seg| !seg.is_empty() && *seg != b".");
            let Some(mut leaf) = filtered.next() else {
                return false;
            };
            let mut dir = AdmittedDir::root();
            for next in filtered {
                let Ok(Some(admitted)) = admit_dir(dir, leaf, patterns, "test path") else {
                    return true;
                };
                dir = admitted;
                leaf = next;
            }
            return classify_leaf(&dir, leaf, patterns).is_none();
        }
        let mut dir = AdmittedDir::root();
        for name in segments {
            match admit_dir(dir, name, patterns, "test path") {
                Ok(Some(admitted)) => dir = admitted,
                _ => return true,
            }
        }
        false
    }

    impl ExcludePatterns {
        /// Whether the user patterns exclude the path. It must be rejected with these patterns but not
        /// with none, so hidden and noise names are never reported as user-excluded.
        fn excludes(&self, segments: &[&[u8]], is_dir: bool) -> bool {
            rejected(self, segments, is_dir) && !rejected(&Self::default(), segments, is_dir)
        }
    }

    #[test]
    fn new_rejects_patterns_that_are_invalid_or_yield_no_rule() {
        for pattern in [
            "", "   ", "#note", "!", "! ", "!\t", "/ ", "!/ ", "[", "[\x1b[2J",
        ] {
            let Err(err) = ExcludePatterns::new([pattern]) else {
                panic!("expected rejection for {pattern:?}");
            };
            let rendered = err.to_string();
            assert!(!rendered.contains('\x1b'), "ESC leaked: {rendered:?}");
            let expected = format!(
                "invalid value '{}' for '--exclude <PATTERN>': ",
                ErrorDetail::new(pattern)
            );
            assert!(
                matches!(&err, DomainError::Usage(msg) if msg.starts_with(&expected)),
                "{pattern:?}: {err}"
            );
        }
    }

    #[test]
    fn posix_character_classes_fail_closed() {
        for pattern in [
            "/config[[:digit:]].txt",
            "*[!ab[:upper:]].txt",
            "[][:alpha:]]",
            "[-[:digit:]]",
            "[0-9-[:digit:]]",
        ] {
            assert!(
                matches!(
                    ExcludePatterns::new([pattern]),
                    Err(DomainError::Usage(detail)) if detail.contains("POSIX character classes")
                ),
                "{pattern:?}"
            );
        }
        // An escaped bracket or the characters ':digit:' in an ordinary class are not a POSIX class.
        for pattern in [r"config\[[:digit:].txt", "config[:digit:].txt"] {
            assert!(ExcludePatterns::new([pattern]).is_ok(), "{pattern:?}");
        }
    }

    #[test]
    fn new_accepts_gitignore_syntax() {
        for pattern in ["*.log", "/top", "build/", "!keep.log", "-foo", "\\#literal"] {
            assert!(ExcludePatterns::new([pattern]).is_ok(), "{pattern}");
        }
        assert_eq!(
            ExcludePatterns::default(),
            ExcludePatterns::new(Vec::<String>::new()).unwrap()
        );
    }

    #[test]
    fn matching_rules_match_gitignore_semantics() {
        type MatchingCase<'a> = (&'a [&'a str], &'a [&'a [u8]], bool, bool);
        let cases: &[MatchingCase<'_>] = &[
            // Trailing whitespace is trimmed; `\ ` preserves a literal trailing space.
            (&["*.log "], &[b"a.log"], false, true),
            (&["a\\ "], &[b"a "], false, true),
            (&["a\\ "], &[b"a"], false, false),
            // Empty set excludes nothing.
            (&[], &[b"a", b"b.txt"], false, false),
            (&[], &[b"build"], true, false),
            // Unanchored patterns match at any depth, including directories and their descendants.
            (&["*.log"], &[b"a.log"], false, true),
            (&["*.log"], &[b"sub", b"deeper", b"a.log"], false, true),
            (&["*.log"], &[b"sub", b"a.txt"], false, false),
            (&["target"], &[b"target", b"debug", b"x.rs"], false, true),
            (&["target"], &[b"sub", b"target"], true, true),
            // Anchored patterns match from the source root only.
            (&["/top"], &[b"top"], false, true),
            (&["/top"], &[b"sub", b"top"], false, false),
            // Directory-only patterns require a directory segment.
            (&["build/"], &[b"build"], true, true),
            (&["build/"], &[b"build"], false, false),
            (&["build/"], &[b"build", b"out.txt"], false, true),
            (&["build/"], &[b"sub", b"build", b"out.txt"], false, true),
            // Negation reinclusively overrides earlier user rules, except under an excluded dir.
            (&["*.log", "!keep.log"], &[b"a.log"], false, true),
            (&["*.log", "!keep.log"], &[b"keep.log"], false, false),
            (
                &["*.log", "!keep.log"],
                &[b"sub", b"keep.log"],
                false,
                false,
            ),
            (&["build/", "!build/keep.txt"], &[b"build"], true, true),
            (
                &["build/", "!build/keep.txt"],
                &[b"build", b"keep.txt"],
                false,
                true,
            ),
            // Case-sensitive and non-UTF-8 lossy matching.
            (&["*.LOG"], &[b"a.LOG"], false, true),
            (&["*.LOG"], &[b"a.log"], false, false),
            (&["*.txt"], &[b"dir", b"\xff.txt"], false, true),
        ];

        for &(pats, segments, is_dir, expected) in cases {
            let ex = if pats.is_empty() {
                ExcludePatterns::default()
            } else {
                patterns(pats)
            };
            assert_eq!(
                ex.excludes(segments, is_dir),
                expected,
                "patterns={pats:?}, is_dir={is_dir}"
            );
        }
    }

    #[test]
    fn both_rule_sources_preserve_escaped_trailing_spaces() {
        let root = Path::new("repo");
        for pattern in ["notes\\  ", "notes\\ \t "] {
            let name = "notes ";
            let gitignore = compile_gitignore(root, pattern.as_bytes()).unwrap();
            let user = patterns(&[pattern]);
            assert!(gitignore.matched(root.join(name), false).is_ignore());
            assert!(user.excludes_leaf("", name.as_bytes()));
            assert!(!gitignore.matched(root.join("notes"), false).is_ignore());
            assert!(!user.excludes_leaf("", b"notes"));
        }
        assert_eq!(trim_rule_end("notes\\\\  "), "notes\\\\");
    }

    #[test]
    fn matching_applies_up_to_depth_limit_regardless_of_byte_length() {
        let ex = patterns(&["file.txt"]);

        let shallow: Vec<&[u8]> = std::iter::repeat_n(b"d".as_slice(), MAX_PATH_DEPTH)
            .chain(std::iter::once(b"file.txt".as_slice()))
            .collect();
        assert!(ex.excludes(&shallow, false));

        // Byte length does not block matching: a match within depth is found however long the prefix is.
        let head = "a".repeat(MAX_PATH_BYTES + 1);
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
    fn equality_compares_patterns() {
        assert_eq!(patterns(&["a", "b"]), patterns(&["a", "b"]));
        assert_ne!(patterns(&["a"]), patterns(&["a", "b"]));
    }

    #[test]
    fn compile_gitignore_strips_bom_skips_invalid_globs_and_rejects_bad_inputs() {
        let dir = Path::new("root");
        let bom = compile_gitignore(dir, "\u{feff}*.bak\n".as_bytes()).expect("BOM line parses");
        assert!(bom.matched("root/x.bak", false).is_ignore());

        let invalid = compile_gitignore(dir, b"[\n*.dat\n").expect("invalid glob line is skipped");
        assert!(invalid.matched("root/z.dat", false).is_ignore());

        // Every leading BOM is stripped, matching `ignore::gitignore`.
        let double_bom = compile_gitignore(dir, "\u{feff}\u{feff}*.bak\n".as_bytes())
            .expect("double BOM line parses");
        assert!(double_bom.matched("root/x.bak", false).is_ignore());

        let err = compile_gitignore(dir, b"ok\n\xff\n").expect_err("non-UTF-8 line is rejected");
        assert_eq!(err, "invalid utf-8 sequence of 1 bytes from index 3");

        let posix_err = compile_gitignore(dir, b"/config[[:digit:]].txt\n")
            .expect_err("POSIX character class is rejected");
        assert!(posix_err.contains("POSIX character classes"), "{posix_err}");
        assert!(compile_gitignore(dir, b"# /config[[:digit:]].txt\n").is_ok());

        let empty = compile_gitignore(dir, b"").expect("empty file parses");
        assert!(empty.matched("root/a.txt", false).is_none());
    }

    mod properties {
        use super::*;
        use proptest::prelude::*;

        fn name() -> impl Strategy<Value = String> {
            "[a-z]{1,8}"
        }

        fn pattern_vec() -> impl Strategy<Value = Vec<String>> {
            let pat = prop_oneof![
                name(),
                name().prop_map(|n| format!("!{n}")),
                name().prop_map(|n| format!("{n}/")),
                name().prop_map(|n| format!("*.{n}")),
            ];
            prop::collection::vec(pat, 0..4)
        }

        proptest! {
            #![proptest_config(crate::core::proptest_config())]

            /// Combining validated sets preserves the behavior of their concatenated rules.
            #[test]
            fn exclude_pattern_combination_matches_concatenated_rules(
                pa in pattern_vec(),
                pb in pattern_vec(),
                seg in "[a-z.]{1,8}",
            ) {
                let a = ExcludePatterns::new(pa.clone()).expect("valid patterns");
                let b = ExcludePatterns::new(pb.clone()).expect("valid patterns");
                let combined = a.try_combine(b).expect("small rule sets compile together");
                let concatenated = ExcludePatterns::new(pa.into_iter().chain(pb)).expect("valid concat");
                prop_assert_eq!(&combined, &concatenated);
                prop_assert_eq!(
                    combined.excludes_leaf("sub", seg.as_bytes()),
                    concatenated.excludes_leaf("sub", seg.as_bytes())
                );
                prop_assert_eq!(
                    combined.advance(String::new(), seg.as_bytes()),
                    concatenated.advance(String::new(), seg.as_bytes())
                );
            }

            /// Any text either compiles to a pattern set or is a usage error: it never panics, and a
            /// compiled set answers every query.
            #[test]
            fn arbitrary_patterns_compile_or_report_a_usage_error(
                pattern in prop::collection::vec(
                    prop::sample::select(&['a', 'b', '*', '?', '[', ']', '!', '#', '\\', ' ', '/', '-', '.', '\t'][..]),
                    0..10,
                ),
                candidate in "[a-c/.]{0,8}",
            ) {
                let pattern = String::from_iter(pattern);
                match ExcludePatterns::new([pattern.as_str()]) {
                    Ok(set) => {
                        let _ = set.excludes_leaf("", candidate.as_bytes());
                        let _ = set.advance(String::new(), candidate.as_bytes());
                    }
                    Err(err) => prop_assert!(matches!(err, DomainError::Usage(_)), "{:?}", err),
                }
            }

            /// A plain name excludes that name at any depth and leaves every other name as the empty
            /// pattern set classifies it.
            #[test]
            fn plain_name_excludes_only_that_name(n in name(), other in name()) {
                prop_assume!(n != other);
                let set = patterns(&[&n]);
                let none = ExcludePatterns::default();
                for is_dir in [false, true] {
                    prop_assert!(rejected(&set, &[n.as_bytes()], is_dir));
                    prop_assert!(rejected(&set, &[b"d", n.as_bytes()], is_dir));
                    prop_assert_eq!(
                        rejected(&set, &[other.as_bytes()], is_dir),
                        rejected(&none, &[other.as_bytes()], is_dir)
                    );
                }
            }

            /// A trailing `/` limits a pattern to directories: it excludes a directory of that name
            /// but leaves a file of that name as the empty pattern set classifies it.
            #[test]
            fn trailing_slash_pattern_matches_directories_only(n in name()) {
                let set = patterns(&[&format!("{n}/")]);
                let none = ExcludePatterns::default();
                prop_assert!(rejected(&set, &[n.as_bytes()], true));
                prop_assert_eq!(
                    rejected(&set, &[n.as_bytes()], false),
                    rejected(&none, &[n.as_bytes()], false)
                );
            }
        }
    }

    #[test]
    fn excludes_leaf_matches_advance_across_fast_and_fallback_paths() {
        let sets = [
            ExcludePatterns::default(),
            patterns(&["*.log", "/root.txt", "sub/*.tmp", "*\u{FFFD}*"]),
        ];
        let cases: &[(&str, &[u8])] = &[
            ("", b"root.txt"),
            ("", b"keep.rs"),
            ("", b"app.log"),
            ("", b"a\\b.txt"),
            ("", b"bad\xffname"),
            ("sub", b"root.txt"),
            ("sub", b"a.tmp"),
            ("sub", b"a\\b.txt"),
            ("sub/deep", b"bad\xff.log"),
        ];
        for set in &sets {
            for &(prefix, segment) in cases {
                let path = push_match_segment(prefix.to_string(), segment);
                assert_eq!(
                    set.excludes_leaf(prefix, segment),
                    !set.patterns.is_empty() && set.rules.matched(&path, false).is_ignore(),
                    "prefix={prefix:?}, segment={segment:?}"
                );
            }
        }
    }

    #[test]
    fn combining_valid_batches_reports_regex_set_limit() {
        let rule = format!("/[ab]/{}", "[cd]".repeat(1536));
        // NFA states use more memory on 64-bit targets. One batch fits; two do not.
        let count = 512 / std::mem::size_of::<usize>();
        let batch = ExcludePatterns::new(std::iter::repeat_n(rule.as_str(), count))
            .expect("one batch fits the regex set limit");

        assert!(matches!(
            batch.clone().try_combine(batch),
            Err(DomainError::Usage(_))
        ));
    }

    #[test]
    fn combining_exclusions_preserves_order_and_empty_identity() -> Result<(), DomainError> {
        let empty = ExcludePatterns::empty();
        assert!(empty.is_empty());
        assert_eq!(empty, ExcludePatterns::default());

        let first = patterns(&["build/", "*.log"]);
        let second = patterns(&["!keep.log", "/top"]);
        assert!(!first.is_empty());

        assert_eq!(ExcludePatterns::empty().try_combine(first.clone())?, first);
        assert_eq!(first.clone().try_combine(ExcludePatterns::empty())?, first);

        let combined = first.try_combine(second)?;
        assert!(!combined.is_empty());
        assert!(combined.excludes_leaf("", b"app.log"));
        assert!(!combined.excludes_leaf("", b"keep.log"));
        assert!(combined.excludes_leaf("", b"top"));
        assert_eq!(combined.advance(String::new(), b"build"), None);

        Ok(())
    }
}
