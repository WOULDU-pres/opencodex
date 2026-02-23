# opencodex

Telegram 채팅으로 AI(Codex)에게 코딩을 시킬 수 있는 서버입니다.

**한 줄 요약**: 서버 컴퓨터에서 `opencodex`를 실행하면, Telegram 봇을 통해 어디서든 AI에게 코드 작성/수정/실행을 요청할 수 있습니다.

---

## 처음 시작하기

### 1단계: 필요한 것 준비

| 준비물 | 설명 | 확인 방법 |
|--------|------|-----------|
| **Telegram Bot** | @BotFather에서 봇 생성 후 토큰 복사 | 토큰: `123456789:ABC...` 형태 |
| **Codex CLI** | AI 백엔드 (OpenAI) | `codex --version` |
| **Rust** | 빌드 도구 | `cargo --version` |

> Rust가 없다면: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`

### 2단계: 빌드

```bash
# 소스 폴더로 이동
cd ~/workspace/opencodex

# 빌드 (처음에 시간이 좀 걸립니다)
cargo build --release
```

빌드가 끝나면 실행 파일이 생깁니다: `./target/release/opencodex`

어디서든 쓸 수 있게 설치하려면:
```bash
cargo install --path . --force --bin opencodex
```

### 3단계: 실행

```bash
# 첫 실행 (토큰을 한 번만 입력하면 자동 저장됩니다)
opencodex ~/my-project --token "여기에_봇토큰_붙여넣기"

# 이후 실행 (토큰 자동으로 불러옴)
opencodex ~/my-project
```

서버가 시작되면 Telegram에서 봇에게 말을 걸면 됩니다.

> **첫 메시지를 보낸 사람이 봇의 주인(Owner)으로 등록됩니다.** 다른 사람은 사용할 수 없습니다.

---

## 사용법 (Telegram에서)

### 기본 사용

봇에게 **일반 텍스트**를 보내면 AI가 답변합니다:

```
이 프로젝트의 README를 한국어로 작성해줘
```

```
src/main.rs에서 에러가 나는데 고쳐줘
```

### 명령어 목록

| 명령어 | 하는 일 | 예시 |
|--------|---------|------|
| `/help` | 도움말 보기 | `/help` |
| `/start 경로` | 작업 폴더 지정 | `/start ~/my-project` |
| `/pwd` | 현재 작업 폴더 확인 | `/pwd` |
| `/cd 경로` | 작업 폴더 변경 | `/cd ~/other-project` |
| `/clear` | AI 대화 초기화 | `/clear` |
| `/stop` | AI 응답 중단 | `/stop` |
| `/down 파일` | 서버에서 파일 받기 | `/down src/main.rs` |
| `!명령어` | 서버에서 쉘 명령 실행 | `!ls -la` |

### 파일 업로드

Telegram에서 파일이나 사진을 보내면 현재 작업 폴더에 자동 저장됩니다.

### 도구 관리 (AI가 사용할 수 있는 도구)

| 명령어 | 하는 일 |
|--------|---------|
| `/availabletools` | 사용 가능한 전체 도구 목록 |
| `/allowedtools` | 현재 허용된 도구 목록 |
| `/allowed +Bash` | Bash 도구 추가 |
| `/allowed -Bash` | Bash 도구 제거 |

### 그룹 채팅에서 사용

그룹에 봇을 초대한 뒤:
- `;메시지` — AI에게 메시지 보내기 (세미콜론으로 시작)
- `/public on` — 그룹 멤버 전원 사용 허용
- `/public off` — Owner만 사용 (기본값)

---

## 실행 옵션

```bash
# 기본 (Codex 백엔드)
opencodex ~/my-project

# OMX 백엔드 사용
opencodex ~/my-project --omx

# 모든 제한 해제 (주의!)
opencodex ~/my-project --madmax
```

### 토큰 우선순위

토큰은 아래 순서로 찾습니다 (위가 우선):

1. `--token "토큰"` (직접 입력)
2. `OPENCODEX_TELEGRAM_TOKEN` 환경변수
3. `TELEGRAM_BOT_TOKEN` 환경변수
4. `~/.opencodex/config.json` 저장값

한 번 입력하면 자동 저장되므로 이후에는 입력하지 않아도 됩니다.

---

## 보안

### 누가 사용할 수 있나요?

| 권한 | 할 수 있는 것 | 대상 |
|------|--------------|------|
| **Owner** | 모든 기능 | 처음 메시지 보낸 사람 (자동 등록) |
| **Public** | `/help`, `/pwd` 등 읽기만 | 그룹에서 `/public on` 시 |
| **차단** | 아무것도 못 함 | 그 외 모든 사용자 |

### 자동 보호 기능

- 사용자 입력에서 위험한 패턴 자동 제거 (프롬프트 인젝션 방어)
- 파일 경로 조작 공격 차단 (`../../etc/passwd` 같은 시도 방지)
- 파일 업로드 50MB 제한
- 설정 파일에 본인만 읽기/쓰기 권한 자동 적용 (Linux/macOS)

---

## 저장되는 파일

| 파일 | 내용 |
|------|------|
| `~/.opencodex/config.json` | 봇 토큰 |
| `~/.opencodex/bot_settings.json` | Owner 정보, 세션 기록 |
| `~/.opencodex/sessions/*.json` | AI 대화 히스토리 |

---

## 개발자 정보

### 프로젝트 구조

```
src/
├── main.rs            # 시작점 (CLI 옵션 처리)
├── auth.rs            # 보안 (권한, 경로 검증, 업로드 제한)
├── codex.rs           # AI 백엔드 연결 (Codex/OMX)
├── session.rs         # 세션 관리, 입력 필터링
├── app.rs             # 설정 디렉터리 이름
└── telegram/
    ├── mod.rs         # 모듈 선언
    ├── bot.rs         # 상태 관리 타입
    ├── commands.rs    # 명령어 처리
    ├── file_ops.rs    # 파일 업/다운로드, 쉘 실행
    ├── message.rs     # AI 스트리밍 응답 처리
    ├── storage.rs     # 설정/세션 파일 읽기/쓰기
    ├── streaming.rs   # Telegram 메시지 변환
    └── tools.rs       # 도구 관리
```

### CI (자동 검증)

코드 푸시 시 GitHub Actions가 자동 실행:
- 코드 포맷 검사 (`cargo fmt`)
- 코드 품질 검사 (`cargo clippy`)
- 테스트 실행 (`cargo test`)
- 보안 취약점 검사 (`cargo audit`)

### 테스트 실행

```bash
cargo test
```

---

## 변경 이력

자세한 변경 사항은 [docs/CHANGELOG.md](docs/CHANGELOG.md)를 참조하세요.
