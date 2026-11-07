# Coding Conventions

**Scope:** Rust rules, architecture, and coupling. Domain behavior is in [`context.md`](../context.md); security, filesystem, and network rules are in [`security_and_filtering.md`](security_and_filtering.md); verification commands are in [`AGENTS.md`](../AGENTS.md) sec. 5.

A tag such as `(clippy: unwrap_used)` names the lint that enforces the rule. Lints are configured in `src/lib.rs`, `src/main.rs`, and `clippy.toml`; CI runs them with `-D warnings`.

---

## 1. Naming

Follow the [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/naming.html) (`C-CASE`, `C-GETTER`, `C-CTOR`, `C-ITER`; RFC 430), plus:

- **Predicates:** Prefix with `is_` or `has_`; no negated identifiers (`is_empty`, not `is_not_empty`).
- **Abbreviations:** Standard only (`id`, `url`, `len`, `src`, `dst`, `err`). Domain terms are spelled out (`extension`, not `ext`).
- **Scope duplication:** Avoid it (`cli::parse`, not `cli::parse_cli`).
- **Mutating names:** `_mut`, `set_`, and `iter_mut` exist only in `src/shell/`; `src/core/` has no `&mut` (sec. 3.1).

## 2. Conversions

Follow `C-CONV` (`as_` free, `to_` expensive, `into_` consuming) and `C-CONV-TRAITS`: prefer `From`, `TryFrom`, `AsRef`, `AsMut` over ad-hoc methods. Implement a conversion on the most specific invariant-bearing type (`str::as_bytes`). Non-panicking extraction uses `into_inner()` or `into_foo()`.

---

## 3. Architecture (Functional Core, Imperative Shell)

### 3.1 `src/core/` (Functional Core)

- Referentially transparent: no I/O, clock, environment, or network state. `src/core/` imports nothing from `crate::shell`.
- State transitions consume ownership: `self -> Self` or linear typestates. `&mut self` is forbidden.
- Forbidden: `std::fs`, `std::net`, `std::env`, `SystemTime`, `eprintln!` (clippy: `disallowed-macros`), interior mutability (`Cell`, `RefCell`, `Atomic*`, `Mutex`, `RwLock`), `static mut`, and `unsafe` (rustc: `forbid(unsafe_code)`).
- Deterministic parameterization: pass parameters directly (`retry_delay(attempt)`), never via ambient state.
- Permitted exceptions:
  1. `std::fmt::Display::fmt(&self, f: &mut Formatter<'_>)`.
  2. Local `let mut` variables and owned value parameters (`mut self`, `mut val`). `&mut` reference parameters are forbidden except in `Display::fmt`.
  3. Interior mutability inside a dependency that is not observable through its API, such as the matching buffer pool of `ignore::gitignore::Gitignore` used by `policy/exclude.rs`. Core code must not add any.

### 3.2 `src/shell/` (Imperative Shell)

- Houses all I/O, network calls, filesystem interactions, and process boundaries.
- Traverses candidates, executes worker reads, and settles candidates via `Selector::observe` in walk order.

### 3.3 Type Invariants

Parse, don't validate ([`AGENTS.md`](../AGENTS.md) sec. 3). Rust forms:

- **Types over sentinels:** enums and newtypes (`OutputTarget::Stdout`, not `"-"`). Parse CLI inputs, URLs, and headers once at ingress into `GitRef`, `Source`.
- **Linear typestates:** phase transitions consume the predecessor by value (`Selector` -> `CollectionResult` -> `RenderedDigest` -> `IngestReport`).
- **`#[must_use]`:** Required on phase types (`Selector`, `CollectionResult`, `RenderedDigest`, `BoundedBuffer`, `SortedFiles`) and pure non-mutating query functions. Not on functions returning `Result` (clippy: `double_must_use`).
- **Invariant wrappers:** Encapsulate ordering or invariants in types with immutable interfaces (`SortedFiles`).
- **Limit relations:** Enforce with `const _: () = { assert!(...); };`, never runtime tests. Exception: a runtime test may compare limit constants with the mirrored values in [`security_and_filtering.md`](security_and_filtering.md) sec. 8, which `const` assertions cannot read.

### 3.4 Cohesion and Coupling

A module is a file or a directory directly below `src/core/` or `src/shell/`. The root `mod.rs` of each is a module of its own. When rules conflict, precedence is: purity (sec. 3.1), coupling, cohesion, size.

**Coupling**

- **Acyclic:** The module dependency graph has no cycles. An import through a parent re-export is a dependency on the parent.
- **Import the definition:** Import an item from the module that defines it, not from a parent re-export. A re-export in a root `mod.rs` exists for the public API only.
- **No `super::super`:** Reach other modules by `crate::` path.
- **Single owner:** Each rule or limit is defined in one module. Limits are defined in `src/core/policy/` only ([`AGENTS.md`](../AGENTS.md) sec. 4).
- **Hub stability:** A module imported by many files holds data, constants, and pure functions, and imports only modules at the same or lower level.
- **Fan-out (review threshold):** More than 5 internal modules imported by one file in `src/core/`, or more than 6 in `src/shell/`, counting non-test code. Exceeding it prompts a review; it is not a defect by itself.

**Cohesion**

- **One responsibility:** A module's entry in [`context.md`](../context.md) sec. 5 describes it without joining unrelated nouns. Split test: the public items fall into two groups whose caller sets are disjoint and which do not reference each other.
- **Invariant with its type:** A type's constructor and validation are in the type's module (sec. 3.3).
- **Single-caller items:** A `pub(crate)` or `pub(super)` item with exactly one external caller file is a candidate to move to that caller. Exempt: items in `src/core/policy/`, items used only within the same directory, core items used by shell, and public API re-exports in `src/lib.rs`.
- **Minimal visibility:** An item referenced only within its own file is private. Unneeded `pub` is caught by rustc `unreachable_pub`.
- **Size (review threshold):** More than 400 non-test lines in one file prompts a review.

**Checks**

- Direction, `super::super`, and visibility: `grep` over `src/`.
- Cycles and fan-out: import graph built from `use` paths, `mod` declarations, and relative-path calls (`local::collect`). Name matching alone yields false positives.

---

## 4. Dependencies

Prefer the standard library. Every dependency in `Cargo.toml` requires an explanatory rationale comment.

---

## 5. Errors and Panics

- Structured domain errors ([`context.md`](../context.md) sec. 2). No stringly-typed errors on public APIs.
- Shell decorates I/O errors with the associated path or operation.
- No `unwrap()` on any path (clippy: `unwrap_used`; tests exempt via `clippy.toml`). `expect()` is permitted solely for provably unreachable invariant branches, with an explanatory message.

---

## 6. Documentation and Comments

### 6.1 Rustdoc

- `///` for items; `//!` for modules and crate roots.
- First line is a single third-person declarative sentence ending with a period ("Returns ...", "Parses ...").
- Written in English. Public items are documented (rustc: `missing_docs`).

### 6.2 Required Sections

- **`# Errors`:** All public functions returning `Result` (clippy: `missing_errors_doc`).
- **`# Panics`:** Any function capable of panicking; clippy: `missing_panics_doc` covers public functions only.
- **`# Examples`:** Public APIs; run as doctests (`no_run` when external state is required).

### 6.3 Guidelines

- Public docs use intra-doc links (`` [`Type`] ``) and never link to private items.
- Implementation comments explain rationale, not visible syntax. Number multi-phase algorithm steps. Never leave commented-out code.

---

## 7. Testing

- **Table-driven tests:** Aggregate cases into `cases = [...]` slices over multiple one-off functions.
- **Contract over wording:** Assert exact text only for output the README specifies (summary line, digest bytes, exit codes). For other errors, assert one fragment unique to the failing branch, or the variant.
- **Property tests:** Core invariants over open input spaces (escaping, sanitization, path normalization, exclude patterns) use `proptest` with `crate::core::proptest_config()` (fixed seed, no failure persistence). They live in `core` only, in a `properties` module beside the unit tests, and keep a table of examples for the cases a property cannot distinguish.
- **In-memory limits:** Validate `MAX_FILE_COUNT` and `MAX_OUTPUT_SIZE` via `Selector` and `CandidateBudget` without disk I/O.
- **Filesystem isolation:** Shell tests use `crate::shell::testing::temp_dir`.
- **Network mocking:** Mock HTTP via local `TcpListener` bound to `127.0.0.1:0` (`MockServer`). External network calls in tests are forbidden.
