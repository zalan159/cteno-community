//! BrowserManager: Per-session Chrome instance lifecycle management.
//!
//! Each Agent session gets its own Chrome process with a copied profile.
//! Instances are lazily created on first `browser_navigate` call.

use super::ax_tree::AXNode;
use super::cdp::CdpConnection;
use super::trace::DialogEvent;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Child;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

/// State for a single browser session.
pub struct BrowserSession {
    /// Chrome process we launched (None if we attached to an existing Chrome).
    pub chrome_process: Option<Child>,
    pub cdp: CdpConnection,
    /// Temp profile dir (None if we attached to an existing Chrome).
    pub tmp_profile_dir: Option<PathBuf>,
    pub page_session_id: Option<String>,
    /// CDP target ID of the attached page (for Target.activateTarget).
    pub page_target_id: Option<String>,
    pub port: u16,
    /// Element index → AX nodeId mapping (from last browser_state/action).
    pub ax_index_map: Vec<String>,
    /// Element index → backendDOMNodeId mapping (for precise DOM resolution).
    pub ax_backend_node_map: Vec<Option<i64>>,
    /// Last AX Tree snapshot (for diffing after actions).
    pub last_ax_snapshot: Vec<AXNode>,
    /// backendDOMNodeId → XPath mapping (for element tracking across DOM changes).
    pub xpath_map: HashMap<i64, String>,
    /// Network capture state (set by browser_network tool).
    pub network_capture: Option<super::network::NetworkCapture>,
    /// Native browser dialogs that were auto-handled (alert, confirm, prompt, auth).
    pub dialog_events: Arc<std::sync::Mutex<Vec<DialogEvent>>>,
    /// Handles for background dialog handler tasks (aborted on session cleanup).
    pub dialog_handler_handles: Vec<JoinHandle<()>>,
}

impl BrowserSession {
    /// Get or attach to the best page target, returning the CDP session ID.
    ///
    /// Priority: URL-matching tab (if target_url given) > non-empty tab > any page tab.
    /// Skips chrome:// and about: tabs.
    pub async fn ensure_page_session(&mut self) -> Result<String, String> {
        if let Some(ref sid) = self.page_session_id {
            return Ok(sid.clone());
        }

        self.attach_best_page_target(None).await
    }

    /// Attach to the best page target, optionally matching a URL.
    /// Resets page_session_id so we always get a fresh attachment.
    pub async fn attach_best_page_target(
        &mut self,
        target_url: Option<&str>,
    ) -> Result<String, String> {
        // Clear existing session so we re-attach
        self.page_session_id = None;

        let targets = self
            .cdp
            .send("Target.getTargets", json!({}), None)
            .await
            .map_err(|e| format!("Failed to get targets: {}", e))?;

        let pages: Vec<&Value> = targets["targetInfos"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter(|t| t["type"].as_str() == Some("page"))
                    .collect()
            })
            .unwrap_or_default();

        if pages.is_empty() {
            return Err("No page target found in Chrome".to_string());
        }

        // Pick the best target
        let target_id = if let Some(url) = target_url {
            // Prefer exact URL match
            pages
                .iter()
                .find(|t| t["url"].as_str() == Some(url))
                .or_else(|| {
                    // Prefix match
                    pages.iter().find(|t| {
                        t["url"]
                            .as_str()
                            .map(|u| u.starts_with(url))
                            .unwrap_or(false)
                    })
                })
                .or_else(|| pages.iter().find(|t| is_real_page(t)))
                .or(pages.first())
        } else {
            // No URL hint: prefer a real page over chrome://newtab
            pages.iter().find(|t| is_real_page(t)).or(pages.first())
        };

        let target_id = target_id
            .and_then(|t| t["targetId"].as_str())
            .ok_or("No suitable page target found")?
            .to_string();

        let sid = match self
            .cdp
            .send(
                "Target.attachToTarget",
                json!({"targetId": target_id, "flatten": true}),
                None,
            )
            .await
        {
            Ok(attach) => attach["sessionId"]
                .as_str()
                .ok_or("Missing sessionId in attach response")?
                .to_string(),
            Err(e) => {
                // Target might already be attached (e.g. by auto-attach or previous session).
                // Try to find it in the attached targets list.
                log::warn!(
                    "[BrowserSession] attachToTarget failed for {}: {}, trying attached list",
                    target_id,
                    e
                );
                let targets = self
                    .cdp
                    .send("Target.getTargets", json!({}), None)
                    .await
                    .map_err(|e2| format!("Failed to get targets after attach failure: {}", e2))?;

                let session_id = targets["targetInfos"]
                    .as_array()
                    .and_then(|arr| {
                        arr.iter().find(|t| {
                            t["targetId"].as_str() == Some(&target_id)
                                && t["attached"].as_bool() == Some(true)
                        })
                    })
                    .and_then(|_| {
                        // Target is attached but we don't have the sessionId from getTargets.
                        // Create a new tab instead and attach to that.
                        None::<String>
                    });

                match session_id {
                    Some(sid) => sid,
                    None => {
                        // Cannot get existing session — create a fresh tab
                        log::info!("[BrowserSession] Creating fresh tab as fallback");
                        let create = self
                            .cdp
                            .send("Target.createTarget", json!({"url": "about:blank"}), None)
                            .await
                            .map_err(|e2| format!("Failed to create fallback tab: {}", e2))?;
                        let new_target_id = create["targetId"]
                            .as_str()
                            .ok_or("Missing targetId after createTarget")?
                            .to_string();
                        let new_attach = self
                            .cdp
                            .send(
                                "Target.attachToTarget",
                                json!({"targetId": new_target_id, "flatten": true}),
                                None,
                            )
                            .await
                            .map_err(|e2| format!("Failed to attach to fallback tab: {}", e2))?;
                        // Update target_id to the new tab
                        let new_sid = new_attach["sessionId"]
                            .as_str()
                            .ok_or("Missing sessionId for fallback tab")?
                            .to_string();
                        // We need to update target_id — reassign below via page_target_id
                        self.page_target_id = Some(new_target_id);
                        new_sid
                    }
                }
            }
        };

        // Enable Page domain
        self.cdp
            .send("Page.enable", json!({}), Some(&sid))
            .await
            .map_err(|e| format!("Failed to enable Page: {}", e))?;

        // Enable DOM domain
        self.cdp
            .send("DOM.enable", json!({}), Some(&sid))
            .await
            .map_err(|e| format!("Failed to enable DOM: {}", e))?;

        // Enable Accessibility domain (required for consistent AX tree results)
        self.cdp
            .send("Accessibility.enable", json!({}), Some(&sid))
            .await
            .map_err(|e| format!("Failed to enable Accessibility: {}", e))?;

        self.page_session_id = Some(sid.clone());
        // Only set target_id if not already set by the fallback tab creation path
        if self.page_target_id.is_none() {
            self.page_target_id = Some(target_id);
        }

        // Set up automatic dialog handlers (JS dialogs, downloads, HTTP auth)
        self.setup_dialog_handlers().await;

        Ok(sid)
    }

    /// Set up automatic handlers for native browser dialogs (JS dialogs, downloads, HTTP auth).
    /// Spawns background tasks that listen for CDP events and respond automatically.
    /// Must be called after a page session is attached (page_session_id is set).
    pub async fn setup_dialog_handlers(&mut self) {
        let sid = match self.page_session_id.as_deref() {
            Some(s) => s.to_string(),
            None => return,
        };

        // 1. JS Dialog auto-handling (alert/confirm/prompt/beforeunload)
        let _ = self.cdp.send("Page.enable", json!({}), Some(&sid)).await;
        let mut dialog_rx = self.cdp.subscribe("Page.javascriptDialogOpening").await;
        let (tx_dialog, alive_dialog) = self.cdp.raw_sender();
        let sid_for_dialog = sid.clone();
        let events_for_dialog = Arc::clone(&self.dialog_events);
        // We need a message counter for dialog commands
        let dialog_msg_counter = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(90000));

        let dialog_handle = tokio::spawn(async move {
            while let Some(event) = dialog_rx.recv().await {
                if !alive_dialog.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                let dialog_type = event["type"].as_str().unwrap_or("alert").to_string();
                let message = event["message"].as_str().unwrap_or("").to_string();
                let accept = true;

                log::info!(
                    "[DialogHandler] Auto-handling {} dialog: {}",
                    dialog_type,
                    if message.len() > 100 {
                        let mut end = 100;
                        while end > 0 && !message.is_char_boundary(end) {
                            end -= 1;
                        }
                        &message[..end]
                    } else {
                        &message
                    }
                );

                let mut params = json!({"accept": accept});
                if dialog_type == "prompt" {
                    params["promptText"] = json!("");
                }

                // Fire-and-forget: send the command without waiting for response
                let msg_id = dialog_msg_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let cmd = json!({
                    "id": msg_id,
                    "method": "Page.handleJavaScriptDialog",
                    "params": params,
                    "sessionId": sid_for_dialog,
                });
                let _ = tx_dialog.send(cmd.to_string());

                if let Ok(mut events) = events_for_dialog.lock() {
                    events.push(DialogEvent {
                        timestamp: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs_f64(),
                        dialog_type,
                        message,
                        accepted: accept,
                    });
                }
            }
        });
        self.dialog_handler_handles.push(dialog_handle);

        // 2. Download behavior — auto-save to temp dir without dialog
        let download_dir = format!("/tmp/cteno-downloads/{}", self.port);
        let _ = std::fs::create_dir_all(&download_dir);
        let _ = self
            .cdp
            .send(
                "Browser.setDownloadBehavior",
                json!({
                    "behavior": "allowAndName",
                    "downloadPath": download_dir,
                    "eventsEnabled": true,
                }),
                None, // Browser-level, not session-scoped
            )
            .await;

        // 3. HTTP Auth — cancel auth dialogs and record the challenge
        let _ = self
            .cdp
            .send(
                "Fetch.enable",
                json!({"handleAuthRequests": true}),
                Some(&sid),
            )
            .await;

        let mut auth_rx = self.cdp.subscribe("Fetch.authRequired").await;
        let (tx_auth, alive_auth) = self.cdp.raw_sender();
        let sid_for_auth = sid.clone();
        let events_for_auth = Arc::clone(&self.dialog_events);
        let auth_msg_counter = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(91000));

        let auth_handle = tokio::spawn(async move {
            while let Some(event) = auth_rx.recv().await {
                if !alive_auth.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                let request_id = event["requestId"].as_str().unwrap_or("").to_string();
                let challenge = &event["authChallenge"];
                let origin = challenge["origin"].as_str().unwrap_or("");
                let scheme = challenge["scheme"].as_str().unwrap_or("");
                let realm = challenge["realm"].as_str().unwrap_or("");
                let message = format!("{}:{} ({})", origin, realm, scheme);

                log::info!("[DialogHandler] HTTP auth challenge: {}", message);

                let msg_id = auth_msg_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let cmd = json!({
                    "id": msg_id,
                    "method": "Fetch.continueWithAuth",
                    "params": {
                        "requestId": request_id,
                        "authChallengeResponse": {"response": "CancelAuth"}
                    },
                    "sessionId": sid_for_auth,
                });
                let _ = tx_auth.send(cmd.to_string());

                if let Ok(mut events) = events_for_auth.lock() {
                    events.push(DialogEvent {
                        timestamp: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs_f64(),
                        dialog_type: "auth".to_string(),
                        message,
                        accepted: false,
                    });
                }
            }
        });
        self.dialog_handler_handles.push(auth_handle);

        log::info!(
            "[BrowserSession] Dialog handlers set up for session {}",
            sid
        );
    }

    /// Send a session-scoped CDP command. On timeout, reload the page and retry once.
    /// This handles the common case where a heavy page (e.g. SPA) blocks Chrome's
    /// renderer main thread, causing Runtime.evaluate / AX tree commands to hang.
    pub async fn send_or_reload(
        &self,
        method: &str,
        params: Value,
    ) -> Result<Value, super::cdp::CdpError> {
        let sid = self
            .page_session_id
            .as_deref()
            .ok_or_else(|| super::cdp::CdpError {
                message: "No page session. Call browser_navigate first.".to_string(),
                code: None,
            })?;

        // First attempt with 10s timeout
        match self
            .cdp
            .send_with_timeout(method, params.clone(), Some(sid), 10)
            .await
        {
            Ok(v) => return Ok(v),
            Err(ref e) if e.message.contains("timed out") => {
                log::warn!(
                    "[BrowserSession] {} timed out, reloading page and retrying",
                    method
                );
            }
            Err(e) => return Err(e),
        }

        // Subscribe to load event BEFORE reloading (avoid race condition)
        let mut load_rx = self.cdp.subscribe("Page.loadEventFired").await;

        // Page.reload is handled by Chrome's browser process — works even when
        // the renderer main thread is stuck.
        let _ = self
            .cdp
            .send_with_timeout("Page.reload", json!({}), Some(sid), 5)
            .await;

        // Wait for the page to finish reloading
        match tokio::time::timeout(tokio::time::Duration::from_secs(8), load_rx.recv()).await {
            Ok(Some(_)) => {
                log::info!("[BrowserSession] Page reloaded, waiting for render");
                tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
            }
            _ => {
                log::warn!("[BrowserSession] Reload load event timeout, retrying anyway");
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            }
        }

        // Retry with 15s timeout
        self.cdp
            .send_with_timeout(method, params, Some(sid), 15)
            .await
    }

    /// Get the current page URL and title.
    pub async fn get_page_info(&self) -> Result<(String, String), String> {
        let result = self
            .send_or_reload(
                "Runtime.evaluate",
                json!({
                    "expression": "JSON.stringify({url: location.href, title: document.title})",
                    "returnByValue": true,
                }),
            )
            .await
            .map_err(|e| format!("Failed to get page info: {}", e))?;

        let val_str = result["result"]["value"].as_str().unwrap_or("{}");

        let info: Value = serde_json::from_str(val_str).unwrap_or(json!({}));
        Ok((
            info["url"].as_str().unwrap_or("").to_string(),
            info["title"].as_str().unwrap_or("").to_string(),
        ))
    }

    /// Get the full AX tree from CDP.
    ///
    /// If the main frame has very few nodes (e.g. PDF viewer, iframe-heavy pages),
    /// also fetches AX trees from child frames and OOPIF targets to provide
    /// complete page content.
    pub async fn get_ax_tree(&self) -> Result<Vec<Value>, String> {
        let result = self
            .send_or_reload("Accessibility.getFullAXTree", json!({"depth": -1}))
            .await
            .map_err(|e| format!("Failed to get AX tree: {}", e))?;

        let sid = self.page_session_id.as_deref().ok_or("No page session")?;

        let mut nodes = result["nodes"].as_array().cloned().unwrap_or_default();

        // If the main frame has very few nodes, the real content is likely inside
        // child frames (e.g. Chrome's built-in PDF viewer, cross-origin iframes).
        // Try to fetch their AX trees and merge.
        if nodes.len() <= 10 {
            // Approach 1: Use Page.getFrameTree to find child frames and query by frameId
            if let Ok(frame_tree) = self
                .cdp
                .send("Page.getFrameTree", json!({}), Some(sid))
                .await
            {
                let child_frame_ids = collect_child_frame_ids(&frame_tree["frameTree"]);
                for frame_id in &child_frame_ids {
                    if let Ok(iframe_result) = self
                        .cdp
                        .send(
                            "Accessibility.getFullAXTree",
                            json!({"depth": -1, "frameId": frame_id}),
                            Some(sid),
                        )
                        .await
                    {
                        if let Some(iframe_nodes) = iframe_result["nodes"].as_array() {
                            if iframe_nodes.len() > 1 {
                                log::info!(
                                    "[Browser] Merged {} AX nodes from child frame {}",
                                    iframe_nodes.len(),
                                    frame_id
                                );
                                nodes.extend(iframe_nodes.iter().cloned());
                            }
                        }
                    }
                }
            }

            // Approach 2: If still few nodes, try OOPIF targets (e.g. Chrome PDF viewer plugin).
            // OOPIFs create separate CDP targets that need their own session.
            if nodes.len() <= 10 {
                if let Ok(targets) = self.cdp.send("Target.getTargets", json!({}), None).await {
                    if let Some(infos) = targets["targetInfos"].as_array() {
                        for target in infos {
                            let t_type = target["type"].as_str().unwrap_or("");
                            // OOPIF targets show up as "iframe" or "other" (PDF plugin)
                            if !matches!(t_type, "iframe" | "other") {
                                continue;
                            }
                            let target_id = match target["targetId"].as_str() {
                                Some(id) => id,
                                None => continue,
                            };

                            // Attach to the OOPIF target
                            let attach_result = self
                                .cdp
                                .send(
                                    "Target.attachToTarget",
                                    json!({"targetId": target_id, "flatten": true}),
                                    None,
                                )
                                .await;

                            let oopif_sid = match attach_result {
                                Ok(r) => match r["sessionId"].as_str() {
                                    Some(s) => s.to_string(),
                                    None => continue,
                                },
                                Err(_) => continue, // already attached or not attachable
                            };

                            // Get AX tree from the OOPIF
                            if let Ok(oopif_result) = self
                                .cdp
                                .send(
                                    "Accessibility.getFullAXTree",
                                    json!({"depth": -1}),
                                    Some(&oopif_sid),
                                )
                                .await
                            {
                                if let Some(oopif_nodes) = oopif_result["nodes"].as_array() {
                                    if oopif_nodes.len() > 1 {
                                        log::info!(
                                            "[Browser] Merged {} AX nodes from OOPIF target {} ({})",
                                            oopif_nodes.len(), target_id, t_type
                                        );
                                        nodes.extend(oopif_nodes.iter().cloned());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(nodes)
    }

    /// Build XPath map from the current page's DOM and store it in this session.
    pub async fn refresh_xpath_map(&mut self) -> Result<(), String> {
        let sid = self.page_session_id.as_deref();
        match super::xpath::build_xpath_map(&self.cdp, sid).await {
            Ok(m) => {
                self.xpath_map = m;
                Ok(())
            }
            Err(e) => {
                log::warn!("[BrowserSession] Failed to build XPath map: {}", e);
                // Non-fatal: keep old map (or empty), don't block the caller.
                Ok(())
            }
        }
    }

    /// Wait for DOM to stabilize (poll AX tree until no changes for 500ms, max 3s).
    pub async fn wait_for_dom_stable(&self) -> Result<(), String> {
        let mut last_count = 0usize;
        let mut stable_since = tokio::time::Instant::now();
        let deadline = stable_since + tokio::time::Duration::from_secs(3);

        loop {
            tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

            let tree = self.get_ax_tree().await.unwrap_or_default();
            let count = tree.len();

            if count != last_count {
                last_count = count;
                stable_since = tokio::time::Instant::now();
            }

            if tokio::time::Instant::now() - stable_since >= tokio::time::Duration::from_millis(500)
            {
                break;
            }

            if tokio::time::Instant::now() >= deadline {
                break;
            }
        }

        Ok(())
    }
}

/// Manages browser sessions, one per Agent session.
pub struct BrowserManager {
    pub(crate) sessions: Mutex<HashMap<String, BrowserSession>>,
    next_port: Mutex<u16>,
}

impl BrowserManager {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            next_port: Mutex::new(9222),
        }
    }

    /// Get or create a browser session for the given Agent session ID.
    ///
    /// Each session gets its own isolated browser to allow parallel usage:
    /// 1. Reuse existing alive session for this session_id
    /// 2. If no other session has a browser yet, attach to user's running Chrome (login state)
    /// 3. Otherwise launch a new Chrome with a copied profile (preserves login state)
    pub async fn get_or_create(&self, session_id: &str, headless: bool) -> Result<(), String> {
        // Preserved state from dead sessions (trace recorder, network capture, dialog events)
        let mut preserved_network: Option<super::network::NetworkCapture> = None;
        let mut preserved_dialog_events: Option<Arc<std::sync::Mutex<Vec<DialogEvent>>>> = None;

        // 1. Reuse existing alive session / remove dead session under lock.
        let any_other_alive = {
            let mut sessions = self.sessions.lock().await;
            if sessions.contains_key(session_id) {
                if let Some(session) = sessions.get(session_id) {
                    if session.cdp.is_alive() {
                        return Ok(());
                    }
                    log::warn!(
                        "[BrowserManager] Session {} has dead CDP connection, recreating",
                        session_id
                    );
                }
                // Remove dead session, but preserve trace & network & dialog state
                preserved_network = sessions
                    .get_mut(session_id)
                    .and_then(|s| s.network_capture.take());
                preserved_dialog_events = sessions
                    .get(session_id)
                    .map(|s| Arc::clone(&s.dialog_events));
                if let Some(old) = sessions.remove(session_id) {
                    Self::cleanup_session(old);
                }
            }
            sessions.values().any(|s| s.cdp.is_alive())
        };

        // 2. Try to attach to user's running Chrome — but only if no other session
        //    already owns a browser. This prevents multiple sessions from sharing
        //    the same Chrome process and interfering with each other.
        if !any_other_alive {
            // No other session has a browser, safe to attach to user's Chrome
            if let Some(mut session) = Self::try_attach_existing().await {
                let port = session.port;
                // Restore preserved state from dead session
                session.network_capture = preserved_network.take();
                if let Some(events) = preserved_dialog_events.take() {
                    session.dialog_events = events;
                }
                let mut sessions = self.sessions.lock().await;
                if sessions
                    .get(session_id)
                    .map(|s| s.cdp.is_alive())
                    .unwrap_or(false)
                {
                    // Another concurrent creator won the race.
                    drop(sessions);
                    Self::cleanup_session(session);
                    return Ok(());
                }
                sessions.insert(session_id.to_string(), session);
                log::info!(
                    "[BrowserManager] Attached to existing Chrome on port {} for session {}",
                    port,
                    session_id
                );
                return Ok(());
            }
        } else {
            log::info!(
                "[BrowserManager] Other sessions have browsers, launching isolated Chrome for session {}",
                session_id
            );
        }

        // 3. Launch a new Chrome process with copied profile (each session gets its own)
        let mut session = Self::launch_new_chrome(session_id, headless, &self.next_port).await?;
        let port = session.port;
        // Restore preserved state from dead session
        session.network_capture = preserved_network.take();
        if let Some(events) = preserved_dialog_events.take() {
            session.dialog_events = events;
        }
        let mut sessions = self.sessions.lock().await;
        if sessions
            .get(session_id)
            .map(|s| s.cdp.is_alive())
            .unwrap_or(false)
        {
            // Another concurrent creator won the race.
            drop(sessions);
            Self::cleanup_session(session);
            return Ok(());
        }
        sessions.insert(session_id.to_string(), session);
        log::info!(
            "[BrowserManager] Launched new Chrome on port {} for session {}",
            port,
            session_id
        );

        Ok(())
    }

    /// Try to connect to an already-running Chrome with CDP on common ports.
    async fn try_attach_existing() -> Option<BrowserSession> {
        for port in 9222..=9230 {
            // Quick probe: can we reach /json/version?
            let url = format!("http://127.0.0.1:{}/json/version", port);
            let resp = match tokio::time::timeout(
                tokio::time::Duration::from_millis(500),
                reqwest::get(&url),
            )
            .await
            {
                Ok(Ok(r)) if r.status().is_success() => r,
                _ => continue,
            };

            // Parse response to verify it's a valid CDP endpoint
            let version: Value = match resp.json().await {
                Ok(v) => v,
                _ => continue,
            };

            if version.get("webSocketDebuggerUrl").is_none() {
                continue;
            }

            log::info!(
                "[BrowserManager] Found existing Chrome CDP on port {}",
                port
            );

            // Connect CDP
            let cdp = match CdpConnection::connect(port).await {
                Ok(c) => c,
                Err(e) => {
                    log::warn!(
                        "[BrowserManager] CDP connect to port {} failed: {}",
                        port,
                        e
                    );
                    continue;
                }
            };

            // Verify there's at least one page target — skip if none
            let targets = cdp.send("Target.getTargets", json!({}), None).await.ok();
            let has_page = targets
                .as_ref()
                .and_then(|t| t["targetInfos"].as_array())
                .map(|arr| arr.iter().any(|t| t["type"].as_str() == Some("page")))
                .unwrap_or(false);
            if !has_page {
                log::info!(
                    "[BrowserManager] Chrome on port {} has no page targets, skipping",
                    port
                );
                continue;
            }

            return Some(BrowserSession {
                chrome_process: None, // we didn't launch it
                cdp,
                tmp_profile_dir: None,
                page_session_id: None,
                page_target_id: None,
                port,
                ax_index_map: Vec::new(),
                ax_backend_node_map: Vec::new(),
                last_ax_snapshot: Vec::new(),
                xpath_map: HashMap::new(),
                network_capture: None,
                dialog_events: Arc::new(std::sync::Mutex::new(Vec::new())),
                dialog_handler_handles: Vec::new(),
            });
        }

        None
    }

    /// Launch a new Chrome process with profile copy.
    async fn launch_new_chrome(
        session_id: &str,
        headless: bool,
        next_port: &Mutex<u16>,
    ) -> Result<BrowserSession, String> {
        // Allocate port
        let port = {
            let mut p = next_port.lock().await;
            let port = *p;
            *p += 1;
            if *p > 9300 {
                *p = 9222;
            }
            port
        };

        // Find the browser first so we can detect the correct profile dir
        let browser_exe = super::chrome::find_chrome()?;

        let mut actual_port = port;
        let mut chrome_process = None;
        let mut profile_dir = None;

        for attempt in 0..5 {
            let tmp_dir = super::chrome::copy_profile(session_id, Some(&browser_exe))?;

            match super::chrome::launch_chrome(&tmp_dir, actual_port, headless) {
                Ok((child, _)) => {
                    chrome_process = Some(child);
                    profile_dir = Some(tmp_dir);
                    break;
                }
                Err(e) => {
                    log::warn!(
                        "[BrowserManager] Chrome launch failed on port {} (attempt {}): {}",
                        actual_port,
                        attempt + 1,
                        e
                    );
                    super::chrome::cleanup_profile(&tmp_dir);
                    actual_port += 1;
                }
            }
        }

        let chrome_process = chrome_process.ok_or("Failed to launch Chrome after 5 attempts")?;
        let profile_dir = profile_dir.ok_or("Failed to create profile")?;

        // Wait for CDP to be ready
        super::chrome::wait_for_cdp(actual_port, 15).await?;

        // Connect CDP
        let cdp = CdpConnection::connect(actual_port).await?;

        Ok(BrowserSession {
            chrome_process: Some(chrome_process),
            cdp,
            tmp_profile_dir: Some(profile_dir),
            page_session_id: None,
            page_target_id: None,
            port: actual_port,
            ax_index_map: Vec::new(),
            ax_backend_node_map: Vec::new(),
            last_ax_snapshot: Vec::new(),
            xpath_map: HashMap::new(),
            network_capture: None,
            dialog_events: Arc::new(std::sync::Mutex::new(Vec::new())),
            dialog_handler_handles: Vec::new(),
        })
    }

    /// Clean up a session (kill process if we launched it, remove profile).
    fn cleanup_session(mut session: BrowserSession) {
        // Abort background dialog handler tasks
        for handle in session.dialog_handler_handles.drain(..) {
            handle.abort();
        }
        if let Some(ref mut child) = session.chrome_process {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(ref dir) = session.tmp_profile_dir {
            super::chrome::cleanup_profile(dir);
        }
    }

    /// Get mutable access to a browser session.
    /// The caller must hold the returned MutexGuard briefly.
    pub async fn with_session<F, R>(&self, session_id: &str, f: F) -> Result<R, String>
    where
        F: FnOnce(
            &mut BrowserSession,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<R, String>> + Send + '_>,
        >,
    {
        let mut session = {
            let mut sessions = self.sessions.lock().await;
            sessions.remove(session_id).ok_or_else(|| {
                "No browser session found. Call browser_navigate first to open a page.".to_string()
            })?
        };

        let result = if !session.cdp.is_alive() {
            Err("Browser connection lost. Call browser_navigate to relaunch.".to_string())
        } else {
            f(&mut session).await
        };

        {
            let mut sessions = self.sessions.lock().await;
            sessions.insert(session_id.to_string(), session);
        }

        result
    }

    /// Check if a session exists and is alive.
    pub async fn has_session(&self, session_id: &str) -> bool {
        let sessions = self.sessions.lock().await;
        sessions
            .get(session_id)
            .map(|s| s.cdp.is_alive())
            .unwrap_or(false)
    }

    /// Ensure a browser session exists for this session_id.
    /// Unlike get_or_create, this only attaches to an already-running Chrome
    /// (won't launch a new one). Returns true if a session is now available.
    /// Won't attach if another session already owns a browser (isolation).
    pub async fn ensure_session(&self, session_id: &str) -> bool {
        let sessions = self.sessions.lock().await;
        if sessions.contains_key(session_id) {
            return true;
        }
        // Don't attach to an existing Chrome if another session already has one
        let any_other_alive = sessions.values().any(|s| s.cdp.is_alive());
        drop(sessions);

        if any_other_alive {
            return false;
        }

        // Try to attach to existing Chrome (non-headless since it's already running)
        if let Some(session) = Self::try_attach_existing().await {
            let port = session.port;
            let mut sessions = self.sessions.lock().await;
            sessions.insert(session_id.to_string(), session);
            log::info!(
                "[BrowserManager] Auto-attached to existing Chrome on port {} for session {}",
                port,
                session_id
            );
            return true;
        }

        false
    }

    /// Close and clean up a specific session.
    /// Uses graceful CDP Browser.close first, falls back to kill.
    pub async fn close_session(&self, session_id: &str) {
        let session_opt = {
            let mut sessions = self.sessions.lock().await;
            sessions.remove(session_id)
        };
        if let Some(mut session) = session_opt {
            // Try graceful close via CDP first (avoids macOS "unexpected quit" dialog)
            if session.cdp.is_alive() {
                let _ = session.cdp.send("Browser.close", json!({}), None).await;
                // Give Chrome a moment to shut down gracefully
                if let Some(ref mut child) = session.chrome_process {
                    for _ in 0..10 {
                        match child.try_wait() {
                            Ok(Some(_)) => break, // exited
                            _ => tokio::time::sleep(tokio::time::Duration::from_millis(200)).await,
                        }
                    }
                }
            }
            session.cdp.close().await;
            Self::cleanup_session(session);
            log::info!("[BrowserManager] Closed browser session {}", session_id);
        }
    }

    /// Get direct mutable access to sessions map.
    /// Used by tool executors that need to mutate session state.
    pub async fn sessions_lock(
        &self,
    ) -> tokio::sync::MutexGuard<'_, HashMap<String, BrowserSession>> {
        self.sessions.lock().await
    }

    /// Close all browser sessions.
    pub async fn close_all(&self) {
        let drained = {
            let mut sessions = self.sessions.lock().await;
            sessions.drain().collect::<Vec<_>>()
        };
        for (id, mut session) in drained {
            // Graceful close via CDP
            if session.cdp.is_alive() {
                let _ = session.cdp.send("Browser.close", json!({}), None).await;
                if let Some(ref mut child) = session.chrome_process {
                    for _ in 0..10 {
                        match child.try_wait() {
                            Ok(Some(_)) => break,
                            _ => tokio::time::sleep(tokio::time::Duration::from_millis(200)).await,
                        }
                    }
                }
            }
            session.cdp.close().await;
            Self::cleanup_session(session);
            log::info!("[BrowserManager] Closed browser session {}", id);
        }
    }
}

/// Check if a CDP target is a "real" page (not chrome://, about:, devtools://).
fn is_real_page(target: &Value) -> bool {
    let url = target["url"].as_str().unwrap_or("");
    !url.is_empty()
        && !url.starts_with("chrome://")
        && !url.starts_with("chrome-extension://")
        && !url.starts_with("about:")
        && !url.starts_with("devtools://")
}

/// Recursively collect child frame IDs from a CDP Page.getFrameTree result.
fn collect_child_frame_ids(frame_tree: &Value) -> Vec<String> {
    let mut ids = Vec::new();
    if let Some(children) = frame_tree["childFrames"].as_array() {
        for child in children {
            if let Some(frame_id) = child["frame"]["id"].as_str() {
                ids.push(frame_id.to_string());
            }
            // Recurse into nested frames
            ids.extend(collect_child_frame_ids(child));
        }
    }
    ids
}
