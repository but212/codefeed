# Coding Conventions

Project-wide coding standards and API design principles for `codefeed`. Combines Rust API Guidelines with project architecture, memory, and safety contracts.

---

## 1. Naming & Case Conventions (Rust API Guidelines: `C-CASE`, `C-GETTER`, `C-CTOR`, `C-ITER`)

### Case Conventions (`C-CASE`, RFC 430)

| Item | Convention | Example |
| :--- | :--- | :--- |
| Crates | `snake_case` (prefer single word) | `codefeed` |
| Modules, files | `snake_case` | `local_io`, `candidate_budget` |
| Types, traits, enums, variants | `UpperCamelCase` | `FileRecord`, `Option::Some` |
| Functions, methods, variables | `snake_case` | `render_text`, `path_arena` |
| Static items, constants | `SCREAMING_SNAKE_CASE` | `MAX_FILE_COUNT`, `MAX_OUTPUT_SIZE` |
| Type parameters | Concise `UpperCamelCase` | `T`, `W: Write` |
| Lifetimes | Short `lowercase` | `'a`, `'de` |
| Features | `kebab-case` | `platform-verifier` |

### Fine Points from RFC 430

- **Acronyms and Initialisms:**
  - In `UpperCamelCase`, acronyms and compound brand names count as a single word (e.g., `Uuid`, `HttpServer`, `BudgetTrie`, `GithubClient`, `Source::Github` — never `UUID`, `HTTPServer`, `BudgetTRIE`, `GitHubClient`, `Source::GitHub`).
  - In `snake_case`, acronyms are completely lowercased (e.g., `is_xid_start`, `sha`).
- **Single-Letter Words:**
  - In `snake_case` or `SCREAMING_SNAKE_CASE`, a word should never consist of a single letter unless it is the final word (e.g., `btree_map` over `b_tree_map`, but `PI_2` over `PI2`).

### Idiomatic Method & Function Naming

- **Getters & Setters (`C-GETTER`):**
  - Do not prefix getters with `get_`. Use the field name directly: `field(&self) -> &Field`.
  - Mutable borrow getters use `_mut` suffix: `field_mut(&mut self) -> &mut Field`.
  - Prefix with `get_` only when returning an element that requires a key or runtime lookup (e.g., `get(&self, key)`).
  - Prefix setters with `set_`: `set_field(&mut self, val: Field)`.

- **Constructors & Builders (`C-CTOR`, RFC 430):**
  - Default constructor is `new(...) -> Self`.
  - Additional constructors take descriptive prefixes: `with_capacity(...)`, `with_more_details(...)`.
  - Conversion constructors follow `from_...(...)`: `from_raw(...)`, `from_utf8(...)`.
  - Fluent builder configuration methods take `with_` or verb/adjective names directly without redundant prefixes.

- **Unwrapping & Panic-Free Extraction (RFC 430):**
  - Reserve `unwrap()` strictly for assertions on `Option` and `Result` where failure explicitly panics.
  - Non-panicking resource/inner extraction must use `into_inner()` (wrapper types) or `into_foo()` (concrete target types), never `unwrap()`.

- **Iterators (`C-ITER`):**
  - Shared reference iterator: `iter(&self) -> Iter`.
  - Mutable reference iterator: `iter_mut(&mut self) -> IterMut`.
  - Consuming iterator: `into_iter(self) -> IntoIter` (via `IntoIterator`).

- **Predicates & Booleans:**
  - Prefer descriptive boolean prefixes: `is_empty()`, `has_children()`, `can_read()`.
  - Avoid negative forms in identifiers (`is_not_empty` $\rightarrow$ `!is_empty()`).

- **Standard Abbreviations:**
  - Keep standard domain abbreviations concise: `id`, `url`, `len`, `src`, `dst`, `err`.
  - Prefer established Rust naming idioms (`len()` over `length()`, `iter()` over `iterator()`).
  - Do not abbreviate domain terms outside the established list; write them out in full (e.g., `extension` / `extensions` over `ext`, such as `SENSITIVE_EXTENSIONS`, `BINARY_EXTENSIONS`, `is_binary_extension`).

- **Brevity vs. Explicitness:**
  - **No Cryptic Truncation:** Never truncate identifiers to the point of losing domain semantics (e.g., avoid `chk_vld_pt()`, `cnt_buf`; use `is_valid_path()`, `buffer_count`). Reserve abbreviations strictly for standard, established domain terms.
  - **No Redundant Verbosity:** Eliminate context already conveyed by the enclosing module, type, or scope (e.g., `file_record.file_record_path` $\rightarrow$ `file_record.path`, `cli::parse_cli()` $\rightarrow$ `cli::parse()`, `source::parse_source()` $\rightarrow$ `source::parse()`, `local::collect_local()` $\rightarrow$ `local::collect()`).
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
- **Locate on Specific Type (`C-CONV-SPECIFIC`):** Define the conversion method on the type carrying stricter invariants (e.g., `str::as_bytes` over slice methods).

---

## 3. Architecture & Boundaries (Functional Core, Imperative Shell)

Codebase is strictly partitioned into pure logic and effectful shell:

### `src/core/` (Functional Core)

- **Purity:** Pure deterministic logic only.
- **Forbidden:** No `std::fs`, `std::net`, `std::env`, `std::time::SystemTime`, `eprintln!`, or `crate::shell` imports.
- **Determinism:** Pass timestamps, entropy, and configurations explicitly as values (e.g., `retry_delay(attempt, entropy)`).

### `src/shell/` (Imperative Shell)

- **Side Effects:** All I/O, network calls, filesystem operations, and process interactions live here.
- **Delegation:** Collect external inputs, map them into domain types (`Item`, `Probe`), and delegate computation/decisions directly to `src/core/`.
- **Clock & Entropy:** Use `crate::shell::current_epoch_nanos()` for wall time and entropy generation.

---

## 4. Memory, Data Layout & Dependencies

Minimize memory footprints, eliminate allocations in loops, and keep dependencies minimal:

- **Arena Storage:** Store file paths and contents in continuous `FileStorage` arenas (`path_arena`, `content_arena`) indexed by `FileSpan`. Iterate via zero-copy `FileRecord<'a>`. Never allocate per-file `String` or `Vec<u8>`.
- **Flat Data Structures:** Maintain flat index structures like `BudgetTrie` (`Vec<BudgetTrieNode>`, `u32` indices) and `CandidateBudget` (`dir_runs` RLE). Avoid pointer-indirected trees and `HashMap` lookups on hot paths.
- **Zero-Copy Streaming:** Stream render output directly to `impl Write + ?Sized` using `Cow<'_, str>` or streaming chunks. Avoid staging full output in memory.
- **Std-First Dependencies:** Default to `std`. Introduce external crates only when in-tree maintenance introduces disproportionate complexity or security risk (e.g., `.gitignore` parsing via `ignore`, HTTP via `ureq`). Document crate rationale inline in `Cargo.toml`.

---

## 5. OS & Network Safety

Enforce defense-in-depth across filesystem and network interactions:

- **Symlink Protection:** Always verify candidate filesystem entries are regular, non-symlink files using `fs::symlink_metadata`. Never traverse symlinks.
- **Atomic File Writes:** Perform all disk writes through `AtomicFile` temp guards, unlinking partially written files on drop in error or panic scenarios.
- **Scoped HTTP Headers:** Enforce `RedirectAuthHeaders::Never`. Gate `Authorization` headers strictly to `is_github_api_origin(url)`.

---

## 6. Error Handling & Panics

- **Error Types:** Provide domain-specific error enums in `core` and `shell`. Avoid unstructured stringly-typed errors.
- **No Uncontrolled Panics:** Never call `unwrap()` or `expect()` on user-facing paths or unvalidated input. Reserve `expect()` for mathematically impossible invariants with explanatory message.
- **Boundary Propagation:** Shell wraps lower-level I/O errors into meaningful domain diagnostics before reporting.

---

## 7. Testing & Verification

- **Verification Matrix:**
  - Lint: `cargo clippy --all-targets -- -D warnings`
  - Format: `cargo fmt --check`
  - Tests: `cargo test`
  - Benchmarks: `cargo bench` (verify no regressions on `trie`, `render`, `xml`, `token`, `selector`, `local`)
- **Table-Driven Fixtures:** Prefer slice fixtures (`let cases = [...]`) over repetitive individual test functions.
- **In-Memory Limits:** Test file count and byte limit boundaries (`MAX_FILE_COUNT`, `MAX_OUTPUT_SIZE`) via in-memory data structures (`Selector`, `CandidateBudget`) rather than physical disk loops.
- **Isolated I/O Tests:** Guard all filesystem-touching tests with `crate::shell::temp_dir` (`TempDirGuard`).
- **Deterministic Network Tests:** Mock network responses using local `TcpListener` (`127.0.0.1:0`) threads instead of external endpoints.
