# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Re-exported domain types `DigestFormat`, `IngestReport`, `Source`, and `IngestError` in `src/lib.rs`.
- `SourceStream` trait and generic streaming collection driver `collect_stream` in `src/shell/orchestrate.rs`, unifying selection, budget management, and completion across all source types.
- Lazy candidate reader iterator `LocalStream` in `src/shell/local.rs` with fast-fail preflight limit validation on creation.
- Streaming archive entry iterator `GitHubStream` in `src/shell/github.rs` yielding `Item`s and managing revision resolution.
- Continuous arena storage `FileStorage` in `src/core/model.rs` backed by contiguous path and content arenas (`String`) and 16-byte index descriptors (`FileSpan`), eliminating per-file heap fragmentation and preserving cache locality.
- In-place span sorting `FileStorage::sort_by_path(&mut self)` operating directly on 16-byte `FileSpan` slices.
- Zero-copy borrowed view `FileRecord<'a>` into files stored within `FileStorage`.
- Unified token estimation state machine `TokenEstimator` in `src/core/digest/token.rs`, shared by `estimate_tokens` and `BoundedEstimatingWriter`.
- Isolated byte budget tracking and top-K file diagnostics in `src/core/digest/budget.rs`.
- Pure CLI argument parser `src/core/cli.rs`, migrating argument and flag evaluation to the functional core without side effects or I/O.
- Domain reference setter `Source::with_reference` in `src/core/model.rs` encapsulating revision validation and GitHub/local source invariants.
- Batch pre-flight candidate verification `CandidateBudget::check_limit` with $O(1)$ fast-path in `src/core/decide.rs`.
- Centralized shell epoch timestamp generator `current_epoch_nanos()` in `src/shell/mod.rs`.
- Encapsulated collection completion in `Selector::finish` in `src/core/decide.rs`, constructing `CollectionResult` directly.

### Fixed

- Decoupled inner stream flushing from `BoundedEstimatingWriter::finish` (`src/core/digest/mod.rs`), preserving computed `estimated_tokens` on downstream `BrokenPipe` during stdout streaming and eliminating redundant duplicate flushing for atomic file targets.
- Prioritized `fs::canonicalize(output)` directly in `OutputFilter::new` (`src/shell/output_exclusion.rs`) before lexical traversal fallback, and normalized parent paths before `fs::create_dir_all` in `AtomicFile::create` (`src/shell/output.rs`) to prevent phantom directory creation from relative parent aliases.
- Stripped tar archive root and filtered hidden entries on raw path bytes prior to candidate slot allocation in `probe_tar_entry` (`src/shell/github.rs`), validated paths with `text_str` to reject embedded NUL bytes, and escaped `0x00` to `\u{FFFD}` in `xml_escape` and `write_xml_escaped` (`src/core/digest/xml.rs`).
- Short-circuited canonical path checks and eliminated redundant `names_match` and `fs::canonicalize` fallback syscalls on Unix in `OutputFilter::is_excluded`.
- Enforced deterministic filesystem traversal order in `collect_local` via `WalkBuilder::sort_by_file_name`, guaranteeing reproducible `CandidateLimitExceeded` diagnostic output across different operating systems and filesystems.
- Dissolved nonexistent output file filtering by returning `Ok(None)` from `OutputFilter::new` when the output path does not exist on disk prior to walking, eliminating redundant path normalization and comparisons.
- Checked `entry.file_type().is_file()` before output exclusion checks in `collect_local`, and used `fs::symlink_metadata` on Unix to prevent following symlinks.
- Rejected directory target paths immediately with `io::ErrorKind::IsADirectory` in `AtomicFile::create` prior to temporary file creation.
- Filtered redundant empty path components (`//`) in `is_hidden_path` (`src/core/policy.rs`) to prevent non-hidden paths with duplicate delimiters from being misclassified as hidden.
- Mapped invalid output file paths lacking a file name component to `IngestError::Usage` in `src/shell/output.rs`.
- Enforced candidate count limits prior to reading file contents from disk in local walk (`src/shell/local.rs`) via `CandidateBudget::check_limit`, preventing unbounded I/O when file counts exceed `MAX_FILE_COUNT`.
- Enforced case-insensitive `.env*` prefix check in `is_sensitive_path` (`src/core/policy.rs`), harmonizing implementation with documentation.
- Replaced hardcoded `"codefeed/0.1"` user agent with `concat!("codefeed/", env!("CARGO_PKG_VERSION"))` in `src/shell/github_client.rs`.

### Changed

- Eliminated duplicate `xml_special` evaluations in `XmlChunks` by caching pending escape sequences and streamlined preflight special byte detection in `src/core/digest/xml.rs`.
- Refactored hot and warm path routines (XML/text header escaping, trie insertion, SHA and name validation, and path classification) to branchless bitwise operations and constant lookup tables.
- Consolidated `OutputFilter`, path identity resolution, and lexical `normalize_path` from `src/shell/output_exclusion.rs` directly into `src/shell/output.rs`, encapsulated destination matching via `OutputFilter::from_target(&OutputTarget)`, and eliminated `output_exclusion.rs`.
- Decoupled item evaluation and storage accumulation in `src/core/decide.rs` by extracting pure `ItemDecision` contract (`Included`, `Skipped`), enabling zero-copy UTF-8/NUL evaluation via `Item::evaluate` and direct accumulation via `Selector::accumulate`.
- Propagated collection stream size hints from `LocalStream` (`ExactSizeIterator`) into `Selector::from_size_hint` in `src/shell/orchestrate.rs`, pre-sizing storage arenas according to upstream candidate counts.
- Optimized `BudgetTrie` in `src/core/digest/trie.rs` into a 16-byte fixed-size Left-Child Right-Sibling (LCRS) node arena and contiguous `label_arena: String`, reducing node size from 48B to 16B and eliminating per-node heap allocations.
- Removed 4 KiB static `ESCAPE_TABLE` in `src/core/digest/xml.rs` in favor of inline pattern matching over `SPECIAL_BITSET`, and refactored `xml_escape` and `escape_header` to single-pass `.position()` scanning.
- Prevented unbounded `String` heap allocations in `CandidateBudget::record_dir_run` (`src/core/decide.rs`) on interleaved directory inputs by probing recent slots.
- Added capacity pre-reservation to `FileStorage` and short-circuited `sort_by_path()` on already-sorted inputs.
- Slimmed `LocalCandidate` in `src/shell/local.rs` from 88B to 56B by replacing `Option<Probe>` with `skip_read: bool`.
- Buffered small streaming writes in `BoundedEstimatingWriter` (`src/core/digest/mod.rs`) via an internal 8 KiB chunk buffer, batching dynamic dispatch and token estimation.
- Explicitly configured `redirect_auth_headers(RedirectAuthHeaders::Never)` in `GithubClient::new` (`src/shell/github_client.rs`), pinning the security contract against header propagation during cross-origin redirects with regression tests.
- Replaced `to_string_lossy()` calls in Windows `is_strictly_beneath` (`src/shell/local_io.rs`) with `OsStr::eq_ignore_ascii_case`, eliminating heap allocations for wide/non-UTF-8 paths.
- Added `debug_assert!` bounds checks to `FileStorage::push` (`src/core/model.rs`) guarding against `u32` offset truncation.
- Deduplicated candidate budget accounting in `Selector::decide` via `ItemResolution::path`, and extracted `CandidateBudget::record_dir_run` and `overflow_error` helper methods in `src/core/decide.rs`.

- Replaced `CandidateBudget` happy-path `HashMap` allocations with run-length encoded directory tracking (`dir_runs: Vec<(String, usize)>`), deferring aggregation until candidate overflow diagnostics are formatted.
- Unified `BudgetTrie` tree root at `nodes[0]`, eliminating `root_children: Vec<u32>` and redundant `Option<u32>` branch checks in `get_or_insert_child`.
- Streamlined `DigestFormat::render` to consume `ExactSizeIterator<Item = FileRecord<'_>>` streaming directly, eliminating 32-byte `Vec<FileRecord>` heap reallocation and re-sorting.
- Inlined byte counting and skip tracking directly into `Selector`, eliminating split-brain states between `CollectionResult.files` and `CollectionResult.trie`.
- Decoupled data collection (`Selector`, `CollectionResult`, `collect_local`, `collect_github`) from presentation format (`DigestFormat`), passing format strictly to `render_digest_to_writer`.
- Shifted `BudgetTrie` XML entity and newline escaping from node insertion time to on-the-fly rendering (`render_to_writer`), pruning cached display strings from `BudgetTrieNode` and keeping trie structures format-neutral.
- Replaced `Vec<FileRecord>` in `CollectionResult` and `Selector` with `FileStorage`, feeding zero-copy borrowed records to digest renderers and diagnostic handlers.
- Encapsulated `FileStorage` internal arena buffers (`spans`, `path_arena`, `content_arena`) behind private visibility.
- Refactored `BudgetTrie` in `src/core/digest/trie.rs` into a contiguous flat node arena (`Vec<BudgetTrieNode>`) with `u32` indexed child tracking (`Vec<u32>`) and `Box<str>` node labels, reducing arena memory footprint on 64-bit platforms and eliminating recursive heap pointer chasing.
- Structured `IngestError::CandidateLimitExceeded` and `IngestError::OutputTooLarge` diagnostics, centralizing message formatting in `error.rs`'s `Display` implementation.
- Replaced external crate `percent-encoding` in `src/core/github_url.rs` with a pure `std` RFC 3986 Unreserved validator and `%XX` hex encoder.
- Replaced external crate `memchr` in `src/core/decide.rs` with `slice::contains(&0)` and in `src/core/digest/text.rs` with `std` pattern matching.
- Replaced external crate `log` in `src/shell/github_client.rs` with `eprintln!` diagnostics.
- Buffered streaming GitHub tarball HTTP response bodies through a 64 KiB `BufReader` in `src/shell/github_client.rs` before decompression.
- Decoded GitHub tarball paths directly from `entry.path_bytes()` in `src/shell/github.rs` via `std::str::from_utf8`.
- Consolidated duplicated HTTP transport error and status retry logic in `src/shell/github_client.rs` into a unified helper.

### Removed

- Removed dead `Selector::is_empty` method and uncalled `text_content` wrapper in `src/core/decide.rs`, switching benchmarks in `benches/bench.rs` to zero-copy `text_str(&[u8])`.
- Removed separate collection entry points `collect_local`, `collect_github`, and `collect_from_archive` in favor of the unified `collect_stream` streaming pipeline.
- Removed redundant `Probe::PreclassifiedBinary` variant, consolidating binary extension skipping with `Probe::Missing`.
- Removed `DigestBudget` and `BudgetOverflow` wrapper types in `src/core/digest/budget.rs`.
- Removed `XmlChunk::ends_with_nl()` in `src/core/digest/xml.rs`, checking trailing newline directly on the source content.
- Removed 4 KiB static character escape table in `src/core/digest/text.rs` in favor of inline byte pattern matching.
- Removed `RequestTarget` enum in `src/core/github_url.rs`, replacing origin checks with `is_github_api_origin`.
- Removed dead `output_identity` and redundant path normalization helpers from `src/shell/output_exclusion.rs`.
- Removed external dependencies `percent-encoding`, `memchr`, and `log` from `Cargo.toml`.
- Removed intermediate single-use `Options` struct from `src/core/model.rs`.
- Removed dead `Probe::Ignored` variant and unused `ItemResolution::Ignored` match arm from `src/core/decide.rs`.
- Removed `src/shell/cli.rs` in favor of `src/core/cli.rs`.
- Removed duplicated `SystemTime` and `UNIX_EPOCH` calls across `output.rs`, `github_client.rs`, and `output_exclusion.rs`.
- Removed dead `Index<usize>` implementation on `FileStorage` in `src/core/model.rs`.
- Removed dead `escaped_metrics` helper and `XmlChunk::len` method in `src/core/digest/xml.rs`.
- Removed `Selector::into_parts` across `src/core/decide.rs`, shell collectors, and benchmarks in favor of `Selector::finish`.

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

[Unreleased]: https://github.com/but212/codefeed/compare/v0.2.2...HEAD
[0.2.2]: https://github.com/but212/codefeed/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/but212/codefeed/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/but212/codefeed/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/but212/codefeed/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/but212/codefeed/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/but212/codefeed/releases/tag/v0.1.0
