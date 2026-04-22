//! Ask Persona Tool Executor
//!
//! Allows a task session worker to ask its dispatching persona a question.
//! Sends the question to the persona's chat session via Socket.IO so it
//! triggers the persona agent to process it.

use crate::tool::ToolExecutor;
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct AskPersonaExecutor;

impl AskPersonaExecutor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AskPersonaExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolExecutor for AskPersonaExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        let question = input
            .get("question")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: question")?;

        let session_id = input
            .get("__session_id")
            .and_then(|v| v.as_str())
            .ok_or("Missing session context")?;

        // Find which persona dispatched this session
        let persona_manager = crate::local_services::persona_manager()?;
        let link = persona_manager
            .store()
            .get_persona_for_session(session_id)?
            .ok_or("This session is not associated with any persona")?;

        if link.session_type != crate::persona::PersonaSessionType::Task {
            return Err("ask_persona can only be used in task sessions".to_string());
        }

        let persona = persona_manager
            .store()
            .get_persona(&link.persona_id)?
            .ok_or("Persona not found")?;

        // Format the question as a message from the task session
        let notification = format!("[Task Session {} asks]: {}", session_id, question);

        // Send via Socket.IO to the persona's chat session
        {
            let spawn_config = crate::local_services::spawn_config()?;
            let handle = spawn_config
                .session_connections
                .get(&persona.chat_session_id)
                .await
                .map(|conn| conn.message_handle())
                .ok_or_else(|| {
                    format!(
                        "Persona chat session {} not in active connections",
                        persona.chat_session_id
                    )
                })?;

            handle.send_initial_user_message(&notification).await?;
        }

        log::info!(
            "[AskPersona] Question from task session {} sent to persona '{}' (chat session {}): {} chars",
            session_id,
            persona.name,
            persona.chat_session_id,
            question.len()
        );

        Ok(json!({
            "message": "Question sent to persona. The persona will process it and may respond or take action.",
            "persona_id": link.persona_id,
            "persona_name": persona.name,
        })
        .to_string())
    }
}
