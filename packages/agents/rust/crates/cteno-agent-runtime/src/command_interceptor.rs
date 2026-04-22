//! Slash Command parsing — runtime-native piece of the command interceptor.
//!
//! Wave 3.3b split the command interceptor:
//!
//! * This file owns the `SlashCommand` enum and the pure `parse()` entry point
//!   (no side effects, no host deps).  Runtime code that wants to peek at
//!   whether a user message is a slash command can use this directly.
//! * The app crate keeps `CommandHandler`, which is the side-effectful executor
//!   (touches `AgentSessionManager` for /clear and /list, `local_services` for
//!   /status).  It remains exposed to the runtime through the existing
//!   `hooks::CommandInterceptor` trait (installed by `agent_hooks`).

/// Slash command discriminator.  Covers every builtin command shape the
/// current handler recognises; unknown commands fall through to `Unknown`
/// so handlers can return a friendly error.
#[derive(Debug, Clone, PartialEq)]
pub enum SlashCommand {
    /// Clear a session (/clear [session_id])
    Clear { session_id: Option<String> },
    /// Show system status (/status)
    Status,
    /// Show help message (/help)
    Help,
    /// Stop current task (/stop [session_id])
    Stop { session_id: Option<String> },
    /// List all sessions (/list or /sessions)
    ListSessions,
    /// Unknown command
    Unknown(String),
}

impl SlashCommand {
    /// Parse a user message.  Returns `None` when the message does not start
    /// with `/`, which signals callers to forward the message to the LLM as
    /// normal chat.
    pub fn parse(message: &str) -> Option<Self> {
        let trimmed = message.trim();
        if !trimmed.starts_with('/') {
            return None;
        }

        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.is_empty() {
            return None;
        }

        let cmd = parts[0];
        let args = &parts[1..];

        Some(match cmd {
            "/clear" => SlashCommand::Clear {
                session_id: args.first().map(|s| s.to_string()),
            },
            "/status" => SlashCommand::Status,
            "/help" => SlashCommand::Help,
            "/stop" => SlashCommand::Stop {
                session_id: args.first().map(|s| s.to_string()),
            },
            "/list" | "/sessions" => SlashCommand::ListSessions,
            _ => SlashCommand::Unknown(cmd.to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_slash_command() {
        assert_eq!(
            SlashCommand::parse("/clear"),
            Some(SlashCommand::Clear { session_id: None })
        );
        assert_eq!(
            SlashCommand::parse("/clear session123"),
            Some(SlashCommand::Clear {
                session_id: Some("session123".to_string())
            })
        );
        assert_eq!(SlashCommand::parse("/status"), Some(SlashCommand::Status));
        assert_eq!(SlashCommand::parse("/help"), Some(SlashCommand::Help));
        assert_eq!(
            SlashCommand::parse("/list"),
            Some(SlashCommand::ListSessions)
        );
        assert_eq!(
            SlashCommand::parse("/sessions"),
            Some(SlashCommand::ListSessions)
        );
        assert_eq!(
            SlashCommand::parse("/unknown"),
            Some(SlashCommand::Unknown("/unknown".to_string()))
        );

        assert_eq!(SlashCommand::parse("cc: 分析代码"), None);
        assert_eq!(SlashCommand::parse("帮我整理邮箱"), None);
        assert_eq!(SlashCommand::parse("hello world"), None);
    }
}
