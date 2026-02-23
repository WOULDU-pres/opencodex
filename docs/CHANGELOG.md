# Changelog

## 2026-02-23 — UX 개선 업데이트 (비개발자 요약)

이번 업데이트는 **텔레그램 봇을 더 안전하고, 더 이해하기 쉽게** 쓰도록 개선한 변경입니다.

### 사용자가 바로 체감하는 변화

- **`/status` 명령 추가**
  - 현재 작업 폴더, 세션 상태, AI 동작 여부, 앱 버전을 한 번에 확인할 수 있습니다.
- **첫 소유자 등록 안내 메시지 추가**
  - 처음 봇을 등록한 사용자는 이제 “소유자로 등록됨” 안내를 바로 받습니다.
- **권한 없는 사용자 안내 강화**
  - 비공개 상태에서 접근하면 이제 무응답이 아니라 안내 메시지를 받습니다.
- **도움말(`/help`) 한국어 개선**
  - 주요 명령 설명을 한국어 중심으로 정리해 이해하기 쉬워졌습니다.

### 안정성/보안 개선

- **쉘 명령(`!`) 60초 제한**
  - 오래 걸리는 명령이 무한 대기하지 않도록 자동 중단됩니다.
- **`/stop` 중단 기능 강화**
  - AI 작업뿐 아니라 실행 중인 쉘 명령도 함께 중단할 수 있습니다.
- **입력 처리 강화**
  - 위험 패턴 필터링 시 사용자에게 안내하고, 입력 길이 상한을 **4,000 → 16,000자**로 확대했습니다.
- **대화 기록 자동 정리**
  - 히스토리는 최근 100개까지만 유지해 과도한 누적을 방지합니다.
- **오래된 세션 파일 자동 정리**
  - 30일 지난 세션 파일은 시작 시 자동 정리됩니다.
- **환경 변수/실행 경고 개선**
  - `.env` 자동 로드 지원, `--madmax` 사용 시 강한 경고 출력, 백엔드 CLI 미설치 시 안내를 제공합니다.

### 운영/배포 개선

- **릴리즈 워크플로우 추가**
  - `v*` 태그 기준으로 Linux/macOS 바이너리 빌드/업로드 자동화가 추가되었습니다.

### 검증 결과

- `cargo build --release` 통과
- `cargo test` **61개 테스트 통과**
- `cargo clippy --all-targets -- -D warnings` 통과
- `cargo fmt --check` 통과

## 2026-02-23 — 보안 강화 + 코드 구조 개선

openclaude 프로젝트의 HANDOFF.md를 참고하여 opencodex에 보안 강화 및 코드 품질 개선을 적용했습니다.

---

### 보안 (Security)

#### [신규] auth.rs 보안 모듈 추가

파일: `src/auth.rs` (17개 테스트 포함)

| 기능 | 설명 |
|------|------|
| 권한 모델 | Owner / Public / Denied 3단계 |
| 명령 위험 등급 | Low → Medium → High → Critical 4단계 분류 |
| 경로 검증 | `is_path_within_sandbox()` — 심볼릭 링크/traversal 공격 차단 |
| 업로드 제한 | `DEFAULT_UPLOAD_LIMIT` = 50MB |

#### [보안 수정] 파일 업로드 권한 우회 (P0)

**문제**: Public 그룹 사용자가 auth 권한 체크를 거치지 않고 파일을 업로드할 수 있었습니다.
파일 업로드 처리가 권한 검사 코드보다 앞에 위치하여, 그룹 채팅에서 `/public on` 상태일 때
비 Owner 사용자도 서버 파일시스템에 파일을 쓸 수 있는 상태였습니다.

**원인**: `handle_message()` 에서 파일 업로드 분기 (line 141)가 auth 게이트 (line 255)보다
먼저 실행되는 코드 순서 문제.

**수정**: 파일 업로드 처리 직전에 `is_owner` 체크 추가. Owner가 아니면 즉시 거부.

파일: `src/telegram/commands.rs`

#### [보안 수정] 설정 파일 권한 노출 (P1)

**문제**: `config.json`과 `bot_settings.json`에 Telegram 봇 토큰이 평문 저장되는데,
파일 권한이 기본값(644)으로 생성되어 같은 서버의 다른 사용자가 읽을 수 있었습니다.

**수정**: Unix에서 파일 쓰기 직후 `0o600` 권한 자동 적용 (본인만 읽기/쓰기).

파일: `src/main.rs`, `src/telegram/storage.rs`

#### [보안 수정] 입력 새니타이징 대소문자 우회 (P1)

**문제**: `sanitize_user_input()`이 정확한 대소문자만 필터링하여,
`SYSTEM PROMPT`, `System Prompt`, `sYsTeM pRoMpT` 등 변형으로 우회 가능했습니다.

**수정**: `to_lowercase()` 변환 후 비교하되 원본 문자열의 오프셋을 보존하는 방식으로 재작성.
12개 테스트 케이스 추가 (대문자, 혼합, 변형 등).

파일: `src/session.rs`

#### [보안 수정] 파일 업로드 용량 미제한 (P2)

**문제**: 파일 업로드 시 크기 제한이 없어 대용량 파일로 서버 디스크를 채울 수 있었습니다.

**수정**: `auth::DEFAULT_UPLOAD_LIMIT` (50MB) 초과 시 업로드 거부.

파일: `src/telegram/file_ops.rs`

---

### 코드 구조 (Architecture)

#### [리팩토링] telegram.rs 모듈 분리

**변경 전**: `src/telegram.rs` 단일 파일 2,636줄
**변경 후**: `src/telegram/` 디렉터리 내 8개 파일

| 파일 | 역할 | 줄 수 |
|------|------|-------|
| `mod.rs` | 모듈 선언, re-export | ~10 |
| `bot.rs` | 상태 타입 정의 | ~75 |
| `commands.rs` | 명령 디스패치 | ~830 |
| `file_ops.rs` | 파일 업/다운, 쉘 실행 | ~270 |
| `message.rs` | AI 스트리밍 처리 | ~530 |
| `storage.rs` | 파일 I/O | ~260 |
| `streaming.rs` | 메시지 변환 유틸 | ~515 |
| `tools.rs` | 도구 관리 | ~260 |

공개 API 변경 없음 (`run_bot`, `resolve_token_by_hash`).

#### [기능] /cd 경로 자동 저장

**변경 전**: `/cd`로 디렉터리를 변경해도 서버 재시작 시 이전 경로를 잊음.
**변경 후**: `/cd` 실행 시 `bot_settings.json`의 `last_sessions`에 자동 저장.
다음 서버 시작 시 자동 복원.

파일: `src/telegram/commands.rs`

---

### 코드 품질 (Quality)

#### [신규] CI 파이프라인

GitHub Actions 자동 검증 추가:

| 검사 | 명령 |
|------|------|
| 포맷 | `cargo fmt --check` |
| 품질 | `cargo clippy --all-targets -- -D warnings` |
| 테스트 | `cargo test` |
| 보안 감사 | `cargo audit` |

파일: `.github/workflows/ci.yml`, `.rustfmt.toml`

#### [수정] clippy 경고 전량 해결

28개 경고 → 0개. 주요 수정 항목:

| 경고 유형 | 수정 방법 |
|-----------|-----------|
| `unsafe_code` (3건) | `#[allow(unsafe_code)]` + SAFETY 주석 (SIGTERM 전송 목적) |
| `dead_code` (7건) | `#[allow(dead_code)]` (미래 사용 대비 공개 API) |
| `derivable_impls` | `#[derive(Default)]` 사용 |
| `needless_borrows` (5건) | `&format!(...)` → `format!(...)` |
| `if_same_then_else` | 동일 분기 병합 |
| `question_mark` | `let...else` → `?` 연산자 |
| `manual_strip` | `strip_prefix()` 메서드 사용 |
| `implicit_saturating_sub` | `saturating_sub()` 사용 |
| `unwrap_used` / `expect_used` | `if let Ok(...)` 패턴 또는 `#[allow]` |

---

### 테스트 현황

총 56개 테스트 통과:

| 모듈 | 테스트 수 | 주요 커버리지 |
|------|-----------|--------------|
| `auth` | 17 | 권한 모델, 명령 분류, 경로 검증, 업로드 제한 |
| `codex` | 27 | 백엔드 인자 생성, JSON 파싱, 세션 ID 검증 |
| `session` | 12 | 입력 새니타이징 (대소문자, 패턴, 변형, 절삭) |
