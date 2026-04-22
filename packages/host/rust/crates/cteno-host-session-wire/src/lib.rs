//! Session relay wire types shared by community and commercial runtimes.
//!
//! These types describe connection scopes and relay/update payload shapes. They
//! are intentionally host-level so local/community code can type-check socket
//! substitutes and replay paths without depending on Happy Server client crates.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names)]
pub enum ConnectionType {
    UserScoped,
    SessionScoped { session_id: String },
    MachineScoped { machine_id: String },
}

impl ConnectionType {
    pub fn client_type(&self) -> &'static str {
        match self {
            Self::UserScoped => "user-scoped",
            Self::SessionScoped { .. } => "session-scoped",
            Self::MachineScoped { .. } => "machine-scoped",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum UpdateEvent {
    #[serde(rename = "new-session")]
    NewSession(NewSessionEvent),
    #[serde(rename = "new-message")]
    NewMessage(NewMessageEvent),
    #[serde(rename = "update-session")]
    UpdateSession(UpdateSessionEvent),
    #[serde(rename = "new-machine")]
    NewMachine(NewMachineEvent),
    #[serde(rename = "update-machine")]
    UpdateMachine(UpdateMachineEvent),
    #[serde(rename = "new-artifact")]
    NewArtifact(NewArtifactEvent),
    #[serde(rename = "update-artifact")]
    UpdateArtifact(UpdateArtifactEvent),
    #[serde(rename = "delete-artifact")]
    DeleteArtifact(DeleteArtifactEvent),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdatePayload {
    pub id: String,
    pub seq: u64,
    pub body: UpdateEvent,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewSessionEvent {
    pub id: String,
    pub seq: u64,
    pub metadata: String,
    #[serde(rename = "metadataVersion")]
    pub metadata_version: u32,
    #[serde(rename = "agentState")]
    pub agent_state: Option<String>,
    #[serde(rename = "agentStateVersion")]
    pub agent_state_version: u32,
    #[serde(rename = "dataEncryptionKey")]
    pub data_encryption_key: Option<String>,
    pub active: bool,
    #[serde(rename = "activeAt")]
    pub active_at: i64,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
    #[serde(rename = "updatedAt")]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewMessageEvent {
    pub sid: String,
    pub message: MessageContent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageContent {
    pub id: String,
    pub seq: u64,
    pub content: EncryptedContent,
    #[serde(rename = "localId")]
    pub local_id: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
    #[serde(rename = "updatedAt")]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedContent {
    pub t: String,
    pub c: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateSessionEvent {
    pub id: String,
    pub metadata: Option<VersionedValue>,
    #[serde(rename = "agentState")]
    pub agent_state: Option<VersionedValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionedValue {
    pub value: Option<String>,
    pub version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewMachineEvent {
    pub id: String,
    pub seq: u64,
    pub metadata: String,
    #[serde(rename = "metadataVersion")]
    pub metadata_version: u32,
    #[serde(rename = "daemonState")]
    pub daemon_state: Option<String>,
    #[serde(rename = "daemonStateVersion")]
    pub daemon_state_version: u32,
    #[serde(rename = "dataEncryptionKey")]
    pub data_encryption_key: Option<String>,
    pub active: bool,
    #[serde(rename = "activeAt")]
    pub active_at: i64,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
    #[serde(rename = "updatedAt")]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateMachineEvent {
    #[serde(rename = "machineId")]
    pub machine_id: String,
    pub metadata: Option<VersionedValue>,
    #[serde(rename = "daemonState")]
    pub daemon_state: Option<VersionedValue>,
    #[serde(rename = "activeAt")]
    pub active_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewArtifactEvent {
    pub id: String,
    pub seq: u64,
    pub header: String,
    #[serde(rename = "headerVersion")]
    pub header_version: u32,
    pub body: String,
    #[serde(rename = "bodyVersion")]
    pub body_version: u32,
    #[serde(rename = "dataEncryptionKey")]
    pub data_encryption_key: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
    #[serde(rename = "updatedAt")]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateArtifactEvent {
    pub id: String,
    pub header: Option<VersionedValue>,
    pub body: Option<VersionedValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteArtifactEvent {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum EphemeralEvent {
    #[serde(rename = "activity")]
    Activity {
        sid: String,
        active: bool,
        #[serde(rename = "activeAt")]
        active_at: i64,
        thinking: Option<bool>,
    },
    #[serde(rename = "machine-activity")]
    MachineActivity {
        #[serde(rename = "machineId")]
        machine_id: String,
        active: bool,
        #[serde(rename = "activeAt")]
        active_at: i64,
    },
    #[serde(rename = "usage")]
    Usage {
        model: String,
        #[serde(rename = "tokensInput")]
        tokens_input: u64,
        #[serde(rename = "tokensOutput")]
        tokens_output: u64,
        #[serde(rename = "costUsd")]
        cost_usd: f64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MachineInfo {
    pub id: String,
    pub metadata: String,
    #[serde(rename = "metadataVersion")]
    pub metadata_version: u32,
    #[serde(rename = "daemonState")]
    pub daemon_state: Option<String>,
    #[serde(rename = "daemonStateVersion")]
    pub daemon_state_version: u32,
    #[serde(rename = "dataEncryptionKey")]
    pub data_encryption_key: Option<String>,
    pub active: bool,
    #[serde(rename = "activeAt")]
    pub active_at: i64,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
    #[serde(rename = "updatedAt")]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonState {
    pub status: String,
    pub pid: Option<u32>,
    #[serde(rename = "httpPort")]
    pub http_port: Option<u16>,
    #[serde(rename = "startedAt")]
    pub started_at: Option<i64>,
    #[serde(rename = "shutdownRequestedAt")]
    pub shutdown_requested_at: Option<i64>,
    #[serde(rename = "shutdownSource")]
    pub shutdown_source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub seq: u64,
    pub metadata: String,
    #[serde(rename = "metadataVersion")]
    pub metadata_version: u32,
    #[serde(rename = "agentState")]
    pub agent_state: Option<String>,
    #[serde(rename = "agentStateVersion")]
    pub agent_state_version: u32,
    #[serde(rename = "dataEncryptionKey")]
    pub data_encryption_key: Option<String>,
    pub active: bool,
    #[serde(rename = "activeAt")]
    pub active_at: i64,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
    #[serde(rename = "updatedAt")]
    pub updated_at: i64,
}
