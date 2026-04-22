//! Slash Command Handler (app side).
//!
//! Wave 3.3b split the interceptor: the pure `SlashCommand` enum and `parse()`
//! now live in `cteno_agent_runtime::command_interceptor`; this module keeps
//! the side-effectful `CommandHandler` which needs `AgentSessionManager`
//! access + `local_services` lookups (tool registry / scheduler / persona
//! manager).  `agent_hooks::AppCommandInterceptor` wraps this handler and
//! installs it against the runtime's `CommandInterceptor` trait.

use std::path::PathBuf;

// Re-export the runtime-native SlashCommand so existing callers (including
// `agent_hooks.rs`) keep using `crate::command_interceptor::SlashCommand`.
pub use cteno_agent_runtime::command_interceptor::SlashCommand;

/// Slash Command Handler
pub struct CommandHandler {
    db_path: PathBuf,
}

impl CommandHandler {
    pub fn new(db_path: PathBuf) -> Self {
        Self { db_path }
    }

    /// Execute a slash command
    pub async fn execute(&self, cmd: SlashCommand, session_id: &str) -> Result<String, String> {
        match cmd {
            SlashCommand::Clear { session_id: target } => {
                let target_session = target.as_deref().unwrap_or(session_id);
                self.clear_session(target_session)
            }
            SlashCommand::Status => self.get_status(),
            SlashCommand::Help => Ok(self.get_help()),
            SlashCommand::Stop { session_id: target } => {
                let target_session = target.as_deref().unwrap_or(session_id);
                self.stop_task(target_session)
            }
            SlashCommand::ListSessions => self.list_sessions(),
            SlashCommand::Unknown(cmd) => Err(format!("未知命令: {}. 使用 /help 查看帮助", cmd)),
        }
    }

    /// Clear a session
    fn clear_session(&self, session_id: &str) -> Result<String, String> {
        let manager = crate::agent_session::AgentSessionManager::new(self.db_path.clone());
        manager
            .clear_messages(session_id)
            .map_err(|e| format!("清除会话失败: {}", e))?;
        Ok(format!("会话 {} 已清空", session_id))
    }

    /// Get system status
    fn get_status(&self) -> Result<String, String> {
        let mut status_lines = vec!["系统状态".to_string(), "".to_string()];
        status_lines.push(format!("数据库: {}", self.db_path.display()));

        if crate::local_services::tool_registry().is_ok() {
            status_lines.push("工具注册表: 已初始化".to_string());
        }
        if crate::local_services::scheduler().is_ok() {
            status_lines.push("调度器: 已初始化".to_string());
        }
        if crate::local_services::persona_manager().is_ok() {
            status_lines.push("Persona 管理器: 已初始化".to_string());
        }

        Ok(status_lines.join("\n"))
    }

    /// Get help message
    fn get_help(&self) -> String {
        r#"
Cteno 命令帮助

**Slash Commands** (立即执行，不经过 LLM):
- /clear [session_id]  - 清空当前会话（或指定会话）
- /status              - 查看系统状态
- /list                - 列出所有会话
- /stop [session_id]   - 停止当前任务
- /help                - 显示此帮助信息

**Agent 路由** (透传到特定 Agent):
- cc: <任务>          - 调用 Claude Code 执行编程任务
  示例: cc: 分析 main.rs 的代码结构

**普通对话**:
直接发送消息，Cteno 会智能理解并调用合适的工具完成任务。
        "#
        .trim()
        .to_string()
    }

    /// Stop a task
    fn stop_task(&self, session_id: &str) -> Result<String, String> {
        Ok(format!("已请求停止会话 {} 的任务", session_id))
    }

    /// List all sessions
    fn list_sessions(&self) -> Result<String, String> {
        let manager = crate::agent_session::AgentSessionManager::new(self.db_path.clone());
        let sessions = manager
            .list_by_agent("worker", None)
            .map_err(|e| format!("获取会话列表失败: {}", e))?;

        let mut lines = vec!["会话列表".to_string(), "".to_string()];

        if sessions.is_empty() {
            lines.push("（暂无会话）".to_string());
        } else {
            for (i, session) in sessions.iter().enumerate() {
                lines.push(format!(
                    "{}. {} [{}] - {} messages",
                    i + 1,
                    session.id,
                    session.agent_id,
                    session.messages.len()
                ));
            }
        }

        Ok(lines.join("\n"))
    }
}
