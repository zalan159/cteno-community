//! Asynchronous SubAgent System
//!
//! Allows parent agents to spawn background tasks without blocking.
//! SubAgents run independently and notify the parent when complete.
//!
//! # Architecture
//!
//! - `SubAgent`: Individual task instance with status tracking
//! - `SubAgentManager`: Global manager for spawning and monitoring tasks
//! - `SubAgentNotification`: Completion notification sent to parent session
//!
//! # Example
//!
//! ```rust
//! let manager = SubAgentManager::new();
//! let id = manager.spawn(
//!     "parent_session_123",
//!     "code_analyzer",
//!     "Analyze all Python files",
//!     None,
//!     agent_config,
//!     exec_ctx,
//! ).await?;
//!
//! // Later: check status
//! let subagent = manager.get(&id).await;
//! ```

pub mod manager;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// SubAgent status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SubAgentStatus {
    /// Waiting to start
    Pending,
    /// Currently running
    Running,
    /// Successfully completed
    Completed,
    /// Failed with error
    Failed,
    /// Stopped by user
    Stopped,
    /// Timed out
    TimedOut,
}

impl std::fmt::Display for SubAgentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Running => write!(f, "running"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Stopped => write!(f, "stopped"),
            Self::TimedOut => write!(f, "timed_out"),
        }
    }
}

/// Cleanup policy for SubAgent after completion
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum CleanupPolicy {
    /// Keep the SubAgent data after completion (default)
    #[default]
    Keep,
    /// Automatically delete after completion (with delay)
    Delete,
}

/// SubAgent instance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgent {
    /// Unique identifier
    pub id: String,
    /// Parent session ID that spawned this SubAgent
    pub parent_session_id: String,
    /// Agent configuration ID (e.g., "code_analyzer")
    pub agent_id: String,
    /// Task description/prompt
    pub task: String,
    /// Human-readable label (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Current status
    pub status: SubAgentStatus,
    /// Creation timestamp (Unix milliseconds)
    pub created_at: i64,
    /// Start timestamp (Unix milliseconds)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    /// Completion timestamp (Unix milliseconds)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<i64>,
    /// Result text (for successful completion)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// Error message (for failure)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Current iteration count (ReAct loop)
    #[serde(default)]
    pub iteration_count: u32,
    /// Cleanup policy after completion
    #[serde(default)]
    pub cleanup: CleanupPolicy,
}

impl SubAgent {
    /// Create a new SubAgent instance
    pub fn new(
        id: String,
        parent_session_id: String,
        agent_id: String,
        task: String,
        label: Option<String>,
        cleanup: CleanupPolicy,
    ) -> Self {
        Self {
            id,
            parent_session_id,
            agent_id,
            task,
            label,
            status: SubAgentStatus::Pending,
            created_at: Utc::now().timestamp_millis(),
            started_at: None,
            completed_at: None,
            result: None,
            error: None,
            iteration_count: 0,
            cleanup,
        }
    }

    /// Calculate elapsed time in seconds
    pub fn elapsed_seconds(&self) -> Option<i64> {
        let start = self.started_at?;
        let end = self
            .completed_at
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        Some((end - start) / 1000)
    }

    /// Check if SubAgent is in a terminal state
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status,
            SubAgentStatus::Completed
                | SubAgentStatus::Failed
                | SubAgentStatus::Stopped
                | SubAgentStatus::TimedOut
        )
    }

    /// Check if SubAgent is active (pending or running)
    pub fn is_active(&self) -> bool {
        matches!(
            self.status,
            SubAgentStatus::Pending | SubAgentStatus::Running
        )
    }
}

/// Notification sent to parent session when SubAgent completes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentNotification {
    /// SubAgent ID
    pub subagent_id: String,
    /// Human-readable label
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Task description
    pub task: String,
    /// Final status
    pub status: SubAgentStatus,
    /// Result text (if successful)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// Error message (if failed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Completion timestamp
    pub completed_at: i64,
    /// Elapsed time in seconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed_seconds: Option<i64>,
}

impl SubAgentNotification {
    /// Create notification from SubAgent
    pub fn from_subagent(subagent: &SubAgent) -> Self {
        Self {
            subagent_id: subagent.id.clone(),
            label: subagent.label.clone(),
            task: subagent.task.clone(),
            status: subagent.status.clone(),
            result: subagent.result.clone(),
            error: subagent.error.clone(),
            completed_at: subagent
                .completed_at
                .unwrap_or_else(|| Utc::now().timestamp_millis()),
            elapsed_seconds: subagent.elapsed_seconds(),
        }
    }

    /// Format notification as a message for the Agent
    pub fn to_message(&self) -> String {
        let label = self.label.as_deref().unwrap_or("Background Task");

        match &self.status {
            SubAgentStatus::Completed => {
                let elapsed = self
                    .elapsed_seconds
                    .map(|s| format!(" (took {} seconds)", s))
                    .unwrap_or_default();

                let result_text = self
                    .result
                    .as_ref()
                    .map(|r| format!("\n\nResult:\n{}", r))
                    .unwrap_or_default();

                format!("[SubAgent '{}' Completed{}]{}", label, elapsed, result_text)
            }
            SubAgentStatus::Failed => {
                let error_msg = self.error.as_deref().unwrap_or("Unknown error");

                format!("[SubAgent '{}' Failed]\n\nError: {}", label, error_msg)
            }
            _ => {
                format!("[SubAgent '{}' {}]", label, self.status)
            }
        }
    }
}

/// Filter options for listing SubAgents
#[derive(Debug, Clone, Default)]
pub struct SubAgentFilter {
    /// Filter by parent session ID
    pub parent_session_id: Option<String>,
    /// Filter by status
    pub status: Option<SubAgentStatus>,
    /// Only include active (pending/running) SubAgents
    pub active_only: bool,
}

impl SubAgentFilter {
    /// Check if a SubAgent matches the filter
    pub fn matches(&self, subagent: &SubAgent) -> bool {
        if let Some(ref pid) = self.parent_session_id {
            if &subagent.parent_session_id != pid {
                return false;
            }
        }

        if let Some(ref status) = self.status {
            if &subagent.status != status {
                return false;
            }
        }

        if self.active_only && !subagent.is_active() {
            return false;
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subagent_creation() {
        let sa = SubAgent::new(
            "test-id".to_string(),
            "parent-123".to_string(),
            "code_analyzer".to_string(),
            "Analyze code".to_string(),
            Some("Code Analysis".to_string()),
            CleanupPolicy::Keep,
        );

        assert_eq!(sa.id, "test-id");
        assert_eq!(sa.status, SubAgentStatus::Pending);
        assert!(sa.is_active());
        assert!(!sa.is_terminal());
    }

    #[test]
    fn test_notification_formatting() {
        let mut sa = SubAgent::new(
            "test-id".to_string(),
            "parent-123".to_string(),
            "code_analyzer".to_string(),
            "Analyze code".to_string(),
            Some("Code Analysis".to_string()),
            CleanupPolicy::Keep,
        );

        sa.status = SubAgentStatus::Completed;
        sa.result = Some("Found 15 issues".to_string());
        sa.started_at = Some(Utc::now().timestamp_millis() - 5000);
        sa.completed_at = Some(Utc::now().timestamp_millis());

        let notif = SubAgentNotification::from_subagent(&sa);
        let msg = notif.to_message();

        assert!(msg.contains("Code Analysis"));
        assert!(msg.contains("Completed"));
        assert!(msg.contains("Found 15 issues"));
    }

    #[test]
    fn test_notification_does_not_truncate_long_result() {
        let mut sa = SubAgent::new(
            "test-id".to_string(),
            "parent-123".to_string(),
            "code_analyzer".to_string(),
            "Analyze code".to_string(),
            Some("Code Analysis".to_string()),
            CleanupPolicy::Keep,
        );

        let long_result = format!("LONG_RESULT_START\n{}\nLONG_RESULT_END", "x".repeat(1000));

        sa.status = SubAgentStatus::Completed;
        sa.result = Some(long_result.clone());
        sa.started_at = Some(Utc::now().timestamp_millis() - 5000);
        sa.completed_at = Some(Utc::now().timestamp_millis());

        let notif = SubAgentNotification::from_subagent(&sa);
        let msg = notif.to_message();

        assert!(msg.contains(&long_result));
        assert!(msg.contains("LONG_RESULT_END"));
    }

    #[test]
    fn test_filter() {
        let sa1 = SubAgent::new(
            "id1".to_string(),
            "parent-123".to_string(),
            "agent1".to_string(),
            "task1".to_string(),
            None,
            CleanupPolicy::Keep,
        );

        let mut sa2 = sa1.clone();
        sa2.id = "id2".to_string();
        sa2.status = SubAgentStatus::Completed;

        let filter = SubAgentFilter {
            parent_session_id: Some("parent-123".to_string()),
            status: None,
            active_only: true,
        };

        assert!(filter.matches(&sa1));
        assert!(!filter.matches(&sa2)); // sa2 is completed (not active)
    }
}
