# Domain Context -- codefeed

Defines domain terms, invariants, and functional-core / imperative-shell boundaries. Filtering and accounting rules: [security § 2-4](docs/security_and_filtering.md); escaping: [security § 7](docs/security_and_filtering.md); network safety: [security § 6](docs/security_and_filtering.md).

## 1. Purpose

Converts a local directory or public GitHub repository into a single text digest for LLMs. Collection and rendering must finish before delivery. File replacement is atomic; stdout delivery follows the [README contract](README.md#output).

## 2. Glossary

### Inputs and Configuration

- `Source`: `Local(PathBuf)` or `Github { owner, repo, reference }` (`#[non_exhaustive]`). Parsed via `Source::parse_os` / `Source::parse`.
- `GithubName`: Validated owner/repo name rejecting empty, `.`, `..`, and invalid characters (`Deref<Target = str>`).
- `GitRef`: Validated, trimmed revision rejecting `.` and `..` (`Deref<Target = str>`).
- `OutputTarget`: `File(PathBuf)` or `Stdout`.
- `ResolvedOutput`: Filesystem-resolved target (`shell`-private, from `resolve_output_target`).
- `DigestFormat`: `Text` or `Xml`.
- `ExcludePatterns`: Validated user patterns in `.gitignore` syntax, relative to source root, case-sensitive. Each pattern yields one rule. `\` matches as U+FFFD on all platforms. Validated sets can be combined in rule order with `ExcludePatterns::try_combine`.
- `RunConfig`: Aggregate of `source`, `output`, `format`, and `exclude`.

### Candidate Collection

- `Candidate<B = ()>`: `Valid(NormalizedPath, B)` or `Unstorable(UnstorablePath)` (`()` at classification, `PathBuf` for local walk results, `Body<B>` at observation). Ignored entries are `None` at classification and discarded before selection.
- `UnstorablePath`: Classification-owned value retaining complete normalized raw-byte identity, deriving its bounded accounting key (`BudgetDirKey`) from its first segment.
- `NormalizedPath`: Non-empty, slash-separated UTF-8 relative path without empty or `.` segments, within `MAX_PATH_BYTES` (I-10).
- `Selector`: Linear state managing `FileStorage`, `CandidateBudget`, and `seen_paths` (`HashSet<Vec<u8>>`, unified raw-byte identity set). `observe` admits and settles a local result; `finish` derives the skipped tally as candidate count minus stored file count.
- `PendingFile`: A storable path that passed `Selector::admit_file`. Owns the selector until `settle` records the body outcome.
- `CandidateBudget`: Candidate counter and per-directory tallies (`selector.rs` private).
- `CollectionResult`: `label`, optional `revision`, `files: SortedFiles`, and `skipped` (`model.rs`).

### Storage

- `FileStorage`: Append-only `path_arena` and `content_arena` indexed by `spans` vector. Read via `iter` yielding `FileRecord`.
- `FileRecord<'a>`: Borrowed `path` and `content` view.
- `SortedFiles`: `FileStorage` sorted by `cmp_path` where no stored path is a directory prefix of another (I-01). Uniqueness enforced upstream at selector admission. Implements `Deref<Target = FileStorage>`.

### Rendering and Results

- `BoundedBuffer`: In-memory writer aborting when output exceeds `MAX_OUTPUT_SIZE`.
- `RenderedDigest`: Struct of `bytes` and `estimated_tokens` without delivery target knowledge.
- `IngestReport`: `output` (from `RunConfig`), `included_files`, `skipped_files`, `estimated_tokens` (`#[non_exhaustive]`).

### Errors

- `ErrorDetail`: Sanitized error text constructed exclusively via `ErrorDetail::new` (`Deref<Target = str>`).
- `DomainError`: Core failures (`Usage`, `InvalidSource`, `UnsupportedSource`, `InvalidMetadata` carrying `ErrorDetail`; `CandidateLimitExceeded(LimitExceededError)`; `OutputTooLarge(LimitExceededError)`).
- `IngestError`: Shell error wrapping `DomainError`, `Io`, `Http(ErrorDetail)`, and `Api(GithubApiError)`.

## 3. Invariants

### 3.0 Overarching Laws

All domain invariants derive from five overarching laws (extending [AGENTS.md § 1.3](AGENTS.md) SSOT and [coding convention § 3](docs/coding_convention.md#3-architecture-functional-core-imperative-shell)):

- **L1 (Boundary Construction & Algebraic Closure):** Untrusted inputs, tainted text, paths, and credentials pass once through a single encapsulated constructor or boundary resolver; invalid combinations are unrepresentable via sum types and typestates.
- **L2 (Lossless Raw-Byte Identity):** OS-path CLI boundaries (`<SOURCE>`, `--output=<PATH>`) preserve native `OsStr` encodings; repository path identity, deduplication, and diagnostic tie-break sorting operate on normalized raw bytes—never lossy UTF-8 or truncated accounting prefixes.
- **L3 (Pre-Charge & Bounded Metering):** Resource slots, depths, allocations, and stream budgets are checked and charged *before* reading, expanding, or allocating; worker pools and channels remain bounded.
- **L4 (Concurrent I/O, Deterministic Serial Settlement):** Shell workers/streams perform bounded reads without mutating output; Core admits and settles slots serially (`Selector`), sorts deterministically (`SortedFiles::new`), buffers (`BoundedBuffer`), and delivers atomically only on success.
- **L5 (Runtime Delegation for Cross-Entry & Opaque-Crate Bounds):** Cross-entry relations (uniqueness, prefix-freedom, cumulative budget) and opaque external-crate reads (`tar`) that per-entry types cannot express are enforced by stateful runtime accumulators and shared meters.

### 3.1 Invariant Registry

| ID | Law | Mechanism | Owner | Irreducible Contract |
| :--- | :--- | :--- | :--- | :--- |
| **I-01** | L4, L5 | `runtime` | `SortedFiles::new` | No stored path is a directory prefix of another; enforced on adjacent pairs after `cmp_path` sorting ([security § 3.4](docs/security_and_filtering.md#34-path-limits)). |
| **I-02** | L1 | `boundary` | `admit_dir`, `classify_leaf` | No classified path or unstorable prefix has a first segment starting with `.` (hidden dirs/files dropped, empty/`.` stripped); root entries key as `"./"`. |
| **I-03** | L3, L5 | `runtime` | `shell/github/archive.rs` | One `Entries::next` call reads `<= ARCHIVE_MAX_HEADER_BYTES` via a shared-reader meter; each entry body is drained before the next call so `tar` extension reads are bounded ([security § 2, 6.4](docs/security_and_filtering.md#64-streaming)). |
| **I-04** | L1 | `type` | `ErrorDetail::new` | Sole sanitizing constructor for untrusted error/path diagnostics; `Display` does not re-sanitize; shell effects (`io::Error`, `GithubApiError`, warnings, `Wrote <PATH>`) also sanitize through it ([security § 7](docs/security_and_filtering.md#7-output-sanitization)). |
| **I-05** | L1, L3 | `type` | `BudgetDirKey` | Candidate budget keys are `<= MAX_PATH_BYTES` ending on a UTF-8 char boundary, constructed from the first segment (or `"./"`) at `path.rs` (bounds key map to `candidates × MAX_PATH_BYTES`). |
| **I-06** | L1 | `type` | `Auth`, `api_error` | `api_error` is the sole constructor of token hints and requires `Auth`; requests bearing a token (including redirect targets) never receive hints ([security § 6.1](docs/security_and_filtering.md#61-authentication)). |
| **I-07** | L1 | `boundary` | `RequestTarget`, `RequestPolicy` | Only GitHub API origins receive tokens; `Authorization`, `X-Github-Api-Version`, and error `Auth` derive from a single `RequestPolicy` (private to `client.rs`). |
| **I-08** | SSOT | `boundary` | `TOKEN_ENV_VARS` | Token env var names exist solely in `TOKEN_ENV_VARS` (`src/core/policy/mod.rs`), read by both lookup order and token hints. |
| **I-09** | L2, L5 | `runtime` | `Selector` admission | No two candidates share a path: both storable and unstorable paths share a single exact raw-byte `seen_paths` set distinct from accounting prefixes ([security § 3.4](docs/security_and_filtering.md#34-path-limits)). |
| **I-10** | L1 | `type` | `NormalizedPath`, `classify_leaf` | `Valid` paths are `<= MAX_PATH_BYTES`; `NormalizedPath` is private and `classify_leaf` is its sole non-test constructor returning `Unstorable` on overflow. |
| **I-11** | L1 | `type` | `Candidate<B>` | Unstorable paths carry no body; storable paths carry `B` (`Body<B>` at observation). |
| **I-12** | L3, L5 | `runtime` | `StreamMeter`, `drain_entry` | Archive budget includes bytes synthesized by `tar`'s sparse reader; logical size is checked against remaining budget before draining untouched sparse entries, then only synthesized bytes are added ([security § 6.4](docs/security_and_filtering.md#64-streaming)). |

### 3.2 Non-Reducible Domain & Pipeline Contracts

- **Input & Exclusions:** `--ref` is rejected for local sources; `-x`/`--exclude` patterns accumulate in argument order and compile once via `ExcludePatterns::new` (invalid patterns cause usage errors). GitHub URL grammar: [README Sources](README.md#sources).
- **Classification & Path Mechanics:** Evaluation order: [security § 2](docs/security_and_filtering.md#2-evaluation-order). `Ignored` entries consume no slots and are omitted from skipped counts. Tar streams classify via `normalize_tar_entry_path`; local walks use `admit_dir` and `classify_leaf`. `admit_dir` enforces `MAX_PATH_DEPTH` uniformly on directory admission after exclusions, so `AdmittedDir` guarantees valid depth and `classify_leaf` is total. `DirPrefix::Unstorable` retains raw bytes after the first invalid or overlong (`> MAX_PATH_BYTES`) segment; budget keys derive from the first valid segment (`"a/"`) or `"./"`.
- **Storage, Ordering & Diagnostics:** `FileStorage::push` rejects before allocation if raw arena bytes exceed `MAX_OUTPUT_SIZE`. `SortedFiles::new` sorts component-wise (`cmp_path`). Arena `u32` bounds (`MAX_OUTPUT_SIZE + MAX_FILE_SIZE + MAX_PATH_BYTES <= u32::MAX`) are verified by `const` assertions in `policy/limits.rs`. Overflow diagnostics generate only on error paths ([README Limits](README.md#limits)): `MAX_OUTPUT_SIZE` lists `OVERFLOW_LARGEST_FILES` (size desc, then path); `MAX_FILE_COUNT` lists `OVERFLOW_LARGEST_DIRECTORIES` (count desc, then name), sorted on raw names before `ErrorDetail` sanitization.
- **Rendering & Delivery:** Escaping: [security § 7](docs/security_and_filtering.md#7-output-sanitization). One-pass tree (`tree.rs`) relies on `NormalizedPath` non-empty segments without trailing `/`; local root uses `root`. `resolve_output_target` resolves destination once at startup and excludes it from traversal; `CollectionResult::render` buffers in `BoundedBuffer` and `deliver` emits only on success ([security § 5](docs/security_and_filtering.md#5-local-filesystem-safety), [README Output](README.md#output)).
- **Local Traversal Pipeline:** (1) `Walk` (`walk.rs`) traverses depth-first in file-name order without following symlinks, yielding paths only for storable candidates; excluded dirs are pruned before reading `.gitignore` (read once per dir; failures: [security § 3.5](docs/security_and_filtering.md#35-ignore-files-local-only)). (2) Up to `MAX_LOCAL_WORKERS` workers inspect metadata and read bounded bodies on `LOCAL_CHANNEL_CAPACITY` channels (skipping reads when declared size `> MAX_FILE_SIZE`). (3) Main thread settles slots in walk order via `Selector::observe`, signaling walker/workers and disconnecting the slot receiver on error.
- **Network & Archive Mechanics:** Parameters, retry classes, stream integrity (**S5**), server-wait resolution (**S6**), and timeouts: [security § 6](docs/security_and_filtering.md#6-github-network-safety). Archives admit paths before reading candidate bodies ([security § 2](docs/security_and_filtering.md#2-evaluation-order)), then settle the body through `PendingFile` without disk writes. Revision SHA extracts from the first `ARCHIVE_PAX_GLOBAL_BYTES` of the PAX header, falling back to the requested ref. `shell/github/archive.rs::collect` drains the metered multi-member decoder after tar termination before `Selector::finish`; `shell/github/client.rs::read_rate_limit_headers` forwards raw header bytes to `RateLimitHeaders::parse` in `src/core/github.rs`.

## 4. Limits and Tunables

Authority and mirror rules: [security header](docs/security_and_filtering.md). Limit relations verified by compile-time assertions in `policy/limits.rs` and `github.rs` ([coding convention § 3.3](docs/coding_convention.md#33-type-invariants)).

## 5. Functional Core and Imperative Shell

Purity rules and one-way Shell-to-Core dependency: [coding convention § 3](docs/coding_convention.md#3-architecture-functional-core-imperative-shell).

### Core (`src/core/`)

- `cli.rs`: CLI argument parsing, help, and version text.
- `path.rs`: Path types, classification, and tar entry path normalization.
- `policy/`: Limits, exclusions, user exclude patterns, `.gitignore` compilation, `TOKEN_ENV_VARS`.
- `source.rs`: `Source`, `GithubName`, `GitRef`, and GitHub URL parsing.
- `model.rs`: Configuration and report types.
- `selector.rs`: Candidate settlement and budget accounting.
- `storage.rs`: Arena storage and path ordering.
- `error.rs`: Domain errors and `ErrorDetail`.
- `github.rs`: API/tarball URL helpers, PAX revision parsing, retry policy, API error types.
- `render/`: Text/XML formatting, escaping, tree rendering, output budget, token estimation.

### Shell (`src/shell/`)

- `mod.rs`: `ingest` pipeline.
- `cli.rs`: CLI orchestration, stream selection, exit codes.
- `error.rs`: `IngestError` and I/O error sanitization.
- `local/`: Local walk, `.gitignore` loading, producer/worker reads.
- `github/`: Collection orchestration in `mod.rs`; HTTP configuration, credentials, request execution, retries, and response errors in `client.rs`; metered tarball ingestion in `archive.rs`.
- `output.rs`: Destination resolution, atomic file writes, and delivery.
- `stdio.rs`: stdout/stderr line writers.
- `testing.rs`, `testing/temp_dir.rs`: Test helpers and shared `TempDirGuard`.

### Public API (`src/lib.rs`)

Exports: `ingest`, `RunConfig`, `ExcludePatterns`, `Source`, `GitRef`, `GithubName`, `OutputTarget`, `DigestFormat`, `IngestReport`, `IngestError`, `DomainError`, `ErrorDetail`, `LimitExceededError`, `GithubApiError`, and hidden `cli::run_cli`. All other items are `pub(crate)`.
