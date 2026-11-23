# codefeed 기여 가이드라인

[English](../CONTRIBUTING.md) | **한국어**

> 이 문서는 [`CONTRIBUTING.md`](../CONTRIBUTING.md)의 번역본입니다. 내용이 다르면 `CONTRIBUTING.md`가 우선합니다.

**상위 원칙:**

- **단일 소유권:** 행동 강령, 보안, 아키텍처, 코딩 및 변경 이력 규칙은 아래 정본 문서가 단독으로 소유합니다. 이 문서는 기여 워크플로우와 필수 검증 게이트만 정의합니다.
- **재현 가능한 도구체인:** Git과 [`Cargo.toml`](../Cargo.toml)의 `rust-version`(MSRV) 이상 Rust를 사용합니다. CI와 일치하도록 Cargo 명령은 `--locked`로 실행합니다(의존성 변경은 CI `cargo deny` 검사 대상입니다).
- **범위 기반 사전 합의:** 기능 추가, CLI 변경, 주요 리팩토링은 코드 작성 전 이슈로 먼저 논의합니다. 회귀 테스트가 포함된 독립적인 오탈자·버그 수정은 사전 이슈 없이 PR을 제출할 수 있습니다.
- **최소 재현 제보:** 버그 제보 전 [기존 GitHub Issues](https://github.com/but212/codefeed/issues)를 확인하고, 최소 재현 정보(실행한 명령어 또는 코드, 기대 동작 및 실제 동작, 운영체제, `rustc --version`)를 포함합니다. 보안 취약점은 절대 공개 이슈로 등록하지 말고 [`SECURITY.md`](../SECURITY.md)를 따릅니다.
- **표준 Git 및 PR 위생:** `main` 기반 토픽 브랜치(`<type>/<topic>`)에서 작업하고, [Conventional Commits](https://www.conventionalcommits.org/)를 준수하며, `main`을 대상으로 [Pull Request Template](../.github/PULL_REQUEST_TEMPLATE.md)을 작성해 PR을 제출합니다.

## 필수 검증 체크리스트

모든 Pull Request는 다음 4단계 검증을 순서대로 통과해야 합니다:

```bash
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --locked
cargo test --locked
```

작업 중에는 가장 좁은 대상부터 실행합니다 (`cargo test --locked <name>`). 회귀 및 계약 변경에는 결정론적(deterministic) 단위 또는 통합 테스트가 필수입니다 (TDD; [`coding_convention.md`](coding_convention.md#7-testing) 참조).

## 정본 문서 참조

| 문서 | 소유 범위 |
| :--- | :--- |
| [`CODE_OF_CONDUCT.ko.md`](CODE_OF_CONDUCT.ko.md) | 협업, 기술적 토론 및 중재 기준 |
| [`SECURITY.md`](../SECURITY.md) | 지원 버전 및 비공개 취약점 제보 |
| [`context.md`](../context.md) | 도메인 용어, 불변식 및 모듈 경계 |
| [`coding_convention.md`](coding_convention.md) | Rust 컨벤션, 함수형 코어 경계, 오류, 문서화 및 테스트 |
| [`security_and_filtering.md`](security_and_filtering.md) | 위협 모델, 제외 규칙, 제한값 미러 및 파일시스템/네트워크 안전성 |
| [`CHANGELOG.md`](../CHANGELOG.md) | `## [Unreleased]`에 기록할 변경 이력 범위 |
