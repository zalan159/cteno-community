#[tauri::command]
pub fn get_permission_snapshot() -> Result<super::PermissionSnapshot, String> {
    Ok(super::current_snapshot())
}

#[tauri::command]
pub fn request_permission(kind: super::PermissionKind) -> Result<super::PermissionState, String> {
    super::request_permission_impl(kind)?;
    Ok(super::current_snapshot().state_for(kind))
}

#[tauri::command]
pub fn open_permission_settings(kind: super::PermissionKind) -> Result<(), String> {
    super::open_permission_settings_impl(kind)
}

#[tauri::command]
pub fn get_ctenoctl_install_status() -> Result<super::CtenoCliInstallStatus, String> {
    super::get_ctenoctl_install_status_impl()
}

#[tauri::command]
pub fn install_ctenoctl() -> Result<super::CtenoCliInstallStatus, String> {
    super::install_ctenoctl_impl()?;
    super::get_ctenoctl_install_status_impl()
}
