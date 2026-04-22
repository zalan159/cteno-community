//! Host filesystem path helpers (app_data_dir resolution, profile/machine_id paths).

use std::path::{Path, PathBuf};

pub fn profiles_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("profiles.json")
}

pub fn proxy_profiles_cache_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("proxy_profiles_cache.json")
}

pub fn machine_id_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("machine_id.txt")
}

pub fn default_tauri_release_app_data_dir() -> PathBuf {
    std::env::var("CTENO_TAURI_APP_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("Cteno")
        })
}

pub fn default_tauri_dev_app_data_dir() -> PathBuf {
    std::env::var("CTENO_TAURI_DEV_APP_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("com.cteno.desktop.dev")
        })
}

pub fn default_headless_app_data_dir() -> PathBuf {
    std::env::var("CTENO_APP_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("Cteno Agentd")
        })
}
