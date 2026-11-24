# Contributing to codefeed

**English** | [한국어](docs/CONTRIBUTING.ko.md)

**Governing Principles:**

- **Single-Source Ownership:** Conduct, security, architecture, coding, and changelog rules live exclusively in their canonical documents below; this file defines only contributor workflow and the mandatory verification gate.
- **Reproducible Toolchain:** Use Git and Rust `>=` `rust-version` in [`Cargo.toml`](Cargo.toml) (MSRV). Run Cargo commands with `--locked` to match CI (dependency changes are also audited by CI `cargo deny`).
- **Scope-Gated Alignment:** Discuss features, CLI changes, or major refactors in an issue before writing code. Self-contained typo and bug fixes with regression tests may be submitted directly as PRs.
- **Minimal Reproducible Reports:** Check [existing GitHub Issues](https://github.com/but212/codefeed/issues) before filing a bug, and include a minimal reproduction (exact command or code, expected vs. actual behavior, OS, and `rustc --version`). Never report security vulnerabilities in public issues — follow [`SECURITY.md`](SECURITY.md).
- **Standard Git & PR Hygiene:** Branch from `main` (`<type>/<topic>`), follow [Conventional Commits](https://www.conventionalcommits.org/), target `main`, and complete the [Pull Request Template](.github/PULL_REQUEST_TEMPLATE.md).

## Verification Checklist

All Pull Requests must pass all four checks in this exact order:

```bash
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --locked
cargo test --locked
```

Iterate with narrow targets (`cargo test --locked <name>`). Regressions and contract changes require deterministic unit or integration tests (TDD; see [`docs/coding_convention.md`](docs/coding_convention.md#7-testing)). For Markdown changes, also lint against [`.markdownlint.json`](.markdownlint.json) (`markdownlint-cli2`, matching CI).

## Canonical References

| Document | Scope |
| :--- | :--- |
| [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md) | Collaboration, technical debate, and moderation |
| [`SECURITY.md`](SECURITY.md) | Supported versions and private vulnerability reporting |
| [`context.md`](context.md) | Domain terms, invariants, and module boundaries |
| [`docs/coding_convention.md`](docs/coding_convention.md) | Rust conventions, functional-core boundary, errors, docs, and tests |
| [`docs/security_and_filtering.md`](docs/security_and_filtering.md) | Threat model, exclusion rules, limits mirror, and filesystem/network safety |
| [`CHANGELOG.md`](CHANGELOG.md) | Qualifying changes recorded under `## [Unreleased]` |
