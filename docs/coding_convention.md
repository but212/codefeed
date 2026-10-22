# Coding Conventions

**Audience:** contributors writing Rust in this repository.

**Scope:** Rust API Guidelines plus this project's architecture, memory, and safety rules. Domain behavior is in [`context.md`](../context.md). Filter and safety rules are in [`security_and_filtering.md`](security_and_filtering.md). Verification commands are in [`AGENTS.md`](../AGENTS.md) sec. 2.

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

- **Acronyms** count as one word in `UpperCamelCase`: `Uuid`, `HttpServer`, `GithubClient`. In `snake_case` they are lowercase: `sha`, `is_xid_start`.
- **Single letters** are not used as words in `snake_case` or `SCREAMING_SNAKE_CASE`, except as the last word: `btree_map`, `PI_2`.

### 1.2 Methods and Types (`C-GETTER`, `C-CTOR`, `C-ITER`)

- Getters omit `get_`: `field(&self) -> &Field`. Use `get_` only for lookup by key: `get(&self, key)`.
- Mutable getters use `_mut`. Setters use `set_`. Both are shell-only, because `&mut self` is forbidden in `src/core/`.
- Constructors: `new(...)` for the default constructor. Others use descriptive prefixes (`with_capacity`). Conversion constructors use `from_...` (`from_utf8`).
- Iterators: `iter(&self)`, `iter_mut(&mut self)` (shell-only), and `into_iter(self)`.
- Predicates use a descriptive prefix (`is_empty`, `has_children`). Avoid negative names: `is_empty`, not `is_not_empty`.
- Non-panicking extraction uses `into_inner()` or `into_foo()`, never `unwrap()`.
- Use established abbreviations only: `id`, `url`, `len`, `src`, `dst`, `err`. Domain terms are spelled out: `extension`, not `ext`.
- Do not truncate identifiers until they lose meaning (`is_valid_path`, not `chk_vld_pt`). Do not repeat the enclosing scope (`cli::parse`, not `cli::parse_cli`).

---

## 2. Conversions (`C-CONV`)

| Prefix | Cost | Ownership | Receiver | Meaning |
| :--- | :--- | :--- | :--- | :--- |
| `as_` | Free | Borrowed to borrowed | `&self` | Zero-cost borrowed view |
| `to_` | Expensive | Borrowed to owned, or `Copy` | `&self` | Allocation or heavy transformation |
| `into_` | Variable | Owned to owned | `self` | Consumes the receiver |

- Implement `From`, `TryFrom`, `AsRef`, and `AsMut` for canonical conversions, instead of custom names.
- Define conversions on the most specific type that carries the invariant: `str::as_bytes`, not a slice method.

---

## 3. Architecture (Functional Core, Imperative Shell)

The module map is in [`context.md`](../context.md) sec. 5. This section lists only the coding rules.

### 3.1 `src/core/` (Functional Core)

- Functions are referentially transparent. The core performs no I/O and reads no time, environment, or network state.
- State changes use consuming ownership: `self -> Self`, or linear typestates. `&mut self` is forbidden.
- Forbidden: `std::fs`, `std::net`, `std::env`, `SystemTime`, `eprintln!`, any `crate::shell` import, interior mutability (`Cell`, `RefCell`, `Atomic*`, `Mutex`, `RwLock`), `static mut`, and `unsafe`.
- Pass parameters as deterministic values: `retry_delay(attempt)`, not a global counter.
- The only exceptions are:
  1. `std::fmt::Display::fmt(&self, f: &mut Formatter<'_>)`.
  2. Local `let mut` variables and owned value parameters (`mut self`, `mut val`). Any `&mut` reference parameter is forbidden, except in `Display::fmt`.

### 3.2 `src/shell/` (Imperative Shell)

- All I/O, network calls, filesystem access, and process interaction live here.
- The shell collects inputs, settles each candidate with `Selector::observe`, and delegates computation and storage to the core.
- Local collection reads files on worker threads and settles results in walk order through bounded channels. The pipeline is described in [context sec. 3.8](../context.md#38-local-traversal-pipeline). Only the main thread calls into the pure selector.

### 3.3 Invariants by Type ("Parse, Don't Validate")

- **Invalid states are unrepresentable.** Use enums, newtypes, and typestates instead of boolean flags, sentinel values, or raw strings. `OutputTarget::Stdout` replaces a `"-"` sentinel.
- **Validate at the boundary.** Parse CLI flags, URLs, and archive headers into domain values (`GitRef`, `Source`) once. Core code relies on that type and does not re-check.
- **Linear typestates.** Multi-phase pipelines are consuming chains (`Selector` -> `CollectionResult` -> `RenderedDigest` -> `IngestReport`). Each phase takes its predecessor by value, so a state cannot be reused after a phase. A phase sequence owned by one function is a plain `match` inside it.
- **`#[must_use]`.** Put it on phase types (`Selector`, `CollectionResult`, `RenderedDigest`, `BoundedBuffer`, `SortedFiles`) and on pure functions whose return value is the point of the call, such as predicates, classification, the retry delay, and the origin gate. Do not add it to a function that returns `Result`, because `Result` is already `#[must_use]` and the attribute would be redundant.
- **Invariant wrappers.** Ordering or layout guarantees get a wrapper type. `SortedFiles` guarantees path order, and it has no mutating methods.
- **Compile-time checks.** Relations between limits are enforced with `const _: () = { assert!(...); };`. Lookup tables are computed at compile time (`[bool; 256]`).

---

## 4. Memory, Data Layout, and Dependencies

- **Arena storage.** Paths and contents live in contiguous arenas indexed by 16-byte `FileSpan`. Iteration yields zero-copy `FileRecord<'a>`. Sorting swaps spans, not strings.
- **Direct structures.** Use `Vec`, `HashMap`, and plain structs. Avoid speculative packing and pointer-heavy trees. Measure before tuning.
- **Bounded rendering.** Output goes into `BoundedBuffer`, which returns `RenderedDigest`. Escaping is a single pass (`write_escaped`). Core rendering performs no I/O.
- **Std first.** Add an external crate only when in-tree code would be disproportionately complex or risky. Each dependency in `Cargo.toml` has an inline comment that gives its rationale. A crate already in the graph through required dependencies may be used directly when that is justified, with the same comment.

---

## 5. OS and Network Rules at the Code Level

The reasoning and threat model are in [`security_and_filtering.md`](security_and_filtering.md) sec. 5-6. These are the code-level rules:

- Check candidate files with `fs::symlink_metadata` before `fs::File::open`. Never follow symlinks.
- Channels between threads must be bounded (`sync_channel`).
- Write files only through `AtomicFile`. Its `TempFile` guard closes the OS handle before unlinking on `Drop`.
- HTTP redirects use `RedirectAuthHeaders::Never`. The `Authorization` header is attached only when `is_github_api_origin(url)`.

---

## 6. Errors and Panics

- Use domain error enums. `DomainError` is in `core`. `IngestError` is in `shell`. Do not use stringly-typed errors on public paths.
- The shell wraps I/O errors with the path or operation that failed before they reach the user.
- Never call `unwrap()` or `expect()` on user input or on a path that a user can reach. `expect()` is allowed only for an impossible invariant, with a message that states the invariant. `unwrap()` is allowed in tests and assertions.

---

## 7. Documentation and Comments

### 7.1 Rustdoc

- `///` documents items. `//!` documents modules and the crate root.
- The first line is a single sentence in the third person, ending with a period: "Returns ...", "Parses ...". Not "Return ..." or "This function ...".
- After the summary, leave a blank line before the contract, context, or invariants.
- Documentation is in English.

### 7.2 Sections

- **`# Errors`**: Required on every public function (`pub` item re-exported from `lib.rs`) that returns `Result`. List each failure mode and the error variant it produces. Crate-internal functions state their failure conditions in one line when those conditions are not obvious from the type.
- **`# Panics`**: Required on any function that can panic.
- **`# Safety`**: Required on any `unsafe` item. The crate has none.
- **`# Examples`**: Required on public API where a usage example helps. Examples are doctests. Use `no_run` when an example needs external state.

### 7.3 Visibility

- **Public API** (`pub` in `lib.rs`): summary, contract, `# Errors`, `# Examples` where useful. `#![warn(missing_docs)]` is enforced.
- **Crate-internal** (`pub(crate)`): one-line summary, plus notes on invariants and boundary assumptions.
- **Private**: name it so it explains itself. Comment only non-trivial bit operations, parsers, and security policies.
- **Links**: Use intra-doc links (`` [`Type`] ``). Public docs must not link to private items.

### 7.4 Implementation Comments

- Explain why, not what: protocol quirks, RFC requirements, performance decisions.
- Number the steps of a multi-phase algorithm (`// 1. PAX global header: ...`).
- Never leave commented-out code in the repository.

---

## 8. Testing

- **Table-driven fixtures.** Prefer one test over a slice of `(input, expected)` rows, such as `let cases = [...]`, over a test function per case.
- **In-memory limits.** Test `MAX_FILE_COUNT` and `MAX_OUTPUT_SIZE` through `Selector` and `CandidateBudget`. Do not write millions of files to disk.
- **Filesystem tests.** Inside `src/shell/`, use `crate::shell::testing::temp_dir("label")`, which returns a `TempDirGuard`. Integration tests in `tests/` use a local `TempDir` helper, as `tests/cli.rs` does. Core code is pure, so core unit tests do not touch the filesystem.
- **Network tests.** Mock HTTP with a local `TcpListener` bound to `127.0.0.1:0`, using the `MockServer` test helper in `src/shell/github.rs`. Tests never reach external endpoints.
- **Compile-time limits.** Relations between limits are checked by `const _` assertions, not by runtime tests.
