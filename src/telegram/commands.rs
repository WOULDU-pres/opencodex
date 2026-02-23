use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use teloxide::prelude::*;
use teloxide::types::ParseMode;

use crate::auth;
use crate::codex;
use crate::i18n;
use crate::session::HistoryType;

use super::bot::{ChatSession, SharedData, SharedState};
use super::file_ops::{handle_down_command, handle_file_upload, handle_shell_command};
use super::message::handle_text_message;
use super::storage::{load_bot_settings, load_existing_session, save_bot_settings};
use super::streaming::{send_long_message, shared_rate_limit_wait, truncate_str};
use super::tools::{
    handle_allowed_command, handle_allowedtools_command, handle_availabletools_command,
};

/// Entry point: start the Telegram bot with long polling.
/// `default_project_dir` is the working directory bound by the CLI binary.
pub async fn run_bot(token: &str, default_project_dir: &str) {
    let bot = Bot::new(token);
    let bot_settings = load_bot_settings(token);

    // Register bot commands for autocomplete
    let commands = vec![
        teloxide::types::BotCommand::new("help", "도움말"),
        teloxide::types::BotCommand::new("start", "세션 시작"),
        teloxide::types::BotCommand::new("pwd", "현재 경로 확인"),
        teloxide::types::BotCommand::new("cd", "작업 경로 변경"),
        teloxide::types::BotCommand::new("clear", "대화 히스토리 초기화"),
        teloxide::types::BotCommand::new("stop", "진행 중 작업 중단"),
        teloxide::types::BotCommand::new("status", "런타임 상태 확인"),
        teloxide::types::BotCommand::new("down", "서버 파일 다운로드"),
        teloxide::types::BotCommand::new("public", "그룹 공개 모드 전환"),
        teloxide::types::BotCommand::new("availabletools", "전체 도구 목록"),
        teloxide::types::BotCommand::new("allowedtools", "허용 도구 목록"),
        teloxide::types::BotCommand::new("allowed", "도구 허용/해제"),
    ];
    if let Err(e) = bot.set_my_commands(commands).await {
        println!("  ⚠ Failed to set bot commands: {e}");
    }

    match bot_settings.owner_user_id {
        Some(owner_id) => println!("  ✓ Owner: {owner_id}"),
        None => println!("  ⚠ No owner registered — first user will be registered as owner"),
    }

    let state: SharedState = Arc::new(tokio::sync::Mutex::new(SharedData {
        sessions: HashMap::new(),
        settings: bot_settings,
        cancel_tokens: HashMap::new(),
        shell_pids: HashMap::new(),
        stop_message_ids: HashMap::new(),
        api_timestamps: HashMap::new(),
    }));

    println!("  ✓ Bot connected — Listening for messages");

    let shared_state = state.clone();
    let token_owned = token.to_string();
    let default_project_dir_owned = default_project_dir.to_string();
    teloxide::repl(bot, move |bot: Bot, msg: Message| {
        let state = shared_state.clone();
        let token = token_owned.clone();
        let default_project_dir = default_project_dir_owned.clone();
        async move { handle_message(bot, msg, state, &token, &default_project_dir).await }
    })
    .await;
}

/// Route incoming messages to appropriate handlers
async fn handle_message(
    bot: Bot,
    msg: Message,
    state: SharedState,
    token: &str,
    default_project_dir: &str,
) -> ResponseResult<()> {
    let chat_id = msg.chat.id;
    let raw_user_name = msg
        .from
        .as_ref()
        .map(|u| u.first_name.as_str())
        .unwrap_or("unknown");
    let timestamp = chrono::Local::now().format("%H:%M:%S");
    let user_id = msg.from.as_ref().map(|u| u.id.0);

    // Auth check (imprinting)
    let Some(uid) = user_id else {
        // No user info (e.g. channel post) -> reject
        return Ok(());
    };
    let is_group_chat = matches!(msg.chat.kind, teloxide::types::ChatKind::Public(_));
    let (imprinted, rejected_private) = {
        let mut data = state.lock().await;
        match data.settings.owner_user_id {
            None => {
                // Imprint: register first user as owner
                data.settings.owner_user_id = Some(uid);
                save_bot_settings(token, &data.settings);
                println!("  [{timestamp}] ★ Owner registered: {raw_user_name} (id:{uid})");
                (true, false)
            }
            Some(owner_id) => {
                if uid != owner_id {
                    // Check if this is a public group chat
                    let chat_key = chat_id.0.to_string();
                    let is_public = is_group_chat
                        && data
                            .settings
                            .as_public_for_group_chat
                            .get(&chat_key)
                            .copied()
                            .unwrap_or(false);
                    if !is_public {
                        // Unregistered user -> reject with guidance
                        println!("  [{timestamp}] ✗ Rejected: {raw_user_name} (id:{uid})");
                        (false, true)
                    } else {
                        // Public group chat: allow non-owner user
                        println!(
                            "  [{timestamp}] ○ [{raw_user_name}(id:{uid})] Public group access"
                        );
                        (false, false)
                    }
                } else {
                    (false, false)
                }
            }
        }
    };
    if rejected_private {
        shared_rate_limit_wait(&state, chat_id).await;
        bot.send_message(chat_id, i18n::MSG_PRIVATE_BOT).await?;
        return Ok(());
    }
    if imprinted {
        shared_rate_limit_wait(&state, chat_id).await;
        bot.send_message(chat_id, i18n::MSG_OWNER_REGISTERED)
            .await?;
    }

    let is_owner = {
        let data = state.lock().await;
        data.settings.owner_user_id == Some(uid)
    };

    let user_name = format!("{}({uid})", raw_user_name);

    // Handle file/photo uploads
    if msg.document().is_some() || msg.photo().is_some() {
        // Auth: file uploads are High risk (modifies filesystem)
        if !is_owner {
            shared_rate_limit_wait(&state, chat_id).await;
            bot.send_message(chat_id, "Permission denied. File uploads are owner-only.")
                .await?;
            return Ok(());
        }
        // In group chats, only process uploads whose caption starts with ';'
        if is_group_chat {
            let caption = msg.caption().unwrap_or("");
            if !caption.starts_with(';') {
                return Ok(());
            }
        }
        let file_hint = if msg.document().is_some() {
            "document"
        } else {
            "photo"
        };
        println!("  [{timestamp}] ◀ [{user_name}] Upload: {file_hint}");
        handle_file_upload(&bot, chat_id, &msg, &state).await?;
        println!("  [{timestamp}] ▶ [{user_name}] Upload complete");
        // If caption contains text after ';', send it to AI as a follow-up message
        if let Some(caption) = msg.caption() {
            let text_part = if is_group_chat {
                // Group chat: extract text after ';'
                caption.find(';').map(|pos| caption[pos + 1..].trim())
            } else {
                // DM: use entire caption as-is
                let trimmed = caption.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed)
                }
            };
            if let Some(text) = text_part {
                if !text.is_empty() {
                    // Block if an AI request is already in progress
                    let ai_busy = {
                        let data = state.lock().await;
                        data.cancel_tokens.contains_key(&chat_id)
                    };
                    if ai_busy {
                        shared_rate_limit_wait(&state, chat_id).await;
                        bot.send_message(chat_id, i18n::MSG_AI_BUSY).await?;
                    } else {
                        handle_text_message(&bot, chat_id, text, &state).await?;
                    }
                }
            }
        }
        return Ok(());
    }

    let Some(raw_text) = msg.text() else {
        return Ok(());
    };

    // Strip @botname suffix from commands (e.g. "/pwd@mybot" -> "/pwd")
    let text = if raw_text.starts_with('/') {
        if let Some(space_pos) = raw_text.find(' ') {
            // "/cmd@bot args" -> "/cmd args"
            let cmd_part = &raw_text[..space_pos];
            let args_part = &raw_text[space_pos..];
            if let Some(at_pos) = cmd_part.find('@') {
                format!("{}{}", &cmd_part[..at_pos], args_part)
            } else {
                raw_text.to_string()
            }
        } else {
            // "/cmd@bot" (no args) -> "/cmd"
            if let Some(at_pos) = raw_text.find('@') {
                raw_text[..at_pos].to_string()
            } else {
                raw_text.to_string()
            }
        }
    } else {
        raw_text.to_string()
    };
    let preview = truncate_str(&text, 60);

    // Auto-restore session from bot_settings.json if not in memory.
    // If there is no previous path, fall back to startup project dir.
    if !text.starts_with("/start") {
        let mut data = state.lock().await;
        if !data.sessions.contains_key(&chat_id) {
            let candidate_path = data
                .settings
                .last_sessions
                .get(&chat_id.0.to_string())
                .cloned()
                .unwrap_or_else(|| default_project_dir.to_string());
            if Path::new(&candidate_path).is_dir() {
                let existing = load_existing_session(&candidate_path);
                let session = data.sessions.entry(chat_id).or_insert_with(|| ChatSession {
                    session_id: None,
                    current_path: None,
                    history: Vec::new(),
                    pending_uploads: Vec::new(),
                    cleared: false,
                });
                session.current_path = Some(candidate_path.clone());
                if let Some((session_data, _)) = existing {
                    session.session_id = Some(session_data.session_id.clone());
                    session.history = session_data.history.clone();
                }
                let ts = chrono::Local::now().format("%H:%M:%S");
                println!("  [{ts}] ↻ [{user_name}] Auto-restored session: {candidate_path}");
            }
        }
    }

    // In group chats, ignore plain text (only /, !, ; prefixed messages are processed)
    if is_group_chat && !text.starts_with('/') && !text.starts_with('!') && !text.starts_with(';') {
        return Ok(());
    }

    // Auth: check command risk vs user permission level
    {
        let data = state.lock().await;
        let is_public_chat = is_group_chat
            && data
                .settings
                .as_public_for_group_chat
                .get(&chat_id.0.to_string())
                .copied()
                .unwrap_or(false);
        let permission =
            auth::get_permission_level(uid, data.settings.owner_user_id, is_public_chat);
        let risk = auth::classify_command(&text);
        if !auth::can_execute(permission, risk) {
            drop(data);
            shared_rate_limit_wait(&state, chat_id).await;
            bot.send_message(chat_id, "Permission denied. This command is owner-only.")
                .await?;
            return Ok(());
        }
    }

    // Block all messages except /stop while an AI request is in progress
    if !text.starts_with("/stop") {
        let data = state.lock().await;
        if data.cancel_tokens.contains_key(&chat_id) {
            drop(data);
            shared_rate_limit_wait(&state, chat_id).await;
            bot.send_message(chat_id, i18n::MSG_AI_BUSY).await?;
            return Ok(());
        }
    }

    if text.starts_with("/stop") {
        println!("  [{timestamp}] ◀ [{user_name}] /stop");
        handle_stop_command(&bot, chat_id, &state).await?;
    } else if text.starts_with("/help") {
        println!("  [{timestamp}] ◀ [{user_name}] /help");
        handle_help_command(&bot, chat_id, &state).await?;
    } else if text.starts_with("/start") {
        println!("  [{timestamp}] ◀ [{user_name}] /start");
        handle_start_command(&bot, chat_id, &text, &state, token, default_project_dir).await?;
    } else if text.starts_with("/clear") {
        println!("  [{timestamp}] ◀ [{user_name}] /clear");
        handle_clear_command(&bot, chat_id, &state).await?;
        println!("  [{timestamp}] ▶ [{user_name}] Session cleared");
    } else if text.starts_with("/pwd") {
        println!("  [{timestamp}] ◀ [{user_name}] /pwd");
        handle_pwd_command(&bot, chat_id, &state).await?;
    } else if text.starts_with("/status") {
        println!("  [{timestamp}] ◀ [{user_name}] /status");
        handle_status_command(&bot, chat_id, &state).await?;
    } else if text.starts_with("/cd") {
        println!(
            "  [{timestamp}] ◀ [{user_name}] /cd {}",
            text.strip_prefix("/cd").unwrap_or("").trim()
        );
        handle_cd_command(&bot, chat_id, &text, &state, token).await?;
    } else if text.starts_with("/down") {
        println!(
            "  [{timestamp}] ◀ [{user_name}] /down {}",
            text.strip_prefix("/down").unwrap_or("").trim()
        );
        handle_down_command(&bot, chat_id, &text, &state).await?;
    } else if text.starts_with("/public") {
        println!(
            "  [{timestamp}] ◀ [{user_name}] /public {}",
            text.strip_prefix("/public").unwrap_or("").trim()
        );
        handle_public_command(&bot, chat_id, &text, &state, token, is_group_chat, is_owner).await?;
    } else if text.starts_with("/availabletools") {
        println!("  [{timestamp}] ◀ [{user_name}] /availabletools");
        handle_availabletools_command(&bot, chat_id, &state).await?;
    } else if text.starts_with("/allowedtools") {
        println!("  [{timestamp}] ◀ [{user_name}] /allowedtools");
        handle_allowedtools_command(&bot, chat_id, &state).await?;
    } else if text.starts_with("/allowed") {
        println!(
            "  [{timestamp}] ◀ [{user_name}] /allowed {}",
            text.strip_prefix("/allowed").unwrap_or("").trim()
        );
        handle_allowed_command(&bot, chat_id, &text, &state, token).await?;
    } else if text.starts_with('!') {
        println!("  [{timestamp}] ◀ [{user_name}] Shell: {preview}");
        handle_shell_command(&bot, chat_id, &text, &state).await?;
        println!("  [{timestamp}] ▶ [{user_name}] Shell done");
    } else if text.starts_with(';') {
        let stripped = text.strip_prefix(';').unwrap_or(&text).trim().to_string();
        if stripped.is_empty() {
            return Ok(());
        }
        let preview = truncate_str(&stripped, 60);
        println!("  [{timestamp}] ◀ [{user_name}] {preview}");
        handle_text_message(&bot, chat_id, &stripped, &state).await?;
    } else {
        println!("  [{timestamp}] ◀ [{user_name}] {preview}");
        handle_text_message(&bot, chat_id, &text, &state).await?;
    }

    Ok(())
}

/// Handle /help command
async fn handle_help_command(
    bot: &Bot,
    chat_id: ChatId,
    state: &SharedState,
) -> ResponseResult<()> {
    let help = i18n::HELP_TEXT_TEMPLATE.replace("{app}", env!("CARGO_BIN_NAME"));

    shared_rate_limit_wait(state, chat_id).await;
    bot.send_message(chat_id, help)
        .parse_mode(ParseMode::Html)
        .await?;

    Ok(())
}

/// Handle /status command - show current runtime state
async fn handle_status_command(
    bot: &Bot,
    chat_id: ChatId,
    state: &SharedState,
) -> ResponseResult<()> {
    let (path, session_id, history_len, ai_active) = {
        let data = state.lock().await;
        let session = data.sessions.get(&chat_id);
        (
            session
                .and_then(|s| s.current_path.clone())
                .unwrap_or_else(|| "-".to_string()),
            session
                .and_then(|s| s.session_id.clone())
                .unwrap_or_else(|| "-".to_string()),
            session.map(|s| s.history.len()).unwrap_or(0),
            data.cancel_tokens.contains_key(&chat_id),
        )
    };

    let backend_path = codex::get_ai_binary_path();
    let backend_name = backend_path
        .and_then(|p| {
            Path::new(p)
                .file_name()
                .and_then(|name| name.to_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "unavailable".to_string());
    let backend_version = backend_path
        .and_then(|path| {
            Command::new(path)
                .arg("--version")
                .output()
                .ok()
                .and_then(|output| {
                    if output.status.success() {
                        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                        if !stdout.is_empty() {
                            Some(stdout)
                        } else {
                            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                            (!stderr.is_empty()).then_some(stderr)
                        }
                    } else {
                        None
                    }
                })
        })
        .unwrap_or_else(|| "unknown".to_string());
    let ai_state = if ai_active { "running" } else { "idle" };

    let message = format!(
        "Status\n\
path: {path}\n\
session_id: {session_id}\n\
history_len: {history_len}\n\
active_ai: {ai_state}\n\
backend: {backend_name}\n\
backend_version: {backend_version}\n\
app_version: {} {}",
        env!("CARGO_BIN_NAME"),
        env!("CARGO_PKG_VERSION")
    );

    shared_rate_limit_wait(state, chat_id).await;
    bot.send_message(chat_id, message).await?;

    Ok(())
}

/// Handle /start <path> command
async fn handle_start_command(
    bot: &Bot,
    chat_id: ChatId,
    text: &str,
    state: &SharedState,
    token: &str,
    default_project_dir: &str,
) -> ResponseResult<()> {
    // Extract path from "/start <path>"
    let path_str = text.strip_prefix("/start").unwrap_or("").trim();

    let canonical_path = if path_str.is_empty() {
        // Bind to startup project directory by default.
        let path = Path::new(default_project_dir);
        if !path.exists() || !path.is_dir() {
            shared_rate_limit_wait(state, chat_id).await;
            bot.send_message(
                chat_id,
                format!(
                    "Error: default project dir is invalid: {}",
                    default_project_dir
                ),
            )
            .await?;
            return Ok(());
        }
        path.canonicalize()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| default_project_dir.to_string())
    } else {
        // Expand ~ to home directory
        let expanded = if path_str.starts_with("~/") || path_str == "~" {
            if let Some(home) = dirs::home_dir() {
                home.join(path_str.strip_prefix("~/").unwrap_or(""))
                    .display()
                    .to_string()
            } else {
                path_str.to_string()
            }
        } else {
            path_str.to_string()
        };
        // Validate path exists
        let path = Path::new(&expanded);
        if !path.exists() || !path.is_dir() {
            shared_rate_limit_wait(state, chat_id).await;
            bot.send_message(
                chat_id,
                format!("Error: '{}' is not a valid directory.", expanded),
            )
            .await?;
            return Ok(());
        }
        path.canonicalize()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| expanded)
    };

    // Try to load existing session for this path
    let existing = load_existing_session(&canonical_path);

    let mut response_lines = Vec::new();

    {
        let mut data = state.lock().await;
        let session = data.sessions.entry(chat_id).or_insert_with(|| ChatSession {
            session_id: None,
            current_path: None,
            history: Vec::new(),
            pending_uploads: Vec::new(),
            cleared: false,
        });

        if let Some((session_data, _)) = &existing {
            session.session_id = Some(session_data.session_id.clone());
            session.current_path = Some(canonical_path.clone());
            session.history = session_data.history.clone();

            let ts = chrono::Local::now().format("%H:%M:%S");
            println!("  [{ts}] ▶ Session restored: {canonical_path}");
            response_lines.push(format!("Session restored at `{}`.", canonical_path));
            response_lines.push(String::new());

            // Show last 5 conversation items
            let history_len = session_data.history.len();
            let start_idx = history_len.saturating_sub(5);
            for item in &session_data.history[start_idx..] {
                let prefix = match item.item_type {
                    HistoryType::User => "You",
                    HistoryType::Assistant => "AI",
                    HistoryType::Error => "Error",
                    HistoryType::System => "System",
                    HistoryType::ToolUse => "Tool",
                    HistoryType::ToolResult => "Result",
                };
                // Truncate long items for display
                let content: String = item.content.chars().take(200).collect();
                let truncated = if item.content.chars().count() > 200 {
                    "..."
                } else {
                    ""
                };
                response_lines.push(format!("[{}] {}{}", prefix, content, truncated));
            }
        } else {
            session.session_id = None;
            session.current_path = Some(canonical_path.clone());
            session.history.clear();

            let ts = chrono::Local::now().format("%H:%M:%S");
            println!("  [{ts}] ▶ Session started: {canonical_path}");
            response_lines.push(format!("Session started at `{}`.", canonical_path));
        }
    }

    // Persist chat_id -> path mapping for auto-restore after restart
    {
        let mut data = state.lock().await;
        data.settings
            .last_sessions
            .insert(chat_id.0.to_string(), canonical_path);
        save_bot_settings(token, &data.settings);
    }

    let response_text = response_lines.join("\n");
    send_long_message(bot, chat_id, &response_text, None, state).await?;

    Ok(())
}

/// Handle /clear command
async fn handle_clear_command(
    bot: &Bot,
    chat_id: ChatId,
    state: &SharedState,
) -> ResponseResult<()> {
    // Cancel in-progress AI request if any
    let cancel_token = {
        let data = state.lock().await;
        data.cancel_tokens.get(&chat_id).cloned()
    };
    if let Some(token) = cancel_token {
        token.cancelled.store(true, Ordering::Relaxed);
        if let Ok(guard) = token.child_pid.lock() {
            if let Some(pid) = *guard {
                #[cfg(unix)]
                // SAFETY: sending SIGTERM to cancel the child AI process
                #[allow(unsafe_code)]
                unsafe {
                    libc::kill(pid as libc::pid_t, libc::SIGTERM);
                }
            }
        }
    }

    {
        let mut data = state.lock().await;
        if let Some(session) = data.sessions.get_mut(&chat_id) {
            session.session_id = None;
            session.history.clear();
            session.pending_uploads.clear();
            session.cleared = true;
        }
        data.cancel_tokens.remove(&chat_id);
        data.stop_message_ids.remove(&chat_id);
    }

    shared_rate_limit_wait(state, chat_id).await;
    bot.send_message(chat_id, i18n::MSG_SESSION_CLEARED).await?;

    Ok(())
}

/// Handle /pwd command - show current session path
async fn handle_pwd_command(bot: &Bot, chat_id: ChatId, state: &SharedState) -> ResponseResult<()> {
    let current_path = {
        let data = state.lock().await;
        data.sessions
            .get(&chat_id)
            .and_then(|s| s.current_path.clone())
    };

    shared_rate_limit_wait(state, chat_id).await;
    match current_path {
        Some(path) => bot.send_message(chat_id, &path).await?,
        None => bot.send_message(chat_id, i18n::MSG_NO_SESSION).await?,
    };

    Ok(())
}

/// Handle /cd command - change working directory without resetting session
async fn handle_cd_command(
    bot: &Bot,
    chat_id: ChatId,
    text: &str,
    state: &SharedState,
    token: &str,
) -> ResponseResult<()> {
    let path_str = text.strip_prefix("/cd").unwrap_or("").trim();

    // No argument: show current path (like /pwd)
    if path_str.is_empty() {
        let current_path = {
            let data = state.lock().await;
            data.sessions
                .get(&chat_id)
                .and_then(|s| s.current_path.clone())
        };
        shared_rate_limit_wait(state, chat_id).await;
        match current_path {
            Some(path) => {
                bot.send_message(chat_id, format!("Current: {path}"))
                    .await?
            }
            None => bot.send_message(chat_id, i18n::MSG_NO_SESSION).await?,
        };
        return Ok(());
    }

    // Expand ~ to home directory
    let expanded = if path_str.starts_with("~/") || path_str == "~" {
        if let Some(home) = dirs::home_dir() {
            home.join(path_str.strip_prefix("~/").unwrap_or(""))
                .display()
                .to_string()
        } else {
            path_str.to_string()
        }
    } else if path_str.starts_with('/') {
        path_str.to_string()
    } else {
        // Relative path: resolve against current_path
        let base = {
            let data = state.lock().await;
            data.sessions
                .get(&chat_id)
                .and_then(|s| s.current_path.clone())
        };
        match base {
            Some(b) => Path::new(&b).join(path_str).display().to_string(),
            None => {
                shared_rate_limit_wait(state, chat_id).await;
                bot.send_message(chat_id, i18n::MSG_NO_SESSION).await?;
                return Ok(());
            }
        }
    };

    // Validate path
    let path = Path::new(&expanded);
    if !path.exists() || !path.is_dir() {
        shared_rate_limit_wait(state, chat_id).await;
        bot.send_message(chat_id, format!("Error: not a valid directory: {expanded}"))
            .await?;
        return Ok(());
    }

    let canonical = path
        .canonicalize()
        .map(|p| p.display().to_string())
        .unwrap_or(expanded);

    // Update only current_path, preserve session and history
    {
        let mut data = state.lock().await;
        if let Some(session) = data.sessions.get_mut(&chat_id) {
            session.current_path = Some(canonical.clone());
        } else {
            shared_rate_limit_wait(state, chat_id).await;
            bot.send_message(chat_id, i18n::MSG_NO_SESSION).await?;
            return Ok(());
        }

        // Persist path so it survives session restarts
        data.settings
            .last_sessions
            .insert(chat_id.0.to_string(), canonical.clone());
        save_bot_settings(token, &data.settings);
    }

    shared_rate_limit_wait(state, chat_id).await;
    bot.send_message(chat_id, format!("Changed to: {canonical}"))
        .await?;

    Ok(())
}

/// Handle /stop command - cancel in-progress AI request
async fn handle_stop_command(
    bot: &Bot,
    chat_id: ChatId,
    state: &SharedState,
) -> ResponseResult<()> {
    let (token, shell_pid) = {
        let mut data = state.lock().await;
        let token = data.cancel_tokens.get(&chat_id).cloned();
        let shell_pid = data.shell_pids.remove(&chat_id);
        (token, shell_pid)
    };
    let has_ai_token = token.is_some();

    if token.is_none() && shell_pid.is_none() {
        shared_rate_limit_wait(state, chat_id).await;
        bot.send_message(chat_id, i18n::MSG_NO_ACTIVE_REQUEST)
            .await?;
        return Ok(());
    }

    // Cancel AI request if present.
    if let Some(token) = token {
        // Ignore duplicate /stop for AI, but still allow shell cancellation below.
        if !token.cancelled.load(Ordering::Relaxed) {
            // Send immediate feedback to user
            shared_rate_limit_wait(state, chat_id).await;
            let stop_msg = bot.send_message(chat_id, i18n::MSG_STOPPING).await?;

            // Store the stop message ID so the polling loop can update it later
            {
                let mut data = state.lock().await;
                data.stop_message_ids.insert(chat_id, stop_msg.id);
            }

            // Set cancellation flag
            token.cancelled.store(true, Ordering::Relaxed);

            // Kill child process directly to unblock reader.lines()
            // When the child dies, its stdout pipe closes -> reader returns EOF -> blocking thread exits
            if let Ok(guard) = token.child_pid.lock() {
                if let Some(pid) = *guard {
                    #[cfg(unix)]
                    // SAFETY: sending SIGTERM to cancel the child AI process
                    #[allow(unsafe_code)]
                    unsafe {
                        libc::kill(pid as libc::pid_t, libc::SIGTERM);
                    }
                }
            }

            let ts = chrono::Local::now().format("%H:%M:%S");
            println!("  [{ts}] ■ Cancel signal sent");
        }
    }

    // Stop running shell command if present.
    if let Some(pid) = shell_pid {
        #[cfg(unix)]
        // SAFETY: sending SIGTERM to stop the running shell process for this chat
        #[allow(unsafe_code)]
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }

        if !has_ai_token {
            // Shell-only stop path still provides immediate feedback.
            shared_rate_limit_wait(state, chat_id).await;
            bot.send_message(chat_id, i18n::MSG_STOPPING).await?;
        }

        let ts = chrono::Local::now().format("%H:%M:%S");
        println!("  [{ts}] ■ Shell stop signal sent (pid:{pid})");
    }

    Ok(())
}

/// Handle /public command - toggle public access for group chats
async fn handle_public_command(
    bot: &Bot,
    chat_id: ChatId,
    text: &str,
    state: &SharedState,
    token: &str,
    is_group_chat: bool,
    is_owner: bool,
) -> ResponseResult<()> {
    if !is_group_chat {
        shared_rate_limit_wait(state, chat_id).await;
        bot.send_message(chat_id, "This command is only available in group chats.")
            .await?;
        return Ok(());
    }

    if !is_owner {
        shared_rate_limit_wait(state, chat_id).await;
        bot.send_message(
            chat_id,
            "Only the bot owner can change public access settings.",
        )
        .await?;
        return Ok(());
    }

    let arg = text
        .strip_prefix("/public")
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let chat_key = chat_id.0.to_string();

    let response_msg = match arg.as_str() {
        "on" => {
            let mut data = state.lock().await;
            data.settings
                .as_public_for_group_chat
                .insert(chat_key, true);
            save_bot_settings(token, &data.settings);
            "Public access <b>enabled</b> for this group.\nAll members can now use the bot."
                .to_string()
        }
        "off" => {
            let mut data = state.lock().await;
            data.settings.as_public_for_group_chat.remove(&chat_key);
            save_bot_settings(token, &data.settings);
            "Public access <b>disabled</b> for this group.\nOnly the owner can use the bot."
                .to_string()
        }
        "" => {
            let data = state.lock().await;
            let is_public = data
                .settings
                .as_public_for_group_chat
                .get(&chat_key)
                .copied()
                .unwrap_or(false);
            let status = if is_public { "enabled" } else { "disabled" };
            format!(
                "Public access is currently <b>{}</b> for this group.\n\n\
                 <code>/public on</code> — Allow all members\n\
                 <code>/public off</code> — Owner only",
                status
            )
        }
        _ => "Usage:\n<code>/public on</code> — Allow all group members\n<code>/public off</code> — Owner only".to_string(),
    };

    shared_rate_limit_wait(state, chat_id).await;
    bot.send_message(chat_id, &response_msg)
        .parse_mode(ParseMode::Html)
        .await?;

    Ok(())
}
