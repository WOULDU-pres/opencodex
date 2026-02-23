use std::collections::HashMap;
use std::sync::Arc;

use teloxide::prelude::*;
use tokio::sync::Mutex;

use crate::codex::{CancelToken, DEFAULT_ALLOWED_TOOLS};

/// Per-chat session state
pub(super) struct ChatSession {
    pub session_id: Option<String>,
    pub current_path: Option<String>,
    pub history: Vec<crate::session::HistoryItem>,
    /// File upload records not yet sent to Claude Code AI.
    /// Drained and prepended to the next user prompt so Claude Code knows about uploaded files.
    pub pending_uploads: Vec<String>,
    /// Set to true by /clear to prevent a racing polling loop from re-populating history.
    pub cleared: bool,
}

/// Bot-level settings persisted to disk
#[derive(Clone, Default)]
pub(super) struct BotSettings {
    pub allowed_tools: HashMap<String, Vec<String>>,
    /// chat_id (string) -> last working directory path
    pub last_sessions: HashMap<String, String>,
    /// Telegram user ID of the registered owner (imprinting auth)
    pub owner_user_id: Option<u64>,
    /// chat_id (string) -> true if group chat is public (non-owner users allowed)
    pub as_public_for_group_chat: HashMap<String, bool>,
}

/// Get allowed tools for a specific chat_id.
/// Returns the chat-specific list if configured, otherwise DEFAULT_ALLOWED_TOOLS.
pub(super) fn get_allowed_tools(settings: &BotSettings, chat_id: ChatId) -> Vec<String> {
    let key = chat_id.0.to_string();
    settings
        .allowed_tools
        .get(&key)
        .cloned()
        .unwrap_or_else(|| {
            DEFAULT_ALLOWED_TOOLS
                .iter()
                .map(|s| s.to_string())
                .collect()
        })
}

/// Shared state: per-chat sessions + bot settings
pub(super) struct SharedData {
    pub sessions: HashMap<ChatId, ChatSession>,
    pub settings: BotSettings,
    /// Per-chat cancel tokens for stopping in-progress AI requests
    pub cancel_tokens: HashMap<ChatId, Arc<CancelToken>>,
    /// Per-chat shell command PID for stopping in-progress `!` commands
    pub shell_pids: HashMap<ChatId, u32>,
    /// Message ID of the "Stopping..." message sent by /stop, so the polling loop can update it
    pub stop_message_ids: HashMap<ChatId, teloxide::types::MessageId>,
    /// Per-chat timestamp of the last Telegram API call (for rate limiting)
    pub api_timestamps: HashMap<ChatId, tokio::time::Instant>,
}

pub(super) type SharedState = Arc<Mutex<SharedData>>;

/// Telegram message length limit
pub(super) const TELEGRAM_MSG_LIMIT: usize = 4096;
