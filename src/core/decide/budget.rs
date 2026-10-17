//! Candidate file budget, run-length directory accounting, and overflow diagnostics.

use std::collections::HashMap;

use crate::core::error::IngestError;
use crate::core::policy::MAX_FILE_COUNT;

#[derive(Clone, Debug)]
pub(crate) enum CompactDir {
    Inline { len: u8, buf: [u8; 22] },
    Heap(Box<str>),
}

impl CompactDir {
    #[inline]
    pub(crate) fn as_str(&self) -> &str {
        match self {
            Self::Inline { len, buf } => std::str::from_utf8(&buf[..*len as usize]).unwrap_or(""),
            Self::Heap(s) => s.as_ref(),
        }
    }

    #[inline]
    pub(crate) fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Inline { len, buf } => &buf[..*len as usize],
            Self::Heap(s) => s.as_bytes(),
        }
    }
}

impl From<&str> for CompactDir {
    fn from(s: &str) -> Self {
        if s.len() <= 22 {
            let mut buf = [0u8; 22];
            buf[..s.len()].copy_from_slice(s.as_bytes());
            Self::Inline {
                len: s.len() as u8,
                buf,
            }
        } else {
            Self::Heap(s.into())
        }
    }
}

/// Candidate-count budget owned by the selector that consumes it.
#[derive(Default, Debug)]
pub struct CandidateBudget {
    count: usize,
    dir_runs: Vec<(CompactDir, usize)>,
}

impl CandidateBudget {
    pub const fn new() -> Self {
        Self {
            count: 0,
            dir_runs: Vec::new(),
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            count: 0,
            dir_runs: Vec::with_capacity(capacity),
        }
    }

    pub fn check_limit<'a, I>(paths: I) -> Result<(), IngestError>
    where
        I: IntoIterator<Item = Option<&'a str>>,
    {
        let mut iter = paths.into_iter();
        if matches!(iter.size_hint().1, Some(upper) if upper <= MAX_FILE_COUNT) {
            return Ok(());
        }
        let mut budget = Self::default();
        iter.try_for_each(|path| budget.add(path))
    }

    pub fn add(&mut self, path: Option<&str>) -> Result<(), IngestError> {
        self.count = self.count.checked_add(1).ok_or_else(file_count_overflow)?;
        self.record_dir_run(extract_dir_slice(path));

        if self.count > MAX_FILE_COUNT {
            return Err(self.overflow_error());
        }
        Ok(())
    }

    fn record_dir_run(&mut self, dir_slice: &str) {
        let dir_bytes = dir_slice.as_bytes();
        if let Some(entry) = self
            .dir_runs
            .iter_mut()
            .rev()
            .take(16)
            .find(|(dir, _)| dir.as_bytes() == dir_bytes)
        {
            entry.1 += 1;
        } else {
            self.dir_runs.push((CompactDir::from(dir_slice), 1));
        }
    }

    #[cold]
    #[inline(never)]
    fn overflow_error(&self) -> IngestError {
        let mut aggregated: HashMap<&str, usize> = HashMap::new();
        for (dir, count) in &self.dir_runs {
            *aggregated.entry(dir.as_str()).or_insert(0) += count;
        }
        IngestError::candidate_limit_exceeded(MAX_FILE_COUNT, top_overflow_directories(&aggregated))
    }
}

#[cold]
#[inline(never)]
fn file_count_overflow() -> IngestError {
    IngestError::invalid_metadata("file count overflow")
}

fn extract_dir_slice(path: Option<&str>) -> &str {
    match path {
        Some(p) => match p.find('/') {
            Some(index) => &p[..=index],
            None => "<root>/",
        },
        None => "<unknown>/",
    }
}

fn top_overflow_directories(dir_counts: &HashMap<&str, usize>) -> Vec<(String, usize)> {
    let mut dirs: Vec<(&str, usize)> = dir_counts.iter().map(|(&d, &c)| (d, c)).collect();
    dirs.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    dirs.truncate(3);
    dirs.into_iter().map(|(d, c)| (d.to_string(), c)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compact_dir_layout_and_parity() {
        assert!(std::mem::size_of::<CompactDir>() <= 24);
        assert!(std::mem::size_of::<(CompactDir, usize)>() <= 32);

        let short_dir = CompactDir::from("src/");
        assert_eq!(short_dir.as_str(), "src/");

        let long_str = "deeply/nested/directory/structure/path/";
        let long_dir = CompactDir::from(long_str);
        assert_eq!(long_dir.as_str(), long_str);
    }

    #[test]
    fn test_compact_dir_as_bytes() {
        let short_dir = CompactDir::from("src/");
        assert_eq!(short_dir.as_bytes(), b"src/");

        let long_str = "deeply/nested/directory/structure/path/";
        let long_dir = CompactDir::from(long_str);
        assert_eq!(long_dir.as_bytes(), long_str.as_bytes());
    }

    #[test]
    fn candidate_budget_enforces_limit() {
        let mut candidates = CandidateBudget::default();
        for _ in 0..MAX_FILE_COUNT {
            candidates.add(None).unwrap();
        }
        assert!(candidates.add(None).is_err());
    }

    #[test]
    fn candidate_budget_check_limit_validates_batch() {
        let under_limit = (0..MAX_FILE_COUNT).map(|_| Some("dir/file.txt"));
        assert!(CandidateBudget::check_limit(under_limit).is_ok());

        let over_limit = (0..=MAX_FILE_COUNT).map(|_| Some("dir/file.txt"));
        assert!(CandidateBudget::check_limit(over_limit).is_err());
    }

    #[test]
    fn candidate_overflow_includes_largest_directories_diagnostic()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut budget = CandidateBudget::default();
        for index in 0..8_000 {
            budget.add(Some(&format!("vendor/pkg_{index}.rs")))?;
        }
        for index in 0..2_000 {
            budget.add(Some(&format!("src/file_{index}.rs")))?;
        }
        let err = budget.add(Some("extra/one.rs")).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("repository contains more than 10000 files"));
        assert!(message.contains("largest directories:"));
        assert!(message.contains("vendor/ (8000 files)"));
        assert!(message.contains("src/ (2000 files)"));
        Ok(())
    }

    #[test]
    fn candidate_budget_handles_many_distinct_directories_deterministically() {
        let mut budget = CandidateBudget::default();
        for i in 0..MAX_FILE_COUNT {
            let path = format!("dir_{i:05}/file.rs");
            budget.add(Some(&path)).unwrap();
        }
        let err = budget.add(Some("dir_00000/extra.rs")).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("repository contains more than 10000 files"));
        assert!(msg.contains("largest directories:"));
        assert!(msg.contains("- dir_00000/ (2 files)"));
    }

    #[test]
    fn extracts_dir_slice_from_paths() {
        assert_eq!(extract_dir_slice(Some("src/core/decide.rs")), "src/");
        assert_eq!(extract_dir_slice(Some("root_file.txt")), "<root>/");
        assert_eq!(extract_dir_slice(None), "<unknown>/");
    }
}
