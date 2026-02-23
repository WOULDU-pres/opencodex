mod app;
mod auth;
mod codex;
mod session;
mod telegram;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};
use teloxide::prelude::*;

#[derive(Parser, Debug)]
#[command(version, about = "Telegram + Codex/OMX bridge")]
struct Cli {
    /// Project directory for Codex/OMX execution
    #[arg(value_name = "PROJECT_DIR")]
    project_dir: Option<String>,

    /// Telegram Bot token (saved to config directory)
    #[arg(long)]
    token: Option<String>,

    /// Enable full permission bypass mode
    #[arg(long)]
    madmax: bool,

    /// Use omx binary as AI backend (default: codex)
    #[arg(long)]
    omx: bool,

    /// Internal: send file to Telegram (used by AI output automation)
    #[arg(long, value_name = "FILE_PATH")]
    sendfile: Option<String>,

    /// Internal: target Telegram chat ID (for --sendfile)
    #[arg(long)]
    chat: Option<i64>,

    /// Internal: token hash key (for --sendfile)
    #[arg(long)]
    key: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct AppConfig {
    token: Option<String>,
}

fn config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(app::dir_name()).join("config.json"))
}

fn load_config() -> AppConfig {
    let Some(path) = config_path() else {
        return AppConfig::default();
    };
    let Ok(content) = fs::read_to_string(path) else {
        return AppConfig::default();
    };
    serde_json::from_str::<AppConfig>(&content).unwrap_or_default()
}

fn write_config_file(path: &Path, config: &AppConfig) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(serialized) = serde_json::to_string_pretty(config) {
        let _ = fs::write(path, &serialized);

        // Protect token file: owner-only read/write (0o600)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
        }
    }
}

fn save_config(config: &AppConfig) {
    if let Some(path) = config_path() {
        write_config_file(&path, config);
    }
}

fn resolve_token(cli_token: Option<String>) -> Result<String> {
    if let Some(token) = cli_token {
        let mut cfg = load_config();
        cfg.token = Some(token.clone());
        save_config(&cfg);
        return Ok(token);
    }

    // Binary-specific env var
    let bin_env_var = "OPENCODEX_TELEGRAM_TOKEN";

    if let Ok(token) = env::var(bin_env_var) {
        if !token.trim().is_empty() {
            let mut cfg = load_config();
            cfg.token = Some(token.clone());
            save_config(&cfg);
            return Ok(token);
        }
    }

    if let Ok(token) = env::var("TELEGRAM_BOT_TOKEN") {
        if !token.trim().is_empty() {
            let mut cfg = load_config();
            cfg.token = Some(token.clone());
            save_config(&cfg);
            return Ok(token);
        }
    }

    let cfg = load_config();
    if let Some(token) = cfg.token {
        if !token.trim().is_empty() {
            return Ok(token);
        }
    }

    anyhow::bail!(
        "Telegram token not found. Use one of:\n  1) {} <project_dir> --token <TOKEN>\n  2) export {}=<TOKEN>\n  3) export TELEGRAM_BOT_TOKEN=<TOKEN>\n  4) save token in ~/{}/config.json",
        env!("CARGO_BIN_NAME"),
        bin_env_var,
        app::dir_name(),
    );
}

async fn validate_telegram_token(token: &str) -> Result<()> {
    let url = format!("https://api.telegram.org/bot{}/getMe", token);
    let resp = reqwest::get(&url)
        .await
        .context("Failed to call Telegram getMe API")?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();

    if !status.is_success() {
        anyhow::bail!(
            "Telegram token validation failed (HTTP {}): {}",
            status,
            body
        );
    }

    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    let ok = parsed.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
    if !ok {
        anyhow::bail!("Telegram token validation failed: {}", body);
    }
    Ok(())
}

async fn handle_sendfile(path: &str, chat_id: i64, hash_key: &str) -> Result<()> {
    let token = telegram::resolve_token_by_hash(hash_key)
        .with_context(|| format!("No bot token found for hash key: {}", hash_key))?;

    let file_path = Path::new(path);
    if !file_path.exists() || !file_path.is_file() {
        anyhow::bail!("file not found: {}", path);
    }

    let bot = Bot::new(token);
    bot.send_document(ChatId(chat_id), teloxide::types::InputFile::file(file_path))
        .await
        .context("failed to send file")?;

    println!("File sent: {}", path);
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    codex::configure_execution(cli.omx, cli.madmax);

    if let Some(path) = cli.sendfile.as_deref() {
        let chat_id = cli
            .chat
            .context("--chat is required when using --sendfile")?;
        let key = cli
            .key
            .as_deref()
            .context("--key is required when using --sendfile")?;
        handle_sendfile(path, chat_id, key).await?;
        return Ok(());
    }

    let project_dir = cli.project_dir.as_deref().context(format!(
        "Usage: {} <project_dir> [--token <TOKEN>] [--madmax] [--omx]",
        env!("CARGO_BIN_NAME"),
    ))?;

    let project_path = Path::new(project_dir);
    if !project_path.exists() || !project_path.is_dir() {
        anyhow::bail!("Invalid project directory: {}", project_dir);
    }

    let canonical_project = project_path
        .canonicalize()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| project_dir.to_string());

    let token = resolve_token(cli.token)?;
    validate_telegram_token(&token).await?;

    println!("{} {}", env!("CARGO_BIN_NAME"), env!("CARGO_PKG_VERSION"));
    println!("project_dir: {}", canonical_project);
    println!(
        "ai_backend: {}",
        if cli.omx {
            "omx (--omx)"
        } else {
            "codex (default)"
        }
    );
    println!("status: connecting Telegram bot...");

    telegram::run_bot(&token, &canonical_project).await;

    Ok(())
}
