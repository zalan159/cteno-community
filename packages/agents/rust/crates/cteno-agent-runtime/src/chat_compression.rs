//! Chat Compression Service
//!
//! Implements conversation compression using LLM-generated "handoff summaries".
//! When conversation history approaches context limits, the full history is
//! summarized into a handoff document, and recent user messages are preserved
//! to maintain task continuity.

use crate::agent_session::SessionMessage;
use crate::llm::{LLMClient, Message};

/// Token budget for preserved user messages after compression (~20K tokens).
const PRESERVED_USER_MESSAGES_TOKEN_BUDGET: usize = 80_000; // chars (~20K tokens at 4 chars/token)

/// Chars per token for rough estimation (consistent with autonomous_agent.rs).
const CHARS_PER_TOKEN: usize = 4;

/// Maximum fraction of compression model's context window to use for the summarization prompt.
/// Leave headroom for the system prompt, instruction text, and the summary output.
const COMPRESS_MODEL_INPUT_FRACTION: f32 = 0.7;

/// Tiered compression models — selected by estimated input size.
/// Each tier: (model_id, context_window_tokens, max_input_tokens_for_this_tier)
///   - max_input = context_window * COMPRESS_MODEL_INPUT_FRACTION
const COMPRESS_TIERS: &[(&str, usize)] = &[
    ("deepseek-chat", 128_000),           // DeepSeek V3, input < ~90K tokens
    ("minimax/minimax-m2.5", 200_000),    // MiniMax M2.5, input < ~140K tokens
    ("bailian/qwen3.5-flash", 1_000_000), // 千问 3.5 Flash, input < ~700K tokens
];

/// Prefixes used by summary messages (old and new formats). These are skipped when
/// selecting user messages to preserve.
const SUMMARY_PREFIXES: &[&str] = &[
    "[Conversation Summary",
    "[Handoff Summary]",
    "[Session Memory]",
];

/// Compression configuration
pub struct CompressionConfig {
    /// Minimum messages before compression kicks in (pre-loop safety net)
    pub min_messages_for_compression: usize,
    /// Model context window size in tokens
    pub context_window_tokens: u32,
    /// Compress when input_tokens exceeds this fraction of context window (e.g. 0.5 = 50%)
    pub token_threshold_fraction: f32,
    /// Model to use for compression
    pub compression_model: String,
    /// Maximum tokens for compression response
    pub max_compression_tokens: u32,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            min_messages_for_compression: 20,
            // DeepSeek supports a 128k context window (prompt + completion).
            // We trigger compression at 50% usage to preserve headroom.
            context_window_tokens: 128_000,
            token_threshold_fraction: 0.8, // Compress at 80% => ~102k
            compression_model: "deepseek-chat".to_string(),
            max_compression_tokens: 2000,
        }
    }
}

/// Map model name to context window size in tokens
fn context_window_for_model(model: &str) -> u32 {
    match model {
        m if m.contains("kimi") => 200_000,
        m if m.contains("minimax") => 200_000,
        m if m.contains("qwen") => 200_000,
        m if m.contains("glm-") => 200_000,
        m if m.contains("deepseek") => 128_000,
        m if m.contains("claude") => 200_000,
        m if m.contains("gpt-4") => 128_000,
        _ => 128_000,
    }
}

/// Compression Service
pub struct CompressionService {
    pub config: CompressionConfig,
}

impl CompressionService {
    /// Create new compression service
    pub fn new(config: CompressionConfig) -> Self {
        Self { config }
    }

    /// Create with default config
    pub fn default_service() -> Self {
        Self::new(CompressionConfig::default())
    }

    /// Create with context window auto-detected from model name
    pub fn for_model(model: &str) -> Self {
        Self::for_model_with_context_window(model, None)
    }

    /// Create with model-based defaults, allowing an explicit context window override.
    pub fn for_model_with_context_window(model: &str, context_window_tokens: Option<u32>) -> Self {
        let resolved_context_window = context_window_tokens
            .filter(|v| *v > 0)
            .unwrap_or_else(|| context_window_for_model(model));
        Self::new(CompressionConfig {
            context_window_tokens: resolved_context_window,
            ..Default::default()
        })
    }

    /// Check if compression is needed (message-count based, pre-loop safety net)
    pub fn needs_compression(&self, history: &[SessionMessage]) -> bool {
        history.len() >= self.config.min_messages_for_compression
    }

    /// Check if compression is needed based on actual token usage from LLM API response.
    /// This is the primary trigger, using real token counts rather than message count heuristics.
    /// Follows Gemini CLI's approach: compress when prompt tokens exceed N% of context window.
    pub fn needs_compression_by_tokens(&self, input_tokens: u32) -> bool {
        let threshold = (self.config.context_window_tokens as f32
            * self.config.token_threshold_fraction) as u32;
        let needed = input_tokens >= threshold;
        if needed {
            log::info!(
                "[Compression] Token threshold hit: {} input tokens >= {} threshold ({}% of {} window)",
                input_tokens, threshold,
                (self.config.token_threshold_fraction * 100.0) as u32,
                self.config.context_window_tokens
            );
        }
        needed
    }

    /// Get the token threshold value
    pub fn token_threshold(&self) -> u32 {
        (self.config.context_window_tokens as f32 * self.config.token_threshold_fraction) as u32
    }

    /// Compress conversation history using a handoff summary approach.
    ///
    /// Strategy:
    /// 1. Send ALL messages to LLM for a handoff summary
    /// 2. Select recent user messages backwards (within token budget)
    /// 3. New history = [selected user messages in chronological order] + [handoff summary]
    pub async fn compress_history(
        &self,
        client: &LLMClient,
        history: &[SessionMessage],
    ) -> Result<Vec<SessionMessage>, String> {
        if history.len() < self.config.min_messages_for_compression {
            return Ok(history.to_vec());
        }

        // Generate handoff summary (internally truncates and selects compression tier)
        let summary = self.generate_handoff_summary(client, history).await?;

        // Select user messages backwards within token budget
        let preserved_users = Self::select_user_messages_backwards(history);

        // Build new history: preserved user messages (chronological) + handoff summary
        let mut compressed_history = Vec::new();
        compressed_history.extend(preserved_users);
        compressed_history.push(SessionMessage {
            role: "user".to_string(),
            content: format!(
                "[Handoff Summary]\nAnother agent session was working on this task. Here is their summary:\n\n{}\n\nContinue from where they left off.",
                summary
            ),
            timestamp: chrono::Utc::now().to_rfc3339(),
        local_id: None,
        });

        log::info!(
            "[Compression] Compressed {} messages → {} (handoff summary + {} preserved user msgs)",
            history.len(),
            compressed_history.len(),
            compressed_history.len() - 1
        );

        Ok(compressed_history)
    }

    /// Zero-cost compression using a pre-built Session Memory markdown.
    /// No LLM call needed — the memory was already maintained incrementally in the background.
    ///
    /// Preserves recent user messages (same logic as `compress_history`) and appends
    /// a synthetic user message containing the rendered session memory markdown.
    pub fn compress_with_session_memory(
        &self,
        memory_markdown: &str,
        history: &[SessionMessage],
    ) -> Vec<SessionMessage> {
        let preserved_users = Self::select_user_messages_backwards(history);

        let mut compressed = Vec::new();
        compressed.extend(preserved_users);
        compressed.push(SessionMessage {
            role: "user".to_string(),
            content: format!(
                "[Session Memory]\n\
                This session is being continued from a previous conversation that ran out of context.\n\
                Below is the incrementally maintained session memory:\n\n{}\n\n\
                Continue from where you left off without asking further questions.",
                memory_markdown
            ),
            timestamp: chrono::Utc::now().to_rfc3339(),
        local_id: None,
        });

        log::info!(
            "[Compression] Session memory compression: {} → {} messages (zero LLM cost)",
            history.len(),
            compressed.len()
        );

        compressed
    }

    /// Select user messages backwards from history, skipping assistant/tool_result/summary
    /// messages, within a token budget.
    fn select_user_messages_backwards(history: &[SessionMessage]) -> Vec<SessionMessage> {
        let mut selected: Vec<SessionMessage> = Vec::new();
        let mut budget_remaining = PRESERVED_USER_MESSAGES_TOKEN_BUDGET;

        for msg in history.iter().rev() {
            // Only keep user messages that are actual user intent
            if msg.role != "user" {
                continue;
            }

            // Skip tool_result blocks (serialized as "BLOCKS:[...]")
            if msg.content.starts_with("BLOCKS:[") {
                continue;
            }

            // Skip old summary messages
            if SUMMARY_PREFIXES.iter().any(|p| msg.content.starts_with(p)) {
                continue;
            }

            // Skip old-format tool markers
            if msg.content.starts_with("[Tool:") || msg.content.starts_with("[Tool Result:") {
                continue;
            }

            let msg_chars = msg.content.len();
            if msg_chars > budget_remaining {
                break;
            }

            budget_remaining -= msg_chars;
            selected.push(msg.clone());
        }

        // Reverse to chronological order
        selected.reverse();
        selected
    }

    /// Truncate long messages to reduce token count before summarization
    fn truncate_long_messages(messages: &[SessionMessage]) -> Vec<SessionMessage> {
        messages
            .iter()
            .map(|msg| {
                if msg.content.len() > 1000 {
                    // Truncate very long messages (find a valid char boundary)
                    let truncate_at = msg
                        .content
                        .char_indices()
                        .take_while(|&(i, _)| i <= 1000)
                        .last()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    SessionMessage {
                        role: msg.role.clone(),
                        content: format!(
                            "{}... [truncated, {} chars total]",
                            &msg.content[..truncate_at],
                            msg.content.len()
                        ),
                        timestamp: msg.timestamp.clone(),
                        local_id: None,
                    }
                } else {
                    msg.clone()
                }
            })
            .collect()
    }

    /// Select the best compression model tier based on estimated input tokens.
    ///
    /// Tiers (cheapest first):
    ///   1. deepseek-chat (128K)
    ///   2. minimax-m2.5 (200K)
    ///   3. qwen3.5-flash (1M)
    ///
    /// Returns (model_id, max_conversation_chars).
    fn select_compress_tier(estimated_input_tokens: usize) -> (&'static str, usize) {
        for &(model_id, context_window) in COMPRESS_TIERS {
            let max_input_tokens = (context_window as f32 * COMPRESS_MODEL_INPUT_FRACTION) as usize;
            if estimated_input_tokens <= max_input_tokens {
                let max_chars = max_input_tokens * CHARS_PER_TOKEN;
                return (model_id, max_chars);
            }
        }
        // All tiers exceeded — use the largest tier's budget (will be truncated)
        let &(model_id, context_window) = COMPRESS_TIERS.last().unwrap();
        let max_chars =
            (context_window as f32 * COMPRESS_MODEL_INPUT_FRACTION) as usize * CHARS_PER_TOKEN;
        (model_id, max_chars)
    }

    /// Generate a handoff summary using LLM.
    ///
    /// The prompt is designed for another LLM to resume the task seamlessly,
    /// following the "handoff summary" pattern from OpenAI Codex CLI.
    ///
    /// Automatically selects the cheapest compression model that can fit the input:
    ///   - deepseek-chat (128K) → minimax-m2.5 (200K) → qwen3.5-flash (1M)
    async fn generate_handoff_summary(
        &self,
        client: &LLMClient,
        messages: &[SessionMessage],
    ) -> Result<String, String> {
        // Estimate total input tokens from truncated messages
        let truncated = Self::truncate_long_messages(messages);
        let estimated_tokens: usize =
            truncated.iter().map(|m| m.content.len()).sum::<usize>() / CHARS_PER_TOKEN;

        // Select the right compression model tier
        let (compress_model, max_conversation_chars) = Self::select_compress_tier(estimated_tokens);

        log::info!(
            "[Compression] Selecting compress model '{}' for ~{} estimated tokens (budget: {} chars)",
            compress_model,
            estimated_tokens,
            max_conversation_chars
        );

        // Build conversation text within the selected tier's budget
        let conversation_text =
            Self::build_conversation_text_within_budget(&truncated, max_conversation_chars);

        let summary_prompt = format!(
            r#"You are creating a handoff summary for another language model that will resume this task.

Write a concise summary that includes:
1. **Original goal**: What was the user trying to accomplish?
2. **Decisions made**: Key choices, approaches selected, trade-offs considered
3. **What's been accomplished**: Completed steps, files modified, tools used successfully
4. **Current state**: Where things stand right now
5. **What remains**: Outstanding tasks or next steps
6. **Gotchas**: Any issues encountered, workarounds applied, or things to watch out for

Be factual and specific. Include file paths, function names, and error messages where relevant.
Do NOT include pleasantries or meta-commentary. Just the facts.

Conversation to summarize:
---
{}
---"#,
            conversation_text
        );

        let llm_messages = vec![Message::user(summary_prompt)];

        // Call LLM for summary using the selected tier model
        match client
            .chat_anthropic(
                compress_model,
                "You create concise handoff summaries for language model task continuity.",
                &llm_messages,
                &[], // No tools needed
                0.3, // Low temperature for consistency
                self.config.max_compression_tokens,
                None,  // No streaming for compression
                false, // No thinking for compression
            )
            .await
        {
            Ok(response) => {
                // Extract text from response
                for content in response.content {
                    if let crate::llm::LLMResponseType::Text { text } = content {
                        log::info!(
                            "[Compression] Summary generated via '{}' ({} chars)",
                            compress_model,
                            text.len()
                        );
                        return Ok(text);
                    }
                }
                Err("No text in compression response".to_string())
            }
            Err(e) => Err(format!(
                "Compression LLM call failed (model={}): {}",
                compress_model, e
            )),
        }
    }

    /// Build conversation text from messages, keeping only the most recent messages
    /// that fit within `max_chars`. This prevents the summarization prompt from
    /// exceeding the compression model's context window.
    fn build_conversation_text_within_budget(
        messages: &[SessionMessage],
        max_chars: usize,
    ) -> String {
        // First try: build full text
        let format_msg = |m: &SessionMessage| -> String {
            let role = match m.role.as_str() {
                "user" => "User",
                "assistant" => "Assistant",
                _ => &m.role,
            };
            format!("{}: {}", role, m.content)
        };

        let full_text: String = messages
            .iter()
            .map(|m| format_msg(m))
            .collect::<Vec<_>>()
            .join("\n\n");

        if full_text.len() <= max_chars {
            return full_text;
        }

        // Full text exceeds budget — take recent messages that fit
        log::info!(
            "[Compression] Conversation text ({} chars) exceeds budget ({} chars), taking recent messages only",
            full_text.len(),
            max_chars
        );

        let mut selected: Vec<String> = Vec::new();
        let mut total_chars = 0usize;

        for msg in messages.iter().rev() {
            let formatted = format_msg(msg);
            let entry_len = formatted.len() + 2; // +2 for "\n\n" separator
            if total_chars + entry_len > max_chars {
                break;
            }
            total_chars += entry_len;
            selected.push(formatted);
        }

        selected.reverse();

        let prefix = format!(
            "[Note: Earlier conversation history ({} messages) was omitted to fit within context limits]\n\n",
            messages.len() - selected.len()
        );
        format!("{}{}", prefix, selected.join("\n\n"))
    }

    /// Hard-truncate session history when LLM compression is not possible
    /// (e.g., compression model also can't handle the volume).
    ///
    /// Strategy: keep only recent user messages within a token budget derived
    /// from the target model's context window, plus a truncation notice.
    /// This is a last-resort fallback — loses assistant context but prevents
    /// API errors from context overflow.
    pub fn hard_truncate_history(
        history: &[SessionMessage],
        target_context_window_tokens: u32,
    ) -> Vec<SessionMessage> {
        // Use 30% of target context window as budget for preserved messages
        let budget_chars = (target_context_window_tokens as f32 * 0.3) as usize * CHARS_PER_TOKEN;
        let mut preserved = Self::select_user_messages_backwards_with_budget(history, budget_chars);

        // Prepend a truncation notice
        preserved.insert(
            0,
            SessionMessage {
                role: "user".to_string(),
                content: format!(
                    "[Context Truncated]\n\
                    The conversation history was truncated because the model was switched to one with a smaller context window ({:.0}K tokens).\n\
                    {} earlier messages were discarded. Only recent user messages are preserved below.\n\
                    If you need context from earlier in the conversation, please ask the user to provide it again.",
                    target_context_window_tokens as f64 / 1000.0,
                    history.len()
                ),
                timestamp: chrono::Utc::now().to_rfc3339(),
            local_id: None,
            },
        );

        log::warn!(
            "[Compression] Hard truncation: {} → {} messages (target context: {}K tokens)",
            history.len(),
            preserved.len(),
            target_context_window_tokens / 1000
        );

        preserved
    }

    /// Select user messages backwards within a custom budget (in chars).
    fn select_user_messages_backwards_with_budget(
        history: &[SessionMessage],
        budget_chars: usize,
    ) -> Vec<SessionMessage> {
        let mut selected: Vec<SessionMessage> = Vec::new();
        let mut budget_remaining = budget_chars;

        for msg in history.iter().rev() {
            if msg.role != "user" {
                continue;
            }
            if msg.content.starts_with("BLOCKS:[") {
                continue;
            }
            if SUMMARY_PREFIXES.iter().any(|p| msg.content.starts_with(p)) {
                continue;
            }
            if msg.content.starts_with("[Tool:") || msg.content.starts_with("[Tool Result:") {
                continue;
            }
            if msg.content.starts_with("[Context Truncated]") {
                continue;
            }

            let msg_chars = msg.content.len();
            if msg_chars > budget_remaining {
                break;
            }

            budget_remaining -= msg_chars;
            selected.push(msg.clone());
        }

        selected.reverse();
        selected
    }

    /// Estimate token count (rough approximation)
    pub fn estimate_tokens(history: &[SessionMessage]) -> usize {
        let total_chars: usize = history.iter().map(|m| m.content.len()).sum();
        total_chars / CHARS_PER_TOKEN
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_needs_compression() {
        let service = CompressionService::default_service();

        // Not enough messages
        let short_history: Vec<SessionMessage> = (0..10)
            .map(|i| SessionMessage {
                role: "user".to_string(),
                content: format!("Message {}", i),
                timestamp: chrono::Utc::now().to_rfc3339(),
                local_id: None,
            })
            .collect();
        assert!(!service.needs_compression(&short_history));

        // Enough messages
        let long_history: Vec<SessionMessage> = (0..25)
            .map(|i| SessionMessage {
                role: "user".to_string(),
                content: format!("Message {}", i),
                timestamp: chrono::Utc::now().to_rfc3339(),
                local_id: None,
            })
            .collect();
        assert!(service.needs_compression(&long_history));
    }

    #[test]
    fn test_truncate_long_messages() {
        let long_content = "x".repeat(2000);
        let messages = vec![SessionMessage {
            role: "assistant".to_string(),
            content: long_content.clone(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            local_id: None,
        }];

        let truncated = CompressionService::truncate_long_messages(&messages);
        assert!(truncated[0].content.len() < long_content.len());
        assert!(truncated[0].content.contains("truncated"));
    }

    #[test]
    fn test_estimate_tokens() {
        let messages = vec![
            SessionMessage {
                role: "user".to_string(),
                content: "Hello, how are you?".to_string(), // ~20 chars
                timestamp: chrono::Utc::now().to_rfc3339(),
                local_id: None,
            },
            SessionMessage {
                role: "assistant".to_string(),
                content: "I'm doing well, thank you for asking!".to_string(), // ~40 chars
                timestamp: chrono::Utc::now().to_rfc3339(),
                local_id: None,
            },
        ];

        let tokens = CompressionService::estimate_tokens(&messages);
        assert!(tokens > 0);
        assert!(tokens < 100); // Should be around 15 tokens
    }

    #[test]
    fn test_kimi_context_window_and_threshold() {
        assert_eq!(context_window_for_model("moonshotai/kimi-k2.5"), 200_000);
        let service = CompressionService::for_model("moonshotai/kimi-k2.5");
        assert_eq!(service.token_threshold(), 160_000);
    }

    #[test]
    fn test_explicit_context_window_override() {
        let service =
            CompressionService::for_model_with_context_window("unknown-model", Some(64_000));
        assert_eq!(service.token_threshold(), 51_200);
    }

    #[test]
    fn test_select_user_messages_backwards() {
        let history = vec![
            SessionMessage {
                role: "user".to_string(),
                content: "Please help me fix the bug".to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                local_id: None,
            },
            SessionMessage {
                role: "assistant".to_string(),
                content: "I'll look into it".to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                local_id: None,
            },
            SessionMessage {
                role: "user".to_string(),
                content: "BLOCKS:[{\"type\":\"tool_result\"}]".to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                local_id: None,
            },
            SessionMessage {
                role: "user".to_string(),
                content: "[Conversation Summary - 10 messages compressed]\nOld summary".to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                local_id: None,
            },
            SessionMessage {
                role: "user".to_string(),
                content: "Now try a different approach".to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                local_id: None,
            },
        ];

        let selected = CompressionService::select_user_messages_backwards(&history);

        // Should select only genuine user messages: "Please help me fix the bug" and "Now try a different approach"
        assert_eq!(selected.len(), 2);
        assert!(selected[0].content.contains("fix the bug"));
        assert!(selected[1].content.contains("different approach"));
    }

    #[test]
    fn test_select_compress_tier() {
        // Small input → deepseek-chat (128K)
        let (model, _) = CompressionService::select_compress_tier(50_000);
        assert_eq!(model, "deepseek-chat");

        // Medium input (> 128K * 0.7 = 89.6K) → minimax-m2.5
        let (model, _) = CompressionService::select_compress_tier(100_000);
        assert_eq!(model, "minimax/minimax-m2.5");

        // Large input (> 200K * 0.7 = 140K) → qwen3.5-flash
        let (model, _) = CompressionService::select_compress_tier(150_000);
        assert_eq!(model, "bailian/qwen3.5-flash");

        // Very large input (> 1M * 0.7 = 700K) → still qwen3.5-flash (largest tier)
        let (model, _) = CompressionService::select_compress_tier(800_000);
        assert_eq!(model, "bailian/qwen3.5-flash");
    }

    #[test]
    fn test_select_user_messages_skips_handoff_summary() {
        let history = vec![
            SessionMessage {
                role: "user".to_string(),
                content: "[Handoff Summary]\nAnother agent session was working...".to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                local_id: None,
            },
            SessionMessage {
                role: "user".to_string(),
                content: "Continue with step 3".to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                local_id: None,
            },
        ];

        let selected = CompressionService::select_user_messages_backwards(&history);
        assert_eq!(selected.len(), 1);
        assert!(selected[0].content.contains("step 3"));
    }

    #[test]
    fn test_compress_with_session_memory() {
        let service = CompressionService::default_service();
        let history = vec![
            SessionMessage {
                role: "user".to_string(),
                content: "Build a web server".to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                local_id: None,
            },
            SessionMessage {
                role: "assistant".to_string(),
                content: "I'll create the server for you.".to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                local_id: None,
            },
            SessionMessage {
                role: "user".to_string(),
                content: "Add a /health route".to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                local_id: None,
            },
        ];

        let memory_md = "# Test\n## Task\nBuild a web server\n";
        let compressed = service.compress_with_session_memory(memory_md, &history);

        // Should have preserved user messages + session memory message
        assert!(compressed.len() >= 2);
        let last = compressed.last().unwrap();
        assert!(last.content.starts_with("[Session Memory]"));
        assert!(last.content.contains("Build a web server"));
        assert!(last.content.contains("Continue from where you left off"));
    }

    #[test]
    fn test_compress_with_session_memory_skips_old_summaries() {
        let service = CompressionService::default_service();
        let history = vec![
            SessionMessage {
                role: "user".to_string(),
                content: "[Session Memory]\nOld memory content".to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                local_id: None,
            },
            SessionMessage {
                role: "user".to_string(),
                content: "Do something new".to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                local_id: None,
            },
        ];

        let compressed = service.compress_with_session_memory("new memory", &history);
        // The old [Session Memory] message should be skipped by select_user_messages_backwards
        let user_msgs: Vec<_> = compressed
            .iter()
            .filter(|m| !m.content.starts_with("[Session Memory]\nThis session"))
            .collect();
        assert_eq!(user_msgs.len(), 1);
        assert!(user_msgs[0].content.contains("Do something new"));
    }

    #[test]
    fn test_select_user_messages_skips_session_memory() {
        let history = vec![
            SessionMessage {
                role: "user".to_string(),
                content: "[Session Memory]\nSome old session memory...".to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                local_id: None,
            },
            SessionMessage {
                role: "user".to_string(),
                content: "Real user message".to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                local_id: None,
            },
        ];

        let selected = CompressionService::select_user_messages_backwards(&history);
        assert_eq!(selected.len(), 1);
        assert!(selected[0].content.contains("Real user message"));
    }
}
