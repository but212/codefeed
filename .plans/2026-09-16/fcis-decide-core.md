# Plan: FCIS decide core

- Status: `complete`
- Updated: 2026-09-16
- Owner: agent

## Goal

`rustingest`의 파일 선택/부기 결정을 per-item 순수 함수(`decide`)로 추출하고, `skipped` 의미를 local·GitHub 간 단일화하며, 중복 정렬을 제거한다. 공개 API와 CLI 출력 형식은 불변.

## Scope

- In:
  - 순수 결정 코어 추출: candidate 계수 → 크기 판정 → 내용/인코딩 판정 → 출력 예산, per-item.
  - D2 `skipped` 통일: GitHub ignored 경로를 skip으로 계수하지 않음.
  - `text_content` 호출을 IO 계층(`read_local_file`, github 루프)에서 결정 코어로 이동.
  - `ingest`의 중복 정렬 제거 (정렬은 `render_digest` 단독 소유).
  - `context.md` 생성 (엔티티·불변식·한도·FCIS 경계).
  - 순수 경계 테스트 + GitHub skip 회귀 테스트.
- Out:
  - trait 포트 기반 전면 재작성.
  - GitHub 2-pass fetch.
  - `IngestError` 분리 (D4=A: flat 유지).
  - 공개 API 시그니처/CLI 메시지·종료 코드·한도 값 변경.
  - `.gitignore`/nested ignore 매칭 의미 변경, 원격 ignore 예산 값 변경.

## Evidence and assumptions

- Confirmed:
  - 코어 모듈 진입점: `src/lib.rs::ingest` (검증 + 정렬 + 수집/렌더/쓰기 혼합, `sort_by` at `src/lib.rs:41`).
  - 순수 상태기계 존재: `src/model.rs::CandidateBudget`, `src/digest.rs::DigestBudget`, `src/collect/github.rs::RemoteIgnoreBudget`.
  - IO 루프: `src/collect/local.rs::collect_local` (`candidates.add()` at :149, walker 필터 at :136), `src/collect/github.rs::collect_github` (ignored `budget.skip()` at :283).
  - binary/UTF-8 판정 셸 위치: `src/collect/local.rs::read_local_file` (`text_content` at :43), `src/collect/github.rs::collect_github` (at :310).
  - 이중 정렬: `src/lib.rs:41` + `src/digest.rs:159`.
  - 공개 표면: `parse_source`, `ingest`, `help_text`, `run_cli`, `IngestError`, `IngestReport`, `Options`, `Source` (`src/lib.rs`).
  - 보안 경계: `open_local_file` `O_NOFOLLOW`/`O_NONBLOCK` (`src/collect/local.rs:46`), `output_identity` 별칭 제외 (`src/output.rs:12`).
  - 테스트: `tests/unit/mod.rs` 24건, `tests/cli.rs` 5건. 내부 심볼 직접 참조: `digest::FileRecord`, `digest::DigestBudget`, `github::classify_body`, `github::is_regular_blob`, `github::RemoteIgnoreBudget`, `local::read_local_file`.
  - 네이티브 명령: `cargo test`, `cargo test --test cli`.
  - 결정 근거: Challenge Docs 세션 확정 D1=C, D2=A, D3=A, D4=A, D5=A.
- Assumption:
  - `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`가 repository-native로 허용됨 (CI 설정 부재).
  - 신규 코어 모듈 경로/이름(`src/decide.rs`, `decide::Selector`)은 `lean-design`(Phase 2)이 조정할 수 있음.
  - `read_local_file` 반환 ADT의 최종 명칭은 `lean-design`이 결정.

## General laws

| ID | Law | Scope / Preconditions | Limits / Counterexample | Linked contracts |
| --- | --- | --- | --- | --- |
| `L-01` | hidden·ignored는 candidate가 아니며 `skipped`에도 계수되지 않는다 | regular 파일, 소스 종류 불문 | local은 walker가 사전 제거, GitHub은 명시 ignore 매칭 | `C-04`, `C-07` |
| `L-02` | candidate 계수는 내용/크기/인코딩 판정보다 먼저 일어난다 | `MAX_FILE_COUNT` 적용 | binary/oversized도 계수에 포함 | `C-05` |
| `L-03` | `skipped`는 candidate였으나 내용/크기/인코딩으로 제외된 수와 정확히 일치한다 | 성공 반환 시 | ignored는 candidate가 아니므로 제외 | `C-02`, `C-03`, `C-04` |
| `L-04` | 파일 경로 정렬은 digest 렌더링이 단독 수행한다 | digest 생성 | 셸/오케스트레이터의 선행 정렬은 중복 | `C-08` |

## Invariants and state rules

- Valid states (`decide` per item): `Include`, `Skip`, `Error(OutputTooLarge)`, `Error(InvalidMetadata)`.
- Forbidden states:
  - ignored 경로가 `candidate` 계수 또는 `skipped`에 포함되는 상태 (D2=A 위반).
  - candidate 계수 없이 내용 판정이 먼저 끝나는 상태 (`L-02` 위반).
  - `IngestReport.included_files != files.len()` 또는 `skipped_files != skip 계수`인 상태.
  - `MAX_OUTPUT_SIZE` 초과분이 `Include`로 확정되는 상태.
- 한도 불변식: `MAX_FILE_SIZE` 1 MiB, `MAX_OUTPUT_SIZE` 20 MiB, `MAX_FILE_COUNT` 10,000, `MAX_JSON_SIZE` 8 MiB, `MAX_REMOTE_IGNORE_FILES` 256, `MAX_REMOTE_IGNORE_BYTES` 8 MiB (값 불변).

## Contracts

| ID | Behavior / Scenario | Condition / Input | Expected Result / Error | Invariant | Test path/name | Implementation boundary | Permanent spec |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `C-01` | regular UTF-8 텍스트 포함 | non-ignored, size ≤ `MAX_FILE_SIZE`, valid UTF-8 | `Include(FileRecord)` | `L-02` | `tests/unit` `decide_includes_text_candidate` | `src/decide.rs` | `context.md` |
| `C-02` | binary/비UTF-8 제외 | NUL 포함 또는 invalid UTF-8 | `Skip`, skip +1 | `L-03` | `decide_skips_binary_candidate` | `src/decide.rs` | `context.md` |
| `C-03` | 크기 경계 | size == `MAX_FILE_SIZE` → Include; size == `MAX_FILE_SIZE`+1 → `Skip` | 계약대로 | `L-03` | `decide_handles_size_boundary` | `src/decide.rs` | `context.md` |
| `C-04` | ignored는 candidate/skip 아님 | `ignored = true` | `Ignore`, candidate 미계수, skip 미계수 | `L-01` | `github_ignored_is_not_skipped` | `src/decide.rs` | `context.md` |
| `C-05` | candidate 선계수 | binary/oversized 혼합, `MAX_FILE_COUNT` 도달 | 초과 시 `Error(InvalidMetadata)` | `L-02` | `decide_counts_candidate_before_content` | `src/decide.rs` | `context.md` |
| `C-06` | 출력 예산 초과 | 누적 렌더 크기 > `MAX_OUTPUT_SIZE` | `Error(OutputTooLarge)`, 실패 항목 미포함 | 한도 불변식 | `decide_rejects_output_overflow` | `src/decide.rs` | `context.md` |
| `C-07` | GitHub ignored skip 미계수 (회귀) | GitHub ignored 경로 1건 + 텍스트 1건 | `included=1`, `skipped=0` | `L-01`, `L-03` | `github_ignored_is_not_skipped` | `src/collect/github.rs` + `src/decide.rs` | `context.md` |
| `C-08` | 정렬 단일 소유 | 입력 역순 2건 | digest 내 `a` < `z`, 렌더 결과 결정적 | `L-04` | `renders_files_in_deterministic_order` (기존) | `src/digest.rs`, `src/lib.rs` | `context.md` |
| `C-09` | 공개 API·CLI 불변 | 기존 `tests/cli.rs` 5건 | 전부 통과, 시그니처 무변경 | — | `tests/cli.rs` | `src/lib.rs`, `src/cli.rs` | `context.md` |

## TDD slices

- Slice 1 (`C-01`): Red `tests/unit` `decide_includes_text_candidate` → Green `src/decide.rs` (Selector + include 경로) → Verify `cargo test --lib`.
- Slice 2 (`C-02`, `C-03`): Red binary/크기 경계 테스트 → Green `text_content`/크기 분기 → Verify `cargo test --lib`.
- Slice 3 (`C-04`, `C-05`): Red ignored/candidate 계수 테스트 → Green ignored 분기 + `CandidateBudget` 연동 → Verify `cargo test --lib`.
- Slice 4 (`C-06`): Red 예산 초과 테스트 → Green `DigestBudget` 연동 → Verify `cargo test --lib`.
- Slice 5 (`C-07`): Red GitHub skip 회귀 테스트 → Green `src/collect/github.rs` ignored `continue` (no `budget.skip()`) + Selector 연결 → Verify `cargo test --lib`.
- Slice 6 (통합 배선): Green `src/collect/local.rs` `read_local_file`을 관측 ADT로 축소(보안 open 유지), `collect_local`/`collect_github`가 Selector 사용; `src/lib.rs:41` 정렬 제거 → Verify `cargo test`.
- Slice 7 (`C-08`, `C-09`): 기존 정렬/CLI 테스트 통과 확인 → Verify `cargo test`, `cargo test --test cli`.
- Refactor: 중복 제거 후 `cargo fmt` + (assumption) `cargo clippy`.

## Checklist

- [x] `C-01` **순수 Selector + include 경로** - `src/decide.rs`, `src/lib.rs`; verify: `cargo test --lib decide_includes_text_candidate` — pass
- [x] `C-02` **binary/비UTF-8 skip** - `src/decide.rs`; verify: `cargo test --lib decide_skips_binary_candidate` — pass
- [x] `C-03` **크기 경계** - `src/decide.rs`; verify: `cargo test --lib decide_handles_size_boundary` — pass
- [x] `C-04` **ignored 비계수** - `src/decide.rs`; verify: `cargo test --lib github_ignored_is_not_skipped` — pass (C-07과 단일 테스트로 통합)
- [x] `C-05` **candidate 선계수** - `src/decide.rs`; verify: `cargo test --lib decide_counts_candidate_before_content` — pass
- [x] `C-06` **출력 예산 초과** - `src/decide.rs`; verify: `cargo test --lib decide_rejects_output_overflow` — pass
- [x] `C-07` **GitHub skip 통일** - `src/collect/github.rs`; verify: `cargo test --lib github_ignored_is_not_skipped` — pass (순수 경로; 루프 배선은 코드/컴파일 검증)
- [x] **local 셸 축소** - `src/collect/local.rs` (`read_local_file`→`Probe`, Selector 사용, 보안 open 유지); verify: `cargo test --lib does_not_follow_local_symlink does_not_block_when_candidate_becomes_fifo bounds_local_file_reads includes_file_at_exact_size_limit` — pass
- [x] `C-08` **정렬 단일화** - `src/lib.rs` 정렬 제거; verify: `cargo test --lib renders_files_in_deterministic_order` — pass
- [x] `C-09` **공개 API·CLI 불변** - `src/lib.rs`, `src/cli.rs`; verify: `cargo test --test cli` — 5 pass
- [x] **기존 내부 테스트 계약 갱신** - `tests/unit/mod.rs` (`LocalRead` 참조 4건 → `Probe`); verify: `cargo test --lib` — pass
- [x] **context.md 생성** - `context.md`; verify: 엔티티/불변식/한도/FCIS 경계 섹션 포함
- [x] **최종 품질 게이트** - verify: `cargo fmt --check` exit 0, `cargo clippy --all-targets -- -D warnings` exit 0

## Decisions and deviations

- 2026-09-16: 실행 시작 승인 — status `in-progress`.
- 2026-09-16: Phase 2 lean-design 확정 — 신규 모듈 `src/decide.rs`; 타입 `ItemPath{Utf8(String),NonUtf8}`, `Probe{Bytes(Vec<u8>),Missing,TooLarge}`, `Item{path,ignored,probe}`, `Decision{Include,Ignore,Skip}`, `Selector{files,candidates,digest}`. 메서드 `Selector::new(label,revision)`, `decide(Item)->Result<Decision,IngestError>`, `into_parts()`. 판정 순서: ignored(무계수) → candidate 계수 → path/크기/인코딩 판정. `read_local_file`은 `Probe` 반환으로 축소(보안 open·bounded read 유지).
- 2026-09-16: 구현 편차 — `Selector` 자체 `skipped` 카운터 대신 기존 `DigestBudget::skip()/skipped()` 재사용 (clippy dead-code로 중복 제거).
- 2026-09-16: 구현 편차 — `C-04`/`C-07`은 동일 순수 경로를 공유하므로 단일 테스트 `github_ignored_is_not_skipped`로 통합. `decide_excludes_ignored_without_counting` 미생성(중복 케이스 회피).
- 2026-09-16: D1=C — FCIS 착수 전 `context.md`로 도메인 의미 선고정.
- 2026-09-16: D2=A — `skipped` = candidate였으나 내용/크기/인코딩으로 제외된 수. GitHub ignored는 skip 미계수 (CLI 관찰 동작 변경, 의도적 정합화).
- 2026-09-16: D3=A — per-item 순수 `decide` 추출 + 이중 정렬 제거 + `text_content` 이동. 포트 trait 미도입.
- 2026-09-16: D4=A — `IngestError` flat 유지.
- 2026-09-16: D5=A — GitHub single-pass 유지, 메타데이터 판정 통과 시에만 fetch.
- 2026-09-16: 기각 — 전체 후보 2단계 `select` (최악 ~10 GiB eager 수집으로 20 MiB early-exit 회귀), trait 포트 전면 재작성 (어댑터당 1개, 주입 이득 미검증), GitHub 2-pass (tree size 선필터로 절감 0).
- 2026-09-16: lean-review P3 2건 해소 — `Item.ignored: bool` 제거하고 `Probe::Ignored` 변형으로 통합(모순 상태 제거), `Selector::decide`의 test-only `Decision` 반환 제거(`Result<(), IngestError>`), 테스트는 `into_parts()` 결과로 판정. `context.md`의 엔티티/FCIS 경계 문구 동기화. 공개 API·CLI·동작 불변.

## Verification

- [x] `cargo test --lib` - 30 passed; 0 failed
- [x] `cargo test --test cli` - 5 passed; 0 failed
- [x] `cargo test` - 30 + 5 passed; 0 failed
- [x] `cargo fmt --check` - exit 0
- [x] `cargo clippy --all-targets -- -D warnings` - exit 0

## Blockers and open questions

- None. `src/decide.rs`/`Probe` 명칭 확정, 내부 테스트 4건 갱신 완료.
