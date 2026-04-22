//! SQLite persistence for notification subscriptions and watermarks.

use rusqlite::{params, Connection};
use std::path::PathBuf;

use super::models::NotificationSubscription;

/// Persistent store for notification subscriptions.
pub struct NotificationStore {
    db_path: PathBuf,
}

impl NotificationStore {
    pub fn new(db_path: PathBuf) -> Self {
        let store = Self { db_path };
        if let Err(e) = store.init_schema() {
            log::error!("[NotifWatcher] Failed to initialize schema: {}", e);
        }
        store
    }

    fn connect(&self) -> Result<Connection, String> {
        Connection::open(&self.db_path).map_err(|e| format!("Failed to open db: {}", e))
    }

    fn init_schema(&self) -> Result<(), String> {
        let conn = self.connect()?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS notification_subscriptions (
                id TEXT PRIMARY KEY,
                persona_id TEXT NOT NULL,
                app_identifier TEXT NOT NULL,
                app_display_name TEXT NOT NULL DEFAULT '',
                enabled INTEGER NOT NULL DEFAULT 1,
                created_at INTEGER NOT NULL,
                UNIQUE(persona_id, app_identifier)
            );

            CREATE TABLE IF NOT EXISTS notification_watermarks (
                app_identifier TEXT PRIMARY KEY,
                last_rec_id INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL
            );",
        )
        .map_err(|e| format!("Schema init failed: {}", e))?;
        Ok(())
    }

    // ── Subscription CRUD ─────────────────────────────────────────────

    pub fn create_subscription(&self, sub: &NotificationSubscription) -> Result<(), String> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT OR REPLACE INTO notification_subscriptions
                (id, persona_id, app_identifier, app_display_name, enabled, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                sub.id,
                sub.persona_id,
                sub.app_identifier,
                sub.app_display_name,
                sub.enabled as i32,
                sub.created_at,
            ],
        )
        .map_err(|e| format!("Insert subscription failed: {}", e))?;
        Ok(())
    }

    pub fn delete_subscription(&self, id: &str) -> Result<bool, String> {
        let conn = self.connect()?;
        let changed = conn
            .execute(
                "DELETE FROM notification_subscriptions WHERE id = ?1",
                params![id],
            )
            .map_err(|e| e.to_string())?;
        Ok(changed > 0)
    }

    pub fn delete_subscription_by_persona_app(
        &self,
        persona_id: &str,
        app_identifier: &str,
    ) -> Result<bool, String> {
        let conn = self.connect()?;
        let changed = conn
            .execute(
                "DELETE FROM notification_subscriptions WHERE persona_id = ?1 AND app_identifier = ?2",
                params![persona_id, app_identifier],
            )
            .map_err(|e| e.to_string())?;
        Ok(changed > 0)
    }

    pub fn update_enabled(&self, id: &str, enabled: bool) -> Result<(), String> {
        let conn = self.connect()?;
        let changed = conn
            .execute(
                "UPDATE notification_subscriptions SET enabled = ?1 WHERE id = ?2",
                params![enabled as i32, id],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("Subscription {} not found", id));
        }
        Ok(())
    }

    pub fn list_by_persona(
        &self,
        persona_id: &str,
    ) -> Result<Vec<NotificationSubscription>, String> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, persona_id, app_identifier, app_display_name, enabled, created_at
                 FROM notification_subscriptions
                 WHERE persona_id = ?1
                 ORDER BY created_at DESC",
            )
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map(params![persona_id], |row| {
                Ok(NotificationSubscription {
                    id: row.get(0)?,
                    persona_id: row.get(1)?,
                    app_identifier: row.get(2)?,
                    app_display_name: row.get(3)?,
                    enabled: row.get::<_, i32>(4)? != 0,
                    created_at: row.get(5)?,
                })
            })
            .map_err(|e| e.to_string())?;

        let mut subs = Vec::new();
        for row in rows {
            subs.push(row.map_err(|e| e.to_string())?);
        }
        Ok(subs)
    }

    /// Get all distinct app_identifiers that have at least one enabled subscription.
    pub fn active_app_identifiers(&self) -> Result<Vec<String>, String> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT app_identifier
                 FROM notification_subscriptions
                 WHERE enabled = 1",
            )
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?;

        let mut identifiers = Vec::new();
        for row in rows {
            identifiers.push(row.map_err(|e| e.to_string())?);
        }
        Ok(identifiers)
    }

    /// Get all enabled subscriptions for a given app identifier.
    pub fn subscriptions_for_app(
        &self,
        app_identifier: &str,
    ) -> Result<Vec<NotificationSubscription>, String> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, persona_id, app_identifier, app_display_name, enabled, created_at
                 FROM notification_subscriptions
                 WHERE app_identifier = ?1 AND enabled = 1",
            )
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map(params![app_identifier], |row| {
                Ok(NotificationSubscription {
                    id: row.get(0)?,
                    persona_id: row.get(1)?,
                    app_identifier: row.get(2)?,
                    app_display_name: row.get(3)?,
                    enabled: row.get::<_, i32>(4)? != 0,
                    created_at: row.get(5)?,
                })
            })
            .map_err(|e| e.to_string())?;

        let mut subs = Vec::new();
        for row in rows {
            subs.push(row.map_err(|e| e.to_string())?);
        }
        Ok(subs)
    }

    /// Delete all subscriptions for a persona (used when persona is deleted).
    pub fn delete_by_persona(&self, persona_id: &str) -> Result<usize, String> {
        let conn = self.connect()?;
        let changed = conn
            .execute(
                "DELETE FROM notification_subscriptions WHERE persona_id = ?1",
                params![persona_id],
            )
            .map_err(|e| e.to_string())?;
        Ok(changed)
    }

    // ── Watermark ─────────────────────────────────────────────────────

    pub fn get_watermark(&self, app_identifier: &str) -> Result<i64, String> {
        let conn = self.connect()?;
        let result = conn.query_row(
            "SELECT last_rec_id FROM notification_watermarks WHERE app_identifier = ?1",
            params![app_identifier],
            |row| row.get::<_, i64>(0),
        );
        match result {
            Ok(id) => Ok(id),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(0),
            Err(e) => Err(format!("Failed to get watermark: {}", e)),
        }
    }

    pub fn set_watermark(&self, app_identifier: &str, last_rec_id: i64) -> Result<(), String> {
        let conn = self.connect()?;
        let now = chrono::Utc::now().timestamp_millis();
        conn.execute(
            "INSERT OR REPLACE INTO notification_watermarks (app_identifier, last_rec_id, updated_at)
             VALUES (?1, ?2, ?3)",
            params![app_identifier, last_rec_id, now],
        )
        .map_err(|e| format!("Failed to set watermark: {}", e))?;
        Ok(())
    }
}
