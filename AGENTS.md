# AGENTS.md

Read `context.md`, `README.md`, `docs/coding_convention.md`, and `docs/security_and_filtering.md` before changing code. Do not duplicate domain terms, limits, or module maps here.

## 1. Verify

- **Lint & Check:** `cargo clippy --all-targets -- -D warnings`
- **Format:** `cargo fmt --check`
- **Test:** `cargo test` (or `cargo test <name>`)

## 2. Code

### FCIS Boundary

- `src/core/`: Pure logic only. No `std::fs`, `std::net`, `std::env`, `SystemTime`, `eprintln!`, or `crate::shell` imports. Pass parameters as deterministic values (e.g., `retry_delay(attempt)`).
- `src/shell/`: Side effects only. Map I/O into `CandidateInput` and delegate decisions to `src/core/`.

### Memory & Dependencies

- Store file paths and contents in contiguous `FileStorage` arenas (`path_arena`, `content_arena`) indexed by 16-byte `FileSpan`; enables zero-copy `FileRecord<'a>` borrowed views throughout ingestion and in-place lexicographical sorting by swapping offsets without moving string data.
- Keep data structures direct and minimal. Avoid speculative optimizations or premature micro-tuning; prioritize structural simplicity.
- Stream output directly to `impl Write + ?Sized` using `Cow<'_, str>` or single-pass `write_escaped`.
- Default to `std` first. Add an external crate only if implementing or maintaining the logic in-tree introduces disproportionate complexity or security risk (e.g., `.gitignore` parsing via `ignore`, HTTP via `ureq`). Document the rationale as an inline comment in `Cargo.toml`.

### OS & Network Safety

- **Local reads (`src/shell/local.rs`):** Verify candidate entries are regular, non-symlink files (`fs::symlink_metadata`) and read bounded contents via standard `fs::File::open`. Never follow symlinks.
- **File writes (`src/shell/output.rs`):** Write through `AtomicFile` so errors or panics unlink temp files on `Drop`.
- **HTTP (`src/shell/github.rs`):** Keep `RedirectAuthHeaders::Never` and gate `Authorization` with `is_github_api_origin(url)`.

## 3. Test

- Use table-driven slice fixtures (`let cases = [...]`) over repetitive test functions.
- Test `MAX_FILE_COUNT` and `MAX_OUTPUT_SIZE` in memory via `Selector` / `CandidateBudget`, not disk loops.
- Use `crate::shell::temp_dir("label")` (`TempDirGuard`) for all filesystem tests.
- Mock HTTP with local `TcpListener` (`127.0.0.1:0`) threads.

## 4. Sync

- **Limits / exclusions / domain types:** Single source of truth is `src/core/policy.rs`. Reference it rather than duplicating constants across documents.
- **CLI flags / exit codes:** Update `src/core/cli.rs`, `tests/cli.rs`, `README.md`.
- **Behavior / architecture:** Update `CHANGELOG.md` under `## [Unreleased]`.

