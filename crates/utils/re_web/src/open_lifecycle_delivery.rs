//! Production-disarmed Rust-side lifecycle delivery permits for strict Web open.
//!
//! The delivery queue state machine is isolated from TypeScript and from the existing Viewer
//! event dispatcher.

use std::collections::BTreeMap;
use std::fmt;
use std::num::NonZeroU64;

use crate::open_lifecycle_sequencer::PublicLifecycleEventV1;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct LifecycleDeliveryTokenV1(NonZeroU64);

impl fmt::Debug for LifecycleDeliveryTokenV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("LifecycleDeliveryTokenV1(<opaque>)")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleDeliveryPhaseV1 {
    RustQueued,
    DeliveredToTypeScript,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleDeliveryErrorV1 {
    CountCapacityReached,
    ByteCapacityReached,
    TokenExhausted,
    UnknownOrSettledToken,
    WrongPhase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleDeliverySettleOutcomeV1 {
    Released { retained_bytes: usize },
    Stale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleListenerErrorCreditOutcomeV1 {
    Reserved,
    AlreadyReserved,
    UnknownOrSettledToken,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LifecycleDeliverySnapshotV1 {
    pub queued_count: usize,
    pub delivered_count: usize,
    pub retained_bytes: usize,
}

struct LifecycleDeliveryEntryV1 {
    event: PublicLifecycleEventV1,
    phase: LifecycleDeliveryPhaseV1,
    retained_bytes: usize,
    listener_error_credit_reserved: bool,
}

pub struct LifecycleDeliveryRegistryV1 {
    next_token: u64,
    max_count: usize,
    max_retained_bytes: usize,
    retained_bytes: usize,
    entries: BTreeMap<LifecycleDeliveryTokenV1, LifecycleDeliveryEntryV1>,
}

impl LifecycleDeliveryRegistryV1 {
    pub fn new_v1(max_count: usize, max_retained_bytes: usize) -> Self {
        Self {
            next_token: 1,
            max_count,
            max_retained_bytes,
            retained_bytes: 0,
            entries: BTreeMap::new(),
        }
    }

    pub fn snapshot_v1(&self) -> LifecycleDeliverySnapshotV1 {
        LifecycleDeliverySnapshotV1 {
            queued_count: self
                .entries
                .values()
                .filter(|entry| entry.phase == LifecycleDeliveryPhaseV1::RustQueued)
                .count(),
            delivered_count: self
                .entries
                .values()
                .filter(|entry| entry.phase == LifecycleDeliveryPhaseV1::DeliveredToTypeScript)
                .count(),
            retained_bytes: self.retained_bytes,
        }
    }

    pub fn event_v1(&self, token: LifecycleDeliveryTokenV1) -> Option<PublicLifecycleEventV1> {
        self.entries.get(&token).map(|entry| entry.event)
    }

    pub fn queue_v1(
        &mut self,
        event: PublicLifecycleEventV1,
        retained_bytes: usize,
    ) -> Result<LifecycleDeliveryTokenV1, LifecycleDeliveryErrorV1> {
        if self.entries.len() >= self.max_count {
            return Err(LifecycleDeliveryErrorV1::CountCapacityReached);
        }
        let Some(next_retained_bytes) = self.retained_bytes.checked_add(retained_bytes) else {
            return Err(LifecycleDeliveryErrorV1::ByteCapacityReached);
        };
        if next_retained_bytes > self.max_retained_bytes {
            return Err(LifecycleDeliveryErrorV1::ByteCapacityReached);
        }

        let token = LifecycleDeliveryTokenV1(
            NonZeroU64::new(self.next_token).ok_or(LifecycleDeliveryErrorV1::TokenExhausted)?,
        );
        self.next_token = self
            .next_token
            .checked_add(1)
            .ok_or(LifecycleDeliveryErrorV1::TokenExhausted)?;
        self.retained_bytes = next_retained_bytes;
        self.entries.insert(
            token,
            LifecycleDeliveryEntryV1 {
                event,
                phase: LifecycleDeliveryPhaseV1::RustQueued,
                retained_bytes,
                listener_error_credit_reserved: false,
            },
        );
        Ok(token)
    }

    pub fn deliver_to_typescript_v1(
        &mut self,
        token: LifecycleDeliveryTokenV1,
    ) -> Result<PublicLifecycleEventV1, LifecycleDeliveryErrorV1> {
        let entry = self
            .entries
            .get_mut(&token)
            .ok_or(LifecycleDeliveryErrorV1::UnknownOrSettledToken)?;
        if entry.phase != LifecycleDeliveryPhaseV1::RustQueued {
            return Err(LifecycleDeliveryErrorV1::WrongPhase);
        }
        entry.phase = LifecycleDeliveryPhaseV1::DeliveredToTypeScript;
        Ok(entry.event)
    }

    pub fn ack_dispatched_v1(
        &mut self,
        token: LifecycleDeliveryTokenV1,
    ) -> LifecycleDeliverySettleOutcomeV1 {
        self.release_v1(token)
    }

    pub fn cancel_delivery_v1(
        &mut self,
        token: LifecycleDeliveryTokenV1,
    ) -> LifecycleDeliverySettleOutcomeV1 {
        self.release_v1(token)
    }

    pub fn reserve_listener_error_credit_v1(
        &mut self,
        token: LifecycleDeliveryTokenV1,
    ) -> LifecycleListenerErrorCreditOutcomeV1 {
        let Some(entry) = self.entries.get_mut(&token) else {
            return LifecycleListenerErrorCreditOutcomeV1::UnknownOrSettledToken;
        };
        if entry.listener_error_credit_reserved {
            return LifecycleListenerErrorCreditOutcomeV1::AlreadyReserved;
        }
        entry.listener_error_credit_reserved = true;
        LifecycleListenerErrorCreditOutcomeV1::Reserved
    }

    pub fn stop_v1(&mut self) -> LifecycleDeliverySnapshotV1 {
        let snapshot = self.snapshot_v1();
        self.entries.clear();
        self.retained_bytes = 0;
        snapshot
    }

    fn release_v1(&mut self, token: LifecycleDeliveryTokenV1) -> LifecycleDeliverySettleOutcomeV1 {
        let Some(entry) = self.entries.remove(&token) else {
            return LifecycleDeliverySettleOutcomeV1::Stale;
        };
        self.retained_bytes = self.retained_bytes.saturating_sub(entry.retained_bytes);
        LifecycleDeliverySettleOutcomeV1::Released {
            retained_bytes: entry.retained_bytes,
        }
    }
}

impl fmt::Debug for LifecycleDeliveryRegistryV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let snapshot = self.snapshot_v1();
        formatter
            .debug_struct("LifecycleDeliveryRegistryV1")
            .field("queued_count", &snapshot.queued_count)
            .field("delivered_count", &snapshot.delivered_count)
            .field("retained_bytes", &snapshot.retained_bytes)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use crate::open_lifecycle_sequencer::PublicLifecycleEventV1;
    use crate::strict_open_wire::OpenOperationIdentity;

    use super::*;

    fn accepted(index: u128) -> PublicLifecycleEventV1 {
        PublicLifecycleEventV1::Accepted {
            operation_id: OpenOperationIdentity::new(index),
        }
    }

    #[test]
    fn capacity_is_retained_until_dispatch_ack_or_cancel() {
        let mut registry = LifecycleDeliveryRegistryV1::new_v1(1, 10);
        let token = registry.queue_v1(accepted(1), 10).unwrap();
        assert_eq!(
            registry.queue_v1(accepted(2), 1),
            Err(LifecycleDeliveryErrorV1::CountCapacityReached)
        );
        assert_eq!(registry.deliver_to_typescript_v1(token), Ok(accepted(1)));
        assert_eq!(
            registry.queue_v1(accepted(2), 1),
            Err(LifecycleDeliveryErrorV1::CountCapacityReached)
        );
        assert_eq!(
            registry.ack_dispatched_v1(token),
            LifecycleDeliverySettleOutcomeV1::Released { retained_bytes: 10 }
        );
        assert!(registry.queue_v1(accepted(2), 1).is_ok());
    }

    #[test]
    fn byte_capacity_rejection_preserves_existing_fifo_item() {
        let mut registry = LifecycleDeliveryRegistryV1::new_v1(2, 10);
        let token = registry.queue_v1(accepted(1), 8).unwrap();
        assert_eq!(
            registry.queue_v1(accepted(2), 3),
            Err(LifecycleDeliveryErrorV1::ByteCapacityReached)
        );
        assert_eq!(registry.event_v1(token), Some(accepted(1)));
        assert_eq!(
            registry.snapshot_v1(),
            LifecycleDeliverySnapshotV1 {
                queued_count: 1,
                delivered_count: 0,
                retained_bytes: 8,
            }
        );
    }

    #[test]
    fn listener_error_credit_is_one_per_delivery_item() {
        let mut registry = LifecycleDeliveryRegistryV1::new_v1(2, 10);
        let token = registry.queue_v1(accepted(1), 1).unwrap();
        assert_eq!(
            registry.reserve_listener_error_credit_v1(token),
            LifecycleListenerErrorCreditOutcomeV1::Reserved
        );
        assert_eq!(
            registry.reserve_listener_error_credit_v1(token),
            LifecycleListenerErrorCreditOutcomeV1::AlreadyReserved
        );
        registry.cancel_delivery_v1(token);
        assert_eq!(
            registry.reserve_listener_error_credit_v1(token),
            LifecycleListenerErrorCreditOutcomeV1::UnknownOrSettledToken
        );
    }

    #[test]
    fn stop_settles_all_entries_and_late_ack_is_stale() {
        let mut registry = LifecycleDeliveryRegistryV1::new_v1(4, 10);
        let queued = registry.queue_v1(accepted(1), 3).unwrap();
        let delivered = registry.queue_v1(accepted(2), 4).unwrap();
        registry.deliver_to_typescript_v1(delivered).unwrap();
        assert_eq!(
            registry.stop_v1(),
            LifecycleDeliverySnapshotV1 {
                queued_count: 1,
                delivered_count: 1,
                retained_bytes: 7,
            }
        );
        assert_eq!(
            registry.ack_dispatched_v1(queued),
            LifecycleDeliverySettleOutcomeV1::Stale
        );
        assert_eq!(
            registry.cancel_delivery_v1(delivered),
            LifecycleDeliverySettleOutcomeV1::Stale
        );
        assert_eq!(
            registry.snapshot_v1(),
            LifecycleDeliverySnapshotV1 {
                queued_count: 0,
                delivered_count: 0,
                retained_bytes: 0,
            }
        );
    }
}
