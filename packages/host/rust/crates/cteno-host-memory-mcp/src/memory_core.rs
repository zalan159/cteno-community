//! Memory core — Markdown file store with keyword grep search.
//!
//! Ported from Cteno 1.0 `apps/desktop/src-tauri/src/memory.rs` with the Tauri
//! dependency stripped. Two scopes live side-by-side:
//! - `Scope::Project` → `{project_dir}/.cteno/memory/`
//! - `Scope::Global`  → `{global_dir}` (e.g. `~/.cteno/memory/`)
//!
//! `recall` always searches both scopes and tags results `[project]` / `[global]`.
//! `save` / `read` / `list` respect an explicit `scope` argument.

use lazy_static::lazy_static;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

const PROJECT_MEMORY_SUBDIR: &str = ".cteno/memory";
const SKIP_DIR_NAMES: &[&str] = &["agents", "skills", "target", "node_modules"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Project,
    Global,
}

impl Scope {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "project" | "private" => Ok(Scope::Project),
            "global" => Ok(Scope::Global),
            other => Err(format!(
                "invalid scope {other:?}, expected 'project' or 'global'"
            )),
        }
    }

    pub fn as_tag(self) -> &'static str {
        match self {
            Scope::Project => "project",
            Scope::Global => "global",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryChunk {
    pub scope: String,
    pub file_path: String,
    pub content: String,
    pub score: f32,
}

pub struct MemoryCore {
    project_root: Option<PathBuf>,
    global_dir: PathBuf,
}

impl MemoryCore {
    pub fn new(project_dir: Option<PathBuf>, global_dir: PathBuf) -> Self {
        Self {
            project_root: project_dir,
            global_dir,
        }
    }

    fn project_dir(&self) -> Option<PathBuf> {
        self.project_root
            .as_ref()
            .map(|p| p.join(PROJECT_MEMORY_SUBDIR))
    }

    fn dir_for(&self, scope: Scope) -> Result<PathBuf, String> {
        match scope {
            Scope::Project => self
                .project_dir()
                .ok_or_else(|| "Project scope requested but no project_dir set".to_string()),
            Scope::Global => Ok(self.global_dir.clone()),
        }
    }

    pub fn save(
        &self,
        file_path: &str,
        content: &str,
        scope: Scope,
        memory_type: Option<&str>,
    ) -> Result<String, String> {
        let normalized = normalize_memory_file_path(file_path)?;
        let base = self.dir_for(scope)?;
        let full_path = base.join(&normalized);

        let content_with_meta = if let Some(t) = memory_type {
            let date = chrono::Local::now().format("%Y-%m-%d").to_string();
            let prefix = if !content.is_empty() && !content.starts_with('\n') {
                ""
            } else {
                ""
            };
            format!("---\ntype: {t}\ndate: {date}\n---\n{prefix}{content}")
        } else {
            content.to_string()
        };

        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }

        let existing = if full_path.exists() {
            std::fs::read_to_string(&full_path).map_err(|e| e.to_string())?
        } else {
            String::new()
        };
        let separator = if existing.is_empty() || existing.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        let new_content = format!("{existing}{separator}{content_with_meta}");

        std::fs::write(&full_path, &new_content).map_err(|e| e.to_string())?;
        Ok(format!("Saved to [{}] {}", scope.as_tag(), normalized))
    }

    pub fn read(&self, file_path: &str, scope: Scope) -> Result<Option<String>, String> {
        let normalized = normalize_memory_file_path(file_path)?;
        let full_path = self.dir_for(scope)?.join(&normalized);
        if !full_path.exists() {
            return Ok(None);
        }
        std::fs::read_to_string(&full_path)
            .map(Some)
            .map_err(|e| e.to_string())
    }

    pub fn list(&self, scope: Option<Scope>) -> Result<Vec<String>, String> {
        let mut files = Vec::new();
        let scopes: &[Scope] = match scope {
            Some(s) => std::slice::from_ref(match s {
                Scope::Project => &Scope::Project,
                Scope::Global => &Scope::Global,
            }),
            None => &[Scope::Project, Scope::Global],
        };
        for s in scopes {
            let Ok(dir) = self.dir_for(*s) else { continue };
            if !dir.exists() {
                continue;
            }
            for rel in walk_md_files(&dir, &dir)? {
                files.push(format!("[{}] {}", s.as_tag(), rel));
            }
        }
        Ok(files)
    }

    pub fn recall(
        &self,
        query: &str,
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

        let mut results: Vec<MemoryChunk> = Vec::new();
        for scope in [Scope::Project, Scope::Global] {
            let Ok(dir) = self.dir_for(scope) else {
                continue;
            };
            if !dir.exists() {
                continue;
            }
            walk_and_collect(&dir, &dir, &keywords, scope, &mut results)?;
        }

        if let Some(filter) = type_filter {
            results.retain(|c| extract_memory_type(&c.content).as_deref() == Some(filter));
        }

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(limit.max(1));
        Ok(results)
    }

    pub fn delete(&self, file_path: &str, scope: Scope) -> Result<(), String> {
        let normalized = normalize_memory_file_path(file_path)?;
        let full_path = self.dir_for(scope)?.join(&normalized);
        if !full_path.exists() {
            return Err(format!(
                "File not found in [{}]: {}",
                scope.as_tag(),
                normalized
            ));
        }
        std::fs::remove_file(&full_path).map_err(|e| e.to_string())
    }
}

fn normalize_memory_file_path(file_path: &str) -> Result<String, String> {
    let trimmed = file_path.trim();
    if trimmed.is_empty() {
        return Err("Memory file_path cannot be empty".to_string());
    }
    let path = Path::new(trimmed);
    if path.is_absolute() {
        return Err("Memory file_path must be scope-relative".to_string());
    }
    let mut components: Vec<String> = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(segment) => {
                let value = segment.to_string_lossy().trim().to_string();
                if !value.is_empty() {
                    components.push(value);
                }
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("Memory file_path cannot contain '..' or absolute prefixes".into());
            }
        }
    }
    if components.is_empty() {
        return Err("Memory file_path cannot be empty".to_string());
    }
    let joined = components.join("/");
    let needs_ext = !joined.to_lowercase().ends_with(".md");
    Ok(if needs_ext {
        format!("{joined}.md")
    } else {
        joined
    })
}

fn extract_keywords(query: &str) -> Vec<String> {
    lazy_static! {
        static ref TOKEN_RE: Regex = Regex::new(r"[\p{L}\p{N}_]+").expect("valid token regex");
    }
    TOKEN_RE
        .find_iter(query)
        .map(|m| m.as_str().to_lowercase())
        .filter(|t| !t.is_empty())
        .collect()
}

fn split_into_chunks(content: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let sections: Vec<&str> = content.split("\n## ").collect();
    for (i, section) in sections.iter().enumerate() {
        let section = if i == 0 {
            section.to_string()
        } else {
            format!("## {section}")
        };
        if section.len() <= 1000 {
            let t = section.trim();
            if !t.is_empty() {
                chunks.push(t.to_string());
            }
        } else {
            for para in section.split("\n\n") {
                let t = para.trim();
                if !t.is_empty() {
                    chunks.push(t.to_string());
                }
            }
        }
    }
    chunks
}

fn keyword_score(chunk: &str, keywords: &[String]) -> f32 {
    if keywords.is_empty() {
        return 0.0;
    }
    let lower = chunk.to_lowercase();
    let matched = keywords
        .iter()
        .filter(|k| lower.contains(k.as_str()))
        .count();
    matched as f32 / keywords.len() as f32
}

fn walk_and_collect(
    dir: &Path,
    base: &Path,
    keywords: &[String],
    scope: Scope,
    results: &mut Vec<MemoryChunk>,
) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("read_dir {dir:?}: {e}"))?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_dir() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if SKIP_DIR_NAMES.contains(&name) {
                    continue;
                }
            }
            walk_and_collect(&path, base, keywords, scope, results)?;
        } else if path.extension().is_some_and(|e| e == "md") {
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let rel = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            for chunk_text in split_into_chunks(&content) {
                let score = keyword_score(&chunk_text, keywords);
                if score > 0.0 {
                    results.push(MemoryChunk {
                        scope: scope.as_tag().to_string(),
                        file_path: rel.clone(),
                        content: chunk_text,
                        score,
                    });
                }
            }
        }
    }
    Ok(())
}

fn walk_md_files(dir: &Path, base: &Path) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    walk_md(dir, base, &mut files).map_err(|e| e.to_string())?;
    files.sort();
    Ok(files)
}

fn walk_md(dir: &Path, base: &Path, out: &mut Vec<String>) -> std::io::Result<()> {
    if !dir.exists() || !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if SKIP_DIR_NAMES.contains(&name) {
                    continue;
                }
            }
            walk_md(&path, base, out)?;
        } else if path.extension().is_some_and(|e| e == "md") {
            if let Ok(rel) = path.strip_prefix(base) {
                out.push(rel.to_string_lossy().to_string());
            }
        }
    }
    Ok(())
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn new_core(proj: &TempDir, global: &TempDir) -> MemoryCore {
        MemoryCore::new(Some(proj.path().to_path_buf()), global.path().to_path_buf())
    }

    #[test]
    fn save_and_recall_round_trip_across_scopes() {
        let proj = TempDir::new().unwrap();
        let global = TempDir::new().unwrap();
        let core = new_core(&proj, &global);

        core.save(
            "knowledge/rust",
            "Rust ownership is about borrowing rules",
            Scope::Project,
            None,
        )
        .unwrap();
        core.save(
            "memory/general",
            "Cosmic rays corrupt memory unpredictably",
            Scope::Global,
            None,
        )
        .unwrap();

        let hits = core.recall("rust ownership", 10, None).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].scope, "project");
        assert!(hits[0].content.contains("borrowing"));

        let hits2 = core.recall("cosmic rays memory", 10, None).unwrap();
        assert_eq!(hits2[0].scope, "global");
    }

    #[test]
    fn normalize_rejects_parent_escape() {
        let err = normalize_memory_file_path("../secrets").unwrap_err();
        assert!(err.contains(".."));
    }

    #[test]
    fn normalize_adds_md_extension() {
        let out = normalize_memory_file_path("knowledge/rust").unwrap();
        assert_eq!(out, "knowledge/rust.md");
    }

    #[test]
    fn recall_filters_by_type() {
        let proj = TempDir::new().unwrap();
        let global = TempDir::new().unwrap();
        let core = new_core(&proj, &global);
        core.save("a", "Alpha content", Scope::Project, Some("user"))
            .unwrap();
        core.save("b", "Alpha beta content", Scope::Project, Some("feedback"))
            .unwrap();
        let only_feedback = core.recall("alpha", 10, Some("feedback")).unwrap();
        assert_eq!(only_feedback.len(), 1);
        assert!(only_feedback[0].content.contains("beta"));
    }

    #[test]
    fn save_refuses_without_project_dir() {
        let global = TempDir::new().unwrap();
        let core = MemoryCore::new(None, global.path().to_path_buf());
        let err = core.save("x", "y", Scope::Project, None).unwrap_err();
        assert!(err.contains("no project_dir"));
    }

    #[test]
    fn list_tags_scopes() {
        let proj = TempDir::new().unwrap();
        let global = TempDir::new().unwrap();
        let core = new_core(&proj, &global);
        core.save("a", "A", Scope::Project, None).unwrap();
        core.save("b", "B", Scope::Global, None).unwrap();
        let all = core.list(None).unwrap();
        assert!(all.iter().any(|s| s.starts_with("[project]")));
        assert!(all.iter().any(|s| s.starts_with("[global]")));
    }
}
