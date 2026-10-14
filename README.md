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
- **Format fidelity**: Plain text (`--format text`) preserves valid UTF-8 file content verbatim (rejecting NUL bytes); ANSI sequences in source files are preserved as-is. XML (`--format xml`) sanitizes disallowed XML control characters to `\u{FFFD}`.
- **Pipe termination**: Downstream pipe closure (`BrokenPipe` / `EPIPE`) on stdout exits cleanly with status 0.
- **Errors**: Invalid CLI arguments and ingest errors print to stderr and exit non-zero.

## Authentication

Public GitHub repositories require no credentials. For private repositories or higher rate limits, set:

- `GITHUB_TOKEN` — Checked first.
- `GH_TOKEN` — Fallback if `GITHUB_TOKEN` is unset or empty.

Credentials are sent as `Authorization: Bearer <token>` strictly to exact-origin `https://api.github.com`. Redirects to foreign hosts never receive authorization headers.

## Filtering and Budgets

- **Hierarchy**: Renders directory structure as an ASCII tree (`├── `, `└── `, `│   `).
- **Exclusions**: Excludes `.git`, hidden entries, symlinks, non-UTF-8 paths/content, lockfiles (`Cargo.lock`, `package-lock.json`, `yarn.lock`, `pnpm-lock.yaml`, `go.sum`), and case-insensitive sensitive patterns (`.env*`, `*.pem`, `*.key`, `*.p12`, `*.pkcs12`, `*.credentials`, `id_rsa*`, `id_ed25519*`, `credentials*`). Sensitive filtering matches path conventions, not deep content. Excluded files consume neither candidate slots nor skipped counts.
- **Gitignore**: Applies `.gitignore` rules in local sources without Git state. Ignores `.ignore` files.
- **Traversal**: Traverses local filesystems via `ignore::WalkBuilder` (`follow_links(false)`) with deterministic sorting, sequential reads, non-symlink metadata validation, and normalized output path exclusion.
- **Remote Ingestion**: Streams GitHub repository archives (`/repos/{owner}/{repo}/tarball/{ref}`) in a single HTTP request via `flate2` and `tar` directly into the selection pipeline with zero intermediate disk files. Extracts commit SHA from `pax_global_header`.
- **Limits**:
  - Candidate files: bounded candidate budget; aborts on overflow with deterministically sorted largest-directory diagnostics from observed candidates at termination.
  - Per-file size: individual file cap (`MAX_FILE_SIZE`); exceeding files increment `skipped`.
  - Digest size: strictly enforced total digest cap; aborts with largest-file diagnostics on overflow.
  - Archive transfer: bounded whole-archive transfer timeout (`ARCHIVE_MAX_SECS`).
- **Network Resilience**: Retries transient transport errors (`Io`, `Timeout`, `ConnectionFailed`) and HTTP 429/5xx with exponential backoff and jitter up to 3 total attempts. Fails immediately on non-transient transport errors and 4xx client errors.
- **Central Policy**: Operational constants, budgets, and patterns reside in `src/core/policy.rs`.
