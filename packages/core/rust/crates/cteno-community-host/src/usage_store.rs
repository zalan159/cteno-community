//! Local usage tracking — SQLite persistence for token consumption.
//!
//! Records per-request usage (profile, model, tokens) and provides
//! aggregated summaries for the frontend usage dashboard.

use rusqlite::{params, Connection};
use serde::Serialize;
use std::path::PathBuf;

/// Persistent store for local usage records.
pub struct UsageStore {
    db_path: PathBuf,
}

#[derive(Debug, Clone, Copy)]
pub struct UsageRecord {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_creation_input_tokens: u32,
    pub cache_read_input_tokens: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageSummary {
    pub total_input: u64,
    pub total_output: u64,
    pub total_cache_read: u64,
    pub total_cache_creation: u64,
    pub by_profile: Vec<ProfileUsage>,
    pub by_day: Vec<DayUsage>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProfileUsage {
    pub profile_id: String,
    pub total_tokens: u64,
    pub models: Vec<ModelUsage>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelUsage {
    pub model: String,
    pub input: u64,
    pub output: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DayUsage {
    pub date: String,
    pub input: u64,
    pub output: u64,
}

impl UsageStore {
    pub fn new(db_path: PathBuf) -> Self {
        let store = Self { db_path };
        if let Err(e) = store.init_schema() {
            log::error!("[UsageStore] Failed to initialize schema: {}", e);
        }
        store
    }

    fn connect(&self) -> Result<Connection, String> {
        Connection::open(&self.db_path).map_err(|e| format!("Failed to open db: {}", e))
    }

    fn init_schema(&self) -> Result<(), String> {
        let conn = self.connect()?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS usage_records (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                profile_id TEXT NOT NULL,
                model TEXT NOT NULL,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                session_id TEXT,
                created_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_usage_profile ON usage_records(profile_id);
            CREATE INDEX IF NOT EXISTS idx_usage_created ON usage_records(created_at);",
        )
        .map_err(|e| format!("Schema init failed: {}", e))?;
        Ok(())
    }

    /// Insert a single usage record.
    pub fn record_usage(
        &self,
        profile_id: &str,
        model: &str,
        usage: &UsageRecord,
        session_id: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.connect()?;
        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "INSERT INTO usage_records (profile_id, model, input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens, session_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                profile_id,
                model,
                usage.input_tokens as i64,
                usage.output_tokens as i64,
                usage.cache_creation_input_tokens as i64,
                usage.cache_read_input_tokens as i64,
                session_id,
                now,
            ],
        )
        .map_err(|e| format!("Failed to insert usage record: {}", e))?;
        Ok(())
    }

    /// Aggregate usage between `start_time` and `end_time` (unix seconds).
    /// When `exclude_proxy` is true, records with profile_id starting with "proxy-" are excluded.
    pub fn query_summary(&self, start_time: i64, end_time: i64) -> Result<UsageSummary, String> {
        self.query_summary_filtered(start_time, end_time, false)
    }

    pub fn query_summary_local_only(
        &self,
        start_time: i64,
        end_time: i64,
    ) -> Result<UsageSummary, String> {
        self.query_summary_filtered(start_time, end_time, true)
    }

    fn query_summary_filtered(
        &self,
        start_time: i64,
        end_time: i64,
        exclude_proxy: bool,
    ) -> Result<UsageSummary, String> {
        let conn = self.connect()?;
        let proxy_filter = if exclude_proxy {
            " AND profile_id NOT LIKE 'proxy-%'"
        } else {
            ""
        };

        let totals_sql = format!(
            "SELECT COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                    COALESCE(SUM(cache_read_tokens),0), COALESCE(SUM(cache_creation_tokens),0)
             FROM usage_records WHERE created_at >= ?1 AND created_at < ?2{}",
            proxy_filter
        );
        let (total_input, total_output, total_cache_read, total_cache_creation) = conn
            .query_row(&totals_sql, params![start_time, end_time], |row| {
                Ok((
                    row.get::<_, i64>(0)? as u64,
                    row.get::<_, i64>(1)? as u64,
                    row.get::<_, i64>(2)? as u64,
                    row.get::<_, i64>(3)? as u64,
                ))
            })
            .map_err(|e| format!("Failed to query totals: {}", e))?;

        let profile_sql = format!(
            "SELECT profile_id, model, SUM(input_tokens), SUM(output_tokens)
             FROM usage_records
             WHERE created_at >= ?1 AND created_at < ?2{}
             GROUP BY profile_id, model
             ORDER BY profile_id, SUM(input_tokens + output_tokens) DESC",
            proxy_filter
        );
        let mut stmt = conn
            .prepare(&profile_sql)
            .map_err(|e| format!("Failed to prepare profile query: {}", e))?;

        let rows: Vec<(String, String, u64, u64)> = stmt
            .query_map(params![start_time, end_time], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)? as u64,
                    row.get::<_, i64>(3)? as u64,
                ))
            })
            .map_err(|e| format!("Failed to query by profile: {}", e))?
            .filter_map(|r| r.ok())
            .collect();

        let mut profile_map: std::collections::BTreeMap<String, Vec<ModelUsage>> =
            std::collections::BTreeMap::new();
        for (pid, model, input, output) in &rows {
            profile_map
                .entry(pid.clone())
                .or_default()
                .push(ModelUsage {
                    model: model.clone(),
                    input: *input,
                    output: *output,
                });
        }
        let by_profile: Vec<ProfileUsage> = profile_map
            .into_iter()
            .map(|(profile_id, models)| {
                let total_tokens: u64 = models.iter().map(|m| m.input + m.output).sum();
                ProfileUsage {
                    profile_id,
                    total_tokens,
                    models,
                }
            })
            .collect();

        let day_sql = format!(
            "SELECT date(created_at, 'unixepoch') as day, SUM(input_tokens), SUM(output_tokens)
             FROM usage_records
             WHERE created_at >= ?1 AND created_at < ?2{}
             GROUP BY day
             ORDER BY day",
            proxy_filter
        );
        let mut day_stmt = conn
            .prepare(&day_sql)
            .map_err(|e| format!("Failed to prepare day query: {}", e))?;

        let by_day: Vec<DayUsage> = day_stmt
            .query_map(params![start_time, end_time], |row| {
                Ok(DayUsage {
                    date: row.get::<_, String>(0)?,
                    input: row.get::<_, i64>(1)? as u64,
                    output: row.get::<_, i64>(2)? as u64,
                })
            })
            .map_err(|e| format!("Failed to query by day: {}", e))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(UsageSummary {
            total_input,
            total_output,
            total_cache_read,
            total_cache_creation,
            by_profile,
            by_day,
        })
    }
}
