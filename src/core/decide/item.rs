/// Candidate input observed by the imperative shell. Non-UTF-8 paths or unreadable
/// items are represented as `Skipped` and still counted against the file budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateInput<'a> {
    /// Bounded content bytes successfully acquired from the source.
    File { path: &'a str, bytes: &'a [u8] },
    /// Item exceeded the per-file size limit.
    TooLarge { path: &'a str },
    /// Item skipped due to metadata error, non-UTF-8 path, or read failure.
    Skipped { path: Option<&'a str> },
}

impl<'a> CandidateInput<'a> {
    #[inline]
    pub const fn path(&self) -> Option<&'a str> {
        match self {
            Self::File { path, .. } | Self::TooLarge { path } => Some(*path),
            Self::Skipped { path } => *path,
        }
    }
}

/// UTF-8 text predicate: NUL-free bytes only.
#[inline]
pub fn text_str(bytes: &[u8]) -> Option<&str> {
    if bytes.contains(&0) {
        return None;
    }
    std::str::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_str_detects_binary_and_handles_probe_boundary() {
        let mut bin = vec![b'a'; 1024];
        bin[12] = 0;
        assert!(text_str(&bin).is_none());

        let mut bin_tail = vec![b'b'; 5000];
        bin_tail[4500] = 0;
        assert!(text_str(&bin_tail).is_none());

        let mut bytes = vec![b'a'; 4095];
        bytes.extend_from_slice("€tail\n".as_bytes());
        let content = text_str(&bytes);
        assert!(content.is_some());
        assert!(content.unwrap().ends_with("€tail\n"));

        let mut bytes_with_nul = vec![b'a'; 4095];
        bytes_with_nul.extend_from_slice("€\0tail\n".as_bytes());
        assert!(text_str(&bytes_with_nul).is_none());
    }
}
