//! Persona Agent System
//!
//! Personas are strategic AI agents with persistent personality and memory.
//! Each persona has its own chat session and can dispatch tasks to worker sessions.

pub mod browser_prompt;
pub mod manager;
pub mod models;
pub mod prompt;
pub mod store;

pub use manager::PersonaManager;
pub use models::{Persona, PersonaSessionLink, PersonaSessionType, WorkspaceBinding};
pub use store::PersonaStore;
