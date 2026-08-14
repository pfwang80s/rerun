//! Deterministic decoder assignment for the Web remote-MCAP path.
//!
//! This module is crate-private and production-disarmed.
//! It accepts only the move-only combined ROS 2/protobuf initializer result, consumes its
//! source-bound recognition capability once, and retains that exact owner and every matching
//! reservation in the assignment result.

#![allow(dead_code)]

use std::alloc::Layout;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;

use crate::remote_chunk_scan::PhysicalChunkMessageEvidenceV1;
use crate::remote_protobuf_descriptor::{
    BoundedRemoteDecoderInitializersV1, FrozenRemoteExecutableConfigV1,
    RemoteProtobufInitializationErrorV1,
};
use crate::remote_ros2_reflection::{
    FrozenRemoteDecoderSlotV1, RemoteKnownNonEmptyEvidenceV1, RemoteMcapSelectedMembershipV1,
    RemoteRos2InitializationError,
};

const REMOTE_ASSIGNMENT_PROFILE_VERSION_V1: u16 = 1;
const MAX_INLINE_ASSIGNMENT_CHANNELS_V1: usize = 256;
const LOCKED_WASM_DLMALLOC_ALIGNMENT_V1: u64 = 8;
const LOCKED_WASM_DLMALLOC_CHUNK_OVERHEAD_V1: u64 = 4;
const LOCKED_WASM_DLMALLOC_MIN_CHUNK_V1: u64 = 16;
const LOCKED_WASM_DLMALLOC_TOP_FOOT_V1: u64 = 40;
const LOCKED_WASM_DLMALLOC_PAGE_V1: u64 = 64 * 1024;

// V1 deliberately certifies no semantic ROS 2 parser/config.
const EXACT_SAFE_SEMANTIC_ROS2_TABLE_V1: [RemoteSemanticParserIdentityV1; 0] = [];

#[cfg(target_arch = "wasm32")]
#[inline(never)]
#[unsafe(no_mangle)]
pub(crate) extern "C" fn rerun_remote_decoder_assignment_stage_v1() -> u32 {
    std::hint::black_box(0x2801_u32)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RemoteSemanticParserIdentityV1 {
    _sealed: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteChannelEligibilityV1 {
    KnownNonEmpty,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteDecoderOwnerV1 {
    Ros2Reflection,
    Protobuf,
    Raw,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnsupportedRemoteDecoderAssignmentV1 {
    SemanticParserNotAllowlisted,
    MissingOwner,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteDecoderAssignmentResourceLimitV1 {
    ChannelCount,
    CensusSteps,
    WorkingBytes,
    RetainedBytes,
    CombinedRetainedBytes,
    ReservationCapacity,
    Arithmetic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteDecoderAssignmentErrorV1 {
    UnsupportedForRemote(UnsupportedRemoteDecoderAssignmentV1),
    ResourceLimitExceeded(RemoteDecoderAssignmentResourceLimitV1),
    InitializerFailed,
    FallibleAllocationFailed,
    StaleSource,
}

impl std::fmt::Display for RemoteDecoderAssignmentErrorV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedForRemote(
                UnsupportedRemoteDecoderAssignmentV1::SemanticParserNotAllowlisted,
            ) => "remote MCAP semantic parser is not allowlisted",
            Self::UnsupportedForRemote(UnsupportedRemoteDecoderAssignmentV1::MissingOwner) => {
                "remote MCAP channel has no decoder owner"
            }
            Self::ResourceLimitExceeded(_) => {
                "remote MCAP decoder assignment exceeds a resource limit"
            }
            Self::InitializerFailed => "remote MCAP decoder initializer failed",
            Self::FallibleAllocationFailed => "remote MCAP decoder assignment allocation failed",
            Self::StaleSource => "remote MCAP decoder assignment source is stale",
        })
    }
}

impl std::error::Error for RemoteDecoderAssignmentErrorV1 {}

fn map_initializer_error(
    error: RemoteProtobufInitializationErrorV1,
) -> RemoteDecoderAssignmentErrorV1 {
    match error {
        RemoteProtobufInitializationErrorV1::StaleSource => {
            RemoteDecoderAssignmentErrorV1::StaleSource
        }
        RemoteProtobufInitializationErrorV1::InvalidRemoteSchema
        | RemoteProtobufInitializationErrorV1::UnsupportedForRemote(_)
        | RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(_)
        | RemoteProtobufInitializationErrorV1::FallibleAllocationFailed => {
            RemoteDecoderAssignmentErrorV1::InitializerFailed
        }
    }
}

fn map_presence_error(error: RemoteRos2InitializationError) -> RemoteDecoderAssignmentErrorV1 {
    match error {
        RemoteRos2InitializationError::StaleSource => RemoteDecoderAssignmentErrorV1::StaleSource,
        RemoteRos2InitializationError::InvalidRemoteSchema
        | RemoteRos2InitializationError::UnsupportedForRemote(_)
        | RemoteRos2InitializationError::ResourceLimitExceeded(_)
        | RemoteRos2InitializationError::FallibleAllocationFailed
        | RemoteRos2InitializationError::ConflictingTopicDecoderSignature
        | RemoteRos2InitializationError::SemanticConfigConflict => {
            RemoteDecoderAssignmentErrorV1::InitializerFailed
        }
    }
}

#[cold]
#[track_caller]
fn assignment_fatal_invariant(reason: &'static str) -> ! {
    panic!("Fatal remote decoder assignment invariant: {reason}")
}

#[cold]
#[track_caller]
fn assignment_fatal_control_plane(reason: &'static str) -> ! {
    panic!("Fatal remote decoder assignment control-plane mismatch: {reason}")
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct UnfrozenRemoteDecoderAssignmentLimitsV1 {
    profile_version: u16,
    max_channels: u64,
    max_census_steps: u64,
    max_working_bytes: u64,
    max_retained_bytes: u64,
    max_combined_retained_bytes: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RemoteDecoderAssignmentBudgetUsageV1 {
    active_assignments: u64,
    working_bytes: u64,
    retained_results: u64,
    retained_bytes: u64,
    combined_retained_bytes: u64,
}

#[derive(Clone, Copy, Debug)]
struct RemoteDecoderAssignmentBudgetCapacityV1 {
    max_active_assignments: u64,
    max_aggregate_working_bytes: u64,
    max_retained_results: u64,
    max_aggregate_retained_bytes: u64,
    max_aggregate_combined_retained_bytes: u64,
}

struct RemoteDecoderAssignmentBudgetStateV1 {
    limits: UnfrozenRemoteDecoderAssignmentLimitsV1,
    capacity: RemoteDecoderAssignmentBudgetCapacityV1,
    usage: Mutex<RemoteDecoderAssignmentBudgetUsageV1>,
    poisoned: AtomicBool,
}

/// Aggregate budget for assignment-local memory and all concurrently retained initializer owners.
pub(crate) struct RemoteDecoderAssignmentBudgetV1 {
    state: Arc<RemoteDecoderAssignmentBudgetStateV1>,
}

impl RemoteDecoderAssignmentBudgetV1 {
    #[cfg(any(test, re_mcap_locked_remote_wasm_allocator_v1))]
    pub(crate) fn new_disarmed_v1(
        limits: UnfrozenRemoteDecoderAssignmentLimitsV1,
        max_active_assignments: u64,
        max_aggregate_working_bytes: u64,
        max_retained_results: u64,
        max_aggregate_retained_bytes: u64,
        max_aggregate_combined_retained_bytes: u64,
    ) -> Self {
        if limits.profile_version != REMOTE_ASSIGNMENT_PROFILE_VERSION_V1
            || limits.max_channels > MAX_INLINE_ASSIGNMENT_CHANNELS_V1 as u64
        {
            assignment_fatal_control_plane("assignment profile exceeds its V1 structural shape");
        }
        Self {
            state: Arc::new(RemoteDecoderAssignmentBudgetStateV1 {
                limits,
                capacity: RemoteDecoderAssignmentBudgetCapacityV1 {
                    max_active_assignments,
                    max_aggregate_working_bytes,
                    max_retained_results,
                    max_aggregate_retained_bytes,
                    max_aggregate_combined_retained_bytes,
                },
                usage: Mutex::new(RemoteDecoderAssignmentBudgetUsageV1::default()),
                poisoned: AtomicBool::new(false),
            }),
        }
    }

    fn reserve(
        &self,
        working_bytes: u64,
        retained_bytes: u64,
        combined_retained_bytes: u64,
    ) -> Result<RemoteDecoderAssignmentWorkReservationV1, RemoteDecoderAssignmentErrorV1> {
        let state = &self.state;
        if state.poisoned.load(Ordering::Acquire) {
            assignment_fatal_control_plane("assignment budget is poisoned");
        }
        if working_bytes > state.limits.max_working_bytes {
            return Err(RemoteDecoderAssignmentErrorV1::ResourceLimitExceeded(
                RemoteDecoderAssignmentResourceLimitV1::WorkingBytes,
            ));
        }
        if retained_bytes > state.limits.max_retained_bytes {
            return Err(RemoteDecoderAssignmentErrorV1::ResourceLimitExceeded(
                RemoteDecoderAssignmentResourceLimitV1::RetainedBytes,
            ));
        }
        if combined_retained_bytes > state.limits.max_combined_retained_bytes {
            return Err(RemoteDecoderAssignmentErrorV1::ResourceLimitExceeded(
                RemoteDecoderAssignmentResourceLimitV1::CombinedRetainedBytes,
            ));
        }
        let mut usage = state.usage.lock();
        let next = RemoteDecoderAssignmentBudgetUsageV1 {
            active_assignments: checked_add(usage.active_assignments, 1)?,
            working_bytes: checked_add(usage.working_bytes, working_bytes)?,
            retained_results: checked_add(usage.retained_results, 1)?,
            retained_bytes: checked_add(usage.retained_bytes, retained_bytes)?,
            combined_retained_bytes: checked_add(
                usage.combined_retained_bytes,
                combined_retained_bytes,
            )?,
        };
        let capacity = state.capacity;
        if next.active_assignments > capacity.max_active_assignments
            || next.working_bytes > capacity.max_aggregate_working_bytes
            || next.retained_results > capacity.max_retained_results
            || next.retained_bytes > capacity.max_aggregate_retained_bytes
            || next.combined_retained_bytes > capacity.max_aggregate_combined_retained_bytes
        {
            return Err(RemoteDecoderAssignmentErrorV1::ResourceLimitExceeded(
                RemoteDecoderAssignmentResourceLimitV1::ReservationCapacity,
            ));
        }
        *usage = next;
        Ok(RemoteDecoderAssignmentWorkReservationV1 {
            state: Some(Arc::clone(state)),
            working_bytes,
            retained_bytes,
            combined_retained_bytes,
        })
    }
}

struct RemoteDecoderAssignmentWorkReservationV1 {
    state: Option<Arc<RemoteDecoderAssignmentBudgetStateV1>>,
    working_bytes: u64,
    retained_bytes: u64,
    combined_retained_bytes: u64,
}

impl RemoteDecoderAssignmentWorkReservationV1 {
    fn complete(mut self) -> RemoteDecoderAssignmentResultReservationV1 {
        let state = self
            .state
            .take()
            .unwrap_or_else(|| assignment_fatal_invariant("assignment reservation was reused"));
        let mut usage = state.usage.lock();
        usage.active_assignments = usage
            .active_assignments
            .checked_sub(1)
            .unwrap_or_else(|| poison_budget(&state, "active assignment count underflowed"));
        usage.working_bytes = usage
            .working_bytes
            .checked_sub(self.working_bytes)
            .unwrap_or_else(|| poison_budget(&state, "assignment working bytes underflowed"));
        drop(usage);
        RemoteDecoderAssignmentResultReservationV1 {
            state,
            retained_bytes: self.retained_bytes,
            combined_retained_bytes: self.combined_retained_bytes,
        }
    }
}

impl Drop for RemoteDecoderAssignmentWorkReservationV1 {
    fn drop(&mut self) {
        let Some(state) = self.state.take() else {
            return;
        };
        let mut usage = state.usage.lock();
        let Some(next) = checked_release_work_usage(
            *usage,
            self.working_bytes,
            self.retained_bytes,
            self.combined_retained_bytes,
        ) else {
            drop(usage);
            poison_budget(&state, "assignment work reservation underflowed");
        };
        *usage = next;
    }
}

struct RemoteDecoderAssignmentResultReservationV1 {
    state: Arc<RemoteDecoderAssignmentBudgetStateV1>,
    retained_bytes: u64,
    combined_retained_bytes: u64,
}

impl Drop for RemoteDecoderAssignmentResultReservationV1 {
    fn drop(&mut self) {
        let mut usage = self.state.usage.lock();
        let Some(retained_results) = usage.retained_results.checked_sub(1) else {
            drop(usage);
            poison_budget(&self.state, "assignment result count underflowed");
        };
        let Some(retained_bytes) = usage.retained_bytes.checked_sub(self.retained_bytes) else {
            drop(usage);
            poison_budget(&self.state, "assignment retained bytes underflowed");
        };
        let Some(combined_retained_bytes) = usage
            .combined_retained_bytes
            .checked_sub(self.combined_retained_bytes)
        else {
            drop(usage);
            poison_budget(&self.state, "combined initializer bytes underflowed");
        };
        usage.retained_results = retained_results;
        usage.retained_bytes = retained_bytes;
        usage.combined_retained_bytes = combined_retained_bytes;
    }
}

fn poison_budget(state: &RemoteDecoderAssignmentBudgetStateV1, reason: &'static str) -> ! {
    state.poisoned.store(true, Ordering::Release);
    assignment_fatal_control_plane(reason)
}

fn checked_release_work_usage(
    usage: RemoteDecoderAssignmentBudgetUsageV1,
    working_bytes: u64,
    retained_bytes: u64,
    combined_retained_bytes: u64,
) -> Option<RemoteDecoderAssignmentBudgetUsageV1> {
    Some(RemoteDecoderAssignmentBudgetUsageV1 {
        active_assignments: usage.active_assignments.checked_sub(1)?,
        working_bytes: usage.working_bytes.checked_sub(working_bytes)?,
        retained_results: usage.retained_results.checked_sub(1)?,
        retained_bytes: usage.retained_bytes.checked_sub(retained_bytes)?,
        combined_retained_bytes: usage
            .combined_retained_bytes
            .checked_sub(combined_retained_bytes)?,
    })
}

struct AssignmentStepOwnerV1 {
    remaining: u64,
}

impl AssignmentStepOwnerV1 {
    fn from_exact(exact: u64, limit: u64) -> Result<Self, RemoteDecoderAssignmentErrorV1> {
        if exact > limit {
            return Err(RemoteDecoderAssignmentErrorV1::ResourceLimitExceeded(
                RemoteDecoderAssignmentResourceLimitV1::CensusSteps,
            ));
        }
        Ok(Self { remaining: exact })
    }

    fn consume_exact(&mut self) {
        self.remaining = self.remaining.checked_sub(1).unwrap_or_else(|| {
            assignment_fatal_invariant("assignment exceeded its exact total step owner")
        });
    }

    fn ensure_exhausted(&self) {
        if self.remaining != 0 {
            assignment_fatal_invariant("assignment did not consume its exact total step owner");
        }
    }
}

/// One immutable `ChannelId -> owner` projection retained by the assignment result.
#[derive(Clone)]
pub(crate) struct RemoteChannelDecoderAssignmentV1 {
    channel_id: u16,
    canonical_channel_record_index: u32,
    eligibility: RemoteChannelEligibilityV1,
    owner: RemoteDecoderOwnerV1,
    executable_config: FrozenRemoteExecutableConfigV1,
    source_binding: crate::remote_chunk_scan::PhysicalChunkSourceBindingV1,
}

impl RemoteChannelDecoderAssignmentV1 {
    #[cfg(test)]
    pub(crate) fn new_for_manifest_test_v1(
        channel_id: u16,
        eligibility: RemoteChannelEligibilityV1,
        owner: RemoteDecoderOwnerV1,
        executable_config: FrozenRemoteExecutableConfigV1,
    ) -> Self {
        Self {
            channel_id,
            canonical_channel_record_index: 0,
            eligibility,
            owner,
            executable_config,
            source_binding: crate::remote_chunk_scan::PhysicalChunkSourceBindingV1::new_unscanned_for_assignment_test_v1(),
        }
    }

    pub(crate) const fn channel_id(&self) -> u16 {
        self.channel_id
    }

    pub(super) const fn canonical_channel_record_index_v1(&self) -> u32 {
        self.canonical_channel_record_index
    }

    pub(crate) const fn eligibility(&self) -> RemoteChannelEligibilityV1 {
        self.eligibility
    }

    pub(crate) const fn owner(&self) -> RemoteDecoderOwnerV1 {
        self.owner
    }

    pub(crate) const fn executable_config(&self) -> FrozenRemoteExecutableConfigV1 {
        self.executable_config
    }

    pub(crate) fn ensure_source_matches_v1(
        &self,
        binding: &crate::remote_chunk_scan::PhysicalChunkSourceBindingV1,
    ) {
        self.source_binding.ensure_matches_v1(binding);
    }
}

trait RemoteDecoderAssignmentAllocationGateV1 {
    fn before_allocation(&self, layout: Layout) -> Result<(), RemoteDecoderAssignmentErrorV1>;
}

struct SystemRemoteDecoderAssignmentAllocationGateV1;

impl RemoteDecoderAssignmentAllocationGateV1 for SystemRemoteDecoderAssignmentAllocationGateV1 {
    fn before_allocation(&self, _layout: Layout) -> Result<(), RemoteDecoderAssignmentErrorV1> {
        Ok(())
    }
}

struct FixedAssignmentArenaV1 {
    storage: Vec<RemoteChannelDecoderAssignmentV1>,
}

impl FixedAssignmentArenaV1 {
    fn try_new(
        capacity: usize,
        gate: &impl RemoteDecoderAssignmentAllocationGateV1,
    ) -> Result<Self, RemoteDecoderAssignmentErrorV1> {
        let layout = Layout::array::<RemoteChannelDecoderAssignmentV1>(capacity)
            .map_err(|_overflow| arithmetic_error())?;
        gate.before_allocation(layout)?;
        let mut storage = Vec::new();
        storage
            .try_reserve_exact(capacity)
            .map_err(|_allocation| RemoteDecoderAssignmentErrorV1::FallibleAllocationFailed)?;
        Ok(Self { storage })
    }

    fn push(&mut self, row: RemoteChannelDecoderAssignmentV1) {
        if self.storage.len() >= self.storage.capacity() {
            assignment_fatal_invariant("assignment census undercounted result rows");
        }
        self.storage.push(row);
    }
}

/// Reserved, move-only source/config/policy owner awaiting recognition and result fill.
pub(crate) struct PreparedRemoteDecoderEligibilityV1<'definitions, 'input, 'source, 'wire> {
    initializers: BoundedRemoteDecoderInitializersV1<'definitions, 'input, 'source, 'wire>,
    membership: RemoteMcapSelectedMembershipV1,
    evidence: Option<RemoteKnownNonEmptyEvidenceV1<'input>>,
    steps: AssignmentStepOwnerV1,
    reservation: RemoteDecoderAssignmentWorkReservationV1,
}

/// Move-only, source/config/policy-bound result consumed by MCAP-029.
pub(crate) struct BoundedRemoteDecoderAssignmentsV1<'definitions, 'input, 'source, 'wire> {
    initializers: BoundedRemoteDecoderInitializersV1<'definitions, 'input, 'source, 'wire>,
    _membership: RemoteMcapSelectedMembershipV1,
    _evidence: Option<RemoteKnownNonEmptyEvidenceV1<'input>>,
    assignments: FixedAssignmentArenaV1,
    _reservation: RemoteDecoderAssignmentResultReservationV1,
}

impl BoundedRemoteDecoderAssignmentsV1<'_, '_, '_, '_> {
    pub(crate) fn ensure_current_for_manifest_v1(
        &self,
    ) -> Result<(), RemoteDecoderAssignmentErrorV1> {
        self.initializers
            .ensure_current_for_assignment_v1()
            .map_err(map_initializer_error)
    }

    pub(crate) fn assignments_for_manifest_v1(&self) -> &[RemoteChannelDecoderAssignmentV1] {
        &self.assignments.storage
    }

    pub(crate) fn bind_executable_factory_v1(
        &self,
        channel_id: u16,
    ) -> Result<
        crate::remote_protobuf_descriptor::RemoteExecutableFactoryV1<'_, '_, '_, '_, '_>,
        crate::remote_protobuf_descriptor::RemoteExecutableAdapterErrorV1,
    > {
        let assignment = self
            .assignments
            .storage
            .binary_search_by_key(&channel_id, RemoteChannelDecoderAssignmentV1::channel_id)
            .ok()
            .and_then(|index| self.assignments.storage.get(index))
            .ok_or(
                crate::remote_protobuf_descriptor::RemoteExecutableAdapterErrorV1::ConfigMismatch,
            )?;
        self.initializers.bind_assignment_factory_v1(assignment)
    }

    pub(crate) fn physical_source_binding_for_manifest_v1(
        &self,
    ) -> Result<
        &crate::remote_chunk_scan::PhysicalChunkSourceBindingV1,
        RemoteDecoderAssignmentErrorV1,
    > {
        self.initializers
            .physical_source_binding_for_manifest_v1()
            .map_err(map_initializer_error)
    }

    pub(crate) fn policy_versions_for_manifest_v1(
        &self,
    ) -> Result<(u16, u16, u16), RemoteDecoderAssignmentErrorV1> {
        Ok(self
            .initializers
            .policy_descriptor_for_assignment_v1()
            .map_err(map_initializer_error)?
            .canonical_versions_for_manifest_v1())
    }

    #[cfg(test)]
    fn assignments_for_test(&self) -> &[RemoteChannelDecoderAssignmentV1] {
        &self.assignments.storage
    }

    #[cfg(test)]
    fn policy_identity_for_test_v1(&self) -> (u16, u16, u16, *const (), *const u8, usize) {
        self.initializers
            .policy_descriptor_for_assignment_v1()
            .unwrap_or_else(|_error| {
                assignment_fatal_invariant("result lost its frozen decoder policy")
            })
            .exact_identity_for_assignment_test_v1()
    }
}

/// Atomically reserves the complete assignment peak before projecting membership or recognizing.
pub(crate) fn prepare_remote_decoder_eligibility_v1<'definitions, 'input, 'source, 'wire>(
    initializers: BoundedRemoteDecoderInitializersV1<'definitions, 'input, 'source, 'wire>,
    budget: &RemoteDecoderAssignmentBudgetV1,
) -> Result<
    PreparedRemoteDecoderEligibilityV1<'definitions, 'input, 'source, 'wire>,
    RemoteDecoderAssignmentErrorV1,
> {
    prepare_remote_decoder_eligibility_with_optional_evidence_v1(initializers, None, budget)
}

pub(crate) fn prepare_remote_decoder_eligibility_with_evidence_v1<
    'definitions,
    'input,
    'source,
    'wire,
>(
    initializers: BoundedRemoteDecoderInitializersV1<'definitions, 'input, 'source, 'wire>,
    evidence: PhysicalChunkMessageEvidenceV1<'input>,
    budget: &RemoteDecoderAssignmentBudgetV1,
) -> Result<
    PreparedRemoteDecoderEligibilityV1<'definitions, 'input, 'source, 'wire>,
    RemoteDecoderAssignmentErrorV1,
> {
    prepare_remote_decoder_eligibility_with_optional_evidence_v1(
        initializers,
        Some(evidence),
        budget,
    )
}

fn prepare_remote_decoder_eligibility_with_optional_evidence_v1<
    'definitions,
    'input,
    'source,
    'wire,
>(
    mut initializers: BoundedRemoteDecoderInitializersV1<'definitions, 'input, 'source, 'wire>,
    physical_evidence: Option<PhysicalChunkMessageEvidenceV1<'input>>,
    budget: &RemoteDecoderAssignmentBudgetV1,
) -> Result<
    PreparedRemoteDecoderEligibilityV1<'definitions, 'input, 'source, 'wire>,
    RemoteDecoderAssignmentErrorV1,
> {
    initializers
        .ensure_current_for_assignment_v1()
        .map_err(map_initializer_error)?;
    let limits = budget.state.limits;
    let selected_count = initializers
        .selected_count_for_assignment_v1()
        .map_err(map_initializer_error)?;
    let selected_count_u64 =
        u64::try_from(selected_count).map_err(|_overflow| arithmetic_error())?;
    if selected_count_u64 > limits.max_channels {
        return Err(RemoteDecoderAssignmentErrorV1::ResourceLimitExceeded(
            RemoteDecoderAssignmentResourceLimitV1::ChannelCount,
        ));
    }
    let canonical_count = initializers
        .canonical_channel_count_for_assignment_v1()
        .map_err(map_initializer_error)?;
    let presence_steps = if physical_evidence.is_some() {
        canonical_count
    } else {
        0
    };
    let exact_steps = checked_add(
        checked_add(canonical_count, presence_steps)?,
        checked_mul(selected_count_u64, 3)?,
    )?;
    let mut steps = AssignmentStepOwnerV1::from_exact(exact_steps, limits.max_census_steps)?;
    let layout = Layout::array::<RemoteChannelDecoderAssignmentV1>(selected_count)
        .map_err(|_overflow| arithmetic_error())?;
    let arena_bytes = locked_wasm_allocation_footprint_v1(layout)?;
    let retained_bytes = checked_add(
        arena_bytes,
        u64::try_from(
            std::mem::size_of::<RemoteMcapSelectedMembershipV1>()
                .checked_add(std::mem::size_of::<
                    Option<RemoteKnownNonEmptyEvidenceV1<'input>>,
                >())
                .ok_or_else(arithmetic_error)?,
        )
        .map_err(|_overflow| arithmetic_error())?,
    )?;
    let working_bytes = u64::try_from(std::mem::size_of::<AssignmentStepOwnerV1>())
        .map_err(|_overflow| arithmetic_error())?;
    let initializer_bytes = initializers
        .combined_retained_bytes_for_assignment_v1()
        .map_err(map_initializer_error)?;
    let combined_retained_bytes = checked_add(initializer_bytes, retained_bytes)?;
    let reservation = budget.reserve(working_bytes, retained_bytes, combined_retained_bytes)?;
    let membership = initializers
        .project_selected_membership_for_assignment_v1()
        .map_err(map_initializer_error)?;
    if membership.selected_len() != selected_count
        || membership.canonical_channel_count() != canonical_count
    {
        assignment_fatal_invariant("semantic-config membership shape changed after reservation");
    }
    let evidence = physical_evidence
        .map(|physical| {
            RemoteKnownNonEmptyEvidenceV1::bind_physical_scan_v1(&membership, physical, || {
                steps.consume_exact();
            })
        })
        .transpose()
        .map_err(map_presence_error)?;
    if let Some(evidence) = &evidence {
        membership.ensure_matches_evidence_v1(evidence);
    }
    Ok(PreparedRemoteDecoderEligibilityV1 {
        initializers,
        membership,
        evidence,
        steps,
        reservation,
    })
}

/// Consumes exact filter eligibility and assigns only its canonical Summary Channels.
///
/// Included channels conservatively enter as `Unknown` in MCAP-028.
/// A future pre-assignment scanner-evidence capability may supply `KnownNonEmpty`; observations
/// after assignment remain telemetry and cannot rewrite this immutable result.
/// `Statistics` and `MessageIndex` records never exclude a channel or create a `KnownEmpty` state.
pub(crate) fn assign_remote_decoders_v1<'definitions, 'input, 'source, 'wire>(
    eligibility: PreparedRemoteDecoderEligibilityV1<'definitions, 'input, 'source, 'wire>,
) -> Result<
    BoundedRemoteDecoderAssignmentsV1<'definitions, 'input, 'source, 'wire>,
    RemoteDecoderAssignmentErrorV1,
> {
    assign_remote_decoders_with_gate_v1(eligibility, &SystemRemoteDecoderAssignmentAllocationGateV1)
}

/// Executes the exact production-disarmed assignment path for the release-Wasm verifier.
#[cfg(target_arch = "wasm32")]
pub(crate) fn run_remote_assignment_artifact_probe_v1<'definitions, 'input, 'source, 'wire>(
    initializers: BoundedRemoteDecoderInitializersV1<'definitions, 'input, 'source, 'wire>,
) -> u32 {
    let limits = UnfrozenRemoteDecoderAssignmentLimitsV1 {
        profile_version: REMOTE_ASSIGNMENT_PROFILE_VERSION_V1,
        max_channels: 2,
        max_census_steps: 64,
        max_working_bytes: u64::MAX,
        max_retained_bytes: u64::MAX,
        max_combined_retained_bytes: u64::MAX,
    };
    let budget = RemoteDecoderAssignmentBudgetV1::new_disarmed_v1(
        limits,
        1,
        u64::MAX,
        1,
        u64::MAX,
        u64::MAX,
    );
    let eligibility = match prepare_remote_decoder_eligibility_v1(initializers, &budget) {
        Ok(eligibility) => eligibility,
        Err(_error) => return 1,
    };
    let result = match assign_remote_decoders_v1(eligibility) {
        Ok(result) => result,
        Err(_error) => return 1,
    };
    let rows = result.assignments.storage.as_slice();
    if rows.len() != 2
        || rows[0].channel_id() != 1
        || rows[0].owner() != RemoteDecoderOwnerV1::Ros2Reflection
        || rows[1].channel_id() != 2
        || rows[1].owner() != RemoteDecoderOwnerV1::Protobuf
        || rows
            .iter()
            .any(|row| row.eligibility() != RemoteChannelEligibilityV1::Unknown)
    {
        return 2;
    }
    drop(result);
    if *budget.state.usage.lock() != RemoteDecoderAssignmentBudgetUsageV1::default() {
        return 3;
    }
    0
}

fn assign_remote_decoders_with_gate_v1<'definitions, 'input, 'source, 'wire>(
    eligibility: PreparedRemoteDecoderEligibilityV1<'definitions, 'input, 'source, 'wire>,
    gate: &impl RemoteDecoderAssignmentAllocationGateV1,
) -> Result<
    BoundedRemoteDecoderAssignmentsV1<'definitions, 'input, 'source, 'wire>,
    RemoteDecoderAssignmentErrorV1,
> {
    #[cfg(target_arch = "wasm32")]
    if rerun_remote_decoder_assignment_stage_v1() != 0x2801 {
        assignment_fatal_invariant("assignment artifact stage identity changed");
    }
    let PreparedRemoteDecoderEligibilityV1 {
        mut initializers,
        membership,
        evidence,
        mut steps,
        reservation,
    } = eligibility;
    initializers
        .ensure_current_for_assignment_v1()
        .map_err(map_initializer_error)?;
    let source_binding = initializers
        .physical_source_binding_for_manifest_v1()
        .map_err(map_initializer_error)?
        .clone();
    let mut assignments = FixedAssignmentArenaV1::try_new(membership.selected_len(), gate)?;
    let mut selected_cursor = 0_usize;
    {
        let mut recognition = initializers
            .take_recognition_v1()
            .map_err(map_initializer_error)?;
        while let Some(channel) = recognition
            .next_matching_channel(|channel_id| {
                steps.consume_exact();
                if selected_cursor < membership.selected_len()
                    && membership.channel_id_at(selected_cursor) == channel_id
                {
                    selected_cursor += 1;
                    true
                } else {
                    false
                }
            })
            .map_err(map_initializer_error)?
        {
            steps.consume_exact();
            let channel_id = channel.channel_id().map_err(map_initializer_error)?;
            let selected_index = selected_cursor.checked_sub(1).unwrap_or_else(|| {
                assignment_fatal_invariant("recognizer yielded a filtered Channel")
            });
            if membership.channel_id_at(selected_index) != channel_id {
                assignment_fatal_invariant("linear membership merge changed Channel identity");
            }
            let channel_eligibility = if evidence
                .as_ref()
                .is_some_and(|evidence| evidence.is_known_non_empty(selected_index))
            {
                RemoteChannelEligibilityV1::KnownNonEmpty
            } else {
                RemoteChannelEligibilityV1::Unknown
            };
            let semantic = channel
                .recognized_by_builtin_semantic()
                .map_err(map_initializer_error)?;
            if semantic && EXACT_SAFE_SEMANTIC_ROS2_TABLE_V1.is_empty() {
                return Err(RemoteDecoderAssignmentErrorV1::UnsupportedForRemote(
                    UnsupportedRemoteDecoderAssignmentV1::SemanticParserNotAllowlisted,
                ));
            }
            let reflection = channel
                .recognized_by_ros2_reflection()
                .map_err(map_initializer_error)?;
            let protobuf = channel
                .recognized_by_protobuf()
                .map_err(map_initializer_error)?;
            steps.consume_exact();
            let owner = channel
                .resolve_bound_owner_v1([semantic, reflection, protobuf])
                .map_err(map_initializer_error)?
                .ok_or(RemoteDecoderAssignmentErrorV1::UnsupportedForRemote(
                    UnsupportedRemoteDecoderAssignmentV1::MissingOwner,
                ))?;
            let owner = match owner {
                FrozenRemoteDecoderSlotV1::SemanticRos2 => {
                    return Err(RemoteDecoderAssignmentErrorV1::UnsupportedForRemote(
                        UnsupportedRemoteDecoderAssignmentV1::SemanticParserNotAllowlisted,
                    ));
                }
                FrozenRemoteDecoderSlotV1::Ros2Reflection => RemoteDecoderOwnerV1::Ros2Reflection,
                FrozenRemoteDecoderSlotV1::Protobuf => RemoteDecoderOwnerV1::Protobuf,
                FrozenRemoteDecoderSlotV1::Raw => RemoteDecoderOwnerV1::Raw,
            };
            steps.consume_exact();
            assignments.push(RemoteChannelDecoderAssignmentV1 {
                channel_id,
                canonical_channel_record_index: channel
                    .canonical_channel_record_index_v1()
                    .map_err(map_initializer_error)?,
                eligibility: channel_eligibility,
                owner,
                executable_config: channel
                    .frozen_executable_config_v1(owner)
                    .map_err(map_initializer_error)?,
                source_binding: source_binding.clone(),
            });
        }
    }
    if selected_cursor != membership.selected_len()
        || assignments.storage.len() != membership.selected_len()
    {
        assignment_fatal_invariant("eligible Channel census and recognition lengths differ");
    }
    steps.ensure_exhausted();

    initializers
        .ensure_current_for_assignment_v1()
        .map_err(map_initializer_error)?;
    Ok(BoundedRemoteDecoderAssignmentsV1 {
        initializers,
        _membership: membership,
        _evidence: evidence,
        assignments,
        _reservation: reservation.complete(),
    })
}

/// Runs the complete production-disarmed ROS scalar path from a finalized MCAP-025A owner.
///
/// This helper exists only to prove the cross-stage capability wiring in host tests. It accepts
/// the same sealed projections consumed by production stages and does not mint replacement
/// physical, partition, or root authority.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteDispatchTestMutationV1 {
    None,
    InvalidateSource,
    CrossWireFirstDescriptor,
}

#[cfg(test)]
pub(crate) fn dispatch_group_from_finalized_source_for_test_v1<'input>(
    source: crate::remote_physical_resolution::ResolvedRemotePhysicalSourceRefV1<'_, 'input>,
    physical: &crate::remote_chunk_scan::PhysicalChunkDefinitionsCapabilityV1<'_, 'input>,
    evidence: crate::remote_chunk_scan::PhysicalChunkMessageEvidenceV1<'input>,
    channel_id: u16,
    mutation: RemoteDispatchTestMutationV1,
) -> crate::remote_chunk_dispatch::RemoteChunkTerminalV1 {
    use crate::remote_protobuf_descriptor::{
        RemoteExecutableAdapterBudgetRootV1, RemoteExecutableAdapterBudgetV1,
        RemoteExecutableAdapterLimitsV1, RemoteProtobufInitializationBudgetV1,
        UnfrozenRemoteProtobufLimitsV1, initialize_remote_protobuf_v1,
        prepare_remote_protobuf_census_v1,
    };
    use crate::remote_protobuf_projection_boundary::RemoteProtobufProfileScopeV1;
    use crate::remote_ros2_reflection::{
        RemoteDecoderPolicyWireV1, RemoteDefinitionsCapability, RemoteDefinitionsSourceState,
        RemoteRos2InitializationBudget, RemoteRos2ProfileScopeV1, RemoteViewerScopeState,
        begin_remote_ros2_admission_v1, freeze_remote_decoder_policy_v1,
        materialize_remote_ros2_definitions_v1, preflight_remote_decoder_topic_signatures_v1,
        prepare_remote_ros2_census_v1,
    };

    let viewer = Box::new(RemoteViewerScopeState::new_for_protobuf_test_v1(1));
    let ros_profile = Box::new(RemoteRos2ProfileScopeV1::new_for_protobuf_test_v1(1));
    let protobuf_profile = Box::new(RemoteProtobufProfileScopeV1::new_disarmed_v1());
    let source_state = RemoteDefinitionsSourceState::new_for_protobuf_test_v1(
        physical.source_binding_v1().clone(),
        &viewer,
        &protobuf_profile,
    );
    let wire = RemoteDecoderPolicyWireV1::canonical_for_protobuf_test_v1();
    let ros_budget = RemoteRos2InitializationBudget::new_for_protobuf_test_v1(
        &source_state,
        &viewer,
        &ros_profile,
        &wire,
    );
    let policy = freeze_remote_decoder_policy_v1(&wire).unwrap();
    let definitions = RemoteDefinitionsCapability::new_for_protobuf_test_v1(
        physical,
        &source_state,
        &policy,
        &ros_budget,
    )
    .unwrap();
    let ros_owner = begin_remote_ros2_admission_v1(definitions, policy, &ros_budget).unwrap();
    let signatures = preflight_remote_decoder_topic_signatures_v1(ros_owner).unwrap();
    let ros_census = prepare_remote_ros2_census_v1(signatures).unwrap();
    let transition = materialize_remote_ros2_definitions_v1(ros_census)
        .unwrap()
        .into_protobuf_transition_v1()
        .unwrap();
    let protobuf_budget = RemoteProtobufInitializationBudgetV1::new_disarmed_v1(
        &viewer,
        &protobuf_profile,
        UnfrozenRemoteProtobufLimitsV1::generous_for_assignment_test_v1(),
        1,
        u64::MAX,
        1,
        u64::MAX,
    );
    let protobuf_census = prepare_remote_protobuf_census_v1(transition, &protobuf_budget).unwrap();
    let initializers = initialize_remote_protobuf_v1(protobuf_census).unwrap();

    let assignment_budget = RemoteDecoderAssignmentBudgetV1::new_disarmed_v1(
        UnfrozenRemoteDecoderAssignmentLimitsV1 {
            profile_version: REMOTE_ASSIGNMENT_PROFILE_VERSION_V1,
            max_channels: MAX_INLINE_ASSIGNMENT_CHANNELS_V1 as u64,
            max_census_steps: 1_000_000,
            max_working_bytes: u64::MAX,
            max_retained_bytes: u64::MAX,
            max_combined_retained_bytes: u64::MAX,
        },
        1,
        u64::MAX,
        1,
        u64::MAX,
        u64::MAX,
    );
    let eligibility =
        prepare_remote_decoder_eligibility_v1(initializers, &assignment_budget).unwrap();
    let assignments = assign_remote_decoders_v1(eligibility).unwrap();
    let group_budget = crate::remote_channel_group::RemoteChannelGroupBudgetV1::new_for_assignment_test_v1(
        crate::remote_channel_group::UnfrozenRemoteChannelGroupLimitsV1::generous_for_assignment_test_v1(),
        1,
        u64::MAX,
    );
    let groups = crate::remote_channel_group::build_immutable_remote_channel_groups_v1(
        assignments,
        &group_budget,
    )
    .unwrap();
    let group_id = groups
        .assignments_v1()
        .iter()
        .find(|assignment| assignment.channel_id() == channel_id)
        .expect("the finalized fixture channel is assigned")
        .group_id();
    let manifest =
        crate::remote_manifest::ImmutableRemoteMcapManifestV1::build_v1(source, groups, 64)
            .unwrap();
    let authority = manifest.temporal_partition_v1(0, group_id).unwrap();
    let validation_budget =
        crate::remote_chunk_validation_count::RemoteValidationCountBudgetV1::new_for_test_v1(
            crate::remote_chunk_validation_count::UnfrozenRemoteValidationCountLimitsV1::generous_for_test_v1(),
            1,
            u64::MAX,
            u64::MAX,
        );
    let plan = match crate::remote_chunk_validation_count::validate_and_count_with_authority_v1(
        evidence,
        &authority,
        &validation_budget,
    ) {
        Ok(plan) => plan,
        Err(error) => {
            return crate::remote_chunk_dispatch::RemoteChunkTerminalV1::Failed(
                crate::remote_chunk_dispatch::RemoteChunkDispatchFailureV1::Validation(error),
            );
        }
    };
    let rows = plan.expected_rows_v1();
    let payload_bytes = plan.expected_payload_bytes_v1();
    let adapter_limits = RemoteExecutableAdapterLimitsV1 {
        max_rows: rows,
        max_payload_bytes: payload_bytes,
        max_steps: 1_000_000,
        max_scratch_bytes: 1_000_000,
        max_builder_bytes: 1_000_000,
        max_output_bytes: 1_000_000,
        max_global_bytes: 8_000_000,
    };
    let adapter_root = RemoteExecutableAdapterBudgetRootV1::new_disarmed_v1(8_000_000);
    let adapter_budgets = plan
        .channels_v1()
        .iter()
        .map(|_| {
            RemoteExecutableAdapterBudgetV1::new_disarmed_with_root_v1(
                adapter_limits,
                &adapter_root,
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    let factories = plan
        .channels_v1()
        .iter()
        .map(|channel| {
            plan.bind_executable_factory_v1(channel.channel_id_v1())
                .unwrap()
        })
        .collect::<Vec<_>>();
    let dispatches = factories
        .iter()
        .zip(plan.channels_v1())
        .zip(&adapter_budgets)
        .enumerate()
        .map(|(ordinal, ((factory, channel), adapter_budget))| {
            let descriptor = factory.typed_output_descriptor_v1().unwrap();
            let descriptor = if mutation == RemoteDispatchTestMutationV1::CrossWireFirstDescriptor
                && ordinal == 0
            {
                descriptor.cross_wired_source_and_config_for_dispatch_test_v1()
            } else {
                descriptor
            };
            let adapter = factory
                .prepare_adapter_v1(
                    channel.message_count_v1(),
                    channel.payload_bytes_v1(),
                    adapter_budget,
                )
                .unwrap();
            crate::remote_chunk_dispatch::RemoteAdmittedChannelDispatchV1::new_v1(
                descriptor, adapter,
            )
        })
        .collect::<Vec<_>>()
        .into_boxed_slice();
    if mutation == RemoteDispatchTestMutationV1::InvalidateSource {
        source_state.invalidate_for_protobuf_test_v1();
    }
    crate::remote_chunk_dispatch::dispatch_admitted_v1(dispatches, &plan)
}

#[cfg(test)]
pub(crate) fn execute_group_from_finalized_source_for_test_v1<'input>(
    source: crate::remote_physical_resolution::ResolvedRemotePhysicalSourceRefV1<'_, 'input>,
    physical: &crate::remote_chunk_scan::PhysicalChunkDefinitionsCapabilityV1<'_, 'input>,
    evidence: crate::remote_chunk_scan::PhysicalChunkMessageEvidenceV1<'input>,
    channel_id: u16,
) -> Vec<re_chunk::Chunk> {
    match dispatch_group_from_finalized_source_for_test_v1(
        source,
        physical,
        evidence,
        channel_id,
        RemoteDispatchTestMutationV1::None,
    ) {
        crate::remote_chunk_dispatch::RemoteChunkTerminalV1::Complete(handoff) => {
            handoff.chunks_v1().cloned().collect()
        }
        crate::remote_chunk_dispatch::RemoteChunkTerminalV1::CompleteEmpty => Vec::new(),
        crate::remote_chunk_dispatch::RemoteChunkTerminalV1::Failed(error) => {
            panic!("the finalized scalar fixture failed dispatch: {error:?}")
        }
    }
}

#[cfg(test)]
pub(crate) fn execute_ros_scalar_from_finalized_source_for_test_v1<'input>(
    source: crate::remote_physical_resolution::ResolvedRemotePhysicalSourceRefV1<'_, 'input>,
    physical: &crate::remote_chunk_scan::PhysicalChunkDefinitionsCapabilityV1<'_, 'input>,
    evidence: crate::remote_chunk_scan::PhysicalChunkMessageEvidenceV1<'input>,
    channel_id: u16,
) -> re_chunk::Chunk {
    let chunks =
        execute_group_from_finalized_source_for_test_v1(source, physical, evidence, channel_id);
    let [chunk] = chunks.as_slice() else {
        panic!("the finalized single-channel fixture emits one chunk");
    };
    chunk.clone()
}

fn checked_add(left: u64, right: u64) -> Result<u64, RemoteDecoderAssignmentErrorV1> {
    left.checked_add(right).ok_or_else(arithmetic_error)
}

fn checked_mul(left: u64, right: u64) -> Result<u64, RemoteDecoderAssignmentErrorV1> {
    left.checked_mul(right).ok_or_else(arithmetic_error)
}

fn arithmetic_error() -> RemoteDecoderAssignmentErrorV1 {
    RemoteDecoderAssignmentErrorV1::ResourceLimitExceeded(
        RemoteDecoderAssignmentResourceLimitV1::Arithmetic,
    )
}

fn align_up_checked(value: u64, alignment: u64) -> Result<u64, RemoteDecoderAssignmentErrorV1> {
    if alignment == 0 || !alignment.is_power_of_two() {
        assignment_fatal_invariant("assignment allocator alignment is invalid");
    }
    let mask = alignment - 1;
    Ok(checked_add(value, mask)? & !mask)
}

fn locked_wasm_allocation_footprint_v1(
    layout: Layout,
) -> Result<u64, RemoteDecoderAssignmentErrorV1> {
    let requested = u64::try_from(layout.size()).map_err(|_overflow| arithmetic_error())?;
    if requested == 0 {
        return Ok(0);
    }
    let alignment = u64::try_from(layout.align()).map_err(|_overflow| arithmetic_error())?;
    let request2size = |request: u64| {
        if request < LOCKED_WASM_DLMALLOC_MIN_CHUNK_V1 - LOCKED_WASM_DLMALLOC_CHUNK_OVERHEAD_V1 - 1
        {
            Ok(LOCKED_WASM_DLMALLOC_MIN_CHUNK_V1)
        } else {
            align_up_checked(
                checked_add(request, LOCKED_WASM_DLMALLOC_CHUNK_OVERHEAD_V1)?,
                LOCKED_WASM_DLMALLOC_ALIGNMENT_V1,
            )
        }
    };
    let chunk = if alignment <= LOCKED_WASM_DLMALLOC_ALIGNMENT_V1 {
        request2size(requested)?
    } else {
        let aligned_payload = request2size(requested)?;
        let memalign_request = checked_add(
            checked_add(aligned_payload, alignment)?,
            LOCKED_WASM_DLMALLOC_MIN_CHUNK_V1 - LOCKED_WASM_DLMALLOC_CHUNK_OVERHEAD_V1,
        )?;
        request2size(memalign_request)?
    };
    align_up_checked(
        checked_add(
            chunk,
            LOCKED_WASM_DLMALLOC_TOP_FOOT_V1 + LOCKED_WASM_DLMALLOC_ALIGNMENT_V1,
        )?,
        LOCKED_WASM_DLMALLOC_PAGE_V1,
    )
}

#[cfg(test)]
mod tests {
    use crate::TopicFilter;
    use crate::decoders::resolve_decoder_owner;
    use crate::remote_chunk_scan::{
        PhysicalChunkAssignmentEvidenceHarnessV1, PhysicalChunkDefinitionsCapabilityV1,
        PhysicalChunkSourceBindingV1,
    };
    use crate::remote_protobuf_descriptor::{
        BoundedRemoteDecoderInitializersV1, RemoteProtobufInitializationBudgetV1,
        UnfrozenRemoteProtobufLimitsV1, initialize_remote_protobuf_v1,
        prepare_remote_protobuf_census_v1,
    };
    use crate::remote_protobuf_projection_boundary::RemoteProtobufProfileScopeV1;
    use crate::remote_ros2_reflection::{
        RemoteDecoderPolicyWireV1, RemoteDefinitionsCapability, RemoteDefinitionsSourceState,
        RemoteRos2InitializationBudget, RemoteRos2InitializationError, RemoteRos2ProfileScopeV1,
        RemoteViewerScopeState, begin_remote_ros2_admission_v1, freeze_remote_decoder_policy_v1,
        materialize_remote_ros2_definitions_v1, preflight_remote_decoder_topic_signatures_v1,
        prepare_remote_ros2_census_v1,
    };
    use crate::remote_summary::definitions::{
        REMOTE_PROTOBUF_ARTIFACT_DESCRIPTOR_V1, ValidatedSummaryDefinitions,
    };
    use crate::remote_summary::validated_summary_definitions_for_test;
    use crate::testing::{
        AdversarialMcapFixture, AdversarialMcapFixtureBuilder, FixtureChannel, FixtureChunk,
        FixtureMessage, FixtureSchema, MessageIndexFault, PartitionFixture, StatisticsFixture,
    };

    use super::*;

    static_assertions::assert_not_impl_any!(
        BoundedRemoteDecoderAssignmentsV1<'static, 'static, 'static, 'static>: Clone, Copy
    );
    static_assertions::assert_not_impl_any!(
        PreparedRemoteDecoderEligibilityV1<'static, 'static, 'static, 'static>: Clone, Copy
    );

    struct StableContext {
        viewer: Box<RemoteViewerScopeState>,
        ros_profile: Box<RemoteRos2ProfileScopeV1>,
        protobuf_profile: Box<RemoteProtobufProfileScopeV1>,
        source: RemoteDefinitionsSourceState,
        wire: RemoteDecoderPolicyWireV1<'static>,
    }

    impl StableContext {
        fn new() -> Self {
            let viewer = Box::new(RemoteViewerScopeState::new_for_protobuf_test_v1(1));
            let ros_profile = Box::new(RemoteRos2ProfileScopeV1::new_for_protobuf_test_v1(1));
            let protobuf_profile = Box::new(RemoteProtobufProfileScopeV1::new_disarmed_v1());
            let source = RemoteDefinitionsSourceState::new_for_protobuf_test_v1(
                PhysicalChunkSourceBindingV1::new_unscanned_for_assignment_test_v1(),
                &viewer,
                &protobuf_profile,
            );
            Self {
                viewer,
                ros_profile,
                protobuf_profile,
                source,
                wire: RemoteDecoderPolicyWireV1::canonical_for_protobuf_test_v1(),
            }
        }

        fn new_with_selected_topics(selected_topics: &'static [&'static str]) -> Self {
            let viewer = Box::new(RemoteViewerScopeState::new_for_protobuf_test_v1(1));
            let ros_profile = Box::new(RemoteRos2ProfileScopeV1::new_for_protobuf_test_v1(1));
            let protobuf_profile = Box::new(RemoteProtobufProfileScopeV1::new_disarmed_v1());
            let source =
                RemoteDefinitionsSourceState::new_for_assignment_test_with_selected_topics_v1(
                    PhysicalChunkSourceBindingV1::new_unscanned_for_assignment_test_v1(),
                    &viewer,
                    &protobuf_profile,
                    selected_topics,
                );
            Self {
                viewer,
                ros_profile,
                protobuf_profile,
                source,
                wire: RemoteDecoderPolicyWireV1::canonical_for_protobuf_test_v1(),
            }
        }

        fn new_with_filter(filter: TopicFilter) -> Self {
            let viewer = Box::new(RemoteViewerScopeState::new_for_protobuf_test_v1(1));
            let ros_profile = Box::new(RemoteRos2ProfileScopeV1::new_for_protobuf_test_v1(1));
            let protobuf_profile = Box::new(RemoteProtobufProfileScopeV1::new_disarmed_v1());
            let source = RemoteDefinitionsSourceState::new_for_assignment_test_with_filter_v1(
                PhysicalChunkSourceBindingV1::new_unscanned_for_assignment_test_v1(),
                &viewer,
                &protobuf_profile,
                filter,
            );
            Self {
                viewer,
                ros_profile,
                protobuf_profile,
                source,
                wire: RemoteDecoderPolicyWireV1::canonical_for_protobuf_test_v1(),
            }
        }

        fn new_with_filter_and_physical(
            filter: TopicFilter,
            physical_source: crate::remote_chunk_scan::PhysicalChunkSourceBindingV1,
        ) -> Self {
            let viewer = Box::new(RemoteViewerScopeState::new_for_protobuf_test_v1(1));
            let ros_profile = Box::new(RemoteRos2ProfileScopeV1::new_for_protobuf_test_v1(1));
            let protobuf_profile = Box::new(RemoteProtobufProfileScopeV1::new_disarmed_v1());
            let source =
                RemoteDefinitionsSourceState::new_for_assignment_test_with_filter_and_physical_v1(
                    physical_source,
                    &viewer,
                    &protobuf_profile,
                    filter,
                );
            Self {
                viewer,
                ros_profile,
                protobuf_profile,
                source,
                wire: RemoteDecoderPolicyWireV1::canonical_for_protobuf_test_v1(),
            }
        }

        fn protobuf_budget(&self) -> RemoteProtobufInitializationBudgetV1<'_, '_> {
            RemoteProtobufInitializationBudgetV1::new_disarmed_v1(
                &self.viewer,
                &self.protobuf_profile,
                UnfrozenRemoteProtobufLimitsV1::generous_for_assignment_test_v1(),
                8,
                256_000_000,
                8,
                256_000_000,
            )
        }
    }

    fn combined_initializers<'definitions, 'input, 'source>(
        definitions: &'definitions ValidatedSummaryDefinitions<'input>,
        context: &'source StableContext,
        protobuf_budget: &RemoteProtobufInitializationBudgetV1<'_, '_>,
    ) -> (
        BoundedRemoteDecoderInitializersV1<'definitions, 'input, 'source, 'source>,
        RemoteRos2InitializationBudget<'source, 'source>,
    ) {
        let physical = PhysicalChunkDefinitionsCapabilityV1::new_unscanned_for_test_with_binding_v1(
            definitions,
            context.source.physical_source_binding_for_test_v1(),
        );
        combined_initializers_with_physical(&physical, context, protobuf_budget)
    }

    fn combined_initializers_with_physical<'definitions, 'input, 'source>(
        physical: &PhysicalChunkDefinitionsCapabilityV1<'definitions, 'input>,
        context: &'source StableContext,
        protobuf_budget: &RemoteProtobufInitializationBudgetV1<'_, '_>,
    ) -> (
        BoundedRemoteDecoderInitializersV1<'definitions, 'input, 'source, 'source>,
        RemoteRos2InitializationBudget<'source, 'source>,
    ) {
        let ros_budget = RemoteRos2InitializationBudget::new_for_protobuf_test_v1(
            &context.source,
            &context.viewer,
            &context.ros_profile,
            &context.wire,
        );
        let policy = freeze_remote_decoder_policy_v1(&context.wire).unwrap();
        let source = RemoteDefinitionsCapability::new_for_protobuf_test_v1(
            physical,
            &context.source,
            &policy,
            &ros_budget,
        )
        .unwrap();
        let owner = begin_remote_ros2_admission_v1(source, policy, &ros_budget).unwrap();
        let evidence = preflight_remote_decoder_topic_signatures_v1(owner).unwrap();
        let prepared_ros = prepare_remote_ros2_census_v1(evidence).unwrap();
        let transition = materialize_remote_ros2_definitions_v1(prepared_ros)
            .unwrap()
            .into_protobuf_transition_v1()
            .unwrap();
        let prepared_protobuf =
            prepare_remote_protobuf_census_v1(transition, protobuf_budget).unwrap();
        (
            initialize_remote_protobuf_v1(prepared_protobuf).unwrap(),
            ros_budget,
        )
    }

    fn assignment_limits() -> UnfrozenRemoteDecoderAssignmentLimitsV1 {
        UnfrozenRemoteDecoderAssignmentLimitsV1 {
            profile_version: REMOTE_ASSIGNMENT_PROFILE_VERSION_V1,
            max_channels: MAX_INLINE_ASSIGNMENT_CHANNELS_V1 as u64,
            max_census_steps: 1_000_000,
            max_working_bytes: 1_000_000,
            max_retained_bytes: 1_000_000,
            max_combined_retained_bytes: u64::MAX,
        }
    }

    fn assignment_budget() -> RemoteDecoderAssignmentBudgetV1 {
        RemoteDecoderAssignmentBudgetV1::new_disarmed_v1(
            assignment_limits(),
            8,
            8_000_000,
            8,
            8_000_000,
            u64::MAX,
        )
    }

    fn assign_for_test_v1<'definitions, 'input, 'source>(
        initializers: BoundedRemoteDecoderInitializersV1<'definitions, 'input, 'source, 'source>,
        budget: &RemoteDecoderAssignmentBudgetV1,
    ) -> Result<
        BoundedRemoteDecoderAssignmentsV1<'definitions, 'input, 'source, 'source>,
        RemoteDecoderAssignmentErrorV1,
    > {
        let eligibility = prepare_remote_decoder_eligibility_v1(initializers, budget)?;
        assign_remote_decoders_v1(eligibility)
    }

    fn assign_with_gate_for_test_v1<'definitions, 'input, 'source>(
        initializers: BoundedRemoteDecoderInitializersV1<'definitions, 'input, 'source, 'source>,
        budget: &RemoteDecoderAssignmentBudgetV1,
        gate: &impl RemoteDecoderAssignmentAllocationGateV1,
    ) -> Result<
        BoundedRemoteDecoderAssignmentsV1<'definitions, 'input, 'source, 'source>,
        RemoteDecoderAssignmentErrorV1,
    > {
        let eligibility = prepare_remote_decoder_eligibility_v1(initializers, budget)?;
        assign_remote_decoders_with_gate_v1(eligibility, gate)
    }

    fn build_groups_for_test_v1<'definitions, 'input, 'source>(
        initializers: BoundedRemoteDecoderInitializersV1<'definitions, 'input, 'source, 'source>,
        assignment_budget: &RemoteDecoderAssignmentBudgetV1,
        group_budget: &crate::remote_channel_group::RemoteChannelGroupBudgetV1,
    ) -> Result<
        crate::remote_channel_group::ImmutableRemoteChannelGroupsV1<
            'definitions,
            'input,
            'source,
            'source,
        >,
        crate::remote_channel_group::RemoteChannelGroupErrorV1,
    > {
        let assignment = assign_for_test_v1(initializers, assignment_budget).map_err(|_error| {
            crate::remote_channel_group::RemoteChannelGroupErrorV1::AssignmentFailed
        })?;
        crate::remote_channel_group::build_immutable_remote_channel_groups_v1(
            assignment,
            group_budget,
        )
    }

    fn fixture(
        schemas: impl IntoIterator<Item = FixtureSchema>,
        channels: impl IntoIterator<Item = FixtureChannel>,
        message_channels: impl IntoIterator<Item = u16>,
    ) -> AdversarialMcapFixture {
        AdversarialMcapFixtureBuilder::new()
            .with_schemas(schemas)
            .with_channels(channels)
            .with_chunks([FixtureChunk::new(
                message_channels
                    .into_iter()
                    .enumerate()
                    .map(|(sequence, channel_id)| {
                        FixtureMessage::new(channel_id, sequence as u32, sequence as u64 + 1)
                    }),
            )])
            .with_partition_fixture(PartitionFixture::default())
            .build()
            .expect("assignment fixture builds")
    }

    #[expect(
        clippy::disallowed_methods,
        reason = "host-only tests verify that source/config mismatches use the fatal control-plane path"
    )]
    fn assert_fatal_control_plane(action: impl FnOnce()) {
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(action)).is_err(),
            "control-plane invariant must use the internal fatal path"
        );
    }

    fn schema(id: u16, name: &str, encoding: &str, data: impl Into<Vec<u8>>) -> FixtureSchema {
        FixtureSchema::new(id, name, encoding).with_data(data)
    }

    fn channel(id: u16, schema_id: u16, topic: &str, encoding: &str) -> FixtureChannel {
        FixtureChannel::schema_less(id, topic).with_schema(schema_id, encoding)
    }

    #[test]
    fn pure_priority_core_prefers_first_recognizer_and_uses_fallback_lazily() {
        let mut fallback_calls = 0;
        assert_eq!(
            resolve_decoder_owner(
                [
                    (RemoteDecoderOwnerV1::Ros2Reflection, true),
                    (RemoteDecoderOwnerV1::Protobuf, true),
                ],
                || {
                    fallback_calls += 1;
                    Some(RemoteDecoderOwnerV1::Raw)
                },
            ),
            Some(RemoteDecoderOwnerV1::Ros2Reflection)
        );
        assert_eq!(fallback_calls, 0);
        assert_eq!(
            resolve_decoder_owner(
                [
                    (RemoteDecoderOwnerV1::Ros2Reflection, false),
                    (RemoteDecoderOwnerV1::Protobuf, false),
                ],
                || {
                    fallback_calls += 1;
                    Some(RemoteDecoderOwnerV1::Raw)
                },
            ),
            Some(RemoteDecoderOwnerV1::Raw)
        );
        assert_eq!(fallback_calls, 1);
    }

    #[test]
    fn noncanonical_policy_fails_before_eligibility_or_assignment_budget() {
        let budget = assignment_budget();
        for wire in [
            RemoteDecoderPolicyWireV1::unknown_version_for_assignment_test_v1(),
            RemoteDecoderPolicyWireV1::reordered_for_assignment_test_v1(),
            RemoteDecoderPolicyWireV1::noncanonical_fallback_for_assignment_test_v1(),
        ] {
            assert!(freeze_remote_decoder_policy_v1(&wire).is_err());
            assert_eq!(
                *budget.state.usage.lock(),
                RemoteDecoderAssignmentBudgetUsageV1::default(),
                "invalid policy must fail before eligibility or assignment ownership",
            );
        }
    }

    #[test]
    fn combined_initializers_assign_reflection_protobuf_and_raw_like_local_plan() {
        let fixture = fixture(
            [
                schema(7, "pkg/Custom", "ros2msg", b"int32 value"),
                schema(
                    8,
                    "rerun.Message",
                    "protobuf",
                    REMOTE_PROTOBUF_ARTIFACT_DESCRIPTOR_V1,
                ),
            ],
            [
                channel(3, 0, "/raw", "raw"),
                channel(2, 8, "/protobuf", "protobuf"),
                channel(1, 7, "/reflection", "cdr"),
            ],
            [1, 2, 3],
        );
        let definitions = validated_summary_definitions_for_test(&fixture);
        let context = StableContext::new();
        let protobuf_budget = context.protobuf_budget();
        let (initializers, ros_budget) =
            combined_initializers(&definitions, &context, &protobuf_budget);
        let assignment_budget = assignment_budget();
        let result = assign_for_test_v1(initializers, &assignment_budget).unwrap();
        assert_eq!(
            result.policy_identity_for_test_v1(),
            context.wire.exact_identity_for_assignment_test_v1(),
            "assignment must retain the exact frozen initializer policy descriptor",
        );
        assert_eq!(
            result
                .assignments_for_test()
                .iter()
                .map(|assignment| (
                    assignment.channel_id(),
                    assignment.eligibility(),
                    assignment.owner()
                ))
                .collect::<Vec<_>>(),
            [
                (
                    3,
                    RemoteChannelEligibilityV1::Unknown,
                    RemoteDecoderOwnerV1::Raw,
                ),
                (
                    2,
                    RemoteChannelEligibilityV1::Unknown,
                    RemoteDecoderOwnerV1::Protobuf,
                ),
                (
                    1,
                    RemoteChannelEligibilityV1::Unknown,
                    RemoteDecoderOwnerV1::Ros2Reflection,
                ),
            ]
        );

        let summary = fixture.read_upstream_summary().unwrap().unwrap();
        let local = crate::DecoderRegistry::all_with_raw_fallback()
            .plan(&fixture.bytes, &summary, &crate::TopicFilter::default())
            .unwrap();
        let local = local
            .assignments
            .iter()
            .map(|assignment| (assignment.channel_id.0, assignment.decoder.to_string()))
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(local.get(&1).map(String::as_str), Some("ros2_reflection"));
        assert_eq!(local.get(&2).map(String::as_str), Some("protobuf"));
        assert_eq!(local.get(&3).map(String::as_str), Some("raw"));

        drop(result);
        assert!(ros_budget.is_idle_for_assignment_test_v1());
        assert!(protobuf_budget.is_idle_for_assignment_test_v1());
        assert_eq!(
            *assignment_budget.state.usage.lock(),
            RemoteDecoderAssignmentBudgetUsageV1::default()
        );
    }

    #[test]
    fn semantic_builtin_is_rejected_without_reflection_or_raw_fallback() {
        for schema_name in [
            "sensor_msgs/msg/BatteryState",
            "sensor_msgs/msg/CompressedImage",
            "sensor_msgs/msg/FluidPressure",
            "sensor_msgs/msg/Illuminance",
            "sensor_msgs/msg/Image",
            "sensor_msgs/msg/Imu",
            "sensor_msgs/msg/Joy",
            "sensor_msgs/msg/JointState",
            "sensor_msgs/msg/NavSatFix",
            "sensor_msgs/msg/PointCloud2",
            "sensor_msgs/msg/Range",
            "sensor_msgs/msg/RelativeHumidity",
            "sensor_msgs/msg/Temperature",
            "std_msgs/msg/Float64Array",
            "std_msgs/msg/Float64MultiArray",
            "tf2_msgs/msg/TFMessage",
        ] {
            let fixture = fixture(
                [schema(7, schema_name, "ros2msg", b"int32 value")],
                [channel(1, 7, "/semantic", "cdr")],
                [1],
            );
            let definitions = validated_summary_definitions_for_test(&fixture);
            let context = StableContext::new();
            let protobuf_budget = context.protobuf_budget();
            let (initializers, ros_budget) =
                combined_initializers(&definitions, &context, &protobuf_budget);
            let assignment_budget = assignment_budget();
            let error = match assign_for_test_v1(initializers, &assignment_budget) {
                Ok(_result) => panic!("semantic parser unexpectedly entered the empty V1 table"),
                Err(error) => error,
            };
            assert_eq!(
                error,
                RemoteDecoderAssignmentErrorV1::UnsupportedForRemote(
                    UnsupportedRemoteDecoderAssignmentV1::SemanticParserNotAllowlisted,
                )
            );
            assert!(ros_budget.is_idle_for_assignment_test_v1());
            assert!(protobuf_budget.is_idle_for_assignment_test_v1());
            assert_eq!(
                *assignment_budget.state.usage.lock(),
                RemoteDecoderAssignmentBudgetUsageV1::default()
            );
        }
    }

    #[test]
    fn frozen_filter_excludes_semantic_recognition_and_result_capacity() {
        let fixture = fixture(
            [schema(
                7,
                "sensor_msgs/msg/Image",
                "ros2msg",
                b"int32 value",
            )],
            [
                channel(1, 7, "/semantic", "cdr"),
                FixtureChannel::schema_less(2, "/raw"),
            ],
            [1, 2],
        );
        let definitions = validated_summary_definitions_for_test(&fixture);
        let context = StableContext::new_with_selected_topics(&["/raw"]);
        let protobuf_budget = context.protobuf_budget();
        let (initializers, ros_budget) =
            combined_initializers(&definitions, &context, &protobuf_budget);
        let limits = UnfrozenRemoteDecoderAssignmentLimitsV1 {
            max_channels: 1,
            ..assignment_limits()
        };
        let budget = RemoteDecoderAssignmentBudgetV1::new_disarmed_v1(
            limits,
            1,
            1_000_000,
            1,
            1_000_000,
            u64::MAX,
        );
        let result = assign_for_test_v1(initializers, &budget).unwrap();
        let row = &result.assignments_for_test()[0];
        assert_eq!(
            (row.channel_id(), row.eligibility(), row.owner()),
            (
                2,
                RemoteChannelEligibilityV1::Unknown,
                RemoteDecoderOwnerV1::Raw
            )
        );
        assert_eq!(context.source.assignment_recognition_count_for_test_v1(), 1);
        drop(result);
        assert!(ros_budget.is_idle_for_assignment_test_v1());
        assert!(protobuf_budget.is_idle_for_assignment_test_v1());
        assert_eq!(
            *budget.state.usage.lock(),
            RemoteDecoderAssignmentBudgetUsageV1::default()
        );
    }

    #[test]
    fn eligibility_owner_carries_unknown_and_known_non_empty_rows() {
        let fixture = fixture(
            [],
            [
                FixtureChannel::schema_less(1, "/observed"),
                FixtureChannel::schema_less(2, "/unknown"),
            ],
            [1],
        );
        let physical = PhysicalChunkAssignmentEvidenceHarnessV1::new(&fixture, 0).unwrap();
        let physical_definitions = physical.definitions_capability_v1();
        let context = StableContext::new_with_filter_and_physical(
            TopicFilter::default(),
            physical.source_binding_v1(),
        );
        let protobuf_budget = context.protobuf_budget();
        let (initializers, ros_budget) =
            combined_initializers_with_physical(&physical_definitions, &context, &protobuf_budget);
        let budget = assignment_budget();
        let evidence = physical.take_evidence_v1();
        let eligibility =
            prepare_remote_decoder_eligibility_with_evidence_v1(initializers, evidence, &budget)
                .unwrap();
        let result = assign_remote_decoders_v1(eligibility).unwrap();
        assert_eq!(
            result
                .assignments_for_test()
                .iter()
                .map(|row| (row.channel_id(), row.eligibility()))
                .collect::<Vec<_>>(),
            [
                (1, RemoteChannelEligibilityV1::KnownNonEmpty),
                (2, RemoteChannelEligibilityV1::Unknown),
            ]
        );
        drop(result);
        assert!(ros_budget.is_idle_for_assignment_test_v1());
        assert!(protobuf_budget.is_idle_for_assignment_test_v1());
    }

    #[test]
    fn physical_presence_is_channel_id_exact_for_same_topic_channels() {
        let fixture = fixture(
            [],
            [
                FixtureChannel::schema_less(1, "/same"),
                FixtureChannel::schema_less(2, "/same"),
            ],
            [2],
        );
        let physical = PhysicalChunkAssignmentEvidenceHarnessV1::new(&fixture, 0).unwrap();
        let physical_definitions = physical.definitions_capability_v1();
        let context = StableContext::new_with_filter_and_physical(
            TopicFilter::default(),
            physical.source_binding_v1(),
        );
        let protobuf_budget = context.protobuf_budget();
        let (initializers, ros_budget) =
            combined_initializers_with_physical(&physical_definitions, &context, &protobuf_budget);
        let budget = assignment_budget();
        let prepared = prepare_remote_decoder_eligibility_with_evidence_v1(
            initializers,
            physical.take_evidence_v1(),
            &budget,
        )
        .unwrap();
        let result = assign_remote_decoders_v1(prepared).unwrap();
        assert_eq!(
            result
                .assignments_for_test()
                .iter()
                .map(|row| (row.channel_id(), row.eligibility()))
                .collect::<Vec<_>>(),
            [
                (1, RemoteChannelEligibilityV1::Unknown),
                (2, RemoteChannelEligibilityV1::KnownNonEmpty),
            ],
        );
        drop(result);
        assert!(!physical.read_is_still_claimed_v1());
        assert!(ros_budget.is_idle_for_assignment_test_v1());
        assert!(protobuf_budget.is_idle_for_assignment_test_v1());
    }

    #[test]
    fn filtered_physical_messages_cannot_mint_presence_for_selected_channels() {
        let fixture = fixture(
            [],
            [
                FixtureChannel::schema_less(1, "/filtered"),
                FixtureChannel::schema_less(2, "/selected"),
            ],
            [1],
        );
        let physical = PhysicalChunkAssignmentEvidenceHarnessV1::new(&fixture, 0).unwrap();
        let physical_definitions = physical.definitions_capability_v1();
        let filter = TopicFilter::default()
            .with_include_patterns(&["^/selected$".to_owned()])
            .unwrap();
        let context =
            StableContext::new_with_filter_and_physical(filter, physical.source_binding_v1());
        let protobuf_budget = context.protobuf_budget();
        let (initializers, ros_budget) =
            combined_initializers_with_physical(&physical_definitions, &context, &protobuf_budget);
        let budget = assignment_budget();
        let prepared = prepare_remote_decoder_eligibility_with_evidence_v1(
            initializers,
            physical.take_evidence_v1(),
            &budget,
        )
        .unwrap();
        let result = assign_remote_decoders_v1(prepared).unwrap();
        let row = &result.assignments_for_test()[0];
        assert_eq!(
            (row.channel_id(), row.eligibility(), row.owner()),
            (
                2,
                RemoteChannelEligibilityV1::Unknown,
                RemoteDecoderOwnerV1::Raw
            )
        );
        drop(result);
        assert!(!physical.read_is_still_claimed_v1());
        assert!(ros_budget.is_idle_for_assignment_test_v1());
        assert!(protobuf_budget.is_idle_for_assignment_test_v1());
    }

    #[test]
    fn stale_physical_source_rejects_presence_and_releases_scan_claim() {
        let fixture = fixture([], [FixtureChannel::schema_less(1, "/observed")], [1]);
        let physical = PhysicalChunkAssignmentEvidenceHarnessV1::new(&fixture, 0).unwrap();
        let physical_definitions = physical.definitions_capability_v1();
        let context = StableContext::new_with_filter_and_physical(
            TopicFilter::default(),
            physical.source_binding_v1(),
        );
        let protobuf_budget = context.protobuf_budget();
        let (initializers, ros_budget) =
            combined_initializers_with_physical(&physical_definitions, &context, &protobuf_budget);
        let evidence = physical.take_evidence_v1();
        assert!(physical.read_is_still_claimed_v1());
        physical.close_source_v1();
        let budget = assignment_budget();
        assert_eq!(
            prepare_remote_decoder_eligibility_with_evidence_v1(initializers, evidence, &budget,)
                .err(),
            Some(RemoteDecoderAssignmentErrorV1::StaleSource),
        );
        assert_eq!(
            *budget.state.usage.lock(),
            RemoteDecoderAssignmentBudgetUsageV1::default()
        );
        assert!(!physical.read_is_still_claimed_v1());
        assert!(ros_budget.is_idle_for_assignment_test_v1());
        assert!(protobuf_budget.is_idle_for_assignment_test_v1());
    }

    #[test]
    fn same_value_evidence_from_another_semantic_config_owner_is_fatal() {
        let fixture = fixture([], [FixtureChannel::schema_less(1, "/observed")], [1]);
        let physical_a = PhysicalChunkAssignmentEvidenceHarnessV1::new(&fixture, 0).unwrap();
        let physical_b = PhysicalChunkAssignmentEvidenceHarnessV1::new(&fixture, 0).unwrap();
        let definitions_a = physical_a.definitions_capability_v1();
        let definitions_b = physical_b.definitions_capability_v1();
        let context_a = StableContext::new_with_filter_and_physical(
            TopicFilter::default(),
            physical_a.source_binding_v1(),
        );
        let context_b = StableContext::new_with_filter_and_physical(
            TopicFilter::default(),
            physical_b.source_binding_v1(),
        );
        let protobuf_budget_a = context_a.protobuf_budget();
        let protobuf_budget_b = context_b.protobuf_budget();
        let (initializers_a, ros_budget_a) =
            combined_initializers_with_physical(&definitions_a, &context_a, &protobuf_budget_a);
        let (initializers_b, ros_budget_b) =
            combined_initializers_with_physical(&definitions_b, &context_b, &protobuf_budget_b);
        let evidence_b = physical_b.take_evidence_v1();
        let assignment_budget = assignment_budget();

        assert_fatal_control_plane(|| {
            let _prepared = prepare_remote_decoder_eligibility_with_evidence_v1(
                initializers_a,
                evidence_b,
                &assignment_budget,
            );
        });

        assert_eq!(
            context_a
                .source
                .assignment_membership_projection_count_for_test_v1(),
            1
        );
        assert_eq!(
            context_a.source.assignment_recognition_count_for_test_v1(),
            0
        );
        assert_eq!(
            *assignment_budget.state.usage.lock(),
            RemoteDecoderAssignmentBudgetUsageV1::default()
        );
        drop(initializers_b);
        assert!(ros_budget_a.is_idle_for_assignment_test_v1());
        assert!(protobuf_budget_a.is_idle_for_assignment_test_v1());
        assert!(ros_budget_b.is_idle_for_assignment_test_v1());
        assert!(protobuf_budget_b.is_idle_for_assignment_test_v1());
    }

    #[test]
    fn filtered_conflicting_topic_is_still_rejected_by_initializer_preflight() {
        let fixture = fixture(
            [],
            [
                FixtureChannel::schema_less(1, "/conflict"),
                FixtureChannel::schema_less(2, "/conflict").with_schema(0, "cdr"),
            ],
            [1],
        );
        let definitions = validated_summary_definitions_for_test(&fixture);
        let context = StableContext::new_with_selected_topics(&[]);
        let ros_budget = RemoteRos2InitializationBudget::new_for_protobuf_test_v1(
            &context.source,
            &context.viewer,
            &context.ros_profile,
            &context.wire,
        );
        let policy = freeze_remote_decoder_policy_v1(&context.wire).unwrap();
        let physical = PhysicalChunkDefinitionsCapabilityV1::new_unscanned_for_test_with_binding_v1(
            &definitions,
            context.source.physical_source_binding_for_test_v1(),
        );
        let source = RemoteDefinitionsCapability::new_for_protobuf_test_v1(
            &physical,
            &context.source,
            &policy,
            &ros_budget,
        )
        .unwrap();
        let owner = begin_remote_ros2_admission_v1(source, policy, &ros_budget).unwrap();
        let error = match preflight_remote_decoder_topic_signatures_v1(owner) {
            Ok(_evidence) => panic!("filtered conflicting topic unexpectedly passed preflight"),
            Err(error) => error,
        };
        assert_eq!(
            error,
            RemoteRos2InitializationError::ConflictingTopicDecoderSignature
        );
        assert_eq!(context.source.assignment_recognition_count_for_test_v1(), 0);
        assert!(ros_budget.is_idle_for_assignment_test_v1());
    }

    struct RejectAllocationGateV1;

    impl RemoteDecoderAssignmentAllocationGateV1 for RejectAllocationGateV1 {
        fn before_allocation(&self, _layout: Layout) -> Result<(), RemoteDecoderAssignmentErrorV1> {
            Err(RemoteDecoderAssignmentErrorV1::FallibleAllocationFailed)
        }
    }

    #[test]
    fn result_allocation_failure_rolls_back_all_three_reservations() {
        let local_fixture = fixture([], [FixtureChannel::schema_less(1, "/raw")], [1]);
        let definitions = validated_summary_definitions_for_test(&local_fixture);
        let context = StableContext::new();
        let protobuf_budget = context.protobuf_budget();
        let (initializers, ros_budget) =
            combined_initializers(&definitions, &context, &protobuf_budget);
        let assignment_budget = assignment_budget();
        let error = match assign_with_gate_for_test_v1(
            initializers,
            &assignment_budget,
            &RejectAllocationGateV1,
        ) {
            Ok(_result) => panic!("rejected assignment allocation unexpectedly succeeded"),
            Err(error) => error,
        };
        assert_eq!(
            error,
            RemoteDecoderAssignmentErrorV1::FallibleAllocationFailed
        );
        assert!(ros_budget.is_idle_for_assignment_test_v1());
        assert!(protobuf_budget.is_idle_for_assignment_test_v1());
        assert_eq!(
            *assignment_budget.state.usage.lock(),
            RemoteDecoderAssignmentBudgetUsageV1::default()
        );
    }

    #[test]
    fn channel_step_and_combined_byte_limits_fail_before_result_allocation() {
        for limited in [
            UnfrozenRemoteDecoderAssignmentLimitsV1 {
                max_channels: 1,
                ..assignment_limits()
            },
            UnfrozenRemoteDecoderAssignmentLimitsV1 {
                max_census_steps: 0,
                ..assignment_limits()
            },
            UnfrozenRemoteDecoderAssignmentLimitsV1 {
                max_combined_retained_bytes: 0,
                ..assignment_limits()
            },
        ] {
            let fixture = fixture(
                [],
                [
                    FixtureChannel::schema_less(1, "/one"),
                    FixtureChannel::schema_less(2, "/two"),
                ],
                [1],
            );
            let definitions = validated_summary_definitions_for_test(&fixture);
            let context = StableContext::new();
            let protobuf_budget = context.protobuf_budget();
            let (initializers, ros_budget) =
                combined_initializers(&definitions, &context, &protobuf_budget);
            let budget = RemoteDecoderAssignmentBudgetV1::new_disarmed_v1(
                limited,
                1,
                u64::MAX,
                1,
                u64::MAX,
                u64::MAX,
            );
            assert!(assign_for_test_v1(initializers, &budget).is_err());
            assert!(ros_budget.is_idle_for_assignment_test_v1());
            assert!(protobuf_budget.is_idle_for_assignment_test_v1());
            assert_eq!(
                *budget.state.usage.lock(),
                RemoteDecoderAssignmentBudgetUsageV1::default()
            );
        }
    }

    #[test]
    fn exact_assignment_and_combined_retained_capacity_boundaries_are_enforced() {
        let assignment_bytes = locked_wasm_allocation_footprint_v1(
            Layout::array::<RemoteChannelDecoderAssignmentV1>(1).unwrap(),
        )
        .unwrap();
        assert_eq!(assignment_bytes, LOCKED_WASM_DLMALLOC_PAGE_V1);
        let retained_bytes = assignment_bytes
            + u64::try_from(
                std::mem::size_of::<RemoteMcapSelectedMembershipV1>()
                    + std::mem::size_of::<Option<RemoteKnownNonEmptyEvidenceV1<'static>>>(),
            )
            .unwrap();

        for subtract_one in [true, false] {
            let fixture = fixture([], [FixtureChannel::schema_less(1, "/raw")], [1]);
            let definitions = validated_summary_definitions_for_test(&fixture);
            let context = StableContext::new();
            let protobuf_budget = context.protobuf_budget();
            let (initializers, ros_budget) =
                combined_initializers(&definitions, &context, &protobuf_budget);
            let initializer_bytes = initializers
                .combined_retained_bytes_for_assignment_v1()
                .unwrap();
            let exact_combined = initializer_bytes.checked_add(retained_bytes).unwrap();
            let capacity = if subtract_one {
                exact_combined - 1
            } else {
                exact_combined
            };
            let limits = UnfrozenRemoteDecoderAssignmentLimitsV1 {
                max_retained_bytes: retained_bytes,
                max_combined_retained_bytes: capacity,
                ..assignment_limits()
            };
            let budget = RemoteDecoderAssignmentBudgetV1::new_disarmed_v1(
                limits,
                1,
                u64::MAX,
                1,
                retained_bytes,
                capacity,
            );
            let result = assign_for_test_v1(initializers, &budget);
            assert_eq!(result.is_ok(), !subtract_one);
            drop(result);
            assert!(ros_budget.is_idle_for_assignment_test_v1());
            assert!(protobuf_budget.is_idle_for_assignment_test_v1());
            assert_eq!(
                *budget.state.usage.lock(),
                RemoteDecoderAssignmentBudgetUsageV1::default()
            );
        }
    }

    struct InvalidateBeforeAllocationGateV1<'source> {
        source: &'source RemoteDefinitionsSourceState,
    }

    impl RemoteDecoderAssignmentAllocationGateV1 for InvalidateBeforeAllocationGateV1<'_> {
        fn before_allocation(&self, _layout: Layout) -> Result<(), RemoteDecoderAssignmentErrorV1> {
            self.source.invalidate_for_protobuf_test_v1();
            Ok(())
        }
    }

    #[test]
    fn stale_before_census_and_after_reservation_roll_back_without_result() {
        for invalidate_during_allocation in [false, true] {
            let fixture = fixture([], [FixtureChannel::schema_less(1, "/raw")], [1]);
            let definitions = validated_summary_definitions_for_test(&fixture);
            let context = StableContext::new();
            let protobuf_budget = context.protobuf_budget();
            let (initializers, ros_budget) =
                combined_initializers(&definitions, &context, &protobuf_budget);
            let budget = assignment_budget();
            let error = if invalidate_during_allocation {
                match assign_with_gate_for_test_v1(
                    initializers,
                    &budget,
                    &InvalidateBeforeAllocationGateV1 {
                        source: &context.source,
                    },
                ) {
                    Ok(_result) => panic!("stale post-allocation owner unexpectedly completed"),
                    Err(error) => error,
                }
            } else {
                context.source.invalidate_for_protobuf_test_v1();
                match assign_for_test_v1(initializers, &budget) {
                    Ok(_result) => panic!("stale pre-census owner unexpectedly completed"),
                    Err(error) => error,
                }
            };
            assert_eq!(error, RemoteDecoderAssignmentErrorV1::StaleSource);
            assert!(ros_budget.is_idle_for_assignment_test_v1());
            assert!(protobuf_budget.is_idle_for_assignment_test_v1());
            assert_eq!(
                *budget.state.usage.lock(),
                RemoteDecoderAssignmentBudgetUsageV1::default()
            );
        }
    }

    #[test]
    fn statistics_and_missing_message_index_do_not_prune_actual_or_summary_only_channels() {
        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_channels([
                FixtureChannel::schema_less(1, "/actual"),
                FixtureChannel::schema_less(2, "/summary-only"),
            ])
            .with_chunks([FixtureChunk::single(FixtureMessage::new(1, 0, 1))])
            .with_statistics_fixture(StatisticsFixture::WrongMessageCount)
            .with_message_index_fault(MessageIndexFault::Missing)
            .with_partition_fixture(PartitionFixture::default())
            .build()
            .unwrap();
        let definitions = validated_summary_definitions_for_test(&fixture);
        let context = StableContext::new();
        let protobuf_budget = context.protobuf_budget();
        let (initializers, ros_budget) =
            combined_initializers(&definitions, &context, &protobuf_budget);
        let budget = assignment_budget();
        let result = assign_for_test_v1(initializers, &budget).unwrap();
        assert_eq!(
            result
                .assignments_for_test()
                .iter()
                .map(|row| (row.channel_id(), row.eligibility(), row.owner()))
                .collect::<Vec<_>>(),
            [
                (
                    1,
                    RemoteChannelEligibilityV1::Unknown,
                    RemoteDecoderOwnerV1::Raw,
                ),
                (
                    2,
                    RemoteChannelEligibilityV1::Unknown,
                    RemoteDecoderOwnerV1::Raw,
                ),
            ]
        );
        drop(result);
        assert!(ros_budget.is_idle_for_assignment_test_v1());
        assert!(protobuf_budget.is_idle_for_assignment_test_v1());
        assert_eq!(
            *budget.state.usage.lock(),
            RemoteDecoderAssignmentBudgetUsageV1::default()
        );
    }

    #[test]
    fn aggregate_contention_fails_before_result_allocation_and_recovers_on_drop() {
        let fixture_a = fixture([], [FixtureChannel::schema_less(1, "/a")], [1]);
        let fixture_b = fixture([], [FixtureChannel::schema_less(1, "/b")], [1]);
        let definitions_a = validated_summary_definitions_for_test(&fixture_a);
        let definitions_b = validated_summary_definitions_for_test(&fixture_b);
        let context_a = StableContext::new();
        let context_b = StableContext::new();
        let protobuf_budget_a = context_a.protobuf_budget();
        let protobuf_budget_b = context_b.protobuf_budget();
        let (initializers_a, ros_budget_a) =
            combined_initializers(&definitions_a, &context_a, &protobuf_budget_a);
        let (initializers_b, ros_budget_b) =
            combined_initializers(&definitions_b, &context_b, &protobuf_budget_b);
        let budget = RemoteDecoderAssignmentBudgetV1::new_disarmed_v1(
            assignment_limits(),
            1,
            1_000_000,
            1,
            1_000_000,
            u64::MAX,
        );
        let result_a = assign_for_test_v1(initializers_a, &budget).unwrap();
        let error = match assign_for_test_v1(initializers_b, &budget) {
            Ok(_result) => panic!("aggregate result capacity unexpectedly admitted twice"),
            Err(error) => error,
        };
        assert_eq!(
            error,
            RemoteDecoderAssignmentErrorV1::ResourceLimitExceeded(
                RemoteDecoderAssignmentResourceLimitV1::ReservationCapacity,
            )
        );
        assert!(ros_budget_b.is_idle_for_assignment_test_v1());
        assert!(protobuf_budget_b.is_idle_for_assignment_test_v1());
        assert_eq!(
            context_b
                .source
                .assignment_membership_projection_count_for_test_v1(),
            0,
            "a rejected prepare must not copy semantic-config membership",
        );
        assert_eq!(
            context_b.source.assignment_recognition_count_for_test_v1(),
            0
        );
        drop(result_a);
        assert!(ros_budget_a.is_idle_for_assignment_test_v1());
        assert!(protobuf_budget_a.is_idle_for_assignment_test_v1());
        assert_eq!(
            *budget.state.usage.lock(),
            RemoteDecoderAssignmentBudgetUsageV1::default()
        );
    }

    #[test]
    fn local_and_aggregate_working_limits_fail_before_any_recognition() {
        let exact_working = u64::try_from(std::mem::size_of::<AssignmentStepOwnerV1>()).unwrap();

        let aggregate_fixture = fixture([], [FixtureChannel::schema_less(1, "/raw")], [1]);
        let definitions = validated_summary_definitions_for_test(&aggregate_fixture);
        let context = StableContext::new();
        let protobuf_budget = context.protobuf_budget();
        let (initializers, ros_budget) =
            combined_initializers(&definitions, &context, &protobuf_budget);
        let zero_working_limits = UnfrozenRemoteDecoderAssignmentLimitsV1 {
            max_working_bytes: 0,
            ..assignment_limits()
        };
        let zero_working_budget = RemoteDecoderAssignmentBudgetV1::new_disarmed_v1(
            zero_working_limits,
            1,
            exact_working,
            1,
            1_000_000,
            u64::MAX,
        );
        let error = match assign_for_test_v1(initializers, &zero_working_budget) {
            Ok(_result) => panic!("zero local working limit unexpectedly ran recognition"),
            Err(error) => error,
        };
        assert_eq!(
            error,
            RemoteDecoderAssignmentErrorV1::ResourceLimitExceeded(
                RemoteDecoderAssignmentResourceLimitV1::WorkingBytes,
            )
        );
        assert_eq!(context.source.assignment_recognition_count_for_test_v1(), 0);
        assert_eq!(
            context
                .source
                .assignment_membership_projection_count_for_test_v1(),
            0,
        );
        assert!(ros_budget.is_idle_for_assignment_test_v1());
        assert!(protobuf_budget.is_idle_for_assignment_test_v1());
        assert_eq!(
            *zero_working_budget.state.usage.lock(),
            RemoteDecoderAssignmentBudgetUsageV1::default()
        );

        let contention_fixture = fixture([], [FixtureChannel::schema_less(1, "/raw")], [1]);
        let definitions = validated_summary_definitions_for_test(&contention_fixture);
        let context = StableContext::new();
        let protobuf_budget = context.protobuf_budget();
        let (initializers, ros_budget) =
            combined_initializers(&definitions, &context, &protobuf_budget);
        let aggregate_budget = RemoteDecoderAssignmentBudgetV1::new_disarmed_v1(
            UnfrozenRemoteDecoderAssignmentLimitsV1 {
                max_working_bytes: exact_working,
                ..assignment_limits()
            },
            2,
            exact_working,
            2,
            1_000_000,
            u64::MAX,
        );
        let blocker = aggregate_budget.reserve(exact_working, 0, 0).unwrap();
        let error = match assign_for_test_v1(initializers, &aggregate_budget) {
            Ok(_result) => panic!("aggregate working contention unexpectedly ran recognition"),
            Err(error) => error,
        };
        assert_eq!(
            error,
            RemoteDecoderAssignmentErrorV1::ResourceLimitExceeded(
                RemoteDecoderAssignmentResourceLimitV1::ReservationCapacity,
            )
        );
        assert_eq!(context.source.assignment_recognition_count_for_test_v1(), 0);
        assert_eq!(
            context
                .source
                .assignment_membership_projection_count_for_test_v1(),
            0,
        );
        assert!(ros_budget.is_idle_for_assignment_test_v1());
        assert!(protobuf_budget.is_idle_for_assignment_test_v1());
        drop(blocker);
        assert_eq!(
            *aggregate_budget.state.usage.lock(),
            RemoteDecoderAssignmentBudgetUsageV1::default()
        );
    }

    #[test]
    fn prepare_holds_the_complete_assignment_reservation_until_drop() {
        let fixture = fixture([], [FixtureChannel::schema_less(1, "/raw")], [1]);
        let definitions = validated_summary_definitions_for_test(&fixture);
        let context = StableContext::new();
        let protobuf_budget = context.protobuf_budget();
        let (initializers, ros_budget) =
            combined_initializers(&definitions, &context, &protobuf_budget);
        let assignment_budget = assignment_budget();

        let prepared =
            prepare_remote_decoder_eligibility_v1(initializers, &assignment_budget).unwrap();
        let usage = *assignment_budget.state.usage.lock();
        assert_eq!(usage.active_assignments, 1);
        assert_eq!(usage.retained_results, 1);
        assert_eq!(
            usage.working_bytes,
            u64::try_from(std::mem::size_of::<AssignmentStepOwnerV1>()).unwrap()
        );
        assert!(usage.retained_bytes > 0);
        assert!(usage.combined_retained_bytes >= usage.retained_bytes);
        assert_eq!(
            context
                .source
                .assignment_membership_projection_count_for_test_v1(),
            1
        );
        assert_eq!(context.source.assignment_recognition_count_for_test_v1(), 0);

        drop(prepared);
        assert_eq!(
            *assignment_budget.state.usage.lock(),
            RemoteDecoderAssignmentBudgetUsageV1::default()
        );
        assert!(ros_budget.is_idle_for_assignment_test_v1());
        assert!(protobuf_budget.is_idle_for_assignment_test_v1());
    }

    #[test]
    fn exact_total_step_owner_and_source_order_linear_merge_are_bounded() {
        const SELECTED_TOPICS: &[&str] = &["/selected-a", "/selected-b"];
        let mut channels = (1_u16..=62)
            .map(|id| FixtureChannel::schema_less(id, format!("/skip-{id}")))
            .collect::<Vec<_>>();
        channels.push(FixtureChannel::schema_less(63, SELECTED_TOPICS[0]));
        channels.push(FixtureChannel::schema_less(64, SELECTED_TOPICS[1]));
        let fixture = fixture([], channels, [63, 64]);
        let definitions = validated_summary_definitions_for_test(&fixture);
        let exact_steps = 64 + 2 * 3;

        for (step_limit, should_succeed) in [(exact_steps - 1, false), (exact_steps, true)] {
            let context = StableContext::new_with_selected_topics(SELECTED_TOPICS);
            let protobuf_budget = context.protobuf_budget();
            let (initializers, ros_budget) =
                combined_initializers(&definitions, &context, &protobuf_budget);
            let limits = UnfrozenRemoteDecoderAssignmentLimitsV1 {
                max_channels: 2,
                max_census_steps: step_limit,
                ..assignment_limits()
            };
            let budget = RemoteDecoderAssignmentBudgetV1::new_disarmed_v1(
                limits,
                1,
                1_000_000,
                1,
                1_000_000,
                u64::MAX,
            );
            let result = assign_for_test_v1(initializers, &budget);
            assert_eq!(result.is_ok(), should_succeed);
            if let Ok(result) = result {
                assert_eq!(
                    result
                        .assignments_for_test()
                        .iter()
                        .map(RemoteChannelDecoderAssignmentV1::channel_id)
                        .collect::<Vec<_>>(),
                    [63, 64],
                );
                assert_eq!(context.source.assignment_recognition_count_for_test_v1(), 2);
                drop(result);
            } else {
                assert_eq!(context.source.assignment_recognition_count_for_test_v1(), 0);
                assert_eq!(
                    context
                        .source
                        .assignment_membership_projection_count_for_test_v1(),
                    0,
                );
            }
            assert_eq!(
                *budget.state.usage.lock(),
                RemoteDecoderAssignmentBudgetUsageV1::default()
            );
            assert!(ros_budget.is_idle_for_assignment_test_v1());
            assert!(protobuf_budget.is_idle_for_assignment_test_v1());
        }
    }

    #[test]
    fn production_source_has_no_raw_or_unbounded_decoder_escape_hatch() {
        let source = include_str!("remote_decoder_assignment.rs");
        for forbidden in [
            "mcap::Summary",
            "collect_empty_channels",
            "mcap::records::Statistics",
            "read_message_indexes",
            "DescriptorPool",
            "MessageSchema::parse",
            "EntityDb",
            "TopicFilter",
            "binary_search",
            ".sort",
        ] {
            assert!(
                !source[..source.find("#[cfg(test)]").unwrap()].contains(forbidden),
                "assignment production path must not contain {forbidden}"
            );
        }
        assert!(EXACT_SAFE_SEMANTIC_ROS2_TABLE_V1.is_empty());
        assert!(!source.contains(concat!("pub(crate) fn ", "raw_assignments")));
    }

    #[test]
    fn immutable_groups_consume_real_assignment_and_retain_all_owner_reservations() {
        let fixture = fixture([], [FixtureChannel::schema_less(1, "/raw")], [1]);
        let definitions = validated_summary_definitions_for_test(&fixture);
        let context = StableContext::new();
        let protobuf_budget = context.protobuf_budget();
        let (initializers, ros_budget) =
            combined_initializers(&definitions, &context, &protobuf_budget);
        let assignment_budget = assignment_budget();
        let group_limits =
            crate::remote_channel_group::UnfrozenRemoteChannelGroupLimitsV1::generous_for_assignment_test_v1();
        let group_budget =
            crate::remote_channel_group::RemoteChannelGroupBudgetV1::new_for_assignment_test_v1(
                group_limits,
                1,
                1_000_000,
            );
        let groups =
            build_groups_for_test_v1(initializers, &assignment_budget, &group_budget).unwrap();
        assert!(!ros_budget.is_idle_for_assignment_test_v1());
        assert!(!protobuf_budget.is_idle_for_assignment_test_v1());
        assert_ne!(
            *assignment_budget.state.usage.lock(),
            RemoteDecoderAssignmentBudgetUsageV1::default()
        );
        assert!(groups.resolve_group(crate::remote_channel_group::StableDecoderGroupIdV1::first_for_assignment_test_v1()).is_some());
        drop(groups);
        assert!(ros_budget.is_idle_for_assignment_test_v1());
        assert!(protobuf_budget.is_idle_for_assignment_test_v1());
        assert_eq!(
            *assignment_budget.state.usage.lock(),
            RemoteDecoderAssignmentBudgetUsageV1::default()
        );
        assert!(group_budget.is_idle_for_assignment_test_v1());
    }

    #[test]
    fn immutable_group_combined_minus_one_rejects_real_build_before_group_allocation() {
        let fixture = fixture([], [FixtureChannel::schema_less(1, "/raw")], [1]);
        let definitions = validated_summary_definitions_for_test(&fixture);
        let context = StableContext::new();
        let protobuf_budget = context.protobuf_budget();
        let (initializers, ros_budget) =
            combined_initializers(&definitions, &context, &protobuf_budget);
        let assignment_budget = assignment_budget();
        let group_limits =
            crate::remote_channel_group::UnfrozenRemoteChannelGroupLimitsV1::generous_for_assignment_test_v1();
        let (working, retained) =
            crate::remote_channel_group::peak_bytes_for_assignment_test_v1(1).unwrap();
        let group_budget = crate::remote_channel_group::RemoteChannelGroupBudgetV1::new_for_test_v1(
            group_limits,
            1,
            working,
            retained,
            working.checked_add(retained).unwrap() - 1,
        );
        let error = match build_groups_for_test_v1(initializers, &assignment_budget, &group_budget)
        {
            Ok(_groups) => panic!("minus-one Channel-group budget unexpectedly built"),
            Err(error) => error,
        };
        assert_eq!(
            error,
            crate::remote_channel_group::RemoteChannelGroupErrorV1::ResourceLimitExceeded(
                crate::remote_channel_group::RemoteChannelGroupResourceLimitV1::ReservationCapacity,
            )
        );
        assert!(ros_budget.is_idle_for_assignment_test_v1());
        assert!(protobuf_budget.is_idle_for_assignment_test_v1());
        assert_eq!(
            *assignment_budget.state.usage.lock(),
            RemoteDecoderAssignmentBudgetUsageV1::default()
        );
        assert!(group_budget.is_idle_for_assignment_test_v1());
    }
}
