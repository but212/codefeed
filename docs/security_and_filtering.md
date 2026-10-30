# Security and Filtering Specification

**Authority:** `src/core/policy/` is the single source of truth for all lists and limits. This document mirrors it; updates to `src/core/policy/` must synchronize this document in the same commit.

---

## 1. Threat Model

| # | Threat | Mitigation | Implementation |
| :--- | :--- | :--- | :--- |
| T1 | Secret disclosure in digest | Path/name exclusion (sec. 3). Content is not scanned (sec. 1.2). | `policy/exclusions.rs`, `policy/classify.rs` |
| T2 | Resource exhaustion | Size, count, depth, and path limits; bounded reads and channels; timeouts (sec. 5, sec. 6). | `policy/limits.rs`, `local/`, `github/` |
| T3 | Credential forwarding across origins | Origin validation; redirects strip authorization headers (sec. 6). | `core/github/url.rs`, `shell/github/` |
| T4 | Injection into terminals, XML, or LLM boundaries | Contextual output escaping (sec. 7). XML format isolates file bodies; text format isolates headers and paths while emitting bodies verbatim. | `core/render/escape.rs`, `core/render/formats.rs` |
| T5 | Traversal outside source root | Symlinks ignored; metadata checked prior to open (sec. 5). | `shell/local/` |

### 1.1 Assumptions

- **Quiescent tree:** Local source files remain unchanged during scanning. Concurrent modifications or symlink substitutions are outside the threat model.
- **Data-only GitHub archives:** Archive tarballs are processed strictly as untrusted data; archive paths follow local path classification rules.

### 1.2 Non-Goals

- **No secret/entropy scanning:** Filtering matches paths and extensions only. Hard-coded secrets in source files are not redacted.
- **In-memory token lifetime:** Tokens held by HTTP client internals, process environment, or error formatters are not zeroized (sec. 6.2).
- **Text format delimiter and ANSI isolation:** In `--format text`, file bodies are emitted verbatim UTF-8 without delimiter rewriting or ANSI escape stripping. Downstream systems or LLM prompts requiring strict delimiter or boundary isolation must use `--format xml`.

---

## 2. Evaluation Order

```
Entry (local walk or tar stream)
 |
 +-- Tar stream entry normalization (archive::normalize_tar_entry_path)
 |     +-- Embedded NUL byte, traversal `..`, or `.` root segment
 |     |     `-- Run fails (InvalidMetadata).
 |     `-- Strips single repo root directory prefix.
 |
 +-- Non-regular file (symlink, FIFO, socket, device, non-file tar header)
 |     `-- Dropped silently. Omitted from count.
 |
 +-- Path classification (policy::classify_path_segments)
 |     +-- Hidden segment, noise lockfile, or sensitive pattern
 |     |     `-- Ignored. Dropped silently. Omitted from count.
 |     +-- Depth > MAX_PATH_DEPTH
 |     |     `-- Run fails (InvalidMetadata).
 |     +-- Non-UTF-8 segment
 |     |     `-- Candidate with prefix (valid segments or "").
 |     `-- Valid UTF-8 path
 |           `-- Candidate.
 |
 `-- Candidate (Selector::observe)
       +-- Charges candidate slot. Overflowing MAX_FILE_COUNT fails the run.
       +-- Exceeds MAX_PATH_BYTES or declared MAX_FILE_SIZE: skipped; body not read.
       +-- Body read (bounded by worker or archive stream).
       `-- Content check passes -> Included. Otherwise skipped.
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
| Path > `MAX_PATH_BYTES` | Consumes candidate slot; counted as skipped; body not read. |

### 3.5 Ignore Files (Local Only)

- Applies `.gitignore` files within source tree (Git binary not required).
- `.ignore`, global gitignore, `.git/info/exclude`, and parent-directory `.gitignore` files are ignored.

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

- **No Symlinks:** Walker uses `follow_links(false)`. Workers call `fs::symlink_metadata` before open, rejecting symlinks to files, directories, or special devices.
- **Output Exclusion:** Canonical destination is resolved upfront (`resolve_output_target`) and excluded from traversal to prevent self-ingest.
- **Atomic Writes:** Destination written via temporary file in target directory, flushed, and renamed. Unix preserves permissions and syncs parent directory. Parent sync failure logs a warning without aborting. `AtomicFile` handles close prior to unlinking on `Drop`.
- **Unavailable vs. Fatal:** `NotFound`, `PermissionDenied`, or Windows sharing (32) / locking (33) errors mark files unavailable (skipped). Other I/O errors or unreadable directories abort the run. First error in walk order is deterministically reported.
- **Bounded Concurrency:** Producer and workers communicate across bounded `sync_channel` queues (`LOCAL_CHANNEL_CAPACITY`). Memory holds at most `LOCAL_CHANNEL_CAPACITY + 2` concurrent file bodies, each bounded to `MAX_FILE_SIZE + 1`. Abort signals set an `AtomicBool` halting worker reads.

---

## 6. GitHub Network Safety

### 6.1 Authentication

- Resolves `GITHUB_TOKEN` first, then `GH_TOKEN`. Empty or whitespace values are ignored.
- Attaches `Authorization: Bearer <token>` strictly when `is_github_api_origin(url)` holds (`https://api.github.com`).
- Enforces `RedirectAuthHeaders::Never`; redirects never transmit credentials.

### 6.2 Memory

- Resolved tokens, environment buffers, and error responses use `zeroize::Zeroizing` to erase memory on drop.

### 6.3 Transport and TLS

- Enforces built-in Mozilla root certificates (`rustls-webpki-roots`). Operating system certificate stores and proxy CAs are rejected.

### 6.4 Streaming

- Fetches tarball in a single HTTP request (`/repos/{owner}/{repo}/tarball/{ref}`). Streams via gzip/tar directly into `Selector::observe` without disk spooling.

### 6.5 Retries

- **Retried:** Transport timeouts/errors, HTTP 429, HTTP 5xx, HTTP 403 with `Retry-After` or zero quota.
- **Not Retried:** Other HTTP 4xx, non-transient transport errors.
- Attempts capped by `MAX_ATTEMPTS`. Exponential backoff starts at `RETRY_INITIAL_DELAY_MS` up to `RETRY_MAX_DELAY_MS`. Server wait delay from `Retry-After` (delta-seconds) or `x-ratelimit-reset` (when remaining quota is zero) exceeding `RETRY_MAX_DELAY_MS` aborts the run immediately.

### 6.6 Time Limits

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
4. **Error and Warning Diagnostics (`sanitize_error_detail`):**
   - Single-line diagnostics emitted to stderr (filesystem paths in I/O errors, root validation errors, HTTP status messages, transport error text) convert `\t`, `\r`, `\n` to space `' '` and all other Unicode control characters to `\u{FFFD}` to prevent terminal escape injection or log forgery.

---

## 8. Limit Values

Single documentation source for numerical limits (mirrors `src/core/policy/limits.rs`):

| Constant | Value | Enforced By | On Overflow |
| :--- | :--- | :--- | :--- |
| `MAX_FILE_SIZE` | 1 MiB | Declared size and bounded read | Skipped, counted in summary |
| `MAX_PATH_BYTES` | 4,096 B | Path classification | Skipped, counted in summary |
| `MAX_PATH_DEPTH` | 256 levels | Path classification | Run fails (`InvalidMetadata`) |
| `MAX_FILE_COUNT` | 10,000 files | Candidate budget | Run fails; lists 3 largest dirs |
| `MAX_OUTPUT_SIZE` | 20 MiB | Storage check & buffer | Run fails; lists 5 largest files |
| `ARCHIVE_BUFFER_BYTES` | 64 KiB | Archive read buffer | Buffering parameter (no overflow) |
| `LOCAL_CHANNEL_CAPACITY` | 64 messages | Channel bounds | Blocks sender (no overflow) |
| `MAX_LOCAL_WORKERS` | 12 threads | Thread pool sizing | Worker concurrency limit |
| `ARCHIVE_MAX_SECS` | 300 s | Archive stream timer | Fails with request error |
| `GITHUB_TIMEOUT_SECS` | 30 s | Header timeout | Retried per sec. 6.5 |
| `MAX_ATTEMPTS` | 3 attempts | Retry loop | Halts retries; run fails |
| `RETRY_INITIAL_DELAY_MS` | 250 ms | Backoff initial delay | Initial retry sleep |
| `RETRY_MAX_DELAY_MS` | 5,000 ms | Backoff cap & wait limit | Longer server wait fails run |
| `ASCII_BYTES_PER_TOKEN` | 4 B | Token heuristic | Token estimate calculation |
