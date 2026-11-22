# AGENTS.md

## 1. Core Principles

1. **Precedence & Scope:** `System > Explicit User Intent > Repository AGENTS.md > Docs > General conventions`. Modify only paths required by the approved request; never alter files during read-only analysis, weaken safeguards/tests, or run unrequested VCS mutations (`stage`, `commit`, `push`, `reset`, `clean`) or destructive deletions (`rm -rf`).
2. **Core / Shell & Type Safety:** Keep `src/core/` pure and total (`self -> Self`, no I/O or `&mut self`, no single-impl/pass-through indirection) and isolate I/O in `src/shell/` per [`docs/coding_convention.md`](docs/coding_convention.md#3-architecture-functional-core-imperative-shell). Validate untrusted inputs at boundaries so invalid states are unrepresentable.
3. **Single Source of Truth:** Every rule, invariant, and limit has one canonical owner; other documents link to it and never restate rules or copy limit values. Code reads constants directly from `src/core/policy/`.
4. **Translation Parity:** Changes to `README.md`, `CONTRIBUTING.md`, or `CODE_OF_CONDUCT.md` must be mirrored in `docs/<NAME>.ko.md` (English source wins on conflict).
5. **Verification & Changelog:** Follow TDD for logic changes, log observable behavior changes under `## [Unreleased]` in `CHANGELOG.md`, and pass all four checks of [`CONTRIBUTING.md`](CONTRIBUTING.md#verification-checklist) in exact order before reporting completion.

## 2. Canonical Document & Change Map

Review relevant owners before editing and update all coupled targets in the same change:

| Domain / Change | Canonical Owner & Coupled Targets |
| :--- | :--- |
| Limits, exclusions, path rules | `src/core/policy/`, `src/core/path.rs` ↔ [`docs/security_and_filtering.md`](docs/security_and_filtering.md) (§ 2, § 3.2–3.3, § 8) |
| Security, filesystem, network | [`docs/security_and_filtering.md`](docs/security_and_filtering.md) (§ 5–6) |
| Domain terms & invariants (`I-xx`) | [`context.md`](context.md) |
| Rust rules & `core`/`shell` boundary | [`docs/coding_convention.md`](docs/coding_convention.md) |
| CLI flags, exit codes, output | `src/core/cli.rs`, `src/shell/cli.rs` (`run_cli`), `src/core/render/`, `tests/cli.rs` ↔ [`README.md`](README.md) |
| Contribution & PR checklist | [`CONTRIBUTING.md`](CONTRIBUTING.md) ↔ `.github/PULL_REQUEST_TEMPLATE.md` (if checklist changes) |
| Vulnerability reporting | [`SECURITY.md`](SECURITY.md) |
| Observable behavior change | [`CHANGELOG.md`](CHANGELOG.md) (`## [Unreleased]`) |
