# Domain Context — codefeed

## Purpose

Convert a local directory or public GitHub repository into a single LLM-friendly text digest. Public behavior, limits, and CLI contracts are documented in `README.md`; coding standards in `docs/coding_convention.md`; this document fixes domain terms, invariants, and the functional-core / imperative-shell boundary.

## Entities and value objects

- `Source` — `Local(PathBuf)` or `Github { owner, repo, reference }` (`#[non_exhaustive]`, constructed via `Source::github`). Enforces owner/repo identifier validation and reference invariants via `GitRef`.
- `GitRef` — Validated non-empty Git revision reference (branch, tag, or commit SHA), rejecting empty and whitespace-only strings.
- `RunConfig` — Execution configuration unifying `source`, `output`, and `format`.
- `CandidateSlot<'a>` — Linear proof token demonstrating a candidate slot was reserved in `CandidateBudget` for `path: &'a str`. `Selector::reserve_candidate` validates candidate counts before reading body bytes; `Selector::record_content` consumes the slot to validate UTF-8 text and NUL-free content (updating storage when valid text, or incrementing `skipped` when absent, oversized, or binary); `Selector::record_non_utf8` accounts for non-UTF-8 paths directly. Shell collectors filter non-regular files (FIFOs, sockets, devices, symlinks) and noise/sensitive/hidden paths pre-candidate (`policy::classify_path`, `policy::is_hidden_segment`), map files unlinked/missing after discovery to `Skipped`, and abort unexpected I/O errors as `IngestError::Io`.
- `FileSpan` — POD indexing path and content byte spans via private offsets, accessed through borrowed slice methods (`path(&arena)`, `content(&arena)`).
- `FileStorage` — Append-only arena buffer (`path_arena`, `content_arena`, `spans`). Provides zero-copy `FileRecord<'a>` borrowed views throughout ingestion and in-place lexicographical sorting (`sort_by_path`) by swapping 16-byte `FileSpan` offsets without string reallocations.
- `FileRecord<'a>` — Zero-copy borrowed digest entry view: `path: &'a str`, `content: &'a str`.
- `CollectionResult` — Uniform format-neutral collection record: `label`, optional `revision`, `files: FileStorage`, `skipped`.
- `IngestReport` — Output summary report: `output`, `included_files`, `skipped_files`, `estimated_tokens` (`#[non_exhaustive]`).
- `OutputTarget` — Digest destination: `File(PathBuf)` or `Stdout`.
- `DigestFormat` — Output serialization format: `Text` or `Xml`.
- `CandidateBudget` — Candidate file counter with directory aggregation (`HashMap<String, usize>`) for limit tracking and overflow diagnostics.
- `Policy` — Centralized operational limits, thresholds, noise lists, and sensitive patterns (`src/core/policy.rs`). Single source of truth.

## Invariants

### Filtering & Accounting

- Pre-candidate exclusions: `.git`, hidden entries (empty path segments from redundant delimiters are filtered to prevent false positives), symlinks, non-UTF-8 paths/content, lockfiles (`Cargo.lock`, `package-lock.json`, `yarn.lock`, `pnpm-lock.yaml`, `go.sum`), and conventional sensitive files matched case-insensitively by prefix (`.env*`, `id_rsa*`, `id_ed25519*`, `credentials*`) and extension (`*.key`/`*.pem`, `*.p12`/`*.pkcs12`, `*.credentials`). Matches path conventions, not file content. Excluded paths consume neither candidate slots nor `skipped` counts.
- Candidate accounting: Consumes candidate slots before reading file body bytes or evaluating content, size, or encoding (`CandidateSlot`).
- `skipped` count: Tracks candidates rejected for content, size, or encoding (including binary files containing NUL bytes or invalid UTF-8). Ignored paths are never counted in `skipped`.
- Output bounding: Rejects content exceeding `MAX_OUTPUT_SIZE`; overflow content is never emitted.
- Diagnostic ranking: `largest_files` uses bounded Top-5 selection with linear position lookup, avoiding full-collection cloning or $O(N \log N)$ sorting while preserving insertion stability for tie-breakers.

### Rendering & Formats

- ASCII tree (`├── `, `└── `, `│   `): Renders deterministic directory structure directly from sorted `FileStorage` with monotonic byte-offset slicing (`render::render_tree`). Format-neutral; escapes control codes and XML entities to prevent tree line-break injections.
- Token estimation: Pure function `estimate_tokens(bytes)` calculates `(bytes / 4).max(1)` when non-zero, `0` when empty. Reported in `IngestReport.estimated_tokens` and completion messages (`~N tokens`). Calibrated for English text and code (~4 bytes/token); multibyte text (such as Korean/CJK) will be underestimated.
- Plain text (`--format text`): Preserves valid UTF-8 file content verbatim (rejecting NUL bytes) for LLM prompts; escapes headers and paths while leaving file bodies unmutated (interactive terminals may reflect ANSI escape codes contained in files).
- XML (`--format xml`): Emits `<repository>`, `<directory_structure>`, and `<files><file path="...">` tags, escaping XML entities (`&`, `<`, `>`, `"`, `'`), attribute whitespace (`\t`, `\n`, `\r` via character references), while preserving raw `\t`, `\n`, `\r` in `<file>` element bodies for LLM readability and token efficiency, and sanitizing disallowed XML 1.0 control characters to `\u{FFFD}`.
- Lexicographical ordering: File-path ordering is established in-place on `spans` via `FileStorage::sort_by_path()` during `Selector::finish()`. Format renderers consume `ExactSizeIterator<Item = FileRecord<'_>>` directly with zero heap re-allocation or re-sorting.
- Header format: Local digest headers use root directory basename (`Repository: <basename>`), omitting absolute paths and system prefixes.

### I/O & Output Target Contracts

- File target: Validates and resolves output path at startup (`resolve_output_target`). Writes atomically via RAII guard (`AtomicFile`) under the canonical parent directory, syncs to disk, and renames on commit; automatically unlinks temp files on error or panic.
- Stdout target: Buffers output into memory up to `MAX_OUTPUT_SIZE` and writes to stdout only on success, preventing partial emission on overflow.
- Completion messages: Emitted to stdout for file targets and stderr for stdout targets.
- Broken pipe: Downstream pipe closure (`BrokenPipe` / `EPIPE`) on stdout exits cleanly with status 0 (preserves estimated tokens on buffer flush; defaults to 0 on mid-stream write termination).
- Local walk exclusion: Excludes existing output files via canonical path comparison resolved once at startup (`resolve_output_target`), avoiding repeated syscalls during traversal. Assumes a quiescent directory tree during scan (see `docs/security_and_filtering.md`).
- Local walk pipeline:
  1. `ignore::WalkBuilder` with `follow_links(false)` and `.sort_by_file_name(|a, b| a.cmp(b))` traverses entries, classifies paths via `classify_local_path`, and reserves a candidate slot in `Selector` (`CandidateSlot`) before reading file bodies.
  2. Bounded sequential reads stream observations directly into `Selector` without intermediate path allocations.
- Size cap enforcement: `MAX_OUTPUT_SIZE` is strictly enforced on bytes written via `BoundedEstimatingWriter`, preventing framing/metadata overflow (both file and stdout targets emit nothing on overflow). Token count is estimated on the same writer stream pass over rendered byte length.
- CLI argument parsing: Linearly parsed into options and source in pure core (`core::cli`) using `std` only, supporting order-independent sources, short flags (`-o`, `-f`, `-r`, `-h`), key-value `=` syntax, and POSIX `--` delimiters without third-party CLI dependencies. CLI execution, help display, and output reporting are encapsulated via `shell::run_cli`.
- Candidate budget tracking: Uses standard `HashMap<String, usize>` during collection, with deterministic overflow diagnostics sorted by count descending then directory ascending.
- Digest budget tracking: Tracked directly in `Selector` and error-terminal: limit breach constructs complete `IngestError::OutputTooLarge` diagnostics without internal transactional rollback.

### Network & GitHub

- Streaming archive: Single HTTP request streams GitHub repository archive (`/repos/{owner}/{repo}/tarball/{ref}`) via `flate2` and `tar` over a 64 KiB `BufReader` stream, streaming entries into `Selector` (`CandidateSlot`) with zero intermediate disk files and fail-fast budget termination. Commit revision is extracted from `pax_global_header`.
- Credential isolation: `Authorization: Bearer <token>` attaches strictly to exact-origin GitHub API (`https://api.github.com`); foreign origins and redirects never receive credentials.
- Retry policy: Retries transient transport errors (`Io`, `Timeout`, `ConnectionFailed`) and HTTP 429 / 5xx with bounded exponential backoff up to `MAX_RETRIES` total attempts (1 initial + up to 2 retries). Fails immediately on non-transient transport errors and 4xx client errors (except 429).

## Limits and Tunables (`src/core/policy.rs`)

Operational constants, limits, thresholds, noise lists, and sensitive patterns are defined exclusively in `src/core/policy.rs` as the single source of truth (`MAX_FILE_SIZE`, `MAX_OUTPUT_SIZE`, `MAX_FILE_COUNT`, `ARCHIVE_MAX_SECS`, `MAX_RETRIES`, `RETRY_INITIAL_DELAY_MS`, `RETRY_MAX_DELAY_MS`, `NOISE`, `SENSITIVE_EXTENSIONS`).

## Functional core / imperative shell boundary

Boundary rule: Pure core performs no I/O, clock, or network calls and never depends on shell code; shell depends on core in one direction.

- Pure core (`src/core/`):
  - `cli.rs`: CLI argument parsing, help detection, and `CliAction` generation (`parse`).
  - `policy.rs`: Operational limits, noise patterns, sensitive patterns, and path classification (`PathStatus`, `classify_path`, `is_hidden_segment`). Single source of truth for limits and path exclusion rules.
  - `model.rs`: Domain value types (`Source`, `CollectionResult`, `IngestReport`, `RunConfig`, `OutputTarget`, `DigestFormat`) and source parsing (`Source::parse`, `Source::parse_os`).
  - `storage.rs`: Continuous arena storage (`FileStorage`, `FileSpan`, `FileRecord`, `FileRecordIter`, `cmp_path`, `largest_files`).
  - `error.rs`: Domain error enum `IngestError` and diagnostic payloads (`CandidateLimitExceededError`).
  - `selector.rs`: Candidate reservation (`CandidateSlot`), directory accounting (`CandidateBudget`), and storage accumulation (`Selector`).
  - `render.rs`: Format-specific streaming serializers (Text/XML), format-neutral ASCII hierarchy tree rendering (`render_tree`), single-pass streaming escaping (`write_escaped`), token estimation heuristic (`estimate_tokens`), and `BoundedEstimatingWriter`.
  - `archive.rs`: Pax header comment extraction and single-pass tar entry path normalization & policy classification (`normalize_tar_entry_path`).
- Imperative shell (`src/shell/`):
  - `src/main.rs`: Binary entry point and CLI completion report formatting.
  - `shell/mod.rs`: Shell module declarations, pipeline orchestration (`ingest`), and scoped test fixture isolation (`TempDirGuard`).
  - `shell/local.rs`: Single-pass filesystem candidate traversal, non-symlink metadata validation, and pre-I/O candidate reservation in `Selector`.
  - `shell/github.rs`: Streaming archive tarball collection (`collect_archive`), HTTP transport (`GithubClient`), retry handling, body bounding, and token resolution.
  - `shell/output.rs`: Atomic file writing via RAII commit guard (`AtomicFile`), stdout streaming, and canonical output path resolution (`resolve_output_target`).
