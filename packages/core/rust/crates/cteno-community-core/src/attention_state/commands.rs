#[tauri::command]
pub async fn update_attention_state(
    active_session_id: Option<String>,
    app_active: bool,
) -> Result<(), String> {
    super::set_attention_state(active_session_id, app_active).await
}
