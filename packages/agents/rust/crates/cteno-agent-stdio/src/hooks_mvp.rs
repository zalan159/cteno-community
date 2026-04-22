//! Hook implementations for the stdio binary.
//!
//! We install the minimum set of hooks needed by the runtime's ReAct loop:
//!
//! - `ToolRegistryProvider` + `tool_registry_handle` — lets the loop list,
//!   describe and execute the built-in tools we register below, plus any
//!   host-injected tools registered dynamically via `tool_inject`.
//! - `ResolvedUrlProvider` — returns a happy-server URL from `HAPPY_SERVER_URL`
//!   (or empty string). stdio does not talk to happy-server directly, but some
//!   executors read the url hook defensively.
//!
//! The actual tool inventory lives in
//! `cteno_agent_runtime::tool_executors::register_all_builtin_executors` so
//! that the stdio binary and the future Tauri host share one list.
//!
//! Every hook that requires host-side wiring (skill registry, persona
//! dispatch, A2UI store, subagent bootstrap, command interceptor, machine
//! socket, spawn config, agent owner, session waker) is intentionally not
//! installed; executors that require them are deliberately excluded from the
//! built-in set.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::RwLock as AsyncRwLock;

use cteno_agent_runtime::hooks::{ResolvedUrlProvider, ToolRegistryProvider};
use cteno_agent_runtime::mcp::MCPRegistry;
use cteno_agent_runtime::runs::RunManager;
use cteno_agent_runtime::tool::registry::ToolRegistry;
use cteno_agent_runtime::tool::ToolExecutor;
use cteno_agent_runtime::tool_executors::register_all_builtin_executors;

/// Bridges the runtime's `ToolRegistryProvider` trait onto a concrete
/// `ToolRegistry` we own. The `tool_registry_handle` hook is installed in
/// parallel so that executors which need direct registry access
/// (tool_search, concurrency_safe checks) keep working.
pub struct StdioToolRegistry {
    inner: Arc<AsyncRwLock<ToolRegistry>>,
}

impl StdioToolRegistry {
    pub fn new(inner: Arc<AsyncRwLock<ToolRegistry>>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl ToolRegistryProvider for StdioToolRegistry {
    async fn execute(&self, tool_name: &str, input: Value) -> Result<String, String> {
        let guard = self.inner.read().await;
        guard.execute(tool_name, input).await
    }

    async fn list_tools(&self) -> Vec<String> {
        let guard = self.inner.read().await;
        guard.list_ids()
    }

    async fn describe(&self, tool_name: &str) -> Option<Value> {
        let guard = self.inner.read().await;
        guard
            .get_config(tool_name)
            .and_then(|cfg| serde_json::to_value(cfg.clone()).ok())
    }
}

pub struct StdioUrlProvider;

impl ResolvedUrlProvider for StdioUrlProvider {
    fn happy_server_url(&self) -> String {
        std::env::var("CTENO_HAPPY_SERVER_URL")
            .or_else(|_| std::env::var("HAPPY_SERVER_URL"))
            .unwrap_or_default()
    }
}

/// Return value of `install_default_registry`: the shared registry handle
/// (so callers can inject more tools dynamically) and the number of built-in
/// tools registered at boot.
pub struct InstalledRegistry {
    pub handle: Arc<AsyncRwLock<ToolRegistry>>,
    pub mcp_registries: SessionMcpRegistries,
    pub builtin_count: usize,
}

pub type SessionMcpRegistries = Arc<AsyncRwLock<HashMap<String, Arc<AsyncRwLock<MCPRegistry>>>>>;

struct SessionMcpToolExecutor {
    registries: SessionMcpRegistries,
    server_id: String,
    tool_name: String,
}

impl SessionMcpToolExecutor {
    fn new(registries: SessionMcpRegistries, server_id: String, tool_name: String) -> Self {
        Self {
            registries,
            server_id,
            tool_name,
        }
    }
}

#[async_trait]
impl ToolExecutor for SessionMcpToolExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        let session_id = input
            .get("__session_id")
            .and_then(Value::as_str)
            .ok_or_else(|| "MCP tool call missing __session_id".to_string())?
            .to_string();

        let registry = {
            let guard = self.registries.read().await;
            guard.get(&session_id).cloned()
        }
        .ok_or_else(|| format!("No MCP registry loaded for session {session_id}"))?;

        let registry = registry.read().await;
        registry
            .call_tool(&self.server_id, &self.tool_name, input)
            .await
    }

    fn supports_background(&self) -> bool {
        false
    }
}

/// Build a ToolRegistry populated with the runtime's built-in tool set and
/// register it globally (both via trait hook and via the concrete
/// `tool_registry_handle`). Returns the shared handle so the caller can
/// dynamically inject further tools.
pub fn install_default_registry(data_dir: PathBuf) -> InstalledRegistry {
    let mut registry = ToolRegistry::new();

    // RunManager is used by shell, run_manager, upload_artifact and
    // image_generation. Scratch logs live under `data_dir/runs/`.
    let run_manager = Arc::new(RunManager::new(data_dir.join("runs")));

    let count = register_all_builtin_executors(&mut registry, data_dir, run_manager);

    let arc_registry = Arc::new(AsyncRwLock::new(registry));
    let mcp_registries = Arc::new(AsyncRwLock::new(HashMap::new()));

    // Install both hooks: the trait-based one for generic callers, and the
    // concrete handle for tool_search / concurrency-aware scheduling.
    let provider = Arc::new(StdioToolRegistry::new(arc_registry.clone()));
    cteno_agent_runtime::hooks::install_tool_registry(provider);
    cteno_agent_runtime::hooks::install_tool_registry_handle(arc_registry.clone());

    // URL provider (best-effort; empty string is fine for offline stdio).
    cteno_agent_runtime::hooks::install_url_provider(Arc::new(StdioUrlProvider));

    InstalledRegistry {
        handle: arc_registry,
        mcp_registries,
        builtin_count: count,
    }
}

/// Load the MCP server set visible to one Cteno stdio session and register
/// its tools into the process-level ToolRegistry.
///
/// The registry hook is process-global, so MCP executors route by the
/// injected `__session_id` parameter added by the autonomous loop.
pub async fn install_session_mcp_tools(
    tool_registry: &Arc<AsyncRwLock<ToolRegistry>>,
    session_registries: &SessionMcpRegistries,
    session_id: &str,
    data_dir: &Path,
    workdir: Option<&str>,
) -> Result<usize, String> {
    let global_config = data_dir.join("mcp_servers.yaml");
    let project_config = workdir
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .map(|w| PathBuf::from(shellexpand::tilde(w).to_string()))
        .map(|w| w.join(".cteno").join("mcp_servers.yaml"));

    let mut mcp_registry = MCPRegistry::new();
    mcp_registry
        .load_from_scoped_configs(&global_config, project_config.as_deref())
        .await?;
    let tool_entries = mcp_registry.get_all_tool_configs();
    let mcp_registry = Arc::new(AsyncRwLock::new(mcp_registry));

    {
        let mut sessions = session_registries.write().await;
        sessions.insert(session_id.to_string(), mcp_registry);
    }

    let mut tools = tool_registry.write().await;
    let count = tool_entries.len();
    for (server_id, tool_name, tool_config) in tool_entries {
        let executor = Arc::new(SessionMcpToolExecutor::new(
            session_registries.clone(),
            server_id,
            tool_name,
        ));
        tools.register(tool_config, executor);
    }

    Ok(count)
}
