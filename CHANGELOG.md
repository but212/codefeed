# Changelog

All notable changes are listed here.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.0.0/). Versioning: [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

### Library consumers and CLI users

Observable behavior. **Action** = code or config change required.

#### Added

- **`--exclude <PATTERN>` (`-x`)**: Drops paths matching a `.gitignore`-style pattern, relative to the source root. Repeatable; applied in order. Local and GitHub sources. Excluded entries are omitted from the digest and skipped count at any path length; excluded local directories are not entered. Matching examines only the first `MAX_PATH_DEPTH + 1` segments. Deeper entries are not excluded by a match past that point and still fail the depth limit. An invalid pattern, or one matching nothing, is a usage error (`src/core/policy/exclude.rs`, `src/core/cli.rs`, `src/shell/local/walk.rs`, `src/core/policy/classify.rs`). Help text and `docs/security_and_filtering.md` sec. 3.6 document the rules.

#### Breaking

- **`RunConfig` has an `exclude` field**: Adds `exclude: ExcludePatterns`. A literal `RunConfig { source, output, format }` no longer compiles. `RunConfig::new` keeps three arguments, sets no exclusions, and is no longer `const`. `ExcludePatterns` is re-exported at crate root (`src/core/model.rs`, `src/lib.rs`).
  - **Action:** Build with `RunConfig::new`, or add `exclude: ExcludePatterns::default()` to each literal.
- **`run_cli` moved to `codefeed::cli`**: Crate root no longer exports `codefeed::run_cli`. The entry point is `codefeed::cli::run_cli` (`#[doc(hidden)]`), used by `src/main.rs`; consumers use `codefeed::ingest` (`src/lib.rs`, `src/shell/cli.rs`).
  - **Action:** Call `codefeed::cli::run_cli` or migrate to `codefeed::ingest`.
- **`Source::Github` owner and repo typed `GithubName`**: `owner` and `repo` in `Source::Github { owner, repo, reference }` are `GithubName`, not `String`. `GithubName` derefs to `str`, rejects invalid characters, `.`, and `..`, and is re-exported at crate root. `Source::github` takes `impl Into<String>` and validates via `GithubName::new` (`src/core/model.rs`, `src/lib.rs`).
  - **Action:** Update matches or bindings that expect owned `String` rather than `&str`.

#### Changed

- **Deterministic local errors**: Local diagnostics are stable across runs. `MAX_FILE_COUNT` and `MAX_OUTPUT_SIZE` errors list files and directories in deterministic order. With several fatal I/O errors, the first in walk order is reported, not the first to arrive (`src/shell/local/pipeline.rs`).
- **Duplicate archive paths fail the run**: A GitHub archive whose entries normalize to one path (e.g. `r1/a.txt` and `r2/a.txt`, or `repo/a//b.txt` and `repo/a/b.txt`) fails with `InvalidMetadata` instead of emitting duplicate `FILE:` entries. The check includes skipped candidates, so a duplicate of an oversized or non-text entry also fails. Paths over `MAX_PATH_BYTES` are skipped before the check (`src/core/selector.rs`, `src/shell/archive.rs`).
- **Local walk owns `.gitignore` loading**: The walker matches with `ignore::gitignore` and walks directories itself, not via the `ignore` crate's directory walker. Matching is unchanged: negation, anchored and directory-only patterns, pruning, BOM handling, skipped invalid globs. A differential test compares the walk with `ignore::WalkBuilder` on one fixture (`src/shell/local/walk.rs`).
- **Symlinked or oversized local `.gitignore` fails the run**: A symlinked `.gitignore` fails with an `Io` error naming its path. The link is not followed; before, its target's rules applied. A `.gitignore` over `MAX_FILE_SIZE` also fails; before, it was read without a limit. Both follow the unreadable-file rule (`src/shell/local/walk.rs`).

#### Fixed

- **GitHub connect and TLS handshake timeouts**: DNS, connect, and TLS handshake had no time limit, so a stalled connect could block a run indefinitely. Each phase is bounded by `GITHUB_TIMEOUT_SECS`, and a timeout retries per the retry policy (`src/shell/github/client.rs`).
- **Long output file names**: The temp file name embedded the output name, so long names failed with a Windows I/O error. Temp files are now `.codefeed.{nanos}.{counter}.tmp` regardless of target name (`src/shell/atomic.rs`).
- **Local `.gitignore` rules come from the validating read**: The `ignore` crate read each `.gitignore` before a separate validation read, so a file changed between reads could silently drop its rules. Each file is now read once and compiled from those bytes. An unreadable or non-UTF-8 file fails with an `Io` error naming its path (`src/shell/local/walk.rs`).
- **Tar entry evaluation order documentation**: `docs/security_and_filtering.md` and `context.md` placed the non-regular entry check after tar path normalization. The code drops non-regular entries first; the docs now match. No runtime change.
- **Local filesystem error sanitization**: Control characters (`\t`, `\r`, `\n`, Unicode control codes) in paths in I/O errors (`io_error_at`, `resolve_output_target`, `validate_root`, `sync_warning`) are sanitized via `sanitize_error_detail` against terminal injection and log forging (`src/shell/error.rs`, `src/shell/local/walk.rs`, `src/shell/local/mod.rs`, `src/shell/output.rs`, `src/shell/atomic.rs`).
- **Tar root segment error wording**: `"Tar entry root segment is '.'; expected a single repository root directory"` → `"Tar entry root segment cannot be '.'"` (`src/core/archive.rs`).
- **Local pipeline early-abort deadlock**: Producer (`produce_slots`) and consumer (`settle_slots`) keep channel endpoint ownership. An early error drops the receiver, disconnects the channel, and ends the producer loop without deadlock when the slot queue overflows (`src/shell/local/pipeline.rs`).
- **Atomic permission error context**: Unix permission errors during atomic output creation include the temp file path (`src/shell/atomic.rs`).
- **PAX archive size overrides**: Tar `process_tar_entry` uses `entry.size()` instead of `entry.header().size()`, so PAX `size=` overrides are checked against `MAX_FILE_SIZE` before reading (`src/shell/archive.rs`).

### Contributors

Internal refactors and test changes. No consumer action required.

#### Core

- **Source attribute doc**: `context.md` said `Source` is `#[non_exhaustive]`; the attribute is on the `Github` variant only (`context.md`).
- **Error sanitization move**: `sanitize_error_detail` moved from `src/core/github/mod.rs` to `src/core/error.rs` (`src/core/error.rs`, `src/core/github/mod.rs`).
- **CLI help constant**: `help_text()` inlined as `pub(crate) const HELP: &str`, removing single-caller indirection (`src/core/cli.rs`, `src/shell/cli.rs`).
- **Hollow tests and test-only derives**: Removed `stdout_overflow_writes_nothing_to_stream` and `attempt_warn_handles_transport_error`; they could not fail on the behavior they named. Removed unused `SortedFiles` `PartialEq`/`Eq` and `RenderedDigest` `Clone`/`PartialEq`/`Eq`. Test-only derives on `CollectionResult`, `SortedFiles`, `FileStorage`, `BoundedBuffer`, and `CandidateBudget` use `cfg_attr(test, ...)` (`src/core/`, `src/shell/output.rs`, `src/shell/github/stream.rs`).
- **CI coverage**: Test job runs `cargo test --doc --locked`. New lint job runs `cargo fmt --check` and `cargo doc --no-deps --locked` with `RUSTDOCFLAGS=-D warnings` (`.github/workflows/ci.yml`).
- **Dead derives and impls**: Removed unused `Default` on `RateLimitHeaders`, `AttemptState`, `SortedFiles`, `FileStorage`. Removed test-only `Display` and `std::error::Error` on `OutputLimitExceeded`; render tests unwrap directly (`src/core/github/retry.rs`, `src/core/storage.rs`, `src/core/render/budget.rs`, `src/core/render/formats.rs`, `src/core/render/tree.rs`).

- **Pedantic lints, token heuristic**: `#[must_use]` on pure domain, parsing, and rendering queries for `clippy::pedantic`. `ASCII_BYTES_PER_TOKEN` is typed `usize`; the `_USIZE` companion is removed; test-only companions sit under `#[cfg(test)]` (`src/core/`, `src/core/policy/limits.rs`, `src/core/render/budget.rs`).
- **Policy split**: `src/core/policy.rs` → `src/core/policy/` (`limits.rs`, `exclusions.rs`, `classify.rs`), with co-located tests. Behavior, constants, and re-exports unchanged (`src/core/policy/`).
- **Path ordering**: Removed conflicting `PartialOrd`, `Ord`, `Hash` on `NormalizedPath`; `cmp_path` is the sole ordering (`src/core/policy/classify.rs`).
- **Test fixtures**: XML and text fixtures share generic `render_digest`; CLI valid-invocation assertions use `parse_run` (`src/core/render/formats.rs`, `src/core/cli.rs`).
- **Test diet**: Pruned redundant subprocess CLI tests (`tests/cli.rs`); `FileStorage` tests decoupled from arena internals (`src/core/storage.rs`); test-only helper panic tests removed (`src/core/policy/classify.rs`).
- **Private `FileSpan`**: Private to `src/core/storage.rs`. Callers and the tree renderer read only through `FileStorage::iter`, preventing cross-storage span use (`src/core/storage.rs`, `src/core/render/tree.rs`).
- **Argument classification**: `cli::parse` classifies each argument (`classify_arg`: separator, help, flag, unknown option, positional) before folding values. Behavior unchanged (`src/core/cli.rs`).
- **XML body escape gate**: `escape_xml_body` checks a precomputed first-byte table before the match; most bytes reject with one lookup. Output unchanged. Scratch benchmark on Rust source: several-fold faster. End-to-end impact not measured (`src/core/render/escape.rs`).

#### Shell

- **Atomic write guard**: `TempState`, `AtomicFile`, `TempFile` replaced by `TempPath` (unlinks on drop unless persisted) and `write_atomic`. Drop order (handle before unlink) follows declaration order. Removed unreachable `Closed` and `Committed` branches (`src/shell/atomic.rs`).
- **Shell error references**: Shell error helpers (incl. `io_error_at`) borrow `&io::Error`, resolving `clippy::needless_pass_by_value` without extra allocation (`src/shell/error.rs`, `src/shell/atomic.rs`, `src/shell/archive.rs`, `src/shell/output.rs`).
- **CLI entry split**: `run_cli`, `report_error`, and CLI tests moved from `src/shell/mod.rs` to `src/shell/cli.rs`; `mod.rs` only orchestrates `ingest` (`src/shell/cli.rs`, `src/shell/mod.rs`).
- **GitHub module split**: `src/shell/github.rs` → `src/shell/github/` (`client`, `stream`). `MockServer` moved to `src/shell/testing.rs`. Tests and behavior unchanged (`src/shell/github/`, `src/shell/testing.rs`).
- **Local pipeline**: Producer sorts the walk (`ignore::WalkBuilder::sort_by_file_name`); workers read bodies concurrently; main thread settles in walk order. In-flight bodies bounded by queue capacity (`LOCAL_CHANNEL_CAPACITY`). Hyperfine release on Windows: large synthetic tree and this repository both within about 2% of before (`src/shell/local/pipeline.rs`, `src/core/policy/limits.rs`).
- **Local module split**: `src/shell/local.rs` → `src/shell/local/` (`walk`, `read`, `pipeline`). Tests and behavior unchanged. `run_cli` reports errors via one helper (`src/shell/mod.rs`).

### Maintainers

Documentation, build, and removals.

#### Build

- **Release profile**: `lto = "thin"` replaces whole-program LTO; `codegen-units = 1` and `panic = "unwind"` kept (`Cargo.toml`).
  - Windows clean `cargo build --release`: thin LTO builds faster than fat LTO; binary a few percent larger. Runtime unmeasured.

---

## [0.4.0]

### Library consumers and CLI users

Observable behavior. **Action** = code or config change required.

#### Breaking

Library consumers: update code that matches on or calls these items.

- **Error types split**: `IngestError` is now `Domain(DomainError)`, `Io(io::Error)`, `Http(String)`, or `Api(String)`. `DomainError` (crate root): `Usage`, `CandidateLimitExceeded`, `InvalidSource`, `UnsupportedSource`, `InvalidMetadata`, `OutputTooLarge`.
  - **Action:** match `IngestError::Domain(DomainError::...)`, not flat variants. GitHub non-success responses -> `Api`; transport failures -> `Http`.
  - `GitRef::new`, `Source::github`, `Source::with_reference` return `Result<_, DomainError>`. `IngestError::usage` -> `DomainError::usage`.
  - `?` converts both into `IngestError`. `Display` strings unchanged (`src/core/error.rs`, `src/shell/error.rs`).
- **`GitRef` string access removed**: `GitRef::as_str` and `impl Display for GitRef`.
  - **Action:** use `&*git_ref` (`GitRef` derefs to `str`) (`src/core/model.rs`).

#### Changed

- **GitHub stream errors**: Timeouts, connection drops, and gzip/tar decode failures during archive streaming -> `IngestError::Http` ("GitHub request failed"), was `Io` ("filesystem error").
  - **Action:** match `Http`, not `Io` (`src/shell/archive.rs`).
- **TLS trust roots**: `ureq` uses compiled Mozilla roots (`rustls-webpki-roots`), not the platform verifier. CAs trusted only via OS store (e.g. corporate proxy) now rejected.
- **Inline CLI values**: `--flag=value` accepts a leading `-` (e.g. `--output=-notes.txt`). Separate-argument values still reject a leading `-`, except `-o -` (stdout); `-o -x` fails. `README.md` documents this (`src/core/cli.rs`).
- **GitHub URL authority**: `Source::parse` rejects userinfo (`user@`) with "repository URLs cannot contain credentials". Only port `443` or none accepted. Before: userinfo ignored, any port accepted.
  - **Action:** remove credentials and non-443 ports from GitHub URLs (`src/core/github/url.rs`).

#### Fixed

- **GitHub rate-limit reset**: `x-ratelimit-reset` gives a retry delay only when `x-ratelimit-remaining` is `0`. Before: a 404 said "retry available"; a 5xx with reset headers was not retried (`src/shell/github.rs`).
- **Unlinked local files**: File deleted between discovery and metadata check is skipped, as at open. Before: silently dropped (`src/shell/local.rs`).
- **Unreadable local files**: `NotFound`, `PermissionDenied`, and Windows sharing or lock violations (32, 33) at the metadata, open, or read step -> skipped, not abort. Before: only open failures were skipped; a metadata or read failure failed the run. Unreadable directories still fail fast (`src/shell/local.rs`).
- **Closed stderr**: No panic. Before: exit 101 when completion message or error was unwritable. Now exits with normal code (`src/shell/output.rs`, `clippy.toml`).
- **Output parent check**: File in place of output directory fails at startup with `NotADirectory`, naming the parent. Before: whole source collected first (tarball downloaded for GitHub), failure named no path (`src/shell/output.rs`).
- **Filesystem error paths**: I/O errors name the failing path and keep the error kind. Before: e.g. `os error 3`, no path (`src/shell/error.rs`, `src/shell/output.rs`, `src/shell/local.rs`).
- **Credential names with invalid UTF-8**: A credential-named file whose final name segment is not valid UTF-8 (e.g. `id_rsa<bytes>.key`) is ignored, as its valid-UTF-8 twin is. Before: counted in the summary as skipped, which exposed its existence (`src/core/policy.rs`).
  - **Action:** `skipped` drops by one for each such file. Expect smaller skipped counts in scripts that compare them.
- **Output directory sync after rename**: A failed parent-directory sync after the new digest is in place is a warning on stderr, and the run exits 0. Before: exit 1 although the output file had already been replaced (`src/shell/atomic.rs`).

#### Performance

- **Archive buffering**: Archive stream buffered before `GzDecoder` in `codefeed::shell::archive::collect` (size in [security section 8](docs/security_and_filtering.md#8-limit-values), `ARCHIVE_BUFFER_BYTES`).
- **Parallel local traversal**: `codefeed::shell::local::collect` walks, reads metadata, and reads bounded content in parallel (`ignore::WalkParallel`, worker channels, thread scope). Kept: zero-copy arenas, non-symlink checks, deterministic lexicographic order.
- **Metadata dedup**: One `fs::symlink_metadata()` per valid candidate replaces `entry.metadata()` / `symlink_metadata()` / `file.metadata()`. No post-open stat. Read buffers pre-reserved to bounded limits.
- **Local queue depth**: Workers block when the local queue is full, so read-ahead memory is bounded by its capacity (`LOCAL_CHANNEL_CAPACITY`, [security section 8](docs/security_and_filtering.md#8-limit-values)) plus one file per worker. This is a throughput trade-off: a smaller queue can slow large local scans. On a 9,000-file synthetic tree (Windows, release build), the current queue was not measurably slower than the previous 128-message queue; a much smaller queue was about 15% slower.

### Contributors

Internal refactors and test changes. No consumer action required.

#### Core

- **Commit SHA predicate**: `is_valid_commit_sha` is a plain `fn` over an iterator, not a `const fn` with a manual loop. Same accepted set: exactly 40 or 64 hex characters (`src/core/archive.rs`).
- **Output size check**: `FileStorage::push` rejects over-limit content before storing; `Selector` has no post-push check. Output target typed `ResolvedOutput` (only `resolve_output_target` builds it), so collection and delivery cannot receive an unresolved request (`src/core/storage.rs`, `src/shell/output.rs`).
- **Effect-free errors**: Core builds only `DomainError`. GitHub transport error constructor -> shell (`src/shell/error.rs`). `Selector::observe<E: From<DomainError>>` passes read errors through.
- **Single candidate entry**: `Selector::observe(status, size_hint, read)` settles each candidate; reads body only after admission. Removed `Admission`, `PendingEntry`, `admit`.
- **`Selector::new`**: Builds empty state directly; removed `impl Default for Selector`.
- **Sorted view**: `SortedFiles` derefs to `FileStorage` (read-only). `SortedFiles::new` still guarantees sortedness.
- **Render dispatch**: `CollectionResult::render` matches `DigestFormat`, calls `render_with`. Removed `DigestFormat::render`, `render::render`, and the forwarding method. Overflow diagnostic built once in `FileStorage::output_too_large_error`.
- **Scratch output**: `BoundedBuffer::push_usize` uses `to_string()`.
- **Path normalization**: `classify_path_segments` drops empty and `.` segments, so every `NormalizedPath` is clean regardless of caller. Archive no longer pre-filters; two unreachable branches removed (`src/core/policy.rs`, `src/core/archive.rs`).
- **Pure label rules**: `repository_url` (`src/core/github/url.rs`) and `repository_name` (`src/core/model.rs`) hold digest label rules; shell extracts path/name and calls them.
- **Retry headers**: Rate-limit headers parsed once into `RateLimitHeaders` (`src/core/github/retry.rs`). Retry delay and rate-limited flag are pure functions of parsed values and explicit `now_secs`.
- **Retry transition**: `AttemptState::retry` builds next state and `Transition::Retry`. `AttemptState::step` only picks the delay (`src/core/github/retry.rs`).
- **GitHub error text**: `api_error`, `sanitize_error_detail`, `status_retry_warning`, `transport_retry_warning` moved from `src/shell/github.rs` to `src/core/github/mod.rs`. `transport_error` stays in shell (`src/shell/error.rs`).
- **GitHub module split**: `core/github.rs` -> `core/github/{url,retry,mod}.rs`. `mod.rs` re-exports what shell and model use; call paths unchanged.
- **Dead code**: Unused `AsRef<str>` for `NormalizedPath` removed (`src/core/policy.rs`).
- **Must-use**: Pipeline phase types (`Selector`, `CollectionResult`, `RenderedDigest`, `BoundedBuffer`, `SortedFiles`) and pure value-returning functions (classification, retry delay, origin gate, size gate, repository label, commit SHA check, GitHub error and warning text) carry `#[must_use]`.

#### CLI

- **Completion message**: `completion_message` moved from `src/shell/mod.rs` to `src/core/cli.rs`. `run_cli` only chooses the stream.
- **Parse accumulator**: `cli::parse` folds flag values and the source positional through a consuming `Parsed`. One function owns "unexpected argument"; one owns "a value is required" (`src/core/cli.rs`).

#### Shell

- **Typed output target**: `OutputTarget` (`File(path)` or `Stdout`) replaces `None` as the stdout signal. `resolve_output_target` returns `ResolvedOutput`, which `deliver` takes (`src/shell/output.rs`).
- **Output parent**: `parent_dir` replaces three copies of the parent-or-`.` expression. Removed a redundant directory check (`src/shell/output.rs`).
- **Atomic write module**: `AtomicFile`, `TempFile`, `write_atomic` moved from `output.rs` to `src/shell/atomic.rs`. No behavior change.
- **Local worker message**: `WorkerMsg = Result<Candidate, IngestError>`. `Candidate` is `NonUtf8(prefix)` or `File(path, Body)`. `Body` is `Unavailable`, `OverLimit(len)`, or `Read(bytes)`.
  - Unreadable file (unlinked, or unavailable at metadata, open, or read) -> `Body::Unavailable`. Skip count unchanged.
  - `OverLimit(len)` carries the metadata length. `observe_candidate` passes it as `size_hint`, so an oversized file is skipped before its read runs. The worker still skips the read itself.
  - Impossible states (e.g. non-UTF-8 path with content, or a read body with an over-limit size) are unrepresentable. A fatal error is the `Err` arm, so there is one error channel.
  - `exceeds_size_limits` is one core predicate. `Selector::observe` and the local worker both call it, so the two gates apply one rule.
  - `LocalVisitorBuilder` merged into `LocalVisitor`.
- **Local reads**: `read_bounded` returns `Result<Option<Vec<u8>>>` (`None` = unavailable) instead of an out-parameter. Buffers pre-size to `min(file length, MAX_FILE_SIZE + 1)`. Test-only `read_local_file_into` removed (`src/shell/local.rs`).
- **Buffers**: Local and archive content is `Vec<u8>`. `Zeroizing` only for the GitHub token and error bodies.
- **Archive stream**: Tar decompression, PAX commit SHA parsing, and entry admission live in `codefeed::shell::archive`; HTTP stays in `codefeed::shell::github`. `process_tar_entry` returns `Selector` (no out-param or tuple). PAX header handled in the entry loop; bounded-read helper inlined.
- **GitHub attempt**: `Attempt::from_result`, `warn`, `into_error` hold classification, warnings, and error construction. `Attempt::Status` no longer duplicates `is_rate_limit`. `GithubClient::request` attaches origin-gated headers. Shell keeps HTTP, body reads, stderr writes (`write_stderr_line`), and sleeps (`src/shell/github.rs`).
- **Temp file state**: `TempFile` holds `TempState` (`Writing(File)`, `Closed`, `Committed`) instead of `Option<File>` + `active: bool`. Invalid access returns `io::Error`, not a panic (`src/shell/atomic.rs`).

#### Tests

- **Selector tables**: Selector tests drive `observe` only. Removed `feed_file` and `feed_skipped`; their cases are rows in `observe_counts_candidates_by_content_outcome` and `observe_does_not_read_settled_candidates`, plus new `observe_enforces_candidate_budget_before_read`.
- **Table tests**: Retry decision and archive path normalization tests are tables. Vacuous capacity assertion dropped from the local read test; local read tests exercise `collect`.
- **Shared tar fixtures**: `gzip_tarball` and `gzip_archive` in `src/shell/testing.rs`. `stream_url` returns `impl Read + Send`.
- **Removed test**: `test_archive_buffer_bytes_invariant` restated the constant. `ARCHIVE_BUFFER_BYTES > 0` remains a compile-time assertion.

### Maintainers

Documentation, build, and removals.

#### Removed

- **Unused Display**: `impl Display for NormalizedPath` (`pub(crate)`, no production caller) (`src/core/policy.rs`).
- **Build config**: Commented-out `[profile.release]` in `Cargo.toml`.

#### Documentation

- **Retry policy**: `README.md`, `context.md`, and `docs/security_and_filtering.md` state that rate-limited 403 responses are retried.
- **Ingestion order**: Local workers read bodies before `Selector::observe`. GitHub archive entries are admitted before reading.
- **Zeroization scope**: `docs/security_and_filtering.md` lists buffers `Zeroizing` erases and copies it does not (HTTP client headers, process environment, error messages).
- **Overflow diagnostics**: Cover only candidates admitted before abort; local counts vary between runs. Local runs also report whichever fatal error reaches the main thread first, so the path and error kind can vary between runs.
- **Doc structure**: One audience per document: `README.md` (users), `context.md` (maintainers, domain), `docs/security_and_filtering.md` (filter and safety spec), `docs/coding_convention.md` (Rust rules), `AGENTS.md` (agent workflow). Limit values only in `docs/security_and_filtering.md` section 8; other docs link there. `src/core/policy.rs` is the authority; code never copies the values.
- **Doc corrections**: Documented `MAX_PATH_BYTES` path skipping, `.ignore` and global-ignore behavior (not read), `transport_error` location, and `# Errors` scope (public API).
- **Documentation alignment**: `README.md` no longer lists `-h` among short flags that accept `=`; `-h=x` is an unknown option. Non-regular archive entries are dropped and not counted, as local ones are. Only delta-seconds `Retry-After` is honored; an HTTP-date falls back to backoff (`docs/security_and_filtering.md` section 6.5). The non-UTF-8 prefix is the valid segments before the first invalid one (`context.md` section 3.2).

## [0.3.2]

### Security

- **Memory zeroization**: `zeroize` applied in `src/shell/` to GitHub tokens (env lookup), HTTP error bodies, and reusable read buffers in local and tarball ingestion.
- **Sensitive patterns**: Added `id_ecdsa*`, `id_dsa*`, `*.pfx` (PKCS#12) to sensitive exclusions (`src/core/policy.rs`).

### Changed

- **Lockfile exclusions**: Added `bun.lock`, `bun.lockb`, `poetry.lock`, `Pipfile.lock`, `composer.lock`, `Gemfile.lock` to `NOISE` (`src/core/policy.rs`).
- **Path depth limit**: Local ingestion rejects paths over `MAX_PATH_DEPTH` (256 levels) with `IngestError::invalid_metadata`, matching archive handling; hidden/noise precedence preserved.
- **Diagnostic ordering**: `largest_files` tie-breaks equal sizes with `cmp_path` (component-wise lexicographic), matching tree order.

### Fixed

- **`GitRef` whitespace**: `GitRef::new` trims ASCII whitespace before storage, preventing `%20` in tarball URLs.
- **Hidden directory filtering**: Local directories pruned via `WalkBuilder::filter_entry` with `is_hidden_segment` (root kept at `depth == 0`).
- **Root non-UTF-8 diagnostics**: Root non-UTF-8 entries (`""` prefix) map to `<root>/`, not `<unknown>/`, on candidate budget overflow.

### Internal

- **Pipeline typestates**: Core collection is a consuming chain (`Selector` -> `CollectionResult` -> `RenderedDigest` -> `IngestReport`). `CollectionResult::render` consumes the collection by value with metrics.
- **Output transport**: `output::emit` replaced by pure I/O `output::deliver(target, bytes)`; `src/shell/output.rs` drops render-format and collection dependencies.
- **Policy matching**: Prefix checks in `src/core/policy.rs` consolidated into `SENSITIVE_PREFIXES`; extension extraction in `is_sensitive_file` is lazy.
- **Tree render**: `render_level` threads one owned `prefix` buffer (`push_str`/`truncate`) instead of cloning per directory.
- **Performance**: Per-entry `.zeroize()` in ingestion loops replaced by `Zeroizing` RAII `Drop` cleanup, avoiding redundant volatile overwrites.
- **Documentation**: Transitive dependency rules in `AGENTS.md`; `.env*` exclusion rationale; `cmp_path` ordering invariant in `context.md` and `storage.rs`.

## [0.3.1]

### Changed

- **Token estimate**: `ascii_bytes / 4 + non_ascii_chars` (minimum 1). Closer for Korean and CJK text; values change for any non-ASCII input.
- **GitHub retries**:
  - HTTP 403 retries only when rate-limited (`Retry-After` present or `x-ratelimit-remaining: 0`).
  - `Retry-After` and `x-ratelimit-reset` are honored. Retries stop immediately if the wait exceeds `RETRY_MAX_DELAY_MS`.
  - API errors include the remaining wait time.
- **Path limits**: Tar entries deeper than 256 directory levels (`MAX_PATH_DEPTH`) are rejected.
- **`GitRef`** rejects `.` and `..` segments.
- **Non-UTF-8 paths** keep their top-level directory prefix and count as skipped.
- **Output ordering** is deterministic across tree, header, and body.
- **Error messages**: A tar `.` root segment reports `root segment is '.'` (not directory traversal). Local traversal errors include the directory path.

### Fixed

- **Atomic file output**: Temp files are cleaned up via an RAII guard on error or panic, on Windows and Unix. Unix commits sync the parent directory; overwrites keep existing permissions.
- **Local walk errors** keep the original `io::ErrorKind` instead of `Other`.

### Internal

- **Admission**: `Ignored` and `Skipped` merged into `Settled(Selector)`. `PendingEntry` must be consumed by `accept` or `skip` (enforced at compile time). Removed `record_entry`, `record_with`, `CandidateSlot`, `Observation`, `ContentResult`. Shell collectors read into reusable scratch buffers. `CandidateBudget` is private to `selector.rs`.
- **Pure core**: No `&mut` parameters in `src/core/` (except `Display::fmt`). Selector, storage, and retry state use value-passing `self`; `AttemptState::step` returns `(Self, Transition)`. CLI parsing uses `ParseState`; path classification uses `PathClassifierState`. `classify_path_segments` and `normalize_tar_entry_path` return owned `PathStatus`. Rendering writes into an in-memory `BoundedBuffer` -> `RenderedDigest`; physical writes moved to `shell/output.rs`.
- **Storage**: `Deref` removed from `SortedFiles` in favor of explicit accessors. `largest_files` uses stable `sort_by_key` and `truncate(5)`. `FileStorage::push` uses `expect` on `u32` conversions.
- **Render**: `render_text`, `render_xml`, and `DigestFormat::render` return `std::io::Result<()>`; the size cap is `written > MAX_OUTPUT_SIZE`. ASCII tree grouping uses slice matching. Escaping goes through `write_escaped` with `escape_header`, `escape_xml_body`, `escape_xml_single_line`. `output_too_large_error` unified.
- **Policy**: Limits centralized in `policy.rs`. `MAX_RETRIES` renamed `MAX_ATTEMPTS`.
- **Shell**: Retry loop uses an `Attempt` enum instead of the inverted `Result<ureq::Error, Response>`. `MockServer` wraps the test listener. `format_completion_message` extracted and tested for completion reporting and exit codes. Removed redundant `#![cfg(test)]` in `testing.rs`.

## [0.3.0]

### Added

- **CLI Dispatch Entry Point**: `codefeed::run_cli` (`src/shell/mod.rs`) encapsulates CLI parsing, help output, ingestion, and stdio reporting.
- **Domain Types & Validation**:
  - `GitRef` enforces non-empty, non-whitespace revision invariants with zero-copy `&str` access.
  - `Source::github` validates owner/name characters, non-emptiness, and traversal segments.
  - `PathStatus` and domain path classifier `classify_path` (`src/core/policy.rs`) unify hidden segment checks, noise files, and sensitive pattern filtering.
  - `is_hidden_segment` (`src/core/policy.rs`) checks raw segment bytes for a leading dot; single source of truth for hidden paths across local traversal and tar archives.
- **Candidate Token**: `CandidateSlot` (`src/core/selector.rs`) linear proof token for candidate budget reservation before content acquisition.
- **Documentation & Quality**:
  - `#![warn(unreachable_pub)]` and `#![warn(missing_docs)]` in `src/lib.rs`; re-exported public API documented with doctests and error contracts.
  - Rustdoc and comment conventions in `docs/coding_convention.md`.

### Changed

- **Breaking Changes (Library API)**:
  - `Source::Github` is `#[non_exhaustive]` (construct via `Source::github` or `Source::parse`); `Source::Github.reference` and `Source::with_reference` changed from `Option<String>` to `Option<GitRef>`.
  - Internal items (`CliAction`, `help_text`, `parse`, internal `IngestError` constructors) demoted to `pub(crate)`; `IngestError::usage` and `codefeed::run_cli` kept. `IngestError` and `IngestReport` marked `#[non_exhaustive]`.
- **CLI & Output Resilience**:
  - **Stdout BrokenPipe**: Locked stdout writes in `run_cli` replace unbuffered prints; exits 0 cleanly on a closed pipe instead of panicking.
  - **Atomic Stdout Delivery**: Stdout buffered up to `MAX_OUTPUT_SIZE` before emission, so overflow writes zero bytes (matches atomic file rollback).
  - **Symlink Output Preservation**: `symlink_metadata` checked in output target resolution so existing leaf symlinks are not canonicalized onto source files.
- **Performance & Streaming Ingestion**:
  - **Pre-I/O Candidate Budget Reservation**: Budget checked before reading local bodies or archive entry bytes; aborts with 0 bytes read when `MAX_FILE_COUNT` is exceeded.
  - **Single-Pass Path Normalization & Policy Filtering**: Hidden-segment, noise, and sensitive-credential filtering in one pass in local traversal and archive normalization.
  - **Monotonic Byte-Offset Tree Rendering**: Depth-indexed re-traversal replaced by monotonic byte-offset slicing.
  - **Zero-Allocation Tree & Storage**: Tree rendered directly from sorted `&FileStorage` with component-wise ordering (`cmp_path`).
  - **XML Attribute & Control Escaping**: `<directory_structure>` and `<file path="...">` escaping unified; XML 1.0 whitespace references (`&#9;`, `&#10;`, `&#13;`) emitted and invalid control characters sanitized.

### Removed

- **Legacy APIs**: Test-only `CandidateInput`, `Selector::feed`, and deprecated `is_ignored_path` / `is_hidden_path` removed.
- **Unused Conversions & Wrappers**: Unused conversion traits (`TryFrom<&str>`, `FromStr`, etc.) removed from `GitRef`; single-caller capacity wrappers removed (`CandidateBudget::with_capacity`, `Selector::with_capacity`, `FileStorage::with_capacity`).

## [0.2.4]

### Added

- `CandidateInput` (`src/core/selector.rs`): unified candidate enum (`File`, `TooLarge`, `Skipped`) across the FCIS boundary.
- `render_tree` (`src/core/tree.rs`): deterministic ASCII tree from sorted path slices.
- `resolve_output_target` (`src/shell/output.rs`): single-pass output target validation and parent canonicalization.
- `policy::is_ignored_path` (`src/core/policy.rs`): direct path exclusion for noise lockfiles, sensitive prefixes/extensions, and hidden components.

### Changed

- **CLI & Output**:
  - Replaced `clap` and `clap_derive` with a ~50-line pure `std` parser keeping all CLI contracts and flags.
  - Replaced output target re-checks with single startup canonicalization and parent resolution (`resolve_output_target`).
- **Tree & Escaping**:
  - Replaced `BudgetTrie` with direct ASCII tree rendering from sorted `FileStorage`.
  - Replaced chunked loops with linear slice iterators; XML attributes streamed via `write_escaped`.
- **Candidate Ingestion & Memory**:
  - `Cow` in `CandidateInput` replaced by borrowed slices; RLE directory tracking replaced by `HashMap<String, usize>`.
  - Candidates streamed directly into `Selector::feed`, removing intermediate probe enums.
  - Binary files detected via UTF-8/NUL byte inspection during body read; static binary extension lists removed.
- **Diagnostics & Error Handling**:
  - 220-line `TokenEstimator` replaced by pure `estimate_tokens(bytes)` heuristic.
  - `retry_delay(attempt)` made pure and deterministic.
  - `IngestError` payloads unboxed.

### Removed

- External dependencies `clap` and `clap_derive`.
- Benchmark binaries and dead modules (`src/core/decide/`, `src/shell/local_io.rs`, `src/shell/github_client.rs`, `benches/bench.rs`).
- Obsolete types: `BudgetTrie`, `BudgetTrieNode`, `OutputFilter`, `Item`, `Probe`, `CompactDir`, `BINARY_EXTENSIONS`.

## [0.2.3]

### Added

- **Storage Arena & Models**:
  - `FileStorage` (`src/core/model.rs`): contiguous path/content arenas indexed by 16-byte `FileSpan`, with `FileRecord<'a>` views and in-place `sort_by_path`.
  - `CompactDir` and `CompactDir::as_bytes` (`src/core/decide.rs`): 24-byte inline directory descriptor for candidate budgets.
  - `ItemPath<'a>` (`src/core/decide.rs`): `Cow<'a, str>` wrapper for zero-copy paths across core/shell boundaries.
  - `Source::with_reference` (`src/core/model.rs`): revision and source invariant validation.
- **Pipeline & Digest Builders**:
  - `CandidateBudget::check_limit` and `Selector::finish` (`src/core/decide.rs`): O(1) batch pre-flight check and direct `CollectionResult` builder with skipped-byte tracking.
  - `TokenEstimator` (`src/core/digest/token.rs`): non-destructive state machine backed by a 256-byte `CHAR_METRIC` table.
  - Streaming shell abstractions: `SourceStream`, `LocalStream`, `GithubStream`.

### Changed

- **Tree & Ingestion**:
  - `BudgetTrie` (`src/core/digest/trie.rs`): rebuilt as a 16-byte POD node arena (`Vec<BudgetTrieNode>`) with UTF-8 truncation at 65,535 bytes.
  - Single-pass path classification and hidden segment scans during local and tarball ingestion.
  - Unified HTTP retries, 64 KiB tarball buffering, and `RedirectAuthHeaders::Never`.
- **Output & Security**:
  - `OutputFilter` and `normalize_path` (`src/shell/output.rs`): zero-syscall lexical matching with parent-canonical normalization.
  - Escaped XML control codes; rejected embedded NUL bytes in tar entry paths.

### Fixed

- CLI parsing maps `-- - -o -` to stdout instead of a file named `"-"`.
- Eliminated double-flush on atomic files; preserved `estimated_tokens` on `BrokenPipe`.
- Rejected directory output targets with `io::ErrorKind::IsADirectory` in `AtomicFile::create`.

### Removed

- Legacy iterator implementations on `LocalStream` and `GithubStream`.
- Crates `rustix`, `percent-encoding`, `memchr`, and `log`.
- Obsolete types, escape tables, and intermediate `Vec<FileRecord>` allocations.

## [0.2.2]

### Added

- `BoundedEstimatingWriter`: enforces `MAX_OUTPUT_SIZE` across framing, tree, metadata, and content bytes while estimating tokens in one pass.

### Changed

- Simplified local walk to a sequential 2-step pipeline (`ignore::WalkBuilder` harvest, then bounded reads), removing channels and worker pools.
- Switched file path sorting in `text::render` and `xml::render` to in-place `sort_unstable_by`.
- Used `trim_ascii()` for `--ref` and GitHub token parsing.

### Removed

- `Collector` and `AnyCollector` in `src/shell/collector.rs`; routing goes directly via `orchestrate::ingest`.

## [0.2.1]

### Added

- `RunConfig` (`src/core/model.rs`) consolidating `source`, `output`, and `format`.
- `Collector` trait and `AnyCollector` enum wrapping `LocalCollector` and `GitHubCollector`.
- Zero-dependency benchmark suite for trie insertion, rendering, and candidate selection.
- Library target (`src/lib.rs`) exposing core entry points and `RunConfig`.

### Fixed

- Case-insensitive sensitive path prefix matching (`src/core/policy.rs`).
- HTTP retries in `src/shell/github_client.rs` restricted to transient errors (`Io`, `Timeout`, `ConnectionFailed`).
- Canonical roots enforced against non-verbatim paths in Windows path verification.

### Changed

- GitHub authorization headers scoped to exact-origin `https://api.github.com`.
- Hardened atomic output delivery with RAII `AtomicFile`.
- `Source` restructured as an ADT embedding `reference: Option<String>` in `Source::GitHub`.
- `largest_files` diagnostic bounded to top-5 linear insertion.

### Removed

- Manual Unix FFI module `src/shell/sys.rs`.

## [0.2.0]

### Added

- Single-request GitHub tarball streaming (`/repos/{owner}/{repo}/tarball/{ref}`) via `flate2` and `tar`, extracting commit SHAs from `pax_global_header`.
- `ARCHIVE_MAX_SECS` (300 s) policy limit.

### Changed

- HTTP timeouts split into 30 s connection and 300 s body streaming limits.

### Removed

- `serde`, `serde_json`, and `libc` dependencies; GitHub Git Trees recursive blob pipeline; `MAX_JSON_SIZE` limit.

## [0.1.2]

### Fixed

- Included depth indentation in `DigestBudget` tree upper bound to prevent 20 MiB overflows.
- Enforced Windows `GetFinalPathNameByHandleW` root containment against junctions and symlinks.
- Replaced linear `Vec` scans in `CandidateBudget` directory counting with `HashMap` lookups.

### Changed

- Documented plain-text raw preservation vs. XML 1.0 control-character sanitization, and path-based sensitive file filtering.

## [0.1.1]

### Added

- Order-independent positional `<SOURCE>`, short flags (`-o`, `-f`, `-r`, `-h`), `--key=val`/`-k=val` syntax, and POSIX `--` delimiter.

### Changed

- Simplified CLI parser to linear traversal; made `DigestBudget` and `BudgetTrie` error-terminal; hardened Unix symlink and hard-link checks.

## [0.1.0] - 2026-10-11

### Added

- Initial CLI converting local directories (`.gitignore`-aware, skipping hidden files and symlinks) and GitHub repositories (`--ref <BRANCH|TAG|COMMIT>`, no local `git`) into plain-text or XML digests (`--format <text|xml>`) written atomically to files or stdout (`--output -`), with streaming token estimation, sensitive/lockfile filtering (`.env*`, `*.key`, `*.pem`, `id_rsa*`), and candidate/output size budgets.

[0.4.0]: https://github.com/but212/codefeed/compare/v0.3.2...v0.4.0
[0.3.2]: https://github.com/but212/codefeed/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/but212/codefeed/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/but212/codefeed/compare/v0.2.4...v0.3.0
[0.2.4]: https://github.com/but212/codefeed/compare/v0.2.3...v0.2.4
[0.2.3]: https://github.com/but212/codefeed/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/but212/codefeed/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/but212/codefeed/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/but212/codefeed/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/but212/codefeed/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/but212/codefeed/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/but212/codefeed/releases/tag/v0.1.0
