//! Generation/job-driven remote MCAP partition staging, registration, and insertion.
//!
//! This module stays production-disarmed and owns no HTTP client, retry transport, or public
//! Store facade. It consumes complete partition outcomes and the matching window-demand phase
//! owners, then applies the existing atomic registration and bounded Store insertion seam.

#![allow(dead_code)]

use std::collections::BTreeSet;
use std::sync::Arc;

use ahash::{HashMap, HashSet};
use re_byte_size::SizeBytes as _;
use re_chunk::{Chunk, ChunkId};
use re_chunk_store::{ChunkStore, WebRemoteMcapRootCapabilityV1};

use crate::remote_chunk_dispatch::RemoteTypedPartitionHandoffV1;
use crate::remote_manifest::{
    DerivationPartitionKeyV1, DerivationPartitionKindV1, ManifestPartitionDescriptorV1,
    ManifestRootDescriptorV1,
};
use crate::remote_partition_residency::{
    PreparedPartitionRegistrationV1, RefetchableRootIndexV1, RootRegistrationErrorV1,
};
use crate::remote_window_demand::{
    RemoteWindowDemandClassV1, RemoteWindowDemandPhaseIdentityV1, RemoteWindowDemandV1,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct DerivationJobKeyV1 {
    partition: DerivationPartitionKeyV1,
}

impl DerivationJobKeyV1 {
    pub(crate) const fn new_v1(partition: DerivationPartitionKeyV1) -> Self {
        Self { partition }
    }

    pub(crate) const fn partition_v1(self) -> DerivationPartitionKeyV1 {
        self.partition
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemotePartitionJobFailureV1 {
    Attempt,
    RangeRequest,
    ActiveVisibleDeadline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemotePartitionJobPhaseFailureV1 {
    class: RemoteWindowDemandClassV1,
    kind: RemotePartitionJobFailureV1,
}

impl RemotePartitionJobPhaseFailureV1 {
    pub(crate) const fn class_v1(self) -> RemoteWindowDemandClassV1 {
        self.class
    }

    pub(crate) const fn kind_v1(self) -> RemotePartitionJobFailureV1 {
        self.kind
    }
}

/// Cumulative phase-local budgets for one window-demand phase.
///
/// These counters are deliberately separate from metadata-opening owners. They are handed to the
/// production-disarmed job coordinator instead of borrowing source-scoped metadata counters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteWindowDemandPhaseOwnerV1 {
    class: RemoteWindowDemandClassV1,
    identity: RemoteWindowDemandPhaseIdentityV1,
    attempts_remaining: u64,
    range_requests_remaining: u64,
    active_visible_deadline_remaining: u64,
}

impl RemoteWindowDemandPhaseOwnerV1 {
    pub(crate) fn bind_v1(
        demand: &RemoteWindowDemandV1,
        attempts_remaining: u64,
        range_requests_remaining: u64,
        active_visible_deadline_remaining: u64,
    ) -> Result<Self, RemotePartitionJobErrorV1> {
        if !demand.can_start_retry_v1() {
            return Err(RemotePartitionJobErrorV1::PhaseOwnerMismatch);
        }
        let phase = demand.phase_v1();
        let identity = phase.identity_v1();
        let policy_inputs = phase
            .policy_inputs_v1()
            .ok_or(RemotePartitionJobErrorV1::PhaseOwnerMismatch)?;
        if range_requests_remaining > policy_inputs.cumulative_range_requests
            || active_visible_deadline_remaining > policy_inputs.active_visible_deadline_millis
        {
            return Err(RemotePartitionJobErrorV1::PhaseOwnerMismatch);
        }
        Ok(Self {
            class: demand.class_v1(),
            identity,
            attempts_remaining,
            range_requests_remaining,
            active_visible_deadline_remaining,
        })
    }

    pub(crate) const fn class_v1(self) -> RemoteWindowDemandClassV1 {
        self.class
    }

    pub(crate) const fn identity_v1(self) -> RemoteWindowDemandPhaseIdentityV1 {
        self.identity
    }

    fn validate_job_v1(
        &self,
        phase_identity: RemoteWindowDemandPhaseIdentityV1,
    ) -> Result<(), RemotePartitionJobErrorV1> {
        if self.identity != phase_identity || self.class != phase_identity.class_v1() {
            return Err(RemotePartitionJobErrorV1::PhaseOwnerMismatch);
        }
        Ok(())
    }

    fn record_failure_v1(
        &mut self,
        failure: RemotePartitionJobFailureV1,
    ) -> Result<(), RemotePartitionJobErrorV1> {
        let exhausted = |kind| RemotePartitionJobPhaseFailureV1 {
            class: self.class,
            kind,
        };
        match failure {
            RemotePartitionJobFailureV1::Attempt => {
                if self.attempts_remaining == 0 {
                    return Err(RemotePartitionJobErrorV1::PhaseFailure(exhausted(
                        RemotePartitionJobFailureV1::Attempt,
                    )));
                }
                self.attempts_remaining -= 1;
            }
            RemotePartitionJobFailureV1::RangeRequest => {
                if self.range_requests_remaining == 0 {
                    return Err(RemotePartitionJobErrorV1::PhaseFailure(exhausted(
                        RemotePartitionJobFailureV1::RangeRequest,
                    )));
                }
                self.range_requests_remaining -= 1;
            }
            RemotePartitionJobFailureV1::ActiveVisibleDeadline => {
                if self.active_visible_deadline_remaining == 0 {
                    return Err(RemotePartitionJobErrorV1::PhaseFailure(exhausted(
                        RemotePartitionJobFailureV1::ActiveVisibleDeadline,
                    )));
                }
                self.active_visible_deadline_remaining -= 1;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RemotePartitionRootInsertionV1 {
    chunk: Arc<Chunk>,
    manifest_descriptor: ManifestRootDescriptorV1,
}

impl RemotePartitionRootInsertionV1 {
    pub(crate) fn seal_v1(
        partition: ManifestPartitionDescriptorV1,
        chunk: Arc<Chunk>,
        manifest_descriptor: ManifestRootDescriptorV1,
    ) -> Result<Self, RemotePartitionJobErrorV1> {
        if manifest_descriptor.partition_key_v1() != partition.key_v1()
            || manifest_descriptor.root_chunk_id_v1() != chunk.id()
        {
            return Err(RemotePartitionJobErrorV1::InvalidBatch);
        }
        let kind_matches = match partition.key_v1().kind_v1() {
            DerivationPartitionKindV1::OpeningStatic => chunk.is_static(),
            DerivationPartitionKindV1::TemporalChannelGroup(_) => !chunk.is_static(),
        };
        if !kind_matches {
            return Err(RemotePartitionJobErrorV1::InvalidBatch);
        }
        Ok(Self {
            chunk,
            manifest_descriptor,
        })
    }

    pub(crate) const fn chunk_v1(&self) -> &Arc<Chunk> {
        &self.chunk
    }

    pub(crate) const fn root_v1(&self) -> ManifestRootDescriptorV1 {
        self.manifest_descriptor
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RemotePartitionInsertionV1 {
    partition: ManifestPartitionDescriptorV1,
    roots: Box<[RemotePartitionRootInsertionV1]>,
}

impl RemotePartitionInsertionV1 {
    pub(crate) fn seal_v1(
        partition: ManifestPartitionDescriptorV1,
        roots: Box<[RemotePartitionRootInsertionV1]>,
    ) -> Result<Self, RemotePartitionJobErrorV1> {
        if roots.is_empty() {
            return Err(RemotePartitionJobErrorV1::InvalidBatch);
        }
        let max_roots = usize::try_from(partition.registration_bound_v1())
            .map_err(|_error| RemotePartitionJobErrorV1::ArithmeticOverflow)?;
        if roots.len() > max_roots {
            return Err(RemotePartitionJobErrorV1::ResourceLimitExceeded);
        }
        let mut unique_ids = BTreeSet::new();
        let mut unique_ordinals = BTreeSet::new();
        for root in &roots {
            if root.root_v1().partition_key_v1() != partition.key_v1()
                || root.root_v1().root_chunk_id_v1() != root.chunk_v1().id()
            {
                return Err(RemotePartitionJobErrorV1::InvalidBatch);
            }
            let kind_matches = match partition.key_v1().kind_v1() {
                DerivationPartitionKindV1::OpeningStatic => root.chunk_v1().is_static(),
                DerivationPartitionKindV1::TemporalChannelGroup(_) => !root.chunk_v1().is_static(),
            };
            if !kind_matches
                || root.root_v1().output_ordinal_v1() >= partition.registration_bound_v1()
            {
                return Err(RemotePartitionJobErrorV1::InvalidBatch);
            }
            unique_ids.insert(root.root_v1().root_chunk_id_v1());
            unique_ordinals.insert(root.root_v1().output_ordinal_v1());
        }
        if unique_ids.len() != roots.len() || unique_ordinals.len() != roots.len() {
            return Err(RemotePartitionJobErrorV1::InvalidBatch);
        }
        Ok(Self { partition, roots })
    }

    pub(crate) fn from_handoff_v1(
        handoff: &RemoteTypedPartitionHandoffV1,
    ) -> Result<Self, RemotePartitionJobErrorV1> {
        let roots = handoff
            .root_handoffs_v1()
            .map(|root| {
                RemotePartitionRootInsertionV1::seal_v1(
                    handoff.partition_v1(),
                    Arc::new(root.chunk_v1().clone()),
                    root.root_v1(),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        Self::seal_v1(handoff.partition_v1(), roots.into_boxed_slice())
    }

    pub(crate) const fn partition_v1(&self) -> ManifestPartitionDescriptorV1 {
        self.partition
    }

    pub(crate) fn roots_v1(&self) -> &[RemotePartitionRootInsertionV1] {
        &self.roots
    }

    pub(crate) fn root_count_v1(&self) -> Result<u64, RemotePartitionJobErrorV1> {
        u64::try_from(self.roots.len())
            .map_err(|_error| RemotePartitionJobErrorV1::ArithmeticOverflow)
    }

    pub(crate) fn total_size_bytes_v1(&self) -> Result<u64, RemotePartitionJobErrorV1> {
        self.roots.iter().try_fold(0_u64, |total, root| {
            total
                .checked_add(root.chunk_v1().total_size_bytes())
                .ok_or(RemotePartitionJobErrorV1::ArithmeticOverflow)
        })
    }

    pub(crate) fn root_chunk_ids_v1(&self) -> BTreeSet<ChunkId> {
        self.roots
            .iter()
            .map(|root| root.root_v1().root_chunk_id_v1())
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RemotePartitionBatchV1 {
    demand_generation: u64,
    work_epoch: u64,
    job_key: DerivationJobKeyV1,
    phase_identity: RemoteWindowDemandPhaseIdentityV1,
    registration: PreparedPartitionRegistrationV1,
    insertion: Option<RemotePartitionInsertionV1>,
}

impl RemotePartitionBatchV1 {
    pub(crate) fn complete_v1(
        demand_generation: u64,
        work_epoch: u64,
        job_key: DerivationJobKeyV1,
        phase_identity: RemoteWindowDemandPhaseIdentityV1,
        registration: PreparedPartitionRegistrationV1,
        insertion: Option<RemotePartitionInsertionV1>,
    ) -> Result<Self, RemotePartitionJobErrorV1> {
        if job_key.partition_v1() != registration.partition_v1().key_v1() {
            return Err(RemotePartitionJobErrorV1::InvalidBatch);
        }
        let expected = registration.expected_root_chunk_ids_v1();
        match (&insertion, &expected) {
            (Some(insertion), Some(expected)) => {
                if insertion.partition_v1() != registration.partition_v1()
                    || insertion.root_chunk_ids_v1().iter().ne(expected.iter())
                {
                    return Err(RemotePartitionJobErrorV1::InvalidBatch);
                }
            }
            (None, None) => {}
            _ => return Err(RemotePartitionJobErrorV1::InvalidBatch),
        }
        Ok(Self {
            demand_generation,
            work_epoch,
            job_key,
            phase_identity,
            registration,
            insertion,
        })
    }

    pub(crate) const fn demand_generation_v1(&self) -> u64 {
        self.demand_generation
    }

    pub(crate) const fn work_epoch_v1(&self) -> u64 {
        self.work_epoch
    }

    pub(crate) const fn job_key_v1(&self) -> DerivationJobKeyV1 {
        self.job_key
    }

    pub(crate) const fn phase_identity_v1(&self) -> RemoteWindowDemandPhaseIdentityV1 {
        self.phase_identity
    }

    pub(crate) const fn registration_v1(&self) -> &PreparedPartitionRegistrationV1 {
        &self.registration
    }

    pub(crate) const fn insertion_v1(&self) -> Option<&RemotePartitionInsertionV1> {
        self.insertion.as_ref()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemotePartitionStagingLimitsV1 {
    pub(crate) max_terminal_batches: u64,
    pub(crate) max_derived_roots: u64,
    pub(crate) max_staged_bytes: u64,
    pub(crate) max_insertion_calls_per_commit: u64,
    pub(crate) max_session_resident_physical_roots: u64,
}

impl RemotePartitionStagingLimitsV1 {
    pub(crate) const fn generous_disarmed_v1() -> Self {
        Self {
            max_terminal_batches: 1_000_000,
            max_derived_roots: 1_000_000,
            max_staged_bytes: u64::MAX,
            max_insertion_calls_per_commit: 1_000_000,
            max_session_resident_physical_roots: 1_000_000,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemotePartitionInsertionOutcomeV1 {
    CompleteEmpty,
    RootsFullyResident,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemotePartitionInsertionAckV1 {
    demand_generation: u64,
    work_epoch: u64,
    job_key: DerivationJobKeyV1,
    root_chunk_ids: BTreeSet<ChunkId>,
    outcome: RemotePartitionInsertionOutcomeV1,
}

impl RemotePartitionInsertionAckV1 {
    pub(crate) const fn demand_generation_v1(&self) -> u64 {
        self.demand_generation
    }

    pub(crate) const fn work_epoch_v1(&self) -> u64 {
        self.work_epoch
    }

    pub(crate) const fn job_key_v1(&self) -> DerivationJobKeyV1 {
        self.job_key
    }

    pub(crate) fn root_chunk_ids_v1(&self) -> &BTreeSet<ChunkId> {
        &self.root_chunk_ids
    }

    pub(crate) const fn outcome_v1(&self) -> RemotePartitionInsertionOutcomeV1 {
        self.outcome
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum RemotePartitionJobErrorV1 {
    #[error("remote MCAP partition job arithmetic overflowed")]
    ArithmeticOverflow,

    #[error("remote MCAP partition job exceeds a resource limit")]
    ResourceLimitExceeded,

    #[error("remote MCAP partition job contains an invalid complete batch")]
    InvalidBatch,

    #[error("remote MCAP partition job phase owner does not match")]
    PhaseOwnerMismatch,

    #[error("remote MCAP partition job exhausted its matching phase budget")]
    PhaseFailure(RemotePartitionJobPhaseFailureV1),

    #[error("remote MCAP partition work is stale")]
    StaleWork,

    #[error("remote MCAP partition derivation is duplicated")]
    DuplicateDerivation,

    #[error("remote MCAP partition is already satisfied")]
    AlreadySatisfied,

    #[error(transparent)]
    Registration(RootRegistrationErrorV1),

    #[error("the Web remote-MCAP Store capability is unavailable")]
    StoreCapability,

    #[error("remote MCAP Store insertion became partially mutated and is terminal gated")]
    InsertionGated,
}

impl From<RootRegistrationErrorV1> for RemotePartitionJobErrorV1 {
    fn from(error: RootRegistrationErrorV1) -> Self {
        Self::Registration(error)
    }
}

pub(crate) struct RemotePartitionDerivationCoordinatorV1 {
    current_generation: u64,
    current_work_epoch: u64,
    limits: RemotePartitionStagingLimitsV1,
    phase_owners: HashMap<RemoteWindowDemandClassV1, RemoteWindowDemandPhaseOwnerV1>,
    staged: HashMap<DerivationJobKeyV1, RemotePartitionBatchV1>,
    staged_roots: u64,
    staged_bytes: u64,
    known_partition_jobs: HashSet<DerivationJobKeyV1>,
}

impl RemotePartitionDerivationCoordinatorV1 {
    pub(crate) fn new_v1(
        generation: u64,
        work_epoch: u64,
        limits: RemotePartitionStagingLimitsV1,
    ) -> Self {
        Self {
            current_generation: generation,
            current_work_epoch: work_epoch,
            limits,
            phase_owners: HashMap::default(),
            staged: HashMap::default(),
            staged_roots: 0,
            staged_bytes: 0,
            known_partition_jobs: HashSet::default(),
        }
    }

    pub(crate) const fn generation_v1(&self) -> u64 {
        self.current_generation
    }

    pub(crate) const fn work_epoch_v1(&self) -> u64 {
        self.current_work_epoch
    }

    #[cfg(test)]
    pub(crate) fn staged_len_v1(&self) -> usize {
        self.staged.len()
    }

    pub(crate) fn install_phase_owner_v1(
        &mut self,
        owner: RemoteWindowDemandPhaseOwnerV1,
    ) -> Result<(), RemotePartitionJobErrorV1> {
        if let Some(existing) = self.phase_owners.get(&owner.class_v1())
            && existing.identity_v1() != owner.identity_v1()
        {
            return Err(RemotePartitionJobErrorV1::PhaseOwnerMismatch);
        }
        self.phase_owners.insert(owner.class_v1(), owner);
        Ok(())
    }

    pub(crate) fn phase_owner_v1(
        &self,
        class: RemoteWindowDemandClassV1,
    ) -> Option<&RemoteWindowDemandPhaseOwnerV1> {
        self.phase_owners.get(&class)
    }

    pub(crate) fn rebind_generation_v1(&mut self, generation: u64, work_epoch: u64) {
        self.current_generation = generation;
        self.current_work_epoch = work_epoch;
        self.staged.clear();
        self.staged_roots = 0;
        self.staged_bytes = 0;
        self.known_partition_jobs.clear();
    }

    fn validate_active_job_v1(
        &self,
        demand_generation: u64,
        work_epoch: u64,
        phase_identity: RemoteWindowDemandPhaseIdentityV1,
    ) -> Result<(), RemotePartitionJobErrorV1> {
        if demand_generation != self.current_generation || work_epoch != self.current_work_epoch {
            return Err(RemotePartitionJobErrorV1::StaleWork);
        }
        self.phase_owners
            .get(&phase_identity.class_v1())
            .ok_or(RemotePartitionJobErrorV1::PhaseOwnerMismatch)?
            .validate_job_v1(phase_identity)
    }

    pub(crate) fn record_phase_failure_v1(
        &mut self,
        demand_generation: u64,
        work_epoch: u64,
        phase_identity: RemoteWindowDemandPhaseIdentityV1,
        failure: RemotePartitionJobFailureV1,
    ) -> Result<(), RemotePartitionJobErrorV1> {
        self.validate_active_job_v1(demand_generation, work_epoch, phase_identity)?;
        self.phase_owners
            .get_mut(&phase_identity.class_v1())
            .expect("validated partition job retains its matching phase owner")
            .record_failure_v1(failure)
    }

    pub(crate) fn stage_terminal_v1(
        &mut self,
        batch: RemotePartitionBatchV1,
        store: &ChunkStore,
        capability: &WebRemoteMcapRootCapabilityV1,
        root_index: &RefetchableRootIndexV1,
    ) -> Result<(), RemotePartitionJobErrorV1> {
        self.validate_active_job_v1(
            batch.demand_generation_v1(),
            batch.work_epoch_v1(),
            batch.phase_identity_v1(),
        )?;
        let job_key = batch.job_key_v1();
        if root_index
            .is_partition_fully_resident_v1(store, capability, job_key.partition_v1())
            .map_err(RemotePartitionJobErrorV1::Registration)?
        {
            return Err(RemotePartitionJobErrorV1::AlreadySatisfied);
        }
        if self.known_partition_jobs.contains(&job_key) {
            return Err(RemotePartitionJobErrorV1::DuplicateDerivation);
        }

        let next_batches = u64::try_from(self.staged.len())
            .map_err(|_error| RemotePartitionJobErrorV1::ArithmeticOverflow)?
            .checked_add(1)
            .ok_or(RemotePartitionJobErrorV1::ArithmeticOverflow)?;
        if next_batches > self.limits.max_terminal_batches {
            return Err(RemotePartitionJobErrorV1::ResourceLimitExceeded);
        }
        let next_roots = batch
            .insertion_v1()
            .map_or(Ok(0), |insertion| insertion.root_count_v1())?;
        let next_roots = self
            .staged_roots
            .checked_add(next_roots)
            .ok_or(RemotePartitionJobErrorV1::ArithmeticOverflow)?;
        if next_roots > self.limits.max_derived_roots {
            return Err(RemotePartitionJobErrorV1::ResourceLimitExceeded);
        }
        let next_bytes = batch
            .insertion_v1()
            .map_or(Ok(0), |insertion| insertion.total_size_bytes_v1())?;
        let next_bytes = self
            .staged_bytes
            .checked_add(next_bytes)
            .ok_or(RemotePartitionJobErrorV1::ArithmeticOverflow)?;
        if next_bytes > self.limits.max_staged_bytes {
            return Err(RemotePartitionJobErrorV1::ResourceLimitExceeded);
        }

        self.staged_roots = next_roots;
        self.staged_bytes = next_bytes;
        self.known_partition_jobs.insert(job_key);
        self.staged.insert(job_key, batch);
        Ok(())
    }

    pub(crate) fn commit_staged_v1(
        &self,
        store: &mut ChunkStore,
        capability: &WebRemoteMcapRootCapabilityV1,
        root_index: &mut RefetchableRootIndexV1,
    ) -> Result<Vec<RemotePartitionInsertionAckV1>, RemotePartitionJobErrorV1> {
        if self.staged.is_empty() {
            return Ok(Vec::new());
        }
        for batch in self.staged.values() {
            self.validate_active_job_v1(
                batch.demand_generation_v1(),
                batch.work_epoch_v1(),
                batch.phase_identity_v1(),
            )?;
        }

        let new_root_count = self.staged.values().try_fold(0_u64, |total, batch| {
            let count = batch
                .insertion_v1()
                .map_or(Ok(0), |insertion| insertion.root_count_v1())?;
            total
                .checked_add(count)
                .ok_or(RemotePartitionJobErrorV1::ArithmeticOverflow)
        })?;
        let current_resident_roots = u64::try_from(store.num_physical_chunks())
            .map_err(|_error| RemotePartitionJobErrorV1::ArithmeticOverflow)?;
        let next_resident_roots = current_resident_roots
            .checked_add(new_root_count)
            .ok_or(RemotePartitionJobErrorV1::ArithmeticOverflow)?;
        if next_resident_roots > self.limits.max_session_resident_physical_roots {
            return Err(RemotePartitionJobErrorV1::ResourceLimitExceeded);
        }
        if new_root_count > self.limits.max_insertion_calls_per_commit {
            return Err(RemotePartitionJobErrorV1::ResourceLimitExceeded);
        }

        let registrations = self
            .staged
            .values()
            .map(RemotePartitionBatchV1::registration_v1)
            .cloned()
            .collect::<Vec<_>>();
        root_index
            .register_commit_set_v1(store, capability, registrations)
            .map_err(RemotePartitionJobErrorV1::Registration)?;

        let mut acks = Vec::new();
        let mut pending_events = Vec::new();
        for batch in self.staged.values() {
            let Some(insertion) = batch.insertion_v1() else {
                let residency =
                    root_index.partition_residency_v1(batch.job_key_v1().partition_v1());
                if residency
                    != crate::remote_partition_residency::PartitionResidencyV1::CompleteEmpty
                {
                    return Err(RemotePartitionJobErrorV1::InsertionGated);
                }
                acks.push(RemotePartitionInsertionAckV1 {
                    demand_generation: batch.demand_generation_v1(),
                    work_epoch: batch.work_epoch_v1(),
                    job_key: batch.job_key_v1(),
                    root_chunk_ids: BTreeSet::new(),
                    outcome: RemotePartitionInsertionOutcomeV1::CompleteEmpty,
                });
                continue;
            };

            for root in insertion.roots_v1() {
                let root_id = root.root_v1().root_chunk_id_v1();
                let descriptor = root_index.root_refetch_descriptor_v1(root_id).ok_or(
                    RemotePartitionJobErrorV1::Registration(
                        RootRegistrationErrorV1::RegistrationConflict,
                    ),
                )?;
                let permit = capability
                    .issue_refetch_v1(store, descriptor)
                    .map_err(|_error| RemotePartitionJobErrorV1::StoreCapability)?;
                let events =
                    match store.insert_external_refetchable_root_v1(permit, root.chunk_v1()) {
                        Ok(events) => events,
                        Err(_error) => {
                            if !pending_events.is_empty() {
                                let _reconciled = root_index.observe_store_events_v1(
                                    store,
                                    capability,
                                    &pending_events,
                                );
                            }
                            return Err(RemotePartitionJobErrorV1::InsertionGated);
                        }
                    };
                pending_events.extend(events);
            }

            root_index
                .observe_store_events_v1(store, capability, &pending_events)
                .map_err(RemotePartitionJobErrorV1::Registration)?;
            pending_events.clear();

            if !root_index
                .is_partition_fully_resident_v1(
                    store,
                    capability,
                    batch.job_key_v1().partition_v1(),
                )
                .map_err(RemotePartitionJobErrorV1::Registration)?
            {
                return Err(RemotePartitionJobErrorV1::InsertionGated);
            }

            acks.push(RemotePartitionInsertionAckV1 {
                demand_generation: batch.demand_generation_v1(),
                work_epoch: batch.work_epoch_v1(),
                job_key: batch.job_key_v1(),
                root_chunk_ids: insertion.root_chunk_ids_v1(),
                outcome: RemotePartitionInsertionOutcomeV1::RootsFullyResident,
            });
        }
        Ok(acks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_channel_group::StableDecoderGroupIdV1;
    use crate::remote_loaded_coverage::RemoteTemporalCoveragePlanV1;
    use crate::remote_manifest::{
        ManifestPartitionDescriptorV1, RemoteMcapSessionIdV1, RemoteRegistrationCapacityV1,
    };
    use re_chunk::RowId;
    use re_chunk_store::{GarbageCollectionOptions, WebRemoteMcapStoreConfigV1};
    use re_log_types::{StoreId, StoreKind, Timeline};
    use re_sdk_types::archetypes;

    fn session_id() -> RemoteMcapSessionIdV1 {
        RemoteMcapSessionIdV1::for_registration_test_v1(91)
    }

    fn temporal_partition(
        session: RemoteMcapSessionIdV1,
        source_ordinal: u32,
        bound: u32,
    ) -> (
        ManifestPartitionDescriptorV1,
        crate::remote_manifest::ManifestRootDescriptorIssuerV1,
    ) {
        ManifestPartitionDescriptorV1::for_registration_test_v1(
            session,
            source_ordinal,
            DerivationPartitionKindV1::TemporalChannelGroup(
                StableDecoderGroupIdV1::first_for_assignment_test_v1(),
            ),
            bound,
        )
    }

    fn temporal_chunk(root_chunk_id: ChunkId) -> Chunk {
        Chunk::builder_with_id(root_chunk_id, "world/points")
            .with_archetype(
                RowId::new(),
                [(Timeline::log_tick(), 1)],
                &archetypes::Points3D::new([[1.0, 2.0, 3.0]]),
            )
            .build()
            .unwrap()
    }

    fn store_and_capability() -> (ChunkStore, WebRemoteMcapRootCapabilityV1) {
        WebRemoteMcapStoreConfigV1::for_test_v1()
            .into_store_v1(StoreId::random(StoreKind::Recording, "partition-job-test"))
    }

    fn new_index(
        session: RemoteMcapSessionIdV1,
        limits: RemoteRegistrationCapacityV1,
        store: &ChunkStore,
        capability: &WebRemoteMcapRootCapabilityV1,
    ) -> RefetchableRootIndexV1 {
        RefetchableRootIndexV1::new_v1(
            session,
            limits,
            store,
            capability,
            RemoteTemporalCoveragePlanV1::for_test_v1(&[None; 8], 1),
        )
        .unwrap()
    }

    fn generous_limits() -> RemoteRegistrationCapacityV1 {
        RemoteRegistrationCapacityV1 {
            max_registered_partitions: 8,
            max_complete_empty_entries: 8,
            max_root_descriptors: 8,
            max_external_origin_bytes: 8
                * re_chunk_store::ExternalRefetchableRootOriginV1::ENCODED_BYTES_V1,
        }
    }

    fn phase_identity(class: RemoteWindowDemandClassV1) -> RemoteWindowDemandPhaseIdentityV1 {
        crate::remote_window_demand::phase_identity_v1(
            session_id(),
            1,
            class,
            re_log_types::TimeInt::new_temporal(1),
            re_log_types::TimeInt::new_temporal(2),
            re_log_types::TimeInt::new_temporal(3),
            true,
        )
    }

    fn installed_phase_owner(
        class: RemoteWindowDemandClassV1,
        attempts: u64,
        ranges: u64,
        deadline: u64,
    ) -> RemoteWindowDemandPhaseOwnerV1 {
        let identity = phase_identity(class);
        let mut demand = RemoteWindowDemandV1::new_uninstalled(class, identity);
        demand
            .install_matching_phase_owner_v1(
                identity,
                crate::remote_window_demand::RemoteWindowDemandPolicyInputsV1 {
                    cumulative_range_requests: ranges,
                    active_visible_deadline_millis: deadline,
                },
            )
            .unwrap();
        RemoteWindowDemandPhaseOwnerV1::bind_v1(&demand, attempts, ranges, deadline).unwrap()
    }

    fn empty_registration(
        session: RemoteMcapSessionIdV1,
        ordinal: u32,
    ) -> PreparedPartitionRegistrationV1 {
        let (partition, _) = temporal_partition(session, ordinal, 1);
        PreparedPartitionRegistrationV1::complete_empty_for_test_v1(partition)
    }

    fn roots_batch(
        generation: u64,
        work_epoch: u64,
        session: RemoteMcapSessionIdV1,
        ordinal: u32,
        class: RemoteWindowDemandClassV1,
    ) -> RemotePartitionBatchV1 {
        let (partition, issuer) = temporal_partition(session, ordinal, 1);
        let root = issuer.descriptor_v1(0).unwrap();
        let chunk = temporal_chunk(root.root_chunk_id_v1());
        let insertion = RemotePartitionInsertionV1::seal_v1(
            partition,
            vec![
                RemotePartitionRootInsertionV1::seal_v1(partition, Arc::new(chunk), root).unwrap(),
            ]
            .into_boxed_slice(),
        )
        .unwrap();
        let registration = PreparedPartitionRegistrationV1::complete_roots_for_test_v1(
            partition,
            &[root],
            insertion.total_size_bytes_v1().unwrap(),
            false,
        )
        .unwrap();
        RemotePartitionBatchV1::complete_v1(
            generation,
            work_epoch,
            DerivationJobKeyV1::new_v1(partition.key_v1()),
            phase_identity(class),
            registration,
            Some(insertion),
        )
        .unwrap()
    }

    #[test]
    fn complete_empty_is_persistent_and_not_reinserted() {
        let session = session_id();
        let (mut store, capability) = store_and_capability();
        let mut index = new_index(session, generous_limits(), &store, &capability);
        let partition_key = empty_registration(session, 0).partition_v1().key_v1();
        let batch = RemotePartitionBatchV1::complete_v1(
            1,
            1,
            DerivationJobKeyV1::new_v1(partition_key),
            phase_identity(RemoteWindowDemandClassV1::PresentationRequired),
            empty_registration(session, 0),
            None,
        )
        .unwrap();
        let mut coordinator = RemotePartitionDerivationCoordinatorV1::new_v1(
            1,
            1,
            RemotePartitionStagingLimitsV1::generous_disarmed_v1(),
        );
        coordinator
            .install_phase_owner_v1(installed_phase_owner(
                RemoteWindowDemandClassV1::PresentationRequired,
                1,
                1,
                1,
            ))
            .unwrap();
        coordinator
            .stage_terminal_v1(batch, &store, &capability, &index)
            .unwrap();
        let acks = coordinator
            .commit_staged_v1(&mut store, &capability, &mut index)
            .unwrap();

        assert_eq!(acks.len(), 1);
        assert_eq!(
            acks[0].outcome_v1(),
            RemotePartitionInsertionOutcomeV1::CompleteEmpty
        );
        assert!(acks[0].root_chunk_ids_v1().is_empty());
        assert!(
            index
                .is_partition_fully_resident_v1(&store, &capability, partition_key)
                .unwrap()
        );
        assert_eq!(
            index.partition_residency_v1(partition_key),
            crate::remote_partition_residency::PartitionResidencyV1::CompleteEmpty
        );

        let second = RemotePartitionBatchV1::complete_v1(
            1,
            1,
            DerivationJobKeyV1::new_v1(partition_key),
            phase_identity(RemoteWindowDemandClassV1::PresentationRequired),
            empty_registration(session, 0),
            None,
        )
        .unwrap();
        assert_eq!(
            coordinator.stage_terminal_v1(second, &store, &capability, &index),
            Err(RemotePartitionJobErrorV1::AlreadySatisfied)
        );
    }

    #[test]
    fn phase_exhaustion_is_matching_and_does_not_touch_registration_counters() {
        let session = session_id();
        let (store, capability) = store_and_capability();
        let index = new_index(session, generous_limits(), &store, &capability);
        let before = index.counters_v1();
        let mut coordinator = RemotePartitionDerivationCoordinatorV1::new_v1(
            1,
            1,
            RemotePartitionStagingLimitsV1::generous_disarmed_v1(),
        );
        let identity = phase_identity(RemoteWindowDemandClassV1::PresentationRequired);
        coordinator
            .install_phase_owner_v1(installed_phase_owner(
                RemoteWindowDemandClassV1::PresentationRequired,
                1,
                2,
                3,
            ))
            .unwrap();

        coordinator
            .record_phase_failure_v1(1, 1, identity, RemotePartitionJobFailureV1::Attempt)
            .unwrap();
        assert_eq!(
            coordinator.record_phase_failure_v1(
                1,
                1,
                identity,
                RemotePartitionJobFailureV1::Attempt,
            ),
            Err(RemotePartitionJobErrorV1::PhaseFailure(
                RemotePartitionJobPhaseFailureV1 {
                    class: RemoteWindowDemandClassV1::PresentationRequired,
                    kind: RemotePartitionJobFailureV1::Attempt,
                },
            ))
        );

        let resume_identity = phase_identity(RemoteWindowDemandClassV1::PlaybackResumeRequired);
        let mut resume = RemotePartitionDerivationCoordinatorV1::new_v1(
            1,
            1,
            RemotePartitionStagingLimitsV1::generous_disarmed_v1(),
        );
        resume
            .install_phase_owner_v1(installed_phase_owner(
                RemoteWindowDemandClassV1::PlaybackResumeRequired,
                1,
                1,
                1,
            ))
            .unwrap();
        resume
            .record_phase_failure_v1(
                1,
                1,
                resume_identity,
                RemotePartitionJobFailureV1::RangeRequest,
            )
            .unwrap();
        assert_eq!(
            resume.record_phase_failure_v1(
                1,
                1,
                resume_identity,
                RemotePartitionJobFailureV1::RangeRequest,
            ),
            Err(RemotePartitionJobErrorV1::PhaseFailure(
                RemotePartitionJobPhaseFailureV1 {
                    class: RemoteWindowDemandClassV1::PlaybackResumeRequired,
                    kind: RemotePartitionJobFailureV1::RangeRequest,
                },
            ))
        );

        let mut deadline = RemotePartitionDerivationCoordinatorV1::new_v1(
            1,
            1,
            RemotePartitionStagingLimitsV1::generous_disarmed_v1(),
        );
        let deadline_identity = phase_identity(RemoteWindowDemandClassV1::PresentationRequired);
        deadline
            .install_phase_owner_v1(installed_phase_owner(
                RemoteWindowDemandClassV1::PresentationRequired,
                1,
                1,
                1,
            ))
            .unwrap();
        deadline
            .record_phase_failure_v1(
                1,
                1,
                deadline_identity,
                RemotePartitionJobFailureV1::ActiveVisibleDeadline,
            )
            .unwrap();
        assert_eq!(
            deadline.record_phase_failure_v1(
                1,
                1,
                deadline_identity,
                RemotePartitionJobFailureV1::ActiveVisibleDeadline,
            ),
            Err(RemotePartitionJobErrorV1::PhaseFailure(
                RemotePartitionJobPhaseFailureV1 {
                    class: RemoteWindowDemandClassV1::PresentationRequired,
                    kind: RemotePartitionJobFailureV1::ActiveVisibleDeadline,
                },
            ))
        );

        assert_eq!(index.counters_v1(), before);
        assert_eq!(store.num_physical_chunks(), 0);
    }

    #[test]
    fn duplicate_derivation_is_rejected_and_failed_capacity_is_atomic() {
        let session = session_id();
        let (mut store, capability) = store_and_capability();
        let index = new_index(session, generous_limits(), &store, &capability);
        let mut coordinator = RemotePartitionDerivationCoordinatorV1::new_v1(
            1,
            1,
            RemotePartitionStagingLimitsV1::generous_disarmed_v1(),
        );
        coordinator
            .install_phase_owner_v1(installed_phase_owner(
                RemoteWindowDemandClassV1::PresentationRequired,
                10,
                10,
                10,
            ))
            .unwrap();
        let first = roots_batch(
            1,
            1,
            session,
            0,
            RemoteWindowDemandClassV1::PresentationRequired,
        );
        coordinator
            .stage_terminal_v1(first.clone(), &store, &capability, &index)
            .unwrap();
        assert_eq!(
            coordinator.stage_terminal_v1(first, &store, &capability, &index),
            Err(RemotePartitionJobErrorV1::DuplicateDerivation)
        );
        assert_eq!(coordinator.staged_len_v1(), 1);

        let second = roots_batch(
            1,
            1,
            session,
            1,
            RemoteWindowDemandClassV1::PresentationRequired,
        );
        let mut capped_index = new_index(
            session,
            RemoteRegistrationCapacityV1 {
                max_registered_partitions: 2,
                max_complete_empty_entries: 0,
                max_root_descriptors: 1,
                max_external_origin_bytes:
                    re_chunk_store::ExternalRefetchableRootOriginV1::ENCODED_BYTES_V1,
            },
            &store,
            &capability,
        );
        let mut capped_coordinator = RemotePartitionDerivationCoordinatorV1::new_v1(
            1,
            1,
            RemotePartitionStagingLimitsV1::generous_disarmed_v1(),
        );
        capped_coordinator
            .install_phase_owner_v1(installed_phase_owner(
                RemoteWindowDemandClassV1::PresentationRequired,
                10,
                10,
                10,
            ))
            .unwrap();
        let first_for_cap = roots_batch(
            1,
            1,
            session,
            0,
            RemoteWindowDemandClassV1::PresentationRequired,
        );
        let second_for_cap = roots_batch(
            1,
            1,
            session,
            1,
            RemoteWindowDemandClassV1::PresentationRequired,
        );
        capped_coordinator
            .stage_terminal_v1(first_for_cap, &store, &capability, &capped_index)
            .unwrap();
        capped_coordinator
            .stage_terminal_v1(second_for_cap, &store, &capability, &capped_index)
            .unwrap();
        assert_eq!(
            capped_coordinator.commit_staged_v1(&mut store, &capability, &mut capped_index),
            Err(RemotePartitionJobErrorV1::Registration(
                RootRegistrationErrorV1::ResourceLimitExceeded
            ))
        );
        assert_eq!(capped_index.counters_v1().registered_partitions, 0);
        assert_eq!(capped_index.counters_v1().root_descriptors, 0);
        assert_eq!(store.num_physical_chunks(), 0);

        assert_eq!(
            second.job_key_v1().partition_v1().source_unit_ordinal_v1(),
            1
        );
    }

    #[test]
    fn resident_physical_root_cap_fails_before_registration_or_insertion() {
        let session = session_id();
        let (mut store, capability) = store_and_capability();
        let mut index = new_index(session, generous_limits(), &store, &capability);
        let before = index.counters_v1();
        let mut limits = RemotePartitionStagingLimitsV1::generous_disarmed_v1();
        limits.max_session_resident_physical_roots = 0;
        let mut coordinator = RemotePartitionDerivationCoordinatorV1::new_v1(1, 1, limits);
        coordinator
            .install_phase_owner_v1(installed_phase_owner(
                RemoteWindowDemandClassV1::PresentationRequired,
                1,
                1,
                1,
            ))
            .unwrap();
        let batch = roots_batch(
            1,
            1,
            session,
            0,
            RemoteWindowDemandClassV1::PresentationRequired,
        );
        let partition_key = batch.job_key_v1().partition_v1();
        coordinator
            .stage_terminal_v1(batch, &store, &capability, &index)
            .unwrap();

        assert_eq!(
            coordinator.commit_staged_v1(&mut store, &capability, &mut index),
            Err(RemotePartitionJobErrorV1::ResourceLimitExceeded)
        );
        assert_eq!(index.counters_v1(), before);
        assert_eq!(
            index.partition_residency_v1(partition_key),
            crate::remote_partition_residency::PartitionResidencyV1::Unknown
        );
        assert_eq!(store.num_physical_chunks(), 0);
    }

    #[test]
    fn stale_work_cannot_publish_but_store_events_still_update_residency() {
        let session = session_id();
        let (mut store, capability) = store_and_capability();
        let mut index = new_index(session, generous_limits(), &store, &capability);
        let mut coordinator = RemotePartitionDerivationCoordinatorV1::new_v1(
            1,
            1,
            RemotePartitionStagingLimitsV1::generous_disarmed_v1(),
        );
        coordinator
            .install_phase_owner_v1(installed_phase_owner(
                RemoteWindowDemandClassV1::PresentationRequired,
                10,
                10,
                10,
            ))
            .unwrap();
        let batch = roots_batch(
            1,
            1,
            session,
            0,
            RemoteWindowDemandClassV1::PresentationRequired,
        );
        let partition_key = batch.job_key_v1().partition_v1();
        coordinator
            .stage_terminal_v1(batch.clone(), &store, &capability, &index)
            .unwrap();
        coordinator
            .commit_staged_v1(&mut store, &capability, &mut index)
            .unwrap();
        assert!(
            index
                .is_partition_fully_resident_v1(&store, &capability, partition_key)
                .unwrap()
        );

        coordinator.rebind_generation_v1(2, 2);
        assert_eq!(
            coordinator.stage_terminal_v1(batch, &store, &capability, &index),
            Err(RemotePartitionJobErrorV1::StaleWork)
        );

        let (deletions, _) = store.gc(&GarbageCollectionOptions::gc_everything());
        if !deletions.is_empty() {
            index
                .observe_store_events_v1(&store, &capability, &deletions)
                .unwrap();
        }
        assert!(
            !index
                .is_partition_fully_resident_v1(&store, &capability, partition_key)
                .unwrap()
        );
    }
}
