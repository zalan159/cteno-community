//! Scheduler timer loop and next-run computation.

use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use super::models::{ScheduleType, ScheduledTask, TaskExecutionType, TaskRunStatus, TaskState};
use super::store::TaskStore;

/// The main task scheduler.
pub struct TaskScheduler {
    store: TaskStore,
}

impl TaskScheduler {
    /// Create a new scheduler backed by the given SQLite database.
    pub fn new(db_path: PathBuf) -> Self {
        let store = TaskStore::new(db_path);
        Self { store }
    }

    // ── public helpers used by HTTP handlers ────────────────────────────

    pub fn list_tasks(&self, enabled_only: bool) -> Result<Vec<ScheduledTask>, String> {
        self.store.list(enabled_only)
    }

    pub fn get_task(&self, id: &str) -> Result<Option<ScheduledTask>, String> {
        self.store.get(id)
    }

    pub fn create_task(&self, task: &ScheduledTask) -> Result<(), String> {
        self.store.create(task)
    }

    pub fn update_task(&self, task: &ScheduledTask) -> Result<(), String> {
        self.store.update_task(task)
    }

    pub fn update_state(&self, id: &str, state: &TaskState) -> Result<(), String> {
        self.store.update_state(id, state)
    }

    pub fn update_enabled(&self, id: &str, enabled: bool) -> Result<(), String> {
        self.store.update_enabled(id, enabled)
    }

    pub fn delete_task(&self, id: &str) -> Result<bool, String> {
        self.store.delete(id)
    }

    pub fn delete_tasks_by_session(&self, session_id: &str) -> Result<usize, String> {
        self.store.delete_by_session(session_id)
    }

    // ── timer loop ─────────────────────────────────────────────────────

    /// Startup recovery: recompute `next_run_at` for all enabled tasks and
    /// clear stale `running_since` markers (> 15 min old).
    fn recover_on_startup(&self) {
        let now = Utc::now().timestamp_millis();
        let tasks = match self.store.list(true) {
            Ok(t) => t,
            Err(e) => {
                log::error!("[Scheduler] Failed to load tasks for recovery: {}", e);
                return;
            }
        };

        for mut task in tasks {
            let mut changed = false;

            // Clear stale running_since (> 15 min)
            if let Some(since) = task.state.running_since {
                if now - since > 15 * 60 * 1000 {
                    log::warn!(
                        "[Scheduler] Clearing stale running_since for task '{}'",
                        task.name
                    );
                    task.state.running_since = None;
                    changed = true;
                }
            }

            // Recompute next_run_at
            let next = compute_next_run(&task.schedule, &task.timezone, now);
            if next != task.state.next_run_at {
                task.state.next_run_at = next;
                changed = true;
            }

            if changed {
                if let Err(e) = self.store.update_state(&task.id, &task.state) {
                    log::error!(
                        "[Scheduler] Failed to update state for '{}': {}",
                        task.name,
                        e
                    );
                }
            }
        }
    }

    /// Main loop: checks for due tasks every 30 seconds.
    pub async fn run(&self) {
        log::info!("[Scheduler] Timer loop starting, recovering tasks...");
        self.recover_on_startup();
        log::info!("[Scheduler] Recovery complete, entering main loop");

        loop {
            if let Err(e) = self.check_due_tasks().await {
                log::error!("[Scheduler] Error checking due tasks: {}", e);
            }
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    }

    /// Find due tasks, dispatch each, update state.
    async fn check_due_tasks(&self) -> Result<(), String> {
        let now = Utc::now().timestamp_millis();
        let due = self.store.get_due_tasks(now)?;

        for mut task in due {
            log::info!(
                "[Scheduler] Task '{}' ({}) is due — dispatching",
                task.name,
                task.id
            );

            // Mark running
            task.state.running_since = Some(now);
            self.store.update_state(&task.id, &task.state)?;

            let task_id = task.id.clone();
            let task_name = task.name.clone();
            let delete_after_run = task.delete_after_run;
            let schedule = task.schedule.clone();
            let timezone = task.timezone.clone();

            let result = self.dispatch_task_for_schedule(&task).await;

            match result {
                Ok(result_summary) => {
                    log::info!(
                        "[Scheduler] Task '{}' ({}) completed: {}",
                        task_name,
                        task_id,
                        &result_summary[..result_summary.len().min(200)]
                    );

                    let new_state = TaskState {
                        running_since: None,
                        last_run_at: Some(now),
                        last_status: Some(TaskRunStatus::Success),
                        last_result_summary: Some(
                            result_summary[..result_summary.len().min(500)].to_string(),
                        ),
                        consecutive_errors: 0,
                        total_runs: task.state.total_runs + 1,
                        next_run_at: compute_next_run(&schedule, &timezone, now),
                    };

                    self.store.update_state(&task_id, &new_state)?;

                    // Handle delete_after_run
                    if delete_after_run {
                        log::info!(
                            "[Scheduler] Deleting one-shot task '{}' after run",
                            task_name
                        );
                        self.store.delete(&task_id)?;
                    }
                }
                Err(e) => {
                    log::error!("[Scheduler] Failed to dispatch task '{}': {}", task_name, e);

                    task.state.running_since = None;
                    task.state.last_run_at = Some(now);
                    task.state.last_status = Some(TaskRunStatus::Failed);
                    task.state.last_result_summary = Some(format!("Failed to start: {}", e));
                    task.state.consecutive_errors += 1;
                    task.state.total_runs += 1;
                    task.state.next_run_at = compute_next_run(&schedule, &timezone, now);

                    self.store.update_state(&task_id, &task.state)?;
                }
            }

            log::info!(
                "[Scheduler] Task '{}' processed. next_run_at={:?}",
                task_name,
                task.state.next_run_at
            );
        }

        Ok(())
    }

    /// Dispatch a scheduled task based on its task_type.
    async fn dispatch_task_for_schedule(&self, task: &ScheduledTask) -> Result<String, String> {
        match &task.task_type {
            TaskExecutionType::Dispatch => self.dispatch_persona_task(task),
            TaskExecutionType::Script => self.execute_script_task(task).await,
        }
    }

    /// Dispatch via PersonaManager::dispatch_task().
    fn dispatch_persona_task(&self, task: &ScheduledTask) -> Result<String, String> {
        let persona_id = task.persona_id.as_deref().ok_or_else(|| {
            format!(
                "Task '{}' has no persona_id — cannot dispatch. \
                 Dispatch tasks must be associated with a persona.",
                task.name
            )
        })?;

        log::info!(
            "[Scheduler] Dispatching via persona '{}' for task '{}'",
            persona_id,
            task.name
        );
        let persona_mgr = crate::local_services::persona_manager()?;
        let session_id = persona_mgr.dispatch_task(
            persona_id,
            &task.task_prompt,
            None,
            None,
            None,
            None,
            None,
            None, // no orchestration label for scheduled tasks
            None,
        )?;
        Ok(session_id)
    }

    /// Execute task_prompt as a script via a Worker session (same pattern as experiment_run).
    /// The worker agent runs the script with full tool access and reports results.
    async fn execute_script_task(&self, task: &ScheduledTask) -> Result<String, String> {
        log::info!(
            "[Scheduler] Dispatching script worker for task '{}': {}",
            task.name,
            &task.task_prompt[..task.task_prompt.len().min(100)]
        );

        let spawn_config = crate::local_services::spawn_config()
            .map_err(|e| format!("SpawnConfig not available: {}", e))?;

        // Resolve workdir: from owner's context or default to home
        let directory = if let Some(owner_id) = task.persona_id.as_deref() {
            // Try persona workdir
            if let Ok(mgr) = crate::local_services::persona_manager() {
                if let Ok(Some(p)) = mgr.store().get_persona(owner_id) {
                    if !p.workdir.is_empty() {
                        p.workdir.clone()
                    } else {
                        "~".to_string()
                    }
                } else {
                    "~".to_string()
                }
            } else {
                "~".to_string()
            }
        } else {
            "~".to_string()
        };

        let profile_id = spawn_config.agent_config.profile_id.read().await.clone();

        let initial_message = format!(
            "你是脚本执行者。请执行以下脚本并报告结果。\n\n\
             ## 脚本\n```\n{}\n```\n\n\
             ## 执行步骤\n\
             1. 用 shell 工具运行脚本\n\
             2. 捕获输出（stdout 和 stderr）\n\
             3. 报告执行结果：成功/失败、输出内容、关键信息\n\n\
             严格执行脚本，不要执行额外操作。",
            task.task_prompt
        );

        let session_id = crate::happy_client::manager::spawn_session_internal(
            &spawn_config,
            &directory,
            "claude",
            &profile_id,
            Some(&initial_message),
            Some(crate::happy_client::permission::PermissionMode::BypassPermissions),
            None,
            None,
        )
        .await
        .map_err(|e| format!("Failed to spawn script worker session: {}", e))?;

        log::info!(
            "[Scheduler] Script worker dispatched for task '{}' -> session {}",
            task.name,
            session_id
        );
        Ok(format!("Worker session started: {}", session_id))
    }
}

// ── next-run computation ───────────────────────────────────────────────

/// Compute the next run time (Unix ms) for a given schedule.
/// Returns `None` when the schedule is exhausted (e.g. a past `At`).
pub fn compute_next_run(schedule: &ScheduleType, timezone: &str, now_ms: i64) -> Option<i64> {
    match schedule {
        ScheduleType::At { at } => {
            let target = parse_iso8601_to_ms(at)?;
            if target > now_ms {
                Some(target)
            } else {
                None // already past
            }
        }
        ScheduleType::Every {
            every_seconds,
            anchor,
        } => {
            let interval_ms = (*every_seconds as i64) * 1000;
            if interval_ms <= 0 {
                return None;
            }
            let anchor_ms = anchor
                .as_ref()
                .and_then(|a| parse_iso8601_to_ms(a))
                .unwrap_or(now_ms);

            if now_ms < anchor_ms {
                // Anchor is in the future — that is the first run.
                return Some(anchor_ms);
            }

            let elapsed = now_ms - anchor_ms;
            let periods = (elapsed / interval_ms) + 1;
            Some(anchor_ms + periods * interval_ms)
        }
        ScheduleType::Cron { expr } => {
            let cron = croner::Cron::new(expr).parse().ok()?;
            let tz: Tz = timezone.parse().ok()?;
            let now_utc = DateTime::from_timestamp_millis(now_ms)?;
            let now_tz = now_utc.with_timezone(&tz);
            let next = cron.find_next_occurrence(&now_tz, false).ok()?;
            Some(next.with_timezone(&Utc).timestamp_millis())
        }
    }
}

/// Parse an ISO-8601 string into Unix milliseconds.
fn parse_iso8601_to_ms(s: &str) -> Option<i64> {
    // Try full DateTime with timezone
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc).timestamp_millis());
    }
    // Try chrono's flexible parser
    if let Ok(dt) = chrono::DateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%:z") {
        return Some(dt.with_timezone(&Utc).timestamp_millis());
    }
    // Try as naive datetime in UTC
    if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return Some(ndt.and_utc().timestamp_millis());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_at_future() {
        let now = Utc::now().timestamp_millis();
        let future = Utc::now() + chrono::Duration::hours(1);
        let at_str = future.to_rfc3339();
        let schedule = ScheduleType::At { at: at_str };
        let next = compute_next_run(&schedule, "Asia/Shanghai", now);
        assert!(next.is_some());
        assert!(next.unwrap() > now);
    }

    #[test]
    fn test_at_past() {
        let now = Utc::now().timestamp_millis();
        let past = Utc::now() - chrono::Duration::hours(1);
        let at_str = past.to_rfc3339();
        let schedule = ScheduleType::At { at: at_str };
        let next = compute_next_run(&schedule, "Asia/Shanghai", now);
        assert!(next.is_none());
    }

    #[test]
    fn test_every_basic() {
        let now = 1000000i64;
        let schedule = ScheduleType::Every {
            every_seconds: 60,
            anchor: None,
        };
        let next = compute_next_run(&schedule, "UTC", now);
        assert!(next.is_some());
        // With anchor = now, next should be now + 60s
        assert_eq!(next.unwrap(), now + 60_000);
    }

    #[test]
    fn test_every_with_anchor() {
        let _anchor_ms = 0i64; // epoch
        let now = 150_000i64; // 150 seconds after epoch
        let schedule = ScheduleType::Every {
            every_seconds: 60,
            anchor: Some("1970-01-01T00:00:00Z".to_string()),
        };
        let next = compute_next_run(&schedule, "UTC", now);
        // periods = (150000 / 60000) + 1 = 3
        // next = 0 + 3 * 60000 = 180000
        assert_eq!(next.unwrap(), 180_000);
    }

    #[test]
    fn test_cron_basic() {
        let now = Utc::now().timestamp_millis();
        let schedule = ScheduleType::Cron {
            expr: "* * * * *".to_string(), // every minute
        };
        let next = compute_next_run(&schedule, "Asia/Shanghai", now);
        assert!(next.is_some());
        // Should be within 60 seconds
        assert!(next.unwrap() - now <= 60_000);
        assert!(next.unwrap() > now);
    }
}
