# opencodex

`opencodex`는 **Telegram ↔ Codex/OMX CLI**를 연결하는 독립형 서버입니다.

핵심 동작:
- `opencodex <project_dir>` 한 줄로 서버 시작
- 기본 백엔드는 Codex(최적화 경로)로 실행
- `--omx` 플래그를 주면 Codex 대신 OMX 바이너리를 직접 실행
- `--madmax` 플래그로 approvals/sandbox 완전 우회 실행
- 첫 DM 사용자 Owner 임프린트 (기본 접근 제어)
- 일반 텍스트 → Codex 스트리밍 응답
- `/help`, `/pwd`, `/stop`, `/down <file>`, `!<shell>` 지원
- 세션 저장(기본): `~/.opencodex/sessions/*.json`
- 내부 파일 전송: `opencodex --sendfile <path> --chat <id> --key <hash>`

> **참고**: `opencodex`와 `openclaude`는 완전히 분리된 독립 프로젝트입니다.
> 각각 자체 바이너리, 설정 디렉터리, Telegram 토큰을 사용합니다.

---

## 1) 사전 준비

1. Telegram Bot Token 발급 (@BotFather)
2. Codex CLI 설치 및 로그인 (`codex --version` 확인, 필요시 `omx --version` 확인)
3. Rust toolchain 설치 (`~/.cargo/bin/cargo --version` 확인)

---

## 2) 빌드

```bash
cd ~/workspace/opencodex
~/.cargo/bin/cargo check
~/.cargo/bin/cargo build --release
```

실행 파일:
```bash
./target/release/opencodex --help
```

원하면 전역 설치:
```bash
~/.cargo/bin/cargo install --path ~/workspace/opencodex --force --bin opencodex
```

> `--bin opencodex`를 붙이면 이 프로젝트의 바이너리만 설치됩니다.

---

## 3) 실행 방법

토큰 우선순위:
1. `--token <TOKEN>`
2. `OPENCODEX_TELEGRAM_TOKEN`
3. `TELEGRAM_BOT_TOKEN`
4. `~/.opencodex/config.json` 저장값

토큰이 CLI 인자 또는 환경변수로 들어오면 `~/.opencodex/config.json`에 자동 저장됩니다.
실행 전 Telegram `getMe` 검증을 수행하므로 잘못된 토큰이면 즉시 종료됩니다.

### 최초 실행 예시

```bash
opencodex ~/workspace/my-project --token "123456789:ABCDEF..."
```

### OMX로 실행

```bash
opencodex ~/workspace/my-project --omx
```

`--omx`로 시작하면 채팅 실행 백엔드는 `codex`가 아니라 `omx`를 사용합니다.
서버 시작 로그에 `ai_backend: omx (--omx)`로 표시됩니다.

### madmax 모드로 실행

```bash
opencodex ~/workspace/my-project --madmax
```

### OMX 팀 작업 호출

팀 스킬/병렬 작업은 Telegram 메시지에서 직접 실행되는 게 아니라,
서버가 호출하는 CLI 세션 내부에서 `omx team ...`으로 실행합니다.

예시:

```bash
omx team 3:executor "작업 설명"
omx team ralph "end-to-end 작업 설명"
```

### 이후 실행 (토큰 생략 가능)

```bash
opencodex ~/workspace/my-project
```

---

## 4) Telegram 명령

- `/help` : 도움말
- `/pwd` : 현재 작업 경로
- `/stop` : 현재 AI 요청 중단
- `/down <file>` : 파일 다운로드
- `!<command>` : 프로젝트 디렉터리에서 쉘 실행

일반 텍스트 메시지는 기본적으로 Codex로 전달되며, `--omx`로 시작한 경우 OMX로 전달됩니다.

---

## 5) 저장 파일

- `~/.opencodex/config.json` : 기본 토큰
- `~/.opencodex/bot_settings.json` : owner, 토큰 해시 매핑, 마지막 세션 정보
- `~/.opencodex/sessions/*.json` : 대화 세션 히스토리

---

## 6) OpenSpec

요청에 따라 OpenSpec 초기화 완료:
- `openspec/` 디렉터리 생성
- `.codex/` OpenSpec 명령/스킬 생성

필요 시:
```bash
cd ~/workspace/opencodex
openspec status
```

---

## 7) 참고 문서 복사

요청대로 아래 파일을 루트에 복사 완료:
- `AGENT.md`
- `WORKFLOW.md`
- `AGENTS.md`
