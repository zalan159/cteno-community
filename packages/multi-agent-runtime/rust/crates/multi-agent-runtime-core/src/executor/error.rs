//! Errors returned by `AgentExecutor` operations.

use thiserror::Error;

/// Unified error type for all `AgentExecutor` methods.
///
/// Vendors raise `Unsupported` when a capability is not implemented for their
/// transport; callers should consult [`AgentCapabilities`](super::capabilities::AgentCapabilities)
/// before invocation to avoid surprises.
#[derive(Debug, Error)]
pub enum AgentExecutorError {
    /// The requested capability is not supported by this vendor / transport.
    #[error("capability not supported by this executor: {capability}")]
    Unsupported {
        /// Human-readable capability identifier (e.g. `"list_sessions"`).
        capability: String,
    },

    /// The underlying subprocess exited unexpectedly.
    #[error("subprocess exited unexpectedly: code={code:?}, stderr={stderr}")]
    SubprocessExited {
        /// Process exit code if available.
        code: Option<i32>,
        /// Captured stderr tail for diagnostics.
        stderr: String,
    },

    /// Generic I/O failure on stdio pipes or file access.
    #[error("io error: {0}")]
    Io(String),

    /// The subprocess produced output that does not conform to the expected
    /// wire protocol (JSON parse failure, schema mismatch, etc.).
    #[error("protocol error: {0}")]
    Protocol(String),

    /// No session matches the provided native session id.
    #[error("session not found: {0}")]
    SessionNotFound(String),

    /// A permission prompt was denied (or aborted) by the user / policy.
    #[error("permission rejected: {reason}")]
    PermissionRejected {
        /// Reason surfaced by the permission handler.
        reason: String,
    },

    /// An operation exceeded its deadline.
    #[error("timeout after {seconds}s: {operation}")]
    Timeout {
        /// Name of the timed-out operation.
        operation: String,
        /// Elapsed seconds before timing out.
        seconds: u64,
    },

    /// Vendor-specific failure mode preserved verbatim as a best-effort.
    #[error("vendor error ({vendor}): {message}")]
    Vendor {
        /// Identifier of the originating vendor (e.g. `"claude"`).
        vendor: &'static str,
        /// Message copied from the vendor transport.
        message: String,
    },
}

impl From<std::io::Error> for AgentExecutorError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}
