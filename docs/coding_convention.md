# Coding Conventions

Project-wide coding standards and API design principles for `codefeed`. Combines Rust API Guidelines with project architecture, memory, and safety contracts.

---

## 1. Naming & Case Conventions (Rust API Guidelines: `C-CASE`, `C-GETTER`, `C-CTOR`, `C-ITER`)

### Case Conventions (`C-CASE`, RFC 430)

| Item | Convention | Example |
| :--- | :--- | :--- |
| Crates | `snake_case` (prefer single word) | `codefeed` |
| Modules, files | `snake_case` | `selector`, `storage` |
| Types, traits, enums, variants | `UpperCamelCase` | `FileRecord`, `Option::Some` |
| Functions, methods, variables | `snake_case` | `render_text`, `path_arena` |
| Static items, constants | `SCREAMING_SNAKE_CASE` | `MAX_FILE_COUNT`, `MAX_OUTPUT_SIZE` |
| Type parameters | Concise `UpperCamelCase` | `T`, `W: Write` |
| Lifetimes | Short `lowercase` | `'a`, `'de` |
| Features | `kebab-case` | `platform-verifier` |

### Fine Points from RFC 430

- **Acronyms and Initialisms:**
  - In `UpperCamelCase`, acronyms and compound brand names count as a single word (`Uuid`, `HttpServer`, `GithubClient`, `Source::Github` — never `UUID`, `HTTPServer`, `GitHubClient`, `Source::GitHub`).
  - In `snake_case`, acronyms are lowercased (`is_xid_start`, `sha`).
- **Single-Letter Words:**
  - In `snake_case` or `SCREAMING_SNAKE_CASE`, never use single-letter words unless it is the final word (`btree_map` over `b_tree_map`; `PI_2` over `PI2`).

### Idiomatic Method & Function Naming

- **Getters & Setters (`C-GETTER`):**
  - Omit `get_` prefix on getters: `field(&self) -> &Field`.
  - Mutable borrow getters use `_mut` suffix: `field_mut(&mut self) -> &mut Field` (shell layer only; `&mut self` is forbidden in `src/core/`).
  - Prefix with `get_` only when lookup by key or runtime search is performed (`get(&self, key)`).
  - Prefix setters with `set_`: `set_field(&mut self, val: Field)` (shell layer only; `&mut self` is forbidden in `src/core/`).

- **Constructors & Builders (`C-CTOR`, RFC 430):**
  - Default constructor: `new(...) -> Self`.
  - Additional constructors use descriptive prefixes: `with_capacity(...)`, `with_more_details(...)`.
  - Conversion constructors: `from_...(...)` (`from_raw`, `from_utf8`).
  - Fluent builder configuration methods use `with_` or verb/adjective names directly without redundant prefixes.

- **Unwrapping & Panic-Free Extraction (RFC 430):**
  - Reserve `unwrap()` strictly for assertions on `Option` and `Result` where failure explicitly panics.
  - Non-panicking resource extraction must use `into_inner()` (wrappers) or `into_foo()` (concrete target types), never `unwrap()`.

- **Iterators (`C-ITER`):**
  - Shared reference iterator: `iter(&self) -> Iter`.
  - Mutable reference iterator: `iter_mut(&mut self) -> IterMut` (shell layer only; `&mut self` is forbidden in `src/core/`).
  - Consuming iterator: `into_iter(self) -> IntoIter` (via `IntoIterator`).

- **Predicates & Booleans:**
  - Use descriptive boolean prefixes: `is_empty()`, `has_children()`, `can_read()`.
  - Avoid negative forms in identifiers (`!is_empty()` over `is_not_empty`).

- **Standard Abbreviations:**
  - Standard abbreviations: `id`, `url`, `len`, `src`, `dst`, `err`.
  - Prefer established Rust naming idioms (`len()` over `length()`, `iter()` over `iterator()`).
  - Do not abbreviate domain terms outside the established list (`extension` / `extensions` over `ext`, e.g., `SENSITIVE_EXTENSIONS`).

- **Brevity vs. Explicitness:**
  - **No Cryptic Truncation:** Never truncate identifiers to the point of losing domain semantics (avoid `chk_vld_pt()`, `cnt_buf`; use `is_valid_path()`, `buffer_count`).
  - **No Redundant Verbosity:** Eliminate context already conveyed by the enclosing module, type, or scope (`file_record.path` over `file_record.file_record_path`, `cli::parse()` over `cli::parse_cli()`, `source::parse()` over `source::parse_source()`, `local::collect()` over `local::collect_local()`).
  - **Direct Intent:** Prefer concise, self-documenting identifiers over paragraph-style names or unnecessary helper wrappers.

---

## 2. Conversion Methods (`C-CONV`, `C-CONV-TRAITS`, `C-CONV-SPECIFIC`)

Ad-hoc conversions follow strict naming, ownership, and cost contracts:

| Prefix | Cost | Ownership Transition | Receiver | Guideline |
| :--- | :--- | :--- | :--- | :--- |
| `as_` | Free ($O(1)$) | Borrowed $\rightarrow$ Borrowed | `&self` or `&mut self` | Returns zero-cost borrowed view or slice |
| `to_` | Expensive | Borrowed $\rightarrow$ Owned / Borrowed $\rightarrow$ Borrowed / Owned $\rightarrow$ Owned (`Copy`) | `&self` (or `self` for `Copy`) | Performs allocation, clone, or heavy transformation |
| `into_` | Variable | Owned $\rightarrow$ Owned | `self` (consumed) | Consumes receiver, reuses buffers or transitions ownership |

### Conversion Principles

- **Standard Traits First (`C-CONV-TRAITS`):** Implement `From`, `TryFrom`, `AsRef`, and `AsMut` for canonical conversions instead of custom names.
- **Locate on Specific Type (`C-CONV-SPECIFIC`):** Define the conversion method on the type carrying stricter invariants (`str::as_bytes` over slice methods).

---

## 3. Architecture & Boundaries (Functional Core, Imperative Shell)

Codebase is strictly partitioned into pure logic and effectful shell:

### `src/core/` (Functional Core)

- **Purity & Referential Transparency:** Pure deterministic logic only. Functions must be referentially transparent and immutable. Side effects strictly belong in `src/shell/`.
- **State Transitions:** State changes occur via consuming ownership moves (`self` -> `Self` / linear typestates). `&mut self` is forbidden.
- **Forbidden:** No `std::fs`, `std::net`, `std::env`, `std::time::SystemTime`, `eprintln!`, or `crate::shell` imports. No interior mutability (`Cell`, `RefCell`, `Atomic*`, `Mutex`, `RwLock`), no `static mut`, and no `unsafe`.
- **Determinism:** Pass parameters explicitly as deterministic values (`retry_delay(attempt)`).
- **Closed List of Exceptions:**
  1. `std::fmt::Display::fmt(&self, f: &mut std::fmt::Formatter<'_>)`: Mandated by standard formatting trait.
  2. Local stack variables and owned value parameters (`let mut` / `mut self`): Enclosed computation with no caller-observable side effects. Any `&mut` reference parameter (including `&mut self`) is forbidden outside `std::fmt::Display::fmt`.

### `src/shell/` (Imperative Shell)

- **Side Effects:** All I/O, network calls, filesystem operations, and process interactions live here.
- **Delegation:** Collect external inputs, settle each candidate via `Selector::observe`, and delegate computation/storage directly to `src/core/`.
- **Parallel Pipeline:** Local directory collection coordinates workers (`ignore::WalkParallel`) and the main thread via bounded `std::sync::mpsc::sync_channel(128)` inside `std::thread::scope`, hiding filesystem/kernel latency (`CreateFileW`) while preserving pure single-threaded state transitions in `src/core/`.

### Type-Driven Invariants ("Parse, Don't Validate")

Enforce invariants structurally through data types and compile-time evaluation rather than scattered runtime checks:

- **Make Invalid States Unrepresentable:** Model domain states with algebraic data types, linear typestates, and newtypes. Avoid boolean flags, sentinel values (distinguish stdout from file paths via `OutputTarget::Stdout` vs `OutputTarget::File`, never sentinel `"-"` strings), or unvalidated primitive strings.
- **Validate at Ingestion Boundaries:** Parse untrusted external inputs (CLI flags, URL paths, archive headers) into strict domain value objects (`GitRef`, `Source`) once at the boundary. Downstream core logic assumes validity by type contract without defensive re-validation.
- **Linear Typestate Transitions:** Enforce multi-phase workflows exposed across call boundaries through consuming ownership transitions (`Selector` $\rightarrow$ `CollectionResult` $\rightarrow$ `RenderedDigest`). Uncompleted workflows must fail to compile (`#[must_use]`). A phase sequence owned by one function stays a plain `match` inside it (`Selector::observe`).
- **Invariant-Preserving Type Wrappers:** Encapsulate ordering or layout guarantees into dedicated wrappers (`SortedFiles(FileStorage)` guarantees lexicographical order, preventing unsorted access).
- **Compile-Time Assertions & Static LUTs:** Enforce constant relationships and operational limits via `const _: () = { assert!(...); };` blocks and CTFE-generated lookup tables (`[bool; 256]`). Catch policy violations at build time with zero runtime cost.

---

## 4. Memory, Data Layout & Dependencies

Minimize memory footprints, eliminate allocations in loops, and keep dependencies minimal:

- **Arena Storage:** Store file paths and contents in contiguous `FileStorage` arenas (`path_arena`, `content_arena`) indexed by 16-byte `FileSpan`. Iterate via zero-copy `FileRecord<'a>`. Enables in-place lexicographical sorting by swapping offsets without moving string data.
- **Direct Data Structures:** Use direct, standard data structures (`Vec`, `HashMap`). Avoid speculative layout packing, premature micro-tuning, or pointer-indirected trees; prioritize structural simplicity.
- **Bounded Value-Passing Rendering:** Render outputs into in-memory `BoundedBuffer` returning `RenderedDigest` using single-pass escape helpers, enforcing `MAX_OUTPUT_SIZE` purely in core without mutable out-parameters or I/O side effects. File delivery writes digest bytes to an atomic temporary file; stdout writes digest bytes on success to guarantee all-or-nothing delivery on overflow.
- **Std-First Dependencies:** Default to `std`. Introduce external crates only when in-tree maintenance introduces disproportionate complexity or security risk (`.gitignore` parsing via `ignore`, HTTP via `ureq`). Document crate rationale inline in `Cargo.toml`.

---

## 5. OS & Network Safety

Enforce defense-in-depth across filesystem and network interactions:

- **Symlink Protection & Worker Safety:** Always verify candidate filesystem entries are regular, non-symlink files using `fs::symlink_metadata` before opening files (`fs::File::open`). Never traverse symlinks. Worker threads validate metadata independently.
- **Bounded Concurrency Backpressure:** Channel communications between worker threads and collection consumers must use bounded channels (`sync_channel`) to prevent unbounded memory growth during filesystem traversal.
- **Atomic File Writes:** Perform all disk writes through `AtomicFile` with inner `TempFile` RAII guards, closing OS file handles before unlinking on `Drop` (cross-platform Windows safety), preserving existing Unix file permissions, and syncing the parent directory on commit.
- **Scoped HTTP Headers:** Enforce `RedirectAuthHeaders::Never`. Gate `Authorization` headers strictly to `is_github_api_origin(url)`.

---

## 6. Error Handling & Panics

- **Error Types:** Provide domain-specific error enums in `core` and `shell`. Avoid unstructured stringly-typed errors.
- **No Uncontrolled Panics:** Never call `unwrap()` or `expect()` on user-facing paths or unvalidated input. Reserve `expect()` for mathematically impossible invariants with explanatory message.
- **Boundary Propagation:** Shell wraps lower-level I/O errors into meaningful domain diagnostics before reporting.

---

## 7. Documentation & Comments (Rust API Guidelines: `C-CRATE-DOC`, `C-EXAMPLE`, `C-FAILURE`, `C-LINK`, `C-QUESTION-MARK`; RFC 1574)

Clear, accurate documentation is enforced across the codebase as a primary architectural and API contract:

### Rustdoc Standards (RFC 1574, `C-CRATE-DOC`)

- **Doc Comment Syntax:** Use `///` for item-level documentation and `//!` for enclosing crate (`src/lib.rs`) and module documentation (`mod.rs` or file roots).
- **Opening Summary Line:** The first line must be a concise, single-sentence summary ending with a period, in third-person singular present indicative ("Returns ...", "Parses ...", "Creates ...", not "Return ..." or "This function ...").
- **Explanatory Body:** Follow summary line with a blank line before providing operational contracts, context, or invariants.
- **Language & Tone:** English only (RFC 1574). High-density, crisp, reader-first prose. Avoid filler or conversational hedging.

### Standard Section Headers

Structure item documentation using standard Markdown level-1 (`#`) section headers:

- **`# Errors`:** Mandatory on any public or crate-visible function returning `Result`. Exhaustively documents all failure conditions and error variants.
- **`# Panics`:** Mandatory on any function that can panic. Explicitly states all conditions triggering `panic!` or `expect!`.
- **`# Safety`:** Mandatory for any `unsafe` function or trait. Explicitly details all safety invariants callers must uphold.
- **`# Examples`:** Practical usage snippets written as runnable, testable doctests (`cargo test --doc`). Mark with `no_run` when side effects or external environments are required.

### Visibility & Lint Enforcement

- **Compiler Enforcement:** The crate root (`src/lib.rs`) enforces `#![warn(missing_docs)]`. All public items (`pub`) re-exported by the library must be documented without warnings.
- **Scope & Depth by Visibility:**
  - **Public API (`pub` in `src/lib.rs`):** Exhaustive documentation with summary line, detailed contracts, `# Errors`, and `# Examples` where applicable.
  - **Crate-Internal Items (`pub(crate)`):** Single-line summary and doc comments documenting boundary assumptions, invariants, and performance characteristics.
  - **Private Items (`fn`, `const`):** Self-documenting through direct naming; document non-trivial bitwise operations, regex-free parsers, or specific security policies.
- **Intra-Doc Links (`C-LINK`):** Use bracketed intra-doc links (`[`Type`]`, [`Module::func`]) to cross-reference types. Public items must not link to private/internal items (`warn(rustdoc::private_intra_doc_links)`); use backticks (`` `core` ``) when referencing internal modules from public documentation.

### Implementation Comments (`//`)

- **"Why, Not What":** Code explains *what* happens; comments explain *why* non-obvious choices were made (protocol quirks, PAX header RFC specifications, ASCII case folding performance, allocation avoidance).
- **Step Annotations:** In complex multi-phase algorithms, use numbered inline comments (`// 1. PAX Global Header: ...`) to delineate execution stages.
- **No Dead Code:** Never leave commented-out code blocks in the repository. Version control preserves prior implementations.

---

## 8. Testing & Verification

- **Verification Matrix:**
  - Lint: `cargo clippy --all-targets -- -D warnings`
  - Format: `cargo fmt --check`
  - Documentation: `cargo doc --no-deps`
  - Tests: `cargo test`
- **Table-Driven Fixtures:** Prefer slice fixtures (`let cases = [...]`) over repetitive individual test functions.
- **In-Memory Limits:** Test file count and byte limit boundaries (`MAX_FILE_COUNT`, `MAX_OUTPUT_SIZE`) via in-memory data structures (`Selector`, `CandidateBudget`) rather than physical disk loops.
- **Isolated I/O Tests:** Guard all filesystem-touching tests with `crate::shell::temp_dir` (`TempDirGuard`).
- **Deterministic Network Tests:** Mock network responses using local `TcpListener` (`127.0.0.1:0`) threads (encapsulated via `MockServer` helpers) instead of external endpoints.
