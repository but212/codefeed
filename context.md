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
- `Item` — one observed candidate entry handed to the pure core: `path` (`ItemPath::Utf8 | NonUtf8`) and `probe` (`Probe::Bytes | Missing | TooLarge`). Pure evaluation maps `Item` into `ItemDecision` (`Included { path, content } | Skipped { path }`) without intermediate string allocations. Shell collectors filter noise and sensitive paths pre-candidate (`PathClassification::Ignored`).
- `FileSpan` — 16-byte POD indexing path and content byte spans (`path_offset`, `path_len`, `content_offset`, `content_len`).
- `FileStorage` — continuous append-only arena buffer with private arenas (`path_arena`, `content_arena`, `spans`) eliminating per-file heap fragmentation while preserving invariant safety and supporting in-place span sorting (`sort_by_path`).
- `FileRecord<'a>` — zero-copy borrowed digest entry view: `path: &'a str`, `content: &'a str`.
- `CollectionResult` — uniform collection record: `label`, optional `revision`, `files: FileStorage`, `skipped`, `trie`. Format-neutral and decoupled from presentation.
- `IngestReport` — `output`, `included_files`, `skipped_files`, `estimated_tokens`.
- `OutputTarget` — digest destination: `File(PathBuf)` or `Stdout`.
- `DigestFormat` — output serialization format: `Text` or `Xml`.
- Budgets — `CandidateBudget` (candidate count with run-length encoded directory tracking fast-path backed by `CompactDir` inline/heap descriptors), tracking candidate limits and diagnostic reporting.
- Policy — centralised tunables, thresholds, noise lists, sensitive patterns, and extension sets
  (`policy.rs`).

## Invariants

### Filtering & Accounting

- Excludes `.git`, hidden entries (empty path segments from redundant delimiters are filtered to prevent false positives), symlinks, non-UTF-8 paths/content, lockfiles (`Cargo.lock`, `package-lock.json`, `yarn.lock`, `pnpm-lock.yaml`, `go.sum`), and conventional sensitive files by case-insensitive filename prefix (`.env*`, `id_rsa*`, `id_ed25519*`, `credentials*`) and extension (`*.key`/`*.pem`, `*.p12`/`*.pkcs12`, `*.credentials`) before candidate accounting (conventional pattern filter, not a content-based secret scanner). They are neither candidates nor `skipped`.
- Consumes candidate slots before evaluating content, size, or encoding.
- `skipped` counts candidates rejected for content, size, or encoding. Ignored paths are never counted in `skipped`.
- Rejects content exceeding `MAX_OUTPUT_SIZE`; overflow content is never emitted.
- Known binary extensions (`BINARY_EXTENSIONS`) are classified as `Probe::Missing` before reading content or remote downloading to save I/O while preserving candidate and skipped accounting.
- Diagnostic largest files reporting (`largest_files`) uses bounded Top-5 selection with linear position lookup, avoiding full-collection cloning or $O(N \log N)$ sorting while preserving insertion stability for tie-breakers.

### Rendering & Formats

- ASCII hierarchy tree (`├── `, `└── `, `│   `) renders deterministic directory structure, escaping control codes and XML entities to prevent tree line-break injections.
- Heuristic token estimation runs via state machine `TokenEstimator`, exposed through pure function `estimate_tokens(&str)` and `BoundedEstimatingWriter`, reported in `IngestReport.estimated_tokens` and completion messages (`~N tokens`).
- Plain text serialization (`--format text`) preserves all valid UTF-8 file content verbatim (rejecting only NUL bytes) to maintain raw fidelity for LLM prompts; headers and paths are escaped, but file bodies are unmutated (viewing raw digests in interactive terminals may affect emulator state if files contain ANSI escape codes).
- XML serialization (`--format xml`) structures output into `<repository>`, `<directory_structure>`, and `<files><file path="...">` tags, escaping XML entities (`&`, `<`, `>`, `"`, `'`) and sanitizing disallowed XML 1.0 control characters to `\u{FFFD}`.
- File-path ordering is strictly lexicographical, established in-place on 16-byte `spans` via `FileStorage::sort_by_path()` during `Selector::finish()`. Format renderers consume `ExactSizeIterator<Item = FileRecord<'_>>` directly with zero heap re-allocation or re-sorting.
- Local digest headers use root directory basename (`Repository: <basename>`), omitting absolute paths and system prefixes.

### I/O & Output Target Contracts

- File target writes atomically via RAII guard (`AtomicFile`): target parent path lexical normalization (`normalize_path`), parent directory verification and creation, temporary file creation under the normalized parent directory, streaming write, disk sync, explicit handle release, atomic rename to the normalized target path, and automatic drop cleanup on error or panic.
- Stdout target writes directly to stdout with no temporary file or filesystem sync.
- Completion messages emit to stdout for file targets and stderr for stdout targets.
- Broken downstream pipe (`BrokenPipe` / `EPIPE`) on stdout exits cleanly with status 0 (preserving estimated tokens when encountered on buffer flush; defaulting to 0 on early mid-stream write termination).
- Local walk excludes existing output files via normalized path comparison (`output::OutputFilter`). The output target is normalized once at initialization (resolving parent symlinks when canonicalizable or falling back to lexical normalization); walk candidates are compared directly without syscalls during traversal.
- Local walk uses a 2-step linear pipeline:
  1. `ignore::WalkBuilder` with `follow_links(false)` and `.sort_by_file_name(|a, b| a.cmp(b))` collects candidate paths into a deterministically sorted list, with pre-flight batch candidate budget verification (`CandidateBudget::check_limit`) to fail fast before reading file bodies.
  2. Bounded sequential reads via `local_io::read_local_file` (verifying regular non-symlink metadata before opening) feed observations into `Selector`. Stream state in `LocalStream` holds only the root label and candidate iterator without retaining redundant root path allocations.
- Digest cap (`MAX_OUTPUT_SIZE`) is strictly enforced on bytes actually written to the output stream via `BoundedEstimatingWriter`, preventing framing/metadata overflow.
- Token count is estimated on the same writer stream pass over rendered bytes.
- `BudgetTrie` stores nodes in a flat continuous 16-byte Left-Child Right-Sibling (LCRS) arena (`Vec<BudgetTrieNode>`) with compact contiguous labels in `label_arena: String`, packed bitwise flags (`FLAG_DIR`), and a unified root directory node at index 0. It is format-neutral; XML entity and newline escaping occurs on-the-fly during `render_to_writer(writer, escape_xml)`. Path components exceeding `MAX_COMPONENT_LEN` (65,535 bytes) are rejected atomically during insertion, returning `IngestError::PathComponentTooLong` through `Selector::finish()`.
- CLI arguments are parsed linearly into options and source in pure core (`core::cli`), supporting order-independent sources, short flags (`-o`, `-f`, `-r`, `-h`), key-value `=` syntax, and POSIX `--` delimiters.
- Candidate budget uses run-length encoding (`dir_runs: Vec<(CompactDir, usize)>`) on contiguous directory entries during collection with zero-allocation 24-byte inline strings (`CompactDir`), avoiding `HashMap` overhead on the happy path while preserving deterministic overflow diagnostics sorted by count descending then directory ascending.
- Digest budget is tracked directly in `Selector` and is error-terminal: limit breach constructs complete `IngestError::OutputTooLarge` diagnostics without internal transactional rollback.

### Network & GitHub

- Single HTTP request streams GitHub repository archive (`/repos/{owner}/{repo}/tarball/{ref}`) via `flate2` and `tar` over a 64 KiB `BufReader` buffered stream, streaming entries into `Selector::decide` with zero intermediate disk files and fail-fast budget termination. Commit revision is extracted from the archive `pax_global_header`.
- HTTP credentials (`Authorization: Bearer <token>`) attach strictly to exact-origin GitHub API (`https://api.github.com`); foreign origins never receive credentials.
- HTTP transport retries transient transport errors (`Io`, `Timeout`, `ConnectionFailed`) and HTTP 429 / 5xx responses with bounded exponential backoff and jitter up to `MAX_RETRIES` total attempts (1 initial + up to 2 retries). Fails immediately on non-transient transport errors and 4xx client errors (except 429).

## Limits and Tunables (`src/core/policy.rs`)

| Constant | Value | Purpose |
| --- | --- | --- |
| `MAX_FILE_SIZE` | 1 MiB | Maximum individual file size accepted |
| `MAX_OUTPUT_SIZE` | 20 MiB | Maximum rendered digest size |
| `MAX_FILE_COUNT` | 10,000 | Maximum candidate files evaluated |
| `ARCHIVE_MAX_SECS` | 300 s | Maximum duration for whole-archive Tarball transfer |
| `MAX_RETRIES` | 3 | Maximum total request attempts (1 initial + up to 2 retries) |
| `RETRY_INITIAL_DELAY_MS` | 250 ms | Base initial delay for exponential backoff |
| `RETRY_MAX_DELAY_MS` | 5,000 ms | Maximum delay cap for exponential backoff |
| `NOISE` | lockfiles | Conventional lockfiles excluded before candidate budget |
| `SENSITIVE_EXTENSIONS` | 5 extensions | Key, cert, and credential extensions (`*.pem`, `*.key`, `*.p12`, `*.pkcs12`, `*.credentials`) excluded before candidate budget (note: `credentials*` is also matched as filename prefix) |
| `BINARY_EXTENSIONS` | 11 extensions | Known binary file extensions filtered pre-download |

## Functional core / imperative shell boundary

Boundary rule: pure core code performs no I/O, clock, network, or entropy and
never depends on shell code; shell code depends on the core in one direction.

- Pure core (`src/core/`):
  - `cli.rs`: Pure CLI argument parsing, help detection, and `CliAction` generation (`parse`).
  - `policy.rs`: all operational limits, noise patterns, sensitive patterns, binary extensions, and
    classification predicate (`PathClassification::classify`).
  - `source.rs`: source string and OS-string parsing.
  - `model.rs`: domain value types (`Source`, `FileRecord`, `CollectionResult`,
    `IngestReport`, `RunConfig`, `OutputTarget`, `DigestFormat`, `FileStorage`).
  - `error.rs`: domain error enum `IngestError` packed into 16 bytes for register-return ABI (`rax`/`rdx`), with boxed diagnostic payloads (`CandidateLimitExceededError`, `PathComponentTooLongError`).
  - `decide/` (`mod.rs`, `item.rs`, `budget.rs`, `selector.rs`): candidate observation, candidate budget RLE accounting, `Selector` storage accumulation, and `largest_files` diagnostic selection.
  - `tree.rs`: Format-neutral ASCII hierarchy tree (`BudgetTrie`, `BudgetTrieNode`, `InsertError`) in flat 16-byte LCRS arena.
  - `render/` (`mod.rs`, `text.rs`, `xml.rs`, `token.rs`): Format-specific streaming serializers (Text/XML), escaping (`escape_header`, `xml_escape`), `TokenEstimator` heuristic token state machine, `render_to_writer`, and streaming `BoundedEstimatingWriter`.
  - `github_url.rs`: Pure `std` URL construction with RFC 3986 percent-encoding and origin credential scoping.
  - `archive.rs`: Pax header comment extraction and archive root stripping.
- Imperative shell (`src/shell/`):
  - `src/main.rs`: binary entry point and user-facing CLI completion report formatting.
  - `shell/orchestrate.rs`: Pipeline orchestration (`ingest`) and unified streaming collection driver (`SourceStream`, `collect_stream`).
  - `shell/local.rs`: 2-step linear filesystem candidate discovery and streaming reader (`LocalStream`).
  - `shell/local_io.rs`: Non-symlink file metadata validation and bounded streaming reads.
  - `shell/github_client.rs`: HTTP transport, retry diagnostics, body bounding, and token resolution.
  - `shell/github.rs`: streaming archive tarball entry iterator (`GithubStream`).
  - `shell/output.rs`: atomic file writing via RAII commit guard (`AtomicFile`), standard output stream emission, and lexical output path filtering (`OutputFilter`).
  - `shell/mod.rs`: shell module declarations, centralized `current_epoch_nanos()`, and scoped test fixture isolation (`TempDirGuard`).
