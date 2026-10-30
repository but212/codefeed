# Coding Conventions

**Scope:** Rust API Guidelines, project architecture, memory layout, and safety rules. Domain behavior is in [`context.md`](../context.md); security rules are in [`security_and_filtering.md`](security_and_filtering.md); verification commands are in [`AGENTS.md`](../AGENTS.md) sec. 5.

---

## 1. Naming

### 1.1 Case (`C-CASE`, RFC 430)

| Item | Convention | Example |
| :--- | :--- | :--- |
| Crates, modules, files | `snake_case` | `selector`, `storage` |
| Types, traits, enums, variants | `UpperCamelCase` | `FileRecord`, `Source::Github` |
| Functions, methods, variables | `snake_case` | `render_text`, `path_arena` |
| Constants and statics | `SCREAMING_SNAKE_CASE` | `MAX_FILE_COUNT` |
| Type parameters | Concise `UpperCamelCase` | `T`, `W: Write` |
| Lifetimes | Short lowercase | `'a`, `'de` |
| Cargo features | `kebab-case` | `platform-verifier` |

- **Acronyms:** Treated as single words in `UpperCamelCase` (`Uuid`, `HttpServer`, `GithubClient`); lowercase in `snake_case` (`sha`, `is_xid_start`).
- **Single letters:** Disallowed as stand-alone words in `snake_case` or `SCREAMING_SNAKE_CASE` except as the final suffix (`btree_map`, `PI_2`).

### 1.2 Methods and Types (`C-GETTER`, `C-CTOR`, `C-ITER`)

- **Getters:** Omit `get_` (`field(&self) -> &Field`). Reserve `get_` for key lookups (`get(&self, key)`).
- **Mutators:** Mutable getters use `_mut`; setters use `set_` (both shell-only; `&mut self` is forbidden in `src/core/`).
- **Constructors:** `new(...)` for default instantiation; descriptive prefixes (`with_capacity`) or conversion constructors (`from_utf8`) otherwise.
- **Iterators:** `iter(&self)`, `iter_mut(&mut self)` (shell-only), `into_iter(self)`.
- **Predicates:** Prefix with `is_` or `has_`; avoid negated identifiers (`is_empty`, not `is_not_empty`).
- **Extraction:** Non-panicking conversions use `into_inner()` or `into_foo()`; never `unwrap()`.
- **Style:** Standard abbreviations only (`id`, `url`, `len`, `src`, `dst`, `err`). Spelled-out domain terms (`extension`, not `ext`). Avoid scope duplication (`cli::parse`, not `cli::parse_cli`).

---

## 2. Conversions (`C-CONV`)

| Prefix | Cost | Ownership | Receiver | Semantics |
| :--- | :--- | :--- | :--- | :--- |
| `as_` | Free | Borrowed to borrowed | `&self` | Zero-cost borrowed view |
| `to_` | Expensive | Borrowed to owned, or `Copy` | `&self` | Allocation or heavy transform |
| `into_` | Variable | Owned to owned | `self` | Consumes receiver |

- Prefer standard traits (`From`, `TryFrom`, `AsRef`, `AsMut`) over ad-hoc conversion methods.
- Implement conversions on the most specific invariant-bearing type (`str::as_bytes`).

---

## 3. Architecture (Functional Core, Imperative Shell)

### 3.1 `src/core/` (Functional Core)

- Referentially transparent: no I/O, clock, environment, or network state.
- State transitions consume ownership: `self -> Self` or linear typestates. `&mut self` is strictly forbidden.
- Forbidden: `std::fs`, `std::net`, `std::env`, `SystemTime`, `eprintln!`, any `crate::shell` import, interior mutability (`Cell`, `RefCell`, `Atomic*`, `Mutex`, `RwLock`), `static mut`, and `unsafe`.
- Deterministic parameterization: pass parameters directly (`retry_delay(attempt)`), never via ambient state.
- Permitted exceptions:
  1. `std::fmt::Display::fmt(&self, f: &mut Formatter<'_>)`.
  2. Local `let mut` variables and owned value parameters (`mut self`, `mut val`). `&mut` reference parameters are forbidden except in `Display::fmt`.

### 3.2 `src/shell/` (Imperative Shell)

- Houses all I/O, network calls, filesystem interactions, and process boundaries.
- Traverses candidates, executes worker reads, and settles candidates via `Selector::observe` in walk order.

### 3.3 Type Invariants ("Parse, Don't Validate")

- **Make invalid states unrepresentable:** Use enums and newtypes instead of booleans, sentinels, or raw strings (`OutputTarget::Stdout`, not `"-"`).
- **Validate at boundaries:** Parse CLI inputs, URLs, and headers into domain types (`GitRef`, `Source`) once at ingress.
- **Linear typestates:** Phase transitions consume predecessor states by value (`Selector` -> `CollectionResult` -> `RenderedDigest` -> `IngestReport`).
- **`#[must_use]`:** Required on phase types (`Selector`, `CollectionResult`, `RenderedDigest`, `BoundedBuffer`, `SortedFiles`) and pure non-mutating query functions. Disallowed on functions returning `Result` (where it is redundant).
- **Invariant wrappers:** Encapsulate ordering or invariants in types with immutable interfaces (`SortedFiles`).
- **Compile-time assertions:** Enforce limit relations via `const _: () = { assert!(...); };`.

---

## 4. Memory, Layout, and Dependencies

- **Arena storage:** Contiguous arenas indexed by 16-byte `FileSpan`. Iteration yields `FileRecord<'a>`. Sorting swaps spans, not payload bytes.
- **Direct layout:** Prefer `Vec`, `HashMap`, and flat structs over pointer-heavy trees.
- **Bounded rendering:** Memory buffering is strictly capped by `BoundedBuffer`. Core rendering performs no I/O.
- **Minimal dependencies:** Prefer standard library. Every dependency in `Cargo.toml` requires an explanatory rationale comment.

---

## 5. OS and Network Rules

- Verify candidate files with `fs::symlink_metadata` before `fs::File::open`. Never follow symlinks.
- Thread channels must be bounded (`sync_channel`).
- Atomic writes: write exclusively via `write_atomic`; the file handle closes before `TempPath` unlinks on `Drop`.
- HTTP redirects enforce `RedirectAuthHeaders::Never`. Transmit `Authorization` headers strictly when `is_github_api_origin(url)` holds.

---

## 6. Errors and Panics

- Structured domain errors: `DomainError` in core, `IngestError` in shell. No stringly-typed errors on public APIs.
- Shell decorates I/O errors with the associated path or operation.
- No `unwrap()` or `expect()` on user-reachable paths. `expect()` is permitted solely for provably unreachable invariant branches with an explanatory message.

---

## 7. Documentation and Comments

### 7.1 Rustdoc

- `///` for items; `//!` for modules and crate roots.
- First line is a single third-person declarative sentence ending with a period ("Returns ...", "Parses ...").
- Written in English. Enforce `#![warn(missing_docs)]` on public items.

### 7.2 Required Sections

- **`# Errors`:** Mandatory on all public functions returning `Result`.
- **`# Panics`:** Mandatory on any function capable of panicking.
- **`# Safety`:** Mandatory on any `unsafe` block or function (the crate has none).
- **`# Examples`:** Required on public APIs; run as doctests (`no_run` when external state is required).

### 7.3 Guidelines

- Public docs use intra-doc links (`` [`Type`] ``) and never link to private items.
- Implementation comments explain rationale, not visible syntax. Number multi-phase algorithm steps. Never leave commented-out code.

---

## 8. Testing

- **Table-driven tests:** Aggregate cases into `cases = [...]` slices over multiple one-off functions.
- **In-memory limits:** Validate `MAX_FILE_COUNT` and `MAX_OUTPUT_SIZE` via `Selector` and `CandidateBudget` without disk I/O.
- **Filesystem isolation:** Use `crate::shell::testing::temp_dir` in shell tests; core unit tests never touch the disk.
- **Network mocking:** Mock HTTP via local `TcpListener` bound to `127.0.0.1:0` (`MockServer`). External network calls in tests are forbidden.
- **Limit checks:** Validate relations via `const _` assertions, never runtime tests.
