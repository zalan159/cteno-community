use tauri::Manager;

fn app_data_dir(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data dir: {e}"))
}

#[tauri::command]
pub fn archive_append_line(
    app: tauri::AppHandle,
    path: String,
    line: String,
) -> Result<(), String> {
    super::archive_append_line_core(&app_data_dir(&app)?, &path, &line)
}

#[tauri::command]
pub fn archive_read_lines(app: tauri::AppHandle, path: String) -> Result<Vec<String>, String> {
    super::archive_read_lines_core(&app_data_dir(&app)?, &path)
}

#[tauri::command]
pub fn archive_exists(app: tauri::AppHandle, path: String) -> Result<bool, String> {
    super::archive_exists_core(&app_data_dir(&app)?, &path)
}

#[tauri::command]
pub fn archive_list_files(
    app: tauri::AppHandle,
    pattern: Option<String>,
) -> Result<Vec<String>, String> {
    super::archive_list_files_core(&app_data_dir(&app)?, pattern)
}

#[tauri::command]
pub fn archive_delete_file(app: tauri::AppHandle, path: String) -> Result<(), String> {
    super::archive_delete_file_core(&app_data_dir(&app)?, &path)
}
