//! Read Executor
//!
//! Read files with pagination, Bom detection, and multi-format support.
//! Based on gemini-cli's read-file tool design.
//!
//! When the current model supports vision (`__supports_vision` injected by agent),
//! image files are uploaded to OSS and returned as image content blocks so the LLM
//! can actually see the image.
use crate::tool::ToolExecutor;
use crate::tool_executors::oss_upload::OssUploader;
use crate::tool_executors::path_resolver;
use async_trait::async_trait;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;

/// Maximum file size to read (20 MB)
const MAX_FILE_SIZE: u64 = 20 * 1024 * 1024;

/// Maximum lines to read by default
const DEFAULT_MAX_LINES: usize = 2000;

/// Maximum line length before truncation
const MAX_LINE_LENGTH: usize = 2000;

/// SVG file size limit (1 MB)
const SVG_MAX_SIZE: u64 = 1024 * 1024;

/// Bom (Byte Order Mark) detection result
#[derive(Debug, Clone)]
enum Bom {
    Utf8,    // EF BB BF
    Utf16LE, // FF FE
    Utf16BE, // FE FF
    Utf32LE, // FF FE 00 00
    Utf32BE, // 00 00 FE FF
}

impl Bom {
    fn byte_length(&self) -> usize {
        match self {
            Bom::Utf8 => 3,
            Bom::Utf16LE | Bom::Utf16BE => 2,
            Bom::Utf32LE | Bom::Utf32BE => 4,
        }
    }
}

/// File type classification
#[derive(Debug, Clone, PartialEq)]
enum FileType {
    Text,
    Image,
    Pdf,
    Audio,
    Video,
    Binary,
    Svg,
}

/// Read result with pagination info
#[derive(Debug)]
struct ReadResult {
    content: String,
    is_truncated: bool,
    original_line_count: usize,
    lines_shown: (usize, usize), // (start_line, end_line) - 1-based
}

/// Read executor
pub struct ReadExecutor {
    max_file_size: u64,
    default_max_lines: usize,
    max_line_length: usize,
}

impl ReadExecutor {
    /// Create a new ReadExecutor with default settings
    pub fn new() -> Self {
        Self {
            max_file_size: MAX_FILE_SIZE,
            default_max_lines: DEFAULT_MAX_LINES,
            max_line_length: MAX_LINE_LENGTH,
        }
    }

    fn resolve_input_path(file_path: &str, workdir: Option<&str>) -> Result<PathBuf, String> {
        // Relative paths are resolved against the injected `workdir` (agent session cwd).
        // This keeps read/write/edit consistent and avoids surprising HOME anchoring.
        path_resolver::resolve_file_path(file_path, workdir)
    }

    /// Detect Bom in file content
    fn detect_bom(bytes: &[u8]) -> Option<Bom> {
        if bytes.len() >= 4 {
            // UTF-32 LE: FF FE 00 00
            if bytes[0..4] == [0xFF, 0xFE, 0x00, 0x00] {
                return Some(Bom::Utf32LE);
            }
            // UTF-32 BE: 00 00 FE FF
            if bytes[0..4] == [0x00, 0x00, 0xFE, 0xFF] {
                return Some(Bom::Utf32BE);
            }
        }

        if bytes.len() >= 3 {
            // UTF-8: EF BB BF
            if bytes[0..3] == [0xEF, 0xBB, 0xBF] {
                return Some(Bom::Utf8);
            }
        }

        if bytes.len() >= 2 {
            // UTF-16 LE: FF FE
            if bytes[0..2] == [0xFF, 0xFE] {
                return Some(Bom::Utf16LE);
            }
            // UTF-16 BE: FE FF
            if bytes[0..2] == [0xFE, 0xFF] {
                return Some(Bom::Utf16BE);
            }
        }

        None
    }

    /// Detect if file is binary
    async fn is_binary_file(path: &Path) -> Result<bool, String> {
        let metadata = fs::metadata(path)
            .await
            .map_err(|e| format!("Failed to get file metadata: {}", e))?;

        let sample_size = std::cmp::min(4096, metadata.len() as usize);
        let mut buffer = vec![0u8; sample_size];

        let file = fs::File::open(path)
            .await
            .map_err(|e| format!("Failed to open file: {}", e))?;

        use tokio::io::AsyncReadExt;
        let mut reader = tokio::io::BufReader::new(file);
        reader
            .read_exact(&mut buffer)
            .await
            .map_err(|e| format!("Failed to read file: {}", e))?;

        // Check for Bom - if present, treat as text
        if Self::detect_bom(&buffer).is_some() {
            return Ok(false);
        }

        // Check for null bytes - strong indicator of binary
        if buffer.contains(&0x00) {
            return Ok(true);
        }

        // Count non-printable characters
        let non_printable_count = buffer
            .iter()
            .filter(|&&byte| byte < 9 || (byte > 13 && byte < 32))
            .count();

        // If >30% non-printable, consider binary
        Ok(non_printable_count as f64 / buffer.len() as f64 > 0.3)
    }

    /// Detect file type
    async fn detect_file_type(path: &Path) -> Result<FileType, String> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        // Special cases
        if matches!(ext.as_str(), "ts" | "mts" | "cts" | "tsx") {
            return Ok(FileType::Text);
        }

        if ext == "svg" {
            return Ok(FileType::Svg);
        }

        // Image extensions
        if matches!(
            ext.as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico"
        ) {
            return Ok(FileType::Image);
        }

        // Audio extensions
        if matches!(
            ext.as_str(),
            "mp3" | "wav" | "aiff" | "aac" | "ogg" | "flac"
        ) {
            return Ok(FileType::Audio);
        }

        // Video extensions
        if matches!(ext.as_str(), "mp4" | "mov" | "avi" | "mkv" | "webm" | "flv") {
            return Ok(FileType::Video);
        }

        // PDF
        if ext == "pdf" {
            return Ok(FileType::Pdf);
        }

        // Known binary extensions
        let binary_exts = [
            "exe", "dll", "so", "dylib", "bin", "dat", "db", "sqlite", "zip", "tar", "gz", "bz2",
            "7z", "rar", "class", "pyc", "o", "a",
        ];
        if binary_exts.contains(&ext.as_str()) {
            return Ok(FileType::Binary);
        }

        // Content-based binary detection
        if Self::is_binary_file(path).await? {
            return Ok(FileType::Binary);
        }

        Ok(FileType::Text)
    }

    /// Read file with encoding detection
    async fn read_file_with_encoding(path: &Path) -> Result<String, String> {
        let bytes = fs::read(path)
            .await
            .map_err(|e| format!("Failed to read file: {}", e))?;

        // Detect and strip Bom
        let (content_bytes, _bom) = match Self::detect_bom(&bytes) {
            Some(bom) => {
                let skip = bom.byte_length();
                (&bytes[skip..], Some(bom))
            }
            None => (bytes.as_slice(), None),
        };

        // Try UTF-8 decoding (most common)
        match String::from_utf8(content_bytes.to_vec()) {
            Ok(s) => Ok(s),
            Err(_) => {
                // Fallback: lossy conversion
                Ok(String::from_utf8_lossy(content_bytes).to_string())
            }
        }
    }

    /// Read text file with pagination
    async fn read_text_file(
        &self,
        path: &Path,
        offset: Option<usize>,
        limit: Option<usize>,
    ) -> Result<ReadResult, String> {
        let content = Self::read_file_with_encoding(path).await?;
        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        let start_line = offset.unwrap_or(0);
        let max_lines = limit.unwrap_or(self.default_max_lines);
        let end_line = std::cmp::min(start_line + max_lines, total_lines);

        if start_line >= total_lines {
            return Err(format!(
                "Offset {} is beyond file length ({} lines)",
                start_line, total_lines
            ));
        }

        let selected_lines = &lines[start_line..end_line];

        // Truncate long lines
        let formatted_lines: Vec<String> = selected_lines
            .iter()
            .enumerate()
            .map(|(idx, line)| {
                let line_num = start_line + idx + 1; // 1-based line numbers
                let content = if line.len() > self.max_line_length {
                    format!(
                        "{}... [truncated]",
                        cteno_community_host::text_utils::truncate_str(line, self.max_line_length,)
                    )
                } else {
                    line.to_string()
                };
                format!("{:6}→{}", line_num, content)
            })
            .collect();

        let is_truncated = start_line > 0 || end_line < total_lines;

        Ok(ReadResult {
            content: formatted_lines.join("\n"),
            is_truncated,
            original_line_count: total_lines,
            lines_shown: (start_line + 1, end_line), // 1-based
        })
    }

    /// Detect MIME type for image files
    fn detect_image_mime(path: &Path) -> &'static str {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        match ext.as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            "bmp" => "image/bmp",
            _ => "image/png",
        }
    }

    /// Read image file and return base64 JSON with `images` array for vision injection.
    async fn read_image_for_vision(&self, path: &Path) -> Result<String, String> {
        let mime = Self::detect_image_mime(path);
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|e| format!("Failed to read image file: {}", e))?;

        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        log::info!(
            "[Read] Image read for vision: {} ({}KB)",
            path.display(),
            b64.len() / 1024
        );

        Ok(serde_json::to_string(&serde_json::json!({
            "file": path.display().to_string(),
            "type": "image",
            "images": [{
                "type": "base64",
                "media_type": mime,
                "data": b64
            }]
        }))
        .unwrap())
    }

    /// Read file based on type
    async fn read_file(
        &self,
        path: &Path,
        offset: Option<usize>,
        limit: Option<usize>,
        supports_vision: bool,
    ) -> Result<String, String> {
        // Check if file exists
        if !path.exists() {
            return Err(format!("File not found: {}", path.display()));
        }

        // Check if it's a directory
        if path.is_dir() {
            return Err(format!("Path is a directory: {}", path.display()));
        }

        // Check file size
        let metadata = fs::metadata(path)
            .await
            .map_err(|e| format!("Failed to get file metadata: {}", e))?;

        if metadata.len() > self.max_file_size {
            return Err(format!(
                "File too large: {} bytes (max: {} bytes)",
                metadata.len(),
                self.max_file_size
            ));
        }

        // Detect file type
        let file_type = Self::detect_file_type(path).await?;

        match file_type {
            FileType::Text => {
                let result = self.read_text_file(path, offset, limit).await?;
                let mut output = result.content;

                if result.is_truncated {
                    let next_offset = result.lines_shown.1;
                    output.push_str(&format!(
                        "\n\n[TRUNCATED]\nShowing lines {}-{} of {} total lines.\n",
                        result.lines_shown.0, result.lines_shown.1, result.original_line_count
                    ));
                    if next_offset < result.original_line_count {
                        output.push_str(&format!(
                            "To read more, use: offset: {}, limit: {}\n",
                            next_offset, self.default_max_lines
                        ));
                    }
                }

                Ok(output)
            }
            FileType::Svg => {
                if metadata.len() > SVG_MAX_SIZE {
                    return Err(format!(
                        "SVG file too large: {} bytes (max: {} MB)",
                        metadata.len(),
                        SVG_MAX_SIZE / 1024 / 1024
                    ));
                }
                Self::read_file_with_encoding(path).await
            }
            FileType::Image => {
                if supports_vision {
                    match self.read_image_for_vision(path).await {
                        Ok(result) => Ok(result),
                        Err(e) => {
                            log::warn!(
                                "[Read] Image read failed, falling back to placeholder: {}",
                                e
                            );
                            Ok(format!(
                                "[Image file: {}]\nFailed to read for vision: {}",
                                path.display(),
                                e
                            ))
                        }
                    }
                } else {
                    Ok(format!(
                        "[Image file: {}]\nUse the file tool to copy or view this image.",
                        path.display()
                    ))
                }
            }
            FileType::Pdf => Ok(format!(
                "[PDF file: {}]\nUse a PDF viewer to read this file.",
                path.display()
            )),
            FileType::Audio => Ok(format!(
                "[Audio file: {}]\nUse an audio player to listen to this file.",
                path.display()
            )),
            FileType::Video => Ok(format!(
                "[Video file: {}]\nUse a video player to watch this file.",
                path.display()
            )),
            FileType::Binary => Ok(format!(
                "[Binary file: {}]\nCannot display binary content.",
                path.display()
            )),
        }
    }
}

impl Default for ReadExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolExecutor for ReadExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        let file_path = input["file_path"]
            .as_str()
            .ok_or("Missing 'file_path' parameter")?;

        if file_path.trim().is_empty() {
            return Err("file_path cannot be empty".to_string());
        }

        let offset = input["offset"].as_u64().map(|n| n as usize);
        let limit = input["limit"].as_u64().map(|n| n as usize);

        // Validate offset and limit
        if let Some(off) = offset {
            if off > 1_000_000 {
                return Err("offset is too large (max: 1,000,000)".to_string());
            }
        }

        if let Some(lim) = limit {
            if lim == 0 {
                return Err("limit must be positive".to_string());
            }
            if lim > 100_000 {
                return Err("limit is too large (max: 100,000)".to_string());
            }
        }

        let workdir = input.get("workdir").and_then(|v| v.as_str());
        let supports_vision = input
            .get("__supports_vision")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let path = Self::resolve_input_path(file_path, workdir)?;
        let result = self
            .read_file(&path, offset, limit, supports_vision)
            .await?;

        // Record file mtime for staleness detection by write/edit tools.
        // Only record for files that actually exist on disk (not error paths).
        if let Some(session_id) = input.get("__session_id").and_then(|v| v.as_str()) {
            if path.exists() {
                if let Ok(metadata) = std::fs::metadata(&path) {
                    if let Ok(mtime) = metadata.modified() {
                        let canonical =
                            std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                        super::file_tracker::record_file_read(
                            session_id,
                            &canonical.to_string_lossy(),
                            mtime,
                        );
                    }
                }
            }
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_read_executor_simple_text() {
        let executor = ReadExecutor::new();
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");

        fs::write(&file_path, "Line 1\nLine 2\nLine 3")
            .await
            .unwrap();

        let input = json!({
            "file_path": file_path.to_str().unwrap()
        });

        let result = executor.execute(input).await;
        assert!(result.is_ok());
        let content = result.unwrap();
        assert!(content.contains("Line 1"));
        assert!(content.contains("Line 2"));
        assert!(content.contains("Line 3"));
    }

    #[tokio::test]
    async fn test_read_executor_with_pagination() {
        let executor = ReadExecutor::new();
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");

        let lines: Vec<String> = (1..=100).map(|i| format!("Line {}", i)).collect();
        fs::write(&file_path, lines.join("\n")).await.unwrap();

        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "offset": 10,
            "limit": 5
        });

        let result = executor.execute(input).await;
        assert!(result.is_ok());
        let content = result.unwrap();
        assert!(content.contains("Line 11"));
        assert!(content.contains("Line 15"));
        assert!(!content.contains("Line 10"));
        assert!(!content.contains("Line 16"));
    }

    #[tokio::test]
    async fn test_read_executor_truncation() {
        let executor = ReadExecutor::new();
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("large.txt");

        let lines: Vec<String> = (1..=5000).map(|i| format!("Line {}", i)).collect();
        fs::write(&file_path, lines.join("\n")).await.unwrap();

        let input = json!({
            "file_path": file_path.to_str().unwrap()
        });

        let result = executor.execute(input).await;
        assert!(result.is_ok());
        let content = result.unwrap();
        assert!(content.contains("[TRUNCATED]"));
        assert!(content.contains("Showing lines 1-2000 of 5000"));
    }

    #[tokio::test]
    async fn test_read_executor_file_not_found() {
        let executor = ReadExecutor::new();
        let input = json!({
            "file_path": "/nonexistent/file.txt"
        });

        let result = executor.execute(input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("File not found"));
    }

    #[tokio::test]
    async fn test_read_executor_long_lines() {
        let executor = ReadExecutor::new();
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("long.txt");

        let long_line = "a".repeat(3000);
        fs::write(&file_path, &long_line).await.unwrap();

        let input = json!({
            "file_path": file_path.to_str().unwrap()
        });

        let result = executor.execute(input).await;
        assert!(result.is_ok());
        let content = result.unwrap();
        assert!(content.contains("[truncated]"));
    }

    #[tokio::test]
    async fn test_bom_detection() {
        // UTF-8 Bom
        let utf8_bom = vec![0xEF, 0xBB, 0xBF];
        assert!(matches!(
            ReadExecutor::detect_bom(&utf8_bom),
            Some(Bom::Utf8)
        ));

        // UTF-16 LE Bom
        let utf16le_bom = vec![0xFF, 0xFE];
        assert!(matches!(
            ReadExecutor::detect_bom(&utf16le_bom),
            Some(Bom::Utf16LE)
        ));

        // No Bom
        let no_bom = vec![0x48, 0x65, 0x6C, 0x6C, 0x6F]; // "Hello"
        assert!(ReadExecutor::detect_bom(&no_bom).is_none());
    }

    #[test]
    fn test_validate_path_dangerous() {
        let result = ReadExecutor::resolve_input_path("../../etc/passwd", Some("/tmp/work"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("escapes workdir"));
    }

    #[test]
    fn test_expand_tilde() {
        let home = path_resolver::user_home_dir();
        let expanded = path_resolver::expand_tilde("~/test.txt");
        assert_eq!(expanded, home.join("test.txt"));
    }

    #[test]
    fn test_validate_relative_path_anchors_to_workdir() {
        let resolved =
            ReadExecutor::resolve_input_path("notes/todo.md", Some("/tmp/work")).unwrap();
        assert_eq!(
            resolved,
            PathBuf::from("/tmp/work").join("notes").join("todo.md")
        );
    }

    #[tokio::test]
    async fn test_read_executor_respects_workdir_for_relative_path() {
        let executor = ReadExecutor::new();
        let dir = tempdir().unwrap();
        let nested = dir.path().join("notes");
        fs::create_dir_all(&nested).await.unwrap();
        let file_path = nested.join("todo.md");
        fs::write(&file_path, "Hello\n").await.unwrap();

        let input = json!({
            "file_path": "notes/todo.md",
            "workdir": dir.path().to_str().unwrap()
        });

        let result = executor.execute(input).await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("Hello"));
    }
}
