# Domain Context — rustingest

## Purpose

Convert a local directory or public GitHub repository into one LLM-friendly
text digest. Public behavior, limits, and CLI contract are documented in
`README.md`; this file fixes domain terms and the functional-core /
imperative-shell boundary.

## Entities and value objects

- `Source` — `Local(PathBuf)` or `GitHub { owner, repo }`.
- `Item` — one observed candidate handed to the pure core: `path`
  (`ItemPath::Utf8 | NonUtf8`) and `probe`
  (`Probe::Bytes | Missing | TooLarge | Ignored`).
- `FileRecord` — digest entry: `path`, `content`.
- `IngestReport` — `output`, `included_files`, `skipped_files`.
- `OutputTarget` — digest destination: `File(PathBuf)` or `Stdout`.
- Budgets — `CandidateBudget` (candidate count), `DigestBudget` (rendered
  bytes and skipped count), `RemoteIgnoreBudget` (remote `.gitignore` count
  and bytes).

## Invariants

- Hidden and ignored paths are excluded before candidate accounting. They are
  neither candidates nor `skipped`.
- A candidate slot is consumed before content, size, or encoding is judged
  (`L-02`).
- `skipped` counts candidates rejected for content, size, or encoding reasons.
  Ignored paths are not `skipped` (`D2=A`).
- Digest size accounting rejects content that would exceed `MAX_OUTPUT_SIZE`;
  overflow is never included.
- File-path ordering is lexicographic and owned solely by `render_digest`.
- A file output target is written atomically: create-new temporary file, sync,
  rename.
- A stdout output target writes the rendered digest to the stream directly, with
  no temporary file and no filesystem sync.
- The success completion message goes to standard output for a file target and to
  standard error for a stdout target; a stdout target's stream carries digest
  text only.
- A closed consumer pipe is a normal early close, not an error: a broken pipe on
  the stdout target exits successfully.
- The local walk excludes the output file by identity. A stdout target has no
  filesystem path, so no path is normalized and nothing is excluded.
- Local file opening never follows symlinks and never blocks on special files
  (`O_NOFOLLOW` / `O_NONBLOCK`).
- HTTP credentials (`Authorization: Bearer <token>`) attach only to
  exact-origin GitHub API and raw-content hosts; lookalike or foreign origins
  never receive credentials.

## Limits

| Constant | Value |
| --- | --- |
| `MAX_FILE_SIZE` | 1 MiB |
| `MAX_OUTPUT_SIZE` | 20 MiB |
| `MAX_FILE_COUNT` | 10,000 |
| `MAX_JSON_SIZE` | 8 MiB |
| `MAX_REMOTE_IGNORE_FILES` | 256 |
| `MAX_REMOTE_IGNORE_BYTES` | 8 MiB |

## Functional core / imperative shell boundary

Boundary rule: pure core code performs no I/O, clock, network, or entropy and
never depends on shell code; shell code depends on the core in one direction.

- Pure core (`src/core/`): source parsing (`source.rs`), domain value types
  and results (`model.rs`), domain errors (`error.rs`), numeric limits
  (`limits.rs`), per-item `Selector::decide` and its candidate budget
  (`decide.rs`), digest budget and rendering (`digest.rs`), ignore matching
  (`ignore.rs`), URL construction and request-target classification for
  credential scoping (`github_url.rs`), and GitHub tree/response
  classification (`github_tree.rs`).
- Imperative shell (`src/shell/`): argument and process IO (`src/main.rs`,
  `shell/cli.rs`), orchestration (`src/lib.rs::ingest`), filesystem walking and
  safe reads (`shell/local.rs`, `shell/local_io.rs`), HTTP transport with
  bounded response bodies and GitHub token credential resolution
  (`shell/github_client.rs`), remote `.gitignore` loading
  (`shell/github_ignore.rs`), GitHub orchestration (`shell/github.rs`),
  and digest emission, atomic file write or stdout stream write
  (`shell/output.rs`). The shell feeds observations to the
  core and reads the accounted result through `Selector::into_parts`.
- `read_local_file` (`src/shell/local_io.rs`) returns a `Probe`; it performs the
  bounded read but leaves text/UTF-8 and budget judgment to the core.
- A shell module may host a pure budget state machine next to the I/O that
  consumes it (`RemoteIgnoreBudget` in `src/shell/github_ignore.rs`).
