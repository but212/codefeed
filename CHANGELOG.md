# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Promoted `percent-encoding`, `memchr`, and `log` from `Cargo.lock` to explicit direct dependencies in `Cargo.toml`.
- Unified token estimation state machine `TokenEstimator` in `src/core/digest/mod.rs`, shared by `estimate_tokens` and `BoundedEstimatingWriter`.
- Encapsulated collection completion in `Selector::finish` in `src/core/decide.rs`, constructing `CollectionResult` directly.
- Instrumented transient transport error and retryable HTTP status logging in `src/shell/github_client.rs` via `log::warn!`.

### Changed

- Replaced manual 16-hex byte percent-encoding loop in `src/core/github_url.rs` with `percent_encoding::utf8_percent_encode`.
- Accelerated `text_content` in `src/core/decide.rs` by checking NUL (`\0`) bytes via `memchr::memchr` before UTF-8 decoding.

### Removed

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
