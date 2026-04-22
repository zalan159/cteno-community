//! Thin re-export shim: the daemon runtime primitives live in
//! `cteno-host-runtime::daemon_state` now. This module is preserved as a
//! compatibility alias so existing `crate::host::daemon_runtime::*` imports
//! keep working.

pub use cteno_host_runtime::daemon_state::*;
