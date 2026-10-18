# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Compiler Documentation Enforcement**: Added `#![warn(missing_docs)]` to `src/lib.rs` and documented all re-exported public API items with doctests and error contracts.
- **Documentation Conventions**: Added Section 7 (`Documentation & Comments`) in `docs/coding_convention.md` defining Rustdoc standards (RFC 1574, `C-CRATE-DOC`, `C-EXAMPLE`, `C-FAILURE`, `C-LINK`, `C-QUESTION-MARK`), standard section headers (`# Errors`, `# Panics`, `# Safety`, `# Examples`), visibility depth, and "why, not what" comment principles.

### Changed

- **Core Storage Separation**: Extracted memory arena buffer and hierarchical path sorting (`FileStorage`, `FileSpan`, `FileRecord`, `FileRecordIter`, `cmp_path`) from `src/core/model.rs` into a dedicated `src/core/storage.rs` module, keeping `model.rs` focused on domain configurations and collection results with idiomatic `IntoIterator` and zero heap allocations.
- **Symlink Output Preservation**: Checked `symlink_metadata` in `resolve_output_target` to prevent canonicalizing existing leaf symlinks to target source files.
- **Archive Path Sanitization**: Rejected embedded NUL bytes and leading `.` / `..` archive root prefixes in `normalize_tar_entry_path`.
- **XML Attribute Whitespace Escaping**: Escaped `\t`, `\n`, and `\r` as character references (`&#9;`, `&#10;`, `&#13;`) in XML attributes (`write_xml_attr_escaped`) to prevent XML 1.0 §3.3.3 attribute normalization corruption.
- **Hidden Non-UTF-8 Path Accounting**: Dropped non-UTF-8 paths containing hidden components in `local::walk_candidates` before candidate budget and skipped accounting.
- **Visibility & Dead Code Demotion**: Removed unreachable `.env` branch in `policy::is_sensitive_file` and demoted single-file `pub(crate)` helpers to module-private across `policy`, `github_url`, `text`, `xml`, `selector`, `model`, and `github`.
- **Boundary Normalization**: Replaced `strip_archive_root` with `normalize_tar_entry_path` in `src/core/archive.rs`, normalizing consecutive slashes, stripping `.` segments, and rejecting directory traversal (`..`) at the ingestion boundary.
- **Policy Simplification**: Removed special-case exceptions for `.` and `..` in `policy::is_hidden_path`, relying on normalized path guarantees.
- **Hierarchical Path Sorting**: Implemented component-wise path sorting (`cmp_path`) in `FileStorage::sort_by_path()`, keeping directory subtrees adjacent.
- **Zero-Allocation Tree Rendering**: Converted `tree::render_tree` to stream directly from `&FileStorage`, completely removing `Child` struct, `Vec<Child>`, and intermediate allocations.
- **Direct Collection Rendering**: Unified `DigestFormat::render` to take `&CollectionResult` directly, eliminating `Vec<&str>` allocations.
- **Streaming Escapes & Newline Injection Defense**: Unified XML streaming escapes with `write_xml_tree_escaped`, preserving newline injection defenses while eliminating intermediate `Cow<'_, str>` and `xml_escape`.
- **Shell Stream Indirection Elimination**: Removed `SourceStream`, `StreamInfo`, `LocalStream`, and `GithubStream`, driving `Selector` directly from `local::collect` and `collect_archive`.
- **Selector State Pruning**: Removed redundant `Selector::content_bytes` in favor of `FileStorage::total_bytes()`.
- **CLI Option Parsing**: Deduplicated flag value extraction into `extract_opt_val` and restricted `"-"` to output flags only.
- **Third-Party Attribution**: Removed stale `clap` entry from `NOTICE`.
- **Candidate Path Arena**: Replaced `Vec<Option<String>>` in `src/shell/local.rs` with `PathArena` (single contiguous `String` buffer + offset spans), evaluating `CandidateBudget::add` on the fly to eliminate batch `check_limit` loops and individual heap string allocations.
- **Encapsulation & Visibility**: Demoted internal items (`FileStorage`, `FileSpan`, `FileRecord`, `CollectionResult`, `Selector`, `CandidateBudget`, `CandidateInput`, constants) from `pub` to `pub(crate)`, and single-file items (`HELP`, `NOISE`, `SENSITIVE_EXTENSIONS`, `RETRY_*`, XML/header helpers) to private.

## [0.2.4]

### Added

- `CandidateInput` (`src/core/selector.rs`): Unified candidate input enum (`File`, `TooLarge`, `Skipped`) simplifying the FCIS boundary data flow.
- `render_tree` (`src/core/tree.rs`): Direct deterministic ASCII tree rendering from sorted path slices.
- `resolve_output_target` (`src/shell/output.rs`): Single-pass output target validation and parent canonicalization.
- `policy::is_ignored_path` (`src/core/policy.rs`): Direct path exclusion for noise lockfiles, sensitive prefixes/extensions, and hidden components.

### Changed

- **Module Architecture**: Consolidated over-fragmented modules: unified `src/core/decide/*` (`budget.rs`, `item.rs`, `selector.rs`, `mod.rs`) into `src/core/selector.rs`, merged `local_io.rs` into `src/shell/local.rs`, and merged `github_client.rs` into `src/shell/github.rs`.
- **Directory Tree**: Eliminated `BudgetTrie` LCRS arena trie and `BudgetTrieNode`, rendering ASCII tree directly from lexicographically sorted `FileStorage`.
- **Hidden Path Matching**: Replaced substring path matching with segment-based matching (`split('/')`), preventing false positives on `./` segments.
- **CLI Parsing**: Dropped `clap` and `clap_derive` dependencies in favor of a minimal ~50-line standard library argument parser preserving all CLI flags, options, and help contracts.
- **Archive Ingestion**: Removed `seen_entries` state tracking and out-of-order PAX header error in `GithubStream`, accepting PAX headers whenever encountered.
- **Output Sanitization & Escaping**: Removed 16-byte manual chunk loops and bitmask lookups in `text.rs` and `xml.rs` in favor of standard linear slice iterators; streamed XML attributes directly via `write_escaped`.
- **Output Filtering**: Eliminated `OutputFilter` struct and repeated filesystem syscalls by resolving canonical output target once at startup.
- **Candidate Input & Budget**: Stripped `Cow` from `CandidateInput` to use borrowed `&str` and `&[u8]`; optimized `CandidateBudget::add` using `get_mut` to eliminate heap allocations for existing directories.
- **Stream Collection Pipeline**: Replaced indirect `Item` / `Probe` / `ItemDecision` evaluation steps with direct candidate streaming into `Selector::feed(CandidateInput)`.
- **Local Collection**: Replaced `Vec<ItemPath>` with `Vec<Option<String>>` candidate list and integrated `LocalFileRead` in `src/shell/local_io.rs`.
- **Token Estimation**: Replaced 220-line `TokenEstimator` lookup table state machine and streaming chunk buffer with a pure function `estimate_tokens(bytes)` (`(bytes / 4).max(1)` for non-zero bytes).
- **Candidate Accounting**: Replaced RLE `dir_runs` and 24-byte `CompactDir` in `CandidateBudget` with standard `HashMap<String, usize>`.
- **Error Types**: Unboxed `IngestError` string and slice payloads (`String`, `Vec<...>`, `CandidateLimitExceededError`); removed speculative 16-byte ABI packing assertions.
- **Retry Backoff**: Removed `RETRY_JITTER_MAX_MS` and entropy plumbing; made `retry_delay(attempt)` a deterministic pure function.
- **Binary Filtering**: Removed `BINARY_EXTENSIONS` hardcoded list and `PathClassification` enum; binary files are detected via UTF-8 / NUL-byte inspection during body read.
- **Testing**: Replaced repeated `SystemTime` temporary directory boilerplate in integration tests with RAII `TempDir`.
- **Documentation & Policy Alignment**: Reconciled optimization guidelines in `AGENTS.md` and `docs/coding_convention.md` to prioritize structural simplicity without benchmark mandates; documented `FileStorage` arena structural rationale (zero-copy views and in-place offset sorting); documented `estimate_tokens` ASCII/code heuristic limitations on multibyte/CJK text; and removed duplicate limit tables in `context.md` in favor of `policy.rs` as single source of truth.

### Removed

- External dependencies: `clap` and `clap_derive` (`Cargo.toml`).
- Module fragments: `src/core/decide/`, `src/shell/local_io.rs`, and `src/shell/github_client.rs`.
- `BudgetTrie`, `BudgetTrieNode`, and `CollectionResult::trie`.
- `OutputFilter` struct and redundant `fs::canonicalize` calls during directory walk.
- `seen_entries` boolean state on `GithubStream`.
- Intermediate stream types: `Item`, `ItemPath`, `Probe`, `ItemDecision`, and `Selector::decide`.
- `benches/bench.rs` (737 lines) and `pub mod __bench` in `src/lib.rs`.
- `docs/internal_bench_result.md`.
- `TokenEstimator` (`src/core/render/token.rs`).
- `CompactDir` and RLE directory accounting in `CandidateBudget`.
- `BINARY_EXTENSIONS`, `is_binary_extension`, `PathClassification`, and `LocalCandidate`.
- Panic-branch byte accessors (`path_bytes`, `name_bytes`) and micro-optimization attributes (`#[cold] #[inline(never)]`).
- Contrived lexical fallback tests for nonexistent output parents.

## [0.2.3]

### Added

- `CompactDir` and `CompactDir::as_bytes` (`src/core/decide.rs`): 24-byte inline directory descriptor (holding up to 22 bytes inline) with zero-validation byte slice projection for candidate budget tracking.
- `ItemPath<'a>` (`src/core/decide.rs`): generalized to `Cow<'a, str>` for zero-copy borrowed paths across core and shell boundaries.
- `FileStorage` (`src/core/model.rs`): contiguous path and content arenas indexed by 16-byte `FileSpan`, with zero-copy `FileRecord<'a>` view and in-place `sort_by_path`.
- `Source::with_reference` (`src/core/model.rs`): revision and source invariant validation.
- `CandidateBudget::check_limit` (`src/core/decide.rs`): batch pre-flight candidate count check with O(1) fast path.
- `Selector::finish` (`src/core/decide.rs`): builds `CollectionResult` directly with inline skipped byte tracking.
- `TokenEstimator` (`src/core/digest/token.rs`): non-destructive token estimation state machine backed by a 256-byte `CHAR_METRIC` lookup table.
- Byte budget tracking & top-K diagnostics (`src/core/digest/budget.rs`): Isolated byte accounting and diagnostic collection.
- `unescape_header` (`src/core/digest/text.rs`): lossless roundtrip decoding for escaped digest headers.
- Pure CLI parser (`src/core/cli.rs`): side-effect-free argument and flag evaluation using `clap` derive.
- `SourceStream` and `collect_stream` (`src/shell/orchestrate.rs`): generic streaming driver and `SourceStream::for_each_item` traversal returning `Result<StreamInfo, IngestError>`, unifying metadata lifecycle with stream completion.
- `LocalStream` (`src/shell/local.rs`): lazy candidate iterator with preflight limit check and a single reusable `work_path: PathBuf`.
- `GithubStream` (`src/shell/github.rs`): streaming archive traversal yielding `Item`s and resolving revisions.
- `read_local_file_into` (`src/shell/local_io.rs`): buffer-reusing reader for bounded local file content into caller-provided scratch buffers.
- `current_epoch_nanos()` (`src/shell/mod.rs`): shared epoch timestamp and entropy source.
- Re-exports (`src/lib.rs`): exposed `DigestFormat`, `IngestReport`, `Source`, and `IngestError`.
- `local_walk_hierarchical_1000` (`benches/bench.rs`): benchmark measuring candidate walking, normalization, sorting, and budget checking isolated from disk reads.

### Changed

- `BudgetTrie` (`src/core/digest/trie.rs`): rebuilt as a flat 16-byte POD node arena (`Vec<BudgetTrieNode>`) with `u32` indices, `label_arena: String`, `push_node` allocation helper, and UTF-8-safe truncation at `u16::MAX` (65,535 bytes).
- `CandidateBudget` (`src/core/decide.rs`): switched from `HashMap` and `String` allocations to 32-byte RLE `dir_runs: Vec<(CompactDir, usize)>` with byte-slice matching, probing recent slots and aggregating borrowed `&str` only during overflow diagnostics.
- `FileSpan` (`src/core/model.rs`): added direct `path` and `content` projection methods into `FileStorage`.
- `XmlChunks` (`src/core/digest/xml.rs`): single-pass XML escaping with cached pending sequences, shared `first_special_index`, and deferred rendering without trie display caching.
- `PathClassification::classify` (`src/core/policy.rs`): consolidated multiple path scans into a single filename/extension extraction pass.
- `retry_delay` (`src/core/policy.rs`): capped backoff delay strictly within 5,000 ms while preserving random jitter.
- `walk_candidates` (`src/shell/local.rs`): enforces `CandidateBudget::check_limit` before `candidates.sort_unstable_by`, preventing unnecessary sorting and disk reads on overflow.
- `LocalCandidate` (`src/shell/local.rs`): reduced size from 88B to 32B by replacing `Option<Probe>` and dropping redundant `path: PathBuf`.
- `probe_tar_entry` (`src/shell/github.rs`): moved path classification ahead of heap allocation; replaced hidden path checks with single-pass byte scan (`starts_with(b".")` and `windows(2).any(|w| w == b"/.")`); stripped root archive prefixes and consecutive slashes.
- `GithubStream` (`src/shell/github.rs`): propagates PAX global header I/O errors and enforces strict rejection of trailing PAX headers after archive entries.
- `OutputFilter` and `normalize_path` (`src/shell/output.rs`): consolidated from `output_exclusion.rs` to zero-syscall lexical matching and parent-canonical normalization; added length pre-check fast path in `paths_match`; fixed relative `..` path truncation.
- Pipeline decoupling: collection (`Selector`, `CollectionResult`, `collect_stream`) decoupled from `DigestFormat`; format passed only to `render_digest_to_writer`.
- HTTP transport (`src/shell/github_client.rs`): unified retry handling, buffered tarball decompression through a 64 KiB `BufReader`, and set `RedirectAuthHeaders::Never` to prevent header leaks on cross-origin redirects.
- Module naming and conventions (`docs/coding_convention.md`):
  - Functions: `core::cli::parse_cli` -> `cli::parse`, `core::source::parse_source` -> `source::parse`, `shell::local::collect_local` -> `local::collect`, `shell::github::collect_github` -> `github::collect`.
  - Types: `Source::GitHub` -> `Source::Github`, `GitHubStream` -> `GithubStream`.
  - Policy constants: `SENSITIVE_EXT` -> `SENSITIVE_EXTENSIONS`, `BINARY_EXT` -> `BINARY_EXTENSIONS`, `is_binary_ext` -> `is_binary_extension`.

### Fixed

- `cli::parse` (`src/core/cli.rs`): fixed bug where specifying `-- - -o -` defaulted to a file named `"-"` instead of `OutputTarget::Stdout`.
- `BoundedEstimatingWriter::finish` (`src/core/digest/mod.rs`): eliminated double-flush on atomic file targets and preserved `estimated_tokens` on downstream `BrokenPipe`.
- `OutputFilter` (`src/shell/output.rs`): canonicalizes existing targets and lexically normalizes non-existent parent paths, fixing `ENOENT` on relative paths; skips redundant syscall fallbacks on Unix.
- `AtomicFile::create` (`src/shell/output.rs`): rejects directory targets immediately with `io::ErrorKind::IsADirectory` before temp file allocation.
- `OutputTarget` validation (`src/shell/output.rs`): maps paths lacking file names to `IngestError::Usage`.
- Tarball NUL escaping (`src/shell/github.rs`, `src/core/digest/xml.rs`): rejects embedded NUL in tar paths, and escapes `0x00` as `\u{FFFD}` in XML output.
- `collect_local` (`src/shell/local.rs`): uses `WalkBuilder::sort_by_file_name` for deterministic cross-platform error reporting; verifies `is_file()` without following symlinks.
- `is_hidden_path` (`src/core/policy.rs`): ignores empty path segments (`//`), preventing false hidden classifications.
- `is_sensitive_path` (`src/core/policy.rs`): made `.env*` prefix matching case-insensitive.
- `GithubClient` user agent (`src/shell/github_client.rs`): dynamic `concat!("codefeed/", env!("CARGO_PKG_VERSION"))` replacing static `"codefeed/0.1"`.

### Removed

- Duplicate stream iterators: removed legacy `Iterator` and `ExactSizeIterator` from `LocalStream` and `GithubStream` in favor of `SourceStream::for_each_item`.
- Deleted modules: `src/shell/output_exclusion.rs` and `src/shell/cli.rs`.
- External dependencies: removed `rustix`, `percent-encoding`, `memchr`, and `log` from `Cargo.toml`.
- Dead and allocating wrappers: `read_local_file` (`src/shell/local_io.rs`), non-into `probe_tar_entry` (`src/shell/github.rs`), and legacy policy helpers (`is_noise_path`, `is_sensitive_path`, `has_binary_extension`).
- Redundant types and variants: `Options`, `Probe::Ignored`, `ItemResolution::Ignored`, `Probe::PreclassifiedBinary`, `DigestBudget`, and `BudgetOverflow`.
- Unused fields and methods: `root: PathBuf` on `LocalStream`, lifetime `'a` on `AtomicFile`, `root_children` on `BudgetTrie`, `XmlChunk::ends_with_nl()`, `XmlChunk::len`, `Selector::is_empty`, `text_content`, `Selector::into_parts`, and `Index<usize>` on `FileStorage`.
- Redundant allocations: static 4 KiB escape tables in `text.rs` and `xml.rs`, intermediate `Vec<FileRecord>` buffers in renderers, and display string caching in trie nodes.

## [0.2.2]

### Added

- `BoundedEstimatingWriter` wrapping output stream serialization to strictly enforce `MAX_OUTPUT_SIZE` against rendered bytes (framing, tree, metadata, content) and estimate tokens in a single pass.

### Changed

- Streamlined local walk to a 2-step linear collection pipeline (`ignore::WalkBuilder` candidate harvest followed by bounded file reads).
- Replaced channel passing and worker thread pools in `src/shell/local.rs` with sequential traversal.
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
- Scoped test fixture directory lifecycle guard (`TempDirGuard`) in `src/shell/mod.rs` with path canonicalization.

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

[0.2.4]: https://github.com/but212/codefeed/compare/v0.2.3...v0.2.4
[0.2.3]: https://github.com/but212/codefeed/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/but212/codefeed/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/but212/codefeed/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/but212/codefeed/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/but212/codefeed/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/but212/codefeed/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/but212/codefeed/releases/tag/v0.1.0
