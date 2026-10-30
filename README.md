# codefeed

[![Crates.io](https://img.shields.io/crates/v/codefeed.svg)](https://crates.io/crates/codefeed)
[![CI](https://github.com/but212/codefeed/actions/workflows/ci.yml/badge.svg)](https://github.com/but212/codefeed/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE-MIT)
[![License: Apache 2.0](https://img.shields.io/badge/license-Apache_2.0-blue.svg)](LICENSE-APACHE)
[![MSRV](https://img.shields.io/badge/rustc-1.88%2B-orange.svg)](Cargo.toml)
[![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/but212/codefeed)

`codefeed` is a fast CLI converting a local directory or public GitHub repository into a single text digest for LLMs.

## Sources

- **Local directory**: `./path/to/project`
- **GitHub repository**: `https://github.com/{owner}/{repo}` (trailing `.git` accepted; queries, fragments, user credentials `user@`, and non-443 ports rejected).

Unsupported protocols (`git@`, `ssh://`, `http://`) and non-GitHub hosts must be cloned locally first:

```bash
git clone --depth 1 <URL> ./repo && codefeed ./repo
```

`codefeed` never executes `git` or clones repositories.

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
codefeed <SOURCE> [--output <PATH|->] [--ref <REF>] [--format <text|xml>] [--exclude <PATTERN>]...
```

Options may appear before or after `<SOURCE>`. Short flags accept `=` syntax (`--format=xml`, `-o=output.txt`).

### Options

- `-o`, `--output <PATH|->`: Output destination (default: `digest.txt`). `-` streams to stdout.
- `-r`, `--ref <BRANCH|TAG|COMMIT>`: GitHub revision (default: default branch). Rejects empty, whitespace-only, `.`, and `..`.
- `-f`, `--format <text|xml>`: Digest format (default: `text`).
- `-x`, `--exclude <PATTERN>`: Drop paths matching a `.gitignore`-style pattern, relative to the source root (e.g. `build/`, `*.log`, `/vendor`). Repeatable; patterns apply in the order given. Works for local and GitHub sources. An invalid pattern, or one that matches nothing (blank, comment, bare `!`), is a usage error. See [`docs/security_and_filtering.md`](docs/security_and_filtering.md) sec. 3.6.
- `-h`, `--help`: Print usage and exit with 0.
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

- **File output** (`-o <PATH>`): Atomic write via temporary file, sync, and rename. Preserves Unix permissions. Parent directory sync failure warns to stderr without failing. Prints `Wrote <PATH> (<N> files, <N> skipped, ~<N> tokens)` to stdout.
- **Stream output** (`-o -`): Digest writes to stdout; summary writes to stderr. Writes nothing on failure.
- **Header**: Local digests start with `Repository: <directory name>`. GitHub digests start with `Repository: https://github.com/{owner}/{repo}`.
- **Text format** (`text`): Verbatim UTF-8 file contents; binary (NUL) or invalid UTF-8 files are skipped. Header labels and paths are escaped; ANSI sequences are kept.
- **XML format** (`xml`): Structured `<repository>`, `<directory_structure>`, and `<files>` elements. Entities are escaped; disallowed XML 1.0 characters become `\u{FFFD}`.
- **Token count**: Heuristic estimate (ASCII bytes / 4 + non-ASCII characters), not an exact tokenizer count.
- **Exit status**: `0` on success or help; `1` on usage or ingest error.
- **Broken pipe**: Exits `0` when the stdout reader closes early.

## What Is Included

Excluded entries are dropped silently and omitted from the skipped count:

- Hidden entries: path segments starting with `.` (e.g. `.git`, `.github`, `.env*`), excluding the source root itself.
- Symlinks and non-regular files (local and archive).
- Lockfiles: `Cargo.lock`, `package-lock.json`, `yarn.lock`, `pnpm-lock.yaml`, `bun.lock`, `bun.lockb`, `go.sum`, `poetry.lock`, `Pipfile.lock`, `composer.lock`, `Gemfile.lock`.
- Credential files matching prefixes (`id_rsa*`, `credentials*`) or extensions (`*.pem`, `*.key`, `*.pfx`, `*.p12`, `*.pkcs12`).
- Files matched by in-tree `.gitignore`. A local `.gitignore` that is a symlink, exceeds `MAX_FILE_SIZE`, is unreadable, or is not valid UTF-8 fails the run instead.
- Paths matched by `--exclude` patterns, including everything under an excluded directory.

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
| `MAX_FILE_SIZE` | File skipped and counted in summary. |
| `MAX_PATH_BYTES` | File skipped and counted in summary. |
| `MAX_PATH_DEPTH` | Run fails. Hidden and noise entries are dropped before this check. |
| `MAX_FILE_COUNT` | Run fails; error lists largest directories. |
| `MAX_OUTPUT_SIZE` | Run fails; no digest written; error lists largest files. |
| `ARCHIVE_MAX_SECS`, `GITHUB_TIMEOUT_SECS` | Request times out. Header timeouts retry per [sec. 6.5-6.6](docs/security_and_filtering.md#6-github-network-safety). |
| `MAX_ATTEMPTS`, `RETRY_INITIAL_DELAY_MS`, `RETRY_MAX_DELAY_MS` | Transient failures retried with backoff. Waits longer than the cap abort the run. See [sec. 6.5](docs/security_and_filtering.md#6-github-network-safety). |

## Authentication

Public repositories require no credentials. Private repositories or higher rate limits read credentials from:

- `GITHUB_TOKEN` (checked first).
- `GH_TOKEN` (fallback when `GITHUB_TOKEN` is unset, empty, or whitespace-only).

Tokens are sent as `Authorization: Bearer <token>` strictly to `https://api.github.com`. Redirects never receive credentials.

## Network Trust

GitHub TLS certificates are validated strictly against Mozilla root certificates bundled in the binary. OS certificate stores and custom proxy CAs are rejected.

## Documentation

| Document | Target | Contents |
| :--- | :--- | :--- |
| [`context.md`](context.md) | Maintainers | Domain terms, invariants, module boundaries |
| [`docs/security_and_filtering.md`](docs/security_and_filtering.md) | Security reviewers | Threat model, exclusion rules, limit values, filesystem and network safety |
| [`docs/coding_convention.md`](docs/coding_convention.md) | Contributors | Rust conventions, functional core boundary, errors, docs, tests |
| [`AGENTS.md`](AGENTS.md) | Coding agents | Mandatory instructions, verification commands, change routing |
| [`CHANGELOG.md`](CHANGELOG.md) | Users / Consumers | Release history and breaking changes |

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE).
