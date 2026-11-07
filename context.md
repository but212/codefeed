# Domain Context -- codefeed

Defines domain terms, core invariants, and the functional-core / imperative-shell boundary. Filtering and accounting rules live in [security sec. 2-4](docs/security_and_filtering.md), escaping in [security sec. 7](docs/security_and_filtering.md), and network safety in [security sec. 6](docs/security_and_filtering.md).

## 1. Purpose

`codefeed` converts a local directory or public GitHub repository into a single text digest for LLMs. Every run either produces a complete digest or fails atomically; partial digests are never emitted.

## 2. Glossary

### Inputs and configuration

- `Source`: `Local(PathBuf)` or `Github { owner, repo, reference }` (`Github` variant is `#[non_exhaustive]`). Constructed via `Source::github` or parsed at the boundary via `Source::parse_os` / `Source::parse`.
- `GithubName`: Validated GitHub owner/repo name. Rejects empty strings, `.`, `..`, and invalid characters. Implements `Deref<Target = str>`.
- `GitRef`: Validated, trimmed, non-empty revision (branch, tag, commit SHA). Rejects `.` and `..`. Implements `Deref<Target = str>`.
- `OutputTarget`: `File(PathBuf)` or `Stdout`.
- `ResolvedOutput`: Filesystem-resolved target (`shell`-private, constructed solely by `resolve_output_target`).
- `DigestFormat`: `Text` or `Xml`.
- `ExcludePatterns`: Validated user `--exclude` patterns in `.gitignore` syntax, relative to the source root and case-sensitive. Each pattern yields exactly one rule. A `\` in a path segment is matched as U+FFFD on every platform. Implements `Default` (no patterns) and `Eq` by pattern text.
- `RunConfig`: Struct of `source`, `output`, `format`, and `exclude`.

### Candidate collection

- `CandidatePath`: Valid candidate path (`Valid(NormalizedPath)` or `NonUtf8(prefix)`). Ignored entries are represented by `None` at the classification boundary and eliminated before selection.
- `NormalizedPath`: Non-empty, slash-separated, UTF-8 relative path without empty or `.` segments.
- `Selector`: Linear state managing arena storage, `CandidateBudget`, the seen-path set used for uniqueness, and skipped tally. Settles candidates via `observe` and completes via `finish`.
- `CandidateBudget`: Candidate counter and per-directory tallies (`selector.rs` private).
- `CollectionResult`: Struct containing `label`, optional `revision`, `files: SortedFiles`, and `skipped`.

### Storage

- `FileStorage`: Append-only `path_arena` and `content_arena` indexed by a `spans` vector. Read via `FileStorage::iter` yielding `FileRecord`.
- `FileSpan`: 16-byte POD of four `u32` arena offsets and lengths (`storage.rs` private).
- `FileRecord<'a>`: Borrowed `path` and `content` view.
- `SortedFiles`: `FileStorage` sorted by `cmp_path`, with no stored path a directory prefix of another (I-01). Path uniqueness is enforced upstream by `Selector::observe`. Implements `Deref<Target = FileStorage>`.

### Rendering and results

- `BoundedBuffer`: In-memory writer aborting when output would exceed `MAX_OUTPUT_SIZE`.
- `RenderedDigest`: Struct of `bytes`, `included_files`, `skipped_files`, and `estimated_tokens`. It knows no delivery target.
- `IngestReport`: Struct of `output` (the destination as requested in `RunConfig`, not canonicalized), `included_files`, `skipped_files`, and `estimated_tokens` (`#[non_exhaustive]`). `ingest` fills it from `RunConfig` and `RenderedDigest`.

### Errors

- `DomainError`: Core failures re-exported from crate root (`Usage`, `CandidateLimitExceeded`, `InvalidSource`, `UnsupportedSource`, `InvalidMetadata`, `OutputTooLarge(OutputTooLargeError)`).
- `IngestError`: Shell error re-exported from crate root wrapping `DomainError`, `Io`, `Http`, and `Api(GithubApiError)`.

## 3. Invariants

### 3.1 Input Parsing

- CLI arguments and URLs parse into value types at the boundary (`cli::parse`, `Source::parse`, `GitRef::new`, `GithubName::new`). Core logic assumes validity without re-validation.
- `--ref` is rejected for local sources.
- `-x`/`--exclude` values accumulate in argument order and are compiled once by `ExcludePatterns::new`. An invalid pattern is a usage error.
- GitHub URLs must match `https://github.com/{owner}/{repo}` (optional `.git`). Port must be empty or `443`; user credentials (`user@`) are rejected.

### 3.2 Path Classification

- Order: hidden -> user exclusion -> noise/sensitive entries -> depth check -> non-UTF-8 prefixing -> valid path ([security sec. 2](docs/security_and_filtering.md#2-evaluation-order)). Tar streams classify each path with `classify_path_segments`. The local walk admits each directory once with `admit_dir` (hidden and user-exclusion checks) and classifies each file with `classify_leaf`, which checks only the file itself.
- `Ignored` entries consume no candidate slot and are omitted from skipped counts.
- Run-aborting checks (depth, tar path normalization, oversized tar headers) are specified in [security sec. 2](docs/security_and_filtering.md#2-evaluation-order); the header budget is I-03 ([sec. 3.9](#39-network)).
- Non-UTF-8 candidates record their valid prefix (`"a/b/"` or `""`), charged to the budget under the first segment (`"a/"`) or root (`"./"`).
- **I-02 (`boundary`, `admit_dir`, `classify_leaf`):** No classified path or non-UTF-8 prefix has a first segment starting with `.`, since hidden directories are never admitted, hidden files are `Ignored`, and empty or `.` segments are dropped. The budget therefore keys root-level entries as `"./"`, which no real directory can produce.

### 3.3 Accounting

- Slot allocation and skipped rules are detailed in [security sec. 4](docs/security_and_filtering.md#4-candidate-and-skipped-accounting).
- `Selector::observe` charges a candidate slot prior to inspecting content.
- Read timing: GitHub streams admit candidates before reading bodies. Local workers read bounded bodies concurrently before main thread settlement.

### 3.4 Storage and Ordering

- `FileStorage::push` rejects candidates before allocation if raw arena bytes would exceed `MAX_OUTPUT_SIZE`.
- `SortedFiles::new` sorts spans via `cmp_path` (component-by-component comparison), guaranteeing deterministic output regardless of concurrency timing.
- **I-01 (`runtime`, `SortedFiles::new`):** No stored path is a directory prefix of another stored path. Enforced after sorting by checking adjacent pairs. A schema, type, or boundary check cannot express this because the rule relates separate entries, and a per-entry parse cannot see the others. Behavior: [security sec. 3.4](docs/security_and_filtering.md#34-path-limits).
- **Path uniqueness (`runtime`, `Selector::observe`):** No two candidates share a path. The seen-path set relates separate entries, so no per-entry type can express it. Behavior: [security sec. 3.4](docs/security_and_filtering.md#34-path-limits).
- **I-05 (`boundary`, `extract_dir_slice`):** Every candidate budget key is at most `MAX_PATH_BYTES` bytes and ends on a UTF-8 char boundary. Keys are cut from the first segment at the single point where they are built, so the key store holds at most candidate count times `MAX_PATH_BYTES` bytes. A `type` or `schema` cannot express a length cap on a `&str` key, and `CandidateBudget::add` needs no separate check.
- Arena indices are `u32`. Compile-time assertions in `policy/limits.rs` ensure `MAX_OUTPUT_SIZE + MAX_FILE_SIZE + MAX_PATH_BYTES <= u32::MAX` ([coding convention sec. 3.3](docs/coding_convention.md#33-type-invariants)).

### 3.5 Rendering

- **Escaping:** Single-pass via `write_escaped` per [security sec. 7](docs/security_and_filtering.md#7-output-sanitization).
- **Tree:** Deterministic text tree generated from `SortedFiles` in one pass (`tree.rs`). Relies on `NormalizedPath` having non-empty segments and no trailing `/`: `render_level` panics if a path ends before a tree offset.
- **Header:** Local sources use root directory name (`root` for filesystem root); GitHub sources use canonical `https://github.com/{owner}/{repo}`.
- **Token Estimate:** ASCII bytes divided by `ASCII_BYTES_PER_TOKEN` plus non-ASCII character count (minimum 1 for non-empty digests, 0 for empty).

### 3.6 Output Delivery

- File destinations resolve at startup; directory targets are rejected; canonical destination is excluded from traversal to prevent self-ingest.
- Writes are atomic: temporary file -> sync -> rename -> parent directory sync (Unix). Prior output remains untouched on failure. Details: [security sec. 5](docs/security_and_filtering.md#5-local-filesystem-safety).
- Stdout buffers the complete digest in memory, emitting only on success. Broken pipe on stdout exits with code `0`.
- Completion summary emits to stdout for file targets, stderr for stdout targets. The file target's path in the summary passes through `sanitize_error_detail`.

### 3.7 Diagnostics

- Overflow diagnostics generate only on error paths.
- `MAX_OUTPUT_SIZE` overflow lists the `OVERFLOW_LARGEST_FILES` largest files (sorted by size descending, then path).
- `MAX_FILE_COUNT` overflow lists the `OVERFLOW_LARGEST_DIRECTORIES` largest directories (sorted by candidate count descending, then name).
- Names in both overflow lists are passed through `sanitize_error_detail` by `DomainError`'s `Display`. The stored values stay raw.
- Local runs produce identical diagnostics deterministically.

### 3.8 Local Traversal Pipeline

1. Producer walks depth-first in file-name order without following links (`Walk`, `walk.rs`). A directory matched by a user exclusion is pruned before its `.gitignore` is read. Entering any other directory reads its `.gitignore` once and compiles its rules from those bytes (failure conditions: [security sec. 3.5](docs/security_and_filtering.md#35-ignore-files-local-only)). Rules match through `ignore::gitignore`. Jobs go out for regular files in walk order.
2. Up to `MAX_LOCAL_WORKERS` workers check metadata, read bounded bodies, and reply on bounded slots.
3. Main thread settles slots in walk order into `Selector::observe`. The first error halts traversal.
4. Channel capacities are bounded by `LOCAL_CHANNEL_CAPACITY` ([security sec. 8](docs/security_and_filtering.md#8-limit-values)).
5. Final digest order is established by `SortedFiles::new`, which also enforces I-01. Path uniqueness is established by `Selector::observe`.

### 3.9 Network

- Request parameters, retry classes, and timeouts follow [security sec. 6](docs/security_and_filtering.md#6-github-network-safety).
- Archive entries stream through `Selector::observe` without disk writes.
- Revision SHA extracts from the first `ARCHIVE_PAX_GLOBAL_BYTES` of the PAX global header, falling back to the requested ref.
- Stream/gzip/tar read failures map to `IngestError::Http`.
- **I-03 (`runtime`, `shell/github/archive.rs`):** One `Entries::next` call reads at most `ARCHIVE_MAX_HEADER_BYTES`. tar reads extension bodies internally, so no type or boundary can express the bound. A byte budget on the shared reader enforces it, and each entry body is drained before the next call so only headers are metered. Behavior: [security sec. 2 and 6.4](docs/security_and_filtering.md#64-streaming).
- **I-04 (`boundary`, `DomainError::usage`, `invalid_source`, `unsupported_source`, `invalid_metadata`):** These constructors pass the message through `sanitize_error_detail`, and `DomainError`'s `Display` does the same for these four variants, so a library consumer that builds a variant directly still gets sanitized stderr text. The stored value stays as given to the variant. Behavior: [security sec. 7](docs/security_and_filtering.md#7-output-sanitization).
- **I-06 (`type`, `Auth`, `api_error`):** `api_error` is the only place that builds the token hint and it takes an `Auth`, so a run whose API request carried a token cannot get the hint, including an error from a redirect target. Behavior: [security sec. 6.1](docs/security_and_filtering.md#61-authentication).
- **I-07 (`boundary`, `GithubClient::credentials`):** Only the GitHub API origin receives the token. The `Authorization` header and the `Auth` given to errors both derive from `credentials(url)`, so they cannot disagree. `GithubClient`'s fields are private to `client.rs`, so no other module reaches the token except through `credentials`.
- **I-08 (`boundary`, `TOKEN_ENV_VARS`):** Token environment variable names exist only in `TOKEN_ENV_VARS` (`src/core/policy/mod.rs`). Lookup order and the token hint both read it. A type cannot hold a list of names, so one definition is the enforcement.

## 4. Limits and Tunables

Limits and exclusion lists are defined in `src/core/policy/`; values are mirrored only in [security sec. 8](docs/security_and_filtering.md#8-limit-values). Relations between limits are enforced by compile-time assertions in `policy/limits.rs` and `github/retry.rs` ([coding convention sec. 3.3](docs/coding_convention.md#33-type-invariants)).

## 5. Functional Core and Imperative Shell

Purity rules and the one-way Shell-to-Core dependency: [coding convention sec. 3](docs/coding_convention.md#3-architecture-functional-core-imperative-shell).

### Core (`src/core/`)

- `cli.rs`: CLI argument parsing (`parse`, `Parsed`), `HELP` and `VERSION` constants, `CliAction` (`Help`, `Version`, `Run`), `completion_message`.
- `path.rs`: Path types and classification (`NormalizedPath`, `CandidatePath`, `AdmittedDir`, `classify_path_segments`, `admit_dir`, `classify_leaf`). Depends on `policy/` and `error`.
- `policy/`: `limits.rs` (limits and compile-time assertions), `exclusions.rs` (lockfiles, sensitive patterns), `exclude.rs` (`ExcludePatterns`), `mod.rs` (re-exports, `TOKEN_ENV_VARS`).
- `source.rs`: `Source`, `GitRef`, `GithubName`, source parsing (`Source::parse` delegates URL rules to `github/`).
- `model.rs`: `RunConfig`, `OutputTarget`, `DigestFormat`, `IngestReport`.
- `collection.rs`: `CollectionResult`, `repository_name`.
- `selector.rs`: `Selector`, `CandidateBudget`, `exceeds_size_limits`.
- `storage.rs`: `FileStorage`, `SortedFiles`, `FileSpan`, `FileRecord`, `cmp_path`, `largest_files`.
- `error.rs`: `DomainError`, `CandidateLimitExceededError`, `OutputTooLargeError`, `sanitize_error_detail`. Depends on no other core module, so the policy-to-error edge is one-way.
- `github/`: `url.rs` (URL parsing, origin check), `archive.rs` (PAX parsing, tar path normalization), `retry.rs` (header parsing, delay calculation, attempt state machine), `mod.rs` (`GithubApiError`, `Auth`, error formatting).
- `render/`: `formats.rs` (text/XML formats), `tree.rs`, `escape.rs`, `budget.rs` (`BoundedBuffer`, `RenderedDigest`, token estimate).

### Shell (`src/shell/`)

- `mod.rs`: `ingest` pipeline.
- `cli.rs`: `run_cli` (CLI orchestration, stream selection, exit codes).
- `error.rs`: `IngestError`, `transport_error`.
- `local/`: `mod.rs` (`collect`), `walk.rs`, `gitignore.rs` (`.gitignore` loading and compilation), `read.rs` (metadata/body read), `pipeline.rs` (producer/worker pipeline).
- `github/`: `mod.rs` (`collect`, HTTP retries, streaming), `archive.rs` (tarball/gzip streaming and admission), `client.rs` (HTTP/TLS client, token handling).
- `atomic.rs`: `write_atomic`, `TempPath`.
- `output.rs`: `resolve_output_target`, `deliver`.
- `stdio.rs`: stdout/stderr diagnostic line writers.
- `testing.rs`: Test helpers (`temp_dir`, tarball builder, `MockServer`).

### Public API (`src/lib.rs`)

Exports: `ingest`, `RunConfig`, `ExcludePatterns`, `Source`, `GitRef`, `GithubName`, `OutputTarget`, `DigestFormat`, `IngestReport`, `IngestError`, `DomainError`, `CandidateLimitExceededError`, `OutputTooLargeError`, `GithubApiError`, and the `#[doc(hidden)]` `cli::run_cli` used by `main.rs`. All other items are `pub(crate)`.
