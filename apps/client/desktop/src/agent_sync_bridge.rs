//! Bridge between the daemon's session spawn path and
//! [`cteno_host_agent_sync`].
//!
//! Before any vendor session is spawned we reconcile each vendor's native
//! config layout so all four (Claude / Codex / Gemini / Cteno) agree on:
//! - the built-in `cteno-memory` MCP server (same stdio bin, same project dir)
//! - `AGENTS.md` as the single-source system prompt
//! - the set of canonical skills (global + per-project)
//!
//! The reconcile is idempotent. Failures are logged but do NOT block spawn —
//! memory-MCP being unavailable is a graceful degradation, not a hard error.

use std::{
    cmp::Ordering,
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use cteno_host_agent_sync::{
    ClaudeSyncer, CodexSyncer, CtenoSyncer, GeminiSyncer, McpSpec, PersonaSpec, SkillSpec,
    VendorSyncer, memory_mcp_spec, reconcile_all,
};
use serde_yaml::Value as YamlValue;
use tokio::sync::OnceCell;

const MEMORY_MCP_BIN_NAME: &str = "cteno-memory-mcp";
const SKILLS_ROOT_DIR: &str = "skills";
const SKILL_MD_UPPER: &str = "SKILL.md";
const SKILL_MD_LOWER: &str = "skill.md";
const PROMPT_SYNC_MARKER: &str = "<!-- cteno:merged-project-agent-md -->";

/// Locate the shipped `cteno-memory-mcp` binary.
///
/// Resolution order:
/// 1. `$CTENO_MEMORY_MCP_BIN` env override (dev/test).
/// 2. Sibling of the current executable (prod — bin ships next to daemon).
/// 3. `$PATH` lookup (fallback for bundler/installer layouts).
///
/// Returns `None` when nothing on the machine can run the server. In that
/// case the reconcile skips the memory MCP entry rather than writing a
/// broken `command = "cteno-memory-mcp"` into each vendor config.
pub(crate) fn locate_memory_mcp_bin() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("CTENO_MEMORY_MCP_BIN") {
        let p = PathBuf::from(explicit);
        if p.exists() {
            return Some(p);
        }
        log::warn!("CTENO_MEMORY_MCP_BIN points at {p:?} which does not exist");
    }

    if let Ok(current) = std::env::current_exe() {
        if let Some(dir) = current.parent() {
            let candidate = dir.join(MEMORY_MCP_BIN_NAME);
            if candidate.exists() {
                return Some(candidate);
            }
            // Tauri `externalBin` bundles sidecars under `{name}-{target-triple}`
            // inside the app `Contents/MacOS/`, so the daemon's sibling lookup
            // must accept that layout too.
            let triple_candidate = dir.join(format!(
                "{MEMORY_MCP_BIN_NAME}-{}",
                env!("TARGET_TRIPLE_FALLBACK")
            ));
            if triple_candidate.exists() {
                return Some(triple_candidate);
            }
            #[cfg(windows)]
            {
                let win_candidate = dir.join(format!("{MEMORY_MCP_BIN_NAME}.exe"));
                if win_candidate.exists() {
                    return Some(win_candidate);
                }
                let win_triple = dir.join(format!(
                    "{MEMORY_MCP_BIN_NAME}-{}.exe",
                    env!("TARGET_TRIPLE_FALLBACK")
                ));
                if win_triple.exists() {
                    return Some(win_triple);
                }
            }
        }
    }

    for candidate in dev_memory_mcp_candidates() {
        if candidate.exists() {
            return Some(candidate);
        }
    }

    which_on_path(MEMORY_MCP_BIN_NAME)
}

fn dev_memory_mcp_candidates() -> Vec<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let bin = if cfg!(windows) {
        format!("{MEMORY_MCP_BIN_NAME}.exe")
    } else {
        MEMORY_MCP_BIN_NAME.to_string()
    };
    vec![
        manifest_dir
            .join("../../../packages/host/rust/target/debug")
            .join(&bin),
        manifest_dir
            .join("../../../packages/host/rust/target/release")
            .join(&bin),
    ]
}

fn which_on_path(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(bin);
        if candidate.exists() {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let win = dir.join(format!("{bin}.exe"));
            if win.exists() {
                return Some(win);
            }
        }
    }
    None
}

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

fn read_project_prompt_candidate(path: PathBuf, label: &'static str) -> Option<(String, String)> {
    let content = fs::read_to_string(&path).ok()?;
    let normalized = normalize_prompt_content(&content);
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

    if candidates.is_empty() {
        return agents;
    }

    let next_content = if candidates.len() == 1 {
        candidates[0].1.clone()
    } else {
        render_merged_project_prompt(&candidates)
    };

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

/// Ensure the Cteno-native MCP config (`{data_dir}/mcp_servers.yaml`, read by
/// `cteno_agent_runtime::mcp::MCPRegistry`) carries a single canonical
/// `cteno-memory` entry pointing at the discovered binary. Called just before
/// `MCPRegistry::load_from_config` so the Cteno agent and the MCP-modal UI see
/// it as a first-class server alongside anything the user added manually.
///
/// Policy:
/// - Other servers in the yaml are left untouched.
/// - Entry id is the stable literal `"cteno-memory"` so the upsert never
///   duplicates across boots.
/// - If the memory bin can't be located, the function is a no-op (and we log
///   a warning) — Cteno keeps running without the memory server.
pub fn ensure_cteno_memory_in_mcp_yaml(yaml_path: &Path) {
    let Some(bin) = locate_memory_mcp_bin() else {
        log::warn!(
            "agent_sync: skipping cteno-memory MCP registration in {yaml_path:?} — bin not found"
        );
        return;
    };

    let existing = std::fs::read_to_string(yaml_path).unwrap_or_default();
    // Parse permissively: any malformed content is treated as "empty servers list"
    // and we rebuild. Persisted config stays valid YAML after the write.
    let mut doc: YamlValue = if existing.trim().is_empty() {
        YamlValue::Mapping(serde_yaml::Mapping::new())
    } else {
        serde_yaml::from_str(&existing).unwrap_or(YamlValue::Mapping(serde_yaml::Mapping::new()))
    };
    if !doc.is_mapping() {
        doc = YamlValue::Mapping(serde_yaml::Mapping::new());
    }

    let root = doc.as_mapping_mut().expect("ensured mapping");
    let servers_key = YamlValue::String("servers".into());
    let servers_entry = root
        .entry(servers_key.clone())
        .or_insert(YamlValue::Sequence(Vec::new()));
    if !servers_entry.is_sequence() {
        *servers_entry = YamlValue::Sequence(Vec::new());
    }
    let servers = servers_entry.as_sequence_mut().expect("ensured sequence");

    // Build the canonical cteno-memory entry. Note: no `--project-dir` — the
    // Cteno agent's MCPRegistry spawns exactly one subprocess at boot, so a
    // fixed per-project scope would be wrong. Per-session project scope is
    // the vendor-specific path (each Claude/Codex/Gemini session spawns its
    // own subprocess with the right workdir via reconcile_at_boot).
    let mut transport = serde_yaml::Mapping::new();
    transport.insert(
        YamlValue::String("type".into()),
        YamlValue::String("stdio".into()),
    );
    transport.insert(
        YamlValue::String("command".into()),
        YamlValue::String(bin.display().to_string()),
    );
    transport.insert(
        YamlValue::String("args".into()),
        YamlValue::Sequence(Vec::new()),
    );
    transport.insert(
        YamlValue::String("env".into()),
        YamlValue::Mapping(serde_yaml::Mapping::new()),
    );

    let mut entry = serde_yaml::Mapping::new();
    entry.insert(
        YamlValue::String("id".into()),
        YamlValue::String("cteno-memory".into()),
    );
    entry.insert(
        YamlValue::String("name".into()),
        YamlValue::String("cteno-memory".into()),
    );
    entry.insert(YamlValue::String("enabled".into()), YamlValue::Bool(true));
    entry.insert(
        YamlValue::String("transport".into()),
        YamlValue::Mapping(transport),
    );
    let entry_val = YamlValue::Mapping(entry);

    // Upsert by id.
    let position = servers.iter().position(|item| {
        item.get("id")
            .and_then(YamlValue::as_str)
            .map(|s| s == "cteno-memory")
            .unwrap_or(false)
    });
    match position {
        Some(idx) => servers[idx] = entry_val,
        None => servers.push(entry_val),
    }

    if let Some(parent) = yaml_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let rendered = match serde_yaml::to_string(&doc) {
        Ok(s) => s,
        Err(err) => {
            log::warn!("agent_sync: failed to serialise mcp_servers.yaml: {err}");
            return;
        }
    };
    if let Err(err) = std::fs::write(yaml_path, rendered) {
        log::warn!("agent_sync: failed to write {yaml_path:?}: {err}");
        return;
    }
    log::info!("agent_sync: ensured cteno-memory MCP entry in {yaml_path:?}");
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
        reconcile_single(&std::env::temp_dir()).await;
        return;
    }

    log::info!(
        "agent_sync: reconciling {} project workdir(s) at boot",
        unique.len()
    );
    for wd in unique {
        reconcile_single(wd).await;
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
    reconcile_single(workdir).await;
}

async fn reconcile_global_skills() {
    let Some(home) = dirs::home_dir() else {
        log::warn!("agent_sync: failed to resolve home directory; skip global skill reconcile");
        return;
    };
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

async fn reconcile_single(workdir: &Path) {
    let authoritative_prompt = ensure_authoritative_prompt_seed(workdir);
    // If AGENTS.md is missing but CLAUDE.md/GEMINI.md exists, we seed AGENTS.md
    // from that content first to avoid prompt content loss during symlink
    // convergence.

    let mut mcp: Vec<McpSpec> = Vec::new();
    match locate_memory_mcp_bin() {
        Some(bin) => mcp.push(memory_mcp_spec(&bin, workdir, None)),
        None => log::warn!(
            "agent_sync: cteno-memory-mcp binary not found (set CTENO_MEMORY_MCP_BIN \
             to override); memory MCP will be absent from vendor configs"
        ),
    }

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
        &mcp,
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
    // parallel. Both test flows below mutate `CTENO_MEMORY_MCP_BIN` / `HOME`
    // / `PATH`, so we serialize them behind a single mutex.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Reconcile must work end-to-end against a synthetic workdir with a
    /// stubbed memory-mcp binary: Claude / Gemini / Codex configs appear
    /// with the `cteno-memory` entry, and `AGENTS.md` symlinks (CLAUDE.md /
    /// GEMINI.md / .cteno/PROMPT.md) are created.
    #[tokio::test(flavor = "current_thread")]
    async fn reconcile_at_boot_writes_every_vendor_layout() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = TempDir::new().unwrap();
        let project = tmp.path();
        std::fs::write(project.join("AGENTS.md"), "system prompt body").unwrap();

        // Stub a memory-mcp bin so reconcile emits the MCP entry.
        let fake_bin = tmp.path().join("fake-cteno-memory-mcp");
        std::fs::write(&fake_bin, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&fake_bin).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&fake_bin, perms).unwrap();
        }
        // Isolate the Codex user-scoped config away from the real ~/.codex.
        let fake_codex_home = tmp.path().join("codex-home");
        std::fs::create_dir_all(&fake_codex_home).unwrap();

        // Safety note: `set_var` is unsafe in 2024-edition Rust; this test
        // runs single-threaded and sets env before any reader observes it.
        // SAFETY: no other thread observes these env vars concurrently.
        unsafe {
            std::env::set_var("CTENO_MEMORY_MCP_BIN", &fake_bin);
            std::env::set_var("HOME", &fake_codex_home);
        }

        reconcile_at_boot(std::slice::from_ref(&project.to_path_buf())).await;

        // Claude .mcp.json
        let claude_mcp: Value =
            serde_json::from_str(&std::fs::read_to_string(project.join(".mcp.json")).unwrap())
                .unwrap();
        assert!(claude_mcp["mcpServers"]["cteno-memory"].is_object());

        // Gemini settings.json
        let gemini: Value = serde_json::from_str(
            &std::fs::read_to_string(project.join(".gemini/settings.json")).unwrap(),
        )
        .unwrap();
        assert!(gemini["mcpServers"]["cteno-memory"].is_object());

        // Symlinked prompt — follow the symlinks to verify they reach
        // the authoritative AGENTS.md content.
        assert_eq!(
            std::fs::read_to_string(project.join("CLAUDE.md"))
                .unwrap()
                .trim(),
            "system prompt body"
        );
        assert_eq!(
            std::fs::read_to_string(project.join("GEMINI.md"))
                .unwrap()
                .trim(),
            "system prompt body"
        );
        assert_eq!(
            std::fs::read_to_string(project.join(".cteno/PROMPT.md"))
                .unwrap()
                .trim(),
            "system prompt body"
        );
    }

    /// Absent memory bin must still produce vendor layouts — just without
    /// the cteno-memory MCP entry. Reconcile should never crash on missing
    /// bin.
    #[tokio::test(flavor = "current_thread")]
    async fn reconcile_at_boot_tolerates_missing_memory_bin() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = TempDir::new().unwrap();
        let project = tmp.path();
        std::fs::write(project.join("AGENTS.md"), "prompt").unwrap();

        // SAFETY: see sibling test.
        unsafe {
            std::env::remove_var("CTENO_MEMORY_MCP_BIN");
        }
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
            "no bin → no cteno-memory entry, got {servers:?}"
        );
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
            std::env::remove_var("CTENO_MEMORY_MCP_BIN");
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
            std::env::remove_var("CTENO_MEMORY_MCP_BIN");
            std::env::set_var("PATH", "/usr/bin:/bin");
        }

        reconcile_at_boot(std::slice::from_ref(&project)).await;

        assert_eq!(
            std::fs::read_to_string(project.join("AGENTS.md"))
                .unwrap()
                .trim(),
            "legacy prompt from claude"
        );
        assert_eq!(
            std::fs::read_to_string(project.join("GEMINI.md"))
                .unwrap()
                .trim(),
            "legacy prompt from claude"
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
            std::env::remove_var("CTENO_MEMORY_MCP_BIN");
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
