//! Tool Loader
//!
//! Loads tools from the tools/ directory by parsing TOOL.md files.

use crate::tool::{ToolCategory, ToolConfig};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Tool loader
pub struct ToolLoader {
    tools_dir: PathBuf,
}

/// TOOL.md YAML frontmatter structure
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ToolFrontmatter {
    id: String,
    name: String,
    description: String,
    category: String,
    version: String,
    #[serde(default)]
    supports_background: bool,
    /// Input schema as YAML (will be converted to JSON)
    input_schema: Option<serde_json::Value>,
    /// Whether this tool should be deferred (loaded on-demand via tool_search)
    #[serde(default)]
    should_defer: bool,
    /// Force always-load (override should_defer and MCP auto-defer)
    #[serde(default)]
    always_load: bool,
    /// 3-10 word keyword hint for ToolSearch fuzzy matching
    #[serde(default)]
    search_hint: Option<String>,
    #[serde(default)]
    is_read_only: bool,
    #[serde(default)]
    is_concurrency_safe: bool,
}

impl ToolLoader {
    /// Create a new loader for the given tools directory
    pub fn new(tools_dir: PathBuf) -> Self {
        Self { tools_dir }
    }

    /// Load all tools from the directory
    pub fn load_all(&self) -> Result<Vec<ToolConfig>, String> {
        if !self.tools_dir.exists() {
            log::warn!("Tools directory does not exist: {:?}", self.tools_dir);
            return Ok(Vec::new());
        }

        let mut tools = Vec::new();

        for entry in fs::read_dir(&self.tools_dir)
            .map_err(|e| format!("Failed to read tools directory: {}", e))?
        {
            let entry = entry.map_err(|e| format!("Failed to read directory entry: {}", e))?;
            let path = entry.path();

            if path.is_dir() {
                let tool_md = path.join("TOOL.md");
                if tool_md.exists() {
                    match self.load_tool_from_file(&tool_md) {
                        Ok(tool) => {
                            log::info!("Loaded tool: {} from {:?}", tool.id, path);
                            tools.push(tool);
                        }
                        Err(e) => {
                            log::error!("Failed to load tool from {:?}: {}", path, e);
                        }
                    }
                }
            }
        }

        Ok(tools)
    }

    /// Load a single tool from a TOOL.md file
    fn load_tool_from_file(&self, path: &PathBuf) -> Result<ToolConfig, String> {
        let content =
            fs::read_to_string(path).map_err(|e| format!("Failed to read file: {}", e))?;

        // Split into YAML frontmatter and Markdown body
        let parts: Vec<&str> = content.splitn(3, "---").collect();
        if parts.len() < 3 {
            return Err("Invalid TOOL.md format: missing YAML frontmatter".to_string());
        }

        let yaml_str = parts[1].trim();
        let instructions = parts[2].trim().to_string();

        // Parse YAML frontmatter
        let frontmatter: ToolFrontmatter = serde_yaml::from_str(yaml_str)
            .map_err(|e| format!("Failed to parse YAML frontmatter: {}", e))?;

        // Parse category
        let category = match frontmatter.category.to_lowercase().as_str() {
            "system" => ToolCategory::System,
            "mcp" => ToolCategory::MCP,
            "subagent" => ToolCategory::SubAgent,
            "persona" => ToolCategory::Persona,
            _ => return Err(format!("Invalid tool category: {}", frontmatter.category)),
        };

        // Build default input schema if not provided
        let input_schema = frontmatter.input_schema.unwrap_or_else(|| {
            serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            })
        });

        Ok(ToolConfig {
            id: frontmatter.id,
            name: frontmatter.name,
            description: frontmatter.description,
            category,
            input_schema,
            instructions,
            supports_background: frontmatter.supports_background,
            should_defer: frontmatter.should_defer,
            always_load: frontmatter.always_load,
            search_hint: frontmatter.search_hint,
            is_read_only: frontmatter.is_read_only,
            is_concurrency_safe: frontmatter.is_concurrency_safe,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_load_tool_from_md() {
        let dir = tempdir().unwrap();
        let tool_dir = dir.path().join("test_tool");
        fs::create_dir(&tool_dir).unwrap();

        let tool_md = tool_dir.join("TOOL.md");
        fs::write(
            &tool_md,
            r#"---
id: "test_tool"
name: "Test Tool"
description: "A test tool"
category: "system"
version: "1.0.0"
supports_background: false
input_schema:
  type: object
  properties:
    param1:
      type: string
  required:
    - param1
---

# Test Tool

This is a test tool for unit testing.

## Usage

Call it with param1.
"#,
        )
        .unwrap();

        let loader = ToolLoader::new(dir.path().to_path_buf());
        let tools = loader.load_all().unwrap();

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].id, "test_tool");
        assert_eq!(tools[0].name, "Test Tool");
        assert_eq!(tools[0].category, ToolCategory::System);
        assert!(!tools[0].supports_background);
        assert!(tools[0].instructions.contains("This is a test tool"));
    }
}
