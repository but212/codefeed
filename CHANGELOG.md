# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

### Changed

- **Policy & Limits**:
  - Centralized operational constants and limits (`MAX_ATTEMPTS`, `RETRY_INITIAL_DELAY_MS`, `RETRY_MAX_DELAY_MS`, `GITHUB_TIMEOUT_SECS`, `MAX_PATH_DEPTH`) into `src/core/policy.rs` as single source of truth.
  - Renamed `MAX_RETRIES` to `MAX_ATTEMPTS` to accurately reflect total attempt budget.
  - Added `MAX_PATH_DEPTH` limit (256 directory levels) enforced during tar archive normalization for valid candidate paths.
- **Candidate Pipeline & Ingestion**:
  - Unified candidate entry recording into `record_observation`, standardizing local filesystem walk and archive entry ingestion while preserving lazy file body evaluation.
  - Streamlined collection loops to delegate candidate slot reservation and deferred reading, preventing unnecessary I/O when candidate budget overflows.
  - Preserved valid directory prefixes for non-UTF-8 path candidates, routing them directly through candidate accounting as skipped entries.
  - Guaranteed deterministic path alignment across tree, header, and body outputs via stable sorting in `FileStorage`.
  - Enforced `GitRef` validation rejecting `.` and `..` segment references.
  - Added explicit directory path context to local traversal I/O error diagnostics.
- **Render Engine**:
  - Simplified digest rendering signatures (`render_text`, `render_xml`, `DigestFormat::render`) to standard `std::io::Result<()>`, routing size cap checks strictly through `written > MAX_OUTPUT_SIZE`.
  - Optimized directory hierarchy grouping in ASCII tree rendering using slice matching, eliminating redundant path scans.
  - Streamlined stream escaping through a unified `write_escaped` helper with dedicated escape rule predicates (`escape_header`, `escape_xml_body`, `escape_xml_single_line`).

### Fixed

- **Atomic File Delivery**: Replaced temporary file handling in `src/shell/output.rs` with `AtomicFile` and an inner `TempPathGuard` RAII guard, guaranteeing cleanup on unexpected I/O errors or panic unwinding.
- **Local Walk Error Kind**: Preserved the inner `std::io::ErrorKind` when formatting traversal errors in `src/shell/local.rs` rather than wrapping in `io::ErrorKind::Other`.

### Internal

- **State Machine Consolidation**:
  - Refactored CLI argument parsing into a single-pass token state machine (`ParseState`) in `src/core/cli.rs`, eliminating nested iterator overhead while maintaining flag validation and error precedence rules.
  - Refactored path segment classification into a single-pass state machine (`PathClassifierState`) in `src/core/policy.rs`, integrating noise and sensitive filtering while strictly preserving tar entry traversal (`..`) error precedence.
  - Consolidated HTTP retry orchestration into a deterministic state machine (`AttemptState::step`) across core and shell boundaries.
- **Diagnostics & Scope**:
  - Unified `output_too_large_error` diagnostic construction across early memory guards and streaming serializers.
  - Privatized `CandidateBudget` visibility to `src/core/selector.rs` internal visibility.

## [0.3.0]

### Added

- **CLI Dispatch Entry Point**: Added `codefeed::run_cli` (`src/shell/mod.rs`) encapsulating CLI parsing, help output, ingestion, and stdio reporting.
- **Domain Types & Validation**:
  - Added [`GitRef`] enforcing non-empty, non-whitespace Git revision invariants with zero-copy `&str` access.
  - Added [`Source::github`] validating repository owner/name characters, non-emptiness, and traversal segments.
  - Added [`PathStatus`] and domain path classifier [`classify_path`] (`src/core/policy.rs`) unifying hidden segment checks, noise files, and sensitive pattern filtering.
  - Added `is_hidden_segment` (`src/core/policy.rs`) checking raw segment bytes for leading dot as the single source of truth for hidden paths across local traversal and tar archives.
- **Candidate Token**: Added [`CandidateSlot`] (`src/core/selector.rs`) linear proof token demonstrating candidate budget reservation prior to content acquisition.
- **Documentation & Quality**:
  - Added `#![warn(unreachable_pub)]` and `#![warn(missing_docs)]` to `src/lib.rs`, documenting all re-exported public API items with doctests and error contracts.
  - Added Rustdoc and comment conventions in `docs/coding_convention.md`.

### Changed

- **Breaking Changes (Library API)**:
  - **Source::Github Encapsulation**: Marked `Source::Github` `#[non_exhaustive]` (construct via `Source::github` or `Source::parse`); updated `Source::Github.reference` and `Source::with_reference` from `Option<String>` to `Option<GitRef>`.
  - **Public API Scoping**: Demoted internal items (`CliAction`, `help_text`, `parse`, and internal `IngestError` constructors) to `pub(crate)`, retaining `IngestError::usage` and `codefeed::run_cli`; marked `IngestError` and `IngestReport` as `#[non_exhaustive]`.
- **CLI & Output Resilience**:
  - **Stdout BrokenPipe Resilience**: Replaced unbuffered prints with locked stdout writes in `run_cli`, exiting cleanly with code 0 instead of panicking on closed downstream pipes.
  - **Atomic Stdout Delivery**: Buffered stdout up to `MAX_OUTPUT_SIZE` before emission so overflow errors write zero bytes, matching atomic file rollback semantics.
  - **Symlink Output Preservation**: Checked `symlink_metadata` in output target resolution to avoid canonicalizing existing leaf symlinks onto source files.
- **Performance & Streaming Ingestion**:
  - **Pre-I/O Candidate Budget Reservation**: Enforced candidate budget checks before reading local file bodies or archive entry bytes, aborting immediately with 0 bytes read upon exceeding `MAX_FILE_COUNT`.
  - **Single-Pass Path Normalization & Policy Filtering**: Integrated hidden-segment, noise, and sensitive credentials filtering into a single pass in local traversal and archive normalization.
  - **Monotonic Byte-Offset Tree Rendering**: Replaced depth-indexed re-traversal in ASCII tree rendering with monotonic byte-offset slicing, eliminating redundant prefix scans.
  - **Zero-Allocation Tree & Storage**: Streamed directory tree rendering directly from sorted `&FileStorage` with component-wise ordering (`cmp_path`).
  - **XML Attribute & Control Escaping**: Unified `<directory_structure>` and `<file path="...">` escaping, properly encoding XML 1.0 whitespace character references (`&#9;`, `&#10;`, `&#13;`) and sanitizing invalid control characters.

### Removed

- **Legacy APIs**: Completely removed test-only `CandidateInput`, `Selector::feed`, and deprecated `is_ignored_path` / `is_hidden_path`.
- **Unused Conversions & Wrappers**: Removed unused conversion traits (`TryFrom<&str>`, `FromStr`, etc.) from `GitRef`; removed single-caller capacity wrappers (`CandidateBudget::with_capacity`, `Selector::with_capacity`, `FileStorage::with_capacity`).

## [0.2.4]

### Added

- `CandidateInput` (`src/core/selector.rs`): Unified candidate enum (`File`, `TooLarge`, `Skipped`) across the FCIS boundary.
- `render_tree` (`src/core/tree.rs`): Deterministic ASCII tree rendering from sorted path slices.
- `resolve_output_target` (`src/shell/output.rs`): Single-pass output target validation and parent canonicalization.
- `policy::is_ignored_path` (`src/core/policy.rs`): Direct path exclusion for noise lockfiles, sensitive prefixes/extensions, and hidden components.

### Changed

- **CLI & Output Handling**:
  - Replaced `clap` and `clap_derive` dependencies with a ~50-line pure `std` parser preserving all CLI contracts and flags.
  - Replaced output target re-checks with single startup canonicalization and parent resolution (`resolve_output_target`).
- **Tree & Escaping**:
  - Replaced `BudgetTrie` with direct ASCII tree rendering from sorted `FileStorage`.
  - Replaced chunked loops with linear slice iterators and streamed XML attributes via `write_escaped`.
- **Candidate Ingestion & Memory**:
  - Replaced `Cow` in `CandidateInput` with borrowed slices; replaced RLE directory tracking with `HashMap<String, usize>`.
  - Streamed candidates directly into `Selector::feed`, eliminating intermediate probe enums.
  - Detected binary files via UTF-8/NUL byte inspection during body read, removing static binary extension lists.
- **Diagnostics & Error Handling**:
  - Replaced 220-line `TokenEstimator` with pure `estimate_tokens(bytes)` heuristic.
  - Made `retry_delay(attempt)` pure and deterministic.
  - Unboxed `IngestError` payloads.

### Removed

- External dependencies `clap` and `clap_derive`.
- Benchmark binaries and dead modules (`src/core/decide/`, `src/shell/local_io.rs`, `src/shell/github_client.rs`, `benches/bench.rs`).
- Obsolete types: `BudgetTrie`, `BudgetTrieNode`, `OutputFilter`, `Item`, `Probe`, `CompactDir`, `BINARY_EXTENSIONS`.

## [0.2.3]

### Added

- **Storage Arena & Models**:
  - `FileStorage` (`src/core/model.rs`): Contiguous path/content arenas indexed by 16-byte `FileSpan` with `FileRecord<'a>` views and in-place `sort_by_path`.
  - `CompactDir` and `CompactDir::as_bytes` (`src/core/decide.rs`): 24-byte inline directory descriptor for candidate budgets.
  - `ItemPath<'a>` (`src/core/decide.rs`): `Cow<'a, str>` wrapper for zero-copy paths across core/shell boundaries.
  - `Source::with_reference` (`src/core/model.rs`): Revision and source invariant validation.
- **Pipeline & Digest Builders**:
  - `CandidateBudget::check_limit` & `Selector::finish` (`src/core/decide.rs`): O(1) batch pre-flight candidate check and direct `CollectionResult` builder with skipped-byte tracking.
  - `TokenEstimator` (`src/core/digest/token.rs`): Non-destructive token state machine backed by a 256-byte `CHAR_METRIC` table.
  - Streaming shell abstractions: `SourceStream`, `LocalStream`, and `GithubStream`.

### Changed

- **Tree & Ingestion**:
  - `BudgetTrie` (`src/core/digest/trie.rs`): Rebuilt as a 16-byte POD node arena (`Vec<BudgetTrieNode>`) with UTF-8 truncation at 65,535 bytes.
  - Single-pass path classification and hidden segment scans during local and tarball ingestion.
  - Unified HTTP retries, 64 KiB tarball buffering, and `RedirectAuthHeaders::Never`.
- **Output & Security**:
  - `OutputFilter` & `normalize_path` (`src/shell/output.rs`): Zero-syscall lexical matching with parent-canonical normalization.
  - Escaped XML control codes and rejected embedded NUL bytes in tar entry paths.

### Fixed

- Fixed CLI parsing mapping `-- - -o -` to stdout instead of a file named `"-"`.
- Eliminated double-flush on atomic files and preserved `estimated_tokens` on `BrokenPipe`.
- Rejected directory output targets with `io::ErrorKind::IsADirectory` in `AtomicFile::create`.

### Removed

- Legacy iterator implementations on `LocalStream` and `GithubStream`.
- Crates `rustix`, `percent-encoding`, `memchr`, and `log`.
- Obsolete types, escape tables, and intermediate `Vec<FileRecord>` allocations.

## [0.2.2]

### Added

- `BoundedEstimatingWriter`: Enforces `MAX_OUTPUT_SIZE` across rendered framing, tree, metadata, and content bytes while estimating tokens in one pass.

### Changed

- Simplified local walk to a sequential 2-step pipeline (`ignore::WalkBuilder` harvest then bounded reads), removing channels and worker pools.
- Switched file path sorting in `text::render` and `xml::render` to in-place `sort_unstable_by`.
- Used `trim_ascii()` for `--ref` and GitHub token parsing.

### Removed

- `Collector` and `AnyCollector` in `src/shell/collector.rs`, routing directly via `orchestrate::ingest`.

## [0.2.1]

### Added

- `RunConfig` (`src/core/model.rs`) consolidating `source`, `output`, and `format`.
- `Collector` trait and `AnyCollector` enum wrapping `LocalCollector` and `GitHubCollector`.
- Zero-dependency benchmark suite for trie insertion, rendering, and candidate selection.
- Library target (`src/lib.rs`) exposing core entry points and `RunConfig`.

### Fixed

- Made sensitive path prefix matching case-insensitive in `src/core/policy.rs`.
- Restricted HTTP retries in `src/shell/github_client.rs` to transient errors (`Io`, `Timeout`, `ConnectionFailed`).
- Enforced canonical roots against non-verbatim paths in Windows path verification.

### Changed

- Scoped GitHub authorization headers to exact-origin `https://api.github.com`.
- Hardened atomic output delivery with RAII `AtomicFile`.
- Restructured `Source` as an ADT embedding `reference: Option<String>` in `Source::GitHub`.
- Bounded `largest_files` diagnostic selection to top-5 linear insertion.

### Removed

- Manual Unix FFI module `src/shell/sys.rs`.

## [0.2.0]

### Added

- Single-request GitHub tarball streaming (`/repos/{owner}/{repo}/tarball/{ref}`) via `flate2` and `tar`, extracting commit SHAs from `pax_global_header`.
- `ARCHIVE_MAX_SECS` (300 s) policy limit.

### Changed

- Split HTTP timeouts into 30 s connection and 300 s body streaming limits.

### Removed

- `serde`, `serde_json`, and `libc` dependencies; GitHub Git Trees recursive blob pipeline; `MAX_JSON_SIZE` limit.

## [0.1.2]

### Fixed

- Included depth indentation in `DigestBudget` tree upper bound to prevent 20 MiB overflows.
- Enforced Windows `GetFinalPathNameByHandleW` root containment against junctions and symlinks.
- Replaced linear `Vec` scans in `CandidateBudget` directory counting with `HashMap` lookups.

### Changed

- Documented plain-text raw preservation vs. XML 1.0 control-character sanitization and path-based sensitive file filtering.

## [0.1.1]

### Added

- Order-independent positional `<SOURCE>`, short flags (`-o`, `-f`, `-r`, `-h`), `--key=val`/`-k=val` syntax, and POSIX `--` delimiter.

### Changed

- Simplified CLI parser to linear traversal; made `DigestBudget` and `BudgetTrie` error-terminal; hardened Unix symlink and hard-link checks.

## [0.1.0] - 2026-10-11

### Added

- Initial CLI converting local directories (`.gitignore`-aware, skipping hidden files and symlinks) and GitHub repositories (`--ref <BRANCH|TAG|COMMIT>`, no local `git`) into plain-text or XML digests (`--format <text|xml>`) written atomically to files or stdout (`--output -`), with streaming token estimation, sensitive/lockfile filtering (`.env*`, `*.key`, `*.pem`, `id_rsa*`), and candidate/output size budgets.

[Unreleased]: https://github.com/but212/codefeed/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/but212/codefeed/compare/v0.2.4...v0.3.0
[0.2.4]: https://github.com/but212/codefeed/compare/v0.2.3...v0.2.4
[0.2.3]: https://github.com/but212/codefeed/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/but212/codefeed/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/but212/codefeed/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/but212/codefeed/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/but212/codefeed/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/but212/codefeed/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/but212/codefeed/releases/tag/v0.1.0
