#![cfg_attr(not(feature = "tauri-commands"), allow(dead_code))]

//! Memory Layer
//!
//! Pure Markdown file-based memory system with keyword grep search.
//! Each persona has a private memory space + access to global memory.

use lazy_static::lazy_static;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::{Component, Path, PathBuf},
};

#[cfg(feature = "tauri-commands")]
pub mod commands;

/// Memory chunk returned from search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryChunk {
    pub file_path: String, // workspace-relative
    pub content: String,   // chunk content
    pub score: f32,        // keyword match score
}

/// Memory search options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchOptions {
    pub limit: Option<usize>,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self { limit: Some(10) }
    }
}

/// Get workspace directory
fn get_workspace_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("workspace")
}

fn normalize_memory_file_path(file_path: &str) -> Result<String, String> {
    let trimmed = file_path.trim();
    if trimmed.is_empty() {
        return Err("Memory file_path cannot be empty".to_string());
    }
    let path = Path::new(trimmed);
    if path.is_absolute() {
        return Err("Memory file_path must be workspace-relative".to_string());
    }

    let mut components: Vec<String> = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(segment) => {
                let value = segment.to_string_lossy().trim().to_string();
                if value.is_empty() {
                    continue;
                }
                components.push(value);
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("Memory file_path cannot contain '..' or absolute prefixes".to_string());
            }
        }
    }

    if components.is_empty() {
        return Err("Memory file_path cannot be empty".to_string());
    }

    Ok(components.join("/"))
}

const PRIVATE_MEMORY_SUBDIR: &str = ".cteno/memory";
const LEGACY_PRIVATE_MEMORY_SUBDIR: &str = ".cteno";
const LEGACY_PRIVATE_SKIP_FILES: &[&str] = &["PROMPT.md"];

/// Resolve private memory directory from persona's workdir.
/// Returns `{workdir}/.cteno/memory/` if persona_workdir is given, None otherwise.
fn private_memory_dir(persona_workdir: Option<&str>) -> Option<PathBuf> {
    persona_workdir.map(|w| {
        let expanded = shellexpand::tilde(w).to_string();
        PathBuf::from(expanded).join(PRIVATE_MEMORY_SUBDIR)
    })
}

/// Legacy Cteno 1.x/early-2.x private memory lived directly under `.cteno/`.
/// Keep it readable so existing users do not lose access after the project
/// scope moves to the same `.cteno/memory/` directory used by memory MCP.
fn legacy_private_memory_dir(persona_workdir: Option<&str>) -> Option<PathBuf> {
    persona_workdir.map(|w| {
        let expanded = shellexpand::tilde(w).to_string();
        PathBuf::from(expanded).join(LEGACY_PRIVATE_MEMORY_SUBDIR)
    })
}

/// Extract search keywords from query
fn extract_keywords(query: &str) -> Vec<String> {
    lazy_static! {
        static ref TOKEN_RE: Regex = Regex::new(r"[\p{L}\p{N}_]+").expect("valid token regex");
    }

    TOKEN_RE
        .find_iter(query)
        .map(|m| m.as_str().to_lowercase())
        .filter(|token| !token.is_empty())
        .collect()
}

/// Split content into indexable chunks (by ## headers, then by paragraphs for large sections)
fn split_into_chunks(content: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let sections: Vec<&str> = content.split("\n## ").collect();

    for (i, section) in sections.iter().enumerate() {
        let section = if i == 0 {
            section.to_string()
        } else {
            format!("## {}", section)
        };

        if section.len() <= 1000 {
            if !section.trim().is_empty() {
                chunks.push(section.trim().to_string());
            }
        } else {
            for para in section.split("\n\n") {
                if !para.trim().is_empty() {
                    chunks.push(para.trim().to_string());
                }
            }
        }
    }

    chunks
}

/// Calculate keyword match score for a chunk
fn keyword_score(chunk: &str, keywords: &[String]) -> f32 {
    if keywords.is_empty() {
        return 0.0;
    }
    let lower = chunk.to_lowercase();
    let mut matched = 0;
    for kw in keywords {
        if lower.contains(kw.as_str()) {
            matched += 1;
        }
    }
    matched as f32 / keywords.len() as f32
}

/// Collect matching chunks from a directory of .md files
fn collect_matching_chunks(
    dir: &Path,
    base: &Path,
    keywords: &[String],
    results: &mut Vec<MemoryChunk>,
) -> Result<(), String> {
    if !dir.exists() || !dir.is_dir() {
        return Ok(());
    }
    walk_and_collect(dir, base, keywords, results, None)
}

/// Collect matching chunks from global workspace
fn collect_matching_chunks_global(
    workspace: &Path,
    keywords: &[String],
    results: &mut Vec<MemoryChunk>,
) -> Result<(), String> {
    walk_and_collect(workspace, workspace, keywords, results, None)
}

/// Directory names to skip when scanning for memory files.
const SKIP_DIR_NAMES: &[&str] = &["agents", "target"];

fn walk_and_collect(
    dir: &Path,
    base: &Path,
    keywords: &[String],
    results: &mut Vec<MemoryChunk>,
    skip: Option<&Path>,
) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("read_dir {:?}: {}", dir, e))?;

    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();

        if let Some(skip_path) = skip {
            if path == skip_path {
                continue;
            }
        }

        if path.is_dir() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if SKIP_DIR_NAMES.contains(&name) {
                    continue;
                }
            }
            walk_and_collect(&path, base, keywords, results, skip)?;
        } else if path.extension().is_some_and(|e| e == "md") {
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let rel_path = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();

            for chunk_text in split_into_chunks(&content) {
                let score = keyword_score(&chunk_text, keywords);
                if score > 0.0 {
                    results.push(MemoryChunk {
                        file_path: rel_path.clone(),
                        content: chunk_text,
                        score,
                    });
                }
            }
        }
    }

    Ok(())
}

/// Extract the `type` field from YAML frontmatter in memory content.
/// Returns None if no frontmatter or no type field.
fn extract_memory_type(content: &str) -> Option<String> {
    if !content.starts_with("---\n") {
        return None;
    }
    let end = content[4..].find("\n---\n")?;
    let frontmatter = &content[4..4 + end];
    for line in frontmatter.lines() {
        if let Some(t) = line.strip_prefix("type: ") {
            return Some(t.trim().to_string());
        }
    }
    None
}

// ============================================================
// Core functions (no AppHandle dependency, for tool executors)
// ============================================================

/// Read a memory file. With persona_workdir: try private first, then global.
pub fn memory_read_core(
    workspace: &Path,
    file_path: &str,
    persona_workdir: Option<&str>,
) -> Result<Option<String>, String> {
    let normalized = normalize_memory_file_path(file_path)?;

    // If persona_workdir given, try private space first
    if let Some(private_dir) = private_memory_dir(persona_workdir) {
        let private_path = private_dir.join(&normalized);
        if private_path.exists() {
            return std::fs::read_to_string(&private_path)
                .map(Some)
                .map_err(|e| e.to_string());
        }
    }
    if let Some(legacy_dir) = legacy_private_memory_dir(persona_workdir) {
        let legacy_path = legacy_dir.join(&normalized);
        if legacy_path.exists() {
            return std::fs::read_to_string(&legacy_path)
                .map(Some)
                .map_err(|e| e.to_string());
        }
    }

    // Fall back to global
    let global_path = workspace.join(&normalized);
    if !global_path.exists() {
        return Ok(None);
    }
    std::fs::read_to_string(&global_path)
        .map(Some)
        .map_err(|e| e.to_string())
}

/// Write to a memory file (overwrite). persona_workdir determines target space.
pub fn memory_write_core(
    workspace: &Path,
    file_path: &str,
    content: &str,
    persona_workdir: Option<&str>,
) -> Result<(), String> {
    let normalized = normalize_memory_file_path(file_path)?;
    let base = match private_memory_dir(persona_workdir) {
        Some(dir) => dir,
        None => workspace.to_path_buf(),
    };
    let full_path = base.join(&normalized);

    if let Some(parent) = full_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    std::fs::write(&full_path, content).map_err(|e| e.to_string())?;
    log::info!(
        "[Memory] Wrote file: {} (workdir: {:?})",
        normalized,
        persona_workdir
    );
    Ok(())
}

/// Append to a memory file. persona_workdir determines target space.
pub fn memory_append_core(
    workspace: &Path,
    file_path: &str,
    content: &str,
    persona_workdir: Option<&str>,
) -> Result<(), String> {
    let normalized = normalize_memory_file_path(file_path)?;
    let base = match private_memory_dir(persona_workdir) {
        Some(dir) => dir,
        None => workspace.to_path_buf(),
    };
    let full_path = base.join(&normalized);

    let existing = if full_path.exists() {
        std::fs::read_to_string(&full_path).map_err(|e| e.to_string())?
    } else {
        String::new()
    };

    let new_content = format!("{}{}", existing, content);

    if let Some(parent) = full_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    std::fs::write(&full_path, &new_content).map_err(|e| e.to_string())?;
    log::info!(
        "[Memory] Appended to: {} (workdir: {:?})",
        normalized,
        persona_workdir
    );
    Ok(())
}

/// Search memory. Searches persona private space ({workdir}/.cteno/) + global workspace.
/// When `type_filter` is Some, only returns chunks whose frontmatter `type` matches.
pub fn memory_search_core(
    workspace: &Path,
    query: &str,
    persona_workdir: Option<&str>,
    limit: usize,
    type_filter: Option<&str>,
) -> Result<Vec<MemoryChunk>, String> {
    let cleaned = query.trim();
    if cleaned.is_empty() {
        return Ok(Vec::new());
    }

    let keywords = extract_keywords(cleaned);
    if keywords.is_empty() {
        return Ok(Vec::new());
    }

    let mut all_chunks = Vec::new();

    // 1. Search persona private directory ({workdir}/.cteno/memory/) if given
    if let Some(private_dir) = private_memory_dir(persona_workdir) {
        collect_matching_chunks(&private_dir, &private_dir, &keywords, &mut all_chunks)?;
        // Tag private results
        for chunk in &mut all_chunks {
            chunk.file_path = format!("[private] {}", chunk.file_path);
        }
    }

    // Legacy private directory ({workdir}/.cteno/) remains searchable. Skip the
    // new memory subdirectory so the same files do not appear twice.
    if let Some(legacy_dir) = legacy_private_memory_dir(persona_workdir) {
        let primary_dir = private_memory_dir(persona_workdir);
        let mut legacy_chunks = Vec::new();
        walk_and_collect(
            &legacy_dir,
            &legacy_dir,
            &keywords,
            &mut legacy_chunks,
            primary_dir.as_deref(),
        )?;
        legacy_chunks
            .retain(|chunk| !LEGACY_PRIVATE_SKIP_FILES.contains(&chunk.file_path.as_str()));
        for chunk in &mut legacy_chunks {
            chunk.file_path = format!("[private] {}", chunk.file_path);
        }
        all_chunks.extend(legacy_chunks);
    }

    // 2. Search global directory
    let global_start = all_chunks.len();
    collect_matching_chunks_global(workspace, &keywords, &mut all_chunks)?;
    // Tag global results
    for chunk in &mut all_chunks[global_start..] {
        chunk.file_path = format!("[global] {}", chunk.file_path);
    }

    // 3. Apply type filter if specified
    if let Some(filter) = type_filter {
        all_chunks.retain(|chunk| {
            extract_memory_type(&chunk.content)
                .map(|t| t == filter)
                .unwrap_or(false)
        });
    }

    // 4. Sort by score descending, truncate
    all_chunks.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    all_chunks.truncate(limit);
    Ok(all_chunks)
}

/// List memory files. With persona_workdir: list private ({workdir}/.cteno/memory/) + global with tags.
pub fn memory_list_core(
    workspace: &Path,
    persona_workdir: Option<&str>,
) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    let mut private_seen = HashSet::new();

    // Private files from {workdir}/.cteno/memory/
    if let Some(private_dir) = private_memory_dir(persona_workdir) {
        if private_dir.exists() {
            let private_files = walk_md_files(&private_dir, &private_dir)?;
            for f in private_files {
                private_seen.insert(f.clone());
                files.push(format!("[private] {}", f));
            }
        }
    }

    // Legacy private files from {workdir}/.cteno/, excluding `.cteno/memory`.
    if let Some(legacy_dir) = legacy_private_memory_dir(persona_workdir) {
        if legacy_dir.exists() {
            let primary_dir = private_memory_dir(persona_workdir);
            let legacy_files = walk_md_files_skip(
                &legacy_dir,
                &legacy_dir,
                primary_dir.as_deref().unwrap_or(Path::new("")),
            )?;
            for f in legacy_files {
                if LEGACY_PRIVATE_SKIP_FILES.contains(&f.as_str()) {
                    continue;
                }
                if private_seen.insert(f.clone()) {
                    files.push(format!("[private] {}", f));
                }
            }
        }
    }

    // Global files
    let global_files = walk_md_files(workspace, workspace)?;
    for f in global_files {
        files.push(format!("[global] {}", f));
    }

    Ok(files)
}

/// Delete a single memory file. Scope-aware: deletes from private or global.
pub fn memory_delete_core(
    workspace: &Path,
    file_path: &str,
    persona_workdir: Option<&str>,
) -> Result<(), String> {
    let normalized = normalize_memory_file_path(file_path)?;

    // Determine target directory based on scope. Private deletes prefer the
    // current project-memory directory, then fall back to the legacy `.cteno/`
    // location so old files shown in the UI remain manageable.
    let base = match private_memory_dir(persona_workdir) {
        Some(dir) => dir,
        None => workspace.to_path_buf(),
    };
    let mut full_path = base.join(&normalized);
    let mut cleanup_base = base.clone();
    if !full_path.exists() {
        if let Some(legacy_dir) = legacy_private_memory_dir(persona_workdir) {
            let legacy_path = legacy_dir.join(&normalized);
            if legacy_path.exists() {
                full_path = legacy_path;
                cleanup_base = legacy_dir;
            }
        }
    }

    if !full_path.exists() {
        return Err(format!("File not found: {}", normalized));
    }

    std::fs::remove_file(&full_path).map_err(|e| e.to_string())?;

    // Clean up empty parent directories
    if let Some(parent) = full_path.parent() {
        let _ = remove_empty_parents(parent, &cleanup_base);
    }

    log::info!(
        "[Memory] Deleted file: {} (workdir: {:?})",
        normalized,
        persona_workdir
    );
    Ok(())
}

/// Clean up a persona's private memory directory ({workdir}/.cteno/memory/)
pub fn cleanup_persona_memory(persona_workdir: &str) -> Result<(), String> {
    let expanded = shellexpand::tilde(persona_workdir).to_string();
    let dir = PathBuf::from(expanded).join(PRIVATE_MEMORY_SUBDIR);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
        log::info!("[Memory] Cleaned up persona memory: {}", persona_workdir);
    }
    Ok(())
}

// ============================================================
// Internal helpers
// ============================================================

/// Remove empty parent directories up to (but not including) the base directory.
fn remove_empty_parents(dir: &Path, base: &Path) -> Result<(), std::io::Error> {
    let mut current = dir.to_path_buf();
    while current != base && current.starts_with(base) {
        if current.is_dir() && std::fs::read_dir(&current)?.next().is_none() {
            std::fs::remove_dir(&current)?;
        } else {
            break;
        }
        match current.parent() {
            Some(p) => current = p.to_path_buf(),
            None => break,
        }
    }
    Ok(())
}

fn walk_md_files(dir: &Path, base: &Path) -> Result<Vec<String>, String> {
    walk_md_files_skip(dir, base, Path::new(""))
}

fn walk_md_files_skip(dir: &Path, base: &Path, skip: &Path) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    if !dir.exists() || !dir.is_dir() {
        return Ok(files);
    }

    fn walk(dir: &Path, base: &Path, skip: &Path, files: &mut Vec<String>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if !skip.as_os_str().is_empty() && path == skip {
                continue;
            }
            if path.is_dir() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if SKIP_DIR_NAMES.contains(&name) {
                        continue;
                    }
                }
                walk(&path, base, skip, files)?;
            } else if path.extension().is_some_and(|e| e == "md") {
                if let Ok(rel) = path.strip_prefix(base) {
                    files.push(rel.to_string_lossy().to_string());
                }
            }
        }
        Ok(())
    }

    walk(dir, base, skip, &mut files).map_err(|e| e.to_string())?;
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_dir(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("cteno-memory-test-{name}-{nonce}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn normalize_memory_file_path_accepts_relative() {
        let normalized = normalize_memory_file_path("knowledge/rust.md").unwrap();
        assert_eq!(normalized, "knowledge/rust.md");
    }

    #[test]
    fn normalize_memory_file_path_rejects_parent_escape() {
        let err = normalize_memory_file_path("../secrets.md").unwrap_err();
        assert!(err.contains("cannot contain '..'"));
    }

    #[test]
    fn extract_keywords_works() {
        let kws = extract_keywords("价格 2026年 API");
        assert_eq!(kws.len(), 3);
        assert!(kws.contains(&"api".to_string()));
    }

    #[test]
    fn keyword_score_basic() {
        let score = keyword_score(
            "This is about API pricing",
            &["api".to_string(), "pricing".to_string()],
        );
        assert!((score - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn keyword_score_partial() {
        let score = keyword_score(
            "This is about API",
            &["api".to_string(), "pricing".to_string()],
        );
        assert!((score - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn private_memory_writes_to_project_memory_subdir() {
        let root = test_dir("private-write");
        let workspace = root.join("global");
        let project = root.join("project");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&project).unwrap();

        memory_write_core(
            &workspace,
            "knowledge/rust.md",
            "project scoped note",
            Some(project.to_str().unwrap()),
        )
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(project.join(".cteno/memory/knowledge/rust.md")).unwrap(),
            "project scoped note"
        );
        assert_eq!(
            memory_read_core(
                &workspace,
                "knowledge/rust.md",
                Some(project.to_str().unwrap())
            )
            .unwrap()
            .as_deref(),
            Some("project scoped note")
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn private_memory_lists_and_deletes_legacy_cteno_files() {
        let root = test_dir("legacy-private");
        let workspace = root.join("global");
        let project = root.join("project");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(project.join(".cteno/knowledge")).unwrap();
        std::fs::write(project.join(".cteno/knowledge/old.md"), "legacy note").unwrap();

        let files = memory_list_core(&workspace, Some(project.to_str().unwrap())).unwrap();
        assert!(files.contains(&"[private] knowledge/old.md".to_string()));
        assert_eq!(
            memory_read_core(
                &workspace,
                "knowledge/old.md",
                Some(project.to_str().unwrap())
            )
            .unwrap()
            .as_deref(),
            Some("legacy note")
        );

        memory_delete_core(
            &workspace,
            "knowledge/old.md",
            Some(project.to_str().unwrap()),
        )
        .unwrap();
        assert!(!project.join(".cteno/knowledge/old.md").exists());

        let _ = std::fs::remove_dir_all(root);
    }
}
