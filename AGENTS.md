# AGENTS.md

Read `context.md` and `README.md` before changing code. Do not duplicate domain terms, limits, or module maps here.

## 1. Verify

- **Lint & Check:** `cargo clippy --all-targets -- -D warnings`
- **Format:** `cargo fmt --check`
- **Test:** `cargo test` (or `cargo test <name>`)
- **Benchmark** (`digest`, `decide`, `local` changes):
  - All: `cargo bench`
  - Filtered: `cargo bench -- --filter <pattern>` (`trie`, `render`, `xml`, `token`, `selector`, `local`)
  - Self-test: `cargo bench -- --self-test`

## 2. Code

### FCIS Boundary

- `src/core/`: Pure logic only. No `std::fs`, `std::net`, `std::env`, `SystemTime`, `eprintln!`, or `crate::shell` imports. Pass time/entropy as values (e.g., `retry_delay(attempt, entropy)`).
- `src/shell/`: Side effects only. Map I/O into `Item` / `Probe` and delegate decisions to `src/core/`. Use `crate::shell::current_epoch_nanos()` for timestamps/entropy.

### Memory & Dependencies

- Store file paths and contents in `FileStorage` arenas (`path_arena`, `content_arena`) indexed by `FileSpan`; iterate via zero-copy `FileRecord<'a>`. Never allocate per-file `String` structs.
- Keep `BudgetTrie` (`Vec<BudgetTrieNode>`, `u32` indices) and `CandidateBudget` (`dir_runs` RLE) flat. Avoid pointer trees or happy-path `HashMap`s.
- Stream output directly to `impl Write + ?Sized` using `Cow<'_, str>` or `XmlChunks`.
- Add no external crates (`clap`, `serde`, `regex`, `tokio`, `libc`, `memchr`, `percent-encoding`, `log`). Use `std` state machines and lookup tables.

### OS & Network Safety

- **Local reads (`src/shell/local_io.rs`):** Open with `O_NOFOLLOW | O_NONBLOCK` (`openat2` + `RESOLVE_BENEATH` on Linux, component `openat` on Unix, `FILE_FLAG_OPEN_REPARSE_POINT` + `GetFinalPathNameByHandleW` on Windows). Never call raw `fs::read` or `fs::File::open` on candidates.
- **File writes (`src/shell/output.rs`):** Write through `AtomicFile` so errors or panics unlink temp files on `Drop`.
- **HTTP (`src/shell/github_client.rs`):** Keep `RedirectAuthHeaders::Never` and gate `Authorization` with `is_github_api_origin(url)`.

## 3. Test

- Use table-driven slice fixtures (`let cases = [...]`) over repetitive test functions.
- Test `MAX_FILE_COUNT` and `MAX_OUTPUT_SIZE` in memory via `Selector` / `CandidateBudget`, not disk loops.
- Use `crate::shell::temp_dir("label")` (`TempDirGuard`) for all filesystem tests.
- Mock HTTP with local `TcpListener` (`127.0.0.1:0`) threads.
- Export new benchmark targets under `#[doc(hidden)] pub mod __bench` in `src/lib.rs`.

## 4. Sync

- **CLI flags / exit codes:** Update `src/core/cli.rs`, `tests/cli.rs`, `README.md`.
- **Limits / exclusions / domain types:** Update `src/core/policy.rs`, `context.md`, `README.md`.
- **Behavior / architecture:** Update `CHANGELOG.md` under `## [Unreleased]`.
