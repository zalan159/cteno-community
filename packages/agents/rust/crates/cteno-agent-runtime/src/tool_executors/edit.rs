//! File Edit Tool Executor
//!
//! Implements search and replace functionality with three-tier matching strategies:
//! 1. Exact match
//! 2. Flexible line match (ignores indentation)
//! 3. Regex flexible match (tokenizes and tolerates whitespace)

use crate::tool::ToolExecutor;
use crate::tool_executors::{path_resolver, sandbox};
use async_trait::async_trait;
use regex::Regex;
use std::fs;
use std::path::PathBuf;

/// File Edit Executor
pub struct EditExecutor {
    llm_client: Option<std::sync::Arc<crate::llm::LLMClient>>,
}

impl EditExecutor {
    pub fn new() -> Self {
        Self { llm_client: None }
    }

    pub fn with_llm_client(llm_client: std::sync::Arc<crate::llm::LLMClient>) -> Self {
        Self {
            llm_client: Some(llm_client),
        }
    }

    fn resolve_file_path(path: &str, workdir: Option<&str>) -> Result<PathBuf, String> {
        path_resolver::resolve_file_path(path, workdir)
    }

    /// Try exact match replacement
    fn try_exact_match(content: &str, old_string: &str) -> Option<(String, usize)> {
        let count = content.matches(old_string).count();
        if count > 0 {
            Some((content.to_string(), count))
        } else {
            None
        }
    }

    /// Try flexible line match (ignores leading whitespace differences)
    /// Returns the pattern to use for replacement while preserving original indentation
    fn try_flexible_line_match(content: &str, old_string: &str) -> Option<String> {
        let old_lines: Vec<&str> = old_string.lines().collect();
        if old_lines.is_empty() {
            return None;
        }

        let content_lines: Vec<&str> = content.lines().collect();

        // Build a pattern matcher that ignores leading whitespace
        for start_idx in 0..content_lines.len() {
            if start_idx + old_lines.len() > content_lines.len() {
                break;
            }

            let mut all_match = true;
            for (i, old_line) in old_lines.iter().enumerate() {
                let content_line = content_lines[start_idx + i];
                // Compare trimmed lines
                if content_line.trim() != old_line.trim() {
                    all_match = false;
                    break;
                }
            }

            if all_match {
                // Found a match, extract the actual text from content
                let matched_lines: Vec<&str> =
                    content_lines[start_idx..start_idx + old_lines.len()].to_vec();
                return Some(matched_lines.join("\n"));
            }
        }

        None
    }

    /// Try regex flexible match (tokenizes by delimiters, tolerates whitespace)
    fn try_regex_flexible_match(content: &str, old_string: &str) -> Option<String> {
        // Tokenize by common delimiters while preserving them
        let delimiters = r"[\(\)\[\]\{\}:;,\.\s]+";
        let delimiter_re = Regex::new(delimiters).ok()?;

        // Split old_string into tokens
        let tokens: Vec<&str> = delimiter_re
            .split(old_string)
            .filter(|t| !t.is_empty())
            .collect();

        if tokens.is_empty() {
            return None;
        }

        // Build a flexible regex pattern: tokens separated by optional whitespace
        let pattern_parts: Vec<String> = tokens.iter().map(|t| regex::escape(t)).collect();

        let pattern = pattern_parts.join(r"[\s\(\)\[\]\{\}:;,\.]*");

        let re = Regex::new(&pattern).ok()?;

        re.find(content).map(|mat| mat.as_str().to_string())
    }

    /// Detect line ending style in content
    fn detect_line_ending(content: &str) -> &'static str {
        if content.contains("\r\n") {
            "\r\n" // Windows CRLF
        } else {
            "\n" // Unix LF
        }
    }

    /// Restore trailing newline if needed
    fn restore_trailing_newline(original: &str, modified: &str) -> String {
        let had_trailing = original.ends_with('\n');
        let has_trailing = modified.ends_with('\n');

        if had_trailing && !has_trailing {
            format!("{}\n", modified)
        } else if !had_trailing && has_trailing {
            modified.trim_end_matches('\n').to_string()
        } else {
            modified.to_string()
        }
    }

    /// Apply replacement with appropriate indentation preservation
    fn apply_replacement(
        content: &str,
        actual_old: &str,
        new_string: &str,
        old_string: &str,
    ) -> String {
        // If old_string differs from actual_old, we need to adjust indentation
        if actual_old == old_string {
            // Exact match, simple replacement
            content.replacen(actual_old, new_string, 1)
        } else {
            // Flexible match, need to preserve original indentation
            let old_lines: Vec<&str> = old_string.lines().collect();
            let actual_lines: Vec<&str> = actual_old.lines().collect();
            let new_lines: Vec<&str> = new_string.lines().collect();

            // Detect indentation of actual content
            let base_indent = if !actual_lines.is_empty() {
                let first_line = actual_lines[0];
                let trimmed = first_line.trim_start();
                &first_line[..first_line.len() - trimmed.len()]
            } else {
                ""
            };

            // Detect indentation of old_string (what user typed)
            let user_indent = if !old_lines.is_empty() {
                let first_line = old_lines[0];
                let trimmed = first_line.trim_start();
                &first_line[..first_line.len() - trimmed.len()]
            } else {
                ""
            };

            // Build new content with adjusted indentation
            let adjusted_new: Vec<String> = new_lines
                .iter()
                .enumerate()
                .map(|(i, line)| {
                    if i == 0 {
                        // First line uses actual content's indentation
                        let trimmed = line.trim_start();
                        format!("{}{}", base_indent, trimmed)
                    } else if line.trim().is_empty() {
                        // Empty lines stay empty
                        String::new()
                    } else {
                        // Other lines: calculate relative indent from user's input
                        let line_trimmed = line.trim_start();
                        let line_indent = &line[..line.len() - line_trimmed.len()];

                        // If user provided indent, calculate relative to their first line
                        let relative_indent = if line_indent.len() > user_indent.len() {
                            &line_indent[user_indent.len()..]
                        } else {
                            ""
                        };

                        format!("{}{}{}", base_indent, relative_indent, line_trimmed)
                    }
                })
                .collect();

            content.replacen(actual_old, &adjusted_new.join("\n"), 1)
        }
    }
}

#[async_trait]
impl ToolExecutor for EditExecutor {
    async fn execute(&self, input: serde_json::Value) -> Result<String, String> {
        // Parse input
        let path = input
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: path")?;

        let instruction = input
            .get("instruction")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: instruction")?;

        let old_string = input
            .get("old_string")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: old_string")?;

        let new_string = input
            .get("new_string")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: new_string")?;

        let expected_replacements = input
            .get("expected_replacements")
            .and_then(|v| v.as_u64())
            .unwrap_or(1) as usize;

        let workdir = input.get("workdir").and_then(|v| v.as_str());
        let sandbox_ctx = sandbox::SandboxContext::from_input(&input);
        let file_path =
            path_resolver::resolve_file_path_sandboxed(path, workdir, &sandbox_ctx, true)?;

        // Validate file exists
        if !file_path.exists() {
            return Err(format!(
                "File not found: {}\n\nTip: Use read tool to verify the path is correct.",
                file_path.display()
            ));
        }

        // --- Staleness detection ---
        if let Some(session_id) = input.get("__session_id").and_then(|v| v.as_str()) {
            let canonical = fs::canonicalize(&file_path)
                .unwrap_or_else(|_| file_path.clone())
                .to_string_lossy()
                .to_string();

            if let Some(read_mtime) =
                super::file_tracker::get_file_read_time(session_id, &canonical)
            {
                if let Ok(metadata) = fs::metadata(&file_path) {
                    if let Ok(current_mtime) = metadata.modified() {
                        if current_mtime > read_mtime {
                            return Err(format!(
                                "FILE_MODIFIED_SINCE_READ: The file '{}' has been modified since you last read it (possibly by a linter, formatter, or the user). Please read the file again before editing to ensure you have the latest content.",
                                file_path.display()
                            ));
                        }
                    }
                }
            }
        }

        // Read current content
        let content =
            fs::read_to_string(&file_path).map_err(|e| format!("Failed to read file: {}", e))?;

        // Detect line ending style
        let original_line_ending = Self::detect_line_ending(&content);

        // Normalize to LF for processing (CRLF -> LF)
        let normalized_content = content.replace("\r\n", "\n");

        // Check if old_string == new_string (no changes)
        if old_string == new_string {
            return Err(
                "No changes to apply. The old_string and new_string are identical.\n\nTip: Ensure you're providing different content for replacement.".to_string()
            );
        }

        // Try matching strategies in order (using normalized content)
        let (actual_old, match_strategy) = {
            // 1. Try exact match first
            if let Some((_, count)) = Self::try_exact_match(&normalized_content, old_string) {
                if count == expected_replacements {
                    (old_string.to_string(), "exact")
                } else if count > expected_replacements {
                    return Err(format!(
                        "Found {} occurrences of the search string, expected {}.\n\nSolution: Add more context lines (3+ before and after) to make the search string unique.\n\nUse read tool to examine the file.",
                        count, expected_replacements
                    ));
                } else {
                    // count < expected, try other strategies
                    if let Some(matched) =
                        Self::try_flexible_line_match(&normalized_content, old_string)
                    {
                        (matched, "flexible_line")
                    } else if let Some(matched) =
                        Self::try_regex_flexible_match(&normalized_content, old_string)
                    {
                        (matched, "regex_flexible")
                    } else {
                        return Err(format!(
                            "Found {} occurrences, expected {}. The text may have changed. Use read tool to check current content.",
                            count, expected_replacements
                        ));
                    }
                }
            }
            // 2. Try flexible line match
            else if let Some(matched) =
                Self::try_flexible_line_match(&normalized_content, old_string)
            {
                (matched, "flexible_line")
            }
            // 3. Try regex flexible match
            else if let Some(matched) =
                Self::try_regex_flexible_match(&normalized_content, old_string)
            {
                (matched, "regex_flexible")
            }
            // No match found - try LLM auto-correction
            else if let Some(llm_client) = &self.llm_client {
                log::info!("[Edit] Attempting LLM auto-correction...");

                let error_msg = "0 occurrences found. The search string was not found in the file.";

                match crate::llm_edit_fixer::fix_edit_with_instruction(
                    instruction,
                    old_string,
                    new_string,
                    error_msg,
                    &normalized_content,
                    llm_client,
                )
                .await
                {
                    Ok(Some(fixed_edit)) => {
                        if fixed_edit.no_changes_required {
                            return Ok(format!(
                                "No changes required. The file already meets the instruction.\n\nExplanation: {}",
                                fixed_edit.explanation
                            ));
                        }

                        log::info!("[Edit] LLM correction: {}", fixed_edit.explanation);

                        // Retry with corrected search string
                        if let Some((_, count)) =
                            Self::try_exact_match(&normalized_content, &fixed_edit.search)
                        {
                            if count == expected_replacements {
                                (fixed_edit.search.clone(), "exact (LLM corrected)")
                            } else {
                                return Err(format!(
                                    "LLM correction found {} occurrences, expected {}.\n\nExplanation: {}",
                                    count, expected_replacements, fixed_edit.explanation
                                ));
                            }
                        } else {
                            return Err(format!(
                                "LLM correction also failed to find a match.\n\nExplanation: {}",
                                fixed_edit.explanation
                            ));
                        }
                    }
                    Ok(None) => {
                        log::warn!("[Edit] LLM returned no correction");
                        return Err(
                            "0 occurrences found. The search string was not found in the file.\n\nPossible causes:\n1. Whitespace/indentation mismatch\n2. File content has changed\n3. Incorrect escaping\n\nSolution: Use read tool to verify current content and retry with exact match.".to_string()
                        );
                    }
                    Err(e) => {
                        log::error!("[Edit] LLM correction failed: {}", e);
                        return Err(
                            "0 occurrences found. The search string was not found in the file.\n\nPossible causes:\n1. Whitespace/indentation mismatch\n2. File content has changed\n3. Incorrect escaping\n\nSolution: Use read tool to verify current content and retry with exact match.".to_string()
                        );
                    }
                }
            } else {
                return Err(
                    "0 occurrences found. The search string was not found in the file.\n\nPossible causes:\n1. Whitespace/indentation mismatch\n2. File content has changed\n3. Incorrect escaping\n\nSolution: Use read tool to verify current content and retry with exact match.".to_string()
                );
            }
        };

        // Verify count after flexible match
        let actual_count = normalized_content.matches(&actual_old).count();
        if actual_count != expected_replacements {
            if actual_count > expected_replacements {
                return Err(format!(
                    "Found {} occurrences using {} matching, expected {}.\n\nSolution: Add more context lines (3+ before and after) to make the search string unique.\n\nUse read tool to examine the file.",
                    actual_count, match_strategy, expected_replacements
                ));
            } else {
                return Err(format!(
                    "Found {} occurrences using {} matching, expected {}.",
                    actual_count, match_strategy, expected_replacements
                ));
            }
        }

        // Apply replacement (on normalized content)
        let mut new_content =
            Self::apply_replacement(&normalized_content, &actual_old, new_string, old_string);

        // Restore trailing newline
        new_content = Self::restore_trailing_newline(&normalized_content, &new_content);

        // Restore original line ending format (LF -> CRLF if needed)
        let final_content = if original_line_ending == "\r\n" {
            new_content.replace("\n", "\r\n")
        } else {
            new_content
        };

        // Write back
        fs::write(&file_path, &final_content)
            .map_err(|e| format!("Failed to write file: {}", e))?;

        // Update file tracker with the new mtime after edit, so subsequent
        // writes/edits don't trigger a false-positive staleness error.
        if let Some(session_id) = input.get("__session_id").and_then(|v| v.as_str()) {
            if let Ok(metadata) = fs::metadata(&file_path) {
                if let Ok(new_mtime) = metadata.modified() {
                    let canonical = fs::canonicalize(&file_path)
                        .unwrap_or_else(|_| file_path.clone())
                        .to_string_lossy()
                        .to_string();
                    super::file_tracker::record_file_read(session_id, &canonical, new_mtime);
                }
            }
        }

        log::info!(
            "[Edit] Successfully edited {} using {} matching",
            file_path.display(),
            match_strategy
        );

        Ok(format!(
            "Successfully replaced {} occurrence(s) in {} using {} matching",
            expected_replacements,
            file_path.display(),
            match_strategy
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_exact_match() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "hello world\nfoo bar\nbaz qux").unwrap();

        let executor = EditExecutor::new();
        let input = serde_json::json!({
            "path": file_path.to_str().unwrap(),
            "instruction": "Replace greeting",
            "old_string": "hello world",
            "new_string": "hi there"
        });

        let result = executor.execute(input).await;
        assert!(result.is_ok(), "Expected success, got: {:?}", result);

        let content = fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "hi there\nfoo bar\nbaz qux");
    }

    #[tokio::test]
    async fn test_flexible_line_match() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        // File has 4-space indent
        fs::write(
            &file_path,
            "    function test() {\n        return 1;\n    }",
        )
        .unwrap();

        let executor = EditExecutor::new();
        // User provides with no indent
        let input = serde_json::json!({
            "path": file_path.to_str().unwrap(),
            "instruction": "Update return value to 2",
            "old_string": "function test() {\n    return 1;\n}",
            "new_string": "function test() {\n    return 2;\n}"
        });

        let result = executor.execute(input).await;
        assert!(result.is_ok(), "Expected success, got: {:?}", result);

        let content = fs::read_to_string(&file_path).unwrap();
        // Should preserve the original 4-space indent
        assert!(content.contains("return 2;"));
    }

    #[tokio::test]
    async fn test_multiple_occurrences_error() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "foo\nfoo\nfoo").unwrap();

        let executor = EditExecutor::new();
        let input = serde_json::json!({
            "path": file_path.to_str().unwrap(),
            "instruction": "Replace all foo with bar",
            "old_string": "foo",
            "new_string": "bar"
        });

        let result = executor.execute(input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("3 occurrences"));
    }

    #[tokio::test]
    async fn test_not_found_error() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "hello world").unwrap();

        let executor = EditExecutor::new();
        let input = serde_json::json!({
            "path": file_path.to_str().unwrap(),
            "instruction": "Replace nonexistent text",
            "old_string": "nonexistent",
            "new_string": "replacement"
        });

        let result = executor.execute(input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("0 occurrences"));
    }

    #[tokio::test]
    async fn test_multiline_replacement() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.rs");
        fs::write(
            &file_path,
            "fn main() {\n    println!(\"Hello\");\n}\n\nfn other() {}\n",
        )
        .unwrap();

        let executor = EditExecutor::new();
        let input = serde_json::json!({
            "path": file_path.to_str().unwrap(),
            "instruction": "Update main function greeting",
            "old_string": "fn main() {\n    println!(\"Hello\");\n}",
            "new_string": "fn main() {\n    println!(\"Hello, World!\");\n}"
        });

        let result = executor.execute(input).await;
        assert!(result.is_ok(), "Expected success, got: {:?}", result);

        let content = fs::read_to_string(&file_path).unwrap();
        assert!(content.contains("Hello, World!"));
        assert!(content.contains("fn other() {}"));
    }

    #[tokio::test]
    async fn test_detect_line_ending_crlf() {
        let content = "line1\r\nline2\r\nline3";
        assert_eq!(EditExecutor::detect_line_ending(content), "\r\n");
    }

    #[tokio::test]
    async fn test_detect_line_ending_lf() {
        let content = "line1\nline2\nline3";
        assert_eq!(EditExecutor::detect_line_ending(content), "\n");
    }

    #[tokio::test]
    async fn test_restore_trailing_newline_add() {
        let original = "content\n";
        let modified = "new_content";
        assert_eq!(
            EditExecutor::restore_trailing_newline(original, modified),
            "new_content\n"
        );
    }

    #[tokio::test]
    async fn test_restore_trailing_newline_remove() {
        let original = "content";
        let modified = "new_content\n";
        assert_eq!(
            EditExecutor::restore_trailing_newline(original, modified),
            "new_content"
        );
    }

    #[tokio::test]
    async fn test_restore_trailing_newline_preserve() {
        let original = "content\n";
        let modified = "new_content\n";
        assert_eq!(
            EditExecutor::restore_trailing_newline(original, modified),
            "new_content\n"
        );
    }

    #[tokio::test]
    async fn test_instruction_parameter_required() {
        let executor = EditExecutor::new();
        let input = serde_json::json!({
            "path": "/tmp/test.txt",
            "old_string": "old",
            "new_string": "new"
            // missing instruction
        });

        let result = executor.execute(input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("instruction"));
    }

    #[tokio::test]
    async fn test_no_changes_error() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "hello world").unwrap();

        let executor = EditExecutor::new();
        let input = serde_json::json!({
            "path": file_path.to_str().unwrap(),
            "instruction": "No real change",
            "old_string": "hello",
            "new_string": "hello"
        });

        let result = executor.execute(input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No changes to apply"));
    }

    #[tokio::test]
    async fn test_edit_resolves_relative_path_with_workdir() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("a").join("b.txt");
        fs::create_dir_all(file_path.parent().unwrap()).unwrap();
        fs::write(&file_path, "hello\n").unwrap();

        let executor = EditExecutor::new();
        let input = serde_json::json!({
            "path": "a/b.txt",
            "workdir": dir.path().to_str().unwrap(),
            "instruction": "Replace hello with world",
            "old_string": "hello",
            "new_string": "world",
            "expected_replacements": 1
        });

        let result = executor.execute(input).await;
        assert!(result.is_ok(), "Expected success, got: {:?}", result);
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "world\n");
    }

    #[tokio::test]
    async fn test_edit_rejects_relative_escape() {
        let dir = tempdir().unwrap();
        let executor = EditExecutor::new();
        let input = serde_json::json!({
            "path": "../escape.txt",
            "workdir": dir.path().to_str().unwrap(),
            "instruction": "No-op",
            "old_string": "x",
            "new_string": "y"
        });

        let result = executor.execute(input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("escapes workdir"));
    }
}
