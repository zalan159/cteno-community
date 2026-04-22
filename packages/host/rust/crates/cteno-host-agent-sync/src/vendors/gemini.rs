//! Gemini CLI adapter.
//!
//! Layout:
//! - MCP:            `{project}/.gemini/settings.json` (top-level `mcpServers`)
//! - Subagents:      `{project}/.gemini/agents/{name}.md` (symlink)
//! - Skills:         `{project}/.gemini/skills/{name}/` (dir symlink) — Gemini
//!                   may ignore this, but the file is harmless to ship.
//! - System prompt:  `{project}/GEMINI.md` (symlink to AGENTS.md)

use std::path::Path;

use anyhow::Result;
use serde_json::Value;

use super::{ensure_object_mut, mcp_to_json, persona_link_path, read_json_or_empty, write_json};
use crate::{
    schemas::{McpSpec, PersonaSpec, SkillSpec},
    symlink::{ensure_symlink, ensure_symlink_to_dir},
    syncer::{SyncReport, VendorSyncer},
};

pub struct GeminiSyncer;

impl GeminiSyncer {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GeminiSyncer {
    fn default() -> Self {
        Self::new()
    }
}

impl VendorSyncer for GeminiSyncer {
    fn vendor_name(&self) -> &'static str {
        "gemini"
    }

    fn sync_mcp(&self, project: &Path, specs: &[McpSpec]) -> Result<SyncReport> {
        let mut report = SyncReport::default();
        let path = project.join(".gemini").join("settings.json");
        let mut root = read_json_or_empty(&path)?;
        let obj = ensure_object_mut(&mut root);
        let servers = obj
            .entry("mcpServers")
            .or_insert_with(|| Value::Object(Default::default()));
        let map = ensure_object_mut(servers);
        for spec in specs {
            map.insert(spec.name.clone(), mcp_to_json(spec));
        }
        write_json(&path, &root)?;
        report.note_write(path);
        Ok(report)
    }

    fn sync_subagents(&self, project: &Path, specs: &[PersonaSpec]) -> Result<SyncReport> {
        let mut report = SyncReport::default();
        let dir = project.join(".gemini").join("agents");
        for p in specs {
            let link = persona_link_path(&dir, &p.name);
            ensure_symlink(&p.source_path, &link)?;
            report.note_write(link);
        }
        Ok(report)
    }

    fn sync_skills(&self, project: &Path, specs: &[SkillSpec]) -> Result<SyncReport> {
        let mut report = SyncReport::default();
        let dir = project.join(".gemini").join("skills");
        for s in specs {
            let link = dir.join(&s.name);
            if should_skip_existing_dir(&link) {
                report.note_skip(
                    link,
                    "pre-existing non-empty directory kept in place (not clobbered)",
                );
                continue;
            }
            ensure_symlink_to_dir(&s.source_dir, &link)?;
            report.note_write(link);
        }
        Ok(report)
    }

    fn sync_system_prompt(&self, project: &Path, authoritative: &Path) -> Result<SyncReport> {
        let mut report = SyncReport::default();
        let link = project.join("GEMINI.md");
        ensure_symlink(authoritative, &link)?;
        report.note_write(link);
        Ok(report)
    }
}

fn should_skip_existing_dir(path: &Path) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return false;
    }
    std::fs::read_dir(path)
        .ok()
        .and_then(|mut it| it.next())
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    #[test]
    fn preserves_existing_security_block() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path();
        let settings = project.join(".gemini/settings.json");
        std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
        std::fs::write(
            &settings,
            r#"{"security":{"auth":{"selectedType":"oauth-personal"}}}"#,
        )
        .unwrap();
        let syncer = GeminiSyncer::new();
        syncer
            .sync_mcp(
                project,
                &[McpSpec {
                    name: "cteno-memory".into(),
                    command: "cteno-memory-mcp".into(),
                    args: vec![],
                    env: BTreeMap::new(),
                    transport: crate::schemas::McpTransport::Stdio,
                    host_managed: true,
                }],
            )
            .unwrap();
        let back: Value =
            serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(back["security"]["auth"]["selectedType"], "oauth-personal");
        assert!(back["mcpServers"]["cteno-memory"].is_object());
    }

    #[test]
    fn gemini_md_symlinks_authoritative() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path();
        let src = project.join("AGENTS.md");
        std::fs::write(&src, "prompt").unwrap();
        GeminiSyncer::new()
            .sync_system_prompt(project, &src)
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(project.join("GEMINI.md")).unwrap(),
            "prompt"
        );
    }
}
