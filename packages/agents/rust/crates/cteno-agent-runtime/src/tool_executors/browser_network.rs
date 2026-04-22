//! Browser Network Tool Executor
//!
//! Monitor network requests via CDP Network domain events.
//! Captures ALL HTTP requests including WebWorkers and pre-initialized instances.

use crate::browser::network::{CapturedRequest, NetworkCapture};
use crate::browser::BrowserManager;
use crate::tool::ToolExecutor;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

pub struct BrowserNetworkExecutor {
    browser_manager: Arc<BrowserManager>,
}

impl BrowserNetworkExecutor {
    pub fn new(browser_manager: Arc<BrowserManager>) -> Self {
        Self { browser_manager }
    }
}

#[async_trait]
impl ToolExecutor for BrowserNetworkExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        let action = input["action"]
            .as_str()
            .ok_or("Missing required parameter: action")?;

        let session_id = input["__session_id"].as_str().unwrap_or("default");

        self.browser_manager.ensure_session(session_id).await;

        let mut session = {
            let mut sessions = self.browser_manager.sessions_lock().await;
            sessions
                .remove(session_id)
                .ok_or("No browser session found. Call browser_navigate first to open a page.")?
        };

        let result = async {
            if !session.cdp.is_alive() {
                return Err("Browser connection lost. Call browser_navigate to relaunch.".to_string());
            }

            let sid = session.ensure_page_session().await?;

            match action {
            "start_capture" => {
                let max_requests = input["max_requests"].as_u64().unwrap_or(200) as usize;
                let filter = input["filter"].as_str().map(|s| s.to_string());
                let method_filter = input["method_filter"].as_str().map(|s| s.to_string());

                // Enable CDP Network domain
                session
                    .cdp
                    .send("Network.enable", json!({}), Some(&sid))
                    .await
                    .map_err(|e| format!("Failed to enable Network domain: {}", e))?;

                let mut capture = NetworkCapture::new(max_requests);
                capture.filter_pattern = filter;
                capture.method_filter = method_filter;

                let requests_buf = Arc::clone(&capture.requests);
                let pending_buf = Arc::clone(&capture.pending);

                // Subscribe to requestWillBeSent
                let mut req_rx = session.cdp.subscribe("Network.requestWillBeSent").await;
                let pending_for_req = Arc::clone(&pending_buf);
                let requests_for_req = Arc::clone(&requests_buf);

                let req_handle = tokio::spawn(async move {
                    while let Some(event) = req_rx.recv().await {
                        let request = &event["request"];
                        let request_id = event["requestId"].as_str().unwrap_or("").to_string();
                        let url = request["url"].as_str().unwrap_or("").to_string();
                        let method = request["method"].as_str().unwrap_or("GET").to_string();
                        let resource_type = event["type"].as_str().map(|s| s.to_string());
                        let timestamp = event["wallTime"].as_f64();
                        let post_data = request["postData"].as_str().map(|s| {
                            if s.len() > 2000 {
                                let mut end = 2000;
                                while end > 0 && !s.is_char_boundary(end) { end -= 1; }
                                s[..end].to_string()
                            } else { s.to_string() }
                        });
                        let headers = request.get("headers").cloned();

                        let captured = CapturedRequest {
                            url,
                            method,
                            status: None,
                            content_type: None,
                            resource_type,
                            timestamp,
                            post_data,
                            request_headers: headers,
                        };

                        if let Ok(mut pending) = pending_for_req.lock() {
                            pending.insert(request_id.clone(), captured);
                        }

                        // Also add to completed immediately (status will be updated later)
                        // This ensures requests that never get a response are still captured
                        if let Ok(pending) = pending_for_req.lock() {
                            if let Some(req) = pending.get(&request_id) {
                                if let Ok(mut reqs) = requests_for_req.lock() {
                                    // Only add if not already there
                                    if !reqs.iter().any(|r| r.url == req.url && r.timestamp == req.timestamp) {
                                        reqs.push(req.clone());
                                    }
                                }
                            }
                        }
                    }
                });
                capture.task_handles.push(req_handle);

                // Subscribe to responseReceived
                let mut resp_rx = session.cdp.subscribe("Network.responseReceived").await;
                let pending_for_resp = Arc::clone(&pending_buf);
                let requests_for_resp = Arc::clone(&requests_buf);

                let resp_handle = tokio::spawn(async move {
                    while let Some(event) = resp_rx.recv().await {
                        let request_id = event["requestId"].as_str().unwrap_or("").to_string();
                        let response = &event["response"];
                        let status = response["status"].as_u64().map(|s| s as u16);
                        let content_type = response["mimeType"].as_str().map(|s| s.to_string());
                        let resource_type = event["type"].as_str().map(|s| s.to_string());

                        // Update the pending request with response info
                        if let Ok(mut pending) = pending_for_resp.lock() {
                            if let Some(req) = pending.remove(&request_id) {
                                let updated = CapturedRequest {
                                    status,
                                    content_type,
                                    resource_type: resource_type.or(req.resource_type),
                                    ..req
                                };
                                // Update in completed list
                                if let Ok(mut reqs) = requests_for_resp.lock() {
                                    // Find and update the matching request
                                    if let Some(existing) = reqs.iter_mut().find(|r| {
                                        r.url == updated.url && r.timestamp == updated.timestamp
                                    }) {
                                        existing.status = updated.status;
                                        existing.content_type = updated.content_type.clone();
                                        existing.resource_type = updated.resource_type.clone();
                                    }
                                }
                            }
                        }
                    }
                });
                capture.task_handles.push(resp_handle);

                session.network_capture = Some(capture);

                Ok("Network capture started (CDP Network.enable). Use get_requests to see captured requests.".to_string())
            }

            "get_requests" => {
                let capture = session.network_capture.as_ref().ok_or(
                    "No active network capture. Call start_capture first.",
                )?;

                // Apply filter overrides if provided
                let filter = input["filter"].as_str();
                let method_filter = input["method_filter"].as_str();

                let all_requests = capture.requests.lock()
                    .map(|r| r.clone())
                    .unwrap_or_default();
                let total = all_requests.len();

                // Build a temporary filter
                let effective_filter = filter
                    .map(|s| s.to_string())
                    .or_else(|| capture.filter_pattern.clone());
                let effective_method = method_filter
                    .map(|s| s.to_string())
                    .or_else(|| capture.method_filter.clone());
                let max = input["max_requests"].as_u64()
                    .map(|v| v as usize)
                    .unwrap_or(capture.max_requests);

                let filtered: Vec<&CapturedRequest> = all_requests
                    .iter()
                    .filter(|r| {
                        if let Some(ref pat) = effective_filter {
                            if !r.url.to_lowercase().contains(&pat.to_lowercase()) {
                                return false;
                            }
                        }
                        if let Some(ref m) = effective_method {
                            if m.to_uppercase() != "ALL"
                                && r.method.to_uppercase() != m.to_uppercase()
                            {
                                return false;
                            }
                        }
                        true
                    })
                    .take(max)
                    .collect();

                let shown = filtered.len();

                if filtered.is_empty() {
                    return Ok(format!(
                        "No requests captured{} ({} total unfiltered).",
                        if effective_filter.is_some() || effective_method.is_some() {
                            " matching filters"
                        } else { "" },
                        total,
                    ));
                }

                let mut lines = Vec::new();
                for (i, req) in filtered.iter().enumerate() {
                    let status_str = req.status
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "---".to_string());
                    let ct = req.content_type.as_deref().unwrap_or("");
                    let rt = req.resource_type.as_deref().unwrap_or("");

                    lines.push(format!(
                        "[{}] {} {} → {} {} {}",
                        i, req.method, req.url, status_str, ct, rt,
                    ));

                    if let Some(ref body) = req.post_data {
                        lines.push(format!("    Body: {}", &body[..body.len().min(200)]));
                    }
                }

                let filter_note = if shown < total {
                    format!(" (showing {} of {} total)", shown, total)
                } else {
                    String::new()
                };

                Ok(format!(
                    "{} request(s) captured{}:\n{}",
                    shown, filter_note, lines.join("\n"),
                ))
            }

            "clear" => {
                if let Some(ref capture) = session.network_capture {
                    capture.clear();
                }
                Ok("Network requests cleared.".to_string())
            }

            "stop_capture" => {
                let count = session.network_capture.as_ref()
                    .map(|c| c.count())
                    .unwrap_or(0);

                if let Some(ref mut capture) = session.network_capture {
                    capture.stop();
                }

                // Disable Network domain
                let _ = session.cdp.send("Network.disable", json!({}), Some(&sid)).await;

                session.network_capture = None;

                Ok(format!(
                    "Network capture stopped. {} request(s) were captured.",
                    count,
                ))
            }

            _ => Err(format!("Unknown browser_network action: {}", action)),
        }
        }
        .await;

        {
            let mut sessions = self.browser_manager.sessions_lock().await;
            sessions.insert(session_id.to_string(), session);
        }

        result
    }
}
