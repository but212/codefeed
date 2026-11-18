//! Candidate observation, budget accounting, and storage accumulation.

use std::collections::HashSet;
use std::hash::{BuildHasher, RandomState};

use memchr::memchr;

use crate::core::error::DomainError;
use crate::core::model::CollectionResult;
use crate::core::path::Candidate;
use crate::core::policy::{
    MAX_FILE_COUNT, MAX_FILE_SIZE, MAX_PATH_BYTES, OVERFLOW_LARGEST_DIRECTORIES,
};
use crate::core::storage::{FileStorage, SortedFiles};

/// UTF-8 text predicate: NUL-free bytes within size limit, returning string slice.
fn classify_content(bytes: &[u8]) -> Option<&str> {
    if bytes.len() as u64 > MAX_FILE_SIZE || memchr(0, bytes).is_some() {
        return None;
    }
    std::str::from_utf8(bytes).ok()
}

/// What a shell reader learned about the content of one storable path.
#[derive(Debug)]
pub(crate) enum Body<B> {
    /// Unavailable at metadata, open, or read.
    Unavailable,
    /// Declared size over `MAX_FILE_SIZE`; not read.
    OverLimit,
    /// Bounded read of at most `MAX_FILE_SIZE + 1` bytes.
    Bytes(B),
}

const EMPTY_BUCKET: u32 = u32::MAX;

fn arena_slice(arena: &str, offset: u32, len: u32) -> &str {
    let start = offset as usize;
    &arena[start..start + len as usize]
}

#[derive(Debug)]
struct CountEntry {
    offset: u32,
    len: u32,
    hash: u64,
    count: usize,
}

impl CountEntry {
    fn key<'a>(&self, arena: &'a str) -> &'a str {
        arena_slice(arena, self.offset, self.len)
    }
}

/// Open-addressed `u32` bucket index shared by string-keyed arena tables.
#[derive(Debug)]
struct BucketTable {
    buckets: Vec<u32>,
    hasher: RandomState,
}

impl BucketTable {
    fn with_capacity(entries_cap: usize) -> Self {
        let bucket_cap = (entries_cap.saturating_mul(4) / 3)
            .max(16)
            .next_power_of_two();
        Self {
            buckets: vec![EMPTY_BUCKET; bucket_cap],
            hasher: RandomState::new(),
        }
    }

    fn hash_key(&self, key: &str) -> u64 {
        self.hasher.hash_one(key)
    }

    fn probe_empty(buckets: &[u32], hash: u64) -> usize {
        let mask = buckets.len() - 1;
        let mut idx = usize::try_from(hash & (mask as u64)).expect("masked hash fits in usize");
        while buckets[idx] != EMPTY_BUCKET {
            idx = (idx + 1) & mask;
        }
        idx
    }

    /// Returns `Ok(entry_idx)` when `key` is present, or `Err(bucket_idx)` pointing to the first
    /// empty probe slot.
    fn find_bucket<'a>(
        &self,
        key: &str,
        hash: u64,
        mut resolve: impl FnMut(u32) -> (u64, &'a str),
    ) -> Result<usize, usize> {
        let mask = self.buckets.len() - 1;
        let mut idx = usize::try_from(hash & (mask as u64)).expect("masked hash fits in usize");
        loop {
            let slot = self.buckets[idx];
            if slot == EMPTY_BUCKET {
                return Err(idx);
            }
            let (entry_hash, entry_key) = resolve(slot);
            if entry_hash == hash && entry_key == key {
                return Ok(slot as usize);
            }
            idx = (idx + 1) & mask;
        }
    }

    fn insert_slot(
        mut self,
        entry_idx: usize,
        hash: u64,
        bucket_idx: usize,
        existing_hashes: impl Iterator<Item = u64>,
    ) -> Self {
        let bucket_idx = if (entry_idx + 1) * 4 > self.buckets.len() * 3 {
            let new_cap = self.buckets.len() * 2;
            let mut new_buckets = vec![EMPTY_BUCKET; new_cap];
            for (idx, existing_hash) in existing_hashes.enumerate() {
                let slot = Self::probe_empty(&new_buckets, existing_hash);
                new_buckets[slot] =
                    u32::try_from(idx).expect("invariant: entries bounded below u32::MAX");
            }
            self.buckets = new_buckets;
            Self::probe_empty(&self.buckets, hash)
        } else {
            bucket_idx
        };
        self.buckets[bucket_idx] = u32::try_from(entry_idx)
            .expect("invariant: entries bounded by MAX_FILE_COUNT < u32::MAX");
        self
    }
}

/// Candidate-count budget and per-directory tallies owned by the selector that consumes it.
/// Directory keys are stored in a shared string arena to avoid per-key allocations.
#[derive(Debug)]
struct CandidateBudget {
    count: usize,
    arena: String,
    entries: Vec<CountEntry>,
    table: BucketTable,
}

#[cfg(test)]
impl Default for CandidateBudget {
    fn default() -> Self {
        Self::with_capacity(16, 256)
    }
}

impl CandidateBudget {
    fn with_capacity(entries_cap: usize, arena_bytes: usize) -> Self {
        Self {
            count: 0,
            arena: String::with_capacity(arena_bytes),
            entries: Vec::with_capacity(entries_cap),
            table: BucketTable::with_capacity(entries_cap),
        }
    }

    /// Returns `Ok(entry_idx)` when `key` is present, or `Err(bucket_idx)` pointing to the first
    /// empty probe slot.
    fn find_bucket(&self, key: &str, hash: u64) -> Result<usize, usize> {
        self.table.find_bucket(key, hash, |slot| {
            let entry = &self.entries[slot as usize];
            (entry.hash, entry.key(&self.arena))
        })
    }

    fn insert_at_new_key(mut self, key: &str, hash: u64, bucket_idx: usize) -> Self {
        let entry_idx = self.entries.len();
        self.table = self.table.insert_slot(
            entry_idx,
            hash,
            bucket_idx,
            self.entries.iter().map(|e| e.hash),
        );

        let offset = u32::try_from(self.arena.len())
            .expect("invariant: MAX_FILE_COUNT * MAX_PATH_BYTES <= u32::MAX");
        let len = u32::try_from(key.len()).expect("invariant: key length <= MAX_PATH_BYTES");
        self.arena.push_str(key);

        self.entries.push(CountEntry {
            offset,
            len,
            hash,
            count: 1,
        });
        self
    }

    /// Increments the count for `key`, inserting `1` if `key` was absent.
    fn increment(mut self, key: &str) -> Self {
        let hash = self.table.hash_key(key);
        match self.find_bucket(key, hash) {
            Ok(entry_idx) => {
                self.entries[entry_idx].count += 1;
                self
            }
            Err(bucket_idx) => self.insert_at_new_key(key, hash, bucket_idx),
        }
    }

    /// Adds a single observed candidate path to the budget accounting.
    fn add(mut self, path: &str) -> Result<Self, DomainError> {
        self.count += 1;
        let dir = extract_dir_slice(path);
        self = self.increment(dir);

        if self.count > MAX_FILE_COUNT {
            return Err(self.overflow_error());
        }
        Ok(self)
    }

    fn overflow_error(&self) -> DomainError {
        let mut dirs: Vec<(&str, usize)> = self
            .entries
            .iter()
            .map(|entry| (entry.key(&self.arena), entry.count))
            .collect();
        dirs.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        dirs.truncate(OVERFLOW_LARGEST_DIRECTORIES);
        DomainError::candidate_limit_exceeded(MAX_FILE_COUNT, dirs)
    }
}

/// Budget key for `path`: its first segment with a trailing `/`, or `./` for root-level entries.
/// No classified first segment starts with `.` (I-02), so `./` never names a real directory.
/// The key is capped at `MAX_PATH_BYTES` on a char boundary (I-05), so overlong segments cannot
/// grow the key store beyond the candidate count times `MAX_PATH_BYTES`.
fn extract_dir_slice(path: &str) -> &str {
    let dir = memchr(b'/', path.as_bytes()).map_or("./", |index| &path[..=index]);
    let mut end = dir.len().min(MAX_PATH_BYTES);
    while !dir.is_char_boundary(end) {
        end -= 1;
    }
    &dir[..end]
}

#[derive(Debug, Clone, Copy)]
enum PathSlot {
    Stored(u32),
    Skipped { offset: u32, len: u32 },
}

#[derive(Debug)]
struct PathIndexEntry {
    hash: u64,
    slot: PathSlot,
}

/// Uniqueness index for storable paths without duplicating `FileStorage::path_arena`.
/// Stored entries resolve directly against `FileStorage`; only skipped entries allocate in `skipped_arena`.
#[derive(Debug)]
struct PathIndex {
    skipped_arena: String,
    entries: Vec<PathIndexEntry>,
    table: BucketTable,
}

impl PathIndex {
    fn with_capacity(entries_cap: usize) -> Self {
        Self {
            skipped_arena: String::new(),
            entries: Vec::with_capacity(entries_cap),
            table: BucketTable::with_capacity(entries_cap),
        }
    }

    fn resolve<'a>(&'a self, slot: PathSlot, storage: &'a FileStorage) -> &'a str {
        match slot {
            PathSlot::Stored(index) => storage.path_at(index as usize),
            PathSlot::Skipped { offset, len } => arena_slice(&self.skipped_arena, offset, len),
        }
    }

    fn find_bucket(&self, key: &str, hash: u64, storage: &FileStorage) -> Result<usize, usize> {
        self.table.find_bucket(key, hash, |slot| {
            let entry = &self.entries[slot as usize];
            (entry.hash, self.resolve(entry.slot, storage))
        })
    }

    fn insert_at(mut self, hash: u64, bucket_idx: usize, slot: PathSlot) -> Self {
        let entry_idx = self.entries.len();
        self.table = self.table.insert_slot(
            entry_idx,
            hash,
            bucket_idx,
            self.entries.iter().map(|e| e.hash),
        );
        self.entries.push(PathIndexEntry { hash, slot });
        self
    }

    fn record_stored(self, hash: u64, bucket_idx: usize, span_index: usize) -> Self {
        let index = u32::try_from(span_index)
            .expect("invariant: entries bounded by MAX_FILE_COUNT < u32::MAX");
        self.insert_at(hash, bucket_idx, PathSlot::Stored(index))
    }

    fn record_skipped(mut self, key: &str, hash: u64, bucket_idx: usize) -> Self {
        let offset = u32::try_from(self.skipped_arena.len())
            .expect("invariant: MAX_FILE_COUNT * MAX_PATH_BYTES <= u32::MAX");
        let len = u32::try_from(key.len()).expect("invariant: key length <= MAX_PATH_BYTES");
        self.skipped_arena.push_str(key);
        self.insert_at(hash, bucket_idx, PathSlot::Skipped { offset, len })
    }
}

/// Pure per-item selection and budget accounting. Owns no I/O; the shell feeds
/// candidates and reads the outcome through [`Selector::finish`].
#[derive(Debug)]
#[must_use]
pub(crate) struct Selector {
    storage: FileStorage,
    candidates: CandidateBudget,
    paths: PathIndex,
    unstorable_paths: HashSet<Vec<u8>>,
}

impl Selector {
    /// Creates a `Selector` with initial arena preallocations.
    pub(crate) fn new() -> Self {
        Self {
            storage: FileStorage::new(),
            candidates: CandidateBudget::with_capacity(32, 512),
            paths: PathIndex::with_capacity(512),
            unstorable_paths: HashSet::new(),
        }
    }

    /// Observes one candidate entry and settles it.
    ///
    /// - [`Candidate::Unstorable`] consumes candidate budget, must have a unique byte identity,
    ///   and is skipped.
    /// - [`Candidate::Valid`] consumes candidate budget and must not repeat an earlier file path.
    ///   [`Body::Bytes`] is stored when it is UTF-8
    ///   text within the size limit, and skipped otherwise. [`Body::Unavailable`] and
    ///   [`Body::OverLimit`] are skipped.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::CandidateLimitExceeded`], [`DomainError::InvalidMetadata`] for a duplicate path, or
    /// [`DomainError::OutputTooLarge`].
    pub(crate) fn observe<B: AsRef<[u8]>>(
        mut self,
        candidate: Candidate<Body<B>>,
    ) -> Result<Self, DomainError> {
        let (path, body) = match candidate {
            Candidate::Unstorable(path) => {
                self.candidates = self.candidates.add(path.prefix())?;
                if !self.unstorable_paths.insert(path.into_identity()) {
                    return Err(DomainError::invalid_metadata(
                        "duplicate entry path (unstorable)",
                    ));
                }
                return Ok(self);
            }
            Candidate::Valid(path, body) => (path, body),
        };
        self.candidates = self.candidates.add(&path)?;
        let hash = self.paths.table.hash_key(&path);
        let Err(bucket_idx) = self.paths.find_bucket(&path, hash, &self.storage) else {
            return Err(DomainError::invalid_metadata(format!(
                "duplicate entry path: {}",
                &*path
            )));
        };

        if let Body::Bytes(bytes) = body
            && let Some(content) = classify_content(bytes.as_ref())
        {
            let span_index = self.storage.len();
            // Raw arena bytes are a strict lower bound for the rendered digest size, so
            // `push` rejects over-limit content before the final rendering pass.
            self.storage = self.storage.push(&path, content)?;
            self.paths = self.paths.record_stored(hash, bucket_idx, span_index);
        } else {
            self.paths = self.paths.record_skipped(&path, hash, bucket_idx);
        }
        Ok(self)
    }

    /// Finalizes selection, sorting files lexicographically in-place, and returns a [`CollectionResult`].
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidMetadata`] when a stored path is a directory prefix of another stored path.
    pub(crate) fn finish(
        self,
        label: String,
        revision: Option<String>,
    ) -> Result<CollectionResult, DomainError> {
        let skipped = self.candidates.count - self.storage.len();
        Ok(CollectionResult {
            label,
            revision,
            files: SortedFiles::new(self.storage)?,
            skipped,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::policy::{
        MAX_FILE_SIZE_USIZE, MAX_OUTPUT_SIZE, MAX_OUTPUT_SIZE_USIZE, MAX_PATH_BYTES,
    };

    #[test]
    fn candidate_overflow_includes_largest_directories_diagnostic()
    -> Result<(), Box<dyn std::error::Error>> {
        let vendor = MAX_FILE_COUNT * 4 / 5;
        let src = MAX_FILE_COUNT - vendor;
        let mut budget = CandidateBudget::default();
        for index in 0..vendor {
            budget = budget.add(&format!("vendor/pkg_{index}.rs"))?;
        }
        for index in 0..src {
            budget = budget.add(&format!("src/file_{index}.rs"))?;
        }
        let err = budget.add("extra/one.rs").unwrap_err();
        let message = err.to_string();
        assert!(message.contains(&format!(
            "repository contains more than {MAX_FILE_COUNT} files"
        )));
        assert!(message.contains("largest directories:"));
        assert!(message.contains(&format!("vendor/ ({vendor} files)")));
        assert!(message.contains(&format!("src/ ({src} files)")));
        Ok(())
    }

    #[test]
    fn candidate_budget_handles_many_distinct_directories_deterministically() {
        let mut budget = CandidateBudget::default();
        for i in 0..MAX_FILE_COUNT {
            let path = format!("dir_{i:05}/file.rs");
            budget = budget.add(&path).unwrap();
        }
        let err = budget.add("dir_00000/extra.rs").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains(&format!(
            "repository contains more than {MAX_FILE_COUNT} files"
        )));
        assert!(msg.contains("largest directories:"));
        assert!(msg.contains("- dir_00000/ (2 files)"));
    }

    #[test]
    fn extract_dir_slice_takes_the_first_segment_capped_at_a_char_boundary() {
        // `a` followed by two-byte `é` puts every character start at an odd offset, so byte
        // `MAX_PATH_BYTES` lands inside a character and the cut moves back one byte.
        let accented = format!("a{}", "é".repeat(MAX_PATH_BYTES));
        let accented_cut = format!("a{}", "é".repeat((MAX_PATH_BYTES - 2) / 2));
        let overlong = "a".repeat(MAX_PATH_BYTES + 10);

        let cases = [
            ("src/core/decide.rs".to_string(), "src/".to_string()),
            ("root_file.txt".to_string(), "./".to_string()),
            (String::new(), "./".to_string()),
            ("vendor/".to_string(), "vendor/".to_string()),
            ("<root>/x.rs".to_string(), "<root>/".to_string()),
            (format!("{overlong}/file.rs"), "a".repeat(MAX_PATH_BYTES)),
            (format!("{accented}/file.rs"), accented_cut),
        ];
        for (path, expected) in cases {
            assert_eq!(extract_dir_slice(&path), expected, "{path:.40}");
        }
        assert_eq!(
            extract_dir_slice(&format!("{}/file.rs", "é".repeat(MAX_PATH_BYTES))).len(),
            MAX_PATH_BYTES
        );
    }

    #[test]
    fn candidate_budget_merges_overlong_directories_sharing_capped_prefix()
    -> Result<(), DomainError> {
        let head = "a".repeat(MAX_PATH_BYTES + 5);
        let mut budget = CandidateBudget::default();
        for index in 0..3 {
            budget = budget.add(&format!("{head}{index}/file.rs"))?;
        }
        assert_eq!(budget.count, 3);
        assert_eq!(budget.entries.len(), 1);
        assert_eq!(budget.entries[0].count, 3);
        assert!(
            budget
                .entries
                .iter()
                .all(|entry| entry.key(&budget.arena).len() <= MAX_PATH_BYTES)
        );
        Ok(())
    }

    #[test]
    fn candidate_overflow_separates_root_files_from_directory_named_root()
    -> Result<(), Box<dyn std::error::Error>> {
        let named_root = MAX_FILE_COUNT * 3 / 5;
        let root_files = MAX_FILE_COUNT - named_root;
        let mut budget = CandidateBudget::default();
        for index in 0..named_root {
            budget = budget.add(&format!("<root>/pkg_{index}.rs"))?;
        }
        for index in 0..root_files {
            budget = budget.add(&format!("f_{index}.rs"))?;
        }
        let message = budget.add("extra.rs").unwrap_err().to_string();
        assert!(
            message.contains(&format!("\n  - <root>/ ({named_root} files)")),
            "{message}"
        );
        // The `extra.rs` entry that overflows the budget is itself a root file.
        assert!(
            message.contains(&format!("\n  - ./ ({} files)", root_files + 1)),
            "{message}"
        );
        Ok(())
    }

    #[test]
    fn classify_content_detects_binary_nul_anywhere() {
        let mut bin = vec![b'a'; 1024];
        bin[12] = 0;
        assert_eq!(classify_content(&bin), None);

        let mut bin_tail = vec![b'b'; 5000];
        bin_tail[4500] = 0;
        assert_eq!(classify_content(&bin_tail), None);

        let mut bytes = vec![b'a'; 4095];
        bytes.extend_from_slice("€tail\n".as_bytes());
        let res = classify_content(&bytes);
        assert!(matches!(res, Some(content) if content.ends_with("€tail\n")));

        let mut bytes_with_nul = vec![b'a'; 4095];
        bytes_with_nul.extend_from_slice("€\0tail\n".as_bytes());
        assert_eq!(classify_content(&bytes_with_nul), None);
    }

    fn file<'a>(path: &str, body: Body<&'a [u8]>) -> Candidate<Body<&'a [u8]>> {
        Candidate::Valid(path.into(), body)
    }

    fn text(path: &str) -> Candidate<Body<&'static [u8]>> {
        file(path, Body::Bytes(b"content"))
    }

    #[test]
    fn output_overflow_includes_largest_files_diagnostic() -> Result<(), Box<dyn std::error::Error>>
    {
        let chunk = vec![b'x'; MAX_FILE_SIZE_USIZE];
        let mut selector = Selector::new();
        // Fill the output to one file short of its limit; the next file overflows it.
        for i in 0..MAX_OUTPUT_SIZE_USIZE / MAX_FILE_SIZE_USIZE - 1 {
            selector = selector.observe(file(
                &format!("file_{i:02}.txt"),
                Body::Bytes(chunk.as_slice()),
            ))?;
        }
        let err = selector
            .observe(file("overflow.txt", Body::Bytes(chunk.as_slice())))
            .unwrap_err();
        let message = err.to_string();
        assert!(message.contains(&format!(
            "digest exceeds the {} MiB output limit",
            MAX_OUTPUT_SIZE / (1024 * 1024)
        )));
        assert!(message.contains("largest files:"));
        assert!(message.contains("file_00.txt"));
        Ok(())
    }

    #[test]
    fn observe_enforces_candidate_budget() -> Result<(), DomainError> {
        let mut selector = Selector::new();
        for i in 0..MAX_FILE_COUNT {
            selector = selector.observe(text(&format!("file_{i}.txt")))?;
        }

        let err = selector.observe(text("overflow.txt")).unwrap_err();
        assert!(matches!(err, DomainError::CandidateLimitExceeded(..)));
        Ok(())
    }

    #[test]
    fn unstorable_candidate_consumes_budget_and_fails_when_exhausted() -> Result<(), DomainError> {
        let mut selector = Selector::new();
        for i in 0..MAX_FILE_COUNT {
            selector = selector.observe(text(&format!("file_{i}.txt")))?;
        }

        let err = selector
            .observe(Candidate::<Body<&[u8]>>::Unstorable(
                ("dir/", b"dir/\xff".as_slice()).into(),
            ))
            .unwrap_err();
        assert!(matches!(err, DomainError::CandidateLimitExceeded(..)));
        Ok(())
    }

    #[test]
    fn observe_counts_candidates_by_content_outcome() -> Result<(), Box<dyn std::error::Error>> {
        // (path, body, included, skipped)
        let cases: [(&str, Body<Vec<u8>>, usize, usize); 8] = [
            ("a.txt", Body::Bytes(b"hello".to_vec()), 1, 0),
            ("bin.bin", Body::Bytes(vec![0, 1]), 0, 1),
            ("bad_utf8", Body::Bytes(vec![0xff]), 0, 1),
            (
                "exact.txt",
                Body::Bytes(vec![b'x'; MAX_FILE_SIZE_USIZE]),
                1,
                0,
            ),
            (
                "over.txt",
                Body::Bytes(vec![b'x'; MAX_FILE_SIZE_USIZE + 1]),
                0,
                1,
            ),
            ("declared_over.txt", Body::OverLimit, 0, 1),
            ("gone.txt", Body::Unavailable, 0, 1),
            ("empty.txt", Body::Bytes(Vec::new()), 1, 0),
        ];
        for (path, body, included, skipped) in cases {
            let selector = Selector::new().observe(Candidate::Valid(path.into(), body))?;
            let collection = selector.finish("test".into(), None)?;
            assert_eq!(collection.files.len(), included, "{path}");
            assert_eq!(collection.skipped, skipped, "{path}");
        }
        Ok(())
    }

    #[test]
    fn finish_counts_skipped_across_mixed_observations() -> Result<(), DomainError> {
        let empty = Selector::new().finish("test".into(), None)?;
        assert_eq!(empty.files.len(), 0);
        assert_eq!(empty.skipped, 0);

        let observations = [
            text("a.txt"),
            file("bin.bin", Body::Bytes(b"\0")),
            file("bad_utf8", Body::Bytes(b"\xff")),
            file("over.txt", Body::OverLimit),
            file("gone.txt", Body::Unavailable),
            Candidate::Unstorable(("dir/", b"dir/\xff".as_slice()).into()),
            file("empty.txt", Body::Bytes(b"")),
        ];
        let selector = observations
            .into_iter()
            .try_fold(Selector::new(), Selector::observe)?;
        let collection = selector.finish("test".into(), None)?;
        assert_eq!(collection.files.len(), 2);
        assert_eq!(collection.skipped, 5);
        Ok(())
    }

    #[test]
    fn unstorable_observation_is_counted_and_skipped() -> Result<(), DomainError> {
        for prefix in ["", "dir/sub/"] {
            let mut identity = prefix.as_bytes().to_vec();
            identity.push(0xff);
            let selector = Selector::new().observe(Candidate::<Body<&[u8]>>::Unstorable(
                (prefix, identity.as_slice()).into(),
            ))?;
            assert_eq!(selector.candidates.count, 1);
            let collection = selector.finish("test".into(), None)?;
            assert_eq!(collection.files.len(), 0);
            assert_eq!(collection.skipped, 1);
        }
        Ok(())
    }

    #[test]
    fn observe_rejects_duplicate_paths_including_skipped() {
        let stored = || Body::Bytes(&b"x"[..]);
        // A skipped candidate still names its path, so it collides with a stored one in either order.
        let cases = [
            (stored(), stored()),
            (stored(), Body::OverLimit),
            (Body::OverLimit, stored()),
            (Body::Unavailable, Body::OverLimit),
        ];
        for (first, second) in cases {
            let err = Selector::new()
                .observe(file("a.txt", first))
                .and_then(|selector| selector.observe(file("a.txt", second)))
                .unwrap_err();
            assert!(
                matches!(&err, DomainError::InvalidMetadata(msg) if msg == "duplicate entry path: a.txt")
            );
        }
    }

    #[test]
    fn path_index_stores_only_skipped_paths_in_skipped_arena() -> Result<(), DomainError> {
        let selector = Selector::new()
            .observe(text("src/main.rs"))?
            .observe(file("src/skipped.bin", Body::OverLimit))?;
        assert_eq!(selector.paths.entries.len(), 2);
        assert_eq!(selector.paths.skipped_arena, "src/skipped.bin");
        Ok(())
    }

    #[test]
    fn candidate_budget_rehashes_and_preserves_all_entries() {
        let mut budget = CandidateBudget::with_capacity(2, 16);
        let initial_buckets = budget.table.buckets.len();
        for i in 0..64 {
            let key = format!("dir_{i}/");
            budget = budget.increment(&key).increment(&key);
        }
        assert!(budget.table.buckets.len() > initial_buckets);
        assert_eq!(budget.entries.len(), 64);
        for i in 0..64 {
            let key = format!("dir_{i}/");
            let hash = budget.table.hash_key(&key);
            let idx = budget.find_bucket(&key, hash).expect("key must exist");
            assert_eq!(budget.entries[idx].count, 2);
        }
    }

    #[test]
    fn candidate_budget_resolves_hash_collisions_by_key_equality() {
        let mut budget = CandidateBudget::with_capacity(4, 32);
        let forced_hash = 0xDEAD_BEEF;
        let Err(b1) = budget.find_bucket("alpha", forced_hash) else {
            panic!("expected empty bucket");
        };
        budget = budget.insert_at_new_key("alpha", forced_hash, b1);

        let Err(b2) = budget.find_bucket("beta", forced_hash) else {
            panic!("expected empty bucket on collision");
        };
        budget = budget.insert_at_new_key("beta", forced_hash, b2);

        let idx_a = budget.find_bucket("alpha", forced_hash).unwrap();
        let idx_b = budget.find_bucket("beta", forced_hash).unwrap();
        assert_eq!(budget.entries[idx_a].key(&budget.arena), "alpha");
        assert_eq!(budget.entries[idx_b].key(&budget.arena), "beta");
        assert_ne!(idx_a, idx_b);
    }
}
