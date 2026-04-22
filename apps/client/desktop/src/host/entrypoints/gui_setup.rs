use crate::commands::MachineAuthState;
use crate::host::shells::TauriHostBootstrap;
use tauri::Manager;

pub(crate) fn register_commercial_gui_state(app: &tauri::App) -> MachineAuthState {
    let machine_auth_state = MachineAuthState::new();
    app.manage(machine_auth_state.clone());
    machine_auth_state
}

pub(crate) fn register_community_gui_state(app: &tauri::App, bootstrap: &TauriHostBootstrap) {
    app.manage(bootstrap.session_connections.clone());
    app.manage(bootstrap.rpc_registry.clone());
    app.manage(bootstrap.local_host_info.clone());
}

pub(crate) fn log_bootstrap_paths(host_paths: &crate::host::core::HostPaths) {
    let builtin_tools_dir = host_paths.builtin_tools_dir.clone();
    log::info!(
        "Builtin tools directory: {:?} (exists: {})",
        builtin_tools_dir,
        builtin_tools_dir.exists()
    );

    let builtin_skills_dir = host_paths.builtin_skills_dir.clone();
    log::info!(
        "Builtin skills directory: {:?} (exists: {})",
        builtin_skills_dir,
        builtin_skills_dir.exists()
    );

    let user_skills_dir = host_paths.user_skills_dir.clone();
    log::info!("Unified skills directory: {:?}", user_skills_dir);

    let user_agents_dir = host_paths.user_agents_dir.clone();
    log::info!("User agents directory: {:?}", user_agents_dir);

    let agents_dir = host_paths.builtin_agents_dir.clone();
    log::info!(
        "Agents directory: {:?} (exists: {})",
        agents_dir,
        agents_dir.exists()
    );

}
