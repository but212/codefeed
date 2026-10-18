# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **GitRef Domain Type**: Added [`GitRef`] enforcing non-empty, non-whitespace Git revision invariants with zero-copy `&str` access.
- **Validated Source Constructor**: Added [`Source::github`] validating repository owner/name characters, non-emptiness, and traversal segments.
- **CLI Dispatch Entry Point**: Added `codefeed::run_cli` (`src/shell/mod.rs`) encapsulating CLI parsing, help output, ingestion, and stdio reporting.
- **Compiler Visibility & Doc Enforcement**: Added `#![warn(unreachable_pub)]` and `#![warn(missing_docs)]` to `src/lib.rs`; documented all re-exported public API items with doctests and error contracts.
- **Documentation Conventions**: Added Section 7 (`Documentation & Comments`) in `docs/coding_convention.md` defining Rustdoc standards (RFC 1574, `C-CRATE-DOC`, `C-EXAMPLE`, `C-FAILURE`, `C-LINK`, `C-QUESTION-MARK`), section headers (`# Errors`, `# Panics`, `# Safety`, `# Examples`), visibility depth, and "why, not what" comment rules.

### Changed

- **CandidateInput Model Simplification & LocalFileRead Elimination**: Eliminated redundant `CandidateInput::TooLarge` in favor of `CandidateInput::Skipped { path: Some(...) }`; replaced the intermediate `LocalFileRead` enum in `src/shell/local.rs` with `Option<&[u8]>`.
- **Single-Pass Local Collection & PathArena Removal**: Replaced the 2-pass local filesystem traversal in `src/shell/local.rs` with single-pass streaming directly into `Selector::feed`, removing `walk_candidates`, `PathArena`, duplicate `CandidateBudget` calculation, and unused `Selector::from_size_hint`; updated candidate overflow contract to fail-fast immediately upon exceeding `MAX_FILE_COUNT`.
- **Core Module Consolidation & Storage Diagnostic Decoupling**: Merged `src/core/tree.rs` into `src/core/render.rs`, removing the private `tree` module; moved `largest_files` diagnostic selection from `src/core/selector.rs` to `src/core/storage.rs` as `FileStorage::largest_files`, decoupling `render.rs` from `selector.rs`.
- **CLI Option Error Precedence**: Deferred `GitRef` validation in `cli::parse` until after `<SOURCE>` presence and source-kind checks, preserving `--help` precedence and variant compatibility errors.
- **Breaking Change — Source::Github Encapsulation**: Marked `Source::Github` `#[non_exhaustive]` (constructed via `Source::github` or `Source::parse`); updated `Source::Github.reference` and `Source::with_reference` from `Option<String>` to `Option<GitRef>`.
- **Breaking Change — Public API Surface & Non-Exhaustive Types**: Demoted `CliAction`, `help_text`, `parse`, and 7 `IngestError` constructors (`candidate_limit_exceeded`, `invalid_source`, `unsupported_source`, `http`, `api`, `invalid_metadata`, `output_too_large`) to `pub(crate)` (retaining `IngestError::usage` and `codefeed::run_cli`); marked `IngestError` and `IngestReport` `#[non_exhaustive]`.
- **Encapsulation & Module Scoping**: Made `FileSpan` fields (`path_offset`, `path_len`, `content_offset`, `content_len`) private to `storage.rs`; demoted `tree`, `github`, `local`, and `output` modules to private; scoped `render_tree`, `largest_files`, `parse_os`, `collect`, `resolve_output_target`, `emit`, and `FileSpan::{path, content}` to `pub(super)`; made single-module helpers (`render_text`, `render_xml`, `estimate_tokens`, `render_stdout_to`, `is_github_api_origin`, `current_epoch_nanos`, `temp_dir`, `TempDirGuard`, `BODY_READ_COUNT`, `Source::parse`) private.
- **Module Consolidation**: Merged `src/core/render/{mod.rs, text.rs, xml.rs}` into `src/core/render.rs`, `orchestrate.rs` into `src/shell/mod.rs` (`pub use shell::ingest;`), `source.rs` into `model.rs` (`Source::parse`, `Source::parse_os`), and GitHub URL/origin helpers into `src/shell/github.rs`.
- **Core Storage Separation**: Extracted `FileStorage`, `FileSpan`, `FileRecord`, `FileRecordIter`, and `cmp_path` from `src/core/model.rs` into `src/core/storage.rs` with zero-allocation `IntoIterator`.
- **Stream Escaping & Path Normalization**: Made `write_stream_escaped` single-pass without pre-scan or empty-slice guards; replaced `escape_header` with streaming `write_header_escaped`, removing `std::borrow::Cow` across the crate; simplified `cmp_path` via `Iterator::cmp`; unified `local::normalize_relative_path_into` via `!scratch.is_empty()`; replaced empty-segment filtering in `tree::component_at` with a debug assertion in `FileStorage::push`.
- **XML Attribute & Tree Escaping**: Unified `<directory_structure>` and `<file path="...">` escaping under `write_xml_attr_escaped`, escaping `\t`, `\n`, and `\r` as `&#9;`, `&#10;`, and `&#13;` (preventing XML 1.0 §3.3.3 normalization corruption and newline injection without doubling backslashes) and removing `xml_tree_special_at`, `write_xml_tree_escaped`, `xml_escape`, and intermediate `Cow<'_, str>`.
- **Atomic Stdout Delivery**: Buffered stdout up to `MAX_OUTPUT_SIZE` before emission so overflow errors write zero bytes, matching file rollback semantics.
- **Symlink Output Preservation**: Checked `symlink_metadata` in `resolve_output_target` to avoid canonicalizing existing leaf symlinks onto source files.
- **Archive Boundary Normalization**: Replaced `strip_archive_root` with `normalize_tar_entry_path` (`src/core/archive.rs`), normalizing consecutive slashes, stripping `.` segments, and rejecting embedded NUL bytes, leading `.`/`..` root prefixes, and `..` traversal.
- **Hidden Path & Policy Cleanup**: Dropped non-UTF-8 paths with hidden components in `local::walk_candidates` before budget/skip accounting; removed `.` and `..` exceptions in `policy::is_hidden_path` and unreachable `.env` branch in `policy::is_sensitive_file`.
- **Zero-Allocation Tree & Collection Rendering**: Sorted paths component-wise (`cmp_path`) in `FileStorage::sort_by_path()`; streamed `tree::render_tree` directly from `&FileStorage` (removing `Child` and `Vec<Child>`); passed `&CollectionResult` directly to `DigestFormat::render` (removing `Vec<&str>`).
- **Direct Ingestion & Candidate Path Arena**: Removed `SourceStream`, `StreamInfo`, `LocalStream`, and `GithubStream`, driving `Selector` directly from `local::collect` and `collect_archive`; replaced `Vec<Option<String>>` in `src/shell/local.rs` with contiguous `PathArena` and on-the-fly `CandidateBudget::add`; dropped redundant `Selector::content_bytes` for `FileStorage::total_bytes()`.
- **CLI Option Parsing & Notice**: Deduplicated option value extraction into `extract_opt_val`, restricted `"-"` to output flags, and removed stale `clap` entry from `NOTICE`.

## [0.2.4]

### Added

- `CandidateInput` (`src/core/selector.rs`): Unified candidate enum (`File`, `TooLarge`, `Skipped`) across the FCIS boundary.
- `render_tree` (`src/core/tree.rs`): Deterministic ASCII tree rendering from sorted path slices.
- `resolve_output_target` (`src/shell/output.rs`): Single-pass output target validation and parent canonicalization.
- `policy::is_ignored_path` (`src/core/policy.rs`): Direct path exclusion for noise lockfiles, sensitive prefixes/extensions, and hidden components.

### Changed

- **Module Architecture**: Unified `src/core/decide/*` (`budget.rs`, `item.rs`, `selector.rs`, `mod.rs`) into `src/core/selector.rs`, `local_io.rs` into `src/shell/local.rs`, and `github_client.rs` into `src/shell/github.rs`.
- **Directory Tree**: Replaced `BudgetTrie` and `BudgetTrieNode` with direct ASCII rendering from sorted `FileStorage`.
- **Hidden Path Matching**: Switched to segment-based matching (`split('/')`), preventing false positives on `./`.
- **CLI Parsing**: Replaced `clap` and `clap_derive` with a ~50-line `std` parser preserving all CLI contracts.
- **Archive Ingestion**: Removed `seen_entries` state and out-of-order PAX header rejection in `GithubStream`.
- **Output Sanitization & Escaping**: Replaced 16-byte chunk loops and bitmask lookups in `text.rs` and `xml.rs` with linear slice iterators; streamed XML attributes via `write_escaped`.
- **Output Filtering**: Replaced `OutputFilter` and repeated `fs::canonicalize` calls with startup output target resolution.
- **Candidate Input, Budget & Accounting**: Replaced `Cow` in `CandidateInput` with borrowed `&str`/`&[u8]`; replaced RLE `dir_runs` and 24-byte `CompactDir` in `CandidateBudget` with `HashMap<String, usize>` using `get_mut`.
- **Stream & Local Collection**: Streamed candidates directly into `Selector::feed(CandidateInput)`, removing `Item`, `Probe`, and `ItemDecision`; replaced `Vec<ItemPath>` with `Vec<Option<String>>` and integrated `LocalFileRead`.
- **Token Estimation**: Replaced 220-line `TokenEstimator` state machine with pure `estimate_tokens(bytes)` (`(bytes / 4).max(1)` for non-zero bytes).
- **Error Types & Retry Backoff**: Unboxed `IngestError` payloads (`String`, `Vec<...>`, `CandidateLimitExceededError`) and dropped 16-byte ABI assertions; removed `RETRY_JITTER_MAX_MS` to make `retry_delay(attempt)` pure and deterministic.
- **Binary Filtering**: Removed `BINARY_EXTENSIONS` and `PathClassification`; detected binary files via UTF-8/NUL inspection during body read.
- **Testing & Docs**: Added RAII `TempDir` for integration tests; aligned `AGENTS.md`, `docs/coding_convention.md`, and `context.md` on structural simplicity, `FileStorage` rationale, `estimate_tokens` CJK caveats, and `policy.rs` as single source of truth.

### Removed

- External crates `clap` and `clap_derive` (`Cargo.toml`).
- Modules `src/core/decide/`, `src/shell/local_io.rs`, `src/shell/github_client.rs`, `benches/bench.rs` (737 lines), `pub mod __bench` (`src/lib.rs`), and `docs/internal_bench_result.md`.
- Types and helpers: `BudgetTrie`, `BudgetTrieNode`, `CollectionResult::trie`, `OutputFilter`, `GithubStream::seen_entries`, `Item`, `ItemPath`, `Probe`, `ItemDecision`, `Selector::decide`, `TokenEstimator` (`src/core/render/token.rs`), `CompactDir`, `BINARY_EXTENSIONS`, `is_binary_extension`, `PathClassification`, `LocalCandidate`, `path_bytes`, `name_bytes`, `#[cold] #[inline(never)]` annotations, and lexical fallback tests for nonexistent output parents.

## [0.2.3]

### Added

- `CompactDir` and `CompactDir::as_bytes` (`src/core/decide.rs`): 24-byte inline directory descriptor (up to 22 bytes inline) with byte-slice projection for candidate budgets.
- `ItemPath<'a>` (`src/core/decide.rs`): `Cow<'a, str>` wrapper for zero-copy paths across core/shell boundaries.
- `FileStorage` (`src/core/model.rs`): Contiguous path/content arenas indexed by 16-byte `FileSpan` with `FileRecord<'a>` views and in-place `sort_by_path`.
- `Source::with_reference` (`src/core/model.rs`): Revision and source invariant validation.
- `CandidateBudget::check_limit` & `Selector::finish` (`src/core/decide.rs`): O(1) batch pre-flight candidate check and direct `CollectionResult` builder with skipped-byte tracking.
- `TokenEstimator` (`src/core/digest/token.rs`): Non-destructive token state machine backed by a 256-byte `CHAR_METRIC` table.
- Digest helpers: Byte budget tracking and top-K diagnostics (`src/core/digest/budget.rs`), `unescape_header` roundtrip decoder (`src/core/digest/text.rs`), and pure `clap` derive CLI parser (`src/core/cli.rs`).
- Streaming shell & benchmarks: `SourceStream`, `collect_stream`, and `StreamInfo` (`src/shell/orchestrate.rs`); `LocalStream` (`src/shell/local.rs`); `GithubStream` (`src/shell/github.rs`); `read_local_file_into` (`src/shell/local_io.rs`); `current_epoch_nanos()` (`src/shell/mod.rs`); root re-exports (`DigestFormat`, `IngestReport`, `Source`, `IngestError` in `src/lib.rs`); and `local_walk_hierarchical_1000` (`benches/bench.rs`).

### Changed

- `BudgetTrie` (`src/core/digest/trie.rs`): Rebuilt as a 16-byte POD node arena (`Vec<BudgetTrieNode>`) with `u32` indices, `label_arena: String`, `push_node`, and UTF-8 truncation at `u16::MAX` (65,535 bytes).
- `CandidateBudget` (`src/core/decide.rs`): Switched from `HashMap` to 32-byte RLE `dir_runs: Vec<(CompactDir, usize)>`, aggregating borrowed `&str` only on overflow.
- `FileSpan` (`src/core/model.rs`): Added `path` and `content` projection methods into `FileStorage`.
- `XmlChunks` (`src/core/digest/xml.rs`): Single-pass XML escaping with cached pending sequences, shared `first_special_index`, and deferred rendering.
- `PathClassification::classify` & `retry_delay` (`src/core/policy.rs`): Consolidated path scans into one pass; capped backoff at 5,000 ms with jitter.
- Local & GitHub ingestion: Enforced `CandidateBudget::check_limit` before `candidates.sort_unstable_by` in `walk_candidates`; shrunk `LocalCandidate` from 88B to 32B (dropping `Option<Probe>` and `path: PathBuf`); classified paths before allocation and scanned hidden segments in one pass (`starts_with(b".")`, `windows(2).any(|w| w == b"/.")`) in `probe_tar_entry`; propagated PAX global header I/O errors and rejected trailing PAX headers in `GithubStream`.
- `OutputFilter` & `normalize_path` (`src/shell/output.rs`): Consolidated from `output_exclusion.rs` with zero-syscall lexical matching, parent-canonical normalization, length pre-checks in `paths_match`, and `..` truncation fixes.
- Pipeline & HTTP transport: Decoupled collection (`Selector`, `CollectionResult`, `collect_stream`) from `DigestFormat` (passed only to `render_digest_to_writer`); unified retries, 64 KiB `BufReader` tarball buffering, and `RedirectAuthHeaders::Never` in `src/shell/github_client.rs`.
- Naming conventions (`docs/coding_convention.md`): Renamed `parse_cli` -> `cli::parse`, `parse_source` -> `source::parse`, `collect_local` -> `local::collect`, `collect_github` -> `github::collect`, `Source::GitHub` -> `Source::Github`, `GitHubStream` -> `GithubStream`, `SENSITIVE_EXT` -> `SENSITIVE_EXTENSIONS`, `BINARY_EXT` -> `BINARY_EXTENSIONS`, and `is_binary_ext` -> `is_binary_extension`.

### Fixed

- `cli::parse` (`src/core/cli.rs`): Mapped `-- - -o -` to `OutputTarget::Stdout` instead of a file named `"-"`.
- `BoundedEstimatingWriter::finish` (`src/core/digest/mod.rs`): Eliminated double-flush on atomic files and preserved `estimated_tokens` on `BrokenPipe`.
- Output handling (`src/shell/output.rs`): Canonicalized existing targets and lexically normalized nonexistent parents in `OutputFilter` (fixing relative `ENOENT` and skipping redundant Unix syscalls); rejected directory targets with `io::ErrorKind::IsADirectory` in `AtomicFile::create`; mapped nameless paths to `IngestError::Usage`.
- Path & security checks: Rejected embedded NUL in tar paths and escaped `0x00` as `\u{FFFD}` in XML (`src/shell/github.rs`, `src/core/digest/xml.rs`); sorted `collect_local` via `WalkBuilder::sort_by_file_name` and verified `is_file()` without following symlinks; ignored empty segments (`//`) in `is_hidden_path`; made `.env*` matching case-insensitive in `is_sensitive_path`; set dynamic `concat!("codefeed/", env!("CARGO_PKG_VERSION"))` user agent in `GithubClient`.

### Removed

- Legacy `Iterator`/`ExactSizeIterator` impls on `LocalStream` and `GithubStream`.
- Modules `src/shell/output_exclusion.rs` and `src/shell/cli.rs`; crates `rustix`, `percent-encoding`, `memchr`, and `log`.
- Wrappers, types, and unused items: `read_local_file`, non-into `probe_tar_entry`, `is_noise_path`, `is_sensitive_path`, `has_binary_extension`, `Options`, `Probe::Ignored`, `ItemResolution::Ignored`, `Probe::PreclassifiedBinary`, `DigestBudget`, `BudgetOverflow`, `LocalStream::root`, `AtomicFile` lifetime `'a`, `BudgetTrie::root_children`, `XmlChunk::ends_with_nl()`, `XmlChunk::len`, `Selector::is_empty`, `text_content`, `Selector::into_parts`, `Index<usize>` on `FileStorage`, 4 KiB escape tables in `text.rs`/`xml.rs`, intermediate `Vec<FileRecord>` buffers, and trie display caches.

## [0.2.2]

### Added

- `BoundedEstimatingWriter`: Enforces `MAX_OUTPUT_SIZE` across rendered framing, tree, metadata, and content bytes while estimating tokens in one pass.

### Changed

- Simplified local walk to a sequential 2-step pipeline (`ignore::WalkBuilder` harvest then bounded reads), removing channels and worker pools in `src/shell/local.rs`.
- Changed `DigestBudget::skip` to return `()`, removing dead error plumbing in `Selector::decide`.
- Switched file path sorting in `text::render` and `xml::render` to in-place `sort_unstable_by`.
- Used `trim_ascii()` for `--ref` and GitHub token parsing in `cli.rs` and `github_client.rs`.
- Used `get_mut` pre-lookups in `CandidateBudget::add` (`src/core/decide.rs`) to avoid key allocations for known directories.

### Removed

- `Collector` and `AnyCollector` in `src/shell/collector.rs`, routing directly via `orchestrate::ingest`.

## [0.2.1]

### Added

- `RunConfig` (`src/core/model.rs`) consolidating `source`, `output`, and `format`.
- `Collector` trait and `AnyCollector` enum (`src/shell/collector.rs`) wrapping `LocalCollector` and `GitHubCollector`.
- Zero-dependency benchmark suite (`benches/bench.rs`) for `BudgetTrie`, `render_digest_to_writer`, `TokenEstimatingWriter`, and `Selector::decide`.
- Library target (`src/lib.rs`) exposing core entry points, `RunConfig`, and `#[doc(hidden)] pub mod __bench`.
- `TempDirGuard` (`src/shell/mod.rs`) with path canonicalization for test fixtures.

### Fixed

- Mapped Unix open flags (`O_DIRECTORY`, `O_NOFOLLOW`) via `rustix::fs::OFlags`.
- Made sensitive path prefix matching (`.env`, `id_rsa`, `id_ed25519`, `credentials`) case-insensitive in `src/core/policy.rs`.
- Restricted HTTP retries in `src/shell/github_client.rs` to transient errors (`Io`, `Timeout`, `ConnectionFailed`).
- Removed non-verbatim path fallback in Windows `verify_beneath_root`, enforcing canonical roots.

### Changed

- Migrated Unix file/directory opening (`src/shell/local_io.rs`) to `rustix::fs`; unified ingestion under `orchestrate::ingest(&RunConfig)` and embedded `format: DigestFormat` in `CollectionResult`.
- Scoped GitHub authorization headers to exact-origin `https://api.github.com` (`src/core/github_url.rs`); hardened atomic output with RAII `AtomicFile` (`src/shell/output.rs`).
- Restructured `Source` as an ADT embedding `reference: Option<String>` in `Source::GitHub`; encapsulated budget metrics into `ContentMetrics` (`src/core/digest/mod.rs`).
- Optimized rendering and budgets: unified XML escaping/metrics under `XmlChunks<'a>` with ASCII lookup tables (`src/core/digest/xml.rs`); generalized `DigestFormat::render`, `text::render`, and `xml::render` over `IntoIterator<Item = &'a FileRecord>`; bounded `largest_files` to top-5 linear insertion; optimized `BudgetTrie::insert` and `BudgetTrieNode` (`Cow<'_, str>`, sparse `Option<String>`, `find_child`, `TREE_DEPTH_BYTE_FACTOR = 6`, `TREE_NODE_BASE_OVERHEAD = 12`, dropped `node_count`/`display_bytes`); made `is_retryable_status`, `retry_delay`, `DigestFormat::escape_tree_xml`, and `num_digits` (`extra_metadata_bytes`) `const fn`; centralized `FormatFrameMetrics::{TEXT, XML}`; used `std::mem::take` in `TokenEstimatingWriter`; used `extract_dir_slice` and `get_mut` in `CandidateBudget::add`.
- Refactored local traversal and tests: consumed candidates by value in `src/shell/local.rs` with persistent scoped workers; eliminated allocations in Windows `is_strictly_beneath` (`src/shell/local_io.rs`); synced largest-directory overflow docs in `README.md` and `context.md`; consolidated unit tests from 122 to 85 using table-driven fixtures and in-memory budget checks.

### Removed

- Manual Unix FFI module `src/shell/sys.rs`.

## [0.2.0]

### Added

- Single-request GitHub tarball streaming (`/repos/{owner}/{repo}/tarball/{ref}`) via `flate2` and `tar`, extracting commit SHAs from `pax_global_header`.
- `ARCHIVE_MAX_SECS` (300 s) policy limit and direct Unix FFI bindings (`src/shell/sys.rs`).

### Changed

- Split HTTP timeouts into 30 s connection and 300 s body streaming limits.

### Removed

- `serde`, `serde_json`, and `libc` dependencies; GitHub Git Trees recursive blob pipeline; `MAX_JSON_SIZE` limit.

## [0.1.2]

### Fixed

- Included depth indentation (`6 * depth + 12 + display.len()`) in `DigestBudget` tree upper bound to prevent 20 MiB overflows.
- Enforced Windows `GetFinalPathNameByHandleW` root containment against junctions/symlinks.
- Replaced linear `Vec` scans in `CandidateBudget` directory counting with `HashMap` lookups.

### Changed

- Documented plain-text raw preservation vs. XML 1.0 control-character sanitization and path-based sensitive file filtering.

## [0.1.1]

### Added

- Order-independent positional `<SOURCE>`, short flags (`-o`, `-f`, `-r`, `-h`), `--key=val`/`-k=val` syntax, and POSIX `--` delimiter.

### Changed

- Simplified CLI parser to linear traversal; made `DigestBudget` and `BudgetTrie` error-terminal; used `std::thread::scope` for GitHub blob fetches; replaced SWAR null scan with `std`; hardened Unix symlink and hard-link checks.

## [0.1.0] - 2026-10-11

### Added

- Initial CLI converting local directories (`.gitignore`-aware, skipping hidden files and symlinks) and GitHub repositories (`--ref <BRANCH|TAG|COMMIT>`, no local `git`) into plain-text or XML digests (`--format <text|xml>`) written atomically to files or stdout (`--output -`), with streaming token estimation, sensitive/lockfile filtering (`.env*`, `*.key`, `*.pem`, `id_rsa*`), and candidate/output size budgets.

[0.2.4]: https://github.com/but212/codefeed/compare/v0.2.3...v0.2.4
[0.2.3]: https://github.com/but212/codefeed/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/but212/codefeed/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/but212/codefeed/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/but212/codefeed/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/but212/codefeed/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/but212/codefeed/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/but212/codefeed/releases/tag/v0.1.0
