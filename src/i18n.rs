pub const MSG_OWNER_REGISTERED: &str =
    "✅ 봇 소유자로 등록되었습니다.\n/help 로 사용 가능한 명령어를 확인하세요.";
pub const MSG_PRIVATE_BOT: &str = "이 봇은 비공개입니다. 봇 소유자에게 문의하세요.";
pub const MSG_NO_SESSION: &str =
    "세션이 없습니다. /start <폴더경로> 로 시작하세요.\n예: /start ~/my-project";
pub const MSG_AI_BUSY: &str = "AI가 작업 중입니다. /stop 으로 중단할 수 있습니다.";
pub const MSG_SESSION_CLEARED: &str = "세션이 초기화되었습니다.";
pub const MSG_NO_ACTIVE_REQUEST: &str = "진행 중인 AI 요청이 없습니다.";
pub const MSG_FILTER_NOTICE: &str = "⚠ 일부 내용이 보안 필터에 의해 수정되었습니다.";
pub const MSG_NO_RESPONSE: &str = "(응답 없음)";
pub const MSG_SHELL_TIMEOUT: &str = "명령 실행 시간 초과 (60초 제한)";
pub const MSG_STOPPING: &str = "중단 중...";

pub const HELP_TEXT_TEMPLATE: &str = "\
<b>{app} 텔레그램 봇</b>
서버 파일 관리와 AI 대화를 지원합니다. (<code>--omx</code> 사용 시 OMX 경유)

<b>세션</b>
<code>/start &lt;path&gt;</code> — 지정 경로에서 세션 시작
<code>/start</code> — 시작 시 전달된 기본 프로젝트 경로로 세션 시작
<code>/pwd</code> — 현재 작업 경로 확인
<code>/cd &lt;path&gt;</code> — 작업 경로 변경
<code>/status</code> — 런타임 상태 확인
<code>/clear</code> — AI 대화 히스토리 초기화
<code>/stop</code> — 진행 중인 AI/쉘 작업 중단

<b>파일 전송</b>
<code>/down &lt;file&gt;</code> — 서버 파일 다운로드
파일/사진 전송 — 현재 세션 경로로 업로드

<b>쉘</b>
<code>!&lt;command&gt;</code> — 쉘 명령 직접 실행 (최대 60초)
예: <code>!ls -la</code>, <code>!git status</code>

<b>AI 대화</b>
일반 메시지는 설정된 AI 백엔드로 전달됩니다.
AI는 세션 경로 내에서 파일 읽기/수정/명령 실행을 수행할 수 있습니다.

<b>도구 관리</b>
<code>/availabletools</code> — 사용 가능한 전체 도구 목록
<code>/allowedtools</code> — 현재 허용된 도구 목록
<code>/allowed +name</code> — 도구 추가 (예: <code>/allowed +Bash</code>)
<code>/allowed -name</code> — 도구 제거

<b>그룹 채팅</b>
<code>;</code><i>메시지</i> — AI에게 메시지 전송
<code>;</code><i>caption</i> — 파일 업로드와 함께 AI 프롬프트 전달
<code>/public on</code> — 그룹 멤버 전체 사용 허용
<code>/public off</code> — 소유자만 사용 (기본값)

<code>/help</code> — 도움말 표시";
