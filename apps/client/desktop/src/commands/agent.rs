use serde::{Deserialize, Serialize};
use tauri::State;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResponse {
    pub session_id: String,
    pub response: String,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
    pub output: Option<String>,
}

#[tauri::command]
pub async fn send_message(
    session_id: String,
    message: String,
    state: State<'_, crate::AppState>,
) -> Result<AgentResponse, String> {
    println!("[Tauri Command] send_message - session: {}, message: {}", session_id, message);

    // TODO: 调用 Autonomous Agent
    // 暂时返回模拟响应
    Ok(AgentResponse {
        session_id: session_id.clone(),
        response: format!("收到消息: {}", message),
        tool_calls: vec![],
    })
}

#[tauri::command]
pub async fn execute_agent(
    session_id: String,
    message: String,
    state: State<'_, crate::AppState>,
) -> Result<String, String> {
    println!("[Tauri Command] execute_agent - session: {}, message: {}", session_id, message);

    // Direct call to ToolRegistry — execute agent tool
    let registry = crate::local_services::tool_registry()
        .map_err(|e| format!("Tool registry not available: {}", e))?;
    let reg = registry.read().await;

    let input = serde_json::json!({
        "session_id": session_id,
        "message": message,
    });

    match reg.execute("agent", input).await {
        Ok(result) => Ok(result),
        Err(e) => Err(format!("Agent execution failed: {}", e)),
    }
}
