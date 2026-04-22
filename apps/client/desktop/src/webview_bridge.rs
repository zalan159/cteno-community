//! Webview Bridge — Execute JS in the Tauri webview and get results back.
//!
//! `WebviewWindow::eval()` is fire-and-forget. To get return values, we:
//! 1. Wrap user JS in an async IIFE that calls `invoke('webview_eval_result', ...)`
//! 2. Block the RPC handler on a oneshot channel
//! 3. The Tauri command `webview_eval_result` sends the result through the channel
//!
//! Screenshot uses `xcap::Window` to capture the Cteno window.

use image::codecs::png::PngEncoder;
use image::ImageEncoder;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tauri::Manager;
use tokio::sync::{oneshot, Mutex};

// ---------------------------------------------------------------------------
// Eval result waiters (same pattern as local_rpc_server::CLI_COMPLETIONS)
// ---------------------------------------------------------------------------

type WaiterMap = Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>>;

static EVAL_WAITERS: OnceLock<WaiterMap> = OnceLock::new();

fn waiters() -> &'static WaiterMap {
    EVAL_WAITERS.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

// ---------------------------------------------------------------------------
// Tauri command — called from JS inside the webview
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn webview_eval_result(eval_id: String, success: bool, result: String) {
    log::debug!(
        "[WebviewBridge] eval result: id={}, success={}, len={}",
        eval_id,
        success,
        result.len()
    );

    let rt = tokio::runtime::Handle::current();
    rt.spawn(async move {
        let mut map = waiters().lock().await;
        if let Some(tx) = map.remove(&eval_id) {
            let value = if success {
                // Try to parse as JSON first, fall back to string
                match serde_json::from_str::<Value>(&result) {
                    Ok(v) => json!({"success": true, "value": v}),
                    Err(_) => json!({"success": true, "value": result}),
                }
            } else {
                json!({"success": false, "error": result})
            };
            let _ = tx.send(value);
        } else {
            log::warn!(
                "[WebviewBridge] No waiter for eval_id={} (timed out?)",
                eval_id
            );
        }
    });
}

// ---------------------------------------------------------------------------
// Execute JS in the webview (called from RPC handler)
// ---------------------------------------------------------------------------

/// Execute a JS expression in the Tauri webview and return the result.
/// Blocks the current thread until the result arrives or timeout.
pub fn execute_eval(script: &str, timeout_secs: u64) -> Result<Value, String> {
    let eval_id = uuid::Uuid::new_v4().to_string();

    // Escape the script for embedding in a JS string template
    let escaped_script = script
        .replace('\\', "\\\\")
        .replace('`', "\\`")
        .replace("${", "\\${");

    // Wrap user script: evaluate, serialize result, invoke Tauri command
    let wrapped_js = format!(
        r#"(async () => {{
  try {{
    const __fn = new Function('return (async () => {{ {script} }})()');
    const __result = await __fn();
    const __serialized = (__result === undefined) ? 'undefined'
      : (__result === null) ? 'null'
      : (typeof __result === 'object') ? JSON.stringify(__result)
      : String(__result);
    window.__TAURI_INTERNALS__.invoke('webview_eval_result', {{
      evalId: '{eval_id}',
      success: true,
      result: __serialized
    }});
  }} catch(e) {{
    window.__TAURI_INTERNALS__.invoke('webview_eval_result', {{
      evalId: '{eval_id}',
      success: false,
      result: String(e)
    }});
  }}
}})();"#,
        script = escaped_script,
        eval_id = eval_id,
    );

    // Create oneshot channel and register waiter
    let (tx, rx) = oneshot::channel::<Value>();

    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            waiters().lock().await.insert(eval_id.clone(), tx);
        });
    });

    // Get AppHandle and eval in webview
    let handle = crate::APP_HANDLE
        .get()
        .ok_or("AppHandle not available (running in daemon mode?)")?;

    let window = handle
        .get_webview_window("main")
        .ok_or("Main webview window not found")?;

    window
        .eval(&wrapped_js)
        .map_err(|e| format!("Failed to eval JS in webview: {}", e))?;

    // Block waiting for result with timeout
    let result = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            match tokio::time::timeout(Duration::from_secs(timeout_secs), rx).await {
                Ok(Ok(value)) => Ok(value),
                Ok(Err(_)) => Err("Eval channel closed unexpectedly".to_string()),
                Err(_) => {
                    // Clean up timed-out waiter
                    waiters().lock().await.remove(&eval_id);
                    Err(format!("Eval timed out after {}s", timeout_secs))
                }
            }
        })
    })?;

    Ok(result)
}

// ---------------------------------------------------------------------------
// Screenshot — capture the Cteno window via xcap::Window
// ---------------------------------------------------------------------------

/// Capture the Cteno application window and save as PNG.
pub fn capture_webview_screenshot(output_dir: Option<&str>) -> Result<Value, String> {
    use tauri::Manager;
    use xcap::Window;

    let windows = Window::all().map_err(|e| format!("Failed to list windows: {}", e))?;

    // Find the Cteno window by matching app name or title
    let cteno_window = windows
        .iter()
        .find(|w| {
            let app_name = w.app_name().unwrap_or_default();
            let title = w.title().unwrap_or_default();
            app_name.contains("Cteno")
                || app_name.contains("cteno")
                || title.contains("Cteno")
                || title.contains("cteno")
        })
        .ok_or("Cteno window not found. Is the app visible?")?;

    let img = cteno_window
        .capture_image()
        .map_err(|e| format!("Failed to capture window: {}", e))?;

    // Determine output directory
    let screenshots_dir = if let Some(dir) = output_dir {
        PathBuf::from(dir)
    } else {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(".agents")
            .join("screenshots")
    };
    std::fs::create_dir_all(&screenshots_dir)
        .map_err(|e| format!("Failed to create screenshots dir: {}", e))?;

    let filename = format!("webview_{}.png", chrono::Utc::now().format("%Y%m%d_%H%M%S"));
    let save_path = screenshots_dir.join(&filename);

    // Encode to PNG
    let mut png_bytes = Vec::new();
    let encoder = PngEncoder::new(Cursor::new(&mut png_bytes));
    encoder
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| format!("Failed to encode PNG: {}", e))?;

    std::fs::write(&save_path, &png_bytes)
        .map_err(|e| format!("Failed to write screenshot: {}", e))?;

    let width = img.width();
    let height = img.height();

    log::info!(
        "[WebviewBridge] Screenshot saved: {} ({}x{})",
        save_path.display(),
        width,
        height
    );

    Ok(json!({
        "path": save_path.to_string_lossy(),
        "width": width,
        "height": height,
        "size_bytes": png_bytes.len(),
    }))
}
