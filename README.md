# codefeed

[![Crates.io](https://img.shields.io/crates/v/codefeed.svg)](https://crates.io/crates/codefeed)
[![CI](https://github.com/but212/codefeed/actions/workflows/ci.yml/badge.svg)](https://github.com/but212/codefeed/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE-MIT)
[![License: Apache 2.0](https://img.shields.io/badge/license-Apache_2.0-blue.svg)](LICENSE-APACHE)
[![MSRV](https://img.shields.io/badge/rustc-1.88%2B-orange.svg)](Cargo.toml)

`codefeed` is a fast CLI that converts a local directory or a public GitHub repository into one text digest that is easy for an LLM to read.

## Sources

- **Local directory**: `./path/to/project`
- **GitHub repository**: `https://github.com/{owner}/{repo}`. A trailing `.git` is accepted. Query strings, fragments, credentials (`user@`), and ports other than `443` are rejected.

Not supported: `git@` and `ssh://` URLs, `http://` URLs, and non-GitHub hosts such as GitLab or Bitbucket. For those, clone first:

```bash
git clone --depth 1 <URL> ./repo && codefeed ./repo
```

`codefeed` never runs the `git` binary and never creates a clone.

## Install

```bash
cargo install --path .
```

Or build a release binary:

```bash
cargo build --release
./target/release/codefeed ./my-repo
```

Requires Rust 1.88 or newer.

## Usage

```bash
codefeed <SOURCE> [--output <PATH|->] [--ref <REF>] [--format <text|xml>]
```

Options can appear before or after `<SOURCE>`. Each short flag (`-o`, `-f`, `-r`) also accepts the `=` form (`--format=xml`, `-o=output.txt`).

### Options

- `-o`, `--output <PATH|->`: Where the digest goes. Defaults to `digest.txt` in the current directory. `-` writes to stdout.
- `-r`, `--ref <BRANCH|TAG|COMMIT>`: GitHub sources only. Without it, the repository's default branch is used. Empty, whitespace-only, `.`, and `..` are rejected.
- `-f`, `--format <text|xml>`: Output format. Defaults to `text`.
- `-h`, `--help`: Print usage and exit with status 0.
- `--`: Treat everything after it as positional arguments, not options. Use it for a directory whose name starts with `-`.

A value written inline (`--flag=value`) may start with `-`, as in `--output=-notes.txt`. A value written as the next argument may not start with `-`. The one exception is `-o -` for stdout.

### Examples

```bash
# Local directory to digest.txt
codefeed ./my-repo

# Custom file, short flags
codefeed ./my-repo -o ./digest.txt -f xml

# Options before the source, stream to another tool
codefeed --format=xml --output=- ./my-repo | your-tool

# GitHub repository at its default branch
codefeed https://github.com/rust-lang/cargo

# GitHub repository at a specific ref
codefeed https://github.com/rust-lang/cargo --ref master -o ./cargo-digest.txt

# Directory whose name starts with a dash
codefeed -- -my-directory
```

## Output

- **File output** (`-o <PATH>`): The digest is written atomically. It goes to a temporary file, which is synced and then renamed over the target. If the run fails, the previous file is left unchanged. On Unix, the existing file's permissions are kept. If syncing the parent directory fails after the rename, a warning goes to stderr and the run still succeeds, because the new file is already in place. The summary line `Wrote <PATH> (<N> files, <N> skipped, ~<N> tokens)` is printed to stdout.
- **Stream output** (`-o -`): The digest goes to stdout and the summary goes to stderr. Nothing is written if the run fails.
- **Header**: Local digests start with `Repository: <directory name>`. GitHub digests start with `Repository: https://github.com/{owner}/{repo}`.
- **Text format** (`text`): File contents are copied exactly as valid UTF-8. Files that contain a NUL byte or are not valid UTF-8 are skipped. Header labels and file paths are escaped, and ANSI sequences in file contents are kept.
- **XML format** (`xml`): The digest contains `<repository>`, `<directory_structure>`, and `<files>` with one `<file path="...">` per file. Entities are escaped. Characters that XML 1.0 does not allow become U+FFFD.
- **Token count**: An estimate, not a tokenizer result. It counts ASCII text by bytes and counts each non-ASCII character as one token. Use it as a rough guide.
- **Exit status**: `0` on success or help. `1` on a usage error or an ingest error. Errors are printed to stderr.
- **Broken pipe**: If the reader of stdout closes early, `codefeed` exits with status `0`.

## What Is Included

Excluded entries are dropped silently and are not counted in the summary:

- Hidden entries: any path segment that starts with `.`, such as `.git`, `.github`, and `.env*`. The source root itself may be hidden.
- Symlinks and other non-regular files (local sources and GitHub archive entries).
- Conventional lockfiles, such as `Cargo.lock`, `package-lock.json`, and `yarn.lock`.
- Credential files, matched by name and case-insensitively. Examples: `id_rsa*`, `credentials*`, `*.pem`, `*.key`, `*.pfx`.
- Files matched by `.gitignore` (local sources).

Skipped entries are counted in the summary:

- Files over the per-file size limit.
- Files that contain a NUL byte or are not valid UTF-8.
- Paths that are not valid UTF-8, or longer than the path-length limit.
- Files that cannot be opened, such as permission-denied or locked files.

Ignore rules for local sources:

- `.gitignore` files inside the source tree are applied, even without a Git repository.
- `.ignore` files are not read.
- The global gitignore, `.git/info/exclude`, and `.gitignore` files above the source root are not read.

The complete lists are in [`docs/security_and_filtering.md`](docs/security_and_filtering.md) §3.

> **Note:** Filtering matches file names and extensions only. It does not scan contents. Do not rely on `codefeed` to remove secrets that are written into tracked source files.

## Limits

Each limit either skips an entry or ends the run. The values and the enforcement rules are in [`docs/security_and_filtering.md`](docs/security_and_filtering.md) §8.

| Limit | When it is reached |
| :--- | :--- |
| `MAX_FILE_SIZE` | The file is skipped and counted as skipped. |
| `MAX_PATH_BYTES` | The file is skipped and counted as skipped. |
| `MAX_PATH_DEPTH` | The run fails. Hidden and noise entries are dropped before this check, so they never fail the run. |
| `MAX_FILE_COUNT` | The run fails. The error lists the largest directories. Local runs may list different directories from run to run. |
| `MAX_OUTPUT_SIZE` | The run fails and no digest is written. The error lists the largest files. |
| `ARCHIVE_MAX_SECS`, `GITHUB_TIMEOUT_SECS` | The GitHub request times out. Timeouts on response headers are retried. See [§6.5–6.6](docs/security_and_filtering.md#6-github-network-safety). |
| `MAX_ATTEMPTS`, `RETRY_INITIAL_DELAY_MS`, `RETRY_MAX_DELAY_MS` | Transient GitHub failures are retried with backoff. If GitHub asks you to wait longer than the cap, the run fails instead of waiting. See [§6.5](docs/security_and_filtering.md#6-github-network-safety). |

## Authentication

Public repositories need no credentials. For private repositories or higher rate limits, set one of these environment variables:

- `GITHUB_TOKEN` is checked first.
- `GH_TOKEN` is used when `GITHUB_TOKEN` is unset, empty, or whitespace-only.

The token is sent as `Authorization: Bearer <token>` only to `https://api.github.com`. Redirects never receive it.

## Network Trust

`codefeed` checks GitHub certificates against the Mozilla root certificates built into the binary. It does not use the operating system's certificate store. A certificate that is trusted only through the OS store, such as a corporate proxy CA, is rejected.

## Documentation

| Document | For | Contents |
| :--- | :--- | :--- |
| [`context.md`](context.md) | Maintainers | Domain terms, invariants, module boundary |
| [`docs/security_and_filtering.md`](docs/security_and_filtering.md) | Security reviewers and filter changes | Threat model, exclusion rules, limit values, filesystem and network safety |
| [`docs/coding_convention.md`](docs/coding_convention.md) | Contributors | Rust rules, functional-core boundary, errors, docs, tests |
| [`AGENTS.md`](AGENTS.md) | Coding agents | Required reading, verification commands, change rules |
| [`CHANGELOG.md`](CHANGELOG.md) | Users and library consumers | Release history and breaking changes |

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE), at your option.
