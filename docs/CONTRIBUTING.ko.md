# codefeed 기여 가이드라인

[English](../CONTRIBUTING.md) | **한국어**

> 이 문서는 [`CONTRIBUTING.md`](../CONTRIBUTING.md)의 번역본입니다. 내용이 다르면 `CONTRIBUTING.md`가 우선합니다.

개발 환경 설정, 기여 절차, 필수 검증 요구사항 및 프로젝트 규칙을 안내합니다.

## 행동 강령 (Code of Conduct)

모든 참여자는 [`CODE_OF_CONDUCT.ko.md`](CODE_OF_CONDUCT.ko.md)를 준수해야 합니다. 건설적인 피드백, 기술적 토론 및 협력적인 문제 해결에 집중합니다.

## 시작하기

### 사전 요구사항

- **Rust**: [`Cargo.toml`](../Cargo.toml)의 `rust-version`(MSRV) 이상 (`rustc --version` / `rustup update stable`).
- **Git**: 버전 관리 도구.

### 로컬 빌드 및 테스트

```bash
git clone https://github.com/but212/codefeed.git
cd codefeed
cargo build
cargo test --locked
```

## 기여 방법

### 1. 버그 제보

[기존 GitHub Issues](https://github.com/but212/codefeed/issues)를 먼저 확인합니다. 새로운 버그인 경우 다음 정보를 포함하여 이슈를 등록합니다:

- 실행한 명령어 또는 코드
- 기대 동작 및 실제 동작
- 운영체제 및 Rust 버전 (`rustc --version`)
- 재현 절차 또는 테스트한 대상 저장소

### 2. 새 기능 및 아키텍처 변경 제안

- 기능 추가, CLI 변경, 주요 리팩토링은 코드 작성 전 이슈를 통해 먼저 논의합니다.
- 오탈자 수정 및 회귀 테스트가 포함된 사소한 버그 수정은 사전 이슈 없이 PR을 제출할 수 있습니다.

### 3. 보안 취약점 제보

공개 이슈를 생성하지 마세요. [`SECURITY.md`](../SECURITY.md)를 따릅니다.

## Pull Request 워크플로우

1. **브랜치 명명**: `main` 기반 토픽 브랜치 생성 (`feat/name`, `fix/description`, `docs/topic`).
2. **커밋 메시지 규칙**: [Conventional Commits](https://www.conventionalcommits.org/) 준수 (`feat:`, `fix:`, `refactor:`, `test:`, `docs:`, `chore:`).
3. **로컬 사전 검증**: PR 생성 전 필수 검증을 통과해야 합니다.
4. **PR 제출**: `main`을 대상으로 [Pull Request Template](../.github/PULL_REQUEST_TEMPLATE.md)을 작성하여 제출합니다.

## 필수 검증 체크리스트

모든 Pull Request는 다음 4단계 검증을 순서대로 통과해야 합니다:

```bash
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo doc --no-deps --locked
cargo test --locked
```

작업 중에는 가장 좁은 대상부터 실행합니다 (`cargo test --locked <name>`). 변경 사항 및 회귀에 대한 결정론적(deterministic) 테스트를 작성합니다 (TDD).

## 아키텍처 및 저장소 가이드라인

`codefeed`는 엄격한 관심사 분리와 단일 소유권 원칙을 적용합니다:

- **코어 및 쉘**: [`context.md`](../context.md) 및 [`coding_convention.md`](coding_convention.md) (`src/core/` 순수 로직 vs. `src/shell/` I/O).
- **보안 및 제한값**: [`security_and_filtering.md`](security_and_filtering.md) (필터링 규칙 및 제한값 미러의 단일 소유자).
- **Rust 컨벤션**: [`coding_convention.md`](coding_convention.md) (`#[must_use]`, 전용 오류 타입, 불변성).
- **변경 이력**: 관찰 가능한 동작 및 기여자 워크플로우 변경 사항은 [`CHANGELOG.md`](../CHANGELOG.md)의 `## [Unreleased]`에 기록합니다.
