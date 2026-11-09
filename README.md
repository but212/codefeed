# codefeed

[![Crates.io](https://img.shields.io/crates/v/codefeed.svg)](https://crates.io/crates/codefeed)
[![CI](https://github.com/but212/codefeed/actions/workflows/ci.yml/badge.svg)](https://github.com/but212/codefeed/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE-MIT)
[![License: Apache 2.0](https://img.shields.io/badge/license-Apache_2.0-blue.svg)](LICENSE-APACHE)
[![MSRV](https://img.shields.io/badge/rustc-1.88%2B-orange.svg)](Cargo.toml)
[![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/but212/codefeed)

**English** | [한국어](docs/README.ko.md)

Fast CLI converting a local directory or public GitHub repository into a single text digest for LLMs.

## Sources

- **Local directory**: `./path/to/project`
- **GitHub repository**: `https://github.com/{owner}/{repo}` (trailing `.git` accepted; queries, fragments, `user@`, and non-443 ports rejected).

Unsupported protocols (`git@`, `ssh://`, `http://`) and non-GitHub hosts must be cloned locally first:

```bash
git clone --depth 1 <URL> ./repo && codefeed ./repo
```

`codefeed` never runs `git` or clones repositories.

## Install

```bash
cargo install codefeed     # build from crates.io
cargo binstall codefeed    # optional: prebuilt binary (requires cargo-binstall)
cargo install --path .     # build from a local checkout
cargo build --release      # release binary at ./target/release/codefeed
```

Requires Rust 1.88+.

## Usage

```bash
codefeed <SOURCE> [--output <PATH|->] [--ref <REF>] [--format <text|xml>] [--exclude <PATTERN>]...
```

Options may appear before or after `<SOURCE>`. Short flags accept `=` syntax (`--format=xml`, `-o=output.txt`).

### Options

- `-o`, `--output <PATH|->`: Destination (default: `digest.txt`). `-` streams to stdout.
- `-r`, `--ref <BRANCH|TAG|COMMIT>`: GitHub revision (default: default branch). Rejects empty, whitespace-only, `.`, and `..`.
- `-f`, `--format <text|xml>`: Digest format (default: `text`).
- `-x`, `--exclude <PATTERN>`: Drop paths matching a `.gitignore`-style pattern, relative to source root. Repeatable, applied in order. Invalid patterns and patterns yielding no rule (blank, comment, bare `!`) are usage errors. Syntax: [`docs/security_and_filtering.md`](docs/security_and_filtering.md) sec. 3.6.
- `-h`, `--help`: Print usage and exit 0. Preempts other argument errors.
- `-V`, `--version`: Print `codefeed <version>` and exit 0. Preempts other argument errors.
- `--`: Treat subsequent arguments as positionals.

Inline values (`--flag=value`) may start with `-` (e.g. `--output=-notes.txt`). Separate values may not, except `-o -`.

### Examples

```bash
codefeed ./my-repo
codefeed ./my-repo -o ./digest.txt -f xml
codefeed --format=xml --output=- ./my-repo | your-tool
codefeed https://github.com/rust-lang/cargo
codefeed https://github.com/rust-lang/cargo --ref master -o ./cargo-digest.txt
codefeed -- -my-directory
```

## Output

- **File output** (`-o <PATH>`): Atomic write via temporary file, sync, and rename. Preserves Unix permissions. Parent directory sync failure warns without failing. Prints `Wrote <PATH> (<N> files, <N> skipped, ~<N> tokens)` to stdout (`<PATH>` as given).
- **Stream output** (`-o -`): Digest to stdout; summary to stderr. Writes nothing on failure.
- **Header**: Local digests start with `Repository: <directory name>`; GitHub digests start with `Repository: https://github.com/{owner}/{repo}`.
- **Text format** (`text`): Verbatim UTF-8 file contents; binary (NUL) or invalid UTF-8 files skipped. Header labels and paths escaped; ANSI sequences preserved.
- **XML format** (`xml`): `<repository>`, `<directory_structure>`, and `<files>`. Entities escaped; disallowed XML 1.0 characters become `\u{FFFD}`.
- **Token count**: Heuristic estimate (floor(ASCII bytes / ratio) from [security sec. 8](docs/security_and_filtering.md#8-limit-values) + one token per non-ASCII character; at least 1 for non-empty digest, 0 for empty), not an exact tokenizer count.
- **Exit status**: `0` on success, help, or version; `1` on usage or ingest error. Exits `0` on broken pipe when stdout reader closes early.

## What Is Included

Excluded entries are dropped silently and omitted from skipped counts (exception: oversized tar headers fail the run; [`docs/security_and_filtering.md`](docs/security_and_filtering.md) sec. 2):

- Hidden entries: path segments starting with `.` (excluding source root itself).
- Symlinks and non-regular files (local and archive).
- Dependency lockfiles (sec. 3.2).
- Credential and key files by prefix or extension (sec. 3.3).
- Files matched by in-tree `.gitignore` (local only; unsafe or unreadable `.gitignore` fails the run, sec. 3.5).
- Paths matched by `--exclude` patterns (sec. 3.6).

Filtering matches paths only; file contents are never scanned. Keep secrets in files with ordinary names out of the digest with `.gitignore` or `--exclude` ([`docs/security_and_filtering.md`](docs/security_and_filtering.md) sec. 1.2).

Skipped entries (counted in summary):

- Files exceeding `MAX_FILE_SIZE`.
- Binary (NUL byte) or invalid UTF-8 files.
- Paths exceeding `MAX_PATH_BYTES` or non-UTF-8.
- Unreadable files (permission denied, locked).

> **Note:** Filtering matches paths and names only, never contents. Complete rules: [`docs/security_and_filtering.md`](docs/security_and_filtering.md) sec. 3.

## Limits

Limits either skip an entry or abort the run. Values and enforcement: [`docs/security_and_filtering.md`](docs/security_and_filtering.md) sec. 8. Timeouts and retries: sec. 6.5-6.6.

| Limit | On Reach |
| :--- | :--- |
| `MAX_FILE_SIZE` | File skipped and counted in summary; oversized `.gitignore` fails the run. |
| `MAX_PATH_BYTES` | File skipped and counted in summary. |
| `MAX_PATH_DEPTH` | Run fails. |
| `MAX_FILE_COUNT` | Run fails; error lists largest directories. |
| `MAX_OUTPUT_SIZE` | Run fails; no digest written; error lists largest files. |
| `ARCHIVE_MAX_HEADER_BYTES` | Run fails. |
| `ARCHIVE_MAX_DECOMPRESSED_BYTES` | Run fails. |
| `MAX_DIR_ENTRIES` | Run fails. |

## Authentication

Public repositories require no credentials. Private access or higher rate limits read credentials from:

- `GITHUB_TOKEN` (checked first).
- `GH_TOKEN` (fallback when `GITHUB_TOKEN` is unset, empty, or whitespace-only).

Tokens are sent as `Authorization: Bearer <token>` strictly to `https://api.github.com`; redirects never receive credentials.

## Network Trust

GitHub TLS certificates validate strictly against bundled Mozilla root certificates. OS stores and proxy CAs are rejected.

## Documentation

| Document | Target | Contents |
| :--- | :--- | :--- |
| [`context.md`](context.md) | Maintainers | Domain terms, invariants, module boundaries |
| [`docs/security_and_filtering.md`](docs/security_and_filtering.md) | Security reviewers | Threat model, exclusion rules, limit values, filesystem and network safety |
| [`docs/coding_convention.md`](docs/coding_convention.md) | Contributors | Rust conventions, functional core boundary, errors, docs, tests |
| [`CONTRIBUTING.md`](CONTRIBUTING.md) | Contributors | Setup, contribution workflow, verification checklist |
| [`SECURITY.md`](SECURITY.md) | Security reporters | Supported versions, private vulnerability reporting |
| [`AGENTS.md`](AGENTS.md) | Coding agents | Mandatory instructions, change routing |
| [`docs/README.ko.md`](docs/README.ko.md) | Korean readers | Translation of this file; this file wins on conflict |
| [`CHANGELOG.md`](CHANGELOG.md) | Users / Consumers | Release history and breaking changes |

## Contributing & License

Contributions are welcome per [`CONTRIBUTING.md`](CONTRIBUTING.md). Dual-licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE).
