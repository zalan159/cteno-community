//! Headless / Tauri identity path seeding and resolution helpers.

use std::path::{Path, PathBuf};

use crate::daemon_state::machine_auth_cache_path;
use crate::paths::{
    default_headless_app_data_dir, default_tauri_dev_app_data_dir,
    default_tauri_release_app_data_dir, machine_id_path, profiles_path, proxy_profiles_cache_path,
};
use crate::{normalize_cli_target, HostIdentityPaths, HostShellKind};

const ACCOUNT_AUTH_STORE_FILE: &str = "headless_account_auth.json";

/// Inline copy of `headless_auth::account_auth_store_path` (pure path join).
///
/// Kept here so the host crate does not depend on app-crate symbols. If the
/// semantics ever diverge we must keep the two definitions in lockstep.
pub fn account_auth_store_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(ACCOUNT_AUTH_STORE_FILE)
}

pub fn seed_headless_identity_from_tauri(app_data_dir: &Path) -> Result<(), String> {
    let tauri_app_data_dir = default_tauri_release_app_data_dir();
    if tauri_app_data_dir == app_data_dir || !tauri_app_data_dir.exists() {
        return Ok(());
    }

    let copy_file = |source: &Path, target: &Path| -> Result<(), String> {
        std::fs::copy(source, target).map_err(|e| {
            format!(
                "Failed to seed {} from {}: {}",
                target.display(),
                source.display(),
                e
            )
        })?;
        log::info!(
            "Seeded agentd {} from tauri app data {}",
            target.display(),
            source.display()
        );
        Ok(())
    };

    let config_source = tauri_app_data_dir.join("config.json");
    let config_target = app_data_dir.join("config.json");
    if config_source.exists() && !config_target.exists() {
        copy_file(&config_source, &config_target)?;
    }

    let profiles_source = tauri_app_data_dir.join("profiles.json");
    let profiles_target = app_data_dir.join("profiles.json");
    let should_seed_profiles = if !profiles_source.exists() {
        false
    } else if !profiles_target.exists() {
        true
    } else {
        let target_raw = std::fs::read_to_string(&profiles_target).unwrap_or_default();
        let source_raw = std::fs::read_to_string(&profiles_source).unwrap_or_default();
        let target_profiles = profile_count_from_raw(&target_raw);
        let source_profiles = profile_count_from_raw(&source_raw);
        target_profiles <= 1 && source_profiles > target_profiles
    };
    if should_seed_profiles {
        copy_file(&profiles_source, &profiles_target)?;
    }

    let proxy_source = tauri_app_data_dir.join("proxy_profiles_cache.json");
    let proxy_target = app_data_dir.join("proxy_profiles_cache.json");
    if proxy_source.exists()
        && (!proxy_target.exists() || proxy_target.metadata().map(|m| m.len()).unwrap_or(0) == 0)
    {
        copy_file(&proxy_source, &proxy_target)?;
    }

    let account_auth_source = account_auth_store_path(&tauri_app_data_dir);
    let account_auth_target = account_auth_store_path(app_data_dir);
    if account_auth_source.exists() && !account_auth_target.exists() {
        copy_file(&account_auth_source, &account_auth_target)?;
    }

    Ok(())
}

/// Count profiles in a profiles.json payload via untyped JSON so host crate
/// does not take a build-time dependency on the app-side `ProfileStore` type.
fn profile_count_from_raw(raw: &str) -> usize {
    if raw.trim().is_empty() {
        return 0;
    }
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|v| {
            v.get("profiles")
                .and_then(|p| p.as_array())
                .map(|arr| arr.len())
        })
        .unwrap_or(0)
}

pub fn resolve_headless_identity_paths(
    app_data_dir: Option<PathBuf>,
) -> Result<HostIdentityPaths, String> {
    let app_data_dir = app_data_dir.unwrap_or_else(default_headless_app_data_dir);
    std::fs::create_dir_all(&app_data_dir)
        .map_err(|e| format!("Failed to create app data dir {:?}: {}", app_data_dir, e))?;
    std::env::set_var("CTENO_APP_DATA_DIR", app_data_dir.as_os_str());

    Ok(HostIdentityPaths {
        shell_kind: HostShellKind::Agentd,
        config_path: app_data_dir.join("config.json"),
        profiles_path: profiles_path(&app_data_dir),
        proxy_profiles_cache_path: proxy_profiles_cache_path(&app_data_dir),
        machine_id_path: machine_id_path(&app_data_dir),
        machine_auth_cache_path: machine_auth_cache_path(&app_data_dir),
        account_auth_store_path: account_auth_store_path(&app_data_dir),
        local_rpc_env_tag: "agentd".to_string(),
        app_data_dir,
    })
}

pub fn resolve_tauri_identity_paths_from_app_data_dir(
    app_data_dir: PathBuf,
    env_tag: &str,
) -> Result<HostIdentityPaths, String> {
    std::fs::create_dir_all(&app_data_dir)
        .map_err(|e| format!("Failed to create app data dir {:?}: {}", app_data_dir, e))?;
    Ok(HostIdentityPaths {
        shell_kind: HostShellKind::Tauri,
        config_path: app_data_dir.join("config.json"),
        profiles_path: profiles_path(&app_data_dir),
        proxy_profiles_cache_path: proxy_profiles_cache_path(&app_data_dir),
        machine_id_path: machine_id_path(&app_data_dir),
        machine_auth_cache_path: machine_auth_cache_path(&app_data_dir),
        account_auth_store_path: account_auth_store_path(&app_data_dir),
        local_rpc_env_tag: env_tag.to_string(),
        app_data_dir,
    })
}

pub fn resolve_cli_target_identity_paths(
    target: Option<&str>,
) -> Result<HostIdentityPaths, String> {
    match target.and_then(normalize_cli_target) {
        Some("agentd") => resolve_headless_identity_paths(Some(default_headless_app_data_dir())),
        Some("dev") => {
            resolve_tauri_identity_paths_from_app_data_dir(default_tauri_dev_app_data_dir(), "dev")
        }
        Some("release") => {
            resolve_tauri_identity_paths_from_app_data_dir(default_tauri_release_app_data_dir(), "")
        }
        _ => {
            if let Ok(env) = std::env::var("CTENO_ENV") {
                return resolve_cli_target_identity_paths(Some(&env));
            }
            resolve_headless_identity_paths(Some(default_headless_app_data_dir()))
        }
    }
}
