# rustingest

Convert a local directory or a public or token-authenticated GitHub repository into one LLM-friendly text file.

## Usage

Requires Rust and Cargo. From the project root:

```bash
cargo run -- <SOURCE> [--output <PATH|->] [--ref <BRANCH|TAG|COMMIT>] [--format <text|xml>]
```

### Local directory

```bash
cargo run -- ./my-repo
cargo run -- /path/to/project --output ./digest.txt
```

### Standard output

```bash
rustingest ./my-repo --output - | your-tool
```

`--output -` writes the digest to standard output and creates no file.

### GitHub repository

```bash
cargo run -- https://github.com/rust-lang/cargo
cargo run -- https://github.com/rust-lang/cargo --ref master --output ./cargo-digest.txt
```

`<SOURCE>` is a local directory or a URL in the form `https://github.com/{owner}/{repo}`.

## Install locally

From a checked-out rustingest repository:

```bash
cargo install --path .
rustingest ./my-repo
```

Cargo installs the binary in its configured binary directory, usually `~/.cargo/bin`.

### Options

- `--output <PATH>` — Write the digest to `PATH`. Defaults to `digest.txt` in the current directory. `-` writes to standard output.
- `--ref <BRANCH|TAG|COMMIT>` — Use a GitHub branch, tag, or commit. Defaults to the repository's default branch and is invalid for local sources.
- `--format <text|xml>` — Serialization format for the digest. Defaults to `text`. `xml` emits an Anthropic/LLM-optimized XML structure with entity escaping.
- `--help`, `-h` — Print usage.

### Authentication

Public GitHub repositories need no credentials. For private repositories or higher rate limits, set one of:

- `GITHUB_TOKEN` — used first when set.
- `GH_TOKEN` — used when `GITHUB_TOKEN` is unset or blank.

Values are trimmed; a blank value falls through to the next variable. The token is sent as `Authorization: Bearer <token>` only to `https://api.github.com` and `https://raw.githubusercontent.com`; requests to any other host never carry credentials.

To run the release binary:

```bash
cargo build --release
target/release/rustingest ./my-repo --output digest.txt
```

When the digest goes to a file, rustingest prints the output path, file counts, and approximate token estimate (`~N tokens`) on success. With `--output -` it reports that summary on standard error instead, so standard output carries only digest text. A consumer that closes the pipe early, as in `rustingest . --output - | head -1`, is not an error. Without a token (see Authentication), GitHub sources must be public; rustingest does not use the Git executable.

## Filtering and limits

- Emits directory structures as formatted ASCII hierarchy trees (`├── `, `└── `, `│   `).
- Excludes `.git`, hidden paths, symlinks, binary files, non-UTF-8 paths or file contents, common lockfiles (`Cargo.lock`, `package-lock.json`, `yarn.lock`, `pnpm-lock.yaml`, `go.sum`), and conventional sensitive files (`.env*`, `*.pem`, `*.key`, `*.p12`, `*.pkcs12`, `id_rsa*`, `id_ed25519*`, `credentials*`).
- Applies `.gitignore` rules inside local sources regardless of Git repository status. Does not apply `.ignore` files.
- Traverses local directories concurrently with multi-threaded `ignore::WalkParallel`, sorting entries into strict deterministic lexicographical order before selection and reading.
- Downloads GitHub files in parallel batches (window size 16) using standard threads, pre-filtering known binary extensions and sensitive files before download to save bandwidth while preserving candidate and skipped accounting.
- Skips files larger than 1 MiB. Returns an error listing the largest directories (based on candidates observed up to the limit) if more than 10,000 candidates remain after hidden, ignored, and sensitive paths are excluded.
- Counts `skipped` as candidates rejected for size, encoding, or content reasons. Hidden, ignored, lockfile, and sensitive paths are excluded before candidate accounting and are not counted as skipped.
- Returns an error listing the largest files without committing content if the digest exceeds 20 MiB.
- Rejects GitHub API responses larger than 8 MiB without parsing truncated JSON.
- Writes a file target to a collision-resistant temporary file, syncs it, and atomically renames it on success. Failed writes preserve the existing output. A standard output target skips the temporary file and sync and writes the rendered digest to the stream directly.
- Retries transient network failures and HTTP 429/5xx responses up to 3 total attempts (1 initial + up to 2 retries) with bounded exponential backoff and jitter (capped at 5s), failing immediately on non-retryable client errors.
- All limits, batch sizes (`FETCH_WINDOW`), retry policies, noise lockfile lists, and binary extensions are centralized in `src/core/policy.rs`.

`--help` exits with status 0. Invalid arguments and ingest errors are written to stderr and exit with a non-zero status.
