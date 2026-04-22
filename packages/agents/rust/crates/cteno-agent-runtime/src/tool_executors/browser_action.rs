//! Browser Action Tool Executor
//!
//! Performs interactive actions on the browser page (click, type, scroll, etc.)
//! Each action automatically returns DOM changes after execution.

use crate::browser::manager::BrowserSession;
use crate::browser::BrowserManager;
use crate::tool::ToolExecutor;
use crate::tool_executors::oss_upload::OssUploader;
use async_trait::async_trait;
use base64::Engine;
use serde_json::{json, Value};
use std::sync::Arc;

pub struct BrowserActionExecutor {
    browser_manager: Arc<BrowserManager>,
    data_dir: std::path::PathBuf,
    oss_uploader: Arc<OssUploader>,
}

impl BrowserActionExecutor {
    pub fn new(
        browser_manager: Arc<BrowserManager>,
        data_dir: std::path::PathBuf,
        oss_uploader: Arc<OssUploader>,
    ) -> Self {
        Self {
            browser_manager,
            data_dir,
            oss_uploader,
        }
    }
}

#[async_trait]
impl ToolExecutor for BrowserActionExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        let action = input["action"]
            .as_str()
            .ok_or("Missing required parameter: action")?;

        let session_id = input["__session_id"].as_str().unwrap_or("default");

        // Auto-attach to existing Chrome if no session exists yet
        self.browser_manager.ensure_session(session_id).await;

        let session = {
            let mut sessions = self.browser_manager.sessions_lock().await;
            sessions
                .remove(session_id)
                .ok_or("No browser session found. Call browser_navigate first.")?
        };

        let result = async {
            if !session.cdp.is_alive() {
                return Err("Browser connection lost. Call browser_navigate to relaunch.".to_string());
            }

            let sid = session
                .page_session_id
                .clone()
                .ok_or("No page session. Call browser_navigate first.")?;

            // Screenshot returns early with image data (no DOM diff needed)
            if action == "screenshot" {
                return execute_screenshot(&session, &sid, &input, &self.data_dir, &self.oss_uploader).await;
            }

            // Execute the action
            let action_result = match action {
                "click" => execute_click(&session, &sid, &input).await,
                "type" => execute_type(&session, &sid, &input).await,
                "scroll" => execute_scroll(&session, &sid, &input).await,
                "evaluate" => execute_evaluate(&session, &sid, &input).await,
                _ => Err(format!("Unknown action: '{}'. Supported: click, type, scroll, evaluate, screenshot. For other operations use browser_cdp.", action)),
            }?;

            // Wait for DOM to stabilize after action
            session.wait_for_dom_stable().await.ok();

            // Get page info
            let (url, title) = session.get_page_info().await.unwrap_or_default();

            Ok(format!(
                "{}\n\nCurrent URL: {}\nTitle: {}",
                action_result, url, title,
            ))
        }
        .await;

        {
            let mut sessions = self.browser_manager.sessions_lock().await;
            sessions.insert(session_id.to_string(), session);
        }

        result
    }
}

// ─── Element resolution ───────────────────────────────────────────

/// Resolved element info for actions.
struct ResolvedElement {
    /// backendDOMNodeId from AX tree (precise DOM node reference)
    backend_node_id: Option<i64>,
    /// Bounding box center coordinates
    x: f64,
    y: f64,
    /// Description for logging
    desc: String,
}

/// Resolve an element from index or selector, returning its coordinates
/// and backendDOMNodeId for precise interaction.
async fn resolve_element(
    session: &BrowserSession,
    sid: &str,
    input: &Value,
) -> Result<ResolvedElement, String> {
    let has_selector = input["selector"].as_str().map_or(false, |s| !s.is_empty());

    // Strategy 1: element_index → backendDOMNodeId → getBoxModel → center coords
    // Skip if index is 0 and a selector is also provided (0 is likely a default value from LLM)
    let use_index = input["element_index"].as_u64().and_then(|idx| {
        if idx == 0 && has_selector {
            None // Skip default index=0 when selector is available
        } else {
            Some(idx as usize)
        }
    });

    if let Some(idx) = use_index {
        // Get backendDOMNodeId from our index map
        let backend_node_id = session.ax_backend_node_map.get(idx).copied().flatten();

        if let Some(bn_id) = backend_node_id {
            // Use getBoxModel to get precise bounding box
            let box_result = session
                .cdp
                .send(
                    "DOM.getBoxModel",
                    json!({"backendNodeId": bn_id}),
                    Some(sid),
                )
                .await;

            if let Ok(model) = box_result {
                if let Some(content) = model
                    .get("model")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                {
                    // content quad: [x1,y1, x2,y2, x3,y3, x4,y4]
                    if content.len() >= 8 {
                        let xs: Vec<f64> = content
                            .iter()
                            .step_by(2)
                            .filter_map(|v| v.as_f64())
                            .collect();
                        let ys: Vec<f64> = content
                            .iter()
                            .skip(1)
                            .step_by(2)
                            .filter_map(|v| v.as_f64())
                            .collect();
                        if !xs.is_empty() && !ys.is_empty() {
                            let cx = xs.iter().sum::<f64>() / xs.len() as f64;
                            let cy = ys.iter().sum::<f64>() / ys.len() as f64;
                            return Ok(ResolvedElement {
                                backend_node_id: Some(bn_id),
                                x: cx,
                                y: cy,
                                desc: format!("element [{}] (backendNodeId={})", idx, bn_id),
                            });
                        }
                    }
                }
            }

            // Fallback: resolve to JS objectId and get boundingClientRect
            let resolve_result = session
                .cdp
                .send(
                    "DOM.resolveNode",
                    json!({"backendNodeId": bn_id}),
                    Some(sid),
                )
                .await;

            if let Ok(resolved) = resolve_result {
                if let Some(object_id) = resolved
                    .get("object")
                    .and_then(|o| o.get("objectId"))
                    .and_then(|v| v.as_str())
                {
                    let rect_result = session
                        .cdp
                        .send(
                            "Runtime.callFunctionOn",
                            json!({
                                "objectId": object_id,
                                "functionDeclaration": "function() { const r = this.getBoundingClientRect(); return {x: r.x + r.width/2, y: r.y + r.height/2}; }",
                                "returnByValue": true,
                            }),
                            Some(sid),
                        )
                        .await;

                    if let Ok(rect) = rect_result {
                        if let Some(val) = rect["result"]["value"].as_object() {
                            let x = val.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
                            let y = val.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
                            if x > 0.0 || y > 0.0 {
                                return Ok(ResolvedElement {
                                    backend_node_id: Some(bn_id),
                                    x,
                                    y,
                                    desc: format!("element [{}] (backendNodeId={})", idx, bn_id),
                                });
                            }
                        }
                    }
                }
            }

            // Even if we can't get coords, return the backend_node_id for JS-based click
            return Ok(ResolvedElement {
                backend_node_id: Some(bn_id),
                x: 0.0,
                y: 0.0,
                desc: format!("element [{}] (backendNodeId={}, no coords)", idx, bn_id),
            });
        }

        // If we have a selector fallback, try that instead of hard-failing
        if !has_selector {
            return Err(format!(
                "Element index {} out of range (max {}). Call browser_state to refresh.",
                idx,
                session.ax_index_map.len().saturating_sub(1)
            ));
        }
        // Fall through to selector strategy
    }

    // Strategy 2: CSS selector → querySelector → coords + backendNodeId
    if let Some(selector) = input["selector"].as_str().filter(|s| !s.is_empty()) {
        // Get coords via JS
        let js = format!(
            r#"
            (() => {{
                const el = document.querySelector({});
                if (!el) return null;
                const r = el.getBoundingClientRect();
                return {{x: r.x + r.width/2, y: r.y + r.height/2}};
            }})()
            "#,
            serde_json::to_string(selector).unwrap_or_default()
        );

        let result = session
            .cdp
            .send(
                "Runtime.evaluate",
                json!({"expression": js, "returnByValue": true}),
                Some(sid),
            )
            .await
            .map_err(|e| format!("Failed to find element: {}", e))?;

        if let Some(val) = result["result"]["value"].as_object() {
            let x = val.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let y = val.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);

            // Also try to get backendNodeId via DOM.querySelector for JS click fallbacks
            let backend_node_id = get_backend_node_id_by_selector(session, sid, selector).await;

            return Ok(ResolvedElement {
                backend_node_id,
                x,
                y,
                desc: format!("\"{}\"", selector),
            });
        }

        return Err(format!("Element not found: {}", selector));
    }

    Err("Either element_index or selector is required".to_string())
}

/// Get backendNodeId for a CSS selector via DOM.getDocument + DOM.querySelector.
async fn get_backend_node_id_by_selector(
    session: &BrowserSession,
    sid: &str,
    selector: &str,
) -> Option<i64> {
    // Get document root
    let doc = session
        .cdp
        .send("DOM.getDocument", json!({"depth": 0}), Some(sid))
        .await
        .ok()?;
    let root_node_id = doc["root"]["nodeId"].as_i64()?;

    // Query selector
    let result = session
        .cdp
        .send(
            "DOM.querySelector",
            json!({"nodeId": root_node_id, "selector": selector}),
            Some(sid),
        )
        .await
        .ok()?;

    let node_id = result["nodeId"].as_i64().filter(|&id| id > 0)?;

    // Get backendNodeId via DOM.describeNode
    let desc = session
        .cdp
        .send("DOM.describeNode", json!({"nodeId": node_id}), Some(sid))
        .await
        .ok()?;

    desc["node"]["backendNodeId"].as_i64()
}

// ─── Click strategies ─────────────────────────────────────────────

/// CDP mouse event click (mouseMoved → mousePressed → mouseReleased).
/// Most reliable for pages that listen to actual mouse position.
async fn cdp_mouse_click(
    session: &BrowserSession,
    sid: &str,
    x: f64,
    y: f64,
) -> Result<(), String> {
    // First move the mouse to the target (some sites need hover state)
    session
        .cdp
        .send(
            "Input.dispatchMouseEvent",
            json!({
                "type": "mouseMoved",
                "x": x, "y": y,
            }),
            Some(sid),
        )
        .await
        .map_err(|e| format!("Mouse move failed: {}", e))?;

    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    session
        .cdp
        .send(
            "Input.dispatchMouseEvent",
            json!({
                "type": "mousePressed",
                "x": x, "y": y,
                "button": "left",
                "clickCount": 1,
            }),
            Some(sid),
        )
        .await
        .map_err(|e| format!("Mouse press failed: {}", e))?;

    session
        .cdp
        .send(
            "Input.dispatchMouseEvent",
            json!({
                "type": "mouseReleased",
                "x": x, "y": y,
                "button": "left",
                "clickCount": 1,
            }),
            Some(sid),
        )
        .await
        .map_err(|e| format!("Mouse release failed: {}", e))?;

    Ok(())
}

/// JS .click() via backendNodeId → resolveNode → callFunctionOn.
/// Works for elements with click handlers that don't need mouse position.
async fn js_click_via_backend_node(
    session: &BrowserSession,
    sid: &str,
    backend_node_id: i64,
) -> Result<(), String> {
    let resolved = session
        .cdp
        .send(
            "DOM.resolveNode",
            json!({"backendNodeId": backend_node_id}),
            Some(sid),
        )
        .await
        .map_err(|e| format!("DOM.resolveNode failed: {}", e))?;

    let object_id = resolved
        .get("object")
        .and_then(|o| o.get("objectId"))
        .and_then(|v| v.as_str())
        .ok_or("Failed to resolve DOM node to JS object")?;

    session
        .cdp
        .send(
            "Runtime.callFunctionOn",
            json!({
                "objectId": object_id,
                "functionDeclaration": "function() { this.click(); }",
            }),
            Some(sid),
        )
        .await
        .map_err(|e| format!("JS click failed: {}", e))?;

    Ok(())
}

/// Full synthetic event dispatch via backendNodeId.
/// Dispatches mousedown → mouseup → click with bubbles/cancelable.
/// Works for delegation-based event listeners (React, Vue, etc.)
async fn dispatch_click_events_via_backend_node(
    session: &BrowserSession,
    sid: &str,
    backend_node_id: i64,
) -> Result<(), String> {
    let resolved = session
        .cdp
        .send(
            "DOM.resolveNode",
            json!({"backendNodeId": backend_node_id}),
            Some(sid),
        )
        .await
        .map_err(|e| format!("DOM.resolveNode failed: {}", e))?;

    let object_id = resolved
        .get("object")
        .and_then(|o| o.get("objectId"))
        .and_then(|v| v.as_str())
        .ok_or("Failed to resolve DOM node to JS object")?;

    session
        .cdp
        .send(
            "Runtime.callFunctionOn",
            json!({
                "objectId": object_id,
                "functionDeclaration": r#"function() {
                    const opts = {bubbles: true, cancelable: true, view: window};
                    this.dispatchEvent(new PointerEvent('pointerdown', opts));
                    this.dispatchEvent(new MouseEvent('mousedown', opts));
                    this.dispatchEvent(new PointerEvent('pointerup', opts));
                    this.dispatchEvent(new MouseEvent('mouseup', opts));
                    this.dispatchEvent(new MouseEvent('click', opts));
                }"#,
            }),
            Some(sid),
        )
        .await
        .map_err(|e| format!("dispatchEvent click failed: {}", e))?;

    Ok(())
}

/// Focus an element via backendNodeId (used before type actions).
async fn focus_element(
    session: &BrowserSession,
    sid: &str,
    backend_node_id: i64,
) -> Result<(), String> {
    session
        .cdp
        .send(
            "DOM.focus",
            json!({"backendNodeId": backend_node_id}),
            Some(sid),
        )
        .await
        .map_err(|e| format!("DOM.focus failed: {}", e))?;
    Ok(())
}

// ─── Action implementations ───────────────────────────────────────

async fn execute_click(
    session: &BrowserSession,
    sid: &str,
    input: &Value,
) -> Result<String, String> {
    let elem = resolve_element(session, sid, input).await?;
    let mut methods_tried = Vec::new();

    // Strategy 1: CDP mouse events (if we have valid coordinates)
    if elem.x > 0.0 || elem.y > 0.0 {
        cdp_mouse_click(session, sid, elem.x, elem.y).await?;
        methods_tried.push("cdp_mouse");

        // Brief wait to check if the click had an effect
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    }

    // Strategy 2: JS .click() via backendNodeId (for delegated handlers)
    if let Some(bn_id) = elem.backend_node_id {
        if let Err(e) = js_click_via_backend_node(session, sid, bn_id).await {
            log::debug!("[BrowserAction] JS click fallback failed: {}", e);
        } else {
            methods_tried.push("js_click");
        }
    }

    // Strategy 3: Full synthetic event dispatch (for React/Vue delegation)
    if let Some(bn_id) = elem.backend_node_id {
        if let Err(e) = dispatch_click_events_via_backend_node(session, sid, bn_id).await {
            log::debug!("[BrowserAction] dispatchEvent fallback failed: {}", e);
        } else {
            methods_tried.push("dispatch_events");
        }
    }

    if methods_tried.is_empty() {
        return Err(format!(
            "Could not click {}: no coordinates and no backendNodeId",
            elem.desc
        ));
    }

    Ok(format!(
        "✅ Clicked {} at ({:.0}, {:.0}) [methods: {}]",
        elem.desc,
        elem.x,
        elem.y,
        methods_tried.join("+"),
    ))
}

async fn execute_type(
    session: &BrowserSession,
    sid: &str,
    input: &Value,
) -> Result<String, String> {
    let text = input["text"]
        .as_str()
        .ok_or("Missing required parameter: text")?;

    let elem = resolve_element(session, sid, input).await?;

    // Focus the element: prefer DOM.focus, fallback to click
    if let Some(bn_id) = elem.backend_node_id {
        if focus_element(session, sid, bn_id).await.is_err() {
            if elem.x > 0.0 || elem.y > 0.0 {
                cdp_mouse_click(session, sid, elem.x, elem.y).await?;
            }
        }
    } else if elem.x > 0.0 || elem.y > 0.0 {
        cdp_mouse_click(session, sid, elem.x, elem.y).await?;
    }

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Use React-compatible nativeInputValueSetter on the focused/resolved element
    let text_json = serde_json::to_string(text).unwrap_or_default();

    // If we have backendNodeId, operate on the exact element
    if let Some(bn_id) = elem.backend_node_id {
        let resolved = session
            .cdp
            .send(
                "DOM.resolveNode",
                json!({"backendNodeId": bn_id}),
                Some(sid),
            )
            .await;
        if let Ok(res) = resolved {
            if let Some(object_id) = res
                .get("object")
                .and_then(|o| o.get("objectId"))
                .and_then(|v| v.as_str())
            {
                let js_fn = format!(
                    r#"function() {{
                        const el = this;
                        el.focus();
                        const setter = Object.getOwnPropertyDescriptor(
                            el.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype,
                            'value'
                        )?.set;
                        if (setter) {{
                            setter.call(el, {});
                        }} else {{
                            el.value = {};
                        }}
                        el.dispatchEvent(new Event('input', {{bubbles: true}}));
                        el.dispatchEvent(new Event('change', {{bubbles: true}}));
                        return 'ok';
                    }}"#,
                    text_json, text_json
                );

                session
                    .cdp
                    .send(
                        "Runtime.callFunctionOn",
                        json!({"objectId": object_id, "functionDeclaration": js_fn}),
                        Some(sid),
                    )
                    .await
                    .map_err(|e| format!("Type via callFunctionOn failed: {}", e))?;

                return Ok(format!(
                    "✅ Typed \"{}\" into {}",
                    truncate(text, 50),
                    elem.desc
                ));
            }
        }
    }

    // Fallback: operate on document.activeElement
    let js = format!(
        r#"
        (() => {{
            const el = document.activeElement;
            if (!el) return 'No focused element';
            const setter = Object.getOwnPropertyDescriptor(
                el.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype,
                'value'
            )?.set;
            if (setter) {{
                setter.call(el, {});
            }} else {{
                el.value = {};
            }}
            el.dispatchEvent(new Event('input', {{bubbles: true}}));
            el.dispatchEvent(new Event('change', {{bubbles: true}}));
            return 'ok';
        }})()
        "#,
        text_json, text_json
    );

    session
        .cdp
        .send(
            "Runtime.evaluate",
            json!({"expression": js, "returnByValue": true}),
            Some(sid),
        )
        .await
        .map_err(|e| format!("Type failed: {}", e))?;

    Ok(format!(
        "✅ Typed \"{}\" into {}",
        truncate(text, 50),
        elem.desc
    ))
}

async fn execute_scroll(
    session: &BrowserSession,
    sid: &str,
    input: &Value,
) -> Result<String, String> {
    let scroll_y = input["scroll_y"].as_i64().unwrap_or(500);

    let js = format!(
        "(() => {{ const before = window.scrollY; window.scrollBy(0, {}); return {{ before: Math.round(before), after: Math.round(window.scrollY), max: Math.round(document.body.scrollHeight - window.innerHeight) }}; }})()",
        scroll_y
    );

    let result = session
        .cdp
        .send(
            "Runtime.evaluate",
            json!({"expression": js, "returnByValue": true}),
            Some(sid),
        )
        .await
        .map_err(|e| format!("Scroll failed: {}", e))?;

    let val = &result["result"]["value"];
    let before = val["before"].as_i64().unwrap_or(0);
    let after = val["after"].as_i64().unwrap_or(0);
    let max = val["max"].as_i64().unwrap_or(0);

    Ok(format!(
        "✅ Scrolled {}px (position: {} → {}, max: {})",
        scroll_y, before, after, max
    ))
}

async fn execute_evaluate(
    session: &BrowserSession,
    sid: &str,
    input: &Value,
) -> Result<String, String> {
    let expression = input["text"]
        .as_str()
        .ok_or("Missing required parameter: text (JS expression)")?;

    let result = session
        .send_or_reload(
            "Runtime.evaluate",
            json!({
                "expression": expression,
                "returnByValue": true,
            }),
        )
        .await
        .map_err(|e| format!("JS evaluation failed: {}", e))?;

    if let Some(exception) = result.get("exceptionDetails") {
        // Extract the most useful error info from CDP exception
        let description = exception
            .get("exception")
            .and_then(|e| e.get("description"))
            .and_then(|d| d.as_str())
            .or_else(|| exception.get("text").and_then(|t| t.as_str()))
            .unwrap_or("Unknown JS error");
        return Err(format!("JS error: {}", description));
    }

    let val = &result["result"];
    let output = match val["type"].as_str() {
        Some("undefined") => "undefined".to_string(),
        Some("string") => val["value"].as_str().unwrap_or("").to_string(),
        _ => serde_json::to_string_pretty(&val["value"]).unwrap_or("null".to_string()),
    };

    Ok(format!("✅ JS result: {}", output))
}

async fn execute_screenshot(
    session: &BrowserSession,
    _sid: &str,
    input: &Value,
    data_dir: &std::path::Path,
    oss_uploader: &OssUploader,
) -> Result<String, String> {
    let full_page = input["full_page"].as_bool().unwrap_or(false);
    let supports_vision = input
        .get("__supports_vision")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let mut params = json!({"format": "png"});
    if full_page {
        params["captureBeyondViewport"] = json!(true);
    }

    let result = session
        .send_or_reload("Page.captureScreenshot", params)
        .await
        .map_err(|e| format!("Screenshot failed: {}", e))?;

    let data_b64 = result["data"]
        .as_str()
        .ok_or("No screenshot data in CDP response")?;

    // Save PNG to workdir/.screenshots/
    let save_path = {
        let workdir = input["__persona_workdir"]
            .as_str()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| data_dir.join("screenshots"));
        let dir = workdir.join(".screenshots");
        let _ = std::fs::create_dir_all(&dir);
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let path = dir.join(format!("browser_{}.png", timestamp));
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data_b64)
            .map_err(|e| format!("Base64 decode: {}", e))?;
        std::fs::write(&path, &bytes).map_err(|e| format!("Save screenshot: {}", e))?;
        path
    };

    // Upload to OSS (graceful degradation)
    let png_bytes = base64::engine::general_purpose::STANDARD
        .decode(data_b64)
        .map_err(|e| format!("Failed to decode base64: {}", e))?;
    let filename = save_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("browser_action.png");
    let image_url = match oss_uploader
        .upload_bytes_and_get_url(&png_bytes, filename, "image/png", 7)
        .await
    {
        Ok(url) => Some(url),
        Err(e) => {
            log::warn!(
                "[BrowserAction] OSS upload failed (degrading gracefully): {}",
                e
            );
            None
        }
    };

    if supports_vision {
        // Vision model: base64 images for LLM + image_url for frontend
        let mut result = json!({
            "type": "browser_screenshot",
            "image_path": save_path.display().to_string(),
            "images": [{
                "type": "base64",
                "media_type": "image/png",
                "data": data_b64,
            }]
        });
        if let Some(url) = image_url {
            result["image_url"] = json!(url);
        }
        Ok(result.to_string())
    } else {
        // Non-vision model: URL + path only, no base64
        let mut result = json!({
            "type": "browser_screenshot",
            "image_path": save_path.display().to_string(),
        });
        if let Some(url) = image_url {
            result["image_url"] = json!(url);
        }
        Ok(result.to_string())
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max - 3).collect();
        format!("{}...", t)
    }
}
