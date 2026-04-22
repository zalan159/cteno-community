//! Claude Code adapter.
//!
//! Layout:
//! - MCP:            `{project}/.mcp.json` (top-level `mcpServers`)
//! - Subagents:      `{project}/.claude/agents/{name}.md` (symlink)
//! - Skills:         `{project}/.claude/skills/{name}/` (dir symlink)
//! - System prompt:  `{project}/CLAUDE.md` (symlink to AGENTS.md)

use std::path::Path;

use anyhow::Result;
use serde_json::Value;

use super::{ensure_object_mut, mcp_to_json, persona_link_path, read_json_or_empty, write_json};
use crate::{
    schemas::{McpSpec, PersonaSpec, SkillSpec},
    symlink::{ensure_symlink, ensure_symlink_to_dir},
    syncer::{SyncReport, VendorSyncer},
};

pub struct ClaudeSyncer;

impl ClaudeSyncer {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ClaudeSyncer {
    fn default() -> Self {
        Self::new()
    }
}

impl VendorSyncer for ClaudeSyncer {
    fn vendor_name(&self) -> &'static str {
        "claude"
    }

    fn sync_mcp(&self, project: &Path, specs: &[McpSpec]) -> Result<SyncReport> {
        let mut report = SyncReport::default();
        let path = project.join(".mcp.json");
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
        let dir = project.join(".claude").join("agents");
        for p in specs {
            let link = persona_link_path(&dir, &p.name);
            ensure_symlink(&p.source_path, &link)?;
            report.note_write(link);
        }
        Ok(report)
    }

    fn sync_skills(&self, project: &Path, specs: &[SkillSpec]) -> Result<SyncReport> {
        let mut report = SyncReport::default();
        let dir = project.join(".claude").join("skills");
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
        let link = project.join("CLAUDE.md");
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

    fn sample_mcp(name: &str) -> McpSpec {
        McpSpec {
            name: name.into(),
            command: "cteno-memory-mcp".into(),
            args: vec!["--project-dir".into(), "/tmp/x".into()],
            env: BTreeMap::new(),
            transport: crate::schemas::McpTransport::Stdio,
            host_managed: true,
        }
    }

    #[test]
    fn mcp_preserves_unrelated_top_level_keys() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path();
        let mcp_path = project.join(".mcp.json");
        std::fs::write(
            &mcp_path,
            r#"{"userCustom":"keep me","mcpServers":{"already-there":{"command":"foo"}}}"#,
        )
        .unwrap();
        let syncer = ClaudeSyncer::new();
        syncer
            .sync_mcp(project, &[sample_mcp("cteno-memory")])
            .unwrap();
        let back: Value =
            serde_json::from_str(&std::fs::read_to_string(&mcp_path).unwrap()).unwrap();
        assert_eq!(back["userCustom"], "keep me");
        let servers = back["mcpServers"].as_object().unwrap();
        assert!(servers.contains_key("cteno-memory"));
        assert!(
            servers.contains_key("already-there"),
            "user-authored server was clobbered: {servers:?}"
        );
    }

    #[test]
    fn system_prompt_symlinks_to_agents_md() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path();
        let agents_md = project.join("AGENTS.md");
        std::fs::write(&agents_md, "system prompt body").unwrap();
        let syncer = ClaudeSyncer::new();
        syncer.sync_system_prompt(project, &agents_md).unwrap();
        let via = std::fs::read_to_string(project.join("CLAUDE.md")).unwrap();
        assert_eq!(via, "system prompt body");
    }

    #[test]
    fn subagents_symlink_into_claude_agents_dir() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path();
        let src = project.join(".cteno/agents/reviewer.md");
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();
        std::fs::write(&src, "---\nname: reviewer\ndescription: reviews\n---\nbody").unwrap();
        let syncer = ClaudeSyncer::new();
        syncer
            .sync_subagents(
                project,
                &[PersonaSpec {
                    name: "reviewer".into(),
                    description: "reviews".into(),
                    markdown: "".into(),
                    source_path: src.clone(),
                }],
            )
            .unwrap();
        let content = std::fs::read_to_string(project.join(".claude/agents/reviewer.md")).unwrap();
        assert!(content.contains("description: reviews"));
    }
}
