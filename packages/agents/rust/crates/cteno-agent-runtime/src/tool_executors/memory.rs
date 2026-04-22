//! Memory Executor
//!
//! Executes persistent memory operations: save, recall, read, list.
//! Supports persona-scoped private memory via __persona_workdir injection.
use crate::tool::ToolExecutor;
use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;

/// Memory operation executor
pub struct MemoryExecutor {
    /// Workspace directory (where memory files are stored)
    workspace_dir: PathBuf,
}

impl MemoryExecutor {
    pub fn new(workspace_dir: PathBuf) -> Self {
        Self { workspace_dir }
    }

    /// Extract persona context from tool input.
    /// Returns (persona_workdir, is_global).
    /// - If no __persona_workdir, everything is global.
    /// - If __persona_workdir present, scope defaults to "private" unless explicitly "global".
    fn resolve_persona_scope(input: &Value) -> (Option<String>, bool) {
        let persona_workdir = input
            .get("__persona_workdir")
            .and_then(|v| v.as_str())
            .map(String::from);
        let scope = input
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or("private");
        let is_global = scope == "global" || persona_workdir.is_none();
        (persona_workdir, is_global)
    }

    /// Look up owner name from __owner_id/__persona_id for display labels.
    fn resolve_persona_name(input: &Value) -> Option<String> {
        let owner_id = input
            .get("__owner_id")
            .or_else(|| input.get("__persona_id"))
            .and_then(|v| v.as_str())?;
        crate::hooks::agent_owner().and_then(|p| p.resolve_owner_name(owner_id))
    }

    fn save(&self, input: &Value) -> Result<String, String> {
        let file_path = input
            .get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("knowledge/general.md");
        let content = input
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: content (for save action)")?;

        let memory_type = input.get("type").and_then(|v| v.as_str());

        let content_with_meta = if let Some(t) = memory_type {
            let date = chrono::Local::now().format("%Y-%m-%d").to_string();
            format!("---\ntype: {}\ndate: {}\n---\n{}", t, date, content)
        } else {
            content.to_string()
        };

        let (persona_workdir, is_global) = Self::resolve_persona_scope(input);
        let workdir = if is_global {
            None
        } else {
            persona_workdir.as_deref()
        };

        cteno_community_core::memory::memory_append_core(
            &self.workspace_dir,
            file_path,
            &content_with_meta,
            workdir,
        )?;

        let scope_label = if workdir.is_some() {
            let name = Self::resolve_persona_name(input);
            format!("private:{}", name.unwrap_or_else(|| "?".to_string()))
        } else {
            "global".to_string()
        };
        Ok(format!("Saved to {} [{}]", file_path, scope_label))
    }

    fn recall(&self, input: &Value) -> Result<String, String> {
        let query = input
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: query (for recall action)")?;

        let type_filter = input.get("type").and_then(|v| v.as_str());

        // recall always searches both private + global
        let (persona_workdir, _) = Self::resolve_persona_scope(input);
        let results = cteno_community_core::memory::memory_search_core(
            &self.workspace_dir,
            query,
            persona_workdir.as_deref(),
            10,
            type_filter,
        )?;

        if results.is_empty() {
            return Ok("No matching memories found.".to_string());
        }

        // Replace [private] with [private:name] in search results
        let persona_name = Self::resolve_persona_name(input);
        let mut output = format!("Found {} results:\n\n", results.len());
        for chunk in &results {
            let tagged_path = if let Some(ref name) = persona_name {
                chunk
                    .file_path
                    .replace("[private]", &format!("[private:{}]", name))
            } else {
                chunk.file_path.clone()
            };
            output.push_str(&format!(
                "--- {} (score: {:.2}) ---\n{}\n\n",
                tagged_path, chunk.score, chunk.content
            ));
        }

        Ok(output)
    }

    fn read(&self, input: &Value) -> Result<String, String> {
        let file_path = input
            .get("file_path")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: file_path (for read action)")?;

        let (persona_workdir, is_global) = Self::resolve_persona_scope(input);
        let workdir = if is_global {
            None
        } else {
            persona_workdir.as_deref()
        };

        match cteno_community_core::memory::memory_read_core(
            &self.workspace_dir,
            file_path,
            workdir,
        )? {
            Some(content) => Ok(content),
            None => Ok(format!("File not found: {}", file_path)),
        }
    }

    fn list(&self, input: &Value) -> Result<String, String> {
        let (persona_workdir, is_global) = Self::resolve_persona_scope(input);
        let workdir = if is_global {
            None
        } else {
            persona_workdir.as_deref()
        };

        let files = cteno_community_core::memory::memory_list_core(&self.workspace_dir, workdir)?;

        if files.is_empty() {
            return Ok("No memory files found.".to_string());
        }

        // Replace [private] with [private:name] in file list
        let persona_name = Self::resolve_persona_name(input);
        let mut output = format!("{} memory files:\n", files.len());
        for f in &files {
            let tagged = if let Some(ref name) = persona_name {
                f.replace("[private]", &format!("[private:{}]", name))
            } else {
                f.clone()
            };
            output.push_str(&format!("- {}\n", tagged));
        }

        Ok(output)
    }
}

#[async_trait]
impl ToolExecutor for MemoryExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        let action = input
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: action")?;

        match action {
            "save" => self.save(&input),
            "recall" => self.recall(&input),
            "read" => self.read(&input),
            "list" => self.list(&input),
            _ => Err(format!(
                "Unknown memory action: {}. Use: save, recall, read, list",
                action
            )),
        }
    }
}
