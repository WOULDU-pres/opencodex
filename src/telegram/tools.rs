use teloxide::prelude::*;
use teloxide::types::ParseMode;

use crate::codex::DEFAULT_ALLOWED_TOOLS;

use super::bot::SharedState;
use super::storage::save_bot_settings;
use super::streaming::{html_escape, send_long_message, shared_rate_limit_wait};

/// Normalize tool name: first letter uppercase, rest lowercase
pub(super) fn normalize_tool_name(name: &str) -> String {
    let lower = name.to_lowercase();
    let mut chars = lower.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

/// All available tools with (description, is_destructive)
pub(super) const ALL_TOOLS: &[(&str, &str, bool)] = &[
    ("Bash", "Execute shell commands", true),
    ("Read", "Read file contents from the filesystem", false),
    ("Edit", "Perform find-and-replace edits in files", true),
    ("Write", "Create or overwrite files", true),
    ("Glob", "Find files by name pattern", false),
    ("Grep", "Search file contents with regex", false),
    (
        "Task",
        "Launch autonomous sub-agents for complex tasks",
        true,
    ),
    ("TaskOutput", "Retrieve output from background tasks", false),
    ("TaskStop", "Stop a running background task", false),
    ("WebFetch", "Fetch and process web page content", true),
    (
        "WebSearch",
        "Search the web for up-to-date information",
        true,
    ),
    ("NotebookEdit", "Edit Jupyter notebook cells", true),
    ("Skill", "Invoke slash-command skills", false),
    (
        "TaskCreate",
        "Create a structured task in the task list",
        false,
    ),
    ("TaskGet", "Retrieve task details by ID", false),
    ("TaskUpdate", "Update task status or details", false),
    ("TaskList", "List all tasks and their status", false),
    (
        "AskUserQuestion",
        "Ask the user a question (interactive)",
        false,
    ),
    ("EnterPlanMode", "Enter planning mode (interactive)", false),
    ("ExitPlanMode", "Exit planning mode (interactive)", false),
];

/// Tool info: (description, is_destructive)
pub(super) fn tool_info(name: &str) -> (&'static str, bool) {
    ALL_TOOLS
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, desc, destr)| (*desc, *destr))
        .unwrap_or(("Custom tool", false))
}

/// Format a risk badge for display
pub(super) fn risk_badge(destructive: bool) -> &'static str {
    if destructive {
        "!!!"
    } else {
        ""
    }
}

/// Handle /availabletools command - show all available tools
pub(super) async fn handle_availabletools_command(
    bot: &Bot,
    chat_id: ChatId,
    state: &SharedState,
) -> ResponseResult<()> {
    let mut msg = String::from("<b>Available Tools</b>\n\n");

    for &(name, desc, destructive) in ALL_TOOLS {
        let badge = risk_badge(destructive);
        if badge.is_empty() {
            msg.push_str(&format!(
                "<code>{}</code> — {}\n",
                html_escape(name),
                html_escape(desc)
            ));
        } else {
            msg.push_str(&format!(
                "<code>{}</code> {} — {}\n",
                html_escape(name),
                badge,
                html_escape(desc)
            ));
        }
    }
    msg.push_str(&format!(
        "\n{} = destructive\nTotal: {}",
        risk_badge(true),
        ALL_TOOLS.len()
    ));

    send_long_message(bot, chat_id, &msg, Some(ParseMode::Html), state).await?;

    Ok(())
}

/// Handle /allowedtools command - show current allowed tools list
pub(super) async fn handle_allowedtools_command(
    bot: &Bot,
    chat_id: ChatId,
    state: &SharedState,
) -> ResponseResult<()> {
    let tools = {
        let data = state.lock().await;
        super::bot::get_allowed_tools(&data.settings, chat_id)
    };

    let mut msg = String::from("<b>Allowed Tools</b>\n\n");
    for tool in &tools {
        let (desc, destructive) = tool_info(tool);
        let badge = risk_badge(destructive);
        if badge.is_empty() {
            msg.push_str(&format!(
                "<code>{}</code> — {}\n",
                html_escape(tool),
                html_escape(desc)
            ));
        } else {
            msg.push_str(&format!(
                "<code>{}</code> {} — {}\n",
                html_escape(tool),
                badge,
                html_escape(desc)
            ));
        }
    }
    msg.push_str(&format!(
        "\n{} = destructive\nTotal: {}",
        risk_badge(true),
        tools.len()
    ));

    shared_rate_limit_wait(state, chat_id).await;
    bot.send_message(chat_id, &msg)
        .parse_mode(ParseMode::Html)
        .await?;

    Ok(())
}

/// Handle /allowed command - add/remove tools
/// Usage: /allowed +toolname  (add)
///        /allowed -toolname  (remove)
pub(super) async fn handle_allowed_command(
    bot: &Bot,
    chat_id: ChatId,
    text: &str,
    state: &SharedState,
    token: &str,
) -> ResponseResult<()> {
    let arg = text.strip_prefix("/allowed").unwrap_or("").trim();

    if arg.is_empty() {
        shared_rate_limit_wait(state, chat_id).await;
        bot.send_message(chat_id, "Usage:\n/allowed +toolname — Add a tool\n/allowed -toolname — Remove a tool\n/allowedtools — Show current list")
            .await?;
        return Ok(());
    }

    // Skip if argument starts with "tools" (that's /allowedtools handled separately)
    if arg.starts_with("tools") {
        // This shouldn't happen due to routing order, but just in case
        return handle_allowedtools_command(bot, chat_id, state).await;
    }

    let (op, raw_name) = if let Some(name) = arg.strip_prefix('+') {
        ('+', name.trim())
    } else if let Some(name) = arg.strip_prefix('-') {
        ('-', name.trim())
    } else {
        shared_rate_limit_wait(state, chat_id).await;
        bot.send_message(
            chat_id,
            "Use +toolname to add or -toolname to remove.\nExample: /allowed +Bash",
        )
        .await?;
        return Ok(());
    };

    if raw_name.is_empty() {
        shared_rate_limit_wait(state, chat_id).await;
        bot.send_message(chat_id, "Tool name cannot be empty.")
            .await?;
        return Ok(());
    }

    let tool_name = normalize_tool_name(raw_name);

    let response_msg = {
        let mut data = state.lock().await;
        let chat_key = chat_id.0.to_string();
        // Ensure this chat has its own tool list (initialize from defaults if missing)
        if !data.settings.allowed_tools.contains_key(&chat_key) {
            let defaults: Vec<String> = DEFAULT_ALLOWED_TOOLS
                .iter()
                .map(|s| s.to_string())
                .collect();
            data.settings
                .allowed_tools
                .insert(chat_key.clone(), defaults);
        }
        #[allow(clippy::unwrap_used)] // key was just inserted above
        let tools = data.settings.allowed_tools.get_mut(&chat_key).unwrap();
        match op {
            '+' => {
                if tools.iter().any(|t| t == &tool_name) {
                    format!(
                        "<code>{}</code> is already in the list.",
                        html_escape(&tool_name)
                    )
                } else {
                    tools.push(tool_name.clone());
                    save_bot_settings(token, &data.settings);
                    format!("Added <code>{}</code>", html_escape(&tool_name))
                }
            }
            '-' => {
                let before_len = tools.len();
                tools.retain(|t| t != &tool_name);
                if tools.len() < before_len {
                    save_bot_settings(token, &data.settings);
                    format!("Removed <code>{}</code>", html_escape(&tool_name))
                } else {
                    format!(
                        "<code>{}</code> is not in the list.",
                        html_escape(&tool_name)
                    )
                }
            }
            _ => unreachable!(),
        }
    };

    shared_rate_limit_wait(state, chat_id).await;
    bot.send_message(chat_id, &response_msg)
        .parse_mode(ParseMode::Html)
        .await?;

    Ok(())
}
