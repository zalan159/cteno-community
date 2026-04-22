//! Browser CDP Tool Executor
//!
//! Sends raw Chrome DevTools Protocol commands to the browser.
//! This is the most flexible browser tool — it can do anything CDP supports.

use crate::browser::BrowserManager;
use crate::tool::ToolExecutor;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

pub struct BrowserCdpExecutor {
    browser_manager: Arc<BrowserManager>,
}

impl BrowserCdpExecutor {
    pub fn new(browser_manager: Arc<BrowserManager>) -> Self {
        Self { browser_manager }
    }
}

#[async_trait]
impl ToolExecutor for BrowserCdpExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        let method = input["method"]
            .as_str()
            .ok_or("Missing required parameter: method")?;

        let params = input.get("params").cloned().unwrap_or(json!({}));
        let timeout = input["timeout"].as_u64().unwrap_or(30) as u64;
        let session_id = input["__session_id"].as_str().unwrap_or("default");

        // Auto-attach to existing Chrome if no session exists
        self.browser_manager.ensure_session(session_id).await;

        let mut session = {
            let mut sessions = self.browser_manager.sessions_lock().await;
            sessions
                .remove(session_id)
                .ok_or("No browser session found. Call browser_navigate first to open a page.")?
        };

        let result = async {
            if !session.cdp.is_alive() {
                return Err(
                    "Browser connection lost. Call browser_navigate to relaunch.".to_string(),
                );
            }

            // Determine if this is a session-scoped or browser-scoped command
            // Browser-level commands (Browser.*, Target.*) don't need a session ID
            let is_browser_level = method.starts_with("Browser.")
                || method.starts_with("Target.")
                || method.starts_with("SystemInfo.");

            let cdp_session_id = if is_browser_level {
                None
            } else {
                // Ensure we have a page session for page-scoped commands
                let sid = session.ensure_page_session().await?;
                Some(sid)
            };

            let result = session
                .cdp
                .send_with_timeout(method, params, cdp_session_id.as_deref(), timeout)
                .await
                .map_err(|e| format!("CDP error: {}", e))?;

            // Return the raw CDP response as pretty JSON
            let formatted =
                serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string());

            Ok(formatted)
        }
        .await;

        {
            let mut sessions = self.browser_manager.sessions_lock().await;
            sessions.insert(session_id.to_string(), session);
        }

        result
    }
}
