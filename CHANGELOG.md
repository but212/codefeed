# Changelog

All notable changes are listed here.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.0.0/). Versioning: [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Scope: `### Library consumers and CLI users` lists observable behavior. `### Contributors` lists contract changes (`pub` and `pub(crate)`), invariant (`I-xx`) changes, and contributor workflow changes only.

---

## [Unreleased]

### Library consumers and CLI users

#### Changed

- **Install docs**: `README.md` and `docs/README.ko.md` lead with `cargo install codefeed` (source build); `cargo binstall codefeed` remains as an optional prebuilt-binary install. The `[package.metadata.binstall]` table is unchanged.

### Contributors

Internal change. No consumer action required. Output is unchanged throughout.

#### Added

- **Contributor guidelines**: `CONTRIBUTING.md` and `docs/CONTRIBUTING.ko.md` provide contributor onboarding, workflow conventions, and verification checklists.
- **Security policy**: `SECURITY.md` owns vulnerability reporting (private GitHub Security Advisories, latest release supported).

#### Changed

- **Document ownership**: Output format (header, token estimate) is owned by `README.md` Output; limit authority is stated once in the `docs/security_and_filtering.md` header; `AGENTS.md` sec. 6 routes `CONTRIBUTING.md` and `SECURITY.md` changes. Contributor-facing documents no longer link to `AGENTS.md`: `CONTRIBUTING.md` owns the verification checklist, `AGENTS.md` links to it, and the pull request template mirrors the commands inline. `context.md` sec. 5 is reduced to module responsibilities, and `docs/security_and_filtering.md` no longer names implementation identifiers.

---

## [0.9.0]

### Library consumers and CLI users

Observable behavior. **Action** = code or config change required.

#### Breaking

- **Error text is `ErrorDetail`**: The payload of `DomainError::{Usage, InvalidSource, UnsupportedSource, InvalidMetadata}` and `IngestError::Http`, and the names in `CandidateLimitExceededError::largest_directories` and `OutputTooLargeError::largest_files`, were `String`. They are now `ErrorDetail`, a new exported type whose only constructor sanitizes control characters. `Display` no longer sanitizes again, and the rendered messages are unchanged (`src/core/error.rs`, `src/shell/error.rs`).
  - **Action:** Read the text with `&*detail`, `detail.to_string()`, or `detail == *"text"`. Build a variant with `ErrorDetail::new(text)`. Code that matched `DomainError::Usage(String)` and similar patterns on the payload type must use `ErrorDetail`.
- **`DomainError::usage` is crate-private**: No external caller existed (`src/core/error.rs`).
  - **Action:** Build `DomainError::Usage(ErrorDetail::new(text))` explicitly.
- **`From<io::Error> for IngestError` removed**: No code path used it, and it admitted unsanitized `io::Error` text (`src/shell/error.rs`).
  - **Action:** Build `IngestError::Io(err)` explicitly.

#### Fixed

- **`-h` and `-V` win over any other argument error**: An invalid `--format`, `--exclude`, unknown option, or extra positional placed before `-h` or `-V` used to fail with a usage error; now the help or version text prints and the exit code is 0. Only a `-h` or `-V` taken as a flag value or placed after `--` is ignored (`src/core/cli.rs`).
- **Archive read cap no longer underflows after an overrun**: `StreamMeter::read_cap` subtracted the running total from `ARCHIVE_MAX_DECOMPRESSED_BYTES` without saturation, so a read request after the total passed the limit panicked in debug builds and wrapped in release builds (`src/shell/github/archive.rs`).
- **URL schemes are case-insensitive**: `HTTPS://github.com/owner/repo` used to be treated as a local path and failed with a filesystem error; it now resolves as a GitHub source. `Git@…` and `SSH://…` are now rejected as unsupported like their lowercase forms (`src/core/source.rs`, `src/core/github/url.rs`).

### Contributors

Internal change. No consumer action required. Output is unchanged throughout.

#### Changed

- **Module consolidation**:
  - `src/core/collection.rs` is dissolved. `CollectionResult` moves to `src/core/selector.rs` (`Selector` -> `CollectionResult`); `repository_name` becomes a private helper in `src/shell/local/mod.rs`.
  - `src/shell/local/gitignore.rs` folds into `walk.rs` and `read.rs` into `pipeline.rs`, leaving `mod.rs`, `pipeline.rs`, `walk.rs`.
  - `src/core/render/escape.rs` folds into `formats.rs`; escaping is private to formatting.
  - `src/shell/github/archive.rs` fan-out drops from 7 to 6 (threshold <= 6).
  - `GithubName` moves to `src/core/github/url.rs` (`codefeed::GithubName` unchanged). `parse_github_url` returns validated `GithubName` values, `valid_github_name` is private, and the percent-encoding table derives from the name alphabet plus `~`.
  - `src/core/mod.rs` links the purity rules in `docs/coding_convention.md`; `src/shell/mod.rs` and `src/shell/cli.rs` import from the defining modules.
  - Test modules use `crate::` paths instead of `super::super` (coding convention sec. 3.4).
- **Invariants**:
  - **I-04**: `sanitize_error_detail` is private to `src/core/error.rs`, so `ErrorDetail::new` is the only sanitizer entry point (callers in `src/core/cli.rs`, `src/core/github/mod.rs`, `src/shell/error.rs`, `src/shell/atomic.rs`).
  - **I-10**: `CandidatePath::NonUtf8` becomes `Unstorable` and also holds a path over `MAX_PATH_BYTES`, so `NormalizedPath` is within the limit by construction. `Selector::observe` drops its path-length check, and `exceeds_size_limits` becomes `exceeds_file_size` (`src/core/path.rs`, `src/core/selector.rs`, `src/shell/local/read.rs`).
  - **I-11**: `Selector::observe` takes one `Observation` (`Unstorable(prefix)` or `File(path, Body)`) instead of a `CandidatePath`, a size hint, and a read closure. The local `Candidate` and `Body` types and `observe_candidate` are removed. The archive reader reads a body before observing it; a body over the declared limit is still not read (`src/core/selector.rs`, `src/shell/local/`, `src/shell/github/archive.rs`).
- **Small cleanups**:
  - `read_bounded` states its size precondition with `expect` instead of a dead fallback.
  - `FileSpan` shares one slice helper for path and content.
  - `RenderedDigest` drops its file counts, which `ingest` reads from `CollectionResult`.
  - The `memmem` prefilters in `src/core/github/archive.rs`, the chunked `u32` counters in `src/core/render/budget.rs`, and the `XML_BODY_START` table with its exhaustive test in `src/core/render/formats.rs` are removed.
- **Tests**: Tests already covered by an exhaustive oracle, a model property, or a corpus table are merged or removed, keeping one literal anchor per oracle.
  - `classification_corpus_is_stable` in `src/core/path.rs` holds the classification rows of the archive and path tests.
  - `src/core/github/{mod,url,archive}.rs`, `src/core/render/formats.rs`, `src/core/selector.rs`, and `src/core/error.rs` drop covered examples.
  - `src/core/policy/exclusions.rs` checks `NOISE`, `SENSITIVE_PREFIXES`, and `SENSITIVE_EXTENSIONS` against `docs/security_and_filtering.md` sec. 3.2-3.3 instead of copying the lists.
  - `is_retryable_http_status` is checked for every status from 100 to 599.
  - The `src/shell/cli.rs` tests move to `tests/cli.rs` (non-existent source exits with code 1).
- **Docs and lints**:
  - `docs/coding_convention.md` links the Rust API Guidelines for naming and conversions, drops rules that duplicate `AGENTS.md`, `context.md`, or `docs/security_and_filtering.md` (OS and network rules, memory layout, limit ownership), and tags rules with the lint that enforces them. `AGENTS.md` points filesystem and network rules to security sec. 5-6 only.
  - `context.md` names `exceeds_file_size`.
  - Added `forbid(unsafe_code)` in `src/lib.rs` and `src/main.rs`; clippy `unwrap_used` (tests exempt via `clippy.toml`), `missing_errors_doc`, and `missing_panics_doc` in `src/lib.rs`. No code change was needed.

---

## [0.8.0]

### Library consumers and CLI users

Observable behavior. **Action** = code or config change required.

#### Added

- **`-V`, `--version`**: Prints `codefeed <version>` to stdout, exits 0. As with `--help`, the first of the two flags wins and later argument errors are ignored (`src/core/cli.rs`, `src/shell/cli.rs`).
- **Release targets, `cargo-binstall`**: Releases add `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-gnu` archives. `[package.metadata.binstall]` lets `cargo binstall codefeed` install the prebuilt binary (`.github/workflows/release.yml`, `Cargo.toml`).

#### Changed

- **`GithubName` drops `#[repr(transparent)]`**: No layout promise identical to `String` (`src/core/source.rs`).
  - **Action:** Replace transmute or FFI use of `GithubName` as `String` with `GithubName::as_str` or `to_string`.

#### Fixed

- **Unbounded listing and decompressed stream**: A local directory with more than `MAX_DIR_ENTRIES` entries (hidden and ignored included), or a GitHub archive whose decompressed stream exceeds `ARCHIVE_MAX_DECOMPRESSED_BYTES`, now fails with `InvalidMetadata` (`src/shell/local/walk.rs`, `src/shell/github/archive.rs`, `src/core/policy/limits.rs`).

### Contributors

Internal change. No consumer action required.

#### Changed

- **Invariants enforced at the boundary**: `GithubClient` fields and `FileStorage::sort_by_path` are private. `NormalizedPath::as_str` is removed in favor of `Deref`. `classify_path_segments` doc matches the depth-window invariant (`src/shell/github/client.rs`, `src/core/storage.rs`, `src/core/path.rs`).
- **Hollow abstractions removed**: `FormatRenderer`, `TextRenderer`, and `XmlRenderer` give way to direct dispatch on `DigestFormat` in `render_digest` (`src/core/render/formats.rs`, `src/core/render/mod.rs`). `PathStatus` becomes `Option<CandidatePath>` (`Valid` or `NonUtf8`), with ignored paths as `None` at `classify_path_segments`, `classify_leaf`, and `normalize_tar_entry_path`, which removes the impossible `Ignored` branch from `Selector::observe` (`src/core/path.rs`, `src/core/selector.rs`, `src/shell/github/archive.rs`). Output unchanged.
- **Tiny modules folded**: `TOKEN_ENV_VARS` moves from `src/core/policy/auth.rs` to `src/core/policy/mod.rs`. The HTTP retry loop moves from `src/shell/github/stream.rs` to `src/shell/github/mod.rs`. Crate paths unchanged.

## [0.7.0]

### Library consumers and CLI users

Observable behavior. **Action** = code or config change required.

#### Breaking

- **`IngestError::Api` carries `GithubApiError`**: The payload was a `String`. It is now a struct with read-only `status() -> u16` and `retry_after() -> Option<Duration>`. The message drops the reason phrase: `GitHub API error: HTTP 404 Not Found: not found` becomes `GitHub API error: HTTP 404: not found`. Private fields keep the code and the text in sync (`src/core/github/mod.rs`, `src/shell/error.rs`).
  - **Action:** Match `IngestError::Api(err)` and call `err.status()` or `err.retry_after()`. Do not parse the reason phrase from `err.to_string()`; it no longer contains one.
- **`DomainError::OutputTooLarge` carries `OutputTooLargeError`**: The payload was a bare list of largest files. It is now a struct with public `limit: u64` and `largest_files`. `Display` text is unchanged (`src/core/error.rs`).
  - **Action:** Match `DomainError::OutputTooLarge(err)` and read `err.largest_files`. The limit is in `err.limit`.

#### Fixed

- **Completion summary sanitizes the destination**: `Wrote <PATH>` sanitizes the output path via `sanitize_error_detail` (`\t`, `\r`, `\n` become spaces; other control characters become `\u{FFFD}`). Before, control characters reached stdout raw (`src/core/cli.rs`, `docs/security_and_filtering.md` sec. 7).
  - **Action:** None for ordinary paths.
- **Overlong directory names no longer grow the budget unbounded**: The per-directory budget key was the full first segment, including skipped paths over `MAX_PATH_BYTES`. It is now truncated to `MAX_PATH_BYTES` at a char boundary, so names sharing a prefix share a key. Overflow diagnostics show the truncated name (`src/core/selector.rs`, `docs/security_and_filtering.md` sec. 3.4).
  - **Action:** None.

#### Changed

- **Non-regular `.gitignore` fails the run**: A local `.gitignore` that is a FIFO, socket, or device fails with `IngestError::Io` (`InvalidInput`, "not a regular file"); before, its rules were dropped silently. A directory named `.gitignore` still adds no rules (`src/shell/local/walk.rs`, `docs/security_and_filtering.md` sec. 3.5).
  - **Action:** Remove or replace a non-regular `.gitignore` in the source tree.
- **Token hint only for anonymous 403 and 429**: A 403 or 429 from a run whose GitHub API request sent a token no longer ends with the `set GITHUB_TOKEN or GH_TOKEN` hint: the token was not the missing step. Anonymous requests keep it. A redirect target gets no token; its error is classified by the API request (`src/core/github/mod.rs`).
  - **Action:** None.
- **Retry warning names the status code only**: The stderr line reads `GitHub HTTP status 503 on attempt 1/3`, not `GitHub HTTP status 503 Service Unavailable on attempt 1/3` (`src/core/github/mod.rs`).
  - **Action:** Do not parse the reason phrase from the warning.

### Contributors

Internal change. No consumer action required.

#### Changed

- **Release profile optimized for binary size**: `[profile.release]` adopts full LTO (`lto = true`), size optimization (`opt-level = "s"`), abort on panic (`panic = "abort"`), symbol stripping (`strip = "symbols"`), and packed split debuginfo (`split-debuginfo = "packed"`, `debug = "full"`), with incremental compilation enabled (`Cargo.toml`). Reduces Windows release executable size from 3,661 KB to 2,131 KB (~41.8% reduction), preserving full symbols in the external PDB.
- **Token estimate counts in chunks**: `estimate_tokens_from_bytes` (`src/core/render/budget.rs`) uses chunked `u32` counters instead of an index loop and is no longer `const fn`. Output and `estimated_tokens` unchanged. A reference test covers every byte value at boundary lengths.
- **Path sort compares bytes**: `cmp_path` (`src/core/storage.rs`) compares bytes, not `split('/')` components. Order and output unchanged; an exhaustive small-alphabet test pins it.
- **Pure helpers take owned values**: `push_match_segment` (`src/core/policy/exclude.rs`) and `admit_dir` (`src/core/policy/classify.rs`) take and return owned values, not `&mut` (coding convention sec. 3). No behavior change.
- **Pure queries are `#[must_use]`**: Non-`Result` `pub(crate)` and `pub(super)` queries in `src/core` (coding convention sec. 3). No behavior change.
- **Tree rendering asserts its invariant**: `render_level` (`src/core/render/tree.rs`) panics on a path ending before the current tree offset instead of dropping the rest of the tree. `NormalizedPath` makes this unreachable; output is unchanged.
- **Path types moved to `core/path.rs`**: `NormalizedPath`, `PathStatus`, `AdmittedDir`, and the classification functions move from `src/core/policy/classify.rs` to `src/core/path.rs`. `error.rs` no longer imports `policy`, removing the `error`/`policy` cycle. `push_match_segment` is `pub(crate)`. No behavior change.
- **Report built in shell**: `RenderedDigest` carries bytes and counts. `CollectionResult::render` takes only the format; `ingest` builds `IngestReport` with the requested output. Output is unchanged (`src/core/render/`, `src/shell/mod.rs`).
- **GitHub API origin injected**: `shell/github::collect` takes the API origin; `ingest` passes `GITHUB_API_BASE_URL`. The global test override and its helpers are removed. The auth header still goes only to that origin (`src/shell/mod.rs`, `src/shell/github/mod.rs`, `src/shell/testing.rs`).
- **Diagnostic writers and `.gitignore` loading split out**: `shell/stdio.rs` owns the stdout and stderr line writers, and `shell/local/gitignore.rs` owns `.gitignore` loading. No behavior change.
- **Selector claims paths in one step**: `Selector::claim_path` records each candidate path, and `CandidateBudget` holds counts only. Check order is unchanged.
- **Depth error mapping in one place**: `DepthExceededError::domain_error` builds the depth error for tar entries and local paths (`src/core/path.rs`, `src/core/archive.rs`, `src/shell/local/walk.rs`).
- **Token variables and token use decided in one place**: `TOKEN_ENV_VARS` (`src/core/policy/auth.rs`) lists `GITHUB_TOKEN` then `GH_TOKEN`; the token hint reads the same list. `api_error` takes `Auth`. `GithubClient::credentials(url)` decides whether a request carries the token; the header and the error's `Auth` both derive from it (`src/core/github/mod.rs`, `src/shell/github/client.rs`, `src/shell/github/stream.rs`).
- **Archive streaming under `shell/github/`**: `src/shell/archive.rs` moves to `src/shell/github/archive.rs`. It is used only by the GitHub path. No behavior change.
- **Source and collection split from `model.rs`**: `Source`, `GitRef`, and `GithubName` move to `src/core/source.rs`, and `CollectionResult` and `repository_name` move to `src/core/collection.rs`. `src/core/model.rs` keeps `RunConfig`, `OutputTarget`, `DigestFormat`, and `IngestReport`. Public paths from `codefeed` are unchanged. No behavior change.
- **Directory prefix separator in one place**: `admit_dir` appends the `/` separator to every admitted directory prefix, so `with_separator`, `join_segment`, and `push_separator` (`src/core/path.rs`) are removed. Output is unchanged.
- **Tar path rules under `core/github/`**: `src/core/archive.rs` moves to `src/core/github/archive.rs`. It is used only by the GitHub path. No behavior change.

---

## [0.6.0]

### Library consumers and CLI users

Observable behavior. **Action** = code or config change required.

#### Breaking

- **`--exclude '! '` is a usage error, not a re-include-all**: Unescaped trailing whitespace is trimmed before parsing, as in `.gitignore`. Before, `! ` and `!<TAB>` were accepted and compiled to a whitelist rule matching every path, re-including all excluded files. `/ ` and `!/ ` were no-ops. All four are now usage errors (`src/core/policy/exclude.rs`, `docs/security_and_filtering.md` sec. 3.6).
  - **Action:** Remove patterns that are only `!` or `/` plus trailing whitespace. `*.log ` matches as before.
- **`--exclude` matches `\` the same on every platform**: A `\` in a path segment matches as U+FFFD everywhere. Before, Windows read `\` as `/`, so `--exclude secret.md` also dropped `docs\secret.md`. On Unix, a pattern matching a `\` in a file name (such as `a\\b` for `a\b`) no longer matches it (`src/core/policy/exclude.rs`, `docs/security_and_filtering.md` sec. 3.6).
  - **Action:** Rewrite Unix patterns that depended on a `\` in a file name.
- **Root entries no longer merge with a directory named `<root>`**: The `MAX_FILE_COUNT` overflow list groups root-level entries under `./`, not `<root>/`. Before, a top-level `<root>` directory shared that key, and its count merged with the root files (`src/core/selector.rs`, `context.md` sec. 3.2).
  - **Action:** Library consumers comparing `CandidateLimitExceededError::largest_directories` names to `"<root>/"` must compare to `"./"`.

#### Fixed

- **Overflow diagnostics sanitize names**: Paths and directory names in the `MAX_FILE_COUNT` and `MAX_OUTPUT_SIZE` overflow lists are sanitized like other stderr diagnostics: `\t`, `\r`, `\n` become spaces; other control characters become `\u{FFFD}` (`src/core/error.rs`).
- **Usage and source errors sanitize echoed input**: Messages that echo an argument, `--exclude` pattern, or URL now pass the same control-character sanitization as overflow lists. The sanitization runs in `DomainError`'s `Display`, so it also covers variants built directly. Before, an ESC in these values reached stderr raw. The required-arguments message is one line: `the following required arguments were not provided: <SOURCE>` (`src/core/error.rs`, `src/core/cli.rs`, `src/core/policy/exclude.rs`, `src/core/github/url.rs`).
  - **Action:** None for ordinary inputs. Scripts that matched the old two-line required-arguments text must match the one-line text.
- **Archive file and directory at one path fail the run**: A tarball that stores `a` as a file and also has entries under `a/` fails with `InvalidMetadata`. Before, the digest listed `a` as both. A skipped `a` (binary, oversized, or non-text) does not trigger it (`src/core/storage.rs`, `docs/security_and_filtering.md` sec. 2 and 3.4).
  - **Action:** None for ordinary repositories.
- **Oversized archive headers fail the run**: A GitHub tarball whose GNU long-name, GNU long-link, or PAX record exceeds the header read budget fails with `InvalidMetadata`, even if policy would drop the entry (hidden, excluded, non-regular). Before, tar read the whole record, then skipped or dropped the entry (`src/shell/archive.rs`, `docs/security_and_filtering.md` sec. 2, 6.4, 8).
  - **Action:** None for ordinary repositories. Headers within budget whose paths exceed `MAX_PATH_BYTES` still skip.

### Contributors

Internal refactors and test changes. No consumer action required.

#### Added

- **Walk and ignore differential tests**: `write_reference_fixture` (`src/shell/local/walk.rs`) covers parent-directory pruning vs child negation, directory trailing-slash semantics, and mixed BOM handling, compared against `ignore::WalkBuilder`.
- **Limit documentation sync test**: Unit test in `src/core/policy/limits.rs` checks every limit constant against the mirror table in `docs/security_and_filtering.md` §8.
- **Offline error-path tests**: Unit tests for mock GitHub streaming, CLI success branches (file and stdout), directory and file validation edges, pipeline channel error transitions, atomic-write temp collisions, and output pipe errors.

#### Changed

- **Remaining read caps moved to policy**: GitHub API error body and PAX global header caps are `GITHUB_ERROR_BODY_BYTES` and `ARCHIVE_PAX_GLOBAL_BYTES` in `src/core/policy/limits.rs`, mirrored in `docs/security_and_filtering.md` sec. 8. Overflow and selector tests derive expected values from the constants. No observable change.
- **Walk carries relative paths**: Walk frames and `RegularFile` hold root-relative paths; `classify_entry` and directory exclusion no longer call `strip_prefix` or its unreachable error branch. No behavior change (`src/shell/local/walk.rs`, `src/shell/local/pipeline.rs`).
- **Docs match code**: Security spec sec. 2 shows tar header read before path normalization; sec. 5 describes the custom walk's symlink handling. README marks `.gitignore` as local-only. `IngestReport::skipped_files` rustdoc lists every skip reason (`docs/security_and_filtering.md`, `README.md`, `src/core/model.rs`).
- **Rendered digest carries its report**: `RenderedDigest` holds `bytes` and `report: IngestReport`; `CollectionResult::render` takes the `OutputTarget`. Crate-internal, no observable change (`src/core/render/`, `src/shell/mod.rs`).
- **Single tarball URL builder**: `tarball_url` takes the API base; the test-only GitHub URL uses it too, so tests exercise production path encoding (`src/core/github/url.rs`, `src/shell/github/mod.rs`).
- **Path classifier without state enum**: `classify_path_segments` returns at the first non-UTF-8 segment instead of tracking `PathClassifierState` (`src/core/policy/classify.rs`).
- **Duplicate broken-pipe tests removed**: One shared `BrokenPipeWriter` serves both output tests (`src/shell/output.rs`).
- **Limit authority clarified**: `AGENTS.md` and `docs/security_and_filtering.md` state that `src/core/policy/` is the sole limit authority and `docs/security_and_filtering.md` §8 the sole documentation mirror.
- **Unused re-export removed**: `src/core/github/mod.rs` imports `sanitize_error_detail` instead of re-exporting it. No consumer-visible path changes.
- **Overflow list sizes moved to policy**: `OVERFLOW_LARGEST_FILES` and `OVERFLOW_LARGEST_DIRECTORIES` in `src/core/policy/limits.rs` replace literals in `src/core/storage.rs` and `src/core/selector.rs`. Values unchanged.

---

## [0.5.0]

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
- **Tar root segment error wording**: `"Tar entry root segment is '.'; expected a single repository root directory"` -> `"Tar entry root segment cannot be '.'"` (`src/core/archive.rs`).
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
- **Policy split**: `src/core/policy.rs` -> `src/core/policy/` (`limits.rs`, `exclusions.rs`, `classify.rs`), with co-located tests. Behavior, constants, and re-exports unchanged (`src/core/policy/`).
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
- **GitHub module split**: `src/shell/github.rs` -> `src/shell/github/` (`client`, `stream`). `MockServer` moved to `src/shell/testing.rs`. Tests and behavior unchanged (`src/shell/github/`, `src/shell/testing.rs`).
- **Local pipeline**: Producer sorts the walk (`ignore::WalkBuilder::sort_by_file_name`); workers read bodies concurrently; main thread settles in walk order. In-flight bodies bounded by queue capacity (`LOCAL_CHANNEL_CAPACITY`). Hyperfine release on Windows: large synthetic tree and this repository both within about 2% of before (`src/shell/local/pipeline.rs`, `src/core/policy/limits.rs`).
- **Local module split**: `src/shell/local.rs` -> `src/shell/local/` (`walk`, `read`, `pipeline`). Tests and behavior unchanged. `run_cli` reports errors via one helper (`src/shell/mod.rs`).

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

[Unreleased]: https://github.com/but212/codefeed/compare/v0.9.0...HEAD
[0.9.0]: https://github.com/but212/codefeed/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/but212/codefeed/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/but212/codefeed/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/but212/codefeed/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/but212/codefeed/compare/v0.4.0...v0.5.0
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
