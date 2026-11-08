# Coding Conventions

**Scope:** Rust rules, architecture, and coupling. Domain behavior is in [`context.md`](../context.md); security, filesystem, and network rules are in [`security_and_filtering.md`](security_and_filtering.md); verification commands are in [`CONTRIBUTING.md`](../CONTRIBUTING.md#verification-checklist).

Rules tagged with `(clippy: <lint>)` or `(rustc: <lint>)` are enforced in CI with `-D warnings`.

---

## 1. Naming

Follow [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/naming.html) (`C-CASE`, `C-GETTER`, `C-CTOR`, `C-ITER`; RFC 430):

- **Predicates:** Prefix with `is_` or `has_`; avoid negated names (`is_empty`, not `is_not_empty`).
- **Abbreviations:** Standard only (`id`, `url`, `len`, `src`, `dst`, `err`). Spell out domain terms (`extension`, not `ext`).
- **Scope Duplication:** Avoid redundant prefixes (`cli::parse`, not `cli::parse_cli`).
- **Mutation:** `_mut`, `set_`, and `iter_mut` belong in `src/shell/`; `src/core/` has no `&mut` parameters outside sec. 3.1 exceptions.

## 2. Conversions

Follow `C-CONV` and `C-CONV-TRAITS`. Prefer `From`, `TryFrom`, `AsRef`, `AsMut` over ad-hoc methods. Implement conversions on the most specific invariant-bearing type (`str::as_bytes`). Non-panicking unwrap uses `into_inner()`.

---

## 3. Architecture (Functional Core, Imperative Shell)

### 3.1 `src/core/` (Functional Core)

- Referentially transparent: no I/O, clock, environment, or network state. Never imports `crate::shell`.
- State transitions consume ownership: `self -> Self` or linear typestates; `&mut self` is forbidden.
- Forbidden: `std::fs`, `std::net`, `std::env`, `SystemTime`, `eprintln!` (clippy: `disallowed-macros`), interior mutability (`Cell`, `RefCell`, `Atomic*`, `Mutex`, `RwLock`), `static mut`, and `unsafe` (rustc: `forbid(unsafe_code)`).
- Deterministic parameters: pass explicitly (`retry_delay(attempt)`), never via ambient state.
- Permitted exceptions:
  1. `Display::fmt(&self, f: &mut Formatter<'_>)`.
  2. Local `let mut` variables and owned value parameters (`mut self`, `mut val`).
  3. Internal dependency buffers unobservable through APIs (e.g. `ignore::gitignore::Gitignore` buffer pool in `policy/exclude.rs`).

### 3.2 `src/shell/` (Imperative Shell)

- Encapsulates all I/O, network calls, filesystem access, and process boundaries.
- Drives candidate traversal, worker reads, and settlement via `Selector::observe` in walk order.

### 3.3 Type Invariants

Parse untrusted input into valid-by-construction types at the boundary; make invalid states unrepresentable:

- **Types over Sentinels:** Enums and newtypes (`OutputTarget::Stdout`, not `"-"`). Parse CLI inputs, URLs, and headers once at ingress into `GitRef`, `Source`.
- **Linear Typestates:** Transitions consume predecessor by value (`Selector` -> `CollectionResult` -> `RenderedDigest` -> `IngestReport`).
- **`#[must_use]`:** Required on phase types (`Selector`, `CollectionResult`, `RenderedDigest`, `BoundedBuffer`, `SortedFiles`) and pure non-mutating query functions; omitted on `Result`-returning functions (clippy: `double_must_use`).
- **Invariant Wrappers:** Encapsulate ordering or structural invariants in types with immutable interfaces (`SortedFiles`).
- **Limit Relations:** Enforce with `const _: () = { assert!(...); };`, never runtime tests (exception: runtime comparison against mirrored table in [`security_and_filtering.md`](security_and_filtering.md) sec. 8).

### 3.4 Cohesion and Coupling

A module is a file or directory directly below `src/core/` or `src/shell/`. Precedence on conflict: purity (sec. 3.1) > coupling > cohesion > size.

#### Coupling

- **Acyclic:** Dependency graph has no cycles. Imports via parent re-exports depend on the parent.
- **Import Definitions:** Import items from their defining module, not parent re-exports (which exist solely for public API).
- **No `super::super`:** Use `crate::` paths.
- **Single Owner:** Each rule or limit is defined in one module ([`security_and_filtering.md`](security_and_filtering.md)).
- **Hub Stability:** Modules imported by many files hold data, constants, and pure functions, importing only same- or lower-level modules.
- **Fan-Out Threshold:** >5 internal modules in `src/core/`, or >6 in `src/shell/`, prompts review.

#### Cohesion

- **Single Responsibility:** A module's entry in [`context.md`](../context.md) sec. 5 describes it cleanly. Split when public items divide into disjoint caller sets with no cross-references.
- **Invariants with Type:** Constructors and validations reside in the type's module.
- **Single-Caller Items:** `pub(crate)`/`pub(super)` items with one caller file should move to that caller (exempt: `src/core/policy/`, same-directory helpers, shell calls to core, `src/lib.rs` exports).
- **Minimal Visibility:** Private by default; unneeded `pub` caught by rustc `unreachable_pub`.
- **File Size Threshold:** >400 non-test lines prompts review.

#### Checks

- Direction, `super::super`, visibility: `grep` over `src/`.
- Cycles and fan-out: Import graph from `use` statements, `mod` declarations, and relative paths.

---

## 4. Dependencies

Prefer the standard library. Every `Cargo.toml` dependency requires an explanatory rationale comment.

---

## 5. Errors and Panics

- Structured domain errors ([`context.md`](../context.md) sec. 2); no stringly-typed errors on public APIs.
- Shell decorates I/O errors with the associated path or operation.
- No `unwrap()` (clippy: `unwrap_used`; tests exempt). `expect()` is permitted solely for provably unreachable invariant branches with explanatory messages.

---

## 6. Documentation and Comments

### 6.1 Rustdoc

- `///` for items; `//!` for modules and crate root.
- First line: single third-person declarative sentence ending with a period. English only.
- Public items must be documented (rustc: `missing_docs`).

### 6.2 Required Sections

- **`# Errors`:** Public functions returning `Result` (clippy: `missing_errors_doc`).
- **`# Panics`:** Functions capable of panicking (clippy: `missing_panics_doc` for public functions).
- **`# Examples`:** Public APIs; verified as doctests (`no_run` when external state is required).

### 6.3 Guidelines

- Public docs use intra-doc links (`` [`Type`] ``); never link to private items.
- Comments explain rationale, not syntax. Number multi-phase algorithm steps. Never commit dead code.

---

## 7. Testing

- **Table-Driven Tests:** Aggregate cases into `cases = [...]` slices over multiple one-off functions.
- **Contract over Wording:** Assert exact text only for README-specified output (summary line, digest bytes, exit codes). For other errors, assert unique variant or fragment.
- **Property Tests:** Open input spaces (escaping, sanitization, path normalization, exclude patterns) use `proptest` with `crate::core::proptest_config()` (fixed seed, no persistence) in `core` `properties` modules alongside example tables.
- **In-Memory Limits:** Validate `MAX_FILE_COUNT` and `MAX_OUTPUT_SIZE` via `Selector` and `CandidateBudget` without disk I/O.
- **Filesystem Isolation:** Shell tests use `crate::shell::testing::temp_dir`.
- **Network Mocking:** Mock HTTP via local `TcpListener` on `127.0.0.1:0` (`MockServer`); external network calls are forbidden.
