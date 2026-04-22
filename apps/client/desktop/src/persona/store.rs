//! SQLite persistence for personas and persona-session links.

use rusqlite::{params, Connection};
use std::path::PathBuf;

use super::models::{Persona, PersonaSessionLink, PersonaSessionType, WorkspaceBinding};

/// Persistent store for personas backed by SQLite.
pub struct PersonaStore {
    db_path: PathBuf,
}

impl PersonaStore {
    pub fn new(db_path: PathBuf) -> Self {
        let store = Self { db_path };
        if let Err(e) = store.init_schema() {
            log::error!("[Persona] Failed to initialize schema: {}", e);
        }
        store
    }

    fn connect(&self) -> Result<Connection, String> {
        Connection::open(&self.db_path).map_err(|e| format!("Failed to open db: {}", e))
    }

    fn init_schema(&self) -> Result<(), String> {
        let conn = self.connect()?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS personas (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                avatar_id TEXT NOT NULL DEFAULT 'default',
                description TEXT NOT NULL DEFAULT '',
                personality_notes TEXT NOT NULL DEFAULT '',
                model TEXT NOT NULL DEFAULT 'deepseek-chat',
                profile_id TEXT,
                agent TEXT DEFAULT 'cteno',
                workdir TEXT NOT NULL DEFAULT '~',
                chat_session_id TEXT NOT NULL,
                is_default INTEGER DEFAULT 0,
                created_at TEXT DEFAULT (datetime('now')),
                updated_at TEXT DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS persona_sessions (
                persona_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                session_type TEXT NOT NULL CHECK(session_type IN ('chat', 'task', 'member')),
                task_description TEXT,
                created_at TEXT DEFAULT (datetime('now')),
                PRIMARY KEY (persona_id, session_id)
            );
            CREATE INDEX IF NOT EXISTS idx_ps_persona ON persona_sessions(persona_id);
            CREATE INDEX IF NOT EXISTS idx_ps_session ON persona_sessions(session_id);

            CREATE TABLE IF NOT EXISTS persona_workspaces (
                persona_id TEXT PRIMARY KEY,
                workspace_id TEXT NOT NULL UNIQUE,
                template_id TEXT NOT NULL,
                provider TEXT NOT NULL,
                default_role_id TEXT,
                model TEXT NOT NULL,
                workdir TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_pw_workspace_id ON persona_workspaces(workspace_id);",
        )
        .map_err(|e| format!("Schema init failed: {}", e))?;

        // Migration: add workdir column if missing
        self.migrate_add_workdir(&conn)?;
        // Migration: add continuous_browsing column if missing
        self.migrate_add_continuous_browsing(&conn)?;
        // Migration: add persona agent/vendor column if missing
        self.migrate_add_persona_agent(&conn)?;
        // Migration: add agent_type column to persona_sessions if missing
        self.migrate_add_agent_type(&conn)?;
        // Migration: add owner_kind column to persona_sessions if missing
        self.migrate_add_owner_kind(&conn)?;
        // Migration: add label column to persona_sessions if missing
        self.migrate_add_label(&conn)?;
        // Migration: expand persona_sessions.session_type check to include member
        self.migrate_allow_member_session_type(&conn)?;

        Ok(())
    }

    fn migrate_add_workdir(&self, conn: &Connection) -> Result<(), String> {
        let has_col: bool = conn
            .prepare("PRAGMA table_info(personas)")
            .map_err(|e| e.to_string())?
            .query_map([], |row| {
                let name: String = row.get(1)?;
                Ok(name)
            })
            .map_err(|e| e.to_string())?
            .any(|r| r.as_deref() == Ok("workdir"));

        if !has_col {
            conn.execute(
                "ALTER TABLE personas ADD COLUMN workdir TEXT NOT NULL DEFAULT '~'",
                [],
            )
            .map_err(|e| format!("Migration add workdir failed: {}", e))?;
            log::info!("[Persona] Migrated: added workdir column");
        }
        Ok(())
    }

    fn migrate_add_continuous_browsing(&self, conn: &Connection) -> Result<(), String> {
        let has_col: bool = conn
            .prepare("PRAGMA table_info(personas)")
            .map_err(|e| e.to_string())?
            .query_map([], |row| {
                let name: String = row.get(1)?;
                Ok(name)
            })
            .map_err(|e| e.to_string())?
            .any(|r| r.as_deref() == Ok("continuous_browsing"));

        if !has_col {
            conn.execute(
                "ALTER TABLE personas ADD COLUMN continuous_browsing INTEGER DEFAULT 0",
                [],
            )
            .map_err(|e| format!("Migration add continuous_browsing failed: {}", e))?;
            log::info!("[Persona] Migrated: added continuous_browsing column");
        }
        Ok(())
    }

    fn migrate_add_persona_agent(&self, conn: &Connection) -> Result<(), String> {
        let has_col: bool = conn
            .prepare("PRAGMA table_info(personas)")
            .map_err(|e| e.to_string())?
            .query_map([], |row| {
                let name: String = row.get(1)?;
                Ok(name)
            })
            .map_err(|e| e.to_string())?
            .any(|r| r.as_deref() == Ok("agent"));

        if !has_col {
            conn.execute(
                "ALTER TABLE personas ADD COLUMN agent TEXT DEFAULT 'cteno'",
                [],
            )
            .map_err(|e| format!("Migration add agent failed: {}", e))?;
            log::info!("[Persona] Migrated: added agent column");
        }
        Ok(())
    }

    fn migrate_add_agent_type(&self, conn: &Connection) -> Result<(), String> {
        let has_col: bool = conn
            .prepare("PRAGMA table_info(persona_sessions)")
            .map_err(|e| e.to_string())?
            .query_map([], |row| {
                let name: String = row.get(1)?;
                Ok(name)
            })
            .map_err(|e| e.to_string())?
            .any(|r| r.as_deref() == Ok("agent_type"));

        if !has_col {
            conn.execute(
                "ALTER TABLE persona_sessions ADD COLUMN agent_type TEXT DEFAULT NULL",
                [],
            )
            .map_err(|e| format!("Migration add agent_type failed: {}", e))?;
            log::info!("[Persona] Migrated: added agent_type column to persona_sessions");
        }
        Ok(())
    }

    fn migrate_add_owner_kind(&self, conn: &Connection) -> Result<(), String> {
        let has_col: bool = conn
            .prepare("PRAGMA table_info(persona_sessions)")
            .map_err(|e| e.to_string())?
            .query_map([], |row| {
                let name: String = row.get(1)?;
                Ok(name)
            })
            .map_err(|e| e.to_string())?
            .any(|r| r.as_deref() == Ok("owner_kind"));

        if !has_col {
            conn.execute(
                "ALTER TABLE persona_sessions ADD COLUMN owner_kind TEXT DEFAULT 'persona'",
                [],
            )
            .map_err(|e| format!("Migration add owner_kind failed: {}", e))?;
            // Back-fill: persona_id starting with 'hagent-' → hypothesis
            conn.execute(
                "UPDATE persona_sessions SET owner_kind = 'hypothesis' WHERE persona_id LIKE 'hagent-%'",
                [],
            )
            .map_err(|e| format!("Migration backfill owner_kind failed: {}", e))?;
            log::info!("[Persona] Migrated: added owner_kind column to persona_sessions");
        }
        Ok(())
    }

    fn migrate_add_label(&self, conn: &Connection) -> Result<(), String> {
        let has_col: bool = conn
            .prepare("PRAGMA table_info(persona_sessions)")
            .map_err(|e| e.to_string())?
            .query_map([], |row| {
                let name: String = row.get(1)?;
                Ok(name)
            })
            .map_err(|e| e.to_string())?
            .any(|r| r.as_deref() == Ok("label"));

        if !has_col {
            conn.execute(
                "ALTER TABLE persona_sessions ADD COLUMN label TEXT DEFAULT NULL",
                [],
            )
            .map_err(|e| format!("Migration add label failed: {}", e))?;
            log::info!("[Persona] Migrated: added label column to persona_sessions");
        }
        Ok(())
    }

    fn migrate_allow_member_session_type(&self, conn: &Connection) -> Result<(), String> {
        let create_sql: Option<String> = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'persona_sessions'",
                [],
                |row| row.get(0),
            )
            .ok();

        if create_sql
            .as_deref()
            .map(|sql| sql.contains("'member'"))
            .unwrap_or(false)
        {
            return Ok(());
        }

        conn.execute_batch(
            "BEGIN;
             ALTER TABLE persona_sessions RENAME TO persona_sessions_old;
             CREATE TABLE persona_sessions (
                 persona_id TEXT NOT NULL,
                 session_id TEXT NOT NULL,
                 session_type TEXT NOT NULL CHECK(session_type IN ('chat', 'task', 'member')),
                 task_description TEXT,
                 created_at TEXT DEFAULT (datetime('now')),
                 agent_type TEXT DEFAULT NULL,
                 owner_kind TEXT DEFAULT 'persona',
                 label TEXT DEFAULT NULL,
                 PRIMARY KEY (persona_id, session_id)
             );
             INSERT INTO persona_sessions (
                 persona_id, session_id, session_type, task_description, created_at, agent_type, owner_kind, label
             )
             SELECT
                 persona_id,
                 session_id,
                 session_type,
                 task_description,
                 created_at,
                 agent_type,
                 COALESCE(owner_kind, 'persona'),
                 label
             FROM persona_sessions_old;
             DROP TABLE persona_sessions_old;
             CREATE INDEX IF NOT EXISTS idx_ps_persona ON persona_sessions(persona_id);
             CREATE INDEX IF NOT EXISTS idx_ps_session ON persona_sessions(session_id);
             COMMIT;",
        )
        .map_err(|e| format!("Migration allow member session type failed: {}", e))?;

        log::info!("[Persona] Migrated: persona_sessions now supports member session type");
        Ok(())
    }

    // ========================================================================
    // Persona CRUD
    // ========================================================================

    pub fn create_persona(&self, persona: &Persona) -> Result<(), String> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO personas
                (id, name, avatar_id, description, personality_notes, model, profile_id,
                 agent, workdir, chat_session_id, is_default, continuous_browsing, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                persona.id,
                persona.name,
                persona.avatar_id,
                persona.description,
                persona.personality_notes,
                persona.model,
                persona.profile_id,
                persona.agent,
                persona.workdir,
                persona.chat_session_id,
                persona.is_default as i32,
                persona.continuous_browsing as i32,
                persona.created_at,
                persona.updated_at,
            ],
        )
        .map_err(|e| format!("Insert persona failed: {}", e))?;
        Ok(())
    }

    pub fn get_persona(&self, id: &str) -> Result<Option<Persona>, String> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, name, avatar_id, description, personality_notes, model,
                        profile_id, agent, workdir, chat_session_id, is_default, continuous_browsing,
                        created_at, updated_at
                 FROM personas WHERE id = ?1",
            )
            .map_err(|e| e.to_string())?;

        let mut rows = stmt
            .query_map(params![id], |row| Ok(row_to_persona(row)))
            .map_err(|e| e.to_string())?;

        match rows.next() {
            Some(Ok(Ok(p))) => Ok(Some(p)),
            Some(Ok(Err(e))) => Err(e),
            Some(Err(e)) => Err(e.to_string()),
            None => Ok(None),
        }
    }

    pub fn list_personas(&self) -> Result<Vec<Persona>, String> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, name, avatar_id, description, personality_notes, model,
                        profile_id, agent, workdir, chat_session_id, is_default, continuous_browsing,
                        created_at, updated_at
                 FROM personas ORDER BY is_default DESC, created_at ASC",
            )
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map([], |row| Ok(row_to_persona(row)))
            .map_err(|e| e.to_string())?;

        let mut personas = Vec::new();
        for row in rows {
            match row {
                Ok(Ok(p)) => personas.push(p),
                Ok(Err(e)) => log::warn!("[Persona] Skipping malformed row: {}", e),
                Err(e) => log::warn!("[Persona] Row error: {}", e),
            }
        }
        Ok(personas)
    }

    pub fn update_persona(&self, persona: &Persona) -> Result<(), String> {
        let conn = self.connect()?;
        let changed = conn
            .execute(
                "UPDATE personas
                 SET name = ?1, avatar_id = ?2, description = ?3, personality_notes = ?4,
                     model = ?5, profile_id = ?6, agent = ?7, workdir = ?8, is_default = ?9,
                     continuous_browsing = ?10, updated_at = datetime('now')
                 WHERE id = ?11",
                params![
                    persona.name,
                    persona.avatar_id,
                    persona.description,
                    persona.personality_notes,
                    persona.model,
                    persona.profile_id,
                    persona.agent,
                    persona.workdir,
                    persona.is_default as i32,
                    persona.continuous_browsing as i32,
                    persona.id,
                ],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("Persona {} not found", persona.id));
        }
        Ok(())
    }

    /// Update only the persona name.
    pub fn update_name(&self, persona_id: &str, name: &str) -> Result<(), String> {
        let conn = self.connect()?;
        let changed = conn
            .execute(
                "UPDATE personas SET name = ?1, updated_at = datetime('now') WHERE id = ?2",
                params![name, persona_id],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("Persona {} not found", persona_id));
        }
        Ok(())
    }

    /// Update only the personality_notes field.
    pub fn update_personality_notes(&self, id: &str, notes: &str) -> Result<(), String> {
        let conn = self.connect()?;
        let changed = conn
            .execute(
                "UPDATE personas SET personality_notes = ?1, updated_at = datetime('now') WHERE id = ?2",
                params![notes, id],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("Persona {} not found", id));
        }
        Ok(())
    }

    /// Update the chat_session_id after Happy Server assigns the real session ID.
    pub fn update_chat_session_id(
        &self,
        persona_id: &str,
        new_session_id: &str,
    ) -> Result<(), String> {
        let conn = self.connect()?;
        // Update persona table
        let changed = conn
            .execute(
                "UPDATE personas SET chat_session_id = ?1, updated_at = datetime('now') WHERE id = ?2",
                params![new_session_id, persona_id],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("Persona {} not found", persona_id));
        }
        // Also update the persona_sessions link if it exists
        conn.execute(
            "UPDATE persona_sessions SET session_id = ?1 WHERE persona_id = ?2 AND session_type = 'chat'",
            params![new_session_id, persona_id],
        )
        .map_err(|e| format!("Failed to update session link: {}", e))?;
        Ok(())
    }

    pub fn delete_persona(&self, id: &str) -> Result<bool, String> {
        let conn = self.connect()?;
        // Also clean up session links
        conn.execute(
            "DELETE FROM persona_sessions WHERE persona_id = ?1",
            params![id],
        )
        .map_err(|e| format!("Failed to delete session links: {}", e))?;
        conn.execute(
            "DELETE FROM persona_workspaces WHERE persona_id = ?1",
            params![id],
        )
        .map_err(|e| format!("Failed to delete workspace binding: {}", e))?;

        let changed = conn
            .execute("DELETE FROM personas WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        Ok(changed > 0)
    }

    // ========================================================================
    // Persona-Session Links
    // ========================================================================

    pub fn link_session(&self, link: &PersonaSessionLink) -> Result<(), String> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT OR REPLACE INTO persona_sessions
                (persona_id, session_id, session_type, task_description, agent_type, owner_kind, label, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                link.persona_id,
                link.session_id,
                link.session_type.as_str(),
                link.task_description,
                link.agent_type,
                link.owner_kind,
                link.label,
                link.created_at,
            ],
        )
        .map_err(|e| format!("Link session failed: {}", e))?;
        Ok(())
    }

    pub fn unlink_session(&self, persona_id: &str, session_id: &str) -> Result<bool, String> {
        let conn = self.connect()?;
        let changed = conn
            .execute(
                "DELETE FROM persona_sessions WHERE persona_id = ?1 AND session_id = ?2",
                params![persona_id, session_id],
            )
            .map_err(|e| e.to_string())?;
        Ok(changed > 0)
    }

    /// Remove a session link by session_id only (when session is killed externally).
    pub fn unlink_by_session_id(&self, session_id: &str) -> Result<bool, String> {
        let conn = self.connect()?;
        let changed = conn
            .execute(
                "DELETE FROM persona_sessions WHERE session_id = ?1 AND session_type = 'task'",
                params![session_id],
            )
            .map_err(|e| e.to_string())?;
        Ok(changed > 0)
    }

    /// Find which persona owns a given session (if any).
    pub fn get_persona_for_session(
        &self,
        session_id: &str,
    ) -> Result<Option<PersonaSessionLink>, String> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT persona_id, session_id, session_type, task_description, agent_type, created_at, owner_kind, label
                 FROM persona_sessions WHERE session_id = ?1",
            )
            .map_err(|e| e.to_string())?;

        let mut rows = stmt
            .query_map(params![session_id], |row| Ok(row_to_link(row)))
            .map_err(|e| e.to_string())?;

        match rows.next() {
            Some(Ok(Ok(link))) => Ok(Some(link)),
            Some(Ok(Err(e))) => Err(e),
            Some(Err(e)) => Err(e.to_string()),
            None => Ok(None),
        }
    }

    /// List all task sessions for a given persona.
    pub fn list_task_sessions(&self, persona_id: &str) -> Result<Vec<PersonaSessionLink>, String> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT persona_id, session_id, session_type, task_description, agent_type, created_at, owner_kind, label
                 FROM persona_sessions
                 WHERE persona_id = ?1 AND session_type = 'task'
                 ORDER BY created_at DESC",
            )
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map(params![persona_id], |row| Ok(row_to_link(row)))
            .map_err(|e| e.to_string())?;

        let mut links = Vec::new();
        for row in rows {
            match row {
                Ok(Ok(link)) => links.push(link),
                Ok(Err(e)) => log::warn!("[Persona] Skipping malformed link: {}", e),
                Err(e) => log::warn!("[Persona] Link row error: {}", e),
            }
        }
        Ok(links)
    }

    /// List all persistent member sessions for a given persona.
    pub fn list_member_sessions(
        &self,
        persona_id: &str,
    ) -> Result<Vec<PersonaSessionLink>, String> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT persona_id, session_id, session_type, task_description, agent_type, created_at, owner_kind, label
                 FROM persona_sessions
                 WHERE persona_id = ?1 AND session_type = 'member'
                 ORDER BY created_at ASC",
            )
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map(params![persona_id], |row| Ok(row_to_link(row)))
            .map_err(|e| e.to_string())?;

        let mut links = Vec::new();
        for row in rows {
            match row {
                Ok(Ok(link)) => links.push(link),
                Ok(Err(e)) => log::warn!("[Persona] Skipping malformed member link: {}", e),
                Err(e) => log::warn!("[Persona] Member link row error: {}", e),
            }
        }
        Ok(links)
    }

    pub fn upsert_workspace_binding(&self, binding: &WorkspaceBinding) -> Result<(), String> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO persona_workspaces
                (persona_id, workspace_id, template_id, provider, default_role_id, model, workdir, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(persona_id) DO UPDATE SET
                workspace_id = excluded.workspace_id,
                template_id = excluded.template_id,
                provider = excluded.provider,
                default_role_id = excluded.default_role_id,
                model = excluded.model,
                workdir = excluded.workdir,
                updated_at = excluded.updated_at",
            params![
                binding.persona_id,
                binding.workspace_id,
                binding.template_id,
                binding.provider,
                binding.default_role_id,
                binding.model,
                binding.workdir,
                binding.created_at,
                binding.updated_at,
            ],
        )
        .map_err(|e| format!("Upsert workspace binding failed: {}", e))?;
        Ok(())
    }

    pub fn get_workspace_binding(
        &self,
        persona_id: &str,
    ) -> Result<Option<WorkspaceBinding>, String> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT persona_id, workspace_id, template_id, provider, default_role_id, model, workdir, created_at, updated_at
                 FROM persona_workspaces
                 WHERE persona_id = ?1",
            )
            .map_err(|e| e.to_string())?;

        let mut rows = stmt
            .query_map(params![persona_id], |row| Ok(row_to_workspace_binding(row)))
            .map_err(|e| e.to_string())?;

        match rows.next() {
            Some(Ok(Ok(binding))) => Ok(Some(binding)),
            Some(Ok(Err(e))) => Err(e),
            Some(Err(e)) => Err(e.to_string()),
            None => Ok(None),
        }
    }

    pub fn list_workspace_bindings(&self) -> Result<Vec<WorkspaceBinding>, String> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT persona_id, workspace_id, template_id, provider, default_role_id, model, workdir, created_at, updated_at
                 FROM persona_workspaces
                 ORDER BY created_at DESC",
            )
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map([], |row| Ok(row_to_workspace_binding(row)))
            .map_err(|e| e.to_string())?;

        let mut bindings = Vec::new();
        for row in rows {
            match row {
                Ok(Ok(binding)) => bindings.push(binding),
                Ok(Err(e)) => log::warn!("[Persona] Skipping malformed workspace binding: {}", e),
                Err(e) => log::warn!("[Persona] Workspace binding row error: {}", e),
            }
        }
        Ok(bindings)
    }

    pub fn delete_workspace_binding(&self, persona_id: &str) -> Result<bool, String> {
        let conn = self.connect()?;
        let changed = conn
            .execute(
                "DELETE FROM persona_workspaces WHERE persona_id = ?1",
                params![persona_id],
            )
            .map_err(|e| format!("Delete workspace binding failed: {}", e))?;
        Ok(changed > 0)
    }

    /// Check if a session belongs to a persona with continuous_browsing enabled.
    /// Returns true only for chat-type sessions of personas with the flag on.
    pub fn is_continuous_browsing_session(&self, session_id: &str) -> Result<bool, String> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT p.continuous_browsing
                 FROM personas p
                 JOIN persona_sessions ps ON p.id = ps.persona_id
                 WHERE ps.session_id = ?1 AND ps.session_type = 'chat'",
            )
            .map_err(|e| e.to_string())?;

        let result: Option<i32> = stmt.query_row(params![session_id], |row| row.get(0)).ok();

        Ok(result.unwrap_or(0) != 0)
    }

    /// Find a persona by its chat_session_id.
    pub fn find_persona_by_chat_session_id(
        &self,
        session_id: &str,
    ) -> Result<Option<Persona>, String> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, name, avatar_id, description, personality_notes, model,
                        profile_id, agent, workdir, chat_session_id, is_default, continuous_browsing,
                        created_at, updated_at
                 FROM personas WHERE chat_session_id = ?1",
            )
            .map_err(|e| e.to_string())?;

        let mut rows = stmt
            .query_map(params![session_id], |row| Ok(row_to_persona(row)))
            .map_err(|e| e.to_string())?;

        match rows.next() {
            Some(Ok(Ok(p))) => Ok(Some(p)),
            Some(Ok(Err(e))) => Err(e),
            Some(Err(e)) => Err(e.to_string()),
            None => Ok(None),
        }
    }

    /// List all chat session IDs for persona filtering.
    pub fn list_chat_session_ids(&self) -> Result<Vec<String>, String> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare("SELECT session_id FROM persona_sessions WHERE session_type = 'chat'")
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                Ok(id)
            })
            .map_err(|e| e.to_string())?;

        let mut ids = Vec::new();
        for row in rows {
            if let Ok(id) = row {
                ids.push(id);
            }
        }
        Ok(ids)
    }
}

/// Convert a rusqlite Row into a Persona.
fn row_to_persona(row: &rusqlite::Row) -> Result<Persona, String> {
    let id: String = row.get(0).map_err(|e| e.to_string())?;
    let name: String = row.get(1).map_err(|e| e.to_string())?;
    let avatar_id: String = row.get(2).map_err(|e| e.to_string())?;
    let description: String = row.get(3).map_err(|e| e.to_string())?;
    let personality_notes: String = row.get(4).map_err(|e| e.to_string())?;
    let model: String = row.get(5).map_err(|e| e.to_string())?;
    let profile_id: Option<String> = row.get(6).map_err(|e| e.to_string())?;
    let agent: Option<String> = row.get(7).map_err(|e| e.to_string())?;
    let workdir: String = row.get(8).map_err(|e| e.to_string())?;
    let chat_session_id: String = row.get(9).map_err(|e| e.to_string())?;
    let is_default_int: i32 = row.get(10).map_err(|e| e.to_string())?;
    let continuous_browsing_int: i32 = row.get(11).map_err(|e| e.to_string())?;
    let created_at: String = row.get(12).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(13).map_err(|e| e.to_string())?;

    Ok(Persona {
        id,
        name,
        avatar_id,
        description,
        personality_notes,
        model,
        profile_id,
        agent: agent.or_else(|| Some("cteno".to_string())),
        workdir,
        chat_session_id,
        is_default: is_default_int != 0,
        continuous_browsing: continuous_browsing_int != 0,
        created_at,
        updated_at,
    })
}

/// Convert a rusqlite Row into a PersonaSessionLink.
fn row_to_link(row: &rusqlite::Row) -> Result<PersonaSessionLink, String> {
    let persona_id: String = row.get(0).map_err(|e| e.to_string())?;
    let session_id: String = row.get(1).map_err(|e| e.to_string())?;
    let session_type_str: String = row.get(2).map_err(|e| e.to_string())?;
    let task_description: Option<String> = row.get(3).map_err(|e| e.to_string())?;
    let agent_type: Option<String> = row.get(4).map_err(|e| e.to_string())?;
    let created_at: String = row.get(5).map_err(|e| e.to_string())?;
    let owner_kind: String = row.get(6).unwrap_or_else(|_| "persona".to_string());
    let label: Option<String> = row.get(7).unwrap_or(None);

    let session_type = PersonaSessionType::from_str(&session_type_str)?;

    Ok(PersonaSessionLink {
        persona_id,
        session_id,
        session_type,
        task_description,
        agent_type,
        owner_kind,
        label,
        created_at,
    })
}

fn row_to_workspace_binding(row: &rusqlite::Row) -> Result<WorkspaceBinding, String> {
    Ok(WorkspaceBinding {
        persona_id: row.get(0).map_err(|e| e.to_string())?,
        workspace_id: row.get(1).map_err(|e| e.to_string())?,
        template_id: row.get(2).map_err(|e| e.to_string())?,
        provider: row.get(3).map_err(|e| e.to_string())?,
        default_role_id: row.get(4).map_err(|e| e.to_string())?,
        model: row.get(5).map_err(|e| e.to_string())?,
        workdir: row.get(6).map_err(|e| e.to_string())?,
        created_at: row.get(7).map_err(|e| e.to_string())?,
        updated_at: row.get(8).map_err(|e| e.to_string())?,
    })
}
