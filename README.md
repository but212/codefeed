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

`codefeed` never invokes `git` or clones repositories. Accepted sources:

- **Local directory**: `./path/to/project`
- **GitHub repository**: canonical `https://github.com/{owner}/{repo}` (optional trailing `.git`; `user@`, queries, fragments, and non-443 ports are rejected).

Any other protocol or host must be cloned locally first:

```bash
git clone --depth 1 <URL> ./repo && codefeed ./repo
```

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

### Options

- `-o`, `--output <PATH|->`: Destination (default: `digest.txt`; `-` streams to stdout).
- `-r`, `--ref <BRANCH|TAG|COMMIT>`: GitHub revision (default: default branch). Rejects empty, whitespace-only, `.`, and `..`.
- `-f`, `--format <text|xml>`: Digest format (default: `text`).
- `-x`, `--exclude <PATTERN>`: Drop paths matching a `.gitignore`-style pattern relative to source root (repeatable, applied in order; invalid or empty patterns are usage errors; syntax: [`docs/security_and_filtering.md`](docs/security_and_filtering.md) § 3.6).
- `-h`, `--help`: Print usage and exit `0`.
- `-V`, `--version`: Print `codefeed <version>` and exit `0`.
- `--`: Treat subsequent arguments as positionals.

**Parsing rules:** Options may appear before or after `<SOURCE>` up to `--`. Value options take either an inline value (`--format=xml`, `-o=output.txt`; may start with `-`, e.g. `--output=-notes.txt`) or the next token (may not start with `-`, except `-o -`); tokens in a separate value position are never parsed as options (even when rejected, as in `--output --help`). Among tokens parsed as options, the first `-h`/`--help` or `-V`/`--version` wins and preempts other argument errors.

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

- **Buffered delivery**: Collection and rendering finish in memory before anything is emitted; failures produce no digest.
  - **File** (`-o <PATH>`): Atomic replacement preserving Unix permissions (parent-directory sync failure warns without failing; [security § 5](docs/security_and_filtering.md#5-local-filesystem-safety)). Prints `Wrote <PATH> (<N> files, <N> skipped, ~<N> tokens)` to stdout (`<PATH>` as given).
  - **Stream** (`-o -`): Digest to stdout, summary to stderr (a downstream write failure can leave a partial stream).
- **Header**: `Repository: <directory name>` for local sources; `Repository: https://github.com/{owner}/{repo}` for GitHub sources.
- **Formats**: `text` emits verbatim UTF-8 file bodies (ANSI preserved) with escaped headers and paths; `xml` emits `<repository>`, `<directory_structure>`, and `<files>` with full XML 1.0 escaping ([security § 7](docs/security_and_filtering.md#7-output-sanitization)).
- **Token count**: Heuristic estimate (`floor(ASCII bytes / ratio)` from [security § 8](docs/security_and_filtering.md#8-limit-values) + 1 per non-ASCII character; at least 1 for non-empty digest, 0 for empty), not an exact tokenizer count.
- **Exit status**: `0` on success, help, version, or broken stdout pipe; `1` on usage or ingest error.

## What Is Included

Every entry falls into one of three disjoint outcomes ([`docs/security_and_filtering.md`](docs/security_and_filtering.md) § 2–4):

- **Excluded** (dropped silently, omitted from skipped counts, never read): Non-regular entries (symlinks, etc.) and path/name-only exclusions — hidden segments (`.`-prefixed, excluding source root), dependency lockfiles (§ 3.2), sensitive credential/key names (§ 3.3), in-tree `.gitignore` (local only, § 3.5), and `--exclude` (§ 3.6). Filtering never inspects file contents; keep secrets in ordinary-named files out via `.gitignore` or `--exclude` (§ 1.2).
- **Skipped** (counted in summary): Regular files failing per-entry checks — exceeding `MAX_FILE_SIZE` or `MAX_PATH_BYTES`, non-UTF-8 paths, binary (NUL) or invalid UTF-8 contents, or unreadable/locked files.
- **Included**: All remaining valid UTF-8 regular files.

## Limits

Values and enforcement: [`docs/security_and_filtering.md`](docs/security_and_filtering.md#8-limit-values) § 8. Timeouts and retries: § 6.5–6.6.

- **Per-entry limits skip** (counted in summary): `MAX_FILE_SIZE`, `MAX_PATH_BYTES`.
- **Structural, archive, and global limits abort** the run: `MAX_PATH_DEPTH` (including empty local directories), `MAX_DIR_ENTRIES`, `MAX_FILE_COUNT` (error lists largest directories), `MAX_OUTPUT_SIZE` (error lists largest files), `ARCHIVE_MAX_HEADER_BYTES` (even for entries that would be excluded), and `ARCHIVE_MAX_DECOMPRESSED_BYTES`.
- **Control-file exception**: Unsafe, unreadable, or oversized (`> MAX_FILE_SIZE`) `.gitignore` files abort the run rather than skipping (§ 3.5).

## GitHub Access & Trust

- **Authentication**: Public repositories require no credentials. Private access or higher rate limits read `GITHUB_TOKEN` first, falling back to `GH_TOKEN` when unset, empty, or whitespace-only.
- **Strict origin & TLS**: Tokens are sent as `Authorization: Bearer <token>` exclusively to `https://api.github.com` (never on redirects). GitHub TLS validates strictly against bundled Mozilla root certificates; OS stores and proxy CAs are rejected ([security § 6](docs/security_and_filtering.md#6-github-network-safety)).

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
