# AGENTS.md

## 1. Precedence and Scope

- **Precedence:** System > Explicit User Intent > Repository `AGENTS.md` > Docs > General conventions.
- **Ambiguity:** Clarify before editing only if behavior diverges or execution is blocked; proceed on reasonable defaults otherwise.
- **Destructive Commands:** Never run `git push`, `git reset --hard`, `git clean -f`, `rm -rf`, or stage/commit/reset/clean without explicit user instruction.
- **Safeguards:** Never weaken safeguards, security boundaries, or regression tests.
- **Minimal Blast Radius:**
  - Read-only analysis alters no files, schemas, tests, or documentation.
  - Modify only paths needed for approved requests; never edit lockfiles, generated assets, schemas, or migrations without instruction.
  - Multi-step tasks track execution via an agent plan artifact (single-session) or dated plan file (`.plans/YYYY-MM-DD/[task-name].md`, cross-session; requires explicit user approval before execution).
  - Fix root causes at domain boundaries. Remove only symbols rendered unused by the current edit; never prune pre-existing dead code.

## 2. Read First

Before changing code, read these documents:

1. [`context.md`](context.md): Domain terms, invariants, and functional-core / imperative-shell boundary.
2. [`README.md`](README.md): CLI and output contracts.
3. [`docs/coding_convention.md`](docs/coding_convention.md): Rust rules.
4. [`docs/security_and_filtering.md`](docs/security_and_filtering.md): Exclusion rules, filesystem, and network safety.

## 3. Modeling and Contracts

- **Baseline First:** Model minimal algebraic domain representations satisfying core functional invariants before applying non-functional constraints (caching, concurrency, optimizations).
- **Impossible States:** Eliminate invalid states at the type/schema level. Avoid defensive branching in internal domain logic; validate untrusted inputs at external boundaries.
- **Speculative Abstractions:** Dissolve single-impl abstractions unless repository patterns or test mocks explicitly warrant them.
- **Assumptions:** Classify external assumptions into Environment, Requirement, or Design. Ground every constraint in observable evidence.
- **SKILL.md Frontmatter:** Emit only single-line scalar strings or inline JSON arrays; never use multiline block scalars (`|`, `>`) or indented list items.

## 4. Hard Rules

- **Core Purity & `#[must_use]`:** Follow [coding convention sec. 3](docs/coding_convention.md#3-architecture-functional-core-imperative-shell).
- **Limits and Lists:** Defined only in `src/core/policy/` (authority). Code reads constants directly; never copy values. In documentation, numerical limit values appear exclusively in [security sec. 8](docs/security_and_filtering.md#8-limit-values) as a mirror; never copy limit values across other markdown documents. Synchronize `src/core/policy/` edits with [security sec. 3-4 and sec. 8](docs/security_and_filtering.md#3-exclusion-lists).
- **Filesystem & Network:** Mandatory per [coding convention sec. 5](docs/coding_convention.md#5-os-and-network-rules-at-the-code-level) and [security sec. 5-6](docs/security_and_filtering.md).
- **Changelog:** Log all behavior changes under `## [Unreleased]` in `CHANGELOG.md`.

## 5. Verification and TDD

- **TDD:** Write failing test -> minimal implementation -> verify pass. Refactor only with passing tests before and after.
- **Deterministic Tests:** Isolate time, randomness, and concurrency. Test observable contracts, transitions, and boundary failures rather than private details.
- **Empirical Proof:** Never claim verification for unrun commands, unexecuted branches, or unverified failure paths. If blocked by missing toolchains or network barriers, report checks as Unverified with blocker details.
- **Execution Order:** Run all four checks in this exact order before reporting completion:
  1. `cargo clippy --all-targets -- -D warnings`
  2. `cargo fmt --check`
  3. `cargo doc --no-deps`
  4. `cargo test` (or narrowest target first: `cargo test <name>`)
- Report failures with exact command output. Never claim unrun or failed steps passed.

## 6. Where Changes Go

| Change | Update |
| :--- | :--- |
| Limit, exclusion list, or domain type | `src/core/policy/` (authority), `docs/security_and_filtering.md` sec. 3-4 and sec. 8 |
| CLI flag or option-value rule | `src/core/cli.rs`, `tests/cli.rs`, `README.md` |
| Exit code | `src/shell/cli.rs` (`run_cli`), `tests/cli.rs`, `README.md` |
| Output format | `src/core/render/`, `README.md` (Output), `context.md` sec. 3.5 |
| Domain invariant or module boundary | `context.md` |
| Security, filesystem, or network behavior | `docs/security_and_filtering.md` |
| Rust coding rule | `docs/coding_convention.md` |
| Any behavior change | `CHANGELOG.md` under `## [Unreleased]` |

## 7. Adversarial Review and Handoff

- **Review Scope:** Active only during explicit review tasks or final diff audits; do not apply during implementation.
- **Admissible Defects:** Requires a constructible counterexample or deterministic static evidence:
  - `[P1]`: Broken invariant, security breach, or data corruption.
  - `[P2]`: Unsound contract, unhandled state transition, or unverified branch.
  - `[P3]`: Dead symbol (report for awareness; do not delete pre-existing dead code), unneeded complexity, or structural redundancy.
- **Handoff:**
  - Lead with decisions and outcomes directly; omit conversational filler and restatements.
  - Resolve integration conflicts by preserving behavior, schema contracts, and unrelated work.
  - Audit the final diff before handoff to guarantee absence of untracked assets, debug noise, secret leakage, or incidental whitespace churn.
