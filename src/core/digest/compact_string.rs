//! Zero-dependency Small String Optimization (SVO) for budget trie paths and nodes.

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CompactString {
    Inline([u8; 23], u8),
    Heap(String),
}

impl Default for CompactString {
    fn default() -> Self {
        CompactString::Inline([0; 23], 0)
    }
}

impl CompactString {
    pub(crate) fn new(s: &str) -> Self {
        let bytes = s.as_bytes();
        if bytes.len() <= 23 {
            let mut arr = [0u8; 23];
            arr[..bytes.len()].copy_from_slice(bytes);
            CompactString::Inline(arr, bytes.len() as u8)
        } else {
            CompactString::Heap(s.to_string())
        }
    }

    #[inline]
    pub(crate) fn as_str(&self) -> &str {
        match self {
            CompactString::Inline(arr, len) => {
                // SAFETY: Constructed from valid UTF-8 str slice
                unsafe { std::str::from_utf8_unchecked(&arr[..*len as usize]) }
            }
            CompactString::Heap(s) => s.as_str(),
        }
    }
}

impl std::ops::Deref for CompactString {
    type Target = str;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl std::borrow::Borrow<str> for CompactString {
    #[inline]
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl std::convert::AsRef<str> for CompactString {
    #[inline]
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_string_stores_inline_or_heap() {
        let short = CompactString::new("short_name.rs");
        assert!(matches!(short, CompactString::Inline(_, 13)));
        assert_eq!(short.as_str(), "short_name.rs");

        let long_str = "a_very_long_path_segment_exceeding_23_bytes.rs";
        let long = CompactString::new(long_str);
        assert!(matches!(long, CompactString::Heap(_)));
        assert_eq!(long.as_str(), long_str);
    }
}
