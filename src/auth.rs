use std::path::Path;

/// Permission levels for bot users.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionLevel {
    /// Bot owner (first user to DM — imprinting auth)
    Owner,
    /// Public-mode user (non-owner in a group chat with public mode enabled)
    Public,
    /// Denied (non-owner in a private or non-public group)
    Denied,
}

/// Risk classification for commands and actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandRisk {
    /// Read-only, no side effects: /help, /pwd, /availabletools
    Low,
    /// May read sensitive data: /down, /allowedtools
    Medium,
    /// Modifies state or executes code: /cd, /allowed, !shell, AI prompts
    High,
    /// Administrative: /stop, /clear, /start, /public
    Critical,
}

/// Classify the risk level of a Telegram command or message.
pub fn classify_command(command_text: &str) -> CommandRisk {
    let trimmed = command_text.trim();
    let lower = trimmed.to_lowercase();

    // Extract the command part (first word)
    let cmd = lower.split_whitespace().next().unwrap_or("");

    match cmd {
        // Low risk: read-only
        "/help" | "/pwd" | "/availabletools" => CommandRisk::Low,

        // Medium risk: may expose data
        "/down" | "/allowedtools" => CommandRisk::Medium,

        // Critical: admin operations
        "/stop" | "/clear" | "/start" | "/public" => CommandRisk::Critical,

        // High risk: modifies state
        "/cd" | "/allowed" => CommandRisk::High,

        _ => {
            // Shell commands (!) are high risk
            if trimmed.starts_with('!') {
                CommandRisk::High
            } else {
                // Regular AI prompts — high risk (executes code via AI)
                CommandRisk::High
            }
        }
    }
}

/// Check whether a user with the given context can execute a command of the given risk.
///
/// - Owners can execute anything.
/// - Public users can only execute Low-risk commands.
/// - Denied users cannot execute anything.
pub fn can_execute(permission: PermissionLevel, risk: CommandRisk) -> bool {
    match permission {
        PermissionLevel::Owner => true,
        PermissionLevel::Public => matches!(risk, CommandRisk::Low),
        PermissionLevel::Denied => false,
    }
}

/// Determine the permission level for a user in a given context.
pub fn get_permission_level(
    user_id: u64,
    owner_user_id: Option<u64>,
    is_public_chat: bool,
) -> PermissionLevel {
    match owner_user_id {
        Some(owner) if user_id == owner => PermissionLevel::Owner,
        Some(_) if is_public_chat => PermissionLevel::Public,
        Some(_) => PermissionLevel::Denied,
        // No owner yet — first user gets owner (imprinting handled elsewhere)
        None => PermissionLevel::Owner,
    }
}

/// Check whether a target path stays within the sandbox root.
///
/// Both paths are canonicalized before comparison to prevent traversal attacks
/// (e.g. `../../etc/passwd`).
#[allow(dead_code)]
pub fn is_path_within_sandbox(target: &Path, sandbox_root: &Path) -> bool {
    let Ok(canonical_target) = target.canonicalize() else {
        // If the path doesn't exist yet, resolve the parent
        if let Some(parent) = target.parent() {
            if let Ok(canonical_parent) = parent.canonicalize() {
                if let Ok(canonical_root) = sandbox_root.canonicalize() {
                    return canonical_parent.starts_with(&canonical_root);
                }
            }
        }
        return false;
    };

    let Ok(canonical_root) = sandbox_root.canonicalize() else {
        return false;
    };

    canonical_target.starts_with(&canonical_root)
}

/// Maximum file upload size in bytes (50 MB).
pub const DEFAULT_UPLOAD_LIMIT: u64 = 50 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_classify_help_is_low() {
        assert_eq!(classify_command("/help"), CommandRisk::Low);
        assert_eq!(classify_command("/pwd"), CommandRisk::Low);
        assert_eq!(classify_command("/availabletools"), CommandRisk::Low);
    }

    #[test]
    fn test_classify_down_is_medium() {
        assert_eq!(classify_command("/down somefile.txt"), CommandRisk::Medium);
        assert_eq!(classify_command("/allowedtools"), CommandRisk::Medium);
    }

    #[test]
    fn test_classify_cd_is_high() {
        assert_eq!(classify_command("/cd /tmp"), CommandRisk::High);
        assert_eq!(classify_command("/allowed add Bash"), CommandRisk::High);
    }

    #[test]
    fn test_classify_shell_is_high() {
        assert_eq!(classify_command("!ls -la"), CommandRisk::High);
        assert_eq!(classify_command("!rm -rf /"), CommandRisk::High);
    }

    #[test]
    fn test_classify_stop_is_critical() {
        assert_eq!(classify_command("/stop"), CommandRisk::Critical);
        assert_eq!(classify_command("/clear"), CommandRisk::Critical);
        assert_eq!(classify_command("/start"), CommandRisk::Critical);
        assert_eq!(classify_command("/public"), CommandRisk::Critical);
    }

    #[test]
    fn test_classify_ai_prompt_is_high() {
        assert_eq!(classify_command("explain this code"), CommandRisk::High);
    }

    #[test]
    fn test_owner_can_execute_all() {
        assert!(can_execute(PermissionLevel::Owner, CommandRisk::Low));
        assert!(can_execute(PermissionLevel::Owner, CommandRisk::Medium));
        assert!(can_execute(PermissionLevel::Owner, CommandRisk::High));
        assert!(can_execute(PermissionLevel::Owner, CommandRisk::Critical));
    }

    #[test]
    fn test_public_can_only_execute_low() {
        assert!(can_execute(PermissionLevel::Public, CommandRisk::Low));
        assert!(!can_execute(PermissionLevel::Public, CommandRisk::Medium));
        assert!(!can_execute(PermissionLevel::Public, CommandRisk::High));
        assert!(!can_execute(PermissionLevel::Public, CommandRisk::Critical));
    }

    #[test]
    fn test_denied_cannot_execute_anything() {
        assert!(!can_execute(PermissionLevel::Denied, CommandRisk::Low));
        assert!(!can_execute(PermissionLevel::Denied, CommandRisk::Critical));
    }

    #[test]
    fn test_get_permission_owner() {
        assert_eq!(
            get_permission_level(123, Some(123), false),
            PermissionLevel::Owner
        );
    }

    #[test]
    fn test_get_permission_public() {
        assert_eq!(
            get_permission_level(456, Some(123), true),
            PermissionLevel::Public
        );
    }

    #[test]
    fn test_get_permission_denied() {
        assert_eq!(
            get_permission_level(456, Some(123), false),
            PermissionLevel::Denied
        );
    }

    #[test]
    fn test_get_permission_no_owner_imprints() {
        assert_eq!(
            get_permission_level(789, None, false),
            PermissionLevel::Owner
        );
    }

    #[test]
    fn test_path_within_sandbox() {
        let tmp = std::env::temp_dir();
        let sandbox = tmp.join("opencodex_test_sandbox");
        let inner = sandbox.join("subdir");
        let _ = fs::create_dir_all(&inner);

        assert!(is_path_within_sandbox(&inner, &sandbox));
        assert!(is_path_within_sandbox(&sandbox, &sandbox));

        let _ = fs::remove_dir_all(&sandbox);
    }

    #[test]
    fn test_path_outside_sandbox() {
        let tmp = std::env::temp_dir();
        let sandbox = tmp.join("opencodex_test_sandbox2");
        let _ = fs::create_dir_all(&sandbox);

        // /tmp itself is outside the sandbox subdirectory
        assert!(!is_path_within_sandbox(&tmp, &sandbox));

        let _ = fs::remove_dir_all(&sandbox);
    }

    #[test]
    fn test_path_traversal_blocked() {
        let tmp = std::env::temp_dir();
        let sandbox = tmp.join("opencodex_test_sandbox3");
        let _ = fs::create_dir_all(&sandbox);

        let traversal = sandbox.join("..").join("..").join("etc").join("passwd");
        assert!(!is_path_within_sandbox(&traversal, &sandbox));

        let _ = fs::remove_dir_all(&sandbox);
    }

    #[test]
    fn test_upload_limit_is_50mb() {
        assert_eq!(DEFAULT_UPLOAD_LIMIT, 50 * 1024 * 1024);
    }
}
