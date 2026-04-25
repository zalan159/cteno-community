//! Database Initialization
//!
//! Sets up SQLite databases for sessions and tasks.

use rusqlite::{Connection, Result as SqliteResult};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

/// Initialize all databases
pub fn init(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = get_data_dir(app)?;
    init_at_data_dir(&data_dir)
}

/// Initialize all databases using an explicit data directory (no Tauri AppHandle required).
pub fn init_at_data_dir(data_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(data_dir)?;

    // Initialize main database
    let db_path = data_dir.join("db").join("cteno.db");
    std::fs::create_dir_all(db_path.parent().unwrap())?;
    init_main_db(&db_path)?;

    // Create workspace directories
    let workspace_path = data_dir.join("workspace");
    std::fs::create_dir_all(&workspace_path)?;
    std::fs::create_dir_all(workspace_path.join("memory"))?;

    // Create default memory files if they don't exist
    create_default_files(&workspace_path)?;

    log::info!("Database initialized at {:?}", data_dir);
    Ok(())
}

/// Initialize the main database (sessions, tasks)
fn init_main_db(path: &PathBuf) -> SqliteResult<()> {
    let conn = Connection::open(path)?;

    conn.execute_batch(
        r#"
        -- Sessions table (general purpose)
        CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            status TEXT DEFAULT 'idle' CHECK(status IN ('idle', 'active', 'waiting')),
            messages TEXT DEFAULT '[]',
            pending_info TEXT,
            created_at TEXT DEFAULT (datetime('now')),
            updated_at TEXT DEFAULT (datetime('now'))
        );

        -- Agent sessions table (for autonomous agents with context retention)
        CREATE TABLE IF NOT EXISTS agent_sessions (
            id TEXT PRIMARY KEY,
            agent_id TEXT NOT NULL,
            user_id TEXT,
            messages TEXT NOT NULL DEFAULT '[]',
            context_data TEXT,
            agent_state TEXT,
            agent_state_version INTEGER NOT NULL DEFAULT 0,
            status TEXT DEFAULT 'active' CHECK(status IN ('active', 'expired', 'closed')),
            created_at TEXT DEFAULT (datetime('now')),
            updated_at TEXT DEFAULT (datetime('now')),
            expires_at TEXT
        );

        -- Outbound messages queue
        CREATE TABLE IF NOT EXISTS outbound_messages (
            id TEXT PRIMARY KEY,
            recipient TEXT NOT NULL,
            text TEXT NOT NULL,
            status TEXT DEFAULT 'pending' CHECK(status IN ('pending', 'sent', 'failed')),
            retry_count INTEGER DEFAULT 0,
            created_at TEXT DEFAULT (datetime('now'))
        );

        -- Indexes
        CREATE INDEX IF NOT EXISTS idx_sessions_user ON sessions(user_id);
        CREATE INDEX IF NOT EXISTS idx_sessions_status ON sessions(status);
        CREATE INDEX IF NOT EXISTS idx_agent_sessions_agent ON agent_sessions(agent_id);
        CREATE INDEX IF NOT EXISTS idx_agent_sessions_user ON agent_sessions(user_id);
        CREATE INDEX IF NOT EXISTS idx_agent_sessions_status ON agent_sessions(status);
        CREATE INDEX IF NOT EXISTS idx_agent_sessions_expires ON agent_sessions(expires_at);
        CREATE INDEX IF NOT EXISTS idx_outbound_status ON outbound_messages(status);
        "#,
    )?;

    // Migration: add owner_session_id column to agent_sessions (for persona task session ownership)
    let has_owner_col: bool = conn
        .prepare("SELECT COUNT(*) FROM pragma_table_info('agent_sessions') WHERE name='owner_session_id'")
        .and_then(|mut stmt| stmt.query_row([], |row| row.get::<_, i64>(0)))
        .unwrap_or(0)
        > 0;
    if !has_owner_col {
        conn.execute_batch("ALTER TABLE agent_sessions ADD COLUMN owner_session_id TEXT")?;
        log::info!("[DB] Migrated agent_sessions: added owner_session_id column");
    }

    // Migration: add vendor column to agent_sessions so the community
    // SessionStoreProvider can scope lookups per vendor (cteno / claude /
    // codex). Existing rows default to "cteno" — the only vendor that
    // existed before this migration.
    let has_vendor_col: bool = conn
        .prepare("SELECT COUNT(*) FROM pragma_table_info('agent_sessions') WHERE name='vendor'")
        .and_then(|mut stmt| stmt.query_row([], |row| row.get::<_, i64>(0)))
        .unwrap_or(0)
        > 0;
    if !has_vendor_col {
        conn.execute_batch(
            "ALTER TABLE agent_sessions ADD COLUMN vendor TEXT NOT NULL DEFAULT 'cteno'",
        )?;
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_agent_sessions_vendor ON agent_sessions(vendor)",
        )?;
        log::info!("[DB] Migrated agent_sessions: added vendor column");
    }

    // Migration: persist local agent state so permission prompts survive
    // frontend reloads or missed Tauri events.
    let has_agent_state_col: bool = conn
        .prepare(
            "SELECT COUNT(*) FROM pragma_table_info('agent_sessions') WHERE name='agent_state'",
        )
        .and_then(|mut stmt| stmt.query_row([], |row| row.get::<_, i64>(0)))
        .unwrap_or(0)
        > 0;
    if !has_agent_state_col {
        conn.execute_batch("ALTER TABLE agent_sessions ADD COLUMN agent_state TEXT")?;
        conn.execute_batch(
            "ALTER TABLE agent_sessions ADD COLUMN agent_state_version INTEGER NOT NULL DEFAULT 0",
        )?;
        log::info!("[DB] Migrated agent_sessions: added agent_state columns");
    }

    Ok(())
}

/// Create default memory files
fn create_default_files(workspace_path: &Path) -> std::io::Result<()> {
    let memory_md = workspace_path.join("MEMORY.md");
    if !memory_md.exists() {
        std::fs::write(
            &memory_md,
            r#"# 长期记忆

## 用户偏好

## 发布习惯

## 经验教训
"#,
        )?;
    }

    let user_md = workspace_path.join("USER.md");
    if !user_md.exists() {
        std::fs::write(
            &user_md,
            r#"# 用户信息

## 基本信息

## 账号信息

## 联系方式
"#,
        )?;
    }

    Ok(())
}

/// Get the application data directory
fn get_data_dir(app: &AppHandle) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let path = app.path().app_data_dir()?;
    Ok(path)
}
