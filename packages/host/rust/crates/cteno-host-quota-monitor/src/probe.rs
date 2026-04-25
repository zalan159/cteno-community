//! Trait each vendor probe implements.

use async_trait::async_trait;

use crate::store::{VendorId, VendorQuota};

#[async_trait]
pub trait VendorQuotaProbe {
    fn vendor(&self) -> VendorId;

    /// Fetch one snapshot. Return `Err` when the probe is in a retryable
    /// failure state (network blip, 5xx). Return `Ok(VendorQuota)` with
    /// `error` set when the failure is a state the user needs to see in
    /// the UI (no credentials, unsupported plan, 401/403).
    async fn poll(&self) -> Result<VendorQuota, String>;
}
