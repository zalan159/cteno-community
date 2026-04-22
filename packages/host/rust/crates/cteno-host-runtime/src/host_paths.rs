//! `HostPaths` builders (Tauri + headless). The Tauri variant takes a pre-resolved
//! directory bundle so the host crate stays free of a `tauri` dependency.

use std::path::PathBuf;

use crate::identity::{account_auth_store_path, resolve_headless_identity_paths};
use crate::paths::{machine_id_path, profiles_path, proxy_profiles_cache_path};
use crate::{HostIdentityPaths, HostPaths, HostShellKind};

/// Resolved directories supplied by the Tauri app layer so that `cteno-host-runtime`
/// does not have to depend on `tauri::AppHandle`.
#[derive(Clone, Debug)]
pub struct ResolvedTauriDirs {
    /// `app.path().app_data_dir()` (already created on disk by the caller, or will be).
    pub app_data_dir: PathBuf,
    /// `app.path().resource_dir()`; used as the base for bundled `tools/skills/agents`.
    pub resource_dir: Option<PathBuf>,
    /// `CARGO_MANIFEST_DIR` of the app crate; used for dev-mode asset lookups
    /// (agents/, helpers/).
    pub manifest_dir: PathBuf,
    /// `CARGO_MANIFEST_DIR` of the `cteno-agent-runtime` crate; used for dev-mode
    /// lookup of bundled `tools/` and `skills/` which live next to the agent
    /// runtime source, not inside the Tauri app crate.
    pub runtime_manifest_dir: PathBuf,
    /// Resolved `config.json` path (from Tauri `get_config_path`).
    pub config_path: PathBuf,
}

pub fn resolve_tauri_paths_from(dirs: ResolvedTauriDirs) -> Result<HostPaths, String> {
    let ResolvedTauriDirs {
        app_data_dir,
        resource_dir,
        manifest_dir,
        runtime_manifest_dir,
        config_path,
    } = dirs;

    std::fs::create_dir_all(&app_data_dir)
        .map_err(|e| format!("Failed to create app data dir {:?}: {}", app_data_dir, e))?;
    std::env::set_var("CTENO_APP_DATA_DIR", app_data_dir.as_os_str());

    // `subdir_manifest_dir` points at the crate that owns the dev-mode source of
    // this asset: agent-runtime for tools/skills, app crate for agents.
    let resolve_builtin_dir = |subdir_manifest_dir: &PathBuf, subdir: &str| -> PathBuf {
        let dev_dir = subdir_manifest_dir.join(subdir);
        if dev_dir.exists() {
            dev_dir
        } else {
            resource_dir
                .as_ref()
                .map(|p| p.join(subdir))
                .unwrap_or_else(|| dev_dir.clone())
        }
    };

    let builtin_tools_dir = resolve_builtin_dir(&runtime_manifest_dir, "tools");
    let builtin_skills_dir = resolve_builtin_dir(&runtime_manifest_dir, "skills");
    let builtin_agents_dir = resolve_builtin_dir(&manifest_dir, "agents");
    let user_skills_dir = dirs::home_dir()
        .ok_or("Failed to get home directory")?
        .join(".agents")
        .join("skills");
    std::fs::create_dir_all(&user_skills_dir)
        .map_err(|e| format!("Failed to create unified skills dir: {}", e))?;

    let user_agents_dir = app_data_dir.join("agents");
    std::fs::create_dir_all(&user_agents_dir)
        .map_err(|e| format!("Failed to create user agents dir: {}", e))?;

    let identity = HostIdentityPaths {
        shell_kind: HostShellKind::Tauri,
        config_path: config_path.clone(),
        profiles_path: profiles_path(&app_data_dir),
        proxy_profiles_cache_path: proxy_profiles_cache_path(&app_data_dir),
        machine_id_path: machine_id_path(&app_data_dir),
        machine_auth_cache_path: crate::daemon_state::machine_auth_cache_path(&app_data_dir),
        account_auth_store_path: account_auth_store_path(&app_data_dir),
        local_rpc_env_tag: cteno_host_bridge_localrpc::env_tag_from_data_dir(
            &app_data_dir.to_string_lossy(),
        ),
        app_data_dir: app_data_dir.clone(),
    };

    Ok(HostPaths {
        identity,
        db_path: app_data_dir.join("db").join("cteno.db"),
        data_dir: app_data_dir.clone(),
        config_path: app_data_dir.join("config.json"),
        builtin_tools_dir,
        builtin_skills_dir,
        user_skills_dir,
        builtin_agents_dir,
        user_agents_dir,
        app_data_dir,
    })
}

/// Resolve headless (`cteno-agentd`) host paths.
///
/// `manifest_dir` is the app (binary) crate's `CARGO_MANIFEST_DIR` and is used
/// for bundled `agents/` lookups. `runtime_manifest_dir` is the
/// `cteno-agent-runtime` crate's `CARGO_MANIFEST_DIR` and is used for bundled
/// `tools/skills/` lookups, which live beside the agent runtime source.
pub fn resolve_headless_paths_with_manifest(
    app_data_dir: Option<PathBuf>,
    manifest_dir: PathBuf,
    runtime_manifest_dir: PathBuf,
) -> Result<HostPaths, String> {
    let identity = resolve_headless_identity_paths(app_data_dir)?;
    let app_data_dir = identity.app_data_dir.clone();

    let builtin_tools_dir = std::env::var("CTENO_BUILTIN_TOOLS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| runtime_manifest_dir.join("tools"));
    let builtin_skills_dir = std::env::var("CTENO_BUILTIN_SKILLS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| runtime_manifest_dir.join("skills"));
    let builtin_agents_dir = std::env::var("CTENO_BUILTIN_AGENTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| manifest_dir.join("agents"));
    let user_skills_dir = crate::daemon_state::ensure_agents_home_dir()?.join("skills");
    std::fs::create_dir_all(&user_skills_dir)
        .map_err(|e| format!("Failed to create unified skills dir: {}", e))?;
    let user_agents_dir = app_data_dir.join("agents");
    std::fs::create_dir_all(&user_agents_dir)
        .map_err(|e| format!("Failed to create user agents dir: {}", e))?;

    Ok(HostPaths {
        identity,
        db_path: app_data_dir.join("db").join("cteno.db"),
        data_dir: app_data_dir.clone(),
        config_path: app_data_dir.join("config.json"),
        builtin_tools_dir,
        builtin_skills_dir,
        user_skills_dir,
        builtin_agents_dir,
        user_agents_dir,
        app_data_dir,
    })
}
