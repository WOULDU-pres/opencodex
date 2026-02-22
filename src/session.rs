use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HistoryType {
    User,
    Assistant,
    Error,
    System,
    ToolUse,
    ToolResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryItem {
    #[serde(rename = "type")]
    pub item_type: HistoryType,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionData {
    pub session_id: String,
    pub history: Vec<HistoryItem>,
    pub current_path: String,
    pub created_at: String,
}

/// Session directory: ~/<app_dir>/sessions
pub fn ai_sessions_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(crate::app::dir_name()).join("sessions"))
}

/// Basic prompt-sanitization copied from existing logic
pub fn sanitize_user_input(input: &str) -> String {
    let mut sanitized = input.to_string();

    let dangerous_patterns = [
        "ignore previous instructions",
        "ignore all previous",
        "disregard previous",
        "forget previous",
        "system prompt",
        "you are now",
        "act as if",
        "pretend you are",
        "new instructions:",
        "[system]",
        "[admin]",
        "---begin",
        "---end",
    ];

    let lower_input = sanitized.to_lowercase();
    for pattern in dangerous_patterns {
        if lower_input.contains(pattern) {
            sanitized = sanitized.replace(pattern, "[filtered]");
            sanitized = sanitized.replace(&pattern.to_lowercase(), "[filtered]");
            sanitized = sanitized.replace(&pattern.to_uppercase(), "[filtered]");
        }
    }

    const MAX_INPUT_LENGTH: usize = 4000;
    if sanitized.len() > MAX_INPUT_LENGTH {
        sanitized.truncate(MAX_INPUT_LENGTH);
        sanitized.push_str("... [truncated]");
    }

    sanitized
}
