//! Tool executor: A2UI Render
//!
//! Processes A2UI protocol messages (createSurface, updateComponents,
//! updateDataModel, deleteSurface) and applies them to the in-memory A2uiStore.
//! Pushes real-time update events to the frontend after each batch.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::hooks;
use crate::tool::ToolExecutor;

pub struct A2uiRenderExecutor;

impl A2uiRenderExecutor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for A2uiRenderExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolExecutor for A2uiRenderExecutor {
    async fn execute(&self, input: Value) -> Result<String, String> {
        let agent_id = input
            .get("__owner_id")
            .or_else(|| input.get("__persona_id"))
            .and_then(|v| v.as_str())
            .ok_or("Missing __owner_id — a2ui_render must be called within an agent session")?
            .to_string();

        let messages = input
            .get("messages")
            .and_then(|v| v.as_array())
            .ok_or("Missing required parameter: messages (array)")?;

        if messages.is_empty() {
            return Err("messages array is empty".to_string());
        }

        let store = hooks::a2ui_store().ok_or("A2uiStoreProvider not installed")?;
        let mut last_version: u64 = 0;
        let mut surfaces_created = 0u32;
        let mut components_updated = 0u32;

        for msg in messages {
            // createSurface
            if let Some(cs) = msg.get("createSurface") {
                let surface_id = cs
                    .get("surfaceId")
                    .and_then(|v| v.as_str())
                    .ok_or("createSurface: missing surfaceId")?;
                let catalog_id = cs
                    .get("catalogId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("cteno/v1");
                last_version = store.create_surface(&agent_id, surface_id, catalog_id);
                surfaces_created += 1;
            }

            // updateComponents
            if let Some(uc) = msg.get("updateComponents") {
                let surface_id = uc
                    .get("surfaceId")
                    .and_then(|v| v.as_str())
                    .ok_or("updateComponents: missing surfaceId")?;
                let components = uc
                    .get("components")
                    .and_then(|v| v.as_array())
                    .ok_or("updateComponents: missing components array")?
                    .clone();
                let count = components.len() as u32;
                last_version = store.update_components(&agent_id, surface_id, components)?;
                components_updated += count;
            }

            // updateDataModel
            if let Some(ud) = msg.get("updateDataModel") {
                let surface_id = ud
                    .get("surfaceId")
                    .and_then(|v| v.as_str())
                    .ok_or("updateDataModel: missing surfaceId")?;
                let data = ud
                    .get("data")
                    .cloned()
                    .unwrap_or(Value::Object(serde_json::Map::new()));
                last_version = store.update_data_model(&agent_id, surface_id, data)?;
            }

            // deleteSurface
            if let Some(ds) = msg.get("deleteSurface") {
                let surface_id = ds
                    .get("surfaceId")
                    .and_then(|v| v.as_str())
                    .ok_or("deleteSurface: missing surfaceId")?;
                store.delete_surface(&agent_id, surface_id);
            }
        }

        // Emit a transport-agnostic host event; the desktop shell fans it
        // out to the Tauri `local-host-event` channel (always) and, in
        // commercial builds, to the machine socket for cross-device relay.
        if let Some(emitter) = hooks::host_event_emitter() {
            let aid = agent_id.clone();
            tokio::spawn(async move {
                emitter.emit_a2ui_updated(&aid).await;
            });
        }

        Ok(json!({
            "success": true,
            "version": last_version,
            "surfacesCreated": surfaces_created,
            "componentsUpdated": components_updated,
        })
        .to_string())
    }
}
