//! Update Personality Tool Executor
//!
//! Updates the persona's own personality notes.

use crate::tool::ToolExecutor;
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct UpdatePersonalityExecutor;

impl UpdatePersonalityExecutor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for UpdatePersonalityExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolExecutor for UpdatePersonalityExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        let notes = input
            .get("notes")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: notes")?;

        let persona_id = crate::agent_owner::extract_owner_id(&input)
            .ok_or("This tool requires an agent owner context")?;

        let persona_manager = crate::local_services::persona_manager()?;
        persona_manager
            .store()
            .update_personality_notes(persona_id, notes)?;

        Ok(json!({
            "message": "Personality notes updated successfully.",
            "notes_length": notes.len(),
        })
        .to_string())
    }
}
