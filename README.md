# codefeed

[![Crates.io](https://img.shields.io/crates/v/codefeed.svg)](https://crates.io/crates/codefeed)
[![CI](https://github.com/but212/codefeed/actions/workflows/ci.yml/badge.svg)](https://github.com/but212/codefeed/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE-MIT)
[![License: Apache 2.0](https://img.shields.io/badge/license-Apache_2.0-blue.svg)](LICENSE-APACHE)
[![MSRV](https://img.shields.io/badge/rustc-1.88%2B-orange.svg)](Cargo.toml)
[![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/but212/codefeed)

**English** | [한국어](docs/README.ko.md)

`codefeed` is a fast CLI converting a local directory or public GitHub repository into a single text digest for LLMs.

## Sources

- **Local directory**: `./path/to/project`
- **GitHub repository**: `https://github.com/{owner}/{repo}` (trailing `.git` accepted; queries, fragments, user credentials `user@`, and non-443 ports rejected).

Unsupported protocols (`git@`, `ssh://`, `http://`) and non-GitHub hosts must be cloned locally first:

```bash
git clone --depth 1 <URL> ./repo && codefeed ./repo
```

`codefeed` never runs `git` or clones repositories.

## Install

```bash
cargo install --path .
```

Or install a prebuilt binary from GitHub Releases with [`cargo-binstall`](https://github.com/cargo-bins/cargo-binstall):

```bash
cargo binstall codefeed
```

Or build a release binary:

```bash
cargo build --release
./target/release/codefeed ./my-repo
```

Requires Rust 1.88+.

## Usage

```bash
codefeed <SOURCE> [--output <PATH|->] [--ref <REF>] [--format <text|xml>] [--exclude <PATTERN>]...
```

Options may appear before or after `<SOURCE>`. Short flags accept `=` syntax (`--format=xml`, `-o=output.txt`).

### Options

- `-o`, `--output <PATH|->`: Output destination (default: `digest.txt`). `-` streams to stdout.
- `-r`, `--ref <BRANCH|TAG|COMMIT>`: GitHub revision (default: default branch). Rejects empty, whitespace-only, `.`, and `..`.
- `-f`, `--format <text|xml>`: Digest format (default: `text`).
- `-x`, `--exclude <PATTERN>`: Drop paths matching a `.gitignore`-style pattern, relative to the source root (e.g. `build/`, `*.log`, `/vendor`). Repeatable, applied in order, for local and GitHub sources. Invalid patterns and patterns yielding no rule (blank, comment, bare `!`) are usage errors. Syntax details are in [`docs/security_and_filtering.md`](docs/security_and_filtering.md) sec. 3.6.
- `-h`, `--help`: Print usage and exit 0.
- `-V`, `--version`: Print `codefeed <version>` and exit 0.
- `--`: Treat subsequent arguments as positionals (for directories starting with `-`).

Inline values (`--flag=value`) may start with `-` (e.g. `--output=-notes.txt`). Separate argument values may not, except `-o -`.

### Examples

```bash
# Local directory to digest.txt
codefeed ./my-repo

# Custom file, short flags
codefeed ./my-repo -o ./digest.txt -f xml

# Stream to stdout, pipe to another tool
codefeed --format=xml --output=- ./my-repo | your-tool

# GitHub repository at default branch
codefeed https://github.com/rust-lang/cargo

# GitHub repository at specific ref
codefeed https://github.com/rust-lang/cargo --ref master -o ./cargo-digest.txt

# Directory starting with a dash
codefeed -- -my-directory
```

## Output

- **File output** (`-o <PATH>`): Atomic write via temporary file, sync, and rename. Preserves Unix permissions. Parent directory sync failure warns to stderr without failing. Prints `Wrote <PATH> (<N> files, <N> skipped, ~<N> tokens)` to stdout, where `<PATH>` is the `-o` value as given.
- **Stream output** (`-o -`): Digest writes to stdout; summary writes to stderr. Writes nothing on failure.
- **Header**: Local digests start with `Repository: <directory name>`. GitHub digests start with `Repository: https://github.com/{owner}/{repo}`.
- **Text format** (`text`): Verbatim UTF-8 file contents; binary (NUL) or invalid UTF-8 files are skipped. Header labels and paths are escaped; ANSI sequences are kept.
- **XML format** (`xml`): Structured `<repository>`, `<directory_structure>`, and `<files>` elements. Entities are escaped; disallowed XML 1.0 characters become `\u{FFFD}`.
- **Token count**: Heuristic estimate (ASCII bytes / `ASCII_BYTES_PER_TOKEN` + non-ASCII characters), not an exact tokenizer count.
- **Exit status**: `0` on success, help, or version; `1` on usage or ingest error.
- **Broken pipe**: Exits `0` when the stdout reader closes early.

## What Is Included

Excluded entries are dropped silently and not counted as skipped. Exception: an oversized tar header fails the run even for an excluded entry ([`docs/security_and_filtering.md`](docs/security_and_filtering.md) sec. 2):

- Hidden entries: path segments starting with `.` (e.g. `.git`, `.github`, `.env*`), excluding the source root itself.
- Symlinks and non-regular files (local and archive).
- Dependency lockfiles (sec. 3.2).
- Credential and key files, matched by name prefix or extension, case-insensitive (sec. 3.3).
- Files matched by in-tree `.gitignore` (local sources only; GitHub archives do not apply `.gitignore`). An unsafe or unreadable local `.gitignore` fails the run instead (sec. 3.5).
- Paths matched by `--exclude` patterns, including everything under an excluded directory (sec. 3.6).

Section numbers above refer to [`docs/security_and_filtering.md`](docs/security_and_filtering.md).

Skipped entries are counted in the summary:

- Files exceeding `MAX_FILE_SIZE`.
- Binary (NUL byte) or invalid UTF-8 files.
- Paths exceeding `MAX_PATH_BYTES` or non-UTF-8.
- Unreadable files (permission denied, locked).

> **Note:** Filtering matches paths and names only, never contents. Do not rely on `codefeed` to redact secrets embedded in tracked source files. Complete rules are in [`docs/security_and_filtering.md`](docs/security_and_filtering.md) sec. 3.

## Limits

Limits either skip an entry or abort the run. Limit values and enforcement details are in [`docs/security_and_filtering.md`](docs/security_and_filtering.md) sec. 8.

| Limit | On Reach |
| :--- | :--- |
| `MAX_FILE_SIZE` | File skipped and counted in summary; an oversized `.gitignore` fails the run. |
| `MAX_PATH_BYTES` | File skipped and counted in summary. |
| `MAX_PATH_DEPTH` | Run fails. |
| `MAX_FILE_COUNT` | Run fails; error lists largest directories. |
| `MAX_OUTPUT_SIZE` | Run fails; no digest written; error lists largest files. |
| `ARCHIVE_MAX_HEADER_BYTES` | Run fails. |
| `ARCHIVE_MAX_DECOMPRESSED_BYTES` | Run fails. |
| `MAX_DIR_ENTRIES` | Run fails. |
| `ARCHIVE_MAX_SECS`, `GITHUB_TIMEOUT_SECS` | Request times out; header timeouts are retried ([sec. 6.5-6.6](docs/security_and_filtering.md#6-github-network-safety)). |
| `MAX_ATTEMPTS`, `RETRY_INITIAL_DELAY_MS`, `RETRY_MAX_DELAY_MS` | Transient failures retried with backoff; waits longer than the cap abort the run ([sec. 6.5](docs/security_and_filtering.md#6-github-network-safety)). |

## Authentication

Public repositories require no credentials. Private access or higher rate limits read credentials from:

- `GITHUB_TOKEN` (checked first).
- `GH_TOKEN` (fallback when `GITHUB_TOKEN` is unset, empty, or whitespace-only).

Tokens are sent as `Authorization: Bearer <token>` strictly to `https://api.github.com`; redirects never receive credentials.

## Network Trust

GitHub TLS certificates validate strictly against bundled Mozilla root certificates. OS certificate stores and custom proxy CAs are rejected.

## Documentation

| Document | Target | Contents |
| :--- | :--- | :--- |
| [`context.md`](context.md) | Maintainers | Domain terms, invariants, module boundaries |
| [`docs/security_and_filtering.md`](docs/security_and_filtering.md) | Security reviewers | Threat model, exclusion rules, limit values, filesystem and network safety |
| [`docs/coding_convention.md`](docs/coding_convention.md) | Contributors | Rust conventions, functional core boundary, errors, docs, tests |
| [`AGENTS.md`](AGENTS.md) | Coding agents | Mandatory instructions, verification commands, change routing |
| [`docs/README.ko.md`](docs/README.ko.md) | Korean readers | Translation of this file; this file wins on conflict |
| [`CHANGELOG.md`](CHANGELOG.md) | Users / Consumers | Release history and breaking changes |

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE).
