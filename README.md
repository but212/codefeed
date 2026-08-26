# rustingest

로컬 디렉터리 또는 public GitHub 저장소를 하나의 LLM 친화적인 텍스트 파일로 변환합니다.

## 사용 방법

Rust와 Cargo가 설치되어 있어야 합니다. 프로젝트 루트에서 다음 형식으로 실행합니다.

```bash
cargo run -- ingest <SOURCE> [--output <PATH>] [--ref <BRANCH|TAG|COMMIT>]
```

### 로컬 디렉터리

```bash
cargo run -- ingest ./my-repo
cargo run -- ingest /path/to/project --output ./digest.txt
```

### public GitHub 저장소

```bash
cargo run -- ingest https://github.com/rust-lang/cargo
cargo run -- ingest https://github.com/rust-lang/cargo --ref master --output ./cargo-digest.txt
```

### 옵션

- `<SOURCE>`: 로컬 디렉터리 경로 또는 `https://github.com/{owner}/{repo}` URL입니다.
- `--output <PATH>`: digest를 저장할 경로입니다. 생략하면 현재 디렉터리의 `digest.txt`를 사용합니다.
- `--ref <BRANCH|TAG|COMMIT>`: GitHub 저장소에서 가져올 브랜치, 태그 또는 커밋입니다. 생략하면 기본 브랜치를 사용하며, 로컬 입력에는 사용할 수 없습니다.
- `--help`, `-h`: 사용법을 출력합니다.

릴리스 바이너리를 빌드한 뒤 직접 실행할 수도 있습니다.

```bash
cargo build --release
target/release/rustingest ingest ./my-repo --output digest.txt
```

실행이 끝나면 포함된 파일 수와 건너뛴 파일 수가 출력됩니다. GitHub 입력은 public 저장소만 지원하며 Git executable은 사용하지 않습니다.

## 필터 정책

- `.git`, hidden path, symlink, binary 파일, UTF-8이 아닌 경로·파일을 제외합니다.
- 로컬 `.gitignore`는 Git 저장소 여부와 관계없이 입력 디렉터리 내부 규칙을 적용하며, `.ignore`는 적용하지 않습니다.
- GitHub 저장소의 중첩 `.gitignore`도 해당 디렉터리 범위와 negation 우선순위를 적용합니다. `.gitignore`는 최대 256개, 누적 8 MiB까지 읽으며 제한을 초과하면 오류를 반환합니다.
- 1 MiB보다 큰 파일은 본문을 읽지 않고 건너뛰며, hidden/ignored 항목을 제외한 파일 후보가 10,000개를 초과하면 오류를 반환합니다.
- digest metadata와 파일 내용을 합한 출력이 20 MiB를 초과하면 내용을 output에 커밋하지 않고 오류를 반환합니다.
- GitHub API 응답은 크기 제한을 넘으면 잘린 JSON을 파싱하지 않고 명시적 오류를 반환합니다.
- 파일 경로를 사전순으로 정렬해 결정론적인 digest를 생성하며, digest header에서는 백슬래시와 줄바꿈 문자를 escape합니다.
- 출력은 충돌 방지 임시 파일에 쓴 뒤 동기화하고 성공 시 atomic rename합니다. 실패하면 기존 output을 보존합니다.

`--help` 또는 `-h`는 usage를 출력하고 성공 코드로 종료합니다. 잘못된 인자와 ingest 오류는 오류 메시지를 stderr에 출력하고 non-zero 코드로 종료합니다.
