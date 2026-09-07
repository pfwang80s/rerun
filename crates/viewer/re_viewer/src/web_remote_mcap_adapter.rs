//! Viewer-side consumer boundary for the Web remote-MCAP adapter.
//!
//! This module is intentionally limited to the adapter's opaque consumer contract. It does not
//! import a transport body, physical lease, cache, reservation, registry, or operation identity.
//! The production Viewer remains disarmed until the external capability is installed by a later
//! release gate.

use re_mcap_web_adapter::consumer_contract::AdapterOperationStatusV1;

/// Storage-free Viewer consumer for one adapter operation.
///
/// The Viewer owns only the opaque operation handle. Cross-layer ownership, revalidation,
/// rollback, and release ordering remain in the adapter and its owning producer crates.
pub(crate) struct RemoteMcapAdapterConsumerV1 {
    operation: Option<re_mcap_web_adapter::consumer_contract::AdapterOperationV1>,
}

impl RemoteMcapAdapterConsumerV1 {
    /// Creates the production-disarmed consumer boundary.
    pub(crate) fn new_disarmed_v1() -> Self {
        Self {
            operation: Some(re_mcap_web_adapter::consumer_contract::initiate_operation_v1()),
        }
    }

    /// Returns the opaque adapter status without exposing operation identity.
    pub(crate) fn status_v1(&self) -> AdapterOperationStatusV1 {
        self.operation
            .as_ref()
            .map_or(AdapterOperationStatusV1::Disarmed, |operation| {
                operation.status_v1()
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewer_consumer_remains_disarmed_and_opaque() {
        let consumer = RemoteMcapAdapterConsumerV1::new_disarmed_v1();
        assert_eq!(consumer.status_v1(), AdapterOperationStatusV1::Disarmed);
    }
}
