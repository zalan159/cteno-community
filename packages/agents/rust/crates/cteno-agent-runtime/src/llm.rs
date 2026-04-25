/**
 * LLM Integration Module
 *
 * Provides integration with Anthropic Messages API compatible endpoints
 * (DeepSeek, Zhipu, Kimi, etc.) for autonomous agents.
 * Supports structured tool calling with content blocks.
 */
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::future::Future;
use std::ops::AddAssign;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// LLM Message role
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Assistant,
    System, // Kept for backward compat; filtered out in API requests
}

/// Image source for vision-capable models (Anthropic format)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageSource {
    #[serde(rename = "type")]
    pub source_type: String, // "base64" | "url"
    pub media_type: String, // "image/jpeg" | "image/png" | "image/webp" | "image/gif"
    pub data: String,       // base64 data or URL
}

/// Content block types for Anthropic Messages API
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
        signature: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        gemini_thought_signature: Option<String>,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default, skip_serializing_if = "is_false")]
        is_error: bool,
    },
    Image {
        source: ImageSource,
    },
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Message content: either a simple string or structured content blocks
#[derive(Debug, Clone)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

impl MessageContent {
    /// Create text content
    pub fn text(s: impl Into<String>) -> Self {
        MessageContent::Text(s.into())
    }

    /// Create content from blocks
    pub fn blocks(blocks: Vec<ContentBlock>) -> Self {
        MessageContent::Blocks(blocks)
    }

    /// Extract text content (joining all text blocks)
    pub fn as_text(&self) -> String {
        match self {
            MessageContent::Text(s) => s.clone(),
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
        }
    }
}

impl Serialize for MessageContent {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            MessageContent::Text(s) => serializer.serialize_str(s),
            MessageContent::Blocks(blocks) => blocks.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for MessageContent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        match value {
            Value::String(s) => Ok(MessageContent::Text(s)),
            Value::Array(_) => {
                let blocks: Vec<ContentBlock> =
                    serde_json::from_value(value).map_err(serde::de::Error::custom)?;
                Ok(MessageContent::Blocks(blocks))
            }
            _ => Err(serde::de::Error::custom(
                "Expected string or array for content",
            )),
        }
    }
}

/// LLM Message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: MessageRole,
    pub content: MessageContent,
}

impl Message {
    /// Create a user text message
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: MessageContent::text(text),
        }
    }

    /// Create an assistant text message
    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: MessageContent::text(text),
        }
    }
}

/// Tool definition for LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: Value, // JSON Schema for tool parameters
}

/// Tool use in LLM response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUse {
    pub id: String,
    pub name: String,
    pub input: Value,
    /// Gemini thought signature — must be sent back in conversation history for function calling.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gemini_thought_signature: Option<String>,
}

/// LLM Response type
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LLMResponseType {
    Text { text: String },
    Thinking { thinking: String, signature: String },
    ToolUse { tool_use: ToolUse },
    Image { media_type: String, data: String },
}

/// LLM Response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMResponse {
    pub content: Vec<LLMResponseType>,
    pub stop_reason: String, // "end_turn" | "tool_use" | "max_tokens"
    pub usage: Usage,
}

/// Token usage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_creation_input_tokens: u32,
    pub cache_read_input_tokens: u32,
}

impl Usage {
    /// Create a zero-value Usage (no tokens consumed).
    pub fn zero() -> Self {
        Self {
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        }
    }

    /// Total input tokens including cached tokens.
    /// This represents the actual context window consumption.
    pub fn total_input_tokens(&self) -> u32 {
        self.input_tokens + self.cache_creation_input_tokens + self.cache_read_input_tokens
    }

    /// Total tokens (input + output) for billing purposes.
    pub fn total_tokens(&self) -> u32 {
        self.total_input_tokens() + self.output_tokens
    }
}

impl AddAssign for Usage {
    fn add_assign(&mut self, rhs: Self) {
        self.input_tokens += rhs.input_tokens;
        self.output_tokens += rhs.output_tokens;
        self.cache_creation_input_tokens += rhs.cache_creation_input_tokens;
        self.cache_read_input_tokens += rhs.cache_read_input_tokens;
    }
}

/// Prefix used to mark structured content blocks in SessionMessage persistence
pub const BLOCKS_PREFIX: &str = "BLOCKS:";
/// HTTP timeout for LLM API calls.
/// Set to 8 hours — agent sessions are long-running by design; we want
/// agents to keep working as long as possible without timing out.
const LLM_HTTP_TIMEOUT_SECS: u64 = 8 * 60 * 60;

/// Minimum interval between streaming delta sends to the frontend (milliseconds).
/// Throttles to ~5 updates/sec to avoid overwhelming Socket.IO with tiny encrypted messages.
const STREAM_DELTA_THROTTLE_MS: u64 = 200;
/// Minimum accumulated chars before sending a delta (even if throttle time hasn't elapsed).
const STREAM_DELTA_MIN_CHARS: usize = 50;

/// Callback for streaming text delta events during SSE parsing.
/// Receives a serde_json::Value (e.g., `{"type": "text-delta", "text": "chunk"}`)
/// and sends it asynchronously to the frontend via Socket.IO.
pub type StreamCallback =
    Arc<dyn Fn(Value) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Authentication mode for LLM API requests
#[derive(Debug, Clone, PartialEq)]
pub enum AuthMode {
    /// Direct API key (x-api-key header) — for BYOK / direct provider access
    ApiKey,
    /// Bearer token (Authorization header) — for Happy Server proxy
    BearerToken,
    /// OpenRouter subkey (Bearer + direct openrouter.ai endpoint).
    /// Base URL is expected to be `https://openrouter.ai/api/v1`; chat maps
    /// to `/messages` (Anthropic-compat), not the happy-server `/v1/llm/chat`
    /// shape.
    OpenRouter,
}

/// LLM API Client (Anthropic Messages API compatible)
///
/// Works with any Anthropic-compatible endpoint:
/// - DeepSeek: https://api.deepseek.com/anthropic
/// - Anthropic: https://api.anthropic.com
/// - Zhipu: https://open.bigmodel.cn/api/anthropic
/// - Kimi: https://api.moonshot.cn/anthropic
/// - Happy Server proxy: https://server/v1/llm/chat
pub struct LLMClient {
    api_key: String,
    base_url: String,
    client: reqwest::Client,
    auth_mode: AuthMode,
    machine_id: Option<String>,
}

/// Keep backward compatibility alias
pub type OpenRouterClient = LLMClient;

impl LLMClient {
    fn normalize_deepseek_thinking_effort(
        model: &str,
        effort: Option<&str>,
    ) -> Option<&'static str> {
        if !model.to_ascii_lowercase().contains("deepseek-v4") {
            return None;
        }
        match effort.map(|value| value.trim().to_ascii_lowercase()).as_deref() {
            Some("low") | Some("medium") | Some("high") => Some("high"),
            Some("xhigh") | Some("max") | Some("maximal") => Some("max"),
            _ => None,
        }
    }

    fn build_http_client(disable_proxy_autodiscovery: bool) -> reqwest::Client {
        let mut builder =
            reqwest::Client::builder().timeout(Duration::from_secs(LLM_HTTP_TIMEOUT_SECS));
        if disable_proxy_autodiscovery {
            builder = builder.no_proxy();
        }
        builder.build().expect("Failed to create HTTP client")
    }

    fn mock_response_text(&self) -> Option<String> {
        self.base_url
            .strip_prefix("mock://")
            .map(|raw| raw.replace('_', " "))
    }

    async fn mock_openai_response(
        &self,
        response_text: String,
        stream_callback: Option<&StreamCallback>,
    ) -> Result<LLMResponse, String> {
        if let Some(callback) = stream_callback {
            callback(json!({ "type": "stream-start" })).await;
            callback(json!({
                "type": "text-delta",
                "text": response_text.clone(),
            }))
            .await;
        }

        Ok(LLMResponse {
            content: vec![LLMResponseType::Text {
                text: response_text,
            }],
            stop_reason: "end_turn".to_string(),
            usage: Usage::zero(),
        })
    }

    /// Create new LLM client with default DeepSeek Anthropic endpoint
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: "https://api.deepseek.com/anthropic".to_string(),
            client: Self::build_http_client(false),
            auth_mode: AuthMode::ApiKey,
            machine_id: None,
        }
    }

    /// Create new client with custom base URL
    pub fn with_base_url(api_key: String, base_url: String) -> Self {
        let disable_proxy_autodiscovery = base_url.starts_with("mock://");
        Self {
            api_key,
            base_url,
            client: Self::build_http_client(disable_proxy_autodiscovery),
            auth_mode: AuthMode::ApiKey,
            machine_id: None,
        }
    }

    /// Create a proxy client that routes through Happy Server.
    /// Uses Bearer token auth instead of x-api-key.
    pub fn with_proxy(auth_token: String, server_url: String) -> Self {
        Self {
            api_key: auth_token,
            base_url: server_url,
            client: Self::build_http_client(false),
            auth_mode: AuthMode::BearerToken,
            machine_id: None,
        }
    }

    /// Create a client that connects directly to OpenRouter with a per-user
    /// subkey. `base_url` should be `https://openrouter.ai/api/v1`.
    pub fn with_openrouter(subkey: String, base_url: String) -> Self {
        Self {
            api_key: subkey,
            base_url,
            client: Self::build_http_client(false),
            auth_mode: AuthMode::OpenRouter,
            machine_id: None,
        }
    }

    /// Set machine ID for proxy billing attribution.
    pub fn set_machine_id(&mut self, id: String) {
        self.machine_id = Some(id);
    }

    /// Create a proxy client with machine ID auto-loaded from persistent storage.
    ///
    /// Checks Tauri app_data_dir first (dev-scoped), then falls back to legacy path.
    pub fn with_proxy_and_machine_id(auth_token: String, server_url: String) -> Self {
        let mut client = Self::with_proxy(auth_token, server_url);
        // Try to load machine_id - check Tauri app data dir (passed via env) then legacy
        let paths_to_try: Vec<std::path::PathBuf> = {
            let mut v = Vec::new();
            // The app_data_dir is set as an env var during init for convenience
            if let Ok(dir) = std::env::var("CTENO_APP_DATA_DIR") {
                v.push(std::path::PathBuf::from(dir).join("machine_id.txt"));
            }
            if let Some(data_dir) = dirs::data_dir() {
                v.push(data_dir.join("Cteno").join("machine_id.txt"));
            }
            v
        };
        for path in paths_to_try {
            if let Ok(id) = std::fs::read_to_string(&path) {
                let id = id.trim().to_string();
                if !id.is_empty() {
                    client.machine_id = Some(id);
                    break;
                }
            }
        }
        client
    }

    /// Call LLM API with messages and tools (Anthropic Messages API format).
    ///
    /// `stream_callback`: Optional callback that receives streaming text deltas
    /// as they arrive from the SSE stream. Used to push partial content to the
    /// frontend in real-time. Only effective for proxy mode (BearerToken auth).
    pub async fn chat_anthropic(
        &self,
        model: &str,
        system_prompt: &str,
        messages: &[Message],
        tools: &[Tool],
        temperature: f32,
        max_tokens: u32,
        stream_callback: Option<&StreamCallback>,
        enable_thinking: bool,
        reasoning_effort: Option<&str>,
    ) -> Result<LLMResponse, String> {
        // Proxy mode uses a flat endpoint; direct mode appends /v1/messages;
        // OpenRouter base_url already includes /api/v1 so just /messages.
        let url = match self.auth_mode {
            AuthMode::BearerToken => format!("{}/v1/llm/chat", self.base_url),
            AuthMode::ApiKey => format!("{}/v1/messages", self.base_url),
            AuthMode::OpenRouter => format!("{}/messages", self.base_url),
        };

        // Build messages (ensure user/assistant alternation, skip system)
        let mut api_messages = self.build_messages(messages);

        // Add cache_control breakpoints to conversation history for prompt caching.
        // Anthropic caches everything from request start up to the last cache_control marker.
        // We mark the second-to-last user turn so the conversation prefix is cached across turns.
        // (The last turn changes each request, so caching it would just waste cache writes.)
        if api_messages.len() >= 3 {
            // Find the second-to-last user message and mark it
            let last_user_idx = api_messages.iter().rposition(|m| m["role"] == "user");
            if let Some(last_idx) = last_user_idx {
                // Find the user message before the last one
                let second_last_user_idx = api_messages[..last_idx]
                    .iter()
                    .rposition(|m| m["role"] == "user");
                let target_idx = second_last_user_idx.unwrap_or(last_idx);
                // Ensure content is array format (required for cache_control on content blocks)
                let msg = &mut api_messages[target_idx];
                let content = msg["content"].take();
                if content.is_string() {
                    msg["content"] = json!([{
                        "type": "text",
                        "text": content.as_str().unwrap_or(""),
                        "cache_control": {"type": "ephemeral"}
                    }]);
                } else if content.is_array() {
                    let mut blocks = content;
                    if let Some(last_block) = blocks.as_array_mut().and_then(|a| a.last_mut()) {
                        last_block["cache_control"] = json!({"type": "ephemeral"});
                    }
                    msg["content"] = blocks;
                }
            }
        }

        // Build request body (Anthropic format)
        let mut request_body = json!({
            "model": model,
            "max_tokens": max_tokens,
            "messages": api_messages,
            "temperature": temperature,
        });

        // System prompt as content block array with cache_control on last block
        if !system_prompt.is_empty() {
            request_body["system"] = json!([{
                "type": "text",
                "text": system_prompt,
                "cache_control": {"type": "ephemeral"}
            }]);
        }

        // Add tools (Anthropic format) with cache_control on last tool
        if !tools.is_empty() {
            let mut tool_values: Vec<Value> = tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": Self::sanitize_schema_for_anthropic(&t.input_schema),
                    })
                })
                .collect();
            // Mark last tool for caching (tools definition is stable across turns)
            if let Some(last_tool) = tool_values.last_mut() {
                last_tool["cache_control"] = json!({"type": "ephemeral"});
            }
            request_body["tools"] = json!(tool_values);
        }

        // Enable streaming when a stream_callback is provided (real-time text display).
        // Otherwise keep stream: false for non-streaming callers.
        let use_streaming = stream_callback.is_some();
        request_body["stream"] = json!(use_streaming);

        // Enable thinking mode when server-configured (temperature already set by profile)
        if enable_thinking {
            request_body["thinking"] = json!({"type": "enabled"});
            if let Some(effort) = Self::normalize_deepseek_thinking_effort(model, reasoning_effort)
            {
                request_body["output_config"] = json!({"effort": effort});
            }
            log::info!("[LLM] Enabled thinking mode for model: {}", model);
        }

        log::info!(
            "LLM API request to {}: {}",
            url,
            serde_json::to_string_pretty(&request_body).unwrap_or_default()
        );

        // Make request with appropriate auth headers
        let mut req = self
            .client
            .post(&url)
            .header("Content-Type", "application/json");

        req = match self.auth_mode {
            AuthMode::ApiKey => req
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("anthropic-beta", "prompt-caching-2024-07-31"),
            AuthMode::BearerToken => {
                // For proxy mode, pass beta as body field (server forwards as header)
                request_body["anthropic_beta"] = json!("prompt-caching-2024-07-31");
                let r = req.header("Authorization", format!("Bearer {}", &self.api_key));
                if let Some(mid) = &self.machine_id {
                    r.header("X-Machine-Id", mid.as_str())
                } else {
                    r
                }
            }
            AuthMode::OpenRouter => req
                .header("Authorization", format!("Bearer {}", &self.api_key))
                .header("anthropic-version", "2023-06-01")
                .header("anthropic-beta", "prompt-caching-2024-07-31"),
        };

        let response = req.json(&request_body).send().await.map_err(|e| {
            log::error!(
                "[LLM] HTTP request failed: {} (url={}, auth_mode={:?})",
                e,
                url,
                self.auth_mode
            );
            format!("HTTP request failed: {}", e)
        })?;

        let status = response.status();

        if use_streaming {
            // Streaming mode: check status from initial response, then parse SSE
            if !status.is_success() {
                let error_text = response
                    .text()
                    .await
                    .unwrap_or_else(|_| "Failed to read error response".to_string());
                if let Ok(err_json) = serde_json::from_str::<Value>(&error_text) {
                    if let Some(error_msg) = err_json.get("error").and_then(|e| e.as_str()) {
                        return Err(error_msg.to_string());
                    }
                }
                return Err(format!("LLM API error ({}): {}", status, error_text));
            }
            self.parse_sse_stream(response, stream_callback).await
        } else {
            // Non-streaming mode: read full JSON response
            let response_text = response
                .text()
                .await
                .map_err(|e| format!("Failed to read response: {}", e))?;

            if !status.is_success() {
                if let Ok(err_json) = serde_json::from_str::<Value>(&response_text) {
                    if let Some(error_msg) = err_json.get("error").and_then(|e| e.as_str()) {
                        return Err(error_msg.to_string());
                    }
                }
                return Err(format!("LLM API error ({}): {}", status, response_text));
            }

            log::info!("LLM API response: {}", response_text);

            let response_json: Value = serde_json::from_str(&response_text)
                .map_err(|e| format!("Failed to parse response JSON: {}", e))?;

            self.parse_response(&response_json)
        }
    }

    /// Parse SSE stream from proxy and reconstruct LLMResponse.
    ///
    /// Supports two SSE formats:
    /// - **Anthropic**: message_start, content_block_start/delta/stop, message_delta, message_stop
    /// - **OpenAI**: chat.completion.chunk with choices[].delta.content/tool_calls and optional reasoning_content
    ///
    /// Format is auto-detected from the first SSE event.
    ///
    /// If `stream_callback` is provided, text deltas are forwarded to the frontend
    /// in real-time (throttled to avoid overwhelming Socket.IO).
    ///
    /// Resilient to stream errors: if the connection breaks mid-stream, returns
    /// whatever content has been accumulated so far instead of failing.
    async fn parse_sse_stream(
        &self,
        response: reqwest::Response,
        stream_callback: Option<&StreamCallback>,
    ) -> Result<LLMResponse, String> {
        let mut buffer = String::new();
        let mut input_tokens: u32 = 0;
        let mut output_tokens: u32 = 0;
        let mut cache_creation_input_tokens: u32 = 0;
        let mut cache_read_input_tokens: u32 = 0;
        let mut stop_reason = "end_turn".to_string();

        // Content blocks being accumulated
        struct BlockState {
            block_type: String,
            text: String,
            thinking: String,
            signature: String,
            tool_id: String,
            tool_name: String,
            tool_input_json: String,
        }
        let mut blocks: Vec<BlockState> = Vec::new();

        // Streaming delta throttle state
        let mut pending_text_delta = String::new();
        let mut pending_thinking_delta = String::new();
        let mut last_delta_send = Instant::now();
        let mut last_thinking_delta_send = Instant::now();
        let mut stream_error: Option<String> = None;

        // Auto-detect format: None = not yet detected
        #[derive(PartialEq, Clone, Copy)]
        enum SseFormat {
            Anthropic,
            OpenAI,
            OpenAIResponses,
            Gemini,
        }
        let mut detected_format: Option<SseFormat> = None;

        // OpenAI/Gemini format: we accumulate text/thinking into a single implicit text block + optional thinking block
        let mut openai_text = String::new();
        let mut openai_thinking = String::new();
        let mut openai_tool_calls: Vec<(String, String, String)> = Vec::new(); // (id, name, arguments_json)
        let mut openai_sent_stream_start = false;

        // Gemini format accumulator (reuses openai_text/openai_thinking for simplicity)
        let mut gemini_tool_calls: Vec<(String, Value)> = Vec::new(); // (name, args)
        let mut gemini_images: Vec<(String, String)> = Vec::new(); // (mimeType, base64_data)
        let mut gemini_sent_stream_start = false;

        let mut stream = response.bytes_stream();
        loop {
            let chunk_result = match stream.next().await {
                Some(result) => result,
                None => break, // Stream ended normally
            };

            let chunk = match chunk_result {
                Ok(c) => c,
                Err(e) => {
                    // Stream broken mid-read. Log and return partial content if available.
                    let err_msg = format!("Stream read error: {}", e);
                    log::warn!("[LLM] {} — will return accumulated content if any", err_msg);
                    stream_error = Some(err_msg);
                    break;
                }
            };

            log::debug!("[LLM] SSE chunk received: {} bytes", chunk.len());
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            // Process complete SSE lines from buffer
            while let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].trim_end_matches('\r').to_string();
                buffer = buffer[newline_pos + 1..].to_string();

                if !line.starts_with("data: ") {
                    continue;
                }
                let json_str = &line[6..];
                if json_str.trim().is_empty() || json_str == "[DONE]" {
                    continue;
                }

                let event: Value = match serde_json::from_str(json_str) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                // Auto-detect format from first event
                if detected_format.is_none() {
                    let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    if event_type.starts_with("response.") {
                        detected_format = Some(SseFormat::OpenAIResponses);
                        log::info!("[LLM] SSE format detected: OpenAI Responses API");
                    } else if !event_type.is_empty() {
                        detected_format = Some(SseFormat::Anthropic);
                        log::info!("[LLM] SSE format detected: Anthropic");
                    } else if event.get("candidates").is_some() {
                        detected_format = Some(SseFormat::Gemini);
                        log::info!("[LLM] SSE format detected: Gemini");
                    } else if event.get("choices").is_some()
                        || event.get("object").and_then(|o| o.as_str())
                            == Some("chat.completion.chunk")
                    {
                        detected_format = Some(SseFormat::OpenAI);
                        log::info!("[LLM] SSE format detected: OpenAI Chat Completions");
                    }
                }

                // ── OpenAI Responses API format ─────────────────────────
                if detected_format == Some(SseFormat::OpenAIResponses) {
                    let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");

                    // Send stream-start on first content event
                    if !openai_sent_stream_start && event_type.starts_with("response.output") {
                        if let Some(cb) = stream_callback {
                            cb(json!({ "type": "stream-start" })).await;
                        }
                        openai_sent_stream_start = true;
                    }

                    match event_type {
                        "response.output_text.delta" => {
                            if let Some(delta) = event.get("delta").and_then(|d| d.as_str()) {
                                openai_text.push_str(delta);

                                if stream_callback.is_some() {
                                    pending_text_delta.push_str(delta);
                                    let elapsed = last_delta_send.elapsed().as_millis() as u64;
                                    if elapsed >= STREAM_DELTA_THROTTLE_MS
                                        || pending_text_delta.len() >= STREAM_DELTA_MIN_CHARS
                                    {
                                        let cb = stream_callback.unwrap();
                                        let text = std::mem::take(&mut pending_text_delta);
                                        cb(json!({ "type": "text-delta", "text": text })).await;
                                        last_delta_send = Instant::now();
                                    }
                                }
                            }
                        }
                        "response.output_item.added" => {
                            // Track function call items (name + call_id)
                            if let Some(item) = event.get("item") {
                                let item_type =
                                    item.get("type").and_then(|t| t.as_str()).unwrap_or("");
                                if item_type == "function_call" {
                                    let call_id = item
                                        .get("call_id")
                                        .and_then(|s| s.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let name = item
                                        .get("name")
                                        .and_then(|s| s.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    openai_tool_calls.push((call_id, name, String::new()));
                                }
                            }
                        }
                        "response.function_call_arguments.delta" => {
                            if let Some(delta) = event.get("delta").and_then(|d| d.as_str()) {
                                // Find the matching tool call by call_id
                                let call_id =
                                    event.get("call_id").and_then(|s| s.as_str()).unwrap_or("");
                                if let Some(tc) = openai_tool_calls
                                    .iter_mut()
                                    .find(|(id, _, _)| id == call_id)
                                {
                                    tc.2.push_str(delta);
                                } else if let Some(tc) = openai_tool_calls.last_mut() {
                                    tc.2.push_str(delta);
                                }
                            }
                        }
                        "response.completed" => {
                            // Extract usage and stop reason
                            if let Some(resp) = event.get("response") {
                                if let Some(usage) = resp.get("usage") {
                                    input_tokens = usage
                                        .get("input_tokens")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(0)
                                        as u32;
                                    output_tokens = usage
                                        .get("output_tokens")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(0)
                                        as u32;
                                    cache_read_input_tokens = usage
                                        .pointer("/input_tokens_details/cached_tokens")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(0)
                                        as u32;
                                }
                                let status = resp
                                    .get("status")
                                    .and_then(|s| s.as_str())
                                    .unwrap_or("completed");
                                stop_reason = match status {
                                    "completed" => "end_turn".to_string(),
                                    "incomplete" => "max_tokens".to_string(),
                                    other => other.to_string(),
                                };
                            }
                        }
                        _ => {} // response.created, response.in_progress, response.content_part.*, etc.
                    }
                    continue;
                }

                // ── OpenAI Chat Completions format ───────────────────────
                if detected_format == Some(SseFormat::OpenAI) {
                    // Send stream-start on first chunk
                    if !openai_sent_stream_start {
                        if let Some(cb) = stream_callback {
                            cb(json!({ "type": "stream-start" })).await;
                        }
                        openai_sent_stream_start = true;
                    }

                    // Extract usage if present (some providers include it in the last chunk)
                    if let Some(usage) = event.get("usage") {
                        if let Some(t) = usage.get("prompt_tokens").and_then(|v| v.as_u64()) {
                            input_tokens = t as u32;
                        }
                        if let Some(t) = usage.get("completion_tokens").and_then(|v| v.as_u64()) {
                            output_tokens = t as u32;
                        }
                    }

                    if let Some(choices) = event.get("choices").and_then(|c| c.as_array()) {
                        for choice in choices {
                            // finish_reason
                            if let Some(fr) = choice.get("finish_reason").and_then(|f| f.as_str()) {
                                stop_reason = match fr {
                                    "stop" => "end_turn".to_string(),
                                    "tool_calls" => "tool_use".to_string(),
                                    "length" => "max_tokens".to_string(),
                                    other => other.to_string(),
                                };
                            }

                            if let Some(delta) = choice.get("delta") {
                                // Text content
                                if let Some(content) = delta.get("content").and_then(|c| c.as_str())
                                {
                                    openai_text.push_str(content);

                                    // Forward to frontend via stream callback (throttled)
                                    if stream_callback.is_some() {
                                        pending_text_delta.push_str(content);
                                        let elapsed = last_delta_send.elapsed().as_millis() as u64;
                                        if elapsed >= STREAM_DELTA_THROTTLE_MS
                                            || pending_text_delta.len() >= STREAM_DELTA_MIN_CHARS
                                        {
                                            let cb = stream_callback.unwrap();
                                            let text = std::mem::take(&mut pending_text_delta);
                                            cb(json!({
                                                "type": "text-delta",
                                                "text": text
                                            }))
                                            .await;
                                            last_delta_send = Instant::now();
                                        }
                                    }
                                }

                                // Reasoning/thinking content (OpenRouter, DeepSeek, etc.)
                                let reasoning = delta
                                    .get("reasoning_content")
                                    .or_else(|| delta.get("reasoning"))
                                    .and_then(|r| r.as_str());
                                if let Some(r) = reasoning {
                                    openai_thinking.push_str(r);

                                    if stream_callback.is_some() {
                                        pending_thinking_delta.push_str(r);
                                        let elapsed =
                                            last_thinking_delta_send.elapsed().as_millis() as u64;
                                        if elapsed >= STREAM_DELTA_THROTTLE_MS
                                            || pending_thinking_delta.len()
                                                >= STREAM_DELTA_MIN_CHARS
                                        {
                                            let cb = stream_callback.unwrap();
                                            let text = std::mem::take(&mut pending_thinking_delta);
                                            cb(json!({
                                                "type": "thinking-delta",
                                                "text": text
                                            }))
                                            .await;
                                            last_thinking_delta_send = Instant::now();
                                        }
                                    }
                                }

                                // Tool calls
                                if let Some(tool_calls) =
                                    delta.get("tool_calls").and_then(|tc| tc.as_array())
                                {
                                    for tc in tool_calls {
                                        let idx =
                                            tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0)
                                                as usize;
                                        // Extend tool_calls vec if needed
                                        while openai_tool_calls.len() <= idx {
                                            openai_tool_calls.push((
                                                String::new(),
                                                String::new(),
                                                String::new(),
                                            ));
                                        }
                                        if let Some(func) = tc.get("function") {
                                            if let Some(id) = tc.get("id").and_then(|s| s.as_str())
                                            {
                                                openai_tool_calls[idx].0 = id.to_string();
                                            }
                                            if let Some(name) =
                                                func.get("name").and_then(|s| s.as_str())
                                            {
                                                openai_tool_calls[idx].1 = name.to_string();
                                            }
                                            if let Some(args) =
                                                func.get("arguments").and_then(|s| s.as_str())
                                            {
                                                openai_tool_calls[idx].2.push_str(args);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    continue;
                }

                // ── Gemini format ───────────────────────────────────────
                if detected_format == Some(SseFormat::Gemini) {
                    if !gemini_sent_stream_start {
                        if let Some(cb) = stream_callback {
                            cb(json!({ "type": "stream-start" })).await;
                        }
                        gemini_sent_stream_start = true;
                    }

                    // Extract usage metadata
                    if let Some(usage) = event.get("usageMetadata") {
                        input_tokens = usage
                            .get("promptTokenCount")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as u32;
                        output_tokens = usage
                            .get("candidatesTokenCount")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as u32;
                        cache_read_input_tokens = usage
                            .get("cachedContentTokenCount")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as u32;
                    }

                    if let Some(candidates) = event.get("candidates").and_then(|c| c.as_array()) {
                        for candidate in candidates {
                            // finish_reason
                            if let Some(fr) = candidate.get("finishReason").and_then(|f| f.as_str())
                            {
                                stop_reason = match fr {
                                    "STOP" => "end_turn".to_string(),
                                    "MAX_TOKENS" => "max_tokens".to_string(),
                                    other => other.to_lowercase(),
                                };
                            }

                            if let Some(parts) = candidate
                                .pointer("/content/parts")
                                .and_then(|p| p.as_array())
                            {
                                for part in parts {
                                    // Text content
                                    if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                                        // Check if this is a thought part
                                        let is_thought = part
                                            .get("thought")
                                            .and_then(|t| t.as_bool())
                                            .unwrap_or(false);
                                        if is_thought {
                                            openai_thinking.push_str(text);
                                            if stream_callback.is_some() {
                                                pending_thinking_delta.push_str(text);
                                                let elapsed =
                                                    last_thinking_delta_send.elapsed().as_millis()
                                                        as u64;
                                                if elapsed >= STREAM_DELTA_THROTTLE_MS
                                                    || pending_thinking_delta.len()
                                                        >= STREAM_DELTA_MIN_CHARS
                                                {
                                                    let cb = stream_callback.unwrap();
                                                    let t =
                                                        std::mem::take(&mut pending_thinking_delta);
                                                    cb(json!({ "type": "thinking-delta", "text": t })).await;
                                                    last_thinking_delta_send = Instant::now();
                                                }
                                            }
                                        } else {
                                            openai_text.push_str(text);
                                            if stream_callback.is_some() {
                                                pending_text_delta.push_str(text);
                                                let elapsed =
                                                    last_delta_send.elapsed().as_millis() as u64;
                                                if elapsed >= STREAM_DELTA_THROTTLE_MS
                                                    || pending_text_delta.len()
                                                        >= STREAM_DELTA_MIN_CHARS
                                                {
                                                    let cb = stream_callback.unwrap();
                                                    let t = std::mem::take(&mut pending_text_delta);
                                                    cb(json!({ "type": "text-delta", "text": t }))
                                                        .await;
                                                    last_delta_send = Instant::now();
                                                }
                                            }
                                        }
                                    }

                                    // Function calls
                                    if let Some(fc) = part.get("functionCall") {
                                        let name = fc
                                            .get("name")
                                            .and_then(|n| n.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        let args = fc.get("args").cloned().unwrap_or(json!({}));
                                        gemini_tool_calls.push((name, args));
                                    }

                                    // Inline image data (e.g. Nano Banana 2 image generation)
                                    // Only keep the first image per response to avoid duplicates
                                    if gemini_images.is_empty() {
                                        if let Some(inline_data) = part.get("inlineData") {
                                            let mime = inline_data
                                                .get("mimeType")
                                                .and_then(|m| m.as_str())
                                                .unwrap_or("image/png")
                                                .to_string();
                                            if let Some(data) =
                                                inline_data.get("data").and_then(|d| d.as_str())
                                            {
                                                log::info!(
                                                    "[LLM] Gemini inlineData: {} ({} bytes base64)",
                                                    mime,
                                                    data.len()
                                                );
                                                gemini_images.push((mime, data.to_string()));
                                            }
                                        }
                                    } else if part.get("inlineData").is_some() {
                                        log::info!("[LLM] Gemini inlineData: skipped (only keeping first image)");
                                    }
                                }
                            }
                        }
                    }
                    continue;
                }

                // ── Anthropic format ───────────────────────────────────
                let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");

                match event_type {
                    "message_start" => {
                        // Signal frontend to reset streaming state for new LLM turn
                        if let Some(cb) = stream_callback {
                            log::info!("[LLM] Sending stream-start callback");
                            cb(json!({ "type": "stream-start" })).await;
                        }
                        if let Some(usage) = event.pointer("/message/usage") {
                            input_tokens = usage
                                .get("input_tokens")
                                .and_then(|t| t.as_u64())
                                .unwrap_or(0) as u32;
                            cache_creation_input_tokens = usage
                                .get("cache_creation_input_tokens")
                                .and_then(|t| t.as_u64())
                                .unwrap_or(0)
                                as u32;
                            cache_read_input_tokens = usage
                                .get("cache_read_input_tokens")
                                .and_then(|t| t.as_u64())
                                .unwrap_or(0)
                                as u32;
                        }
                    }
                    "content_block_start" => {
                        let block = event.get("content_block").unwrap_or(&Value::Null);
                        let bt = block
                            .get("type")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string();
                        blocks.push(BlockState {
                            block_type: bt.clone(),
                            text: String::new(),
                            thinking: String::new(),
                            signature: String::new(),
                            tool_id: if bt == "tool_use" {
                                block
                                    .get("id")
                                    .and_then(|s| s.as_str())
                                    .unwrap_or("tool_0")
                                    .to_string()
                            } else {
                                String::new()
                            },
                            tool_name: if bt == "tool_use" {
                                block
                                    .get("name")
                                    .and_then(|s| s.as_str())
                                    .unwrap_or("")
                                    .to_string()
                            } else {
                                String::new()
                            },
                            tool_input_json: String::new(),
                        });
                    }
                    "content_block_delta" => {
                        let index =
                            event.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                        if let Some(block) = blocks.get_mut(index) {
                            if let Some(delta) = event.get("delta") {
                                let delta_type =
                                    delta.get("type").and_then(|t| t.as_str()).unwrap_or("");
                                match delta_type {
                                    "text_delta" => {
                                        if let Some(t) = delta.get("text").and_then(|t| t.as_str())
                                        {
                                            block.text.push_str(t);

                                            // Forward to frontend via stream callback (throttled)
                                            if stream_callback.is_some() {
                                                pending_text_delta.push_str(t);
                                                let elapsed =
                                                    last_delta_send.elapsed().as_millis() as u64;
                                                if elapsed >= STREAM_DELTA_THROTTLE_MS
                                                    || pending_text_delta.len()
                                                        >= STREAM_DELTA_MIN_CHARS
                                                {
                                                    let cb = stream_callback.unwrap();
                                                    let text =
                                                        std::mem::take(&mut pending_text_delta);
                                                    cb(json!({
                                                        "type": "text-delta",
                                                        "text": text
                                                    }))
                                                    .await;
                                                    last_delta_send = Instant::now();
                                                }
                                            }
                                        }
                                    }
                                    "thinking_delta" => {
                                        if let Some(t) =
                                            delta.get("thinking").and_then(|t| t.as_str())
                                        {
                                            block.thinking.push_str(t);

                                            // Forward thinking delta to frontend (throttled)
                                            if stream_callback.is_some() {
                                                pending_thinking_delta.push_str(t);
                                                let elapsed =
                                                    last_thinking_delta_send.elapsed().as_millis()
                                                        as u64;
                                                if elapsed >= STREAM_DELTA_THROTTLE_MS
                                                    || pending_thinking_delta.len()
                                                        >= STREAM_DELTA_MIN_CHARS
                                                {
                                                    let cb = stream_callback.unwrap();
                                                    let text =
                                                        std::mem::take(&mut pending_thinking_delta);
                                                    cb(json!({
                                                        "type": "thinking-delta",
                                                        "text": text
                                                    }))
                                                    .await;
                                                    last_thinking_delta_send = Instant::now();
                                                }
                                            }
                                        }
                                    }
                                    "signature_delta" => {
                                        if let Some(s) =
                                            delta.get("signature").and_then(|s| s.as_str())
                                        {
                                            block.signature.push_str(s);
                                        }
                                    }
                                    "input_json_delta" => {
                                        if let Some(j) =
                                            delta.get("partial_json").and_then(|j| j.as_str())
                                        {
                                            block.tool_input_json.push_str(j);
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    "message_delta" => {
                        if let Some(delta) = event.get("delta") {
                            if let Some(sr) = delta.get("stop_reason").and_then(|s| s.as_str()) {
                                stop_reason = sr.to_string();
                            }
                        }
                        if let Some(usage) = event.get("usage") {
                            output_tokens = usage
                                .get("output_tokens")
                                .and_then(|t| t.as_u64())
                                .unwrap_or(0) as u32;
                        }
                    }
                    _ => {} // content_block_stop, message_stop, ping, etc.
                }
            }
        }

        // Flush remaining streaming deltas
        if let Some(cb) = stream_callback {
            if !pending_text_delta.is_empty() {
                cb(json!({
                    "type": "text-delta",
                    "text": pending_text_delta
                }))
                .await;
            }
            if !pending_thinking_delta.is_empty() {
                cb(json!({
                    "type": "thinking-delta",
                    "text": pending_thinking_delta
                }))
                .await;
            }
        }

        // Convert OpenAI/Gemini/Responses accumulated state into blocks
        if matches!(
            detected_format,
            Some(SseFormat::OpenAI) | Some(SseFormat::OpenAIResponses) | Some(SseFormat::Gemini)
        ) {
            if !openai_thinking.is_empty() {
                blocks.push(BlockState {
                    block_type: "thinking".to_string(),
                    text: String::new(),
                    thinking: openai_thinking,
                    signature: String::new(),
                    tool_id: String::new(),
                    tool_name: String::new(),
                    tool_input_json: String::new(),
                });
            }
            if !openai_text.is_empty() {
                blocks.push(BlockState {
                    block_type: "text".to_string(),
                    text: openai_text,
                    thinking: String::new(),
                    signature: String::new(),
                    tool_id: String::new(),
                    tool_name: String::new(),
                    tool_input_json: String::new(),
                });
            }
            for (id, name, args) in openai_tool_calls {
                if !name.is_empty() {
                    blocks.push(BlockState {
                        block_type: "tool_use".to_string(),
                        text: String::new(),
                        thinking: String::new(),
                        signature: String::new(),
                        tool_id: id,
                        tool_name: name,
                        tool_input_json: args,
                    });
                }
            }
            // Gemini function calls (args are already Value, serialize to JSON string)
            for (i, (name, args)) in gemini_tool_calls.into_iter().enumerate() {
                blocks.push(BlockState {
                    block_type: "tool_use".to_string(),
                    text: String::new(),
                    thinking: String::new(),
                    signature: String::new(),
                    tool_id: format!("toolu_gemini_{}", i),
                    tool_name: name,
                    tool_input_json: serde_json::to_string(&args).unwrap_or_default(),
                });
            }
        }

        // Convert accumulated blocks to LLMResponseType
        let mut content = Vec::new();
        for block in &blocks {
            match block.block_type.as_str() {
                "text" => {
                    if !block.text.is_empty() {
                        content.push(LLMResponseType::Text {
                            text: block.text.clone(),
                        });
                    }
                }
                "thinking" => {
                    content.push(LLMResponseType::Thinking {
                        thinking: block.thinking.clone(),
                        signature: block.signature.clone(),
                    });
                }
                "tool_use" => {
                    let input: Value =
                        serde_json::from_str(&block.tool_input_json).unwrap_or(json!({}));
                    content.push(LLMResponseType::ToolUse {
                        tool_use: ToolUse {
                            id: block.tool_id.clone(),
                            name: block.tool_name.clone(),
                            input,
                            gemini_thought_signature: None,
                        },
                    });
                }
                _ => {
                    log::warn!("[LLM] Unknown SSE content block type: {}", block.block_type);
                }
            }
        }

        // Append Gemini inline images as Image content blocks
        for (mime, data) in gemini_images {
            content.push(LLMResponseType::Image {
                media_type: mime,
                data,
            });
        }

        // If stream broke but we accumulated content, return it (degraded but usable)
        if let Some(err) = stream_error {
            if content.is_empty() {
                return Err(err);
            }
            log::warn!(
                "[LLM] Stream broke after accumulating {} content blocks — returning partial result",
                content.len()
            );
        }

        log::info!(
            "[LLM] SSE stream complete: {} content blocks, stop_reason={}, tokens(in={}, out={})",
            content.len(),
            stop_reason,
            input_tokens,
            output_tokens
        );

        Ok(LLMResponse {
            content,
            stop_reason,
            usage: Usage {
                input_tokens,
                output_tokens,
                cache_creation_input_tokens,
                cache_read_input_tokens,
            },
        })
    }

    /// Build messages array for Anthropic API
    ///
    /// - Skips System role messages (system prompt goes in top-level field)
    /// - Merges consecutive same-role messages (Anthropic requires alternation)
    /// - Ensures first message is from user
    fn build_messages(&self, messages: &[Message]) -> Vec<Value> {
        let mut result: Vec<Value> = vec![];

        for msg in messages {
            // Skip system messages - they go in the top-level "system" field
            if msg.role == MessageRole::System {
                continue;
            }

            let role_str = match msg.role {
                MessageRole::User => "user",
                MessageRole::Assistant => "assistant",
                MessageRole::System => continue,
            };

            let content_value = match &msg.content {
                MessageContent::Text(s) => json!(s),
                MessageContent::Blocks(blocks) => {
                    let serialized: Vec<Value> = blocks
                        .iter()
                        .map(|block| match block {
                            ContentBlock::Image { source } => {
                                if source.source_type == "url" {
                                    json!({
                                        "type": "image",
                                        "source": {
                                            "type": "url",
                                            "url": source.data,
                                        }
                                    })
                                } else {
                                    json!({
                                        "type": "image",
                                        "source": {
                                            "type": "base64",
                                            "media_type": source.media_type,
                                            "data": source.data,
                                        }
                                    })
                                }
                            }
                            other => serde_json::to_value(other).unwrap_or(json!(null)),
                        })
                        .collect();
                    json!(serialized)
                }
            };

            // Check if we need to merge with previous message (same role)
            if let Some(last) = result.last_mut() {
                let last_role = last["role"].as_str().unwrap_or("");
                if last_role == role_str {
                    // Merge into the existing message
                    last["content"] = merge_contents(&last["content"], &content_value);
                    continue;
                }
            }

            result.push(json!({
                "role": role_str,
                "content": content_value,
            }));
        }

        // Ensure first message is from user (Anthropic requirement)
        if result.first().and_then(|f| f["role"].as_str()) != Some("user") {
            result.insert(
                0,
                json!({
                    "role": "user",
                    "content": "Continue.",
                }),
            );
        }

        result
    }

    /// Parse Anthropic Messages API response
    fn parse_response(&self, response: &Value) -> Result<LLMResponse, String> {
        // Anthropic response format:
        // {
        //   "content": [{"type": "text", ...}, {"type": "tool_use", ...}],
        //   "stop_reason": "end_turn" | "tool_use",
        //   "usage": {"input_tokens": N, "output_tokens": N}
        // }

        let content_array = response
            .get("content")
            .and_then(|c| c.as_array())
            .ok_or("No content array in response")?;

        let mut content = vec![];

        for block in content_array {
            let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");

            match block_type {
                "text" => {
                    let text = block
                        .get("text")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string();
                    if !text.is_empty() {
                        content.push(LLMResponseType::Text { text });
                    }
                }
                "tool_use" => {
                    let tool_use = ToolUse {
                        id: block
                            .get("id")
                            .and_then(|id| id.as_str())
                            .unwrap_or("tool_0")
                            .to_string(),
                        name: block
                            .get("name")
                            .and_then(|n| n.as_str())
                            .ok_or("No name in tool_use block")?
                            .to_string(),
                        input: block.get("input").cloned().unwrap_or(json!({})),
                        gemini_thought_signature: None,
                    };
                    content.push(LLMResponseType::ToolUse { tool_use });
                }
                "thinking" => {
                    let thinking_text = block
                        .get("thinking")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string();
                    let signature = block
                        .get("signature")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string();
                    content.push(LLMResponseType::Thinking {
                        thinking: thinking_text,
                        signature,
                    });
                }
                _ => {
                    log::warn!("Unknown content block type: {}", block_type);
                }
            }
        }

        let stop_reason = response
            .get("stop_reason")
            .and_then(|r| r.as_str())
            .unwrap_or("end_turn")
            .to_string();

        let usage = response
            .get("usage")
            .map(|u| Usage {
                input_tokens: u.get("input_tokens").and_then(|t| t.as_u64()).unwrap_or(0) as u32,
                output_tokens: u.get("output_tokens").and_then(|t| t.as_u64()).unwrap_or(0) as u32,
                cache_creation_input_tokens: u
                    .get("cache_creation_input_tokens")
                    .and_then(|t| t.as_u64())
                    .unwrap_or(0) as u32,
                cache_read_input_tokens: u
                    .get("cache_read_input_tokens")
                    .and_then(|t| t.as_u64())
                    .unwrap_or(0) as u32,
            })
            .unwrap_or(Usage {
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
            });

        Ok(LLMResponse {
            content,
            stop_reason,
            usage,
        })
    }

    /// Call LLM API with OpenAI Responses API format.
    pub async fn chat_openai(
        &self,
        model: &str,
        system_prompt: &str,
        messages: &[Message],
        tools: &[Tool],
        temperature: f32,
        max_tokens: u32,
        stream_callback: Option<&StreamCallback>,
        enable_thinking: bool,
        reasoning_effort: Option<&str>,
    ) -> Result<LLMResponse, String> {
        if let Some(response_text) = self.mock_response_text() {
            return self
                .mock_openai_response(response_text, stream_callback)
                .await;
        }

        // Always stream in proxy mode; stream when callback is provided for direct API
        let use_streaming = self.auth_mode == AuthMode::BearerToken || stream_callback.is_some();
        let url = match self.auth_mode {
            AuthMode::BearerToken => format!("{}/v1/llm/openai-chat", self.base_url),
            AuthMode::ApiKey => format!("{}/v1/responses", self.base_url),
            // OpenRouter does not implement the "Responses API" shape; Cteno
            // routes OpenAI-Responses-format models through the Anthropic
            // endpoint instead. Fail fast with a clear message so callers
            // pick a different profile rather than debugging 404s.
            AuthMode::OpenRouter => {
                return Err(
                    "chat_openai (Responses API) is not supported on the OpenRouter direct path; \
                     use Anthropic-format chat_anthropic or route this model through the Happy \
                     Server proxy."
                        .to_string(),
                );
            }
        };

        let mut request_body = json!({
            "model": model,
            "max_output_tokens": max_tokens,
            "temperature": temperature,
            "stream": use_streaming,
        });

        if !system_prompt.is_empty() {
            request_body["instructions"] = json!(system_prompt);
        }

        if enable_thinking {
            request_body["thinking"] = json!({"type": "enabled"});
            if let Some(effort) = Self::normalize_deepseek_thinking_effort(model, reasoning_effort)
            {
                request_body["reasoning_effort"] = json!(effort);
            }
            log::info!("[LLM] Enabled OpenAI-format thinking mode for model: {}", model);
        }

        let input = self.build_openai_input(messages);
        request_body["input"] = json!(input);

        if !tools.is_empty() {
            request_body["tools"] = json!(tools
                .iter()
                .map(|t| json!({
                    "type": "function",
                    "name": t.name,
                    "description": t.description,
                    "parameters": Self::sanitize_schema_for_openai(&t.input_schema),
                }))
                .collect::<Vec<_>>());
        }

        log::info!(
            "LLM OpenAI API request to {}: {}",
            url,
            serde_json::to_string_pretty(&request_body).unwrap_or_default()
        );

        let req = self
            .client
            .post(&url)
            .header("Content-Type", "application/json");

        let req = match self.auth_mode {
            AuthMode::ApiKey => req.header("Authorization", format!("Bearer {}", &self.api_key)),
            AuthMode::BearerToken => {
                let r = req.header("Authorization", format!("Bearer {}", &self.api_key));
                if let Some(mid) = &self.machine_id {
                    r.header("X-Machine-Id", mid.as_str())
                } else {
                    r
                }
            }
            // Unreachable: the url-match above returns Err for OpenRouter.
            AuthMode::OpenRouter => unreachable!(),
        };

        let response = req.json(&request_body).send().await.map_err(|e| {
            log::error!(
                "[LLM] OpenAI HTTP request failed: {} (url={}, auth_mode={:?})",
                e,
                url,
                self.auth_mode
            );
            format!("HTTP request failed: {}", e)
        })?;

        let status = response.status();

        if !status.is_success() {
            let response_text = response
                .text()
                .await
                .map_err(|e| format!("Failed to read response: {}", e))?;
            return Err(format!("OpenAI API error ({}): {}", status, response_text));
        }

        if use_streaming {
            return self.parse_sse_stream(response, stream_callback).await;
        }

        let response_text = response
            .text()
            .await
            .map_err(|e| format!("Failed to read response: {}", e))?;

        log::info!("LLM OpenAI API response: {}", response_text);

        let response_json: Value = serde_json::from_str(&response_text)
            .map_err(|e| format!("Failed to parse response JSON: {}", e))?;

        self.parse_openai_response(&response_json)
    }

    /// Sanitize a JSON Schema for Anthropic compatibility.
    /// Anthropic forbids `oneOf`/`anyOf`/`allOf` at the top level of input_schema.
    /// Reuses the same flattening logic as OpenAI sanitization.
    fn sanitize_schema_for_anthropic(schema: &Value) -> Value {
        Self::sanitize_schema_for_openai(schema)
    }

    /// Sanitize a JSON Schema for OpenAI compatibility.
    /// OpenAI requires top-level `type: "object"` and forbids `oneOf`/`anyOf`/`allOf`/`not` at top level.
    /// We flatten variant schemas by merging all properties and making all fields optional.
    fn sanitize_schema_for_openai(schema: &Value) -> Value {
        let obj = match schema.as_object() {
            Some(o) => o,
            None => return schema.clone(),
        };

        // If top-level has oneOf/anyOf/allOf, flatten into a single object
        for key in &["oneOf", "anyOf", "allOf"] {
            if let Some(variants) = obj.get(*key).and_then(|v| v.as_array()) {
                let mut merged_props = serde_json::Map::new();
                let mut all_required: Vec<String> = vec![];

                for variant in variants {
                    if let Some(props) = variant.get("properties").and_then(|p| p.as_object()) {
                        for (k, v) in props {
                            merged_props.insert(k.clone(), v.clone());
                        }
                    }
                    // Collect required fields but we'll intersect later
                    if let Some(req) = variant.get("required").and_then(|r| r.as_array()) {
                        for r in req {
                            if let Some(s) = r.as_str() {
                                if !all_required.contains(&s.to_string()) {
                                    all_required.push(s.to_string());
                                }
                            }
                        }
                    }
                }

                // Find required fields that exist in ALL variants
                let common_required: Vec<String> = all_required
                    .into_iter()
                    .filter(|r| {
                        variants.iter().all(|v| {
                            v.get("required")
                                .and_then(|req| req.as_array())
                                .map(|arr| arr.iter().any(|x| x.as_str() == Some(r)))
                                .unwrap_or(false)
                        })
                    })
                    .collect();

                let mut result = serde_json::Map::new();
                result.insert("type".to_string(), json!("object"));
                if let Some(desc) = obj.get("description") {
                    result.insert("description".to_string(), desc.clone());
                }
                result.insert("properties".to_string(), json!(merged_props));
                if !common_required.is_empty() {
                    result.insert("required".to_string(), json!(common_required));
                }
                result.insert("additionalProperties".to_string(), json!(false));
                return json!(result);
            }
        }

        // No oneOf/anyOf/allOf — just remove `not` and `enum` at top level if present
        let mut cleaned = obj.clone();
        cleaned.remove("not");
        // top-level enum is not allowed for function parameters
        if cleaned.get("type").and_then(|t| t.as_str()) != Some("object") {
            if cleaned.contains_key("enum") {
                cleaned.remove("enum");
            }
        }
        json!(cleaned)
    }

    /// Convert internal Message list to OpenAI Responses API input array.
    fn build_openai_input(&self, messages: &[Message]) -> Vec<Value> {
        let mut input: Vec<Value> = vec![];
        let mut assistant_msg_counter = 0;

        for msg in messages {
            if msg.role == MessageRole::System {
                continue;
            }

            match &msg.content {
                MessageContent::Text(text) => match msg.role {
                    MessageRole::User => {
                        input.push(json!({
                            "type": "message",
                            "role": "user",
                            "content": [{"type": "input_text", "text": text}]
                        }));
                    }
                    MessageRole::Assistant => {
                        assistant_msg_counter += 1;
                        input.push(json!({
                            "type": "message",
                            "role": "assistant",
                            "id": format!("msg_{}", assistant_msg_counter),
                            "status": "completed",
                            "content": [{"type": "output_text", "text": text, "annotations": []}]
                        }));
                    }
                    _ => {}
                },
                MessageContent::Blocks(blocks) => {
                    let mut content_parts: Vec<Value> = vec![];
                    let mut function_calls: Vec<Value> = vec![];
                    let mut function_outputs: Vec<Value> = vec![];

                    for block in blocks {
                        match block {
                            ContentBlock::Text { text } => {
                                if msg.role == MessageRole::User {
                                    content_parts.push(json!({"type": "input_text", "text": text}));
                                } else {
                                    content_parts.push(
                                        json!({"type": "output_text", "text": text, "annotations": []}),
                                    );
                                }
                            }
                            ContentBlock::Image { source } => {
                                let image_url = if source.source_type == "base64" {
                                    format!("data:{};base64,{}", source.media_type, source.data)
                                } else {
                                    source.data.clone()
                                };
                                content_parts
                                    .push(json!({"type": "input_image", "image_url": image_url}));
                            }
                            ContentBlock::ToolUse {
                                id, name, input, ..
                            } => {
                                function_calls.push(json!({
                                    "type": "function_call",
                                    "id": format!("fc_{}", id),
                                    "call_id": id,
                                    "name": name,
                                    "arguments": serde_json::to_string(input).unwrap_or_default(),
                                }));
                            }
                            ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                                ..
                            } => {
                                function_outputs.push(json!({
                                    "type": "function_call_output",
                                    "call_id": tool_use_id,
                                    "output": content,
                                }));
                            }
                            ContentBlock::Thinking { .. } => {
                                // OpenAI doesn't support thinking blocks, skip
                            }
                        }
                    }

                    if !content_parts.is_empty() {
                        if msg.role == MessageRole::User {
                            input.push(json!({
                                "type": "message",
                                "role": "user",
                                "content": content_parts,
                            }));
                        } else {
                            assistant_msg_counter += 1;
                            input.push(json!({
                                "type": "message",
                                "role": "assistant",
                                "id": format!("msg_{}", assistant_msg_counter),
                                "status": "completed",
                                "content": content_parts,
                            }));
                        }
                    }

                    input.extend(function_calls);
                    input.extend(function_outputs);
                }
            }
        }
        input
    }

    /// Call LLM API with Gemini native protocol.
    pub async fn chat_gemini(
        &self,
        model: &str,
        system_prompt: &str,
        messages: &[Message],
        tools: &[Tool],
        temperature: f32,
        max_tokens: u32,
        image_output: bool,
        stream_callback: Option<&StreamCallback>,
    ) -> Result<LLMResponse, String> {
        let url = match self.auth_mode {
            AuthMode::BearerToken => format!("{}/v1/llm/gemini-chat", self.base_url),
            AuthMode::ApiKey => format!(
                "{}/gemini/v1beta/models/{}:streamGenerateContent?alt=sse",
                self.base_url, model
            ),
            // Same as chat_openai: the Gemini-native protocol is not exposed
            // on the OpenRouter subkey path. Route Gemini models through the
            // Anthropic-compatible `/messages` endpoint instead (OpenRouter's
            // "gemini-*" slugs accept Anthropic-format requests).
            AuthMode::OpenRouter => {
                return Err(
                    "chat_gemini (native protocol) is not supported on the OpenRouter direct \
                     path; call chat_anthropic with the openrouter gemini slug instead."
                        .to_string(),
                );
            }
        };

        let contents = self.build_gemini_contents(messages).await;

        let mut gen_config = json!({
            "maxOutputTokens": max_tokens,
            "temperature": temperature,
        });
        if image_output {
            gen_config["responseModalities"] = json!(["TEXT", "IMAGE"]);
            gen_config["numberOfImages"] = json!(1);
        }

        let mut request_body = json!({
            "contents": contents,
            "generationConfig": gen_config,
        });

        // Proxy mode includes model in body (server needs it to build upstream URL)
        if self.auth_mode == AuthMode::BearerToken {
            request_body["model"] = json!(model);
        }

        if !system_prompt.is_empty() {
            request_body["systemInstruction"] = json!({
                "parts": [{"text": system_prompt}]
            });
        }

        if !tools.is_empty() {
            let declarations: Vec<Value> = tools
                .iter()
                .map(|t| {
                    let sanitized = Self::sanitize_schema_for_gemini(&t.input_schema);
                    let params = Self::uppercase_types(&sanitized);
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "parameters": params,
                    })
                })
                .collect();
            request_body["tools"] = json!([{
                "functionDeclarations": declarations,
            }]);
        }

        // Log conversation structure for debugging
        if let Some(contents) = request_body.get("contents").and_then(|c| c.as_array()) {
            let structure: Vec<String> = contents
                .iter()
                .map(|c| {
                    let role = c.get("role").and_then(|r| r.as_str()).unwrap_or("?");
                    let parts: Vec<String> = c
                        .get("parts")
                        .and_then(|p| p.as_array())
                        .map(|arr| {
                            arr.iter()
                                .map(|p| {
                                    if p.get("text").is_some() {
                                        "text".to_string()
                                    } else if p.get("functionCall").is_some() {
                                        format!(
                                            "functionCall({})",
                                            p["functionCall"]["name"].as_str().unwrap_or("?")
                                        )
                                    } else if p.get("functionResponse").is_some() {
                                        format!(
                                            "functionResponse({})",
                                            p["functionResponse"]["name"].as_str().unwrap_or("?")
                                        )
                                    } else if p.get("inlineData").is_some() {
                                        "image".to_string()
                                    } else {
                                        "unknown".to_string()
                                    }
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    format!("{}:[{}]", role, parts.join(","))
                })
                .collect();
            log::info!(
                "LLM Gemini conversation structure: {}",
                structure.join(" → ")
            );
        }
        log::info!(
            "LLM Gemini API request to {} (body size: {} bytes)",
            url,
            serde_json::to_string(&request_body)
                .map(|s| s.len())
                .unwrap_or(0)
        );

        let req = self
            .client
            .post(&url)
            .header("Content-Type", "application/json");

        let req = match self.auth_mode {
            AuthMode::ApiKey => req.header("x-goog-api-key", &self.api_key),
            AuthMode::BearerToken => {
                let r = req.header("Authorization", format!("Bearer {}", &self.api_key));
                if let Some(mid) = &self.machine_id {
                    r.header("X-Machine-Id", mid.as_str())
                } else {
                    r
                }
            }
            // Unreachable: the gemini url-match above returns Err for OpenRouter.
            AuthMode::OpenRouter => unreachable!(),
        };

        let response = req.json(&request_body).send().await.map_err(|e| {
            log::error!(
                "[LLM] Gemini HTTP request failed: {} (url={}, auth_mode={:?})",
                e,
                url,
                self.auth_mode
            );
            format!("HTTP request failed: {}", e)
        })?;

        let status = response.status();

        if !status.is_success() {
            let response_text = response
                .text()
                .await
                .map_err(|e| format!("Failed to read response: {}", e))?;
            return Err(format!("Gemini API error ({}): {}", status, response_text));
        }

        // Parse SSE stream (Gemini streaming format is auto-detected)
        self.parse_sse_stream(response, stream_callback).await
    }

    /// Convert internal Message list to Gemini contents array.
    /// System messages are skipped (handled via systemInstruction).
    /// Consecutive same-role messages are merged (Gemini requires alternating roles).
    async fn build_gemini_contents(&self, messages: &[Message]) -> Vec<Value> {
        // Build a map from tool_use_id → tool_name for ToolResult lookups
        let mut tool_id_to_name: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for msg in messages {
            if let MessageContent::Blocks(blocks) = &msg.content {
                for block in blocks {
                    if let ContentBlock::ToolUse { id, name, .. } = block {
                        tool_id_to_name.insert(id.clone(), name.clone());
                    }
                }
            }
        }

        let mut result: Vec<Value> = vec![];

        for msg in messages {
            if msg.role == MessageRole::System {
                continue;
            }

            let mut parts: Vec<Value> = vec![];
            // Determine gemini role for this message's non-tool-result parts
            let gemini_role = match msg.role {
                MessageRole::User => "user",
                MessageRole::Assistant => "model",
                MessageRole::System => continue,
            };

            match &msg.content {
                MessageContent::Text(text) => {
                    parts.push(json!({"text": text}));
                }
                MessageContent::Blocks(blocks) => {
                    // Separate: tool results go into user role, everything else goes into the message role
                    let mut main_parts: Vec<Value> = vec![];
                    let mut tool_response_parts: Vec<Value> = vec![];

                    for block in blocks {
                        match block {
                            ContentBlock::Text { text } => {
                                main_parts.push(json!({"text": text}));
                            }
                            ContentBlock::Image { source } => {
                                if source.source_type == "url" {
                                    // Gemini requires inlineData; download and convert URL to base64
                                    match download_image_as_base64(&source.data).await {
                                        Ok(b64) => {
                                            main_parts.push(json!({
                                                "inlineData": {
                                                    "mimeType": source.media_type,
                                                    "data": b64,
                                                }
                                            }));
                                        }
                                        Err(e) => {
                                            log::error!(
                                                "[LLM] Failed to download image for Gemini: {}",
                                                e
                                            );
                                        }
                                    }
                                } else {
                                    main_parts.push(json!({
                                        "inlineData": {
                                            "mimeType": source.media_type,
                                            "data": source.data,
                                        }
                                    }));
                                }
                            }
                            ContentBlock::ToolUse {
                                name,
                                input,
                                gemini_thought_signature,
                                ..
                            } => {
                                let mut fc_part = json!({
                                    "functionCall": {
                                        "name": name,
                                        "args": input,
                                    }
                                });
                                // Include thoughtSignature as sibling of functionCall (required by ofox.ai)
                                if let Some(sig) = gemini_thought_signature {
                                    fc_part["thoughtSignature"] = json!(sig);
                                }
                                main_parts.push(fc_part);
                            }
                            ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                                ..
                            } => {
                                let name = tool_id_to_name
                                    .get(tool_use_id)
                                    .cloned()
                                    .unwrap_or_else(|| tool_use_id.clone());
                                tool_response_parts.push(json!({
                                    "functionResponse": {
                                        "name": name,
                                        "response": {"result": content},
                                    }
                                }));
                            }
                            ContentBlock::Thinking { .. } => {
                                // Gemini doesn't support thinking blocks, skip
                            }
                        }
                    }

                    // Add main parts under the message's own role
                    if !main_parts.is_empty() {
                        parts = main_parts;
                    }

                    // Tool responses must be under "user" role in their OWN message.
                    // Gemini rejects functionResponse mixed with text parts.
                    if !tool_response_parts.is_empty() {
                        // Emit main_parts first (if any) under the message's own role
                        if !parts.is_empty() {
                            Self::merge_or_push(&mut result, gemini_role, parts);
                            parts = vec![];
                        }
                        // Push functionResponse as a SEPARATE user message (never merge with text)
                        result.push(json!({
                            "role": "user",
                            "parts": tool_response_parts,
                        }));
                        continue;
                    }
                }
            }

            if !parts.is_empty() {
                Self::merge_or_push(&mut result, gemini_role, parts);
            }
        }

        result
    }

    /// Merge parts into the last message if same role, otherwise push a new message.
    /// Never merge into/with messages containing functionCall or functionResponse,
    /// as Gemini requires these to be in their own dedicated messages.
    fn merge_or_push(result: &mut Vec<Value>, role: &str, parts: Vec<Value>) {
        let has_function_parts = |arr: &[Value]| -> bool {
            arr.iter()
                .any(|p| p.get("functionCall").is_some() || p.get("functionResponse").is_some())
        };

        if let Some(last) = result.last_mut() {
            if last.get("role").and_then(|r| r.as_str()) == Some(role) {
                if let Some(existing_parts) = last.get_mut("parts").and_then(|p| p.as_array_mut()) {
                    // Don't merge if either side has function parts
                    if !has_function_parts(existing_parts) && !has_function_parts(&parts) {
                        existing_parts.extend(parts);
                        return;
                    }
                }
            }
        }
        result.push(json!({"role": role, "parts": parts}));
    }

    /// Parse Gemini generateContent response into LLMResponse.
    fn parse_gemini_response(&self, response: &Value) -> Result<LLMResponse, String> {
        let candidates = response
            .get("candidates")
            .and_then(|c| c.as_array())
            .ok_or("No candidates array in Gemini response")?;

        let candidate = candidates
            .first()
            .ok_or("Empty candidates array in Gemini response")?;

        let parts = candidate
            .get("content")
            .and_then(|c| c.get("parts"))
            .and_then(|p| p.as_array());

        let mut content = vec![];
        let mut has_function_call = false;

        if let Some(parts) = parts {
            for (index, part) in parts.iter().enumerate() {
                if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                    if !text.is_empty() {
                        content.push(LLMResponseType::Text {
                            text: text.to_string(),
                        });
                    }
                }
                if let Some(fc) = part.get("functionCall") {
                    has_function_call = true;
                    let name = fc
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    let args = fc.get("args").cloned().unwrap_or(json!({}));
                    // Capture thoughtSignature — sibling of functionCall in the part, required by ofox.ai
                    let thought_signature = part
                        .get("thoughtSignature")
                        .and_then(|s| s.as_str())
                        .map(|s| s.to_string());
                    content.push(LLMResponseType::ToolUse {
                        tool_use: ToolUse {
                            id: format!("gemini_call_{}", index),
                            name,
                            input: args,
                            gemini_thought_signature: thought_signature,
                        },
                    });
                }
            }
        }

        let finish_reason = candidate
            .get("finishReason")
            .and_then(|r| r.as_str())
            .unwrap_or("");

        let stop_reason = if has_function_call || finish_reason.contains("TOOL") {
            "tool_use"
        } else {
            "end_turn"
        }
        .to_string();

        let usage = response
            .get("usageMetadata")
            .map(|u| Usage {
                input_tokens: u
                    .get("promptTokenCount")
                    .and_then(|t| t.as_u64())
                    .unwrap_or(0) as u32,
                output_tokens: u
                    .get("candidatesTokenCount")
                    .and_then(|t| t.as_u64())
                    .unwrap_or(0) as u32,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: u
                    .get("cachedContentTokenCount")
                    .and_then(|t| t.as_u64())
                    .unwrap_or(0) as u32,
            })
            .unwrap_or(Usage::zero());

        Ok(LLMResponse {
            content,
            stop_reason,
            usage,
        })
    }

    /// Recursively uppercase all "type" field values in a JSON schema for Gemini compatibility.
    /// e.g. "object" → "OBJECT", "string" → "STRING", etc.
    /// Recursively strip fields not supported by Gemini API schemas.
    /// Gemini only supports: type, description, properties, required, items, enum, format, nullable.
    /// Must remove: additionalProperties, oneOf, anyOf, allOf, not, default, examples, $ref, etc.
    fn sanitize_schema_for_gemini(schema: &Value) -> Value {
        match schema {
            Value::Object(map) => {
                let mut new_map = serde_json::Map::new();
                for (k, v) in map {
                    match k.as_str() {
                        // Keep supported fields
                        "type" | "description" | "required" | "enum" | "format" | "nullable" => {
                            new_map.insert(k.clone(), v.clone());
                        }
                        // Recursively sanitize nested schemas
                        "properties" => {
                            if let Some(props) = v.as_object() {
                                let mut sanitized_props = serde_json::Map::new();
                                for (pk, pv) in props {
                                    sanitized_props
                                        .insert(pk.clone(), Self::sanitize_schema_for_gemini(pv));
                                }
                                new_map.insert(k.clone(), Value::Object(sanitized_props));
                            }
                        }
                        "items" => {
                            new_map.insert(k.clone(), Self::sanitize_schema_for_gemini(v));
                        }
                        // Flatten oneOf/anyOf/allOf at any nesting level
                        "oneOf" | "anyOf" | "allOf" => {
                            if let Some(variants) = v.as_array() {
                                for variant in variants {
                                    if let Some(props) =
                                        variant.get("properties").and_then(|p| p.as_object())
                                    {
                                        let existing = new_map
                                            .entry("properties".to_string())
                                            .or_insert_with(|| json!({}));
                                        if let Some(existing_obj) = existing.as_object_mut() {
                                            for (pk, pv) in props {
                                                existing_obj.insert(
                                                    pk.clone(),
                                                    Self::sanitize_schema_for_gemini(pv),
                                                );
                                            }
                                        }
                                    }
                                }
                                if !new_map.contains_key("type") {
                                    new_map.insert("type".to_string(), json!("object"));
                                }
                            }
                        }
                        // Drop everything else (additionalProperties, not, default, examples, $ref, etc.)
                        _ => {}
                    }
                }
                Value::Object(new_map)
            }
            Value::Array(arr) => Value::Array(
                arr.iter()
                    .map(|v| Self::sanitize_schema_for_gemini(v))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    fn uppercase_types(schema: &Value) -> Value {
        match schema {
            Value::Object(map) => {
                let mut new_map = serde_json::Map::new();
                for (k, v) in map {
                    if k == "type" {
                        if let Some(s) = v.as_str() {
                            new_map.insert(k.clone(), json!(s.to_uppercase()));
                        } else {
                            new_map.insert(k.clone(), Self::uppercase_types(v));
                        }
                    } else {
                        new_map.insert(k.clone(), Self::uppercase_types(v));
                    }
                }
                Value::Object(new_map)
            }
            Value::Array(arr) => {
                Value::Array(arr.iter().map(|v| Self::uppercase_types(v)).collect())
            }
            other => other.clone(),
        }
    }

    /// Parse OpenAI Responses API response into LLMResponse.
    fn parse_openai_response(&self, response: &Value) -> Result<LLMResponse, String> {
        let output = response
            .get("output")
            .and_then(|o| o.as_array())
            .ok_or("No output array in OpenAI response")?;

        let mut content = vec![];
        let mut has_function_call = false;

        for item in output {
            let item_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match item_type {
                "message" => {
                    if let Some(item_content) = item.get("content").and_then(|c| c.as_array()) {
                        for part in item_content {
                            let part_type = part.get("type").and_then(|t| t.as_str()).unwrap_or("");
                            if part_type == "output_text" {
                                if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                                    if !text.is_empty() {
                                        content.push(LLMResponseType::Text {
                                            text: text.to_string(),
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
                "function_call" => {
                    has_function_call = true;
                    let name = item
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    let call_id = item
                        .get("call_id")
                        .and_then(|c| c.as_str())
                        .or_else(|| item.get("id").and_then(|i| i.as_str()))
                        .unwrap_or("tool_0")
                        .to_string();
                    let arguments = item
                        .get("arguments")
                        .and_then(|a| a.as_str())
                        .unwrap_or("{}");
                    let input: Value = serde_json::from_str(arguments).unwrap_or(json!({}));
                    content.push(LLMResponseType::ToolUse {
                        tool_use: ToolUse {
                            id: call_id,
                            name,
                            input,
                            gemini_thought_signature: None,
                        },
                    });
                }
                _ => {
                    log::warn!("[LLM] Unknown OpenAI output item type: {}", item_type);
                }
            }
        }

        let stop_reason = if has_function_call {
            "tool_use"
        } else {
            "end_turn"
        }
        .to_string();

        let usage = response
            .get("usage")
            .map(|u| Usage {
                input_tokens: u.get("input_tokens").and_then(|t| t.as_u64()).unwrap_or(0) as u32,
                output_tokens: u.get("output_tokens").and_then(|t| t.as_u64()).unwrap_or(0) as u32,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
            })
            .unwrap_or(Usage::zero());

        Ok(LLMResponse {
            content,
            stop_reason,
            usage,
        })
    }
}

/// Download an image from a URL and return base64-encoded data.
/// Used for Gemini which only supports inlineData (no URL references).
async fn download_image_as_base64(url: &str) -> Result<String, String> {
    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("Image download failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("Image download returned {}", resp.status()));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("Failed to read image bytes: {}", e))?;

    use base64::Engine;
    Ok(base64::engine::general_purpose::STANDARD.encode(&bytes))
}

/// Merge two content values when consecutive same-role messages need combining
fn merge_contents(a: &Value, b: &Value) -> Value {
    match (a, b) {
        // Both strings: concatenate
        (Value::String(a_str), Value::String(b_str)) => {
            json!(format!("{}\n\n{}", a_str, b_str))
        }
        // Mixed: convert to block arrays and combine
        _ => {
            let mut blocks = to_blocks_value(a);
            blocks.extend(to_blocks_value(b));
            Value::Array(blocks)
        }
    }
}

/// Convert content value to array of content block values
fn to_blocks_value(content: &Value) -> Vec<Value> {
    match content {
        Value::String(s) => vec![json!({"type": "text", "text": s})],
        Value::Array(arr) => arr.clone(),
        _ => vec![],
    }
}

/// Convert a SessionMessage content string back to MessageContent
/// Handles both plain text and BLOCKS: prefixed structured content
pub fn parse_session_content(content: &str) -> MessageContent {
    if let Some(json_str) = content.strip_prefix(BLOCKS_PREFIX) {
        if let Ok(blocks) = serde_json::from_str::<Vec<ContentBlock>>(json_str) {
            return MessageContent::Blocks(blocks);
        }
    }
    MessageContent::Text(content.to_string())
}

/// Serialize MessageContent for storage in SessionMessage
pub fn serialize_content_for_session(content: &MessageContent) -> String {
    match content {
        MessageContent::Text(s) => s.clone(),
        MessageContent::Blocks(blocks) => {
            format!(
                "{}{}",
                BLOCKS_PREFIX,
                serde_json::to_string(blocks).unwrap_or_default()
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_serialization() {
        let msg = Message::user("Hello");
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"role\":\"user\""));
        assert!(json.contains("\"content\":\"Hello\""));
    }

    #[test]
    fn test_content_block_serialization() {
        let blocks = vec![
            ContentBlock::Text {
                text: "Let me check".to_string(),
            },
            ContentBlock::ToolUse {
                id: "toolu_1".to_string(),
                name: "shell".to_string(),
                input: json!({"command": "ls"}),
                gemini_thought_signature: None,
            },
        ];
        let json = serde_json::to_string(&blocks).unwrap();
        assert!(json.contains("\"type\":\"text\""));
        assert!(json.contains("\"type\":\"tool_use\""));
        assert!(json.contains("\"name\":\"shell\""));
    }

    #[test]
    fn test_tool_result_serialization() {
        // Normal result (is_error omitted)
        let block = ContentBlock::ToolResult {
            tool_use_id: "toolu_1".to_string(),
            content: "file1.txt".to_string(),
            is_error: false,
        };
        let json = serde_json::to_string(&block).unwrap();
        assert!(json.contains("\"type\":\"tool_result\""));
        assert!(!json.contains("is_error"));

        // Error result (is_error included)
        let block = ContentBlock::ToolResult {
            tool_use_id: "toolu_1".to_string(),
            content: "command not found".to_string(),
            is_error: true,
        };
        let json = serde_json::to_string(&block).unwrap();
        assert!(json.contains("\"is_error\":true"));
    }

    #[test]
    fn test_message_content_text() {
        let content = MessageContent::text("hello");
        assert_eq!(content.as_text(), "hello");
        let json = serde_json::to_value(&content).unwrap();
        assert_eq!(json, json!("hello"));
    }

    #[test]
    fn test_message_content_blocks() {
        let content = MessageContent::blocks(vec![
            ContentBlock::Text {
                text: "hello".to_string(),
            },
            ContentBlock::Text {
                text: " world".to_string(),
            },
        ]);
        assert_eq!(content.as_text(), "hello world");
    }

    #[test]
    fn test_session_content_roundtrip() {
        let blocks = vec![ContentBlock::ToolResult {
            tool_use_id: "toolu_1".to_string(),
            content: "result".to_string(),
            is_error: false,
        }];
        let content = MessageContent::Blocks(blocks);
        let serialized = serialize_content_for_session(&content);
        assert!(serialized.starts_with(BLOCKS_PREFIX));

        let parsed = parse_session_content(&serialized);
        assert_eq!(parsed.as_text(), ""); // ToolResult has no text
        if let MessageContent::Blocks(parsed_blocks) = parsed {
            assert_eq!(parsed_blocks.len(), 1);
        } else {
            panic!("Expected Blocks");
        }
    }

    #[test]
    fn test_tool_definition() {
        let tool = Tool {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "param1": {"type": "string"}
                },
                "required": ["param1"]
            }),
        };

        assert_eq!(tool.name, "test_tool");
    }
}
