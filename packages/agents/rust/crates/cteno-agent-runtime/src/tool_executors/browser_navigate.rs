//! Browser Navigate Tool Executor
//!
//! Opens a URL in Chrome via CDP. Lazily launches Chrome on first call,
//! copying the user's profile for login state preservation.
//! When attaching to an existing Chrome, lists all open tabs.

use crate::browser::BrowserManager;
use crate::tool::ToolExecutor;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

pub struct BrowserNavigateExecutor {
    browser_manager: Arc<BrowserManager>,
}

impl BrowserNavigateExecutor {
    pub fn new(browser_manager: Arc<BrowserManager>) -> Self {
        Self { browser_manager }
    }
}

#[async_trait]
impl ToolExecutor for BrowserNavigateExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        let url = input["url"]
            .as_str()
            .ok_or("Missing required parameter: url")?;

        let headless = input["headless"].as_bool().unwrap_or(false);
        let wait_seconds = input["wait_seconds"].as_f64().unwrap_or(3.0);

        let session_id = input["__session_id"].as_str().unwrap_or("default");

        // Ensure browser is running (may attach to existing Chrome or launch new)
        self.browser_manager
            .get_or_create(session_id, headless)
            .await?;

        let bm = &self.browser_manager;
        let url_owned = url.to_string();
        let wait_ms = (wait_seconds * 1000.0) as u64;

        let mut session = {
            let mut sessions = bm.sessions.lock().await;
            sessions
                .remove(session_id)
                .ok_or("Browser session not found after creation")?
        };

        let result = async {
            // List existing tabs before navigation
            let existing_tabs = list_page_tabs(&session.cdp).await;

            // ── Strategy: reuse existing session OR create fresh tab ──
            //
            // If we already have a page session, try Page.navigate on it.
            // Otherwise (first call, or session stale), create a new tab
            // directly with the URL — this is the most reliable path because
            // Chrome handles the navigation internally.

            let sid = if let Some(ref existing_sid) = session.page_session_id {
                // Try navigating the existing tab
                log::info!(
                    "[BrowserNavigate] Reusing existing page session, navigating to {}",
                    url_owned
                );
                let existing_sid_clone = existing_sid.clone();

                // Subscribe to load event BEFORE navigation
                let mut load_rx = session.cdp.subscribe("Page.loadEventFired").await;

                let nav_result = session
                    .cdp
                    .send(
                        "Page.navigate",
                        json!({"url": url_owned}),
                        Some(&existing_sid_clone),
                    )
                    .await;

                match nav_result {
                    Ok(ref r) => {
                        // Check for navigation errors
                        if let Some(error_text) = r.get("errorText").and_then(|v| v.as_str()) {
                            if !error_text.is_empty() {
                                log::warn!(
                                    "[BrowserNavigate] Page.navigate errorText: {}",
                                    error_text
                                );
                                // Fall through to create-tab approach below
                                drop(load_rx);
                                create_tab_and_navigate(&mut session, &url_owned, wait_ms).await?
                            } else {
                                wait_for_load(&mut load_rx, wait_ms).await;
                                existing_sid_clone
                            }
                        } else {
                            wait_for_load(&mut load_rx, wait_ms).await;
                            existing_sid_clone
                        }
                    }
                    Err(e) => {
                        log::warn!(
                            "[BrowserNavigate] Page.navigate failed on existing session: {}, creating new tab",
                            e
                        );
                        drop(load_rx);
                        create_tab_and_navigate(&mut session, &url_owned, wait_ms).await?
                    }
                }
            } else {
                // No existing page session — create tab directly with URL
                log::info!(
                    "[BrowserNavigate] No page session, creating tab with URL: {}",
                    url_owned
                );
                create_tab_and_navigate(&mut session, &url_owned, wait_ms).await?
            };

            // Bring to foreground
            if let Some(ref target_id) = session.page_target_id {
                if let Err(e) = session
                    .cdp
                    .send("Target.activateTarget", json!({"targetId": target_id}), None)
                    .await
                {
                    log::warn!("[BrowserNavigate] Target.activateTarget failed: {}", e);
                }
            }
            if let Err(e) = session
                .cdp
                .send("Page.bringToFront", json!({}), Some(&sid))
                .await
            {
                log::warn!("[BrowserNavigate] Page.bringToFront failed: {}", e);
            }

            // Get page info
            let (page_url, page_title) = session.get_page_info().await?;

            log::info!("[BrowserNavigate] Page loaded: {} - {}", page_url, page_title);

            // Get initial AX tree snapshot
            let ax_nodes = session.get_ax_tree().await.unwrap_or_default();
            let parsed = crate::browser::ax_tree::parse_ax_tree(&ax_nodes);
            let result = crate::browser::ax_tree::build_indexed_tree(&parsed, 50, false, None);
            let tree_text = crate::browser::ax_tree::render_tree(&result.nodes);
            let count = result.nodes.len();

            session.ax_index_map = result.node_id_map;
            session.ax_backend_node_map = result.backend_node_map;
            session.last_ax_snapshot = parsed;

            // Build response — include tab list if there are other tabs
            let mut response = format!(
                "Navigated to: {}\nTitle: {}\n\nPage structure (first 50 elements):\n{}\n\n{} elements indexed.",
                page_url, page_title, tree_text, count
            );

            if existing_tabs.len() > 1 {
                response.push_str(&format!(
                    "\n\nOther open tabs ({} total):\n{}\nUse browser_manage(switch_tab) to operate on a different tab.",
                    existing_tabs.len(),
                    existing_tabs.join("\n")
                ));
            }

            Ok(response)
        }
        .await;

        {
            let mut sessions = bm.sessions.lock().await;
            sessions.insert(session_id.to_string(), session);
        }

        result
    }
}

/// Create a new tab with the URL and attach to it.
/// This is the most reliable navigation path — Chrome opens and loads the URL
/// internally, bypassing any session attachment issues with existing tabs.
async fn create_tab_and_navigate(
    session: &mut crate::browser::manager::BrowserSession,
    url: &str,
    wait_ms: u64,
) -> Result<String, String> {
    // Create tab directly with the target URL
    let result = session
        .cdp
        .send("Target.createTarget", json!({"url": url}), None)
        .await
        .map_err(|e| format!("Failed to create tab: {}", e))?;

    let target_id = result["targetId"]
        .as_str()
        .ok_or("Missing targetId after createTarget")?
        .to_string();

    log::info!(
        "[BrowserNavigate] Created tab {} with URL {}",
        target_id,
        url
    );

    // Attach to the new tab
    let attach = session
        .cdp
        .send(
            "Target.attachToTarget",
            json!({"targetId": target_id, "flatten": true}),
            None,
        )
        .await
        .map_err(|e| format!("Failed to attach to new tab: {}", e))?;

    let sid = attach["sessionId"]
        .as_str()
        .ok_or("Missing sessionId")?
        .to_string();

    // Enable required domains
    session
        .cdp
        .send("Page.enable", json!({}), Some(&sid))
        .await
        .ok();
    session
        .cdp
        .send("DOM.enable", json!({}), Some(&sid))
        .await
        .ok();
    session
        .cdp
        .send("Accessibility.enable", json!({}), Some(&sid))
        .await
        .ok();

    // Update session state
    session.page_session_id = Some(sid.clone());
    session.page_target_id = Some(target_id);

    // Wait for the page to load (Chrome is already loading the URL from createTarget)
    let mut load_rx = session.cdp.subscribe("Page.loadEventFired").await;
    wait_for_load(&mut load_rx, wait_ms).await;

    Ok(sid)
}

/// Wait for Page.loadEventFired with timeout, with a small extra delay for JS rendering.
async fn wait_for_load(load_rx: &mut tokio::sync::mpsc::UnboundedReceiver<Value>, wait_ms: u64) {
    if wait_ms == 0 {
        return;
    }

    match tokio::time::timeout(tokio::time::Duration::from_millis(wait_ms), load_rx.recv()).await {
        Ok(Some(_)) => {
            log::info!("[BrowserNavigate] Page.loadEventFired received");
            // Small extra wait for JS rendering
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }
        _ => {
            log::warn!(
                "[BrowserNavigate] Page.loadEventFired timeout after {}ms, continuing",
                wait_ms
            );
        }
    }
}

/// List all page tabs for display.
async fn list_page_tabs(cdp: &crate::browser::cdp::CdpConnection) -> Vec<String> {
    let result = match cdp.send("Target.getTargets", json!({}), None).await {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    let mut tabs = Vec::new();
    if let Some(infos) = result["targetInfos"].as_array() {
        // Filter to page targets first, then enumerate — ensures indices match switch_tab
        let pages: Vec<&Value> = infos
            .iter()
            .filter(|t| t["type"].as_str() == Some("page"))
            .collect();
        for (idx, info) in pages.iter().enumerate() {
            let title = info["title"].as_str().unwrap_or("Untitled");
            let url = info["url"].as_str().unwrap_or("");
            tabs.push(format!("[{}] \"{}\" — {}", idx, title, url));
        }
    }

    tabs
}
