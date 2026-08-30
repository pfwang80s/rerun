//! Production-disarmed source/operation/recording lifecycle registry for strict Web open.
//!
//! This module models the public identity split required by strict Web MCAP open.
//! It does not hook into native viewer routing or the existing compatibility `open()` path.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::open_source_terminal::OpenSourceToken;
use crate::store_publication::{StoreGenerationId, StorePublicationIdentity};
use crate::strict_open_wire::{
    OpenOperationIdentity, PublicOpenRequestIdentity, PublicRecordingIdentity,
};

#[derive(Clone, Debug)]
pub struct OpenLifecycleRegistryLimitsV1 {
    pub max_sources: usize,
    pub max_operations: usize,
    pub max_recordings: usize,
    pub max_subscriptions_per_operation: usize,
    pub max_removed_tombstones: usize,
}

impl OpenLifecycleRegistryLimitsV1 {
    pub const fn for_tests_v1() -> Self {
        Self {
            max_sources: 8,
            max_operations: 16,
            max_recordings: 32,
            max_subscriptions_per_operation: 8,
            max_removed_tombstones: 8,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenLifecycleRegistryErrorV1 {
    SourceLimitReached,
    OperationLimitReached,
    RecordingLimitReached,
    SubscriptionLimitReached,
    RemovedTombstoneLimitReached,
    DuplicateOperation,
    DuplicateRecording,
    DuplicateStorePublication,
    UnknownOperation,
    UnknownRecording,
    SourceMismatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordingLifecycleStateV1 {
    Active,
    Removed,
}

struct SourceLifecycleEntryV1 {
    operations: BTreeSet<OpenOperationIdentity>,
    recordings: BTreeSet<PublicRecordingIdentity>,
}

impl SourceLifecycleEntryV1 {
    fn new_v1() -> Self {
        Self {
            operations: BTreeSet::new(),
            recordings: BTreeSet::new(),
        }
    }
}

struct OperationLifecycleEntryV1 {
    source_token: OpenSourceToken,
    request_id: PublicOpenRequestIdentity,
    recording_subscriptions: BTreeSet<PublicRecordingIdentity>,
}

struct RecordingLifecycleEntryV1 {
    source_token: OpenSourceToken,
    store_target: StoreGenerationId,
    state: RecordingLifecycleStateV1,
    subscribers: BTreeSet<OpenOperationIdentity>,
}

pub struct OpenLifecycleRegistryV1 {
    limits: OpenLifecycleRegistryLimitsV1,
    sources: BTreeMap<OpenSourceToken, SourceLifecycleEntryV1>,
    operations: BTreeMap<OpenOperationIdentity, OperationLifecycleEntryV1>,
    recordings: BTreeMap<PublicRecordingIdentity, RecordingLifecycleEntryV1>,
    store_reverse: BTreeMap<StoreGenerationId, PublicRecordingIdentity>,
}

impl OpenLifecycleRegistryV1 {
    pub fn new_v1(limits: OpenLifecycleRegistryLimitsV1) -> Self {
        Self {
            limits,
            sources: BTreeMap::new(),
            operations: BTreeMap::new(),
            recordings: BTreeMap::new(),
            store_reverse: BTreeMap::new(),
        }
    }

    pub fn source_count_v1(&self) -> usize {
        self.sources.len()
    }

    pub fn operation_count_v1(&self) -> usize {
        self.operations.len()
    }

    pub fn recording_count_v1(&self) -> usize {
        self.recordings.len()
    }

    pub fn removed_tombstone_count_v1(&self) -> usize {
        self.recordings
            .values()
            .filter(|entry| entry.state == RecordingLifecycleStateV1::Removed)
            .count()
    }

    pub fn source_operation_count_v1(&self, source_token: OpenSourceToken) -> Option<usize> {
        self.sources
            .get(&source_token)
            .map(|source| source.operations.len())
    }

    pub fn recording_subscriber_count_v1(
        &self,
        recording_id: PublicRecordingIdentity,
    ) -> Option<usize> {
        self.recordings
            .get(&recording_id)
            .map(|recording| recording.subscribers.len())
    }

    pub fn recording_state_v1(
        &self,
        recording_id: PublicRecordingIdentity,
    ) -> Option<RecordingLifecycleStateV1> {
        self.recordings
            .get(&recording_id)
            .map(|recording| recording.state)
    }

    pub fn lookup_recording_by_store_v1(
        &self,
        target: &StoreGenerationId,
    ) -> Option<PublicRecordingIdentity> {
        self.store_reverse.get(target).copied()
    }

    pub fn register_operation_v1(
        &mut self,
        source_token: OpenSourceToken,
        operation_id: OpenOperationIdentity,
        request_id: PublicOpenRequestIdentity,
    ) -> Result<(), OpenLifecycleRegistryErrorV1> {
        if self.operations.contains_key(&operation_id) {
            return Err(OpenLifecycleRegistryErrorV1::DuplicateOperation);
        }
        if self.operations.len() >= self.limits.max_operations {
            return Err(OpenLifecycleRegistryErrorV1::OperationLimitReached);
        }
        if !self.sources.contains_key(&source_token)
            && self.sources.len() >= self.limits.max_sources
        {
            return Err(OpenLifecycleRegistryErrorV1::SourceLimitReached);
        }

        self.sources
            .entry(source_token)
            .or_insert_with(SourceLifecycleEntryV1::new_v1)
            .operations
            .insert(operation_id);
        self.operations.insert(
            operation_id,
            OperationLifecycleEntryV1 {
                source_token,
                request_id,
                recording_subscriptions: BTreeSet::new(),
            },
        );
        Ok(())
    }

    pub fn operation_request_id_v1(
        &self,
        operation_id: OpenOperationIdentity,
    ) -> Option<PublicOpenRequestIdentity> {
        self.operations
            .get(&operation_id)
            .map(|operation| operation.request_id)
    }

    pub fn publish_recording_v1(
        &mut self,
        operation_id: OpenOperationIdentity,
        recording_id: PublicRecordingIdentity,
        publication: &StorePublicationIdentity,
    ) -> Result<(), OpenLifecycleRegistryErrorV1> {
        if self.recordings.contains_key(&recording_id) {
            return Err(OpenLifecycleRegistryErrorV1::DuplicateRecording);
        }
        let store_target = publication.fixed_target();
        if self.store_reverse.contains_key(&store_target) {
            return Err(OpenLifecycleRegistryErrorV1::DuplicateStorePublication);
        }
        if self.recordings.len() >= self.limits.max_recordings {
            return Err(OpenLifecycleRegistryErrorV1::RecordingLimitReached);
        }
        let operation = self
            .operations
            .get_mut(&operation_id)
            .ok_or(OpenLifecycleRegistryErrorV1::UnknownOperation)?;
        if operation.recording_subscriptions.len() >= self.limits.max_subscriptions_per_operation {
            return Err(OpenLifecycleRegistryErrorV1::SubscriptionLimitReached);
        }

        let source_token = operation.source_token;
        operation.recording_subscriptions.insert(recording_id);
        self.sources
            .get_mut(&source_token)
            .expect("operation source exists")
            .recordings
            .insert(recording_id);
        self.store_reverse
            .insert(store_target.clone(), recording_id);
        self.recordings.insert(
            recording_id,
            RecordingLifecycleEntryV1 {
                source_token,
                store_target,
                state: RecordingLifecycleStateV1::Active,
                subscribers: BTreeSet::from([operation_id]),
            },
        );
        Ok(())
    }

    pub fn attach_recording_v1(
        &mut self,
        operation_id: OpenOperationIdentity,
        recording_id: PublicRecordingIdentity,
    ) -> Result<(), OpenLifecycleRegistryErrorV1> {
        let operation = self
            .operations
            .get_mut(&operation_id)
            .ok_or(OpenLifecycleRegistryErrorV1::UnknownOperation)?;
        let recording = self
            .recordings
            .get_mut(&recording_id)
            .ok_or(OpenLifecycleRegistryErrorV1::UnknownRecording)?;
        if operation.source_token != recording.source_token {
            return Err(OpenLifecycleRegistryErrorV1::SourceMismatch);
        }
        if recording.state != RecordingLifecycleStateV1::Active {
            return Err(OpenLifecycleRegistryErrorV1::UnknownRecording);
        }
        if !operation.recording_subscriptions.contains(&recording_id)
            && operation.recording_subscriptions.len()
                >= self.limits.max_subscriptions_per_operation
        {
            return Err(OpenLifecycleRegistryErrorV1::SubscriptionLimitReached);
        }

        operation.recording_subscriptions.insert(recording_id);
        recording.subscribers.insert(operation_id);
        Ok(())
    }

    pub fn remove_recording_v1(
        &mut self,
        recording_id: PublicRecordingIdentity,
    ) -> Result<(), OpenLifecycleRegistryErrorV1> {
        let Some(recording) = self.recordings.get(&recording_id) else {
            return Err(OpenLifecycleRegistryErrorV1::UnknownRecording);
        };
        if recording.state == RecordingLifecycleStateV1::Removed {
            return Ok(());
        }

        let store_target = recording.store_target.clone();
        let source_token = recording.source_token;
        let has_subscribers = !recording.subscribers.is_empty();
        if has_subscribers
            && self.removed_tombstone_count_v1() >= self.limits.max_removed_tombstones
        {
            return Err(OpenLifecycleRegistryErrorV1::RemovedTombstoneLimitReached);
        }

        self.store_reverse.remove(&store_target);
        if !has_subscribers {
            self.recordings.remove(&recording_id);
            self.remove_recording_from_source_v1(source_token, recording_id);
            return Ok(());
        }

        self.recordings
            .get_mut(&recording_id)
            .expect("recording still exists")
            .state = RecordingLifecycleStateV1::Removed;
        Ok(())
    }

    pub fn dispose_operation_v1(
        &mut self,
        operation_id: OpenOperationIdentity,
    ) -> Result<(), OpenLifecycleRegistryErrorV1> {
        let operation = self
            .operations
            .remove(&operation_id)
            .ok_or(OpenLifecycleRegistryErrorV1::UnknownOperation)?;
        if let Some(source) = self.sources.get_mut(&operation.source_token) {
            source.operations.remove(&operation_id);
        }

        let subscriptions = operation
            .recording_subscriptions
            .iter()
            .copied()
            .collect::<Vec<_>>();
        for recording_id in subscriptions {
            let Some(recording) = self.recordings.get_mut(&recording_id) else {
                continue;
            };
            recording.subscribers.remove(&operation_id);
            if recording.state == RecordingLifecycleStateV1::Removed
                && recording.subscribers.is_empty()
            {
                let source_token = recording.source_token;
                self.recordings.remove(&recording_id);
                self.remove_recording_from_source_v1(source_token, recording_id);
            }
        }
        self.remove_empty_source_v1(operation.source_token);
        Ok(())
    }

    fn remove_recording_from_source_v1(
        &mut self,
        source_token: OpenSourceToken,
        recording_id: PublicRecordingIdentity,
    ) {
        if let Some(source) = self.sources.get_mut(&source_token) {
            source.recordings.remove(&recording_id);
        }
        self.remove_empty_source_v1(source_token);
    }

    fn remove_empty_source_v1(&mut self, source_token: OpenSourceToken) {
        let should_remove = self
            .sources
            .get(&source_token)
            .is_some_and(|source| source.operations.is_empty() && source.recordings.is_empty());
        if should_remove {
            self.sources.remove(&source_token);
        }
    }
}

impl fmt::Debug for OpenLifecycleRegistryV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenLifecycleRegistryV1")
            .field("limits", &self.limits)
            .field("sources", &self.sources.len())
            .field("operations", &self.operations.len())
            .field("recordings", &self.recordings.len())
            .field("store_reverse", &self.store_reverse.len())
            .field("removed_tombstones", &self.removed_tombstone_count_v1())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use re_log_types::StoreId;

    use crate::store_publication::allocate_store_publication_identity;

    use super::*;

    fn source(index: u64) -> OpenSourceToken {
        OpenSourceToken::new_for_test_v1(index)
    }

    fn operation(index: u128) -> OpenOperationIdentity {
        OpenOperationIdentity::new(index)
    }

    fn request(index: u128) -> PublicOpenRequestIdentity {
        PublicOpenRequestIdentity::new(index)
    }

    fn recording(index: u128) -> PublicRecordingIdentity {
        PublicRecordingIdentity::new(index)
    }

    fn publication(
        application_id: &'static str,
        recording_id: &'static str,
    ) -> StorePublicationIdentity {
        allocate_store_publication_identity(StoreId::recording(application_id, recording_id))
            .expect("test publication allocates")
    }

    #[test]
    fn every_open_operation_is_distinct_while_source_can_be_shared() {
        let mut registry =
            OpenLifecycleRegistryV1::new_v1(OpenLifecycleRegistryLimitsV1::for_tests_v1());
        let source = source(1);
        registry
            .register_operation_v1(source, operation(10), request(20))
            .unwrap();
        registry
            .register_operation_v1(source, operation(11), request(21))
            .unwrap();

        assert_eq!(registry.source_count_v1(), 1);
        assert_eq!(registry.operation_count_v1(), 2);
        assert_eq!(registry.source_operation_count_v1(source), Some(2));
        assert_eq!(
            registry.operation_request_id_v1(operation(10)),
            Some(request(20))
        );
        assert_eq!(
            registry.operation_request_id_v1(operation(11)),
            Some(request(21))
        );
    }

    #[test]
    fn exact_store_reverse_mapping_distinguishes_same_recording_id() {
        let mut registry =
            OpenLifecycleRegistryV1::new_v1(OpenLifecycleRegistryLimitsV1::for_tests_v1());
        let source = source(1);
        registry
            .register_operation_v1(source, operation(10), request(20))
            .unwrap();
        let first = publication("app-a", "same-recording");
        let second = publication("app-b", "same-recording");
        registry
            .publish_recording_v1(operation(10), recording(30), &first)
            .unwrap();
        registry
            .publish_recording_v1(operation(10), recording(31), &second)
            .unwrap();

        assert_eq!(
            registry.lookup_recording_by_store_v1(&first.fixed_target()),
            Some(recording(30))
        );
        assert_eq!(
            registry.lookup_recording_by_store_v1(&second.fixed_target()),
            Some(recording(31))
        );
        assert_eq!(registry.recording_count_v1(), 2);
    }

    #[test]
    fn removed_tombstone_lifetime_follows_explicit_subscriber_dispose() {
        let mut registry =
            OpenLifecycleRegistryV1::new_v1(OpenLifecycleRegistryLimitsV1::for_tests_v1());
        let source = source(1);
        registry
            .register_operation_v1(source, operation(10), request(20))
            .unwrap();
        registry
            .register_operation_v1(source, operation(11), request(21))
            .unwrap();
        let publication = publication("app-a", "rec-a");
        registry
            .publish_recording_v1(operation(10), recording(30), &publication)
            .unwrap();
        registry
            .attach_recording_v1(operation(11), recording(30))
            .unwrap();

        registry.remove_recording_v1(recording(30)).unwrap();
        assert_eq!(
            registry.recording_state_v1(recording(30)),
            Some(RecordingLifecycleStateV1::Removed)
        );
        assert_eq!(registry.removed_tombstone_count_v1(), 1);
        assert_eq!(
            registry.recording_subscriber_count_v1(recording(30)),
            Some(2)
        );

        registry.dispose_operation_v1(operation(10)).unwrap();
        assert_eq!(
            registry.recording_subscriber_count_v1(recording(30)),
            Some(1)
        );
        assert_eq!(registry.removed_tombstone_count_v1(), 1);

        registry.dispose_operation_v1(operation(11)).unwrap();
        assert_eq!(registry.recording_state_v1(recording(30)), None);
        assert_eq!(registry.removed_tombstone_count_v1(), 0);
        assert_eq!(registry.source_count_v1(), 0);
    }

    #[test]
    fn tombstone_capacity_failure_preserves_live_reverse_mapping() {
        let mut limits = OpenLifecycleRegistryLimitsV1::for_tests_v1();
        limits.max_removed_tombstones = 1;
        let mut registry = OpenLifecycleRegistryV1::new_v1(limits);
        let source = source(1);
        registry
            .register_operation_v1(source, operation(10), request(20))
            .unwrap();
        let first = publication("app-a", "rec-a");
        let second = publication("app-a", "rec-b");
        registry
            .publish_recording_v1(operation(10), recording(30), &first)
            .unwrap();
        registry
            .publish_recording_v1(operation(10), recording(31), &second)
            .unwrap();

        registry.remove_recording_v1(recording(30)).unwrap();
        assert_eq!(
            registry.remove_recording_v1(recording(31)),
            Err(OpenLifecycleRegistryErrorV1::RemovedTombstoneLimitReached)
        );
        assert_eq!(registry.removed_tombstone_count_v1(), 1);
        assert_eq!(
            registry.lookup_recording_by_store_v1(&second.fixed_target()),
            Some(recording(31))
        );
        assert_eq!(
            registry.recording_state_v1(recording(31)),
            Some(RecordingLifecycleStateV1::Active)
        );
    }
}
