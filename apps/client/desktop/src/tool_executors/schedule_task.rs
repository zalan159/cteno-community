//! Schedule Task Tool Executor
//!
//! Creates a scheduled task via direct call to the TaskScheduler.

use crate::tool::ToolExecutor;
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct ScheduleTaskExecutor;

impl ScheduleTaskExecutor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ScheduleTaskExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolExecutor for ScheduleTaskExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        let name = input
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: name")?;
        let task_prompt = input
            .get("task_prompt")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: task_prompt")?;
        let schedule_kind = input
            .get("schedule_kind")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: schedule_kind")?;
        let timezone = input
            .get("timezone")
            .and_then(|v| v.as_str())
            .unwrap_or("Asia/Shanghai");
        let delete_after_run = input
            .get("delete_after_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Build schedule based on kind
        let schedule: crate::scheduler::ScheduleType = match schedule_kind {
            "at" => {
                let at = if let Some(delay_seconds) =
                    input.get("schedule_in_seconds").and_then(|v| v.as_u64())
                {
                    compute_relative_at_iso(delay_seconds, timezone)?
                } else {
                    input
                        .get("schedule_at")
                        .and_then(|v| v.as_str())
                        .ok_or(
                            "Either schedule_at or schedule_in_seconds is required when schedule_kind=at",
                        )?
                        .to_string()
                };
                crate::scheduler::ScheduleType::At { at }
            }
            "every" => {
                let every_seconds = input
                    .get("schedule_every_seconds")
                    .and_then(|v| v.as_u64())
                    .ok_or("schedule_every_seconds is required when schedule_kind=every")?;
                if every_seconds < 60 {
                    return Err("schedule_every_seconds must be at least 60".to_string());
                }
                crate::scheduler::ScheduleType::Every {
                    every_seconds,
                    anchor: None,
                }
            }
            "cron" => {
                let expr = input
                    .get("schedule_cron")
                    .and_then(|v| v.as_str())
                    .ok_or("schedule_cron is required when schedule_kind=cron")?;
                crate::scheduler::ScheduleType::Cron {
                    expr: expr.to_string(),
                }
            }
            _ => return Err(format!("Invalid schedule_kind: {}", schedule_kind)),
        };

        // Determine task_type
        let task_type_str = input
            .get("task_type")
            .and_then(|v| v.as_str())
            .unwrap_or("dispatch");
        let task_type = crate::scheduler::TaskExecutionType::from_str_lossy(task_type_str);

        let (persona_id, session_id) = match &task_type {
            crate::scheduler::TaskExecutionType::Script => {
                // Script tasks record owner_id for UI grouping,
                // but execution doesn't go through PersonaManager.
                let pid = crate::agent_owner::extract_owner_id(&input);
                let sid = resolve_session_id_for_owner(pid, &input);
                (pid, sid)
            }
            crate::scheduler::TaskExecutionType::Dispatch => {
                let pid = crate::agent_owner::extract_owner_id(&input);
                let sid = resolve_session_id_for_owner(pid, &input);
                (pid, sid)
            }
        };

        // Compute initial next_run
        let now_ms = chrono::Utc::now().timestamp_millis();
        let next_run_at = crate::scheduler::timer::compute_next_run(&schedule, timezone, now_ms);

        let task = crate::scheduler::ScheduledTask {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            task_prompt: task_prompt.to_string(),
            enabled: true,
            delete_after_run,
            schedule,
            timezone: timezone.to_string(),
            session_id,
            persona_id: persona_id.map(|s| s.to_string()),
            task_type,
            state: crate::scheduler::TaskState {
                next_run_at,
                ..Default::default()
            },
            created_at: now_ms,
            updated_at: now_ms,
        };

        let scheduler = crate::local_services::scheduler()
            .map_err(|e| format!("Scheduler not available: {}", e))?;
        scheduler.create_task(&task)?;

        let next_run_str = if let Some(ms) = next_run_at {
            format_timestamp_ms(ms)
        } else {
            "N/A".to_string()
        };

        Ok(json!({
            "id": task.id,
            "name": name,
            "next_run": next_run_str,
            "message": format!("Scheduled task '{}' created. Next run: {}", name, next_run_str)
        })
        .to_string())
    }
}

/// Resolve the session_id for a scheduled task based on the owner ID.
/// Tries persona first, then hypothesis agent, then falls back to __session_id or "scheduler".
fn resolve_session_id_for_owner(owner_id: Option<&str>, input: &serde_json::Value) -> String {
    if let Some(pid) = owner_id {
        // Try as persona first
        if let Ok(mgr) = crate::local_services::persona_manager() {
            if let Ok(Some(p)) = mgr.store().get_persona(pid) {
                return p.chat_session_id;
            }
        }
    }
    input
        .get("__session_id")
        .and_then(|v| v.as_str())
        .unwrap_or("scheduler")
        .to_string()
}

fn compute_relative_at_iso(delay_seconds: u64, timezone: &str) -> Result<String, String> {
    if delay_seconds == 0 {
        return Err("schedule_in_seconds must be greater than 0".to_string());
    }

    let seconds =
        i64::try_from(delay_seconds).map_err(|_| "schedule_in_seconds is too large".to_string())?;
    let tz = timezone
        .parse::<chrono_tz::Tz>()
        .map_err(|_| format!("Invalid timezone: {}", timezone))?;
    let at = chrono::Utc::now()
        .with_timezone(&tz)
        .checked_add_signed(chrono::Duration::seconds(seconds))
        .ok_or("Computed schedule time overflowed".to_string())?;

    Ok(at.to_rfc3339())
}

fn format_timestamp_ms(ms: i64) -> String {
    use chrono::{DateTime, FixedOffset, Utc};
    let dt = DateTime::<Utc>::from_timestamp_millis(ms)
        .unwrap_or_default()
        .with_timezone(&FixedOffset::east_opt(8 * 3600).unwrap());
    dt.format("%Y-%m-%d %H:%M").to_string()
}
