# Domain Context -- codefeed

Defines domain terms, core invariants, and the functional-core / imperative-shell boundary. Filtering and accounting rules live in [security sec. 2-4](docs/security_and_filtering.md), escaping in [security sec. 7](docs/security_and_filtering.md), and network safety in [security sec. 6](docs/security_and_filtering.md).

## 1. Purpose

`codefeed` converts a local directory or public GitHub repository into a single text digest for LLMs. Every run either produces a complete digest or fails atomically; partial digests are never emitted.

## 2. Glossary

### Inputs and configuration

- `Source`: `Local(PathBuf)` or `Github { owner, repo, reference }` (`#[non_exhaustive]`). Constructed via `Source::github` or parsed at the boundary via `Source::parse_os` / `Source::parse`.
- `GithubName`: Validated GitHub owner/repo name. Rejects empty strings, `.`, `..`, and invalid characters. Implements `Deref<Target = str>`.
- `GitRef`: Validated, trimmed, non-empty revision (branch, tag, commit SHA). Rejects `.` and `..`. Implements `Deref<Target = str>`.
- `OutputTarget`: `File(PathBuf)` or `Stdout`.
- `ResolvedOutput`: Filesystem-resolved target (`shell`-private, constructed solely by `resolve_output_target`).
- `DigestFormat`: `Text` or `Xml`.
- `RunConfig`: Tuple of `source`, `output`, and `format`.

### Candidate collection

- `PathStatus`: Result of path classification: `Valid(NormalizedPath)`, `Ignored`, or `NonUtf8Candidate(prefix)`.
- `NormalizedPath`: Non-empty, slash-separated, UTF-8 relative path without empty or `.` segments.
- `Selector`: Linear state managing arena storage, `CandidateBudget`, and skipped tally. Settles candidates via `observe` and completes via `finish`.
- `CandidateBudget`: Candidate counter and per-directory tallies (`selector.rs` private).
- `CollectionResult`: Struct containing `label`, optional `revision`, `files: SortedFiles`, and `skipped`.

### Storage

- `FileStorage`: Append-only `path_arena` and `content_arena` indexed by a `spans` vector. Read via `FileStorage::iter` yielding `FileRecord`.
- `FileSpan`: 16-byte POD of four `u32` arena offsets and lengths (`storage.rs` private).
- `FileRecord<'a>`: Borrowed `path` and `content` view.
- `SortedFiles`: `FileStorage` sorted by `cmp_path`. Path uniqueness is enforced upstream by `Selector::observe`. Implements `Deref<Target = FileStorage>`.

### Rendering and results

- `BoundedBuffer`: In-memory writer aborting when output would exceed `MAX_OUTPUT_SIZE`.
- `RenderedDigest`: Struct of `bytes`, `estimated_tokens`, `included_files`, and `skipped_files`.
- `IngestReport`: Struct of `output`, `included_files`, `skipped_files`, and `estimated_tokens` (`#[non_exhaustive]`).

### Errors

- `DomainError`: Core failures re-exported from crate root (`Usage`, `CandidateLimitExceeded`, `InvalidSource`, `UnsupportedSource`, `InvalidMetadata`, `OutputTooLarge`).
- `IngestError`: Shell error re-exported from crate root wrapping `DomainError`, `Io`, `Http`, and `Api`.

## 3. Invariants

### 3.1 Input Parsing

- CLI arguments and URLs parse into value types at the boundary (`cli::parse`, `Source::parse`, `GitRef::new`, `GithubName::new`). Core logic assumes validity without re-validation.
- `--ref` is rejected for local sources.
- GitHub URLs must match `https://github.com/{owner}/{repo}` (optional `.git`). Port must be empty or `443`; user credentials (`user@`) are rejected.

### 3.2 Path Classification

- Order: hidden/noise/sensitive entries -> depth check -> non-UTF-8 prefixing -> valid path ([security sec. 2](docs/security_and_filtering.md#2-evaluation-order)). Shared by local walk and tar stream via `classify_path_segments`.
- `Ignored` entries consume no candidate slot and are omitted from skipped counts.
- Depth limit violation is the only path check aborting the entire run during classification. Hidden and noise entries are dropped before depth evaluation. For GitHub tar archives, `normalize_tar_entry_path` also aborts the run with `DomainError::InvalidMetadata` upon encountering embedded NUL bytes, traversal segments (`..`), or a `.` root segment, for regular-file entries. Non-regular tar entries are dropped before normalization runs.
- Non-UTF-8 candidates record their valid prefix (`"a/b/"` or `""`), charged to the budget under the first segment (`"a/"`) or root (`"<root>/"`).

### 3.3 Accounting

- Slot allocation and skipped rules are detailed in [security sec. 4](docs/security_and_filtering.md#4-candidate-and-skipped-accounting).
- `Selector::observe` charges a candidate slot prior to inspecting content.
- Read timing: GitHub streams admit candidates before reading bodies. Local workers read bounded bodies concurrently before main thread settlement.

### 3.4 Storage and Ordering

- `FileStorage::push` rejects candidates before allocation if raw arena bytes would exceed `MAX_OUTPUT_SIZE`.
- `SortedFiles::new` sorts spans via `cmp_path` (component-by-component comparison), guaranteeing deterministic output regardless of concurrency timing.
- `Selector::observe` rejects a candidate whose path (at most `MAX_PATH_BYTES`) equals an earlier candidate's path, with `DomainError::InvalidMetadata`. The check covers skipped candidates, so no path names two candidates. Longer paths are skipped before the check.
- Arena indices are `u32`. Compile-time assertions in `policy/limits.rs` ensure `MAX_OUTPUT_SIZE + MAX_FILE_SIZE + MAX_PATH_BYTES <= u32::MAX`.

### 3.5 Rendering

- **Escaping:** Single-pass via `write_escaped` per [security sec. 7](docs/security_and_filtering.md#7-output-sanitization).
- **Tree:** Deterministic text tree generated from `SortedFiles` in one pass (`tree.rs`).
- **Header:** Local sources use root directory name (`root` for filesystem root); GitHub sources use canonical `https://github.com/{owner}/{repo}`.
- **Token Estimate:** ASCII bytes divided by `ASCII_BYTES_PER_TOKEN` plus non-ASCII character count (minimum 1 for non-empty digests, 0 for empty).

### 3.6 Output Delivery

- File destinations resolve at startup; directory targets are rejected; canonical destination is excluded from traversal to prevent self-ingest.
- Writes are atomic: temporary file -> sync -> rename -> parent directory sync (Unix). Prior output remains untouched on failure. Parent sync failure logs a warning without failing.
- Stdout buffers the complete digest in memory, emitting only on success. Broken pipe on stdout exits with code `0`.
- Completion summary emits to stdout for file targets, stderr for stdout targets.

### 3.7 Diagnostics

- Overflow diagnostics generate only on error paths.
- `MAX_OUTPUT_SIZE` overflow lists the 5 largest files (sorted by size descending, then path).
- `MAX_FILE_COUNT` overflow lists the 3 largest directories (sorted by candidate count descending, then name).
- Local runs produce identical diagnostics deterministically.

### 3.8 Local Traversal Pipeline

1. Producer walks depth-first in file-name order without following links (`Walk`, `walk.rs`). Entering a directory reads its `.gitignore` once and compiles its rules from those bytes (unreadable or non-UTF-8 fails the run, [security sec. 3.5](docs/security_and_filtering.md#35-ignore-files-local-only)). Rules match through `ignore::gitignore`. Jobs go out for regular files in walk order.
2. Up to `MAX_LOCAL_WORKERS` workers check metadata, read bounded bodies, and reply on bounded slots.
3. Main thread settles slots in walk order into `Selector::observe`. The first error halts traversal.
4. Channel capacities are bounded by `LOCAL_CHANNEL_CAPACITY` ([security sec. 8](docs/security_and_filtering.md#8-limit-values)).
5. Final digest order is established by `SortedFiles::new`. Path uniqueness is established by `Selector::observe`.

### 3.9 Network

- Request parameters, retry classes, and timeouts follow [security sec. 6](docs/security_and_filtering.md#6-github-network-safety).
- Archive entries stream through `Selector::observe` without disk writes.
- Revision SHA extracts from PAX global header, falling back to the requested ref.
- Stream/gzip/tar read failures map to `IngestError::Http`.

## 4. Limits and Tunables

All limits and exclusion lists are defined in `src/core/policy/`. Code reads constants directly. Limit values are stated exclusively in [security sec. 8](docs/security_and_filtering.md#8-limit-values). Invariant relations are enforced by compile-time assertions in `policy/limits.rs` and `github/retry.rs`.

## 5. Functional Core and Imperative Shell

Core is referentially pure (no I/O, clock, env, or shell imports). Shell depends on Core in one direction only ([coding convention sec. 3](docs/coding_convention.md#3-architecture-functional-core-imperative-shell)).

### Core (`src/core/`)

- `cli.rs`: CLI argument parsing (`parse`, `Parsed`), `HELP` constant, `CliAction`, `completion_message`.
- `policy/`: `limits.rs` (limits and compile-time assertions), `exclusions.rs` (lockfiles, sensitive patterns), `classify.rs` (`classify_path_segments`, `NormalizedPath`, `PathStatus`).
- `model.rs`: `Source`, `GitRef`, `GithubName`, `RunConfig`, `OutputTarget`, `DigestFormat`, `CollectionResult`, `IngestReport`.
- `selector.rs`: `Selector`, `CandidateBudget`, `exceeds_size_limits`.
- `storage.rs`: `FileStorage`, `SortedFiles`, `FileSpan`, `FileRecord`, `cmp_path`, `largest_files`.
- `error.rs`: `DomainError`, `CandidateLimitExceededError`, `sanitize_error_detail`.
- `archive.rs`: PAX parsing, tar path normalization (`normalize_tar_entry_path`).
- `github/`: `url.rs` (URL parsing, origin check), `retry.rs` (header parsing, delay calculation, attempt state machine), `mod.rs` (error formatting).
- `render/`: `formats.rs` (text/XML formats), `tree.rs`, `escape.rs`, `budget.rs` (`BoundedBuffer`, `RenderedDigest`, token estimate).

### Shell (`src/shell/`)

- `main.rs`: Entry point invoking `run_cli`.
- `mod.rs`: `ingest` pipeline.
- `cli.rs`: `run_cli` (CLI orchestration, stream selection, exit codes).
- `error.rs`: `IngestError`, `transport_error`.
- `local/`: `mod.rs` (`collect`), `walk.rs`, `read.rs` (metadata/body read), `pipeline.rs` (producer/worker pipeline).
- `archive.rs`: Tarball/gzip streaming and admission.
- `github/`: `mod.rs` (`collect`), `client.rs` (HTTP/TLS client, token handling), `stream.rs` (archive streaming, retries).
- `atomic.rs`: `write_atomic`, `AtomicFile`, `TempFile`.
- `output.rs`: `resolve_output_target`, `deliver`, stdout/stderr line writers.
- `testing.rs`: Test helpers (`temp_dir`, tarball builder, `MockServer`).

### Public API (`src/lib.rs`)

Exports: `ingest`, `RunConfig`, `Source`, `GitRef`, `GithubName`, `OutputTarget`, `DigestFormat`, `IngestReport`, `IngestError`, `DomainError`, and `CandidateLimitExceededError`. All other items are `pub(crate)`.
