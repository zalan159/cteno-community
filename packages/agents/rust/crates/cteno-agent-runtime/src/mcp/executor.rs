//! MCP Tool Executor — rmcp-based
//!
//! Implements ToolExecutor trait for MCP tools.
//! Routes tool calls through the MCPRegistry.

use crate::mcp::MCPRegistry;
use crate::tool::ToolExecutor;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::RwLock;

/// MCP Tool Executor
///
/// Routes tool execution to the appropriate MCP server via the shared MCPRegistry.
pub struct MCPToolExecutor {
    registry: Arc<RwLock<MCPRegistry>>,
    server_id: String,
    tool_name: String,
}

impl MCPToolExecutor {
    /// Create a new MCP tool executor
    pub fn new(registry: Arc<RwLock<MCPRegistry>>, server_id: String, tool_name: String) -> Self {
        Self {
            registry,
            server_id,
            tool_name,
        }
    }
}

#[async_trait]
impl ToolExecutor for MCPToolExecutor {
    async fn execute(&self, input: serde_json::Value) -> Result<String, String> {
        let registry = self.registry.read().await;
        registry
            .call_tool(&self.server_id, &self.tool_name, input)
            .await
    }

    fn supports_background(&self) -> bool {
        false
    }
}
