//! Candidate path, probe, and evaluation items.

use std::borrow::Cow;

/// Path text as observed by the imperative shell. Non-UTF-8 paths are still
/// candidates and must be counted, then skipped.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ItemPath<'a> {
    Utf8(Cow<'a, str>),
    NonUtf8,
}

impl<'a> ItemPath<'a> {
    #[inline]
    pub fn as_utf8_str(&self) -> Option<&str> {
        match self {
            Self::Utf8(s) => Some(s.as_ref()),
            Self::NonUtf8 => None,
        }
    }
}

impl<'a> From<&'a str> for ItemPath<'a> {
    fn from(s: &'a str) -> Self {
        Self::Utf8(Cow::Borrowed(s))
    }
}

impl<'a> From<String> for ItemPath<'a> {
    fn from(s: String) -> Self {
        Self::Utf8(Cow::Owned(s))
    }
}

/// Result of the shell's bounded resource acquisition. `TooLarge` and `Missing`
/// carry no content and consume a candidate slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probe<'a> {
    Bytes(Cow<'a, [u8]>),
    Missing,
    TooLarge,
}

impl<'a> From<&'a [u8]> for Probe<'a> {
    fn from(b: &'a [u8]) -> Self {
        Self::Bytes(Cow::Borrowed(b))
    }
}

impl<'a> From<Vec<u8>> for Probe<'a> {
    fn from(v: Vec<u8>) -> Self {
        Self::Bytes(Cow::Owned(v))
    }
}

#[derive(Clone, Debug)]
pub struct Item<'a> {
    pub path: ItemPath<'a>,
    pub probe: Probe<'a>,
}

/// Pure decision result evaluating an observed candidate item against content,
/// encoding, and size invariants.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ItemDecision<'a> {
    Included { path: &'a str, content: &'a str },
    Skipped { path: Option<&'a str> },
}

impl<'a> ItemDecision<'a> {
    #[inline]
    pub(crate) const fn path(&self) -> Option<&'a str> {
        match self {
            Self::Included { path, .. } => Some(path),
            Self::Skipped { path } => *path,
        }
    }
}

impl<'a> Item<'a> {
    #[inline]
    pub(crate) fn evaluate(&self) -> ItemDecision<'_> {
        match (&self.path, &self.probe) {
            (ItemPath::Utf8(path), Probe::Bytes(bytes)) => match text_str(bytes.as_ref()) {
                Some(content) => ItemDecision::Included {
                    path: path.as_ref(),
                    content,
                },
                None => ItemDecision::Skipped {
                    path: Some(path.as_ref()),
                },
            },
            (ItemPath::Utf8(path), Probe::Missing | Probe::TooLarge) => ItemDecision::Skipped {
                path: Some(path.as_ref()),
            },
            (ItemPath::NonUtf8, _) => ItemDecision::Skipped { path: None },
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

    fn bytes_probe<'a>(bytes: impl Into<Cow<'a, [u8]>>) -> Probe<'a> {
        Probe::Bytes(bytes.into())
    }

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

    #[test]
    fn item_resolve_classifies_all_variants() {
        let missing = Item {
            path: ItemPath::Utf8("binary.png".into()),
            probe: Probe::Missing,
        };
        match missing.evaluate() {
            ItemDecision::Skipped { path: Some(path) } => assert_eq!(path, "binary.png"),
            _ => panic!("expected Skipped(Some)"),
        }

        let included = Item {
            path: ItemPath::Utf8("included.txt".into()),
            probe: bytes_probe(b"hello".to_vec()),
        };
        match included.evaluate() {
            ItemDecision::Included { path, content } => {
                assert_eq!(path, "included.txt");
                assert_eq!(content, "hello");
            }
            _ => panic!("expected Included"),
        }

        let binary = Item {
            path: ItemPath::Utf8("binary.bin".into()),
            probe: bytes_probe(vec![0, 1, 2]),
        };
        match binary.evaluate() {
            ItemDecision::Skipped { path: Some(path) } => assert_eq!(path, "binary.bin"),
            _ => panic!("expected Skipped(Some)"),
        }

        let non_utf8 = Item {
            path: ItemPath::NonUtf8,
            probe: bytes_probe(b"text".to_vec()),
        };
        assert!(matches!(
            non_utf8.evaluate(),
            ItemDecision::Skipped { path: None }
        ));
    }

    #[test]
    fn item_evaluation_separates_decisions_without_heap_allocation() {
        let utf8_item = Item {
            path: ItemPath::Utf8(Cow::Borrowed("src/lib.rs")),
            probe: bytes_probe(b"pub fn run() {}".to_vec()),
        };
        assert_eq!(
            utf8_item.evaluate(),
            ItemDecision::Included {
                path: "src/lib.rs",
                content: "pub fn run() {}",
            }
        );

        let binary_item = Item {
            path: ItemPath::Utf8(Cow::Borrowed("assets/image.png")),
            probe: bytes_probe(vec![0x89, 0x50, 0x00, 0x47]),
        };
        assert_eq!(
            binary_item.evaluate(),
            ItemDecision::Skipped {
                path: Some("assets/image.png"),
            }
        );

        let missing_item = Item {
            path: ItemPath::Utf8(Cow::Borrowed("missing.txt")),
            probe: Probe::Missing,
        };
        assert_eq!(
            missing_item.evaluate(),
            ItemDecision::Skipped {
                path: Some("missing.txt"),
            }
        );

        let non_utf8_item = Item {
            path: ItemPath::NonUtf8,
            probe: Probe::Missing,
        };
        assert_eq!(
            non_utf8_item.evaluate(),
            ItemDecision::Skipped { path: None }
        );
    }
}
