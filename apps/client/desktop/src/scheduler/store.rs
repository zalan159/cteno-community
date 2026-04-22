//! SQLite persistence for scheduled tasks.

use rusqlite::{params, Connection};
use std::path::PathBuf;

use super::models::{ScheduleType, ScheduledTask, TaskExecutionType, TaskState};

/// Persistent store backed by SQLite.
pub struct TaskStore {
    db_path: PathBuf,
}

impl TaskStore {
    pub fn new(db_path: PathBuf) -> Self {
        let store = Self { db_path };
        if let Err(e) = store.init_schema() {
            log::error!("[Scheduler] Failed to initialize schema: {}", e);
        }
        store
    }

    fn connect(&self) -> Result<Connection, String> {
        Connection::open(&self.db_path).map_err(|e| format!("Failed to open db: {}", e))
    }

    fn init_schema(&self) -> Result<(), String> {
        let conn = self.connect()?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS scheduled_tasks (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                task_prompt TEXT NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1,
                delete_after_run INTEGER NOT NULL DEFAULT 0,
                schedule_json TEXT NOT NULL,
                timezone TEXT NOT NULL DEFAULT 'Asia/Shanghai',
                session_id TEXT NOT NULL,
                state_json TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_scheduled_tasks_enabled
                ON scheduled_tasks(enabled);",
        )
        .map_err(|e| format!("Schema init failed: {}", e))?;

        // Migration: add persona_id column (nullable)
        Self::migrate_add_persona_id(&conn)?;
        // Migration: add task_type column (defaults to "dispatch")
        Self::migrate_add_task_type(&conn)?;

        Ok(())
    }

    fn migrate_add_persona_id(conn: &Connection) -> Result<(), String> {
        let has_column: bool = conn
            .prepare("PRAGMA table_info(scheduled_tasks)")
            .and_then(|mut stmt| {
                stmt.query_map([], |row| {
                    let name: String = row.get(1)?;
                    Ok(name)
                })
                .map(|rows| rows.filter_map(|r| r.ok()).any(|n| n == "persona_id"))
            })
            .unwrap_or(false);

        if !has_column {
            conn.execute_batch("ALTER TABLE scheduled_tasks ADD COLUMN persona_id TEXT;")
                .map_err(|e| format!("Migration (add persona_id) failed: {}", e))?;
            log::info!("[Scheduler] Migration: added persona_id column");
        }
        Ok(())
    }

    fn migrate_add_task_type(conn: &Connection) -> Result<(), String> {
        let has_column: bool = conn
            .prepare("PRAGMA table_info(scheduled_tasks)")
            .and_then(|mut stmt| {
                stmt.query_map([], |row| {
                    let name: String = row.get(1)?;
                    Ok(name)
                })
                .map(|rows| rows.filter_map(|r| r.ok()).any(|n| n == "task_type"))
            })
            .unwrap_or(false);

        if !has_column {
            conn.execute_batch(
                "ALTER TABLE scheduled_tasks ADD COLUMN task_type TEXT NOT NULL DEFAULT 'dispatch';",
            )
            .map_err(|e| format!("Migration (add task_type) failed: {}", e))?;
            log::info!("[Scheduler] Migration: added task_type column");
        }
        Ok(())
    }

    /// Insert a new task.
    pub fn create(&self, task: &ScheduledTask) -> Result<(), String> {
        let conn = self.connect()?;
        let schedule_json = serde_json::to_string(&task.schedule).map_err(|e| e.to_string())?;
        let state_json = serde_json::to_string(&task.state).map_err(|e| e.to_string())?;

        conn.execute(
            "INSERT INTO scheduled_tasks
                (id, name, task_prompt, enabled, delete_after_run,
                 schedule_json, timezone, session_id, state_json,
                 created_at, updated_at, persona_id, task_type)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                task.id,
                task.name,
                task.task_prompt,
                task.enabled as i32,
                task.delete_after_run as i32,
                schedule_json,
                task.timezone,
                task.session_id,
                state_json,
                task.created_at,
                task.updated_at,
                task.persona_id,
                task.task_type.as_str(),
            ],
        )
        .map_err(|e| format!("Insert failed: {}", e))?;
        Ok(())
    }

    /// Get a task by id.
    pub fn get(&self, id: &str) -> Result<Option<ScheduledTask>, String> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, name, task_prompt, enabled, delete_after_run,
                        schedule_json, timezone, session_id, state_json,
                        created_at, updated_at, persona_id, task_type
                 FROM scheduled_tasks WHERE id = ?1",
            )
            .map_err(|e| e.to_string())?;

        let mut rows = stmt
            .query_map(params![id], |row| Ok(row_to_task(row)))
            .map_err(|e| e.to_string())?;

        match rows.next() {
            Some(Ok(Ok(task))) => Ok(Some(task)),
            Some(Ok(Err(e))) => Err(e),
            Some(Err(e)) => Err(e.to_string()),
            None => Ok(None),
        }
    }

    /// List tasks. If `enabled_only` is true, only returns enabled tasks.
    pub fn list(&self, enabled_only: bool) -> Result<Vec<ScheduledTask>, String> {
        let conn = self.connect()?;
        let sql = if enabled_only {
            "SELECT id, name, task_prompt, enabled, delete_after_run,
                    schedule_json, timezone, session_id, state_json,
                    created_at, updated_at, persona_id, task_type
             FROM scheduled_tasks WHERE enabled = 1 ORDER BY created_at DESC"
        } else {
            "SELECT id, name, task_prompt, enabled, delete_after_run,
                    schedule_json, timezone, session_id, state_json,
                    created_at, updated_at, persona_id, task_type
             FROM scheduled_tasks ORDER BY created_at DESC"
        };

        let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| Ok(row_to_task(row)))
            .map_err(|e| e.to_string())?;

        let mut tasks = Vec::new();
        for row in rows {
            match row {
                Ok(Ok(task)) => tasks.push(task),
                Ok(Err(e)) => log::warn!("[Scheduler] Skipping malformed row: {}", e),
                Err(e) => log::warn!("[Scheduler] Row error: {}", e),
            }
        }
        Ok(tasks)
    }

    /// Update task state (next_run_at, running_since, last_run_at, etc.).
    pub fn update_state(&self, id: &str, state: &TaskState) -> Result<(), String> {
        let conn = self.connect()?;
        let state_json = serde_json::to_string(state).map_err(|e| e.to_string())?;
        let now = chrono::Utc::now().timestamp_millis();
        let changed = conn
            .execute(
                "UPDATE scheduled_tasks SET state_json = ?1, updated_at = ?2 WHERE id = ?3",
                params![state_json, now, id],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("Task {} not found", id));
        }
        Ok(())
    }

    /// Enable or disable a task.
    pub fn update_enabled(&self, id: &str, enabled: bool) -> Result<(), String> {
        let conn = self.connect()?;
        let now = chrono::Utc::now().timestamp_millis();
        let changed = conn
            .execute(
                "UPDATE scheduled_tasks SET enabled = ?1, updated_at = ?2 WHERE id = ?3",
                params![enabled as i32, now, id],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("Task {} not found", id));
        }
        Ok(())
    }

    /// Delete all tasks belonging to a session. Returns the number of deleted rows.
    pub fn delete_by_session(&self, session_id: &str) -> Result<usize, String> {
        let conn = self.connect()?;
        let changed = conn
            .execute(
                "DELETE FROM scheduled_tasks WHERE session_id = ?1",
                params![session_id],
            )
            .map_err(|e| format!("Delete by session failed: {}", e))?;
        Ok(changed)
    }

    /// Delete a task. Returns true if a row was actually removed.
    pub fn delete(&self, id: &str) -> Result<bool, String> {
        let conn = self.connect()?;
        let changed = conn
            .execute("DELETE FROM scheduled_tasks WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        Ok(changed > 0)
    }

    /// Return enabled tasks whose `next_run_at` <= `now_ms` and are not already running.
    pub fn get_due_tasks(&self, now_ms: i64) -> Result<Vec<ScheduledTask>, String> {
        let conn = self.connect()?;
        // We query all enabled tasks and filter in Rust because json_extract on
        // state_json varies across SQLite builds. The number of scheduled tasks
        // per user is small (< 100), so this is fine.
        let mut stmt = conn
            .prepare(
                "SELECT id, name, task_prompt, enabled, delete_after_run,
                        schedule_json, timezone, session_id, state_json,
                        created_at, updated_at, persona_id, task_type
                 FROM scheduled_tasks WHERE enabled = 1",
            )
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map([], |row| Ok(row_to_task(row)))
            .map_err(|e| e.to_string())?;

        let mut due = Vec::new();
        for row in rows {
            match row {
                Ok(Ok(task)) => {
                    if let Some(next) = task.state.next_run_at {
                        if next <= now_ms && task.state.running_since.is_none() {
                            due.push(task);
                        }
                    }
                }
                Ok(Err(e)) => log::warn!("[Scheduler] Skipping malformed row: {}", e),
                Err(e) => log::warn!("[Scheduler] Row error: {}", e),
            }
        }
        Ok(due)
    }

    /// Full update of a task (for PATCH endpoint).
    pub fn update_task(&self, task: &ScheduledTask) -> Result<(), String> {
        let conn = self.connect()?;
        let schedule_json = serde_json::to_string(&task.schedule).map_err(|e| e.to_string())?;
        let state_json = serde_json::to_string(&task.state).map_err(|e| e.to_string())?;
        let now = chrono::Utc::now().timestamp_millis();

        let changed = conn
            .execute(
                "UPDATE scheduled_tasks
                 SET name = ?1, task_prompt = ?2, enabled = ?3,
                     delete_after_run = ?4, schedule_json = ?5,
                     timezone = ?6, session_id = ?7, state_json = ?8,
                     updated_at = ?9, persona_id = ?10, task_type = ?11
                 WHERE id = ?12",
                params![
                    task.name,
                    task.task_prompt,
                    task.enabled as i32,
                    task.delete_after_run as i32,
                    schedule_json,
                    task.timezone,
                    task.session_id,
                    state_json,
                    now,
                    task.persona_id,
                    task.task_type.as_str(),
                    task.id,
                ],
            )
            .map_err(|e| e.to_string())?;

        if changed == 0 {
            return Err(format!("Task {} not found", task.id));
        }
        Ok(())
    }
}

/// Convert a rusqlite Row into a ScheduledTask (called inside query_map closure).
fn row_to_task(row: &rusqlite::Row) -> Result<ScheduledTask, String> {
    let id: String = row.get(0).map_err(|e| e.to_string())?;
    let name: String = row.get(1).map_err(|e| e.to_string())?;
    let task_prompt: String = row.get(2).map_err(|e| e.to_string())?;
    let enabled_int: i32 = row.get(3).map_err(|e| e.to_string())?;
    let delete_after_run_int: i32 = row.get(4).map_err(|e| e.to_string())?;
    let schedule_json: String = row.get(5).map_err(|e| e.to_string())?;
    let timezone: String = row.get(6).map_err(|e| e.to_string())?;
    let session_id: String = row.get(7).map_err(|e| e.to_string())?;
    let state_json: String = row.get(8).map_err(|e| e.to_string())?;
    let created_at: i64 = row.get(9).map_err(|e| e.to_string())?;
    let updated_at: i64 = row.get(10).map_err(|e| e.to_string())?;
    let persona_id: Option<String> = row.get(11).ok();
    let task_type_str: String = row
        .get::<_, String>(12)
        .unwrap_or_else(|_| "dispatch".to_string());
    let task_type = TaskExecutionType::from_str_lossy(&task_type_str);

    let schedule: ScheduleType =
        serde_json::from_str(&schedule_json).map_err(|e| format!("Bad schedule_json: {}", e))?;
    let state: TaskState =
        serde_json::from_str(&state_json).map_err(|e| format!("Bad state_json: {}", e))?;

    Ok(ScheduledTask {
        id,
        name,
        task_prompt,
        enabled: enabled_int != 0,
        delete_after_run: delete_after_run_int != 0,
        schedule,
        timezone,
        session_id,
        persona_id,
        task_type,
        state,
        created_at,
        updated_at,
    })
}
