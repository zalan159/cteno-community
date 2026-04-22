//! LLM Profile System
//!
//! Manages per-session LLM configurations stored locally on the Machine.
//! Each profile contains a chat endpoint (main model) and a compress endpoint (summary model).

use serde::{Deserialize, Serialize};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Default proxy profile used when no profile is explicitly specified.
/// All sessions default to proxy mode so usage goes through Happy Server billing.
pub const DEFAULT_PROXY_PROFILE: &str = "proxy-minimax/minimax-m2.5:free";

/// Built-in direct profile used when Cteno runs without Happy auth.
pub const DEFAULT_DIRECT_PROFILE: &str = "default";

/// Default profile for browser agent sessions (vision-capable).
pub const BROWSER_AGENT_PROFILE: &str = "proxy-kimi-k2.5";

/// API protocol format for LLM endpoints
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub enum ApiFormat {
    #[default]
    #[serde(rename = "anthropic")]
    Anthropic,
    #[serde(rename = "openai")]
    OpenAI,
    #[serde(rename = "gemini")]
    Gemini,
}

/// A single LLM API endpoint configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmEndpoint {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub temperature: f32,
    pub max_tokens: u32,
    /// Optional explicit context window size (tokens) for compression threshold calculation.
    /// If absent, client falls back to model-name-based defaults.
    pub context_window_tokens: Option<u32>,
}

/// An LLM profile with chat and compress endpoints
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmProfile {
    pub id: String,
    pub name: String,
    pub chat: LlmEndpoint,
    pub compress: LlmEndpoint,
    #[serde(default)]
    pub supports_vision: bool,
    #[serde(default)]
    pub supports_computer_use: bool,
    #[serde(default)]
    pub api_format: ApiFormat,
    /// Enable thinking/reasoning mode (server-driven)
    #[serde(default)]
    pub thinking: bool,
    /// Whether this is a free model (no balance deduction)
    #[serde(default)]
    pub is_free: bool,
    /// Model supports function calling / tool use (default true)
    #[serde(default = "default_true")]
    pub supports_function_calling: bool,
    /// Model can generate images in chat response (requires responseModalities)
    #[serde(default)]
    pub supports_image_output: bool,
}

impl LlmProfile {
    /// Resolve the compress API key with fallback: compress → chat → global
    pub fn resolve_compress_api_key(&self, global_api_key: &str) -> String {
        if !self.compress.api_key.is_empty() {
            self.compress.api_key.clone()
        } else if !self.chat.api_key.is_empty() {
            self.chat.api_key.clone()
        } else {
            global_api_key.to_string()
        }
    }
}

/// Storage container for all profiles
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileStore {
    pub profiles: Vec<LlmProfile>,
    pub default_profile_id: String,
}

impl ProfileStore {
    /// Get a profile by ID (checks user profiles only; use get_profile_or_proxy for proxy support)
    pub fn get_profile(&self, id: &str) -> Option<&LlmProfile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    /// Get a profile by ID, falling back to dynamic proxy profiles
    pub fn get_profile_or_proxy(
        &self,
        id: &str,
        proxy_profiles: &[LlmProfile],
    ) -> Option<LlmProfile> {
        if let Some(p) = self.profiles.iter().find(|p| p.id == id) {
            return Some(p.clone());
        }
        proxy_profiles.iter().find(|p| p.id == id).cloned()
    }

    /// Get the default profile
    pub fn get_default(&self) -> &LlmProfile {
        self.get_profile(&self.default_profile_id)
            .unwrap_or_else(|| {
                self.profiles
                    .first()
                    .expect("ProfileStore must have at least one profile")
            })
    }

    /// Get the default profile, honoring proxy defaults when available.
    pub fn get_default_or_proxy(&self, proxy_profiles: &[LlmProfile]) -> LlmProfile {
        self.get_profile_or_proxy(&self.default_profile_id, proxy_profiles)
            .unwrap_or_else(|| self.get_default().clone())
    }

    /// Delete a profile by ID. Returns false if it's the default or not found.
    pub fn delete_profile(&mut self, id: &str) -> bool {
        if id == self.default_profile_id {
            return false;
        }
        let before = self.profiles.len();
        self.profiles.retain(|p| p.id != id);
        self.profiles.len() < before
    }

    /// Save or update a profile. If a profile with the same ID exists, replace it.
    /// Empty api_key fields are treated as "keep existing" to avoid overwriting
    /// keys that the frontend cannot display (only masked versions are sent).
    pub fn save_profile(&mut self, mut profile: LlmProfile) {
        if let Some(existing) = self.profiles.iter_mut().find(|p| p.id == profile.id) {
            if profile.chat.api_key.is_empty() {
                profile.chat.api_key = existing.chat.api_key.clone();
            }
            if profile.compress.api_key.is_empty() {
                profile.compress.api_key = existing.compress.api_key.clone();
            }
            *existing = profile;
        } else {
            self.profiles.push(profile);
        }
    }

    /// Create a display version with masked API keys, including dynamic proxy profiles
    pub fn to_display(
        &self,
        global_api_key: &str,
        proxy_profiles: &[LlmProfile],
    ) -> serde_json::Value {
        let format_profile = |p: &LlmProfile, is_proxy: bool| -> serde_json::Value {
            serde_json::json!({
                "id": p.id,
                "name": p.name,
                "isProxy": is_proxy,
                "supportsVision": p.supports_vision,
                "supportsComputerUse": p.supports_computer_use,
                "apiFormat": match p.api_format {
                    ApiFormat::Anthropic => "anthropic",
                    ApiFormat::OpenAI => "openai",
                    ApiFormat::Gemini => "gemini",
                },
                "isFree": p.is_free,
                "chat": {
                    "api_key_masked": if is_proxy { "(代理模式)".to_string() } else { mask_api_key(&p.chat.api_key, global_api_key) },
                    "base_url": p.chat.base_url,
                    "model": p.chat.model,
                    "temperature": p.chat.temperature,
                    "max_tokens": p.chat.max_tokens,
                    "context_window_tokens": p.chat.context_window_tokens,
                },
                "compress": {
                    "api_key_masked": if is_proxy { "(代理模式)".to_string() } else { mask_api_key(&p.compress.api_key, global_api_key) },
                    "base_url": p.compress.base_url,
                    "model": p.compress.model,
                    "temperature": p.compress.temperature,
                    "max_tokens": p.compress.max_tokens,
                    "context_window_tokens": p.compress.context_window_tokens,
                },
            })
        };

        let mut profiles: Vec<serde_json::Value> = Vec::new();

        // Add dynamic proxy profiles first
        for p in proxy_profiles {
            profiles.push(format_profile(p, true));
        }

        // Add user profiles
        for p in &self.profiles {
            profiles.push(format_profile(p, false));
        }

        serde_json::json!({
            "profiles": profiles,
            "defaultProfileId": self.default_profile_id,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedProfileSelection {
    pub profile_id: String,
    pub profile: LlmProfile,
}

pub fn default_cteno_profile_id(has_happy_auth: bool) -> &'static str {
    if has_happy_auth {
        DEFAULT_PROXY_PROFILE
    } else {
        DEFAULT_DIRECT_PROFILE
    }
}

pub fn direct_fallback_selection(store: &ProfileStore) -> ResolvedProfileSelection {
    let profile = store
        .get_profile(&store.default_profile_id)
        .filter(|profile| !is_proxy_profile(&profile.id))
        .cloned()
        .or_else(|| store.get_profile(DEFAULT_DIRECT_PROFILE).cloned())
        .or_else(|| {
            store
                .profiles
                .iter()
                .find(|profile| !is_proxy_profile(&profile.id))
                .cloned()
        })
        .unwrap_or_else(get_default_profile);

    ResolvedProfileSelection {
        profile_id: profile.id.clone(),
        profile,
    }
}

fn normalize_selector(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn effort_prefers_thinking(effort: Option<&str>) -> Option<bool> {
    let effort = normalize_selector(effort)?;
    if effort.eq_ignore_ascii_case("low")
        || effort.eq_ignore_ascii_case("none")
        || effort.eq_ignore_ascii_case("off")
    {
        return Some(false);
    }
    if effort.eq_ignore_ascii_case("medium")
        || effort.eq_ignore_ascii_case("high")
        || effort.eq_ignore_ascii_case("max")
        || effort.eq_ignore_ascii_case("maximal")
    {
        return Some(true);
    }
    None
}

fn pick_model_match<'a>(
    matches: impl IntoIterator<Item = &'a LlmProfile>,
    requested_effort: Option<&str>,
) -> Option<&'a LlmProfile> {
    let matches = matches.into_iter().collect::<Vec<_>>();
    if matches.is_empty() {
        return None;
    }

    if let Some(prefer_thinking) = effort_prefers_thinking(requested_effort) {
        if let Some(profile) = matches
            .iter()
            .copied()
            .find(|profile| profile.thinking == prefer_thinking)
        {
            return Some(profile);
        }
    }

    matches.into_iter().next()
}

pub fn resolve_profile_selection(
    store: &ProfileStore,
    proxy_profiles: &[LlmProfile],
    requested_profile_id: Option<&str>,
    requested_model: Option<&str>,
    requested_effort: Option<&str>,
) -> Option<ResolvedProfileSelection> {
    if let Some(profile_id) = normalize_selector(requested_profile_id) {
        return store
            .get_profile_or_proxy(profile_id, proxy_profiles)
            .map(|profile| ResolvedProfileSelection {
                profile_id: profile.id.clone(),
                profile,
            });
    }

    let model = normalize_selector(requested_model)?;

    if let Some(profile) = store.get_profile_or_proxy(model, proxy_profiles) {
        return Some(ResolvedProfileSelection {
            profile_id: profile.id.clone(),
            profile,
        });
    }

    let user_match = pick_model_match(
        store
            .profiles
            .iter()
            .filter(|profile| profile.chat.model == model),
        requested_effort,
    );
    if let Some(profile) = user_match {
        return Some(ResolvedProfileSelection {
            profile_id: profile.id.clone(),
            profile: profile.clone(),
        });
    }

    let proxy_match = pick_model_match(
        proxy_profiles
            .iter()
            .filter(|profile| profile.chat.model == model),
        requested_effort,
    );
    proxy_match.map(|profile| ResolvedProfileSelection {
        profile_id: profile.id.clone(),
        profile: profile.clone(),
    })
}

fn default_profile_selection(
    store: &ProfileStore,
    proxy_profiles: &[LlmProfile],
) -> ResolvedProfileSelection {
    let profile = store.get_default_or_proxy(proxy_profiles);
    ResolvedProfileSelection {
        profile_id: profile.id.clone(),
        profile,
    }
}

async fn load_proxy_profiles_for_resolution(
    server_url: &str,
    app_data_dir: &Path,
) -> Vec<LlmProfile> {
    if server_url.trim().is_empty() {
        return load_proxy_profiles_cache(app_data_dir);
    }
    fetch_proxy_profiles_from_server(server_url, app_data_dir).await
}

pub async fn resolve_profile_request(
    app_data_dir: &Path,
    server_url: &str,
    requested_profile_id: Option<&str>,
    requested_model: Option<&str>,
    requested_effort: Option<&str>,
) -> ResolvedProfileSelection {
    let store = load_profiles(app_data_dir);
    let proxy_profiles = load_proxy_profiles_for_resolution(server_url, app_data_dir).await;

    if let Some(selection) = resolve_profile_selection(
        &store,
        &proxy_profiles,
        requested_profile_id,
        requested_model,
        requested_effort,
    ) {
        return selection;
    }

    let fallback = default_profile_selection(&store, &proxy_profiles);
    if let Some(profile_id) = normalize_selector(requested_profile_id) {
        log::warn!(
            "LLM profile '{}' not found; falling back to default profile '{}'",
            profile_id,
            fallback.profile_id
        );
    } else if let Some(model) = normalize_selector(requested_model) {
        log::warn!(
            "No LLM profile matched model '{}' (effort={:?}); falling back to default profile '{}'",
            model,
            normalize_selector(requested_effort),
            fallback.profile_id
        );
    }

    fallback
}

/// Mask an API key for display
fn default_true() -> bool {
    true
}

fn mask_api_key(key: &str, global_key: &str) -> String {
    if key.is_empty() {
        if global_key.is_empty() {
            "(未配置)".to_string()
        } else {
            "(使用全局)".to_string()
        }
    } else if key.len() <= 8 {
        "***".to_string()
    } else {
        format!("***{}", &key[key.len() - 4..])
    }
}

/// Returns the built-in default profile
pub fn get_default_profile() -> LlmProfile {
    LlmProfile {
        id: "default".to_string(),
        name: "DeepSeek Reasoner".to_string(),
        chat: LlmEndpoint {
            api_key: String::new(),
            base_url: "https://api.deepseek.com/anthropic".to_string(),
            model: "deepseek-reasoner".to_string(),
            temperature: 0.7,
            max_tokens: 32000,
            context_window_tokens: None,
        },
        compress: LlmEndpoint {
            api_key: String::new(),
            base_url: "https://api.deepseek.com/anthropic".to_string(),
            model: "deepseek-chat".to_string(),
            temperature: 0.3,
            max_tokens: 3200,
            context_window_tokens: None,
        },
        supports_vision: false,
        supports_computer_use: false,
        api_format: ApiFormat::Anthropic,
        thinking: false,
        is_free: false,
        supports_function_calling: true,
        supports_image_output: false,
    }
}

/// Get the path to profiles.json
fn profiles_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("profiles.json")
}

/// Load profiles from disk, creating defaults if file doesn't exist
pub fn load_profiles(app_data_dir: &Path) -> ProfileStore {
    let path = profiles_path(app_data_dir);

    if path.exists() {
        match std::fs::read_to_string(&path) {
            Ok(content) => match serde_json::from_str::<ProfileStore>(&content) {
                Ok(mut store) if !store.profiles.is_empty() => {
                    log::info!(
                        "Loaded {} LLM profiles from {:?}",
                        store.profiles.len(),
                        path
                    );
                    // Migrate: old defaults → current free default
                    if store.default_profile_id == "default"
                        || store.default_profile_id == "proxy-deepseek-reasoner"
                    {
                        store.default_profile_id = DEFAULT_PROXY_PROFILE.to_string();
                        log::info!("Migrated default_profile_id to '{}'", DEFAULT_PROXY_PROFILE);
                        if let Err(e) = save_profiles(app_data_dir, &store) {
                            log::warn!("Failed to persist profile migration: {}", e);
                        }
                    }
                    return store;
                }
                Ok(_) => {
                    log::warn!("profiles.json is empty, creating defaults");
                }
                Err(e) => {
                    log::error!("Failed to parse profiles.json: {}, creating defaults", e);
                }
            },
            Err(e) => {
                log::error!("Failed to read profiles.json: {}, creating defaults", e);
            }
        }
    }

    // Create default store
    let store = ProfileStore {
        profiles: vec![get_default_profile()],
        default_profile_id: DEFAULT_PROXY_PROFILE.to_string(),
    };

    // Save defaults to disk
    if let Err(e) = save_profiles(app_data_dir, &store) {
        log::error!("Failed to save default profiles: {}", e);
    }

    store
}

/// Path to the proxy profiles disk cache
fn proxy_profiles_cache_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("proxy_profiles_cache.json")
}

/// Save proxy profiles cache to disk
fn save_proxy_profiles_cache(app_data_dir: &Path, profiles: &[LlmProfile]) {
    let path = proxy_profiles_cache_path(app_data_dir);
    match serde_json::to_string_pretty(profiles) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                log::warn!("Failed to write proxy profiles cache: {}", e);
            }
        }
        Err(e) => log::warn!("Failed to serialize proxy profiles cache: {}", e),
    }
}

/// Load proxy profiles cache from disk (fallback when server is unreachable)
pub fn load_proxy_profiles_cache(app_data_dir: &Path) -> Vec<LlmProfile> {
    let path = proxy_profiles_cache_path(app_data_dir);
    if !path.exists() {
        return Vec::new();
    }
    match std::fs::read_to_string(&path) {
        Ok(content) => match serde_json::from_str::<Vec<LlmProfile>>(&content) {
            Ok(profiles) => {
                log::info!("Loaded {} proxy profiles from disk cache", profiles.len());
                profiles
            }
            Err(e) => {
                log::warn!("Failed to parse proxy profiles cache: {}", e);
                Vec::new()
            }
        },
        Err(e) => {
            log::warn!("Failed to read proxy profiles cache: {}", e);
            Vec::new()
        }
    }
}

fn build_http_client() -> Result<reqwest::Client, String> {
    match try_build_http_client(false) {
        Ok(client) => Ok(client),
        Err(primary_error) => {
            log::warn!(
                "proxy profile client init failed with system proxy settings: {}; retrying without proxy autodiscovery",
                primary_error
            );
            try_build_http_client(true)
        }
    }
}

fn try_build_http_client(no_proxy: bool) -> Result<reqwest::Client, String> {
    let builder = if no_proxy {
        reqwest::Client::builder().no_proxy()
    } else {
        reqwest::Client::builder()
    };

    let _guard = client_build_panic_hook_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = catch_unwind(AssertUnwindSafe(|| builder.build()));
    std::panic::set_hook(previous_hook);

    result
        .map_err(|payload| {
            if let Some(message) = payload.downcast_ref::<&'static str>() {
                (*message).to_string()
            } else if let Some(message) = payload.downcast_ref::<String>() {
                message.clone()
            } else {
                "unknown panic while building reqwest client".to_string()
            }
        })?
        .map_err(|error| error.to_string())
}

fn client_build_panic_hook_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Fetch proxy profiles dynamically from server's GET /v1/balance/models endpoint.
/// On success, caches to disk. On failure, loads from disk cache.
pub async fn fetch_proxy_profiles_from_server(
    server_url: &str,
    app_data_dir: &Path,
) -> Vec<LlmProfile> {
    let url = format!("{}/v1/balance/models", server_url.trim_end_matches('/'));
    let client = match build_http_client() {
        Ok(client) => client,
        Err(error) => {
            log::warn!(
                "Failed to initialize proxy profile HTTP client: {}, using disk cache",
                error
            );
            return load_proxy_profiles_cache(app_data_dir);
        }
    };

    let response = match client
        .get(&url)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            log::warn!(
                "Failed to fetch proxy models from server: {}, using disk cache",
                e
            );
            return load_proxy_profiles_cache(app_data_dir);
        }
    };

    if !response.status().is_success() {
        log::warn!(
            "Server returned {} for /v1/balance/models, using disk cache",
            response.status()
        );
        return load_proxy_profiles_cache(app_data_dir);
    }

    let body: serde_json::Value = match response.json().await {
        Ok(v) => v,
        Err(e) => {
            log::warn!(
                "Failed to parse proxy models response: {}, using disk cache",
                e
            );
            return load_proxy_profiles_cache(app_data_dir);
        }
    };

    let models = match body.get("models").and_then(|m| m.as_array()) {
        Some(arr) => arr,
        None => {
            log::warn!("Missing 'models' array in /v1/balance/models response, using disk cache");
            return load_proxy_profiles_cache(app_data_dir);
        }
    };

    // Find the compress model ID (isCompressModel=true), fallback to "deepseek-chat"
    let compress_model_id = models
        .iter()
        .find(|m| {
            m.get("isCompressModel")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        })
        .and_then(|m| m.get("id").and_then(|v| v.as_str()))
        .unwrap_or("deepseek-chat")
        .to_string();

    let profiles: Vec<LlmProfile> = models
        .iter()
        .filter_map(|m| {
            let id = m.get("id")?.as_str()?;
            let name = m.get("name")?.as_str()?;
            let context_window_tokens = m
                .get("contextWindowTokens")
                .and_then(|v| v.as_u64())
                .and_then(|v| {
                    if v <= u32::MAX as u64 {
                        Some(v as u32)
                    } else {
                        None
                    }
                });
            let supports_vision = m
                .get("supportsVision")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let supports_computer_use = m
                .get("supportsComputerUse")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let api_format = match m.get("apiFormat").and_then(|v| v.as_str()) {
                Some("openai") => ApiFormat::OpenAI,
                Some("gemini") => ApiFormat::Gemini,
                _ => ApiFormat::Anthropic,
            };

            let temperature = m
                .get("temperature")
                .and_then(|v| v.as_f64())
                .map(|v| v as f32)
                .unwrap_or(0.7);
            let thinking = m.get("thinking").and_then(|v| v.as_bool()).unwrap_or(false);
            let is_free = m.get("isFree").and_then(|v| v.as_bool()).unwrap_or(false);
            let supports_function_calling = m
                .get("supportsFunctionCalling")
                .and_then(|v| v.as_bool())
                .unwrap_or(true); // default true
            let supports_image_output = m
                .get("supportsImageOutput")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            Some(LlmProfile {
                id: format!("proxy-{}", id),
                name: name.to_string(),
                chat: LlmEndpoint {
                    api_key: String::new(),
                    base_url: String::new(),
                    model: id.to_string(),
                    temperature,
                    max_tokens: 32000,
                    context_window_tokens,
                },
                compress: LlmEndpoint {
                    api_key: String::new(),
                    base_url: String::new(),
                    model: compress_model_id.clone(),
                    temperature: 0.3,
                    max_tokens: 3200,
                    context_window_tokens: None,
                },
                supports_vision,
                supports_computer_use,
                api_format,
                thinking,
                is_free,
                supports_function_calling,
                supports_image_output,
            })
        })
        .collect();

    log::info!(
        "Fetched {} proxy models from server (compress={})",
        profiles.len(),
        compress_model_id
    );

    // Cache to disk for offline fallback
    save_proxy_profiles_cache(app_data_dir, &profiles);

    profiles
}

/// Check if a profile ID is a built-in proxy profile
pub fn is_proxy_profile(profile_id: &str) -> bool {
    profile_id.starts_with("proxy-")
}

/// Save profiles to disk
pub fn save_profiles(app_data_dir: &Path, store: &ProfileStore) -> Result<(), String> {
    let path = profiles_path(app_data_dir);

    let content = serde_json::to_string_pretty(store)
        .map_err(|e| format!("Failed to serialize profiles: {}", e))?;

    std::fs::write(&path, content)
        .map_err(|e| format!("Failed to write profiles to {:?}: {}", path, e))?;

    log::info!("Saved {} LLM profiles to {:?}", store.profiles.len(), path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_profile(id: &str, model: &str, thinking: bool) -> LlmProfile {
        LlmProfile {
            id: id.to_string(),
            name: id.to_string(),
            chat: LlmEndpoint {
                api_key: if id.starts_with("user-") {
                    "user-key".to_string()
                } else {
                    String::new()
                },
                base_url: if id.starts_with("proxy-") {
                    String::new()
                } else {
                    "https://example.com".to_string()
                },
                model: model.to_string(),
                temperature: 0.2,
                max_tokens: 4096,
                context_window_tokens: None,
            },
            compress: LlmEndpoint {
                api_key: String::new(),
                base_url: "https://example.com".to_string(),
                model: format!("{model}-compress"),
                temperature: 0.1,
                max_tokens: 1024,
                context_window_tokens: None,
            },
            supports_vision: false,
            supports_computer_use: false,
            api_format: ApiFormat::Anthropic,
            thinking,
            is_free: false,
            supports_function_calling: true,
            supports_image_output: false,
        }
    }

    #[test]
    fn resolver_prefers_explicit_profile_id() {
        let store = ProfileStore {
            profiles: vec![build_profile("user-direct", "gpt-5.1", false)],
            default_profile_id: "user-direct".to_string(),
        };

        let selection = resolve_profile_selection(
            &store,
            &[],
            Some("user-direct"),
            Some("ignored-model"),
            Some("high"),
        )
        .expect("selection");

        assert_eq!(selection.profile_id, "user-direct");
    }

    #[test]
    fn resolver_prefers_user_profile_before_proxy_model_match() {
        let store = ProfileStore {
            profiles: vec![build_profile("user-gpt5", "gpt-5.1", false)],
            default_profile_id: "user-gpt5".to_string(),
        };
        let proxy_profiles = vec![build_profile("proxy-gpt5", "gpt-5.1", true)];

        let selection =
            resolve_profile_selection(&store, &proxy_profiles, None, Some("gpt-5.1"), None)
                .expect("selection");

        assert_eq!(selection.profile_id, "user-gpt5");
    }

    #[test]
    fn resolver_uses_effort_to_prefer_thinking_profile() {
        let store = ProfileStore {
            profiles: vec![
                build_profile("user-fast", "gpt-5.1", false),
                build_profile("user-think", "gpt-5.1", true),
            ],
            default_profile_id: "user-fast".to_string(),
        };

        let selection = resolve_profile_selection(&store, &[], None, Some("gpt-5.1"), Some("high"))
            .expect("selection");

        assert_eq!(selection.profile_id, "user-think");
        assert!(selection.profile.thinking);
    }

    #[test]
    fn cteno_default_profile_switches_with_auth_state() {
        assert_eq!(default_cteno_profile_id(false), DEFAULT_DIRECT_PROFILE);
        assert_eq!(default_cteno_profile_id(true), DEFAULT_PROXY_PROFILE);
    }

    #[test]
    fn direct_fallback_prefers_store_default_when_it_is_direct() {
        let store = ProfileStore {
            profiles: vec![
                build_profile("user-direct", "deepseek-chat", false),
                build_profile("user-secondary", "gpt-5.1", false),
            ],
            default_profile_id: "user-direct".to_string(),
        };

        let selection = direct_fallback_selection(&store);

        assert_eq!(selection.profile_id, "user-direct");
    }

    #[test]
    fn direct_fallback_uses_builtin_default_when_store_default_is_proxy() {
        let store = ProfileStore {
            profiles: vec![build_profile("proxy-user", "minimax-m2.5", false)],
            default_profile_id: DEFAULT_PROXY_PROFILE.to_string(),
        };

        let selection = direct_fallback_selection(&store);

        assert_eq!(selection.profile_id, DEFAULT_DIRECT_PROFILE);
        assert_eq!(
            selection.profile.chat.base_url,
            "https://api.deepseek.com/anthropic"
        );
    }

    #[tokio::test]
    async fn resolve_profile_request_falls_back_to_cached_proxy_default() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = ProfileStore {
            profiles: vec![get_default_profile()],
            default_profile_id: DEFAULT_PROXY_PROFILE.to_string(),
        };
        save_profiles(temp.path(), &store).expect("save profiles");

        let proxy_profiles = vec![build_profile(DEFAULT_PROXY_PROFILE, "minimax-m2.5", false)];
        let cache_path = temp.path().join("proxy_profiles_cache.json");
        std::fs::write(
            &cache_path,
            serde_json::to_string_pretty(&proxy_profiles).expect("serialize cache"),
        )
        .expect("write cache");

        let selection =
            resolve_profile_request(temp.path(), "", None, Some("missing-model"), Some("medium"))
                .await;

        assert_eq!(selection.profile_id, DEFAULT_PROXY_PROFILE);
    }
}
