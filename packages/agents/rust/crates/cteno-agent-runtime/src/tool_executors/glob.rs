//! Glob Executor
//!
//! Fast file pattern matching tool. Searches for files matching a glob pattern,
//! excludes common noise directories, and returns results sorted by mtime (newest first).

use crate::tool::ToolExecutor;
use crate::tool_executors::path_resolver;
use async_trait::async_trait;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Maximum number of results to return.
const MAX_RESULTS: usize = 100;

/// Timeout for glob operations to prevent blocking the thread pool.
const GLOB_TIMEOUT: Duration = Duration::from_secs(20);

/// Directories that are always excluded from glob searches.
const EXCLUDED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    ".next",
    "dist",
    "build",
    ".cache",
    "__pycache__",
];

pub struct GlobExecutor;

impl GlobExecutor {
    pub fn new() -> Self {
        Self
    }

    /// Check if a path contains any excluded directory component.
    fn is_excluded(path: &Path) -> bool {
        for component in path.components() {
            if let std::path::Component::Normal(name) = component {
                if let Some(name_str) = name.to_str() {
                    if EXCLUDED_DIRS.contains(&name_str) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Get the modification time of a file, returning epoch on error.
    fn get_mtime(path: &Path) -> SystemTime {
        std::fs::metadata(path)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH)
    }
}

impl Default for GlobExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolExecutor for GlobExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        let pattern = input["pattern"]
            .as_str()
            .ok_or("Missing 'pattern' parameter")?;

        if pattern.trim().is_empty() {
            return Err("pattern cannot be empty".to_string());
        }

        let workdir = input.get("workdir").and_then(|v| v.as_str());
        let path_param = input.get("path").and_then(|v| v.as_str());

        // Determine search root: explicit `path` param > injected workdir > home
        let search_root = if let Some(p) = path_param {
            let expanded = path_resolver::expand_tilde(p);
            if expanded.is_absolute() {
                path_resolver::normalize_lexical(&expanded)
            } else {
                let base = path_resolver::resolve_workdir(workdir);
                path_resolver::normalize_lexical(&base.join(expanded))
            }
        } else {
            path_resolver::resolve_workdir(workdir)
        };

        if !search_root.exists() {
            return Err(format!(
                "Search directory does not exist: {}",
                search_root.display()
            ));
        }

        if !search_root.is_dir() {
            return Err(format!(
                "Path is not a directory: {}",
                search_root.display()
            ));
        }

        // Build the full glob pattern by joining the search root with the user pattern.
        let full_pattern = search_root.join(pattern);
        let full_pattern_str = full_pattern
            .to_str()
            .ok_or("Invalid pattern: contains non-UTF8 characters")?
            .to_string();

        // Execute glob matching (blocking I/O, run on blocking thread with timeout)
        let search_root_clone = search_root.clone();
        let glob_future = tokio::task::spawn_blocking(move || {
            let mut files: Vec<(PathBuf, SystemTime)> = Vec::new();

            let entries = match glob::glob(&full_pattern_str) {
                Ok(paths) => paths,
                Err(e) => return Err(format!("Invalid glob pattern: {}", e)),
            };

            for entry in entries {
                match entry {
                    Ok(path) => {
                        // Skip directories themselves — we only want files
                        if path.is_dir() {
                            continue;
                        }

                        // Skip excluded directories
                        let relative = path.strip_prefix(&search_root_clone).unwrap_or(&path);
                        if GlobExecutor::is_excluded(relative) {
                            continue;
                        }

                        let mtime = GlobExecutor::get_mtime(&path);
                        files.push((path, mtime));
                    }
                    Err(_) => {
                        // Permission denied or other OS errors — skip silently
                        continue;
                    }
                }
            }

            // Sort by mtime descending (newest first)
            files.sort_by(|a, b| b.1.cmp(&a.1));

            Ok(files)
        });

        let result = match tokio::time::timeout(GLOB_TIMEOUT, glob_future).await {
            Ok(join_result) => join_result.map_err(|e| format!("Glob task failed: {}", e))??,
            Err(_) => {
                return Err(format!(
                    "Glob timed out after {}s. The pattern '{}' may be too broad — try a more specific pattern or narrower path.",
                    GLOB_TIMEOUT.as_secs(), pattern
                ));
            }
        };

        let total = result.len();

        if total == 0 {
            return Ok("No files found".to_string());
        }

        // Truncate to MAX_RESULTS
        let truncated = total > MAX_RESULTS;
        let shown: Vec<&(PathBuf, SystemTime)> = result.iter().take(MAX_RESULTS).collect();

        // Convert to relative paths
        let mut lines: Vec<String> = shown
            .iter()
            .map(|(path, _)| {
                path.strip_prefix(&search_root)
                    .unwrap_or(path)
                    .display()
                    .to_string()
            })
            .collect();

        if truncated {
            lines.push(format!(
                "\n[Truncated: showing {} of {} files. Use a more specific pattern to narrow results.]",
                MAX_RESULTS, total
            ));
        }

        Ok(lines.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_glob_finds_rust_files() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("main.rs"), "fn main() {}").unwrap();
        std::fs::write(src.join("lib.rs"), "pub mod foo;").unwrap();
        std::fs::write(src.join("readme.md"), "# Hello").unwrap();

        let executor = GlobExecutor::new();
        let input = json!({
            "pattern": "**/*.rs",
            "path": dir.path().to_str().unwrap()
        });

        let result = executor.execute(input).await.unwrap();
        assert!(result.contains("main.rs"));
        assert!(result.contains("lib.rs"));
        assert!(!result.contains("readme.md"));
    }

    #[tokio::test]
    async fn test_glob_excludes_node_modules() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("index.js"), "").unwrap();
        let nm = dir.path().join("node_modules").join("pkg");
        std::fs::create_dir_all(&nm).unwrap();
        std::fs::write(nm.join("index.js"), "").unwrap();

        let executor = GlobExecutor::new();
        let input = json!({
            "pattern": "**/*.js",
            "path": dir.path().to_str().unwrap()
        });

        let result = executor.execute(input).await.unwrap();
        assert!(result.contains("index.js"));
        // Should NOT contain node_modules path
        assert!(!result.contains("node_modules"));
    }

    #[tokio::test]
    async fn test_glob_no_files() {
        let dir = tempdir().unwrap();
        let executor = GlobExecutor::new();
        let input = json!({
            "pattern": "**/*.xyz_nonexistent",
            "path": dir.path().to_str().unwrap()
        });

        let result = executor.execute(input).await.unwrap();
        assert_eq!(result, "No files found");
    }

    #[tokio::test]
    async fn test_glob_invalid_directory() {
        let executor = GlobExecutor::new();
        let input = json!({
            "pattern": "**/*.rs",
            "path": "/nonexistent/directory/abc123"
        });

        let result = executor.execute(input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("does not exist"));
    }

    #[tokio::test]
    async fn test_glob_truncation() {
        let dir = tempdir().unwrap();
        // Create 150 files
        for i in 0..150 {
            std::fs::write(dir.path().join(format!("file_{:03}.txt", i)), "content").unwrap();
        }

        let executor = GlobExecutor::new();
        let input = json!({
            "pattern": "*.txt",
            "path": dir.path().to_str().unwrap()
        });

        let result = executor.execute(input).await.unwrap();
        assert!(result.contains("[Truncated: showing 100 of 150 files"));
    }

    #[tokio::test]
    async fn test_glob_empty_pattern() {
        let executor = GlobExecutor::new();
        let input = json!({
            "pattern": ""
        });

        let result = executor.execute(input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("pattern cannot be empty"));
    }

    #[tokio::test]
    async fn test_glob_excludes_git_directory() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("README.md"), "# Hello").unwrap();
        let git_dir = dir.path().join(".git").join("objects");
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::write(git_dir.join("pack"), "binary").unwrap();

        let executor = GlobExecutor::new();
        let input = json!({
            "pattern": "**/*",
            "path": dir.path().to_str().unwrap()
        });

        let result = executor.execute(input).await.unwrap();
        assert!(result.contains("README.md"));
        assert!(!result.contains(".git"));
    }
}
