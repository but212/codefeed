# rustingest

Convert a local directory or public GitHub repository into one LLM-friendly text file.

## Usage

Requires Rust and Cargo. From the project root:

```bash
cargo run -- <SOURCE> [--output <PATH>] [--ref <BRANCH|TAG|COMMIT>]
```

### Local directory

```bash
cargo run -- ./my-repo
cargo run -- /path/to/project --output ./digest.txt
```

### Public GitHub repository

```bash
cargo run -- https://github.com/rust-lang/cargo
cargo run -- https://github.com/rust-lang/cargo --ref master --output ./cargo-digest.txt
```

`<SOURCE>` is a local directory or a URL in the form `https://github.com/{owner}/{repo}`.

### Options

- `--output <PATH>` — Write the digest to `PATH`. Defaults to `digest.txt` in the current directory.
- `--ref <BRANCH|TAG|COMMIT>` — Use a GitHub branch, tag, or commit. Defaults to the repository's default branch and is invalid for local sources.
- `--help`, `-h` — Print usage.

To run the release binary:

```bash
cargo build --release
target/release/rustingest ./my-repo --output digest.txt
```

On success, rustingest prints the output path and the number of included and skipped files. GitHub sources must be public; rustingest does not use the Git executable.

## Filtering and limits

- Excludes `.git`, hidden paths, symlinks, binary files, and non-UTF-8 paths or file contents.
- Applies `.gitignore` rules inside local sources regardless of Git repository status. Does not apply `.ignore` files.
- Applies nested GitHub `.gitignore` rules within their directory scope, including negation precedence. Reads at most 256 files and 8 MiB in total; exceeding either limit is an error.
- Skips files larger than 1 MiB. Returns an error if more than 10,000 candidates remain after hidden and ignored paths are excluded.
- Returns an error without committing content if the digest exceeds 20 MiB.
- Rejects GitHub API responses larger than 8 MiB without parsing truncated JSON.
- Sorts file paths lexicographically for deterministic output. Escapes backslashes and newlines in digest headers.
- Writes to a collision-resistant temporary file, syncs it, and atomically renames it on success. Failed writes preserve the existing output.

`--help` exits with status 0. Invalid arguments and ingest errors are written to stderr and exit with a non-zero status.
