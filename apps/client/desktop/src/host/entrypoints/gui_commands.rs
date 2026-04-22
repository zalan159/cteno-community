/// Build the Tauri invoke handler.
///
/// Desktop Tauri commands share one local-IPC + Happy Server approval surface.
/// Login state now decides which cloud-backed flows are active.
macro_rules! gui_invoke_handler {
    () => {
        tauri::generate_handler![
            crate::get_config,
            crate::save_config,
            crate::get_status,
            crate::check_permissions,
            cteno_community_host::permissions::commands::get_permission_snapshot,
            cteno_community_host::permissions::commands::request_permission,
            cteno_community_host::permissions::commands::open_permission_settings,
            cteno_community_host::permissions::commands::get_ctenoctl_install_status,
            cteno_community_host::permissions::commands::install_ctenoctl,
            crate::open_url,
            crate::read_file_base64,
            crate::restart_app,
            crate::frontend_log,
            cteno_community_core::attention_state::commands::update_attention_state,
            cteno_community_core::memory::commands::memory_read,
            cteno_community_core::memory::commands::memory_write,
            cteno_community_core::memory::commands::memory_append,
            cteno_community_core::memory::commands::memory_log_today,
            cteno_community_core::memory::commands::memory_list_files,
            cteno_community_core::memory::commands::memory_search,
            cteno_community_core::archive::commands::archive_append_line,
            cteno_community_core::archive::commands::archive_read_lines,
            cteno_community_core::archive::commands::archive_exists,
            cteno_community_core::archive::commands::archive_list_files,
            cteno_community_core::archive::commands::archive_delete_file,
            cteno_community_core::power::commands::get_power_status,
            cteno_community_core::power::commands::start_prevent_sleep,
            cteno_community_core::power::commands::stop_prevent_sleep,
            crate::commands::send_message_local,
            crate::commands::get_session_messages,
            crate::commands::local_rpc,
            crate::commands::get_local_host_info,
            crate::commands::list_available_vendors,
            crate::commands::list_vendor_models,
            crate::commands::probe_vendor_connection,
            crate::commands::cteno_auth_save_credentials,
            crate::commands::cteno_auth_clear_credentials,
            crate::commands::cteno_auth_get_snapshot,
            crate::commands::cteno_auth_force_refresh_now,
            crate::commands::oauth_loopback_start,
            crate::commands::oauth_loopback_wait,
            crate::webview_bridge::webview_eval_result,
        ]
    };
}
