//! Vendor-specific `VendorSyncer` implementations. Each adapter translates the
//! canonical specs into that vendor's native on-disk layout.

pub mod claude;
pub mod codex;
pub mod cteno;
pub mod gemini;

pub use claude::ClaudeSyncer;
pub use codex::CodexSyncer;
pub use cteno::CtenoSyncer;
pub use gemini::GeminiSyncer;

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::schemas::{McpSpec, McpTransport};

pub(crate) const LEGACY_CTENO_MEMORY_MCP_NAME: &str = "cteno-memory";

/// Shared helper — render an `McpSpec` as the JSON shape expected by Claude /
/// Gemini (Codex uses TOML and has its own renderer).
pub(crate) fn mcp_to_json(spec: &McpSpec) -> Value {
    let mut obj = serde_json::Map::new();
    match &spec.transport {
        McpTransport::Stdio => {
            obj.insert("command".into(), Value::String(spec.command.clone()));
            if !spec.args.is_empty() {
                obj.insert(
                    "args".into(),
                    Value::Array(spec.args.iter().cloned().map(Value::String).collect()),
                );
            }
        }
        McpTransport::StreamableHttp { url } => {
            obj.insert("type".into(), Value::String("http".into()));
            obj.insert("url".into(), Value::String(url.clone()));
        }
    }
    if !spec.env.is_empty() {
        let mut env = serde_json::Map::new();
        for (k, v) in &spec.env {
            env.insert(k.clone(), Value::String(v.clone()));
        }
        obj.insert("env".into(), Value::Object(env));
    }
    Value::Object(obj)
}

/// Read a JSON file, returning an empty object on missing/empty/invalid file
/// so callers can always merge into something.
pub(crate) fn read_json_or_empty(path: &Path) -> anyhow::Result<Value> {
    match std::fs::read_to_string(path) {
        Ok(s) if s.trim().is_empty() => Ok(Value::Object(Default::default())),
        Ok(s) => Ok(serde_json::from_str(&s).unwrap_or(Value::Object(Default::default()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Value::Object(Default::default())),
        Err(e) => Err(anyhow::anyhow!("read {path:?}: {e}")),
    }
}

pub(crate) fn write_json(path: &Path, value: &Value) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let s = serde_json::to_string_pretty(value)?;
    std::fs::write(path, s)?;
    Ok(())
}

pub(crate) fn ensure_object_mut(v: &mut Value) -> &mut serde_json::Map<String, Value> {
    if !v.is_object() {
        *v = Value::Object(Default::default());
    }
    v.as_object_mut().unwrap()
}

pub(crate) fn persona_link_path(vendor_dir: &Path, name: &str) -> PathBuf {
    vendor_dir.join(format!("{name}.md"))
}
