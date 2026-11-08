# codefeed

[![Crates.io](https://img.shields.io/crates/v/codefeed.svg)](https://crates.io/crates/codefeed)
[![CI](https://github.com/but212/codefeed/actions/workflows/ci.yml/badge.svg)](https://github.com/but212/codefeed/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](../LICENSE-MIT)
[![License: Apache 2.0](https://img.shields.io/badge/license-Apache_2.0-blue.svg)](../LICENSE-APACHE)
[![MSRV](https://img.shields.io/badge/rustc-1.88%2B-orange.svg)](../Cargo.toml)
[![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/but212/codefeed)

[English](../README.md) | **한국어**

> 이 문서는 [`README.md`](../README.md)의 번역본입니다. 내용이 다르면 `README.md`가 우선합니다.

`codefeed`는 로컬 디렉터리나 공개 GitHub 저장소를 LLM을 위한 단일 텍스트 다이제스트로 변환하는 빠른 CLI 도구입니다.

## 소스

- **로컬 디렉터리**: `./path/to/project`
- **GitHub 저장소**: `https://github.com/{owner}/{repo}` (끝의 `.git` 허용; 쿼리, 프래그먼트, 사용자 자격 증명 `user@`, 443 이외의 포트는 거부됨).

지원되지 않는 프로토콜(`git@`, `ssh://`, `http://`) 및 GitHub 이외 호스트(예: GitLab, Bitbucket, 자체 호스팅 Git 서버)는 먼저 로컬로 클론해야 합니다:

```bash
git clone --depth 1 <URL> ./repo && codefeed ./repo
```

`codefeed`는 `git`을 직접 실행하거나 저장소를 클론하지 않습니다.

## 설치

```bash
cargo install --path .
```

또는 [`cargo-binstall`](https://github.com/cargo-bins/cargo-binstall)로 GitHub Releases의 사전 빌드 바이너리를 설치합니다:

```bash
cargo binstall codefeed
```

또는 릴리스 바이너리를 빌드합니다:

```bash
cargo build --release
./target/release/codefeed ./my-repo
```

Rust 1.88+ 필요.

## 사용법

```bash
codefeed <SOURCE> [--output <PATH|->] [--ref <REF>] [--format <text|xml>] [--exclude <PATTERN>]...
```

옵션은 `<SOURCE>` 앞이나 뒤에 올 수 있습니다. 짧은 플래그는 `=` 문법을 지원합니다(`--format=xml`, `-o=output.txt`).

### 옵션

- `-o`, `--output <PATH|->`: 출력 대상 (기본값: `digest.txt`). `-`는 stdout으로 스트리밍합니다.
- `-r`, `--ref <BRANCH|TAG|COMMIT>`: GitHub 리비전 (기본값: 기본 브랜치). 빈 문자열, 공백만 있는 문자열, `.`, `..`은 거부됩니다.
- `-f`, `--format <text|xml>`: 다이제스트 형식 (기본값: `text`).
- `-x`, `--exclude <PATTERN>`: 소스 루트 기준 `.gitignore` 스타일 패턴과 일치하는 경로를 제외합니다(예: `build/`, `*.log`, `/vendor`). 여러 번 지정할 수 있으며, 지정된 순서대로 로컬 및 GitHub 소스에 적용됩니다. 유효하지 않은 패턴이나 규칙을 만들지 않는 패턴(빈 줄, 주석, 단독 `!`)은 사용법 오류입니다. 문법 세부 사항은 [`security_and_filtering.md`](security_and_filtering.md) 3.6절을 참조하세요.
- `-h`, `--help`: 사용법 출력 후 0으로 종료. 다른 인수 오류보다 우선합니다.
- `-V`, `--version`: `codefeed <버전>` 출력 후 0으로 종료. 다른 인수 오류보다 우선합니다.
- `--`: 이후 인수를 위치 인수로 처리(`-`로 시작하는 디렉터리 경로용).

인라인 값(`--flag=value`)은 `-`로 시작할 수 있습니다(예: `--output=-notes.txt`). `-o -`를 제외하고 별도 분리된 인수 값은 `-`로 시작할 수 없습니다.

### 예제

```bash
# 로컬 디렉터리를 digest.txt로 변환
codefeed ./my-repo

# 사용자 지정 파일, 짧은 플래그 사용
codefeed ./my-repo -o ./digest.txt -f xml

# stdout으로 스트리밍하여 다른 도구로 파이프 연결
codefeed --format=xml --output=- ./my-repo | your-tool

# 기본 브랜치의 GitHub 저장소 변환
codefeed https://github.com/rust-lang/cargo

# 특정 ref의 GitHub 저장소 변환
codefeed https://github.com/rust-lang/cargo --ref master -o ./cargo-digest.txt

# 하이픈(-)으로 시작하는 디렉터리
codefeed -- -my-directory
```

## 출력

- **파일 출력** (`-o <PATH>`): 임시 파일 생성, 동기화(sync), 이름 변경(rename)을 통한 원자적(atomic) 쓰기. Unix 권한을 보존합니다. 부모 디렉터리 동기화 실패 시 오류로 중단되지 않고 stderr에 경고만 출력합니다. stdout에 `Wrote <PATH> (<N> files, <N> skipped, ~<N> tokens)`를 출력하며, 여기서 `<PATH>`는 입력받은 `-o` 값 그대로 표시됩니다.
- **스트림 출력** (`-o -`): 다이제스트는 stdout으로, 요약 정보는 stderr로 출력됩니다. 실패 시 아무것도 출력하지 않습니다.
- **헤더**: 로컬 다이제스트는 `Repository: <디렉터리 이름>`으로 시작합니다. GitHub 다이제스트는 `Repository: https://github.com/{owner}/{repo}`로 시작합니다.
- **텍스트 형식** (`text`): UTF-8 파일 내용이 그대로 포함되며, 바이너리(NUL 바이트 포함) 파일이나 잘못된 UTF-8 파일은 건너뜁니다. 헤더 레이블과 경로는 이스케이프 처리되며 ANSI 시퀀스는 유지됩니다.
- **XML 형식** (`xml`): 구조화된 `<repository>`, `<directory_structure>`, `<files>` 요소로 구성됩니다. 엔티티는 이스케이프 처리되며 XML 1.0에서 허용되지 않는 문자는 `\u{FFFD}`로 대체됩니다.
- **토큰 수**: 휴리스틱 추정치(ASCII 바이트 / `ASCII_BYTES_PER_TOKEN` + 비-ASCII 문자 수)이며, 정밀한 토크나이저 계산 결과가 아닙니다.
- **종료 상태 코드**: 성공, 도움말 또는 버전 출력 시 `0`, 사용법 오류 또는 수집(ingest) 오류 시 `1`.
- **파이프 중단(Broken pipe)**: stdout 리더가 일찍 닫힌 경우 `0`으로 정상 종료합니다.

## 포함 및 제외 기준

제외된 항목은 알림 없이 조용히 무시되며 건너뛴(skipped) 파일 수에 포함되지 않습니다. 예외: 크기를 초과하는 tar 헤더는 제외 항목이더라도 실행을 실패시킵니다([`security_and_filtering.md`](security_and_filtering.md) 2절):

- 숨김 항목: `.`으로 시작하는 경로 세그먼트(예: `.git`, `.github`, `.env*`), 단 소스 루트 자체는 제외.
- 심볼릭 링크 및 비정규(non-regular) 파일 (로컬 및 아카이브 공통).
- 의존성 락파일 (3.2절).
- 이름 접두사 또는 확장자로 판별하는 자격 증명·키 파일, 대소문자 구분 없음 (3.3절).
- 트리 내 `.gitignore`에 일치하는 파일 (로컬 소스에만 적용되며 GitHub 아카이브에는 `.gitignore`가 적용되지 않음). 안전하지 않거나 읽을 수 없는 로컬 `.gitignore`는 실행을 실패시킵니다 (3.5절).
- `--exclude` 패턴과 일치하는 경로 (제외된 디렉터리 하위의 모든 항목 포함) (3.6절).

위의 절 번호는 [`security_and_filtering.md`](security_and_filtering.md)를 가리킵니다.

건너뛴(skipped) 항목은 요약 정보에 집계됩니다:

- `MAX_FILE_SIZE`를 초과하는 파일.
- 바이너리(NUL 바이트 포함) 파일 또는 잘못된 UTF-8 파일.
- `MAX_PATH_BYTES`를 초과하거나 UTF-8이 아닌 경로.
- 읽을 수 없는 파일 (권한 없음, 잠김 등).

> **참고:** 필터링은 경로와 파일 이름만 검사하며 파일 내용은 검사하지 않습니다. 추적 대상 소스 파일 내에 하드코딩된 비밀 정보를 `codefeed`가 제거해 줄 것으로 신뢰하지 마세요. 전체 규칙은 [`security_and_filtering.md`](security_and_filtering.md) 3절에 기술되어 있습니다.

## 제한 사항

제한 기준에 도달하면 해당 항목을 건너뛰거나 전체 실행을 중단합니다. 구체적인 제한 수치와 적용 세부사항은 [`security_and_filtering.md`](security_and_filtering.md) 8절을 참조하세요.

| 제한 | 도달 시 |
| :--- | :--- |
| `MAX_FILE_SIZE` | 파일을 건너뛰고 요약의 건너뛴 수에 집계. 제한을 넘는 `.gitignore`는 실행을 중단. |
| `MAX_PATH_BYTES` | 파일을 건너뛰고 요약의 건너뛴 수에 집계. |
| `MAX_PATH_DEPTH` | 실행 실패. |
| `MAX_FILE_COUNT` | 실행 실패; 가장 큰 디렉터리 목록 출력. |
| `MAX_OUTPUT_SIZE` | 실행 실패; 출력 미작성 및 가장 큰 파일 목록 출력. |
| `ARCHIVE_MAX_HEADER_BYTES` | 실행 실패. |
| `ARCHIVE_MAX_DECOMPRESSED_BYTES` | 실행 실패. |
| `MAX_DIR_ENTRIES` | 실행 실패. |
| `ARCHIVE_MAX_SECS`, `GITHUB_TIMEOUT_SECS` | 요청 시간 초과; 헤더 타임아웃은 재시도 ([6.5-6.6절](security_and_filtering.md#6-github-network-safety)). |
| `MAX_ATTEMPTS`, `RETRY_INITIAL_DELAY_MS`, `RETRY_MAX_DELAY_MS` | 일시적 실패는 백오프로 재시도; 대기 시간이 상한을 초과하면 중단 ([6.5절](security_and_filtering.md#6-github-network-safety)). |

## 인증

공개 저장소는 자격 증명이 필요 없습니다. 비공개 저장소 접근 또는 속도 제한 상향 시 다음 환경 변수를 읽습니다:

- `GITHUB_TOKEN` (우선 검사).
- `GH_TOKEN` (`GITHUB_TOKEN`이 설정되지 않았거나 비어 있거나 공백만 있는 경우 대체).

토큰은 `Authorization: Bearer <token>` 헤더로 `https://api.github.com`에만 전송되며, 리다이렉트 시 전달되지 않습니다.

## 네트워크 신뢰

GitHub TLS 인증서는 바이너리에 번들된 Mozilla 루트 인증서로만 엄격히 검증합니다. OS 인증서 저장소 및 프록시 CA는 거부됩니다.

## 문서

| 문서 | 대상 독자 | 내용 |
| :--- | :--- | :--- |
| [`context.md`](../context.md) | 유지보수자 | 도메인 용어, 불변식, 모듈 경계 |
| [`security_and_filtering.md`](security_and_filtering.md) | 보안 검토자 | 위협 모델, 제외 규칙, 제한 수치, 파일시스템 및 네트워크 안전성 |
| [`coding_convention.md`](coding_convention.md) | 기여자 | Rust 컨벤션, 함수형 코어 경계, 오류 처리, 문서화, 테스트 |
| [`AGENTS.md`](../AGENTS.md) | 코딩 에이전트 | 필수 지침, 검증 명령어, 변경 사항 라우팅 |
| [`README.md`](../README.md) | 영어 독자 | 이 문서의 원본; 내용이 다르면 원본이 우선 |
| [`CHANGELOG.md`](../CHANGELOG.md) | 사용자 / 소비자 | 릴리스 이력 및 주요 변경점 |

## 라이선스

[MIT](../LICENSE-MIT) 또는 [Apache 2.0](../LICENSE-APACHE) 듀얼 라이선스가 적용됩니다.
