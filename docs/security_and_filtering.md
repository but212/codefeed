# Security and Filtering Specification

**Authority:** `src/core/policy/` is the single source of truth for all lists and limits. § 3.2–3.3 and § 8 mirror it for automated parity tests; updates to `src/core/policy/` must synchronize them in the same commit. Invariants (`I-xx`) and overarching laws (`L1`–`L5`) live in [`context.md`](../context.md) § 3.

**Governing Laws (extending `L1`–`L5`):**

- **S1 (Untrusted Data & Early Exclusion):** Local and archive entries are untrusted data. Non-regular entries and § 3 exclusions are dropped before candidate slot charging or body reading.
- **S2 (Pre-Charge Bounds & Error Partition):** Stream, directory, structural, uniqueness, and prefix bounds are checked before reading or allocating and abort the run (`InvalidMetadata`, except `MAX_FILE_COUNT` -> `CandidateLimitExceeded`); per-entry path, size, encoding, and availability failures consume a candidate slot and skip.
- **S3 (Candidate Accounting Identity):** Every non-ignored entry consumes one candidate slot; `Included` iff all pre-read checks and bounded UTF-8 body read succeed (`Skipped = Candidates - Included`).
- **S4 (Contextual Single-Pass Sanitization):** Untrusted paths, metadata, and diagnostics escape in a single pass per output context (`I-04`); only Text file bodies emit verbatim UTF-8.

---

## 1. Threat Model

| # | Threat | Mitigation |
| :--- | :--- | :--- |
| T1 | Secret disclosure in digest | Path/name exclusion (§ 3). Content is not scanned (§ 1.2). |
| T2 | Resource exhaustion | Size, count, depth, and path limits; bounded reads and channels; timeouts (§ 5, § 6). |
| T3 | Credential forwarding across origins | Origin validation; redirects strip authorization headers (§ 6). |
| T4 | Injection into terminals, XML, or LLM boundaries | Contextual output escaping (§ 7). XML isolates file bodies; text isolates headers/paths while emitting bodies verbatim. |
| T5 | Traversal outside source root | Symlinks ignored; metadata checked prior to open (§ 5). |

### 1.1 Assumptions

- **Quiescent tree:** Local source files remain unchanged during scanning. Concurrent modifications or symlink swaps are outside the threat model.
- **Data-only GitHub archives:** Per **S1**, tarballs are processed strictly as untrusted data under local path-classification rules.

### 1.2 Non-Goals

- **No secret/entropy scanning:** Filtering matches paths and names only. Hard-coded secrets in source files are not redacted.
- **In-memory token lifetime:** Zeroization is limited to § 6.2 scope.
- **Text format delimiter and ANSI isolation:** `--format text` emits file bodies as verbatim UTF-8 without delimiter rewriting or ANSI stripping. Strict isolation requires `--format xml`.

---

## 2. Evaluation Order

By **S1** and **S2**, both pipelines follow `[Physical/Stream Bounds] -> [Early Exclusion] -> [Structural/Depth Check] -> [Candidate Observation] -> [Prefix-Free Assembly (I-01)]`, with the following source-specific precedence:

1. **Local walk (directory admission):**
   - Pruning (hidden, `.gitignore`, or `--exclude` per § 3) runs *before* depth checks; pruned directories are never entered and their `.gitignore` is never read.
   - Depth `> MAX_PATH_DEPTH` fails (`InvalidMetadata`) *before* reading `.gitignore` (once; § 3.5) or listing entries (`> MAX_DIR_ENTRIES`, hidden/ignored included, fails with `InvalidMetadata`).
2. **Tar stream (entry framing & normalization, *before* § 3 exclusions):**
   - Header reads (`ARCHIVE_MAX_HEADER_BYTES`, including GNU `L`/`K` and PAX `x` per `I-03`) and decompressed stream bytes (`ARCHIVE_MAX_DECOMPRESSED_BYTES`, including reconstructed sparse holes per `I-12`) fail with `InvalidMetadata` even for entries that would be dropped.
   - Non-regular entries (symlinks, FIFOs, sockets, devices, non-file tar headers) drop silently before path normalization.
   - Tar paths with embedded NUL, `..`, or `.` root segments fail (`InvalidMetadata`); otherwise the first segment (repository root) is stripped.
3. **Path classification:**
   - Each directory segment runs hidden and `--exclude` (§ 3.6) checks before enforcing `MAX_PATH_DEPTH` (`InvalidMetadata`), matching local walk admission.
   - Leaf segments run hidden, `--exclude`, noise lockfile, and sensitive patterns (§ 3.2–3.3), returning `Ignored` (omitted from counts).
   - Non-UTF-8 segments or paths `> MAX_PATH_BYTES` become `Unstorable` candidates retaining lossless normalized raw-byte identity (`L2`, `I-05`, `I-10`, `I-11`).
4. **Candidate observation & assembly:**
   - Charges candidate slot (`MAX_FILE_COUNT` overflow aborts); duplicate normalized raw-byte paths (including skipped) fail (`InvalidMetadata`, `I-09`).
   - Declared size `> MAX_FILE_SIZE` skips without reading; otherwise bounded read settles inclusion (§ 4).
   - Digest assembly rejects any stored path that is a directory prefix of another (`InvalidMetadata`, `I-01`; skipped candidates excluded).

---

## 3. Exclusion Lists

### 3.1 Hidden Entries

By `I-02`, after stripping redundant separators (`a//b`), any non-root path segment starting with `.` (`.git`, `.github`, `.vscode`, `.env*`) is hidden; source root is exempt (`codefeed ./.project` succeeds). Local walks prune hidden directories; archives filter per entry.

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
- `.env` files are excluded by § 3.1. Matches evaluate on ASCII bytes regardless of path UTF-8 validity (e.g. `id_rsa\xff.key` is ignored, not counted as skipped).

### 3.4 Path Limits

Governed by **S2**, § 2, § 4, § 8, and [`context.md`](../context.md) (`L2`, `I-01`, `I-05`, `I-09`, `I-10`): `MAX_PATH_DEPTH`, duplicate normalized raw-byte paths, and stored directory-prefix conflicts fail with `InvalidMetadata`; paths `> MAX_PATH_BYTES` consume a candidate slot and skip without reading.

### 3.5 Ignore Files (Local Only)

- Applies in-tree `.gitignore` files only (no Git binary, global gitignore, `.git/info/exclude`, or parent files); each directory's `.gitignore` is read once on entry to validate and compile rules.
- **Strict control-file failure:** Unlike skipped source files, a `.gitignore` that is a symlink, non-regular/non-directory (e.g. FIFO), `> MAX_FILE_SIZE`, unreadable, or invalid UTF-8 fails the run (`IngestError::Io`). A `.gitignore` directory contributes no rules.
- A rule using a POSIX named character class (e.g. `[[:digit:]]`) fails (`IngestError::Io`), even if otherwise malformed; other invalid globs are skipped. Use ranges such as `[0-9]` (`[` as a range endpoint does not start a named class).

### 3.6 User Exclusions (`--exclude`)

- `.gitignore` syntax relative to source root, case-sensitive; each pattern yields one rule after trimming unescaped trailing whitespace. Blank lines, comments, bare `!`, unclosed `[`, invalid globs, or POSIX named character classes fail at parse time (`DomainError::Usage`).
- `\` in path segments matches `U+FFFD` on all platforms (never a separator).
- Applies identically to local and GitHub sources, independently of `.gitignore`. `!` re-includes only paths excluded by other user patterns and never under an excluded ancestor directory.

---

## 4. Candidate and Skipped Accounting

By **S1**, **S2**, and **S3**:

- **Ignored (§ 3 or non-regular):** `Slot = No`, `Counted Skipped = No`, `Body Read = No`.
- **Candidates (all other regular/tar-file entries):** `Slot = Yes`; overflowing `MAX_FILE_COUNT` aborts without partial output.
- **Lazy Bounded Read:** Body read (bounded to `MAX_FILE_SIZE`) is attempted iff path is valid UTF-8 `<= MAX_PATH_BYTES` and declared size `<= MAX_FILE_SIZE`.
- **Inclusion:** `Included` (`Counted Skipped = No`) iff the bounded body read succeeds with non-binary (no NUL) valid UTF-8 `<= MAX_FILE_SIZE`. All other candidates (non-UTF-8 or oversized path, oversized declared or read size, unavailable at metadata/open/read, binary or invalid UTF-8) are `Counted Skipped = Yes` (`Skipped = Candidates - Included`).

---

## 5. Local Filesystem Safety

- **No Symlinks (`L1`, T5):** Walk inspects link metadata before open, never follows symlinks beneath root, and drops non-directory, non-regular entries. Exception: a symlinked source root is followed once.
- **Output Exclusion:** Directory-suffixed output paths (`out/`, `out/.`) are rejected before filesystem resolution. Canonical destination is resolved upfront and excluded from traversal.
- **Atomic Writes (`L4`):** Writes to `.codefeed.{nanos}.{counter}.tmp`, flushes, and renames, cleaning up the temporary file on error. Unix preserves permissions and syncs the parent directory (sync failure logs a warning without aborting).
- **Unavailable vs. Fatal:** `NotFound`, `PermissionDenied`, or Windows sharing (32) / locking (33) mark files unavailable (skipped). Other I/O errors or unreadable directories abort, reporting the deterministic first error in walk order.
- **Bounded Concurrency (`L3`, `L4`):** Bounded queues (`LOCAL_CHANNEL_CAPACITY`) and bounded reads (`MAX_FILE_SIZE + 1`) cap concurrent in-memory bodies at `LOCAL_CHANNEL_CAPACITY + 2`. Abort signals stop the walk between entries and prevent workers from starting further reads.

---

## 6. GitHub Network Safety

### 6.1 Authentication

Governed by `I-06`, `I-07`, and `I-08` (`TOKEN_ENV_VARS` lookup order, strict `https://api.github.com` origin with redirect credential stripping, and anonymous-only token hints on HTTP 403/429). Empty or whitespace-only token values are ignored.

### 6.2 Memory

Tokens, environment buffers, and error responses are zeroized on drop. Copies in HTTP client internals, OS process environment, or error formatters are not zeroized.

### 6.3 Transport and TLS

Enforces built-in Mozilla root certificates (`rustls-webpki-roots`). OS stores and custom proxy CAs are rejected.

### 6.4 Streaming

- Streams `/repos/{owner}/{repo}/tarball/{ref}` through gzip/tar into candidate observation without disk spooling (`L3`, `L4`).
- Concatenated gzip members form one tar stream. After tar termination, remaining members are drained and every trailer is validated before delivery (`IngestError::Http` on missing/invalid checksum or size); post-tar decoded bytes charge `ARCHIVE_MAX_DECOMPRESSED_BYTES`, not `ARCHIVE_MAX_HEADER_BYTES`.
- Per `I-03` and `I-12`, tar header reads (including padding, `L`/`K`, `x`, and sparse blocks) meter against `ARCHIVE_MAX_HEADER_BYTES`, and sparse entries exceeding remaining `ARCHIVE_MAX_DECOMPRESSED_BYTES` fail (`InvalidMetadata`) before expansion (entries within budget remain excluded as non-regular files).

### 6.5 Retries

- **Retried:** Transport timeouts/errors, HTTP 429, HTTP 5xx, HTTP 403 with `Retry-After` or zero quota.
- **Not Retried:** Other HTTP 4xx, non-transient transport errors.
- Valid `Retry-After` (decimal seconds or HTTP-date, including legacy forms) overrides quota-reset headers. Dates and `x-ratelimit-reset` resolve against current Unix time (due/past values wait 0); overflowing all-digit seconds saturate instead of falling back to backoff; malformed values fall back to quota-reset or backoff.
- Attempts cap at `MAX_ATTEMPTS`; exponential backoff spans `RETRY_INITIAL_DELAY_MS` to `RETRY_MAX_DELAY_MS`. Server waits (`Retry-After` or `x-ratelimit-reset`) exceeding `RETRY_MAX_DELAY_MS` abort immediately.

### 6.6 Time Limits

- DNS resolution, connection, TLS handshake, and response headers: within `GITHUB_TIMEOUT_SECS`.
- Archive download: within `ARCHIVE_MAX_SECS`.

---

## 7. Output Sanitization

By **S4** (single pass per context):

1. **Text (`--format text`):**
   - Header labels and paths: escapes `\`, `\r`, `\n`. Controls (`0x00`–`0x08`, `0x0B`, `0x0C`, `0x0E`–`0x1F`, `0x7F`) map to `\u{FFFD}`.
   - File bodies: verbatim UTF-8; ANSI sequences preserved.
2. **XML (`--format xml`):**
   - Entities escaped (`&`, `<`, `>`, `"`, `'`). Attribute whitespace (`\t`, `\n`, `\r`) encoded as `&#9;`, `&#10;`, `&#13;`. Disallowed XML 1.0 characters (C0 controls and `U+FFFE`/`U+FFFF`) map to `\u{FFFD}`.
3. **Tree Labels:** Follow the active format's single-line escaping (text header rules or XML rules).
4. **Error and Warning Diagnostics (`I-04`):** Single-line stderr diagnostics and stdout `Wrote <PATH>` convert `\t`, `\r`, `\n` to `' '` and other controls to `\u{FFFD}`.

---

## 8. Limit Values

Sole documentation mirror for numerical limits (authoritative definitions reside in `src/core/policy/limits.rs`). Outcomes at each limit are in [`README.md`](../README.md#limits).

| Constant | Value | Enforced By |
| :--- | :--- | :--- |
| `MAX_FILE_SIZE` | 1 MiB | Declared size, bounded read, and `.gitignore` read (a `.gitignore` over the limit fails the run) |
| `MAX_PATH_BYTES` | 4,096 B | Path classification |
| `MAX_PATH_DEPTH` | 256 levels | Local directory admission and tar file classification |
| `MAX_FILE_COUNT` | 10,000 files | Candidate budget |
| `MAX_OUTPUT_SIZE` | 20 MiB | Storage check & buffer |
| `ARCHIVE_BUFFER_BYTES` | 64 KiB | Archive read buffer (buffering parameter) |
| `ARCHIVE_MAX_HEADER_BYTES` | 64 KiB | Archive header read budget |
| `ARCHIVE_MAX_DECOMPRESSED_BYTES` | 256 MiB | Decompressed stream and reconstructed sparse holes |
| `MAX_DIR_ENTRIES` | 20,000 entries | Local directory listing |
| `ARCHIVE_PAX_GLOBAL_BYTES` | 1 KiB | PAX global header read (revision falls back to the requested ref) |
| `GITHUB_ERROR_BODY_BYTES` | 4 KiB | GitHub API error body read (rest of body left out of the message) |
| `LOCAL_CHANNEL_CAPACITY` | 64 messages | Channel bounds (blocks sender) |
| `MAX_LOCAL_WORKERS` | 12 threads | Thread pool sizing |
| `LOCAL_BUFFER_RETAIN_BYTES` | 128 KiB | Local read buffer pool retention cap |
| `ARCHIVE_MAX_SECS` | 300 s | Archive stream timer |
| `GITHUB_TIMEOUT_SECS` | 30 s | Resolve, connect, header timeouts |
| `MAX_ATTEMPTS` | 3 attempts | Retry loop |
| `RETRY_INITIAL_DELAY_MS` | 250 ms | Backoff initial delay |
| `RETRY_MAX_DELAY_MS` | 5,000 ms | Backoff cap & wait limit |
| `ASCII_BYTES_PER_TOKEN` | 4 B | Token heuristic |
| `OVERFLOW_LARGEST_FILES` | 5 files | Overflow diagnostic (files listed) |
| `OVERFLOW_LARGEST_DIRECTORIES` | 3 directories | Overflow diagnostic (directories listed) |
