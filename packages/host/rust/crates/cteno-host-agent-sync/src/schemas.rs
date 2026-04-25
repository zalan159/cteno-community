//! Canonical, vendor-neutral specs. Each vendor adapter converts these to
//! vendor-native shapes (JSON / TOML / markdown files).

use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum McpTransport {
    /// `command` + `args` + `env`. The server is launched as a subprocess and
    /// speaks JSON-RPC over stdio. This is what every vendor supports today.
    Stdio,
    /// Streamable-HTTP transport. Only Claude and Gemini support this natively;
    /// Codex adapter will fall back to stdio when it encounters this variant.
    StreamableHttp { url: String },
}

impl Default for McpTransport {
    fn default() -> Self {
        McpTransport::Stdio
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpSpec {
    /// The key used in each vendor's `mcpServers` map.
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub transport: McpTransport,
    /// True when this entry is managed by the host. Reconcile overwrites
    /// host-managed entries in-place; user-authored entries are left untouched.
    #[serde(default = "default_true")]
    pub host_managed: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaSpec {
    /// Stable slug used as the filename: `{name}.md`.
    pub name: String,
    /// Required by Claude/Gemini frontmatter; one-line summary.
    pub description: String,
    /// Full markdown document (with YAML frontmatter already inlined). Written
    /// verbatim into the authoritative path; each vendor target is a symlink.
    pub markdown: String,
    /// Authoritative on-disk location (e.g. `~/.cteno/agents/{name}.md` or
    /// `{project}/.cteno/agents/{name}.md`). The syncer creates a symlink
    /// from each vendor's expected path back to this file.
    pub source_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillSpec {
    /// Directory name (and skill id).
    pub name: String,
    /// Authoritative directory on disk, e.g. `~/.cteno/skills/web-search/` or
    /// `{project}/.cteno/skills/foo/`. Vendor targets are symlinks to this.
    pub source_dir: PathBuf,
}
