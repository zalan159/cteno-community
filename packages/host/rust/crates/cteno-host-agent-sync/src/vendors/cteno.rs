//! Cteno self-sync adapter.
//!
//! Cteno's authoritative trees (`~/.cteno/`, `{project}/.cteno/`) ARE the
//! canonical source — nothing to symlink out. The only action we take is
//! pointing `{project}/.cteno/PROMPT.md` at the authoritative `AGENTS.md`
//! (and only if they diverge), so any Cteno-runtime code that reads PROMPT.md
//! still resolves to the unified prompt.
//!
//! Subagents / skills are served by reading the authoritative trees directly
//! from Cteno's own runtime. MCP still needs a project-scoped config file so
//! the stdio runtime can merge `{project}/.cteno/mcp_servers.yaml` over the
//! global MCP config at session init.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{
    schemas::{McpSpec, McpTransport, PersonaSpec, SkillSpec},
    symlink::ensure_symlink,
    syncer::{SyncReport, VendorSyncer},
};

pub struct CtenoSyncer;

impl CtenoSyncer {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CtenoSyncer {
    fn default() -> Self {
        Self::new()
    }
}

impl VendorSyncer for CtenoSyncer {
    fn vendor_name(&self) -> &'static str {
        "cteno"
    }

    fn sync_mcp(&self, project: &Path, specs: &[McpSpec]) -> Result<SyncReport> {
        let mut report = SyncReport::default();
        if specs.is_empty() {
            return Ok(report);
        }

        let path = project.join(".cteno").join("mcp_servers.yaml");
        let mut config = read_yaml_or_empty(&path)?;
        for spec in specs {
            upsert_server(&mut config.servers, spec);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, serde_yaml::to_string(&config)?)?;
        report.note_write(path);
        Ok(report)
    }

    fn sync_subagents(&self, _project: &Path, _specs: &[PersonaSpec]) -> Result<SyncReport> {
        Ok(SyncReport::default())
    }

    fn sync_skills(&self, _project: &Path, _specs: &[SkillSpec]) -> Result<SyncReport> {
        Ok(SyncReport::default())
    }

    fn sync_system_prompt(&self, project: &Path, authoritative: &Path) -> Result<SyncReport> {
        let mut report = SyncReport::default();
        let link = project.join(".cteno").join("PROMPT.md");
        ensure_symlink(authoritative, &link)?;
        report.note_write(link);
        Ok(report)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CtenoMcpConfig {
    #[serde(default)]
    servers: Vec<CtenoMcpServer>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CtenoMcpServer {
    id: String,
    name: String,
    #[serde(default = "default_true")]
    enabled: bool,
    transport: CtenoMcpTransport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum CtenoMcpTransport {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: std::collections::BTreeMap<String, String>,
    },
    HttpSse {
        url: String,
        #[serde(default)]
        headers: std::collections::BTreeMap<String, String>,
    },
}

fn default_true() -> bool {
    true
}

fn read_yaml_or_empty(path: &Path) -> Result<CtenoMcpConfig> {
    match std::fs::read_to_string(path) {
        Ok(s) if s.trim().is_empty() => Ok(CtenoMcpConfig::default()),
        Ok(s) => {
            serde_yaml::from_str(&s).with_context(|| format!("parse Cteno MCP config {:?}", path))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(CtenoMcpConfig::default()),
        Err(e) => Err(e).with_context(|| format!("read Cteno MCP config {:?}", path)),
    }
}

fn upsert_server(servers: &mut Vec<CtenoMcpServer>, spec: &McpSpec) {
    let rendered = CtenoMcpServer {
        id: spec.name.clone(),
        name: spec.name.clone(),
        enabled: true,
        transport: match &spec.transport {
            McpTransport::Stdio => CtenoMcpTransport::Stdio {
                command: spec.command.clone(),
                args: spec.args.clone(),
                env: spec.env.clone(),
            },
            McpTransport::StreamableHttp { url } => CtenoMcpTransport::HttpSse {
                url: url.clone(),
                headers: Default::default(),
            },
        },
    };

    if let Some(existing) = servers.iter_mut().find(|server| server.id == spec.name) {
        *existing = rendered;
    } else {
        servers.push(rendered);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syncer::VendorSyncer;
    use std::collections::BTreeMap;

    #[test]
    fn sync_mcp_writes_project_config_and_preserves_user_servers() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project = temp.path();
        let cteno_dir = project.join(".cteno");
        let config_path = cteno_dir.join("mcp_servers.yaml");
        std::fs::create_dir_all(&cteno_dir).expect("cteno dir");
        std::fs::write(
            &config_path,
            r#"
servers:
  - id: user-server
    name: user-server
    enabled: false
    transport:
      type: stdio
      command: user-command
"#,
        )
        .expect("existing config");

        let spec = McpSpec {
            name: "cteno-memory".to_string(),
            command: "/bin/cteno-memory-mcp".to_string(),
            args: vec!["--project-dir".to_string(), project.display().to_string()],
            env: BTreeMap::new(),
            transport: McpTransport::Stdio,
            host_managed: true,
        };

        let report = CtenoSyncer::new()
            .sync_mcp(project, &[spec])
            .expect("sync mcp");
        assert_eq!(report.wrote, vec![config_path.clone()]);

        let config = read_yaml_or_empty(&config_path).expect("read rendered config");
        assert_eq!(config.servers.len(), 2);

        let user = config
            .servers
            .iter()
            .find(|server| server.id == "user-server")
            .expect("user server preserved");
        assert!(!user.enabled);

        let memory = config
            .servers
            .iter()
            .find(|server| server.id == "cteno-memory")
            .expect("cteno memory server");
        match &memory.transport {
            CtenoMcpTransport::Stdio { command, args, .. } => {
                assert_eq!(command, "/bin/cteno-memory-mcp");
                assert!(args.iter().any(|arg| arg == "--project-dir"));
                assert!(args.iter().any(|arg| arg == &project.display().to_string()));
            }
            CtenoMcpTransport::HttpSse { .. } => panic!("expected stdio transport"),
        }
    }
}
