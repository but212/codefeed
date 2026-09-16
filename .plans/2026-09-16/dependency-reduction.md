# Plan: Dependency reduction (HTTP client + URL)

- Status: `complete`
- Updated: 2026-09-16
- Owner: agent

## Goal

`rustingest`의 의존성 footprint를 줄인다: 무거운 `reqwest`(+tokio/hyper/aws-lc-rs/quinn)를 경량 blocking `ureq`로 교체하고, `reqwest` 제거로 실효 절감이 생긴 `url`(+idna/icu)을 자체 URL 빌더로 대체한다. 공개 API, CLI 계약, digest 출력, 수집 의미론은 불변.

## Scope

- In:
  - `reqwest` → `ureq` 3.x 교체 (`src/collect/github.rs` HTTP 계층).
  - URL 조립을 `url::Url` → 자체 문자열 빌더 + percent-encoding으로 전환 (API/raw 엔드포인트).
  - `Cargo.toml`에서 `reqwest`, `url` 제거 및 `ureq` 추가; `Cargo.lock` 갱신.
  - 순수 URL 빌더 단위 테스트 + body limit/error 매핑 테스트.
  - 필요 시 `context.md`에 의존성/경계 문구 동기화.
- Out:
  - `ignore`, `libc`, `serde`, `serde_json` 교체 (필수 유지).
  - TLS/HTTP를 직접 구현 (보안상 비권장).
  - 공개 API 시그니처, CLI 메시지/종료 코드, digest 형식, `MAX_*` 한도 값 변경.
  - GitHub 2-pass fetch, ignore 매칭 의미, `decide` 코어 변경.
  - 비동기/tokio 도입.

## Evidence and assumptions

- Confirmed:
  - 직접 의존성 6개: `ignore` 0.4, `reqwest` 0.13, `serde` 1, `serde_json` 1, `url` 2, `libc` 0.2 (`Cargo.toml`).
  - `Cargo.lock` 총 168 패키지. `reqwest` 전이(고유) 82, `url` 전이 32(대부분 idna/icu), `ignore` 12, `serde` 6, `serde_json` 4, `libc` 0.
  - HTTP 사용부: `src/collect/github.rs` — `reqwest::blocking::{Client,Response}`, `reqwest::StatusCode` (`:5-6`), `Client::builder().user_agent(GITHUB_USER_AGENT).timeout(30s)` (`:118-124`), `response()`/`read_response()` (`:239-260`, `:355-361`), `api_error()` (`:344-350`), `raw_file()` (`:153-181`), `json<T>` (`:250-261`).
  - URL 사용부: `src/source.rs::parse_github_url` (`:27-57`), `src/collect/github.rs::api_url` (`:324-337`), `raw_file`의 `Url::parse`+`path_segments_mut`+`query_pairs_mut` (`:160-171`).
  - Bounded read 불변식: `read_response`가 `limit+1`까지 read 후 `classify_body`로 `Complete/Exceeded` 판정 (`src/collect/github.rs:195-201`, `:355-361`). 테스트: `tests/unit/mod.rs::distinguishes_complete_and_oversized_bodies` (`:406`).
  - ureq 3.4.2 (rust-version 1.85, MIT/Apache): `default-features=false, features=["rustls"]` → 전이(고유) 22; `+platform-verifier` → 25. `ureq::http::StatusCode` 재노출(`lib.rs:541`). `Body::with_config().limit(n).read_to_vec()`가 초과 시 `Error::BodyExceedsLimit(u64)`(`src/body/limit.rs:24`). `ConfigBuilder::{user_agent, timeout_global, http_status_as_error, max_redirects}` 제공. 기본 `max_redirects=10`(reqwest와 동일), `user_agent` 기본값은 `ureq/{ver}`.
  - 현재 reqwest는 `rustls-platform-verifier`/`rustls-native-certs` 사용(Lock 및 reqwest subtree) → OS 신뢰 저장소.
  - 테스트는 네트워크 비의존: `tests/unit/mod.rs` 30건 + `tests/cli.rs` 5건, GitHub 실호출 테스트 없음.
  - 네이티브 명령: `cargo test`, `cargo test --test cli`, `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` (기존 plan에서 repository-native로 확립, CI 설정 부재).
- Assumption:
  - `ureq::http::StatusCode` Display가 `reqwest::StatusCode`와 동일 형식(`200 OK`)이라 `api_error` 메시지가 보존됨 (동일 `http` crate 기반이므로 성립 예상; 구현 시 확인).
  - `http::Uri`가 자체 percent-encoding된 경로를 수용함.
- Open decision:
  - `D1` HTTP 클라이언트/신뢰 저장소 선택 (아래 Decisions 참조) — 사용자 확인 대기.

## General laws

| ID | Law | Scope / Preconditions | Limits / Counterexample | Linked contracts |
| --- | --- | --- | --- | --- |
| `L-01` | GitHub 수집의 관측 결과(포함/스킵/에러 종류·메시지 형식)는 HTTP 클라이언트 교체 전후 동일하다 | HTTP 계층만 교체 | TLS 신뢰 저장소를 바꾸면 신뢰 실패 케이스가 달라짐 → `D1`에서 parity 선택 시 성립 | `C-01`, `C-02` |
| `L-02` | 공개 표면(`parse_source`, `ingest`, `run_cli`, `help_text`, `IngestError`, `IngestReport`, `Options`, `Source`)은 불변이다 | 전체 변경 | 시그니처 추가/삭제는 위반 | `C-05` |
| `L-03` | 원격 `.gitignore` 매칭·budget·`decide` 의미론은 불변이다; 변경은 수집 셸에 한정된다 | `src/collect/*` | `decide.rs`/`ignore.rs` 로직 변경은 범위 밖 | `C-05` |
| `L-04` | 모든 HTTP body read는 유한 상한을 가진다 | `response()` 경로 | `limit` 없는 `read_to_vec`는 위반 | `C-02` |

## Invariants and state rules

- Valid states (HTTP read): `LimitedBody::{Complete(Vec<u8>), Exceeded}` (불변).
- Forbidden states:
  - 상한 없는 body read (unbounded `read_to_vec`).
  - non-2xx에서 body read 없이 에러 상세를 만드는 상태.
  - `MAX_JSON_SIZE`/`MAX_FILE_SIZE` 초과분이 `Complete`로 확정되는 상태.
  - 공개 API 시그니처 또는 `MAX_*` 값 변경.
  - percent-encoding 누락으로 `Uri` 파싱 실패 또는 경로 오염(`%`, `#`, `?`, 공백, 비ASCII).
- 한도 불변식(값 불변): `MAX_FILE_SIZE` 1 MiB, `MAX_OUTPUT_SIZE` 20 MiB, `MAX_FILE_COUNT` 10,000, `MAX_JSON_SIZE` 8 MiB, `MAX_REMOTE_IGNORE_FILES` 256, `MAX_REMOTE_IGNORE_BYTES` 8 MiB, 요청 timeout 30s, redirect 10.

## Contracts

| ID | Behavior / Scenario | Condition / Input | Expected Result / Error | Invariant | Test path/name | Implementation boundary | Permanent spec |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `C-01` | GitHub HTTP 요청 동등 | GitHub API/raw GET | UA=`rustingest/0.1`, global timeout 30s, redirect ≤10, non-2xx → `IngestError::Api("HTTP {status}: {detail}")` | `L-01`, `L-04` | 신규 단위(에러 매핑) + 수동 smoke | `src/collect/github.rs` | `context.md` |
| `C-02` | body 상한/초과 판정 유지 | limit 초과 응답 | `LimitedBody::Exceeded`; JSON 초과 시 `IngestError::Api("... exceeds the {MAX_JSON_SIZE} byte limit")` | `L-04` | 신규 `limited_body` 매핑 테스트 | `src/collect/github.rs` | `context.md` |
| `C-03` | API/raw URL 조립 | owner/repo/revision/path 세그먼트 | `https://api.github.com/repos/...`, `recursive=1`, raw `.../{owner}/{repo}/{revision}/{path}` | percent-encoding 불변식 | 신규 URL 빌더 테스트 | `src/collect/github.rs`(또는 신규 모듈) | `context.md` |
| `C-04` | 의존성 축소 | `Cargo.toml`/`Cargo.lock` | `reqwest`·`url` 제거, `ureq` 추가; Lock 패키지 수 감소 | — | `cargo tree` 수치 | `Cargo.toml` | — |
| `C-05` | 공개 API·CLI·동작 불변 | 기존 전체 테스트 | `tests/unit` 30 + `tests/cli` 5 전부 통과, 시그니처 무변경 | `L-02`, `L-03` | `tests/unit/mod.rs`, `tests/cli.rs` | `src/lib.rs`, `src/cli.rs`, `src/collect/*` | `context.md` |
| `C-06` | percent-encoding 경계 | 공백/`%`/`#`/`?`/`+`/`/`/비ASCII 세그먼트 | unreserved `[A-Za-z0-9-._~]` 외 전부 `%XX`; `/`는 세그먼트 구분자로만 사용 | percent-encoding 불변식 | 신규 인코더 테스트 | 신규 순수 함수 | `context.md` |

## TDD slices

- Slice 1 (`C-03`, `C-06`): Red URL 빌더/percent-encoder 테스트(`api_url`, raw URL, 경계 문자) → Green 순수 함수(String 기반) → Verify `cargo test --lib`.
- Slice 2 (`C-01`, `C-02`): Red `limited_body`/초과 매핑 테스트(`ureq::Error::BodyExceedsLimit` → `Exceeded`) → Green `GithubClient`를 `ureq::Agent` 기반으로 교체, `read_response`를 `limit().read_to_vec()`로 전환, `api_error` 타입 교체 → Verify `cargo test --lib` + `cargo build`.
- Slice 3 (`C-04`): `Cargo.toml`에서 `reqwest`·`url` 제거, `ureq` 추가 → Verify `cargo tree` 수치 + 전체 테스트.
- Refactor: 공통 헬퍼 정리 후 `cargo fmt` + `cargo clippy`.

## Checklist

- [x] `C-06` **percent-encoder 순수 함수** - `src/collect/github.rs::encode_path_segment`; verify: `cargo test --lib percent_encodes_url_path_segments` — pass
- [x] `C-03` **URL 빌더** - `src/collect/github.rs::{api_url,raw_url}`; verify: `cargo test --lib builds_github_api_and_raw_urls` — pass
- [x] `C-01`/`C-02` **ureq HTTP 계층 교체** - `GithubClient`(`src/collect/github.rs`), `src/source.rs` 파서 교체; verify: `cargo test` 32 pass + `cargo build` pass
- [x] `C-04` **의존성 교체** - `Cargo.toml`, `Cargo.lock`; verify: `cargo tree`에서 reqwest/url/idna/icu 부재, 패키지 168 → 84
- [x] `C-05` **불변 확인** - 기존 테스트; verify: `cargo test` (32+5 pass), `cargo test --test cli` (5 pass)
- [x] **문서 동기화** - `context.md`; verify: 변경 없음(공개 계약·FCIS 경계·한도 불변)
- [x] **최종 품질 게이트** - verify: `cargo fmt --check` exit 0, `cargo clippy --all-targets -- -D warnings` exit 0
- [x] **수동 smoke(비게이트)** - verify: `cargo run -- https://github.com/octocat/Hello-World` → 1 file, `--ref master` → 1 file, `--ref nope-404` → `GitHub API error: HTTP 422 ...`

## Decisions and deviations

- 2026-09-16: 우선순위 승인 — ① `reqwest` 경량 교체, ② 그 위에서 `url` 제거. (`challenge-docs` 입력)
- 2026-09-16: `D2` = 자체 URL 빌더로 `url` 제거 (조건: `reqwest` 제거 성립 시 실효 절감). 근거: `ureq`는 `url`을 전이로 끌지 않고, 입력이 github.com 전용 + 검증된 ASCII 이름으로 제한되어 자체 인코더 위험이 낮음.
- 2026-09-16: `D1`=A — `ureq` 3 + `features=["rustls","platform-verifier"]`. 근거: 전이 25 ≪ 82, 현재 reqwest의 OS 신뢰 저장소(rustls-platform-verifier) 동작 유지 → `L-01` parity.
- 2026-09-16: 실행 승인 — "권장 우선순위대로 작업해보자" + `D1`=A. status `in-progress`.
- 2026-09-16: `D1`=A 구현 — `ureq::Agent`를 `user_agent(GITHUB_USER_AGENT)` + `timeout_global(30s)` + `http_status_as_error(false)` + `tls_config(TlsConfig::builder().root_certs(RootCerts::PlatformVerifier).build())`로 구성. ureq 기본 root certs가 `WebPki`이므로 reqwest parity를 위해 `PlatformVerifier`를 명시적으로 설정.
- 2026-09-16: 구현 편차 — body 상한 read는 `Body::into_reader().take(limit+1).read_to_end()` + 기존 `classify_body`로 유지. `BodyExceedsLimit` 에러 복구 대신 기존 `Complete/Exceeded` 의미론과 테스트를 그대로 보존.
- 2026-09-16: 구현 편차 — `encode_path_segment`는 unreserved `[A-Za-z0-9-._~]`만 남기고 전부 `%XX`. WHATWG path-segment 세트가 남기는 일부 sub-delim을 추가 인코딩(서버 디코딩 후 동등, 안전한 상위집합).
- 2026-09-16: `D2` 구현 — `src/collect/github.rs`에 `api_url`/`raw_url`/`encode_path_segment`(순수, `pub(crate)`), `src/source.rs::parse_github_url`을 수동 파서로 교체. 대소문자 무관 호스트·userinfo·포트를 허용해 기존 수용 범위 보존; 빈 authority는 `InvalidSource` 유지. WHATWG의 개행/탭 제거는 미재현(희귀 입력, 문서화된 편차).
- 2026-09-16: 문서 동기화 — `context.md`/`README.md` 변경 없음. 공개 계약·FCIS 경계·`MAX_*`·digest 형식 불변, 의존성 선택은 영구 도메인 명세가 아님. ADR 미생성(저장소에 ADR 관례 부재 → 결정은 본 plan에 기록).
- 2026-09-16: 기각 — B(webpki-roots, 신뢰 저장소 동작 변경), C(attohttpc, native-tls/MPL), D(reqwest 유지, 절감 0).
- 2026-09-16: 기각 — HTTP/TLS 직접 구현(보안 경계, 비권장), `ignore`/`libc`/`serde`/`serde_json` 교체(필수 유지).

## Verification

- [x] `cargo test` - 32 lib + 5 cli passed; 0 failed
- [x] `cargo test --test cli` - 5 passed; 0 failed
- [x] `cargo fmt --check` - exit 0
- [x] `cargo clippy --all-targets -- -D warnings` - exit 0
- [x] `cargo tree` 패키지 수 - 168 → 84 (`reqwest`/`url`/`idna`/`icu_*` 부재, `ureq` 3.4.2 + `rustls-platform-verifier`)
- [x] 수동 GitHub smoke (비게이트) - 정상/`--ref`/HTTP 422 에러 경로 통과

## Blockers and open questions

- None. `D1`=A, `D2` 실행 완료. `api_error`의 `StatusCode` Display 형식은 `http` crate 공통으로 동일 확인(422 메시지 검증).
