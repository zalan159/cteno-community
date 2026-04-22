//! Compatibility shim for the shared host session codec.
//!
//! The implementation lives in `cteno-host-session-codec` so local/community
//! session code no longer depends on Happy Server client crates for plaintext
//! session payload handling.

pub use cteno_host_session_codec::SessionMessageCodec;
