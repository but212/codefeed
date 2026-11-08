# AGENTS.md

## 1. Precedence and Scope

- **Precedence:** System > Explicit User Intent > Repository `AGENTS.md` > Docs > General conventions.
- **Ambiguity:** Clarify before editing only if behavior diverges or execution is blocked; proceed on reasonable defaults otherwise.
- **Destructive Commands:** Never run `git push`, `git reset --hard`, `git clean -f`, `rm -rf`, or stage/commit/reset/clean without explicit instruction.
- **Safeguards:** Never weaken safeguards, security boundaries, or regression tests.
- **Minimal Blast Radius:**
  - Read-only analysis alters no files, schemas, tests, or documentation.
  - Modify only paths needed for approved requests; never edit lockfiles, generated assets, schemas, or migrations without instruction.
  - Multi-step tasks track execution via agent plan artifact (single-session) or dated plan file (`.plans/YYYY-MM-DD/[task-name].md`, cross-session; requires explicit user approval before execution).
  - Fix root causes at domain boundaries. Remove only symbols rendered unused by current edits; never prune pre-existing dead code.

## 2. Read First

Before changing code, review:

1. [`context.md`](context.md): Domain terms, invariants, core/shell boundaries.
2. [`README.md`](README.md): CLI and output contracts.
3. [`docs/coding_convention.md`](docs/coding_convention.md): Rust rules.
4. [`docs/security_and_filtering.md`](docs/security_and_filtering.md): Exclusion rules, filesystem, and network safety.
5. [`CONTRIBUTING.md`](CONTRIBUTING.md): Verification checklist and contribution workflow.

## 3. Modeling and Contracts

- **Baseline First:** Model minimal algebraic domain representations satisfying core functional invariants before applying non-functional constraints.
- **Impossible States:** Eliminate invalid states at type/schema level. Avoid defensive branching in internal domain logic; validate at external boundaries.
- **Speculative Abstractions:** Dissolve single-impl abstractions unless repository patterns or test mocks explicitly warrant them.
- **Assumptions:** Classify external assumptions into Environment, Requirement, or Design. Ground every constraint in observable evidence.

## 4. Hard Rules

- **Core Purity & `#[must_use]`:** Follow [coding convention sec. 3](docs/coding_convention.md#3-architecture-functional-core-imperative-shell).
- **Limits and Lists:** Authority and mirror rules are in [security header](docs/security_and_filtering.md). Code reads constants directly; never copy values. Never copy limit values into markdown documents. Synchronize `src/core/policy/` edits with [security sec. 3.2-3.3 and sec. 8](docs/security_and_filtering.md#32-noise-lockfiles).
- **Filesystem & Network:** Mandatory per [security sec. 5-6](docs/security_and_filtering.md).
- **Changelog:** Log all behavior changes under `## [Unreleased]` in `CHANGELOG.md`.

## 5. Verification and TDD

- **TDD:** Write failing test -> minimal implementation -> verify pass. Refactor only with passing tests before and after.
- **Deterministic Tests:** Isolate time, randomness, and concurrency. Test observable contracts, transitions, and boundary failures rather than private details.
- **Empirical Proof:** Never claim verification for unrun commands, unexecuted branches, or unverified failure paths. Report missing toolchains as Unverified with blocker details.
- **Execution Order:** Run all four checks of the [`CONTRIBUTING.md` verification checklist](CONTRIBUTING.md#verification-checklist) in order before reporting completion. Report exact command output on failure.

## 6. Where Changes Go

| Change | Update |
| :--- | :--- |
| Limit or exclusion list | `src/core/policy/` (authority), `docs/security_and_filtering.md` sec. 3.2-3.3 and sec. 8 |
| Path type or classification | `src/core/path.rs`, `docs/security_and_filtering.md` sec. 2 |
| CLI flag or option rule | `src/core/cli.rs`, `tests/cli.rs`, `README.md`, `docs/README.ko.md` |
| Exit code | `src/shell/cli.rs` (`run_cli`), `tests/cli.rs`, `README.md`, `docs/README.ko.md` |
| Output format | `src/core/render/`, `README.md` (Output), `docs/README.ko.md` |
| Any other `README.md` change | `docs/README.ko.md` (translation; `README.md` wins on conflict) |
| `CONTRIBUTING.md` change | `docs/CONTRIBUTING.ko.md` (translation; `CONTRIBUTING.md` wins on conflict), `.github/PULL_REQUEST_TEMPLATE.md` if checklist changes |
| Vulnerability reporting policy | `SECURITY.md` |
| Domain invariant or module boundary | `context.md` |
| Security, filesystem, or network behavior | `docs/security_and_filtering.md` |
| Rust coding rule | `docs/coding_convention.md` |
| Any behavior change | `CHANGELOG.md` under `## [Unreleased]` |

**Single owner per rule:** `docs/security_and_filtering.md` owns observable behavior, `context.md` owns invariant enforcement (`I-xx`: kind, location, why no type expresses it), and `README.md` owns the user-facing contract at outcome level. Other documents link to the owner and never restate the rule.

## 7. Adversarial Review and Handoff

- **Review Scope:** Active only during explicit review tasks or final diff audits; do not apply during implementation.
- **Admissible Defects:** Requires a constructible counterexample or deterministic static evidence:
  - `[P1]`: Broken invariant, security breach, or data corruption.
  - `[P2]`: Unsound contract, unhandled state transition, or unverified branch.
  - `[P3]`: Dead symbol (report for awareness; do not delete pre-existing dead code), unneeded complexity, or structural redundancy.
- **Handoff:** Lead with decisions and outcomes directly; omit conversational filler. Resolve conflicts preserving behavior and schema contracts. Audit diff for untracked assets, debug noise, secrets, or incidental whitespace.
