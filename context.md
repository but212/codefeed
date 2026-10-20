# Domain Context — codefeed

## Purpose

Convert a local directory or public GitHub repository into a single LLM-friendly text digest. Public behavior, limits, and CLI contracts are in `README.md`; coding standards in `docs/coding_convention.md`; this document fixes domain terms, invariants, and the functional-core / imperative-shell boundary.

## Entities and value objects

- `Source` — `Local(PathBuf)` or `Github { owner, repo, reference }` (`#[non_exhaustive]`, constructed via `Source::github`). Enforces owner/repo identifier validation and reference invariants via `GitRef`.
- `GitRef` — Validated non-empty Git revision reference (branch, tag, commit SHA), rejecting empty and whitespace-only strings.
- `RunConfig` — Execution configuration unifying `source`, `output`, and `format`.
- `Admission` & `PendingEntry` — Linear two-phase admission typestate in `Selector`. `Selector::admit(status, size_hint)` evaluates paths against policy and budget limits without allocating closures, yielding `Admission::Settled(Selector)` (ignored/skipped paths with updated counts) or `Admission::Pending(PendingEntry)`. Admitted candidates complete linearly via `pending.accept(&[u8])` (evaluates UTF-8 text and NUL-free content; stores in arena or increments `skipped` on oversize, binary, or non-UTF-8) or `pending.skip()` (unreadable/missing files). Shell collectors filter non-regular files (FIFOs, sockets, devices, symlinks) and noise/sensitive/hidden paths pre-candidate, map files unlinked after discovery to `skip()`, and abort unexpected I/O errors as `IngestError::Io`.
- `FileSpan` — 16-byte POD indexing path and content byte spans via private offsets, accessed via borrowed slice methods (`path(&arena)`, `content(&arena)`).
- `FileStorage` — Append-only arena buffer (`path_arena`, `content_arena`, `spans`). Provides zero-copy `FileRecord<'a>` borrowed views and in-place component-wise lexicographical sorting (`sort_by_path`) without string reallocations.
- `FileRecord<'a>` — Zero-copy borrowed digest entry view: `path: &'a str`, `content: &'a str`.
- `SortedFiles` — Lexicographically sorted file storage arena wrapper guaranteeing path ordering invariants.
- `CollectionResult` — Uniform format-neutral collection record: `label`, optional `revision`, `files: SortedFiles`, `skipped`. Provides linear consuming transition via `collection.render(format) -> Result<RenderedDigest, IngestError>`.
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
  - **Linear Typestates:** Two-phase candidate evaluation (`Admission::Pending(PendingEntry)` completed via `accept` or `skip`) and end-to-end consuming pipeline (`Selector` -> `CollectionResult` -> `RenderedDigest` -> `IngestReport`) enforce linear consumption at compile time without tracking flags.
  - **Structural Invariant Wrappers:** Structural properties are captured as distinct types (`SortedFiles` guarantees component-wise lexicographical path order; `OutputTarget` eliminates sentinel paths like `"-"` at the type level).
  - **Compile-Time Assertions (CTFE):** Cross-constant operational constraints (`MAX_OUTPUT_SIZE + MAX_FILE_SIZE + MAX_PATH_BYTES <= u32::MAX`, retry backoff limits, timeout hierarchies) are asserted at build time via `const _: () = { assert!(...); };` blocks.
  - **Compile-Time Lookup Tables:** Character classification and URI escaping rules are computed into static `.rodata` tables (`[bool; 256]`) at compilation, eliminating runtime branches and bitwise overhead.

### Filtering & Accounting

- Pre-candidate exclusions: `.git`, hidden entries (including `.env*`; empty path segments from redundant delimiters are filtered to prevent false positives), symlinks, lockfiles (`Cargo.lock`, `package-lock.json`, `yarn.lock`, `pnpm-lock.yaml`, `go.sum`), and sensitive files matched case-insensitively by prefix (`id_rsa*`, `id_ed25519*`, `credentials*`) and extension (`*.key`/`*.pem`, `*.p12`/`*.pkcs12`, `*.credentials`; defined in `src/core/policy.rs`). Matches path conventions, not file content. Excluded paths consume neither candidate slots nor `skipped` counts.
- Candidate accounting: `Selector::admit` consumes candidate slots before reading file bodies or evaluating content. Non-UTF-8 paths consume candidate budget with their valid top-level directory prefix (`"dir/"` or `""`) and are marked as skipped (`Admission::Settled`).
- `skipped` count: Tracks candidates rejected for content, size, or encoding (binary with NUL bytes, oversized files, or non-UTF-8 paths/content). Ignored paths are never counted in `skipped`.
- Output bounding & Size cap enforcement: `MAX_OUTPUT_SIZE` is enforced via a two-stage check: `Selector::add_file` early-fails during collection using raw stored arena bytes (`path_arena + content_arena`); `BoundedBuffer` enforces `MAX_OUTPUT_SIZE` during in-memory rendering across headers, tree, framing, and body bytes. Because escaping and framing only expand or preserve byte lengths (`rendered_bytes >= raw_bytes`), early-fail checks never reject valid outputs. Overflow content is never emitted; file and stdout targets emit nothing on overflow.
- Diagnostic ranking: `largest_files` is called exclusively on output limit overflow paths, sorting by descending size and tie-breaking by component-wise path order (`cmp_path`) to select top-5 largest files deterministically.

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
- Completion messages: Emitted to stdout for file targets and stderr for stdout targets via `shell/mod.rs::run_cli`.
- Broken pipe: Downstream pipe closure (`BrokenPipe` / `EPIPE`) on stdout exits cleanly with status 0 (preserves estimated tokens on buffer flush; defaults to 0 on mid-stream write termination).
- Local walk exclusion: Excludes existing output files via canonical path comparison resolved once at startup (`resolve_output_target`), avoiding repeated syscalls during traversal. Assumes a quiescent directory tree during scan (see `docs/security_and_filtering.md`).
- Local walk pipeline:
  1. `ignore::WalkBuilder` with `follow_links(false)` and `.sort_by_file_name(|a, b| a.cmp(b))` traverses entries, classifies paths via `classify_local_path`, and evaluates admission in `Selector` (`Selector::admit`) before reading file bodies.
  2. Bounded sequential reads into a reusable content scratch buffer complete pending entries (`PendingEntry::accept` / `skip`) directly into `Selector`.
- CLI argument parsing: Linearly parsed into options and source in pure core (`core::cli`) using `std` only, supporting order-independent sources, short flags (`-o`, `-f`, `-r`, `-h`), key-value `=` syntax, and POSIX `--` delimiters without third-party CLI dependencies. CLI execution, help display, and output reporting are encapsulated via `shell::run_cli`.
- Candidate budget tracking: Standard `HashMap<String, usize>` during collection, with deterministic overflow diagnostics sorted by count descending then directory ascending.
- Digest budget tracking: Tracked directly in `Selector` and error-terminal: limit breach constructs complete `IngestError::OutputTooLarge` diagnostics without internal transactional rollback.

### Network & GitHub

- Streaming archive: Single HTTP request streams GitHub repository archive (`/repos/{owner}/{repo}/tarball/{ref}`) via `flate2` and `tar` over a 64 KiB `BufReader` stream, streaming entries into `Selector` (`Selector::admit` / `accept`) with zero intermediate disk files and fail-fast budget termination. Commit revision is extracted from `pax_global_header`.
- Credential isolation: `Authorization: Bearer <token>` attaches strictly to exact-origin GitHub API (`https://api.github.com`); foreign origins and redirects never receive credentials.
- Retry policy: Retries transient transport errors (`Io`, `Timeout`, `ConnectionFailed`) and HTTP 429 / 5xx with bounded exponential backoff up to `MAX_ATTEMPTS` total attempts (1 initial + up to 2 retries). Fails immediately on non-transient transport errors and 4xx client errors (except 429).

## Limits and Tunables (`src/core/policy.rs`)

Operational constants, limits, thresholds, noise lists, and sensitive patterns are defined exclusively in `src/core/policy.rs` as the single source of truth (`MAX_FILE_SIZE`, `MAX_OUTPUT_SIZE`, `MAX_FILE_COUNT`, `ARCHIVE_MAX_SECS`, `MAX_ATTEMPTS`, `RETRY_INITIAL_DELAY_MS`, `RETRY_MAX_DELAY_MS`, `GITHUB_TIMEOUT_SECS`, `NOISE`, `SENSITIVE_EXTENSIONS`).

## Functional core / imperative shell boundary

Boundary rule: Pure core performs no I/O, clock, or network calls and never depends on shell code; shell depends on core in one direction.

- Pure core (`src/core/`):
  - `cli.rs`: CLI argument parsing, help detection, and `CliAction` generation (`parse`).
  - `policy.rs`: Operational limits, noise patterns, sensitive patterns, and path classification (`PathStatus`, `is_ignored_filename`, `is_hidden_segment`). Single source of truth for limits and path exclusion rules.
  - `model.rs`: Domain value types (`Source`, `CollectionResult`, `IngestReport`, `RunConfig`, `OutputTarget`, `DigestFormat`) and source parsing (`Source::parse`, `Source::parse_os`).
  - `github.rs`: Pure GitHub domain rules, identifier validation (`valid_github_name`), URL parsing (`parse_github_url`), tarball URL construction (`tarball_url`), API origin security policy (`is_github_api_origin`), and retry state transitions (`AttemptState::step`).
  - `storage.rs`: Continuous arena storage (`FileStorage`, `SortedFiles`, `FileSpan`, `FileRecord`, `cmp_path`, `largest_files`).
  - `error.rs`: Domain error enum `IngestError` and diagnostic payloads (`CandidateLimitExceededError`).
  - `selector.rs`: Candidate admission (`Admission`, `PendingEntry`), directory accounting (`CandidateBudget`), content classification, and storage accumulation (`Selector`).
  - `render/`: Format-specific serializers (`formats.rs` with `FormatRenderer`, `TextRenderer`, `XmlRenderer`), format-neutral ASCII hierarchy tree rendering (`tree.rs`), single-pass escaping (`escape.rs`), and bounded buffer with token estimation (`budget.rs`).
  - `archive.rs`: Pax header comment extraction and single-pass tar entry path normalization & policy classification (`normalize_tar_entry_path`).
- Imperative shell (`src/shell/`):
  - `src/main.rs`: Binary entry point and CLI completion report formatting.
  - `shell/mod.rs`: Shell module declarations and pipeline orchestration (`ingest`, `run_cli`).
  - `shell/testing.rs`: Scoped test fixture isolation (`TempDirGuard`, `temp_dir`).
  - `shell/local.rs`: Single-pass filesystem candidate traversal, non-symlink metadata validation, and pre-I/O candidate admission in `Selector`.
  - `shell/github.rs`: Streaming archive tarball collection (`collect_archive`), HTTP transport (`GithubClient`), retry handling, body bounding, and token resolution.
  - `shell/output.rs`: Atomic file writing via RAII commit guard (`AtomicFile`) and inner `TempFile` guard, stdout streaming, and canonical output path resolution (`resolve_output_target`).
