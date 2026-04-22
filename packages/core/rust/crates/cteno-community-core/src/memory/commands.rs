use tauri::Manager;

fn workspace_dir(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data dir: {e}"))?;
    Ok(super::get_workspace_dir(&app_data_dir))
}

#[tauri::command]
pub async fn memory_read(
    app: tauri::AppHandle,
    file_path: String,
) -> Result<Option<String>, String> {
    super::memory_read_core(&workspace_dir(&app)?, &file_path, None)
}

#[tauri::command]
pub async fn memory_write(
    app: tauri::AppHandle,
    file_path: String,
    content: String,
) -> Result<(), String> {
    super::memory_write_core(&workspace_dir(&app)?, &file_path, &content, None)
}

#[tauri::command]
pub async fn memory_append(
    app: tauri::AppHandle,
    file_path: String,
    content: String,
) -> Result<(), String> {
    super::memory_append_core(&workspace_dir(&app)?, &file_path, &content, None)
}

#[tauri::command]
pub async fn memory_log_today(app: tauri::AppHandle, content: String) -> Result<(), String> {
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let file_path = format!("memory/{today}.md");
    let workspace = workspace_dir(&app)?;
    let full_path = workspace.join(&file_path);

    let header = if !full_path.exists() {
        format!("# {today}\n\n")
    } else {
        String::new()
    };

    let timestamp = chrono::Local::now().format("%H:%M").to_string();
    let entry = format!("{header}## {timestamp}\n{content}\n\n");

    super::memory_append_core(&workspace, &file_path, &entry, None)
}

#[tauri::command]
pub async fn memory_list_files(app: tauri::AppHandle) -> Result<Vec<String>, String> {
    super::memory_list_core(&workspace_dir(&app)?, None)
}

#[tauri::command]
pub async fn memory_search(
    app: tauri::AppHandle,
    query: String,
    options: Option<super::SearchOptions>,
) -> Result<Vec<super::MemoryChunk>, String> {
    let opts = options.unwrap_or_default();
    let limit = opts.limit.unwrap_or(10);
    super::memory_search_core(&workspace_dir(&app)?, &query, None, limit, None)
}
