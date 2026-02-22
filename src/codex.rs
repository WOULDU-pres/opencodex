use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

/// Cached path to selected AI binary.
static AI_BINARY_PATH: OnceLock<Option<String>> = OnceLock::new();

#[derive(Debug, Clone, Copy, Default)]
struct ExecutionOptions {
    use_omx: bool,
    madmax: bool,
}

static EXECUTION_OPTIONS: OnceLock<ExecutionOptions> = OnceLock::new();

pub fn configure_execution(use_omx: bool, madmax: bool) {
    let _ = EXECUTION_OPTIONS.set(ExecutionOptions { use_omx, madmax });
}

fn execution_options() -> &'static ExecutionOptions {
    EXECUTION_OPTIONS.get_or_init(ExecutionOptions::default)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackendKind {
    Codex,
    Omx,
}

fn backend_kind() -> BackendKind {
    if execution_options().use_omx {
        BackendKind::Omx
    } else {
        BackendKind::Codex
    }
}

fn ai_binary_name() -> &'static str {
    match backend_kind() {
        BackendKind::Codex => "codex",
        BackendKind::Omx => "omx",
    }
}

/// Resolve path to selected executable.
/// First tries `which <binary>`, then falls back to `bash -lc "which <binary>"`
/// for environments where shell init files are required.
fn resolve_ai_binary_path() -> Option<String> {
    let binary = ai_binary_name();

    if let Ok(output) = Command::new("which").arg(binary).output() {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Some(path);
            }
        }
    }

    if let Ok(output) = Command::new("bash")
        .args(["-lc", &format!("which {}", binary)])
        .output()
    {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Some(path);
            }
        }
    }

    None
}

fn get_ai_binary_path() -> Option<&'static str> {
    AI_BINARY_PATH
        .get_or_init(resolve_ai_binary_path)
        .as_deref()
}

/// Debug logging helper (active only when COKACDIR_DEBUG=1)
fn debug_log(msg: &str) {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    let enabled = ENABLED.get_or_init(|| {
        std::env::var("COKACDIR_DEBUG")
            .map(|v| v == "1")
            .unwrap_or(false)
    });
    if !*enabled {
        return;
    }

    let Some(home) = dirs::home_dir() else {
        return;
    };

    let log_path = home
        .join(crate::app::dir_name())
        .join("debug")
        .join(format!("{}.log", ai_binary_name()));
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&log_path) {
        let timestamp = chrono::Local::now().format("%H:%M:%S%.3f");
        let _ = writeln!(file, "[{}] {}", timestamp, msg);
    }
}

#[derive(Debug, Clone)]
pub struct CodexResponse {
    pub success: bool,
    pub response: Option<String>,
    pub session_id: Option<String>,
    pub error: Option<String>,
}

/// Streaming message types for real-time Codex/OMX responses
#[derive(Debug, Clone)]
pub enum StreamMessage {
    /// Initialization - contains thread/session ID
    Init { session_id: String },
    /// Text response chunk
    Text { content: String },
    /// Tool use started
    ToolUse { name: String, input: String },
    /// Tool execution result
    ToolResult { content: String, is_error: bool },
    /// Background task notification
    TaskNotification {
        task_id: String,
        status: String,
        summary: String,
    },
    /// Completion
    Done {
        result: String,
        session_id: Option<String>,
    },
    /// Error
    Error { message: String },
}

/// Token for cooperative cancellation of streaming requests.
/// Holds a flag and the child process PID so the caller can terminate it.
pub struct CancelToken {
    pub cancelled: std::sync::atomic::AtomicBool,
    pub child_pid: std::sync::Mutex<Option<u32>>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self {
            cancelled: std::sync::atomic::AtomicBool::new(false),
            child_pid: std::sync::Mutex::new(None),
        }
    }
}

/// Cached regex pattern for session/thread ID validation
fn session_id_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"^[a-zA-Z0-9_-]+$").expect("Invalid session ID regex"))
}

/// Validate session/thread ID format
fn is_valid_session_id(session_id: &str) -> bool {
    !session_id.is_empty() && session_id.len() <= 64 && session_id_regex().is_match(session_id)
}

/// Default allowed tools configuration.
/// Kept for Telegram-side tool allow/deny UX compatibility.
pub const DEFAULT_ALLOWED_TOOLS: &[&str] = &[
    "Bash",
    "Read",
    "Edit",
    "Write",
    "Glob",
    "Grep",
    "Task",
    "TaskOutput",
    "TaskStop",
    "WebFetch",
    "WebSearch",
    "NotebookEdit",
    "Skill",
    "TaskCreate",
    "TaskGet",
    "TaskUpdate",
    "TaskList",
];

fn default_system_prompt() -> &'static str {
    r#"You are a terminal coding assistant running through Codex/OMX CLI.
Be concise. Focus on practical, safe, non-interactive execution.
Respond in the same language as the user.

SECURITY RULES (MUST FOLLOW):
- NEVER execute destructive commands like rm -rf, format, mkfs, dd, etc.
- NEVER modify system files in /etc, /sys, /proc, /boot
- NEVER execute commands that could harm the system or compromise security
- If a request seems dangerous, explain the risk and suggest a safer alternative

BASH EXECUTION RULES (MUST FOLLOW):
- All commands MUST run non-interactively without user input
- Use -y, --yes, or --non-interactive flags where applicable
- Use -m flag for commit messages (e.g. git commit -m \"message\")
- Disable pagers with --no-pager or pipe to cat
- NEVER use commands that open editors (vim, nano, etc.)
- NEVER use commands that wait for stdin without arguments
- NEVER use interactive flags like -i"#
}

fn build_full_prompt(
    prompt: &str,
    system_prompt: Option<&str>,
    allowed_tools: Option<&[String]>,
) -> String {
    let mut sections: Vec<String> = Vec::new();

    let effective_system_prompt = match system_prompt {
        None => Some(default_system_prompt()),
        Some("") => None,
        Some(p) => Some(p),
    };

    if let Some(sp) = effective_system_prompt {
        sections.push(format!("SYSTEM:\n{}", sp));
    }

    if let Some(tools) = allowed_tools {
        if !tools.is_empty() {
            sections.push(format!(
                "TOOL CONSTRAINT:\nOnly use the following tools when needed: {}",
                tools.join(", ")
            ));
        }
    }

    sections.push(prompt.to_string());
    sections.join("\n\n")
}

fn codex_args(session_id: Option<&str>, working_dir: &str) -> Result<Vec<String>, String> {
    let mut args = vec!["-C".to_string(), working_dir.to_string()];

    if execution_options().madmax {
        args.push("--dangerously-bypass-approvals-and-sandbox".to_string());
    } else {
        args.push("--sandbox".to_string());
        args.push("danger-full-access".to_string());
        args.push("-a".to_string());
        args.push("never".to_string());
    }

    args.push("exec".to_string());

    if let Some(sid) = session_id {
        if !is_valid_session_id(sid) {
            return Err("Invalid session ID format".to_string());
        }
        args.push("resume".to_string());
        args.push(sid.to_string());
        args.push("--json".to_string());
        args.push("-".to_string());
    } else {
        args.push("--json".to_string());
        args.push("--skip-git-repo-check".to_string());
        args.push("-".to_string());
    }

    Ok(args)
}

fn omx_args(session_id: Option<&str>, working_dir: &str) -> Result<Vec<String>, String> {
    // Keep OMX invocation direct (`omx ...`) but pass Codex-compatible exec flags.
    // OMX forwards these to Codex while preserving OMX behaviors (team/HUD modes).
    let mut args = vec!["--cd".to_string(), working_dir.to_string()];

    if execution_options().madmax {
        // OMX-native madmax alias.
        args.push("--madmax".to_string());
    } else {
        args.push("--sandbox".to_string());
        args.push("danger-full-access".to_string());
        args.push("-a".to_string());
        args.push("never".to_string());
    }

    args.push("exec".to_string());

    if let Some(sid) = session_id {
        if !is_valid_session_id(sid) {
            return Err("Invalid session ID format".to_string());
        }
        args.push("resume".to_string());
        args.push(sid.to_string());
        args.push("--json".to_string());
        args.push("-".to_string());
    } else {
        args.push("--json".to_string());
        args.push("--skip-git-repo-check".to_string());
        args.push("-".to_string());
    }

    Ok(args)
}

fn backend_args(
    backend: BackendKind,
    session_id: Option<&str>,
    working_dir: &str,
) -> Result<Vec<String>, String> {
    match backend {
        BackendKind::Codex => codex_args(session_id, working_dir),
        BackendKind::Omx => omx_args(session_id, working_dir),
    }
}

#[derive(Debug)]
struct StreamingAttemptOutcome {
    done_sent: bool,
    last_session_id: Option<String>,
    status_success: bool,
    status_code: Option<i32>,
    stderr_output: String,
    emitted_message_count: usize,
}

#[derive(Debug)]
enum StreamingAttemptState {
    Completed(StreamingAttemptOutcome),
    Cancelled,
}

fn is_retryable_resume_error(stderr_output: &str) -> bool {
    let lower = stderr_output.to_lowercase();
    let known_patterns = [
        "failed to resume",
        "could not resume",
        "cannot resume",
        "can't resume",
        "unable to resume",
        "invalid value for '--resume'",
        "invalid value for \"--resume\"",
        "thread not found",
        "session not found",
        "conversation not found",
        "unknown thread",
        "unknown session",
        "no such thread",
        "no such session",
        "expired thread",
        "expired session",
    ];

    if known_patterns.iter().any(|pattern| lower.contains(pattern)) {
        return true;
    }

    let has_resume_context = lower.contains("--resume") || lower.contains("resume ");
    let has_missing_or_invalid_hint = lower.contains("not found")
        || lower.contains("does not exist")
        || lower.contains("unknown")
        || lower.contains("no such")
        || lower.contains("expired")
        || lower.contains("invalid");

    has_resume_context && has_missing_or_invalid_hint
}

fn execute_command_streaming_once(
    ai_bin: &str,
    binary_name: &str,
    args: &[String],
    full_prompt: &str,
    working_dir: &str,
    sender: &Sender<StreamMessage>,
    cancel_token: Option<std::sync::Arc<CancelToken>>,
) -> Result<StreamingAttemptState, String> {
    let mut child = Command::new(ai_bin)
        .args(args)
        .current_dir(working_dir)
        .env_remove("CLAUDECODE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to start {}: {}", binary_name, e))?;

    if let Some(ref token) = cancel_token {
        *token.child_pid.lock().unwrap() = Some(child.id());
    }

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(full_prompt.as_bytes())
            .map_err(|e| format!("Failed to write prompt to {} stdin: {}", binary_name, e))?;
    }

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Failed to capture stdout".to_string())?;

    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Failed to capture stderr".to_string())?;

    let stderr_handle = std::thread::spawn(move || {
        let mut buf = String::new();
        let mut reader = BufReader::new(stderr);
        let _ = reader.read_to_string(&mut buf);
        buf
    });

    let mut reader = BufReader::new(stdout);
    let mut line_buf = String::new();
    let mut last_session_id: Option<String> = None;
    let mut done_sent = false;
    let mut emitted_message_count: usize = 0;

    loop {
        if let Some(ref token) = cancel_token {
            if token.cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                debug_log("Cancel detected — killing AI process");
                let _ = child.kill();
                let _ = child.wait();
                return Ok(StreamingAttemptState::Cancelled);
            }
        }

        line_buf.clear();
        let read = reader
            .read_line(&mut line_buf)
            .map_err(|e| format!("Failed to read {} output: {}", binary_name, e))?;

        if read == 0 {
            break;
        }

        let line = line_buf.trim();
        if line.is_empty() {
            continue;
        }

        debug_log(&format!("line: {}", line));

        let Ok(json) = serde_json::from_str::<Value>(line) else {
            continue;
        };

        let parsed = parse_codex_stream_line(&json);
        for mut msg in parsed {
            match &mut msg {
                StreamMessage::Init { session_id } => {
                    last_session_id = Some(session_id.clone());
                }
                StreamMessage::Done {
                    session_id,
                    result: _,
                } => {
                    if session_id.is_none() {
                        *session_id = last_session_id.clone();
                    }
                    done_sent = true;
                }
                StreamMessage::Text { .. }
                | StreamMessage::ToolUse { .. }
                | StreamMessage::ToolResult { .. }
                | StreamMessage::TaskNotification { .. }
                | StreamMessage::Error { .. } => {}
            }

            if sender.send(msg).is_err() {
                debug_log("Receiver dropped while streaming; stopping send loop");
                break;
            }

            emitted_message_count += 1;
        }
    }

    if let Some(ref token) = cancel_token {
        if token.cancelled.load(std::sync::atomic::Ordering::Relaxed) {
            debug_log("Cancel detected after stdout loop — killing AI process");
            let _ = child.kill();
            let _ = child.wait();
            return Ok(StreamingAttemptState::Cancelled);
        }
    }

    let status = child
        .wait()
        .map_err(|e| format!("{} process wait failed: {}", binary_name, e))?;
    let stderr_output = stderr_handle.join().unwrap_or_else(|_| "".to_string());

    Ok(StreamingAttemptState::Completed(StreamingAttemptOutcome {
        done_sent,
        last_session_id,
        status_success: status.success(),
        status_code: status.code(),
        stderr_output,
        emitted_message_count,
    }))
}

/// Execute a command using the selected AI backend (Codex by default, OMX with --omx)
pub fn execute_command(
    prompt: &str,
    session_id: Option<&str>,
    working_dir: &str,
    allowed_tools: Option<&[String]>,
) -> CodexResponse {
    let (tx, rx) = mpsc::channel();

    let run_result = execute_command_streaming(
        prompt,
        session_id,
        working_dir,
        tx,
        None,
        allowed_tools,
        None,
    );

    if let Err(e) = run_result {
        return CodexResponse {
            success: false,
            response: None,
            session_id: None,
            error: Some(e),
        };
    }

    let mut response = String::new();
    let mut final_session_id = session_id.map(String::from);
    let mut saw_error: Option<String> = None;

    for msg in rx {
        match msg {
            StreamMessage::Init { session_id } => {
                final_session_id = Some(session_id);
            }
            StreamMessage::Text { content } => {
                if !response.is_empty() {
                    response.push('\n');
                }
                response.push_str(&content);
            }
            StreamMessage::Done { result, session_id } => {
                if response.trim().is_empty() && !result.trim().is_empty() {
                    response = result;
                }
                if session_id.is_some() {
                    final_session_id = session_id;
                }
            }
            StreamMessage::Error { message } => {
                saw_error = Some(message);
            }
            StreamMessage::ToolUse { .. }
            | StreamMessage::ToolResult { .. }
            | StreamMessage::TaskNotification { .. } => {}
        }
    }

    if let Some(error) = saw_error {
        return CodexResponse {
            success: false,
            response: if response.trim().is_empty() {
                None
            } else {
                Some(response)
            },
            session_id: final_session_id,
            error: Some(error),
        };
    }

    CodexResponse {
        success: true,
        response: Some(response.trim().to_string()),
        session_id: final_session_id,
        error: None,
    }
}

/// Check if selected AI backend CLI is available
pub fn is_codex_available() -> bool {
    #[cfg(not(unix))]
    {
        false
    }

    #[cfg(unix)]
    {
        get_ai_binary_path().is_some()
    }
}

/// Backward-compatible alias.
pub fn is_claude_available() -> bool {
    is_codex_available()
}

/// Check if platform supports AI features
pub fn is_ai_supported() -> bool {
    cfg!(unix)
}

/// Execute a command using the selected AI backend with streaming JSON output.
/// If `system_prompt` is None, uses the default system prompt.
/// If `system_prompt` is Some(""), no system prompt is prepended.
pub fn execute_command_streaming(
    prompt: &str,
    session_id: Option<&str>,
    working_dir: &str,
    sender: Sender<StreamMessage>,
    system_prompt: Option<&str>,
    allowed_tools: Option<&[String]>,
    cancel_token: Option<std::sync::Arc<CancelToken>>,
) -> Result<(), String> {
    debug_log("========================================");
    debug_log("=== execute_command_streaming START ===");
    debug_log("========================================");

    let binary_name = ai_binary_name();
    let backend = backend_kind();
    let ai_bin = get_ai_binary_path().ok_or_else(|| {
        format!(
            "{} CLI not found. Is {} CLI installed?",
            binary_name, binary_name
        )
    })?;

    let full_prompt = build_full_prompt(prompt, system_prompt, allowed_tools);
    debug_log(&format!("Prompt length: {}", full_prompt.len()));
    let mut attempt_session_id = session_id.map(String::from);
    let mut retried_without_resume = false;

    loop {
        let args = backend_args(backend, attempt_session_id.as_deref(), working_dir)?;

        debug_log(&format!("Command: {}", ai_bin));
        debug_log(&format!("Backend: {:?}", backend));
        debug_log(&format!("Args: {:?}", args));

        let attempt = execute_command_streaming_once(
            ai_bin,
            binary_name,
            &args,
            &full_prompt,
            working_dir,
            &sender,
            cancel_token.clone(),
        )?;

        let StreamingAttemptState::Completed(outcome) = attempt else {
            return Ok(());
        };

        if !outcome.status_success
            && attempt_session_id.is_some()
            && !retried_without_resume
            && outcome.emitted_message_count == 0
            && is_retryable_resume_error(&outcome.stderr_output)
        {
            let stale = attempt_session_id.as_deref().unwrap_or_default();
            debug_log(&format!(
                "Detected stale --resume session ({stale}). Retrying without resume."
            ));
            attempt_session_id = None;
            retried_without_resume = true;
            continue;
        }

        if !outcome.status_success {
            let message = if !outcome.stderr_output.trim().is_empty() {
                outcome.stderr_output.trim().to_string()
            } else {
                format!("{} exited with code {:?}", binary_name, outcome.status_code)
            };
            let _ = sender.send(StreamMessage::Error { message });
        }

        if !outcome.done_sent {
            let _ = sender.send(StreamMessage::Done {
                result: String::new(),
                session_id: outcome.last_session_id,
            });
        }

        break;
    }

    debug_log("======================================");
    debug_log("=== execute_command_streaming END ===");
    debug_log("======================================");

    Ok(())
}

/// Parse one Codex/OMX JSONL event line into zero or more StreamMessage values.
fn parse_codex_stream_line(json: &Value) -> Vec<StreamMessage> {
    let mut messages = Vec::new();

    let Some(event_type) = json.get("type").and_then(|v| v.as_str()) else {
        return messages;
    };

    match event_type {
        // OMX (Claude-compatible) stream-json init event
        "system" => {
            if json.get("subtype").and_then(|v| v.as_str()) == Some("init") {
                if let Some(session_id) = json.get("session_id").and_then(|v| v.as_str()) {
                    messages.push(StreamMessage::Init {
                        session_id: session_id.to_string(),
                    });
                }
            }
        }
        // OMX (Claude-compatible) stream-json assistant event
        "assistant" => {
            if let Some(content) = json
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|v| v.as_array())
            {
                for block in content {
                    match block.get("type").and_then(|v| v.as_str()) {
                        Some("text") => {
                            let text = block
                                .get("text")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            if !text.is_empty() {
                                messages.push(StreamMessage::Text { content: text });
                            }
                        }
                        Some("tool_use") => {
                            let name = block
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("Tool")
                                .to_string();
                            let input = block
                                .get("input")
                                .map(|v| {
                                    if let Some(s) = v.as_str() {
                                        s.to_string()
                                    } else {
                                        serde_json::to_string(v).unwrap_or_else(|_| String::new())
                                    }
                                })
                                .unwrap_or_default();
                            messages.push(StreamMessage::ToolUse { name, input });
                        }
                        _ => {}
                    }
                }
            }
        }
        // OMX (Claude-compatible) stream-json final result event
        "result" => {
            let result_text = json
                .get("result")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let session_id = json
                .get("session_id")
                .and_then(|v| v.as_str())
                .map(String::from);
            let is_error = json
                .get("is_error")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if is_error {
                let errors = json
                    .get("errors")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str())
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();

                let message = if !errors.is_empty() {
                    errors
                } else if !result_text.trim().is_empty() {
                    result_text.clone()
                } else {
                    "OMX execution failed".to_string()
                };

                messages.push(StreamMessage::Error { message });
            }

            messages.push(StreamMessage::Done {
                result: result_text,
                session_id,
            });
        }
        // Codex stream-json init event
        "thread.started" => {
            if let Some(thread_id) = json.get("thread_id").and_then(|v| v.as_str()) {
                messages.push(StreamMessage::Init {
                    session_id: thread_id.to_string(),
                });
            }
        }
        // Codex stream-json tool start event
        "item.started" => {
            if let Some(item) = json.get("item") {
                if item.get("type").and_then(|v| v.as_str()) == Some("command_execution") {
                    let command = item
                        .get("command")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if !command.is_empty() {
                        messages.push(StreamMessage::ToolUse {
                            name: "Bash".to_string(),
                            input: command,
                        });
                    }
                }
            }
        }
        // Codex stream-json item completion event
        "item.completed" => {
            if let Some(item) = json.get("item") {
                match item.get("type").and_then(|v| v.as_str()) {
                    Some("agent_message") => {
                        let text = item
                            .get("text")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        if !text.is_empty() {
                            messages.push(StreamMessage::Text { content: text });
                        }
                    }
                    Some("command_execution") => {
                        let output = item
                            .get("aggregated_output")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .trim_end()
                            .to_string();
                        let exit_code = item.get("exit_code").and_then(|v| v.as_i64());
                        let is_error = exit_code.unwrap_or(0) != 0;

                        if !output.is_empty() || is_error {
                            let content = if !output.is_empty() {
                                output
                            } else {
                                format!("Command exited with code {}", exit_code.unwrap_or(-1))
                            };
                            messages.push(StreamMessage::ToolResult { content, is_error });
                        }
                    }
                    Some("error") => {
                        let message = item
                            .get("message")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .trim()
                            .to_string();

                        // Ignore local unstable-feature warning noise.
                        if !message.is_empty()
                            && !message.contains("Under-development features enabled")
                        {
                            messages.push(StreamMessage::Error { message });
                        }
                    }
                    _ => {}
                }
            }
        }
        // Codex stream-json turn completion event
        "turn.completed" => {
            messages.push(StreamMessage::Done {
                result: String::new(),
                session_id: None,
            });
        }
        _ => {}
    }

    messages
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_json(input: &str) -> Value {
        match serde_json::from_str::<Value>(input) {
            Ok(v) => v,
            Err(e) => panic!("failed to parse json in test: {}", e),
        }
    }

    #[test]
    fn test_session_id_valid() {
        assert!(is_valid_session_id("abc123"));
        assert!(is_valid_session_id("session-1"));
        assert!(is_valid_session_id("session_2"));
        assert!(is_valid_session_id("019c85d5-1ead-7011-8693-c2d63bef2311"));
    }

    #[test]
    fn test_session_id_rejections() {
        assert!(!is_valid_session_id(""));
        assert!(!is_valid_session_id("a b"));
        assert!(!is_valid_session_id("session/../hack"));
        assert!(!is_valid_session_id(&"a".repeat(65)));
    }

    #[test]
    fn test_session_id_regex_caching() {
        let regex1 = session_id_regex();
        let regex2 = session_id_regex();
        assert!(std::ptr::eq(regex1, regex2));
    }

    #[test]
    fn test_retryable_resume_error_positive_patterns() {
        assert!(is_retryable_resume_error(
            "Error: invalid value for '--resume <SESSION_ID>'"
        ));
        assert!(is_retryable_resume_error(
            "thread not found for resume id abc123"
        ));
        assert!(is_retryable_resume_error(
            "Failed to resume session: does not exist"
        ));
    }

    #[test]
    fn test_retryable_resume_error_negative_patterns() {
        assert!(!is_retryable_resume_error(
            "network timeout while contacting API"
        ));
        assert!(!is_retryable_resume_error(
            "permission denied while writing file"
        ));
    }

    #[test]
    fn test_parse_thread_started() {
        let json = parse_json(r#"{"type":"thread.started","thread_id":"thread-123"}"#);
        let msgs = parse_codex_stream_line(&json);
        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            StreamMessage::Init { session_id } => assert_eq!(session_id, "thread-123"),
            _ => panic!("expected init message"),
        }
    }

    #[test]
    fn test_parse_omx_init() {
        let json = parse_json(
            r#"{"type":"system","subtype":"init","session_id":"54c57e53-7575-4fd6-820a-8432dc14ccb6"}"#,
        );
        let msgs = parse_codex_stream_line(&json);
        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            StreamMessage::Init { session_id } => {
                assert_eq!(session_id, "54c57e53-7575-4fd6-820a-8432dc14ccb6")
            }
            _ => panic!("expected init message"),
        }
    }

    #[test]
    fn test_parse_omx_assistant_text() {
        let json = parse_json(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello from OMX"}]}}"#,
        );
        let msgs = parse_codex_stream_line(&json);
        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            StreamMessage::Text { content } => assert_eq!(content, "Hello from OMX"),
            _ => panic!("expected text message"),
        }
    }

    #[test]
    fn test_parse_omx_result_success() {
        let json = parse_json(
            r#"{"type":"result","is_error":false,"result":"done","session_id":"sess-1"}"#,
        );
        let msgs = parse_codex_stream_line(&json);
        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            StreamMessage::Done { result, session_id } => {
                assert_eq!(result, "done");
                assert_eq!(session_id.as_deref(), Some("sess-1"));
            }
            _ => panic!("expected done message"),
        }
    }

    #[test]
    fn test_parse_omx_result_error() {
        let json = parse_json(
            r#"{"type":"result","is_error":true,"errors":["boom"],"result":"","session_id":"sess-2"}"#,
        );
        let msgs = parse_codex_stream_line(&json);
        assert_eq!(msgs.len(), 2);
        match &msgs[0] {
            StreamMessage::Error { message } => assert_eq!(message, "boom"),
            _ => panic!("expected error message"),
        }
        match &msgs[1] {
            StreamMessage::Done { result, session_id } => {
                assert_eq!(result, "");
                assert_eq!(session_id.as_deref(), Some("sess-2"));
            }
            _ => panic!("expected done message"),
        }
    }

    #[test]
    fn test_parse_agent_message() {
        let json = parse_json(
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"hello"}}"#,
        );
        let msgs = parse_codex_stream_line(&json);
        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            StreamMessage::Text { content } => assert_eq!(content, "hello"),
            _ => panic!("expected text message"),
        }
    }

    #[test]
    fn test_parse_command_started() {
        let json = parse_json(
            r#"{"type":"item.started","item":{"type":"command_execution","command":"/bin/bash -lc pwd"}}"#,
        );
        let msgs = parse_codex_stream_line(&json);
        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            StreamMessage::ToolUse { name, input } => {
                assert_eq!(name, "Bash");
                assert!(input.contains("pwd"));
            }
            _ => panic!("expected tool use message"),
        }
    }

    #[test]
    fn test_parse_command_completed_success() {
        let json = parse_json(
            r#"{"type":"item.completed","item":{"type":"command_execution","aggregated_output":"/tmp\n","exit_code":0}}"#,
        );
        let msgs = parse_codex_stream_line(&json);
        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            StreamMessage::ToolResult { content, is_error } => {
                assert_eq!(content, "/tmp");
                assert!(!is_error);
            }
            _ => panic!("expected tool result message"),
        }
    }

    #[test]
    fn test_parse_command_completed_error() {
        let json = parse_json(
            r#"{"type":"item.completed","item":{"type":"command_execution","aggregated_output":"boom\n","exit_code":1}}"#,
        );
        let msgs = parse_codex_stream_line(&json);
        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            StreamMessage::ToolResult { content, is_error } => {
                assert_eq!(content, "boom");
                assert!(*is_error);
            }
            _ => panic!("expected tool result message"),
        }
    }

    #[test]
    fn test_parse_warning_error_ignored() {
        let json = parse_json(
            r#"{"type":"item.completed","item":{"type":"error","message":"Under-development features enabled: child_agents_md"}}"#,
        );
        let msgs = parse_codex_stream_line(&json);
        assert!(msgs.is_empty());
    }

    #[test]
    fn test_parse_real_error_forwarded() {
        let json = parse_json(
            r#"{"type":"item.completed","item":{"type":"error","message":"failed to run"}}"#,
        );
        let msgs = parse_codex_stream_line(&json);
        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            StreamMessage::Error { message } => assert_eq!(message, "failed to run"),
            _ => panic!("expected error message"),
        }
    }

    #[test]
    fn test_parse_turn_completed() {
        let json = parse_json(r#"{"type":"turn.completed"}"#);
        let msgs = parse_codex_stream_line(&json);
        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            StreamMessage::Done { .. } => {}
            _ => panic!("expected done message"),
        }
    }

    #[test]
    fn test_is_ai_supported() {
        #[cfg(unix)]
        assert!(is_ai_supported());

        #[cfg(not(unix))]
        assert!(!is_ai_supported());
    }

    #[test]
    fn test_codex_response_error_struct() {
        let response = CodexResponse {
            success: false,
            response: None,
            session_id: None,
            error: Some("error".to_string()),
        };
        assert!(!response.success);
        assert_eq!(response.error.as_deref(), Some("error"));
    }

    #[test]
    fn test_codex_response_success_struct() {
        let response = CodexResponse {
            success: true,
            response: Some("ok".to_string()),
            session_id: Some("thread-1".to_string()),
            error: None,
        };
        assert!(response.success);
        assert_eq!(response.response.as_deref(), Some("ok"));
        assert_eq!(response.session_id.as_deref(), Some("thread-1"));
        assert!(response.error.is_none());
    }

    #[test]
    fn test_backend_kind_defaults_to_codex() {
        assert_eq!(backend_kind(), BackendKind::Codex);
        assert_eq!(ai_binary_name(), "codex");
    }

    #[test]
    fn test_codex_args_default_session() {
        let args = codex_args(None, "/tmp/project").expect("args should build");
        assert_eq!(
            args,
            vec![
                "-C",
                "/tmp/project",
                "--sandbox",
                "danger-full-access",
                "-a",
                "never",
                "exec",
                "--json",
                "--skip-git-repo-check",
                "-",
            ]
        );
    }

    #[test]
    fn test_codex_args_resume_session() {
        let args = codex_args(Some("session-1"), "/tmp/project").expect("args should build");
        assert_eq!(
            args,
            vec![
                "-C",
                "/tmp/project",
                "--sandbox",
                "danger-full-access",
                "-a",
                "never",
                "exec",
                "resume",
                "session-1",
                "--json",
                "-",
            ]
        );
    }

    #[test]
    fn test_omx_args_default_session() {
        let args = omx_args(None, "/tmp/project").expect("args should build");
        assert_eq!(
            args,
            vec![
                "--cd",
                "/tmp/project",
                "--sandbox",
                "danger-full-access",
                "-a",
                "never",
                "exec",
                "--json",
                "--skip-git-repo-check",
                "-",
            ]
        );
    }

    #[test]
    fn test_omx_args_resume_session() {
        let args = omx_args(Some("session-1"), "/tmp/project").expect("args should build");
        assert_eq!(
            args,
            vec![
                "--cd",
                "/tmp/project",
                "--sandbox",
                "danger-full-access",
                "-a",
                "never",
                "exec",
                "resume",
                "session-1",
                "--json",
                "-",
            ]
        );
    }

    #[test]
    fn test_backend_args_dispatch() {
        let codex = backend_args(BackendKind::Codex, None, "/tmp/project")
            .expect("codex args should build");
        assert!(codex.contains(&"exec".to_string()));

        let omx = backend_args(BackendKind::Omx, Some("session-1"), "/tmp/project")
            .expect("omx args should build");
        assert!(omx.contains(&"exec".to_string()));
        assert!(omx.contains(&"resume".to_string()));
    }

    #[test]
    fn test_resolve_ai_binary_path_uses_codex() {
        let has_codex = std::process::Command::new("which")
            .arg("codex")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !has_codex {
            return;
        }

        let path = resolve_ai_binary_path().expect("codex path should resolve");
        assert!(path.contains("codex"), "expected codex path, got: {}", path);
    }
}
