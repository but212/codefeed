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

/// Wrapper guaranteeing that [`FileStorage`] is sorted lexicographically by path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SortedFiles(FileStorage);

impl SortedFiles {
    #[inline]
    pub(crate) fn new(mut storage: FileStorage) -> Self {
        storage.sort_by_path();
        Self(storage)
    }
}

impl std::ops::Deref for SortedFiles {
    type Target = FileStorage;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
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
        Self {
            spans: Vec::with_capacity(512),
            path_arena: String::with_capacity(16 * 1024),
            content_arena: String::with_capacity(256 * 1024),
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
        self.spans
            .sort_by(|a, b| cmp_path(a.path(path_arena), b.path(path_arena)));
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
    pub(crate) fn iter(&self) -> impl ExactSizeIterator<Item = FileRecord<'_>> {
        self.spans.iter().map(move |&span| self.get(span))
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

/// Diagnostic ranking for digest size: selects up to five largest files.
pub(crate) fn largest_files<'a>(
    files: impl IntoIterator<Item = FileRecord<'a>>,
) -> Vec<(String, usize)> {
    let mut top: Vec<(&'a str, usize)> = Vec::with_capacity(6);

    for file in files {
        let size = file.content.len();
        if top.len() == 5 && size <= top.last().unwrap().1 {
            continue;
        }

        let index = top
            .iter()
            .position(|probe| probe.1 < size)
            .unwrap_or(top.len());
        if index < 5 {
            top.insert(index, (file.path, size));
            top.truncate(5);
        }
    }

    top.into_iter()
        .map(|(path, size)| (path.to_string(), size))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn largest_files_selects_top_five_with_stability() {
        let files = [
            FileRecord {
                path: "f1.txt",
                content: "1",
            },
            FileRecord {
                path: "f2.txt",
                content: "22",
            },
            FileRecord {
                path: "f3.txt",
                content: "333",
            },
            FileRecord {
                path: "f4.txt",
                content: "4444",
            },
            FileRecord {
                path: "f5_a.txt",
                content: "55555",
            },
            FileRecord {
                path: "f5_b.txt",
                content: "55555",
            },
            FileRecord {
                path: "f6.txt",
                content: "666666",
            },
        ];
        let result = largest_files(files);
        assert_eq!(result.len(), 5);
        assert_eq!(result[0], ("f6.txt".into(), 6));
        assert_eq!(result[1], ("f5_a.txt".into(), 5));
        assert_eq!(result[2], ("f5_b.txt".into(), 5));
        assert_eq!(result[3], ("f4.txt".into(), 4));
        assert_eq!(result[4], ("f3.txt".into(), 3));
    }

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
