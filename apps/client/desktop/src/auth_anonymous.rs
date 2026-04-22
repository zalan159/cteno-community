//! Community-build anonymous state is purely local.
//!
//! Unlogged desktop sessions do not provision anonymous server accounts or
//! register device keypairs. They only need a stable local `machineId` so the
//! host runtime can route machine-scoped RPC on this machine. When the user
//! later logs in with their real account, that same machine id is registered
//! through the normal authenticated flow.

use std::path::Path;

/// Ensure the local app data dir exists and return the stable local machine id.
///
/// This is intentionally offline-only: generating or loading the machine id
/// must not contact Happy Server.
pub fn ensure_local_machine_id(app_data_dir: &Path) -> Result<String, String> {
    std::fs::create_dir_all(app_data_dir)
        .map_err(|e| format!("Failed to create app data dir: {e}"))?;
    let path = app_data_dir.join("machine_id.txt");
    if path.exists() {
        let machine_id = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read machine ID: {e}"))?
            .trim()
            .to_string();
        if !machine_id.is_empty() {
            return Ok(machine_id);
        }
    }

    let machine_id = format!("cteno-{}", uuid::Uuid::new_v4());
    std::fs::write(&path, &machine_id).map_err(|e| format!("Failed to save machine ID: {e}"))?;
    Ok(machine_id)
}
