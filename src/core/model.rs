use std::path::PathBuf;

/// Ingestion source location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Local filesystem directory.
    Local(PathBuf),
    /// Public or authenticated GitHub repository.
    GitHub {
        /// Repository owner (user or organization).
        owner: String,
        /// Repository name.
        repo: String,
        /// Optional GitHub branch, tag, or commit SHA.
        reference: Option<String>,
    },
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

/// Ingestion options controlling destination and serialization format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Target destination for the rendered digest. Defaults to `digest.txt`.
    pub output: OutputTarget,
    /// Serialization format for the digest. Defaults to plain text.
    pub format: DigestFormat,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            output: OutputTarget::File(PathBuf::from("digest.txt")),
            format: DigestFormat::Text,
        }
    }
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
    pub fn new(source: Source, output: OutputTarget, format: DigestFormat) -> Self {
        Self {
            source,
            output,
            format,
        }
    }

    pub fn from_source_and_options(source: Source, options: Options) -> Self {
        Self {
            source,
            output: options.output,
            format: options.format,
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
pub struct FileSpan {
    pub path_offset: u32,
    pub path_len: u32,
    pub content_offset: u32,
    pub content_len: u32,
}

/// Continuous arena storage for file paths and contents.
/// Eliminates per-file heap allocations and preserves cache locality.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileStorage {
    pub spans: Vec<FileSpan>,
    pub path_arena: String,
    pub content_arena: String,
}

impl FileStorage {
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.spans.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    /// Appends path and content to the arenas and records a [`FileSpan`].
    pub fn push(&mut self, path: &str, content: &str) -> FileSpan {
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
    pub fn get(&self, span: FileSpan) -> FileRecord<'_> {
        let p_start = span.path_offset as usize;
        let p_end = p_start + span.path_len as usize;
        let c_start = span.content_offset as usize;
        let c_end = c_start + span.content_len as usize;

        FileRecord {
            path: &self.path_arena[p_start..p_end],
            content: &self.content_arena[c_start..c_end],
        }
    }

    /// Resolves a span by index into a borrowed [`FileRecord`].
    #[inline]
    pub fn get_at(&self, index: usize) -> Option<FileRecord<'_>> {
        self.spans.get(index).map(|&span| self.get(span))
    }

    /// Iterator yielding borrowed [`FileRecord`] views over the arena.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = FileRecord<'_>> {
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

impl<'a> FromIterator<&'a FileRecord<'a>> for FileStorage {
    fn from_iter<T: IntoIterator<Item = &'a FileRecord<'a>>>(iter: T) -> Self {
        let mut storage = Self::new();
        for record in iter {
            storage.push(record.path, record.content);
        }
        storage
    }
}

/// Zero-copy borrowed view into a file record stored within [`FileStorage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileRecord<'a> {
    pub path: &'a str,
    pub content: &'a str,
}

impl FileRecord<'_> {
    #[inline]
    pub fn path(&self) -> &str {
        self.path
    }

    #[inline]
    pub fn content(&self) -> &str {
        self.content
    }
}

/// Raw files and metadata collected by the shell before digest rendering.
#[derive(Debug, Clone)]
pub struct CollectionResult {
    pub label: String,
    pub revision: Option<String>,
    pub files: FileStorage,
    pub skipped: usize,
    pub trie: crate::core::digest::BudgetTrie,
    pub format: DigestFormat,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_storage_maintains_continuous_arena_and_zero_copy_views() {
        let mut storage = FileStorage::new();
        assert!(storage.is_empty());
        assert_eq!(storage.len(), 0);

        let span1 = storage.push("path/to/first.rs", "pub fn first() {}");
        let span2 = storage.push("second.txt", "second content\n");

        assert_eq!(storage.len(), 2);
        assert!(!storage.is_empty());

        let rec1 = storage.get(span1);
        assert_eq!(rec1.path(), "path/to/first.rs");
        assert_eq!(rec1.content(), "pub fn first() {}");

        let rec2 = storage.get(span2);
        assert_eq!(rec2.path(), "second.txt");
        assert_eq!(rec2.content(), "second content\n");

        assert_eq!(storage.get_at(0), Some(rec1));
        assert_eq!(storage.get_at(1), Some(rec2));
        assert_eq!(storage.get_at(2), None);

        let collected: Vec<FileRecord<'_>> = storage.iter().collect();
        assert_eq!(collected, vec![rec1, rec2]);

        // Arena contents verification
        assert_eq!(storage.path_arena, "path/to/first.rssecond.txt");
        assert_eq!(storage.content_arena, "pub fn first() {}second content\n");

        // FromIterator verification
        let reconstructed: FileStorage = collected.into_iter().collect();
        assert_eq!(reconstructed, storage);
    }
}
