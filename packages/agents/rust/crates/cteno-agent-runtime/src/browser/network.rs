//! Network capture state for browser_network tool.
//!
//! Uses CDP Network domain events (requestWillBeSent, responseReceived) to capture
//! all HTTP requests — including those from WebWorkers and pre-initialized instances
//! that JS monkey-patching would miss.

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::task::JoinHandle;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturedRequest {
    pub url: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_data: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_headers: Option<serde_json::Value>,
}

/// CDP-based network capture state.
pub struct NetworkCapture {
    pub is_capturing: bool,
    pub filter_pattern: Option<String>,
    pub method_filter: Option<String>,
    pub max_requests: usize,
    /// Shared buffer for captured requests (written by background tasks).
    pub requests: Arc<std::sync::Mutex<Vec<CapturedRequest>>>,
    /// Map of requestId → partial CapturedRequest (waiting for response).
    pub pending: Arc<std::sync::Mutex<std::collections::HashMap<String, CapturedRequest>>>,
    /// Background task handles (request + response listeners).
    pub task_handles: Vec<JoinHandle<()>>,
}

impl NetworkCapture {
    pub fn new(max_requests: usize) -> Self {
        Self {
            is_capturing: true,
            filter_pattern: None,
            method_filter: None,
            max_requests,
            requests: Arc::new(std::sync::Mutex::new(Vec::new())),
            pending: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            task_handles: Vec::new(),
        }
    }

    /// Get filtered requests from the buffer.
    pub fn get_filtered_requests(&self) -> Vec<CapturedRequest> {
        let requests = self.requests.lock().unwrap_or_else(|e| e.into_inner());
        requests
            .iter()
            .filter(|r| {
                if let Some(ref pattern) = self.filter_pattern {
                    if !r.url.to_lowercase().contains(&pattern.to_lowercase()) {
                        return false;
                    }
                }
                if let Some(ref method) = self.method_filter {
                    if method.to_uppercase() != "ALL"
                        && r.method.to_uppercase() != method.to_uppercase()
                    {
                        return false;
                    }
                }
                true
            })
            .take(self.max_requests)
            .cloned()
            .collect()
    }

    /// Stop capturing and clean up background tasks.
    pub fn stop(&mut self) {
        self.is_capturing = false;
        for handle in self.task_handles.drain(..) {
            handle.abort();
        }
    }

    /// Clear captured requests.
    pub fn clear(&self) {
        if let Ok(mut requests) = self.requests.lock() {
            requests.clear();
        }
        if let Ok(mut pending) = self.pending.lock() {
            pending.clear();
        }
    }

    /// Total number of captured requests.
    pub fn count(&self) -> usize {
        self.requests.lock().map(|r| r.len()).unwrap_or(0)
    }
}

impl Drop for NetworkCapture {
    fn drop(&mut self) {
        for handle in self.task_handles.drain(..) {
            handle.abort();
        }
    }
}
