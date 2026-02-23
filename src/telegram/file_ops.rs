use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use teloxide::prelude::*;

use crate::auth;
use crate::i18n;
use crate::session::{enforce_history_cap, HistoryItem, HistoryType};

use super::bot::SharedState;
use super::storage::save_session_to_file;
use super::streaming::{html_escape, send_long_message, shared_rate_limit_wait};

const SHELL_TIMEOUT: Duration = Duration::from_secs(60);

/// Handle /down <filepath> - send file to user
pub(super) async fn handle_down_command(
    bot: &Bot,
    chat_id: ChatId,
    text: &str,
    state: &SharedState,
) -> ResponseResult<()> {
    let file_path = text.strip_prefix("/down").unwrap_or("").trim();

    if file_path.is_empty() {
        shared_rate_limit_wait(state, chat_id).await;
        bot.send_message(
            chat_id,
            "Usage: /down <filepath>\nExample: /down /home/kst/file.txt",
        )
        .await?;
        return Ok(());
    }

    // Resolve relative path using current session path
    let resolved_path = if Path::new(file_path).is_absolute() {
        file_path.to_string()
    } else {
        let current_path = {
            let data = state.lock().await;
            data.sessions
                .get(&chat_id)
                .and_then(|s| s.current_path.clone())
        };
        match current_path {
            Some(base) => format!("{}/{}", base.trim_end_matches('/'), file_path),
            None => {
                shared_rate_limit_wait(state, chat_id).await;
                bot.send_message(chat_id, i18n::MSG_NO_SESSION).await?;
                return Ok(());
            }
        }
    };

    let path = Path::new(&resolved_path);
    if !path.exists() {
        shared_rate_limit_wait(state, chat_id).await;
        bot.send_message(chat_id, format!("File not found: {}", resolved_path))
            .await?;
        return Ok(());
    }
    if !path.is_file() {
        shared_rate_limit_wait(state, chat_id).await;
        bot.send_message(chat_id, format!("Not a file: {}", resolved_path))
            .await?;
        return Ok(());
    }

    shared_rate_limit_wait(state, chat_id).await;
    bot.send_document(chat_id, teloxide::types::InputFile::file(path))
        .await?;

    Ok(())
}

/// Handle file/photo upload - save to current session path
pub(super) async fn handle_file_upload(
    bot: &Bot,
    chat_id: ChatId,
    msg: &Message,
    state: &SharedState,
) -> ResponseResult<()> {
    // Get current session path
    let current_path = {
        let data = state.lock().await;
        data.sessions
            .get(&chat_id)
            .and_then(|s| s.current_path.clone())
    };

    let Some(save_dir) = current_path else {
        shared_rate_limit_wait(state, chat_id).await;
        bot.send_message(chat_id, i18n::MSG_NO_SESSION).await?;
        return Ok(());
    };

    // Get file_id and file_name
    let (file_id, file_name) = if let Some(doc) = msg.document() {
        let name = doc
            .file_name
            .clone()
            .unwrap_or_else(|| "uploaded_file".to_string());
        (doc.file.id.clone(), name)
    } else if let Some(photos) = msg.photo() {
        // Get the largest photo
        if let Some(photo) = photos.last() {
            let name = format!("photo_{}.jpg", photo.file.unique_id);
            (photo.file.id.clone(), name)
        } else {
            return Ok(());
        }
    } else {
        return Ok(());
    };

    // Download file from Telegram via HTTP
    shared_rate_limit_wait(state, chat_id).await;
    let file = bot.get_file(&file_id).await?;
    let url = format!(
        "https://api.telegram.org/file/bot{}/{}",
        bot.token(),
        file.path
    );
    let buf = match reqwest::get(&url).await {
        Ok(resp) => match resp.bytes().await {
            Ok(bytes) => bytes,
            Err(e) => {
                shared_rate_limit_wait(state, chat_id).await;
                bot.send_message(chat_id, format!("Download failed: {}", e))
                    .await?;
                return Ok(());
            }
        },
        Err(e) => {
            shared_rate_limit_wait(state, chat_id).await;
            bot.send_message(chat_id, format!("Download failed: {}", e))
                .await?;
            return Ok(());
        }
    };

    // Enforce upload size limit
    if buf.len() as u64 > auth::DEFAULT_UPLOAD_LIMIT {
        shared_rate_limit_wait(state, chat_id).await;
        bot.send_message(
            chat_id,
            format!(
                "File too large ({:.1} MB). Limit is {} MB.",
                buf.len() as f64 / (1024.0 * 1024.0),
                auth::DEFAULT_UPLOAD_LIMIT / (1024 * 1024)
            ),
        )
        .await?;
        return Ok(());
    }

    // Save to session path (sanitize file_name to prevent path traversal)
    let safe_name = Path::new(&file_name)
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("uploaded_file"));
    let dest = Path::new(&save_dir).join(safe_name);
    let file_size = buf.len();
    match fs::write(&dest, &buf) {
        Ok(_) => {
            let msg_text = format!("Saved: {}\n({} bytes)", dest.display(), file_size);
            shared_rate_limit_wait(state, chat_id).await;
            bot.send_message(chat_id, &msg_text).await?;
        }
        Err(e) => {
            shared_rate_limit_wait(state, chat_id).await;
            bot.send_message(chat_id, format!("Failed to save file: {}", e))
                .await?;
            return Ok(());
        }
    }

    // Record upload in session history and pending queue for Claude Code
    let upload_record = format!(
        "[File uploaded] {} → {} ({} bytes)",
        file_name,
        dest.display(),
        file_size
    );
    {
        let mut data = state.lock().await;
        if let Some(session) = data.sessions.get_mut(&chat_id) {
            session.history.push(HistoryItem {
                item_type: HistoryType::User,
                content: upload_record.clone(),
            });
            enforce_history_cap(&mut session.history);
            session.pending_uploads.push(upload_record);
            save_session_to_file(session, &save_dir);
        }
    }

    Ok(())
}

/// Handle !command - execute shell command directly
pub(super) async fn handle_shell_command(
    bot: &Bot,
    chat_id: ChatId,
    text: &str,
    state: &SharedState,
) -> ResponseResult<()> {
    let cmd_str = text.strip_prefix('!').unwrap_or("").trim();

    if cmd_str.is_empty() {
        shared_rate_limit_wait(state, chat_id).await;
        bot.send_message(
            chat_id,
            "Usage: !<command>\nExample: !mkdir /home/kst/testcode",
        )
        .await?;
        return Ok(());
    }

    // Get current_path for working directory (default to home directory)
    let working_dir = {
        let data = state.lock().await;
        data.sessions
            .get(&chat_id)
            .and_then(|s| s.current_path.clone())
            .unwrap_or_else(|| {
                dirs::home_dir()
                    .map(|h| h.display().to_string())
                    .unwrap_or_else(|| "/".to_string())
            })
    };

    let cmd_owned = cmd_str.to_string();
    let working_dir_clone = working_dir.clone();
    let state_for_blocking = state.clone();

    // Run shell command in blocking thread with stdin closed and timeout
    let result = tokio::task::spawn_blocking(move || {
        let mut child = std::process::Command::new("bash")
            .args(["-c", &cmd_owned])
            .current_dir(&working_dir_clone)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;

        let shell_pid = child.id();
        {
            let mut data = state_for_blocking.blocking_lock();
            data.shell_pids.insert(chat_id, shell_pid);
        }

        let execution_result = {
            let start = Instant::now();
            let mut timed_out = false;

            let mut output = loop {
                match child.try_wait() {
                    Ok(Some(_status)) => {
                        break child.wait_with_output().map_err(|e| e.to_string())?
                    }
                    Ok(None) => {
                        if start.elapsed() > SHELL_TIMEOUT {
                            timed_out = true;
                            let _ = child.kill();
                            break child.wait_with_output().map_err(|e| e.to_string())?;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    Err(e) => return Err(e.to_string()),
                }
            };

            if timed_out {
                if !output.stderr.is_empty() {
                    output.stderr.push(b'\n');
                }
                output
                    .stderr
                    .extend_from_slice(i18n::MSG_SHELL_TIMEOUT.as_bytes());
            }
            Ok(output)
        };

        {
            let mut data = state_for_blocking.blocking_lock();
            data.shell_pids.remove(&chat_id);
        }

        execution_result
    })
    .await;

    let response = match result {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let exit_code = output.status.code().unwrap_or(-1);

            let mut parts = Vec::new();

            if !stdout.is_empty() {
                parts.push(format!("<pre>{}</pre>", html_escape(stdout.trim_end())));
            }
            if !stderr.is_empty() {
                parts.push(format!(
                    "stderr:\n<pre>{}</pre>",
                    html_escape(stderr.trim_end())
                ));
            }
            if parts.is_empty() || exit_code != 0 {
                parts.push(format!("(exit code: {})", exit_code));
            }

            parts.join("\n")
        }
        Ok(Err(e)) => format!("Failed to execute: {}", html_escape(&e)),
        Err(e) => format!("Task error: {}", html_escape(&e.to_string())),
    };

    send_long_message(
        bot,
        chat_id,
        &response,
        Some(teloxide::types::ParseMode::Html),
        state,
    )
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::SHELL_TIMEOUT;

    #[test]
    fn test_shell_timeout_constant_exists() {
        assert_eq!(SHELL_TIMEOUT.as_secs(), 60);
    }
}
