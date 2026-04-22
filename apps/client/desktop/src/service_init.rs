//! Service Initialization
//!
//! Initializes all services: ToolRegistry, MCP, Scheduler, PersonaManager, etc.
//! All internal callers use direct in-process calls via `local_services`.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn happy_server_url() -> String {
    crate::resolved_happy_server_url()
}

// ============================================================================
// Public Skill Loading (accessible from other modules)
// ============================================================================

/// Sync builtin skills into the unified directory.
/// For each skill in `builtin_dir`, copies it to `unified_dir` if not already present,
/// or upgrades it if the builtin version is newer.
/// Also removes skills marked as "builtin" that no longer exist in `builtin_dir`.
pub fn sync_builtin_skills(builtin_dir: &std::path::Path, unified_dir: &std::path::Path) {
    // Collect current builtin skill names
    let builtin_names: std::collections::HashSet<String> = if builtin_dir.exists() {
        fs::read_dir(builtin_dir)
            .ok()
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| {
                        let name = e.file_name().to_string_lossy().to_string();
                        e.path().is_dir() && !name.starts_with('.') && name != "SKILL.md"
                    })
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect()
            })
            .unwrap_or_default()
    } else {
        std::collections::HashSet::new()
    };

    // Remove orphaned builtin skills from unified directory
    if unified_dir.exists() {
        if let Ok(entries) = fs::read_dir(unified_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !entry.path().is_dir() || name.starts_with('.') {
                    continue;
                }
                // Check if this skill is marked as builtin but no longer exists in builtin_dir
                if !builtin_names.contains(&name) {
                    if let Some(meta) = read_source_meta(&entry.path()) {
                        if meta.source_type == "builtin" {
                            log::info!(
                                "Removing orphaned builtin skill '{}' (no longer in builtin dir)",
                                name
                            );
                            if let Err(e) = fs::remove_dir_all(entry.path()) {
                                log::warn!("Failed to remove orphaned skill '{}': {}", name, e);
                            }
                        }
                    }
                }
            }
        }
    }

    // Sync current builtin skills
    if !builtin_dir.exists() {
        return;
    }
    let entries = match fs::read_dir(builtin_dir) {
        Ok(e) => e,
        Err(e) => {
            log::warn!("Failed to read builtin skills dir {:?}: {}", builtin_dir, e);
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if !path.is_dir() || name.starts_with('.') || name == "SKILL.md" {
            continue;
        }
        let dest = unified_dir.join(&name);
        let builtin_digest = compute_skill_digest(&path);
        if dest.exists() {
            let source_meta = read_source_meta(&dest);

            // Prefer digest-based upgrade for builtin-managed skills.
            // This handles updates where version is unchanged or missing.
            let digest_upgrade_reason = match (builtin_digest.as_deref(), source_meta.as_ref()) {
                (Some(expected), Some(meta))
                    if meta.source_type == "builtin"
                        && meta
                            .source_key
                            .as_deref()
                            .map(|k| k == name)
                            .unwrap_or(true) =>
                {
                    match meta.source_digest.as_deref() {
                        Some(existing) if existing == expected => None,
                        Some(existing) => {
                            Some(format!("digest changed ({} -> {})", existing, expected))
                        }
                        None => Some("missing digest in source metadata".to_string()),
                    }
                }
                _ => None,
            };

            if let Some(reason) = digest_upgrade_reason {
                log::info!("Upgrading builtin skill '{}': {}", name, reason);
                if let Err(e) = fs::remove_dir_all(&dest) {
                    log::warn!("Failed to remove old skill '{}': {}", name, e);
                    continue;
                }
            } else {
                // Fallback to version-based upgrade.
                let builtin_version = read_skill_version(&path);
                let existing_version = read_skill_version(&dest);
                match (&builtin_version, &existing_version) {
                    (Some(bv), Some(ev)) if version_is_newer(bv, ev) => {
                        log::info!("Upgrading builtin skill '{}': {} -> {}", name, ev, bv);
                        if let Err(e) = fs::remove_dir_all(&dest) {
                            log::warn!("Failed to remove old skill '{}': {}", name, e);
                            continue;
                        }
                    }
                    (Some(bv), None) => {
                        log::info!(
                            "Upgrading builtin skill '{}' (no existing version -> {})",
                            name,
                            bv
                        );
                        if let Err(e) = fs::remove_dir_all(&dest) {
                            log::warn!("Failed to remove old skill '{}': {}", name, e);
                            continue;
                        }
                    }
                    _ => {
                        // Same version, but ensure source meta is written
                        write_builtin_source_meta(&dest, &name, builtin_digest.as_deref());
                        continue;
                    }
                }
            }
        }
        if let Err(e) = copy_dir_recursive(&path, &dest) {
            log::warn!("Failed to sync builtin skill '{}': {}", name, e);
        } else {
            write_builtin_source_meta(&dest, &name, builtin_digest.as_deref());
            log::info!("Synced builtin skill '{}' to {:?}", name, dest);
        }
    }
}

const SOURCE_META_FILE: &str = ".cteno-source.json";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SkillSourceMeta {
    pub source_type: String,
    #[serde(default)]
    pub source_key: Option<String>,
    #[serde(default)]
    pub source_digest: Option<String>,
}

fn read_source_meta(skill_dir: &std::path::Path) -> Option<SkillSourceMeta> {
    let path = skill_dir.join(SOURCE_META_FILE);
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str::<SkillSourceMeta>(&content).ok()
}

fn write_builtin_source_meta(skill_dir: &std::path::Path, name: &str, source_digest: Option<&str>) {
    let mut meta = serde_json::json!({
        "sourceType": "builtin",
        "sourceKey": name,
        "installedAt": chrono::Utc::now().to_rfc3339()
    });
    if let Some(digest) = source_digest {
        if let Some(obj) = meta.as_object_mut() {
            obj.insert(
                "sourceDigest".to_string(),
                serde_json::Value::String(digest.to_string()),
            );
        }
    }
    let path = skill_dir.join(SOURCE_META_FILE);
    if let Err(e) = fs::write(
        &path,
        serde_json::to_string_pretty(&meta).unwrap_or_default(),
    ) {
        log::warn!(
            "Failed to write source meta for builtin skill '{}': {}",
            name,
            e
        );
    }
}

/// Read the version string from a skill directory's SKILL.md frontmatter.
/// Also checks lowercase `skill.md` as fallback.
fn read_skill_version(skill_dir: &std::path::Path) -> Option<String> {
    let skill_md = skill_dir.join("SKILL.md");
    let content = fs::read_to_string(&skill_md)
        .or_else(|_| fs::read_to_string(skill_dir.join("skill.md")))
        .ok()?;
    if !content.starts_with("---") {
        return None;
    }
    let rest = &content[3..];
    let end_pos = rest.find("\n---")?;
    let yaml_str = &rest[..end_pos];
    let yaml: serde_yaml::Value = serde_yaml::from_str(yaml_str).ok()?;
    if let Some(version) = yaml.get("version").and_then(yaml_scalar_to_version_string) {
        return Some(version);
    }

    yaml.get("metadata")
        .and_then(|meta| meta.get("version"))
        .and_then(yaml_scalar_to_version_string)
}

fn yaml_scalar_to_version_string(value: &serde_yaml::Value) -> Option<String> {
    match value {
        serde_yaml::Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn fnv1a_update(mut hash: u64, bytes: &[u8]) -> u64 {
    const FNV_PRIME: u64 = 1099511628211;
    for b in bytes {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn collect_files_recursive(
    root: &Path,
    current: &Path,
    out: &mut Vec<PathBuf>,
) -> std::io::Result<()> {
    let entries = fs::read_dir(current)?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            // Ignore internal git metadata folders if any.
            if name == ".git" {
                continue;
            }
            collect_files_recursive(root, &path, out)?;
        } else {
            let rel = path
                .strip_prefix(root)
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::Other, "strip_prefix failed"))?
                .to_path_buf();
            if rel.file_name().and_then(|n| n.to_str()) == Some(SOURCE_META_FILE) {
                continue;
            }
            out.push(rel);
        }
    }
    Ok(())
}

/// Compute a deterministic digest for all files in a skill directory.
/// Used to detect content changes when version is unchanged or absent.
fn compute_skill_digest(skill_dir: &Path) -> Option<String> {
    if !skill_dir.exists() {
        return None;
    }

    let mut files = Vec::new();
    collect_files_recursive(skill_dir, skill_dir, &mut files).ok()?;
    files.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));

    let mut hash: u64 = 0xcbf29ce484222325; // FNV-1a offset basis
    for rel in files {
        let rel_str = rel.to_string_lossy();
        hash = fnv1a_update(hash, rel_str.as_bytes());
        hash = fnv1a_update(hash, &[0]);

        let bytes = fs::read(skill_dir.join(&rel)).ok()?;
        hash = fnv1a_update(hash, &bytes);
        hash = fnv1a_update(hash, &[0xff]);
    }

    Some(format!("{:016x}", hash))
}

/// Check if `new_ver` is strictly newer than `old_ver` using semver-style comparison.
/// Supports dotted numeric versions (e.g. "1.2.3"). Falls back to string comparison.
fn version_is_newer(new_ver: &str, old_ver: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> { v.split('.').filter_map(|s| s.parse().ok()).collect() };
    let new_parts = parse(new_ver);
    let old_parts = parse(old_ver);
    if new_parts.is_empty() || old_parts.is_empty() {
        return new_ver > old_ver; // fallback to string comparison
    }
    let max_len = new_parts.len().max(old_parts.len());
    for i in 0..max_len {
        let n = new_parts.get(i).copied().unwrap_or(0);
        let o = old_parts.get(i).copied().unwrap_or(0);
        if n > o {
            return true;
        }
        if n < o {
            return false;
        }
    }
    false // equal
}

/// Recursively copy a directory from `src` to `dst`.
fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

/// Load all skills from builtin + global + workspace directories, merging by ID.
/// Priority: workspace > global > builtin (higher priority overrides lower).
/// Mirrors the three-layer pattern used by `load_all_agents()`.
pub fn load_all_skills(
    builtin_dir: &std::path::Path,
    global_dir: &std::path::Path,
    workspace_dir: Option<&std::path::Path>,
) -> Vec<SkillConfig> {
    // Ensure builtin skills are present in the global directory
    sync_builtin_skills(builtin_dir, global_dir);

    // Collect builtin skill IDs for is_bundled tagging
    let builtin_ids: std::collections::HashSet<String> = if builtin_dir.exists() {
        fs::read_dir(builtin_dir)
            .ok()
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| {
                        let name = e.file_name().to_string_lossy().to_string();
                        e.path().is_dir() && !name.starts_with('.') && name != "SKILL.md"
                    })
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect()
            })
            .unwrap_or_default()
    } else {
        std::collections::HashSet::new()
    };

    // Load global skills (includes synced builtins)
    let mut skills_map = std::collections::HashMap::new();
    log::info!("Loading skills from global: {:?}", global_dir);
    load_skills_from_dir(global_dir, &mut skills_map);

    // Mark builtin skills
    for (id, skill) in skills_map.iter_mut() {
        if builtin_ids.contains(id) {
            skill.is_bundled = true;
        }
    }

    // Load workspace skills (highest priority, overrides global/builtin by ID)
    if let Some(ws_dir) = workspace_dir {
        if ws_dir.exists() {
            log::info!("Loading skills from workspace: {:?}", ws_dir);
            let mut ws_skills = std::collections::HashMap::new();
            load_skills_from_dir(ws_dir, &mut ws_skills);
            for (id, mut skill) in ws_skills {
                skill.source = Some("workspace".to_string());
                skills_map.insert(id, skill);
            }
        }
    }

    let mut skills: Vec<SkillConfig> = skills_map.into_values().collect();
    skills.sort_by(|a, b| a.id.cmp(&b.id));
    skills
}

/// Build a lightweight skill index message for injection into runtime context.
/// Lists all installed skills with name + description + when_to_use, budget-constrained.
/// Aligned with Claude Code's `formatCommandsWithinBudget` approach.
pub fn build_skill_index_message(
    all_skills: &[SkillConfig],
    context_window_tokens: u32,
) -> Option<String> {
    // Filter out skills hidden from the model
    let visible: Vec<&SkillConfig> = all_skills
        .iter()
        .filter(|s| !s.disable_model_invocation)
        .collect();

    if visible.is_empty() {
        return None;
    }

    // Budget: 1% of context window in chars (tokens * 4 chars/token * 0.01)
    let budget = (context_window_tokens as usize) * 4 / 100;

    // Format each skill entry, truncate to 250 chars
    let format_entry = |s: &SkillConfig| -> String {
        let mut entry = format!(
            "- {}: {}",
            s.id,
            s.description.replace('\n', " ").trim().to_string()
        );
        if let Some(ref wtu) = s.when_to_use {
            entry.push_str(&format!(
                " -- {}",
                wtu.replace('\n', " ").trim().to_string()
            ));
        }
        if entry.len() > 250 {
            // Find a valid UTF-8 char boundary at or before 247
            let mut end = 247;
            while end > 0 && !entry.is_char_boundary(end) {
                end -= 1;
            }
            entry.truncate(end);
            entry.push_str("...");
        }
        entry
    };

    // Bundled skills always included
    let mut bundled_entries: Vec<String> = Vec::new();
    let mut non_bundled: Vec<&SkillConfig> = Vec::new();
    for s in &visible {
        if s.is_bundled {
            bundled_entries.push(format_entry(s));
        } else {
            non_bundled.push(s);
        }
    }

    // Sort non-bundled by name
    non_bundled.sort_by(|a, b| a.id.cmp(&b.id));

    // Calculate remaining budget after bundled entries
    let bundled_chars: usize = bundled_entries.iter().map(|e| e.len() + 1).sum();
    let remaining_budget = budget.saturating_sub(bundled_chars);

    let mut non_bundled_entries: Vec<String> = Vec::new();
    let mut used_chars = 0usize;
    let mut truncated_count = 0usize;
    for s in &non_bundled {
        let entry = format_entry(s);
        let entry_len = entry.len() + 1; // +1 for newline
        if used_chars + entry_len <= remaining_budget {
            used_chars += entry_len;
            non_bundled_entries.push(entry);
        } else {
            truncated_count = non_bundled.len() - non_bundled_entries.len();
            break;
        }
    }

    let mut lines = vec![
        "## Available Skills".to_string(),
        "Use the `skill` tool with `activate` operation to load a skill's full instructions."
            .to_string(),
        String::new(),
    ];
    lines.extend(bundled_entries);
    lines.extend(non_bundled_entries);

    if truncated_count > 0 {
        lines.push(format!(
            "... and {} more skill(s) (use `skill` tool with `list` operation to see all)",
            truncated_count
        ));
    }

    Some(lines.join("\n"))
}

// ============================================================================
// Public Agent Loading (accessible from other modules)
// ============================================================================

/// Load all agents from a single directory.
///
/// Supports two on-disk layouts simultaneously:
/// - **Flat** (aligned with Claude / Gemini): `{dir}/{name}.md` — id = filename
///   stem. This is the canonical layout for cross-vendor symlinking; new agents
///   should be authored this way.
/// - **Legacy nested**: `{dir}/{name}/AGENT.md` (+ optional `config.json`). Kept
///   for backward compatibility with existing user installations.
///
/// If an id appears in both layouts under the same `dir`, the flat file wins
/// (preferring the canonical shape).
pub fn load_agents_from_dir(dir: &std::path::Path) -> Vec<AgentConfig> {
    let mut agents: std::collections::HashMap<String, AgentConfig> =
        std::collections::HashMap::new();
    if !dir.exists() {
        return agents.into_values().collect();
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            log::warn!("Failed to read agents dir {:?}: {}", dir, e);
            return agents.into_values().collect();
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        if name.starts_with('.') {
            continue;
        }

        if path.is_file() {
            // Flat format: single markdown file.
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let id = match path.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            if id.eq_ignore_ascii_case("README") {
                continue;
            }
            match load_agent_from_md_file(&path, &id) {
                Ok(config) => {
                    agents.insert(config.id.clone(), config);
                }
                Err(e) => log::warn!("Failed to load flat agent {:?}: {}", path, e),
            }
        } else if path.is_dir() {
            // Nested legacy format: `{dir}/{name}/AGENT.md` (+ config.json).
            if agents.contains_key(&name) {
                // Flat already loaded with this id — flat wins.
                continue;
            }
            match load_agent_from_dir(&path, &name) {
                Ok(config) => {
                    agents.insert(config.id.clone(), config);
                }
                Err(e) => log::warn!("Failed to load agent {}: {}", name, e),
            }
        }
    }

    let mut list: Vec<AgentConfig> = agents.into_values().collect();
    list.sort_by(|a, b| a.id.cmp(&b.id));
    list
}

/// Load a single flat-format agent file (`{name}.md`).
fn load_agent_from_md_file(
    md_path: &std::path::Path,
    default_id: &str,
) -> Result<AgentConfig, String> {
    let content =
        fs::read_to_string(md_path).map_err(|e| format!("Failed to read {:?}: {}", md_path, e))?;
    parse_agent_md(&content, default_id)
}

/// Load all agents from builtin + global + workspace directories, merging by ID.
/// Priority: workspace > global > builtin (higher priority overrides lower).
pub fn load_all_agents(
    builtin_dir: &std::path::Path,
    global_dir: &std::path::Path,
    workspace_dir: Option<&std::path::Path>,
) -> Vec<AgentConfig> {
    let mut agents_map = std::collections::HashMap::new();

    // Load builtin agents (lowest priority)
    for mut agent in load_agents_from_dir(builtin_dir) {
        agent.source = Some("builtin".to_string());
        agents_map.insert(agent.id.clone(), agent);
    }

    // Load global agents (overrides builtin by ID)
    for mut agent in load_agents_from_dir(global_dir) {
        agent.source = Some("global".to_string());
        agents_map.insert(agent.id.clone(), agent);
    }

    // Load workspace agents (highest priority, overrides global/builtin)
    if let Some(ws_dir) = workspace_dir {
        for mut agent in load_agents_from_dir(ws_dir) {
            agent.source = Some("workspace".to_string());
            agents_map.insert(agent.id.clone(), agent);
        }
    }

    let mut agents: Vec<AgentConfig> = agents_map.into_values().collect();
    agents.sort_by(|a, b| a.id.cmp(&b.id));
    agents
}

/// Get agents that are exposed as tools (for parent agent to call)
pub fn get_agent_tools(agents: &[AgentConfig]) -> Vec<AgentConfig> {
    agents
        .iter()
        .filter(|a| a.expose_as_tool.unwrap_or(false))
        .cloned()
        .collect()
}

// `AgentConfig::to_tool` migrated to `cteno_agent_runtime::agent_config` in
// Wave 2.2b (called by `autonomous_agent::build_agent_tools`).

// ============================================================================
// Skill Types
// ============================================================================

// All skill/agent config types (including `SkillConfig`) now live in
// `cteno_agent_runtime::agent_config` (Wave 2.3a).  The host crate re-exports
// them so existing call sites keep resolving; FS loaders (`load_all_skills`,
// `load_all_agents`) still live here because they own the layout.
pub use cteno_agent_runtime::agent_config::{
    AgentConfig, AgentRoutingConfig, AgentSessionConfig, AgentType, SkillConfig, SkillContext,
    SkillParam, StringOrVec,
};

// ============================================================================
// Agent Types — migrated to cteno_agent_runtime::agent_config (see top of file)
// ============================================================================

// ============================================================================
// Skill / Agent Loading Helpers
// ============================================================================

/// Parse SKILL.md file with YAML frontmatter
fn parse_skill_md(content: &str, default_id: &str) -> Result<SkillConfig, String> {
    if !content.starts_with("---") {
        return Err("SKILL.md must start with YAML frontmatter (---)".to_string());
    }

    let rest = &content[3..];
    let end_pos = rest
        .find("\n---")
        .ok_or("SKILL.md missing closing frontmatter (---)")?;

    let yaml_content = &rest[..end_pos];
    let markdown_content = rest[end_pos + 4..].trim();

    let mut config: SkillConfig = serde_yaml::from_str(yaml_content)
        .map_err(|e| format!("Failed to parse YAML frontmatter: {}", e))?;

    if config.id.is_empty() {
        config.id = default_id.to_string();
    }
    if config.name.is_empty() {
        config.name = config.id.clone();
    }
    if !markdown_content.is_empty() {
        config.instructions = Some(markdown_content.to_string());
    }

    // Post-processing: if context=Fork and no agent specified, default to "worker"
    if config.context == Some(SkillContext::Fork) && config.agent.is_none() {
        config.agent = Some("worker".to_string());
    }

    Ok(config)
}

/// Load skill config from directory (SKILL.md or config.json)
fn load_skill_from_dir(
    skill_dir: &std::path::Path,
    default_id: &str,
) -> Result<SkillConfig, String> {
    let skill_md_path = if skill_dir.join("SKILL.md").exists() {
        skill_dir.join("SKILL.md")
    } else {
        skill_dir.join("skill.md") // fallback to lowercase
    };
    let config_json_path = skill_dir.join("config.json");

    let mut config = if skill_md_path.exists() {
        let content = fs::read_to_string(&skill_md_path)
            .map_err(|e| format!("Failed to read SKILL.md: {}", e))?;
        let mut config = parse_skill_md(&content, default_id)?;

        if config_json_path.exists() {
            if let Ok(json_content) = fs::read_to_string(&config_json_path) {
                if let Ok(json_config) = serde_json::from_str::<SkillConfig>(&json_content) {
                    if !json_config.params.is_empty() {
                        config.params = json_config.params;
                    }
                }
            }
        }

        config
    } else if config_json_path.exists() {
        let content = fs::read_to_string(&config_json_path)
            .map_err(|e| format!("Failed to read config.json: {}", e))?;
        serde_json::from_str(&content).map_err(|e| format!("Failed to parse config.json: {}", e))?
    } else {
        return Err(format!(
            "No SKILL.md or config.json found in {:?}",
            skill_dir
        ));
    };

    config.path = Some(skill_dir.to_path_buf());
    Ok(config)
}

/// Load skills from a single directory into the map (keyed by skill ID)
fn load_skills_from_dir(
    dir: &std::path::Path,
    skills_map: &mut std::collections::HashMap<String, SkillConfig>,
) {
    if !dir.exists() {
        return;
    }

    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            log::warn!("Failed to read skills dir {:?}: {}", dir, e);
            return;
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                log::warn!("Failed to read entry: {}", e);
                continue;
            }
        };
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        // Skip SKILL.md at root level (it's the system guide)
        if name == "SKILL.md" {
            continue;
        }

        if path.is_dir() {
            match load_skill_from_dir(&path, &name) {
                Ok(config) => {
                    skills_map.insert(config.id.clone(), config);
                }
                Err(e) => log::warn!("Failed to load skill {}: {}", name, e),
            }
        }
    }
}

/// Parse AGENT.md file with YAML frontmatter
fn parse_agent_md(content: &str, default_id: &str) -> Result<AgentConfig, String> {
    if !content.starts_with("---") {
        return Err("AGENT.md must start with YAML frontmatter (---)".to_string());
    }

    let rest = &content[3..];
    let end_pos = rest
        .find("\n---")
        .ok_or("AGENT.md missing closing frontmatter (---)")?;

    let yaml_content = &rest[..end_pos];
    let markdown_content = rest[end_pos + 4..].trim();

    let mut config: AgentConfig = serde_yaml::from_str(yaml_content)
        .map_err(|e| format!("Failed to parse YAML frontmatter: {}", e))?;

    if config.id.is_empty() {
        config.id = default_id.to_string();
    }
    if !markdown_content.is_empty() {
        config.instructions = Some(markdown_content.to_string());
    }

    Ok(config)
}

/// Load agent config from directory (AGENT.md or config.json)
fn load_agent_from_dir(
    agent_dir: &std::path::Path,
    default_id: &str,
) -> Result<AgentConfig, String> {
    let agent_md_path = agent_dir.join("AGENT.md");
    let config_json_path = agent_dir.join("config.json");

    if agent_md_path.exists() {
        let content = fs::read_to_string(&agent_md_path)
            .map_err(|e| format!("Failed to read AGENT.md: {}", e))?;
        let mut config = parse_agent_md(&content, default_id)?;

        if config_json_path.exists() {
            if let Ok(json_content) = fs::read_to_string(&config_json_path) {
                if let Ok(json_config) = serde_json::from_str::<AgentConfig>(&json_content) {
                    if !json_config.params.is_empty() {
                        config.params = json_config.params;
                    }
                }
            }
        }

        Ok(config)
    } else if config_json_path.exists() {
        let content = fs::read_to_string(&config_json_path)
            .map_err(|e| format!("Failed to read config.json: {}", e))?;
        serde_json::from_str(&content).map_err(|e| format!("Failed to parse config.json: {}", e))
    } else {
        Err(format!(
            "No AGENT.md or config.json found in {:?}",
            agent_dir
        ))
    }
}

// ============================================================================
// Service Initialization
// ============================================================================

/// Initialize all services. Previously `start_server()` — no longer starts an
/// HTTP server. All internal callers use `local_services::*` directly.
pub async fn initialize_services(
    db_path: PathBuf,
    data_dir: PathBuf,
    tools_dir: PathBuf,
    skills_dir: PathBuf,
    user_skills_dir: PathBuf,
    agents_dir: PathBuf,
    config_path: PathBuf,
) {
    let builtin_agents_dir = agents_dir.clone();
    let user_agents_dir = data_dir.join("agents");
    let _ = std::fs::create_dir_all(&user_agents_dir);

    // Initialize background runs manager (in-memory; logs under data_dir/runs/)
    let run_manager = Arc::new(crate::runs::RunManager::new(data_dir.clone()));
    run_manager.cleanup_old_logs();
    let background_task_registry =
        Arc::new(cteno_host_session_registry::BackgroundTaskRegistry::new());
    crate::local_services::install_background_task_registry(background_task_registry.clone());

    // Load LLM profile store and global API key (for tools that need LLM access)
    let profile_store = Arc::new(tokio::sync::RwLock::new(crate::llm_profile::load_profiles(
        &data_dir,
    )));
    let global_api_key = {
        let key = std::fs::read_to_string(&config_path)
            .ok()
            .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
            .and_then(|config| {
                config
                    .get("llm_api_key")
                    .or_else(|| config.get("openrouter_key"))
                    .and_then(|k| k.as_str().map(|s| s.to_string()))
            })
            .unwrap_or_default();
        Arc::new(tokio::sync::RwLock::new(key))
    };
    let proxy_profiles = Arc::new(tokio::sync::RwLock::new(
        crate::llm_profile::fetch_proxy_profiles_from_server(&happy_server_url(), &data_dir).await,
    ));
    crate::local_services::install_agent_runtime_context(
        crate::local_services::AgentRuntimeContext {
            db_path: db_path.clone(),
            data_dir: data_dir.clone(),
            config_path: config_path.clone(),
            builtin_skills_dir: skills_dir.clone(),
            user_skills_dir: user_skills_dir.clone(),
            builtin_agents_dir,
            user_agents_dir,
            profile_store: profile_store.clone(),
            proxy_profiles: proxy_profiles.clone(),
            global_api_key: global_api_key.clone(),
        },
    );

    // Build a SubprocessSupervisor (Unix) so cteno-agent child pids are
    // tracked in a persistent pid file. On Windows the stub returns Err and
    // we fall back to unsupervised mode — the executor treats `None` as a
    // no-op.
    let supervisor_arc: Option<Arc<cteno_host_runtime::SubprocessSupervisor>> = {
        let pid_file = data_dir.join("cteno-subprocess-pids.json");
        match cteno_host_runtime::SubprocessSupervisor::new(pid_file.clone()) {
            Ok(sup) => {
                log::info!("SubprocessSupervisor ready (pid file: {:?})", pid_file);
                let arc = Arc::new(sup);
                crate::local_services::install_subprocess_supervisor(arc.clone());
                Some(arc)
            }
            Err(e) => {
                log::warn!("SubprocessSupervisor init failed: {e} — running in unsupervised mode");
                None
            }
        }
    };

    // Multi-vendor agent executor registry — shared SessionStoreProvider
    // backed by the local SQLite DB. Failure here means cteno-agent sidecar
    // binary is missing; we log and continue (sessions will fall back to the
    // legacy in-process path until the registry is resolvable).
    let session_store = crate::session_store_impl::build_session_store(db_path.clone());
    match crate::executor_registry::ExecutorRegistry::build_with_supervisor(
        session_store,
        supervisor_arc.clone(),
    ) {
        Ok(registry) => {
            let vendors = registry.available_vendors();
            log::info!(
                "ExecutorRegistry initialised; available vendors: {}",
                vendors.join(", ")
            );
            crate::local_services::install_executor_registry(Arc::new(registry));

            // Preheat vendor connections in the background so the first
            // session spawn hits a live handle. Boot is NOT blocked on this.
            if let Ok(reg) = crate::local_services::executor_registry() {
                tokio::spawn(async move {
                    log::info!("ExecutorRegistry: starting connection preheat");
                    reg.preheat_all().await;
                    log::info!("ExecutorRegistry: connection preheat complete");
                });
            }
        }
        Err(e) => {
            log::warn!(
                "ExecutorRegistry init failed: {e} — multi-vendor adapters unavailable this session"
            );
        }
    }

    // Initialize Tool Registry
    log::info!("Initializing Tool Registry...");
    let mut tool_registry = crate::tool::registry::ToolRegistry::new();

    // Load tools from tools/ directory
    if tools_dir.exists() {
        let tool_loader = crate::tool_loader::ToolLoader::new(tools_dir.clone());
        match tool_loader.load_all() {
            Ok(tools) => {
                log::info!("Loaded {} tools from {:?}", tools.len(), tools_dir);
                for tool_config in tools {
                    if tool_config.id == "upload_artifact" {
                        log::info!("Skipped disabled tool: upload_artifact");
                        continue;
                    }
                    tool_registry.register_tool(tool_config.clone());
                    log::info!("Registered tool: {} ({})", tool_config.id, tool_config.name);
                }
            }
            Err(e) => {
                log::error!("Failed to load tools: {}", e);
            }
        }
    } else {
        log::warn!("Tools directory not found: {:?}", tools_dir);
    }

    // Register native tool executors
    log::info!("Registering native tool executors...");

    let tool_loader_for_exec = crate::tool_loader::ToolLoader::new(tools_dir.clone());
    if let Ok(all_tools) = tool_loader_for_exec.load_all() {
        // Shell executor
        if let Some(shell_config) = all_tools.iter().find(|t| t.id == "shell") {
            let executor = Arc::new(crate::tool_executors::ShellExecutor::new(
                run_manager.clone(),
            ));
            tool_registry.register(shell_config.clone(), executor);
            log::info!("Registered shell executor");
        }

        // Read executor
        if let Some(read_config) = all_tools.iter().find(|t| t.id == "read") {
            let executor = Arc::new(crate::tool_executors::ReadExecutor::new());
            tool_registry.register(read_config.clone(), executor);
        }

        // Edit executor (with LLM auto-correction)
        if let Some(edit_config) = all_tools.iter().find(|t| t.id == "edit") {
            let edit_api_key = std::fs::read_to_string(&config_path)
                .ok()
                .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
                .and_then(|config| {
                    config
                        .get("llm_api_key")
                        .or_else(|| config.get("openrouter_key"))
                        .and_then(|k| k.as_str().map(|s| s.to_string()))
                });

            if let Some(api_key) = edit_api_key {
                let llm_client = Arc::new(crate::llm::LLMClient::with_base_url(
                    api_key,
                    "https://api.deepseek.com/anthropic".to_string(),
                ));
                let executor = Arc::new(crate::tool_executors::EditExecutor::with_llm_client(
                    llm_client,
                ));
                tool_registry.register(edit_config.clone(), executor);
                log::info!("Registered edit executor with LLM auto-correction");
            } else {
                let executor = Arc::new(crate::tool_executors::EditExecutor::new());
                tool_registry.register(edit_config.clone(), executor);
                log::info!("Registered edit executor without LLM (no API key configured)");
            }
        }

        // Write executor
        if let Some(write_config) = all_tools.iter().find(|t| t.id == "write") {
            let executor = Arc::new(crate::tool_executors::WriteExecutor::new());
            tool_registry.register(write_config.clone(), executor);
            log::info!("Registered write executor");
        }

        // Grep executor
        if let Some(grep_config) = all_tools.iter().find(|t| t.id == "grep") {
            let executor = Arc::new(crate::tool_executors::GrepExecutor::new());
            tool_registry.register(grep_config.clone(), executor);
            log::info!("Registered grep executor");
        }

        // Glob executor
        if let Some(glob_config) = all_tools.iter().find(|t| t.id == "glob") {
            let executor = Arc::new(crate::tool_executors::GlobExecutor::new());
            tool_registry.register(glob_config.clone(), executor);
            log::info!("Registered glob executor");
        }

        // WebSearch executor
        if let Some(websearch_config) = all_tools.iter().find(|t| t.id == "websearch") {
            let executor = Arc::new(crate::tool_executors::WebSearchExecutor::new(
                data_dir.clone(),
            ));
            tool_registry.register(websearch_config.clone(), executor);
            log::info!("Registered websearch executor");
        }

        // Memory executor
        if let Some(memory_config) = all_tools.iter().find(|t| t.id == "memory") {
            let workspace_dir = data_dir.join("workspace");
            let executor = Arc::new(crate::tool_executors::MemoryExecutor::new(workspace_dir));
            tool_registry.register(memory_config.clone(), executor);
            log::info!("Registered memory executor");
        }

        // Unified skill executor
        if let Some(skill_config) = all_tools.iter().find(|t| t.id == "skill") {
            let executor = Arc::new(crate::tool_executors::SkillExecutor::new(
                skills_dir.clone(),
                user_skills_dir.clone(),
            ));
            tool_registry.register(skill_config.clone(), executor);
            log::info!("Registered unified skill executor");
        }

        // Schedule task executor
        if let Some(schedule_task_config) = all_tools.iter().find(|t| t.id == "schedule_task") {
            let executor = Arc::new(crate::tool_executors::ScheduleTaskExecutor::new());
            tool_registry.register(schedule_task_config.clone(), executor);
            log::info!("Registered schedule_task executor");
        }

        // List scheduled tasks executor
        if let Some(list_config) = all_tools.iter().find(|t| t.id == "list_scheduled_tasks") {
            let executor = Arc::new(crate::tool_executors::ListScheduledTasksExecutor::new());
            tool_registry.register(list_config.clone(), executor);
            log::info!("Registered list_scheduled_tasks executor");
        }

        // Delete scheduled task executor
        if let Some(delete_config) = all_tools.iter().find(|t| t.id == "delete_scheduled_task") {
            let executor = Arc::new(crate::tool_executors::DeleteScheduledTaskExecutor::new());
            tool_registry.register(delete_config.clone(), executor);
            log::info!("Registered delete_scheduled_task executor");
        }

        // upload_artifact is temporarily disabled (local workspace browsing flow)

        // Image generation executor
        if let Some(image_gen_config) = all_tools.iter().find(|t| t.id == "image_generation") {
            let executor = Arc::new(crate::tool_executors::ImageGenerationExecutor::new(
                run_manager.clone(),
                data_dir.clone(),
            ));
            tool_registry.register(image_gen_config.clone(), executor);
            log::info!("Registered image_generation executor");
        }

        // image_understanding removed — vision-capable models handle images inline

        // Fetch executor
        if let Some(fetch_config) = all_tools.iter().find(|t| t.id == "fetch") {
            let executor = Arc::new(crate::tool_executors::FetchExecutor::new(
                profile_store.clone(),
                global_api_key.clone(),
            ));
            tool_registry.register(fetch_config.clone(), executor);
            log::info!("Registered fetch executor");
        }

        if let Some(update_plan_config) = all_tools.iter().find(|t| t.id == "update_plan") {
            let executor = Arc::new(crate::tool_executors::UpdatePlanExecutor::new());
            tool_registry.register(update_plan_config.clone(), executor);
            log::info!("Registered update_plan executor");
        }

        // ===== Persona Tools =====

        if let Some(config) = all_tools.iter().find(|t| t.id == "dispatch_task") {
            let executor = Arc::new(crate::tool_executors::DispatchTaskExecutor::new());
            tool_registry.register(config.clone(), executor);
            log::info!("Registered dispatch_task executor");
        }

        if let Some(config) = all_tools.iter().find(|t| t.id == "list_task_sessions") {
            let executor = Arc::new(crate::tool_executors::ListTaskSessionsExecutor::new());
            tool_registry.register(config.clone(), executor);
            log::info!("Registered list_task_sessions executor");
        }

        if let Some(config) = all_tools.iter().find(|t| t.id == "get_session_output") {
            let executor = Arc::new(crate::tool_executors::GetSessionOutputExecutor::new(
                db_path.clone(),
            ));
            tool_registry.register(config.clone(), executor);
            log::info!("Registered get_session_output executor");
        }

        if let Some(config) = all_tools.iter().find(|t| t.id == "send_to_session") {
            let executor = Arc::new(crate::tool_executors::SendToSessionExecutor::new());
            tool_registry.register(config.clone(), executor);
            log::info!("Registered send_to_session executor");
        }

        if let Some(config) = all_tools.iter().find(|t| t.id == "close_task_session") {
            let executor = Arc::new(crate::tool_executors::CloseTaskSessionExecutor::new(
                db_path.clone(),
            ));
            tool_registry.register(config.clone(), executor);
            log::info!("Registered close_task_session executor");
        }

        if let Some(config) = all_tools.iter().find(|t| t.id == "update_personality") {
            let executor = Arc::new(crate::tool_executors::UpdatePersonalityExecutor::new());
            tool_registry.register(config.clone(), executor);
            log::info!("Registered update_personality executor");
        }

        if let Some(config) = all_tools.iter().find(|t| t.id == "ask_persona") {
            let executor = Arc::new(crate::tool_executors::AskPersonaExecutor::new());
            tool_registry.register(config.clone(), executor);
            log::info!("Registered ask_persona executor");
        }

        // ===== Utility Tools =====

        if let Some(config) = all_tools.iter().find(|t| t.id == "wait") {
            let executor = Arc::new(crate::tool_executors::WaitExecutor::new());
            tool_registry.register(config.clone(), executor);
            log::info!("Registered wait executor");
        }

        if let Some(config) = all_tools.iter().find(|t| t.id == "a2ui_render") {
            let executor = Arc::new(crate::tool_executors::A2uiRenderExecutor::new());
            tool_registry.register(config.clone(), executor);
            log::info!("Registered a2ui_render executor");
        }

        // Shared OSS uploader for screenshot tools
        let shared_oss_uploader = Arc::new(crate::tool_executors::oss_upload::OssUploader::new(
            data_dir.clone(),
        ));

        // Screenshot and computer_use share a CoordScale so that taking a
        // screenshot via either tool correctly updates coordinate mapping.
        let shared_coord_scale = Arc::new(std::sync::Mutex::new(
            crate::tool_executors::CoordScale::default(),
        ));

        if let Some(config) = all_tools.iter().find(|t| t.id == "screenshot") {
            let executor = Arc::new(crate::tool_executors::ScreenshotExecutor::new(
                data_dir.clone(),
                shared_coord_scale.clone(),
                shared_oss_uploader.clone(),
            ));
            tool_registry.register(config.clone(), executor);
            log::info!("Registered screenshot executor");
        }

        if let Some(config) = all_tools.iter().find(|t| t.id == "computer_use") {
            let executor = Arc::new(crate::tool_executors::ComputerUseExecutor::new(
                data_dir.clone(),
                shared_coord_scale.clone(),
            ));
            tool_registry.register(config.clone(), executor);
            log::info!("Registered computer_use executor");
        }

        // Browser automation tools (share a single BrowserManager)
        let browser_manager = Arc::new(crate::browser::BrowserManager::new());
        crate::local_services::install_browser_manager(browser_manager.clone());

        if let Some(config) = all_tools.iter().find(|t| t.id == "browser_navigate") {
            let executor = Arc::new(crate::tool_executors::BrowserNavigateExecutor::new(
                browser_manager.clone(),
            ));
            tool_registry.register(config.clone(), executor);
            log::info!("Registered browser_navigate executor");
        }

        if let Some(config) = all_tools.iter().find(|t| t.id == "browser_action") {
            let executor = Arc::new(crate::tool_executors::BrowserActionExecutor::new(
                browser_manager.clone(),
                data_dir.clone(),
                shared_oss_uploader.clone(),
            ));
            tool_registry.register(config.clone(), executor);
            log::info!("Registered browser_action executor");
        }

        if let Some(config) = all_tools.iter().find(|t| t.id == "browser_manage") {
            let executor = Arc::new(crate::tool_executors::BrowserManageExecutor::new(
                browser_manager.clone(),
            ));
            tool_registry.register(config.clone(), executor);
            log::info!("Registered browser_manage executor");
        }

        if let Some(config) = all_tools.iter().find(|t| t.id == "browser_network") {
            let executor = Arc::new(crate::tool_executors::BrowserNetworkExecutor::new(
                browser_manager.clone(),
            ));
            tool_registry.register(config.clone(), executor);
            log::info!("Registered browser_network executor");
        }

        if let Some(config) = all_tools.iter().find(|t| t.id == "browser_adapter") {
            let default_adapters_dir = tools_dir
                .parent()
                .unwrap_or(&tools_dir)
                .join("default_adapters");
            let executor = Arc::new(crate::tool_executors::BrowserAdapterExecutor::new(
                browser_manager.clone(),
                default_adapters_dir,
            ));
            tool_registry.register(config.clone(), executor);
            log::info!("Registered browser_adapter executor");
        }

        if let Some(config) = all_tools.iter().find(|t| t.id == "browser_cdp") {
            let executor = Arc::new(crate::tool_executors::BrowserCdpExecutor::new(
                browser_manager.clone(),
            ));
            tool_registry.register(config.clone(), executor);
            log::info!("Registered browser_cdp executor");
        }
    }

    // Initialize MCP Registry
    log::info!("Initializing MCP Registry...");
    let mut mcp_registry = crate::mcp::MCPRegistry::new();

    let mcp_config_path = {
        let app_data_mcp = data_dir.join("mcp_servers.yaml");
        if app_data_mcp.exists() {
            app_data_mcp
        } else {
            let fallback = data_dir.join("..").join("mcp_servers.yaml");
            fallback.canonicalize().unwrap_or(fallback)
        }
    };
    let mcp_save_path = data_dir.join("mcp_servers.yaml");
    mcp_registry.set_config_path(mcp_save_path.clone());

    // Ensure Cteno's built-in cross-vendor memory MCP is registered before
    // `load_from_config` runs, so it shows up in the MCP modal and the agent's
    // tool list the same way user-added servers do.
    crate::agent_sync_bridge::ensure_cteno_memory_in_mcp_yaml(&mcp_save_path);

    if mcp_config_path.exists() {
        log::info!("Loading MCP servers from {:?}", mcp_config_path);
        match mcp_registry.load_from_config(&mcp_config_path).await {
            Ok(_) => {
                log::info!("MCP servers loaded: {}", mcp_registry.server_count());
            }
            Err(e) => {
                log::error!("Failed to load MCP config: {}", e);
            }
        }
    } else {
        log::info!(
            "MCP config not found at {:?}, skipping MCP initialization",
            mcp_config_path
        );
    }

    let mcp_registry = Arc::new(tokio::sync::RwLock::new(mcp_registry));

    // Register MCP tools into ToolRegistry
    {
        let mcp_reg = mcp_registry.read().await;
        let tool_entries = mcp_reg.get_all_tool_configs();
        log::info!("Registering {} MCP tools", tool_entries.len());
        for (server_id, tool_name, tool_config) in tool_entries {
            let executor = Arc::new(crate::mcp::MCPToolExecutor::new(
                mcp_registry.clone(),
                server_id,
                tool_name,
            ));
            tool_registry.register(tool_config.clone(), executor);
            log::info!("Registered MCP tool: {}", tool_config.id);
        }
    }

    let tool_registry = Arc::new(tokio::sync::RwLock::new(tool_registry));

    // Initialize task scheduler and start timer loop
    let scheduler = Arc::new(crate::scheduler::TaskScheduler::new(db_path.clone()));
    background_task_registry.set_scheduled_job_source(Arc::new(
        crate::local_services::LocalScheduledJobSource::new(scheduler.clone()),
    ));
    {
        let scheduler_for_run = scheduler.clone();
        tokio::spawn(async move {
            scheduler_for_run.run().await;
        });
    }

    // Initialize shared TaskGraphEngine
    let graph_engine = Arc::new(crate::task_graph::TaskGraphEngine::new());

    // Initialize PersonaManager
    let persona_manager = Arc::new(crate::persona::PersonaManager::new(
        db_path.clone(),
        graph_engine.clone(),
    ));

    // Initialize notification watcher and start polling loop (macOS only)
    #[cfg(target_os = "macos")]
    let notification_watcher = {
        let watcher = Arc::new(crate::notification_watcher::NotificationWatcher::new(
            db_path.clone(),
        ));
        let watcher_for_run = watcher.clone();
        tokio::spawn(async move {
            watcher_for_run.run().await;
        });
        watcher
    };

    let usage_store = Arc::new(cteno_community_host::usage_store::UsageStore::new(
        db_path.clone(),
    ));

    // Initialize A2UI store (in-memory declarative UI component trees)
    crate::local_services::install_a2ui_store(Arc::new(crate::a2ui::A2uiStore::new()));

    // Initialize Orchestration store (in-memory flow visualization)
    crate::local_services::install_orchestration_store(Arc::new(
        crate::orchestration::OrchestrationStore::new(),
    ));

    // Install agent-runtime hooks before local_services::install so any
    // later service initialisation can already resolve URL / registry hooks.
    crate::agent_hooks::install_all(
        tool_registry.clone(),
        db_path.clone(),
        skills_dir.clone(),
        user_skills_dir.clone(),
    );

    // Spawn the auth refresh guard. Depends on AuthStore being installed
    // already (done in `setup_tauri_host` / `start_headless_host`); starts a
    // tokio task that polls every 60s and rotates within 5 minutes of expiry.
    crate::auth_store_boot::spawn_refresh_guard();

    // Wire #2 — install the transport-side AuthRefreshHook so Socket.IO 401s
    // can trigger a refresh-and-retry without reverse-depending on the app
    // crate.
    crate::auth_store_boot::install_transport_auth_refresh_hook();

    // Wire #3 — subscribe to AuthStore and maintain user-scoped +
    // machine-scoped long-lived Socket.IO connections in sync with the
    // logged-in access token. Brings sockets up on login, tears them down on
    // logout, and re-dials on rotation so the server-side handshake sees the
    // fresh token.
    crate::auth_store_boot::spawn_user_and_machine_sockets_guard();

    // Wire #4 — register this machine against the logged-in account exactly
    // once per access-token transition; non-fatal on failure so a flaky
    // network can't block login.
    crate::auth_store_boot::subscribe_register_machine_once();

    // Wire #5 — broadcast rotated access tokens into every live Cteno agent
    // subprocess. Covers JS-side refreshes (`ensureFreshAccess`) that reach
    // `AuthStore` via `cteno_auth_save_credentials`; the Rust refresh guard
    // already broadcasts its own rotations.
    crate::auth_store_boot::subscribe_broadcast_token_to_agents();

    #[cfg(target_os = "macos")]
    crate::local_services::install(
        run_manager,
        scheduler,
        persona_manager,
        usage_store,
        notification_watcher,
        tool_registry,
        mcp_registry,
    );

    #[cfg(not(target_os = "macos"))]
    crate::local_services::install(
        run_manager,
        scheduler,
        persona_manager,
        usage_store,
        tool_registry,
        mcp_registry,
    );

    // Cross-vendor config reconcile (memory-mcp entry, AGENTS.md symlink fan-out,
    // etc.) — runs once at boot so vendor CLI subprocesses see the updated
    // configs the first time they're spawned. See
    // `packages/host/rust/crates/cteno-host-agent-sync` and the accompanying
    // feedback memory on why this is not per-session.
    crate::agent_sync_bridge::reconcile_at_boot_from_db(&db_path).await;

    if let Err(error) = crate::host::daemon_runtime::mark_daemon_ready() {
        log::warn!("Failed to mark daemon ready: {}", error);
    }
    log::info!("All services initialized successfully");

    // Block forever (services are running in background tasks)
    // The caller (lib.rs) spawns this in a thread, and the tokio runtime
    // keeps the background tasks alive.
    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
    }
}

// ============================================================================
// Tests — subagent FS loading (flat vs nested formats)
// ============================================================================

#[cfg(test)]
mod subagent_loader_tests {
    use super::*;
    use tempfile::TempDir;

    const BASIC_FLAT: &str = r#"---
name: "reviewer"
description: "reviews changes"
version: "1.0.0"
---

You are a careful reviewer.
"#;

    const BASIC_NESTED: &str = r#"---
name: "planner"
description: "plans tasks"
version: "1.0.0"
---

You plan task execution.
"#;

    #[test]
    fn flat_md_file_is_loaded() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("reviewer.md"), BASIC_FLAT).unwrap();
        let agents = load_agents_from_dir(tmp.path());
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].id, "reviewer");
        assert_eq!(agents[0].description, "reviews changes");
    }

    #[test]
    fn nested_dir_still_works() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("planner");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("AGENT.md"), BASIC_NESTED).unwrap();
        let agents = load_agents_from_dir(tmp.path());
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].id, "planner");
    }

    #[test]
    fn flat_wins_when_both_formats_present_under_same_id() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("reviewer.md"),
            r#"---
name: "reviewer"
description: "from flat"
version: "1.0.0"
---
flat body
"#,
        )
        .unwrap();
        let nested = tmp.path().join("reviewer");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(
            nested.join("AGENT.md"),
            r#"---
name: "reviewer"
description: "from nested"
version: "1.0.0"
---
nested body
"#,
        )
        .unwrap();
        let agents = load_agents_from_dir(tmp.path());
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].description, "from flat");
    }

    #[test]
    fn both_formats_coexist_with_different_ids() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("reviewer.md"), BASIC_FLAT).unwrap();
        let dir = tmp.path().join("planner");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("AGENT.md"), BASIC_NESTED).unwrap();
        let mut agents = load_agents_from_dir(tmp.path());
        agents.sort_by(|a, b| a.id.cmp(&b.id));
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0].id, "planner");
        assert_eq!(agents[1].id, "reviewer");
    }

    #[test]
    fn flat_md_ignores_readme_md_and_hidden_files() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("README.md"), "docs").unwrap();
        std::fs::write(tmp.path().join(".hidden.md"), "nope").unwrap();
        std::fs::write(tmp.path().join("reviewer.md"), BASIC_FLAT).unwrap();
        let agents = load_agents_from_dir(tmp.path());
        assert_eq!(agents.len(), 1, "got: {agents:?}");
        assert_eq!(agents[0].id, "reviewer");
    }
}
