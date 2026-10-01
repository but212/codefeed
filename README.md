# rustingest

Convert a local directory or a public or token-authenticated GitHub repository into one LLM-friendly text file.

## Usage

Requires Rust and Cargo. From the project root:

```bash
cargo run -- <SOURCE> [--output <PATH|->] [--ref <BRANCH|TAG|COMMIT>]
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

When the digest goes to a file, rustingest prints the output path and the number of included and skipped files on success. With `--output -` it reports that summary on standard error instead, so standard output carries only digest text. A consumer that closes the pipe early, as in `rustingest . --output - | head -1`, is not an error. Without a token (see Authentication), GitHub sources must be public; rustingest does not use the Git executable.

## Filtering and limits

- Excludes `.git`, hidden paths, symlinks, binary files, and non-UTF-8 paths or file contents.
- Applies `.gitignore` rules inside local sources regardless of Git repository status. Does not apply `.ignore` files.
- Applies nested GitHub `.gitignore` rules within their directory scope, including negation precedence. Reads at most 256 files and 8 MiB in total; exceeding either limit is an error.
- Skips files larger than 1 MiB. Returns an error if more than 10,000 candidates remain after hidden and ignored paths are excluded.
- Counts `skipped` as candidates rejected for size, encoding, or content reasons. Hidden and ignored paths are excluded before candidate accounting and are not counted as skipped.
- Returns an error without committing content if the digest exceeds 20 MiB.
- Rejects GitHub API responses larger than 8 MiB without parsing truncated JSON.
- Sorts file paths lexicographically for deterministic output. Escapes backslashes and newlines in digest headers.
- Writes a file target to a collision-resistant temporary file, syncs it, and atomically renames it on success. Failed writes preserve the existing output. A standard output target skips the temporary file and sync and writes the rendered digest to the stream directly.

`--help` exits with status 0. Invalid arguments and ingest errors are written to stderr and exit with a non-zero status.
