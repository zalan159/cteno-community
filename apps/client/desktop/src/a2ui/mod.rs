//! A2UI (Agent-to-User Interface) Protocol Implementation
//!
//! Aligned with Google's A2UI v0.9 specification.
//! Provides an in-memory store for declarative UI component trees that agents
//! can update via the `a2ui_render` tool. The frontend renders these as native
//! React Native components instead of WebView HTML.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::RwLock;

/// A rendering surface — a named container for UI components.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A2uiSurface {
    pub surface_id: String,
    pub catalog_id: String,
    /// Ordered list of components on this surface.
    pub components: Vec<Value>,
    /// Data model for data binding (JSON Pointer paths).
    pub data_model: Value,
    /// Monotonically increasing version for change detection.
    pub version: u64,
}

/// In-memory store for A2UI surfaces, keyed by agent_id → surface_id.
pub struct A2uiStore {
    /// agent_id → surface_id → Surface
    surfaces: RwLock<HashMap<String, HashMap<String, A2uiSurface>>>,
}

impl A2uiStore {
    pub fn new() -> Self {
        Self {
            surfaces: RwLock::new(HashMap::new()),
        }
    }

    /// Create a new surface for an agent. Overwrites if surface_id already exists.
    pub fn create_surface(&self, agent_id: &str, surface_id: &str, catalog_id: &str) -> u64 {
        let mut map = self.surfaces.write().unwrap();
        let agent_surfaces = map.entry(agent_id.to_string()).or_default();
        let surface = A2uiSurface {
            surface_id: surface_id.to_string(),
            catalog_id: catalog_id.to_string(),
            components: Vec::new(),
            data_model: Value::Null,
            version: 1,
        };
        agent_surfaces.insert(surface_id.to_string(), surface);
        1
    }

    /// Update (upsert) components on a surface.
    /// Components with matching `id` are replaced; new ones are appended.
    pub fn update_components(
        &self,
        agent_id: &str,
        surface_id: &str,
        components: Vec<Value>,
    ) -> Result<u64, String> {
        let mut map = self.surfaces.write().unwrap();
        let surface = map
            .get_mut(agent_id)
            .and_then(|m| m.get_mut(surface_id))
            .ok_or_else(|| {
                format!(
                    "Surface '{}' not found for agent '{}'",
                    surface_id, agent_id
                )
            })?;

        for new_comp in components {
            let new_id = new_comp
                .get("id")
                .and_then(|v| v.as_str())
                .map(String::from);
            if let Some(ref id) = new_id {
                // Replace existing component with same id
                if let Some(pos) = surface
                    .components
                    .iter()
                    .position(|c| c.get("id").and_then(|v| v.as_str()) == Some(id))
                {
                    surface.components[pos] = new_comp;
                    continue;
                }
            }
            // Append new component
            surface.components.push(new_comp);
        }

        surface.version += 1;
        Ok(surface.version)
    }

    /// Update the data model on a surface (merge at top level).
    pub fn update_data_model(
        &self,
        agent_id: &str,
        surface_id: &str,
        data: Value,
    ) -> Result<u64, String> {
        let mut map = self.surfaces.write().unwrap();
        let surface = map
            .get_mut(agent_id)
            .and_then(|m| m.get_mut(surface_id))
            .ok_or_else(|| {
                format!(
                    "Surface '{}' not found for agent '{}'",
                    surface_id, agent_id
                )
            })?;

        // Merge top-level keys into existing data model
        if let (Value::Object(existing), Value::Object(new_data)) = (&mut surface.data_model, &data)
        {
            for (k, v) in new_data {
                existing.insert(k.clone(), v.clone());
            }
        } else {
            surface.data_model = data;
        }

        surface.version += 1;
        Ok(surface.version)
    }

    /// Delete a surface.
    pub fn delete_surface(&self, agent_id: &str, surface_id: &str) -> bool {
        let mut map = self.surfaces.write().unwrap();
        if let Some(agent_surfaces) = map.get_mut(agent_id) {
            agent_surfaces.remove(surface_id).is_some()
        } else {
            false
        }
    }

    /// Get all surfaces for an agent.
    pub fn get_surfaces(&self, agent_id: &str) -> Vec<A2uiSurface> {
        let map = self.surfaces.read().unwrap();
        map.get(agent_id)
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Clear all surfaces for an agent (cleanup on agent deletion).
    pub fn clear_agent(&self, agent_id: &str) {
        let mut map = self.surfaces.write().unwrap();
        map.remove(agent_id);
    }
}

impl Default for A2uiStore {
    fn default() -> Self {
        Self::new()
    }
}
