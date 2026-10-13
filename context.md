# Domain Context — codefeed

## Purpose

Convert a local directory or public GitHub repository into one LLM-friendly
text digest. Public behavior, limits, and CLI contract are documented in
`README.md`; this file fixes domain terms and the functional-core /
imperative-shell boundary.

## Entities and value objects

- `Source` — `Local(PathBuf)` or `GitHub { owner, repo, reference }`.
- `RunConfig` — execution configuration unifying `source`, `output`, and `format`.
- `Item` — one observed candidate handed to the pure core: `path`
  (`ItemPath::Utf8 | NonUtf8`) and `probe`
  (`Probe::Bytes | Missing | TooLarge | Ignored`).
- `FileRecord` — digest entry: `path`, `content`.
- `CollectionResult` — uniform collection record: `label`, optional `revision`, `files`, `skipped`, `trie`, `format`.
- `IngestReport` — `output`, `included_files`, `skipped_files`, `estimated_tokens`.
- `OutputTarget` — digest destination: `File(PathBuf)` or `Stdout`.
- `DigestFormat` — output serialization format: `Text` or `Xml`.
- Budgets — `CandidateBudget` (candidate count), `DigestBudget` (content bytes and skipped count, tracking `MAX_OUTPUT_SIZE`).
- Policy — centralised tunables, thresholds, noise lists, sensitive patterns, and extension sets
  (`policy.rs`).

## Invariants

### Filtering & Accounting

- Excludes `.git`, hidden entries, symlinks, non-UTF-8 paths/content, lockfiles (`Cargo.lock`, `package-lock.json`, `yarn.lock`, `pnpm-lock.yaml`, `go.sum`), and conventional sensitive files by case-insensitive filename/extension (`.env*`, `*.key`/`*.pem`, `*.p12`/`*.pkcs12`, `id_rsa*`/`id_ed25519*`, `credentials*`) before candidate accounting (conventional pattern filter, not a content-based secret scanner). They are neither candidates nor `skipped`.
- Consumes candidate slots before evaluating content, size, or encoding.
- `skipped` counts candidates rejected for content, size, or encoding. Ignored paths are never counted in `skipped`.
- Rejects content exceeding `MAX_OUTPUT_SIZE`; overflow content is never emitted.
- Known binary extensions (`BINARY_EXT`) are classified as `Probe::Missing` before remote download to save bandwidth while preserving candidate and skipped accounting.
- Diagnostic largest files reporting (`largest_files`) uses bounded Top-5 selection with linear position lookup, avoiding full-collection cloning or $O(N \log N)$ sorting while preserving insertion stability for tie-breakers.

### Rendering & Formats

- ASCII hierarchy tree (`├── `, `└── `, `│   `) renders deterministic directory structure, escaping control codes and XML entities to prevent tree line-break injections.
- Heuristic token estimation runs via state machine `TokenEstimator`, exposed through pure function `estimate_tokens(&str)` and `BoundedEstimatingWriter`, reported in `IngestReport.estimated_tokens` and completion messages (`~N tokens`).
- Plain text serialization (`--format text`) preserves all valid UTF-8 file content verbatim (rejecting only NUL bytes) to maintain raw fidelity for LLM prompts; headers and paths are escaped, but file bodies are unmutated (viewing raw digests in interactive terminals may affect emulator state if files contain ANSI escape codes).
- XML serialization (`--format xml`) structures output into `<repository>`, `<directory_structure>`, and `<files><file path="...">` tags, escaping XML entities (`&`, `<`, `>`, `"`, `'`) and sanitizing disallowed XML 1.0 control characters to `\u{FFFD}`.
- File-path ordering is strictly lexicographical and owned solely by `render_digest_to_writer` / `render_digest`. Format rendering consumes `IntoIterator<Item = &FileRecord>` to support streaming and arbitrary iteration pipelines while preserving deterministic path sorting.
- Local digest headers use root directory basename (`Repository: <basename>`), omitting absolute paths and system prefixes.

### I/O & Output Target Contracts

- File target writes atomically via RAII guard (`AtomicFile`): temporary file creation, streaming write, disk sync, explicit handle release, atomic rename, and automatic drop cleanup on error or panic.
- Stdout target writes directly to stdout with no temporary file or filesystem sync.
- Completion messages emit to stdout for file targets and stderr for stdout targets.
- Broken downstream pipe (`BrokenPipe` / `EPIPE`) on stdout exits cleanly with status 0.
- Local walk excludes the output file by device/inode and path identity (`output_exclusion::OutputFilter`).
- Local walk uses a 2-step linear pipeline with TOCTOU-resistant containment checks:
  1. `ignore::WalkBuilder` with `follow_links(false)` collects candidate paths into a deterministically sorted list.
  2. Bounded, containment-checked file reads via `local_io::read_local_file` (`openat2(RESOLVE_BENEATH)` / non-blocking Win32 handles) feed observations into `Selector`.
- Digest cap (`MAX_OUTPUT_SIZE`) is strictly enforced on bytes actually written to the output stream via `BoundedEstimatingWriter`, preventing framing/metadata overflow.
- Token count is estimated on the same writer stream pass over rendered bytes.
- Trie insertion is iterative with fast-path sibling comparison (`find_child`), avoiding stack frame exhaustion on deeply nested directories.
- CLI arguments are parsed linearly into options and source without third-party dependencies, supporting order-independent sources, short flags (`-o`, `-f`, `-r`, `-h`), key-value `=` syntax, and POSIX `--` delimiters.
- Candidate budget uses `HashMap` with `extract_dir_slice`, formatting overflow diagnostics from observed candidates at termination sorted deterministically by count descending then directory ascending.
- Digest budget is error-terminal: limit breach immediately errors and aborts without internal transactional rollback.

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
| `SENSITIVE_EXT` | 5 extensions | Key, cert, credential extensions excluded before candidate budget |
| `BINARY_EXT` | 11 extensions | Known binary file extensions filtered pre-download |

## Functional core / imperative shell boundary

Boundary rule: pure core code performs no I/O, clock, network, or entropy and
never depends on shell code; shell code depends on the core in one direction.

- Pure core (`src/core/`):
  - `policy.rs`: all operational limits, noise patterns, sensitive patterns, binary extensions, and
    their predicates (`is_noise_path`, `is_sensitive_path`, `has_binary_extension`).
  - `source.rs`: source string and OS-string parsing.
  - `model.rs`: domain value types (`Source`, `FileRecord`, `CollectionResult`,
    `IngestReport`, `RunConfig`, `Options`, `OutputTarget`, `DigestFormat`).
  - `error.rs`: domain error enum `IngestError` with structured diagnostic payload.
  - `decide.rs`: `Selector::decide`, `Selector::finish`, `CandidateBudget`, item evaluation via `memchr`, and
    candidate overflow diagnostics.
  - `digest/`: `DigestBudget`, size check, deterministic
    digest rendering (`render_digest_to_writer`), format implementations (`text.rs`, `xml.rs`), trie representation (`trie.rs`), and token estimation (`TokenEstimator`, `estimate_tokens`).
  - `github_url.rs`: URL construction via `percent-encoding` and origin credential scoping.
  - `archive.rs`: Pax header comment extraction and archive root stripping.
- Imperative shell (`src/shell/`):
  - `shell/cli.rs`: Pure CLI argument parsing, help detection, and `CliAction` generation.
  - `shell/orchestrate.rs`: Pipeline orchestration (`ingest`) directly dispatching collection, emission, and digest rendering.
  - `src/main.rs`: binary entry point and user-facing CLI completion report formatting.
  - `shell/local.rs`: 2-step linear filesystem walking and collection (`collect_local`).
  - `shell/local_io.rs`: TOCTOU-resistant file opening and root containment enforcement (`openat2` / Win32 handle resolution).
  - `shell/output_exclusion.rs`: Output file identity tracking (`OutputFilter`) using device and inode checks.
  - `shell/github_client.rs`: HTTP transport, retry logging via `log`, body bounding, and token resolution.
  - `shell/github.rs`: streaming archive GitHub collection (`collect_github`).
  - `shell/output.rs`: atomic file writing via RAII commit guard (`AtomicFile`) or standard output stream emission.
  - `shell/mod.rs`: shell module declarations and scoped test fixture isolation (`TempDirGuard`).
