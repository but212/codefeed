# rustingest

Fast CLI that converts a local directory or GitHub repository into an LLM-friendly text digest.

## Scope

`rustingest` supports two source types:

- Local directory paths (`./path/to/project`).
- HTTPS GitHub repository URLs (`https://github.com/{owner}/{repo}`).

`rustingest` does not invoke the `git` binary, manage temporary disk clones, or parse non-GitHub remote URLs (`git@`, `ssh://`, GitLab, Bitbucket). For unsupported remote repositories, clone locally first:

```bash
git clone --depth 1 <URL> ./repo && rustingest ./repo
```

## Usage

```bash
cargo run -- <SOURCE> [--output <PATH|->] [--ref <BRANCH|TAG|COMMIT>] [--format <text|xml>]
```

### Examples

```bash
# Local directory to default digest.txt
rustingest ./my-repo

# Local directory to custom file
rustingest /path/to/project --output ./digest.txt

# Stream pure digest to stdout (diagnostics go to stderr)
rustingest ./my-repo --output - | your-tool

# GitHub repository (default branch)
rustingest https://github.com/rust-lang/cargo

# GitHub repository at specific revision
rustingest https://github.com/rust-lang/cargo --ref master --output ./cargo-digest.txt
```

### Options

- `--output <PATH>` — Output destination. Defaults to `digest.txt`. Pass `-` for stdout.
- `--ref <BRANCH|TAG|COMMIT>` — GitHub branch, tag, or commit SHA. Invalid for local sources.
- `--format <text|xml>` — Serialization format. Defaults to `text`. `xml` emits LLM-optimized XML tags with entity escaping.
- `--help`, `-h` — Print usage and exit with code 0.

### Installation

```bash
cargo install --path .
```

Or build the release binary directly:

```bash
cargo build --release
./target/release/rustingest ./my-repo
```

## Output and Exit Contracts

- **File output (`--output <PATH>`)**: Writes to a temporary file, syncs to disk, and atomically renames. Emits `<PATH> (<N> files, <N> skipped, ~<N> tokens)` to stdout. Preserves existing files on failure.
- **Stream output (`--output -`)**: Writes raw digest to stdout; emits the completion summary to stderr.
- **Pipe termination**: Downstream pipe closure (`BrokenPipe` / `EPIPE`) exits with status 0.
- **Errors**: Invalid CLI arguments and ingest errors print to stderr and exit non-zero.

## Authentication

Public GitHub repositories require no credentials. For private repositories or higher rate limits, set:

- `GITHUB_TOKEN` — Checked first.
- `GH_TOKEN` — Fallback if `GITHUB_TOKEN` is unset or empty.

Credentials are sent as `Authorization: Bearer <token>` strictly to `https://api.github.com` and `https://raw.githubusercontent.com`. Foreign hosts never receive authorization headers.

## Filtering and Budgets

- **Hierarchy**: Renders directory structure as an ASCII tree (`├── `, `└── `, `│   `).
- **Exclusions**: Excludes `.git`, hidden entries, symlinks, non-UTF-8 paths/content, lockfiles (`Cargo.lock`, `package-lock.json`, `yarn.lock`, `pnpm-lock.yaml`, `go.sum`), and sensitive patterns (`.env*`, `*.pem`, `*.key`, `*.p12`, `*.pkcs12`, `id_rsa*`, `id_ed25519*`, `credentials*`). Excluded files consume neither candidate slots nor skipped counts.
- **Gitignore**: Applies `.gitignore` rules in local sources without Git state. Ignores `.ignore` files.
- **Traversal**: Traverses local filesystems via `ignore::WalkParallel` with strict deterministic lexicographical sorting.
- **Remote Fetch**: Fetches GitHub blobs concurrently in batches of 16 (`FETCH_WINDOW`) via scoped OS threads, pre-filtering binary extensions before download.
- **Limits**:
  - Max candidate files: 10,000 (aborts with largest-directory diagnostics on overflow).
  - Max file size: 1 MiB per file (files exceeding this are counted as `skipped`).
  - Max digest size: 20 MiB (aborts with largest-file diagnostics on overflow).
  - Max GitHub API JSON body: 8 MiB (rejects truncated responses).
- **Network Resilience**: Retries transient transport errors and HTTP 429/5xx up to 3 total attempts with exponential backoff and jitter (capped at 5s). Fails immediately on 4xx client errors.
- **Central Policy**: Operational constants and patterns reside in `src/core/policy.rs`.
