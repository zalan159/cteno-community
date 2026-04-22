//! Browser Adapter Tool Executor
//!
//! Run or manage site-specific browser automation adapters.
//! Adapters are pre-built JS scripts that extract structured data from websites
//! using the browser's authenticated session.

use crate::browser::adapter::SiteAdapter;
use crate::browser::BrowserManager;
use crate::tool::ToolExecutor;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

pub struct BrowserAdapterExecutor {
    browser_manager: Arc<BrowserManager>,
    default_adapters_dir: PathBuf,
}

impl BrowserAdapterExecutor {
    pub fn new(browser_manager: Arc<BrowserManager>, default_adapters_dir: PathBuf) -> Self {
        Self {
            browser_manager,
            default_adapters_dir,
        }
    }

    /// Resolve the adapters directory from persona workdir.
    fn adapters_dir(input: &Value, default_adapters_dir: &Path) -> PathBuf {
        let workdir = input["__persona_workdir"]
            .as_str()
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                dirs::data_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("cteno")
            });
        let dir = workdir.join(".cteno").join("adapters");
        // Install defaults on first access
        SiteAdapter::install_defaults(default_adapters_dir, &dir);
        dir
    }
}

use std::path::Path;

#[async_trait]
impl ToolExecutor for BrowserAdapterExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        let action = input["action"]
            .as_str()
            .ok_or("Missing required parameter: action")?;

        let adapters_dir = Self::adapters_dir(&input, &self.default_adapters_dir);

        match action {
            "list" => execute_list(&adapters_dir),
            "show" => {
                let name = input["adapter_name"]
                    .as_str()
                    .ok_or("Missing required parameter: adapter_name")?;
                execute_show(&adapters_dir, name)
            }
            "run" => {
                let name = input["adapter_name"]
                    .as_str()
                    .ok_or("Missing required parameter: adapter_name")?;
                let args = input.get("args").cloned().unwrap_or(json!({}));
                let session_id = input["__session_id"].as_str().unwrap_or("default");
                execute_run(
                    &self.browser_manager,
                    &adapters_dir,
                    name,
                    &args,
                    session_id,
                )
                .await
            }
            "create" => {
                let adapter_json = input["adapter_json"]
                    .as_str()
                    .ok_or("Missing required parameter: adapter_json")?;
                execute_create(&adapters_dir, adapter_json)
            }
            "delete" => {
                let name = input["adapter_name"]
                    .as_str()
                    .ok_or("Missing required parameter: adapter_name")?;
                execute_delete(&adapters_dir, name)
            }
            "search" => {
                let query = input["adapter_name"].as_str().unwrap_or("");
                execute_search(query).await
            }
            "install" => {
                let name = input["adapter_name"]
                    .as_str()
                    .ok_or("Missing required parameter: adapter_name (e.g. 'bilibili/search')")?;
                execute_install(name, &adapters_dir).await
            }
            _ => Err(format!(
                "Unknown action: '{}'. Use: run, list, show, create, delete, search, install",
                action
            )),
        }
    }
}

fn execute_list(adapters_dir: &Path) -> Result<String, String> {
    let adapters = SiteAdapter::load_all(adapters_dir);
    if adapters.is_empty() {
        return Ok("No adapters installed. Use action 'create' to add one.".to_string());
    }

    let mut output = format!("{} adapters available:\n\n", adapters.len());
    output.push_str(&format!(
        "{:<25} {:<25} {}\n",
        "NAME", "DOMAIN", "DESCRIPTION"
    ));
    output.push_str(&format!(
        "{:<25} {:<25} {}\n",
        "----", "------", "-----------"
    ));

    for a in &adapters {
        output.push_str(&format!(
            "{:<25} {:<25} {}\n",
            a.name, a.domain, a.description
        ));
    }

    Ok(output)
}

fn execute_show(adapters_dir: &Path, name: &str) -> Result<String, String> {
    let adapter = SiteAdapter::load(adapters_dir, name).ok_or(format!(
        "Adapter '{}' not found. Use action 'list' to see available adapters.",
        name
    ))?;

    let mut output = format!("# {}\n\n", adapter.name);
    output.push_str(&format!("Domain: {}\n", adapter.domain));
    output.push_str(&format!("Description: {}\n", adapter.description));
    output.push_str(&format!("Read-only: {}\n", adapter.read_only));

    if !adapter.args.is_empty() {
        output.push_str("\nArguments:\n");
        for arg in &adapter.args {
            let req = if arg.required { " (required)" } else { "" };
            let def = arg
                .default
                .as_ref()
                .map(|d| format!(" [default: {}]", d))
                .unwrap_or_default();
            output.push_str(&format!(
                "  - {}{}{}: {}\n",
                arg.name, req, def, arg.description
            ));
        }
    }

    output.push_str(&format!(
        "\nScript ({} chars):\n```javascript\n{}\n```\n",
        adapter.script.len(),
        adapter.script
    ));

    Ok(output)
}

async fn execute_run(
    browser_manager: &BrowserManager,
    adapters_dir: &Path,
    name: &str,
    args: &Value,
    session_id: &str,
) -> Result<String, String> {
    let adapter = SiteAdapter::load(adapters_dir, name).ok_or(format!(
        "Adapter '{}' not found. Use action 'list' to see available adapters.",
        name
    ))?;

    // Validate required args
    for arg in &adapter.args {
        if arg.required && args.get(&arg.name).is_none() {
            return Err(format!(
                "Missing required argument '{}': {}",
                arg.name, arg.description
            ));
        }
    }

    // Build args with defaults filled in
    let mut full_args = args.clone();
    if let Some(obj) = full_args.as_object_mut() {
        for arg in &adapter.args {
            if !obj.contains_key(&arg.name) {
                if let Some(ref default) = arg.default {
                    obj.insert(arg.name.clone(), Value::String(default.clone()));
                }
            }
        }
    }

    // Auto-attach to existing Chrome if no session exists
    browser_manager.ensure_session(session_id).await;

    let mut session = {
        let mut sessions = browser_manager.sessions_lock().await;
        sessions
            .remove(session_id)
            .ok_or("No browser session found. Call browser_navigate first to open a page.")?
    };

    let run_result = async {
        if !session.cdp.is_alive() {
            return Err("Browser connection lost. Call browser_navigate to relaunch.".to_string());
        }

        // Find or create a tab matching the adapter's domain
        let sid = find_or_create_domain_tab(&mut session, &adapter.domain).await?;

        // Execute the adapter script
        let script = adapter.build_script(&full_args);
        let result = session
            .cdp
            .send_with_timeout(
                "Runtime.evaluate",
                json!({
                    "expression": script,
                    "awaitPromise": true,
                    "returnByValue": true,
                }),
                Some(&sid),
                30,
            )
            .await
            .map_err(|e| format!("Script execution failed: {}", e))?;

        // Check for exceptions
        if let Some(exception) = result.get("exceptionDetails") {
            let text = exception["exception"]["description"]
                .as_str()
                .or(exception["text"].as_str())
                .unwrap_or("Unknown error");
            return Err(format!("Adapter error: {}", text));
        }

        // Extract the return value
        let value = &result["result"]["value"];
        if value.is_null() {
            return Ok("Adapter returned no data.".to_string());
        }

        // Check for adapter-level error
        if let Some(error) = value.get("error") {
            let hint = value
                .get("hint")
                .and_then(|h| h.as_str())
                .map(|h| format!("\nHint: {}", h))
                .unwrap_or_default();
            return Err(format!(
                "{}{}",
                error.as_str().unwrap_or("Adapter error"),
                hint
            ));
        }

        // Format the result as pretty JSON
        let formatted = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
        Ok(format!(
            "Adapter '{}' result:\n\n{}",
            adapter.name, formatted
        ))
    }
    .await;

    {
        let mut sessions = browser_manager.sessions_lock().await;
        sessions.insert(session_id.to_string(), session);
    }

    run_result
}

/// Find a tab matching the domain or create one by navigating to it.
async fn find_or_create_domain_tab(
    session: &mut crate::browser::manager::BrowserSession,
    domain: &str,
) -> Result<String, String> {
    // First check if current page already matches
    if let Some(ref sid) = session.page_session_id {
        let info = session
            .cdp
            .send(
                "Runtime.evaluate",
                json!({
                    "expression": "location.hostname",
                    "returnByValue": true,
                }),
                Some(sid),
            )
            .await;
        if let Ok(info) = info {
            let hostname = info["result"]["value"].as_str().unwrap_or("");
            if hostname == domain || hostname.ends_with(&format!(".{}", domain)) {
                return Ok(sid.clone());
            }
        }
    }

    // Search all tabs for a matching domain
    let targets = session
        .cdp
        .send("Target.getTargets", json!({}), None)
        .await
        .map_err(|e| format!("Failed to get targets: {}", e))?;

    if let Some(infos) = targets["targetInfos"].as_array() {
        for target in infos {
            if target["type"].as_str() != Some("page") {
                continue;
            }
            let url = target["url"].as_str().unwrap_or("");
            if url.contains(domain) {
                let target_id = target["targetId"].as_str().unwrap_or("");
                if !target_id.is_empty() {
                    // Attach to this tab
                    let sid = session.attach_best_page_target(Some(url)).await?;
                    return Ok(sid);
                }
            }
        }
    }

    // No matching tab — create one
    let url = format!("https://{}", domain);
    let create_result = session
        .cdp
        .send("Target.createTarget", json!({"url": url}), None)
        .await
        .map_err(|e| format!("Failed to create tab for {}: {}", domain, e))?;

    let target_id = create_result["targetId"]
        .as_str()
        .ok_or("Missing targetId after createTarget")?;

    let attach_result = session
        .cdp
        .send(
            "Target.attachToTarget",
            json!({"targetId": target_id, "flatten": true}),
            None,
        )
        .await
        .map_err(|e| format!("Failed to attach to new tab: {}", e))?;

    let sid = attach_result["sessionId"]
        .as_str()
        .ok_or("Missing sessionId")?
        .to_string();

    // Enable required domains
    let _ = session.cdp.send("Page.enable", json!({}), Some(&sid)).await;
    let _ = session.cdp.send("DOM.enable", json!({}), Some(&sid)).await;

    // Wait for the page to load
    let mut load_rx = session.cdp.subscribe("Page.loadEventFired").await;
    match tokio::time::timeout(tokio::time::Duration::from_secs(10), load_rx.recv()).await {
        Ok(Some(_)) => {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }
        _ => {
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        }
    }

    session.page_session_id = Some(sid.clone());
    session.page_target_id = Some(target_id.to_string());

    Ok(sid)
}

fn execute_create(adapters_dir: &Path, adapter_json: &str) -> Result<String, String> {
    let adapter: SiteAdapter =
        serde_json::from_str(adapter_json).map_err(|e| format!("Invalid adapter JSON: {}", e))?;

    if adapter.name.is_empty() {
        return Err("Adapter name cannot be empty".to_string());
    }
    if adapter.domain.is_empty() {
        return Err("Adapter domain cannot be empty".to_string());
    }
    if adapter.script.is_empty() {
        return Err("Adapter script cannot be empty".to_string());
    }

    adapter.save(adapters_dir)?;
    Ok(format!(
        "Adapter '{}' saved successfully (domain: {}).",
        adapter.name, adapter.domain
    ))
}

fn execute_delete(adapters_dir: &Path, name: &str) -> Result<String, String> {
    SiteAdapter::delete(adapters_dir, name)?;
    Ok(format!("Adapter '{}' deleted.", name))
}

async fn execute_search(query: &str) -> Result<String, String> {
    let results = SiteAdapter::search_remote(query).await?;
    if results.is_empty() {
        return Ok(format!(
            "No adapters found matching '{}' in bb-sites. Try a broader search or leave empty to list all sites.",
            query
        ));
    }

    let mut output = format!(
        "Found {} sites in bb-sites (epiral/bb-sites):\n\n",
        results.len()
    );
    for info in &results {
        output.push_str(&format!(
            "  {} ({} adapters)\n",
            info.site,
            info.adapters.len()
        ));
        for adapter in &info.adapters {
            output.push_str(&format!("    - {}\n", adapter));
        }
    }
    output.push_str("\nUse action='install' adapter_name='site/adapter' to install one.");
    Ok(output)
}

async fn execute_install(name: &str, adapters_dir: &Path) -> Result<String, String> {
    // Check if already installed
    if SiteAdapter::load(adapters_dir, name).is_some() {
        return Ok(format!(
            "Adapter '{}' is already installed. Use action='show' to view it.",
            name
        ));
    }

    let adapter = SiteAdapter::install_from_remote(name, adapters_dir).await?;
    let args_desc = if adapter.args.is_empty() {
        "none".to_string()
    } else {
        adapter
            .args
            .iter()
            .map(|a| {
                let req = if a.required { " (required)" } else { "" };
                format!("{}{}", a.name, req)
            })
            .collect::<Vec<_>>()
            .join(", ")
    };

    Ok(format!(
        "Installed '{}' from bb-sites.\n  Domain: {}\n  Description: {}\n  Args: {}\n\nUse action='run' adapter_name='{}' to execute it.",
        adapter.name, adapter.domain, adapter.description, args_desc, adapter.name
    ))
}
