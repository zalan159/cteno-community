//! Cross-vendor agent config syncer.
//!
//! The authoritative source of truth for user-configured MCP servers, markdown
//! subagents, skills, and per-project system prompt lives under Cteno's own trees
//! (`~/.cteno/`, `{project}/.cteno/`). This crate owns the reconciliation
//! job that writes that same content into each vendor's native config
//! location so Claude / Codex / Gemini / Cteno all see the same things.
//!
//! Strategy:
//! - Symlinks where structurally possible (subagents, skills, system prompt).
//! - Config-file merge where the vendor file is a shared blob (Claude JSON,
//!   Codex TOML, Gemini JSON). Cteno-managed keys are written authoritatively;
//!   all other user-managed keys are preserved untouched.
//!
//! Host-as-authority: reconcile overwrites vendor-managed Cteno entries every
//! time. Users who want to change MCP / subagent / skill state should do it
//! through Cteno's UI; hand-edits inside `.claude/` `.gemini/` `.codex/` will
//! be clobbered on the next reconcile.

pub mod presets;
pub mod schemas;
pub mod symlink;
pub mod syncer;
pub mod vendors;

pub use presets::memory_mcp_spec;

pub use schemas::{McpSpec, McpTransport, PersonaSpec, SkillSpec};
pub use symlink::{ensure_symlink, ensure_symlink_to_dir};
pub use syncer::{SyncReport, VendorSyncer};
pub use vendors::{ClaudeSyncer, CodexSyncer, CtenoSyncer, GeminiSyncer};

use std::path::Path;

use anyhow::Result;

/// Run every vendor syncer over the same spec set. Failures abort — callers
/// should treat reconcile as atomic-enough (no partial-state contract, but
/// each vendor either fully succeeds or leaves an error trail for the user).
pub fn reconcile_all(
    project: &Path,
    authoritative_prompt: &Path,
    mcp: &[McpSpec],
    personas: &[PersonaSpec],
    skills: &[SkillSpec],
    vendors: &[&dyn VendorSyncer],
) -> Result<SyncReport> {
    let mut report = SyncReport::default();
    for v in vendors {
        let tag = v.vendor_name();
        tracing::debug!(vendor = tag, "reconcile: mcp");
        report.merge(v.sync_mcp(project, mcp)?);
        tracing::debug!(vendor = tag, "reconcile: subagents");
        report.merge(v.sync_subagents(project, personas)?);
        tracing::debug!(vendor = tag, "reconcile: skills");
        report.merge(v.sync_skills(project, skills)?);
        tracing::debug!(vendor = tag, "reconcile: system_prompt");
        report.merge(v.sync_system_prompt(project, authoritative_prompt)?);
    }
    Ok(report)
}
