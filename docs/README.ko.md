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

- **로컬 디렉터리**: `./path/to/project`
- **GitHub 저장소**: `https://github.com/{owner}/{repo}` (끝의 `.git` 허용; 쿼리, 프래그먼트, `user@`, 443 이외 포트 거부).

지원되지 않는 프로토콜(`git@`, `ssh://`, `http://`) 및 GitHub 이외 호스트는 로컬로 먼저 클론해야 합니다:

```bash
git clone --depth 1 <URL> ./repo && codefeed ./repo
```

`codefeed`는 `git`을 직접 실행하거나 저장소를 클론하지 않습니다.

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

옵션은 `<SOURCE>` 앞이나 뒤에 올 수 있습니다. 짧은 플래그는 `=` 문법을 지원합니다(`--format=xml`, `-o=output.txt`).

### 옵션

- `-o`, `--output <PATH|->`: 출력 대상 (기본값: `digest.txt`). `-`는 stdout으로 스트리밍합니다.
- `-r`, `--ref <BRANCH|TAG|COMMIT>`: GitHub 리비전 (기본값: 기본 브랜치). 빈 문자열, 공백 전용, `.`, `..`은 거부됩니다.
- `-f`, `--format <text|xml>`: 다이제스트 포맷 (기본값: `text`).
- `-x`, `--exclude <PATTERN>`: 소스 루트 기준 `.gitignore` 형식 패턴과 일치하는 경로를 제외합니다. 반복 지정 가능하며 지정 순서대로 적용됩니다. 유효하지 않은 패턴 및 규칙을 생성하지 않는 패턴(빈 줄, 주석, 단독 `!`)은 사용법 오류입니다. 문법: [`security_and_filtering.md`](security_and_filtering.md) 3.6절.
- `-h`, `--help`: 사용법을 출력하고 0으로 종료합니다. 다른 인수 오류보다 우선합니다.
- `-V`, `--version`: `codefeed <version>`을 출력하고 0으로 종료합니다. 다른 인수 오류보다 우선합니다.
- `--`: 이후 인수를 위치 인수로 처리합니다.

인라인 값(`--flag=value`)은 `-`로 시작할 수 있습니다(예: `--output=-notes.txt`). `-o -`를 제외하고 분리된 인수 값은 `-`로 시작할 수 없습니다.

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

- **파일 출력** (`-o <PATH>`): 임시 파일 생성, 동기화, 이름 변경을 통한 원자적(atomic) 쓰기. Unix 권한 보존. 상위 디렉터리 동기화 실패 시 실패 없이 stderr에 경고만 출력합니다. stdout에 `Wrote <PATH> (<N> files, <N> skipped, ~<N> tokens)`를 출력합니다(`<PATH>`는 입력값 그대로).
- **스트림 출력** (`-o -`): 다이제스트는 stdout으로, 요약은 stderr로 출력합니다. 실패 시 아무것도 출력하지 않습니다.
- **헤더**: 로컬 다이제스트는 `Repository: <디렉터리 이름>`으로 시작하고, GitHub 다이제스트는 `Repository: https://github.com/{owner}/{repo}`로 시작합니다.
- **텍스트 포맷** (`text`): UTF-8 파일 본문을 그대로 출력합니다. 바이너리(NUL) 또는 유효하지 않은 UTF-8 파일은 건너뜁니다. 헤더 레이블과 경로는 이스케이프되며, ANSI 시퀀스는 유지됩니다.
- **XML 포맷** (`xml`): `<repository>`, `<directory_structure>`, `<files>` 요소로 구조화됩니다. 엔티티는 이스케이프되며, XML 1.0 미허용 문자는 `\u{FFFD}`로 변환됩니다.
- **토큰 수**: 휴리스틱 추정치(토큰당 ASCII 바이트 비율([security 8절](security_and_filtering.md#8-limit-values)) + 비-ASCII 문자당 1토큰; 비어있지 않은 다이제스트는 최소 1, 빈 다이제스트는 0)이며 엄밀한 토크나이저 카운트가 아닙니다.
- **종료 코드**: 성공, 도움말, 버전 출력 시 `0`; 사용법 또는 수집 실패 시 `1`. stdout 파이프가 닫히면(broken pipe) `0`으로 정상 종료합니다.

## 포함 및 제외 규칙

제외된 항목은 경고 없이 생략되며 건너뛴 항목(skipped)에 집계되지 않습니다(예외: tar 헤더 크기 초과는 제외 항목이어도 실패; [`security_and_filtering.md`](security_and_filtering.md) 2절):

- 숨김 항목: `.`으로 시작하는 경로 세그먼트(소스 루트 제외).
- 심볼릭 링크 및 비정규 파일(로컬 및 아카이브 공통).
- 의존성 잠금 파일(3.2절).
- 민감한 인증 정보 파일(접두사 또는 확장자 일치, 3.3절).
- 저장소 내 `.gitignore` 규칙에 일치하는 파일(로컬 전용; 안전하지 않거나 읽을 수 없는 `.gitignore`는 실패, 3.5절).
- `--exclude` 패턴에 일치하는 경로(3.6절).

건너뛴 항목(요약 줄에 skipped로 집계):

- `MAX_FILE_SIZE`를 초과하는 파일.
- 바이너리(NUL 바이트) 또는 유효하지 않은 UTF-8 파일.
- `MAX_PATH_BYTES` 초과 또는 비-UTF-8 경로.
- 읽기 불가 파일(권한 거부, 잠금 등).

> **참고:** 필터링은 경로와 파일명만 검사하며 파일 내용은 검사하지 않습니다. 완전한 규칙: [`security_and_filtering.md`](security_and_filtering.md) 3절.

## 제한값

제한값 도달 시 항목을 건너뛰거나 전체 실행을 중단합니다. 수치와 적용 세부사항: [`security_and_filtering.md`](security_and_filtering.md) 8절. 타임아웃 및 재시도: 6.5-6.6절.

| 제한값 | 도달 시 동작 |
| :--- | :--- |
| `MAX_FILE_SIZE` | 파일 건너뜀 및 요약에 집계; `.gitignore` 초과 시 실행 중단. |
| `MAX_PATH_BYTES` | 파일 건너뜀 및 요약에 집계. |
| `MAX_PATH_DEPTH` | 실행 중단. |
| `MAX_FILE_COUNT` | 실행 중단; 오류 메시지에 가장 큰 디렉터리 목록 표시. |
| `MAX_OUTPUT_SIZE` | 실행 중단; 다이제스트 미출력; 가장 큰 파일 목록 표시. |
| `ARCHIVE_MAX_HEADER_BYTES` | 실행 중단. |
| `ARCHIVE_MAX_DECOMPRESSED_BYTES` | 실행 중단. |
| `MAX_DIR_ENTRIES` | 실행 중단. |

## 인증

공개 저장소는 인증이 필요하지 않습니다. 비공개 저장소 접근 또는 속도 제한 상향 시 자격 증명을 읽습니다:

- `GITHUB_TOKEN` (우선 확인).
- `GH_TOKEN` (`GITHUB_TOKEN`이 없거나 빈 값일 때 폴백).

토큰은 `https://api.github.com`에만 `Authorization: Bearer <token>`으로 전송되며 리다이렉트 대상으로는 전송되지 않습니다.

## 네트워크 신뢰

GitHub TLS 인증서는 내장된 Mozilla 루트 인증서만을 엄격히 신뢰합니다. OS 인증서 저장소나 프록시 CA는 거부됩니다.

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
