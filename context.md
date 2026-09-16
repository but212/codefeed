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
- Output is written atomically: create-new temporary file, sync, rename.
- Local file opening never follows symlinks and never blocks on special files
  (`O_NOFOLLOW` / `O_NONBLOCK`).

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

- Pure core (`src/decide.rs`, `src/source.rs`, `src/digest.rs`,
  `src/model.rs`, `src/collect/ignore.rs`): source parsing, per-item
  `Selector::decide`, budget state machines, digest rendering, ignore
  matching. No I/O, clock, network, or entropy.
- Imperative shell (`src/main.rs`, `src/cli.rs`, `src/lib.rs::ingest`,
  `src/collect/local.rs`, `src/collect/github.rs`, `src/output.rs`): argument
  and process IO, filesystem walking and safe reads, HTTP, atomic writes. It
  feeds observations to the core and reads the accounted result through
  `Selector::into_parts`.
- `read_local_file` returns a `Probe`; it performs the bounded read but leaves
  text/UTF-8 and budget judgment to the core.
