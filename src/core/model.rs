use std::path::PathBuf;

use crate::core::error::IngestError;

/// Ingestion source location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Local filesystem directory.
    Local(PathBuf),
    /// Public or authenticated GitHub repository.
    Github {
        /// Repository owner (user or organization).
        owner: String,
        /// Repository name.
        repo: String,
        /// Optional GitHub branch, tag, or commit SHA.
        reference: Option<String>,
    },
}

impl Source {
    /// Returns a new `Source` instance with the specified Git revision reference.
    ///
    /// # Errors
    ///
    /// Returns [`IngestError::InvalidSource`] if `reference` is empty/whitespace or
    /// if called on a [`Source::Local`] variant.
    pub fn with_reference(self, reference: Option<String>) -> Result<Self, IngestError> {
        if reference
            .as_ref()
            .is_some_and(|r| r.trim_ascii().is_empty())
        {
            return Err(IngestError::invalid_source("--ref cannot be empty"));
        }
        match self {
            Self::Github { owner, repo, .. } => Ok(Self::Github {
                owner,
                repo,
                reference,
            }),
            Self::Local(_) => {
                if reference.is_some() {
                    Err(IngestError::invalid_source(
                        "--ref is only valid for GitHub sources",
                    ))
                } else {
                    Ok(self)
                }
            }
        }
    }
}

/// Where the digest is delivered. Distinguishes a filesystem path from the
/// standard output stream at the type level; there is no sentinel path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputTarget {
    /// Writes the digest atomically to a filesystem path.
    File(PathBuf),
    /// Writes the digest directly to standard output without file creation.
    Stdout,
}

/// Output serialization format for the LLM digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DigestFormat {
    /// Delimiter-separated plain text format (default).
    #[default]
    Text,
    /// Anthropic and LLM-optimized XML structured format.
    Xml,
}

/// Execution configuration unifying source, output destination, and format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunConfig {
    /// Ingestion source location.
    pub source: Source,
    /// Target destination for the rendered digest.
    pub output: OutputTarget,
    /// Serialization format for the digest.
    pub format: DigestFormat,
}

impl RunConfig {
    /// Creates a new `RunConfig` with the given source, output target, and format.
    pub const fn new(source: Source, output: OutputTarget, format: DigestFormat) -> Self {
        Self {
            source,
            output,
            format,
        }
    }
}

/// Summary report emitted upon successful digest generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestReport {
    /// Target destination where the digest was delivered.
    pub output: OutputTarget,
    /// Number of candidate files included in the rendered digest.
    pub included_files: usize,
    /// Number of candidate files rejected due to size, encoding, or binary content.
    pub skipped_files: usize,
    /// Approximate token count of the rendered digest.
    pub estimated_tokens: usize,
}

/// 16-byte POD indexing a file path and content slice in [`FileStorage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct FileSpan {
    pub(crate) path_offset: u32,
    pub(crate) path_len: u32,
    pub(crate) content_offset: u32,
    pub(crate) content_len: u32,
}

impl FileSpan {
    #[inline(always)]
    pub(crate) fn path<'a>(&self, arena: &'a str) -> &'a str {
        let start = self.path_offset as usize;
        let end = start + self.path_len as usize;
        &arena[start..end]
    }

    #[inline(always)]
    pub(crate) fn content<'a>(&self, arena: &'a str) -> &'a str {
        let start = self.content_offset as usize;
        let end = start + self.content_len as usize;
        &arena[start..end]
    }
}

/// Continuous arena storage for file paths and contents.
/// Eliminates per-file heap allocations and preserves cache locality.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FileStorage {
    spans: Vec<FileSpan>,
    path_arena: String,
    content_arena: String,
}

impl FileStorage {
    #[inline]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub(crate) fn with_capacity(spans_cap: usize, path_bytes: usize, content_bytes: usize) -> Self {
        Self {
            spans: Vec::with_capacity(spans_cap),
            path_arena: String::with_capacity(path_bytes),
            content_arena: String::with_capacity(content_bytes),
        }
    }

    #[inline]
    pub(crate) const fn len(&self) -> usize {
        self.spans.len()
    }

    #[inline]
    pub(crate) fn spans(&self) -> &[FileSpan] {
        &self.spans
    }

    #[inline]
    pub(crate) fn path_arena(&self) -> &str {
        &self.path_arena
    }

    #[inline]
    pub(crate) fn total_bytes(&self) -> u64 {
        (self.path_arena.len() + self.content_arena.len()) as u64
    }

    /// Sorts spans hierarchically by path components in-place within the path arena.
    pub(crate) fn sort_by_path(&mut self) {
        let path_arena = &self.path_arena;
        let is_sorted = self
            .spans
            .windows(2)
            .all(|w| cmp_path(w[0].path(path_arena), w[1].path(path_arena)).is_le());
        if is_sorted {
            return;
        }
        self.spans
            .sort_unstable_by(|a, b| cmp_path(a.path(path_arena), b.path(path_arena)));
    }

    /// Appends path and content to the arenas and records a [`FileSpan`].
    pub(crate) fn push(&mut self, path: &str, content: &str) -> FileSpan {
        debug_assert!(u32::try_from(self.path_arena.len()).is_ok());
        debug_assert!(u32::try_from(path.len()).is_ok());
        debug_assert!(u32::try_from(self.content_arena.len()).is_ok());
        debug_assert!(u32::try_from(content.len()).is_ok());

        let path_offset = self.path_arena.len() as u32;
        let path_len = path.len() as u32;
        self.path_arena.push_str(path);

        let content_offset = self.content_arena.len() as u32;
        let content_len = content.len() as u32;
        self.content_arena.push_str(content);

        let span = FileSpan {
            path_offset,
            path_len,
            content_offset,
            content_len,
        };
        self.spans.push(span);
        span
    }

    /// Resolves a [`FileSpan`] into a borrowed [`FileRecord`].
    #[inline]
    pub(crate) fn get(&self, span: FileSpan) -> FileRecord<'_> {
        FileRecord {
            path: span.path(&self.path_arena),
            content: span.content(&self.content_arena),
        }
    }

    /// Iterator yielding borrowed [`FileRecord`] views over the arena.
    pub(crate) fn iter(&self) -> impl ExactSizeIterator<Item = FileRecord<'_>> {
        self.spans.iter().map(move |&span| self.get(span))
    }
}

impl<'a> FromIterator<FileRecord<'a>> for FileStorage {
    fn from_iter<T: IntoIterator<Item = FileRecord<'a>>>(iter: T) -> Self {
        let mut storage = Self::new();
        for record in iter {
            storage.push(record.path, record.content);
        }
        storage
    }
}

/// Compares two normalized slash-separated relative paths hierarchically by path components.
pub(crate) fn cmp_path(a: &str, b: &str) -> std::cmp::Ordering {
    let mut a_parts = a.split('/');
    let mut b_parts = b.split('/');
    loop {
        match (a_parts.next(), b_parts.next()) {
            (Some(seg_a), Some(seg_b)) => match seg_a.cmp(seg_b) {
                std::cmp::Ordering::Equal => continue,
                other => return other,
            },
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (None, None) => return std::cmp::Ordering::Equal,
        }
    }
}

/// Zero-copy borrowed view into a file record stored within [`FileStorage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileRecord<'a> {
    pub(crate) path: &'a str,
    pub(crate) content: &'a str,
}

/// Raw files and metadata collected by the shell before digest rendering.
#[derive(Debug, Clone)]
pub(crate) struct CollectionResult {
    pub(crate) label: String,
    pub(crate) revision: Option<String>,
    pub(crate) files: FileStorage,
    pub(crate) skipped: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_storage_maintains_continuous_arena_and_zero_copy_views() {
        let mut storage = FileStorage::new();
        assert_eq!(storage.len(), 0);

        let span1 = storage.push("path/to/first.rs", "pub fn first() {}");
        let span2 = storage.push("second.txt", "second content\n");

        assert_eq!(storage.len(), 2);

        let rec1 = storage.get(span1);
        assert_eq!(rec1.path, "path/to/first.rs");
        assert_eq!(rec1.content, "pub fn first() {}");

        let rec2 = storage.get(span2);
        assert_eq!(rec2.path, "second.txt");
        assert_eq!(rec2.content, "second content\n");

        let mut iter = storage.iter();
        assert_eq!(iter.next(), Some(rec1));
        assert_eq!(iter.next(), Some(rec2));
        assert_eq!(iter.next(), None);

        let collected: Vec<FileRecord<'_>> = storage.iter().collect();
        assert_eq!(collected, vec![rec1, rec2]);

        // Arena contents verification
        assert_eq!(storage.path_arena, "path/to/first.rssecond.txt");
        assert_eq!(storage.content_arena, "pub fn first() {}second content\n");

        // FromIterator verification
        let reconstructed: FileStorage = collected.into_iter().collect();
        assert_eq!(reconstructed, storage);
    }

    #[test]
    fn file_storage_sorts_by_path_in_place() {
        let mut storage = FileStorage::new();
        storage.push("z/file.rs", "content_z");
        storage.push("a/file.rs", "content_a");
        storage.push("m/file.rs", "content_m");

        storage.sort_by_path();

        let paths: Vec<&str> = storage.iter().map(|f| f.path).collect();
        assert_eq!(paths, vec!["a/file.rs", "m/file.rs", "z/file.rs"]);
    }

    #[test]
    fn file_storage_sorts_hierarchically() {
        let mut storage = FileStorage::new();
        storage.push("src-test/b.rs", "content_b");
        storage.push("src/a.rs", "content_a");

        storage.sort_by_path();

        let paths: Vec<&str> = storage.iter().map(|f| f.path).collect();
        assert_eq!(paths, vec!["src/a.rs", "src-test/b.rs"]);
    }

    #[test]
    fn source_with_reference_validates_and_updates_reference() {
        let gh = Source::Github {
            owner: "rust-lang".into(),
            repo: "rust".into(),
            reference: None,
        };
        let updated = gh.with_reference(Some("main".into())).unwrap();
        assert_eq!(
            updated,
            Source::Github {
                owner: "rust-lang".into(),
                repo: "rust".into(),
                reference: Some("main".into()),
            }
        );

        let cleared = updated.with_reference(None).unwrap();
        assert_eq!(
            cleared,
            Source::Github {
                owner: "rust-lang".into(),
                repo: "rust".into(),
                reference: None,
            }
        );

        let empty_err = cleared
            .clone()
            .with_reference(Some("   ".into()))
            .unwrap_err();
        assert!(
            matches!(empty_err, IngestError::InvalidSource(msg) if msg.contains("--ref cannot be empty"))
        );

        let local = Source::Local(PathBuf::from("."));
        let local_ok = local.clone().with_reference(None).unwrap();
        assert_eq!(local_ok, local);

        let local_err = local.with_reference(Some("main".into())).unwrap_err();
        assert!(
            matches!(local_err, IngestError::InvalidSource(msg) if msg.contains("--ref is only valid for GitHub sources"))
        );
    }
}
