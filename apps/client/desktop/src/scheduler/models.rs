//! Scheduler data models
//!
//! Defines ScheduledTask, ScheduleType, TaskState and related types.

use serde::{Deserialize, Serialize};

/// Execution type for a scheduled task.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TaskExecutionType {
    /// Dispatch a prompt to a Persona via PersonaManager::dispatch_task().
    Dispatch,
    /// Execute `task_prompt` as a shell command directly.
    Script,
}

impl Default for TaskExecutionType {
    fn default() -> Self {
        Self::Dispatch
    }
}

impl TaskExecutionType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Dispatch => "dispatch",
            Self::Script => "script",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "script" => Self::Script,
            _ => Self::Dispatch,
        }
    }
}

/// A scheduled (timed) task persisted in SQLite.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTask {
    /// Unique identifier (UUID)
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Full prompt that the Agent will execute (or shell command for Script type)
    pub task_prompt: String,
    /// Whether this task is enabled
    pub enabled: bool,
    /// Delete after a single successful run
    pub delete_after_run: bool,
    /// Schedule definition
    pub schedule: ScheduleType,
    /// IANA timezone (e.g. "Asia/Shanghai")
    pub timezone: String,
    /// Happy session ID for result delivery
    pub session_id: String,
    /// Persona that owns this task (for Dispatch type), or hypothesis agent ID (for Hypothesis type).
    pub persona_id: Option<String>,
    /// Task execution type: dispatch (default), script, or hypothesis.
    pub task_type: TaskExecutionType,
    /// Runtime state
    pub state: TaskState,
    /// Creation time (Unix milliseconds)
    pub created_at: i64,
    /// Last update time (Unix milliseconds)
    pub updated_at: i64,
}

/// How the task is scheduled.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ScheduleType {
    /// Fire once at a specific ISO-8601 datetime.
    At { at: String },
    /// Repeat at a fixed interval.
    Every {
        every_seconds: u64,
        anchor: Option<String>,
    },
    /// Standard 5-field cron expression.
    Cron { expr: String },
}

/// Runtime state of a scheduled task.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TaskState {
    /// Next scheduled run (Unix ms). None = expired / not computable.
    pub next_run_at: Option<i64>,
    /// Set while the task is executing (prevents concurrent runs).
    pub running_since: Option<i64>,
    /// When the task last ran (Unix ms).
    pub last_run_at: Option<i64>,
    /// Outcome of the last run.
    pub last_status: Option<TaskRunStatus>,
    /// Short summary of last result.
    pub last_result_summary: Option<String>,
    /// How many times in a row the task has failed.
    pub consecutive_errors: u32,
    /// Lifetime run count.
    pub total_runs: u32,
}

/// Outcome of a single run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TaskRunStatus {
    Success,
    Failed,
    TimedOut,
    Skipped,
}
