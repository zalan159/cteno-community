//! Fetch Tool Executor
//!
//! Fetches web pages, extracts main content, and optionally compresses using LLM.
//! Includes an in-process LRU cache (15-minute TTL, 50 entries) and cross-domain
//! redirect detection.

use crate::llm::{LLMClient, LLMResponseType, Message};
use crate::llm_profile::ProfileStore;
use crate::tool::ToolExecutor;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

// ---------------------------------------------------------------------------
// URL → response cache (global, shared across all FetchExecutor instances)
// ---------------------------------------------------------------------------

struct CacheEntry {
    content: String,
    inserted_at: Instant,
}

static FETCH_CACHE: LazyLock<Mutex<HashMap<String, CacheEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

const CACHE_TTL: Duration = Duration::from_secs(15 * 60); // 15 minutes
const MAX_CACHE_ENTRIES: usize = 50;

fn get_cached(url: &str) -> Option<String> {
    let mut cache = FETCH_CACHE.lock().unwrap();
    // Evict expired entries first
    cache.retain(|_, v| v.inserted_at.elapsed() < CACHE_TTL);
    cache.get(url).map(|e| e.content.clone())
}

fn set_cache(url: &str, content: &str) {
    let mut cache = FETCH_CACHE.lock().unwrap();
    // If at capacity, evict the oldest entry
    if cache.len() >= MAX_CACHE_ENTRIES && !cache.contains_key(url) {
        if let Some(oldest_key) = cache
            .iter()
            .min_by_key(|(_, v)| v.inserted_at)
            .map(|(k, _)| k.clone())
        {
            cache.remove(&oldest_key);
        }
    }
    cache.insert(
        url.to_string(),
        CacheEntry {
            content: content.to_string(),
            inserted_at: Instant::now(),
        },
    );
}

pub struct FetchExecutor {
    client: reqwest::Client,
    profile_store: Arc<RwLock<ProfileStore>>,
    global_api_key: Arc<RwLock<String>>,
}

impl FetchExecutor {
    pub fn new(
        profile_store: Arc<RwLock<ProfileStore>>,
        global_api_key: Arc<RwLock<String>>,
    ) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .user_agent("Mozilla/5.0 (compatible; CtenoBot/1.0)")
                .build()
                .expect("Failed to create HTTP client"),
            profile_store,
            global_api_key,
        }
    }

    async fn create_compress_client(&self, profile_id: &str) -> Result<LLMClient, String> {
        let store = self.profile_store.read().await;
        let profile = store
            .get_profile(profile_id)
            .unwrap_or_else(|| store.get_default());

        let global_key = self.global_api_key.read().await;
        let api_key = profile.resolve_compress_api_key(&global_key);

        Ok(LLMClient::with_base_url(
            api_key,
            profile.compress.base_url.clone(),
        ))
    }

    async fn compress_content(
        &self,
        client: &LLMClient,
        profile_id: &str,
        url: &str,
        prompt: &str,
        markdown: &str,
    ) -> Result<String, String> {
        let store = self.profile_store.read().await;
        let profile = store
            .get_profile(profile_id)
            .unwrap_or_else(|| store.get_default());
        let compress_cfg = profile.compress.clone();
        // Release the read lock before the async LLM call
        drop(store);

        let system = "你是网页内容提取助手。根据用户问题从网页中提取相关信息，保留重要细节。";
        let user_msg = format!(
            "网页 URL: {}\n\n用户问题: {}\n\n网页内容:\n{}",
            url, prompt, markdown
        );

        log::info!(
            "[Fetch] Compressing {} chars with model {}",
            markdown.len(),
            compress_cfg.model
        );

        let response = client
            .chat_anthropic(
                &compress_cfg.model,
                system,
                &[Message::user(user_msg)],
                &[],
                compress_cfg.temperature,
                compress_cfg.max_tokens,
                None,  // No streaming for fetch compression
                false, // No thinking for fetch compression
                None,
            )
            .await
            .map_err(|e| format!("Compression LLM call failed: {}", e))?;

        // Extract text from response
        for content in response.content {
            if let LLMResponseType::Text { text } = content {
                log::info!("[Fetch] Compressed to {} chars", text.len());
                return Ok(text);
            }
        }

        Err("No text in compression response".to_string())
    }
}

#[async_trait]
impl ToolExecutor for FetchExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        let url = input["url"].as_str().ok_or("Missing 'url' parameter")?;
        let prompt = input["prompt"]
            .as_str()
            .ok_or("Missing 'prompt' parameter")?;
        let raw = input.get("raw").and_then(|v| v.as_bool()).unwrap_or(false);
        let max_length = input
            .get("max_length")
            .and_then(|v| v.as_u64())
            .unwrap_or(10000) as usize;

        // Extract profile_id from injected context
        let profile_id = input
            .get("__profile_id")
            .and_then(|v| v.as_str())
            .ok_or("Missing __profile_id in tool input (internal error)")?;

        log::info!("[Fetch] Fetching URL: {} (profile: {})", url, profile_id);

        // ── Check cache ─────────────────────────────────────────────
        if let Some(cached) = get_cached(url) {
            log::info!("[Fetch] Cache hit for {} ({} chars)", url, cached.len());
            let markdown = format!("{}\n\n(cached)", cached);
            if raw || markdown.len() < max_length {
                return Ok(markdown);
            }
            let compress_client = self.create_compress_client(profile_id).await?;
            return self
                .compress_content(&compress_client, profile_id, url, prompt, &markdown)
                .await;
        }

        // ── Fetch HTML ──────────────────────────────────────────────
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("Failed to fetch URL: {}", e))?;

        if !response.status().is_success() {
            return Err(format!("HTTP error: {}", response.status()));
        }

        // ── Cross-domain redirect detection ─────────────────────────
        let final_url = response.url().clone();
        let original_host = url::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(|h| h.to_lowercase()));
        let final_host = final_url.host_str().map(|h| h.to_lowercase());

        if original_host.is_some() && final_host.is_some() && original_host != final_host {
            let msg = format!(
                "REDIRECT DETECTED: The URL redirects to a different host.\n\n\
                 Original URL: {}\n\
                 Redirect URL: {}\n\n\
                 To fetch content from the redirect target, make a new fetch request with:\n\
                 - url: \"{}\"\n\
                 - prompt: \"{}\"",
                url, final_url, final_url, prompt,
            );
            log::info!("[Fetch] Cross-domain redirect: {} -> {}", url, final_url);
            return Ok(msg);
        }

        let html = response
            .text()
            .await
            .map_err(|e| format!("Failed to read response: {}", e))?;

        // ── Extract readable content ────────────────────────────────
        let markdown = extract_readable_content(&html)?;

        // ── Populate cache ──────────────────────────────────────────
        set_cache(url, &markdown);

        // ── Return raw or compress ──────────────────────────────────
        if raw || markdown.len() < max_length {
            log::info!("[Fetch] Returning raw content ({} chars)", markdown.len());
            return Ok(markdown);
        }

        let compress_client = self.create_compress_client(profile_id).await?;
        self.compress_content(&compress_client, profile_id, url, prompt, &markdown)
            .await
    }
}

/// Extract readable content from HTML and convert to Markdown.
///
/// Runs in a dedicated thread with an 8 MB stack because the `readability`
/// crate can recurse very deeply on complex HTML, overflowing the default
/// tokio worker stack (which crashed the whole process — see crash report
/// cteno-2026-03-17-032658).
fn extract_readable_content(html: &str) -> Result<String, String> {
    let html = html.to_owned();
    let (tx, rx) = std::sync::mpsc::channel();

    std::thread::Builder::new()
        .name("readability-extract".into())
        .stack_size(8 * 1024 * 1024) // 8 MB stack
        .spawn(move || {
            let result = std::panic::catch_unwind(|| {
                use readability::extractor;
                let product = extractor::extract(
                    &mut html.as_bytes(),
                    &url::Url::parse("http://example.com").unwrap(),
                )
                .map_err(|e| format!("Failed to extract content: {}", e))?;
                Ok::<String, String>(html2md::parse_html(&product.content))
            });
            let _ = tx.send(result);
        })
        .map_err(|e| format!("Failed to spawn readability thread: {}", e))?;

    match rx.recv() {
        Ok(Ok(result)) => result,
        Ok(Err(panic_info)) => {
            log::error!("[Fetch] readability panicked: {:?}", panic_info);
            Err("Content extraction failed: page too complex to parse".to_string())
        }
        Err(_) => Err("Content extraction thread terminated unexpectedly".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_readable_content() {
        let html = r#"
            <html>
            <head><title>Test Page</title></head>
            <body>
                <nav>Navigation</nav>
                <article>
                    <h1>Main Title</h1>
                    <p>This is the main content of the article. It needs to be long enough for readability to consider it worth extracting.</p>
                    <p>Here is another paragraph with more details about the topic being discussed.</p>
                </article>
                <footer>Footer</footer>
            </body>
            </html>
        "#;

        let result = extract_readable_content(html);
        assert!(result.is_ok(), "extract_readable_content should succeed");
        let markdown = result.unwrap();
        assert!(
            !markdown.is_empty(),
            "extracted content should not be empty"
        );
    }
}
