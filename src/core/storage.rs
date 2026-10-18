//! Continuous arena storage and zero-copy views for collected files.
//!
//! Eliminates per-file heap allocations by maintaining contiguous string arenas
//! for paths and contents, indexed by compact 16-byte [`FileSpan`] records.

use std::cmp::Ordering;

/// 16-byte POD indexing a file path and content slice in [`FileStorage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct FileSpan {
    path_offset: u32,
    path_len: u32,
    content_offset: u32,
    content_len: u32,
}

impl FileSpan {
    #[inline(always)]
    pub(super) fn path<'a>(&self, arena: &'a str) -> &'a str {
        let start = self.path_offset as usize;
        let end = start + self.path_len as usize;
        &arena[start..end]
    }

    #[inline(always)]
    pub(super) fn content<'a>(&self, arena: &'a str) -> &'a str {
        let start = self.content_offset as usize;
        let end = start + self.content_len as usize;
        &arena[start..end]
    }
}

/// Zero-copy borrowed view into a file record stored within [`FileStorage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileRecord<'a> {
    pub(crate) path: &'a str,
    pub(crate) content: &'a str,
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
    #[cfg(test)]
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
    pub(crate) fn push(&mut self, path: &str, content: &str) {
        debug_assert!(!path.is_empty(), "file path must not be empty");
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
    pub(crate) fn iter(&self) -> FileRecordIter<'_> {
        FileRecordIter {
            storage: self,
            index: 0,
        }
    }
}

/// Zero-copy iterator yielding borrowed [`FileRecord`] items from [`FileStorage`].
pub(crate) struct FileRecordIter<'a> {
    storage: &'a FileStorage,
    index: usize,
}

impl<'a> Iterator for FileRecordIter<'a> {
    type Item = FileRecord<'a>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.index < self.storage.len() {
            let span = self.storage.spans[self.index];
            self.index += 1;
            Some(self.storage.get(span))
        } else {
            None
        }
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.storage.len().saturating_sub(self.index);
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for FileRecordIter<'_> {
    #[inline]
    fn len(&self) -> usize {
        self.storage.len().saturating_sub(self.index)
    }
}

impl<'a> IntoIterator for &'a FileStorage {
    type Item = FileRecord<'a>;
    type IntoIter = FileRecordIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[cfg(test)]
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
fn cmp_path(a: &str, b: &str) -> Ordering {
    a.split('/').cmp(b.split('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_storage_maintains_continuous_arena_and_zero_copy_views() {
        let mut storage = FileStorage::new();
        assert_eq!(storage.len(), 0);

        storage.push("path/to/first.rs", "pub fn first() {}");
        storage.push("second.txt", "second content\n");

        assert_eq!(storage.len(), 2);

        let span1 = storage.spans()[0];
        let span2 = storage.spans()[1];
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

        // IntoIterator verification
        let into_collected: Vec<FileRecord<'_>> = (&storage).into_iter().collect();
        assert_eq!(into_collected, vec![rec1, rec2]);

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
}
