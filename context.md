# Domain Context — codefeed

## Purpose

Convert a local directory or public GitHub repository into one LLM-friendly
text digest. Public behavior, limits, and CLI contract are documented in
`README.md`; coding standards and Rust API conventions are defined in
`docs/coding_convention.md`; this file fixes domain terms and the functional-core /
imperative-shell boundary.

## Entities and value objects

- `Source` — `Local(PathBuf)` or `GitHub { owner, repo, reference }`. Encapsulates reference validation and assignment via `Source::with_reference`.
- `RunConfig` — execution configuration unifying `source`, `output`, and `format`.
- `CandidateInput` — one observed candidate entry handed to the pure core: `File { path, bytes }`, `TooLarge { path }`, or `Skipped { path }`. Pure evaluation in `Selector::feed` verifies UTF-8 text and NUL-free content, registers directory counts with `CandidateBudget`, and updates storage. Shell collectors filter noise and sensitive paths pre-candidate (`policy::is_ignored_path`).
- `FileSpan` — POD indexing path and content byte spans (`path_offset`, `path_len`, `content_offset`, `content_len`).
- `FileStorage` — continuous append-only arena buffer with private arenas (`path_arena`, `content_arena`, `spans`); provides zero-copy borrowed `FileRecord<'a>` views throughout ingestion and enables in-place lexicographical sorting (`sort_by_path`) by swapping 16-byte `FileSpan` offsets without string reallocations.
- `FileRecord<'a>` — zero-copy borrowed digest entry view: `path: &'a str`, `content: &'a str`.
- `CollectionResult` — uniform collection record: `label`, optional `revision`, `files: FileStorage`, `skipped`. Format-neutral and decoupled from presentation.
- `IngestReport` — `output`, `included_files`, `skipped_files`, `estimated_tokens`.
- `OutputTarget` — digest destination: `File(PathBuf)` or `Stdout`.
- `DigestFormat` — output serialization format: `Text` or `Xml`.
- Budgets — `CandidateBudget` (candidate count with directory aggregation backed by standard `HashMap<String, usize>`), tracking candidate limits and diagnostic reporting.
- Policy — centralised operational limits, thresholds, noise lists, and sensitive patterns (`policy.rs`). Single source of truth for limits.

## Invariants

### Filtering & Accounting

- Excludes `.git`, hidden entries (empty path segments from redundant delimiters are filtered to prevent false positives), symlinks, non-UTF-8 paths/content, lockfiles (`Cargo.lock`, `package-lock.json`, `yarn.lock`, `pnpm-lock.yaml`, `go.sum`), and conventional sensitive files by case-insensitive filename prefix (`.env*`, `id_rsa*`, `id_ed25519*`, `credentials*`) and extension (`*.key`/`*.pem`, `*.p12`/`*.pkcs12`, `*.credentials`) before candidate accounting (conventional pattern filter, not a content-based secret scanner). They are neither candidates nor `skipped`.
- Consumes candidate slots before evaluating content, size, or encoding.
- `skipped` counts candidates rejected for content, size, or encoding (including binary files containing NUL bytes or invalid UTF-8). Ignored paths are never counted in `skipped`.
- Rejects content exceeding `MAX_OUTPUT_SIZE`; overflow content is never emitted.
- Diagnostic largest files reporting (`largest_files`) uses bounded Top-5 selection with linear position lookup, avoiding full-collection cloning or $O(N \log N)$ sorting while preserving insertion stability for tie-breakers.

### Rendering & Formats

- ASCII hierarchy tree (`├── `, `└── `, `│   `) renders deterministic directory structure, escaping control codes and XML entities to prevent tree line-break injections.
- Heuristic token estimation runs via pure function `estimate_tokens(bytes)` (`(bytes / 4).max(1)` when non-zero, `0` when empty), reported in `IngestReport.estimated_tokens` and completion messages (`~N tokens`). This is a fast byte heuristic calibrated for English text and source code (~4 bytes/token); multibyte text (such as Korean/CJK) will be underestimated.
- Plain text serialization (`--format text`) preserves all valid UTF-8 file content verbatim (rejecting only NUL bytes) to maintain raw fidelity for LLM prompts; headers and paths are escaped, but file bodies are unmutated (viewing raw digests in interactive terminals may affect emulator state if files contain ANSI escape codes).
- XML serialization (`--format xml`) structures output into `<repository>`, `<directory_structure>`, and `<files><file path="...">` tags, escaping XML entities (`&`, `<`, `>`, `"`, `'`) and sanitizing disallowed XML 1.0 control characters to `\u{FFFD}`.
- File-path ordering is strictly lexicographical, established in-place on `spans` via `FileStorage::sort_by_path()` during `Selector::finish()`. Format renderers consume `ExactSizeIterator<Item = FileRecord<'_>>` directly with zero heap re-allocation or re-sorting.
- Local digest headers use root directory basename (`Repository: <basename>`), omitting absolute paths and system prefixes.

### I/O & Output Target Contracts

- File target writes atomically via RAII guard (`AtomicFile`): target output path is validated and resolved in a single step at startup (`resolve_output_target`), temporary file is created under the canonical parent directory, written, synced to disk, and atomically renamed on commit, with automatic drop cleanup on error or panic.
- Stdout target writes directly to stdout with no temporary file or filesystem sync.
- Completion messages emit to stdout for file targets and stderr for stdout targets.
- Broken downstream pipe (`BrokenPipe` / `EPIPE`) on stdout exits cleanly with status 0 (preserving estimated tokens when encountered on buffer flush; defaulting to 0 on early mid-stream write termination).
- Local walk excludes existing output files via canonical path comparison. The output target is resolved once at initialization (`resolve_output_target`); walk candidates are compared directly without repeated syscalls during traversal.
- Local walk uses a 2-step linear pipeline:
  1. `ignore::WalkBuilder` with `follow_links(false)` and `.sort_by_file_name(|a, b| a.cmp(b))` collects candidate paths into a deterministically sorted list, with pre-flight batch candidate budget verification (`CandidateBudget::check_limit`) to fail fast before reading file bodies.
  2. Bounded sequential reads (verifying regular non-symlink metadata before opening) feed observations into `Selector`. Stream state in `LocalStream` holds only the root label and candidate iterator without retaining redundant root path allocations.
- Digest cap (`MAX_OUTPUT_SIZE`) is strictly enforced on bytes actually written to the output stream via `BoundedEstimatingWriter`, preventing framing/metadata overflow (atomic file rollback removes partial output; stdout stream terminates immediately at `MAX_OUTPUT_SIZE`).
- Token count is estimated on the same writer stream pass over rendered byte length.
- ASCII directory tree renders directly from sorted `FileStorage` paths without intermediate tree arena allocation (`tree::render_tree`). It is format-neutral; XML entity and newline escaping occurs on-the-fly during rendering.
- CLI arguments are parsed linearly into options and source in pure core (`core::cli`) using `std` only, supporting order-independent sources, short flags (`-o`, `-f`, `-r`, `-h`), key-value `=` syntax, and POSIX `--` delimiters without third-party CLI dependencies.
- Candidate budget uses standard `HashMap<String, usize>` during collection, with deterministic overflow diagnostics sorted by count descending then directory ascending.
- Digest budget is tracked directly in `Selector` and is error-terminal: limit breach constructs complete `IngestError::OutputTooLarge` diagnostics without internal transactional rollback.

### Network & GitHub

- Single HTTP request streams GitHub repository archive (`/repos/{owner}/{repo}/tarball/{ref}`) via `flate2` and `tar` over a 64 KiB `BufReader` buffered stream, streaming entries into `Selector::feed` with zero intermediate disk files and fail-fast budget termination. Commit revision is extracted from the archive `pax_global_header`.
- HTTP credentials (`Authorization: Bearer <token>`) attach strictly to exact-origin GitHub API (`https://api.github.com`); foreign origins never receive credentials.
- HTTP transport retries transient transport errors (`Io`, `Timeout`, `ConnectionFailed`) and HTTP 429 / 5xx responses with bounded exponential backoff up to `MAX_RETRIES` total attempts (1 initial + up to 2 retries). Fails immediately on non-transient transport errors and 4xx client errors (except 429).

## Limits and Tunables (`src/core/policy.rs`)

Operational constants, limits, thresholds, noise lists, and sensitive patterns are defined exclusively in `src/core/policy.rs` as the single source of truth. Refer to that module for current bounds (`MAX_FILE_SIZE`, `MAX_OUTPUT_SIZE`, `MAX_FILE_COUNT`, `ARCHIVE_MAX_SECS`, `MAX_RETRIES`, `RETRY_INITIAL_DELAY_MS`, `RETRY_MAX_DELAY_MS`, `NOISE`, `SENSITIVE_EXTENSIONS`).

## Functional core / imperative shell boundary

Boundary rule: pure core code performs no I/O, clock, or network and
never depends on shell code; shell code depends on the core in one direction.

- Pure core (`src/core/`):
  - `cli.rs`: Pure CLI argument parsing, help detection, and `CliAction` generation (`parse`).
  - `policy.rs`: all operational limits, noise patterns, sensitive patterns, and path exclusion predicate (`is_ignored_path`). Single source of truth for limits.
  - `source.rs`: source string and OS-string parsing.
  - `model.rs`: domain value types (`Source`, `FileRecord`, `CollectionResult`,
    `IngestReport`, `RunConfig`, `OutputTarget`, `DigestFormat`, `FileStorage`).
  - `error.rs`: domain error enum `IngestError` and diagnostic payloads (`CandidateLimitExceededError`).
  - `selector.rs`: candidate observation (`CandidateInput`), candidate budget directory accounting (`CandidateBudget`), `Selector` storage accumulation, and `largest_files` diagnostic selection.
  - `tree.rs`: Format-neutral ASCII hierarchy tree rendering (`render_tree`) directly from sorted paths.
  - `render/` (`mod.rs`, `text.rs`, `xml.rs`): Format-specific streaming serializers (Text/XML), single-pass escaping (`escape_header`, `xml_escape`, `write_escaped`), token estimation heuristic (`estimate_tokens`), `render_to_writer`, and streaming `BoundedEstimatingWriter`.
  - `github_url.rs`: Pure `std` URL construction with RFC 3986 percent-encoding and origin credential scoping.
  - `archive.rs`: Pax header comment extraction and archive root stripping.
- Imperative shell (`src/shell/`):
  - `src/main.rs`: binary entry point and user-facing CLI completion report formatting.
  - `shell/orchestrate.rs`: Pipeline orchestration (`ingest`) and unified streaming collection driver (`SourceStream`, `collect_stream`).
  - `shell/local.rs`: 2-step linear filesystem candidate discovery, non-symlink metadata validation, and bounded streaming reads (`LocalStream`).
  - `shell/github.rs`: streaming archive tarball entry iterator (`GithubStream`), HTTP transport (`GithubClient`), retry handling, body bounding, and token resolution.
  - `shell/output.rs`: atomic file writing via RAII commit guard (`AtomicFile`), standard output stream emission, and canonical output path resolution (`resolve_output_target`).
  - `shell/mod.rs`: shell module declarations and scoped test fixture isolation (`TempDirGuard`).
