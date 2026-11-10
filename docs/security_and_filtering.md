# Security and Filtering Specification

**Authority:** `src/core/policy/` is the single source of truth for all lists and limits. § 3.2-3.3 and § 8 mirror it; updates to `src/core/policy/` must synchronize them in the same commit. § 8 mirrors every limit in `src/core/policy/limits.rs` (`*_USIZE` casts excluded). All other sections are canonical behavior specifications. Invariant enforcement (`I-xx`) is in [`context.md`](../context.md) § 3.

---

## 1. Threat Model

| # | Threat | Mitigation | Implementation |
| :--- | :--- | :--- | :--- |
| T1 | Secret disclosure in digest | Path/name exclusion (§ 3). Content is not scanned (§ 1.2). | `policy/exclusions.rs`, `path.rs` |
| T2 | Resource exhaustion | Size, count, depth, and path limits; bounded reads and channels; timeouts (§ 5, § 6). | `policy/limits.rs`, `local/`, `github/` |
| T3 | Credential forwarding across origins | Origin validation; redirects strip authorization headers (§ 6). | `core/github/url.rs`, `shell/github/` |
| T4 | Injection into terminals, XML, or LLM boundaries | Contextual output escaping (§ 7). XML format isolates file bodies; text format isolates headers and paths while emitting bodies verbatim. | `core/render/formats.rs` |
| T5 | Traversal outside source root | Symlinks ignored; metadata checked prior to open (§ 5). | `shell/local/` |

### 1.1 Assumptions

- **Quiescent tree:** Local source files remain unchanged during scanning. Concurrent modifications or symlink swaps are outside the threat model.
- **Data-only GitHub archives:** Tarballs are processed strictly as untrusted data; archive paths follow local classification rules.

### 1.2 Non-Goals

- **No secret/entropy scanning:** Filtering matches paths and extensions only. Hard-coded secrets in source files are not redacted.
- **In-memory token lifetime:** Zeroization is limited to § 6.2 scope.
- **Text format delimiter and ANSI isolation:** `--format text` emits file bodies as verbatim UTF-8 without delimiter rewriting or ANSI stripping. Boundaries requiring strict isolation must use `--format xml`.

---

## 2. Evaluation Order

```
Entry (local walk or tar stream)
 |
 +-- Directory (local walk only)
 |     +-- Entered: `.gitignore` read once; the same bytes validate and compile its rules.
 |     |     `-- Symlink, non-regular file, larger than `MAX_FILE_SIZE`, unreadable, or not valid UTF-8 -> Run fails (Io). § 3.5.
 |     +-- Listing over `MAX_DIR_ENTRIES` entries (hidden and ignored included) -> Run fails (InvalidMetadata).
 |     +-- Hidden or ignored by a `.gitignore` -> Pruned, not entered; its `.gitignore` is never read.
 |     `-- Matched by a user exclusion (§ 3.6) -> Pruned, not entered; its `.gitignore` is never read.
 |
 +-- Tar header read (`Entries::next`, GNU `L`/`K` and PAX `x` bodies included). Runs before path classification and applies to every entry type.
 |     `-- Over `ARCHIVE_MAX_HEADER_BYTES` for one read -> Run fails (InvalidMetadata), even for entries that would be dropped (I-03).
 +-- Decompressed archive stream over `ARCHIVE_MAX_DECOMPRESSED_BYTES` -> Run fails (InvalidMetadata), entry bodies included.
 |
 +-- Non-regular file (symlink, FIFO, socket, device, non-file tar header)
 |     `-- Dropped silently. Omitted from count.
 |
 +-- Tar stream entry normalization
 |     +-- Embedded NUL byte, traversal `..`, or `.` root segment
 |     |     `-- Run fails (InvalidMetadata).
 |     `-- Strips the first path segment (repository root directory).
 |
 +-- Path classification
 |     +-- Hidden segment
 |     |     `-- Ignored. Dropped silently. Omitted from count.
 |     +-- Matched by a user exclusion (§ 3.6)
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
- Source root is exempt (`codefeed ./.project` succeeds). Redundant separators (`a//b`) are stripped prior to evaluation.
- Local walks prune hidden directories; archives filter entries individually.

### 3.2 Noise Lockfiles

Exact matches against final path segment:

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
- `.env` files are excluded by hidden entry rules (§ 3.1).
- Matches evaluate on ASCII bytes regardless of path UTF-8 validity (e.g. `id_rsa\xff.key` is ignored, not counted as skipped).

### 3.4 Path Limits

| Rule | Effect |
| :--- | :--- |
| Depth > `MAX_PATH_DEPTH` | Run fails (`InvalidMetadata`). Hidden/noise items dropped prior to check. |
| Path > `MAX_PATH_BYTES` | Consumes candidate slot; counted as skipped; body unread. Truncated prefix keys budget (I-05). |
| Path equal to another candidate's path | Run fails (`InvalidMetadata`). Applies to skipped candidates too. |
| Stored path is directory prefix of another (`a` and `a/…`) | Run fails (`InvalidMetadata`) at digest assembly. Skipped candidates excluded. |

### 3.5 Ignore Files (Local Only)

- Applies in-tree `.gitignore` files (Git binary not required). Global gitignore, `.git/info/exclude`, and parent files are ignored.
- Local walk reads a directory's `.gitignore` once on entry. The bytes validating the file compile its rules.
- A `.gitignore` that is a symlink, non-regular/non-directory (e.g. FIFO), exceeds `MAX_FILE_SIZE`, is unreadable, or not valid UTF-8 fails the run (`IngestError::Io`). A `.gitignore` directory contributes no rules; invalid globs are skipped.

### 3.6 User Exclusions (`--exclude`)

- `.gitignore` syntax relative to source root, case-sensitive. Each pattern yields one rule. Unescaped trailing whitespace trimmed before parsing. Blank lines, comments, bare `!`, unclosed `[`, or invalid globs fail at parse time (`DomainError::Usage`).
- `\` in path segments matches U+FFFD on all platforms (never a separator; identical on Windows/Unix).
- Applies identically to local and GitHub sources, independently of `.gitignore`. `!` re-includes only paths excluded by other user patterns; never re-includes files under an excluded ancestor directory.
- Excluded local directories are pruned without reading their `.gitignore`. Excluded entries are omitted from skipped tallies.
- Matching tests the first `MAX_PATH_DEPTH + 1` segments, capping work per path. Matches beyond this window fail the depth limit instead.

---

## 4. Candidate and Skipped Accounting

| Outcome | Candidate Slot | Counted Skipped | Body Read |
| :--- | :---: | :---: | :---: |
| Ignored (§ 3) | No | No | No |
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

- **No Symlinks:** Walk classifies entries without following symlinks and drops non-directory, non-regular files. Link metadata is inspected before open. Symlinked root is followed once; entries beneath follow no-symlink rules.
- **Output Exclusion:** Canonical destination resolved upfront (`resolve_output_target`) and excluded from traversal.
- **Atomic Writes:** Destination written via `.codefeed.{nanos}.{counter}.tmp`, flushed, and renamed. Unix preserves permissions and syncs parent directory (sync failure logs warning without aborting). Temporary file cleaned up on error.
- **Unavailable vs. Fatal:** `NotFound`, `PermissionDenied`, or Windows sharing (32) / locking (33) mark files unavailable (skipped). Other I/O errors or unreadable directories abort. Deterministic first error in walk order reported.
- **Bounded Concurrency:** Producer/workers communicate via bounded `sync_channel` queues (`LOCAL_CHANNEL_CAPACITY`). Memory holds at most `LOCAL_CHANNEL_CAPACITY + 2` concurrent file bodies, each bounded to `MAX_FILE_SIZE + 1`. Abort signals halt worker reads.

---

## 6. GitHub Network Safety

### 6.1 Authentication

- Resolves `TOKEN_ENV_VARS` in order (`GITHUB_TOKEN`, then `GH_TOKEN`). Empty or whitespace-only values ignored.
- Sends `Authorization: Bearer <token>` strictly to `https://api.github.com` (`is_github_api_origin`) (I-07).
- 403 and 429 error messages name token variables solely for anonymous requests (I-06). Authenticated requests receive no token hint. Redirect targets never receive credentials (`RedirectAuthHeaders::Never`).

### 6.2 Memory

Tokens, environment buffers, and error responses use `zeroize::Zeroizing` to erase memory on drop. Copies in HTTP client internals, OS process environment, or error formatters are not zeroized.

### 6.3 Transport and TLS

Enforces built-in Mozilla root certificates (`rustls-webpki-roots`). OS stores and custom proxy CAs are rejected.

### 6.4 Streaming

- Fetches tarball via `/repos/{owner}/{repo}/tarball/{ref}`. Streams via gzip/tar directly into `Selector::observe` without disk spooling.
- Each tar header read is metered against `ARCHIVE_MAX_HEADER_BYTES` (I-03), covering padding, GNU `L`/`K`, PAX `x`, and sparse blocks. Overruns abort with `InvalidMetadata`. Entire decompressed stream is metered against `ARCHIVE_MAX_DECOMPRESSED_BYTES`.

### 6.5 Retries

- **Retried:** Transport timeouts/errors, HTTP 429, HTTP 5xx, HTTP 403 with `Retry-After` or zero quota.
- **Not Retried:** Other HTTP 4xx, non-transient transport errors.
- Attempts capped by `MAX_ATTEMPTS`. Exponential backoff spans `RETRY_INITIAL_DELAY_MS` to `RETRY_MAX_DELAY_MS`. Server wait delays from `Retry-After` or `x-ratelimit-reset` exceeding `RETRY_MAX_DELAY_MS` abort immediately.

### 6.6 Time Limits

- DNS resolution, connection, and TLS handshake: within `GITHUB_TIMEOUT_SECS`.
- Response headers: within `GITHUB_TIMEOUT_SECS`.
- Archive download: within `ARCHIVE_MAX_SECS`.

---

## 7. Output Sanitization

Sanitization runs in a single pass:

1. **Text (`--format text`):**
   - Header labels and paths: escapes `\`, `\r`, `\n`. Controls (`0x00`-`0x08`, `0x0B`, `0x0C`, `0x0E`-`0x1F`, `0x7F`) map to `\u{FFFD}`.
   - File bodies: verbatim UTF-8; ANSI sequences preserved.
2. **XML (`--format xml`):**
   - Entities escaped (`&`, `<`, `>`, `"`, `'`). Attribute whitespace (`\t`, `\n`, `\r`) encoded as character references (`&#9;`, `&#10;`, `&#13;`). Disallowed XML 1.0 controls map to `\u{FFFD}`.
3. **Tree Labels:** Escaped for control characters and XML entities.
4. **Error and Warning Diagnostics (`ErrorDetail`, `sanitize_error_detail`, I-04):**
   - Single-line diagnostics emitted to stderr (paths in I/O errors, root validation errors, HTTP status messages, transport error text, domain messages, overflow lists) convert `\t`, `\r`, `\n` to space `' '` and other control characters to `\u{FFFD}` to prevent terminal injection or log forgery. Destination in stdout completion summary (`Wrote <PATH>`) is sanitized identically.

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
