# Domain Context — codefeed

## Purpose

Convert a local directory or public GitHub repository into a single LLM-friendly text digest. Public behavior, limits, and CLI contracts are in `README.md`; coding standards in `docs/coding_convention.md`; this document fixes domain terms, invariants, and the functional-core / imperative-shell boundary.

## Entities and value objects

- `Source` — `Local(PathBuf)` or `Github { owner, repo, reference }` (`#[non_exhaustive]`, constructed via `Source::github`). Enforces owner/repo identifier validation and reference invariants via `GitRef`.
- `GitRef` — Validated non-empty Git revision reference (branch, tag, commit SHA), rejecting empty and whitespace-only strings.
- `RunConfig` — Execution configuration unifying `source`, `output`, and `format`.
- `Admission` & `PendingEntry` — Linear two-phase admission typestate in `Selector`. `Selector::admit(status, size_hint)` evaluates paths against policy and budget limits without allocating closures, yielding `Admission::Settled(Selector)` (ignored/skipped paths with updated counts) or `Admission::Pending(PendingEntry)`. Admitted candidates complete linearly via `pending.accept(&[u8])` (evaluates UTF-8 text and NUL-free content; stores in arena or increments `skipped` on oversize, binary, or non-UTF-8) or `pending.skip()` (unreadable/missing files). Shell collectors filter non-regular files (FIFOs, sockets, devices, symlinks) and noise/sensitive/hidden paths pre-candidate, map files unlinked after discovery or unreadable at open (`PermissionDenied`, Windows sharing violation) to `skip()`, and abort unexpected I/O errors as `IngestError::Io`.
- `FileSpan` — 16-byte POD indexing path and content byte spans via private offsets, accessed via borrowed slice methods (`path(&arena)`, `content(&arena)`).
- `FileStorage` — Append-only arena buffer (`path_arena`, `content_arena`, `spans`). Provides zero-copy `FileRecord<'a>` borrowed views and in-place component-wise lexicographical sorting (`sort_by_path`) without string reallocations.
- `FileRecord<'a>` — Zero-copy borrowed digest entry view: `path: &'a str`, `content: &'a str`.
- `SortedFiles` — Lexicographically sorted file storage arena wrapper guaranteeing path ordering invariants.
- `CollectionResult` — Uniform format-neutral collection record: `label`, optional `revision`, `files: SortedFiles`, `skipped`. Provides linear consuming transition via `collection.render(format) -> Result<RenderedDigest, DomainError>`.
- `RenderedDigest` — Formatted in-memory digest containing `bytes`, `estimated_tokens`, `included_files`, and `skipped_files`. Directly yields `IngestReport` via `into_report(output)`.
- `IngestReport` — Output summary report: `output`, `included_files`, `skipped_files`, `estimated_tokens` (`#[non_exhaustive]`).
- `OutputTarget` — Digest destination: `File(PathBuf)` or `Stdout`.
- `DigestFormat` — Output serialization format: `Text` or `Xml`.
- `CandidateBudget` — Candidate file counter with directory aggregation (`HashMap<String, usize>`) for limit tracking and overflow diagnostics.
- `Policy` — Centralized operational limits, thresholds, noise lists, and sensitive patterns (`src/core/policy.rs`). Single source of truth.

## Invariants

### Type-Driven Invariants & Data Enforcement

- **Enforce Invariants via Data & Types:** Encode invariants into types, linear typestates, and compile-time constants rather than scattered runtime conditionals:
  - **Boundary Validation via Value Objects:** Untrusted inputs are parsed once at construction boundaries into strict types (`GitRef`, `Source`). Internal consumers rely on validity by type contract.
  - **Linear Typestates:** Two-phase candidate evaluation (`Selector::observe` settles each candidate through an internal `Admission::Pending(PendingEntry)` completed via `accept` or `skip`) and end-to-end consuming pipeline (`Selector` -> `CollectionResult` -> `RenderedDigest` -> `IngestReport`) enforce linear consumption at compile time without tracking flags.
  - **Structural Invariant Wrappers:** Structural properties are captured as distinct types (`SortedFiles` guarantees component-wise lexicographical path order; `OutputTarget` eliminates sentinel paths like `"-"` at the type level).
  - **Compile-Time Assertions (CTFE):** Cross-constant operational constraints (`MAX_OUTPUT_SIZE + MAX_FILE_SIZE + MAX_PATH_BYTES <= u32::MAX`, retry backoff limits, timeout hierarchies) are asserted at build time via `const _: () = { assert!(...); };` blocks.
  - **Compile-Time Lookup Tables:** Character classification and URI escaping rules are computed into static `.rodata` tables (`[bool; 256]`) at compilation, eliminating runtime branches and bitwise overhead.

### Filtering & Accounting

- Pre-candidate exclusions: `.git`, hidden entries (including `.env*`; empty path segments from redundant delimiters are filtered to prevent false positives), symlinks, lockfiles (`Cargo.lock`, `package-lock.json`, `yarn.lock`, `pnpm-lock.yaml`, `bun.lock`, `bun.lockb`, `go.sum`, `poetry.lock`, `Pipfile.lock`, `composer.lock`, `Gemfile.lock`), and sensitive files matched case-insensitively by prefix (`id_rsa*`, `id_ed25519*`, `id_ecdsa*`, `id_dsa*`, `credentials*`) and extension (`*.key`/`*.pem`, `*.p12`/`*.pkcs12`, `*.pfx`, `*.credentials`; defined in `src/core/policy.rs`). Matches path conventions, not file content. Excluded paths consume neither candidate slots nor `skipped` counts.
- Candidate accounting: `Selector::observe` consumes candidate slots before evaluating content. GitHub archive entries are admitted before their bodies are read (the body read runs only inside `observe`); local workers read bounded bodies concurrently, and the main thread calls `observe` afterward. Non-UTF-8 paths consume candidate budget with their valid top-level directory prefix (`"dir/"` or `""`) and are marked as skipped (settled without reading content).
- `skipped` count: Tracks candidates rejected for content, size, or encoding (binary with NUL bytes, oversized files, or non-UTF-8 paths/content). Ignored paths are never counted in `skipped`.
- Output bounding & Size cap enforcement: `MAX_OUTPUT_SIZE` is enforced via a two-stage check: `Selector::add_file` early-fails during collection using raw stored arena bytes (`path_arena + content_arena`); `BoundedBuffer` enforces `MAX_OUTPUT_SIZE` during in-memory rendering across headers, tree, framing, and body bytes. Because escaping and framing only expand or preserve byte lengths (`rendered_bytes >= raw_bytes`), early-fail checks never reject valid outputs. Overflow content is never emitted; file and stdout targets emit nothing on overflow.
- Diagnostic ranking: `largest_files` is called exclusively on output limit overflow paths, sorting by descending size and tie-breaking by component-wise path order (`cmp_path`) to select top-5 largest files deterministically over the files stored so far; on local early-fail overflow that set depends on worker order and can vary between runs.

### Rendering & Formats

- ASCII tree (`├── `, `└── `, `│   `): Renders deterministic directory structure directly from sorted `FileStorage` with monotonic byte-offset slicing (`render::render_tree`). Format-neutral; escapes control codes and XML entities to prevent tree line-break injections.
- Token estimation: Pure function `estimate_tokens_from_bytes(bytes)` calculates `(ascii_bytes / 4 + non_ascii_chars).max(1)` when non-zero, `0` when empty. Reported in `IngestReport.estimated_tokens` and completion messages (`~N tokens`). Calibrated for English text, code (~4 bytes/token), and multibyte text (leading UTF-8 bytes per character).
- Plain text (`--format text`): Preserves valid UTF-8 file content verbatim (rejecting NUL bytes); escapes header metadata and file paths (`\`, `\r`, `\n`, C0 control characters `0x00..=0x08`, `0x0B`, `0x0C`, `0x0E..=0x1F`, DEL `0x7F` to `\u{FFFD}`) while leaving file bodies unmutated.
- XML (`--format xml`): Emits `<repository>`, `<directory_structure>`, and `<files><file path="...">` tags, escaping XML entities (`&`, `<`, `>`, `"`, `'`), attribute whitespace (`\t`, `\n`, `\r` via character references), while preserving raw `\t`, `\n`, `\r` in `<file>` element bodies and sanitizing disallowed XML 1.0 control characters to `\u{FFFD}`.
- Lexicographical ordering: File-path ordering is established in-place on `spans` via `SortedFiles::new(FileStorage)` during `Selector::finish()`. Format renderers consume `ExactSizeIterator<Item = FileRecord<'_>>` directly with zero heap reallocation or re-sorting.
- Header format: Local digest headers use root directory basename (`Repository: <basename>`), omitting absolute paths and system prefixes.

### I/O & Output Target Contracts

- File target: Validates and resolves output path at startup (`resolve_output_target`). Writes atomically via `AtomicFile` with inner `TempFile` closing open handles before unlinking on `Drop` (cross-platform Windows safety), preserving existing Unix permissions, syncing to disk, renaming on commit, disarming cleanup, and syncing parent directory on Unix. Automatically unlinks temp files on error or panic.
- Stdout target: Buffers output into memory up to `MAX_OUTPUT_SIZE` and writes to stdout only on success, preventing partial emission on overflow.
- Completion messages: Text built by `core/cli.rs::completion_message`; `shell/mod.rs::run_cli` writes it to stdout for file targets and stderr for stdout targets.
- Broken pipe: Downstream pipe closure (`BrokenPipe` / `EPIPE`) on stdout exits cleanly with status 0.
- Local walk exclusion: Excludes existing output files via canonical path comparison resolved once at startup (`resolve_output_target`), avoiding repeated syscalls during traversal. Assumes a quiescent directory tree during scan (see `docs/security_and_filtering.md`).
- Local walk pipeline:
  1. Producer-consumer concurrency: `ignore::WalkBuilder::build_parallel` workers walk directories, classify paths (`classify_local_path`), inspect `fs::symlink_metadata` (rejecting symlinks pre-open), and read bounded content (`read_bounded`, capped at `MAX_FILE_SIZE + 1`) concurrently across available CPU cores.
  2. Bounded channel backpressure: Workers transmit `WorkerMsg` through `std::sync::mpsc::sync_channel(128)` to the main thread inside `std::thread::scope`, strictly bounding memory usage.
  3. Linear consumption & cancellation: Main thread consumes messages sequentially into `Selector` (`observe`). Traversal aborts immediately via `AtomicBool` flag and receiver hang-up on error or budget overflow (`CandidateLimitExceeded`, `OutputTooLarge`).
  4. Deterministic output: Resulting storage is sorted component-wise lexicographically via `SortedFiles::new` (`cmp_path`), guaranteeing byte-for-byte reproducibility regardless of thread execution order.
- CLI argument parsing: Linearly parsed into options and source in pure core (`core::cli`) using `std` only, supporting order-independent sources, short flags (`-o`, `-f`, `-r`, `-h`), key-value `=` syntax, and POSIX `--` delimiters without third-party CLI dependencies. CLI execution, help display, and output reporting are encapsulated via `shell::run_cli`.
- Candidate budget tracking: Standard `HashMap<String, usize>` during collection. Overflow diagnostics are sorted by count descending then directory ascending, but cover only candidates admitted before abort; local runs can report different counts because worker completion order varies.
- Digest budget tracking: Tracked directly in `Selector` and error-terminal: limit breach constructs complete `DomainError::OutputTooLarge` diagnostics without internal transactional rollback.

### Network & GitHub

- Streaming archive: Single HTTP request streams GitHub repository archive (`/repos/{owner}/{repo}/tarball/{ref}`) via `flate2` and `tar` over a 64 KiB `BufReader` stream, streaming entries into `Selector` (`Selector::observe`) with zero intermediate disk files and fail-fast budget termination. Commit revision is extracted from `pax_global_header`. Stream, gzip, and tar read failures surface as `IngestError::Http`.
- Credential isolation: `Authorization: Bearer <token>` attaches strictly to exact-origin GitHub API (`https://api.github.com`); foreign origins and redirects never receive credentials.
- Retry policy: Retries transient transport errors (`Io`, `Timeout`, `ConnectionFailed`), HTTP 429 / 5xx, and rate-limited 403 responses (`Retry-After` present or `x-ratelimit-remaining: 0`) with bounded exponential backoff up to `MAX_ATTEMPTS` total attempts (1 initial + up to 2 retries). Delay comes from `Retry-After`, else from `x-ratelimit-reset` when `x-ratelimit-remaining` is `0`; a delay above `RETRY_MAX_DELAY_MS` fails instead of waiting. Fails immediately on non-transient transport errors and other 4xx client errors.

## Limits and Tunables (`src/core/policy.rs`)

Operational constants, limits, thresholds, noise lists, and sensitive patterns are defined exclusively in `src/core/policy.rs` as the single source of truth (`MAX_FILE_SIZE`, `MAX_OUTPUT_SIZE`, `MAX_FILE_COUNT`, `ARCHIVE_MAX_SECS`, `MAX_ATTEMPTS`, `RETRY_INITIAL_DELAY_MS`, `RETRY_MAX_DELAY_MS`, `GITHUB_TIMEOUT_SECS`, `NOISE`, `SENSITIVE_EXTENSIONS`, `SENSITIVE_PREFIXES`).

## Functional core / imperative shell boundary

Boundary rule: Pure core performs no I/O, clock, or network calls and never depends on shell code; shell depends on core in one direction.

- Pure core (`src/core/`):
  - `cli.rs`: CLI argument parsing (`parse`, folded through the `Parsed` accumulator), help detection, `CliAction` generation, and the completion summary text (`completion_message`).
  - `policy.rs`: Operational limits, noise patterns, sensitive patterns, and path classification (`PathStatus`, `is_ignored_filename`, `is_hidden_segment`). Single source of truth for limits and path exclusion rules.
  - `model.rs`: Domain value types (`Source`, `CollectionResult`, `IngestReport`, `RunConfig`, `OutputTarget`, `DigestFormat`) and source parsing (`Source::parse`, `Source::parse_os`).
  - `github.rs`: Pure GitHub domain rules, identifier validation (`valid_github_name`), URL parsing (`parse_github_url`), tarball URL construction (`tarball_url`), API origin security policy (`is_github_api_origin`), retry state transitions (`AttemptState::step`), and API error and retry warning text (`api_error`, `status_retry_warning`, `transport_retry_warning`).
  - `storage.rs`: Continuous arena storage (`FileStorage`, `SortedFiles`, `FileSpan`, `FileRecord`, `cmp_path`, `largest_files`).
  - `error.rs`: Pure domain error enum `DomainError` and diagnostic payloads (`CandidateLimitExceededError`). Core never constructs effect errors.
  - `selector.rs`: Candidate settlement (`observe`, with internal `Admission` and `PendingEntry`), directory accounting (`CandidateBudget`), content classification, and storage accumulation (`Selector`).
  - `render/`: Format-specific serializers (`formats.rs` with `FormatRenderer`, `TextRenderer`, `XmlRenderer`), format-neutral ASCII hierarchy tree rendering (`tree.rs`), single-pass escaping (`escape.rs`), and bounded buffer with token estimation (`budget.rs`).
  - `archive.rs`: Pax header comment extraction and single-pass tar entry path normalization & policy classification (`normalize_tar_entry_path`).
- Imperative shell (`src/shell/`):
  - `src/main.rs`: Binary entry point only; delegates to `run_cli`.
  - `shell/mod.rs`: Shell module declarations, pipeline orchestration (`ingest`, `run_cli`), and the stream choice for the completion message.
  - `shell/error.rs`: Ingestion error `IngestError` (`Domain(DomainError)`, `Io`, `Http`) and the transport error constructor (`transport_error`). The only place effect errors are built.
  - `shell/testing.rs`: Scoped test fixture isolation (`TempDirGuard`, `temp_dir`) and shared tarball builders (`gzip_archive`, `gzip_tarball`).
  - `shell/local.rs`: Single-pass filesystem candidate traversal, non-symlink metadata validation, and pre-I/O candidate admission in `Selector`.
  - `shell/archive.rs`: Tarball stream decompression, PAX commit SHA extraction, and per-entry admission through `Selector::observe` (`collect`).
  - `shell/github.rs`: Streaming tarball request (`GithubClient`), HTTP transport, retry attempts (`Attempt`), warning printing, body bounding, and token resolution.
  - `shell/output.rs`: Atomic file writing via RAII commit guard (`AtomicFile`) and inner `TempFile` guard, stdout streaming, and canonical output path resolution (`resolve_output_target`).
