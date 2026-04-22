//! MCP (Model Context Protocol) Integration — rmcp-based
//!
//! This module provides integration with MCP servers using the official rmcp SDK,
//! supporting both stdio (child process) and streamable HTTP (SSE) transports.
//!
//! ## Architecture
//!
//! - `MCPRegistry`: Manages multiple MCP server connections via rmcp
//! - `MCPToolExecutor`: Implements ToolExecutor trait for MCP tools
//! - Config persisted in `mcp_servers.yaml`

pub mod error;
pub mod executor;
pub mod registry;

pub use error::{bun_not_found_error, command_not_found_error, MCPToolError};
pub use executor::MCPToolExecutor;
pub use registry::{sanitize_for_tool_name, MCPRegistry, MCPServerInfo};

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// MCP Server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MCPServerConfig {
    /// Unique identifier
    pub id: String,
    /// Display name
    pub name: String,
    /// Whether this server is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Transport configuration
    pub transport: MCPTransport,
}

fn default_true() -> bool {
    true
}

/// Transport configuration for an MCP server
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MCPTransport {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: HashMap<String, String>,
    },
    HttpSse {
        url: String,
        #[serde(default)]
        headers: HashMap<String, String>,
    },
}

/// Server connection status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ServerStatus {
    Connected,
    Disconnected,
    Error(String),
}

/// Persisted configuration file format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MCPConfigFile {
    #[serde(default)]
    pub servers: Vec<MCPServerConfig>,
}
