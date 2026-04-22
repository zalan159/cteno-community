//! LLM Edit Fixer
//!
//! Uses LLM to automatically correct failed edit operations by analyzing
//! the instruction, failed parameters, and current file content.

use crate::llm::LLMClient;
use serde::{Deserialize, Serialize};

const EDIT_SYS_PROMPT: &str = r#"
You are an expert code-editing assistant specializing in debugging failed search-and-replace operations.

# Primary Goal
Analyze a failed edit and provide a corrected `search` string that matches the file precisely.
The correction should be minimal, staying close to the original failed `search` string.
Do NOT invent a new edit; fix the provided parameters.

# Rules
1. **Minimal Correction** - Fix whitespace, indentation, line endings, or small context differences
2. **Explain the Fix** - State exactly why original failed and how new search resolves it
3. **Preserve replace** - Do NOT modify `replace` unless instruction requires it
4. **No Changes Case** - Set `no_changes_required` to true if file already satisfies instruction
5. **Exactness** - Final `search` must be EXACT literal text from file (no escaping)
"#;

#[derive(Debug, Serialize, Deserialize)]
pub struct SearchReplaceEdit {
    pub search: String,
    pub replace: String,
    #[serde(default)]
    pub no_changes_required: bool,
    pub explanation: String,
}

pub async fn fix_edit_with_instruction(
    instruction: &str,
    old_string: &str,
    new_string: &str,
    error: &str,
    current_content: &str,
    llm_client: &LLMClient,
) -> Result<Option<SearchReplaceEdit>, String> {
    let user_prompt = format!(
        r#"# Goal of the Original Edit
<instruction>
{instruction}
</instruction>

# Failed Attempt Details
- **Original `search` parameter (failed):**
<search>
{old_string}
</search>

- **Original `replace` parameter:**
<replace>
{new_string}
</replace>

- **Error Encountered:**
<error>
{error}
</error>

# Full File Content
<file_content>
{current_content}
</file_content>

# Your Task
Provide a corrected `search` string that will succeed. Keep correction minimal and explain the precise reason for failure.

Respond with JSON:
{{
  "search": "corrected search string",
  "replace": "corrected replace string",
  "no_changes_required": false,
  "explanation": "Reason for the fix"
}}
"#
    );

    // 构建消息 (system prompt goes via chat_anthropic() parameter, not in messages)
    let messages = vec![crate::llm::Message::user(user_prompt)];

    // 调用 LLM（无工具，纯文本生成）
    let response = llm_client
        .chat_anthropic(
            "deepseek-chat", // 使用 DeepSeek
            EDIT_SYS_PROMPT,
            &messages,
            &[],   // 无工具
            0.2,   // 低温度，保守修正
            2048,  // 足够的 token
            None,  // No streaming for edit fixer
            false, // No thinking for edit fixer
        )
        .await?;

    // 解析响应
    if let Some(crate::llm::LLMResponseType::Text { text }) = response.content.first() {
        // 尝试从文本中提取 JSON
        let json_result = extract_json_from_text(text)?;
        let edit: SearchReplaceEdit = serde_json::from_value(json_result)
            .map_err(|e| format!("Failed to parse LLM response as SearchReplaceEdit: {}", e))?;
        return Ok(Some(edit));
    }

    Ok(None)
}

fn extract_json_from_text(text: &str) -> Result<serde_json::Value, String> {
    // 查找 JSON 代码块
    if let Some(start) = text.find("```json") {
        let content_start = start + 7; // skip "```json"
        if let Some(end_offset) = text[content_start..].find("```") {
            let json_str = text[content_start..content_start + end_offset].trim();
            return serde_json::from_str(json_str)
                .map_err(|e| format!("Failed to parse JSON: {}", e));
        }
    }

    // 尝试直接解析整个文本
    if let Some(start) = text.find('{') {
        if let Some(end) = text.rfind('}') {
            let json_str = &text[start..=end];
            return serde_json::from_str(json_str)
                .map_err(|e| format!("Failed to parse JSON: {}", e));
        }
    }

    Err("No valid JSON found in LLM response".to_string())
}
