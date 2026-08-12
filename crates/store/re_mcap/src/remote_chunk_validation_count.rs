//! Exact first-pass validation/count plans for Web remote MCAP.
//!
//! This stage consumes only MCAP-025 message evidence and the sealed MCAP-029 manifest owner.
//! It never constructs a parser, Arrow builder, or local decoder initializer.

#![allow(dead_code)]

use std::alloc::Layout;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::remote_channel_group::{ImmutableRemoteChannelGroupsV1, StableDecoderGroupIdV1};
use crate::remote_chunk_scan::{
    PhysicalChunkMessageEvidenceV1, PhysicalChunkValidationError, ValidatedPhysicalChunkExtent,
};

const REMOTE_VALIDATION_COUNT_PROFILE_VERSION_V1: u16 = 1;
const REMOTE_DECODER_RESOURCE_BOUND_VERSION_V1: u16 = 1;
const LOCKED_WASM_DLMALLOC_ALIGNMENT_V1: u64 = 8;
const LOCKED_WASM_DLMALLOC_CHUNK_OVERHEAD_V1: u64 = 4;
const LOCKED_WASM_DLMALLOC_MIN_CHUNK_V1: u64 = 16;
const LOCKED_WASM_DLMALLOC_TOP_FOOT_V1: u64 = 40;
const LOCKED_WASM_DLMALLOC_PAGE_V1: u64 = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteValidationCountResourceLimitV1 {
    ChannelCount,
    GroupCount,
    MessageCount,
    PayloadBytes,
    RetainedBytes,
    CombinedRetainedBytes,
    ReservationCapacity,
    Arithmetic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteValidationCountErrorV1 {
    ResourceLimitExceeded(RemoteValidationCountResourceLimitV1),
    PhysicalValidationFailed,
    ManifestMismatch,
    StaleSource,
    FallibleAllocationFailed,
}

impl std::fmt::Display for RemoteValidationCountErrorV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::ResourceLimitExceeded(_) => {
                "remote MCAP validation/count exceeds a resource limit"
            }
            Self::PhysicalValidationFailed => "remote MCAP physical validation failed",
            Self::ManifestMismatch => "remote MCAP validation/count manifest does not match",
            Self::StaleSource => "remote MCAP validation/count source is stale",
            Self::FallibleAllocationFailed => "remote MCAP validation/count allocation failed",
        })
    }
}

impl std::error::Error for RemoteValidationCountErrorV1 {}

fn map_physical_error(error: PhysicalChunkValidationError) -> RemoteValidationCountErrorV1 {
    match error {
        PhysicalChunkValidationError::SourceClosed
        | PhysicalChunkValidationError::StaleSourceGeneration
        | PhysicalChunkValidationError::StaleReadGeneration => {
            RemoteValidationCountErrorV1::StaleSource
        }
        _ => RemoteValidationCountErrorV1::PhysicalValidationFailed,
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct UnfrozenRemoteValidationCountLimitsV1 {
    profile_version: u16,
    max_channels: u64,
    max_groups: u64,
    max_messages: u64,
    max_payload_bytes: u64,
    max_retained_bytes: u64,
    max_combined_retained_bytes: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RemoteValidationCountBudgetUsageV1 {
    active_plans: u64,
    retained_bytes: u64,
    combined_retained_bytes: u64,
}

struct RemoteValidationCountBudgetStateV1 {
    limits: UnfrozenRemoteValidationCountLimitsV1,
    max_active_plans: u64,
    max_aggregate_retained_bytes: u64,
    max_aggregate_combined_retained_bytes: u64,
    usage: Mutex<RemoteValidationCountBudgetUsageV1>,
}

pub(crate) struct RemoteValidationCountBudgetV1 {
    state: Arc<RemoteValidationCountBudgetStateV1>,
}

impl RemoteValidationCountBudgetV1 {
    #[cfg(test)]
    pub(crate) fn new_for_test_v1(
        limits: UnfrozenRemoteValidationCountLimitsV1,
        max_active_plans: u64,
        max_aggregate_retained_bytes: u64,
        max_aggregate_combined_retained_bytes: u64,
    ) -> Self {
        Self {
            state: Arc::new(RemoteValidationCountBudgetStateV1 {
                limits,
                max_active_plans,
                max_aggregate_retained_bytes,
                max_aggregate_combined_retained_bytes,
                usage: Mutex::new(RemoteValidationCountBudgetUsageV1::default()),
            }),
        }
    }

    fn reserve(
        &self,
        retained_bytes: u64,
        combined_retained_bytes: u64,
    ) -> Result<RemoteValidationCountReservationV1, RemoteValidationCountErrorV1> {
        let limits = self.state.limits;
        if retained_bytes > limits.max_retained_bytes {
            return Err(RemoteValidationCountErrorV1::ResourceLimitExceeded(
                RemoteValidationCountResourceLimitV1::RetainedBytes,
            ));
        }
        if combined_retained_bytes > limits.max_combined_retained_bytes {
            return Err(RemoteValidationCountErrorV1::ResourceLimitExceeded(
                RemoteValidationCountResourceLimitV1::CombinedRetainedBytes,
            ));
        }
        let mut usage = self.state.usage.lock();
        let next = RemoteValidationCountBudgetUsageV1 {
            active_plans: checked_add(usage.active_plans, 1)?,
            retained_bytes: checked_add(usage.retained_bytes, retained_bytes)?,
            combined_retained_bytes: checked_add(
                usage.combined_retained_bytes,
                combined_retained_bytes,
            )?,
        };
        if next.active_plans > self.state.max_active_plans
            || next.retained_bytes > self.state.max_aggregate_retained_bytes
            || next.combined_retained_bytes > self.state.max_aggregate_combined_retained_bytes
        {
            return Err(RemoteValidationCountErrorV1::ResourceLimitExceeded(
                RemoteValidationCountResourceLimitV1::ReservationCapacity,
            ));
        }
        *usage = next;
        Ok(RemoteValidationCountReservationV1 {
            state: Arc::clone(&self.state),
            retained_bytes,
            combined_retained_bytes,
        })
    }

    #[cfg(test)]
    pub(crate) fn is_idle_for_test_v1(&self) -> bool {
        *self.state.usage.lock() == RemoteValidationCountBudgetUsageV1::default()
    }
}

#[cfg(test)]
impl UnfrozenRemoteValidationCountLimitsV1 {
    pub(crate) const fn generous_for_test_v1() -> Self {
        Self {
            profile_version: REMOTE_VALIDATION_COUNT_PROFILE_VERSION_V1,
            max_channels: 256,
            max_groups: 256,
            max_messages: 1_000_000,
            max_payload_bytes: 64 * 1024 * 1024,
            max_retained_bytes: 64 * 1024 * 1024,
            max_combined_retained_bytes: u64::MAX,
        }
    }

    pub(crate) const fn with_combined_limit_for_test_v1(mut self, limit: u64) -> Self {
        self.max_combined_retained_bytes = limit;
        self
    }
}

struct RemoteValidationCountReservationV1 {
    state: Arc<RemoteValidationCountBudgetStateV1>,
    retained_bytes: u64,
    combined_retained_bytes: u64,
}

impl Drop for RemoteValidationCountReservationV1 {
    fn drop(&mut self) {
        let mut usage = self.state.usage.lock();
        usage.active_plans = usage
            .active_plans
            .checked_sub(1)
            .expect("validation plan count underflowed");
        usage.retained_bytes = usage
            .retained_bytes
            .checked_sub(self.retained_bytes)
            .expect("validation retained bytes underflowed");
        usage.combined_retained_bytes = usage
            .combined_retained_bytes
            .checked_sub(self.combined_retained_bytes)
            .expect("validation combined bytes underflowed");
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ExactChannelDispatchCountV1 {
    channel_id: u16,
    message_count: u64,
    payload_bytes: u64,
    selected_group: StableDecoderGroupIdV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DecoderDispatchResourceBoundV1 {
    version: u16,
    group_id: StableDecoderGroupIdV1,
    exact_num_rows: u64,
    exact_payload_bytes: u64,
}

pub(crate) struct ValidatedChunkDispatchPlanV1<'definitions, 'input, 'source, 'wire> {
    // Keep the physical/decompressed owner before the manifest initializer owner.
    evidence: PhysicalChunkMessageEvidenceV1<'input>,
    manifest: ImmutableRemoteChannelGroupsV1<'definitions, 'input, 'source, 'wire>,
    channels: Box<[ExactChannelDispatchCountV1]>,
    selected_groups: Box<[StableDecoderGroupIdV1]>,
    resource_bounds: Box<[DecoderDispatchResourceBoundV1]>,
    extent: ValidatedPhysicalChunkExtent,
    _reservation: RemoteValidationCountReservationV1,
}

impl ValidatedChunkDispatchPlanV1<'_, '_, '_, '_> {
    pub(crate) fn ensure_current_v1(&self) -> Result<(), RemoteValidationCountErrorV1> {
        self.evidence
            .ensure_current_v1()
            .map_err(map_physical_error)
    }

    pub(crate) fn channels_v1(&self) -> &[ExactChannelDispatchCountV1] {
        &self.channels
    }

    #[cfg(test)]
    pub(crate) fn resource_bounds_for_test_v1(&self) -> &[DecoderDispatchResourceBoundV1] {
        &self.resource_bounds
    }
}

#[cfg(test)]
impl ExactChannelDispatchCountV1 {
    pub(crate) const fn values_for_test_v1(self) -> (u16, u64, u64, u32) {
        (
            self.channel_id,
            self.message_count,
            self.payload_bytes,
            self.selected_group.as_u32(),
        )
    }
}

#[cfg(test)]
impl DecoderDispatchResourceBoundV1 {
    pub(crate) const fn values_for_test_v1(self) -> (u16, u32, u64, u64) {
        (
            self.version,
            self.group_id.as_u32(),
            self.exact_num_rows,
            self.exact_payload_bytes,
        )
    }
}

#[cfg(test)]
pub(crate) fn exact_combined_retained_bytes_for_test_v1(
    evidence: &PhysicalChunkMessageEvidenceV1<'_>,
    manifest: &ImmutableRemoteChannelGroupsV1<'_, '_, '_, '_>,
) -> Result<u64, RemoteValidationCountErrorV1> {
    let channels = evidence
        .channel_census_v1()
        .map_err(map_physical_error)?
        .len();
    let mut groups = 0_usize;
    for (index, assignment) in manifest.assignments_v1().iter().enumerate() {
        if manifest.assignments_v1()[..index]
            .iter()
            .all(|candidate| candidate.group_id() != assignment.group_id())
        {
            groups = groups.checked_add(1).ok_or_else(arithmetic_error)?;
        }
    }
    checked_add(
        checked_add(
            exact_retained_bytes(channels, groups)?,
            evidence
                .retained_physical_bytes_v1()
                .map_err(map_physical_error)?,
        )?,
        manifest.retained_bytes_for_validation_v1(),
    )
}

pub(crate) fn validate_and_count_physical_chunk_v1<'definitions, 'input, 'source, 'wire>(
    evidence: PhysicalChunkMessageEvidenceV1<'input>,
    manifest: ImmutableRemoteChannelGroupsV1<'definitions, 'input, 'source, 'wire>,
    budget: &RemoteValidationCountBudgetV1,
) -> Result<
    ValidatedChunkDispatchPlanV1<'definitions, 'input, 'source, 'wire>,
    RemoteValidationCountErrorV1,
> {
    if budget.state.limits.profile_version != REMOTE_VALIDATION_COUNT_PROFILE_VERSION_V1 {
        return Err(RemoteValidationCountErrorV1::ResourceLimitExceeded(
            RemoteValidationCountResourceLimitV1::Arithmetic,
        ));
    }
    evidence.ensure_current_v1().map_err(map_physical_error)?;
    manifest
        .ensure_current_for_validation_v1()
        .map_err(|_error| RemoteValidationCountErrorV1::StaleSource)?;
    manifest
        .ensure_matches_physical_evidence_v1(&evidence)
        .map_err(|_error| RemoteValidationCountErrorV1::ManifestMismatch)?;
    let census = evidence.channel_census_v1().map_err(map_physical_error)?;
    let channel_len = census.len();
    let assignments = manifest.assignments_v1();
    if census.len() != assignments.len() {
        return Err(RemoteValidationCountErrorV1::ManifestMismatch);
    }
    let channel_count = u64::try_from(channel_len).map_err(|_error| arithmetic_error())?;
    if channel_count > budget.state.limits.max_channels {
        return Err(RemoteValidationCountErrorV1::ResourceLimitExceeded(
            RemoteValidationCountResourceLimitV1::ChannelCount,
        ));
    }

    let mut exact_messages = 0_u64;
    let mut exact_payload = 0_u64;
    let mut group_count = 0_usize;
    for (channel, assignment) in census.zip(assignments) {
        if channel.channel_id() != assignment.channel_id() {
            return Err(RemoteValidationCountErrorV1::ManifestMismatch);
        }
        exact_messages = checked_add(exact_messages, channel.message_count())?;
        exact_payload = checked_add(exact_payload, channel.payload_bytes())?;
        let first_group = assignments
            .iter()
            .take_while(|candidate| candidate.channel_id() != assignment.channel_id())
            .all(|candidate| candidate.group_id() != assignment.group_id());
        if first_group {
            group_count = group_count.checked_add(1).ok_or_else(arithmetic_error)?;
        }
    }
    if exact_messages > budget.state.limits.max_messages {
        return Err(RemoteValidationCountErrorV1::ResourceLimitExceeded(
            RemoteValidationCountResourceLimitV1::MessageCount,
        ));
    }
    if exact_payload > budget.state.limits.max_payload_bytes {
        return Err(RemoteValidationCountErrorV1::ResourceLimitExceeded(
            RemoteValidationCountResourceLimitV1::PayloadBytes,
        ));
    }
    let group_count_u64 = u64::try_from(group_count).map_err(|_error| arithmetic_error())?;
    if group_count_u64 > budget.state.limits.max_groups {
        return Err(RemoteValidationCountErrorV1::ResourceLimitExceeded(
            RemoteValidationCountResourceLimitV1::GroupCount,
        ));
    }
    let retained_bytes = exact_retained_bytes(channel_len, group_count)?;
    let combined_retained_bytes = checked_add(
        checked_add(
            retained_bytes,
            evidence
                .retained_physical_bytes_v1()
                .map_err(map_physical_error)?,
        )?,
        manifest.retained_bytes_for_validation_v1(),
    )?;
    let reservation = budget.reserve(retained_bytes, combined_retained_bytes)?;

    let mut channels = Vec::new();
    channels
        .try_reserve_exact(channel_len)
        .map_err(|_error| RemoteValidationCountErrorV1::FallibleAllocationFailed)?;
    let mut selected_groups = Vec::new();
    selected_groups
        .try_reserve_exact(group_count)
        .map_err(|_error| RemoteValidationCountErrorV1::FallibleAllocationFailed)?;
    let mut resource_bounds = Vec::new();
    resource_bounds
        .try_reserve_exact(group_count)
        .map_err(|_error| RemoteValidationCountErrorV1::FallibleAllocationFailed)?;
    for (channel, assignment) in evidence
        .channel_census_v1()
        .map_err(map_physical_error)?
        .zip(assignments)
    {
        channels.push(ExactChannelDispatchCountV1 {
            channel_id: channel.channel_id(),
            message_count: channel.message_count(),
            payload_bytes: channel.payload_bytes(),
            selected_group: assignment.group_id(),
        });
        if !selected_groups.contains(&assignment.group_id()) {
            selected_groups.push(assignment.group_id());
            resource_bounds.push(DecoderDispatchResourceBoundV1 {
                version: REMOTE_DECODER_RESOURCE_BOUND_VERSION_V1,
                group_id: assignment.group_id(),
                exact_num_rows: 0,
                exact_payload_bytes: 0,
            });
        }
        let bound_index = selected_groups
            .iter()
            .position(|group| *group == assignment.group_id())
            .expect("selected group owns one resource bound");
        let bound = resource_bounds
            .get_mut(bound_index)
            .expect("selected group owns one resource bound");
        bound.exact_num_rows = checked_add(bound.exact_num_rows, channel.message_count())?;
        bound.exact_payload_bytes =
            checked_add(bound.exact_payload_bytes, channel.payload_bytes())?;
    }
    evidence.ensure_current_v1().map_err(map_physical_error)?;
    manifest
        .ensure_current_for_validation_v1()
        .map_err(|_error| RemoteValidationCountErrorV1::StaleSource)?;
    let extent = evidence.extent_v1().map_err(map_physical_error)?;
    Ok(ValidatedChunkDispatchPlanV1 {
        evidence,
        manifest,
        channels: channels.into_boxed_slice(),
        selected_groups: selected_groups.into_boxed_slice(),
        resource_bounds: resource_bounds.into_boxed_slice(),
        extent,
        _reservation: reservation,
    })
}

fn exact_retained_bytes(
    channels: usize,
    groups: usize,
) -> Result<u64, RemoteValidationCountErrorV1> {
    let bytes = locked_footprint(
        Layout::array::<ExactChannelDispatchCountV1>(channels)
            .map_err(|_error| arithmetic_error())?,
    )?
    .checked_add(locked_footprint(
        Layout::array::<StableDecoderGroupIdV1>(groups).map_err(|_error| arithmetic_error())?,
    )?)
    .ok_or_else(arithmetic_error)?
    .checked_add(locked_footprint(
        Layout::array::<DecoderDispatchResourceBoundV1>(groups)
            .map_err(|_error| arithmetic_error())?,
    )?)
    .ok_or_else(arithmetic_error)?;
    Ok(bytes)
}

fn locked_footprint(layout: Layout) -> Result<u64, RemoteValidationCountErrorV1> {
    let requested = u64::try_from(layout.size()).map_err(|_error| arithmetic_error())?;
    if requested == 0 {
        return Ok(0);
    }
    let alignment = u64::try_from(layout.align()).map_err(|_error| arithmetic_error())?;
    if !alignment.is_power_of_two() {
        return Err(arithmetic_error());
    }
    let align_up = |value: u64, alignment: u64| {
        value
            .checked_add(alignment - 1)
            .map(|value| value & !(alignment - 1))
            .ok_or_else(arithmetic_error)
    };
    let request2size = |request: u64| {
        if request < LOCKED_WASM_DLMALLOC_MIN_CHUNK_V1 - LOCKED_WASM_DLMALLOC_CHUNK_OVERHEAD_V1 - 1
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
        request2size(requested)?
            .checked_add(alignment)
            .and_then(|value| {
                value.checked_add(
                    LOCKED_WASM_DLMALLOC_MIN_CHUNK_V1 - LOCKED_WASM_DLMALLOC_CHUNK_OVERHEAD_V1,
                )
            })
            .ok_or_else(arithmetic_error)
            .and_then(request2size)?
    };
    align_up(
        chunk
            .checked_add(LOCKED_WASM_DLMALLOC_TOP_FOOT_V1 + LOCKED_WASM_DLMALLOC_ALIGNMENT_V1)
            .ok_or_else(arithmetic_error)?,
        LOCKED_WASM_DLMALLOC_PAGE_V1,
    )
}

fn checked_add(left: u64, right: u64) -> Result<u64, RemoteValidationCountErrorV1> {
    left.checked_add(right).ok_or_else(arithmetic_error)
}

fn arithmetic_error() -> RemoteValidationCountErrorV1 {
    RemoteValidationCountErrorV1::ResourceLimitExceeded(
        RemoteValidationCountResourceLimitV1::Arithmetic,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_peak_budget_rejects_one_byte_short_and_restores_on_drop() {
        let limits = UnfrozenRemoteValidationCountLimitsV1::generous_for_test_v1();
        let retained = exact_retained_bytes(2, 1).unwrap();
        let budget = RemoteValidationCountBudgetV1::new_for_test_v1(limits, 1, retained, retained);
        let reservation = budget.reserve(retained, retained).unwrap();
        assert_eq!(budget.state.usage.lock().active_plans, 1);
        drop(reservation);
        assert_eq!(
            *budget.state.usage.lock(),
            RemoteValidationCountBudgetUsageV1::default()
        );
        let too_small =
            RemoteValidationCountBudgetV1::new_for_test_v1(limits, 1, retained, retained - 1);
        assert!(matches!(
            too_small.reserve(retained, retained),
            Err(RemoteValidationCountErrorV1::ResourceLimitExceeded(
                RemoteValidationCountResourceLimitV1::ReservationCapacity
            ))
        ));
    }

    #[test]
    fn production_surface_cannot_reenter_local_unbounded_initializers() {
        let source = include_str!("remote_chunk_validation_count.rs");
        let production = source
            .split("#[cfg(test)]\nmod tests")
            .next()
            .expect("production source precedes tests");
        for forbidden in [
            "MessageParser",
            "DescriptorPool::decode",
            "MessageSchema::parse",
            "DecoderRegistry::",
            "mcap::Summary",
            "MessageIndex",
            "Statistics",
        ] {
            assert!(
                !production.contains(forbidden),
                "validation/count must not use {forbidden}"
            );
        }
        assert!(production.contains("PhysicalChunkMessageEvidenceV1"));
        assert!(production.contains("ImmutableRemoteChannelGroupsV1"));
        assert!(production.contains("REMOTE_DECODER_RESOURCE_BOUND_VERSION_V1"));
    }
}
