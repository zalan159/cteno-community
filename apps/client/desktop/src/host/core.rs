//! Thin Tauri wrappers around `cteno-host-runtime`. The real work (path
//! resolution, machine runtime trait seam, service init) lives in the host
//! crate; this file only injects Tauri-specific inputs and re-exports types so
//! existing `crate::host::core::*` call sites keep compiling.

use std::path::PathBuf;

pub use cteno_host_runtime::machine::{
    spawn_service_initializer, HostMachineRuntime, HostMachineSpawnConfig, HostMachineSpawner,
    LocalWorkspaceRpcRegistrar, MachineIdProvider, ServiceInitConfig, ServiceInitializer,
};
pub use cteno_host_runtime::{
    account_auth_store_path, default_headless_app_data_dir, default_tauri_dev_app_data_dir,
    default_tauri_release_app_data_dir, normalize_cli_target, resolve_cli_target_identity_paths,
    resolve_headless_identity_paths, resolve_tauri_identity_paths_from_app_data_dir,
    seed_headless_identity_from_tauri, HostIdentityPaths, HostPaths, HostShellKind,
};

use cteno_host_runtime::host_paths::{
    resolve_headless_paths_with_manifest, resolve_tauri_paths_from, ResolvedTauriDirs,
};
use tauri::Manager;

/// Tauri-side wrapper: resolve the directories we need from the `AppHandle` and
/// delegate to the host crate. The host crate itself does not depend on Tauri.
pub fn resolve_tauri_paths(app: &tauri::AppHandle) -> Result<HostPaths, String> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data dir: {}", e))?;
    let resource_dir = app.path().resource_dir().ok();
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let runtime_manifest_dir = cteno_agent_runtime::runtime_resources_dir();
    let config_path = crate::get_config_path(app)?;

    resolve_tauri_paths_from(ResolvedTauriDirs {
        app_data_dir,
        resource_dir,
        manifest_dir,
        runtime_manifest_dir,
        config_path,
    })
}

/// App-side wrapper for headless path resolution — supplies the app crate's
/// `CARGO_MANIFEST_DIR` (for agents/helpers) and the agent-runtime crate's
/// `CARGO_MANIFEST_DIR` (for tools/skills) so bundled asset lookups land on
/// the right location.
pub fn resolve_headless_paths(app_data_dir: Option<PathBuf>) -> Result<HostPaths, String> {
    resolve_headless_paths_with_manifest(
        app_data_dir,
        PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        cteno_agent_runtime::runtime_resources_dir(),
    )
}
