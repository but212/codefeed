# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `unescape_header` in `src/core/digest/text.rs`: restores escaped digest header strings back to their original form for lossless roundtrip and parsing validation.
- `CompactDir` in `src/core/decide.rs`: 24-byte inline directory descriptor holding up to 22 bytes inline with heap fallback for longer paths, eliminating heap allocations during candidate budget tracking.
- `local_walk_hierarchical_1000` benchmark suite in `benches/bench.rs` to measure directory candidate walking, path normalization, sorting, and initial budget checking isolated from disk content reads.

### Changed

- Naming convention improvements per `docs/coding_convention.md`:
  - Eliminated redundant verbosity in module functions: `core::cli::parse_cli` -> `cli::parse` (re-exported as `codefeed::parse`), `core::source::parse_source`/`parse_source_os` -> `source::parse`/`source::parse_os`, `shell::local::collect_local` -> `local::collect`, and `shell::github::collect_github` -> `github::collect`.
  - Unified GitHub-related types to RFC 430 single-word capitalization: `Source::GitHub` -> `Source::Github`, `GitHubStream` -> `GithubStream`.
  - Replaced extension abbreviations with full descriptive names in `src/core/policy.rs`: `SENSITIVE_EXT` -> `SENSITIVE_EXTENSIONS`, `BINARY_EXT` -> `BINARY_EXTENSIONS`, and `is_binary_ext` -> `is_binary_extension`.
- `TokenEstimator` in `src/core/digest/token.rs`: replaced runtime bit-branching with a precomputed 256-byte lookup table (`CHAR_METRIC`) to accelerate per-byte token estimation, and changed `finish(&self)` to be non-destructive so token counts can be read repeatedly without mutating state.
- `normalize_path` in `src/shell/output.rs`: fixed path truncation bugs where leading `..` in relative paths (such as `../a` or `a/../../b`) were dropped, ensuring correct target file resolution and output filter matching.
- `PathClassification::classify` in `src/core/policy.rs`: unified multiple full-path scans into a single filename/extension extraction pass, eliminating redundant string traversals before candidate evaluation.
- `retry_delay` in `src/core/policy.rs`: bounded backoff delay strictly within 5,000 ms while preserving random jitter on high attempt counts to prevent thundering-herd retry collisions.
- `BudgetTrie::get_or_insert_child` in `src/core/digest/trie.rs`: unified duplicate node allocation branches into a single `push_node` helper, preventing arena index synchronization bugs.
- `strip_archive_root` in `src/core/archive.rs`: stripped consecutive slashes after root tarball prefixes to prevent leading slashes in relative paths.
- `first_special_index` in `src/core/digest/xml.rs`: extracted shared special character search for `xml_escape` and `write_escaped` to eliminate duplicate scanning logic.
- `CandidateBudget::overflow_error` in `src/core/decide.rs`: aggregated directory counts with `HashMap<&str, usize>` borrowing directly from `CompactDir`, eliminating heap allocations during candidate overflow reporting.
- `FileSpan` in `src/core/model.rs`: added `path` and `content` projection methods, simplifying `FileStorage::sort_by_path` and `FileStorage::get` into clean, single-expression arena lookups.
- `probe_tar_entry` in `src/shell/github.rs`: converted byte-level hidden path inspection into a zero-allocation single-pass scan via `starts_with(b".")` and `windows(2).any(|w| w == b"/.")`.
- `CandidateBudget::dir_runs` in `src/core/decide.rs`: migrated from `Vec<(String, usize)>` to `Vec<(CompactDir, usize)>`, aligning entries to 32 bytes for cache-line density (2 entries per 64B cache line).
- `LocalCandidate` in `src/shell/local.rs`: removed redundant `path: PathBuf` field, reducing struct size from 56 bytes to 32 bytes (2 entries per 64B cache line) and eliminating per-file `PathBuf` heap allocations during directory walking.
- `LocalStream` in `src/shell/local.rs`: lazily resolves local file paths via a single reusable `work_path: PathBuf` buffer during iteration.

### Removed

- `is_noise_path`, `is_sensitive_path`, and `has_binary_extension` in `src/core/policy.rs`: removed unused legacy path wrapper functions in favor of `PathClassification::classify`.

## [0.2.3]

### Added

- Re-export `DigestFormat`, `IngestReport`, `Source`, `IngestError` from `src/lib.rs`.
- `SourceStream` trait and generic `collect_stream` driver in `src/shell/orchestrate.rs`, unifying selection, budget, and completion for all sources.
- Lazy `LocalStream` iterator in `src/shell/local.rs` with fast-fail preflight limit check on creation.
- Streaming `GitHubStream` in `src/shell/github.rs` yielding `Item`s and resolving revisions.
- `FileStorage` in `src/core/model.rs`: contiguous path and content arenas with 16-byte `FileSpan` descriptors. No per-file heap allocation; keeps cache locality.
- `FileStorage::sort_by_path(&mut self)` sorts `FileSpan`s in place.
- Zero-copy `FileRecord<'a>` view into `FileStorage`.
- `TokenEstimator` state machine in `src/core/digest/token.rs`, shared by `estimate_tokens` and `BoundedEstimatingWriter`.
- Isolated byte budget tracking and top-K file diagnostics in `src/core/digest/budget.rs`.
- Pure CLI parser in `src/core/cli.rs`: argument and flag evaluation with no side effects or I/O.
- `Source::with_reference` in `src/core/model.rs`: validates revisions and GitHub/local source invariants.
- `CandidateBudget::check_limit` in `src/core/decide.rs`: batch pre-flight candidate check with O(1) fast path.
- `current_epoch_nanos()` in `src/shell/mod.rs`: shared epoch timestamp source.
- `Selector::finish` in `src/core/decide.rs` builds `CollectionResult` directly.

### Fixed

- `BoundedEstimatingWriter::finish` (`src/core/digest/mod.rs`) no longer double-flushes atomic file targets. Keeps `estimated_tokens` when a downstream `BrokenPipe` occurs.
- `OutputFilter::new` (`src/shell/output.rs`) tries `fs::canonicalize(output)` first, lexical traversal second. `AtomicFile` normalizes parent paths for temp creation and rename. Fixes `ENOENT` from uncollapsed relative parents.
- `probe_tar_entry` (`src/shell/github.rs`) strips the tar root and filters hidden entries on raw bytes before slot allocation. Validates with `text_str` and rejects embedded NUL. `xml_escape` and `write_xml_escaped` (`src/core/digest/xml.rs`) escape `0x00` as `\u{FFFD}`.
- `OutputFilter::is_excluded` short-circuits canonical path checks. On Unix, skips redundant `names_match` and `fs::canonicalize` fallback syscalls.
- `collect_local` uses `WalkBuilder::sort_by_file_name` for deterministic traversal. `CandidateLimitExceeded` output is now reproducible across OSes and filesystems.
- `OutputFilter::new` lexically normalizes nonexistent output targets. Destination paths with uncollapsed relative components are excluded consistently.
- `collect_local` checks `entry.file_type().is_file()` before output exclusion. Uses `fs::symlink_metadata` on Unix to avoid following symlinks.
- `AtomicFile::create` rejects directory targets immediately with `io::ErrorKind::IsADirectory`, before temp file creation.
- `is_hidden_path` (`src/core/policy.rs`) ignores empty components (`//`). Visible paths with duplicate delimiters are no longer classified as hidden.
- Output paths without a file name map to `IngestError::Usage` (`src/shell/output.rs`).
- `src/shell/local.rs` enforces candidate count limits before reading file contents, via `CandidateBudget::check_limit`. Prevents unbounded I/O past `MAX_FILE_COUNT`.
- `is_sensitive_path` (`src/core/policy.rs`): `.env*` prefix check is case-insensitive, matching the docs.
- User agent built from `concat!("codefeed/", env!("CARGO_PKG_VERSION"))` instead of hardcoded `"codefeed/0.1"` (`src/shell/github_client.rs`).

### Changed

- `src/core/cli.rs` migrated to `clap` (derive). Drops manual token iteration and ad-hoc error reporting. Keeps POSIX delimiters and help format.
- `collect_local` (`src/shell/local.rs`) and `collect_github` (`src/shell/github.rs`) own stream construction and archive decompression. `orchestrate::ingest` is a plain collect-then-emit pipeline.
- `CandidateBudget::check_limit` is the only count-limit check. Removed the redundant `Selector` pass-through wrapper.
- `src/shell/local_io.rs`: platform TOCTOU checks (`openat2`, Win32 reparse flags) replaced with `fs::symlink_metadata` and `fs::File::open`.
- `OutputFilter` (`src/shell/output.rs`): dev/inode comparison and multi-layer fallback replaced with zero-syscall lexical matching and parent-canonical normalization.
- `XmlChunks` caches pending escape sequences. Preflight special-byte detection streamlined (`src/core/digest/xml.rs`).

### Removed

- Unused `root: PathBuf` field in `LocalStream` (`src/shell/local.rs`).
- Obsolete lifetime `'a` on `AtomicFile` (`src/shell/output.rs`).
- `rustix` dependency and platform FFI hooks.
- Hot and warm paths (XML/text escaping, trie insertion, SHA and name validation, path classification) moved to branchless bitwise ops and constant lookup tables.
- `output_exclusion.rs` deleted. Its `OutputFilter`, path identity, and `normalize_path` now live in `src/shell/output.rs`, with `OutputFilter::from_target(&OutputTarget)`.
- `src/core/decide.rs`: item evaluation and accumulation split into pure `ItemDecision` (`Included`, `Skipped`), `Item::evaluate`, and `Selector::accumulate`.
- `LocalStream` size hint (`ExactSizeIterator`) passed to `Selector::from_size_hint` to pre-size arenas.
- `BudgetTrie` (`src/core/digest/trie.rs`) rebuilt as a 16-byte LCRS node arena and contiguous `label_arena: String`. Node size 48B → 16B.
- 4 KiB `ESCAPE_TABLE` (`src/core/digest/xml.rs`) replaced by inline matching over `SPECIAL_BITSET`. `xml_escape` and `escape_header` scan in one pass with `.position()`.
- `CandidateBudget::record_dir_run` avoids unbounded `String` allocations on interleaved directory inputs by probing recent slots.
- `FileStorage` capacity pre-reservation. `sort_by_path()` short-circuits on sorted input.
- `LocalCandidate` slimmed from 88B to 56B (`Option<Probe>` → `skip_read: bool`).
- `BoundedEstimatingWriter` buffers small writes in an 8 KiB chunk buffer.
- `GithubClient::new` sets `redirect_auth_headers(RedirectAuthHeaders::Never)`. Header propagation on cross-origin redirects is blocked, with regression tests.
- Windows `is_strictly_beneath` (`src/shell/local_io.rs`): `to_string_lossy()` → `OsStr::eq_ignore_ascii_case`. No allocation for wide or non-UTF-8 paths.
- `debug_assert!` bounds checks in `FileStorage::push` (`src/core/model.rs`) guard against `u32` offset truncation.
- `Selector::decide` dedupes budget accounting via `ItemResolution::path`. Extracted `CandidateBudget::record_dir_run` and `overflow_error`.
- `CandidateBudget` happy path: `HashMap` allocations → run-length encoded `dir_runs: Vec<(String, usize)>`. Aggregated only when overflow diagnostics are formatted.
- `BudgetTrie` root at `nodes[0]`. Removed `root_children: Vec<u32>` and redundant `Option<u32>` checks in `get_or_insert_child`.
- `DigestFormat::render` streams `ExactSizeIterator<Item = FileRecord<'_>>`. No 32-byte `Vec<FileRecord>` reallocation or re-sort.
- `Selector` tracks skipped bytes inline. Removes split state between `CollectionResult.files` and `CollectionResult.trie`.
- Collection (`Selector`, `CollectionResult`, `collect_local`, `collect_github`) decoupled from `DigestFormat`. Format goes only to `render_digest_to_writer`.
- XML entity and newline escaping moved from trie insertion to render time (`render_to_writer`). `BudgetTrieNode` no longer caches display strings.
- `Vec<FileRecord>` in `CollectionResult` and `Selector` → `FileStorage`. Zero-copy borrowed records feed renderers and diagnostics.
- `FileStorage` internal buffers (`spans`, `path_arena`, `content_arena`) made private.
- `BudgetTrie` (`src/core/digest/trie.rs`) → flat `Vec<BudgetTrieNode>` arena, `u32` child indices, `Box<str>` labels. No recursive pointer chasing.
- `IngestError::CandidateLimitExceeded` and `IngestError::OutputTooLarge` carry structured fields. Message formatting centralized in `error.rs` `Display`.
- `percent-encoding` in `src/core/github_url.rs` → std RFC 3986 unreserved validator and `%XX` encoder.
- `memchr` → `slice::contains(&0)` in `src/core/decide.rs`, and std pattern matching in `src/core/digest/text.rs`.
- `log` → `eprintln!` in `src/shell/github_client.rs`.
- Tarball bodies buffered through a 64 KiB `BufReader` before decompression (`src/shell/github_client.rs`).
- Tarball paths decoded from `entry.path_bytes()` via `std::str::from_utf8` (`src/shell/github.rs`).
- Duplicate HTTP transport and status retry logic in `src/shell/github_client.rs` unified into one helper.
- Dead `Selector::is_empty` and `text_content` in `src/core/decide.rs`. Benchmarks use zero-copy `text_str(&[u8])`.
- `collect_local`, `collect_github`, `collect_from_archive` → unified `collect_stream`.
- `Probe::PreclassifiedBinary` merged into `Probe::Missing`.
- `DigestBudget` and `BudgetOverflow` (`src/core/digest/budget.rs`).
- `XmlChunk::ends_with_nl()`. Trailing newline checked on source content.
- 4 KiB static character escape table in `src/core/digest/text.rs`.
- `RequestTarget` enum (`src/core/github_url.rs`). Origin checks use `is_github_api_origin`.
- Dead `output_identity` and redundant normalization helpers in `src/shell/output_exclusion.rs`.
- `percent-encoding`, `memchr`, `log` dependencies from `Cargo.toml`.
- Single-use `Options` struct in `src/core/model.rs`.
- Dead `Probe::Ignored` variant and `ItemResolution::Ignored` arm in `src/core/decide.rs`.
- `src/shell/cli.rs` (superseded by `src/core/cli.rs`).
- Duplicate `SystemTime` and `UNIX_EPOCH` calls in `output.rs`, `github_client.rs`, `output_exclusion.rs`.
- Dead `Index<usize>` impl on `FileStorage` (`src/core/model.rs`).
- Dead `escaped_metrics` helper and `XmlChunk::len` (`src/core/digest/xml.rs`).
- `Selector::into_parts` → `Selector::finish`.

## [0.2.2]

### Added

- `BoundedEstimatingWriter` wrapping output stream serialization to strictly enforce `MAX_OUTPUT_SIZE` against rendered bytes (framing, tree, metadata, content) and estimate tokens in a single pass.

### Changed

- Streamlined local walk to a 2-step linear collection pipeline (`ignore::WalkBuilder` candidate harvest followed by bounded file reads).
- Replaced channel passing and worker thread pools in `src/shell/local.rs` with sequential traversal, simplifying codebase structure.
- Simplified `DigestBudget::skip` to return `()` directly, removing dead error plumbing in `Selector::decide`.
- Converted unique file path sorting in `text::render` and `xml::render` to in-place pdqsort (`sort_unstable_by`).
- Replaced multi-byte Unicode whitespace sweeps with `trim_ascii()` for `--ref` and GitHub token parsing in `cli.rs` and `github_client.rs`.
- Optimized `CandidateBudget::add` using `get_mut` pre-lookups in `src/core/decide.rs`, eliminating redundant key allocations for known directory paths.

### Removed

- Intermediate `Collector` / `AnyCollector` trait abstraction in `src/shell/collector.rs`, routing collection directly via `orchestrate::ingest`.

## [0.2.1]

### Added

- Execution configuration model `RunConfig` in `src/core/model.rs` consolidating `source`, `output`, and `format` into a unified pipeline parameter.
- Collection abstraction `Collector` trait and stack-allocated static dispatch enum `AnyCollector` in `src/shell/collector.rs`, encapsulating `LocalCollector` and `GitHubCollector`.
- Zero-dependency benchmark runner and synthetic microbenchmark suite (`benches/bench.rs`) covering `BudgetTrie`, `render_digest_to_writer`, `TokenEstimatingWriter`, and batch `Selector::decide`.
- Library target (`src/lib.rs`) exposing core entry points, `RunConfig`, and benchmark facade (`#[doc(hidden)] pub mod __bench`).
- Scoped test fixture directory lifecycle guard (`TempDirGuard`) in `src/shell/mod.rs` with path canonicalization, eliminating manual cleanup boilerplate.

### Fixed

- Unix FFI open flags (`O_DIRECTORY`, `O_NOFOLLOW`) mapped directly through `rustix::fs::OFlags` across all Unix targets.
- Case-insensitive prefix classification for sensitive paths (`.env`, `id_rsa`, `id_ed25519`, `credentials`) in `src/core/policy.rs`.
- Differentiated HTTP transport error retry in `src/shell/github_client.rs`, restricting retries to transient errors (`Io`, `Timeout`, `ConnectionFailed`) while failing immediately on non-transient transport failures.
- Non-verbatim path fallback removal in `verify_beneath_root` on Windows, enforcing canonical root preconditions.

### Changed

- Migrated Unix file and directory opening in `src/shell/local_io.rs` to `rustix::fs`, eliminating manual architecture-specific FFI definitions and raw system calls.
- Unified ingestion pipeline entry point (`orchestrate::ingest`) accepting `&RunConfig` and dispatching via `AnyCollector`.
- Embedded `format: DigestFormat` directly into `CollectionResult`, establishing a single source of truth and removing redundant format parameters from `render_digest_to_writer`.
- Scoped GitHub API authorization headers strictly to exact-origin `https://api.github.com` requests in `src/core/github_url.rs`.
- Pruned unread `node_count` and `display_bytes` tracking fields from `BudgetTrie`.
- Refactored local directory window processing in `src/shell/local.rs` to consume candidates by value, eliminating `Default` and `Clone` requirements on `ItemPath` and `LocalCandidate`.
- Synchronized documentation in `README.md` and `context.md` to reflect deterministic largest-directory diagnostics from observed candidates upon candidate limit overflow.
- Pruned and consolidated the internal unit test suite across `src/core/` and `src/shell/` from 122 to 85 tests, eliminating redundant assertions, tautological constant checks, a 10,005-file physical disk allocation loop, and standalone mock threads while converting CLI, policy, and XML tests to table-driven fixtures.
- Restructured `Source` as an Algebraic Data Type (ADT) embedding `reference: Option<String>` into `Source::GitHub`, trimming `Options` to common settings and structurally preventing invalid reference configurations at compile time.
- Hardened atomic file output against panic unwinding and I/O failures using an RAII commit guard (`AtomicFile`) in `src/shell/output.rs`.
- Encapsulated `DigestBudget` accounting metrics into an immutable, checked `ContentMetrics` value object in `src/core/digest/mod.rs`.
- Unified XML escaping, metric calculation, and streaming serialization under a single zero-allocation `XmlChunks<'a>` iterator in `src/core/digest/xml.rs`.
- Optimized diagnostic `largest_files` selection using bounded top-5 capacity with linear position lookup, eliminating full collection clones and $O(N \log N)$ sorting.
- Generalized `DigestFormat::render`, `text::render`, and `xml::render` over generic `IntoIterator<Item = &'a FileRecord>`.
- Optimized `BudgetTrie::insert` with iterative descent, zero-allocation path parsing, and O(1) last-sibling matching.
- Optimized `BudgetTrieNode` storage and `tree_display` via `Cow<'_, str>` and sparse escaping (`Option<String>`), eliminating redundant heap allocations for unescaped paths and simplifying `BudgetTrie::insert` with unified `find_child` slot lookup.
- Accelerated XML escaping and metric calculation via byte-level ASCII lookup tables and slice chunk writes in `src/core/digest/xml.rs`.
- Replaced per-batch thread spawning in local file reading with persistent scoped workers and atomic work stealing in `src/shell/local.rs`.
- Hardened and optimized Windows root containment checks (`is_strictly_beneath`) by eliminating heap allocations and redundant path canonicalizations in `src/shell/local_io.rs`.
- Unified word-boundary flag tracking in `TokenEstimatingWriter` via `std::mem::take` in `src/core/digest/token.rs`.
- Extracted `BudgetTrie` depth upper bound multipliers (`TREE_DEPTH_BYTE_FACTOR = 6`, `TREE_NODE_BASE_OVERHEAD = 12`) as explicit domain constants in `src/core/digest/trie.rs`.
- Converted pure policy predicates (`is_retryable_status`, `retry_delay`) and format flags (`DigestFormat::escape_tree_xml`) to `const fn`.
- Centralized format frame metrics into associated constants (`FormatFrameMetrics::TEXT`, `FormatFrameMetrics::XML`) in `src/core/digest/mod.rs`, eliminating redundant intermediate wrapper functions.
- Optimized metadata digit overhead calculation (`extra_metadata_bytes`) using pure mathematical `const fn num_digits`, eliminating `to_string()` heap allocations during budget checks in `src/core/digest/text.rs`.
- Optimized `CandidateBudget::add` using `extract_dir_slice` and cached `get_mut` lookups in `src/core/decide.rs`, eliminating redundant `String` key allocations for known directory paths.

### Removed

- Manual Unix FFI bindings and architecture-specific constants module `src/shell/sys.rs`.

## [0.2.0]

### Added

- Single-request GitHub repository ingestion streaming whole-archive tarballs (`/repos/{owner}/{repo}/tarball/{ref}`) via `flate2` and `tar`.
- Extraction of exact Git commit SHA from tarball `pax_global_header` without secondary API queries.
- `ARCHIVE_MAX_SECS` policy limit (300 s) bounding whole-archive transfer time.
- Direct Unix FFI bindings in `src/shell/sys.rs` replacing `libc`.

### Changed

- Separated HTTP connection timeout (30 s) from response body streaming timeout (300 s).

### Removed

- Unused `serde` and `serde_json` dependencies.
- External `libc` crate dependency on Unix targets.
- GitHub Git Trees API recursive tree traversal and concurrent blob fetch pipeline.
- `MAX_JSON_SIZE` policy limit.

## [0.1.2]

### Fixed

- Fixed `DigestBudget` tree upper bound calculation to account for depth-dependent indentation overhead (`6 * depth + 12 + display.len()`), preventing 20 MiB budget breaches in deeply nested trees.
- Hardened Windows file reading against junction/symlink root escapes via `GetFinalPathNameByHandleW` boundary checks.
- Fixed `CandidateBudget` directory counting bottleneck by replacing linear `Vec` scans with zero-allocation `HashMap` lookups, preserving deterministic diagnostic sorting.

### Changed

- Clarified plain text raw preservation contract for LLM prompt fidelity, distinguishing it from XML 1.0 control character sanitization.
- Documented sensitive file filtering as conventional path/extension matching rather than content-based secret scanning.

## [0.1.1]

### Added

- Order-independent positional source argument resolution.
- Short option aliases (`-o`, `-f`, `-r`, `-h`) and key-value parameter syntax (`--key=val`, `-k=val`).
- POSIX `--` option delimiter to treat subsequent arguments as positional sources.

### Changed

- Simplified CLI argument parser to linear traversal while retaining syntax contracts.
- Refactored `DigestBudget` and `BudgetTrie` to be error-terminal, removing transactional rollback tracking.
- Streamlined GitHub blob fetching using scoped worker threads (`std::thread::scope`).
- Replaced custom SWAR bitwise null scan with standard library routines.
- Hardened Unix intermediate symlink checks and preserved inode-based hard link exclusion.

## [0.1.0] - 2026-10-11

### Added

- CLI converting local directories and GitHub repositories into LLM-friendly digests.
- Local directory walking with `.gitignore` awareness, hidden file exclusion, and symlink skipping.
- Remote GitHub ingestion via Git trees and concurrent blob downloads without local `git` binary.
- Plain text and XML serialization formats (`--format <text|xml>`) with entity escaping and control character sanitization.
- Flexible output destinations: atomic file writing or stdout streaming (`--output -`).
- Git revision selection for remote repositories (`--ref <BRANCH|TAG|COMMIT>`).
- Streaming heuristic token estimation in digest output and completion summary.
- Built-in sensitive file and credential filtering (`.env*`, `*.key`, `*.pem`, `id_rsa*`).
- Conventional lockfile and noise path exclusion before candidate accounting.
- Resource budgets enforcing maximum candidate file counts and output sizes.

[0.2.3]: https://github.com/but212/codefeed/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/but212/codefeed/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/but212/codefeed/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/but212/codefeed/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/but212/codefeed/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/but212/codefeed/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/but212/codefeed/releases/tag/v0.1.0
