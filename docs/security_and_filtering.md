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

---

## 2. Path Classification & Filtering Hierarchy

Every encountered entry (local filesystem or remote archive) passes through a multi-stage classification pipeline defined in `src/core/policy.rs` (`PathClassification`):

```
Entry Discovered (Local Walk or Tar Stream)
   │
   ├── Ignored (Pre-candidate Filter)
   │     ├── Noise Lockfiles (Cargo.lock, package-lock.json, etc.)
   │     ├── Sensitive Patterns (.env*, id_rsa*, credentials*, *.key, *.pem, etc.)
   │     ├── Hidden Components (.git, .github, .config, etc.)
   │     └── Symlinks (Local filesystem only)
   │     └── [Result: Dropped immediately. Not counted in candidate budget or skipped tally.]
   │
   ├── PreclassifiedBinary
   │     └── Matches BINARY_EXTENSIONS (.png, .jpg, .pdf, .zip, etc.)
   │     └── [Result: Candidate slot consumed; body read/fetch skipped (Probe::Missing); counted in skipped.]
   │
   └── NeedsContent
         └── Regular candidate file
         └── [Result: Candidate slot consumed; body validated for UTF-8 encoding and MAX_FILE_SIZE.]
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
| `MAX_COMPONENT_LEN` | 65,535 B (`u16::MAX`) | $O(1)$ length check and `find_overlong_component` fail-fast | `IngestError::PathComponentTooLong` |
| `MAX_OUTPUT_SIZE` | 20 MiB (20,971,520 B) | `BoundedEstimatingWriter` byte counter | `IngestError::OutputTooLarge` with Top-5 largest files |
| `ARCHIVE_MAX_SECS` | 300 seconds | Whole-archive stream transfer timeout | `IngestError::Network` timeout |

### Candidate vs. Skipped Accounting Rules

- **Ignored paths:** Dropped during discovery. They consume **zero** candidate slots and do **not** appear in skipped counts.
- **Binary extensions (`BINARY_EXTENSIONS`):** Consume 1 candidate slot in `CandidateBudget`. Content download/read is skipped via `Probe::Missing`, incrementing `skipped_files`.
- **Oversized / Non-UTF-8 / NUL-byte files:** Consume 1 candidate slot. Rejected during probe inspection, incrementing `skipped_files`.

---

## 4. Local Filesystem Safety

Local directory traversal in `src/shell/local.rs` and `src/shell/local_io.rs` adheres to the following invariants:

1. **No Symlink Following:**
   - Traversal configures `WalkBuilder::new().follow_links(false)`.
   - File inspection verifies non-symlink metadata (`fs::symlink_metadata(path)?.file_type().is_file()`).
   - Symlinks to files, directories, or special devices are completely ignored, avoiding infinite symlink recursion and arbitrary file reads outside the root.
2. **Output Target Exclusion (`OutputFilter`):**
   - The destination digest path is resolved and normalized at startup.
   - Files matching the canonical or normalized target path are excluded during traversal to prevent recursive ingestion loops when generating digests inside the source directory.
3. **Atomic Writes (`AtomicFile`):**
   - Digest files are written to a temporary sibling file in the normalized parent directory.
   - Flushed and synced to disk before being atomically renamed to the destination path.
   - Temporary files are immediately unlinked on `Drop` if an error occurs or the process panics.

---

## 5. Network & Remote Safety (GitHub Ingestion)

Remote ingestion via `src/shell/github.rs` and `src/shell/github_client.rs` operates under strict isolation:

1. **Origin-Gated Authentication:**
   - GitHub tokens (`GITHUB_TOKEN` or `GH_TOKEN`) are attached as `Authorization: Bearer <token>` **only** when the request destination exactly matches `https://api.github.com`.
   - Client redirect policy enforces `RedirectAuthHeaders::Never` so authentication tokens are never leaked to external CDNs or redirect mirrors.
2. **Diskless Streaming Pipeline:**
   - Remote repositories are streamed directly from `/repos/{owner}/{repo}/tarball/{ref}` as a single HTTP response.
   - Archive entries are unpacked via `flate2` and `tar` directly into memory arena buffers (`FileStorage`) without writing tarballs or unpacked files to disk.
3. **Bounded Retries & Jitter:**
   - Bounded exponential backoff with random jitter (`MAX_RETRIES = 3`, initial delay 250 ms, cap 5,000 ms, jitter 50 ms).
   - Only retryable transport errors (I/O, timeout, connection drop) and HTTP 429 / 5xx status codes trigger retries. 4xx client errors fail immediately.

---

## 6. Output Sanitization & Formatting

1. **Plain Text (`--format text`):**
   - Header labels and metadata are normalized.
   - Valid UTF-8 file contents are emitted verbatim (only files with NUL bytes or invalid UTF-8 are rejected) to preserve exact code fidelity for LLM consumption.
2. **XML Format (`--format xml`):**
   - Standard XML entities (`&`, `<`, `>`, `"`, `'`) are escaped.
   - Disallowed XML 1.0 control characters (`\u{0000}` through `\u{001F}` excluding `\t`, `\n`, `\r`) are sanitized to `\u{FFFD}`.
3. **Directory Tree Rendering:**
   - Node labels in the ASCII hierarchy tree are escaped to prevent line-break injections from breaking tree visualization.
