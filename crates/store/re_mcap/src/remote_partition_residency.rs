//! Atomic partition/root registration for production-disarmed Web remote-MCAP Stores.

#![allow(dead_code)]

use std::collections::BTreeSet;

use ahash::{HashMap, HashSet};
use re_byte_size::SizeBytes as _;
use re_chunk::{Chunk, ChunkId};
use re_chunk_store::{
    ChunkStore, ChunkStoreEvent, ExternalRefetchableRootDescriptorV1,
    ExternalRefetchableRootErrorV1, ExternalRefetchableRootExistenceV1,
    ExternalRefetchableRootOriginV1, WebRemoteMcapRootCapabilityV1, WebRemoteMcapStoreIdentityV1,
};

use crate::remote_chunk_dispatch::{RemoteChunkTerminalV1, RemoteTypedPartitionHandoffV1};
use crate::remote_loaded_coverage::{
    CanonicalIndexedExtentV1, CompleteIndexedCoverageV1, PartitionSatisfactionTransitionV1,
    RemoteLoadedCoverageIndexV1, RemoteTemporalCoveragePlanV1,
};

use crate::remote_manifest::{
    DerivationPartitionKeyV1, DerivationPartitionKindV1, ImmutableRemoteMcapManifestV1,
    ManifestOpeningStaticAuthorityV1, ManifestPartitionDescriptorV1, ManifestRootDescriptorV1,
    ManifestTemporalPartitionAuthorityV1, RemoteManifestErrorV1, RemoteMcapSessionIdV1,
    RemoteRegistrationCapacityV1, RemoteRegistrationReservationV1,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PartitionResidencyV1 {
    Unknown,
    InTransit,
    CompleteEmpty,
    Roots(Box<[ChunkId]>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RefetchableRootLoadStateV1 {
    Unloaded,
    InTransit,
    FullyLoaded,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RemoteRegistrationCountersV1 {
    pub(crate) registered_partitions: u64,
    pub(crate) complete_empty_entries: u64,
    pub(crate) root_descriptors: u64,
    pub(crate) external_origin_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RegisteredRootV1 {
    partition: DerivationPartitionKeyV1,
    manifest_descriptor: ManifestRootDescriptorV1,
    external_descriptor: ExternalRefetchableRootDescriptorV1,
    estimated_bytes: u64,
    load_state: RefetchableRootLoadStateV1,
}

impl RegisteredRootV1 {
    fn has_same_registration_v1(&self, other: &Self) -> bool {
        self.partition == other.partition
            && self.manifest_descriptor == other.manifest_descriptor
            && self.external_descriptor == other.external_descriptor
            && self.estimated_bytes == other.estimated_bytes
    }
}

pub(crate) struct RefetchableRootIndexV1 {
    session_id: RemoteMcapSessionIdV1,
    store_identity: WebRemoteMcapStoreIdentityV1,
    limits: RemoteRegistrationCapacityV1,
    counters: RemoteRegistrationCountersV1,
    partitions: HashMap<DerivationPartitionKeyV1, PartitionResidencyV1>,
    roots: HashMap<ChunkId, RegisteredRootV1>,
    store_origin_capacity_bytes: u64,
    allocated_store_origin_capacity_bytes: u64,
    loaded_coverage: RemoteLoadedCoverageIndexV1,
    _loaded_coverage_reservation: RemoteRegistrationReservationV1,
    _reservation: RemoteRegistrationReservationV1,
    temporary_reservation: RemoteRegistrationReservationV1,
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
struct RegistrySnapshotV1 {
    store_identity: WebRemoteMcapStoreIdentityV1,
    counters: RemoteRegistrationCountersV1,
    partitions: HashMap<DerivationPartitionKeyV1, PartitionResidencyV1>,
    roots: HashMap<ChunkId, RegisteredRootV1>,
    allocated_store_origin_capacity_bytes: u64,
    loaded_coverage: crate::remote_loaded_coverage::RemoteLoadedCoverageSnapshotV1,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PreparedRootRegistrationV1 {
    manifest_descriptor: ManifestRootDescriptorV1,
    estimated_bytes: u64,
    is_static: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PreparedPartitionOutcomeV1 {
    CompleteEmpty,
    Roots {
        roots: Box<[PreparedRootRegistrationV1]>,
        root_ids: Box<[ChunkId]>,
    },
}

fn validate_prepared_root_ids_v1(
    roots: &[PreparedRootRegistrationV1],
    root_ids: &[ChunkId],
) -> Result<(), RootRegistrationErrorV1> {
    if roots.len() != root_ids.len() {
        return Err(RootRegistrationErrorV1::InvalidManifestIdentity);
    }
    if root_ids.windows(2).any(|ids| ids[0] >= ids[1])
        || roots.iter().any(|root| {
            root_ids
                .binary_search(&root.manifest_descriptor.root_chunk_id_v1())
                .is_err()
        })
    {
        return Err(RootRegistrationErrorV1::RegistrationConflict);
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PreparedPartitionRegistrationV1 {
    partition: ManifestPartitionDescriptorV1,
    outcome: PreparedPartitionOutcomeV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum RootRegistrationErrorV1 {
    #[error("the remote MCAP partition registration exceeds its session limit")]
    ResourceLimitExceeded,

    #[error("the remote MCAP partition registration conflicts with existing metadata")]
    RegistrationConflict,

    #[error("the remote MCAP partition registration does not match its immutable manifest")]
    InvalidManifestIdentity,

    #[error("the remote MCAP terminal outcome is not registrable")]
    InvalidTerminalOutcome,

    #[error("the remote MCAP derived root kind does not match its partition")]
    PartitionKindMismatch,

    #[error("the Web remote-MCAP Store capability is unavailable")]
    StoreCapability,
}

impl From<ExternalRefetchableRootErrorV1> for RootRegistrationErrorV1 {
    fn from(_error: ExternalRefetchableRootErrorV1) -> Self {
        Self::StoreCapability
    }
}

impl PreparedPartitionRegistrationV1 {
    pub(crate) const fn partition_v1(&self) -> ManifestPartitionDescriptorV1 {
        self.partition
    }

    pub(crate) fn expected_root_chunk_ids_v1(&self) -> Option<Box<[ChunkId]>> {
        match &self.outcome {
            PreparedPartitionOutcomeV1::CompleteEmpty => None,
            PreparedPartitionOutcomeV1::Roots { root_ids, .. } => Some(root_ids.clone()),
        }
    }

    pub(crate) fn registered_root_for_insertion_v1(
        &self,
        root_chunk_id: ChunkId,
    ) -> Option<(ManifestRootDescriptorV1, bool)> {
        match &self.outcome {
            PreparedPartitionOutcomeV1::CompleteEmpty => None,
            PreparedPartitionOutcomeV1::Roots { roots, .. } => roots
                .iter()
                .find(|root| root.manifest_descriptor.root_chunk_id_v1() == root_chunk_id)
                .map(|root| (root.manifest_descriptor, root.is_static)),
        }
    }

    fn complete_empty_v1(partition: ManifestPartitionDescriptorV1) -> Self {
        Self {
            partition,
            outcome: PreparedPartitionOutcomeV1::CompleteEmpty,
        }
    }

    fn complete_roots_v1(
        partition: ManifestPartitionDescriptorV1,
        handoff: &RemoteTypedPartitionHandoffV1,
    ) -> Result<Self, RootRegistrationErrorV1> {
        let roots = handoff
            .root_handoffs_v1()
            .map(|handoff| {
                let chunk = handoff.chunk_v1();
                let manifest_descriptor = handoff.root_v1();
                if manifest_descriptor.partition_key_v1() != partition.key_v1()
                    || manifest_descriptor.root_chunk_id_v1() != chunk.id()
                {
                    return Err(RootRegistrationErrorV1::InvalidManifestIdentity);
                }
                let kind_matches = match partition.key_v1().kind_v1() {
                    DerivationPartitionKindV1::OpeningStatic => chunk.is_static(),
                    DerivationPartitionKindV1::TemporalChannelGroup(_) => !chunk.is_static(),
                };
                if !kind_matches {
                    return Err(RootRegistrationErrorV1::PartitionKindMismatch);
                }
                Ok(PreparedRootRegistrationV1 {
                    manifest_descriptor,
                    estimated_bytes: chunk.total_size_bytes(),
                    is_static: chunk.is_static(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if roots.is_empty()
            || roots.len()
                > usize::try_from(partition.registration_bound_v1())
                    .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?
        {
            return Err(RootRegistrationErrorV1::ResourceLimitExceeded);
        }
        let unique_roots = roots
            .iter()
            .map(|root| root.manifest_descriptor.root_chunk_id_v1())
            .collect::<BTreeSet<_>>();
        if unique_roots.len() != roots.len() {
            return Err(RootRegistrationErrorV1::RegistrationConflict);
        }
        Ok(Self {
            partition,
            outcome: PreparedPartitionOutcomeV1::Roots {
                root_ids: unique_roots.into_iter().collect(),
                roots: roots.into_boxed_slice(),
            },
        })
    }
}

#[cfg(test)]
impl PreparedPartitionRegistrationV1 {
    pub(crate) fn complete_empty_for_test_v1(partition: ManifestPartitionDescriptorV1) -> Self {
        Self::complete_empty_v1(partition)
    }

    pub(crate) fn complete_roots_for_test_v1(
        partition: ManifestPartitionDescriptorV1,
        roots: &[ManifestRootDescriptorV1],
        estimated_bytes: u64,
        is_static: bool,
    ) -> Result<Self, RootRegistrationErrorV1> {
        if roots.is_empty()
            || roots.len()
                > usize::try_from(partition.registration_bound_v1())
                    .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?
        {
            return Err(RootRegistrationErrorV1::ResourceLimitExceeded);
        }
        let prepared = roots
            .iter()
            .map(|root| PreparedRootRegistrationV1 {
                manifest_descriptor: *root,
                estimated_bytes,
                is_static,
            })
            .collect::<Vec<_>>();
        let root_ids = prepared
            .iter()
            .map(|root| root.manifest_descriptor.root_chunk_id_v1())
            .collect::<BTreeSet<_>>();
        if root_ids.len() != prepared.len() {
            return Err(RootRegistrationErrorV1::RegistrationConflict);
        }
        Ok(Self {
            partition,
            outcome: PreparedPartitionOutcomeV1::Roots {
                roots: prepared.into_boxed_slice(),
                root_ids: root_ids.into_iter().collect(),
            },
        })
    }
}

pub(crate) fn prepare_temporal_terminal_registration_v1(
    authority: &ManifestTemporalPartitionAuthorityV1<'_, '_, '_, '_, '_, '_>,
    terminal: &RemoteChunkTerminalV1,
) -> Result<PreparedPartitionRegistrationV1, RootRegistrationErrorV1> {
    authority
        .ensure_current_v1()
        .map_err(|_error| RootRegistrationErrorV1::InvalidManifestIdentity)?;
    match terminal {
        RemoteChunkTerminalV1::Complete(handoff) => {
            PreparedPartitionRegistrationV1::complete_roots_v1(authority.partition_v1(), handoff)
        }
        RemoteChunkTerminalV1::CompleteEmpty => Ok(
            PreparedPartitionRegistrationV1::complete_empty_v1(authority.partition_v1()),
        ),
        RemoteChunkTerminalV1::Failed(_) => Err(RootRegistrationErrorV1::InvalidTerminalOutcome),
    }
}

#[cfg(test)]
pub(crate) fn prepare_terminal_registration_for_test_v1(
    terminal: &RemoteChunkTerminalV1,
) -> Result<PreparedPartitionRegistrationV1, RootRegistrationErrorV1> {
    match terminal {
        RemoteChunkTerminalV1::Complete(handoff) => {
            PreparedPartitionRegistrationV1::complete_roots_v1(handoff.partition_v1(), handoff)
        }
        RemoteChunkTerminalV1::CompleteEmpty => {
            Err(RootRegistrationErrorV1::InvalidTerminalOutcome)
        }
        RemoteChunkTerminalV1::Failed(_) => Err(RootRegistrationErrorV1::InvalidTerminalOutcome),
    }
}

pub(crate) struct RemoteOpeningStaticRootHandoffV1 {
    chunk: Chunk,
    root: ManifestRootDescriptorV1,
}

impl RemoteOpeningStaticRootHandoffV1 {
    pub(crate) fn seal_v1(
        authority: &ManifestOpeningStaticAuthorityV1<'_, '_, '_, '_, '_, '_>,
        output_ordinal: u32,
        chunk: Chunk,
    ) -> Result<Self, RootRegistrationErrorV1> {
        authority
            .ensure_current_v1()
            .map_err(|_error| RootRegistrationErrorV1::InvalidManifestIdentity)?;
        let root = authority
            .issue_root_v1(output_ordinal)
            .map_err(|_error| RootRegistrationErrorV1::InvalidManifestIdentity)?;
        if !chunk.is_static() || chunk.id() != root.root_chunk_id_v1() {
            return Err(RootRegistrationErrorV1::PartitionKindMismatch);
        }
        Ok(Self { chunk, root })
    }
}

pub(crate) enum RemoteOpeningStaticTerminalV1 {
    Complete(Box<[RemoteOpeningStaticRootHandoffV1]>),
    CompleteEmpty,
    Failed,
}

pub(crate) fn prepare_opening_static_terminal_registration_v1(
    authority: &ManifestOpeningStaticAuthorityV1<'_, '_, '_, '_, '_, '_>,
    terminal: &RemoteOpeningStaticTerminalV1,
) -> Result<PreparedPartitionRegistrationV1, RootRegistrationErrorV1> {
    authority
        .ensure_current_v1()
        .map_err(|_error| RootRegistrationErrorV1::InvalidManifestIdentity)?;
    match terminal {
        RemoteOpeningStaticTerminalV1::Complete(roots) => {
            if roots.is_empty()
                || roots.len()
                    > usize::try_from(authority.partition_v1().registration_bound_v1())
                        .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?
            {
                return Err(RootRegistrationErrorV1::ResourceLimitExceeded);
            }
            let prepared = roots
                .iter()
                .map(|handoff| PreparedRootRegistrationV1 {
                    manifest_descriptor: handoff.root,
                    estimated_bytes: handoff.chunk.total_size_bytes(),
                    is_static: true,
                })
                .collect::<Vec<_>>();
            let root_ids = prepared
                .iter()
                .map(|root| root.manifest_descriptor.root_chunk_id_v1())
                .collect::<BTreeSet<_>>();
            if root_ids.len() != prepared.len() {
                return Err(RootRegistrationErrorV1::RegistrationConflict);
            }
            Ok(PreparedPartitionRegistrationV1 {
                partition: authority.partition_v1(),
                outcome: PreparedPartitionOutcomeV1::Roots {
                    roots: prepared.into_boxed_slice(),
                    root_ids: root_ids.into_iter().collect(),
                },
            })
        }
        RemoteOpeningStaticTerminalV1::CompleteEmpty => Ok(
            PreparedPartitionRegistrationV1::complete_empty_v1(authority.partition_v1()),
        ),
        RemoteOpeningStaticTerminalV1::Failed => {
            Err(RootRegistrationErrorV1::InvalidTerminalOutcome)
        }
    }
}

pub(crate) fn registry_retained_bytes_for_capacity_v1(
    capacity: RemoteRegistrationCapacityV1,
) -> Result<u64, RemoteManifestErrorV1> {
    let index_bytes = registry_retained_bytes_with_hash_slack_v1(capacity, 4)?;
    let store_origin_bytes =
        ExternalRefetchableRootOriginV1::store_lineage_retained_bytes_for_roots_v1(
            capacity.max_root_descriptors,
            4,
        )
        .ok_or(RemoteManifestErrorV1::ArithmeticOverflow)?;
    index_bytes
        .checked_add(store_origin_bytes)
        .ok_or(RemoteManifestErrorV1::ArithmeticOverflow)
}

pub(crate) fn temporary_registration_bytes_v1(
    partitions: u64,
    roots: u64,
) -> Result<u64, RootRegistrationErrorV1> {
    temporary_registration_bytes_with_hash_slack_v1(partitions, roots, 4)
}

fn temporary_registration_bytes_with_hash_slack_v1(
    partitions: u64,
    roots: u64,
    hash_capacity_slack: u64,
) -> Result<u64, RootRegistrationErrorV1> {
    const HASH_BUCKET_OVERHEAD_V1: u64 = 32;
    let partition_bucket = u64::try_from(
        std::mem::size_of::<DerivationPartitionKeyV1>()
            + std::mem::size_of::<&PreparedPartitionOutcomeV1>(),
    )
    .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?
    .checked_add(HASH_BUCKET_OVERHEAD_V1)
    .ok_or(RootRegistrationErrorV1::ResourceLimitExceeded)?;
    let root_bucket =
        u64::try_from(std::mem::size_of::<ChunkId>() + std::mem::size_of::<RegisteredRootV1>())
            .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?
            .checked_add(HASH_BUCKET_OVERHEAD_V1)
            .ok_or(RootRegistrationErrorV1::ResourceLimitExceeded)?;
    let partition_bytes = partitions
        .checked_mul(hash_capacity_slack)
        .ok_or(RootRegistrationErrorV1::ResourceLimitExceeded)?
        .checked_mul(partition_bucket)
        .ok_or(RootRegistrationErrorV1::ResourceLimitExceeded)?;
    let root_bytes = roots
        .checked_mul(hash_capacity_slack)
        .ok_or(RootRegistrationErrorV1::ResourceLimitExceeded)?
        .checked_mul(root_bucket)
        .ok_or(RootRegistrationErrorV1::ResourceLimitExceeded)?;
    partition_bytes
        .checked_add(root_bytes)
        .ok_or(RootRegistrationErrorV1::ResourceLimitExceeded)
}

fn registry_retained_bytes_with_hash_slack_v1(
    capacity: RemoteRegistrationCapacityV1,
    hash_capacity_slack: u64,
) -> Result<u64, RemoteManifestErrorV1> {
    const HASH_BUCKET_OVERHEAD_V1: u64 = 32;
    let partition_bucket = u64::try_from(
        std::mem::size_of::<DerivationPartitionKeyV1>()
            + std::mem::size_of::<PartitionResidencyV1>(),
    )
    .map_err(|_error| RemoteManifestErrorV1::ArithmeticOverflow)?
    .checked_add(HASH_BUCKET_OVERHEAD_V1)
    .ok_or(RemoteManifestErrorV1::ArithmeticOverflow)?;
    let root_bucket =
        u64::try_from(std::mem::size_of::<ChunkId>() + std::mem::size_of::<RegisteredRootV1>())
            .map_err(|_error| RemoteManifestErrorV1::ArithmeticOverflow)?
            .checked_add(HASH_BUCKET_OVERHEAD_V1)
            .ok_or(RemoteManifestErrorV1::ArithmeticOverflow)?;
    let partition_bytes = capacity
        .max_registered_partitions
        .checked_mul(hash_capacity_slack)
        .and_then(|count| count.checked_mul(partition_bucket))
        .ok_or(RemoteManifestErrorV1::ArithmeticOverflow)?;
    let root_bytes = capacity
        .max_root_descriptors
        .checked_mul(hash_capacity_slack)
        .and_then(|count| count.checked_mul(root_bucket))
        .ok_or(RemoteManifestErrorV1::ArithmeticOverflow)?;
    let root_set_bytes = capacity
        .max_root_descriptors
        .checked_mul(
            u64::try_from(std::mem::size_of::<ChunkId>())
                .map_err(|_error| RemoteManifestErrorV1::ArithmeticOverflow)?,
        )
        .ok_or(RemoteManifestErrorV1::ArithmeticOverflow)?;
    u64::try_from(
        std::mem::size_of::<RefetchableRootIndexV1>()
            + std::mem::size_of::<HashMap<DerivationPartitionKeyV1, PartitionResidencyV1>>()
            + std::mem::size_of::<HashMap<ChunkId, RegisteredRootV1>>(),
    )
    .map_err(|_error| RemoteManifestErrorV1::ArithmeticOverflow)?
    .checked_add(partition_bytes)
    .and_then(|bytes| bytes.checked_add(root_bytes))
    .and_then(|bytes| bytes.checked_add(root_set_bytes))
    .ok_or(RemoteManifestErrorV1::ArithmeticOverflow)
}

impl RefetchableRootIndexV1 {
    #[cfg(test)]
    fn snapshot_v1(&self) -> RegistrySnapshotV1 {
        RegistrySnapshotV1 {
            store_identity: self.store_identity.clone(),
            counters: self.counters,
            partitions: self.partitions.clone(),
            roots: self.roots.clone(),
            allocated_store_origin_capacity_bytes: self.allocated_store_origin_capacity_bytes,
            loaded_coverage: self.loaded_coverage.snapshot_v1(),
        }
    }

    pub(crate) fn for_manifest_v1(
        manifest: &mut ImmutableRemoteMcapManifestV1<'_, '_, '_, '_, '_>,
        store: &ChunkStore,
        capability: &WebRemoteMcapRootCapabilityV1,
    ) -> Result<Self, RootRegistrationErrorV1> {
        let store_identity = capability.store_identity_v1(store)?;
        let limits = manifest.registration_capacity_v1();
        let mut partitions = HashMap::default();
        let mut roots = HashMap::default();
        partitions
            .try_reserve(
                usize::try_from(limits.max_registered_partitions)
                    .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?,
            )
            .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?;
        roots
            .try_reserve(
                usize::try_from(limits.max_root_descriptors)
                    .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?,
            )
            .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?;
        let actual_capacity = RemoteRegistrationCapacityV1 {
            max_registered_partitions: u64::try_from(partitions.capacity())
                .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?,
            max_complete_empty_entries: limits.max_complete_empty_entries,
            max_root_descriptors: u64::try_from(roots.capacity())
                .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?,
            max_external_origin_bytes: limits.max_external_origin_bytes,
        };
        let index_bytes = registry_retained_bytes_with_hash_slack_v1(actual_capacity, 1)
            .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?;
        let store_origin_retained_bytes =
            ExternalRefetchableRootOriginV1::store_lineage_retained_bytes_for_roots_v1(
                limits.max_root_descriptors,
                4,
            )
            .ok_or(RootRegistrationErrorV1::ResourceLimitExceeded)?;
        let actual_bytes = index_bytes
            .checked_add(store_origin_retained_bytes)
            .ok_or(RootRegistrationErrorV1::ResourceLimitExceeded)?;
        let (reservation, temporary_reservation) = manifest
            .take_registration_reservation_v1()
            .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?;
        let (coverage_plan, loaded_coverage_reservation) = manifest
            .take_temporal_coverage_plan_v1()
            .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?;
        let loaded_coverage =
            RemoteLoadedCoverageIndexV1::from_plan_v1(coverage_plan, &loaded_coverage_reservation)
                .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?;
        if actual_bytes > reservation.bytes_v1() {
            return Err(RootRegistrationErrorV1::ResourceLimitExceeded);
        }
        Ok(Self {
            session_id: manifest.session_id_v1(),
            store_identity,
            limits,
            counters: RemoteRegistrationCountersV1::default(),
            partitions,
            roots,
            store_origin_capacity_bytes:
                ExternalRefetchableRootOriginV1::store_lineage_capacity_bytes_for_roots_v1(
                    limits.max_root_descriptors,
                    4,
                )
                .ok_or(RootRegistrationErrorV1::ResourceLimitExceeded)?,
            allocated_store_origin_capacity_bytes: 0,
            loaded_coverage,
            _loaded_coverage_reservation: loaded_coverage_reservation,
            _reservation: reservation,
            temporary_reservation,
        })
    }

    #[cfg(test)]
    pub(crate) fn new_v1(
        session_id: RemoteMcapSessionIdV1,
        limits: RemoteRegistrationCapacityV1,
        store: &ChunkStore,
        capability: &WebRemoteMcapRootCapabilityV1,
        coverage_plan: RemoteTemporalCoveragePlanV1,
    ) -> Result<Self, RootRegistrationErrorV1> {
        let bytes = registry_retained_bytes_for_capacity_v1(limits)
            .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?;
        let temporary_bytes = temporary_registration_bytes_v1(
            limits.max_registered_partitions,
            limits.max_root_descriptors,
        )?;
        let coverage_bytes = coverage_plan
            .retained_index_bytes_v1()
            .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?;
        let budget = crate::remote_manifest::RemoteRegistrationBudgetV1::new_disarmed_v1(
            bytes
                .checked_add(temporary_bytes)
                .and_then(|bytes| bytes.checked_add(coverage_bytes))
                .ok_or(RootRegistrationErrorV1::ResourceLimitExceeded)?,
        );
        let reservation = budget
            .reserve_v1(bytes)
            .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?;
        let temporary_reservation = budget
            .reserve_v1(temporary_bytes)
            .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?;
        let loaded_coverage_reservation = budget
            .reserve_v1(coverage_bytes)
            .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?;
        let mut partitions = HashMap::default();
        let mut roots = HashMap::default();
        partitions
            .try_reserve(
                usize::try_from(limits.max_registered_partitions)
                    .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?,
            )
            .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?;
        roots
            .try_reserve(
                usize::try_from(limits.max_root_descriptors)
                    .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?,
            )
            .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?;
        let loaded_coverage =
            RemoteLoadedCoverageIndexV1::from_plan_v1(coverage_plan, &loaded_coverage_reservation)
                .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?;
        Ok(Self {
            session_id,
            store_identity: capability.store_identity_v1(store)?,
            limits,
            counters: RemoteRegistrationCountersV1::default(),
            partitions,
            roots,
            store_origin_capacity_bytes:
                ExternalRefetchableRootOriginV1::store_lineage_capacity_bytes_for_roots_v1(
                    limits.max_root_descriptors,
                    4,
                )
                .ok_or(RootRegistrationErrorV1::ResourceLimitExceeded)?,
            allocated_store_origin_capacity_bytes: 0,
            loaded_coverage,
            _loaded_coverage_reservation: loaded_coverage_reservation,
            _reservation: reservation,
            temporary_reservation,
        })
    }

    pub(crate) fn counters_v1(&self) -> RemoteRegistrationCountersV1 {
        self.counters
    }

    pub(crate) fn root_refetch_descriptor_v1(
        &self,
        root: ChunkId,
    ) -> Option<ExternalRefetchableRootDescriptorV1> {
        self.roots.get(&root).map(|root| root.external_descriptor)
    }

    pub(crate) fn root_load_state_v1(&self, root: ChunkId) -> Option<RefetchableRootLoadStateV1> {
        self.roots.get(&root).map(|root| root.load_state)
    }

    pub(crate) fn root_manifest_registration_v1(
        &self,
        root: ChunkId,
    ) -> Option<(ManifestRootDescriptorV1, bool)> {
        self.roots.get(&root).map(|root| {
            (
                root.manifest_descriptor,
                matches!(
                    root.partition.kind_v1(),
                    DerivationPartitionKindV1::OpeningStatic
                ),
            )
        })
    }

    pub(crate) fn partition_residency_v1(
        &self,
        partition: DerivationPartitionKeyV1,
    ) -> PartitionResidencyV1 {
        self.partitions
            .get(&partition)
            .cloned()
            .unwrap_or(PartitionResidencyV1::Unknown)
    }

    pub(crate) fn register_commit_set_v1(
        &mut self,
        store: &mut ChunkStore,
        capability: &WebRemoteMcapRootCapabilityV1,
        registrations: Vec<PreparedPartitionRegistrationV1>,
    ) -> Result<(), RootRegistrationErrorV1> {
        if capability.store_identity_v1(store)? != self.store_identity {
            return Err(RootRegistrationErrorV1::StoreCapability);
        }
        let input_partitions = u64::try_from(registrations.len())
            .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?;
        let input_roots = registrations
            .iter()
            .try_fold(0_u64, |count, registration| {
                let roots = match &registration.outcome {
                    PreparedPartitionOutcomeV1::CompleteEmpty => 0,
                    PreparedPartitionOutcomeV1::Roots { roots, .. } => {
                        u64::try_from(roots.len())
                            .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?
                    }
                };
                count
                    .checked_add(roots)
                    .ok_or(RootRegistrationErrorV1::ResourceLimitExceeded)
            })?;
        if input_partitions > self.limits.max_registered_partitions
            || input_roots > self.limits.max_root_descriptors
        {
            return Err(RootRegistrationErrorV1::ResourceLimitExceeded);
        }
        let temporary_bytes = temporary_registration_bytes_v1(input_partitions, input_roots)?;
        if temporary_bytes > self.temporary_reservation.bytes_v1() {
            return Err(RootRegistrationErrorV1::ResourceLimitExceeded);
        }
        let mut partition_delta =
            HashMap::<DerivationPartitionKeyV1, &PreparedPartitionOutcomeV1>::default();
        let mut root_delta = HashMap::<ChunkId, RegisteredRootV1>::default();
        partition_delta
            .try_reserve(
                usize::try_from(input_partitions)
                    .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?,
            )
            .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?;
        root_delta
            .try_reserve(
                usize::try_from(input_roots)
                    .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?,
            )
            .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?;
        let actual_temporary_bytes = temporary_registration_bytes_with_hash_slack_v1(
            u64::try_from(partition_delta.capacity())
                .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?,
            u64::try_from(root_delta.capacity())
                .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?,
            1,
        )?;
        if actual_temporary_bytes > self.temporary_reservation.bytes_v1() {
            return Err(RootRegistrationErrorV1::ResourceLimitExceeded);
        }
        let mut next_counters = self.counters;
        let mut new_partition_map_entries = 0_u64;

        for registration in &registrations {
            let partition_key = registration.partition.key_v1();
            if partition_key.session_id_v1() != self.session_id {
                return Err(RootRegistrationErrorV1::InvalidManifestIdentity);
            }
            if matches!(
                partition_key.kind_v1(),
                DerivationPartitionKindV1::TemporalChannelGroup(_)
            ) {
                self.loaded_coverage
                    .validate_temporal_source_unit_v1(partition_key.source_unit_ordinal_v1())
                    .map_err(|_error| RootRegistrationErrorV1::InvalidManifestIdentity)?;
            }
            if let PreparedPartitionOutcomeV1::Roots { roots, root_ids } = &registration.outcome {
                if roots.is_empty()
                    || roots.len()
                        > usize::try_from(registration.partition.registration_bound_v1())
                            .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?
                {
                    return Err(RootRegistrationErrorV1::ResourceLimitExceeded);
                }
                validate_prepared_root_ids_v1(roots, root_ids)?;
                for root in roots {
                    if root.manifest_descriptor.partition_key_v1() != partition_key
                        || root.manifest_descriptor.output_ordinal_v1()
                            >= registration.partition.registration_bound_v1()
                    {
                        return Err(RootRegistrationErrorV1::InvalidManifestIdentity);
                    }
                    let kind_matches = match partition_key.kind_v1() {
                        DerivationPartitionKindV1::OpeningStatic => root.is_static,
                        DerivationPartitionKindV1::TemporalChannelGroup(_) => !root.is_static,
                    };
                    if !kind_matches {
                        return Err(RootRegistrationErrorV1::PartitionKindMismatch);
                    }
                }
            }
            let already_registered = match self.partitions.get(&partition_key) {
                Some(existing)
                    if match (existing, &registration.outcome) {
                        (
                            PartitionResidencyV1::CompleteEmpty,
                            PreparedPartitionOutcomeV1::CompleteEmpty,
                        ) => true,
                        (
                            PartitionResidencyV1::Roots(existing),
                            PreparedPartitionOutcomeV1::Roots { root_ids, .. },
                        ) => existing.as_ref() == root_ids.as_ref(),
                        _ => false,
                    } =>
                {
                    true
                }
                Some(PartitionResidencyV1::InTransit | PartitionResidencyV1::Unknown) | None => {
                    if let Some(prior) = partition_delta.get(&partition_key) {
                        let prior_matches = match (*prior, &registration.outcome) {
                            (
                                PreparedPartitionOutcomeV1::CompleteEmpty,
                                PreparedPartitionOutcomeV1::CompleteEmpty,
                            ) => true,
                            (
                                PreparedPartitionOutcomeV1::Roots {
                                    root_ids: prior, ..
                                },
                                PreparedPartitionOutcomeV1::Roots {
                                    root_ids: current, ..
                                },
                            ) => prior == current,
                            _ => false,
                        };
                        if !prior_matches {
                            return Err(RootRegistrationErrorV1::RegistrationConflict);
                        }
                        true
                    } else {
                        if !self.partitions.contains_key(&partition_key) {
                            new_partition_map_entries = new_partition_map_entries
                                .checked_add(1)
                                .ok_or(RootRegistrationErrorV1::ResourceLimitExceeded)?;
                        }
                        partition_delta.insert(partition_key, &registration.outcome);
                        false
                    }
                }
                Some(_) => return Err(RootRegistrationErrorV1::RegistrationConflict),
            };

            if !already_registered {
                next_counters.registered_partitions = next_counters
                    .registered_partitions
                    .checked_add(1)
                    .filter(|count| *count <= self.limits.max_registered_partitions)
                    .ok_or(RootRegistrationErrorV1::ResourceLimitExceeded)?;
            }

            match &registration.outcome {
                PreparedPartitionOutcomeV1::CompleteEmpty => {
                    if !already_registered {
                        next_counters.complete_empty_entries = next_counters
                            .complete_empty_entries
                            .checked_add(1)
                            .filter(|count| *count <= self.limits.max_complete_empty_entries)
                            .ok_or(RootRegistrationErrorV1::ResourceLimitExceeded)?;
                    }
                }
                PreparedPartitionOutcomeV1::Roots { roots, .. } => {
                    for root in roots {
                        let root_chunk_id = root.manifest_descriptor.root_chunk_id_v1();
                        let external_descriptor = capability.validate_root_origin_v1(
                            store,
                            root_chunk_id,
                            root.is_static,
                        )?;
                        let registered = RegisteredRootV1 {
                            partition: partition_key,
                            manifest_descriptor: root.manifest_descriptor,
                            external_descriptor,
                            estimated_bytes: root.estimated_bytes,
                            load_state: RefetchableRootLoadStateV1::Unloaded,
                        };
                        match self.roots.get(&root_chunk_id) {
                            Some(existing) if existing.has_same_registration_v1(&registered) => {}
                            Some(_) => {
                                return Err(RootRegistrationErrorV1::RegistrationConflict);
                            }
                            None => {
                                if let Some(prior) = root_delta.get(&root_chunk_id) {
                                    if !prior.has_same_registration_v1(&registered) {
                                        return Err(RootRegistrationErrorV1::RegistrationConflict);
                                    }
                                    continue;
                                }
                                root_delta.insert(root_chunk_id, registered);
                                next_counters.root_descriptors = next_counters
                                    .root_descriptors
                                    .checked_add(1)
                                    .filter(|count| *count <= self.limits.max_root_descriptors)
                                    .ok_or(RootRegistrationErrorV1::ResourceLimitExceeded)?;
                                next_counters.external_origin_bytes = next_counters
                                    .external_origin_bytes
                                    .checked_add(ExternalRefetchableRootOriginV1::ENCODED_BYTES_V1)
                                    .filter(|bytes| *bytes <= self.limits.max_external_origin_bytes)
                                    .ok_or(RootRegistrationErrorV1::ResourceLimitExceeded)?;
                            }
                        }
                    }
                }
            }
        }

        let new_root_count = next_counters
            .root_descriptors
            .checked_sub(self.counters.root_descriptors)
            .ok_or(RootRegistrationErrorV1::RegistrationConflict)?;
        if self
            .partitions
            .len()
            .checked_add(
                usize::try_from(new_partition_map_entries)
                    .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?,
            )
            .and_then(|count| u64::try_from(count).ok())
            .is_none_or(|count| count > self.limits.max_registered_partitions)
            || self
                .roots
                .len()
                .checked_add(
                    usize::try_from(new_root_count)
                        .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?,
                )
                .and_then(|count| u64::try_from(count).ok())
                .is_none_or(|count| count > self.limits.max_root_descriptors)
        {
            return Err(RootRegistrationErrorV1::ResourceLimitExceeded);
        }
        let allocated_store_origin_bytes = capability.try_reserve_root_origins_v1(
            store,
            usize::try_from(new_root_count)
                .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?,
        )?;
        let next_allocated_store_origin_capacity_bytes = self
            .allocated_store_origin_capacity_bytes
            .checked_add(allocated_store_origin_bytes)
            .expect("admitted remote root origin capacity accounting cannot overflow");
        assert!(
            next_allocated_store_origin_capacity_bytes <= self.store_origin_capacity_bytes,
            "reserved remote root origin capacity exceeds its admitted retained-byte headroom"
        );
        self.allocated_store_origin_capacity_bytes = next_allocated_store_origin_capacity_bytes;

        for (root_chunk_id, root) in &root_delta {
            capability
                .register_root_origin_v1(
                    store,
                    *root_chunk_id,
                    root.external_descriptor.is_static_v1(),
                )
                .expect("validated and reserved remote root origin commit cannot fail");
        }
        self.roots.extend(root_delta);

        let mut coverage_ranges_dirty = false;
        for registration in registrations {
            let partition_key = registration.partition.key_v1();
            if self
                .partitions
                .get(&partition_key)
                .is_some_and(|residency| {
                    !matches!(
                        residency,
                        PartitionResidencyV1::InTransit | PartitionResidencyV1::Unknown
                    )
                })
            {
                continue;
            }
            let residency = match registration.outcome {
                PreparedPartitionOutcomeV1::CompleteEmpty => PartitionResidencyV1::CompleteEmpty,
                PreparedPartitionOutcomeV1::Roots { root_ids, .. } => {
                    PartitionResidencyV1::Roots(root_ids)
                }
            };
            self.partitions.insert(partition_key, residency);
            if matches!(
                self.partitions.get(&partition_key),
                Some(PartitionResidencyV1::CompleteEmpty)
            ) && matches!(
                partition_key.kind_v1(),
                DerivationPartitionKindV1::TemporalChannelGroup(_)
            ) {
                coverage_ranges_dirty |= self
                    .loaded_coverage
                    .apply_partition_transition_in_batch_v1(
                        partition_key.source_unit_ordinal_v1(),
                        PartitionSatisfactionTransitionV1::BecameSatisfied,
                    )
                    .expect("sealed temporal partition registration matches its coverage plan");
            }
        }
        self.loaded_coverage
            .finish_transition_batch_v1(coverage_ranges_dirty)
            .expect("sealed temporal partition registration keeps coverage counters consistent");
        self.counters = next_counters;
        Ok(())
    }

    pub(crate) fn mark_partition_in_transit_v1(
        &mut self,
        partition: ManifestPartitionDescriptorV1,
    ) -> Result<(), RootRegistrationErrorV1> {
        let partition = partition.key_v1();
        if partition.session_id_v1() != self.session_id {
            return Err(RootRegistrationErrorV1::InvalidManifestIdentity);
        }
        if !self.partitions.contains_key(&partition)
            && u64::try_from(self.partitions.len())
                .ok()
                .is_none_or(|len| len >= self.limits.max_registered_partitions)
        {
            return Err(RootRegistrationErrorV1::ResourceLimitExceeded);
        }
        self.partitions
            .entry(partition)
            .or_insert(PartitionResidencyV1::InTransit);
        if let Some(PartitionResidencyV1::Roots(roots)) = self.partitions.get(&partition) {
            for root in roots {
                if let Some(record) = self.roots.get_mut(root)
                    && record.load_state == RefetchableRootLoadStateV1::Unloaded
                {
                    record.load_state = RefetchableRootLoadStateV1::InTransit;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn observe_store_events_v1(
        &mut self,
        store: &ChunkStore,
        capability: &WebRemoteMcapRootCapabilityV1,
        events: &[ChunkStoreEvent],
    ) -> Result<(), RootRegistrationErrorV1> {
        if capability.store_identity_v1(store)? != self.store_identity
            || events.iter().any(|event| event.store_id != store.id())
        {
            return Err(RootRegistrationErrorV1::StoreCapability);
        }
        if events.is_empty() {
            return Ok(());
        }

        let max_affected_partitions = events
            .len()
            .checked_mul(2)
            .map(|count| count.min(self.partitions.len()))
            .ok_or(RootRegistrationErrorV1::ResourceLimitExceeded)?;
        let mut affected_partitions = HashSet::<DerivationPartitionKeyV1>::default();
        affected_partitions
            .try_reserve(max_affected_partitions)
            .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?;
        let affected_temporary_bytes = temporary_registration_bytes_with_hash_slack_v1(
            u64::try_from(affected_partitions.capacity())
                .map_err(|_error| RootRegistrationErrorV1::ResourceLimitExceeded)?,
            0,
            1,
        )?;
        if affected_temporary_bytes > self.temporary_reservation.bytes_v1() {
            return Err(RootRegistrationErrorV1::ResourceLimitExceeded);
        }
        for event in events {
            let mut observe_root = |root_id: ChunkId| {
                if let Some(root) = self.roots.get(&root_id) {
                    affected_partitions.insert(root.partition);
                }
            };
            if let Some(addition) = event.diff.to_addition() {
                observe_root(addition.chunk_before_processing.id());
                observe_root(addition.chunk_after_processing.id());
            } else if let Some(deletion) = event.diff.to_deletion() {
                observe_root(deletion.chunk.id());
            }
        }

        let partitions = &self.partitions;
        let roots = &mut self.roots;
        let loaded_coverage = &mut self.loaded_coverage;
        let mut coverage_ranges_dirty = false;
        for partition_key in affected_partitions.iter().copied() {
            let Some(residency) = partitions.get(&partition_key) else {
                continue;
            };
            let PartitionResidencyV1::Roots(root_ids) = residency else {
                continue;
            };
            let was_satisfied = root_ids.iter().all(|root_id| {
                roots
                    .get(root_id)
                    .is_some_and(|root| root.load_state == RefetchableRootLoadStateV1::FullyLoaded)
            });
            for root_id in root_ids {
                let root = roots
                    .get_mut(root_id)
                    .expect("registered partition roots have matching root descriptors");
                root.load_state =
                    match store.external_refetchable_root_existence_v1(root.external_descriptor) {
                        Ok(ExternalRefetchableRootExistenceV1::Resident) => {
                            RefetchableRootLoadStateV1::FullyLoaded
                        }
                        Ok(ExternalRefetchableRootExistenceV1::Unloaded) | Err(_) => {
                            RefetchableRootLoadStateV1::Unloaded
                        }
                    };
            }
            let is_satisfied = root_ids.iter().all(|root_id| {
                roots
                    .get(root_id)
                    .is_some_and(|root| root.load_state == RefetchableRootLoadStateV1::FullyLoaded)
            });
            if matches!(
                partition_key.kind_v1(),
                DerivationPartitionKindV1::TemporalChannelGroup(_)
            ) && was_satisfied != is_satisfied
            {
                let transition = if is_satisfied {
                    PartitionSatisfactionTransitionV1::BecameSatisfied
                } else {
                    PartitionSatisfactionTransitionV1::BecameUnsatisfied
                };
                coverage_ranges_dirty |= loaded_coverage
                    .apply_partition_transition_in_batch_v1(
                        partition_key.source_unit_ordinal_v1(),
                        transition,
                    )
                    .expect("registered temporal partitions match their coverage plan");
            }
        }
        loaded_coverage
            .finish_transition_batch_v1(coverage_ranges_dirty)
            .expect("registered temporal partitions keep coverage counters consistent");
        Ok(())
    }

    pub(crate) fn loaded_ranges_v1(&self) -> &[re_log_types::AbsoluteTimeRange] {
        self.loaded_coverage.loaded_ranges_v1()
    }

    pub(crate) const fn indexed_extent_v1(&self) -> CanonicalIndexedExtentV1 {
        self.loaded_coverage.indexed_extent_v1()
    }

    pub(crate) fn complete_indexed_coverage_v1(&self) -> CompleteIndexedCoverageV1 {
        self.loaded_coverage.complete_indexed_coverage_v1()
    }

    #[cfg(test)]
    fn coverage_rebuild_count_v1(&self) -> u64 {
        self.loaded_coverage.rebuild_count_v1()
    }

    pub(crate) fn is_partition_fully_resident_v1(
        &self,
        store: &ChunkStore,
        capability: &WebRemoteMcapRootCapabilityV1,
        partition: DerivationPartitionKeyV1,
    ) -> Result<bool, RootRegistrationErrorV1> {
        if capability.store_identity_v1(store)? != self.store_identity {
            return Err(RootRegistrationErrorV1::StoreCapability);
        }
        match self.partitions.get(&partition) {
            Some(PartitionResidencyV1::CompleteEmpty) => Ok(true),
            Some(PartitionResidencyV1::Roots(roots)) if !roots.is_empty() => {
                Ok(roots.iter().all(|id| {
                    let Some(root) = self.roots.get(id) else {
                        return false;
                    };
                    root.load_state == RefetchableRootLoadStateV1::FullyLoaded
                        && store
                            .external_refetchable_root_existence_v1(root.external_descriptor)
                            .is_ok_and(|state| {
                                state == ExternalRefetchableRootExistenceV1::Resident
                            })
                }))
            }
            _ => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use re_chunk::{Chunk, RowId};
    use re_chunk_store::{
        ChunkDirectLineage, ChunkStoreConfig, GarbageCollectionOptions, WebRemoteMcapStoreConfigV1,
    };
    use re_log_types::{StoreId, StoreKind, TimePoint, Timeline};
    use re_sdk_types::archetypes;

    use crate::remote_channel_group::StableDecoderGroupIdV1;
    use crate::remote_manifest::ManifestRootDescriptorIssuerV1;

    use super::*;

    fn temporal_chunk(root_chunk_id: ChunkId) -> Arc<Chunk> {
        Arc::new(
            Chunk::builder_with_id(root_chunk_id, "world/points")
                .with_archetype(
                    RowId::new(),
                    [(Timeline::log_tick(), 1)],
                    &archetypes::Points3D::new([[1.0, 2.0, 3.0]]),
                )
                .build()
                .unwrap(),
        )
    }

    fn static_chunk(root_chunk_id: ChunkId) -> Chunk {
        Chunk::builder_with_id(root_chunk_id, "world/static-points")
            .with_archetype(
                RowId::new(),
                TimePoint::STATIC,
                &archetypes::Points3D::new([[1.0, 2.0, 3.0]]),
            )
            .build()
            .unwrap()
    }

    fn store_and_capability() -> (ChunkStore, WebRemoteMcapRootCapabilityV1) {
        WebRemoteMcapStoreConfigV1::for_test_v1().into_store_v1(StoreId::random(
            StoreKind::Recording,
            "partition-registration-test",
        ))
    }

    fn temporal_partition(
        session: RemoteMcapSessionIdV1,
        source_ordinal: u32,
        bound: u32,
    ) -> (
        ManifestPartitionDescriptorV1,
        ManifestRootDescriptorIssuerV1,
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

    fn roots_registration(
        partition: ManifestPartitionDescriptorV1,
        issuer: &ManifestRootDescriptorIssuerV1,
        ordinals: impl IntoIterator<Item = u32>,
    ) -> PreparedPartitionRegistrationV1 {
        let roots = ordinals
            .into_iter()
            .map(|ordinal| PreparedRootRegistrationV1 {
                manifest_descriptor: issuer.descriptor_v1(ordinal).unwrap(),
                estimated_bytes: 1024,
                is_static: false,
            })
            .collect::<Vec<_>>();
        let root_ids = roots
            .iter()
            .map(|root| root.manifest_descriptor.root_chunk_id_v1())
            .collect::<BTreeSet<_>>();
        PreparedPartitionRegistrationV1 {
            partition,
            outcome: PreparedPartitionOutcomeV1::Roots {
                roots: roots.into_boxed_slice(),
                root_ids: root_ids.into_iter().collect(),
            },
        }
    }

    fn opening_static_partition(
        session: RemoteMcapSessionIdV1,
    ) -> (
        ManifestPartitionDescriptorV1,
        ManifestRootDescriptorIssuerV1,
    ) {
        ManifestPartitionDescriptorV1::for_registration_test_v1(
            session,
            u32::MAX,
            DerivationPartitionKindV1::OpeningStatic,
            1,
        )
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

    fn new_index_with_coverage(
        session: RemoteMcapSessionIdV1,
        limits: RemoteRegistrationCapacityV1,
        store: &ChunkStore,
        capability: &WebRemoteMcapRootCapabilityV1,
        intervals: &[Option<(i64, i64)>],
        expected_selected_group_count: u32,
    ) -> RefetchableRootIndexV1 {
        RefetchableRootIndexV1::new_v1(
            session,
            limits,
            store,
            capability,
            RemoteTemporalCoveragePlanV1::for_test_v1(intervals, expected_selected_group_count),
        )
        .unwrap()
    }

    fn generous_limits() -> RemoteRegistrationCapacityV1 {
        RemoteRegistrationCapacityV1 {
            max_registered_partitions: 8,
            max_complete_empty_entries: 8,
            max_root_descriptors: 8,
            max_external_origin_bytes: 8 * ExternalRefetchableRootOriginV1::ENCODED_BYTES_V1,
        }
    }

    #[test]
    fn commit_set_limit_failure_is_bitwise_atomic() {
        let session = RemoteMcapSessionIdV1::for_registration_test_v1(1);
        let (first_partition, first_issuer) = temporal_partition(session, 0, 1);
        let (second_partition, second_issuer) = temporal_partition(session, 1, 1);
        let registrations = vec![
            roots_registration(first_partition, &first_issuer, [0]),
            roots_registration(second_partition, &second_issuer, [0]),
        ];
        let (mut store, capability) = store_and_capability();
        let mut index = new_index(
            session,
            RemoteRegistrationCapacityV1 {
                max_registered_partitions: 2,
                max_complete_empty_entries: 0,
                max_root_descriptors: 1,
                max_external_origin_bytes: ExternalRefetchableRootOriginV1::ENCODED_BYTES_V1,
            },
            &store,
            &capability,
        );
        let before = index.snapshot_v1();

        assert_eq!(
            index.register_commit_set_v1(&mut store, &capability, registrations.clone()),
            Err(RootRegistrationErrorV1::ResourceLimitExceeded)
        );
        assert_eq!(index.snapshot_v1(), before);
        for registration in registrations {
            let PreparedPartitionOutcomeV1::Roots { roots, .. } = registration.outcome else {
                unreachable!();
            };
            for root in roots {
                assert!(
                    store
                        .direct_lineage(&root.manifest_descriptor.root_chunk_id_v1())
                        .is_none()
                );
            }
        }
    }

    #[test]
    fn identity_conflict_leaves_registry_unchanged() {
        let session = RemoteMcapSessionIdV1::for_registration_test_v1(2);
        let (partition, issuer) = temporal_partition(session, 0, 1);
        let registration = roots_registration(partition, &issuer, [0]);
        let PreparedPartitionOutcomeV1::Roots { roots, .. } = &registration.outcome else {
            unreachable!();
        };
        let root_chunk_id = roots[0].manifest_descriptor.root_chunk_id_v1();
        let (mut store, capability) = store_and_capability();
        store.insert_chunk(&temporal_chunk(root_chunk_id)).unwrap();
        let mut index = new_index(session, generous_limits(), &store, &capability);
        let before = index.snapshot_v1();

        assert_eq!(
            index.register_commit_set_v1(&mut store, &capability, vec![registration]),
            Err(RootRegistrationErrorV1::StoreCapability)
        );
        assert_eq!(index.snapshot_v1(), before);
        assert!(matches!(
            store.direct_lineage(&root_chunk_id),
            Some(ChunkDirectLineage::Volatile)
        ));
    }

    #[test]
    fn legal_full_registration_is_idempotent_and_event_driven() {
        let session = RemoteMcapSessionIdV1::for_registration_test_v1(3);
        let (roots_partition, issuer) = temporal_partition(session, 0, 1);
        let (empty_partition, _) = temporal_partition(session, 1, 1);
        let registrations = vec![
            roots_registration(roots_partition, &issuer, [0]),
            PreparedPartitionRegistrationV1::complete_empty_v1(empty_partition),
        ];
        let (mut store, capability) = store_and_capability();
        let mut index = new_index(
            session,
            RemoteRegistrationCapacityV1 {
                max_registered_partitions: 2,
                max_complete_empty_entries: 1,
                max_root_descriptors: 1,
                max_external_origin_bytes: ExternalRefetchableRootOriginV1::ENCODED_BYTES_V1,
            },
            &store,
            &capability,
        );

        index
            .register_commit_set_v1(&mut store, &capability, registrations.clone())
            .unwrap();
        let registered = index.snapshot_v1();
        index
            .register_commit_set_v1(&mut store, &capability, registrations.clone())
            .unwrap();
        assert_eq!(index.snapshot_v1(), registered);
        assert_eq!(
            index.counters_v1(),
            RemoteRegistrationCountersV1 {
                registered_partitions: 2,
                complete_empty_entries: 1,
                root_descriptors: 1,
                external_origin_bytes: ExternalRefetchableRootOriginV1::ENCODED_BYTES_V1,
            }
        );
        assert!(
            index
                .is_partition_fully_resident_v1(&store, &capability, empty_partition.key_v1())
                .unwrap()
        );

        let PreparedPartitionOutcomeV1::Roots { roots, .. } = &registrations[0].outcome else {
            unreachable!();
        };
        let root_chunk_id = roots[0].manifest_descriptor.root_chunk_id_v1();
        let external = index.roots[&root_chunk_id].external_descriptor;
        let permit = capability.issue_refetch_v1(&store, external).unwrap();
        let events = store
            .insert_external_refetchable_root_v1(permit, &temporal_chunk(root_chunk_id))
            .unwrap();
        assert!(
            !index
                .is_partition_fully_resident_v1(&store, &capability, roots_partition.key_v1())
                .unwrap()
        );
        index
            .observe_store_events_v1(&store, &capability, &events)
            .unwrap();
        assert!(
            index
                .is_partition_fully_resident_v1(&store, &capability, roots_partition.key_v1())
                .unwrap()
        );

        let (events, _) = store.gc(&GarbageCollectionOptions::gc_everything());
        assert!(
            !index
                .is_partition_fully_resident_v1(&store, &capability, roots_partition.key_v1())
                .unwrap()
        );
        index
            .observe_store_events_v1(&store, &capability, &events)
            .unwrap();
        assert_eq!(
            index.roots[&root_chunk_id].load_state,
            RefetchableRootLoadStateV1::Unloaded
        );
        assert!(
            !index
                .is_partition_fully_resident_v1(&store, &capability, roots_partition.key_v1())
                .unwrap()
        );
    }

    #[test]
    fn kind_and_manifest_mismatch_fail_before_mutation() {
        let session = RemoteMcapSessionIdV1::for_registration_test_v1(4);
        let (partition, issuer) = temporal_partition(session, 0, 1);
        let mut wrong_kind = roots_registration(partition, &issuer, [0]);
        let PreparedPartitionOutcomeV1::Roots { roots, .. } = &mut wrong_kind.outcome else {
            unreachable!();
        };
        roots[0].is_static = true;
        let (mut store, capability) = store_and_capability();
        let mut index = new_index(session, generous_limits(), &store, &capability);
        let before = index.snapshot_v1();

        assert_eq!(
            index.register_commit_set_v1(&mut store, &capability, vec![wrong_kind]),
            Err(RootRegistrationErrorV1::PartitionKindMismatch)
        );
        assert_eq!(index.snapshot_v1(), before);
        assert_eq!(store.num_physical_chunks(), 0);
    }

    #[test]
    fn idempotent_replay_rejects_changed_root_metadata() {
        let session = RemoteMcapSessionIdV1::for_registration_test_v1(5);
        let (partition, issuer) = temporal_partition(session, 0, 1);
        let registration = roots_registration(partition, &issuer, [0]);
        let (mut store, capability) = store_and_capability();
        let mut index = new_index(session, generous_limits(), &store, &capability);
        index
            .register_commit_set_v1(&mut store, &capability, vec![registration.clone()])
            .unwrap();
        let before = index.snapshot_v1();
        let mut changed = registration;
        let PreparedPartitionOutcomeV1::Roots { roots, .. } = &mut changed.outcome else {
            unreachable!();
        };
        roots[0].estimated_bytes += 1;

        assert_eq!(
            index.register_commit_set_v1(&mut store, &capability, vec![changed]),
            Err(RootRegistrationErrorV1::RegistrationConflict)
        );
        assert_eq!(index.snapshot_v1(), before);
    }

    #[test]
    fn root_id_membership_requires_sorted_unique_manifest_ids() {
        let session = RemoteMcapSessionIdV1::for_registration_test_v1(10);
        let (partition, issuer) = temporal_partition(session, 0, 3);
        let mut registration = roots_registration(partition, &issuer, 0..3);
        let PreparedPartitionOutcomeV1::Roots { root_ids, .. } = &mut registration.outcome else {
            unreachable!();
        };
        root_ids.swap(0, 2);
        let (mut store, capability) = store_and_capability();
        let mut index = new_index(session, generous_limits(), &store, &capability);
        let before = index.snapshot_v1();

        assert_eq!(
            index.register_commit_set_v1(&mut store, &capability, vec![registration]),
            Err(RootRegistrationErrorV1::RegistrationConflict)
        );
        assert_eq!(index.snapshot_v1(), before);
        assert_eq!(store.num_physical_chunks(), 0);
    }

    #[test]
    fn opening_static_registration_is_sealed_and_atomic_with_temporal() {
        let session = RemoteMcapSessionIdV1::for_registration_test_v1(6);
        let (static_partition, static_issuer) = opening_static_partition(session);
        let static_authority = ManifestOpeningStaticAuthorityV1::for_registration_test_v1(
            static_partition,
            &static_issuer,
        );
        let static_root = static_authority.issue_root_v1(0).unwrap();
        let static_handoff = RemoteOpeningStaticRootHandoffV1::seal_v1(
            &static_authority,
            0,
            static_chunk(static_root.root_chunk_id_v1()),
        )
        .unwrap();
        let static_registration = prepare_opening_static_terminal_registration_v1(
            &static_authority,
            &RemoteOpeningStaticTerminalV1::Complete(vec![static_handoff].into_boxed_slice()),
        )
        .unwrap();
        let (temporal_partition, temporal_issuer) = temporal_partition(session, 0, 1);
        let temporal_registration = roots_registration(temporal_partition, &temporal_issuer, [0]);
        let (mut store, capability) = store_and_capability();
        let mut index = new_index(session, generous_limits(), &store, &capability);

        index
            .register_commit_set_v1(
                &mut store,
                &capability,
                vec![static_registration, temporal_registration],
            )
            .unwrap();

        assert_eq!(index.counters_v1().registered_partitions, 2);
        assert_eq!(index.counters_v1().root_descriptors, 2);
        assert!(
            store
                .direct_lineage(&static_root.root_chunk_id_v1())
                .is_some()
        );

        let temporal_root = temporal_chunk(static_root.root_chunk_id_v1());
        assert!(matches!(
            RemoteOpeningStaticRootHandoffV1::seal_v1(
                &static_authority,
                0,
                temporal_root.as_ref().clone(),
            ),
            Err(RootRegistrationErrorV1::PartitionKindMismatch)
        ));
        assert_eq!(
            prepare_opening_static_terminal_registration_v1(
                &static_authority,
                &RemoteOpeningStaticTerminalV1::Failed,
            ),
            Err(RootRegistrationErrorV1::InvalidTerminalOutcome)
        );
    }

    #[test]
    fn opening_static_complete_empty_covers_zero_indexed_units() {
        let session = RemoteMcapSessionIdV1::for_registration_test_v1(7);
        let (partition, issuer) = opening_static_partition(session);
        let authority =
            ManifestOpeningStaticAuthorityV1::for_registration_test_v1(partition, &issuer);
        let registration = prepare_opening_static_terminal_registration_v1(
            &authority,
            &RemoteOpeningStaticTerminalV1::CompleteEmpty,
        )
        .unwrap();
        let (mut store, capability) = store_and_capability();
        let mut index = new_index(session, generous_limits(), &store, &capability);

        index
            .register_commit_set_v1(&mut store, &capability, vec![registration])
            .unwrap();

        assert!(
            index
                .is_partition_fully_resident_v1(&store, &capability, partition.key_v1())
                .unwrap()
        );
    }

    #[test]
    fn store_instance_and_stale_event_isolation_are_enforced() {
        let session = RemoteMcapSessionIdV1::for_registration_test_v1(8);
        let (partition, issuer) = temporal_partition(session, 0, 1);
        let registration = roots_registration(partition, &issuer, [0]);
        let PreparedPartitionOutcomeV1::Roots { roots, .. } = &registration.outcome else {
            unreachable!();
        };
        let root_chunk_id = roots[0].manifest_descriptor.root_chunk_id_v1();
        let shared_store_id = StoreId::random(StoreKind::Recording, "same-public-store-id");
        let (mut store, capability) =
            WebRemoteMcapStoreConfigV1::for_test_v1().into_store_v1(shared_store_id.clone());
        let (mut other_store, other_capability) =
            WebRemoteMcapStoreConfigV1::for_test_v1().into_store_v1(shared_store_id);
        let mut index = new_index(session, generous_limits(), &store, &capability);
        index
            .register_commit_set_v1(&mut store, &capability, vec![registration])
            .unwrap();

        assert_eq!(
            index.is_partition_fully_resident_v1(
                &other_store,
                &other_capability,
                partition.key_v1(),
            ),
            Err(RootRegistrationErrorV1::StoreCapability)
        );
        assert_eq!(
            index.observe_store_events_v1(&other_store, &other_capability, &[]),
            Err(RootRegistrationErrorV1::StoreCapability)
        );

        let mut foreign_events = Vec::new();
        for _ in 0..8 {
            foreign_events = other_store
                .insert_chunk(&temporal_chunk(ChunkId::new()))
                .unwrap();
        }
        index
            .observe_store_events_v1(&store, &capability, &foreign_events)
            .unwrap();
        assert_eq!(
            index.roots[&root_chunk_id].load_state,
            RefetchableRootLoadStateV1::Unloaded
        );

        let external = index.roots[&root_chunk_id].external_descriptor;
        let permit = capability.issue_refetch_v1(&store, external).unwrap();
        let addition = store
            .insert_external_refetchable_root_v1(permit, &temporal_chunk(root_chunk_id))
            .unwrap();
        index
            .observe_store_events_v1(&store, &capability, &addition)
            .unwrap();
        assert_eq!(
            index.roots[&root_chunk_id].load_state,
            RefetchableRootLoadStateV1::FullyLoaded
        );

        let (stale_deletion, _) = store.gc(&GarbageCollectionOptions::gc_everything());
        index
            .observe_store_events_v1(&store, &capability, &stale_deletion)
            .unwrap();
        let permit = capability.issue_refetch_v1(&store, external).unwrap();
        let reload = store
            .insert_external_refetchable_root_v1(permit, &temporal_chunk(root_chunk_id))
            .unwrap();
        index
            .observe_store_events_v1(&store, &capability, &reload)
            .unwrap();
        index
            .observe_store_events_v1(&store, &capability, &stale_deletion)
            .unwrap();
        assert_eq!(
            index.roots[&root_chunk_id].load_state,
            RefetchableRootLoadStateV1::FullyLoaded
        );
    }

    #[test]
    fn large_commit_set_uses_bounded_delta_registries() {
        const ROOTS: u32 = 512;
        let session = RemoteMcapSessionIdV1::for_registration_test_v1(9);
        let (partition, issuer) = temporal_partition(session, 0, ROOTS);
        let registration = roots_registration(partition, &issuer, 0..ROOTS);
        let limits = RemoteRegistrationCapacityV1 {
            max_registered_partitions: 1,
            max_complete_empty_entries: 0,
            max_root_descriptors: u64::from(ROOTS),
            max_external_origin_bytes: u64::from(ROOTS)
                * ExternalRefetchableRootOriginV1::ENCODED_BYTES_V1,
        };
        let (mut store, capability) = store_and_capability();
        let mut index = new_index(session, limits, &store, &capability);

        index
            .register_commit_set_v1(&mut store, &capability, vec![registration.clone()])
            .unwrap();
        let registered = index.snapshot_v1();
        index
            .register_commit_set_v1(&mut store, &capability, vec![registration])
            .unwrap();

        assert_eq!(index.snapshot_v1(), registered);
        assert_eq!(index.counters_v1().root_descriptors, u64::from(ROOTS));
        assert_eq!(index.roots.len(), usize::try_from(ROOTS).unwrap());
    }

    #[test]
    fn complete_empty_is_loaded_and_survives_gc() {
        let session = RemoteMcapSessionIdV1::for_registration_test_v1(11);
        let (partition, _) = temporal_partition(session, 0, 1);
        let (mut store, capability) = store_and_capability();
        let mut index = new_index_with_coverage(
            session,
            generous_limits(),
            &store,
            &capability,
            &[Some((10, 20))],
            1,
        );
        index
            .register_commit_set_v1(
                &mut store,
                &capability,
                vec![PreparedPartitionRegistrationV1::complete_empty_v1(
                    partition,
                )],
            )
            .unwrap();

        assert_eq!(
            index.indexed_extent_v1(),
            CanonicalIndexedExtentV1::Known(re_log_types::AbsoluteTimeRange::new(10, 20))
        );
        assert_eq!(
            index.loaded_ranges_v1(),
            &[re_log_types::AbsoluteTimeRange::new(10, 20)]
        );
        assert_eq!(
            index.complete_indexed_coverage_v1(),
            CompleteIndexedCoverageV1::Complete
        );

        let (events, _) = store.gc(&GarbageCollectionOptions::gc_everything());
        if !events.is_empty() {
            index
                .observe_store_events_v1(&store, &capability, &events)
                .unwrap();
        }
        assert_eq!(
            index.loaded_ranges_v1(),
            &[re_log_types::AbsoluteTimeRange::new(10, 20)]
        );

        let registered = index.snapshot_v1();
        index
            .register_commit_set_v1(
                &mut store,
                &capability,
                vec![PreparedPartitionRegistrationV1::complete_empty_v1(
                    partition,
                )],
            )
            .unwrap();
        assert_eq!(index.snapshot_v1(), registered);
    }

    #[test]
    fn gc_and_reload_update_cached_loaded_coverage() {
        let session = RemoteMcapSessionIdV1::for_registration_test_v1(12);
        let (partition, issuer) = temporal_partition(session, 0, 1);
        let registration = roots_registration(partition, &issuer, [0]);
        let PreparedPartitionOutcomeV1::Roots { roots, .. } = &registration.outcome else {
            unreachable!();
        };
        let root_chunk_id = roots[0].manifest_descriptor.root_chunk_id_v1();
        let (mut store, capability) = store_and_capability();
        let mut index = new_index_with_coverage(
            session,
            generous_limits(),
            &store,
            &capability,
            &[Some((0, 10))],
            1,
        );
        index
            .register_commit_set_v1(&mut store, &capability, vec![registration])
            .unwrap();
        assert!(index.loaded_ranges_v1().is_empty());

        let descriptor = index.roots[&root_chunk_id].external_descriptor;
        let permit = capability.issue_refetch_v1(&store, descriptor).unwrap();
        let addition = store
            .insert_external_refetchable_root_v1(permit, &temporal_chunk(root_chunk_id))
            .unwrap();
        index
            .observe_store_events_v1(&store, &capability, &addition)
            .unwrap();
        assert_eq!(
            index.loaded_ranges_v1(),
            &[re_log_types::AbsoluteTimeRange::new(0, 10)]
        );
        assert_eq!(
            index.complete_indexed_coverage_v1(),
            CompleteIndexedCoverageV1::Complete
        );

        let (deletion, _) = store.gc(&GarbageCollectionOptions::gc_everything());
        index
            .observe_store_events_v1(&store, &capability, &deletion)
            .unwrap();
        assert!(index.loaded_ranges_v1().is_empty());
        assert_eq!(
            index.complete_indexed_coverage_v1(),
            CompleteIndexedCoverageV1::Incomplete
        );

        let permit = capability.issue_refetch_v1(&store, descriptor).unwrap();
        let reload = store
            .insert_external_refetchable_root_v1(permit, &temporal_chunk(root_chunk_id))
            .unwrap();
        index
            .observe_store_events_v1(&store, &capability, &reload)
            .unwrap();
        assert_eq!(
            index.loaded_ranges_v1(),
            &[re_log_types::AbsoluteTimeRange::new(0, 10)]
        );

        let reloaded = index.snapshot_v1();
        index
            .observe_store_events_v1(&store, &capability, &reload)
            .unwrap();
        assert_eq!(index.snapshot_v1(), reloaded);
    }

    #[test]
    fn complete_empty_batch_rebuilds_loaded_ranges_once() {
        let session = RemoteMcapSessionIdV1::for_registration_test_v1(14);
        let (first, _) = temporal_partition(session, 0, 1);
        let (second, _) = temporal_partition(session, 1, 1);
        let (mut store, capability) = store_and_capability();
        let mut index = new_index_with_coverage(
            session,
            generous_limits(),
            &store,
            &capability,
            &[Some((0, 10)), Some((5, 15))],
            1,
        );
        let before = index.coverage_rebuild_count_v1();

        index
            .register_commit_set_v1(
                &mut store,
                &capability,
                vec![
                    PreparedPartitionRegistrationV1::complete_empty_v1(first),
                    PreparedPartitionRegistrationV1::complete_empty_v1(second),
                ],
            )
            .unwrap();

        assert_eq!(index.coverage_rebuild_count_v1(), before + 1);
        assert_eq!(
            index.loaded_ranges_v1(),
            &[re_log_types::AbsoluteTimeRange::new(0, 15)]
        );
    }

    #[test]
    fn root_event_batch_reconciles_only_affected_partitions_and_rebuilds_once() {
        let session = RemoteMcapSessionIdV1::for_registration_test_v1(15);
        let (first_partition, first_issuer) = temporal_partition(session, 0, 1);
        let (second_partition, second_issuer) = temporal_partition(session, 1, 1);
        let first_registration = roots_registration(first_partition, &first_issuer, [0]);
        let second_registration = roots_registration(second_partition, &second_issuer, [0]);
        let first_root = match &first_registration.outcome {
            PreparedPartitionOutcomeV1::Roots { roots, .. } => {
                roots[0].manifest_descriptor.root_chunk_id_v1()
            }
            PreparedPartitionOutcomeV1::CompleteEmpty => unreachable!(),
        };
        let second_root = match &second_registration.outcome {
            PreparedPartitionOutcomeV1::Roots { roots, .. } => {
                roots[0].manifest_descriptor.root_chunk_id_v1()
            }
            PreparedPartitionOutcomeV1::CompleteEmpty => unreachable!(),
        };
        let (mut store, capability) = store_and_capability();
        let mut index = new_index_with_coverage(
            session,
            generous_limits(),
            &store,
            &capability,
            &[Some((0, 10)), Some((5, 15))],
            1,
        );
        index
            .register_commit_set_v1(
                &mut store,
                &capability,
                vec![first_registration, second_registration],
            )
            .unwrap();

        let mut additions = Vec::new();
        for root_id in [first_root, second_root] {
            let descriptor = index.roots[&root_id].external_descriptor;
            let permit = capability.issue_refetch_v1(&store, descriptor).unwrap();
            additions.extend(
                store
                    .insert_external_refetchable_root_v1(permit, &temporal_chunk(root_id))
                    .unwrap(),
            );
        }
        let before_additions = index.coverage_rebuild_count_v1();
        index
            .observe_store_events_v1(&store, &capability, &additions)
            .unwrap();
        assert_eq!(index.coverage_rebuild_count_v1(), before_additions + 1);
        assert_eq!(
            index.loaded_ranges_v1(),
            &[re_log_types::AbsoluteTimeRange::new(0, 15)]
        );

        let loaded = index.snapshot_v1();
        index
            .observe_store_events_v1(&store, &capability, &additions)
            .unwrap();
        assert_eq!(index.snapshot_v1(), loaded);

        let irrelevant = ChunkStoreEvent {
            store_id: store.id().clone(),
            store_generation: store.generation(),
            event_id: u64::MAX,
            diff: re_chunk_store::ChunkStoreDiff::SchemaAddition(
                re_chunk_store::ChunkStoreDiffSchemaAddition {
                    new_columns: Vec::new(),
                },
            ),
        };
        index
            .observe_store_events_v1(&store, &capability, &[irrelevant])
            .unwrap();
        assert_eq!(index.snapshot_v1(), loaded);

        let (deletions, _) = store.gc(&GarbageCollectionOptions::gc_everything());
        let before_deletions = index.coverage_rebuild_count_v1();
        index
            .observe_store_events_v1(&store, &capability, &deletions)
            .unwrap();
        assert_eq!(index.coverage_rebuild_count_v1(), before_deletions + 1);
        assert!(index.loaded_ranges_v1().is_empty());
    }

    #[test]
    fn out_of_range_temporal_source_fails_before_registry_or_coverage_mutation() {
        let session = RemoteMcapSessionIdV1::for_registration_test_v1(13);
        let (partition, _) = temporal_partition(session, 1, 1);
        let (mut store, capability) = store_and_capability();
        let mut index = new_index_with_coverage(
            session,
            generous_limits(),
            &store,
            &capability,
            &[Some((0, 10))],
            1,
        );
        let before = index.snapshot_v1();

        assert_eq!(
            index.register_commit_set_v1(
                &mut store,
                &capability,
                vec![PreparedPartitionRegistrationV1::complete_empty_v1(
                    partition,
                )],
            ),
            Err(RootRegistrationErrorV1::InvalidManifestIdentity)
        );
        assert_eq!(index.snapshot_v1(), before);
    }

    #[test]
    fn ordinary_store_configuration_stays_unchanged() {
        let store = ChunkStore::new(
            StoreId::random(StoreKind::Recording, "ordinary-store"),
            ChunkStoreConfig::default(),
        );
        assert_eq!(store.config(), &ChunkStoreConfig::default());
    }
}
