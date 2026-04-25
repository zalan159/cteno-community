use std::path::PathBuf;

pub mod auth;
pub mod daemon_state;
pub mod events;
pub mod host_paths;
pub mod identity;
pub mod llm_key;
pub mod machine;
pub mod paths;
pub mod session_sync;
pub mod subprocess_supervisor;

pub use auth::{
    expires_at_ms_from_seconds, now_ms, refresh_tokens, AuthSnapshot, AuthStore,
    AuthTokensResponse, RefreshError, AUTH_STORE_FILE,
};

pub use llm_key::{LlmKeyRecord, LlmKeyStore, LLM_KEY_STORE_FILE};

pub use subprocess_supervisor::{SubprocessSupervisor, SupervisedProcess};

pub use host_paths::{
    resolve_headless_paths_with_manifest, resolve_tauri_paths_from, ResolvedTauriDirs,
};
pub use identity::{
    account_auth_store_path, resolve_cli_target_identity_paths, resolve_headless_identity_paths,
    resolve_tauri_identity_paths_from_app_data_dir, seed_headless_identity_from_tauri,
};
pub use paths::{
    default_headless_app_data_dir, default_tauri_dev_app_data_dir,
    default_tauri_release_app_data_dir, machine_id_path, profiles_path, proxy_profiles_cache_path,
};
pub use session_sync::{
    install_session_sync_service, session_sync_service, SessionSyncMessage, SessionSyncService,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostShellKind {
    Tauri,
    Agentd,
}

impl HostShellKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Tauri => "tauri",
            Self::Agentd => "agentd",
        }
    }
}

pub fn normalize_cli_target(raw: &str) -> Option<&'static str> {
    match raw {
        "agentd" => Some("agentd"),
        "tauri" | "release" => Some("release"),
        "tauri-dev" | "dev" => Some("dev"),
        _ => None,
    }
}

#[derive(Clone, Debug)]
pub struct HostIdentityPaths {
    pub shell_kind: HostShellKind,
    pub app_data_dir: PathBuf,
    pub config_path: PathBuf,
    pub profiles_path: PathBuf,
    pub proxy_profiles_cache_path: PathBuf,
    pub machine_id_path: PathBuf,
    pub machine_auth_cache_path: PathBuf,
    pub account_auth_store_path: PathBuf,
    pub local_rpc_env_tag: String,
}

#[derive(Clone)]
pub struct HostPaths {
    pub identity: HostIdentityPaths,
    pub app_data_dir: PathBuf,
    pub db_path: PathBuf,
    pub data_dir: PathBuf,
    pub builtin_tools_dir: PathBuf,
    pub builtin_skills_dir: PathBuf,
    pub user_skills_dir: PathBuf,
    pub builtin_agents_dir: PathBuf,
    pub user_agents_dir: PathBuf,
    pub config_path: PathBuf,
}
