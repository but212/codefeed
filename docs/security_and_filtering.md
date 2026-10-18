# Security and Filtering Specification

Comprehensive specification of security boundaries, pattern-based exclusion, candidate evaluation, and transport protection in `codefeed`.

---

## 1. Threat Model & Design Principles

`codefeed` aggregates source code repositories into a consolidated digest for consumption by Large Language Models (LLMs). This ingestion model introduces specific operational and security risks:

1. **Information Disclosure (Secret Leakage):** Accidental leakage of local or repository secrets (private keys, credential stores, `.env` files) into prompts or output files.
2. **Resource Exhaustion (Denial of Service):** Pathological directory trees, infinite symlink loops, oversized binary assets, or tar bombs exhausting memory or disk space.
3. **Credential Forwarding (SSRF / Confused Deputy):** Forwarding GitHub API authorization headers to untrusted third-party hosts upon HTTP redirection.
4. **Output Integrity / Stream Injection:** Control character injection corrupting terminal state, XML structures, or LLM context boundaries.

`codefeed` addresses these risks through strict boundary classification, zero-copy bounded memory arenas, and non-following I/O policies.

> [!NOTE]
> **Quiescent Local Tree Threat Model:** Local directory traversal assumes the target tree is quiescent during scanning (no concurrent writer or adversary swapping paths or replacing regular files with symlinks mid-scan). Bounding hostile concurrent filesystem mutations is explicitly outside the CLI tool's scope.

---

## 2. Path Classification & Filtering Hierarchy

Every encountered entry (local filesystem or remote archive) passes through path filtering and candidate evaluation in a single pass (`normalize_tar_entry_path` / `classify_local_path` using `is_hidden_segment` and `is_ignored_filename`):

```
Entry Discovered (Local Walk or Tar Stream)
   │
   ├── Ignored (Pre-candidate Filter: is_hidden_segment / is_ignored_filename)
   │     ├── Noise Lockfiles (Cargo.lock, package-lock.json, etc.)
   │     ├── Sensitive Patterns (.env*, id_rsa*, credentials*, *.key, *.pem, etc.)
   │     ├── Hidden Components (.git, .github, .config, etc.)
   │     └── Symlinks (Local filesystem only)
   │     └── [Result: Dropped immediately. Not counted in candidate budget or skipped tally.]
   │
   └── Candidate
         ├── Non-UTF-8 path: Preserves valid top-level directory prefix ("dir/") or "" ("<unknown>/");
         │                    reserves candidate slot and records skipped.
         └── Regular candidate file:
               └── [Result: Candidate slot consumed; body validated for UTF-8 encoding (no NUL bytes) and MAX_FILE_SIZE. Non-text or oversized files increment skipped.]
```

### 2.1 Conventional Noise Exclusions

Conventional package manager lockfiles contain redundant metadata that bloats prompt token count without semantic source value. They are filtered prior to candidate accounting:

| Lockfile Name | Ecosystem |
| :--- | :--- |
| `Cargo.lock` | Rust / Cargo |
| `package-lock.json` | Node.js / npm |
| `yarn.lock` | Node.js / Yarn |
| `pnpm-lock.yaml` | Node.js / pnpm |
| `go.sum` | Go modules |

### 2.2 Sensitive File Exclusions

`codefeed` enforces case-insensitive pattern matching on file basenames and extensions to prevent credential exposure.

> [!WARNING]
> Pattern-based filtering matches file path conventions only. It is not an active entropy- or content-based secret scanner. Users must not rely on `codefeed` to redact hardcoded credentials within tracked source code files.

- **Filename Prefixes (Case-Insensitive):**
  - `.env*` (e.g., `.env`, `.env.local`, `.env.production`)
  - `id_rsa*` (e.g., `id_rsa`, `id_rsa.pub`)
  - `id_ed25519*` (e.g., `id_ed25519`, `id_ed25519.pub`)
  - `credentials*` (e.g., `credentials.json`, `credentials.ini`)
- **Extensions in `SENSITIVE_EXTENSIONS` (Case-Insensitive):**
  - `*.pem`
  - `*.key`
  - `*.p12`
  - `*.pkcs12`
  - `*.credentials`

### 2.3 Hidden Files and Directories

Any path containing a component starting with a leading dot (`.`) is considered hidden and dropped:

- Empty path segments caused by duplicate delimiters (e.g., `a//b`) are stripped to prevent false-positive hidden classifications.
- Local repository `.git` folders and configuration directories (`.github`, `.vscode`) are excluded.
- Local `.gitignore` rules are respected through `ignore::WalkBuilder` (note: `.ignore` files are ignored to match standard Git behavior).

---

## 3. Candidate Accounting & Ingestion Budgets

`codefeed` enforces deterministic bounds on computation and memory allocations via `src/core/policy.rs`:

| Parameter | Limit | Enforcement Mechanism | Failure Mode |
| :--- | :--- | :--- | :--- |
| `MAX_FILE_COUNT` | 10,000 files | `CandidateBudget` slot check before reading content | `IngestError::CandidateLimitExceeded` with directory diagnostics |
| `MAX_FILE_SIZE` | 1 MiB (1,048,576 B) | Stream boundary read (`take(MAX_FILE_SIZE + 1)`) | Skipped; tallied in `skipped_files` report |
| `MAX_OUTPUT_SIZE` | 20 MiB (20,971,520 B) | `BoundedEstimatingWriter` byte counter | `IngestError::OutputTooLarge` with Top-5 largest files |
| `ARCHIVE_MAX_SECS` | 300 seconds | Whole-archive stream transfer timeout | `IngestError::Http` timeout |

### Candidate vs. Skipped Accounting Rules

- **Ignored paths:** Dropped during discovery. They consume **zero** candidate slots and do **not** appear in skipped counts.
- **Oversized / Non-UTF-8 / NUL-byte binary files:** Consume 1 candidate slot. Rejected during path/content inspection, incrementing `skipped_files`.

---

## 4. Local Filesystem Safety

Local directory traversal in `src/shell/local.rs` adheres to the following invariants:

1. **No Symlink Following:**
   - Traversal configures `WalkBuilder::new().follow_links(false)`.
   - File inspection verifies non-symlink metadata (`fs::symlink_metadata(path)?.file_type().is_file()`).
   - Symlinks to files, directories, or special devices are completely ignored, avoiding infinite symlink recursion and arbitrary file reads outside the root.
   - Assumes a quiescent directory tree during scan; TOCTOU symlink substitution by concurrent local processes is outside the threat model.
2. **Output Target Exclusion:**
   - The destination digest path is resolved and validated once at startup (`resolve_output_target`).
   - Files matching the canonical target path are excluded during traversal to prevent recursive ingestion loops when generating digests inside the source directory.
3. **Atomic Writes (`AtomicFile`):**
   - Digest files are written to a temporary sibling file in the resolved parent directory.
   - Flushed and synced to disk before being atomically renamed to the destination path.
   - Temporary files are immediately unlinked on `Drop` if an error occurs or the process panics.

---

## 5. Network & Remote Safety (GitHub Ingestion)

Remote ingestion via `src/shell/github.rs` operates under strict isolation:

1. **Origin-Gated Authentication:**
   - GitHub tokens (`GITHUB_TOKEN` or `GH_TOKEN`) are attached as `Authorization: Bearer <token>` **only** when the request destination exactly matches `https://api.github.com`.
   - Client redirect policy enforces `RedirectAuthHeaders::Never` so authentication tokens are never leaked to external CDNs or redirect mirrors.
2. **Diskless Streaming Pipeline:**
   - Remote repositories are streamed directly from `/repos/{owner}/{repo}/tarball/{ref}` as a single HTTP response.
   - Archive entries are unpacked via `flate2` and `tar` directly into memory arena buffers (`FileStorage`) without writing tarballs or unpacked files to disk.
3. **Bounded Retries:**
   - Bounded exponential backoff (`MAX_RETRIES = 3`, initial delay 250 ms, cap 5,000 ms).
   - Only retryable transport errors (I/O, timeout, connection drop) and HTTP 429 / 5xx status codes trigger retries. 4xx client errors fail immediately.

---

## 6. Output Sanitization & Formatting

1. **Plain Text (`--format text`):**
   - Header labels and metadata are normalized.
   - Valid UTF-8 file contents are emitted verbatim (only files with NUL bytes or invalid UTF-8 are rejected) to preserve exact code fidelity for LLM consumption.
2. **XML Format (`--format xml`):**
   - Standard XML entities (`&`, `<`, `>`, `"`, `'`) are escaped.
   - Disallowed XML 1.0 control characters (`\u{0000}` through `\u{001F}` excluding `\t`, `\n`, `\r`) are sanitized to `\u{FFFD}`.
3. **Directory Tree & Header Rendering:**
   - Header metadata labels and node labels in the ASCII hierarchy tree are escaped (`\`, `\r`, `\n`, C0 control characters `0x00..=0x08`, `0x0B`, `0x0C`, `0x0E..=0x1F`, and DEL `0x7F` to `\u{FFFD}`) to prevent line-break injections or terminal state corruption.
