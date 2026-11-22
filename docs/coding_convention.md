# Coding Conventions

**Scope:** Rust rules, architecture, and coupling. Domain behavior is in [`context.md`](../context.md); security, filesystem, and network rules are in [`security_and_filtering.md`](security_and_filtering.md); verification commands are in [`CONTRIBUTING.md`](../CONTRIBUTING.md#verification-checklist).

**Governing Laws & Enforcement:**

- **CI Lints (`-D warnings`):** `src/lib.rs` (`forbid(unsafe_code)`, `missing_docs`, `unreachable_pub`, `clippy::unwrap_used`, `clippy::missing_errors_doc`, `clippy::missing_panics_doc`), `clippy.toml` (`disallowed-macros`, `allow-unwrap-in-tests`), and `RUSTDOCFLAGS="-D warnings" cargo doc`.
- **Baseline Principles:** Rust API Guidelines, Functional Core / Imperative Shell (FCIS), Parse Don't Validate (unrepresentable invalid states), acyclic SSOT modules without hollow indirection, and deterministic isolated tests.
- This document specifies only **project-specific contracts, thresholds, exceptions, and conventions not enforced by tooling**.

---

## 1. Naming

Follow [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/naming.html) (`C-CASE`, `C-GETTER`, `C-CTOR`, `C-ITER`; RFC 430). Spell out domain terms (`extension`, not `ext`).

## 2. Conversions

Follow `C-CONV` and `C-CONV-TRAITS` (`From`, `TryFrom`, `AsRef`, `AsMut`) on the most specific invariant-bearing type. Non-panicking unwrap uses `into_inner()`.

---

## 3. Architecture (Functional Core, Imperative Shell)

### 3.1 `src/core/` (Functional Core)

Referentially transparent and pure: no I/O, clock, environment, network, `crate::shell` imports, `&mut self`, interior mutability, or `static mut`. Pass parameters explicitly and transition state by value (`self -> Self`).

**Permitted exceptions:**

1. `Display::fmt(&self, f: &mut Formatter<'_>)`.
2. Local `let mut` variables and owned value parameters (`mut self`, `mut val`).
3. Internal dependency buffers unobservable through APIs (e.g. `ignore::gitignore::Gitignore` buffer pool in `policy/exclude.rs`).

### 3.2 `src/shell/` (Imperative Shell)

Encapsulates all I/O and process boundaries. Drives candidate traversal, worker reads, and settlement via `Selector::observe` in walk order.

### 3.3 Type Invariants

Parse untrusted input at the boundary into valid-by-construction types (enums/newtypes over sentinels, linear typestates, closed value transitions, immutable invariant wrappers).

- **`#[must_use]`:** Required on phase types (`Selector`, `CollectionResult`, `RenderedDigest`, `BoundedBuffer`, `SortedFiles`) and pure non-mutating query functions (omitted on `Result`; `clippy::double_must_use`).
- **Limit Relations:** Enforce via `const _: () = { assert!(...); };` (exception: runtime comparison against mirrored table in [`security_and_filtering.md`](security_and_filtering.md) § 8).

### 3.4 Cohesion and Coupling

A module is a direct child of `src/core/` or `src/shell/`. Precedence on conflict: purity (§ 3.1) > coupling > cohesion > size.

- **Coupling & Visibility:** Acyclic DAG, single rule owner ([`security_and_filtering.md`](security_and_filtering.md)), no hollow indirection (single-impl traits or pass-through wrappers in `src/core/`), and minimal visibility (`unreachable_pub`). Import from defining modules (not parent re-exports) and use `crate::` paths (no `super::super`).
- **Single-Caller Items:** Move `pub(crate)`/`pub(super)` items with one caller file into that caller (exempt: `src/core/policy/`, same-directory helpers, `shell` -> `core` calls, `src/lib.rs` exports).
- **Review Thresholds:** Fan-out `>5` internal modules (`src/core/`) or `>6` (`src/shell/`); file size `>400` non-test lines.
- **Checks:** `grep` over `src/` for direction, `super::super`, and visibility; import graph for cycles and fan-out.

---

## 4. Dependencies

Prefer `std`. Every `Cargo.toml` dependency requires an explanatory rationale comment.

---

## 5. Errors and Panics

- Structured domain errors ([`context.md`](../context.md) § 2); `shell` decorates I/O errors with the associated path or operation.
- `unwrap()` is forbidden outside tests (`clippy::unwrap_used`). `expect()` is permitted solely for provably unreachable invariant branches with explanatory messages.

---

## 6. Documentation and Comments

Enforced by `rustc` (`missing_docs`), `clippy` (`missing_errors_doc`, `missing_panics_doc`), and `cargo doc` (`-D warnings`).

- **Rustdoc Style:** `///` for items (`//!` for modules and crate root). First line is a single third-person declarative sentence ending with a period, English only.
- **Examples:** Public APIs require `# Examples` verified as doctests (`no_run` when external state is required).
- **Comments:** Explain non-obvious rationale, not syntax; number multi-phase algorithm steps; never commit dead code.

---

## 7. Testing

Prefer deterministic, table-driven tests (`cases = [...]`) asserting behavioral contracts (exact text only for README-specified output; unique variant or fragment otherwise).

- **Property Tests:** Open input spaces (escaping, sanitization, path normalization, exclude patterns) use `proptest` with `crate::core::proptest_config()` in `core` `properties` modules.
- **In-Memory Limits:** Validate `MAX_FILE_COUNT` and `MAX_OUTPUT_SIZE` via `Selector` and `CandidateBudget` without disk I/O.
- **Isolation & Mocking:** Shell tests use `crate::shell::testing::temp_dir`; mock HTTP via local `TcpListener` on `127.0.0.1:0` (`MockServer`), never external network calls.
