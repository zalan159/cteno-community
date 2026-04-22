//! Session Memory — Incremental extraction system
//!
//! Maintains a structured memory document for each agent session by periodically
//! extracting key information from new messages using cheap/free LLM models.
//! When context compression is needed, the pre-built memory replaces the expensive
//! LLM-based handoff summary, achieving zero-cost compression.

use crate::agent_session::SessionMessage;
use crate::llm::{LLMClient, LLMResponseType, Message};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// Structured session memory maintained by incremental extraction.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionMemory {
    pub title: String,
    pub task_spec: String,
    pub current_state: String,
    pub decisions: Vec<String>,
    pub files_touched: Vec<FileTouched>,
    pub errors_and_fixes: Vec<String>,
    pub key_results: Vec<String>,
    pub remaining_work: Vec<String>,
    pub worklog: Vec<WorklogEntry>,
    pub meta: ExtractionMeta,
}

/// A file that was read/created/modified during the session.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FileTouched {
    pub path: String,
    pub description: String,
}

/// A single worklog entry summarising what happened in one extraction window.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorklogEntry {
    pub timestamp: String,
    pub summary: String,
}

/// Persisted metadata that lets us resume tracking across session reconnects.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExtractionMeta {
    pub last_extracted_message_index: usize,
    pub last_extracted_at_tokens: u32,
    pub tool_calls_since_last: u32,
    pub extraction_count: u32,
}

// ---------------------------------------------------------------------------
// Extraction trigger tracker (in-memory, initialised from ExtractionMeta)
// ---------------------------------------------------------------------------

/// Initial token threshold before the first extraction fires.
const INIT_TOKEN_THRESHOLD: u32 = 10_000;

/// Token growth required between subsequent extractions.
const UPDATE_TOKEN_THRESHOLD: u32 = 5_000;

/// Minimum tool calls required alongside the token threshold.
const TOOL_CALLS_THRESHOLD: u32 = 3;

/// Tiered extraction models (proxy mode). Tried in order; first success wins.
/// (model_id, context_window_tokens)
pub const EXTRACTION_TIERS: &[(&str, usize)] = &[("deepseek-chat", 128_000)];

/// Tracks whether an extraction should fire. Lives on the stack in the ReAct
/// loop and is initialised from the persisted `ExtractionMeta`.
pub struct ExtractionTracker {
    last_extraction_tokens: u32,
    tool_calls_since: u32,
    initialized: bool,
    last_message_index: usize,
}

impl ExtractionTracker {
    /// Restore tracker state from persisted meta (supports session resume).
    pub fn from_meta(meta: Option<&ExtractionMeta>) -> Self {
        match meta {
            Some(m) => Self {
                last_extraction_tokens: m.last_extracted_at_tokens,
                tool_calls_since: m.tool_calls_since_last,
                initialized: m.extraction_count > 0,
                last_message_index: m.last_extracted_message_index,
            },
            None => Self {
                last_extraction_tokens: 0,
                tool_calls_since: 0,
                initialized: false,
                last_message_index: 0,
            },
        }
    }

    /// Record tool calls executed this turn.
    pub fn record_tool_calls(&mut self, count: u32) {
        self.tool_calls_since += count;
    }

    /// Check whether an extraction should be triggered now.
    pub fn should_extract(&self, current_tokens: u32) -> bool {
        if !self.initialized {
            // First extraction: need both token threshold and tool calls
            current_tokens >= INIT_TOKEN_THRESHOLD && self.tool_calls_since >= TOOL_CALLS_THRESHOLD
        } else {
            // Subsequent: need token growth AND tool calls
            let growth = current_tokens.saturating_sub(self.last_extraction_tokens);
            growth >= UPDATE_TOKEN_THRESHOLD && self.tool_calls_since >= TOOL_CALLS_THRESHOLD
        }
    }

    /// Mark that an extraction was dispatched at this point.
    pub fn mark_extracted(&mut self, current_tokens: u32, message_index: usize) {
        self.last_extraction_tokens = current_tokens;
        self.tool_calls_since = 0;
        self.initialized = true;
        self.last_message_index = message_index;
    }

    /// The message index from which the next delta should start.
    pub fn last_message_index(&self) -> usize {
        self.last_message_index
    }
}

// ---------------------------------------------------------------------------
// Extraction LLM calls
// ---------------------------------------------------------------------------

/// Run a single extraction against one model.
pub async fn run_extraction(
    client: &LLMClient,
    model: &str,
    messages: &[SessionMessage],
    existing_memory: Option<&SessionMemory>,
    delta_start: usize,
) -> Result<SessionMemory, String> {
    // Build delta text
    let delta_end = messages.len();
    if delta_start >= delta_end {
        return Err("No new messages to extract from".to_string());
    }

    let delta_text: String = messages[delta_start..delta_end]
        .iter()
        .map(|m| {
            let role = match m.role.as_str() {
                "user" => "User",
                "assistant" => "Assistant",
                _ => &m.role,
            };
            format!("{}: {}", role, m.content)
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    let existing_json = match existing_memory {
        Some(mem) => serde_json::to_string_pretty(mem).unwrap_or_else(|_| "null".to_string()),
        None => "null".to_string(),
    };

    let system_prompt = "You are a session memory extraction agent. You maintain a structured JSON document that captures the essential state of an AI assistant's working session. Be precise, factual, and concise. Output ONLY valid JSON, no markdown fences, no commentary.";

    let prompt = format!(
        r#"CURRENT SESSION MEMORY:
---
{}
---

NEW MESSAGES SINCE LAST UPDATE:
---
{}
---

Merge new information into the existing memory document, updating all sections.
Output ONLY a JSON object with these exact fields:
{{
  "title": "short session title",
  "task_spec": "what the user asked for",
  "current_state": "where things stand right now",
  "decisions": ["key decisions made"],
  "files_touched": [{{"path":"...", "description":"..."}}],
  "errors_and_fixes": ["error encountered and how it was fixed"],
  "key_results": ["important outcomes achieved"],
  "remaining_work": ["what still needs to be done"],
  "worklog_entry": "brief summary of what happened in these new messages"
}}"#,
        existing_json, delta_text
    );

    let llm_messages = vec![Message::user(prompt)];

    let response = client
        .chat_anthropic(
            model,
            system_prompt,
            &llm_messages,
            &[], // no tools
            0.3, // low temperature
            2000,
            None,  // no streaming
            false, // no thinking
        )
        .await
        .map_err(|e| format!("Extraction LLM call failed (model={}): {}", model, e))?;

    // Extract text from response
    let text = response
        .content
        .iter()
        .find_map(|c| match c {
            LLMResponseType::Text { text } => Some(text.clone()),
            _ => None,
        })
        .ok_or_else(|| "No text in extraction response".to_string())?;

    // Parse JSON
    let json_val = parse_extraction_json(&text)?;

    // Build SessionMemory from parsed JSON
    let mut memory = SessionMemory {
        title: json_val
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        task_spec: json_val
            .get("task_spec")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        current_state: json_val
            .get("current_state")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        decisions: json_array_to_strings(json_val.get("decisions")),
        files_touched: json_val
            .get("files_touched")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        Some(FileTouched {
                            path: item.get("path")?.as_str()?.to_string(),
                            description: item
                                .get("description")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
        errors_and_fixes: json_array_to_strings(json_val.get("errors_and_fixes")),
        key_results: json_array_to_strings(json_val.get("key_results")),
        remaining_work: json_array_to_strings(json_val.get("remaining_work")),
        worklog: existing_memory
            .map(|m| m.worklog.clone())
            .unwrap_or_default(),
        meta: ExtractionMeta {
            last_extracted_message_index: delta_end,
            last_extracted_at_tokens: existing_memory
                .map(|m| m.meta.last_extracted_at_tokens)
                .unwrap_or(0),
            tool_calls_since_last: 0,
            extraction_count: existing_memory
                .map(|m| m.meta.extraction_count + 1)
                .unwrap_or(1),
        },
    };

    // Append new worklog entry
    if let Some(entry_text) = json_val.get("worklog_entry").and_then(|v| v.as_str()) {
        if !entry_text.is_empty() {
            memory.worklog.push(WorklogEntry {
                timestamp: chrono::Utc::now().to_rfc3339(),
                summary: entry_text.to_string(),
            });
        }
    }

    Ok(memory)
}

/// Try extraction with fallback across tiers (proxy) or a single model (BYOK).
pub async fn run_extraction_with_fallback(
    client: &LLMClient,
    is_proxy: bool,
    profile_compress_model: Option<&str>,
    messages: &[SessionMessage],
    existing_memory: Option<&SessionMemory>,
    delta_start: usize,
) -> Result<SessionMemory, String> {
    if is_proxy {
        // Proxy mode: try each tier in order
        let mut last_err = String::new();
        for &(model, _ctx_window) in EXTRACTION_TIERS {
            match run_extraction(client, model, messages, existing_memory, delta_start).await {
                Ok(memory) => {
                    log::info!(
                        "[SessionMemory] Extraction succeeded with model '{}'",
                        model
                    );
                    return Ok(memory);
                }
                Err(e) => {
                    log::warn!(
                        "[SessionMemory] Extraction failed with model '{}': {}, trying next tier",
                        model,
                        e
                    );
                    last_err = e;
                }
            }
        }
        Err(format!(
            "All extraction tiers failed. Last error: {}",
            last_err
        ))
    } else {
        // BYOK mode: use the profile's compress model
        let model = profile_compress_model.unwrap_or("deepseek-chat");
        run_extraction(client, model, messages, existing_memory, delta_start).await
    }
}

// ---------------------------------------------------------------------------
// JSON parsing helpers
// ---------------------------------------------------------------------------

/// Parse extraction JSON from LLM output. Tries:
/// 1. Direct parse
/// 2. Extract from ```json ... ``` code blocks
/// 3. Find outermost { ... } braces
pub fn parse_extraction_json(text: &str) -> Result<serde_json::Value, String> {
    // Try 1: direct parse
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(text) {
        return Ok(val);
    }

    // Try 2: extract from ```json code block
    if let Some(start) = text.find("```json") {
        let after_fence = &text[start + 7..];
        if let Some(end) = after_fence.find("```") {
            let json_str = after_fence[..end].trim();
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
                return Ok(val);
            }
        }
    }

    // Try 2b: extract from ``` code block (no language tag)
    if let Some(start) = text.find("```\n") {
        let after_fence = &text[start + 4..];
        if let Some(end) = after_fence.find("```") {
            let json_str = after_fence[..end].trim();
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
                return Ok(val);
            }
        }
    }

    // Try 3: find outermost braces
    if let Some(open) = text.find('{') {
        if let Some(close) = text.rfind('}') {
            if close > open {
                let json_str = &text[open..=close];
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
                    return Ok(val);
                }
            }
        }
    }

    Err(format!(
        "Failed to parse extraction JSON from LLM output: {}",
        &text[..text.len().min(200)]
    ))
}

/// Helper: convert a JSON array of strings to Vec<String>.
fn json_array_to_strings(val: Option<&serde_json::Value>) -> Vec<String> {
    val.and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render a SessionMemory as human-readable markdown for injection into
/// the compressed message history.
pub fn render_as_markdown(memory: &SessionMemory) -> String {
    let mut out = String::new();

    if !memory.title.is_empty() {
        out.push_str(&format!("# {}\n\n", memory.title));
    }

    if !memory.task_spec.is_empty() {
        out.push_str(&format!("## Task\n{}\n\n", memory.task_spec));
    }

    if !memory.current_state.is_empty() {
        out.push_str(&format!("## Current State\n{}\n\n", memory.current_state));
    }

    if !memory.decisions.is_empty() {
        out.push_str("## Decisions\n");
        for d in &memory.decisions {
            out.push_str(&format!("- {}\n", d));
        }
        out.push('\n');
    }

    if !memory.files_touched.is_empty() {
        out.push_str("## Files Touched\n");
        for f in &memory.files_touched {
            out.push_str(&format!("- `{}`: {}\n", f.path, f.description));
        }
        out.push('\n');
    }

    if !memory.errors_and_fixes.is_empty() {
        out.push_str("## Errors & Fixes\n");
        for e in &memory.errors_and_fixes {
            out.push_str(&format!("- {}\n", e));
        }
        out.push('\n');
    }

    if !memory.key_results.is_empty() {
        out.push_str("## Key Results\n");
        for r in &memory.key_results {
            out.push_str(&format!("- {}\n", r));
        }
        out.push('\n');
    }

    if !memory.remaining_work.is_empty() {
        out.push_str("## Remaining Work\n");
        for w in &memory.remaining_work {
            out.push_str(&format!("- {}\n", w));
        }
        out.push('\n');
    }

    if !memory.worklog.is_empty() {
        out.push_str("## Worklog\n");
        for entry in &memory.worklog {
            out.push_str(&format!("- [{}] {}\n", entry.timestamp, entry.summary));
        }
        out.push('\n');
    }

    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- ExtractionTracker ---

    #[test]
    fn test_tracker_from_none_requires_init_threshold() {
        let mut tracker = ExtractionTracker::from_meta(None);
        // Not enough tokens
        tracker.record_tool_calls(5);
        assert!(!tracker.should_extract(5_000));
        // Enough tokens but not enough tool calls
        let tracker2 = ExtractionTracker::from_meta(None);
        assert!(!tracker2.should_extract(15_000));
        // Both conditions met
        assert!(tracker.should_extract(10_000));
    }

    #[test]
    fn test_tracker_from_meta_requires_update_threshold() {
        let meta = ExtractionMeta {
            last_extracted_message_index: 10,
            last_extracted_at_tokens: 20_000,
            tool_calls_since_last: 0,
            extraction_count: 1,
        };
        let mut tracker = ExtractionTracker::from_meta(Some(&meta));
        assert_eq!(tracker.last_message_index(), 10);

        // Not enough growth
        tracker.record_tool_calls(5);
        assert!(!tracker.should_extract(23_000));

        // Enough growth + tool calls
        assert!(tracker.should_extract(25_000));
    }

    #[test]
    fn test_tracker_mark_extracted_resets_counters() {
        let mut tracker = ExtractionTracker::from_meta(None);
        tracker.record_tool_calls(5);
        assert!(tracker.should_extract(10_000));

        tracker.mark_extracted(10_000, 20);
        assert_eq!(tracker.last_message_index(), 20);
        // After marking, tool_calls reset so should_extract is false
        assert!(!tracker.should_extract(15_000));

        // Add more tool calls and token growth
        tracker.record_tool_calls(3);
        assert!(tracker.should_extract(15_000));
    }

    #[test]
    fn test_tracker_initialized_flag() {
        let mut tracker = ExtractionTracker::from_meta(None);
        assert!(!tracker.initialized);
        tracker.mark_extracted(10_000, 5);
        assert!(tracker.initialized);
    }

    // --- parse_extraction_json ---

    #[test]
    fn test_parse_direct_json() {
        let input = r#"{"title":"test","task_spec":"do stuff"}"#;
        let val = parse_extraction_json(input).unwrap();
        assert_eq!(val["title"], "test");
    }

    #[test]
    fn test_parse_json_in_code_block() {
        let input = "Here is the result:\n```json\n{\"title\":\"test\"}\n```\nDone.";
        let val = parse_extraction_json(input).unwrap();
        assert_eq!(val["title"], "test");
    }

    #[test]
    fn test_parse_json_in_plain_code_block() {
        let input = "Result:\n```\n{\"title\":\"plain\"}\n```";
        let val = parse_extraction_json(input).unwrap();
        assert_eq!(val["title"], "plain");
    }

    #[test]
    fn test_parse_json_from_braces() {
        let input = "Some preamble text {\"title\":\"braces\"} and trailing text";
        let val = parse_extraction_json(input).unwrap();
        assert_eq!(val["title"], "braces");
    }

    #[test]
    fn test_parse_json_fails_on_garbage() {
        let input = "no json here at all";
        assert!(parse_extraction_json(input).is_err());
    }

    // --- render_as_markdown ---

    #[test]
    fn test_render_empty_memory() {
        let mem = SessionMemory::default();
        let md = render_as_markdown(&mem);
        assert!(md.is_empty() || md.trim().is_empty());
    }

    #[test]
    fn test_render_full_memory() {
        let mem = SessionMemory {
            title: "Test Session".to_string(),
            task_spec: "Build a web server".to_string(),
            current_state: "Server is running".to_string(),
            decisions: vec!["Use axum framework".to_string()],
            files_touched: vec![FileTouched {
                path: "src/main.rs".to_string(),
                description: "Entry point".to_string(),
            }],
            errors_and_fixes: vec!["Fixed missing import".to_string()],
            key_results: vec!["Server responds on /health".to_string()],
            remaining_work: vec!["Add /echo route".to_string()],
            worklog: vec![WorklogEntry {
                timestamp: "2026-01-01T00:00:00Z".to_string(),
                summary: "Created initial server".to_string(),
            }],
            meta: ExtractionMeta::default(),
        };

        let md = render_as_markdown(&mem);
        assert!(md.contains("# Test Session"));
        assert!(md.contains("## Task"));
        assert!(md.contains("Build a web server"));
        assert!(md.contains("## Current State"));
        assert!(md.contains("## Decisions"));
        assert!(md.contains("Use axum framework"));
        assert!(md.contains("## Files Touched"));
        assert!(md.contains("`src/main.rs`"));
        assert!(md.contains("## Errors & Fixes"));
        assert!(md.contains("## Key Results"));
        assert!(md.contains("## Remaining Work"));
        assert!(md.contains("## Worklog"));
        assert!(md.contains("Created initial server"));
    }
}
