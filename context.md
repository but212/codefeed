# Domain Context -- codefeed

This file defines domain terms, invariants that must always hold, and the functional-core / imperative-shell boundary. Rules that already have a home are linked, not restated: filtering and accounting in [security sec. 2-4](docs/security_and_filtering.md), escaping in [security sec. 7](docs/security_and_filtering.md), and network safety in [security sec. 6](docs/security_and_filtering.md).

## 1. Purpose

codefeed turns a local directory or a public GitHub repository into one text digest for an LLM. Every run either produces a complete digest or fails. It never emits a partial digest.

## 2. Glossary

### Inputs and configuration

- `Source` -- `Local(PathBuf)` or `Github { owner, repo, reference }`. Built publicly through `Source::github` or parsed from CLI arguments via `Source::parse_os` / `Source::parse`, validating owner and repository names into `GithubName`. The `Github` variant is `#[non_exhaustive]`.
- `GithubName` -- Validated GitHub owner or repository name. Rejects empty strings, `.`, `..`, and invalid characters. Derefs to `str`.
- `GitRef` -- Validated, trimmed, non-empty revision (branch, tag, or commit SHA). Rejects `.` and `..`. Derefs to `str`.
- `OutputTarget` -- `File(PathBuf)` or `Stdout`. Stdout is a variant, never a sentinel path.
- `ResolvedOutput` -- `OutputTarget` resolved against the filesystem. Shell-private. Only `resolve_output_target` constructs it.
- `DigestFormat` -- `Text` or `Xml`.
- `RunConfig` -- `source`, `output`, and `format` together.

### Candidate collection

- `PathStatus` -- Result of path classification: `Valid(NormalizedPath)`, `Ignored`, or `NonUtf8Candidate(prefix)`.
- `NormalizedPath` -- Non-empty, slash-separated, UTF-8 relative path. Empty and `.` segments are removed before it is built.
- `Selector` -- Linear state holding storage, candidate budget, and skipped count. `observe` settles one candidate. `finish` produces a `CollectionResult`.
- `CandidateBudget` -- Candidate counter plus per-directory counts. Private to `selector.rs`.
- `CollectionResult` -- `label`, optional `revision`, `files: SortedFiles`, and `skipped`.

### Storage

- `FileStorage` -- Append-only private `path_arena` and `content_arena` plus a `spans` vector. Callers read records only through `FileStorage::iter`, which yields `FileRecord` views.
- `FileSpan` -- 16-byte POD of four `u32` values (offsets and lengths) into the arenas. Private to `storage.rs`, so no span leaves storage and none can be passed to another storage.
- `FileRecord<'a>` -- Borrowed `path` and `content` views.
- `SortedFiles` -- `FileStorage` sorted by `cmp_path`. Read-only through `Deref`.

### Rendering and results

- `BoundedBuffer` -- In-memory output that fails once it would exceed `MAX_OUTPUT_SIZE`.
- `RenderedDigest` -- `bytes`, `estimated_tokens`, `included_files`, and `skipped_files`.
- `IngestReport` -- `output`, `included_files`, `skipped_files`, and `estimated_tokens`. `#[non_exhaustive]`.

### Errors

- `DomainError` -- Core rule failures, re-exported from the crate root: `Usage`, `CandidateLimitExceeded`, `InvalidSource`, `UnsupportedSource`, `InvalidMetadata`, `OutputTooLarge`.
- `IngestError` -- Shell error that wraps `DomainError` and adds `Io`, `Http`, and `Api`. Re-exported from the crate root.

## 3. Invariants

### 3.1 Inputs are parsed once

- CLI arguments and URLs become value objects at the boundary (`cli::parse`, `Source::parse`, `GitRef::new`, `GithubName::new`). Core code after that point assumes validity and does not re-validate.
- `--ref` is rejected for local sources.
- A GitHub URL must be exactly `https://github.com/{owner}/{repo}`, with an optional trailing `.git`. Other schemes and hosts are rejected. Userinfo (`user@`) is rejected, and the port must be empty or `443`.

### 3.2 Path classification

- Classification order and rules are in [security sec. 2](docs/security_and_filtering.md#2-evaluation-order). Local and archive entries share one function, `classify_path_segments`.
- An `Ignored` entry consumes no candidate slot and is never counted as skipped.
- A depth failure is the only path rule that fails the whole run. Hidden and noise entries are classified first, so they never fail the run.
- A non-UTF-8 entry carries a prefix: the valid segments before its first invalid segment, each followed by `/` (`a/b/`), or `""` when the first segment is invalid. The budget charges the entry to the first segment of the prefix (`a/`), or to `<root>/` for an empty prefix.

### 3.3 Accounting

- Which outcomes consume a slot and which count as skipped is in [security sec. 4](docs/security_and_filtering.md#4-candidate-and-skipped-accounting).
- `Selector::observe` charges a slot before it looks at content. The body is read only after admission.
- The timing of the read differs by source. GitHub entries are admitted before their body is read. Local workers read a bounded body before the main thread calls `observe`, so a local read can happen before the budget check.

### 3.4 Storage and ordering

- `FileStorage::push` rejects a candidate before storing it when the raw arena bytes would exceed `MAX_OUTPUT_SIZE`. Rendered output is never smaller than the raw arena bytes, so this early check never rejects a digest that would fit.
- `SortedFiles::new` sorts spans by `cmp_path`, which compares path components. The result is byte-identical regardless of worker timing.
- Arena offsets and lengths are `u32`. A compile-time assertion in `policy/limits.rs` keeps `MAX_OUTPUT_SIZE + MAX_FILE_SIZE + MAX_PATH_BYTES` within `u32::MAX`.

### 3.5 Rendering

- **Escaping** is defined in [security sec. 7](docs/security_and_filtering.md#7-output-sanitization). The renderers are `formats.rs` (text and XML), `tree.rs`, and `escape.rs`.
- **Tree**: A text tree, with branch prefixes defined in `tree.rs`, is built from the sorted storage in one pass. It is format-neutral.
- **Header**: Local digests use the root directory name (`root` for a filesystem root). GitHub digests use the canonical URL `https://github.com/{owner}/{repo}` (`repository_url`).
- **Token estimate** (`estimate_tokens_from_bytes`): ASCII bytes divided by `ASCII_BYTES_PER_TOKEN`, plus one for each non-ASCII character (counted by UTF-8 lead bytes), with a minimum of 1 for non-empty output and 0 for empty output. This is a heuristic, not a tokenizer count.

### 3.6 Output delivery

- File targets are resolved once at startup. The parent directory must exist, and a directory target is rejected. The canonical output path is excluded from the walk, so the digest never ingests itself.
- File writes are atomic: temp file, sync, rename, then sync the parent directory on Unix. A failed run leaves the previous file in place. A failed parent-directory sync after the rename is a warning, not a failure, because the new file is already in place.
- Stdout targets buffer the whole digest and write it only on success.
- A broken pipe on stdout is not an error.
- The completion summary goes to stdout for file targets and to stderr for stdout targets. It is built by `completion_message` in `core/cli.rs`. `run_cli` in `shell/cli.rs` chooses the stream.

### 3.7 Diagnostics

- Overflow diagnostics are built only on error paths.
- `MAX_OUTPUT_SIZE` overflow lists the five largest files, sorted by size and then by path.
- `MAX_FILE_COUNT` overflow lists the three largest directories, sorted by count and then by name.
- Local runs report the same diagnostics on every run. The walk is sorted and candidates are settled in walk order, so the candidates admitted before an abort are the same each time.

### 3.8 Local traversal pipeline

1. A producer thread walks with `WalkBuilder::sort_by_file_name` and `follow_links(false)`. For each entry it classifies the path and sends a job for each regular file. It sends one slot per entry, in walk order.
2. Up to `MAX_LOCAL_WORKERS` workers take jobs. Each worker checks metadata, reads a bounded body, and replies on its slot. After an abort, workers reply without reading.
3. The main thread settles slots in walk order and calls `Selector::observe` for each candidate. The first error in walk order stops the run.
4. The job and slot channels each hold `LOCAL_CHANNEL_CAPACITY` messages ([security sec. 8](docs/security_and_filtering.md#8-limit-values)).
5. The final order comes from `SortedFiles`. Both the walk order and the settle order are `cmp_path` order.

### 3.9 Network

- Request shape, retry classes, and time limits are in [security sec. 6](docs/security_and_filtering.md#6-github-network-safety).
- Entries are streamed through `Selector::observe` and never written to disk.
- The revision is the commit SHA from the PAX global header. If the header is missing, the requested ref is used.
- Stream, gzip, and tar read errors during the body read map to `IngestError::Http`, not `Io`.

## 4. Limits and tunables

All limits and exclusion lists are defined in `src/core/policy/`. Code reads them from there and never copies their values. Limit values are stated only in [security sec. 8](docs/security_and_filtering.md#8-limit-values).

Compile-time assertions in `policy/limits.rs` and `github/retry.rs` check relations between limits.

## 5. Functional core and imperative shell

Core code is pure. It performs no I/O, reads no clock or environment, and never depends on the shell. The shell depends on the core in one direction only. The coding rules that enforce this are in [`docs/coding_convention.md`](docs/coding_convention.md) sec. 3.

### Core (`src/core/`)

- `cli.rs` -- Argument parsing (`parse`, folded through `Parsed`), help text, `CliAction`, and `completion_message`.
- `policy/` -- `mod.rs` (re-exports), `limits.rs` (numeric limits and compile-time assertions), `exclusions.rs` (noise and sensitive lists), and `classify.rs` (`classify_path_segments`, `is_hidden_segment`, `is_ignored_filename`, `NormalizedPath`, `PathStatus`).
- `model.rs` -- `Source`, `GitRef`, `GithubName`, `RunConfig`, `OutputTarget`, `DigestFormat`, `CollectionResult`, `IngestReport`, and `repository_name`.
- `selector.rs` -- `Selector::observe` and `finish`, `CandidateBudget`, and `exceeds_size_limits`.
- `storage.rs` -- `FileStorage`, `SortedFiles`, `FileSpan`, `FileRecord`, `cmp_path`, and `largest_files`.
- `error.rs` -- `DomainError` and `CandidateLimitExceededError`.
- `archive.rs` -- PAX comment parsing and tar entry path normalization (`normalize_tar_entry_path`).
- `github/url.rs` -- Name validation, URL parsing, tarball URL construction, `is_github_api_origin`, and `repository_url`.
- `github/retry.rs` -- Rate-limit header parsing (`RateLimitHeaders`), retry delay (`retry_delay`), and the attempt state machine (`AttemptState::step`).
- `github/mod.rs` -- API and retry message text, and `sanitize_error_detail`.
- `render/` -- `mod.rs` (format dispatch), `formats.rs` (text and XML), `tree.rs`, `escape.rs` (`write_escaped`), and `budget.rs` (`BoundedBuffer`, `RenderedDigest`, token estimate).

### Shell (`src/shell/`)

- `main.rs` (crate root binary) -- Calls `run_cli` and exits with its code.
- `mod.rs` -- `ingest` (pipeline).
- `cli.rs` -- `run_cli` (argument handling, exit codes, and stream choice).
- `error.rs` -- `IngestError` and `transport_error`, the only place effect errors are built.
- `local/` -- `mod.rs` (`collect`, root validation, label, and candidate observation), `walk.rs` (sorted walker and path classification), `read.rs` (one file's metadata, size gate, and bounded body), and `pipeline.rs` (producer, read workers, and in-order settlement to the selector).
- `archive.rs` -- Gzip and tar streaming, PAX SHA extraction, and entry admission (`collect`).
- `github/` -- `mod.rs` (orchestration and `collect`), `client.rs` (HTTP client, TLS, and token resolution), and `stream.rs` (tarball streaming, warnings, and retry loop).
- `atomic.rs` -- `write_atomic`, `AtomicFile`, and `TempFile`: temporary file, sync, and rename.
- `output.rs` -- `resolve_output_target`, `deliver`, and the stdout/stderr line writers.
- `testing.rs` -- Test-only fixtures: `temp_dir`, tarball builders, and `MockServer`.

### Public API (`src/lib.rs`)

`ingest`, `RunConfig`, `Source`, `GitRef`, `GithubName`, `OutputTarget`, `DigestFormat`, `IngestReport`, `IngestError`, `DomainError`, and `CandidateLimitExceededError`. `run_cli` is internal under `codefeed::cli`. Everything else is `pub(crate)`.
