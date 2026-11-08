# Contributing to codefeed

**English** | [한국어](docs/CONTRIBUTING.ko.md)

Guidelines for environment setup, contribution workflow, verification, and repository conventions.

## Code of Conduct

All contributors must adhere to [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md). Focus on constructive feedback, technical debate, and collaborative problem-solving.

## Getting Started

### Prerequisites

- **Rust**: `rust-version` in [`Cargo.toml`](Cargo.toml) (MSRV) or newer (`rustc --version` / `rustup update stable`).
- **Git**: Version control.

### Local Build and Test

```bash
git clone https://github.com/but212/codefeed.git
cd codefeed
cargo build
cargo test --locked
```

## How to Contribute

### 1. Reporting Bugs

Check [existing GitHub Issues](https://github.com/but212/codefeed/issues) first. If new, open an issue including:

- Exact command or code executed
- Expected vs. actual behavior
- OS and Rust version (`rustc --version`)
- Minimal reproduction steps or target repositories

### 2. Suggesting Features or Architectural Changes

- Open an issue to discuss features, CLI changes, or major refactors before writing code.
- Small typo and bug fixes with regression tests may be submitted directly as PRs.

### 3. Reporting Security Vulnerabilities

Do not open public issues. Follow [`SECURITY.md`](SECURITY.md).

## Pull Request Workflow

1. **Branch Naming**: Topic branches from `main` (`feat/name`, `fix/description`, `docs/topic`).
2. **Commit Style**: [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`, `refactor:`, `test:`, `docs:`, `chore:`).
3. **Verify Locally**: Run mandatory verification checks before opening a PR.
4. **Submit PR**: Target `main` and complete the [Pull Request Template](.github/PULL_REQUEST_TEMPLATE.md).

## Verification Checklist

All Pull Requests must pass all four checks in this exact order:

```bash
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo doc --no-deps --locked
cargo test --locked
```

Run narrow test targets while iterating (`cargo test --locked <name>`). Regressions and contract changes require deterministic unit or integration tests (TDD).

## Architecture and Guidelines

`codefeed` enforces strict separation of concerns and single-source ownership:

- **Core & Shell**: [`context.md`](context.md) and [`docs/coding_convention.md`](docs/coding_convention.md) (`src/core/` pure logic vs. `src/shell/` I/O).
- **Security & Limits**: [`docs/security_and_filtering.md`](docs/security_and_filtering.md) (single owner of filtering rules and limits mirror).
- **Rust Conventions**: [`docs/coding_convention.md`](docs/coding_convention.md) (`#[must_use]`, custom errors, immutability).
- **Changelog**: Log observable and contributor workflow changes under `## [Unreleased]` in [`CHANGELOG.md`](CHANGELOG.md).
