//! Browser Manage Tool Executor
//!
//! Tab management and browser lifecycle operations.

use crate::browser::BrowserManager;
use crate::tool::ToolExecutor;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

pub struct BrowserManageExecutor {
    browser_manager: Arc<BrowserManager>,
}

impl BrowserManageExecutor {
    pub fn new(browser_manager: Arc<BrowserManager>) -> Self {
        Self { browser_manager }
    }
}

#[async_trait]
impl ToolExecutor for BrowserManageExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        let action = input["action"]
            .as_str()
            .ok_or("Missing required parameter: action")?;

        let session_id = input["__session_id"].as_str().unwrap_or("default");

        if action == "close_browser" {
            self.browser_manager.close_session(session_id).await;
            return Ok("Browser closed and cleaned up.".to_string());
        }

        // Auto-attach to existing Chrome if no session exists yet
        self.browser_manager.ensure_session(session_id).await;

        let mut session = {
            let mut sessions = self.browser_manager.sessions_lock().await;
            sessions
                .remove(session_id)
                .ok_or("No browser session found. Call browser_navigate first to open a page.")?
        };

        let result = match action {
            "list_tabs" => {
                let tabs =
                    get_page_targets(&session.cdp, session.page_session_id.as_deref()).await?;

                if tabs.is_empty() {
                    Ok("No tabs open.".to_string())
                } else {
                    Ok(format!("{} tab(s):\n{}", tabs.len(), tabs.join("\n")))
                }
            }

            "switch_tab" => {
                // Find the target — by tab_index, url/title match, or raw target_id
                let target_id = resolve_tab_target(&session.cdp, &input).await?;

                // Activate the target (brings it to front)
                session
                    .cdp
                    .send(
                        "Target.activateTarget",
                        json!({"targetId": target_id}),
                        None,
                    )
                    .await
                    .map_err(|e| format!("Failed to activate tab: {}", e))?;

                // Attach CDP session to it
                let attach = session
                    .cdp
                    .send(
                        "Target.attachToTarget",
                        json!({"targetId": target_id, "flatten": true}),
                        None,
                    )
                    .await
                    .map_err(|e| format!("Failed to attach to tab: {}", e))?;

                let sid = attach["sessionId"]
                    .as_str()
                    .ok_or("Missing sessionId")?
                    .to_string();

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

                session.page_session_id = Some(sid);
                session.ax_index_map.clear();
                session.ax_backend_node_map.clear();
                session.last_ax_snapshot.clear();

                // Get page info for confirmation
                let (url, title) = session.get_page_info().await.unwrap_or_default();

                Ok(format!("Switched to tab: {} ({})", title, url))
            }

            "new_tab" => {
                let url = input["url"].as_str().unwrap_or("about:blank");

                let result = session
                    .cdp
                    .send("Target.createTarget", json!({"url": url}), None)
                    .await
                    .map_err(|e| format!("Failed to create tab: {}", e))?;

                let target_id = result["targetId"].as_str().unwrap_or("unknown");

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

                session.page_session_id = Some(sid);
                session.ax_index_map.clear();
                session.ax_backend_node_map.clear();
                session.last_ax_snapshot.clear();

                Ok(format!("New tab opened: {} ({})", url, target_id))
            }

            "close_tab" => {
                let target_id = resolve_tab_target(&session.cdp, &input).await?;

                session
                    .cdp
                    .send("Target.closeTarget", json!({"targetId": target_id}), None)
                    .await
                    .map_err(|e| format!("Failed to close tab: {}", e))?;

                session.page_session_id = None;
                session.ax_index_map.clear();
                session.ax_backend_node_map.clear();
                session.last_ax_snapshot.clear();

                Ok(format!("Tab closed: {}", target_id))
            }

            _ => Err(format!("Unknown browser_manage action: {}", action)),
        };

        {
            let mut sessions = self.browser_manager.sessions_lock().await;
            sessions.insert(session_id.to_string(), session);
        }

        result
    }
}

/// Get all page targets, formatted with index and active marker.
async fn get_page_targets(
    cdp: &crate::browser::cdp::CdpConnection,
    active_session_id: Option<&str>,
) -> Result<Vec<String>, String> {
    let result = cdp
        .send("Target.getTargets", json!({}), None)
        .await
        .map_err(|e| format!("Failed to get targets: {}", e))?;

    let mut tabs = Vec::new();
    if let Some(infos) = result["targetInfos"].as_array() {
        // Get the targetId of the currently attached session for marking
        let active_target_id = if active_session_id.is_some() {
            infos.iter().find_map(|t| {
                if t["type"].as_str() == Some("page")
                    && t.get("attached").and_then(|v| v.as_bool()).unwrap_or(false)
                {
                    t["targetId"].as_str().map(|s| s.to_string())
                } else {
                    None
                }
            })
        } else {
            None
        };

        // Filter to page targets first, then enumerate — ensures indices match resolve_tab_target
        let pages: Vec<&Value> = infos
            .iter()
            .filter(|t| t["type"].as_str() == Some("page"))
            .collect();
        for (idx, info) in pages.iter().enumerate() {
            let tid = info["targetId"].as_str().unwrap_or("?");
            let title = info["title"].as_str().unwrap_or("Untitled");
            let url = info["url"].as_str().unwrap_or("");
            let is_active = active_target_id.as_deref() == Some(tid);
            let marker = if is_active { " *active*" } else { "" };

            tabs.push(format!("[{}] \"{}\" — {}{}", idx, title, url, marker));
        }
    }

    Ok(tabs)
}

/// Resolve a tab target from input — supports tab_index, url/title search, or raw target_id.
async fn resolve_tab_target(
    cdp: &crate::browser::cdp::CdpConnection,
    input: &Value,
) -> Result<String, String> {
    // Direct target_id
    if let Some(tid) = input["target_id"].as_str() {
        return Ok(tid.to_string());
    }

    // Get all page targets
    let result = cdp
        .send("Target.getTargets", json!({}), None)
        .await
        .map_err(|e| format!("Failed to get targets: {}", e))?;

    let pages: Vec<&Value> = result["targetInfos"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter(|t| t["type"].as_str() == Some("page"))
                .collect()
        })
        .unwrap_or_default();

    if pages.is_empty() {
        return Err("No page tabs found".to_string());
    }

    // By tab_index
    if let Some(idx) = input["tab_index"].as_u64() {
        let idx = idx as usize;
        return pages
            .get(idx)
            .and_then(|t| t["targetId"].as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| format!("Tab index {} out of range (0-{})", idx, pages.len() - 1));
    }

    // By URL or title match (substring)
    if let Some(query) = input["url"].as_str().or(input["text"].as_str()) {
        let query_lower = query.to_lowercase();
        for t in &pages {
            let url = t["url"].as_str().unwrap_or("").to_lowercase();
            let title = t["title"].as_str().unwrap_or("").to_lowercase();
            if url.contains(&query_lower) || title.contains(&query_lower) {
                return t["targetId"]
                    .as_str()
                    .map(|s| s.to_string())
                    .ok_or_else(|| "Target has no ID".to_string());
            }
        }
        return Err(format!(
            "No tab matching \"{}\". Use list_tabs to see available tabs.",
            query
        ));
    }

    Err("Specify tab_index, url, text, or target_id to identify the tab".to_string())
}
