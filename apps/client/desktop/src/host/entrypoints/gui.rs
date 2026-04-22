use env_logger::Env;
use tauri::Manager;
use tauri_plugin_deep_link::DeepLinkExt;

pub(crate) fn run() {
    // Initialize logger with default level "info" for cteno, filter out noisy llama logs
    env_logger::Builder::from_env(Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    if let Err(e) = crate::host::shells::prepare_tauri_runtime_env() {
        panic!("{}", e);
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // Focus the main window when a second instance is launched.
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            // Setup system tray
            crate::tray::setup(app)?;

            let machine_auth_state =
                crate::host::entrypoints::gui_setup::register_commercial_gui_state(app);

            let config = crate::get_config(app.handle().clone()).unwrap_or_default();
            let api_key = config.llm_api_key.clone().unwrap_or_default();
            let bootstrap =
                crate::host::shells::setup_tauri_host(app, machine_auth_state.clone(), api_key)?;
            crate::host::entrypoints::gui_setup::log_bootstrap_paths(&bootstrap.paths);
            crate::host::entrypoints::gui_setup::register_community_gui_state(app, &bootstrap);

            let app_handle = app.handle().clone();
            app.deep_link().on_open_url(move |event| {
                let urls = event.urls();
                if let Some(url) = urls.first() {
                    log::info!("received deep link: {url}");
                }

                if let Some(window) = app_handle.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.unminimize();
                    let _ = window.set_focus();
                }
            });

            log::info!("Cteno initialized successfully");
            Ok(())
        })
        .invoke_handler(gui_invoke_handler!())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
