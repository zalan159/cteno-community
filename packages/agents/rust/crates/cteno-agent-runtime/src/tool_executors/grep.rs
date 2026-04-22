//! Grep Executor
//!
//! Searches file contents using ripgrep (rg). Supports regex patterns,
//! glob/type filters, multiline matching, context lines, and pagination.

use crate::tool::ToolExecutor;
use crate::tool_executors::path_resolver;
use async_trait::async_trait;
use serde_json::Value;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

/// Default cap on output entries when head_limit is unspecified.
const DEFAULT_HEAD_LIMIT: usize = 250;

/// Timeout for ripgrep execution.
const GREP_TIMEOUT: Duration = Duration::from_secs(20);

/// Version-control directories excluded automatically.
const VCS_DIRECTORIES: &[&str] = &[".git", ".svn", ".hg", ".bzr", ".jj", ".sl"];

pub struct GrepExecutor;

impl GrepExecutor {
    pub fn new() -> Self {
        Self
    }

    /// Build the ripgrep argument list from the tool input.
    fn build_args(input: &Value, search_dir: &Path) -> Result<Vec<String>, String> {
        let pattern = input["pattern"]
            .as_str()
            .ok_or("Missing required 'pattern' parameter")?;

        if pattern.trim().is_empty() {
            return Err("pattern cannot be empty".to_string());
        }

        let output_mode = input
            .get("output_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("files_with_matches");

        let case_insensitive = input
            .get("case_insensitive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let multiline = input
            .get("multiline")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let line_numbers = input
            .get("line_numbers")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let context = input.get("context").and_then(|v| v.as_u64());
        let context_before = input.get("context_before").and_then(|v| v.as_u64());
        let context_after = input.get("context_after").and_then(|v| v.as_u64());

        let glob_filter = input.get("glob").and_then(|v| v.as_str());
        let type_filter = input.get("type").and_then(|v| v.as_str());

        let mut args: Vec<String> = Vec::new();

        // Search hidden files but exclude VCS directories
        args.push("--hidden".to_string());
        for dir in VCS_DIRECTORIES {
            args.push("--glob".to_string());
            args.push(format!("!{}", dir));
        }

        // Also exclude node_modules by default
        args.push("--glob".to_string());
        args.push("!node_modules".to_string());

        // Limit max line length to avoid binary/minified noise
        args.push("--max-columns".to_string());
        args.push("500".to_string());

        // Multiline mode
        if multiline {
            args.push("-U".to_string());
            args.push("--multiline-dotall".to_string());
        }

        // Case insensitive
        if case_insensitive {
            args.push("-i".to_string());
        }

        // Output mode
        match output_mode {
            "files_with_matches" => {
                args.push("-l".to_string());
            }
            "count" => {
                args.push("-c".to_string());
            }
            "content" => {
                // Line numbers
                if line_numbers {
                    args.push("-n".to_string());
                }
                // Context lines: -C takes precedence over -B/-A
                if let Some(c) = context {
                    args.push("-C".to_string());
                    args.push(c.to_string());
                } else {
                    if let Some(b) = context_before {
                        args.push("-B".to_string());
                        args.push(b.to_string());
                    }
                    if let Some(a) = context_after {
                        args.push("-A".to_string());
                        args.push(a.to_string());
                    }
                }
            }
            _ => {
                return Err(format!(
                    "Invalid output_mode '{}'. Must be one of: content, files_with_matches, count",
                    output_mode
                ));
            }
        }

        // Pattern (use -e if it starts with - to avoid option confusion)
        if pattern.starts_with('-') {
            args.push("-e".to_string());
            args.push(pattern.to_string());
        } else {
            args.push(pattern.to_string());
        }

        // Type filter
        if let Some(t) = type_filter {
            args.push("--type".to_string());
            args.push(t.to_string());
        }

        // Glob filter — split on whitespace but preserve brace patterns
        if let Some(glob) = glob_filter {
            let raw_patterns: Vec<&str> = glob.split_whitespace().collect();
            for raw in raw_patterns {
                if raw.contains('{') && raw.contains('}') {
                    // Brace pattern, keep as-is
                    args.push("--glob".to_string());
                    args.push(raw.to_string());
                } else {
                    // May contain commas for multiple patterns
                    for part in raw.split(',').filter(|s| !s.is_empty()) {
                        args.push("--glob".to_string());
                        args.push(part.to_string());
                    }
                }
            }
        }

        // Search directory
        args.push(search_dir.to_string_lossy().to_string());

        Ok(args)
    }

    /// Apply offset + head_limit pagination to a list of lines.
    fn paginate(lines: Vec<String>, offset: usize, head_limit: usize) -> (Vec<String>, bool) {
        let after_offset: Vec<String> = lines.into_iter().skip(offset).collect();
        // head_limit == 0 means unlimited
        if head_limit == 0 {
            return (after_offset, false);
        }
        let truncated = after_offset.len() > head_limit;
        let result: Vec<String> = after_offset.into_iter().take(head_limit).collect();
        (result, truncated)
    }

    /// Convert absolute paths in output lines to paths relative to workdir.
    fn relativize(line: &str, base: &Path) -> String {
        let base_str = base.to_string_lossy();
        let prefix = if base_str.ends_with('/') {
            base_str.to_string()
        } else {
            format!("{}/", base_str)
        };
        if line.starts_with(prefix.as_str()) {
            line[prefix.len()..].to_string()
        } else {
            line.to_string()
        }
    }
}

impl Default for GrepExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolExecutor for GrepExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        // Resolve search directory
        let explicit_path = input.get("path").and_then(|v| v.as_str());
        let workdir = input.get("workdir").and_then(|v| v.as_str());
        let search_dir = if let Some(p) = explicit_path {
            path_resolver::expand_tilde(p)
        } else {
            path_resolver::resolve_workdir(workdir)
        };

        if !search_dir.exists() {
            return Err(format!(
                "Search path does not exist: {}",
                search_dir.display()
            ));
        }

        let output_mode = input
            .get("output_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("files_with_matches");

        let offset = input.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

        let head_limit = input
            .get("head_limit")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(DEFAULT_HEAD_LIMIT);

        let args = Self::build_args(&input, &search_dir)?;

        // Execute rg
        let mut cmd = Command::new("rg");
        for arg in &args {
            cmd.arg(arg);
        }
        cmd.stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let output = match tokio::time::timeout(GREP_TIMEOUT, cmd.output()).await {
            Ok(result) => {
                result.map_err(|e| format!("Failed to execute rg: {}. Is ripgrep installed?", e))?
            }
            Err(_) => {
                return Err(format!(
                    "Grep timed out after {}s. The search scope may be too broad — try a more specific path or pattern.",
                    GREP_TIMEOUT.as_secs()
                ));
            }
        };

        let exit_code = output.status.code().unwrap_or(-1);

        // rg exit codes:
        //   0 = matches found
        //   1 = no matches (NOT an error)
        //   2+ = actual error
        if exit_code >= 2 {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            return Err(format!("ripgrep error (exit {}): {}", exit_code, stderr));
        }

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();

        if exit_code == 1 || stdout.trim().is_empty() {
            return Ok("No matches found".to_string());
        }

        // Split into lines and process based on output_mode
        let raw_lines: Vec<String> = stdout
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| Self::relativize(l, &search_dir))
            .collect();

        let total = raw_lines.len();
        let (lines, truncated) = Self::paginate(raw_lines, offset, head_limit);

        match output_mode {
            "files_with_matches" => {
                let count = lines.len();
                let mut result =
                    format!("Found {} file{}", count, if count == 1 { "" } else { "s" });
                if truncated {
                    result.push_str(&format!(" (showing {} of {})", count, total - offset));
                }
                result.push('\n');
                result.push_str(&lines.join("\n"));
                if truncated {
                    result.push_str(&format!(
                        "\n\n[Results truncated. Use offset: {} to see more]",
                        offset + head_limit
                    ));
                }
                Ok(result)
            }
            "count" => {
                // Lines are file:count format; sum up totals
                let mut total_matches: u64 = 0;
                let mut file_count: u64 = 0;
                for line in &lines {
                    if let Some(colon_pos) = line.rfind(':') {
                        if let Ok(n) = line[colon_pos + 1..].parse::<u64>() {
                            total_matches += n;
                            file_count += 1;
                        }
                    }
                }
                let mut result = lines.join("\n");
                result.push_str(&format!(
                    "\n\nFound {} total occurrence{} across {} file{}.",
                    total_matches,
                    if total_matches == 1 { "" } else { "s" },
                    file_count,
                    if file_count == 1 { "" } else { "s" }
                ));
                if truncated {
                    result.push_str(&format!(
                        "\n[Results truncated. Use offset: {} to see more]",
                        offset + head_limit
                    ));
                }
                Ok(result)
            }
            "content" | _ => {
                // Content mode — raw output with line numbers/context
                let mut result = lines.join("\n");
                if truncated {
                    result.push_str(&format!(
                        "\n\n[Results truncated. Showing {} of {} lines. Use offset: {} to see more]",
                        lines.len(),
                        total - offset,
                        offset + head_limit
                    ));
                }
                Ok(result)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_grep_no_matches() {
        let executor = GrepExecutor::new();
        let dir = tempdir().unwrap();
        tokio::fs::write(dir.path().join("test.txt"), "hello world\n")
            .await
            .unwrap();

        let input = json!({
            "pattern": "nonexistent_pattern_xyz",
            "path": dir.path().to_str().unwrap()
        });
        let result = executor.execute(input).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "No matches found");
    }

    #[tokio::test]
    async fn test_grep_files_with_matches() {
        let executor = GrepExecutor::new();
        let dir = tempdir().unwrap();
        tokio::fs::write(dir.path().join("a.rs"), "fn main() {}\n")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("b.txt"), "nothing here\n")
            .await
            .unwrap();

        let input = json!({
            "pattern": "fn main",
            "path": dir.path().to_str().unwrap()
        });
        let result = executor.execute(input).await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.contains("a.rs"));
        assert!(!output.contains("b.txt"));
    }

    #[tokio::test]
    async fn test_grep_content_mode() {
        let executor = GrepExecutor::new();
        let dir = tempdir().unwrap();
        tokio::fs::write(
            dir.path().join("test.rs"),
            "fn hello() {\n    println!(\"hi\");\n}\n",
        )
        .await
        .unwrap();

        let input = json!({
            "pattern": "hello",
            "path": dir.path().to_str().unwrap(),
            "output_mode": "content"
        });
        let result = executor.execute(input).await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.contains("fn hello"));
    }

    #[tokio::test]
    async fn test_grep_count_mode() {
        let executor = GrepExecutor::new();
        let dir = tempdir().unwrap();
        tokio::fs::write(dir.path().join("test.txt"), "aaa\naaa\nbbb\naaa\n")
            .await
            .unwrap();

        let input = json!({
            "pattern": "aaa",
            "path": dir.path().to_str().unwrap(),
            "output_mode": "count"
        });
        let result = executor.execute(input).await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.contains("3 total occurrence"));
    }

    #[tokio::test]
    async fn test_grep_case_insensitive() {
        let executor = GrepExecutor::new();
        let dir = tempdir().unwrap();
        tokio::fs::write(dir.path().join("test.txt"), "Hello World\nhello world\n")
            .await
            .unwrap();

        let input = json!({
            "pattern": "HELLO",
            "path": dir.path().to_str().unwrap(),
            "output_mode": "count",
            "case_insensitive": true
        });
        let result = executor.execute(input).await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.contains("2 total occurrence"));
    }

    #[tokio::test]
    async fn test_grep_pagination() {
        let lines: Vec<String> = (1..=10).map(|i| format!("line{}", i)).collect();
        let (result, truncated) = GrepExecutor::paginate(lines, 2, 3);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0], "line3");
        assert!(truncated);
    }

    #[tokio::test]
    async fn test_grep_empty_pattern() {
        let executor = GrepExecutor::new();
        let input = json!({
            "pattern": "",
            "path": "/tmp"
        });
        let result = executor.execute(input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cannot be empty"));
    }

    #[tokio::test]
    async fn test_grep_nonexistent_path() {
        let executor = GrepExecutor::new();
        let input = json!({
            "pattern": "test",
            "path": "/nonexistent/path/that/does/not/exist"
        });
        let result = executor.execute(input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("does not exist"));
    }

    #[test]
    fn test_relativize() {
        let base = Path::new("/home/user/project");
        assert_eq!(
            GrepExecutor::relativize("/home/user/project/src/main.rs", base),
            "src/main.rs"
        );
        assert_eq!(
            GrepExecutor::relativize("/other/path/file.txt", base),
            "/other/path/file.txt"
        );
    }
}
