//! `VendorSyncer` trait — contract implemented by each vendor adapter
//! (Claude / Codex / Gemini / Cteno). A future `ConfigSyncer` façade will
//! fan out a single `reconcile_all()` call to every registered vendor.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::schemas::{McpSpec, PersonaSpec, SkillSpec};

/// What changed in a single reconcile pass. Reports are purely informational;
/// failures abort the reconcile and bubble up as `anyhow::Error`.
#[derive(Debug, Default)]
pub struct SyncReport {
    pub wrote: Vec<PathBuf>,
    pub skipped: Vec<(PathBuf, String)>,
}

impl SyncReport {
    pub fn note_write(&mut self, p: impl Into<PathBuf>) {
        self.wrote.push(p.into());
    }
    pub fn note_skip(&mut self, p: impl Into<PathBuf>, reason: impl Into<String>) {
        self.skipped.push((p.into(), reason.into()));
    }
    pub fn merge(&mut self, other: SyncReport) {
        self.wrote.extend(other.wrote);
        self.skipped.extend(other.skipped);
    }
}

pub trait VendorSyncer: Send + Sync {
    fn vendor_name(&self) -> &'static str;

    /// Write canonical MCP entries into this vendor's config file, preserving
    /// user-authored entries. For per-project vendors (Claude), `project` is
    /// the project root; for user-scope vendors (Codex/Gemini), the adapter
    /// picks the right file.
    fn sync_mcp(&self, project: &Path, specs: &[McpSpec]) -> Result<SyncReport>;

    /// Symlink each subagent's authoritative markdown into the vendor's
    /// native agents directory.
    fn sync_subagents(&self, project: &Path, specs: &[PersonaSpec]) -> Result<SyncReport>;

    /// Symlink each skill dir into the vendor's native skills directory. Skip
    /// silently for vendors without a native skills concept (return empty
    /// SyncReport, not error).
    fn sync_skills(&self, project: &Path, specs: &[SkillSpec]) -> Result<SyncReport>;

    /// Symlink the vendor's expected system-prompt path (CLAUDE.md / AGENTS.md
    /// / GEMINI.md) to the authoritative source. `authoritative` is the single
    /// source of truth — by convention `{project}/AGENTS.md`.
    fn sync_system_prompt(&self, project: &Path, authoritative: &Path) -> Result<SyncReport>;
}

/// Default no-op implementations for vendors without a given capability.
pub mod nop {
    use super::*;

    pub fn empty_report() -> Result<SyncReport> {
        Ok(SyncReport::default())
    }
}
