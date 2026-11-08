# Domain Context -- codefeed

Defines domain terms, invariants, and functional-core / imperative-shell boundaries. Filtering and accounting rules: [security sec. 2-4](docs/security_and_filtering.md); escaping: [security sec. 7](docs/security_and_filtering.md); network safety: [security sec. 6](docs/security_and_filtering.md).

## 1. Purpose

Converts a local directory or public GitHub repository into a single text digest for LLMs. Every run produces a complete digest or fails atomically; partial output is never emitted.

## 2. Glossary

### Inputs and Configuration

- `Source`: `Local(PathBuf)` or `Github { owner, repo, reference }` (`#[non_exhaustive]`). Parsed via `Source::parse_os` / `Source::parse`.
- `GithubName`: Validated owner/repo name rejecting empty, `.`, `..`, and invalid characters (`Deref<Target = str>`).
- `GitRef`: Validated, trimmed revision rejecting `.` and `..` (`Deref<Target = str>`).
- `OutputTarget`: `File(PathBuf)` or `Stdout`.
- `ResolvedOutput`: Filesystem-resolved target (`shell`-private, from `resolve_output_target`).
- `DigestFormat`: `Text` or `Xml`.
- `ExcludePatterns`: Validated user patterns in `.gitignore` syntax, relative to source root, case-sensitive. Each pattern yields one rule. `\` matches as U+FFFD on all platforms.
- `RunConfig`: Aggregate of `source`, `output`, `format`, and `exclude`.

### Candidate Collection

- `CandidatePath`: `Valid(NormalizedPath)` or `Unstorable(prefix)`. Ignored entries are `None` at classification and discarded before selection.
- `NormalizedPath`: Non-empty, slash-separated UTF-8 relative path without empty or `.` segments, within `MAX_PATH_BYTES` (I-10).
- `Selector`: Linear state managing arena storage, `CandidateBudget`, seen-path set for uniqueness, and skipped tally. Settles candidates via `observe` and completes via `finish`.
- `Observation`: Input to `Selector::observe`: `Unstorable(prefix)` or `File(NormalizedPath, Body)` (`Body`: `Unavailable`, `OverLimit`, `Bytes`).
- `CandidateBudget`: Candidate counter and per-directory tallies (`selector.rs` private).
- `CollectionResult`: `label`, optional `revision`, `files: SortedFiles`, and `skipped`.

### Storage

- `FileStorage`: Append-only `path_arena` and `content_arena` indexed by `spans` vector. Read via `iter` yielding `FileRecord`.
- `FileRecord<'a>`: Borrowed `path` and `content` view.
- `SortedFiles`: `FileStorage` sorted by `cmp_path` where no stored path is a directory prefix of another (I-01). Uniqueness enforced upstream by `Selector::observe`. Implements `Deref<Target = FileStorage>`.

### Rendering and Results

- `BoundedBuffer`: In-memory writer aborting when output exceeds `MAX_OUTPUT_SIZE`.
- `RenderedDigest`: Struct of `bytes` and `estimated_tokens` without delivery target knowledge.
- `IngestReport`: `output` (from `RunConfig`), `included_files`, `skipped_files`, `estimated_tokens` (`#[non_exhaustive]`).

### Errors

- `ErrorDetail`: Sanitized error text constructed exclusively via `ErrorDetail::new` (`Deref<Target = str>`).
- `DomainError`: Core failures (`Usage`, `InvalidSource`, `UnsupportedSource`, `InvalidMetadata` carrying `ErrorDetail`; `CandidateLimitExceeded`; `OutputTooLarge(OutputTooLargeError)`).
- `IngestError`: Shell error wrapping `DomainError`, `Io`, `Http(ErrorDetail)`, and `Api(GithubApiError)`.

## 3. Invariants

### 3.1 Input Parsing

- Arguments and URLs parse into value types at boundary (`cli::parse`, `Source::parse`, `GitRef::new`, `GithubName::new`). Core assumes validity without re-validation.
- `--ref` rejected for local sources.
- `-x`/`--exclude` patterns accumulate in argument order and compile once via `ExcludePatterns::new`. Invalid patterns cause usage errors.
- GitHub URL grammar: [README Sources](README.md#sources).

### 3.2 Path Classification

- Order: [security sec. 2](docs/security_and_filtering.md#2-evaluation-order). Tar streams classify via `classify_path_segments`. Local walks admit directories via `admit_dir` and files via `classify_leaf`.
- `Ignored` entries consume no candidate slots and are omitted from skipped counts.
- Run-aborting checks (depth, tar path normalization, oversized tar headers): [security sec. 2](docs/security_and_filtering.md#2-evaluation-order); header budget is I-03 ([sec. 3.9](#39-network)).
- Unstorable candidates record valid prefix (`"a/b/"` or `""`), charged to budget under first segment (`"a/"`) or root (`"./"`).
- **I-10 (`type`, `NormalizedPath`, `classify_leaf`):** A `Valid` path is at most `MAX_PATH_BYTES` bytes. `NormalizedPath` is private and `classify_leaf` is its sole non-test constructor returning `Unstorable` on overflow, so `Selector` needs no path-length check.
- **I-02 (`boundary`, `admit_dir`, `classify_leaf`):** No classified path or unstorable prefix has a first segment starting with `.`, since hidden directories are never admitted, hidden files are `Ignored`, and empty/`.` segments are dropped. Root entries key as `"./"`, which no real directory produces.

### 3.3 Accounting

- Slot allocation and skipped rules: [security sec. 4](docs/security_and_filtering.md#4-candidate-and-skipped-accounting).
- `Selector::observe` charges a candidate slot prior to inspecting content.
- Read timing: Sources read bounded bodies before `Selector::observe`, except when declared size exceeds `MAX_FILE_SIZE` (`OverLimit`, skipped without reading). Local workers read concurrently; archive streams read in-place and drain to next header.
- **I-11 (`type`, `Observation`):** Unstorable paths carry no body; storable paths always carry a `Body`. Neither bodies for unstorable paths nor storable paths without read outcomes can be constructed.

### 3.4 Storage and Ordering

- `FileStorage::push` rejects candidates before allocation if raw arena bytes would exceed `MAX_OUTPUT_SIZE`.
- `SortedFiles::new` sorts spans via `cmp_path` (component-wise), guaranteeing deterministic output across concurrency timings.
- **I-01 (`runtime`, `SortedFiles::new`):** No stored path is a directory prefix of another stored path. Enforced after sorting by checking adjacent pairs. A schema, type, or boundary check cannot express this because the rule relates separate entries, and a per-entry parse cannot see the others. Behavior: [security sec. 3.4](docs/security_and_filtering.md#34-path-limits).
- **I-09 (`runtime`, `Selector::observe`):** No two candidates share a path. The seen-path set relates separate entries, so no per-entry type can express it. Behavior: [security sec. 3.4](docs/security_and_filtering.md#34-path-limits).
- **I-05 (`boundary`, `extract_dir_slice`):** Candidate budget keys are at most `MAX_PATH_BYTES` bytes ending on a UTF-8 char boundary. Cut from the first segment at creation, bounding key storage to candidates times `MAX_PATH_BYTES`. A `type` or `schema` cannot express a length cap on a `&str` key, and `CandidateBudget::add` needs no separate check.
- Arena indices are `u32`. Assertions in `policy/limits.rs` verify `MAX_OUTPUT_SIZE + MAX_FILE_SIZE + MAX_PATH_BYTES <= u32::MAX` ([coding convention sec. 3.3](docs/coding_convention.md#33-type-invariants)).

### 3.5 Rendering

- **Escaping:** Single-pass via `write_escaped` per [security sec. 7](docs/security_and_filtering.md#7-output-sanitization).
- **Tree:** Deterministic tree generated from `SortedFiles` in one pass (`tree.rs`). Relies on `NormalizedPath` non-empty segments without trailing `/`.
- **Header and token estimate:** [README Output](README.md#output). Local root uses `root`.

### 3.6 Output Delivery

- `resolve_output_target` resolves destination once at startup, excluding it from traversal. `write_atomic` writes atomically: [security sec. 5](docs/security_and_filtering.md#5-local-filesystem-safety).
- `deliver` buffers digest in memory, emitting only on success. Summary streams and broken pipe handling: [README Output](README.md#output). File target path in summary sanitized via `ErrorDetail::new`.

### 3.7 Diagnostics

- Overflow diagnostics generate only on error paths: [README Limits](README.md#limits).
- `MAX_OUTPUT_SIZE` lists `OVERFLOW_LARGEST_FILES` files (sorted size descending, then path).
- `MAX_FILE_COUNT` lists `OVERFLOW_LARGEST_DIRECTORIES` directories (sorted count descending, then name).
- Overflow names stored as `ErrorDetail`, sanitized during construction. Sort runs on raw names first.

### 3.8 Local Traversal Pipeline

1. Depth-first walk in file-name order without link-following (`Walk`, `walk.rs`). Excluded directories pruned before `.gitignore` read. Other directories read `.gitignore` once and compile rules from those bytes (failures: [security sec. 3.5](docs/security_and_filtering.md#35-ignore-files-local-only)).
2. Up to `MAX_LOCAL_WORKERS` workers inspect metadata, read bounded bodies, and return on bounded channels (`LOCAL_CHANNEL_CAPACITY`).
3. Main thread settles slots in walk order into `Selector::observe`. First error halts traversal.
4. Final digest order and I-01 established by `SortedFiles::new`; uniqueness by `Selector::observe`.

### 3.9 Network

- Request parameters, retry classes, timeouts: [security sec. 6](docs/security_and_filtering.md#6-github-network-safety).
- Archive entries stream into `Selector::observe` without disk writes.
- Revision SHA extracts from first `ARCHIVE_PAX_GLOBAL_BYTES` of PAX header, falling back to requested ref.
- Stream/gzip/tar failures map to `IngestError::Http`.
- **I-03 (`runtime`, `shell/github/archive.rs`):** One `Entries::next` call reads at most `ARCHIVE_MAX_HEADER_BYTES`. tar reads extension bodies internally, so no type or boundary can express the bound. A byte budget on the shared reader enforces it, and each entry body is drained before the next call so only headers are metered. Behavior: [security sec. 2 and 6.4](docs/security_and_filtering.md#64-streaming).
- **I-04 (`type`, `ErrorDetail::new`):** Every error text that may echo user input or remote payloads is an `ErrorDetail`, whose sole constructor sanitizes. Direct `DomainError` builds cannot store unsanitized text, and `Display` does not sanitize again. Non-`DomainError` effects (`io::Error`, `GithubApiError`, retry warnings) sanitize via `ErrorDetail::new`. Behavior: [security sec. 7](docs/security_and_filtering.md#7-output-sanitization).
- **I-06 (`type`, `Auth`, `api_error`):** `api_error` is the sole constructor of token hints and requires `Auth`. A request bearing a token never receives the hint, including errors from redirect targets. Behavior: [security sec. 6.1](docs/security_and_filtering.md#61-authentication).
- **I-07 (`boundary`, `GithubClient::credentials`):** Only GitHub API origins receive tokens. Authorization headers and error `Auth` derive from `credentials(url)`, preventing divergence. Fields are private to `client.rs`.
- **I-08 (`boundary`, `TOKEN_ENV_VARS`):** Token variable names exist solely in `TOKEN_ENV_VARS` (`src/core/policy/mod.rs`), read by lookup order and token hints. A type cannot express a name list, making single definition the enforcement.

## 4. Limits and Tunables

Authority and mirror rules: [security header](docs/security_and_filtering.md). Limit relations verified by compile-time assertions in `policy/limits.rs` and `github/retry.rs` ([coding convention sec. 3.3](docs/coding_convention.md#33-type-invariants)).

## 5. Functional Core and Imperative Shell

Purity rules and one-way Shell-to-Core dependency: [coding convention sec. 3](docs/coding_convention.md#3-architecture-functional-core-imperative-shell).

### Core (`src/core/`)

- `cli.rs`: CLI argument parsing, help, and version text.
- `path.rs`: Path types and classification.
- `policy/`: Limits, exclusions, user exclude patterns, `TOKEN_ENV_VARS`.
- `source.rs`: `Source` and `GitRef` parsing; URLs delegated to `github/`.
- `model.rs`: Configuration and report types.
- `selector.rs`: Candidate settlement, budget, and `CollectionResult`.
- `storage.rs`: Arena storage and path ordering.
- `error.rs`: Domain errors and `ErrorDetail`.
- `github/`: URL grammar, tar normalization, retry logic, API error types.
- `render/`: Text/XML formatting, escaping, tree rendering, output budget, token estimation.

### Shell (`src/shell/`)

- `mod.rs`: `ingest` pipeline.
- `cli.rs`: CLI orchestration, stream selection, exit codes.
- `error.rs`: `IngestError` and transport error mapping.
- `local/`: Local walk, `.gitignore` loading, producer/worker reads.
- `github/`: Archive fetch with retries, tarball streaming, TLS client, token handling.
- `atomic.rs`: Atomic file writes.
- `output.rs`: Destination resolution and delivery.
- `stdio.rs`: stdout/stderr line writers.
- `testing.rs`: Test helpers.

### Public API (`src/lib.rs`)

Exports: `ingest`, `RunConfig`, `ExcludePatterns`, `Source`, `GitRef`, `GithubName`, `OutputTarget`, `DigestFormat`, `IngestReport`, `IngestError`, `DomainError`, `ErrorDetail`, `CandidateLimitExceededError`, `OutputTooLargeError`, `GithubApiError`, and hidden `cli::run_cli`. All other items are `pub(crate)`.
