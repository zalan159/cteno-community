//! Cross-vendor memory MCP server.
//!
//! Exposes four tools over stdio MCP so Claude/Codex/Gemini/Cteno sessions can
//! share the same Markdown-backed memory bank:
//! - `memory_save`   — append content (optional frontmatter `type`)
//! - `memory_recall` — keyword search across project + global scopes
//! - `memory_read`   — read a specific file
//! - `memory_list`   — list all memory files
//!
//! Memory lives on disk as plain Markdown (no SQLite, no vectors). Two scopes:
//! - **project** — `{project_dir}/.cteno/memory/` (per-project knowledge)
//! - **global**  — `{global_dir}` (shared across projects, default `~/.cteno/memory/`)

pub mod memory_core;
pub mod server;

pub use memory_core::{MemoryChunk, MemoryCore, Scope};
pub use server::MemoryServer;
