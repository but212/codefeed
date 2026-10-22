# AGENTS.md

**Audience:** coding agents working in this repository.

This file holds workflow rules only. Each rule points to the document that explains it.

## 1. Read First

Before changing code, read these four documents:

1. [`context.md`](context.md): domain terms, invariants, and the functional-core / imperative-shell boundary.
2. [`README.md`](README.md): the user-visible CLI and output contract.
3. [`docs/coding_convention.md`](docs/coding_convention.md): Rust rules.
4. [`docs/security_and_filtering.md`](docs/security_and_filtering.md): exclusion rules and filesystem and network safety.

## 2. Verify

Run all four before reporting a code change as done. Run them in this order:

- `cargo clippy --all-targets -- -D warnings`
- `cargo fmt --check`
- `cargo doc --no-deps`
- `cargo test` (or `cargo test <name>`)

Report failures with their output. Do not report a step as passing if it was not run.

## 3. Hard Rules

- **Core purity and the `#[must_use]` rule** follow [coding convention sec. 3](docs/coding_convention.md#3-architecture-functional-core-imperative-shell).
- **Limits and lists** are defined only in `src/core/policy.rs`. Code reads the constant and never copies its value. Limit values are stated only in [security sec. 8](docs/security_and_filtering.md#8-limit-values); other documents link there. When a value changes, update `policy.rs` and sec. 8 in the same change.
- **Filesystem and network rules** are in [coding convention sec. 5](docs/coding_convention.md#5-os-and-network-rules-at-the-code-level) and [security sec. 5-6](docs/security_and_filtering.md). They are not optional.
- **Filters and limits move together.** A change to `policy.rs` updates [security sec. 3-4 and sec. 8](docs/security_and_filtering.md#3-exclusion-lists) in the same change.
- **Behavior changes are logged** under `## [Unreleased]` in `CHANGELOG.md`.

## 4. Where Changes Go

| Change | Update |
| :--- | :--- |
| Limit, exclusion list, or domain type | `src/core/policy.rs` (authority), `docs/security_and_filtering.md` sec. 3-4 and sec. 8 |
| CLI flag or option-value rule | `src/core/cli.rs`, `tests/cli.rs`, `README.md` |
| Exit code | `src/shell/mod.rs` (`run_cli`), `tests/cli.rs`, `README.md` |
| Output format | `src/core/render/`, `README.md` (Output), `context.md` sec. 3.5 |
| Domain invariant or module boundary | `context.md` |
| Security, filesystem, or network behavior | `docs/security_and_filtering.md` |
| Rust coding rule | `docs/coding_convention.md` |
| Any behavior change | `CHANGELOG.md` under `## [Unreleased]` |
