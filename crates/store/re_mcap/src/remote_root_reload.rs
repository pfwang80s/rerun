//! Production-disarmed stable root reload and refetch ownership for remote-MCAP Stores.
//!
//! This module is deliberately absent from native/local MCAP production builds. It owns no HTTP
//! client, metadata-opening counters, current-seek counters, or Store facade; it only reuses the
//! already-registered [`crate::remote_partition_residency::RefetchableRootIndexV1`] identity and
//! the exact external-root insertion seam.

use std::collections::BTreeSet;

use re_chunk::ChunkId;
use re_chunk_store::{ChunkStore, ChunkStoreEvent, WebRemoteMcapRootCapabilityV1};

use crate::remote_manifest::DerivationPartitionKeyV1;
use crate::remote_partition_job::RemotePartitionInsertionV1;
use crate::remote_partition_residency::{
    PartitionResidencyV1, RefetchableRootIndexV1, RefetchableRootLoadStateV1,
    RootRegistrationErrorV1,
};

/// Demand layer that owns a reload attempt.
///
/// This is intentionally not the metadata-opening or current-seek retry policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum RemoteRootReloadDemandClassV1 {
    PresentationRequired,
    PlaybackResumeRequired,
}

impl RemoteRootReloadDemandClassV1 {
    pub(crate) const fn display_name_v1(self) -> &'static str {
        match self {
            Self::PresentationRequired => "presentation_required",
            Self::PlaybackResumeRequired => "playback_resume_required",
        }
    }
}

/// Frozen retry-policy inputs for one refetch phase owner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RemoteRootReloadPolicyInputsV1 {
    pub(crate) cumulative_range_requests: u64,
    pub(crate) active_visible_deadline_millis: u64,
}

/// Frozen identity for one refetch phase owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RemoteRootReloadPhaseIdentityV1 {
    partition: DerivationPartitionKeyV1,
    source_generation: u64,
    class: RemoteRootReloadDemandClassV1,
}

impl RemoteRootReloadPhaseIdentityV1 {
    pub(crate) fn new_v1(
        partition: DerivationPartitionKeyV1,
        class: RemoteRootReloadDemandClassV1,
    ) -> Self {
        Self {
            partition,
            source_generation: partition.source_generation_v1(),
            class,
        }
    }

    pub(crate) const fn partition_v1(self) -> DerivationPartitionKeyV1 {
        self.partition
    }

    pub(crate) const fn source_generation_v1(self) -> u64 {
        self.source_generation
    }

    pub(crate) const fn class_v1(self) -> RemoteRootReloadDemandClassV1 {
        self.class
    }
}

/// Refetch phase-local retry budgets.
///
/// The three counters are owned by this reload phase and never alias metadata-opening or
/// current-seek counters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteRootReloadPhaseOwnerV1 {
    identity: RemoteRootReloadPhaseIdentityV1,
    attempts_remaining: u64,
    range_requests_remaining: u64,
    active_visible_deadline_remaining: u64,
}

impl RemoteRootReloadPhaseOwnerV1 {
    pub(crate) fn bind_v1(
        identity: RemoteRootReloadPhaseIdentityV1,
        policy_inputs: RemoteRootReloadPolicyInputsV1,
        attempts_remaining: u64,
        range_requests_remaining: u64,
        active_visible_deadline_remaining: u64,
    ) -> Result<Self, RemoteRootReloadErrorV1> {
        if range_requests_remaining > policy_inputs.cumulative_range_requests
            || active_visible_deadline_remaining > policy_inputs.active_visible_deadline_millis
        {
            return Err(RemoteRootReloadErrorV1::PhaseOwnerMismatch);
        }
        Ok(Self {
            identity,
            attempts_remaining,
            range_requests_remaining,
            active_visible_deadline_remaining,
        })
    }

    pub(crate) const fn identity_v1(self) -> RemoteRootReloadPhaseIdentityV1 {
        self.identity
    }

    pub(crate) const fn attempts_remaining_v1(self) -> u64 {
        self.attempts_remaining
    }

    pub(crate) const fn range_requests_remaining_v1(self) -> u64 {
        self.range_requests_remaining
    }

    pub(crate) const fn active_visible_deadline_remaining_v1(self) -> u64 {
        self.active_visible_deadline_remaining
    }

    fn validate_partition_v1(
        &self,
        partition: DerivationPartitionKeyV1,
    ) -> Result<(), RemoteRootReloadErrorV1> {
        if self.identity.partition != partition
            || self.identity.source_generation != partition.source_generation_v1()
        {
            return Err(RemoteRootReloadErrorV1::PhaseOwnerMismatch);
        }
        Ok(())
    }

    fn validate_admission_v1(&self) -> Result<(), RemoteRootReloadErrorV1> {
        if self.attempts_remaining == 0 {
            return Err(RemoteRootReloadErrorV1::PhaseFailure(
                RemoteRootReloadPhaseFailureV1 {
                    class: self.identity.class,
                    kind: RemoteRootReloadFailureV1::Attempt,
                },
            ));
        }
        if self.range_requests_remaining == 0 {
            return Err(RemoteRootReloadErrorV1::PhaseFailure(
                RemoteRootReloadPhaseFailureV1 {
                    class: self.identity.class,
                    kind: RemoteRootReloadFailureV1::RangeRequest,
                },
            ));
        }
        if self.active_visible_deadline_remaining == 0 {
            return Err(RemoteRootReloadErrorV1::PhaseFailure(
                RemoteRootReloadPhaseFailureV1 {
                    class: self.identity.class,
                    kind: RemoteRootReloadFailureV1::ActiveVisibleDeadline,
                },
            ));
        }
        Ok(())
    }

    pub(crate) fn record_failure_v1(
        &mut self,
        failure: RemoteRootReloadFailureV1,
    ) -> Result<(), RemoteRootReloadErrorV1> {
        let exhausted = |kind| RemoteRootReloadPhaseFailureV1 {
            class: self.identity.class,
            kind,
        };
        match failure {
            RemoteRootReloadFailureV1::Attempt => {
                if self.attempts_remaining == 0 {
                    return Err(RemoteRootReloadErrorV1::PhaseFailure(exhausted(
                        RemoteRootReloadFailureV1::Attempt,
                    )));
                }
                self.attempts_remaining -= 1;
            }
            RemoteRootReloadFailureV1::RangeRequest => {
                if self.range_requests_remaining == 0 {
                    return Err(RemoteRootReloadErrorV1::PhaseFailure(exhausted(
                        RemoteRootReloadFailureV1::RangeRequest,
                    )));
                }
                self.range_requests_remaining -= 1;
            }
            RemoteRootReloadFailureV1::ActiveVisibleDeadline => {
                if self.active_visible_deadline_remaining == 0 {
                    return Err(RemoteRootReloadErrorV1::PhaseFailure(exhausted(
                        RemoteRootReloadFailureV1::ActiveVisibleDeadline,
                    )));
                }
                self.active_visible_deadline_remaining -= 1;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteRootReloadFailureV1 {
    Attempt,
    RangeRequest,
    ActiveVisibleDeadline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteRootReloadPhaseFailureV1 {
    class: RemoteRootReloadDemandClassV1,
    kind: RemoteRootReloadFailureV1,
}

impl RemoteRootReloadPhaseFailureV1 {
    pub(crate) const fn class_v1(self) -> RemoteRootReloadDemandClassV1 {
        self.class
    }

    pub(crate) const fn kind_v1(self) -> RemoteRootReloadFailureV1 {
        self.kind
    }
}

/// Resource caps for one production-disarmed reload attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteRootReloadLimitsV1 {
    pub(crate) max_reload_roots: u64,
    pub(crate) max_reload_bytes: u64,
    pub(crate) max_insertion_calls_per_reload: u64,
}

impl RemoteRootReloadLimitsV1 {
    pub(crate) const fn generous_disarmed_v1() -> Self {
        Self {
            max_reload_roots: 1_000_000,
            max_reload_bytes: u64::MAX,
            max_insertion_calls_per_reload: 1_000_000,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum RemoteRootReloadErrorV1 {
    #[error("remote MCAP root reload arithmetic overflowed")]
    ArithmeticOverflow,

    #[error("remote MCAP root reload exceeds a resource limit")]
    ResourceLimitExceeded,

    #[error("remote MCAP root reload references an invalid registered partition")]
    InvalidReload,

    #[error("remote MCAP root reload phase owner does not match")]
    PhaseOwnerMismatch,

    #[error("remote MCAP root reload exhausted its matching phase budget")]
    PhaseFailure(RemoteRootReloadPhaseFailureV1),

    #[error("remote MCAP root reload is already resident")]
    AlreadyResident,

    #[error("remote MCAP root reload is already in transit")]
    ReloadInProgress,

    #[error(transparent)]
    Registration(RootRegistrationErrorV1),

    #[error("the Web remote-MCAP Store capability is unavailable")]
    StoreCapability,

    #[error("remote MCAP root reload became partially mutated and is terminal gated")]
    InsertionGated,

    #[error("remote MCAP root reload capability is terminalized")]
    Terminalized,
}

impl From<RootRegistrationErrorV1> for RemoteRootReloadErrorV1 {
    fn from(error: RootRegistrationErrorV1) -> Self {
        Self::Registration(error)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteRootReloadOutcomeV1 {
    partition: DerivationPartitionKeyV1,
    root_chunk_ids: BTreeSet<ChunkId>,
}

impl RemoteRootReloadOutcomeV1 {
    pub(crate) const fn partition_v1(&self) -> DerivationPartitionKeyV1 {
        self.partition
    }

    pub(crate) fn root_chunk_ids_v1(&self) -> &BTreeSet<ChunkId> {
        &self.root_chunk_ids
    }
}

/// Fail-closed owner of the dynamic refetch capability.
///
/// [`Self::terminate_v1`] revokes the underlying [`WebRemoteMcapRootCapabilityV1`] before any
/// further reload is admitted.
pub(crate) struct RemoteRootReloadControllerV1 {
    root_index: RefetchableRootIndexV1,
    capability: Option<WebRemoteMcapRootCapabilityV1>,
    limits: RemoteRootReloadLimitsV1,
    terminal: bool,
}

impl RemoteRootReloadControllerV1 {
    pub(crate) fn new_v1(
        root_index: RefetchableRootIndexV1,
        capability: WebRemoteMcapRootCapabilityV1,
        limits: RemoteRootReloadLimitsV1,
    ) -> Self {
        Self {
            root_index,
            capability: Some(capability),
            limits,
            terminal: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn root_index_v1(&self) -> &RefetchableRootIndexV1 {
        &self.root_index
    }

    pub(crate) fn terminate_v1(&mut self) {
        if self.terminal {
            return;
        }
        self.terminal = true;
        if let Some(capability) = self.capability.take() {
            capability.revoke_v1();
        }
    }

    pub(crate) fn observe_store_events_v1(
        &mut self,
        store: &ChunkStore,
        events: &[ChunkStoreEvent],
    ) -> Result<(), RemoteRootReloadErrorV1> {
        if self.terminal {
            return Err(RemoteRootReloadErrorV1::Terminalized);
        }
        let capability = self
            .capability
            .as_ref()
            .ok_or(RemoteRootReloadErrorV1::Terminalized)?;
        self.root_index
            .observe_store_events_v1(store, capability, events)
            .map_err(RemoteRootReloadErrorV1::Registration)
    }

    pub(crate) fn reload_partition_v1(
        &mut self,
        store: &mut ChunkStore,
        insertion: &RemotePartitionInsertionV1,
        owner: &mut RemoteRootReloadPhaseOwnerV1,
    ) -> Result<RemoteRootReloadOutcomeV1, RemoteRootReloadErrorV1> {
        if self.terminal {
            return Err(RemoteRootReloadErrorV1::Terminalized);
        }
        let partition = insertion.partition_v1().key_v1();
        owner.validate_partition_v1(partition)?;
        owner.validate_admission_v1()?;

        let capability = self
            .capability
            .as_ref()
            .ok_or(RemoteRootReloadErrorV1::Terminalized)?;
        validate_reload_partition_v1(&self.root_index, store, capability, insertion, partition)?;

        let root_count = insertion
            .root_count_v1()
            .map_err(|_error| RemoteRootReloadErrorV1::InvalidReload)?;
        let reload_bytes = insertion
            .total_size_bytes_v1()
            .map_err(|_error| RemoteRootReloadErrorV1::InvalidReload)?;
        if root_count > self.limits.max_reload_roots
            || reload_bytes > self.limits.max_reload_bytes
            || root_count > self.limits.max_insertion_calls_per_reload
        {
            return Err(RemoteRootReloadErrorV1::ResourceLimitExceeded);
        }

        let events =
            match insert_reload_roots_v1(&mut self.root_index, capability, store, insertion) {
                Ok(events) => events,
                Err(error) => return Err(record_reload_failure_v1(owner, error)),
            };
        self.root_index
            .observe_store_events_v1(store, capability, &events)
            .map_err(RemoteRootReloadErrorV1::Registration)?;
        if !self
            .root_index
            .is_partition_fully_resident_v1(store, capability, partition)
            .map_err(RemoteRootReloadErrorV1::Registration)?
        {
            return Err(record_reload_failure_v1(
                owner,
                RemoteRootReloadErrorV1::InsertionGated,
            ));
        }

        Ok(RemoteRootReloadOutcomeV1 {
            partition,
            root_chunk_ids: insertion.root_chunk_ids_v1(),
        })
    }
}

fn record_reload_failure_v1(
    owner: &mut RemoteRootReloadPhaseOwnerV1,
    error: RemoteRootReloadErrorV1,
) -> RemoteRootReloadErrorV1 {
    let Some(failure) = reload_failure_class_v1(&error) else {
        return error;
    };
    match owner.record_failure_v1(failure) {
        Ok(()) => error,
        Err(phase_failure) => phase_failure,
    }
}

fn reload_failure_class_v1(error: &RemoteRootReloadErrorV1) -> Option<RemoteRootReloadFailureV1> {
    // This module owns no Fetch/decode transport. In this production-disarmed layer the only
    // retryable failures it can observe are the refetch-capability seam and partial Store
    // insertion. Those map to RangeRequest and Attempt respectively. The active-visible deadline
    // is wall-clock policy and therefore remains caller-driven through
    // [`RemoteRootReloadPhaseOwnerV1::record_failure_v1`].
    match error {
        RemoteRootReloadErrorV1::StoreCapability => Some(RemoteRootReloadFailureV1::RangeRequest),
        RemoteRootReloadErrorV1::InsertionGated => Some(RemoteRootReloadFailureV1::Attempt),
        _ => None,
    }
}

fn validate_reload_partition_v1(
    root_index: &RefetchableRootIndexV1,
    store: &ChunkStore,
    capability: &WebRemoteMcapRootCapabilityV1,
    insertion: &RemotePartitionInsertionV1,
    partition: DerivationPartitionKeyV1,
) -> Result<(), RemoteRootReloadErrorV1> {
    if insertion.partition_v1().key_v1() != partition {
        return Err(RemoteRootReloadErrorV1::InvalidReload);
    }
    if root_index
        .is_partition_fully_resident_v1(store, capability, partition)
        .map_err(RemoteRootReloadErrorV1::Registration)?
    {
        return Err(RemoteRootReloadErrorV1::AlreadyResident);
    }
    let registered_root_ids = match root_index.partition_residency_v1(partition) {
        PartitionResidencyV1::Roots(root_ids) => root_ids.iter().copied().collect::<BTreeSet<_>>(),
        PartitionResidencyV1::InTransit => {
            return Err(RemoteRootReloadErrorV1::ReloadInProgress);
        }
        PartitionResidencyV1::CompleteEmpty | PartitionResidencyV1::Unknown => {
            return Err(RemoteRootReloadErrorV1::InvalidReload);
        }
    };
    let insertion_root_ids = insertion.root_chunk_ids_v1();
    if insertion_root_ids.is_empty() || !insertion_root_ids.is_subset(&registered_root_ids) {
        return Err(RemoteRootReloadErrorV1::InvalidReload);
    }

    let mut has_unloaded_root = false;
    for root in insertion.roots_v1() {
        let root_chunk_id = root.root_v1().root_chunk_id_v1();
        let (registered_descriptor, registered_is_static) = root_index
            .root_manifest_registration_v1(root_chunk_id)
            .ok_or(RemoteRootReloadErrorV1::InvalidReload)?;
        if registered_descriptor != root.root_v1()
            || registered_is_static != root.chunk_v1().is_static()
        {
            return Err(RemoteRootReloadErrorV1::InvalidReload);
        }
        match root_index.root_load_state_v1(root_chunk_id) {
            Some(RefetchableRootLoadStateV1::Unloaded) => has_unloaded_root = true,
            Some(RefetchableRootLoadStateV1::FullyLoaded) => {}
            Some(RefetchableRootLoadStateV1::InTransit) | None => {
                return Err(RemoteRootReloadErrorV1::ReloadInProgress);
            }
        }
    }
    if !has_unloaded_root {
        return Err(RemoteRootReloadErrorV1::InvalidReload);
    }
    Ok(())
}

fn insert_reload_roots_v1(
    root_index: &mut RefetchableRootIndexV1,
    capability: &WebRemoteMcapRootCapabilityV1,
    store: &mut ChunkStore,
    insertion: &RemotePartitionInsertionV1,
) -> Result<Vec<ChunkStoreEvent>, RemoteRootReloadErrorV1> {
    let mut pending_events = Vec::new();
    for root in insertion.roots_v1() {
        let root_chunk_id = root.root_v1().root_chunk_id_v1();
        match root_index.root_load_state_v1(root_chunk_id) {
            Some(RefetchableRootLoadStateV1::FullyLoaded) => continue,
            Some(RefetchableRootLoadStateV1::Unloaded) => {}
            Some(RefetchableRootLoadStateV1::InTransit) | None => {
                return Err(RemoteRootReloadErrorV1::ReloadInProgress);
            }
        }
        let descriptor = root_index
            .root_refetch_descriptor_v1(root_chunk_id)
            .ok_or(RemoteRootReloadErrorV1::InvalidReload)?;
        let permit = match capability.issue_refetch_v1(store, descriptor) {
            Ok(permit) => permit,
            Err(_error) => {
                if !pending_events.is_empty() {
                    let _reconciled =
                        root_index.observe_store_events_v1(store, capability, &pending_events);
                }
                return Err(RemoteRootReloadErrorV1::StoreCapability);
            }
        };
        match store.insert_external_refetchable_root_v1(permit, root.chunk_v1()) {
            Ok(events) => pending_events.extend(events),
            Err(_error) => {
                if !pending_events.is_empty() {
                    let _reconciled =
                        root_index.observe_store_events_v1(store, capability, &pending_events);
                }
                return Err(RemoteRootReloadErrorV1::InsertionGated);
            }
        }
    }
    Ok(pending_events)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use re_byte_size::SizeBytes as _;
    use re_chunk::{Chunk, ChunkId};
    use re_chunk_store::{
        ChunkTrackingMode, GarbageCollectionOptions, LatestAtQuery, WebRemoteMcapStoreConfigV1,
    };
    use re_log_types::{EntityPath, StoreId, StoreKind, Timeline};
    use re_sdk_types::archetypes;

    use crate::remote_channel_group::{CanonicalSourceOrderKeyV1, StableDecoderGroupIdV1};
    use crate::remote_loaded_coverage::RemoteTemporalCoveragePlanV1;
    use crate::remote_manifest::{
        DerivationPartitionKindV1, ManifestPartitionDescriptorV1, ManifestRootDescriptorV1,
        RemoteMcapSessionIdV1, RemoteRegistrationCapacityV1,
    };
    use crate::remote_partition_job::RemotePartitionRootInsertionV1;
    use crate::remote_partition_residency::{
        PreparedPartitionRegistrationV1, RefetchableRootLoadStateV1,
    };

    use super::*;

    type RegisteredTwoRootFixtureV1 = (
        ChunkStore,
        WebRemoteMcapRootCapabilityV1,
        RefetchableRootIndexV1,
        ManifestPartitionDescriptorV1,
        Vec<ChunkId>,
        Vec<Arc<Chunk>>,
        Vec<ManifestRootDescriptorV1>,
        RemotePartitionInsertionV1,
    );

    fn session_id() -> RemoteMcapSessionIdV1 {
        RemoteMcapSessionIdV1::for_registration_test_v1(92)
    }

    fn temporal_partition(
        session: RemoteMcapSessionIdV1,
    ) -> (
        ManifestPartitionDescriptorV1,
        crate::remote_manifest::ManifestRootDescriptorIssuerV1,
    ) {
        ManifestPartitionDescriptorV1::for_registration_test_v1(
            session,
            0,
            DerivationPartitionKindV1::TemporalChannelGroup(
                StableDecoderGroupIdV1::first_for_assignment_test_v1(),
            ),
            1,
        )
    }

    fn tied_chunk(root_chunk_id: re_chunk::ChunkId) -> Arc<Chunk> {
        tied_chunk_with_offsets(
            root_chunk_id,
            [
                (8_u64, 1_u64, 0_u32, 7_i64),
                (9_u64, 2_u64, 0_u32, 7_i64),
                (10_u64, 3_u64, 0_u32, 7_i64),
            ],
        )
    }

    fn tied_chunk_with_offsets(
        root_chunk_id: ChunkId,
        offsets: [(u64, u64, u32, i64); 3],
    ) -> Arc<Chunk> {
        let mut builder = Chunk::builder_with_id(root_chunk_id, "world/points");
        for (top_level_offset, record_local_offset, derived_ordinal, time) in offsets {
            let row_id = CanonicalSourceOrderKeyV1::new(
                top_level_offset,
                record_local_offset,
                derived_ordinal,
            )
            .unwrap()
            .stable_row_id();
            builder = builder.with_archetype(
                row_id,
                [(Timeline::log_tick(), time)],
                &archetypes::Points3D::new([[
                    top_level_offset as f32,
                    record_local_offset as f32,
                    derived_ordinal as f32,
                ]]),
            );
        }
        Arc::new(builder.build().unwrap())
    }

    fn root_ids(results: &re_chunk_store::QueryResults) -> BTreeSet<ChunkId> {
        results.chunks.iter().map(|chunk| chunk.id()).collect()
    }

    fn latest_at_overlap_ids(store: &ChunkStore) -> BTreeSet<ChunkId> {
        let query = LatestAtQuery::new(*Timeline::log_tick().name(), 7);
        let results = store.latest_at_relevant_chunks_for_all_components(
            ChunkTrackingMode::Ignore,
            &query,
            &EntityPath::from("world/points"),
            false,
        );
        assert!(!results.is_partial());
        root_ids(&results)
    }

    fn store_and_capability() -> (ChunkStore, WebRemoteMcapRootCapabilityV1) {
        WebRemoteMcapStoreConfigV1::for_test_v1()
            .into_store_v1(StoreId::random(StoreKind::Recording, "root-reload-test"))
    }

    fn generous_limits() -> RemoteRegistrationCapacityV1 {
        RemoteRegistrationCapacityV1 {
            max_registered_partitions: 1,
            max_complete_empty_entries: 0,
            max_root_descriptors: 1,
            max_external_origin_bytes:
                re_chunk_store::ExternalRefetchableRootOriginV1::ENCODED_BYTES_V1,
        }
    }

    fn new_index(
        session: RemoteMcapSessionIdV1,
        store: &ChunkStore,
        capability: &WebRemoteMcapRootCapabilityV1,
    ) -> RefetchableRootIndexV1 {
        RefetchableRootIndexV1::new_v1(
            session,
            generous_limits(),
            store,
            capability,
            RemoteTemporalCoveragePlanV1::for_test_v1(&[Some((0, 10))], 1),
        )
        .unwrap()
    }

    fn registered_partition_with_chunk(
        session: RemoteMcapSessionIdV1,
    ) -> (
        ChunkStore,
        WebRemoteMcapRootCapabilityV1,
        RefetchableRootIndexV1,
        re_chunk::ChunkId,
        Arc<Chunk>,
        RemotePartitionInsertionV1,
    ) {
        let (mut store, capability) = store_and_capability();
        let mut index = new_index(session, &store, &capability);
        let (partition, issuer) = temporal_partition(session);
        let root = issuer.descriptor_v1(0).unwrap();
        let chunk = tied_chunk(root.root_chunk_id_v1());
        let insertion = RemotePartitionInsertionV1::seal_v1(
            partition,
            vec![
                RemotePartitionRootInsertionV1::seal_v1(partition, Arc::clone(&chunk), root)
                    .unwrap(),
            ]
            .into_boxed_slice(),
        )
        .unwrap();
        let registration = PreparedPartitionRegistrationV1::complete_roots_for_test_v1(
            partition,
            &[root],
            chunk.total_size_bytes(),
            false,
        )
        .unwrap();
        index
            .register_commit_set_v1(&mut store, &capability, vec![registration])
            .unwrap();
        (
            store,
            capability,
            index,
            root.root_chunk_id_v1(),
            chunk,
            insertion,
        )
    }

    fn registered_partition_with_two_roots(
        session: RemoteMcapSessionIdV1,
    ) -> RegisteredTwoRootFixtureV1 {
        let (mut store, capability) = store_and_capability();
        let limits = RemoteRegistrationCapacityV1 {
            max_registered_partitions: 1,
            max_complete_empty_entries: 0,
            max_root_descriptors: 2,
            max_external_origin_bytes: 2
                * re_chunk_store::ExternalRefetchableRootOriginV1::ENCODED_BYTES_V1,
        };
        let mut index = RefetchableRootIndexV1::new_v1(
            session,
            limits,
            &store,
            &capability,
            RemoteTemporalCoveragePlanV1::for_test_v1(&[Some((0, 10))], 1),
        )
        .unwrap();
        let (partition, issuer) = ManifestPartitionDescriptorV1::for_registration_test_v1(
            session,
            0,
            DerivationPartitionKindV1::TemporalChannelGroup(
                StableDecoderGroupIdV1::first_for_assignment_test_v1(),
            ),
            2,
        );
        let roots = vec![
            issuer.descriptor_v1(0).unwrap(),
            issuer.descriptor_v1(1).unwrap(),
        ];
        let chunks = vec![
            tied_chunk(roots[0].root_chunk_id_v1()),
            tied_chunk_with_offsets(
                roots[1].root_chunk_id_v1(),
                [
                    (20_u64, 1_u64, 0_u32, 7_i64),
                    (21_u64, 2_u64, 0_u32, 7_i64),
                    (22_u64, 3_u64, 0_u32, 7_i64),
                ],
            ),
        ];
        let insertion = RemotePartitionInsertionV1::seal_v1(
            partition,
            vec![
                RemotePartitionRootInsertionV1::seal_v1(
                    partition,
                    Arc::clone(&chunks[0]),
                    roots[0],
                )
                .unwrap(),
                RemotePartitionRootInsertionV1::seal_v1(
                    partition,
                    Arc::clone(&chunks[1]),
                    roots[1],
                )
                .unwrap(),
            ]
            .into_boxed_slice(),
        )
        .unwrap();
        let registration = PreparedPartitionRegistrationV1::complete_roots_for_test_v1(
            partition,
            &roots,
            chunks[0].total_size_bytes(),
            false,
        )
        .unwrap();
        index
            .register_commit_set_v1(&mut store, &capability, vec![registration])
            .unwrap();
        (
            store,
            capability,
            index,
            partition,
            roots.iter().map(|root| root.root_chunk_id_v1()).collect(),
            chunks,
            roots,
            insertion,
        )
    }

    fn insert_root(
        store: &mut ChunkStore,
        capability: &WebRemoteMcapRootCapabilityV1,
        index: &mut RefetchableRootIndexV1,
        chunk: &Arc<Chunk>,
    ) {
        let descriptor = index.root_refetch_descriptor_v1(chunk.id()).unwrap();
        let permit = capability.issue_refetch_v1(store, descriptor).unwrap();
        let events = store
            .insert_external_refetchable_root_v1(permit, chunk)
            .unwrap();
        index
            .observe_store_events_v1(store, capability, &events)
            .unwrap();
    }

    #[test]
    fn reload_after_gc_preserves_stable_chunk_and_row_identity() {
        let session = session_id();
        let (mut store, capability, mut index, root_id, chunk, insertion) =
            registered_partition_with_chunk(session);
        let expected_chunk = (*chunk).clone();
        let expected_row_ids = chunk.row_ids().collect::<Vec<_>>();
        insert_root(&mut store, &capability, &mut index, &chunk);

        assert_eq!(
            index.root_load_state_v1(root_id),
            Some(RefetchableRootLoadStateV1::FullyLoaded)
        );
        assert_eq!(**store.physical_chunk(&root_id).unwrap(), expected_chunk);
        assert_eq!(
            store
                .physical_chunk(&root_id)
                .unwrap()
                .row_ids()
                .collect::<Vec<_>>(),
            expected_row_ids
        );

        let (deletions, _) = store.gc(&GarbageCollectionOptions::gc_everything());
        index
            .observe_store_events_v1(&store, &capability, &deletions)
            .unwrap();
        assert_eq!(
            index.root_load_state_v1(root_id),
            Some(RefetchableRootLoadStateV1::Unloaded)
        );
        assert!(store.physical_chunk(&root_id).is_none());

        let (registered_descriptor, _) = index.root_manifest_registration_v1(root_id).unwrap();
        let fresh_chunk = tied_chunk(root_id);
        let fresh_insertion = RemotePartitionInsertionV1::seal_v1(
            insertion.partition_v1(),
            vec![
                RemotePartitionRootInsertionV1::seal_v1(
                    insertion.partition_v1(),
                    Arc::clone(&fresh_chunk),
                    registered_descriptor,
                )
                .unwrap(),
            ]
            .into_boxed_slice(),
        )
        .unwrap();

        let counters_before_reload = index.counters_v1();
        let mut controller = RemoteRootReloadControllerV1::new_v1(
            index,
            capability,
            RemoteRootReloadLimitsV1::generous_disarmed_v1(),
        );
        let identity = RemoteRootReloadPhaseIdentityV1::new_v1(
            insertion.partition_v1().key_v1(),
            RemoteRootReloadDemandClassV1::PresentationRequired,
        );
        let mut owner = RemoteRootReloadPhaseOwnerV1::bind_v1(
            identity,
            RemoteRootReloadPolicyInputsV1 {
                cumulative_range_requests: 1,
                active_visible_deadline_millis: 1,
            },
            1,
            1,
            1,
        )
        .unwrap();
        let outcome = controller
            .reload_partition_v1(&mut store, &fresh_insertion, &mut owner)
            .unwrap();

        assert_eq!(
            outcome.root_chunk_ids_v1(),
            &std::iter::once(root_id).collect::<BTreeSet<_>>()
        );
        assert_eq!(**store.physical_chunk(&root_id).unwrap(), expected_chunk);
        assert_eq!(
            store
                .physical_chunk(&root_id)
                .unwrap()
                .row_ids()
                .collect::<Vec<_>>(),
            expected_row_ids
        );
        assert_eq!(
            controller.root_index_v1().root_load_state_v1(root_id),
            Some(RefetchableRootLoadStateV1::FullyLoaded)
        );
        assert_eq!(
            controller.root_index_v1().counters_v1(),
            counters_before_reload
        );
    }

    #[test]
    fn reload_reconstructs_overlapping_chunks_from_fresh_insertion() {
        let session = session_id();
        let (mut store, capability, mut index, partition, root_ids, chunks, roots, insertion) =
            registered_partition_with_two_roots(session);
        for chunk in &chunks {
            insert_root(&mut store, &capability, &mut index, chunk);
        }

        let expected_row_ids = chunks
            .iter()
            .map(|chunk| chunk.row_ids().collect::<Vec<_>>())
            .collect::<Vec<_>>();
        let expected_overlap_ids = latest_at_overlap_ids(&store);
        assert_eq!(
            expected_overlap_ids,
            root_ids.iter().copied().collect::<BTreeSet<_>>()
        );

        let (deletions, _) = store.gc(&GarbageCollectionOptions::gc_everything());
        index
            .observe_store_events_v1(&store, &capability, &deletions)
            .unwrap();

        let fresh_chunks = vec![
            tied_chunk(root_ids[0]),
            tied_chunk_with_offsets(
                root_ids[1],
                [
                    (20_u64, 1_u64, 0_u32, 7_i64),
                    (21_u64, 2_u64, 0_u32, 7_i64),
                    (22_u64, 3_u64, 0_u32, 7_i64),
                ],
            ),
        ];
        let fresh_insertion = RemotePartitionInsertionV1::seal_v1(
            partition,
            vec![
                RemotePartitionRootInsertionV1::seal_v1(
                    partition,
                    Arc::clone(&fresh_chunks[0]),
                    roots[0],
                )
                .unwrap(),
                RemotePartitionRootInsertionV1::seal_v1(
                    partition,
                    Arc::clone(&fresh_chunks[1]),
                    roots[1],
                )
                .unwrap(),
            ]
            .into_boxed_slice(),
        )
        .unwrap();
        let counters_before_reload = index.counters_v1();
        let mut controller = RemoteRootReloadControllerV1::new_v1(
            index,
            capability,
            RemoteRootReloadLimitsV1::generous_disarmed_v1(),
        );
        let identity = RemoteRootReloadPhaseIdentityV1::new_v1(
            insertion.partition_v1().key_v1(),
            RemoteRootReloadDemandClassV1::PresentationRequired,
        );
        let mut owner = RemoteRootReloadPhaseOwnerV1::bind_v1(
            identity,
            RemoteRootReloadPolicyInputsV1 {
                cumulative_range_requests: 2,
                active_visible_deadline_millis: 1,
            },
            1,
            2,
            1,
        )
        .unwrap();

        controller
            .reload_partition_v1(&mut store, &fresh_insertion, &mut owner)
            .unwrap();

        for (root_id, fresh_chunk) in root_ids.iter().zip(&fresh_chunks) {
            assert!(Arc::ptr_eq(
                store.physical_chunk(root_id).unwrap(),
                fresh_chunk
            ));
        }
        for (root_id, expected_rows) in root_ids.iter().zip(&expected_row_ids) {
            assert_eq!(
                store
                    .physical_chunk(root_id)
                    .unwrap()
                    .row_ids()
                    .collect::<Vec<_>>(),
                *expected_rows
            );
        }
        assert_eq!(latest_at_overlap_ids(&store), expected_overlap_ids);
        assert_eq!(
            controller.root_index_v1().counters_v1(),
            counters_before_reload
        );
    }

    #[test]
    fn partial_gc_reload_skips_resident_roots_and_accepts_matching_subset() {
        let session = session_id();
        let (mut store, capability, mut index, partition, root_ids, chunks, _roots, insertion) =
            registered_partition_with_two_roots(session);
        for chunk in &chunks {
            insert_root(&mut store, &capability, &mut index, chunk);
        }

        let mut options = GarbageCollectionOptions::gc_everything();
        options.protected_chunks.insert(root_ids[0]);
        let (deletions, _) = store.gc(&options);
        index
            .observe_store_events_v1(&store, &capability, &deletions)
            .unwrap();
        assert!(store.physical_chunk(&root_ids[0]).is_some());
        assert!(store.physical_chunk(&root_ids[1]).is_none());

        let mut controller = RemoteRootReloadControllerV1::new_v1(
            index,
            capability,
            RemoteRootReloadLimitsV1::generous_disarmed_v1(),
        );
        let identity = RemoteRootReloadPhaseIdentityV1::new_v1(
            insertion.partition_v1().key_v1(),
            RemoteRootReloadDemandClassV1::PresentationRequired,
        );
        let mut owner = RemoteRootReloadPhaseOwnerV1::bind_v1(
            identity,
            RemoteRootReloadPolicyInputsV1 {
                cumulative_range_requests: 2,
                active_visible_deadline_millis: 1,
            },
            1,
            2,
            1,
        )
        .unwrap();

        let outcome = controller
            .reload_partition_v1(&mut store, &insertion, &mut owner)
            .unwrap();
        assert_eq!(
            outcome.root_chunk_ids_v1(),
            &root_ids.iter().copied().collect::<BTreeSet<_>>()
        );
        assert!(Arc::ptr_eq(
            store.physical_chunk(&root_ids[0]).unwrap(),
            &chunks[0]
        ));
        assert!(store.physical_chunk(&root_ids[1]).is_some());

        let mut options = GarbageCollectionOptions::gc_everything();
        options.protected_chunks.insert(root_ids[0]);
        let (deletions, _) = store.gc(&options);
        controller
            .observe_store_events_v1(&store, &deletions)
            .unwrap();
        assert!(store.physical_chunk(&root_ids[1]).is_none());

        let (missing_descriptor, _) = controller
            .root_index_v1()
            .root_manifest_registration_v1(root_ids[1])
            .unwrap();
        let subset_insertion = RemotePartitionInsertionV1::seal_v1(
            partition,
            vec![
                RemotePartitionRootInsertionV1::seal_v1(
                    partition,
                    tied_chunk(root_ids[1]),
                    missing_descriptor,
                )
                .unwrap(),
            ]
            .into_boxed_slice(),
        )
        .unwrap();
        let mut owner = RemoteRootReloadPhaseOwnerV1::bind_v1(
            identity,
            RemoteRootReloadPolicyInputsV1 {
                cumulative_range_requests: 2,
                active_visible_deadline_millis: 1,
            },
            1,
            2,
            1,
        )
        .unwrap();
        let outcome = controller
            .reload_partition_v1(&mut store, &subset_insertion, &mut owner)
            .unwrap();
        assert_eq!(
            outcome.root_chunk_ids_v1(),
            &std::iter::once(root_ids[1]).collect::<BTreeSet<_>>()
        );
        assert!(store.physical_chunk(&root_ids[1]).is_some());
        assert_eq!(
            controller.root_index_v1().root_load_state_v1(root_ids[0]),
            Some(RefetchableRootLoadStateV1::FullyLoaded)
        );
        assert_eq!(
            controller.root_index_v1().root_load_state_v1(root_ids[1]),
            Some(RefetchableRootLoadStateV1::FullyLoaded)
        );
    }

    #[test]
    fn exhausted_reload_owner_is_rejected_before_mutation() {
        let session = session_id();
        let (mut store, capability, mut index, root_id, _chunk, insertion) =
            registered_partition_with_chunk(session);
        let (deletions, _) = store.gc(&GarbageCollectionOptions::gc_everything());
        index
            .observe_store_events_v1(&store, &capability, &deletions)
            .unwrap();

        let mut controller = RemoteRootReloadControllerV1::new_v1(
            index,
            capability,
            RemoteRootReloadLimitsV1::generous_disarmed_v1(),
        );
        let identity = RemoteRootReloadPhaseIdentityV1::new_v1(
            insertion.partition_v1().key_v1(),
            RemoteRootReloadDemandClassV1::PlaybackResumeRequired,
        );
        let cases = [
            (0, 1, 1, RemoteRootReloadFailureV1::Attempt),
            (1, 0, 1, RemoteRootReloadFailureV1::RangeRequest),
            (1, 1, 0, RemoteRootReloadFailureV1::ActiveVisibleDeadline),
        ];
        for (attempts, range_requests, deadline, kind) in cases {
            let mut owner = RemoteRootReloadPhaseOwnerV1::bind_v1(
                identity,
                RemoteRootReloadPolicyInputsV1 {
                    cumulative_range_requests: 1,
                    active_visible_deadline_millis: 1,
                },
                attempts,
                range_requests,
                deadline,
            )
            .unwrap();
            assert_eq!(
                controller.reload_partition_v1(&mut store, &insertion, &mut owner),
                Err(RemoteRootReloadErrorV1::PhaseFailure(
                    RemoteRootReloadPhaseFailureV1 {
                        class: RemoteRootReloadDemandClassV1::PlaybackResumeRequired,
                        kind,
                    },
                ))
            );
            assert!(store.physical_chunk(&root_id).is_none());
            assert_eq!(
                controller.root_index_v1().root_load_state_v1(root_id),
                Some(RefetchableRootLoadStateV1::Unloaded)
            );
        }
    }

    #[test]
    fn reload_refetch_failure_consumes_range_budget_and_reconciles_pending_events() {
        let session = session_id();
        let (mut store, capability, index, _partition, root_ids, chunks, _roots, insertion) =
            registered_partition_with_two_roots(session);

        // Make root 1 resident behind the index's back, while root 0 remains unloaded. During
        // reload, root 0 succeeds first and must be reconciled before root 1's refetch failure.
        let descriptor = index.root_refetch_descriptor_v1(root_ids[1]).unwrap();
        let permit = capability.issue_refetch_v1(&store, descriptor).unwrap();
        let _events = store
            .insert_external_refetchable_root_v1(permit, &chunks[1])
            .unwrap();
        assert_eq!(
            index.root_load_state_v1(root_ids[0]),
            Some(RefetchableRootLoadStateV1::Unloaded)
        );
        assert_eq!(
            index.root_load_state_v1(root_ids[1]),
            Some(RefetchableRootLoadStateV1::Unloaded)
        );

        let mut controller = RemoteRootReloadControllerV1::new_v1(
            index,
            capability,
            RemoteRootReloadLimitsV1::generous_disarmed_v1(),
        );
        let identity = RemoteRootReloadPhaseIdentityV1::new_v1(
            insertion.partition_v1().key_v1(),
            RemoteRootReloadDemandClassV1::PresentationRequired,
        );
        let mut owner = RemoteRootReloadPhaseOwnerV1::bind_v1(
            identity,
            RemoteRootReloadPolicyInputsV1 {
                cumulative_range_requests: 2,
                active_visible_deadline_millis: 1,
            },
            1,
            2,
            1,
        )
        .unwrap();

        assert_eq!(
            controller.reload_partition_v1(&mut store, &insertion, &mut owner),
            Err(RemoteRootReloadErrorV1::StoreCapability)
        );
        assert_eq!(owner.range_requests_remaining_v1(), 1);
        assert_eq!(owner.attempts_remaining_v1(), 1);
        assert_eq!(owner.active_visible_deadline_remaining_v1(), 1);
        assert_eq!(
            controller.root_index_v1().root_load_state_v1(root_ids[0]),
            Some(RefetchableRootLoadStateV1::FullyLoaded)
        );
        assert_eq!(
            controller.root_index_v1().root_load_state_v1(root_ids[1]),
            Some(RefetchableRootLoadStateV1::FullyLoaded)
        );
        assert!(store.physical_chunk(&root_ids[0]).is_some());
        assert!(store.physical_chunk(&root_ids[1]).is_some());
    }

    #[test]
    fn terminalized_reload_revokes_refetch_before_mutation() {
        let session = session_id();
        let (mut store, capability, mut index, root_id, _chunk, insertion) =
            registered_partition_with_chunk(session);
        let (deletions, _) = store.gc(&GarbageCollectionOptions::gc_everything());
        index
            .observe_store_events_v1(&store, &capability, &deletions)
            .unwrap();

        let mut controller = RemoteRootReloadControllerV1::new_v1(
            index,
            capability,
            RemoteRootReloadLimitsV1::generous_disarmed_v1(),
        );
        controller.terminate_v1();
        let identity = RemoteRootReloadPhaseIdentityV1::new_v1(
            insertion.partition_v1().key_v1(),
            RemoteRootReloadDemandClassV1::PresentationRequired,
        );
        let mut owner = RemoteRootReloadPhaseOwnerV1::bind_v1(
            identity,
            RemoteRootReloadPolicyInputsV1 {
                cumulative_range_requests: 1,
                active_visible_deadline_millis: 1,
            },
            1,
            1,
            1,
        )
        .unwrap();

        assert_eq!(
            controller.reload_partition_v1(&mut store, &insertion, &mut owner),
            Err(RemoteRootReloadErrorV1::Terminalized)
        );
        assert!(store.physical_chunk(&root_id).is_none());
        assert_eq!(
            controller.root_index_v1().root_load_state_v1(root_id),
            Some(RefetchableRootLoadStateV1::Unloaded)
        );
    }

    #[test]
    fn refetch_phase_budgets_are_independent_and_exhausted_locally() {
        let identity = RemoteRootReloadPhaseIdentityV1::new_v1(
            temporal_partition(session_id()).0.key_v1(),
            RemoteRootReloadDemandClassV1::PlaybackResumeRequired,
        );
        let mut owner = RemoteRootReloadPhaseOwnerV1::bind_v1(
            identity,
            RemoteRootReloadPolicyInputsV1 {
                cumulative_range_requests: 2,
                active_visible_deadline_millis: 3,
            },
            1,
            2,
            3,
        )
        .unwrap();

        owner
            .record_failure_v1(RemoteRootReloadFailureV1::Attempt)
            .unwrap();
        assert_eq!(owner.attempts_remaining_v1(), 0);
        assert_eq!(
            owner.record_failure_v1(RemoteRootReloadFailureV1::Attempt),
            Err(RemoteRootReloadErrorV1::PhaseFailure(
                RemoteRootReloadPhaseFailureV1 {
                    class: RemoteRootReloadDemandClassV1::PlaybackResumeRequired,
                    kind: RemoteRootReloadFailureV1::Attempt,
                },
            ))
        );

        owner
            .record_failure_v1(RemoteRootReloadFailureV1::RangeRequest)
            .unwrap();
        assert_eq!(owner.range_requests_remaining_v1(), 1);
        owner
            .record_failure_v1(RemoteRootReloadFailureV1::RangeRequest)
            .unwrap();
        assert_eq!(owner.range_requests_remaining_v1(), 0);

        owner
            .record_failure_v1(RemoteRootReloadFailureV1::ActiveVisibleDeadline)
            .unwrap();
        assert_eq!(owner.active_visible_deadline_remaining_v1(), 2);
    }

    #[test]
    fn reload_rejects_re_registration_of_different_root_set() {
        let session = session_id();
        let (mut store, capability, mut index, root_id, _chunk, insertion) =
            registered_partition_with_chunk(session);
        let (deletions, _) = store.gc(&GarbageCollectionOptions::gc_everything());
        index
            .observe_store_events_v1(&store, &capability, &deletions)
            .unwrap();

        let other_id = re_chunk::ChunkId::new();
        let (partition, _issuer) = temporal_partition(session);
        let other_root =
            crate::remote_manifest::ManifestRootDescriptorV1::for_registration_mismatch_test_v1(
                partition.key_v1(),
                0,
                other_id,
            );
        let bad_insertion = RemotePartitionInsertionV1::seal_v1(
            partition,
            vec![
                RemotePartitionRootInsertionV1::seal_v1(
                    partition,
                    tied_chunk(other_id),
                    other_root,
                )
                .unwrap(),
            ]
            .into_boxed_slice(),
        )
        .unwrap();

        let mut controller = RemoteRootReloadControllerV1::new_v1(
            index,
            capability,
            RemoteRootReloadLimitsV1::generous_disarmed_v1(),
        );
        let identity = RemoteRootReloadPhaseIdentityV1::new_v1(
            insertion.partition_v1().key_v1(),
            RemoteRootReloadDemandClassV1::PresentationRequired,
        );
        let mut owner = RemoteRootReloadPhaseOwnerV1::bind_v1(
            identity,
            RemoteRootReloadPolicyInputsV1 {
                cumulative_range_requests: 1,
                active_visible_deadline_millis: 1,
            },
            1,
            1,
            1,
        )
        .unwrap();

        assert_eq!(
            controller.reload_partition_v1(&mut store, &bad_insertion, &mut owner),
            Err(RemoteRootReloadErrorV1::InvalidReload)
        );
        assert_eq!(
            controller.root_index_v1().root_load_state_v1(root_id),
            Some(RefetchableRootLoadStateV1::Unloaded)
        );
    }

    #[test]
    fn reload_rejects_same_root_id_with_different_manifest_descriptor() {
        let session = session_id();
        let (mut store, capability, mut index, partition, root_ids, _chunks, _roots, insertion) =
            registered_partition_with_two_roots(session);
        let (deletions, _) = store.gc(&GarbageCollectionOptions::gc_everything());
        index
            .observe_store_events_v1(&store, &capability, &deletions)
            .unwrap();

        let mismatched_descriptor = ManifestRootDescriptorV1::for_registration_mismatch_test_v1(
            partition.key_v1(),
            1,
            root_ids[0],
        );
        let bad_insertion = RemotePartitionInsertionV1::seal_v1(
            partition,
            vec![
                RemotePartitionRootInsertionV1::seal_v1(
                    partition,
                    tied_chunk(root_ids[0]),
                    mismatched_descriptor,
                )
                .unwrap(),
            ]
            .into_boxed_slice(),
        )
        .unwrap();

        let mut controller = RemoteRootReloadControllerV1::new_v1(
            index,
            capability,
            RemoteRootReloadLimitsV1::generous_disarmed_v1(),
        );
        let identity = RemoteRootReloadPhaseIdentityV1::new_v1(
            insertion.partition_v1().key_v1(),
            RemoteRootReloadDemandClassV1::PresentationRequired,
        );
        let mut owner = RemoteRootReloadPhaseOwnerV1::bind_v1(
            identity,
            RemoteRootReloadPolicyInputsV1 {
                cumulative_range_requests: 1,
                active_visible_deadline_millis: 1,
            },
            1,
            1,
            1,
        )
        .unwrap();

        assert_eq!(
            controller.reload_partition_v1(&mut store, &bad_insertion, &mut owner),
            Err(RemoteRootReloadErrorV1::InvalidReload)
        );
        assert!(store.physical_chunk(&root_ids[0]).is_none());
        assert_eq!(
            controller.root_index_v1().root_load_state_v1(root_ids[0]),
            Some(RefetchableRootLoadStateV1::Unloaded)
        );
    }
}
