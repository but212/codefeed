# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

### Fixed

- **GitHub rate-limit reset**: `x-ratelimit-reset` produces a retry delay only when `x-ratelimit-remaining` is `0`. Previously a 404 reported "retry available" and a 5xx response carrying reset headers was not retried (`src/shell/github.rs`).
- **Unlinked local files**: A file deleted after directory discovery but before its metadata check is counted as skipped, matching a deletion at file-open time. Previously it was dropped without being counted (`src/shell/local.rs`).
- **Unreadable local files**: A file that cannot be opened (`PermissionDenied`, or a Windows sharing violation from another process holding it) is counted as skipped instead of aborting the run, matching `docs/security_and_filtering.md`. Unreadable directories still fail fast (`src/shell/local.rs`).

### Changed

- **Dash-prefixed inline CLI values**: `--flag=value` accepts a value starting with `-`, such as `--output=-notes.txt`. Separate-argument values still reject a leading `-` except `-o -` for stdout, and `-o -x` still fails (`src/core/cli.rs`).

### Performance

- **WebPKI Root Certificate Verification**: Switched `ureq` TLS roots from platform verifier to compiled Mozilla roots (`rustls-webpki-roots`), removing Windows CryptoAPI (`CertGetCertificateChain`) traversal overhead and trimming OS-specific C-bindings from dependencies.
- **Archive Stream Decompression Buffering**: Buffered incoming archive stream ahead of `GzDecoder` in `codefeed::shell::archive::collect` with a tuned 64 KiB buffer (`ARCHIVE_BUFFER_BYTES`), smoothing chunked decompression I/O.
- **Parallel Directory Traversal and Reading**: Parallelized local directory walking, metadata inspection, and bounded content reading in `codefeed::shell::local::collect` using `ignore::WalkParallel` with worker channels and thread scoping, preserving zero-copy arena storage, non-symlink security checks, and deterministic lexicographical ordering.
- **Metadata and I/O Deduplication**: Deduplicated local file metadata queries in `codefeed::shell::local::collect`, replacing the triple `entry.metadata()`, `symlink_metadata()`, and `file.metadata()` sequence with a single `fs::symlink_metadata()` check per valid candidate, eliminating redundant post-open stat calls and pre-reserving read buffer capacity up to bounded file limits.

### Internal

- **Archive Stream Decoupling**: Extracted tarball stream decompression, PAX commit SHA parsing, and entry admission into a dedicated `codefeed::shell::archive` module, separating HTTP transport concerns in `codefeed::shell::github` from streaming tar entry processing.
- **Content Buffers**: Local and archive file-content buffers are plain `Vec<u8>`. `Zeroizing` remains only for the GitHub token and error bodies.
- **Local Read Helper**: `read_bounded` returns `Result<Option<Vec<u8>>>` (`None` for an unavailable file) instead of filling an out-parameter buffer. The test-only `read_local_file_into` duplicate was removed, and its tests exercise `collect`. Behavior is unchanged (`src/shell/local.rs`).
- **Dead Code**: Removed a redundant directory check in `src/shell/output.rs` and an unused `AsRef<str>` impl for `NormalizedPath` in `src/core/policy.rs`.
- **Retry Header Decision**: Rate-limit headers are parsed once per failed GitHub response into `RateLimitHeaders` (`src/core/github.rs`). The retry delay and rate-limited flag are pure functions of the parsed values and an explicit `now_secs`, so their tests no longer depend on the wall clock. Behavior is unchanged.
- **Local Worker Message**: `WorkerMsg` has three variants, `NonUtf8(prefix)`, `Regular { path, size, content }`, and `Error`. Every candidate goes through `Selector::observe`, and impossible combinations such as non-UTF-8 paths with content are no longer representable. The path-length and size gate is one core predicate, `exceeds_size_limits`, shared by admission and the local worker. The duplicate `LocalVisitorBuilder` is merged into `LocalVisitor`. Behavior is unchanged.
- **Render Dispatch**: `CollectionResult::render` matches `DigestFormat` directly and calls `render_with`. The `DigestFormat::render` method, the `render::render` free function, and the forwarding method on `CollectionResult` were removed. The overflow diagnostic is built once, in `FileStorage::output_too_large_error`.
- **Single Candidate Entry**: Local and archive collectors settle each candidate through one core method, `Selector::observe(status, size_hint, read)`, which reads the body only after admission. `Admission` and `PendingEntry` are now private to `selector.rs`, so shell code no longer matches on them. Behavior is unchanged.
- **Sorted View**: `SortedFiles` derefs to `FileStorage` for read-only access instead of re-declaring its getters. Sortedness is still guaranteed by `SortedFiles::new`.
- **Scratch Buffers**: Local content reads pre-size the buffer to `min(file length, MAX_FILE_SIZE + 1)`, and `BoundedBuffer::push_usize` formats through `to_string()`. Output is unchanged.
- **Path Normalization**: `classify_path_segments` drops empty and `.` segments itself, so every `NormalizedPath` is free of them regardless of caller; the archive no longer pre-filters, and two unreachable branches are removed. Behavior is unchanged (`src/core/policy.rs`, `src/core/archive.rs`).
- **GitHub Attempt**: `Attempt::from_result`, `Attempt::warn`, and `Attempt::into_error` hold response classification, retry warnings, and error construction, and `Attempt::Status` no longer duplicates `is_rate_limit`. `GithubClient::request` attaches the origin-gated headers. Behavior is unchanged (`src/shell/github.rs`).
- **GitHub Retry Transition**: `AttemptState::retry` builds the next state and `Transition::Retry` in one place, and `AttemptState::step` only decides which delay applies. Behavior is unchanged (`src/core/github.rs`).
- **Output Parent**: `parent_dir` replaces three copies of the parent-or-`.` expression in `src/shell/output.rs`. Behavior is unchanged.
- **Completion Message**: `completion_message` moved from `src/shell/mod.rs` to `src/core/cli.rs`, since it is pure formatting. `run_cli` only chooses the stream. Text is unchanged.
- **CLI Parse Accumulator**: `cli::parse` folds flag values and the source positional through a consuming `Parsed` accumulator. One function owns the "unexpected argument" check and one owns the "a value is required" message. Accepted arguments and error text are unchanged (`src/core/cli.rs`).
- **Archive Entry Loop**: `shell/archive.rs` handles the PAX header inside the entry loop and returns `Selector` from `process_tar_entry`, replacing the out-parameter and tuple return. The separate bounded-read helper was inlined. Revision and skip behavior are unchanged.
- **GitHub Error Text**: `api_error`, `sanitize_error_detail`, `status_retry_warning`, `transport_retry_warning`, and `transport_error` moved from `src/shell/github.rs` to `src/core/github.rs`. The shell keeps HTTP calls, body reads, `eprintln!`, and sleeps. Message bytes are unchanged.
- **Selector Constructor**: `Selector::new` builds the empty state directly, and the separate `impl Default for Selector` was removed. The HashMap capacity is unchanged.
- **Test Tables**: Selector tests drive `Selector::observe` only. The `feed_file` and `feed_skipped` helpers were removed, and their cases are now rows in `observe_counts_candidates_by_content_outcome` and `observe_does_not_read_settled_candidates`, plus the new `observe_enforces_candidate_budget_before_read`. The retry decision test and archive path normalization test are tables. The vacuous capacity assertion in the local read test was dropped.
- **Tests**: Removed `test_archive_buffer_bytes_invariant`, which restated the constant; `ARCHIVE_BUFFER_BYTES > 0` stays a compile-time assertion.

### Removed

- **Breaking (library API)**: `GitRef::as_str` and `impl Display for GitRef` are removed. `GitRef` derefs to `str`, so use `&*git_ref` (`src/core/model.rs`).
- **Unused Display**: `impl Display for NormalizedPath` is removed. It had no production caller (`src/core/policy.rs`).
- **Build config**: The commented-out `[profile.release]` stanza is removed from `Cargo.toml`.

### Documentation

- **Retry Policy**: `README.md`, `context.md`, and `docs/security_and_filtering.md` now state that rate-limited 403 responses are retried.
- **Ingestion Order**: Docs now state that local workers read file bodies before the main thread calls `Selector::admit`; GitHub archive entries are admitted before reading.
- **Overflow Diagnostics**: Docs now state that overflow diagnostics cover only candidates admitted before abort, so local counts can vary between runs.
- **Inline CLI Values**: `README.md` documents that `--flag=value` accepts values starting with `-`.

## [0.3.2]

### Security

- **Memory zeroization**: Applied `zeroize` in `src/shell/` to GitHub tokens during env lookup, HTTP error bodies, and reusable read buffers in local and tarball ingestion.
- **Sensitive patterns**: Added `id_ecdsa*`, `id_dsa*`, and `*.pfx` (PKCS#12) to sensitive exclusions in `src/core/policy.rs`.

### Changed

- **Lockfile exclusions**: Added `bun.lock`, `bun.lockb`, `poetry.lock`, `Pipfile.lock`, `composer.lock`, and `Gemfile.lock` to conventional noise lockfiles (`NOISE`) in `src/core/policy.rs`.
- **Path depth limit**: Local ingestion rejects paths exceeding `MAX_PATH_DEPTH` (256 levels) with `IngestError::invalid_metadata`, matching archive handling while preserving hidden/noise precedence.
- **Diagnostic ordering**: `largest_files` tie-breaks equal file sizes with `cmp_path` (component-wise lexicographical order), matching tree ordering.

### Fixed

- **`GitRef` whitespace**: `GitRef::new` trims ASCII whitespace before storage, preventing `%20` in tarball URLs.
- **Hidden directory filtering**: Pruned local directories via `WalkBuilder::filter_entry` using `is_hidden_segment` (preserving root at `depth == 0`).
- **Root non-UTF-8 diagnostics**: Mapped root non-UTF-8 entries (`""` prefix) to `<root>/` instead of `<unknown>/` on candidate budget overflow.

### Internal

- **Pipeline typestates**: Refactored core collection into a consuming chain (`Selector` -> `CollectionResult` -> `RenderedDigest` -> `IngestReport`), where `CollectionResult::render` consumes the collection by value with metrics.
- **Output transport**: Replaced `output::emit` with pure I/O `output::deliver(target, bytes)`, removing render format and collection dependencies from `src/shell/output.rs`.
- **Policy matching**: Consolidated prefix checks in `src/core/policy.rs` into `SENSITIVE_PREFIXES` and lazy-evaluated extension extraction in `is_sensitive_file`.
- **Tree render**: `render_level` threads a single owned `prefix` buffer (`push_str`/`truncate`) instead of cloning per directory.
- **Performance**: Replaced per-entry `.zeroize()` in ingestion loops with `Zeroizing` RAII `Drop` cleanup, avoiding redundant volatile overwrites during streaming.
- **Documentation**: Documented transitive dependency rules in `AGENTS.md`, `.env*` exclusion rationale, and component-wise `cmp_path` ordering invariant in `context.md` and `storage.rs`.

## [0.3.1]

### Changed

- **Token estimate**: now `ascii_bytes / 4 + non_ascii_chars` (minimum 1). `estimated_tokens` is closer for Korean and CJK text, and values change for any non-ASCII input.
- **GitHub retries**:
  - HTTP 403 retries only when rate-limited (`Retry-After` present or `x-ratelimit-remaining: 0`).
  - `Retry-After` and `x-ratelimit-reset` are honored. Retries stop immediately if the wait exceeds `RETRY_MAX_DELAY_MS`.
  - API errors include the remaining wait time.
- **Path limits**: tar entries deeper than 256 directory levels (`MAX_PATH_DEPTH`) are rejected.
- **`GitRef`** rejects `.` and `..` segments.
- **Non-UTF-8 paths** keep their top-level directory prefix and count as skipped.
- **Output ordering** is deterministic across tree, header, and body.
- **Error messages**: a tar `.` root segment now reports `root segment is '.'` instead of directory traversal. Local traversal errors include the directory path.

### Fixed

- **Atomic file output**: temp files are cleaned up via an RAII guard on error or panic, on both Windows and Unix. Unix commits sync the parent directory, and overwrites keep existing file permissions.
- **Local walk errors** keep the original `io::ErrorKind` instead of `Other`.

### Internal

- **Admission**: `Ignored` and `Skipped` merged into `Settled(Selector)`. `PendingEntry` must be consumed by `accept` or `skip`, enforced at compile time. Removed `record_entry`, `record_with`, `CandidateSlot`, `Observation`, and `ContentResult`. Shell collectors read into reusable scratch buffers. `CandidateBudget` is private to `selector.rs`.
- **Pure core**: no `&mut` parameters in `src/core/` (except `Display::fmt`). Selector, storage, and retry state use value-passing `self`; `AttemptState::step` returns `(Self, Transition)`. CLI parsing uses `ParseState`, and path classification uses `PathClassifierState`. `classify_path_segments` and `normalize_tar_entry_path` return owned `PathStatus`. Rendering writes into an in-memory `BoundedBuffer` returning `RenderedDigest`; physical writes moved to `shell/output.rs`.
- **Storage**: `Deref` removed from `SortedFiles` in favor of explicit accessors. `largest_files` uses stable `sort_by_key` and `truncate(5)`. `FileStorage::push` uses `expect` on `u32` conversions.
- **Render**: `render_text`, `render_xml`, and `DigestFormat::render` return `std::io::Result<()>`, and the size cap is `written > MAX_OUTPUT_SIZE`. ASCII tree grouping uses slice matching. Escaping goes through `write_escaped` with `escape_header`, `escape_xml_body`, and `escape_xml_single_line`. `output_too_large_error` is unified.
- **Policy**: limits centralized in `policy.rs`. `MAX_RETRIES` renamed to `MAX_ATTEMPTS`.
- **Shell**: retry loop uses an `Attempt` enum in place of the inverted `Result<ureq::Error, Response>`. `MockServer` wraps the test listener. `format_completion_message` is extracted and tested for completion reporting and exit codes. Removed redundant `#![cfg(test)]` in `testing.rs`.

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

[0.3.2]: https://github.com/but212/codefeed/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/but212/codefeed/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/but212/codefeed/compare/v0.2.4...v0.3.0
[0.2.4]: https://github.com/but212/codefeed/compare/v0.2.3...v0.2.4
[0.2.3]: https://github.com/but212/codefeed/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/but212/codefeed/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/but212/codefeed/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/but212/codefeed/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/but212/codefeed/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/but212/codefeed/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/but212/codefeed/releases/tag/v0.1.0
