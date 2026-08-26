# local_ingest

로컬 디렉터리 또는 public GitHub 저장소를 하나의 LLM 친화적인 텍스트 파일로 변환합니다.

## 사용법

```bash
cargo run -- ingest <SOURCE> [--output <PATH>] [--ref <BRANCH|TAG|COMMIT>]
```

예시:

```bash
cargo run -- ingest ./my-repo
cargo run -- ingest https://github.com/rust-lang/cargo --ref master --output digest.txt
```

`--output`을 생략하면 현재 디렉터리의 `digest.txt`에 기록합니다. GitHub 입력은 `https://github.com/{owner}/{repo}` 형식의 public 저장소만 지원하며 Git executable은 사용하지 않습니다.

## 필터 정책

- `.git`, hidden path, symlink, binary 파일, UTF-8이 아닌 경로·파일을 제외합니다.
- 로컬 `.gitignore`는 Git 저장소 여부와 관계없이 입력 디렉터리 내부 규칙을 적용합니다.
- GitHub 저장소의 중첩 `.gitignore`도 해당 디렉터리 범위와 negation 우선순위를 적용합니다.
- 1 MiB보다 큰 파일은 건너뛰며, hidden/ignored 항목을 제외한 파일 후보가 10,000개를 초과하면 오류를 반환합니다.
- digest metadata와 파일 내용을 합한 출력이 20 MiB를 초과하면 내용을 output에 커밋하지 않고 오류를 반환합니다.
- GitHub API 응답은 크기 제한을 넘으면 잘린 JSON을 파싱하지 않고 명시적 오류를 반환합니다.
- 파일 경로를 사전순으로 정렬해 결정론적인 digest를 생성합니다.
- 출력은 충돌 방지 임시 파일에 쓴 뒤 동기화하고 성공 시 atomic rename합니다. 실패하면 기존 output을 보존합니다.

`--help` 또는 `-h`는 usage를 출력하고 성공 코드로 종료합니다. 잘못된 인자와 ingest 오류는 오류 메시지를 stderr에 출력하고 non-zero 코드로 종료합니다.
