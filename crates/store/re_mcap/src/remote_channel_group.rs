//! Immutable Channel groups and deterministic source-row identity for Web remote MCAP.
//!
//! This consumes the complete move-only MCAP-028 assignment owner. It cannot accept raw Summary
//! or schema bytes, and the result keeps all MCAP-026/027/028 reservations alive.

#![allow(dead_code)]

use std::alloc::Layout;
use std::cmp::Ordering;
use std::ops::Range;
use std::sync::Arc;

use parking_lot::Mutex;
use re_chunk::RowId;

use crate::remote_decoder_assignment::{
    BoundedRemoteDecoderAssignmentsV1, RemoteChannelDecoderAssignmentV1,
    RemoteChannelEligibilityV1, RemoteDecoderAssignmentErrorV1, RemoteDecoderOwnerV1,
};
use crate::remote_protobuf_descriptor::FrozenRemoteExecutableConfigV1;

const REMOTE_CHANNEL_GROUP_PROFILE_VERSION_V1: u16 = 1;
const MAX_REMOTE_CHANNEL_GROUPS_V1: usize = 256;
const ROW_ID_RECORD_LOCAL_OFFSET_LIMIT_V1: u64 = 1_u64 << 48;
const LOCKED_WASM_DLMALLOC_ALIGNMENT_V1: u64 = 8;
const LOCKED_WASM_DLMALLOC_CHUNK_OVERHEAD_V1: u64 = 4;
const LOCKED_WASM_DLMALLOC_MIN_CHUNK_V1: u64 = 16;
const LOCKED_WASM_DLMALLOC_TOP_FOOT_V1: u64 = 40;
const LOCKED_WASM_DLMALLOC_PAGE_V1: u64 = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteChannelGroupResourceLimitV1 {
    GroupCount,
    MembershipCount,
    RetainedBytes,
    ReservationCapacity,
    Arithmetic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnsupportedRemoteChannelGroupV1 {
    MembershipOverlap,
    MissingMembership,
    RegistrationContractUnavailable,
    RecordLocalOffset,
    DerivedOrdinal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteChannelGroupErrorV1 {
    ResourceLimitExceeded(RemoteChannelGroupResourceLimitV1),
    UnsupportedForRemote(UnsupportedRemoteChannelGroupV1),
    AssignmentFailed,
    FallibleAllocationFailed,
}

impl std::fmt::Display for RemoteChannelGroupErrorV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::ResourceLimitExceeded(_) => "remote MCAP Channel groups exceed a resource limit",
            Self::UnsupportedForRemote(UnsupportedRemoteChannelGroupV1::MembershipOverlap) => {
                "remote MCAP Channel membership overlaps"
            }
            Self::UnsupportedForRemote(UnsupportedRemoteChannelGroupV1::MissingMembership) => {
                "remote MCAP Channel membership is incomplete"
            }
            Self::UnsupportedForRemote(
                UnsupportedRemoteChannelGroupV1::RegistrationContractUnavailable,
            ) => "remote MCAP decoder registration contract is unavailable",
            Self::UnsupportedForRemote(UnsupportedRemoteChannelGroupV1::RecordLocalOffset) => {
                "remote MCAP source record offset is unsupported"
            }
            Self::UnsupportedForRemote(UnsupportedRemoteChannelGroupV1::DerivedOrdinal) => {
                "remote MCAP derived row ordinal is unsupported"
            }
            Self::AssignmentFailed => "remote MCAP decoder assignment became invalid",
            Self::FallibleAllocationFailed => "remote MCAP Channel-group allocation failed",
        })
    }
}

impl std::error::Error for RemoteChannelGroupErrorV1 {}

fn map_assignment_error(_error: RemoteDecoderAssignmentErrorV1) -> RemoteChannelGroupErrorV1 {
    RemoteChannelGroupErrorV1::AssignmentFailed
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct UnfrozenRemoteChannelGroupLimitsV1 {
    profile_version: u16,
    max_groups: u64,
    max_memberships: u64,
    max_retained_bytes: u64,
}

impl UnfrozenRemoteChannelGroupLimitsV1 {
    #[cfg(any(test, rerun_mcap_phase_a_proof_v1))]
    pub(crate) const fn generous_for_phase_a_measurement_v1() -> Self {
        Self {
            profile_version: REMOTE_CHANNEL_GROUP_PROFILE_VERSION_V1,
            max_groups: MAX_REMOTE_CHANNEL_GROUPS_V1 as u64,
            max_memberships: MAX_REMOTE_CHANNEL_GROUPS_V1 as u64,
            max_retained_bytes: 1_000_000,
        }
    }

    #[cfg(test)]
    const fn generous_for_test_v1() -> Self {
        Self::generous_for_phase_a_measurement_v1()
    }

    #[cfg(test)]
    pub(crate) const fn generous_for_assignment_test_v1() -> Self {
        Self::generous_for_phase_a_measurement_v1()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RemoteChannelGroupBudgetUsageV1 {
    active_builds: u64,
    working_bytes: u64,
    active_results: u64,
    retained_bytes: u64,
}

struct RemoteChannelGroupBudgetStateV1 {
    limits: UnfrozenRemoteChannelGroupLimitsV1,
    max_active_results: u64,
    max_aggregate_working_bytes: u64,
    max_aggregate_retained_bytes: u64,
    max_aggregate_combined_bytes: u64,
    usage: Mutex<RemoteChannelGroupBudgetUsageV1>,
}

pub(crate) struct RemoteChannelGroupBudgetV1 {
    state: Arc<RemoteChannelGroupBudgetStateV1>,
}

impl RemoteChannelGroupBudgetV1 {
    #[cfg(any(test, rerun_mcap_phase_a_proof_v1))]
    pub(crate) fn new_for_phase_a_measurement_v1(
        limits: UnfrozenRemoteChannelGroupLimitsV1,
        max_active_results: u64,
        max_aggregate_working_bytes: u64,
        max_aggregate_retained_bytes: u64,
        max_aggregate_combined_bytes: u64,
    ) -> Self {
        Self {
            state: Arc::new(RemoteChannelGroupBudgetStateV1 {
                limits,
                max_active_results,
                max_aggregate_working_bytes,
                max_aggregate_retained_bytes,
                max_aggregate_combined_bytes,
                usage: Mutex::new(RemoteChannelGroupBudgetUsageV1::default()),
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn new_for_test_v1(
        limits: UnfrozenRemoteChannelGroupLimitsV1,
        max_active_results: u64,
        max_aggregate_working_bytes: u64,
        max_aggregate_retained_bytes: u64,
        max_aggregate_combined_bytes: u64,
    ) -> Self {
        Self::new_for_phase_a_measurement_v1(
            limits,
            max_active_results,
            max_aggregate_working_bytes,
            max_aggregate_retained_bytes,
            max_aggregate_combined_bytes,
        )
    }

    #[cfg(test)]
    pub(crate) fn new_for_assignment_test_v1(
        limits: UnfrozenRemoteChannelGroupLimitsV1,
        max_active_results: u64,
        max_aggregate_retained_bytes: u64,
    ) -> Self {
        Self::new_for_test_v1(
            limits,
            max_active_results,
            max_aggregate_retained_bytes,
            max_aggregate_retained_bytes,
            max_aggregate_retained_bytes,
        )
    }

    pub(crate) fn is_idle_for_assignment_test_v1(&self) -> bool {
        *self.state.usage.lock() == RemoteChannelGroupBudgetUsageV1::default()
    }

    fn reserve(
        &self,
        working_bytes: u64,
        retained_bytes: u64,
    ) -> Result<RemoteChannelGroupWorkReservationV1, RemoteChannelGroupErrorV1> {
        if retained_bytes > self.state.limits.max_retained_bytes {
            return Err(RemoteChannelGroupErrorV1::ResourceLimitExceeded(
                RemoteChannelGroupResourceLimitV1::RetainedBytes,
            ));
        }
        let mut usage = self.state.usage.lock();
        let next = RemoteChannelGroupBudgetUsageV1 {
            active_builds: usage
                .active_builds
                .checked_add(1)
                .ok_or_else(arithmetic_error)?,
            working_bytes: usage
                .working_bytes
                .checked_add(working_bytes)
                .ok_or_else(arithmetic_error)?,
            active_results: usage
                .active_results
                .checked_add(1)
                .ok_or_else(arithmetic_error)?,
            retained_bytes: usage
                .retained_bytes
                .checked_add(retained_bytes)
                .ok_or_else(arithmetic_error)?,
        };
        if next.active_builds > self.state.max_active_results
            || next.working_bytes > self.state.max_aggregate_working_bytes
            || next.active_results > self.state.max_active_results
            || next.retained_bytes > self.state.max_aggregate_retained_bytes
            || next
                .working_bytes
                .checked_add(next.retained_bytes)
                .ok_or_else(arithmetic_error)?
                > self.state.max_aggregate_combined_bytes
        {
            return Err(RemoteChannelGroupErrorV1::ResourceLimitExceeded(
                RemoteChannelGroupResourceLimitV1::ReservationCapacity,
            ));
        }
        *usage = next;
        Ok(RemoteChannelGroupWorkReservationV1 {
            state: Some(Arc::clone(&self.state)),
            working_bytes,
            retained_bytes,
        })
    }
}

struct RemoteChannelGroupWorkReservationV1 {
    state: Option<Arc<RemoteChannelGroupBudgetStateV1>>,
    working_bytes: u64,
    retained_bytes: u64,
}

impl RemoteChannelGroupWorkReservationV1 {
    fn complete(mut self) -> RemoteChannelGroupReservationV1 {
        let state = self
            .state
            .take()
            .expect("Channel-group work reservation reused");
        let mut usage = state.usage.lock();
        usage.active_builds = usage
            .active_builds
            .checked_sub(1)
            .expect("Channel-group build accounting underflowed");
        usage.working_bytes = usage
            .working_bytes
            .checked_sub(self.working_bytes)
            .expect("Channel-group working-byte accounting underflowed");
        drop(usage);
        RemoteChannelGroupReservationV1 {
            state,
            retained_bytes: self.retained_bytes,
        }
    }
}

impl Drop for RemoteChannelGroupWorkReservationV1 {
    fn drop(&mut self) {
        let Some(state) = self.state.take() else {
            return;
        };
        let mut usage = state.usage.lock();
        usage.active_builds = usage
            .active_builds
            .checked_sub(1)
            .expect("Channel-group build accounting underflowed");
        usage.working_bytes = usage
            .working_bytes
            .checked_sub(self.working_bytes)
            .expect("Channel-group working-byte accounting underflowed");
        usage.active_results = usage
            .active_results
            .checked_sub(1)
            .expect("Channel-group result accounting underflowed");
        usage.retained_bytes = usage
            .retained_bytes
            .checked_sub(self.retained_bytes)
            .expect("Channel-group retained-byte accounting underflowed");
    }
}

struct RemoteChannelGroupReservationV1 {
    state: Arc<RemoteChannelGroupBudgetStateV1>,
    retained_bytes: u64,
}

impl Drop for RemoteChannelGroupReservationV1 {
    fn drop(&mut self) {
        let mut usage = self.state.usage.lock();
        usage.active_results = usage
            .active_results
            .checked_sub(1)
            .expect("Channel-group result accounting underflowed");
        usage.retained_bytes = usage
            .retained_bytes
            .checked_sub(self.retained_bytes)
            .expect("Channel-group retained-byte accounting underflowed");
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct StableDecoderGroupIdV1(u32);

impl StableDecoderGroupIdV1 {
    pub(crate) const fn as_u32(self) -> u32 {
        self.0
    }

    #[cfg(test)]
    pub(crate) const fn first_for_assignment_test_v1() -> Self {
        Self(0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct DecoderConfigHashV1([u8; 16]);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum RemoteDecoderIdentifierV1 {
    Protobuf,
    Raw,
    Ros2Reflection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct RemotePartitionRegistrationBoundsV1 {
    max_roots_per_partition: u32,
    max_external_origin_bytes_per_partition: u64,
}

impl RemotePartitionRegistrationBoundsV1 {
    pub(crate) const fn max_roots_per_partition_v1(self) -> u32 {
        self.max_roots_per_partition
    }

    pub(crate) const fn max_external_origin_bytes_per_partition_v1(self) -> u64 {
        self.max_external_origin_bytes_per_partition
    }
}

#[derive(Debug, PartialEq, Eq)]
struct CanonicalChannelGroupProposalV1 {
    decoder_identifier: RemoteDecoderIdentifierV1,
    executable_config: FrozenRemoteExecutableConfigV1,
    channels: Vec<u16>,
    registration_bounds: RemotePartitionRegistrationBoundsV1,
}

impl Ord for CanonicalChannelGroupProposalV1 {
    fn cmp(&self, other: &Self) -> Ordering {
        self.decoder_identifier
            .cmp(&other.decoder_identifier)
            .then_with(|| self.executable_config.cmp(&other.executable_config))
            .then_with(|| self.channels.cmp(&other.channels))
            .then_with(|| self.registration_bounds.cmp(&other.registration_bounds))
    }
}

impl PartialOrd for CanonicalChannelGroupProposalV1 {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ImmutableChannelGroupV1 {
    group_id: StableDecoderGroupIdV1,
    decoder_identifier: RemoteDecoderIdentifierV1,
    executable_config: FrozenRemoteExecutableConfigV1,
    membership: Range<usize>,
    registration_bounds: RemotePartitionRegistrationBoundsV1,
}

impl ImmutableChannelGroupV1 {
    pub(crate) const fn group_id(&self) -> StableDecoderGroupIdV1 {
        self.group_id
    }

    pub(crate) const fn decoder_config_hash(&self) -> DecoderConfigHashV1 {
        DecoderConfigHashV1(self.executable_config.canonical_digest_v1())
    }

    pub(crate) const fn registration_bounds_v1(&self) -> RemotePartitionRegistrationBoundsV1 {
        self.registration_bounds
    }

    pub(crate) const fn decoder_identifier_v1(&self) -> RemoteDecoderIdentifierV1 {
        self.decoder_identifier
    }

    pub(crate) const fn derived_chunk_profile_v1(
        &self,
    ) -> crate::remote_deterministic_insertion::RemoteDerivedChunkProfileV1 {
        self.executable_config.derived_chunk_profile_v1()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TemporalChannelAssignmentV1 {
    channel_id: u16,
    eligibility: RemoteChannelEligibilityV1,
    group_id: StableDecoderGroupIdV1,
}

impl TemporalChannelAssignmentV1 {
    pub(crate) const fn channel_id(self) -> u16 {
        self.channel_id
    }

    pub(crate) const fn group_id(self) -> StableDecoderGroupIdV1 {
        self.group_id
    }
}

/// Non-`Copy`, lifetime-bound resolution of one dense group ID.
pub(crate) struct ResolvedChannelGroupV1<'manifest> {
    group: &'manifest ImmutableChannelGroupV1,
    channels: &'manifest [u16],
}

impl ResolvedChannelGroupV1<'_> {
    pub(crate) fn descriptor(&self) -> &ImmutableChannelGroupV1 {
        self.group
    }

    pub(crate) fn channels(&self) -> &[u16] {
        self.channels
    }
}

pub(crate) struct ImmutableRemoteChannelGroupsV1<'definitions, 'input, 'source, 'wire> {
    _assignment_owner: BoundedRemoteDecoderAssignmentsV1<'definitions, 'input, 'source, 'wire>,
    groups: Vec<ImmutableChannelGroupV1>,
    memberships: Vec<u16>,
    assignments: Vec<TemporalChannelAssignmentV1>,
    _reservation: RemoteChannelGroupReservationV1,
}

impl ImmutableRemoteChannelGroupsV1<'_, '_, '_, '_> {
    pub(crate) fn groups_v1(&self) -> &[ImmutableChannelGroupV1] {
        &self.groups
    }
    pub(crate) fn ensure_current_for_validation_v1(&self) -> Result<(), RemoteChannelGroupErrorV1> {
        self._assignment_owner
            .ensure_current_for_manifest_v1()
            .map_err(map_assignment_error)
    }

    pub(crate) fn ensure_matches_physical_evidence_v1(
        &self,
        evidence: &crate::remote_chunk_scan::PhysicalChunkMessageEvidenceV1<'_>,
    ) -> Result<(), RemoteChannelGroupErrorV1> {
        let binding = self
            ._assignment_owner
            .physical_source_binding_for_manifest_v1()
            .map_err(map_assignment_error)?;
        evidence.ensure_matches_source_v1(binding);
        Ok(())
    }

    pub(crate) fn resolve_group(
        &self,
        group_id: StableDecoderGroupIdV1,
    ) -> Option<ResolvedChannelGroupV1<'_>> {
        let group = self.groups.get(usize::try_from(group_id.0).ok()?)?;
        (group.group_id == group_id).then(|| ResolvedChannelGroupV1 {
            group,
            channels: &self.memberships[group.membership.clone()],
        })
    }

    pub(crate) fn channels_for_group_v1(&self, group_id: StableDecoderGroupIdV1) -> Option<&[u16]> {
        let group = self.groups.get(usize::try_from(group_id.0).ok()?)?;
        (group.group_id == group_id).then(|| &self.memberships[group.membership.clone()])
    }

    pub(crate) fn assignments_v1(&self) -> &[TemporalChannelAssignmentV1] {
        &self.assignments
    }

    pub(crate) fn bind_executable_factory_v1(
        &self,
        channel_id: u16,
    ) -> Result<
        crate::remote_protobuf_descriptor::RemoteExecutableFactoryV1<'_, '_, '_, '_, '_>,
        crate::remote_protobuf_descriptor::RemoteExecutableAdapterErrorV1,
    > {
        self._assignment_owner
            .bind_executable_factory_v1(channel_id)
    }

    pub(crate) fn retained_bytes_for_validation_v1(&self) -> u64 {
        self._reservation.retained_bytes
    }
}

pub(crate) fn build_immutable_remote_channel_groups_v1<'definitions, 'input, 'source, 'wire>(
    assignment_owner: BoundedRemoteDecoderAssignmentsV1<'definitions, 'input, 'source, 'wire>,
    budget: &RemoteChannelGroupBudgetV1,
) -> Result<
    ImmutableRemoteChannelGroupsV1<'definitions, 'input, 'source, 'wire>,
    RemoteChannelGroupErrorV1,
> {
    let limits = budget.state.limits;
    if limits.profile_version != REMOTE_CHANNEL_GROUP_PROFILE_VERSION_V1
        || limits.max_groups > MAX_REMOTE_CHANNEL_GROUPS_V1 as u64
        || limits.max_memberships > MAX_REMOTE_CHANNEL_GROUPS_V1 as u64
    {
        return Err(RemoteChannelGroupErrorV1::ResourceLimitExceeded(
            RemoteChannelGroupResourceLimitV1::GroupCount,
        ));
    }
    assignment_owner
        .ensure_current_for_manifest_v1()
        .map_err(map_assignment_error)?;
    let rows = assignment_owner.assignments_for_manifest_v1();
    let row_count = u64::try_from(rows.len()).map_err(|_error| arithmetic_error())?;
    if row_count > limits.max_memberships {
        return Err(RemoteChannelGroupErrorV1::ResourceLimitExceeded(
            RemoteChannelGroupResourceLimitV1::MembershipCount,
        ));
    }
    let (working_bytes, retained_bytes) = exact_peak_bytes_v1(rows.len(), rows.len())?;
    let reservation = budget.reserve(working_bytes, retained_bytes)?;

    let mut proposals = Vec::new();
    proposals
        .try_reserve_exact(rows.len())
        .map_err(|_error| RemoteChannelGroupErrorV1::FallibleAllocationFailed)?;
    for row in rows {
        let executable_config = row.executable_config();
        let mut channels = Vec::new();
        channels
            .try_reserve_exact(1)
            .map_err(|_error| RemoteChannelGroupErrorV1::FallibleAllocationFailed)?;
        channels.push(row.channel_id());
        proposals.push(CanonicalChannelGroupProposalV1 {
            decoder_identifier: decoder_identifier(row.owner()),
            executable_config,
            channels,
            registration_bounds: registration_bounds(executable_config)?,
        });
    }

    let (groups, memberships, assignments) = canonicalize_groups_v1(proposals, rows, limits)?;
    assignment_owner
        .ensure_current_for_manifest_v1()
        .map_err(map_assignment_error)?;
    Ok(ImmutableRemoteChannelGroupsV1 {
        _assignment_owner: assignment_owner,
        groups,
        memberships,
        assignments,
        _reservation: reservation.complete(),
    })
}

#[expect(
    clippy::type_complexity,
    reason = "the three contiguous immutable arenas are intentionally returned together"
)]
fn canonicalize_groups_v1(
    mut proposals: Vec<CanonicalChannelGroupProposalV1>,
    source_assignments: &[RemoteChannelDecoderAssignmentV1],
    limits: UnfrozenRemoteChannelGroupLimitsV1,
) -> Result<
    (
        Vec<ImmutableChannelGroupV1>,
        Vec<u16>,
        Vec<TemporalChannelAssignmentV1>,
    ),
    RemoteChannelGroupErrorV1,
> {
    for proposal in &mut proposals {
        proposal.channels.sort_unstable();
        proposal.channels.dedup();
        if proposal.channels.is_empty() {
            return Err(RemoteChannelGroupErrorV1::UnsupportedForRemote(
                UnsupportedRemoteChannelGroupV1::MissingMembership,
            ));
        }
    }
    proposals.sort_unstable();
    let mut merged = Vec::<CanonicalChannelGroupProposalV1>::new();
    merged
        .try_reserve_exact(proposals.len())
        .map_err(|_error| RemoteChannelGroupErrorV1::FallibleAllocationFailed)?;
    for proposal in proposals {
        if let Some(existing) = merged.last_mut()
            && existing.decoder_identifier == proposal.decoder_identifier
            && existing.executable_config == proposal.executable_config
            && existing.registration_bounds == proposal.registration_bounds
        {
            existing
                .channels
                .try_reserve_exact(proposal.channels.len())
                .map_err(|_error| RemoteChannelGroupErrorV1::FallibleAllocationFailed)?;
            existing.channels.extend(proposal.channels);
        } else {
            merged.push(proposal);
        }
    }
    let mut proposals = merged;
    for proposal in &mut proposals {
        proposal.channels.sort_unstable();
        proposal.channels.dedup();
        let membership_count =
            u32::try_from(proposal.channels.len()).map_err(|_overflow| arithmetic_error())?;
        proposal.registration_bounds = RemotePartitionRegistrationBoundsV1 {
            max_roots_per_partition: proposal
                .registration_bounds
                .max_roots_per_partition
                .checked_mul(membership_count)
                .ok_or_else(arithmetic_error)?,
            max_external_origin_bytes_per_partition: proposal
                .registration_bounds
                .max_external_origin_bytes_per_partition
                .checked_mul(u64::from(membership_count))
                .ok_or_else(arithmetic_error)?,
        };
    }
    if proposals.len() as u64 > limits.max_groups {
        return Err(RemoteChannelGroupErrorV1::ResourceLimitExceeded(
            RemoteChannelGroupResourceLimitV1::GroupCount,
        ));
    }
    let total_memberships = proposals.iter().try_fold(0_u64, |total, proposal| {
        total.checked_add(proposal.channels.len() as u64)
    });
    let total_memberships = total_memberships.ok_or_else(arithmetic_error)?;
    if total_memberships > limits.max_memberships {
        return Err(RemoteChannelGroupErrorV1::ResourceLimitExceeded(
            RemoteChannelGroupResourceLimitV1::MembershipCount,
        ));
    }
    let membership_count =
        usize::try_from(total_memberships).map_err(|_error| arithmetic_error())?;
    let mut groups = Vec::new();
    groups
        .try_reserve_exact(proposals.len())
        .map_err(|_error| RemoteChannelGroupErrorV1::FallibleAllocationFailed)?;
    let mut memberships = Vec::new();
    memberships
        .try_reserve_exact(membership_count)
        .map_err(|_error| RemoteChannelGroupErrorV1::FallibleAllocationFailed)?;
    let mut projection = Vec::new();
    projection
        .try_reserve_exact(membership_count)
        .map_err(|_error| RemoteChannelGroupErrorV1::FallibleAllocationFailed)?;

    for (dense_index, proposal) in proposals.into_iter().enumerate() {
        let group_id = StableDecoderGroupIdV1(
            u32::try_from(dense_index).map_err(|_error| arithmetic_error())?,
        );
        let membership_start = memberships.len();
        for &channel_id in &proposal.channels {
            let source = source_assignments
                .binary_search_by_key(&channel_id, RemoteChannelDecoderAssignmentV1::channel_id)
                .ok()
                .map(|index| source_assignments[index].clone())
                .ok_or(RemoteChannelGroupErrorV1::UnsupportedForRemote(
                    UnsupportedRemoteChannelGroupV1::MissingMembership,
                ))?;
            if decoder_identifier(source.owner()) != proposal.decoder_identifier
                || source.executable_config() != proposal.executable_config
            {
                return Err(RemoteChannelGroupErrorV1::UnsupportedForRemote(
                    UnsupportedRemoteChannelGroupV1::MembershipOverlap,
                ));
            }
            memberships.push(channel_id);
            projection.push(TemporalChannelAssignmentV1 {
                channel_id,
                eligibility: source.eligibility(),
                group_id,
            });
        }
        let membership_end = memberships.len();
        groups.push(ImmutableChannelGroupV1 {
            group_id,
            decoder_identifier: proposal.decoder_identifier,
            executable_config: proposal.executable_config,
            membership: membership_start..membership_end,
            registration_bounds: proposal.registration_bounds,
        });
    }
    projection.sort_unstable_by_key(|assignment| assignment.channel_id);
    if projection
        .windows(2)
        .any(|pair| pair[0].channel_id == pair[1].channel_id)
    {
        return Err(RemoteChannelGroupErrorV1::UnsupportedForRemote(
            UnsupportedRemoteChannelGroupV1::MembershipOverlap,
        ));
    }
    if projection.len() != source_assignments.len()
        || projection
            .iter()
            .zip(source_assignments)
            .any(|(projected, source)| {
                projected.channel_id != source.channel_id()
                    || projected.eligibility != source.eligibility()
            })
    {
        return Err(RemoteChannelGroupErrorV1::UnsupportedForRemote(
            UnsupportedRemoteChannelGroupV1::MissingMembership,
        ));
    }
    Ok((groups, memberships, projection))
}

fn decoder_identifier(owner: RemoteDecoderOwnerV1) -> RemoteDecoderIdentifierV1 {
    match owner {
        RemoteDecoderOwnerV1::Ros2Reflection => RemoteDecoderIdentifierV1::Ros2Reflection,
        RemoteDecoderOwnerV1::Protobuf => RemoteDecoderIdentifierV1::Protobuf,
        RemoteDecoderOwnerV1::Raw => RemoteDecoderIdentifierV1::Raw,
    }
}

fn registration_bounds(
    config: FrozenRemoteExecutableConfigV1,
) -> Result<RemotePartitionRegistrationBoundsV1, RemoteChannelGroupErrorV1> {
    let (max_roots_per_partition, max_external_origin_bytes_per_partition) =
        config.registration_contract_v1();
    if max_roots_per_partition == 0 {
        return Err(RemoteChannelGroupErrorV1::UnsupportedForRemote(
            UnsupportedRemoteChannelGroupV1::RegistrationContractUnavailable,
        ));
    }
    Ok(RemotePartitionRegistrationBoundsV1 {
        max_roots_per_partition,
        max_external_origin_bytes_per_partition,
    })
}

fn exact_peak_bytes_v1(
    groups: usize,
    memberships: usize,
) -> Result<(u64, u64), RemoteChannelGroupErrorV1> {
    let footprint = |layout: Layout| -> Result<u64, RemoteChannelGroupErrorV1> {
        let requested = u64::try_from(layout.size()).map_err(|_overflow| arithmetic_error())?;
        if requested == 0 {
            return Ok(0);
        }
        let alignment = u64::try_from(layout.align()).map_err(|_overflow| arithmetic_error())?;
        let align_up = |value: u64, alignment: u64| -> Result<u64, RemoteChannelGroupErrorV1> {
            if alignment == 0 || !alignment.is_power_of_two() {
                return Err(arithmetic_error());
            }
            value
                .checked_add(alignment - 1)
                .map(|v| v & !(alignment - 1))
                .ok_or_else(arithmetic_error)
        };
        let request2size = |request: u64| {
            if request
                < LOCKED_WASM_DLMALLOC_MIN_CHUNK_V1 - LOCKED_WASM_DLMALLOC_CHUNK_OVERHEAD_V1 - 1
            {
                Ok(LOCKED_WASM_DLMALLOC_MIN_CHUNK_V1)
            } else {
                align_up(
                    request
                        .checked_add(LOCKED_WASM_DLMALLOC_CHUNK_OVERHEAD_V1)
                        .ok_or_else(arithmetic_error)?,
                    LOCKED_WASM_DLMALLOC_ALIGNMENT_V1,
                )
            }
        };
        let chunk = if alignment <= LOCKED_WASM_DLMALLOC_ALIGNMENT_V1 {
            request2size(requested)?
        } else {
            let payload = request2size(requested)?;
            request2size(
                payload
                    .checked_add(alignment)
                    .and_then(|v| {
                        v.checked_add(
                            LOCKED_WASM_DLMALLOC_MIN_CHUNK_V1
                                - LOCKED_WASM_DLMALLOC_CHUNK_OVERHEAD_V1,
                        )
                    })
                    .ok_or_else(arithmetic_error)?,
            )?
        };
        align_up(
            chunk
                .checked_add(LOCKED_WASM_DLMALLOC_TOP_FOOT_V1 + LOCKED_WASM_DLMALLOC_ALIGNMENT_V1)
                .ok_or_else(arithmetic_error)?,
            LOCKED_WASM_DLMALLOC_PAGE_V1,
        )
    };
    let array = |layout: Result<Layout, std::alloc::LayoutError>| -> Result<u64, RemoteChannelGroupErrorV1> {
        footprint(layout.map_err(|_layout| arithmetic_error())?)
    };
    let retained = array(Layout::array::<ImmutableChannelGroupV1>(groups))?
        .checked_add(array(Layout::array::<u16>(memberships))?)
        .ok_or_else(arithmetic_error)?
        .checked_add(array(Layout::array::<TemporalChannelAssignmentV1>(
            memberships,
        ))?)
        .ok_or_else(arithmetic_error)?;
    let singleton_channel = array(Layout::array::<u16>(1))?;
    let proposal_array = array(Layout::array::<CanonicalChannelGroupProposalV1>(groups))?;
    let working = proposal_array
        .checked_add(proposal_array)
        .ok_or_else(arithmetic_error)?
        .checked_add(
            singleton_channel
                .checked_mul(u64::try_from(groups).map_err(|_overflow| arithmetic_error())?)
                .ok_or_else(arithmetic_error)?,
        )
        .ok_or_else(arithmetic_error)?
        .checked_add(array(Layout::array::<u16>(memberships))?)
        .ok_or_else(arithmetic_error)?;
    Ok((working, retained))
}

#[cfg(test)]
pub(crate) fn peak_bytes_for_assignment_test_v1(
    rows: usize,
) -> Result<(u64, u64), RemoteChannelGroupErrorV1> {
    exact_peak_bytes_v1(rows, rows)
}

fn arithmetic_error() -> RemoteChannelGroupErrorV1 {
    RemoteChannelGroupErrorV1::ResourceLimitExceeded(RemoteChannelGroupResourceLimitV1::Arithmetic)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct CanonicalSourceOrderKeyV1 {
    top_level_record_absolute_offset: u64,
    record_local_offset: u64,
    derived_ordinal: u16,
}

impl CanonicalSourceOrderKeyV1 {
    pub(crate) fn new(
        top_level_record_absolute_offset: u64,
        record_local_offset: u64,
        derived_ordinal: u32,
    ) -> Result<Self, RemoteChannelGroupErrorV1> {
        if record_local_offset >= ROW_ID_RECORD_LOCAL_OFFSET_LIMIT_V1 {
            return Err(RemoteChannelGroupErrorV1::UnsupportedForRemote(
                UnsupportedRemoteChannelGroupV1::RecordLocalOffset,
            ));
        }
        let derived_ordinal = u16::try_from(derived_ordinal).map_err(|_error| {
            RemoteChannelGroupErrorV1::UnsupportedForRemote(
                UnsupportedRemoteChannelGroupV1::DerivedOrdinal,
            )
        })?;
        Ok(Self {
            top_level_record_absolute_offset,
            record_local_offset,
            derived_ordinal,
        })
    }

    pub(crate) fn stable_row_id(self) -> RowId {
        let low = (self.record_local_offset << 16) | self.derived_ordinal as u64;
        RowId::from_u128(((self.top_level_record_absolute_offset as u128) << 64) | low as u128)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(owner: RemoteDecoderOwnerV1, schema: u16) -> FrozenRemoteExecutableConfigV1 {
        let kind = match owner {
            RemoteDecoderOwnerV1::Ros2Reflection => 1,
            RemoteDecoderOwnerV1::Protobuf => 2,
            RemoteDecoderOwnerV1::Raw => 3,
        };
        let mut digest = [0; 16];
        digest[0] = kind;
        digest[1..3].copy_from_slice(&schema.to_le_bytes());
        let _ = digest;
        FrozenRemoteExecutableConfigV1::new_for_channel_group_test_v1(kind, schema)
    }

    fn source_assignment(
        channel_id: u16,
        owner: RemoteDecoderOwnerV1,
        schema: u16,
    ) -> RemoteChannelDecoderAssignmentV1 {
        RemoteChannelDecoderAssignmentV1::new_for_manifest_test_v1(
            channel_id,
            RemoteChannelEligibilityV1::Unknown,
            owner,
            config(owner, schema),
        )
    }

    fn proposal(
        owner: RemoteDecoderOwnerV1,
        schema: u16,
        channels: &[u16],
    ) -> CanonicalChannelGroupProposalV1 {
        CanonicalChannelGroupProposalV1 {
            decoder_identifier: decoder_identifier(owner),
            executable_config: config(owner, schema),
            channels: channels.to_vec(),
            registration_bounds: registration_bounds(config(owner, schema)).unwrap(),
        }
    }

    #[test]
    fn canonical_groups_merge_equal_decoder_config_and_bind_sorted_membership() {
        let sources = [
            source_assignment(2, RemoteDecoderOwnerV1::Raw, 0),
            source_assignment(7, RemoteDecoderOwnerV1::Raw, 0),
        ];
        let proposals = vec![
            proposal(RemoteDecoderOwnerV1::Raw, 0, &[7]),
            proposal(RemoteDecoderOwnerV1::Raw, 0, &[2]),
            proposal(RemoteDecoderOwnerV1::Raw, 0, &[2]),
        ];
        let (groups, memberships, projection) = canonicalize_groups_v1(
            proposals,
            &sources,
            UnfrozenRemoteChannelGroupLimitsV1::generous_for_test_v1(),
        )
        .unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].group_id.as_u32(), 0);
        assert_eq!(&memberships[groups[0].membership.clone()], &[2, 7]);
        let per_channel = registration_bounds(config(RemoteDecoderOwnerV1::Raw, 0)).unwrap();
        assert_eq!(
            groups[0].registration_bounds.max_roots_per_partition,
            per_channel.max_roots_per_partition * 2
        );
        assert_eq!(
            groups[0]
                .registration_bounds
                .max_external_origin_bytes_per_partition,
            per_channel.max_external_origin_bytes_per_partition * 2
        );
        assert_eq!(projection[0].channel_id(), 2);
        assert_eq!(projection[1].channel_id(), 7);
        assert_eq!(projection[0].group_id(), groups[0].group_id());
        assert_eq!(projection[1].group_id(), groups[0].group_id());
    }

    #[test]
    fn membership_overlap_and_config_rebinding_are_rejected() {
        let sources = [source_assignment(3, RemoteDecoderOwnerV1::Raw, 0)];
        let overlap = vec![
            proposal(RemoteDecoderOwnerV1::Raw, 0, &[3]),
            proposal(RemoteDecoderOwnerV1::Raw, 7, &[3]),
        ];
        assert_eq!(
            canonicalize_groups_v1(
                overlap,
                &sources,
                UnfrozenRemoteChannelGroupLimitsV1::generous_for_test_v1(),
            ),
            Err(RemoteChannelGroupErrorV1::UnsupportedForRemote(
                UnsupportedRemoteChannelGroupV1::MembershipOverlap
            ))
        );
    }

    #[test]
    fn reservation_is_exact_and_drop_restores_budget() {
        let limits = UnfrozenRemoteChannelGroupLimitsV1::generous_for_test_v1();
        let (working, exact) = exact_peak_bytes_v1(2, 2).unwrap();
        let combined = working.checked_add(exact).unwrap();
        let budget =
            RemoteChannelGroupBudgetV1::new_for_test_v1(limits, 1, working, exact, combined);
        let reservation = budget.reserve(working, exact).unwrap();
        assert_eq!(
            *budget.state.usage.lock(),
            RemoteChannelGroupBudgetUsageV1 {
                active_builds: 1,
                working_bytes: working,
                active_results: 1,
                retained_bytes: exact,
            }
        );
        let result_reservation = reservation.complete();
        assert_eq!(budget.state.usage.lock().working_bytes, 0);
        drop(result_reservation);
        assert_eq!(
            *budget.state.usage.lock(),
            RemoteChannelGroupBudgetUsageV1::default()
        );

        let too_small =
            RemoteChannelGroupBudgetV1::new_for_test_v1(limits, 1, working, exact, combined - 1);
        assert!(matches!(
            too_small.reserve(working, exact),
            Err(RemoteChannelGroupErrorV1::ResourceLimitExceeded(
                RemoteChannelGroupResourceLimitV1::ReservationCapacity
            ))
        ));
        assert_eq!(
            *too_small.state.usage.lock(),
            RemoteChannelGroupBudgetUsageV1::default()
        );
    }

    #[test]
    fn stable_row_id_preserves_canonical_source_order_and_bounds() {
        let first = CanonicalSourceOrderKeyV1::new(9, 12, 4).unwrap();
        let second = CanonicalSourceOrderKeyV1::new(9, 12, 5).unwrap();
        let later = CanonicalSourceOrderKeyV1::new(10, 0, 0).unwrap();
        assert!(first.stable_row_id() < second.stable_row_id());
        assert!(second.stable_row_id() < later.stable_row_id());
        assert_eq!(
            first.stable_row_id().as_tuid().as_u128(),
            (9_u128 << 64) | (12_u128 << 16) | 4
        );
        assert!(CanonicalSourceOrderKeyV1::new(0, 1_u64 << 48, 0).is_err());
        assert!(CanonicalSourceOrderKeyV1::new(0, 0, u16::MAX as u32 + 1).is_err());
    }
}
