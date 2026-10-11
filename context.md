# Domain Context — codefeed

## Purpose

Convert a local directory or public GitHub repository into one LLM-friendly
text digest. Public behavior, limits, and CLI contract are documented in
`README.md`; this file fixes domain terms and the functional-core /
imperative-shell boundary.

## Entities and value objects

- `Source` — `Local(PathBuf)` or `GitHub { owner, repo }`.
- `Item` — one observed candidate handed to the pure core: `path`
  (`ItemPath::Utf8 | NonUtf8`) and `probe`
  (`Probe::Bytes | Missing | TooLarge | Ignored`).
- `FileRecord` — digest entry: `path`, `content`.
- `CollectionResult` — uniform collection record: `label`, optional `revision`, `files`, `skipped`, `trie`.
- `IngestReport` — `output`, `included_files`, `skipped_files`, `estimated_tokens`.
- `OutputTarget` — digest destination: `File(PathBuf)` or `Stdout`.
- `DigestFormat` — output serialization format: `Text` or `Xml`.
- Budgets — `CandidateBudget` (candidate count), `DigestBudget` (rendered
  bytes and skipped count).
- Policy — centralised tunables, thresholds, noise lists, sensitive patterns, and extension sets
  (`policy.rs`).

## Invariants

### Filtering & Accounting
- Excludes `.git`, hidden entries, symlinks, non-UTF-8 paths/content, lockfiles (`Cargo.lock`, `package-lock.json`, `yarn.lock`, `pnpm-lock.yaml`, `go.sum`), and sensitive files (`.env*`, `*.key`/`*.pem`, `*.p12`/`*.pkcs12`, `id_rsa*`/`id_ed25519*`, `credentials*`) before candidate accounting. They are neither candidates nor `skipped`.
- Consumes candidate slots before evaluating content, size, or encoding.
- `skipped` counts candidates rejected for content, size, or encoding. Ignored paths are never counted in `skipped`.
- Rejects content exceeding `MAX_OUTPUT_SIZE`; overflow content is never emitted.
- Known binary extensions (`BINARY_EXT`) are classified as `Probe::Missing` before remote download to save bandwidth while preserving candidate and skipped accounting.

### Rendering & Formats
- ASCII hierarchy tree (`├── `, `└── `, `│   `) matches `DigestBudget` byte-for-byte, escaping control codes and XML entities to prevent tree line-break injections.
- Streaming heuristic token estimation runs over the full digest stream, reported in `IngestReport.estimated_tokens` and completion messages (`~N tokens`).
- XML serialization (`--format xml`) structures output into `<repository>`, `<directory_structure>`, and `<files><file path="...">` tags, escaping XML entities (`&`, `<`, `>`, `"`, `'`) and sanitizing disallowed XML 1.0 control characters to `\u{FFFD}`. Overhead and escaped bytes are accounted for in `DigestBudget`.
- File-path ordering is strictly lexicographical and owned solely by `render_digest_to_writer` / `render_digest`.
- Local digest headers use root directory basename (`Repository: <basename>`), omitting absolute paths and system prefixes.

### I/O & Output Target Contracts
- File target writes atomically: temporary file, disk sync, rename.
- Stdout target writes directly to stdout with no temporary file or filesystem sync.
- Completion messages emit to stdout for file targets and stderr for stdout targets.
- Broken downstream pipe (`BrokenPipe` / `EPIPE`) on stdout exits cleanly with status 0.
- Local walk excludes the output file by identity (case-insensitively on Windows and macOS). Stdout target normalizes and excludes nothing.
- Local file reads never follow symlinks or block on special files (`O_NOFOLLOW` / `O_NONBLOCK`). Missing files are treated as `Probe::Missing` without aborting.
- Local walk uses multi-threaded `ignore::WalkParallel` with strict deterministic lexicographical sorting before `Selector` consumption.
- CLI arguments are decoded via single-pass `ArgTokenizer` into semantic tokens, supporting order-independent sources, short flags (`-o`, `-f`, `-r`, `-h`), key-value `=` syntax, and POSIX `--` delimiters without third-party parser dependencies.

### Network & GitHub
- Scoped threads fetch GitHub blobs concurrently in batches (`FETCH_WINDOW = 16`), feeding items to `Selector::decide` in deterministic lexicographical path order matching local collection.
- HTTP credentials (`Authorization: Bearer <token>`) attach strictly to exact-origin GitHub API and raw-content hosts; foreign origins never receive credentials.
- HTTP transport retries transient transport errors and HTTP 429 / 5xx responses with bounded exponential backoff and jitter up to `MAX_RETRIES` total attempts (1 initial + up to 2 retries). Fails immediately on 4xx client errors (except 429).

## Limits and Tunables (`src/core/policy.rs`)

| Constant | Value | Purpose |
| --- | --- | --- |
| `MAX_FILE_SIZE` | 1 MiB | Maximum individual file size accepted |
| `MAX_OUTPUT_SIZE` | 20 MiB | Maximum rendered digest size |
| `MAX_FILE_COUNT` | 10,000 | Maximum candidate files evaluated |
| `MAX_JSON_SIZE` | 8 MiB | Maximum GitHub API response body |
| `FETCH_WINDOW` | 16 | Number of concurrent remote download threads |
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
    `IngestReport`, `Options`, `OutputTarget`, `DigestFormat`).
  - `error.rs`: domain error enum `IngestError` with structured diagnostic payload.
  - `decide.rs`: `Selector::decide`, `CandidateBudget`, item evaluation, and
    candidate overflow diagnostics.
  - `digest/`: `DigestBudget`, defense-in-depth size check, deterministic
    digest rendering (`render_digest_to_writer`), format implementations (`text.rs`, `xml.rs`), trie representation (`trie.rs`), and token estimation (`token.rs`).
  - `github_url.rs`: URL construction and origin credential scoping.
  - `github_tree.rs`: GitHub tree DTO parsing and blob verification.
- Imperative shell (`src/shell/`):
  - `shell/cli.rs`: Pure CLI argument parsing, help detection, and `CliAction` generation.
  - `shell/orchestrate.rs`: Pipeline orchestration (`ingest`) coordinating collection, emission, and digest rendering.
  - `src/main.rs`: binary entry point and user-facing CLI completion report formatting.
  - `shell/local.rs`: filesystem walking and collection (`collect_local`).
  - `shell/local_io.rs`: safe, non-following bounded file reads (`read_local_file`).
  - `shell/output_exclusion.rs`: output file identity detection and path normalization.
  - `shell/github_client.rs`: HTTP transport, body bounding, and token resolution.
  - `shell/github.rs`: windowed parallel GitHub tree download orchestration (`collect_github`).
  - `shell/output.rs`: atomic file writing or standard output stream emission.
