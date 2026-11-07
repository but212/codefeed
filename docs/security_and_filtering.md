# Security and Filtering Specification

**Authority:** `src/core/policy/` is the single source of truth for all lists and limits. Sec. 3.2-3.3 and sec. 8 mirror it; updates to `src/core/policy/` must synchronize them in the same commit. All other sections are the canonical behavior specification. Invariant enforcement (`I-xx`) is in [`context.md`](../context.md) sec. 3.

---

## 1. Threat Model

| # | Threat | Mitigation | Implementation |
| :--- | :--- | :--- | :--- |
| T1 | Secret disclosure in digest | Path/name exclusion (sec. 3). Content is not scanned (sec. 1.2). | `policy/exclusions.rs`, `path.rs` |
| T2 | Resource exhaustion | Size, count, depth, and path limits; bounded reads and channels; timeouts (sec. 5, sec. 6). | `policy/limits.rs`, `local/`, `github/` |
| T3 | Credential forwarding across origins | Origin validation; redirects strip authorization headers (sec. 6). | `core/github/url.rs`, `shell/github/` |
| T4 | Injection into terminals, XML, or LLM boundaries | Contextual output escaping (sec. 7). XML format isolates file bodies; text format isolates headers and paths while emitting bodies verbatim. | `core/render/escape.rs`, `core/render/formats.rs` |
| T5 | Traversal outside source root | Symlinks ignored; metadata checked prior to open (sec. 5). | `shell/local/` |

### 1.1 Assumptions

- **Quiescent tree:** Local source files remain unchanged during scanning. Concurrent modifications or symlink substitutions are outside the threat model.
- **Data-only GitHub archives:** Archive tarballs are processed strictly as untrusted data; archive paths follow local path classification rules.

### 1.2 Non-Goals

- **No secret/entropy scanning:** Filtering matches paths and extensions only. Hard-coded secrets in source files are not redacted.
- **In-memory token lifetime:** Zeroization is limited to the scope in sec. 6.2.
- **Text format delimiter and ANSI isolation:** In `--format text`, file bodies are emitted verbatim UTF-8 without delimiter rewriting or ANSI escape stripping. Downstream systems or LLM prompts requiring strict delimiter or boundary isolation must use `--format xml`.

---

## 2. Evaluation Order

```
Entry (local walk or tar stream)
 |
 +-- Directory (local walk only)
 |     +-- Entered: `.gitignore` read once; the same bytes validate and compile its rules.
 |     |     `-- Symlink, non-regular file, larger than `MAX_FILE_SIZE`, unreadable, or not valid UTF-8 -> Run fails (Io). Sec. 3.5.
 |     +-- Listing over `MAX_DIR_ENTRIES` entries (hidden and ignored included) -> Run fails (InvalidMetadata).
 |     +-- Hidden or ignored by a `.gitignore` -> Pruned, not entered; its `.gitignore` is never read.
 |     `-- Matched by a user exclusion (sec. 3.6) -> Pruned, not entered; its `.gitignore` is never read.
 |
 +-- Tar header read (`Entries::next`, GNU `L`/`K` and PAX `x` bodies included). Runs before path classification and applies to every entry type.
 |     `-- Over `ARCHIVE_MAX_HEADER_BYTES` for one read -> Run fails (InvalidMetadata), even for entries that would be dropped (I-03).
 +-- Decompressed archive stream over `ARCHIVE_MAX_DECOMPRESSED_BYTES` -> Run fails (InvalidMetadata), entry bodies included.
 |
 +-- Non-regular file (symlink, FIFO, socket, device, non-file tar header)
 |     `-- Dropped silently. Omitted from count.
 |
 +-- Tar stream entry normalization (archive::normalize_tar_entry_path)
 |     +-- Embedded NUL byte, traversal `..`, or `.` root segment
 |     |     `-- Run fails (InvalidMetadata).
 |     `-- Strips the first path segment (repository root directory).
 |
 +-- Path classification (path::classify_path_segments)
 |     +-- Hidden segment
 |     |     `-- Ignored. Dropped silently. Omitted from count.
 |     +-- Matched by a user exclusion (sec. 3.6)
 |     |     `-- Ignored. Dropped silently. Omitted from count.
 |     +-- Noise lockfile or sensitive pattern
 |     |     `-- Ignored. Dropped silently. Omitted from count.
 |     +-- Depth > MAX_PATH_DEPTH
 |     |     `-- Run fails (InvalidMetadata).
 |     +-- Non-UTF-8 segment, or path > MAX_PATH_BYTES
 |     |     `-- Candidate with prefix (valid segments or "").
 |     `-- Valid UTF-8 path
 |           `-- Candidate.
 |
 +-- Candidate (Selector::observe)
 |     +-- Charges candidate slot. Overflowing MAX_FILE_COUNT fails the run.
 |     +-- Path equal to an earlier candidate's path (skipped ones included) -> Run fails (InvalidMetadata).
 |     +-- Declared size > MAX_FILE_SIZE: skipped; body not read.
 |     +-- Body read (bounded by worker or archive stream).
 |     `-- Content check passes -> Included. Otherwise skipped.
 |
 `-- Digest assembly (SortedFiles::new)
       `-- Stored path is a directory prefix of another stored path -> Run fails (InvalidMetadata) (I-01). Skipped candidates are not compared, so the digest tree shows each name as either a file or a directory.
```

---

## 3. Exclusion Lists

### 3.1 Hidden Entries

- Path segments starting with `.` are hidden (`.git`, `.github`, `.vscode`, `.env*`).
- The source root itself is exempt (`codefeed ./.project` succeeds).
- Redundant separators (`a//b`) are stripped prior to evaluation.
- Local traversals prune hidden directories during walk; archives filter entries individually.

### 3.2 Noise Lockfiles

Exact matches against the final path segment:

| Lockfile | Ecosystem | Lockfile | Ecosystem |
| :--- | :--- | :--- | :--- |
| `Cargo.lock` | Rust / Cargo | `bun.lock` | Node.js / Bun (text) |
| `package-lock.json` | Node.js / npm | `bun.lockb` | Node.js / Bun (binary) |
| `yarn.lock` | Node.js / Yarn | `go.sum` | Go modules |
| `pnpm-lock.yaml` | Node.js / pnpm | `poetry.lock` | Python / Poetry |
| `composer.lock` | PHP / Composer | `Pipfile.lock` | Python / Pipenv |
| `Gemfile.lock` | Ruby / Bundler | | |

### 3.3 Sensitive Names (Case-Insensitive)

- **Prefixes (`SENSITIVE_PREFIXES`):** `id_rsa`, `id_ed25519`, `id_ecdsa`, `id_dsa`, `credentials`. Matches start of final segment (`id_rsa.pub`, `credentials.json`).
- **Extensions (`SENSITIVE_EXTENSIONS`):** `pem`, `key`, `p12`, `pkcs12`, `pfx`, `credentials`. Matches characters following final dot (`server.key`, `aws.credentials`).
- `.env` files are excluded by hidden entry rules (sec. 3.1).
- Matches evaluate on ASCII bytes regardless of path UTF-8 validity (e.g. `id_rsa\xff.key` is ignored, not counted as skipped).

### 3.4 Path Limits

| Rule | Effect |
| :--- | :--- |
| Depth > `MAX_PATH_DEPTH` | Run fails with `InvalidMetadata`. Hidden/noise items dropped prior to check. |
| Path > `MAX_PATH_BYTES` | Consumes candidate slot; counted as skipped; body not read. Its directory prefix keys the budget, truncated to `MAX_PATH_BYTES` on a char boundary (I-05). |
| Path equal to another candidate's path | Run fails with `InvalidMetadata`. Applies to skipped candidates too. |
| Stored path is a directory prefix of another stored path (`a` and `a/…`) | Run fails with `InvalidMetadata` at digest assembly. Skipped candidates are not compared. |

### 3.5 Ignore Files (Local Only)

- Applies `.gitignore` files within source tree (Git binary not required).
- `.ignore`, global gitignore, `.git/info/exclude`, and parent-directory `.gitignore` files are ignored.
- The local walk reads a directory's `.gitignore` once, when it enters that directory. The bytes that validate the file are the bytes that compile its rules, so no later read can change them.
- A `.gitignore` that is a symlink, is neither a regular file nor a directory (such as a FIFO), exceeds `MAX_FILE_SIZE`, is unreadable, or is not valid UTF-8 fails the run (`IngestError::Io`, path named). The symlink is not followed. A directory named `.gitignore` contributes no rules. A rule set that fails to compile fails the run too. Invalid glob lines are skipped.

### 3.6 User Exclusions (`--exclude`)

- Patterns use `.gitignore` syntax and are relative to the source root. Matching is case-sensitive, as in git. Each pattern must yield exactly one rule. Unescaped trailing whitespace is trimmed before parsing, as in `.gitignore`, so `'! '` is a bare `!`. A blank line, a comment, a bare `!`, or an invalid glob fails at parse time with `DomainError::Usage`. An unclosed `[` is an error here, though a `.gitignore` file accepts it.
- A `\` in a path segment is matched as U+FFFD on every platform, so it is never a separator and never matches a `\` in a pattern. Results are the same on Windows and Unix.
- The rule applies to local and GitHub sources in the same way. It is independent of `.gitignore` (sec. 3.5): a path is dropped when either one drops it. A `!` pattern re-includes only paths that other user patterns exclude.
- A path is excluded when it or any ancestor directory matches. Ancestors are tested as directories, from the top down. A `!` pattern cannot re-include a file under an excluded directory.
- Excluded local directories are pruned, so their `.gitignore` is never read. Excluded entries are not candidates and are omitted from the skipped count.
- Matching tests the first `MAX_PATH_DEPTH + 1` segments of a path, whatever its byte length, so a match on a path longer than `MAX_PATH_BYTES` still excludes it. This caps the tests per path and bounds matching work for attacker-controlled archive paths (threat T2, sec. 1).
- Classifying one path takes time linear in its byte length: each admitted directory extends its prefixes in place, and the match prefix stops growing past the window.
- A match beyond the first `MAX_PATH_DEPTH + 1` segments is not seen. Such an entry is not excluded and fails its depth limit (sec. 3.4), as any other deep entry does.

---

## 4. Candidate and Skipped Accounting

| Outcome | Candidate Slot | Counted Skipped | Body Read |
| :--- | :---: | :---: | :---: |
| Ignored (sec. 3) | No | No | No |
| Non-UTF-8 path | Yes | Yes | No |
| Path > `MAX_PATH_BYTES` | Yes | Yes | No |
| Declared size > `MAX_FILE_SIZE` | Yes | Yes | No |
| Unavailable at metadata, open, or read | Yes | Yes | Attempted |
| Binary (NUL) or invalid UTF-8 | Yes | Yes | Yes |
| Read bytes > `MAX_FILE_SIZE` | Yes | Yes | Bounded |
| Included | Yes | No | Yes |

Candidate budget overflow aborts the run without emitting partial output.

---

## 5. Local Filesystem Safety

- **No Symlinks:** The walk classifies entries with `DirEntry::file_type()`, which does not follow links, and drops entries that are neither directories nor regular files. Workers call `fs::symlink_metadata` before open, rejecting symlinks to files, directories, or special devices. The root path is canonicalized first, so a symlinked root is followed; the no-symlink rule applies to every entry beneath it.
- **Output Exclusion:** Canonical destination is resolved upfront (`resolve_output_target`) and excluded from traversal to prevent self-ingest.
- **Atomic Writes:** Destination written via temporary file `.codefeed.{nanos}.{counter}.tmp` in target directory, flushed, and renamed. The name does not depend on the target name. Unix preserves permissions and syncs parent directory. Parent sync failure logs a warning without aborting. `write_atomic` drops the file handle before `TempPath` unlinks the temporary file on `Drop`.
- **Unavailable vs. Fatal:** `NotFound`, `PermissionDenied`, or Windows sharing (32) / locking (33) errors mark files unavailable (skipped). Other I/O errors or unreadable directories abort the run. A directory's `.gitignore` is not a file candidate and follows sec. 3.5. First error in walk order is deterministically reported.
- **Bounded Concurrency:** Producer and workers communicate across bounded `sync_channel` queues (`LOCAL_CHANNEL_CAPACITY`). Memory holds at most `LOCAL_CHANNEL_CAPACITY + 2` concurrent file bodies, each bounded to `MAX_FILE_SIZE + 1`. Abort signals set an `AtomicBool` halting worker reads.

---

## 6. GitHub Network Safety

### 6.1 Authentication

- Resolves the variables listed in `TOKEN_ENV_VARS` (`src/core/policy/mod.rs`) in order: `GITHUB_TOKEN` first, then `GH_TOKEN`. Empty or whitespace values are ignored.
- Attaches `Authorization: Bearer <token>` strictly when `is_github_api_origin(url)` holds (`https://api.github.com`) (I-07).
- The error message for 403 and 429 names the token variables only for an anonymous request (I-06). A request that carried a token gets no hint. A redirect target is not sent the token; its error is classified by the original API request.
- Enforces `RedirectAuthHeaders::Never`; redirects never transmit credentials.

### 6.2 Memory

- Resolved tokens, environment buffers, and error responses use `zeroize::Zeroizing` to erase memory on drop. Copies held by HTTP client internals, the process environment, or error formatters are not zeroized.

### 6.3 Transport and TLS

- Enforces built-in Mozilla root certificates (`rustls-webpki-roots`). Operating system certificate stores and proxy CAs are rejected.

### 6.4 Streaming

- Fetches tarball in a single HTTP request (`/repos/{owner}/{repo}/tarball/{ref}`). Streams via gzip/tar directly into `Selector::observe` without disk spooling.
- Each tar header read is metered against `ARCHIVE_MAX_HEADER_BYTES` (I-03), so an extension body cannot allocate past that budget before its path is checked. One read covers the previous entry's padding, GNU `L`/`K` and PAX `x` bodies, GNU sparse extension blocks, and the header. An overrun is never a skip, because the path is not parsed. Entry bodies are not metered by the header budget; the whole decompressed stream is metered against `ARCHIVE_MAX_DECOMPRESSED_BYTES`.

### 6.5 Retries

- **Retried:** Transport timeouts/errors, HTTP 429, HTTP 5xx, HTTP 403 with `Retry-After` or zero quota.
- **Not Retried:** Other HTTP 4xx, non-transient transport errors.
- Attempts capped by `MAX_ATTEMPTS`. Exponential backoff starts at `RETRY_INITIAL_DELAY_MS` up to `RETRY_MAX_DELAY_MS`. Server wait delay from `Retry-After` (delta-seconds) or `x-ratelimit-reset` (when remaining quota is zero) exceeding `RETRY_MAX_DELAY_MS` aborts the run immediately.

### 6.6 Time Limits

- DNS resolution and connection, including the TLS handshake, must complete within `GITHUB_TIMEOUT_SECS`.
- Response headers must arrive within `GITHUB_TIMEOUT_SECS`.
- Archive download must complete within `ARCHIVE_MAX_SECS`.

---

## 7. Output Sanitization

Sanitization runs in a single pass via `write_escaped`:

1. **Text (`--format text`):**
   - Header labels and paths: escapes `\`, `\r`, `\n`. Control characters (`0x00`-`0x08`, `0x0B`, `0x0C`, `0x0E`-`0x1F`, `0x7F`) map to `\u{FFFD}`.
   - File bodies: emitted verbatim UTF-8; ANSI sequences preserved.
2. **XML (`--format xml`):**
   - Entities escaped (`&`, `<`, `>`, `"`, `'`).
   - Attribute whitespace (`\t`, `\n`, `\r`) encoded as character references (`&#9;`, `&#10;`, `&#13;`).
   - Disallowed XML 1.0 control codes map to `\u{FFFD}`.
3. **Tree Labels:** Escaped for control characters and XML entities to prevent output corruption.
4. **Error and Warning Diagnostics (`ErrorDetail`, `sanitize_error_detail`, I-04):**
   - Single-line diagnostics emitted to stderr (filesystem paths in I/O errors, root validation errors, HTTP status messages, transport error text, usage, invalid-source, unsupported-source, and invalid-metadata messages (held as `ErrorDetail`, sanitized when built, since they echo arguments, patterns, or URLs), each path and directory name in the `MAX_OUTPUT_SIZE` and `MAX_FILE_COUNT` overflow lists) convert `\t`, `\r`, `\n` to space `' '` and all other Unicode control characters to `\u{FFFD}` to prevent terminal escape injection or log forgery. The file-target completion summary (`Wrote <PATH>`) goes to stdout, and its destination path is sanitized the same way.

---

## 8. Limit Values

Sole documentation mirror for numerical limits (authoritative definitions reside in `src/core/policy/limits.rs`). Outcomes at each limit are in [`README.md`](../README.md#limits).

| Constant | Value | Enforced By |
| :--- | :--- | :--- |
| `MAX_FILE_SIZE` | 1 MiB | Declared size, bounded read, and `.gitignore` read (a `.gitignore` over the limit fails the run) |
| `MAX_PATH_BYTES` | 4,096 B | Path classification |
| `MAX_PATH_DEPTH` | 256 levels | Path classification |
| `MAX_FILE_COUNT` | 10,000 files | Candidate budget |
| `MAX_OUTPUT_SIZE` | 20 MiB | Storage check & buffer |
| `ARCHIVE_BUFFER_BYTES` | 64 KiB | Archive read buffer (buffering parameter) |
| `ARCHIVE_MAX_HEADER_BYTES` | 64 KiB | Archive header read budget |
| `ARCHIVE_MAX_DECOMPRESSED_BYTES` | 256 MiB | Decompressed archive stream meter |
| `MAX_DIR_ENTRIES` | 20,000 entries | Local directory listing |
| `ARCHIVE_PAX_GLOBAL_BYTES` | 1 KiB | PAX global header read (revision falls back to the requested ref) |
| `GITHUB_ERROR_BODY_BYTES` | 4 KiB | GitHub API error body read (rest of body left out of the message) |
| `LOCAL_CHANNEL_CAPACITY` | 64 messages | Channel bounds (blocks sender) |
| `MAX_LOCAL_WORKERS` | 12 threads | Thread pool sizing |
| `ARCHIVE_MAX_SECS` | 300 s | Archive stream timer |
| `GITHUB_TIMEOUT_SECS` | 30 s | Resolve, connect, header timeouts |
| `MAX_ATTEMPTS` | 3 attempts | Retry loop |
| `RETRY_INITIAL_DELAY_MS` | 250 ms | Backoff initial delay |
| `RETRY_MAX_DELAY_MS` | 5,000 ms | Backoff cap & wait limit |
| `ASCII_BYTES_PER_TOKEN` | 4 B | Token heuristic |
| `OVERFLOW_LARGEST_FILES` | 5 files | Overflow diagnostic (files listed) |
| `OVERFLOW_LARGEST_DIRECTORIES` | 3 directories | Overflow diagnostic (directories listed) |
