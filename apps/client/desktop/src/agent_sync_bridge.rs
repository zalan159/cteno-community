//! Bridge between the daemon's session spawn path and
//! [`cteno_host_agent_sync`].
//!
//! Before any vendor session is spawned we reconcile each vendor's native
//! config layout so all four (Claude / Codex / Gemini / Cteno) agree on:
//! - `AGENTS.md` as the single-source system prompt
//! - Cteno memory CLI invocation instructions
//! - the set of canonical skills (global + per-project)
//!
//! The reconcile is idempotent. Failures are logged but do NOT block spawn —
//! vendor-native config drift is a graceful degradation, not a hard error.

use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use cteno_host_agent_sync::{
    reconcile_all, ClaudeSyncer, CodexSyncer, CtenoSyncer, GeminiSyncer, McpSpec,
    McpTransport as SyncMcpTransport, PersonaSpec, SkillSpec, VendorSyncer,
};
use tokio::sync::OnceCell;

const SKILLS_ROOT_DIR: &str = "skills";
const SKILL_MD_UPPER: &str = "SKILL.md";
const SKILL_MD_LOWER: &str = "skill.md";
const PROMPT_SYNC_MARKER: &str = "<!-- cteno:merged-project-agent-md -->";
const MEMORY_CLI_PROMPT_START: &str = "<!-- cteno:memory-cli-bridge:start -->";
const MEMORY_CLI_PROMPT_END: &str = "<!-- cteno:memory-cli-bridge:end -->";

#[derive(Clone, Debug)]
struct DiscoveredSkill {
    dir_name: String,
    skill_name: String,
    version: Option<String>,
    source_dir: PathBuf,
    source_rank: usize,
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("create_dir_all {dst:?}: {e}"))?;
    let entries = fs::read_dir(src).map_err(|e| format!("read_dir {src:?}: {e}"))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("read_dir entry {src:?}: {e}"))?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            fs::copy(&from, &to).map_err(|e| format!("copy {from:?} -> {to:?}: {e}"))?;
        }
    }
    Ok(())
}

fn yaml_to_string(v: &serde_yaml::Value) -> Option<String> {
    match v {
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

fn parse_skill_identity(skill_dir: &Path, default_name: &str) -> Option<(String, Option<String>)> {
    let skill_md = skill_dir.join(SKILL_MD_UPPER);
    let content = fs::read_to_string(&skill_md)
        .or_else(|_| fs::read_to_string(skill_dir.join(SKILL_MD_LOWER)))
        .ok()?;
    if !content.starts_with("---") {
        return None;
    }
    let rest = &content[3..];
    let end_pos = rest.find("\n---")?;
    let yaml_str = &rest[..end_pos];
    let yaml: serde_yaml::Value = serde_yaml::from_str(yaml_str).ok()?;
    let skill_name = yaml
        .get("name")
        .and_then(yaml_to_string)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default_name.to_string());
    let version = yaml.get("version").and_then(yaml_to_string).or_else(|| {
        yaml.get("metadata")
            .and_then(|meta| meta.get("version"))
            .and_then(yaml_to_string)
    });
    Some((skill_name, version))
}

fn compare_version_strings(a: &str, b: &str) -> Option<Ordering> {
    let parse = |v: &str| -> Vec<u64> { v.split('.').filter_map(|s| s.parse().ok()).collect() };
    let ap = parse(a);
    let bp = parse(b);
    if ap.is_empty() || bp.is_empty() {
        return Some(a.cmp(b));
    }
    let max_len = ap.len().max(bp.len());
    for i in 0..max_len {
        let av = ap.get(i).copied().unwrap_or(0);
        let bv = bp.get(i).copied().unwrap_or(0);
        match av.cmp(&bv) {
            Ordering::Equal => {}
            non_eq => return Some(non_eq),
        }
    }
    Some(Ordering::Equal)
}

fn should_replace_skill(current: &DiscoveredSkill, candidate: &DiscoveredSkill) -> bool {
    match (&current.version, &candidate.version) {
        (Some(cur), Some(cand)) => match compare_version_strings(cand, cur) {
            Some(Ordering::Greater) => true,
            Some(Ordering::Less) => false,
            _ => candidate.source_rank < current.source_rank,
        },
        (None, Some(_)) => true,
        (Some(_), None) => false,
        (None, None) => candidate.source_rank < current.source_rank,
    }
}

fn discover_skills_from_root(root: &Path, rank: usize) -> Vec<DiscoveredSkill> {
    if !root.exists() || !root.is_dir() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let entries = match fs::read_dir(root) {
        Ok(e) => e,
        Err(err) => {
            log::warn!("agent_sync: failed to read skills root {root:?}: {err}");
            return out;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let dir_name = entry.file_name().to_string_lossy().to_string();
        if dir_name.starts_with('.') {
            continue;
        }
        let Some((skill_name, version)) = parse_skill_identity(&path, &dir_name) else {
            continue;
        };
        out.push(DiscoveredSkill {
            dir_name,
            skill_name,
            version,
            source_dir: path,
            source_rank: rank,
        });
    }
    out
}

fn collect_canonical_skill_specs(
    canonical_root: &Path,
    source_roots: &[(&Path, usize)],
) -> Vec<SkillSpec> {
    let mut canonical_by_name: HashMap<String, String> = HashMap::new();
    for entry in discover_skills_from_root(canonical_root, 0) {
        canonical_by_name
            .entry(entry.skill_name.to_lowercase())
            .or_insert(entry.dir_name);
    }

    let mut selected: HashMap<String, DiscoveredSkill> = HashMap::new();
    for (root, rank) in source_roots {
        for skill in discover_skills_from_root(root, *rank) {
            let key = skill.skill_name.to_lowercase();
            match selected.get(&key) {
                Some(existing) if !should_replace_skill(existing, &skill) => {}
                _ => {
                    selected.insert(key, skill);
                }
            }
        }
    }

    if selected.is_empty() && !canonical_root.exists() {
        return Vec::new();
    }

    if let Err(err) = fs::create_dir_all(canonical_root) {
        log::warn!("agent_sync: failed to create canonical skills root {canonical_root:?}: {err}");
        return Vec::new();
    }

    let mut specs: Vec<SkillSpec> = Vec::new();
    for (name_key, skill) in selected {
        let dir_name = canonical_by_name
            .get(&name_key)
            .cloned()
            .unwrap_or_else(|| skill.dir_name.clone());
        let canonical_dir = canonical_root.join(&dir_name);

        if canonical_dir != skill.source_dir {
            if canonical_dir.exists() {
                if let Err(err) = fs::remove_dir_all(&canonical_dir) {
                    log::warn!(
                        "agent_sync: failed to replace canonical skill dir {canonical_dir:?}: {err}"
                    );
                    continue;
                }
            }
            if let Err(err) = copy_dir_recursive(&skill.source_dir, &canonical_dir) {
                log::warn!(
                    "agent_sync: failed to copy skill {name_key} into canonical root: {err}"
                );
                continue;
            }
        }

        specs.push(SkillSpec {
            name: dir_name,
            source_dir: canonical_dir,
        });
    }
    specs.sort_by(|a, b| a.name.cmp(&b.name));
    specs
}

fn collect_global_skills(home: &Path) -> Vec<SkillSpec> {
    let canonical = home.join(".agents").join(SKILLS_ROOT_DIR);
    let claude = home.join(".claude").join(SKILLS_ROOT_DIR);
    let gemini = home.join(".gemini").join(SKILLS_ROOT_DIR);
    collect_canonical_skill_specs(&canonical, &[(&canonical, 0), (&claude, 1), (&gemini, 2)])
}

fn collect_project_skills(workdir: &Path) -> Vec<SkillSpec> {
    let canonical = workdir.join(".cteno").join(SKILLS_ROOT_DIR);
    let claude = workdir.join(".claude").join(SKILLS_ROOT_DIR);
    let gemini = workdir.join(".gemini").join(SKILLS_ROOT_DIR);
    collect_canonical_skill_specs(&canonical, &[(&canonical, 0), (&claude, 1), (&gemini, 2)])
}

fn normalize_prompt_content(content: &str) -> String {
    content.trim().replace("\r\n", "\n")
}

fn strip_cteno_memory_cli_block(content: &str) -> String {
    let Some(start) = content.find(MEMORY_CLI_PROMPT_START) else {
        return content.trim().to_string();
    };
    let Some(end_rel) = content[start..].find(MEMORY_CLI_PROMPT_END) else {
        return content.trim().to_string();
    };
    let end = start + end_rel + MEMORY_CLI_PROMPT_END.len();
    let mut out = String::new();
    out.push_str(content[..start].trim_end());
    out.push_str(content[end..].trim_start());
    out.trim().to_string()
}

fn render_cteno_memory_cli_block() -> &'static str {
    r#"<!-- cteno:memory-cli-bridge:start -->
## Cteno Memory CLI

Use `ctenoctl memory` for persistent Cteno memory.

Default scope is project + global. Pass `--project-dir "$PWD"` from the current project root so project memory maps to `.cteno/memory/`. Pass `--scope global` when you only want global memory.

Command schema:

```json
{
  "list": {
    "command": "ctenoctl memory list --project-dir <path> [--scope auto|private|global]",
    "output": { "success": "boolean", "data": ["[private] file.md", "[global] file.md"] }
  },
  "recall": {
    "command": "ctenoctl memory recall --query <string> --project-dir <path> [--limit 10] [--type <string>]",
    "args": { "query": "required string", "project_dir": "optional path", "limit": "optional integer", "type": "optional frontmatter type" },
    "output": { "success": "boolean", "data": [{ "file_path": "string", "content": "markdown chunk", "score": "number" }] }
  },
  "read": {
    "command": "ctenoctl memory read <file_path> --project-dir <path> [--scope auto|private|global]",
    "args": { "file_path": "required workspace-relative path" },
    "output": { "success": "boolean", "data": "markdown or null" }
  },
  "save": {
    "command": "ctenoctl memory save --file-path <file_path> --content <markdown> --project-dir <path> [--scope private|global]",
    "args": { "file_path": "required workspace-relative path", "content": "required markdown" },
    "output": { "success": "boolean" }
  }
}
```

Run `ctenoctl memory schema` for the machine-readable schema.
<!-- cteno:memory-cli-bridge:end -->"#
}

fn ensure_cteno_memory_cli_block(content: &str) -> String {
    let base = strip_cteno_memory_cli_block(&normalize_prompt_content(content));
    if base.is_empty() {
        format!("{}\n", render_cteno_memory_cli_block())
    } else {
        format!("{}\n\n{}\n", base, render_cteno_memory_cli_block())
    }
}

fn read_project_prompt_candidate(path: PathBuf, label: &'static str) -> Option<(String, String)> {
    let content = fs::read_to_string(&path).ok()?;
    let normalized = strip_cteno_memory_cli_block(&normalize_prompt_content(&content));
    if normalized.is_empty() {
        return None;
    }
    Some((label.to_string(), normalized))
}

fn render_merged_project_prompt(candidates: &[(String, String)]) -> String {
    let mut out = String::from("# Project Agent Instructions\n\n");
    out.push_str(PROMPT_SYNC_MARKER);
    out.push_str("\n\n");
    out.push_str(
        "Cteno keeps this file as the shared per-project instruction source for Codex, Claude, Gemini, and Cteno.\n",
    );
    for (label, content) in candidates {
        out.push_str("\n## From ");
        out.push_str(label);
        out.push_str("\n\n");
        out.push_str(content.trim());
        out.push('\n');
    }
    out
}

fn ensure_authoritative_prompt_seed(workdir: &Path) -> PathBuf {
    let agents = workdir.join("AGENTS.md");

    let mut candidates: Vec<(String, String)> = Vec::new();
    for (path, label) in [
        (agents.clone(), "AGENTS.md"),
        (workdir.join("CLAUDE.md"), "CLAUDE.md"),
        (workdir.join("GEMINI.md"), "GEMINI.md"),
    ] {
        let Some((label, content)) = read_project_prompt_candidate(path, label) else {
            continue;
        };
        if candidates
            .iter()
            .any(|(_, existing)| existing.trim() == content.trim())
        {
            continue;
        }
        candidates.push((label, content));
    }

    let base_content = if candidates.is_empty() {
        "# Project Agent Instructions\n\nCteno keeps this file as the shared per-project instruction source for Codex, Claude, Gemini, and Cteno.\n".to_string()
    } else if candidates.len() == 1 {
        candidates[0].1.clone()
    } else {
        render_merged_project_prompt(&candidates)
    };
    let next_content = ensure_cteno_memory_cli_block(&base_content);

    let current = fs::read_to_string(&agents)
        .ok()
        .map(|s| normalize_prompt_content(&s));
    if current.as_deref() == Some(next_content.trim()) {
        return agents;
    }

    if let Some(parent) = agents.parent() {
        if let Err(err) = fs::create_dir_all(parent) {
            log::warn!("agent_sync: failed to create AGENTS.md parent for {workdir:?}: {err}");
            return agents;
        }
    }
    match fs::write(&agents, next_content) {
        Ok(()) => {
            log::info!("agent_sync: reconciled project agent md into AGENTS.md for {workdir:?}");
        }
        Err(err) => {
            log::warn!("agent_sync: failed to write AGENTS.md for {workdir:?}: {err}");
        }
    }

    agents
}

struct Syncers {
    claude: Arc<ClaudeSyncer>,
    codex: Arc<CodexSyncer>,
    gemini: Arc<GeminiSyncer>,
    cteno: Arc<CtenoSyncer>,
}

static SYNCERS: OnceCell<Syncers> = OnceCell::const_new();

async fn syncers() -> &'static Syncers {
    SYNCERS
        .get_or_init(|| async {
            Syncers {
                claude: Arc::new(ClaudeSyncer::new()),
                codex: Arc::new(CodexSyncer::new()),
                gemini: Arc::new(GeminiSyncer::new()),
                cteno: Arc::new(CtenoSyncer::new()),
            }
        })
        .await
}

/// Enumerate distinct workdirs from the `personas` + `persona_workspaces`
/// tables and kick off a boot reconcile. Swallows DB errors — a fresh install
/// with no personas yet is fine; reconcile will still touch user-scope
/// configs (Codex) via the empty-workdirs path.
pub async fn reconcile_at_boot_from_db(db_path: &Path) {
    let workdirs = match collect_workdirs_from_db(db_path) {
        Ok(list) => list,
        Err(err) => {
            log::warn!(
                "agent_sync: failed to enumerate persona workdirs from {db_path:?}: {err}; \
                 falling back to user-scope-only reconcile"
            );
            Vec::new()
        }
    };
    reconcile_at_boot(&workdirs).await;
}

fn collect_workdirs_from_db(db_path: &Path) -> Result<Vec<PathBuf>, String> {
    use rusqlite::Connection;
    let conn = Connection::open(db_path).map_err(|e| format!("open {db_path:?}: {e}"))?;

    let mut out: Vec<PathBuf> = Vec::new();
    let mut push_if_good = |raw: String| {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return;
        }
        let expanded = shellexpand::tilde(trimmed).to_string();
        let p = PathBuf::from(&expanded);
        if p.is_absolute() && p.exists() {
            out.push(p);
        }
    };

    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT workdir FROM personas WHERE workdir IS NOT NULL AND workdir != ''",
        )
        .map_err(|e| format!("prepare personas: {e}"))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| format!("query personas: {e}"))?;
    for row in rows {
        if let Ok(w) = row {
            push_if_good(w);
        }
    }

    if let Ok(mut stmt2) = conn.prepare(
        "SELECT DISTINCT workdir FROM persona_workspaces WHERE workdir IS NOT NULL AND workdir != ''",
    ) {
        if let Ok(rows2) = stmt2.query_map([], |row| row.get::<_, String>(0)) {
            for row in rows2 {
                if let Ok(w) = row {
                    push_if_good(w);
                }
            }
        }
    }

    Ok(out)
}

/// Reconcile a list of project workdirs at daemon startup.
///
/// Skips non-absolute paths and non-existent workdirs. Deduplicates so each
/// directory is only touched once even if multiple personas share a workdir.
/// If `workdirs` is empty we still do one invocation with an empty list so
/// user-scope configs (Codex `~/.codex/config.toml`) get written; those
/// ignore the workdir argument.
pub async fn reconcile_at_boot(workdirs: &[PathBuf]) {
    reconcile_global_skills().await;
    let mcp = current_mcp_specs_from_registry().await;

    reconcile_workdirs(workdirs, &mcp).await;
}

async fn reconcile_workdirs(workdirs: &[PathBuf], mcp: &[McpSpec]) {
    let mut seen = std::collections::HashSet::new();
    let unique: Vec<&PathBuf> = workdirs
        .iter()
        .filter(|p| p.is_absolute() && p.exists() && seen.insert(p.as_path().to_path_buf()))
        .collect();

    if unique.is_empty() {
        // Still do one reconcile pass so user-scope vendors get their
        // config updated — Codex's `~/.codex/config.toml` is workdir-
        // independent. Use the system temp dir as a harmless stand-in
        // for the per-project vendors (their writes will be no-ops for
        // a path that doesn't match a real project).
        log::info!(
            "agent_sync: no project workdirs to reconcile at boot; writing user-scope configs only"
        );
        reconcile_single(&std::env::temp_dir(), mcp).await;
        return;
    }

    log::info!(
        "agent_sync: reconciling {} project workdir(s) at boot",
        unique.len()
    );
    for wd in unique {
        reconcile_single(wd, mcp).await;
    }
}

/// Reconcile global skills (`~/.agents/skills`) into vendor user-scope skill
/// roots (`~/.claude/skills`, `~/.gemini/skills`) immediately.
///
/// This is used by skill-store/UI mutations so users don't need to restart
/// the daemon to see skills across vendors.
pub async fn reconcile_global_skills_now() {
    reconcile_global_skills().await;
}

/// Reconcile one project workdir immediately.
///
/// Intended for workspace/project creation flows that want cross-vendor
/// projection without waiting for next daemon restart.
pub async fn reconcile_project_now(workdir: &Path) {
    if !workdir.is_absolute() || !workdir.exists() {
        return;
    }
    let mcp = current_mcp_specs_from_registry().await;
    reconcile_single(workdir, &mcp).await;
}

/// Reconcile MCP changes immediately after the user edits MCP servers in the
/// management UI/CLI. This keeps Claude/Codex/Gemini/Cteno vendor config files
/// in step without waiting for the next daemon restart.
pub async fn reconcile_mcp_now_from_db(db_path: &Path) {
    if let Some(home) = dirs::home_dir() {
        cleanup_legacy_user_scope_memory_mcp(&home);
    }

    let mcp = current_mcp_specs_from_registry().await;
    let workdirs = match collect_workdirs_from_db(db_path) {
        Ok(list) => list,
        Err(err) => {
            log::warn!(
                "agent_sync: failed to enumerate persona workdirs after MCP mutation from {db_path:?}: {err}; \
                 falling back to user-scope-only reconcile"
            );
            Vec::new()
        }
    };

    reconcile_workdirs(&workdirs, &mcp).await;
}

async fn current_mcp_specs_from_registry() -> Vec<McpSpec> {
    let Ok(registry) = crate::local_services::mcp_registry() else {
        log::debug!("agent_sync: MCP registry unavailable; projecting empty MCP spec list");
        return Vec::new();
    };
    let reg = registry.read().await;
    mcp_specs_from_configs(&reg.server_configs())
}

fn mcp_specs_from_configs(configs: &[crate::mcp::MCPServerConfig]) -> Vec<McpSpec> {
    configs
        .iter()
        .filter(|config| config.enabled && config.id != "cteno-memory")
        .map(|config| match &config.transport {
            crate::mcp::MCPTransport::Stdio { command, args, env } => McpSpec {
                name: config.id.clone(),
                command: command.clone(),
                args: args.clone(),
                env: env
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<BTreeMap<_, _>>(),
                transport: SyncMcpTransport::Stdio,
                host_managed: true,
            },
            crate::mcp::MCPTransport::HttpSse { url, .. } => McpSpec {
                name: config.id.clone(),
                command: String::new(),
                args: Vec::new(),
                env: BTreeMap::new(),
                transport: SyncMcpTransport::StreamableHttp { url: url.clone() },
                host_managed: true,
            },
        })
        .collect()
}

async fn reconcile_global_skills() {
    let Some(home) = dirs::home_dir() else {
        log::warn!("agent_sync: failed to resolve home directory; skip global skill reconcile");
        return;
    };
    cleanup_legacy_user_scope_memory_mcp(&home);
    let skills = collect_global_skills(&home);
    let s = syncers().await;
    for v in [
        s.claude.as_ref() as &dyn VendorSyncer,
        s.gemini.as_ref() as &dyn VendorSyncer,
        s.codex.as_ref() as &dyn VendorSyncer,
        s.cteno.as_ref() as &dyn VendorSyncer,
    ] {
        if let Err(err) = v.sync_skills(&home, &skills) {
            log::warn!(
                "agent_sync: global skill reconcile failed for vendor {}: {err:#}",
                v.vendor_name()
            );
        }
    }
}

fn cleanup_legacy_user_scope_memory_mcp(home: &Path) {
    let gemini_settings = home.join(".gemini").join("settings.json");
    let Ok(raw) = fs::read_to_string(&gemini_settings) else {
        return;
    };
    let Ok(mut root) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return;
    };
    let removed = root
        .get_mut("mcpServers")
        .and_then(|servers| servers.as_object_mut())
        .and_then(|servers| servers.remove("cteno-memory"))
        .is_some();
    if !removed {
        return;
    }
    match serde_json::to_string_pretty(&root) {
        Ok(rendered) => {
            if let Err(err) = fs::write(&gemini_settings, rendered) {
                log::warn!(
                    "agent_sync: failed to remove legacy cteno-memory from {gemini_settings:?}: {err}"
                );
            } else {
                log::info!("agent_sync: removed legacy cteno-memory MCP from {gemini_settings:?}");
            }
        }
        Err(err) => {
            log::warn!(
                "agent_sync: failed to serialize cleaned Gemini settings {gemini_settings:?}: {err}"
            );
        }
    }
}

async fn reconcile_single(workdir: &Path, mcp: &[McpSpec]) {
    let authoritative_prompt = ensure_authoritative_prompt_seed(workdir);
    // If AGENTS.md is missing but CLAUDE.md/GEMINI.md exists, we seed AGENTS.md
    // from that content first to avoid prompt content loss during symlink
    // convergence.

    // Personas still reserved for a follow-up pass.
    let personas: Vec<PersonaSpec> = Vec::new();
    let skills: Vec<SkillSpec> = collect_project_skills(workdir);

    let s = syncers().await;
    let vendors: &[&dyn VendorSyncer] = &[
        s.claude.as_ref(),
        s.codex.as_ref(),
        s.gemini.as_ref(),
        s.cteno.as_ref(),
    ];

    let mcp_entries = mcp.len();
    match reconcile_all(
        workdir,
        &authoritative_prompt,
        mcp,
        &personas,
        &skills,
        vendors,
    ) {
        Ok(report) => {
            log::info!(
                "agent_sync: reconciled {} path(s), {} mcp entr{} for {workdir:?}",
                report.wrote.len(),
                mcp_entries,
                if mcp_entries == 1 { "y" } else { "ies" }
            );
            for (path, reason) in &report.skipped {
                log::debug!("agent_sync: skipped {path:?}: {reason}");
            }
        }
        Err(err) => {
            log::warn!("agent_sync: reconcile failed for {workdir:?}: {err:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use tempfile::TempDir;

    // Env vars are process-global; `tokio::test` can interleave cases in
    // parallel. Test flows below mutate `HOME` / `PATH`, so we serialize them
    // behind a single mutex.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Reconcile must work end-to-end against a synthetic workdir: vendor MCP
    /// config files are present without the old built-in `cteno-memory` entry,
    /// and `AGENTS.md` symlinks (CLAUDE.md / GEMINI.md / .cteno/PROMPT.md) are
    /// created with the Cteno memory CLI instructions.
    #[tokio::test(flavor = "current_thread")]
    async fn reconcile_at_boot_writes_every_vendor_layout() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = TempDir::new().unwrap();
        let project = tmp.path();
        std::fs::write(project.join("AGENTS.md"), "system prompt body").unwrap();

        // Isolate the Codex user-scoped config away from the real ~/.codex.
        let fake_codex_home = tmp.path().join("codex-home");
        std::fs::create_dir_all(&fake_codex_home).unwrap();

        // Safety note: `set_var` is unsafe in 2024-edition Rust; this test
        // runs single-threaded and sets env before any reader observes it.
        // SAFETY: no other thread observes these env vars concurrently.
        unsafe {
            std::env::set_var("HOME", &fake_codex_home);
        }

        reconcile_at_boot(std::slice::from_ref(&project.to_path_buf())).await;

        // Claude .mcp.json
        let claude_mcp: Value =
            serde_json::from_str(&std::fs::read_to_string(project.join(".mcp.json")).unwrap())
                .unwrap();
        assert!(!claude_mcp["mcpServers"]["cteno-memory"].is_object());

        // Gemini settings.json
        let gemini: Value = serde_json::from_str(
            &std::fs::read_to_string(project.join(".gemini/settings.json")).unwrap(),
        )
        .unwrap();
        assert!(!gemini["mcpServers"]["cteno-memory"].is_object());

        // Symlinked prompt — follow the symlinks to verify they reach
        // the authoritative AGENTS.md content.
        let agents = std::fs::read_to_string(project.join("AGENTS.md")).unwrap();
        assert!(agents.contains("system prompt body"), "{agents}");
        assert!(agents.contains(MEMORY_CLI_PROMPT_START), "{agents}");
        assert!(agents.contains("ctenoctl memory recall"), "{agents}");
        assert_eq!(
            std::fs::read_to_string(project.join("CLAUDE.md")).unwrap(),
            agents
        );
        assert_eq!(
            std::fs::read_to_string(project.join("GEMINI.md")).unwrap(),
            agents
        );
        assert_eq!(
            std::fs::read_to_string(project.join(".cteno/PROMPT.md")).unwrap(),
            agents
        );
    }

    /// Reconcile must not reintroduce the legacy cteno-memory MCP entry.
    #[tokio::test(flavor = "current_thread")]
    async fn reconcile_at_boot_tolerates_missing_memory_bin() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = TempDir::new().unwrap();
        let project = tmp.path();
        std::fs::write(project.join("AGENTS.md"), "prompt").unwrap();

        // Point HOME at a temp dir so the codex syncer writes there, not
        // against the real ~/.codex — otherwise this test would poison
        // user state. We can't easily stub `current_exe()`, so this also
        // prevents the real sibling lookup from picking anything up.
        let fake_home = tmp.path().join("home-empty");
        std::fs::create_dir_all(&fake_home).unwrap();
        // SAFETY: see sibling test.
        unsafe {
            std::env::set_var("HOME", &fake_home);
            std::env::set_var("PATH", "/usr/bin:/bin"); // strip any dev-env PATH entry
        }

        reconcile_at_boot(std::slice::from_ref(&project.to_path_buf())).await;

        // Vendor config files still get created; just no memory entry.
        let mcp_path = project.join(".mcp.json");
        assert!(
            mcp_path.exists(),
            "claude .mcp.json should still be written"
        );
        let claude_mcp: Value =
            serde_json::from_str(&std::fs::read_to_string(&mcp_path).unwrap()).unwrap();
        let servers = claude_mcp
            .get("mcpServers")
            .and_then(|v| v.as_object())
            .expect("mcpServers object");
        assert!(
            !servers.contains_key("cteno-memory"),
            "no built-in cteno-memory entry, got {servers:?}"
        );
    }

    #[test]
    fn mcp_specs_from_configs_projects_enabled_non_legacy_servers() {
        let specs = mcp_specs_from_configs(&[
            crate::mcp::MCPServerConfig {
                id: "filesystem".into(),
                name: "Filesystem".into(),
                enabled: true,
                transport: crate::mcp::MCPTransport::Stdio {
                    command: "npx".into(),
                    args: vec![
                        "-y".into(),
                        "@modelcontextprotocol/server-filesystem".into(),
                    ],
                    env: HashMap::from([("ROOT".into(), "/tmp".into())]),
                },
            },
            crate::mcp::MCPServerConfig {
                id: "disabled".into(),
                name: "Disabled".into(),
                enabled: false,
                transport: crate::mcp::MCPTransport::Stdio {
                    command: "disabled".into(),
                    args: Vec::new(),
                    env: HashMap::new(),
                },
            },
            crate::mcp::MCPServerConfig {
                id: "cteno-memory".into(),
                name: "Cteno Memory".into(),
                enabled: true,
                transport: crate::mcp::MCPTransport::Stdio {
                    command: "cteno-memory-mcp".into(),
                    args: Vec::new(),
                    env: HashMap::new(),
                },
            },
            crate::mcp::MCPServerConfig {
                id: "remote".into(),
                name: "Remote".into(),
                enabled: true,
                transport: crate::mcp::MCPTransport::HttpSse {
                    url: "https://example.com/mcp".into(),
                    headers: HashMap::new(),
                },
            },
        ]);

        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].name, "filesystem");
        assert_eq!(specs[0].command, "npx");
        assert_eq!(
            specs[0].args,
            vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-filesystem".to_string()
            ]
        );
        assert_eq!(specs[0].env.get("ROOT").map(String::as_str), Some("/tmp"));
        assert_eq!(specs[1].name, "remote");
        assert!(matches!(
            specs[1].transport,
            SyncMcpTransport::StreamableHttp { ref url } if url == "https://example.com/mcp"
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reconcile_at_boot_syncs_global_and_project_skills() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("AGENTS.md"), "prompt").unwrap();

        // Global: prefer newer Claude version over older canonical .agents version.
        let global_agents = home.join(".agents/skills/research");
        let global_claude = home.join(".claude/skills/research");
        std::fs::create_dir_all(&global_agents).unwrap();
        std::fs::create_dir_all(&global_claude).unwrap();
        std::fs::write(
            global_agents.join("SKILL.md"),
            "---\nname: research\nversion: 1.0.0\n---\nold\n",
        )
        .unwrap();
        std::fs::write(
            global_claude.join("SKILL.md"),
            "---\nname: research\nversion: 1.2.0\n---\nnew\n",
        )
        .unwrap();

        // Project-local skill from canonical project root.
        let project_skill = project.join(".cteno/skills/local-tool");
        std::fs::create_dir_all(&project_skill).unwrap();
        std::fs::write(
            project_skill.join("SKILL.md"),
            "---\nname: local-tool\nversion: 1.0.0\n---\nproject\n",
        )
        .unwrap();

        // SAFETY: serialized by ENV_LOCK.
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var("PATH", "/usr/bin:/bin");
        }

        reconcile_at_boot(std::slice::from_ref(&project)).await;

        // Canonical global skill upgraded from Claude's newer copy.
        let canonical =
            std::fs::read_to_string(home.join(".agents/skills/research/SKILL.md")).unwrap();
        assert!(
            canonical.contains("version: 1.2.0"),
            "expected canonical global skill to be upgraded, got: {canonical}"
        );

        // Global sync reaches Gemini user-scope.
        assert!(
            home.join(".gemini/skills/research").exists(),
            "global skill should sync to ~/.gemini/skills"
        );

        // Project sync reaches both vendor project scopes.
        assert!(project.join(".claude/skills/local-tool").exists());
        assert!(project.join(".gemini/skills/local-tool").exists());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reconcile_at_boot_seeds_agents_md_from_existing_vendor_prompt() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = TempDir::new().unwrap();
        let project = tmp.path().join("project");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&home).unwrap();

        // Simulate an existing project that only has CLAUDE.md content.
        std::fs::write(project.join("CLAUDE.md"), "legacy prompt from claude").unwrap();

        // SAFETY: serialized by ENV_LOCK.
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var("PATH", "/usr/bin:/bin");
        }

        reconcile_at_boot(std::slice::from_ref(&project)).await;

        let agents = std::fs::read_to_string(project.join("AGENTS.md")).unwrap();
        assert!(agents.contains("legacy prompt from claude"), "{agents}");
        assert!(agents.contains(MEMORY_CLI_PROMPT_START), "{agents}");
        assert_eq!(
            std::fs::read_to_string(project.join("GEMINI.md")).unwrap(),
            agents
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reconcile_at_boot_merges_distinct_project_agent_md_files() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = TempDir::new().unwrap();
        let project = tmp.path().join("project");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&home).unwrap();

        std::fs::write(project.join("AGENTS.md"), "codex shared rules").unwrap();
        std::fs::write(project.join("CLAUDE.md"), "claude project rules").unwrap();
        std::fs::write(project.join("GEMINI.md"), "gemini project rules").unwrap();

        // SAFETY: serialized by ENV_LOCK.
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var("PATH", "/usr/bin:/bin");
        }

        reconcile_at_boot(std::slice::from_ref(&project)).await;

        let merged = std::fs::read_to_string(project.join("AGENTS.md")).unwrap();
        assert!(merged.contains(PROMPT_SYNC_MARKER), "{merged}");
        assert!(merged.contains("## From AGENTS.md"), "{merged}");
        assert!(merged.contains("codex shared rules"), "{merged}");
        assert!(merged.contains("## From CLAUDE.md"), "{merged}");
        assert!(merged.contains("claude project rules"), "{merged}");
        assert!(merged.contains("## From GEMINI.md"), "{merged}");
        assert!(merged.contains("gemini project rules"), "{merged}");
        assert!(merged.contains(MEMORY_CLI_PROMPT_START), "{merged}");
        assert_eq!(
            std::fs::read_to_string(project.join("CLAUDE.md")).unwrap(),
            merged
        );
        assert_eq!(
            std::fs::read_to_string(project.join("GEMINI.md")).unwrap(),
            merged
        );
    }
}
