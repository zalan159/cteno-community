#[tauri::command]
pub fn get_power_status() -> bool {
    super::is_sleep_prevented()
}

#[tauri::command]
pub fn start_prevent_sleep(reason: String) -> Result<(), String> {
    super::prevent_sleep(&reason)
}

#[tauri::command]
pub fn stop_prevent_sleep() -> Result<(), String> {
    super::allow_sleep()
}
