//! File Write Tool Executor
//!
//! Creates or overwrites files. Automatically creates parent directories.
//! Includes staleness detection: rejects writes to files that were modified
//! externally (by linters, formatters, or the user) since the agent last read them.

use crate::tool::ToolExecutor;
use crate::tool_executors::{path_resolver, sandbox};
use async_trait::async_trait;
use std::fs;
use std::path::PathBuf;

/// File Write Executor
pub struct WriteExecutor;

impl WriteExecutor {
    pub fn new() -> Self {
        Self
    }

    fn resolve_file_path(path: &str, workdir: Option<&str>) -> Result<PathBuf, String> {
        path_resolver::resolve_file_path(path, workdir)
    }

    /// Canonical path string for the file tracker key.
    /// Falls back to the resolved path if canonicalize fails (e.g. new file).
    fn canonical_path_str(path: &PathBuf) -> String {
        fs::canonicalize(path)
            .unwrap_or_else(|_| path.clone())
            .to_string_lossy()
            .to_string()
    }
}

#[async_trait]
impl ToolExecutor for WriteExecutor {
    async fn execute(&self, input: serde_json::Value) -> Result<String, String> {
        let path = input
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: path")?;

        let content = input
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: content")?;

        let sandbox_ctx = sandbox::SandboxContext::from_input(&input);
        let file_path = path_resolver::resolve_file_path_sandboxed(
            path,
            input.get("workdir").and_then(|v| v.as_str()),
            &sandbox_ctx,
            true,
        )?;

        let session_id = input.get("__session_id").and_then(|v| v.as_str());

        // Check if file already exists (for reporting and staleness check)
        let already_exists = file_path.exists();

        // --- Staleness detection ---
        let mut warning: Option<String> = None;

        if already_exists {
            if let Some(sid) = session_id {
                let canonical = Self::canonical_path_str(&file_path);

                if let Some(read_mtime) = super::file_tracker::get_file_read_time(sid, &canonical) {
                    // File was previously read by this session — check for external modifications.
                    if let Ok(metadata) = fs::metadata(&file_path) {
                        if let Ok(current_mtime) = metadata.modified() {
                            if current_mtime > read_mtime {
                                return Err(format!(
                                    "FILE_MODIFIED_SINCE_READ: The file '{}' has been modified since you last read it (possibly by a linter, formatter, or the user). Please read the file again before writing to ensure you have the latest content.",
                                    file_path.display()
                                ));
                            }
                        }
                    }
                } else {
                    // File exists but was never read by this session — warn but allow.
                    warning = Some(format!(
                        "Warning: Writing to existing file '{}' without reading it first. Consider reading the file first to avoid overwriting important content.",
                        file_path.display()
                    ));
                }
            }
        }

        // Create parent directories if needed
        if let Some(parent) = file_path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).map_err(|e| {
                    format!("Failed to create directory '{}': {}", parent.display(), e)
                })?;
            }
        }

        // Write the file
        fs::write(&file_path, content)
            .map_err(|e| format!("Failed to write file '{}': {}", file_path.display(), e))?;

        // Update the file tracker with the new mtime so subsequent writes
        // don't trigger a false-positive staleness error.
        if let Some(sid) = session_id {
            if let Ok(metadata) = fs::metadata(&file_path) {
                if let Ok(new_mtime) = metadata.modified() {
                    let canonical = Self::canonical_path_str(&file_path);
                    super::file_tracker::record_file_read(sid, &canonical, new_mtime);
                }
            }
        }

        let line_count = content.lines().count();
        let byte_count = content.len();

        let mut result = if already_exists {
            format!(
                "File overwritten: {} ({} lines, {} bytes)",
                file_path.display(),
                line_count,
                byte_count
            )
        } else {
            format!(
                "File created: {} ({} lines, {} bytes)",
                file_path.display(),
                line_count,
                byte_count
            )
        };

        if let Some(warn) = warning {
            result.push('\n');
            result.push_str(&warn);
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_write_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("new_file.txt");

        let executor = WriteExecutor::new();
        let result = executor
            .execute(json!({
                "path": file_path.to_str().unwrap(),
                "content": "hello world\n"
            }))
            .await;

        assert!(result.is_ok());
        assert!(result.unwrap().contains("File created"));
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "hello world\n");
    }

    #[tokio::test]
    async fn test_write_overwrite_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("existing.txt");
        fs::write(&file_path, "old content").unwrap();

        let executor = WriteExecutor::new();
        let result = executor
            .execute(json!({
                "path": file_path.to_str().unwrap(),
                "content": "new content\n"
            }))
            .await;

        assert!(result.is_ok());
        assert!(result.unwrap().contains("File overwritten"));
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "new content\n");
    }

    #[tokio::test]
    async fn test_write_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("a").join("b").join("c").join("file.txt");

        let executor = WriteExecutor::new();
        let result = executor
            .execute(json!({
                "path": file_path.to_str().unwrap(),
                "content": "nested file\n"
            }))
            .await;

        assert!(result.is_ok());
        assert!(result.unwrap().contains("File created"));
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "nested file\n");
    }

    #[tokio::test]
    async fn test_write_respects_workdir_for_relative_path() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("nested").join("file.txt");

        let executor = WriteExecutor::new();
        let result = executor
            .execute(json!({
                "path": "nested/file.txt",
                "content": "from workdir\n",
                "workdir": dir.path().to_str().unwrap()
            }))
            .await;

        assert!(result.is_ok());
        assert!(result.unwrap().contains("File created"));
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "from workdir\n");
    }

    #[tokio::test]
    async fn test_write_rejects_relative_escape() {
        let dir = tempfile::tempdir().unwrap();
        let executor = WriteExecutor::new();
        let result = executor
            .execute(json!({
                "path": "../escape.txt",
                "content": "nope\n",
                "workdir": dir.path().to_str().unwrap()
            }))
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("escapes workdir"));
    }

    #[tokio::test]
    async fn test_write_rejects_stale_file() {
        use std::time::{Duration, SystemTime};

        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("stale.txt");
        fs::write(&file_path, "original").unwrap();

        let canonical = fs::canonicalize(&file_path).unwrap();
        let canonical_str = canonical.to_string_lossy().to_string();

        // Simulate: agent read the file a while ago
        let old_time = SystemTime::now() - Duration::from_secs(60);
        super::super::file_tracker::record_file_read("stale-test-sess", &canonical_str, old_time);

        // File was modified *after* the recorded read (its real mtime is "now")
        let executor = WriteExecutor::new();
        let result = executor
            .execute(json!({
                "path": file_path.to_str().unwrap(),
                "content": "overwrite attempt\n",
                "__session_id": "stale-test-sess"
            }))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("FILE_MODIFIED_SINCE_READ"),
            "Expected staleness error, got: {}",
            err
        );
        // Original content should be untouched
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "original");
    }

    #[tokio::test]
    async fn test_write_allows_after_fresh_read() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("fresh.txt");
        fs::write(&file_path, "original").unwrap();

        let canonical = fs::canonicalize(&file_path).unwrap();
        let canonical_str = canonical.to_string_lossy().to_string();

        // Record the current mtime (simulates a fresh read)
        let mtime = fs::metadata(&file_path).unwrap().modified().unwrap();
        super::super::file_tracker::record_file_read("fresh-test-sess", &canonical_str, mtime);

        let executor = WriteExecutor::new();
        let result = executor
            .execute(json!({
                "path": file_path.to_str().unwrap(),
                "content": "updated content\n",
                "__session_id": "fresh-test-sess"
            }))
            .await;

        assert!(result.is_ok(), "Expected success, got: {:?}", result);
        assert!(result.unwrap().contains("File overwritten"));
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "updated content\n");
    }

    #[tokio::test]
    async fn test_write_warns_without_prior_read() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("no_read.txt");
        fs::write(&file_path, "existing").unwrap();

        let executor = WriteExecutor::new();
        let result = executor
            .execute(json!({
                "path": file_path.to_str().unwrap(),
                "content": "overwritten\n",
                "__session_id": "no-read-test-sess"
            }))
            .await;

        // Should succeed but include a warning
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(
            output.contains("Warning"),
            "Expected warning, got: {}",
            output
        );
        assert!(output.contains("without reading"));
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "overwritten\n");
    }

    #[tokio::test]
    async fn test_write_no_session_id_no_check() {
        // Without __session_id, staleness check is skipped entirely (backward compat)
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("no_session.txt");
        fs::write(&file_path, "existing").unwrap();

        let executor = WriteExecutor::new();
        let result = executor
            .execute(json!({
                "path": file_path.to_str().unwrap(),
                "content": "overwritten\n"
            }))
            .await;

        assert!(result.is_ok());
        assert!(result.unwrap().contains("File overwritten"));
    }
}
