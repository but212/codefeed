# codefeed

[![Crates.io](https://img.shields.io/crates/v/codefeed.svg)](https://crates.io/crates/codefeed)
[![CI](https://github.com/but212/codefeed/actions/workflows/ci.yml/badge.svg)](https://github.com/but212/codefeed/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](../LICENSE-MIT)
[![License: Apache 2.0](https://img.shields.io/badge/license-Apache_2.0-blue.svg)](../LICENSE-APACHE)
[![MSRV](https://img.shields.io/badge/rustc-1.88%2B-orange.svg)](../Cargo.toml)
[![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/but212/codefeed)

[English](../README.md) | **한국어**

> 이 문서는 [`README.md`](../README.md)의 번역본입니다. 내용이 다르면 `README.md`가 우선합니다.

로컬 디렉터리나 공개 GitHub 저장소를 LLM을 위한 단일 텍스트 다이제스트로 변환하는 빠른 CLI 도구입니다.

## 소스

`codefeed`는 `git`을 실행하거나 저장소를 클론하지 않습니다. 허용되는 소스:

- **로컬 디렉터리**: `./path/to/project`
- **GitHub 저장소**: 정규 `https://github.com/{owner}/{repo}` (끝의 `.git` 선택 허용; `user@`, 쿼리, 프래그먼트, 443 이외 포트는 거부).

그 외 프로토콜이나 호스트는 로컬로 먼저 클론해야 합니다:

```bash
git clone --depth 1 <URL> ./repo && codefeed ./repo
```

## 설치

```bash
cargo install codefeed     # crates.io에서 빌드
cargo binstall codefeed    # 선택: 사전 빌드 바이너리 (cargo-binstall 필요)
cargo install --path .     # 로컬 체크아웃에서 빌드
cargo build --release      # ./target/release/codefeed 에 릴리스 바이너리 빌드
```

Rust 1.88+ 필요.

## 사용법

```bash
codefeed <SOURCE> [--output <PATH|->] [--ref <REF>] [--format <text|xml>] [--exclude <PATTERN>]...
```

### 옵션

- `-o`, `--output <PATH|->`: 출력 대상 (기본값: `digest.txt`; `-`는 stdout으로 스트리밍).
- `-r`, `--ref <BRANCH|TAG|COMMIT>`: GitHub 리비전 (기본값: 기본 브랜치). 빈 문자열, 공백 전용, `.`, `..`은 거부됩니다.
- `-f`, `--format <text|xml>`: 다이제스트 포맷 (기본값: `text`).
- `-x`, `--exclude <PATTERN>`: 소스 루트 기준 `.gitignore` 형식 패턴과 일치하는 경로를 제외합니다(반복 지정 가능, 지정 순서대로 적용; 유효하지 않거나 빈 패턴은 사용법 오류; 문법: [`security_and_filtering.md`](security_and_filtering.md) 3.6절).
- `-h`, `--help`: 사용법을 출력하고 `0`으로 종료합니다.
- `-V`, `--version`: `codefeed <version>`을 출력하고 `0`으로 종료합니다.
- `--`: 이후 인수를 위치 인수로 처리합니다.

**파싱 규칙:** 옵션은 `--` 이전까지 `<SOURCE>` 앞뒤 어디든 올 수 있습니다. 값 옵션은 인라인 값(`--format=xml`, `-o=output.txt`; `-`로 시작 허용, 예: `--output=-notes.txt`) 또는 다음 토큰(`-`로 시작 불가, 단 `-o -` 예외)으로 값을 받으며, 분리된 값 위치의 토큰은 옵션으로 해석되지 않습니다(`--output --help`처럼 값이 거부되는 경우 포함). 옵션으로 파싱된 토큰 중 최초의 `-h`/`--help` 또는 `-V`/`--version`이 적용되며 다른 인수 오류보다 우선합니다.

### 예시

```bash
codefeed ./my-repo
codefeed ./my-repo -o ./digest.txt -f xml
codefeed --format=xml --output=- ./my-repo | your-tool
codefeed https://github.com/rust-lang/cargo
codefeed https://github.com/rust-lang/cargo --ref master -o ./cargo-digest.txt
codefeed -- -my-directory
```

## 출력

- **버퍼링 후 일괄 전달**: 수집과 렌더링이 메모리에서 완료된 뒤에만 출력하며, 실패 시 다이제스트를 생성하지 않습니다.
  - **파일** (`-o <PATH>`): Unix 권한을 보존하는 원자적(atomic) 교체(상위 디렉터리 동기화 실패 시 중단 없이 경고; [security 5절](security_and_filtering.md#5-local-filesystem-safety)). stdout에 `Wrote <PATH> (<N> files, <N> skipped, ~<N> tokens)`를 출력합니다(`<PATH>`는 입력값 그대로).
  - **스트림** (`-o -`): 다이제스트는 stdout, 요약은 stderr로 출력합니다(전송 중 쓰기 실패 시 부분 스트림이 남을 수 있음).
- **헤더**: 로컬 소스는 `Repository: <디렉터리 이름>`, GitHub 소스는 `Repository: https://github.com/{owner}/{repo}`.
- **포맷**: `text`는 헤더와 경로만 이스케이프하고 UTF-8 파일 본문을 원문 그대로 출력합니다(ANSI 유지). `xml`은 `<repository>`, `<directory_structure>`, `<files>` 구조에 XML 1.0 전체 이스케이프를 적용합니다([security 7절](security_and_filtering.md#7-output-sanitization)).
- **토큰 수**: 휴리스틱 추정치(`floor(ASCII 바이트 / 비율)`([security 8절](security_and_filtering.md#8-limit-values)) + 비-ASCII 문자당 1토큰; 비어 있지 않은 다이제스트는 최소 1, 빈 다이제스트는 0)이며 엄밀한 토크나이저 카운트가 아닙니다.
- **종료 코드**: 성공, 도움말, 버전 출력, 또는 broken stdout pipe 시 `0`; 사용법 또는 수집 오류 시 `1`.

## 포함 및 제외 규칙

모든 항목은 상호 배타적인 세 가지 결과 중 하나로 분류됩니다([`security_and_filtering.md`](security_and_filtering.md) 2–4절):

- **제외(Excluded)** (경고 없이 생략, skipped 미집계, 내용 미읽기): 비정규 항목(심볼릭 링크 등)과 경로·이름 기반 제외 규칙 — 숨김 세그먼트(`.` 접두사, 소스 루트 제외), 의존성 잠금 파일(3.2절), 민감한 인증 정보·키 파일명(3.3절), 저장소 내 `.gitignore`(로컬 전용, 3.5절), `--exclude`(3.6절). 필터링은 파일 내용을 검사하지 않으므로 평범한 이름의 파일에 든 비밀은 `.gitignore` 또는 `--exclude`로 제외하십시오(1.2절).
- **건너뜀(Skipped)** (요약에 집계): 항목별 검사에 실패한 정규 파일 — `MAX_FILE_SIZE` 또는 `MAX_PATH_BYTES` 초과, 비-UTF-8 경로, 바이너리(NUL) 또는 유효하지 않은 UTF-8 본문, 읽기 불가·잠금 파일.
- **포함(Included)**: 나머지 모든 유효한 UTF-8 정규 파일.

## 제한값

수치와 적용 세부사항: [`security_and_filtering.md`](security_and_filtering.md#8-limit-values) 8절. 타임아웃 및 재시도: 6.5–6.6절.

- **항목별 한도는 건너뜀** (요약에 집계): `MAX_FILE_SIZE`, `MAX_PATH_BYTES`.
- **구조·아카이브·전역 한도는 실행 중단**: `MAX_PATH_DEPTH`(빈 로컬 디렉터리 포함), `MAX_DIR_ENTRIES`, `MAX_FILE_COUNT`(오류에 가장 큰 디렉터리 목록 표시), `MAX_OUTPUT_SIZE`(오류에 가장 큰 파일 목록 표시), `ARCHIVE_MAX_HEADER_BYTES`(제외 대상 항목에도 적용), `ARCHIVE_MAX_DECOMPRESSED_BYTES`.
- **제어 파일 예외**: 안전하지 않거나 읽을 수 없거나 `MAX_FILE_SIZE`를 초과하는 `.gitignore` 파일은 건너뛰지 않고 실행을 중단합니다(3.5절).

## GitHub 접근 및 신뢰

- **인증**: 공개 저장소는 인증이 필요하지 않습니다. 비공개 저장소 접근 또는 속도 제한 상향 시 `GITHUB_TOKEN`을 우선 확인하고, 없거나 빈 값·공백 전용이면 `GH_TOKEN`으로 폴백합니다.
- **엄격한 출처 및 TLS**: 토큰은 `https://api.github.com`에만 `Authorization: Bearer <token>`으로 전송되며 리다이렉트 대상으로는 전송되지 않습니다. GitHub TLS는 내장된 Mozilla 루트 인증서만 신뢰하며 OS 저장소나 프록시 CA는 거부됩니다([security 6절](security_and_filtering.md#6-github-network-safety)).

## 문서

| 문서 | 대상 독자 | 내용 |
| :--- | :--- | :--- |
| [`context.md`](../context.md) | 유지보수자 | 도메인 용어, 불변식, 모듈 경계 |
| [`security_and_filtering.md`](security_and_filtering.md) | 보안 검토자 | 위협 모델, 제외 규칙, 제한값, 파일시스템/네트워크 안전성 |
| [`coding_convention.md`](coding_convention.md) | 기여자 | Rust 컨벤션, 함수형 코어 경계, 오류, 문서화, 테스트 |
| [`CONTRIBUTING.ko.md`](CONTRIBUTING.ko.md) | 기여자 | 개발 환경 설정, 기여 절차, 필수 검증 체크리스트 |
| [`SECURITY.md`](../SECURITY.md) | 보안 제보자 | 지원 버전, 비공개 취약점 제보 |
| [`AGENTS.md`](../AGENTS.md) | 코딩 에이전트 | 필수 지침, 문서 수정 라우팅 |
| [`README.md`](../README.md) | 영문 독자 | 원본 문서; 번역 충돌 시 원본 우선 |
| [`CHANGELOG.md`](../CHANGELOG.md) | 사용자 / 라이브러리 소비자 | 릴리스 내역 및 변경 사항 |

## 기여 및 라이선스

[`CONTRIBUTING.ko.md`](CONTRIBUTING.ko.md)에 따라 기여할 수 있습니다. [MIT](../LICENSE-MIT) 또는 [Apache 2.0](../LICENSE-APACHE) 듀얼 라이선스가 적용됩니다.
