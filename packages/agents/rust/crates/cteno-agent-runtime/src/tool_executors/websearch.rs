//! Web Search Tool Executor
//!
//! Proxies web search requests through Happy Server.
//! The server holds the Tavily API key — no local configuration needed.
//! Supports client-side domain filtering (allowed_domains / blocked_domains).
use crate::tool::ToolExecutor;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;
use url::Url;

fn happy_server_url() -> String {
    crate::hooks::resolved_happy_server_url()
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    answer: Option<String>,
    #[serde(default)]
    results: Vec<SearchResult>,
}

#[derive(Debug, Deserialize)]
struct SearchResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    content: String,
}

/// Web Search Executor — calls Happy Server /v1/search/web
pub struct WebSearchExecutor {
    client: reqwest::Client,
    app_data_dir: PathBuf,
}

impl WebSearchExecutor {
    pub fn new(app_data_dir: PathBuf) -> Self {
        Self {
            client: reqwest::Client::new(),
            app_data_dir,
        }
    }

    /// Read the cached auth token from machine_auth.json
    fn read_auth_token(&self) -> Result<String, String> {
        let path = self.app_data_dir.join("machine_auth.json");
        let raw = std::fs::read_to_string(&path).map_err(|e| {
            format!(
                "Web search requires Happy Server connection. Failed to read auth: {}",
                e
            )
        })?;
        let parsed: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| format!("Failed to parse machine auth cache: {}", e))?;
        parsed
            .get("token")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| "No auth token found in machine_auth.json".to_string())
    }

    async fn search(
        &self,
        auth_token: &str,
        query: &str,
        max_results: usize,
        allowed_domains: &[String],
        blocked_domains: &[String],
    ) -> Result<String, String> {
        let server_url = happy_server_url();
        let url = format!("{}/v1/search/web", server_url.trim_end_matches('/'));

        // Request extra results when filtering so we still have enough after
        // client-side domain filtering.
        let has_filter = !allowed_domains.is_empty() || !blocked_domains.is_empty();
        let fetch_count = if has_filter {
            max_results * 2
        } else {
            max_results
        };

        let response = self
            .client
            .post(&url)
            .bearer_auth(auth_token)
            .json(&json!({
                "query": query,
                "max_results": fetch_count
            }))
            .timeout(std::time::Duration::from_secs(20))
            .send()
            .await
            .map_err(|e| format!("Failed to reach search service: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unable to read error response".to_string());
            return Err(format!(
                "Search service returned error {}: {}",
                status, body
            ));
        }

        let result: SearchResponse = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse search results: {}", e))?;

        // ── Client-side domain filtering ────────────────────────────
        let filtered: Vec<&SearchResult> = result
            .results
            .iter()
            .filter(|item| {
                let host = Url::parse(item.url.trim())
                    .ok()
                    .and_then(|u| u.host_str().map(|h| h.to_lowercase()));
                let host = match host {
                    Some(h) => h,
                    None => return true, // keep results with unparseable URLs
                };

                // allowed_domains takes priority when both are specified
                if !allowed_domains.is_empty() {
                    return allowed_domains.iter().any(|d| {
                        host == d.to_lowercase()
                            || host.ends_with(&format!(".{}", d.to_lowercase()))
                    });
                }
                if !blocked_domains.is_empty() {
                    return !blocked_domains.iter().any(|d| {
                        host == d.to_lowercase()
                            || host.ends_with(&format!(".{}", d.to_lowercase()))
                    });
                }
                true
            })
            .take(max_results)
            .collect();

        let mut output = String::new();
        if let Some(ref answer) = result.answer {
            if !answer.trim().is_empty() {
                output.push_str("## Summary\n\n");
                output.push_str(answer.trim());
                output.push_str("\n\n");
            }
        }

        if !filtered.is_empty() {
            output.push_str("### Results\n\n");
            for (i, item) in filtered.iter().enumerate() {
                output.push_str(&format!(
                    "{}. {}\n   URL: {}\n",
                    i + 1,
                    item.title.trim(),
                    item.url.trim()
                ));
                if !item.content.trim().is_empty() {
                    let snippet = truncate_chars(item.content.trim(), 280);
                    output.push_str(&format!("   Snippet: {}\n", snippet));
                }
                output.push('\n');
            }
        }

        if output.is_empty() {
            output = format!(
                "No results found for '{}'. Try rephrasing your query or being more specific.",
                query
            );
        }

        // Append source citation reminder
        output.push_str("\nREMINDER: You MUST include the sources above in your response to the user using markdown hyperlinks: [Title](URL)");

        Ok(output)
    }
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (count, ch) in input.chars().enumerate() {
        if count >= max_chars {
            out.push_str("...");
            break;
        }
        out.push(ch);
    }
    out
}

#[async_trait]
impl ToolExecutor for WebSearchExecutor {
    async fn execute(&self, input: serde_json::Value) -> Result<String, String> {
        let query = input
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: query")?;

        let max_results = input
            .get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(5) as usize;

        if query.trim().is_empty() {
            return Err("Query cannot be empty".to_string());
        }

        if max_results > 10 {
            return Err("max_results cannot exceed 10".to_string());
        }

        // Parse optional domain filters
        let allowed_domains: Vec<String> = input
            .get("allowed_domains")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let blocked_domains: Vec<String> = input
            .get("blocked_domains")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        // Validate: cannot specify both (allowed_domains takes priority, but warn)
        if !allowed_domains.is_empty() && !blocked_domains.is_empty() {
            log::warn!(
                "[WebSearch] Both allowed_domains and blocked_domains specified; \
                 allowed_domains takes priority"
            );
        }

        log::info!(
            "[WebSearch] Searching for: {}{}{}",
            query,
            if !allowed_domains.is_empty() {
                format!(" (allowed: {:?})", allowed_domains)
            } else {
                String::new()
            },
            if !blocked_domains.is_empty() {
                format!(" (blocked: {:?})", blocked_domains)
            } else {
                String::new()
            },
        );

        let auth_token = self.read_auth_token()?;
        self.search(
            &auth_token,
            query,
            max_results,
            &allowed_domains,
            &blocked_domains,
        )
        .await
    }
}
