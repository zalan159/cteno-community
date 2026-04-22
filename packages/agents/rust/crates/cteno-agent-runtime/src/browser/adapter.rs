use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

/// The bb-sites GitHub repo for remote adapter discovery.
const BB_SITES_REPO: &str = "epiral/bb-sites";
const BB_SITES_API: &str = "https://api.github.com/repos/epiral/bb-sites/contents";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteAdapter {
    pub name: String,
    pub domain: String,
    pub description: String,
    #[serde(default)]
    pub args: Vec<AdapterArg>,
    /// JavaScript function body to execute in the browser context.
    /// The function receives an args object: `async function(args) { ... }`
    pub script: String,
    #[serde(default)]
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterArg {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub required: bool,
    pub default: Option<String>,
}

impl SiteAdapter {
    /// Load all adapters from a directory (*.json files).
    pub fn load_all(dir: &Path) -> Vec<SiteAdapter> {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return Vec::new(),
        };

        let mut adapters = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            match std::fs::read_to_string(&path) {
                Ok(content) => match serde_json::from_str::<SiteAdapter>(&content) {
                    Ok(adapter) => adapters.push(adapter),
                    Err(e) => {
                        log::warn!("[SiteAdapter] Failed to parse {:?}: {}", path, e);
                    }
                },
                Err(e) => {
                    log::warn!("[SiteAdapter] Failed to read {:?}: {}", path, e);
                }
            }
        }
        adapters.sort_by(|a, b| a.name.cmp(&b.name));
        adapters
    }

    /// Load a single adapter by name.
    pub fn load(dir: &Path, name: &str) -> Option<SiteAdapter> {
        let filename = name.replace('/', "_");
        let path = dir.join(format!("{}.json", filename));
        let content = std::fs::read_to_string(&path).ok()?;
        serde_json::from_str(&content).ok()
    }

    /// Save an adapter to disk.
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let filename = self.name.replace('/', "_");
        let path = dir.join(format!("{}.json", filename));
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| e.to_string())
    }

    /// Delete an adapter.
    pub fn delete(dir: &Path, name: &str) -> Result<(), String> {
        let filename = name.replace('/', "_");
        let path = dir.join(format!("{}.json", filename));
        if !path.exists() {
            return Err(format!("Adapter '{}' not found", name));
        }
        std::fs::remove_file(path).map_err(|e| e.to_string())
    }

    /// Build the JS execution script with args substituted.
    /// Wraps the script in an isolated context with a clean fetch reference
    /// to avoid conflicts with sites that patch window.fetch (e.g. GitHub).
    pub fn build_script(&self, args: &Value) -> String {
        format!(
            r#"(async function(args) {{
    // Use a clean fetch from a fresh iframe to bypass page-level fetch patches
    const __iframe = document.createElement('iframe');
    __iframe.style.display = 'none';
    document.body.appendChild(__iframe);
    const fetch = __iframe.contentWindow.fetch.bind(__iframe.contentWindow);
    try {{
{script}
    }} finally {{
        document.body.removeChild(__iframe);
    }}
}})({args})"#,
            script = self.script,
            args = args.to_string()
        )
    }

    /// Parse a bb-sites JS file into a SiteAdapter.
    /// Format: `/* @meta { JSON } */\n\nasync function(args) { ... }`
    pub fn from_bb_sites_js(content: &str) -> Result<SiteAdapter, String> {
        // Extract @meta JSON block
        let meta_start = content.find("/* @meta").ok_or("No @meta block found")?;
        let json_start = content[meta_start..].find('{').ok_or("No JSON in @meta")? + meta_start;
        let meta_end = content.find("*/").ok_or("Unclosed @meta block")?;
        let meta_json = &content[json_start..meta_end].trim();

        let meta: Value = serde_json::from_str(meta_json)
            .map_err(|e| format!("Failed to parse @meta JSON: {}", e))?;

        let name = meta["name"]
            .as_str()
            .ok_or("Missing name in @meta")?
            .to_string();
        let description = meta["description"].as_str().unwrap_or("").to_string();
        let domain = meta["domain"].as_str().unwrap_or("").to_string();
        let read_only = meta["readOnly"].as_bool().unwrap_or(false);

        // Parse args from @meta
        let mut args = Vec::new();
        if let Some(args_obj) = meta["args"].as_object() {
            for (key, val) in args_obj {
                args.push(AdapterArg {
                    name: key.clone(),
                    description: val["description"].as_str().unwrap_or("").to_string(),
                    required: val["required"].as_bool().unwrap_or(false),
                    default: val["default"].as_str().map(|s| s.to_string()),
                });
            }
        }

        // Extract function body (everything after the @meta comment)
        let func_start = content[meta_end + 2..]
            .find("async function")
            .ok_or("No async function found after @meta")?
            + meta_end
            + 2;
        // Find the opening brace of the function
        let body_start =
            content[func_start..].find('{').ok_or("No function body")? + func_start + 1;
        // Find the matching closing brace (last '}' in the file)
        let body_end = content.rfind('}').ok_or("No closing brace")?;
        let script = content[body_start..body_end].to_string();

        Ok(SiteAdapter {
            name,
            domain,
            description,
            args,
            script,
            read_only,
        })
    }

    /// Search remote bb-sites repository for adapters matching a query.
    /// Returns a list of (site_name, adapter_files) from the GitHub API.
    pub async fn search_remote(query: &str) -> Result<Vec<RemoteAdapterInfo>, String> {
        let client = reqwest::Client::new();

        // First, get the list of site directories
        let resp = client
            .get(BB_SITES_API)
            .header("User-Agent", "Cteno-Browser-Adapter")
            .header("Accept", "application/vnd.github.v3+json")
            .send()
            .await
            .map_err(|e| format!("Failed to fetch bb-sites directory: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("GitHub API returned {}", resp.status()));
        }

        let entries: Vec<Value> = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse GitHub response: {}", e))?;

        let query_lower = query.to_lowercase();
        let mut results = Vec::new();

        for entry in &entries {
            let name = entry["name"].as_str().unwrap_or("");
            let entry_type = entry["type"].as_str().unwrap_or("");
            if entry_type != "dir" || name.starts_with('.') {
                continue;
            }
            if !query_lower.is_empty() && !name.to_lowercase().contains(&query_lower) {
                continue;
            }

            // Fetch the site's directory to list available adapters
            let site_url = format!("{}/{}", BB_SITES_API, name);
            let site_resp = client
                .get(&site_url)
                .header("User-Agent", "Cteno-Browser-Adapter")
                .header("Accept", "application/vnd.github.v3+json")
                .send()
                .await;

            if let Ok(site_resp) = site_resp {
                if site_resp.status().is_success() {
                    if let Ok(files) = site_resp.json::<Vec<Value>>().await {
                        let adapters: Vec<String> = files
                            .iter()
                            .filter_map(|f| {
                                let fname = f["name"].as_str()?;
                                if fname.ends_with(".js") {
                                    Some(format!("{}/{}", name, fname.trim_end_matches(".js")))
                                } else {
                                    None
                                }
                            })
                            .collect();

                        if !adapters.is_empty() {
                            results.push(RemoteAdapterInfo {
                                site: name.to_string(),
                                adapters,
                            });
                        }
                    }
                }
            }
        }

        Ok(results)
    }

    /// Install a specific adapter from bb-sites by downloading and converting it.
    pub async fn install_from_remote(name: &str, target_dir: &Path) -> Result<SiteAdapter, String> {
        // name format: "bilibili/search" → site="bilibili", file="search.js"
        let parts: Vec<&str> = name.splitn(2, '/').collect();
        if parts.len() != 2 {
            return Err(format!(
                "Invalid adapter name '{}'. Use format: site/adapter (e.g. bilibili/search)",
                name
            ));
        }
        let (site, adapter_file) = (parts[0], parts[1]);

        let raw_url = format!(
            "https://raw.githubusercontent.com/{}/main/{}/{}.js",
            BB_SITES_REPO, site, adapter_file
        );

        let client = reqwest::Client::new();
        let resp = client
            .get(&raw_url)
            .header("User-Agent", "Cteno-Browser-Adapter")
            .send()
            .await
            .map_err(|e| format!("Failed to download adapter: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!(
                "Adapter '{}' not found in bb-sites (HTTP {}). Use action 'search' to find available adapters.",
                name, resp.status()
            ));
        }

        let js_content = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read adapter content: {}", e))?;

        let adapter = Self::from_bb_sites_js(&js_content)?;
        adapter.save(target_dir)?;

        Ok(adapter)
    }

    /// Copy default adapters to a target directory if it doesn't already contain adapters.
    pub fn install_defaults(defaults_dir: &Path, target_dir: &Path) {
        if !defaults_dir.exists() {
            return;
        }
        // Only install if target dir is empty or doesn't exist
        let target_has_adapters = target_dir.exists()
            && std::fs::read_dir(target_dir)
                .ok()
                .map(|mut d| {
                    d.any(|e| {
                        e.ok()
                            .map(|e| {
                                e.path().extension().and_then(|ext| ext.to_str()) == Some("json")
                            })
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false);

        if target_has_adapters {
            return;
        }

        if let Err(e) = std::fs::create_dir_all(target_dir) {
            log::warn!("[SiteAdapter] Failed to create adapters dir: {}", e);
            return;
        }

        let entries = match std::fs::read_dir(defaults_dir) {
            Ok(e) => e,
            Err(_) => return,
        };

        let mut count = 0;
        for entry in entries.flatten() {
            let src = entry.path();
            if src.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Some(filename) = src.file_name() {
                let dst = target_dir.join(filename);
                if let Err(e) = std::fs::copy(&src, &dst) {
                    log::warn!("[SiteAdapter] Failed to copy {:?}: {}", src, e);
                } else {
                    count += 1;
                }
            }
        }
        if count > 0 {
            log::info!("[SiteAdapter] Installed {} default adapters", count);
        }
    }
}

/// Info about a remote site's available adapters.
#[derive(Debug, Clone)]
pub struct RemoteAdapterInfo {
    pub site: String,
    pub adapters: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_bb_sites_js() {
        let js = r#"/* @meta
{
  "name": "bilibili/search",
  "description": "Search Bilibili videos",
  "domain": "www.bilibili.com",
  "args": {
    "keyword": {"required": true, "description": "Search keyword"},
    "page": {"required": false, "description": "Page number"}
  },
  "readOnly": true
}
*/

async function(args) {
  const resp = await fetch('https://api.bilibili.com/search?q=' + args.keyword);
  return await resp.json();
}"#;

        let adapter = SiteAdapter::from_bb_sites_js(js).unwrap();
        assert_eq!(adapter.name, "bilibili/search");
        assert_eq!(adapter.description, "Search Bilibili videos");
        assert_eq!(adapter.domain, "www.bilibili.com");
        assert!(adapter.read_only);
        assert_eq!(adapter.args.len(), 2);
        assert!(adapter
            .args
            .iter()
            .any(|a| a.name == "keyword" && a.required));
        assert!(adapter.args.iter().any(|a| a.name == "page" && !a.required));
        assert!(adapter.script.contains("fetch"));
        assert!(!adapter.script.contains("async function"));
    }

    #[tokio::test]
    async fn test_search_remote() {
        // Search for "bilibili" — should find it in bb-sites
        let results = SiteAdapter::search_remote("bilibili").await;
        match results {
            Ok(results) => {
                assert!(!results.is_empty(), "Should find bilibili in bb-sites");
                let bilibili = results.iter().find(|r| r.site == "bilibili");
                assert!(bilibili.is_some(), "Should have a bilibili site");
                let adapters = &bilibili.unwrap().adapters;
                assert!(
                    adapters.iter().any(|a| a.contains("search")),
                    "bilibili should have a search adapter"
                );
            }
            Err(e) => {
                // Network might be unavailable in CI
                eprintln!("Skipping remote test (network error): {}", e);
            }
        }
    }

    #[tokio::test]
    async fn test_install_from_remote() {
        let tmp = std::env::temp_dir().join("cteno-adapter-test");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        match SiteAdapter::install_from_remote("bilibili/search", &tmp).await {
            Ok(adapter) => {
                assert_eq!(adapter.name, "bilibili/search");
                assert!(!adapter.script.is_empty());
                // Verify file was saved
                assert!(tmp.join("bilibili_search.json").exists());
            }
            Err(e) => {
                eprintln!("Skipping install test (network error): {}", e);
            }
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
