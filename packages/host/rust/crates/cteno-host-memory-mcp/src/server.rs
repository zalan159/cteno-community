//! MCP stdio server exposing the four memory tools.

use std::sync::Arc;

use rmcp::{
    handler::server::{tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler,
};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::memory_core::{MemoryCore, Scope};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SaveRequest {
    /// Relative path under the chosen scope. `.md` is appended if missing.
    /// Examples: `"knowledge/rust.md"`, `"feedback_testing"`, `"memory/2026-04-21"`.
    pub file_path: String,
    /// Markdown content to append.
    pub content: String,
    /// `"project"` (default) or `"global"`. Project scope requires the server
    /// to have been started with `--project-dir`.
    #[serde(default)]
    pub scope: Option<String>,
    /// Optional `user` / `feedback` / `project` / `reference` tag. When set,
    /// the content is wrapped in a YAML frontmatter block with `type` + `date`.
    #[serde(default, rename = "type")]
    pub memory_type: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RecallRequest {
    /// Natural-language query — tokens are lowercased and matched against
    /// Markdown chunks across both `project` and `global` scopes.
    pub query: String,
    /// Max results to return. Defaults to 10.
    #[serde(default)]
    pub limit: Option<u32>,
    /// Restrict to entries whose frontmatter `type` matches.
    #[serde(default, rename = "type")]
    pub type_filter: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadRequest {
    pub file_path: String,
    #[serde(default)]
    pub scope: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListRequest {
    /// Omit to list both scopes (results tagged `[project]` / `[global]`).
    #[serde(default)]
    pub scope: Option<String>,
}

#[derive(Clone)]
pub struct MemoryServer {
    core: Arc<MemoryCore>,
    tool_router: ToolRouter<Self>,
}

fn parse_scope(raw: Option<&str>, default: Scope) -> Result<Scope, McpError> {
    match raw {
        None => Ok(default),
        Some(s) => Scope::parse(s).map_err(|e| McpError::invalid_params(e, None)),
    }
}

fn text_ok(msg: impl Into<String>) -> CallToolResult {
    CallToolResult::success(vec![Content::text(msg.into())])
}

#[tool_router]
impl MemoryServer {
    pub fn new(core: Arc<MemoryCore>) -> Self {
        Self {
            core,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Append content to a memory file. scope: 'project' (default, per-project knowledge) or 'global' (shared across projects). Optional `type` tag (user/feedback/project/reference) wraps content in YAML frontmatter."
    )]
    async fn memory_save(
        &self,
        Parameters(req): Parameters<SaveRequest>,
    ) -> Result<CallToolResult, McpError> {
        let scope = parse_scope(req.scope.as_deref(), Scope::Project)?;
        self.core
            .save(
                &req.file_path,
                &req.content,
                scope,
                req.memory_type.as_deref(),
            )
            .map(text_ok)
            .map_err(|e| McpError::internal_error(e, None))
    }

    #[tool(
        description = "Keyword search across BOTH project and global memory. Returns up to `limit` chunks tagged [project] / [global], ranked by keyword coverage. Optional `type` filter restricts to entries with that frontmatter type."
    )]
    async fn memory_recall(
        &self,
        Parameters(req): Parameters<RecallRequest>,
    ) -> Result<CallToolResult, McpError> {
        let limit = req.limit.unwrap_or(10).max(1) as usize;
        let chunks = self
            .core
            .recall(&req.query, limit, req.type_filter.as_deref())
            .map_err(|e| McpError::internal_error(e, None))?;
        if chunks.is_empty() {
            return Ok(text_ok("No matching memories found."));
        }
        let mut out = format!("Found {} results:\n\n", chunks.len());
        for c in &chunks {
            out.push_str(&format!(
                "--- [{}] {} (score: {:.2}) ---\n{}\n\n",
                c.scope, c.file_path, c.score, c.content
            ));
        }
        Ok(text_ok(out))
    }

    #[tool(description = "Read a single memory file. scope: 'project' (default) or 'global'.")]
    async fn memory_read(
        &self,
        Parameters(req): Parameters<ReadRequest>,
    ) -> Result<CallToolResult, McpError> {
        let scope = parse_scope(req.scope.as_deref(), Scope::Project)?;
        match self
            .core
            .read(&req.file_path, scope)
            .map_err(|e| McpError::internal_error(e, None))?
        {
            Some(content) => Ok(text_ok(content)),
            None => Ok(text_ok(format!(
                "File not found in [{}]: {}",
                scope.as_tag(),
                req.file_path
            ))),
        }
    }

    #[tool(
        description = "List all memory files. Omit scope to list both (tagged [project] / [global])."
    )]
    async fn memory_list(
        &self,
        Parameters(req): Parameters<ListRequest>,
    ) -> Result<CallToolResult, McpError> {
        let scope = match req.scope.as_deref() {
            None => None,
            Some(s) => Some(Scope::parse(s).map_err(|e| McpError::invalid_params(e, None))?),
        };
        let files = self
            .core
            .list(scope)
            .map_err(|e| McpError::internal_error(e, None))?;
        if files.is_empty() {
            return Ok(text_ok("No memory files found."));
        }
        let mut out = format!("{} memory files:\n", files.len());
        for f in &files {
            out.push_str(&format!("- {f}\n"));
        }
        Ok(text_ok(out))
    }
}

#[tool_handler]
impl ServerHandler for MemoryServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "Cteno cross-vendor memory. Four tools over plain Markdown: \
                 memory_save, memory_recall, memory_read, memory_list. \
                 Two scopes: 'project' (per-project) and 'global' (shared)."
                    .into(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}
