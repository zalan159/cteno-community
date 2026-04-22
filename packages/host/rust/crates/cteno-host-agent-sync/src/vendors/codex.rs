//! Codex CLI adapter.
//!
//! Layout:
//! - MCP:            `~/.codex/config.toml` under `[mcp_servers.{name}]`
//! - Subagents:      no-op (Codex has no native markdown subagent concept)
//! - Skills:         no-op (same)
//! - System prompt:  `{project}/AGENTS.md` IS the authoritative source — no-op
//!
//! MCP entries are written surgically with `toml_edit` so unrelated sections
//! (`model`, `[projects.*]`, `[features]`) are preserved byte-for-byte.

use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

use anyhow::{Context, Result};
use toml_edit::{value, Array, DocumentMut, Item, Table};

use crate::{
    schemas::{McpSpec, McpTransport, PersonaSpec, SkillSpec},
    syncer::{SyncReport, VendorSyncer},
};

pub struct CodexSyncer {
    config_path: PathBuf,
}

impl CodexSyncer {
    pub fn new() -> Self {
        Self {
            config_path: default_config_path(),
        }
    }

    pub fn with_config_path(path: PathBuf) -> Self {
        Self { config_path: path }
    }
}

impl Default for CodexSyncer {
    fn default() -> Self {
        Self::new()
    }
}

fn default_config_path() -> PathBuf {
    static P: OnceLock<PathBuf> = OnceLock::new();
    P.get_or_init(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".codex")
            .join("config.toml")
    })
    .clone()
}

impl VendorSyncer for CodexSyncer {
    fn vendor_name(&self) -> &'static str {
        "codex"
    }

    fn sync_mcp(&self, _project: &Path, specs: &[McpSpec]) -> Result<SyncReport> {
        let mut report = SyncReport::default();
        let source = match std::fs::read_to_string(&self.config_path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => Err(e).with_context(|| format!("read {:?}", self.config_path))?,
        };
        let mut doc: DocumentMut = source
            .parse()
            .with_context(|| format!("parse TOML {:?}", self.config_path))?;

        let root_servers = doc.as_table_mut().entry("mcp_servers").or_insert_with(|| {
            let mut t = Table::new();
            t.set_implicit(true);
            Item::Table(t)
        });
        let servers_tbl = root_servers
            .as_table_mut()
            .context("`mcp_servers` is not a table")?;

        for spec in specs {
            let mut entry = Table::new();
            match &spec.transport {
                McpTransport::Stdio => {
                    entry.insert("command", value(spec.command.clone()));
                    if !spec.args.is_empty() {
                        let arr: Array = spec.args.iter().cloned().collect();
                        entry.insert("args", value(arr));
                    }
                }
                McpTransport::StreamableHttp { url } => {
                    // Codex does not speak HTTP MCP yet; record the URL as a
                    // hint key for operator visibility, but leave no command.
                    entry.insert("url", value(url.clone()));
                }
            }
            if !spec.env.is_empty() {
                let mut env = Table::new();
                for (k, v) in &spec.env {
                    env.insert(k, value(v.clone()));
                }
                entry.insert("env", Item::Table(env));
            }
            servers_tbl.insert(&spec.name, Item::Table(entry));
        }

        if let Some(parent) = self.config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.config_path, doc.to_string())?;
        report.note_write(self.config_path.clone());
        Ok(report)
    }

    fn sync_subagents(&self, _project: &Path, _specs: &[PersonaSpec]) -> Result<SyncReport> {
        Ok(SyncReport::default())
    }

    fn sync_skills(&self, _project: &Path, _specs: &[SkillSpec]) -> Result<SyncReport> {
        Ok(SyncReport::default())
    }

    fn sync_system_prompt(&self, _project: &Path, _authoritative: &Path) -> Result<SyncReport> {
        // AGENTS.md is Codex's native authoritative prompt — already there.
        Ok(SyncReport::default())
    }
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
            args: vec!["--project-dir".into(), "/proj".into()],
            env: BTreeMap::new(),
            transport: McpTransport::Stdio,
            host_managed: true,
        }
    }

    #[test]
    fn inserts_mcp_server_without_destroying_other_sections() {
        let tmp = TempDir::new().unwrap();
        let cfg = tmp.path().join("config.toml");
        std::fs::write(
            &cfg,
            r#"model = "gpt-5"
model_reasoning_effort = "high"

[features]
multi_agent = true

[projects."/Users/zal/code"]
trust_level = "trusted"
"#,
        )
        .unwrap();
        let syncer = CodexSyncer::with_config_path(cfg.clone());
        syncer
            .sync_mcp(tmp.path(), &[sample_mcp("cteno-memory")])
            .unwrap();
        let back = std::fs::read_to_string(&cfg).unwrap();
        assert!(back.contains(r#"model = "gpt-5""#), "{back}");
        assert!(back.contains("[features]"), "{back}");
        assert!(back.contains("multi_agent = true"), "{back}");
        assert!(back.contains("trust_level = \"trusted\""), "{back}");
        assert!(
            back.contains("[mcp_servers.cteno-memory]"),
            "missing mcp entry:\n{back}"
        );
        assert!(back.contains("cteno-memory-mcp"), "{back}");
        assert!(back.contains("--project-dir"), "{back}");
    }

    #[test]
    fn second_reconcile_overwrites_own_entry_not_user_entries() {
        let tmp = TempDir::new().unwrap();
        let cfg = tmp.path().join("config.toml");
        std::fs::write(
            &cfg,
            r#"[mcp_servers.user-authored]
command = "my-mcp"
"#,
        )
        .unwrap();
        let syncer = CodexSyncer::with_config_path(cfg.clone());
        syncer
            .sync_mcp(tmp.path(), &[sample_mcp("cteno-memory")])
            .unwrap();
        syncer
            .sync_mcp(tmp.path(), &[sample_mcp("cteno-memory")])
            .unwrap();
        let back = std::fs::read_to_string(&cfg).unwrap();
        assert!(
            back.contains("[mcp_servers.user-authored]"),
            "user entry clobbered:\n{back}"
        );
        assert!(back.contains("[mcp_servers.cteno-memory]"), "{back}");
        assert_eq!(
            back.matches("[mcp_servers.cteno-memory]").count(),
            1,
            "duplicate entries after second reconcile:\n{back}"
        );
    }
}
