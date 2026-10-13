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
- `Collector` / `AnyCollector` — imperative shell collection abstraction (`trait Collector`) and static dispatch enum (`AnyCollector { Local(LocalCollector), GitHub(GitHubCollector) }`).
- `IngestReport` — `output`, `included_files`, `skipped_files`, `estimated_tokens`.
- `OutputTarget` — digest destination: `File(PathBuf)` or `Stdout`.
- `DigestFormat` — output serialization format: `Text` or `Xml`.
- `FormatFrameMetrics` — format frame overhead value object (`fixed_frame`, `revision_overhead`, `per_file`) with associated compile-time constants (`TEXT`, `XML`) decoupling serialization specifications from `DigestBudget` accounting.
- Budgets — `CandidateBudget` (candidate count), `DigestBudget` (rendered
  bytes and skipped count, composing `ContentMetrics` value object for checked file accounting).
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

- ASCII hierarchy tree (`├── `, `└── `, `│   `) matches `DigestBudget` byte-for-byte, escaping control codes and XML entities to prevent tree line-break injections.
- `DigestBudget` accounts tree rendering size via a mathematically proven per-node depth upper bound (`TREE_DEPTH_BYTE_FACTOR * depth + TREE_NODE_BASE_OVERHEAD + display_len` = `6 * depth + 12 + display_len`), guaranteeing that fast-path budget checks never underestimate rendered output size and strictly preventing breaches of `MAX_OUTPUT_SIZE`. When approaching the threshold, it falls back to exact size calculation.
- Format frame overheads and tag lengths (`XML_FIXED_FRAME_BYTES`, `XML_PER_FILE_TAG_BYTES`, `TEXT_FIXED_FRAME_BYTES`, `TEXT_FILE_BLOCK_BYTES`) are compile-time composite constants unified under associated constants `FormatFrameMetrics::TEXT` and `FormatFrameMetrics::XML` in `DigestFormat::rendered_base_size`, eliminating runtime wrapper overhead. Text metadata digit overhead (`extra_metadata_bytes`) is computed via pure mathematical `const fn num_digits`, eliminating heap allocations during budget checks.
- Streaming heuristic token estimation runs over the full digest stream, reported in `IngestReport.estimated_tokens` and completion messages (`~N tokens`).
- Plain text serialization (`--format text`) preserves all valid UTF-8 file content verbatim (rejecting only NUL bytes) to maintain raw fidelity for LLM prompts; headers and paths are escaped, but file bodies are unmutated (viewing raw digests in interactive terminals may affect emulator state if files contain ANSI escape codes).
- XML serialization (`--format xml`) structures output into `<repository>`, `<directory_structure>`, and `<files><file path="...">` tags, escaping XML entities (`&`, `<`, `>`, `"`, `'`) and sanitizing disallowed XML 1.0 control characters to `\u{FFFD}`, unified under `XmlChunks<'a>` streaming iterator across metric measurement, string assembly, and I/O writes. Overhead and escaped bytes are accounted for in `DigestBudget`.
- File-path ordering is strictly lexicographical and owned solely by `render_digest_to_writer` / `render_digest`. Format rendering consumes `IntoIterator<Item = &FileRecord>` to support streaming and arbitrary iteration pipelines while preserving deterministic path sorting.
- Local digest headers use root directory basename (`Repository: <basename>`), omitting absolute paths and system prefixes.

### I/O & Output Target Contracts

- File target writes atomically via RAII guard (`AtomicFile`): temporary file creation, streaming write, disk sync, explicit handle release, atomic rename, and automatic drop cleanup on error or panic.
- Stdout target writes directly to stdout with no temporary file or filesystem sync.
- Completion messages emit to stdout for file targets and stderr for stdout targets.
- Broken downstream pipe (`BrokenPipe` / `EPIPE`) on stdout exits cleanly with status 0.
- Local walk excludes the output file by identity (case-insensitively on Windows and macOS). Stdout target normalizes and excludes nothing.
- Local file reads never follow symlinks or block on special files (`O_NOFOLLOW` / `O_NONBLOCK`). Intermediate directory symlinks and junctions are prevented via `openat2` (Linux), component-wise `openat` verification (Unix), or `GetFinalPathNameByHandleW` handle canonical component verification against `canonical_root` (Windows). Missing files and verified root escapes are treated as `Probe::Missing` without aborting; generic permission denied and FFI errors fail closed with `IngestError::Io`.
- Local walk uses multi-threaded `ignore::WalkParallel` with thread-local batching (`Vec` chunk buffer flushed on drop or at 128 items into a `Mutex<Vec<LocalCandidate>>`), relative path single-buffer assembly (`normalize_relative_path`), and deterministic sorting (`sort_unstable_by`) before `Selector` consumption. Probing and budget evaluation execute in chunks of `FETCH_WINDOW`, utilizing a scoped worker pool within `std::thread::scope` with work-stealing atomic indices per window, flat-mapped thread joins, and index sorting. Candidate ownership is consumed by value per window so in-flight memory remains bounded and early budget errors preempt subsequent I/O.
- Windows containment checks (`is_strictly_beneath`) verify handle canonical paths (`GetFinalPathNameByHandleW`) against `canonical_root` component-wise without intermediate heap allocations.
- Trie insertion is iterative and non-recursive with $O(1)$ fast-path sibling comparison against previously inserted nodes (`find_child`), avoiding stack frame exhaustion on deeply nested directories. `BudgetTrieNode` leverages `Cow<'_, str>` and sparse escaping (`Option<String>`), eliminating redundant heap allocations for unescaped paths. Trie rendering reuses a single mutable `String` buffer across recursive node visits (`push_str` / `truncate`), achieving zero heap allocations during serialization.
- CLI arguments are parsed linearly into options and source without third-party dependencies, supporting order-independent sources, short flags (`-o`, `-f`, `-r`, `-h`), key-value `=` syntax, and POSIX `--` delimiters.
- Candidate budget uses `HashMap` with `extract_dir_slice`, formatting overflow diagnostics from observed candidates at termination sorted deterministically by count descending then directory ascending.
- Digest budget is error-terminal: limit breach immediately errors and aborts without internal transactional rollback.

### Network & GitHub

- Single HTTP request streams GitHub repository archive (`/repos/{owner}/{repo}/tarball/{ref}`) via `flate2` and `tar`, streaming entries into `Selector::decide` with zero intermediate disk files and fail-fast budget termination. Commit revision is extracted from the archive `pax_global_header`.
- HTTP credentials (`Authorization: Bearer <token>`) attach strictly to exact-origin GitHub API (`https://api.github.com`); foreign origins never receive credentials.
- HTTP transport retries transient transport errors (`Io`, `Timeout`, `ConnectionFailed`) and HTTP 429 / 5xx responses with bounded exponential backoff and jitter up to `MAX_RETRIES` total attempts (1 initial + up to 2 retries). Fails immediately on non-transient transport errors and 4xx client errors (except 429).

## Limits and Tunables (`src/core/policy.rs`)

| Constant | Value | Purpose |
| --- | --- | --- |
| `MAX_FILE_SIZE` | 1 MiB | Maximum individual file size accepted |
| `MAX_OUTPUT_SIZE` | 20 MiB | Maximum rendered digest size |
| `MAX_FILE_COUNT` | 10,000 | Maximum candidate files evaluated |
| `FETCH_WINDOW` | 16 | Number of concurrent local file reader threads in a batch |
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
  - `decide.rs`: `Selector::decide`, `CandidateBudget`, item evaluation, and
    candidate overflow diagnostics.
  - `digest/`: `DigestBudget`, defense-in-depth size check, deterministic
    digest rendering (`render_digest_to_writer`), format implementations (`text.rs`, `xml.rs`), trie representation (`trie.rs`), and token estimation (`token.rs`).
  - `github_url.rs`: URL construction and origin credential scoping.
  - `archive.rs`: Pax header comment extraction and archive root stripping.
- Imperative shell (`src/shell/`):
  - `shell/cli.rs`: Pure CLI argument parsing, help detection, and `CliAction` generation.
  - `shell/collector.rs`: Ingestion collector abstraction (`Collector` trait) and static dispatch (`AnyCollector`).
  - `shell/orchestrate.rs`: Pipeline orchestration (`ingest`) coordinating collection, emission, and digest rendering.
  - `src/main.rs`: binary entry point and user-facing CLI completion report formatting.
  - `shell/local.rs`: filesystem walking and collection (`LocalCollector`, `collect_local`).
  - `shell/local_io.rs`: safe, non-following bounded file reads (`read_local_file`).
  - `shell/output_exclusion.rs`: output file identity detection and path normalization.
  - `shell/github_client.rs`: HTTP transport, body bounding, and token resolution.
  - `shell/github.rs`: streaming archive GitHub collection (`GitHubCollector`, `collect_github`).
  - `shell/output.rs`: atomic file writing via RAII commit guard (`AtomicFile`) or standard output stream emission.
  - `shell/mod.rs`: shell module declarations and scoped test fixture isolation (`TempDirGuard`).
