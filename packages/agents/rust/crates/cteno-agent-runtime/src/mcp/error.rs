//! MCP Error Diagnostics
//!
//! Provides structured error information for MCP tool failures,
//! including environment diagnostics and fix suggestions.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// MCP tool execution error with diagnostic information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MCPToolError {
    pub error_type: String,
    pub error_message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<MCPDiagnostic>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<EnvironmentInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fix_suggestions: Vec<FixSuggestion>,
}

/// MCP server diagnostic information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MCPDiagnostic {
    pub server_id: String,
    pub server_name: String,
    pub transport: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// System environment information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentInfo {
    pub os: String,
    pub arch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    pub available_commands: HashMap<String, bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_commands: Vec<String>,
}

/// Fix suggestion for resolving the error
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixSuggestion {
    pub title: String,
    pub description: String,
    pub commands: Vec<String>,
    pub auto_fixable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires_user_action: Option<bool>,
}

impl MCPToolError {
    /// Create a "server not connected" error
    pub fn server_not_connected(
        server_id: &str,
        config: &super::MCPServerConfig,
        last_error: Option<String>,
    ) -> Self {
        let env_info = detect_environment();
        let fix_suggestions = generate_fix_suggestions_for_server(config, &env_info);

        Self {
            error_type: "mcp_server_not_connected".to_string(),
            error_message: format!("MCP server '{}' is not connected", server_id),
            diagnostic: Some(MCPDiagnostic {
                server_id: server_id.to_string(),
                server_name: config.name.clone(),
                transport: match &config.transport {
                    super::MCPTransport::Stdio { .. } => "stdio".to_string(),
                    super::MCPTransport::HttpSse { .. } => "sse".to_string(),
                },
                command: match &config.transport {
                    super::MCPTransport::Stdio { command, .. } => Some(command.clone()),
                    _ => None,
                },
                args: match &config.transport {
                    super::MCPTransport::Stdio { args, .. } => Some(args.clone()),
                    _ => None,
                },
                status: "disconnected".to_string(),
                last_error,
            }),
            environment: Some(env_info),
            fix_suggestions,
        }
    }

    /// Create a "server not found" error
    pub fn server_not_found(server_id: &str) -> Self {
        Self {
            error_type: "mcp_server_not_found".to_string(),
            error_message: format!("MCP server '{}' not found in configuration", server_id),
            diagnostic: None,
            environment: Some(detect_environment()),
            fix_suggestions: vec![FixSuggestion {
                title: "Add MCP server to configuration".to_string(),
                description: format!(
                    "Server '{}' is not configured in mcp_servers.yaml",
                    server_id
                ),
                commands: vec![
                    "# Edit your mcp_servers.yaml file".to_string(),
                    format!("# Add configuration for server: {}", server_id),
                ],
                auto_fixable: false,
                estimated_time: None,
                requires_user_action: Some(true),
            }],
        }
    }

    /// Create a "tool call failed" error
    pub fn tool_call_failed(
        server_id: &str,
        server_name: &str,
        tool_name: &str,
        error: &str,
    ) -> Self {
        Self {
            error_type: "mcp_tool_call_failed".to_string(),
            error_message: format!("MCP tool '{}' call failed: {}", tool_name, error),
            diagnostic: Some(MCPDiagnostic {
                server_id: server_id.to_string(),
                server_name: server_name.to_string(),
                transport: "connected".to_string(),
                command: None,
                args: None,
                status: "error".to_string(),
                last_error: Some(error.to_string()),
            }),
            environment: None,
            fix_suggestions: vec![FixSuggestion {
                title: "Check tool arguments".to_string(),
                description: "Verify that the tool arguments are correct and the tool supports the requested operation".to_string(),
                commands: vec![
                    format!("# Tool: {}", tool_name),
                    format!("# Error: {}", error),
                    "# Review the tool's documentation for correct usage".to_string(),
                ],
                auto_fixable: false,
                estimated_time: None,
                requires_user_action: None,
            }],
        }
    }

    /// Convert to agent-readable message
    pub fn to_agent_message(&self) -> String {
        let json =
            serde_json::to_string_pretty(self).unwrap_or_else(|_| self.error_message.clone());

        format!(
            "Tool Execution Error (with diagnostic information):\n\n\
             {}\n\n\
             IMPORTANT: This error includes structured diagnostic information.\n\
             You can attempt to fix this automatically by:\n\
             1. Analyzing the environment information and fix_suggestions\n\
             2. For suggestions with auto_fixable=true, execute the suggested commands using the shell tool\n\
             3. After executing fixes, retry the original tool call\n\
             4. If fixes require user action (requires_user_action=true), explain to the user what they need to do\n\n\
             Example workflow:\n\
             - If Node.js is missing: execute 'brew install node' (if on macOS)\n\
             - If an npm package is missing: execute 'npm install -g <package>'\n\
             - Then retry the original tool call",
            json
        )
    }
}

/// Detect system environment
fn detect_environment() -> EnvironmentInfo {
    let os = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "unknown"
    };

    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else {
        "unknown"
    };

    // Check for common commands
    let commands_to_check = if cfg!(windows) {
        vec![
            "node", "npm", "npx", "python3", "pip3", "winget", "bun", "cargo", "git",
        ]
    } else {
        vec![
            "node", "npm", "npx", "python3", "pip3", "brew", "apt", "bun", "cargo",
        ]
    };
    let mut available_commands = HashMap::new();

    for cmd in commands_to_check {
        let exists = which::which(cmd).is_ok();
        available_commands.insert(cmd.to_string(), exists);
    }

    EnvironmentInfo {
        os: os.to_string(),
        arch: arch.to_string(),
        shell: std::env::var("SHELL").ok().or_else(|| {
            if cfg!(windows) {
                Some("powershell".to_string())
            } else {
                None
            }
        }),
        available_commands,
        required_commands: vec![],
    }
}

/// Generate fix suggestions for a server configuration
fn generate_fix_suggestions_for_server(
    config: &super::MCPServerConfig,
    env: &EnvironmentInfo,
) -> Vec<FixSuggestion> {
    let mut suggestions = Vec::new();

    if let super::MCPTransport::Stdio { command, args, .. } = &config.transport {
        // Node.js MCP servers
        if command == "npx" || command == "node" {
            if !env.available_commands.get("node").copied().unwrap_or(false) {
                suggestions.push(FixSuggestion {
                    title: "Install Node.js".to_string(),
                    description: "This MCP server requires Node.js runtime".to_string(),
                    commands: if env.os == "macos" {
                        vec!["brew install node".to_string()]
                    } else if env.os == "linux" {
                        vec!["sudo apt install nodejs npm".to_string()]
                    } else if env.os == "windows" {
                        vec!["winget install OpenJS.NodeJS".to_string()]
                    } else {
                        vec!["# Download from: https://nodejs.org/".to_string()]
                    },
                    auto_fixable: (env.os == "macos"
                        && env.available_commands.get("brew").copied().unwrap_or(false))
                        || env.os == "windows",
                    estimated_time: Some("2-5 minutes".to_string()),
                    requires_user_action: None,
                });
            }

            // Check if MCP server package needs installation
            if let Some(first_arg) = args.first() {
                if first_arg.starts_with("@modelcontextprotocol/") || first_arg.starts_with("@") {
                    suggestions.push(FixSuggestion {
                        title: format!("Install MCP server package: {}", first_arg),
                        description: "Install the required MCP server package globally".to_string(),
                        commands: vec![format!("npm install -g {}", first_arg)],
                        auto_fixable: true,
                        estimated_time: Some("30-60 seconds".to_string()),
                        requires_user_action: None,
                    });
                }
            }
        }

        // Python MCP servers
        if (command == "python3" || command == "python")
            && !env
                .available_commands
                .get("python3")
                .copied()
                .unwrap_or(false)
        {
            suggestions.push(FixSuggestion {
                title: "Install Python 3".to_string(),
                description: "This MCP server requires Python 3".to_string(),
                commands: if env.os == "macos" {
                    vec!["brew install python3".to_string()]
                } else if env.os == "linux" {
                    vec!["sudo apt install python3 python3-pip".to_string()]
                } else if env.os == "windows" {
                    vec!["winget install Python.Python.3.12".to_string()]
                } else {
                    vec!["# Download from: https://www.python.org/".to_string()]
                },
                auto_fixable: (env.os == "macos"
                    && env.available_commands.get("brew").copied().unwrap_or(false))
                    || env.os == "windows",
                estimated_time: Some("1-3 minutes".to_string()),
                requires_user_action: None,
            });
        }
    }

    // If no specific suggestions, provide generic guidance
    if suggestions.is_empty() {
        suggestions.push(FixSuggestion {
            title: "Check MCP server configuration".to_string(),
            description: "Verify the MCP server is correctly installed and configured".to_string(),
            commands: vec![
                "# Check your mcp_servers.yaml configuration".to_string(),
                format!("# Server: {}", config.name),
                format!("# Transport: {:?}", config.transport),
            ],
            auto_fixable: false,
            estimated_time: None,
            requires_user_action: Some(true),
        });
    }

    suggestions
}

/// Create a diagnostic error for Bun not found
pub fn bun_not_found_error() -> String {
    let env = detect_environment();
    let error = MCPToolError {
        error_type: "bun_not_found".to_string(),
        error_message: "Bun runtime is not installed".to_string(),
        diagnostic: None,
        environment: Some(env.clone()),
        fix_suggestions: vec![FixSuggestion {
            title: "Install Bun".to_string(),
            description: "Bun is a fast JavaScript runtime, required for executing Node.js skills"
                .to_string(),
            commands: if cfg!(windows) {
                vec!["powershell -c \"irm bun.sh/install.ps1 | iex\"".to_string()]
            } else {
                vec!["curl -fsSL https://bun.sh/install | bash".to_string()]
            },
            auto_fixable: true,
            estimated_time: Some("1-2 minutes".to_string()),
            requires_user_action: None,
        }],
    };
    error.to_agent_message()
}

/// Create a diagnostic error for command not found
pub fn command_not_found_error(command: &str, context: &str) -> String {
    let env = detect_environment();

    let (install_cmd, auto_fixable) = match command {
        "ffmpeg" => (
            if env.os == "macos" {
                "brew install ffmpeg"
            } else if env.os == "windows" {
                "winget install Gyan.FFmpeg"
            } else {
                "sudo apt install ffmpeg"
            },
            (env.os == "macos" && env.available_commands.get("brew").copied().unwrap_or(false))
                || env.os == "windows",
        ),
        "git" => (
            if env.os == "macos" {
                "brew install git"
            } else if env.os == "windows" {
                "winget install Git.Git"
            } else {
                "sudo apt install git"
            },
            (env.os == "macos" && env.available_commands.get("brew").copied().unwrap_or(false))
                || env.os == "windows",
        ),
        _ => ("# Install the required command", false),
    };

    let error = MCPToolError {
        error_type: "command_not_found".to_string(),
        error_message: format!("Command '{}' not found", command),
        diagnostic: None,
        environment: Some(env),
        fix_suggestions: vec![FixSuggestion {
            title: format!("Install {}", command),
            description: format!("{} is required for: {}", command, context),
            commands: vec![install_cmd.to_string()],
            auto_fixable,
            estimated_time: Some("1-3 minutes".to_string()),
            requires_user_action: None,
        }],
    };
    error.to_agent_message()
}
