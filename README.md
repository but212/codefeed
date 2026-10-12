# codefeed

[![Crates.io](https://img.shields.io/crates/v/codefeed.svg)](https://crates.io/crates/codefeed)
[![CI](https://github.com/but212/codefeed/actions/workflows/ci.yml/badge.svg)](https://github.com/but212/codefeed/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE-MIT)
[![License: Apache 2.0](https://img.shields.io/badge/license-Apache_2.0-blue.svg)](LICENSE-APACHE)
[![MSRV](https://img.shields.io/badge/rustc-1.88%2B-orange.svg)](Cargo.toml)

Fast CLI that converts a local directory or GitHub repository into an LLM-friendly text digest.

## Scope

`codefeed` supports two source types:

- Local directory paths (`./path/to/project`).
- HTTPS GitHub repository URLs (`https://github.com/{owner}/{repo}`).

`codefeed` does not invoke the `git` binary, manage temporary disk clones, or parse non-GitHub remote URLs (`git@`, `ssh://`, GitLab, Bitbucket). For unsupported remote repositories, clone locally first:

```bash
git clone --depth 1 <URL> ./repo && codefeed ./repo
```

## Usage

```bash
codefeed <SOURCE> [--output <PATH|->] [--ref <REF>] [--format <text|xml>]
```

`codefeed` accepts options before or after the `<SOURCE>` argument, supports short aliases (`-o`, `-f`, `-r`, `-h`), and accepts `=` separators (`--format=xml`, `-o=output.txt`).

### Examples

```bash
# Local directory to default digest.txt
codefeed ./my-repo

# Local directory to custom file with short flags
codefeed ./my-repo -o ./digest.txt -f xml

# Options placed before the source with key-value syntax
codefeed --format=xml --output=- ./my-repo | your-tool

# Stream pure digest to stdout (diagnostics go to stderr)
codefeed ./my-repo --output - | your-tool

# GitHub repository (default branch)
codefeed https://github.com/rust-lang/cargo

# GitHub repository at specific revision
codefeed https://github.com/rust-lang/cargo --ref master --output ./cargo-digest.txt

# Use POSIX delimiter for directories starting with a dash
codefeed -- -my-directory
```

### Options

- `-o`, `--output <PATH|->` — Output destination. Defaults to `digest.txt`. Pass `-` for stdout. Supports `--output=<PATH>`.
- `-r`, `--ref <BRANCH|TAG|COMMIT>` — GitHub branch, tag, or commit SHA. Invalid for local sources. Supports `--ref=<REF>`.
- `-f`, `--format <text|xml>` — Serialization format. Defaults to `text`. `xml` emits LLM-optimized XML tags with entity escaping. Supports `--format=<FORMAT>`.
- `-h`, `--help` — Print usage and exit with code 0.
- `--` — Option delimiter. Treats all subsequent arguments as positional sources.

### Installation

```bash
cargo install --path .
```

Or build the release binary directly:

```bash
cargo build --release
./target/release/codefeed ./my-repo
```

## Output and Exit Contracts

- **File output (`--output <PATH>`)**: Writes to a temporary file, syncs to disk, and atomically renames. Emits `<PATH> (<N> files, <N> skipped, ~<N> tokens)` to stdout. Preserves existing files on failure.
- **Stream output (`--output -`)**: Writes raw digest to stdout; emits the completion summary to stderr.
- **Format fidelity**: Plain text (`--format text`) preserves valid UTF-8 file content verbatim for LLM prompt fidelity (rejecting only NUL bytes). Note that raw digests containing ANSI escape sequences in source files are preserved as-is and may affect terminal emulators if viewed directly without piping or a pager. XML format (`--format xml`) sanitizes disallowed XML control characters to `\u{FFFD}`.
- **Pipe termination**: Downstream pipe closure (`BrokenPipe` / `EPIPE`) on stdout exits cleanly with status 0.
- **Errors**: Invalid CLI arguments and ingest errors print to stderr and exit non-zero.

## Authentication

Public GitHub repositories require no credentials. For private repositories or higher rate limits, set:

- `GITHUB_TOKEN` — Checked first.
- `GH_TOKEN` — Fallback if `GITHUB_TOKEN` is unset or empty.

Credentials are sent as `Authorization: Bearer <token>` strictly to exact-origin `https://api.github.com`. Foreign hosts (including archive redirect targets) never receive authorization headers.

## Filtering and Budgets

- **Hierarchy**: Renders directory structure as an ASCII tree (`├── `, `└── `, `│   `).
- **Exclusions**: Excludes `.git`, hidden entries, symlinks, non-UTF-8 paths/content, lockfiles (`Cargo.lock`, `package-lock.json`, `yarn.lock`, `pnpm-lock.yaml`, `go.sum`), and conventional sensitive patterns (`.env*`, `*.pem`, `*.key`, `*.p12`, `*.pkcs12`, `id_rsa*`, `id_ed25519*`, `credentials*`). Note: Sensitive file filtering is a conventional filename/extension filter, not a deep content-based secret scanner. Excluded files consume neither candidate slots nor skipped counts.
- **Gitignore**: Applies `.gitignore` rules in local sources without Git state. Ignores `.ignore` files.
- **Traversal**: Traverses local filesystems via `ignore::WalkParallel` with strict deterministic lexicographical sorting. Prevents symlink and intermediate directory junction traversal (`openat2`/`openat` on Unix, native handle canonical path component verification on Windows).
- **Remote Ingestion**: Streams GitHub repository archives (`/repos/{owner}/{repo}/tarball/{ref}`) in a single HTTP request ($O(1)$) using `flate2` and `tar` directly into the selection pipeline with zero intermediate disk files, eliminating CDN rate limits and per-file connection overhead. Extracts commit revision from the archive `pax_global_header`.
- **Limits**:
  - Max candidate files: 10,000 (accounted in expected $O(N)$ time with zero-allocation lookups; aborts with deterministically sorted largest-directory diagnostics on overflow).
  - Max file size: 1 MiB per file (files exceeding this are counted as `skipped`).
  - Max digest size: 20 MiB (strictly enforced via mathematical depth-aware tree upper bound accounting; aborts with largest-file diagnostics on overflow).
  - Max archive transfer duration: 300 s (`ARCHIVE_MAX_SECS`).
  - Local reader batch size: 16 threads (`FETCH_WINDOW`).
- **Network Resilience**: Retries transient transport errors and HTTP 429/5xx up to 3 total attempts with exponential backoff and jitter (capped at 5s). Fails immediately on 4xx client errors.
- **Central Policy**: Operational constants and patterns reside in `src/core/policy.rs`.
