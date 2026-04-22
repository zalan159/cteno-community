//! MCP Registry — rmcp-based
//!
//! Manages multiple MCP server connections using the official rmcp SDK.
//! Supports stdio (child process) and streamable HTTP (SSE) transports.

use super::{MCPConfigFile, MCPServerConfig, MCPTransport, ServerStatus};
use crate::tool::{ToolCategory, ToolConfig};
use rmcp::model::CallToolRequestParams;
use rmcp::service::RunningService;
use rmcp::transport::TokioChildProcess;
use rmcp::ServiceExt;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Sanitize a string to only contain [a-zA-Z0-9_-] (valid for LLM tool names)
pub fn sanitize_for_tool_name(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Cached tool info from an MCP server
#[derive(Debug, Clone)]
pub struct MCPToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// State for an MCP server (may or may not be connected)
struct MCPServerState {
    config: MCPServerConfig,
    service: Option<RunningService<rmcp::RoleClient, ()>>,
    tools: Vec<MCPToolInfo>,
    status: ServerStatus,
    scope: String,
}

/// MCP Registry — manages all MCP server connections
pub struct MCPRegistry {
    servers: HashMap<String, MCPServerState>,
    config_path: PathBuf,
}

impl MCPRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            servers: HashMap::new(),
            config_path: PathBuf::new(),
        }
    }

    /// Load MCP servers from a YAML configuration file
    pub async fn load_from_config(&mut self, config_path: &Path) -> Result<(), String> {
        self.config_path = config_path.to_path_buf();

        let configs = Self::read_config_file(config_path)?;
        self.load_configs(configs.servers.into_iter().map(|c| (c, "global")))
            .await
    }

    /// Load global + project MCP configs into one registry.
    ///
    /// Project entries override global entries with the same `id`. The
    /// registry's save path stays pointed at the global file because merged
    /// registries are normally used for session startup, not editing.
    pub async fn load_from_scoped_configs(
        &mut self,
        global_config_path: &Path,
        project_config_path: Option<&Path>,
    ) -> Result<(), String> {
        self.config_path = global_config_path.to_path_buf();

        let mut merged: HashMap<String, (MCPServerConfig, &'static str)> = HashMap::new();

        if global_config_path.exists() {
            let config = Self::read_config_file(global_config_path)?;
            for server in config.servers {
                merged.insert(server.id.clone(), (server, "global"));
            }
        } else {
            log::warn!("MCP global config file not found: {:?}", global_config_path);
        }

        if let Some(project_config_path) = project_config_path {
            if project_config_path.exists() {
                let config = Self::read_config_file(project_config_path)?;
                for server in config.servers {
                    merged.insert(server.id.clone(), (server, "project"));
                }
            }
        }

        self.load_configs(merged.into_values()).await
    }

    fn read_config_file(config_path: &Path) -> Result<MCPConfigFile, String> {
        if !config_path.exists() {
            log::warn!("MCP config file not found: {:?}", config_path);
            return Ok(MCPConfigFile { servers: vec![] });
        }

        let config_str = std::fs::read_to_string(config_path)
            .map_err(|e| format!("Failed to read MCP config: {}", e))?;

        serde_yaml::from_str(&config_str).map_err(|e| format!("Failed to parse MCP config: {}", e))
    }

    async fn load_configs<I>(&mut self, configs: I) -> Result<(), String>
    where
        I: IntoIterator<Item = (MCPServerConfig, &'static str)>,
    {
        for (server_config, scope) in configs {
            if server_config.enabled {
                log::info!(
                    "Connecting MCP server: {} ({})",
                    server_config.name,
                    server_config.id
                );
                if let Err(e) = self.connect_server(server_config.clone(), scope).await {
                    log::error!("Failed to connect MCP server {}: {}", server_config.name, e);
                }
            } else {
                self.servers.insert(
                    server_config.id.clone(),
                    MCPServerState {
                        config: server_config,
                        service: None,
                        tools: vec![],
                        status: ServerStatus::Disconnected,
                        scope: scope.to_string(),
                    },
                );
            }
        }

        Ok(())
    }

    /// Connect to an MCP server based on its transport config
    async fn connect_server(&mut self, config: MCPServerConfig, scope: &str) -> Result<(), String> {
        let server_id = config.id.clone();

        match self.try_connect(&config).await {
            Ok((service, tools)) => {
                let tool_count = tools.len();
                log::info!(
                    "MCP server {} connected with {} tools",
                    config.name,
                    tool_count
                );
                self.servers.insert(
                    server_id,
                    MCPServerState {
                        config,
                        service: Some(service),
                        tools,
                        status: ServerStatus::Connected,
                        scope: scope.to_string(),
                    },
                );
            }
            Err(e) => {
                log::error!("MCP server {} failed to connect: {}", config.name, e);
                self.servers.insert(
                    server_id,
                    MCPServerState {
                        config,
                        service: None,
                        tools: vec![],
                        status: ServerStatus::Error(e),
                        scope: scope.to_string(),
                    },
                );
            }
        }

        Ok(())
    }

    /// Try to connect to an MCP server, returning the service and tools on success
    async fn try_connect(
        &self,
        config: &MCPServerConfig,
    ) -> Result<(RunningService<rmcp::RoleClient, ()>, Vec<MCPToolInfo>), String> {
        match &config.transport {
            MCPTransport::Stdio { command, args, env } => {
                Self::connect_stdio(command, args, env).await
            }
            MCPTransport::HttpSse { url, headers } => Self::connect_http_sse(url, headers).await,
        }
    }

    /// Connect to a stdio MCP server via child process
    async fn connect_stdio(
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> Result<(RunningService<rmcp::RoleClient, ()>, Vec<MCPToolInfo>), String> {
        log::info!("MCP stdio: spawning '{}' with args {:?}", command, args);

        // First, test that the command exists and can start
        let mut test_cmd = tokio::process::Command::new(command);
        for arg in args {
            test_cmd.arg(arg);
        }
        for (k, v) in env {
            test_cmd.env(k, v);
        }
        // Set cwd to HOME to avoid monorepo workspace conflicts with npx
        if let Ok(home) = std::env::var("HOME") {
            test_cmd.current_dir(&home);
        }
        test_cmd.stdin(std::process::Stdio::piped());
        test_cmd.stdout(std::process::Stdio::piped());
        test_cmd.stderr(std::process::Stdio::piped());

        let mut child = test_cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn '{}': {} (check PATH)", command, e))?;

        log::info!("MCP stdio: child process spawned (pid: {:?})", child.id());

        // Give the process a moment to start, then check if it's still running
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        match child.try_wait() {
            Ok(Some(status)) => {
                // Process already exited — read stderr for error info
                let stderr = if let Some(mut stderr) = child.stderr.take() {
                    use tokio::io::AsyncReadExt;
                    let mut buf = String::new();
                    let _ = stderr.read_to_string(&mut buf).await;
                    buf
                } else {
                    String::new()
                };
                let stdout = if let Some(mut stdout) = child.stdout.take() {
                    use tokio::io::AsyncReadExt;
                    let mut buf = String::new();
                    let _ = stdout.read_to_string(&mut buf).await;
                    buf
                } else {
                    String::new()
                };
                log::error!("MCP stdio: process exited immediately with {}", status);
                if !stderr.is_empty() {
                    log::error!(
                        "MCP stdio stderr: {}",
                        stderr.chars().take(500).collect::<String>()
                    );
                }
                if !stdout.is_empty() {
                    log::error!(
                        "MCP stdio stdout: {}",
                        stdout.chars().take(500).collect::<String>()
                    );
                }
                return Err(format!(
                    "Process '{}' exited immediately ({}). stderr: {}",
                    command,
                    status,
                    stderr.chars().take(200).collect::<String>()
                ));
            }
            Ok(None) => {
                log::info!("MCP stdio: process is running, proceeding with rmcp handshake");
            }
            Err(e) => {
                log::warn!("MCP stdio: couldn't check process status: {}", e);
            }
        }

        // Kill the test child — we'll respawn via rmcp
        let _ = child.kill().await;

        // Spawn via rmcp's TokioChildProcess using direct command builder
        let mut cmd = tokio::process::Command::new(command);
        cmd.args(args);
        for (k, v) in env {
            cmd.env(k, v);
        }
        // Set cwd to HOME to avoid monorepo workspace conflicts with npx
        if let Ok(home) = std::env::var("HOME") {
            cmd.current_dir(home);
        }

        let transport = TokioChildProcess::new(cmd)
            .map_err(|e| format!("Failed to spawn MCP child process '{}': {}", command, e))?;

        let service = ()
            .serve(transport)
            .await
            .map_err(|e| format!("Failed to initialize MCP service: {}", e))?;

        let tools = Self::fetch_tools(&service).await?;

        Ok((service, tools))
    }

    /// Connect to an HTTP SSE MCP server
    async fn connect_http_sse(
        url: &str,
        _headers: &HashMap<String, String>,
    ) -> Result<(RunningService<rmcp::RoleClient, ()>, Vec<MCPToolInfo>), String> {
        use rmcp::transport::streamable_http_client::{
            StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
        };

        let config = StreamableHttpClientTransportConfig {
            uri: Arc::from(url),
            ..Default::default()
        };
        let transport = StreamableHttpClientTransport::with_client(reqwest::Client::new(), config);

        let service: RunningService<rmcp::RoleClient, ()> = ()
            .serve(transport)
            .await
            .map_err(|e| format!("Failed to initialize MCP HTTP SSE service: {}", e))?;

        let tools = Self::fetch_tools(&service).await?;

        Ok((service, tools))
    }

    /// Fetch tools from a connected service
    async fn fetch_tools(
        service: &RunningService<rmcp::RoleClient, ()>,
    ) -> Result<Vec<MCPToolInfo>, String> {
        let tools_result = service
            .list_tools(Default::default())
            .await
            .map_err(|e| format!("Failed to list tools: {}", e))?;

        let tools: Vec<MCPToolInfo> = tools_result
            .tools
            .into_iter()
            .map(|t| MCPToolInfo {
                name: t.name.to_string(),
                description: t.description.as_deref().unwrap_or("").to_string(),
                input_schema: serde_json::to_value(&*t.input_schema).unwrap_or_default(),
            })
            .collect();

        Ok(tools)
    }

    /// Save the current config to disk
    fn save_config(&self) -> Result<(), String> {
        if self.config_path.as_os_str().is_empty() {
            return Err("Config path not set".to_string());
        }

        let configs: Vec<MCPServerConfig> =
            self.servers.values().map(|s| s.config.clone()).collect();

        let config_file = MCPConfigFile { servers: configs };
        let yaml = serde_yaml::to_string(&config_file)
            .map_err(|e| format!("Failed to serialize MCP config: {}", e))?;

        std::fs::write(&self.config_path, yaml)
            .map_err(|e| format!("Failed to write MCP config: {}", e))?;

        log::info!("MCP config saved to {:?}", self.config_path);
        Ok(())
    }

    /// Add a new server, persist to config, and attempt connection (non-fatal if connection fails)
    pub async fn add_server(&mut self, config: MCPServerConfig) -> Result<(), String> {
        if self.servers.contains_key(&config.id) {
            return Err(format!("Server '{}' already exists", config.id));
        }

        // Always add and save first, then attempt connection
        if config.enabled {
            self.connect_server(config, "global").await?;
        } else {
            self.servers.insert(
                config.id.clone(),
                MCPServerState {
                    config,
                    service: None,
                    tools: vec![],
                    status: ServerStatus::Disconnected,
                    scope: "global".to_string(),
                },
            );
        }
        self.save_config()?;
        Ok(())
    }

    /// Remove a server, disconnect it, and persist to config
    pub async fn remove_server(&mut self, server_id: &str) -> Result<(), String> {
        if let Some(mut state) = self.servers.remove(server_id) {
            if let Some(ref mut service) = state.service {
                if state.status == ServerStatus::Connected {
                    if let Err(e) = service.close().await {
                        log::warn!("Error closing MCP service {}: {}", server_id, e);
                    }
                }
            }
            self.save_config()?;
            Ok(())
        } else {
            Err(format!("Server '{}' not found", server_id))
        }
    }

    /// Toggle a server enabled/disabled. If disabling, disconnect.
    pub async fn toggle_server(&mut self, server_id: &str, enabled: bool) -> Result<(), String> {
        if let Some(state) = self.servers.get_mut(server_id) {
            state.config.enabled = enabled;
            if !enabled && state.status == ServerStatus::Connected {
                if let Some(ref mut service) = state.service {
                    if let Err(e) = service.close().await {
                        log::warn!("Error closing MCP service {}: {}", server_id, e);
                    }
                }
                state.service = None;
                state.status = ServerStatus::Disconnected;
                state.tools.clear();
            }
            self.save_config()?;
            Ok(())
        } else {
            Err(format!("Server '{}' not found", server_id))
        }
    }

    /// Reconnect a server (disconnect then re-connect)
    pub async fn reconnect_server(&mut self, server_id: &str) -> Result<(), String> {
        let (config, scope) = self
            .servers
            .get(server_id)
            .map(|s| (s.config.clone(), s.scope.clone()))
            .ok_or_else(|| format!("Server '{}' not found", server_id))?;

        // Disconnect existing
        if let Some(mut state) = self.servers.remove(server_id) {
            if let Some(ref mut service) = state.service {
                if state.status == ServerStatus::Connected {
                    let _ = service.close().await;
                }
            }
        }

        // Re-connect
        self.connect_server(config, &scope).await
    }

    /// Call a tool on a specific MCP server
    pub async fn call_tool(
        &self,
        server_id: &str,
        tool_name: &str,
        arguments: Value,
    ) -> Result<String, String> {
        let state = self.servers.get(server_id);

        // Server not found - return detailed diagnostic
        let state = match state {
            Some(s) => s,
            None => {
                let error = super::error::MCPToolError::server_not_found(server_id);
                return Err(error.to_agent_message());
            }
        };

        // Server not connected - return detailed diagnostic
        if state.status != ServerStatus::Connected {
            let last_error = match &state.status {
                ServerStatus::Error(e) => Some(e.clone()),
                _ => None,
            };

            let error = super::error::MCPToolError::server_not_connected(
                server_id,
                &state.config,
                last_error,
            );
            return Err(error.to_agent_message());
        }

        let service = state.service.as_ref().ok_or_else(|| {
            let error = super::error::MCPToolError::tool_call_failed(
                server_id,
                &state.config.name,
                tool_name,
                "Service object is None",
            );
            error.to_agent_message()
        })?;

        let args_obj = arguments.as_object().cloned();

        let result = service
            .call_tool(CallToolRequestParams {
                meta: None,
                name: tool_name.to_string().into(),
                arguments: args_obj,
                task: None,
            })
            .await
            .map_err(|e| {
                let error = super::error::MCPToolError::tool_call_failed(
                    server_id,
                    &state.config.name,
                    tool_name,
                    &e.to_string(),
                );
                error.to_agent_message()
            })?;

        if result.is_error == Some(true) {
            let error_text: String = result
                .content
                .iter()
                .map(|c| {
                    use std::ops::Deref;
                    match c.deref() {
                        rmcp::model::RawContent::Text(t) => t.text.clone(),
                        other => format!("{:?}", other),
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            return Err(format!("MCP tool error: {}", error_text));
        }

        let text: String = result
            .content
            .iter()
            .map(|c| {
                use std::ops::Deref;
                match c.deref() {
                    rmcp::model::RawContent::Text(t) => t.text.clone(),
                    rmcp::model::RawContent::Image(_) => "[Image content]".to_string(),
                    rmcp::model::RawContent::Resource(_) => "[Resource content]".to_string(),
                    rmcp::model::RawContent::Audio(_) => "[Audio content]".to_string(),
                    rmcp::model::RawContent::ResourceLink(r) => format!("[Resource: {}]", r.uri),
                }
            })
            .collect::<Vec<_>>()
            .join("\n\n");

        Ok(text)
    }

    /// List all servers and their status
    pub fn list_servers(&self) -> Vec<MCPServerInfo> {
        self.servers
            .values()
            .map(|state| {
                let (status_str, error_str) = match &state.status {
                    ServerStatus::Connected => ("connected".to_string(), None),
                    ServerStatus::Disconnected => ("disconnected".to_string(), None),
                    ServerStatus::Error(e) => ("error".to_string(), Some(e.clone())),
                };
                MCPServerInfo {
                    id: state.config.id.clone(),
                    name: state.config.name.clone(),
                    enabled: state.config.enabled,
                    tool_name_prefix: sanitize_for_tool_name(&state.config.name),
                    transport: match &state.config.transport {
                        MCPTransport::Stdio { .. } => "stdio".to_string(),
                        MCPTransport::HttpSse { .. } => "http_sse".to_string(),
                    },
                    command: match &state.config.transport {
                        MCPTransport::Stdio { command, .. } => Some(command.clone()),
                        _ => None,
                    },
                    args: match &state.config.transport {
                        MCPTransport::Stdio { args, .. } => Some(args.clone()),
                        _ => None,
                    },
                    url: match &state.config.transport {
                        MCPTransport::HttpSse { url, .. } => Some(url.clone()),
                        _ => None,
                    },
                    status: status_str,
                    tool_count: state.tools.len(),
                    error: error_str,
                    scope: state.scope.clone(),
                }
            })
            .collect()
    }

    /// Get all tools from a specific server
    pub fn get_server_tools(&self, server_id: &str) -> Vec<MCPToolInfo> {
        self.servers
            .get(server_id)
            .map(|s| s.tools.clone())
            .unwrap_or_default()
    }

    /// Get all tools from all connected servers, as (server_id, ToolConfig) pairs.
    /// The tool ID uses server NAME for LLM readability (e.g. `mcp__filesystem__read_file`).
    pub fn get_all_tool_configs(&self) -> Vec<(String, String, ToolConfig)> {
        let mut configs = Vec::new();
        for (server_id, state) in &self.servers {
            if state.status != ServerStatus::Connected {
                continue;
            }
            for tool in &state.tools {
                let tool_name = tool.name.clone();
                configs.push((
                    server_id.clone(),
                    tool_name,
                    ToolConfig {
                        id: format!(
                            "mcp__{}__{}",
                            sanitize_for_tool_name(&state.config.name),
                            sanitize_for_tool_name(&tool.name)
                        ),
                        name: format!("{} (MCP:{})", tool.name, state.config.name),
                        description: tool.description.clone(),
                        category: ToolCategory::MCP,
                        input_schema: tool.input_schema.clone(),
                        instructions: format!(
                            "MCP Tool from server '{}'\n\n{}",
                            state.config.name, tool.description
                        ),
                        supports_background: false,
                        should_defer: false, // MCP tools auto-defer via category check
                        always_load: false,
                        search_hint: None,
                        is_read_only: false,
                        is_concurrency_safe: false,
                    },
                ));
            }
        }
        configs
    }

    /// Get connected server count
    pub fn server_count(&self) -> usize {
        self.servers.len()
    }

    /// Get the server IDs of all connected servers
    pub fn connected_server_ids(&self) -> Vec<String> {
        self.servers
            .iter()
            .filter(|(_, s)| s.status == ServerStatus::Connected)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Get the display name for a server by its ID
    pub fn get_server_name(&self, server_id: &str) -> Option<String> {
        self.servers.get(server_id).map(|s| s.config.name.clone())
    }

    /// Set config path (for when we create the registry before loading)
    pub fn set_config_path(&mut self, path: PathBuf) {
        self.config_path = path;
    }
}

impl Default for MCPRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Server info for API responses (matches frontend MCPServerItem type)
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MCPServerInfo {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub transport: String,
    /// Sanitized name used as prefix in tool names (e.g., "filesystem" for tool "mcp__filesystem__read_file")
    /// Frontend should use this value for MCP selection/filtering, NOT the server id.
    pub tool_name_prefix: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Status as a plain string: "connected", "disconnected", or "error"
    pub status: String,
    /// Config layer this server came from: "global" or "project".
    pub scope: String,
    pub tool_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn load_scoped_configs_merges_project_over_global_without_dropping_disabled_servers() {
        let temp = tempfile::tempdir().expect("tempdir");
        let global_path = temp.path().join("mcp_servers.yaml");
        let project_dir = temp.path().join("work").join(".cteno");
        let project_path = project_dir.join("mcp_servers.yaml");
        std::fs::create_dir_all(&project_dir).expect("project dir");

        std::fs::write(
            &global_path,
            r#"
servers:
  - id: shared
    name: global-shared
    enabled: false
    transport:
      type: stdio
      command: global-command
  - id: global-only
    name: global-only
    enabled: false
    transport:
      type: stdio
      command: global-only-command
"#,
        )
        .expect("global yaml");

        std::fs::write(
            &project_path,
            r#"
servers:
  - id: shared
    name: project-shared
    enabled: false
    transport:
      type: stdio
      command: project-command
"#,
        )
        .expect("project yaml");

        let mut registry = MCPRegistry::new();
        registry
            .load_from_scoped_configs(&global_path, Some(project_path.as_path()))
            .await
            .expect("load scoped config");

        let mut servers = registry.list_servers();
        servers.sort_by(|a, b| a.id.cmp(&b.id));

        assert_eq!(servers.len(), 2);
        let global_only = servers
            .iter()
            .find(|server| server.id == "global-only")
            .unwrap();
        assert_eq!(global_only.scope, "global");
        assert_eq!(global_only.status, "disconnected");

        let shared = servers.iter().find(|server| server.id == "shared").unwrap();
        assert_eq!(shared.name, "project-shared");
        assert_eq!(shared.command.as_deref(), Some("project-command"));
        assert_eq!(shared.scope, "project");
        assert_eq!(shared.status, "disconnected");
    }

    /// Test that rmcp can connect to a stdio MCP server (filesystem)
    /// and successfully complete the protocol handshake + list tools.
    #[tokio::test]
    async fn test_rmcp_stdio_connect_filesystem() {
        // Check if npx is available
        let npx_check = tokio::process::Command::new("npx")
            .arg("--version")
            .current_dir(std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string()))
            .output()
            .await;

        if npx_check.is_err() || !npx_check.unwrap().status.success() {
            eprintln!("Skipping test: npx not available");
            return;
        }

        let result = MCPRegistry::connect_stdio(
            "npx",
            &[
                "-y".to_string(),
                "@modelcontextprotocol/server-filesystem".to_string(),
                "/tmp".to_string(),
            ],
            &HashMap::new(),
        )
        .await;

        match result {
            Ok((mut service, tools)) => {
                println!("MCP connection successful! {} tools found", tools.len());
                for tool in &tools {
                    println!("  - {} : {}", tool.name, tool.description);
                }
                assert!(
                    !tools.is_empty(),
                    "Filesystem MCP server should expose at least one tool"
                );
                let _ = service.close().await;
            }
            Err(e) => {
                panic!("MCP connection failed: {}", e);
            }
        }
    }
}
