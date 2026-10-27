# Security and Filtering Specification

**Audience:** security reviewers, and anyone changing filters, limits, filesystem code, or network code.

**Authority:** `src/core/policy.rs` is the single source of truth for every list and limit value. The values and lists in this document mirror it. A change to `policy.rs` must update this document in the same commit.

---

## 1. Threat Model

| # | Threat | Mitigation | Where |
| :--- | :--- | :--- | :--- |
| T1 | Secret disclosure into prompts or output | Hidden-entry, noise, and sensitive-name exclusion (sec. 3). Content is not scanned (non-goal, sec. 1.2). | `policy.rs`, `selector.rs` |
| T2 | Resource exhaustion | Size, count, depth, and path-length limits; output cap; bounded reads and channels; archive timeout (sec. 5, sec. 6). | `policy.rs`, `local/`, `github/` |
| T3 | Credential forwarding to other origins | Exact-origin gate; redirects never carry authorization (sec. 6). | `core/github/url.rs`, `shell/github/` |
| T4 | Output injection into terminals, XML, or LLM context boundaries | Format-specific escaping (sec. 7). | `core/render/escape.rs` |
| T5 | Reading files outside the source root | Symlinks are never followed; metadata is checked before open (sec. 5). | `shell/local/` |

### 1.1 Assumptions

- **Quiescent local tree.** The local source is assumed not to change during the scan. A concurrent process that swaps paths or replaces regular files with symlinks is outside the threat model.
- **Trusted GitHub content.** Archive content is trusted only as data. Archive paths are normalized and classified exactly like local paths.

### 1.2 Non-goals

- **No content scanning.** Filters match path conventions, not file contents. They are not an entropy or secret scanner. Do not rely on codefeed to redact credentials hard-coded in tracked source files.
- **In-memory copies of tokens.** Copies held by the HTTP client, the process environment, and rendered error messages are not zeroized (sec. 6.2).

---

## 2. Evaluation Order

```
Entry (local walk or tar stream)
 |
 +-- Not a regular file: local symlink, FIFO, socket, or device; archive entry of any other tar type
 |     `-- Dropped. Not counted.
 |
 +-- Path classification (policy::classify_path_segments)
 |     +-- Hidden segment, noise lockfile, or sensitive name
 |     |     `-- Ignored. Dropped. Not counted.
 |     +-- Depth above MAX_PATH_DEPTH
 |     |     `-- Run fails (InvalidMetadata).
 |     +-- Non-UTF-8 segment
 |     |     `-- Candidate with prefix: valid segments before the first invalid one ("a/b/"), or "" if the first is invalid.
 |     `-- Valid path
 |           `-- Candidate.
 |
 `-- Candidate (Selector::observe)
       +-- Consumes one candidate slot. Exceeding MAX_FILE_COUNT fails the run.
       +-- Path or declared size over its limit: skipped, body not read.
       +-- Body read (local: bounded by a worker; GitHub: bounded by the stream)
       `-- Included only if it passes the content checks. Otherwise skipped.
             Outcomes per case: sec. 4.
```

---

## 3. Exclusion Lists

### 3.1 Hidden Entries

- Any path segment that starts with `.` is hidden, including `.git`, `.github`, `.vscode`, `.env`, and `.env.*`.
- The local source root itself is exempt, so `codefeed ./.my-project` works.
- Empty segments produced by repeated separators (`a//b`) are dropped before classification, so they cannot cause a false hidden match.
- Local walks prune hidden directories during traversal. Archive entries are classified one at a time.

### 3.2 Noise Lockfiles

Lockfiles contain generated metadata that adds tokens without source value. They match the final path segment exactly.

| Lockfile | Ecosystem |
| :--- | :--- |
| `Cargo.lock` | Rust / Cargo |
| `package-lock.json` | Node.js / npm |
| `yarn.lock` | Node.js / Yarn |
| `pnpm-lock.yaml` | Node.js / pnpm |
| `bun.lock` | Node.js / Bun (text) |
| `bun.lockb` | Node.js / Bun (binary) |
| `go.sum` | Go modules |
| `poetry.lock` | Python / Poetry |
| `Pipfile.lock` | Python / Pipenv |
| `composer.lock` | PHP / Composer |
| `Gemfile.lock` | Ruby / Bundler |

### 3.3 Sensitive Names (case-insensitive)

- **Filename prefixes** (`SENSITIVE_PREFIXES`): `id_rsa`, `id_ed25519`, `id_ecdsa`, `id_dsa`, `credentials`. Each prefix matches the start of the final segment, so `id_rsa`, `id_rsa.pub`, and `credentials.json` are all excluded.
- **Extensions** (`SENSITIVE_EXTENSIONS`): `pem`, `key`, `p12`, `pkcs12`, `pfx`, `credentials`. The extension is the text after the last dot, so `server.key` and `aws.credentials` are excluded.

Environment files such as `.env` are excluded by the hidden-entry rule (sec. 3.1), not by this list.

The name rules apply to the final segment even when it is not valid UTF-8. Prefixes and extensions are ASCII, so a name such as `id_rsa\xff.key` is ignored, not counted as skipped (sec. 4).

### 3.4 Path Limits

| Rule | Effect |
| :--- | :--- |
| Depth above `MAX_PATH_DEPTH` | Run fails with `InvalidMetadata`. Hidden and noise entries are dropped before this check, so they never fail the run. |
| Path longer than `MAX_PATH_BYTES` | Candidate slot consumed. Counted as skipped. Body not read. |

### 3.5 Ignore Files (local sources only)

- `.gitignore` files inside the source tree are applied, without requiring a Git repository.
- `.ignore` files are not read, to match standard Git behavior.
- The global gitignore, `.git/info/exclude`, and `.gitignore` files above the source root are not read.

---

## 4. Candidate and Skipped Accounting

| Outcome | Candidate slot | Counted as skipped | Body read |
| :--- | :---: | :---: | :---: |
| Ignored by sec. 3 | No | No | No |
| Non-UTF-8 path | Yes | Yes | No |
| Path longer than `MAX_PATH_BYTES` | Yes | Yes | No |
| Declared size above `MAX_FILE_SIZE` | Yes | Yes | No |
| Unavailable at metadata, open, or read (including unlinked after discovery) | Yes | Yes | Attempted, unless metadata failed |
| Binary (NUL) or invalid UTF-8 | Yes | Yes | Yes |
| Read exceeds `MAX_FILE_SIZE` | Yes | Yes | Bounded |
| Included | Yes | No | Yes |

When the candidate budget overflows, the run fails and no partial digest is written. The check happens before the body read for GitHub entries. For local entries, the worker has already read a bounded body.

---

## 5. Local Filesystem Safety

Implemented in `src/shell/local/` and `src/shell/output.rs`.

1. **No symlink following.** The walk uses `follow_links(false)`. Symlinks are dropped before path classification. Before opening any file, the worker calls `fs::symlink_metadata` and requires a regular file that is not a symlink. Symlinks to files, directories, or special files are never read.
2. **Output exclusion.** The output path is resolved once at startup (`resolve_output_target`) and excluded from the walk by canonical path. This prevents the digest from ingesting itself.
3. **Atomic writes.** The digest is written to a temporary file in the destination's parent directory. The file is synced, then renamed to the destination. On Unix, the existing file's permissions are kept and the parent directory is synced after the rename. A failed sync is reported as a warning on stderr, because the rename has already replaced the target. The temporary handle is closed before the file is unlinked on `Drop`, which keeps cleanup reliable on Windows and Unix, including during error and panic unwinding.
4. **Unavailable versus fatal.** At the metadata, open, and read steps, a per-file `NotFound`, `PermissionDenied`, or Windows sharing (32) or lock (33) violation is recorded as unavailable and counted as skipped. Any other error at those steps fails the run with `IngestError::Io`. An unreadable directory fails the run and names the directory. When several entries fail, the first one in walk order is reported, so the same input gives the same error on every run.
5. **Bounded concurrency.** The producer, the workers, and the main thread communicate through two `sync_channel`s of `LOCAL_CHANNEL_CAPACITY` messages (sec. 8): jobs from the producer to the workers, and slots from the producer to the main thread. A full channel blocks its sender, so no queue grows without bound during a large scan. At most `LOCAL_CHANNEL_CAPACITY + 2` file bodies are held at once: those in the slot channel, the one the producer holds while it waits to send, and the one the main thread is settling. The bound does not depend on the worker count. Each body is at most `MAX_FILE_SIZE + 1` bytes (sec. 4). This bounds the bytes read and then discarded when a budget overflow or error stops the run. An error or budget overflow sets an `AtomicBool`, so workers skip their remaining reads, and the main thread drops its receiver, which stops the producer.

---

## 6. GitHub Network Safety

Implemented in `src/shell/github/`, with pure rules in `src/core/github/`.

### 6.1 Origin-Gated Authentication

- Token resolution checks `GITHUB_TOKEN` first, then `GH_TOKEN`. Each value is trimmed. An empty or whitespace-only value is ignored, and resolution continues with the next variable.
- `Authorization: Bearer <token>` is attached only when `is_github_api_origin(url)` is true, which means the URL is exactly `https://api.github.com` or starts with it followed by `/`.
- The HTTP client is configured with `RedirectAuthHeaders::Never`. A redirect never carries authorization, including redirects to CDNs or mirrors.

### 6.2 Memory

- The resolved token, the environment lookup buffer, and the raw error-body buffer are held in `zeroize::Zeroizing` and erased on drop.
- Copies held by the HTTP client's request headers, the process environment, and rendered error messages are not zeroized (sec. 1.2).

### 6.3 Transport and TLS

- The client uses the Mozilla root certificates compiled into the binary (`rustls-webpki-roots`). It does not use the operating system's trust store. A certificate trusted only by the OS store, such as a corporate proxy CA, is rejected.

### 6.4 Streaming

- Each repository is fetched in a single request to `/repos/{owner}/{repo}/tarball/{ref}`.
- Entries are streamed through gzip and tar decoders into `Selector::observe`. Nothing is written to disk. Each entry is admitted before its body is read.

### 6.5 Retries

- Retried: transport errors that are I/O, timeout, or connection failures; HTTP 429; HTTP 5xx; and HTTP 403 when it is rate-limited (`Retry-After` present, or the quota header reports zero remaining).
- Not retried: other 4xx responses and non-transient transport errors.
- The total number of attempts is capped by `MAX_ATTEMPTS`. Backoff starts at `RETRY_INITIAL_DELAY_MS`, doubles on each retry, and is capped at `RETRY_MAX_DELAY_MS`.
- When the server gives a wait time (`Retry-After`, or the quota reset time once the quota is exhausted), that wait is used. If it is longer than `RETRY_MAX_DELAY_MS`, the run fails instead of waiting. Only the delta-seconds form of `Retry-After` is honored. Other forms are treated as rate-limited and fall back to backoff.

### 6.6 Time Limits

- Response headers must arrive within `GITHUB_TIMEOUT_SECS`.
- The whole archive transfer must finish within `ARCHIVE_MAX_SECS`.

---

## 7. Output Sanitization

Escaping is applied in one pass by `write_escaped`, with rules chosen per context (`escape_header`, `escape_xml_body`, `escape_xml_single_line`).

1. **Text** (`--format text`)
   - Header labels and file paths: backslash, CR, and LF are escaped. Other control characters in the list below become `\u{FFFD}`.
   - File bodies: valid UTF-8 with no NUL byte is emitted verbatim, so code is exact for the LLM. ANSI sequences are preserved.
   - Control characters for header and path escaping: `0x00`-`0x08`, `0x0B`, `0x0C`, `0x0E`-`0x1F`, and `0x7F`.
2. **XML** (`--format xml`)
   - Entities `&`, `<`, `>`, `"`, and `'` are escaped.
   - Tabs, LF, and CR in attribute values are written as character references (`&#9;`, `&#10;`, `&#13;`).
   - Tabs, LF, and CR in element bodies are kept.
   - Disallowed XML 1.0 control characters (`U+0000`-`U+001F` except tab, LF, and CR) become `\u{FFFD}`.
3. **Tree labels**: Node labels in the tree are escaped for control codes and XML entities, so a file name cannot break a line or corrupt the terminal or XML structure.

---

## 8. Limit Values

This section is the only place in the documentation that states limit values. Other documents link here. `src/core/policy.rs` is authoritative: code reads each constant from there and never copies its value.

| Constant | Value | Enforced by | On overflow |
| :--- | :--- | :--- | :--- |
| `MAX_FILE_SIZE` | 1 MiB per file | Declared size and bounded read (sec. 4) | Skipped, counted as skipped |
| `MAX_PATH_BYTES` | 4,096 bytes | Path classification (sec. 3.4) | Skipped, counted as skipped |
| `MAX_PATH_DEPTH` | 256 levels | Path classification (sec. 3.4) | Run fails (`InvalidMetadata`) |
| `MAX_FILE_COUNT` | 10,000 candidate files | Candidate budget (sec. 4) | Run fails; lists up to three largest directories |
| `MAX_OUTPUT_SIZE` | 20 MiB | Storage early check and rendering buffer | Run fails; no output; lists five largest files |
| `ARCHIVE_BUFFER_BYTES` | 64 KiB | Archive read buffer | Not a limit; sets buffering only |
| `LOCAL_CHANNEL_CAPACITY` | 64 messages | Local job and slot channels (sec. 5.5) | Not a limit; a full channel blocks its sender |
| `MAX_LOCAL_WORKERS` | 12 threads | Local read workers (sec. 5.5) | Not a limit; fewer cores use fewer threads |
| `ARCHIVE_MAX_SECS` | 300 seconds | Whole archive transfer (sec. 6.6) | Download fails with a request error |
| `GITHUB_TIMEOUT_SECS` | 30 seconds | Response headers (sec. 6.6) | Treated as transient; retried (sec. 6.5) |
| `MAX_ATTEMPTS` | 3 attempts (1 initial, 2 retries) | Retry loop (sec. 6.5) | Transient failures stop retrying; the run fails |
| `RETRY_INITIAL_DELAY_MS` | 250 ms | Backoff (sec. 6.5) | Not an overflow; first retry delay |
| `RETRY_MAX_DELAY_MS` | 5,000 ms | Backoff cap and server wait limit (sec. 6.5) | Longer server wait fails the run |
| `ASCII_BYTES_PER_TOKEN` | 4 | Token estimate (context sec. 3.5) | Not a limit; estimate ratio only |
