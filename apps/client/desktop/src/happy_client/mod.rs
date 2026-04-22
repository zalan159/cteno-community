//! Happy Client Module
//!
//! Rust implementation of Happy Server client protocol.
//! Provides authentication, WebSocket connection, and encryption.
//!
//! The desktop app always links the Happy Server client crates for cloud
//! upload, machine registration, and login-gated sync features.

pub mod local_sink;
pub mod manager;
pub mod permission;
mod profile_rpc;
pub mod runtime;
pub mod session;
pub(crate) mod session_helpers;
pub mod socket;
pub use local_sink::DesktopLocalSink;

#[cfg(feature = "commercial-cloud")]
pub use cteno_happy_client_core::{
    decrypt_data, encrypt_data, AuthToken, EncryptionVariant, HappyAuth,
};
#[cfg(feature = "commercial-cloud")]
pub use cteno_happy_client_machine::{MachineManager, MachineMetadata};
#[cfg(feature = "commercial-cloud")]
pub use cteno_happy_client_rpc::{
    EncryptedRpcHandler, RpcHandler, RpcHandlerFuture, RpcRegistry, RpcRequest, RpcResponse,
};
#[cfg(not(feature = "commercial-cloud"))]
pub use cteno_host_rpc_core::{RpcHandler, RpcHandlerFuture, RpcRegistry, RpcRequest, RpcResponse};
#[cfg(not(feature = "commercial-cloud"))]
pub use cteno_host_session_codec::{decrypt_data, encrypt_data, EncryptionVariant};
pub use cteno_host_session_wire::{
    ConnectionType, DaemonState, EphemeralEvent, MachineInfo, SessionInfo, UpdateEvent,
    UpdatePayload, VersionedValue,
};
pub use manager::HappyClientManager;
pub use session::{
    resume_session_connection, spawn_session_internal, SessionAgentConfig, SessionConnection,
    SessionConnectionsMap, SessionRegistry, SpawnSessionConfig,
};
pub use socket::HappySocket;
pub use socket::HeartbeatManager;
