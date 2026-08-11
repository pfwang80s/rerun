//! Bounded ROS 2 reflection initialization for the Web remote-MCAP path.
//!
//! This module is deliberately crate-private and production-disarmed.
//! It never calls the allocation-bearing local [`re_ros_msg::MessageSchema`] parser.
//! Instead it validates every observed ROS 2 schema before the first allocation, then materializes
//! a fixed set of contiguous arenas containing only borrowed source spans.

#![allow(dead_code)]

#[cfg(all(target_arch = "wasm32", not(re_mcap_locked_remote_wasm_allocator_v1)))]
compile_error!("remote ROS 2 allocator accounting requires the locked release-Wasm artifact");

#[cfg(all(re_mcap_locked_remote_wasm_allocator_v1, not(test)))]
include!(concat!(
    env!("OUT_DIR"),
    "/re_mcap_remote_ros2_generated_capability_v1.rs"
));

#[cfg(all(re_mcap_locked_remote_wasm_allocator_v1, not(test)))]
const _: ReMcapRemoteRos2GeneratedCapabilityV1 = RE_MCAP_REMOTE_ROS2_GENERATED_CAPABILITY_V1;

use std::alloc::Layout;
use std::cell::Cell;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;

use crate::remote_summary::definitions::{
    CanonicalSchemaDefinition, SummaryDefinitionProjectionRecord, ValidatedSummaryDefinitions,
};

const ROS2_SCHEMA_ENCODING: &str = "ros2msg";
const REMOTE_POLICY_VERSION_V1: u16 = 1;
const REMOTE_ALLOWLIST_VERSION_V1: u16 = 1;
const REMOTE_GRAMMAR_VERSION_V1: u16 = 1;
const MAX_STACK_DEPENDENCY_DEPTH_V1: usize = 64;
// Rust 1.95.0 (59807616e1fa2540724bfbac14d7976d7e4a3860) vendors dlmalloc 0.2.10
// for wasm32 `System`. These constants are the wasm32 values used by that exact artifact.
const LOCKED_WASM_DLMALLOC_ALIGNMENT_V1: u64 = 8;
const LOCKED_WASM_DLMALLOC_CHUNK_OVERHEAD_V1: u64 = 4;
const LOCKED_WASM_DLMALLOC_MIN_CHUNK_V1: u64 = 16;
const LOCKED_WASM_DLMALLOC_TOP_FOOT_V1: u64 = 40;
const LOCKED_WASM_DLMALLOC_PAGE_V1: u64 = 64 * 1024;
// Frozen against the release-Wasm initializer probe by the Web artifact verifier.
const LOCKED_REMOTE_ROS2_FRAME_SPILL_CEILING_V1: u64 = 16 * 1024;
const SYNTHETIC_PADDING_NAME: &str = "structure_needs_at_least_one_member";
const CANONICAL_DECODER_IDENTITIES_V1: [&str; 4] = [
    "semantic_ros2:v1-empty",
    "ros2_reflection:v1",
    "protobuf:v1",
    "raw:v1",
];
const CANONICAL_FALLBACK_IDENTITY_V1: &str = "raw:v1";

macro_rules! remote_ros2_artifact_stage_anchor {
    ($name:ident, $identity:literal) => {
        #[cfg(target_arch = "wasm32")]
        #[inline(never)]
        #[unsafe(no_mangle)]
        pub(crate) extern "C" fn $name() -> u32 {
            std::hint::black_box($identity)
        }
    };
}

remote_ros2_artifact_stage_anchor!(rerun_remote_ros2_signature_stage_v1, 0x2601_u32);
remote_ros2_artifact_stage_anchor!(rerun_remote_ros2_census_stage_v1, 0x2602_u32);
remote_ros2_artifact_stage_anchor!(rerun_remote_ros2_name_stage_v1, 0x2603_u32);
remote_ros2_artifact_stage_anchor!(rerun_remote_ros2_complex_stage_v1, 0x2604_u32);
remote_ros2_artifact_stage_anchor!(rerun_remote_ros2_peak_stage_v1, 0x2605_u32);

#[cfg(target_arch = "wasm32")]
fn verify_artifact_stage_identity(actual: u32, expected: u32) {
    if actual != expected {
        remote_ros2_fatal_invariant("artifact stage identity changed");
    }
}

/// Allocation-free wire view of the policy identity supplied by the future remote route.
///
/// No production constructor exists while the route is disarmed.
pub(crate) struct RemoteDecoderPolicyWireV1<'wire> {
    allowlist_version: u16,
    assignment_version: u16,
    grammar_version: u16,
    decoder_identities: &'wire [&'wire str],
    fallback_identity: &'wire str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteDecoderPolicyError {
    UnknownVersion,
    NonCanonicalDecoderIdentity,
    NonCanonicalFallbackIdentity,
}

impl std::fmt::Display for RemoteDecoderPolicyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::UnknownVersion => "remote decoder policy version is unsupported",
            Self::NonCanonicalDecoderIdentity => {
                "remote decoder policy identities are not canonical"
            }
            Self::NonCanonicalFallbackIdentity => {
                "remote decoder fallback identity is not canonical"
            }
        })
    }
}

impl std::error::Error for RemoteDecoderPolicyError {}

/// Sealed, move-only proof of the exact V1 decoder policy.
pub(crate) struct FrozenRemoteDecoderPolicyV1<'wire> {
    wire: &'wire RemoteDecoderPolicyWireV1<'wire>,
}

pub(crate) fn freeze_remote_decoder_policy_v1<'wire>(
    wire: &'wire RemoteDecoderPolicyWireV1<'wire>,
) -> Result<FrozenRemoteDecoderPolicyV1<'wire>, RemoteDecoderPolicyError> {
    if wire.allowlist_version != REMOTE_ALLOWLIST_VERSION_V1
        || wire.assignment_version != REMOTE_POLICY_VERSION_V1
        || wire.grammar_version != REMOTE_GRAMMAR_VERSION_V1
    {
        return Err(RemoteDecoderPolicyError::UnknownVersion);
    }
    if wire.decoder_identities != CANONICAL_DECODER_IDENTITIES_V1 {
        return Err(RemoteDecoderPolicyError::NonCanonicalDecoderIdentity);
    }
    if wire.fallback_identity != CANONICAL_FALLBACK_IDENTITY_V1 {
        return Err(RemoteDecoderPolicyError::NonCanonicalFallbackIdentity);
    }
    Ok(FrozenRemoteDecoderPolicyV1 { wire })
}

/// Source-generation state owned by the future remote session.
///
/// `Cell` is intentional: this production-disarmed Web capability is confined to the Viewer
/// thread, and avoiding an exposed scalar token makes cross-source reconstruction impossible.
pub(crate) struct RemoteDefinitionsSourceState {
    generation: Cell<u64>,
    viewer_scope: *const RemoteViewerScopeState,
}

impl RemoteDefinitionsSourceState {
    fn is_current(&self, generation: u64) -> bool {
        self.generation.get() == generation
    }
}

pub(crate) struct RemoteViewerScopeState {
    _sealed_identity: u8,
}

pub(crate) struct RemoteRos2ProfileScopeV1 {
    _sealed_identity: u8,
}

/// A sealed view of MCAP-020 definitions tied to one live source generation.
pub(crate) struct RemoteDefinitionsCapability<'definitions, 'input, 'source> {
    definitions: &'definitions ValidatedSummaryDefinitions<'input>,
    source: &'source RemoteDefinitionsSourceState,
    generation: u64,
}

impl RemoteDefinitionsCapability<'_, '_, '_> {
    fn ensure_current(&self) -> Result<(), RemoteRos2InitializationError> {
        if self.source.is_current(self.generation) {
            Ok(())
        } else {
            Err(RemoteRos2InitializationError::StaleSource)
        }
    }
}

/// Sealed proof that all canonical Channels with the same topic have one decoder-relevant
/// signature under the exact source and policy.
pub(crate) struct RemoteDecoderTopicSignatureEvidenceV1<'definitions, 'input, 'source, 'wire> {
    owner: RemoteRos2AdmissionOwnerV1<'definitions, 'input, 'source, 'wire>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnsupportedForRemote {
    Ros2Wstring,
    GrammarFeature,
    DependencyGraph,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteRos2ResourceLimit {
    SchemaCount,
    DefinitionBytes,
    SpecificationCount,
    FieldCount,
    ConstantCount,
    MemberCount,
    LineBytes,
    TokenBytes,
    IdentifierBytes,
    DefaultBytes,
    StringLiteralBytes,
    StringBound,
    ArrayBound,
    DependencyEdges,
    DependencyDepth,
    SignatureSteps,
    ProjectionSteps,
    RecognitionSteps,
    CensusSteps,
    MaterializationSteps,
    RetainedBytes,
    WorkingBytes,
    ReservationCapacity,
    Arithmetic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteRos2InitializationError {
    InvalidRemoteSchema,
    UnsupportedForRemote(UnsupportedForRemote),
    ResourceLimitExceeded(RemoteRos2ResourceLimit),
    FallibleAllocationFailed,
    StaleSource,
    ConflictingTopicDecoderSignature,
}

impl std::fmt::Display for RemoteRos2InitializationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidRemoteSchema => "remote ROS 2 schema is invalid",
            Self::UnsupportedForRemote(UnsupportedForRemote::Ros2Wstring) => {
                "remote ROS 2 schema uses unsupported wstring data"
            }
            Self::UnsupportedForRemote(UnsupportedForRemote::GrammarFeature) => {
                "remote ROS 2 schema uses a grammar feature outside the V1 profile"
            }
            Self::UnsupportedForRemote(UnsupportedForRemote::DependencyGraph) => {
                "remote ROS 2 schema dependency graph is unsupported"
            }
            Self::ResourceLimitExceeded(_) => "remote ROS 2 schema exceeds a resource limit",
            Self::FallibleAllocationFailed => "remote ROS 2 schema allocation failed",
            Self::StaleSource => "remote ROS 2 source is stale",
            Self::ConflictingTopicDecoderSignature => {
                "remote MCAP topic has conflicting decoder signatures"
            }
        })
    }
}

impl std::error::Error for RemoteRos2InitializationError {}

#[cold]
#[track_caller]
fn remote_ros2_fatal_invariant(reason: &'static str) -> ! {
    panic!("Fatal remote ROS 2 initializer invariant: {reason}")
}

#[cold]
#[track_caller]
fn remote_ros2_fatal_control_plane(reason: &'static str) -> ! {
    panic!("Fatal remote ROS 2 initializer control-plane mismatch: {reason}")
}

pub(crate) fn preflight_remote_decoder_topic_signatures_v1<'definitions, 'input, 'source, 'wire>(
    mut owner: RemoteRos2AdmissionOwnerV1<'definitions, 'input, 'source, 'wire>,
) -> Result<
    RemoteDecoderTopicSignatureEvidenceV1<'definitions, 'input, 'source, 'wire>,
    RemoteRos2InitializationError,
> {
    #[cfg(target_arch = "wasm32")]
    verify_artifact_stage_identity(rerun_remote_ros2_signature_stage_v1(), 0x2601);
    owner.ensure_current()?;
    let definitions = owner.source.definitions;

    for left_record in definitions.projection_records() {
        owner.steps.consume()?;
        let SummaryDefinitionProjectionRecord::Channel {
            id: left_id,
            record_index: left_index,
        } = left_record
        else {
            continue;
        };
        if !is_canonical_channel_record_metered(definitions, left_index, left_id, &mut owner.steps)?
        {
            continue;
        }
        let left = definitions
            .channel_at_record(left_index)
            .unwrap_or_else(|| remote_ros2_fatal_invariant("channel projection changed kind"));

        for right_record in definitions.projection_records() {
            owner.steps.consume()?;
            let SummaryDefinitionProjectionRecord::Channel {
                id: right_id,
                record_index: right_index,
            } = right_record
            else {
                continue;
            };
            if right_index <= left_index
                || !is_canonical_channel_record_metered(
                    definitions,
                    right_index,
                    right_id,
                    &mut owner.steps,
                )?
            {
                continue;
            }
            let right = definitions
                .channel_at_record(right_index)
                .unwrap_or_else(|| remote_ros2_fatal_invariant("channel projection changed kind"));
            owner.steps.consume()?;
            if metered_str_eq(&left.topic, &right.topic, &mut owner.steps)?
                && !same_decoder_relevant_signature(definitions, left, right, &mut owner.steps)?
            {
                return Err(RemoteRos2InitializationError::ConflictingTopicDecoderSignature);
            }
        }
    }

    owner.ensure_current()?;
    Ok(RemoteDecoderTopicSignatureEvidenceV1 { owner })
}

fn is_canonical_channel_record_metered(
    definitions: &ValidatedSummaryDefinitions<'_>,
    record_index: usize,
    channel_id: u16,
    steps: &mut RemoteRos2StepOwnershipV1,
) -> Result<bool, RemoteRos2InitializationError> {
    for record in definitions.projection_records() {
        steps.consume()?;
        if matches!(
            record,
            SummaryDefinitionProjectionRecord::Channel {
                id,
                record_index: earlier,
            } if id == channel_id && earlier < record_index
        ) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn same_decoder_relevant_signature(
    definitions: &ValidatedSummaryDefinitions<'_>,
    left: &mcap::records::Channel,
    right: &mcap::records::Channel,
    steps: &mut RemoteRos2StepOwnershipV1,
) -> Result<bool, RemoteRos2InitializationError> {
    if !metered_str_eq(&left.message_encoding, &right.message_encoding, steps)? {
        return Ok(false);
    }
    match (
        canonical_channel_schema_metered(definitions, left, steps)?,
        canonical_channel_schema_metered(definitions, right, steps)?,
    ) {
        (None, None) => Ok(true),
        (Some(left), Some(right)) => {
            Ok(
                metered_str_eq(&left.header.name, &right.header.name, steps)?
                    && metered_str_eq(&left.header.encoding, &right.header.encoding, steps)?
                    && metered_bytes_eq(left.data, right.data, steps)?,
            )
        }
        (None, Some(_)) | (Some(_), None) => Ok(false),
    }
}

fn metered_bytes_eq(
    left: &[u8],
    right: &[u8],
    steps: &mut RemoteRos2StepOwnershipV1,
) -> Result<bool, RemoteRos2InitializationError> {
    steps.consume_bytes(left.len())?;
    steps.consume_bytes(right.len())?;
    Ok(left == right)
}

fn metered_str_eq(
    left: &str,
    right: &str,
    steps: &mut RemoteRos2StepOwnershipV1,
) -> Result<bool, RemoteRos2InitializationError> {
    metered_bytes_eq(left.as_bytes(), right.as_bytes(), steps)
}

fn canonical_channel_schema_metered<'a>(
    definitions: &'a ValidatedSummaryDefinitions<'_>,
    channel: &mcap::records::Channel,
    steps: &mut RemoteRos2StepOwnershipV1,
) -> Result<Option<CanonicalSchemaDefinition<'a>>, RemoteRos2InitializationError> {
    if channel.schema_id == 0 {
        return Ok(None);
    }
    for projection in definitions.projection_records() {
        steps.consume()?;
        if let SummaryDefinitionProjectionRecord::Schema { id, record_index } = projection
            && id == channel.schema_id
        {
            return Ok(definitions
                .schema_at_record(record_index)
                .map(Some)
                .unwrap_or_else(|| remote_ros2_fatal_invariant("schema projection changed kind")));
        }
    }
    remote_ros2_fatal_invariant("validated channel references no canonical schema")
}

/// Remote-only, measurement-profile limits.
///
/// The grammar version is fixed above; MCAP-088 may freeze values but cannot change syntax.
#[derive(Clone, Copy, Debug)]
pub(crate) struct UnfrozenRemoteRos2LimitsV1 {
    max_schemas: u64,
    max_definition_bytes: u64,
    max_specifications: u64,
    max_fields: u64,
    max_constants: u64,
    max_members: u64,
    max_line_bytes: u64,
    max_token_bytes: u64,
    max_identifier_bytes: u64,
    max_default_bytes: u64,
    max_string_literal_bytes: u64,
    max_string_bound: u64,
    max_array_bound: u64,
    max_dependency_edges: u64,
    max_dependency_depth: u64,
    max_signature_steps: u64,
    max_projection_steps: u64,
    max_recognition_steps: u64,
    max_census_steps: u64,
    max_materialization_steps: u64,
    max_retained_bytes: u64,
    max_working_bytes: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RemoteRos2BudgetUsage {
    active_initializers: u64,
    working_bytes: u64,
    retained_results: u64,
    retained_bytes: u64,
}

#[derive(Clone, Copy, Debug)]
struct RemoteRos2BudgetCapacity {
    max_active_initializers: u64,
    max_working_bytes: u64,
    max_retained_results: u64,
    max_retained_bytes: u64,
}

struct RemoteRos2BudgetState {
    limits: UnfrozenRemoteRos2LimitsV1,
    capacity: RemoteRos2BudgetCapacity,
    usage: Mutex<RemoteRos2BudgetUsage>,
    poisoned: AtomicBool,
    generation: u64,
    viewer_scope_identity: usize,
    profile_scope_identity: usize,
    policy_wire_identity: usize,
}

/// Source/profile budget for the complete initializer peak and retained parsed result.
pub(crate) struct RemoteRos2InitializationBudget<'source, 'wire> {
    state: Arc<RemoteRos2BudgetState>,
    source: &'source RemoteDefinitionsSourceState,
    generation: u64,
    viewer_scope: *const RemoteViewerScopeState,
    profile_scope: *const RemoteRos2ProfileScopeV1,
    policy_wire: &'wire RemoteDecoderPolicyWireV1<'wire>,
}

impl std::fmt::Debug for RemoteRos2InitializationBudget<'_, '_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RemoteRos2InitializationBudget")
            .field("limits", &self.state.limits)
            .field("capacity", &self.state.capacity)
            .finish_non_exhaustive()
    }
}

impl RemoteRos2InitializationBudget<'_, '_> {
    fn reserve(
        &self,
        working_bytes: u64,
        retained_bytes: u64,
    ) -> Result<RemoteRos2WorkReservation, RemoteRos2InitializationError> {
        self.state.reserve(working_bytes, retained_bytes)
    }
}

impl RemoteRos2BudgetState {
    fn reserve(
        self: &Arc<Self>,
        working_bytes: u64,
        retained_bytes: u64,
    ) -> Result<RemoteRos2WorkReservation, RemoteRos2InitializationError> {
        if self.poisoned.load(Ordering::Acquire) {
            remote_ros2_fatal_control_plane("initializer budget is poisoned");
        }
        let limits = &self.limits;
        if working_bytes > limits.max_working_bytes {
            return Err(RemoteRos2InitializationError::ResourceLimitExceeded(
                RemoteRos2ResourceLimit::WorkingBytes,
            ));
        }
        if retained_bytes > limits.max_retained_bytes {
            return Err(RemoteRos2InitializationError::ResourceLimitExceeded(
                RemoteRos2ResourceLimit::RetainedBytes,
            ));
        }

        let mut usage = self.usage.lock();
        let next = RemoteRos2BudgetUsage {
            active_initializers: checked_add(usage.active_initializers, 1)?,
            working_bytes: checked_add(usage.working_bytes, working_bytes)?,
            retained_results: checked_add(usage.retained_results, 1)?,
            retained_bytes: checked_add(usage.retained_bytes, retained_bytes)?,
        };
        let capacity = self.capacity;
        if next.active_initializers > capacity.max_active_initializers
            || next.working_bytes > capacity.max_working_bytes
            || next.retained_results > capacity.max_retained_results
            || next.retained_bytes > capacity.max_retained_bytes
        {
            return Err(RemoteRos2InitializationError::ResourceLimitExceeded(
                RemoteRos2ResourceLimit::ReservationCapacity,
            ));
        }
        *usage = next;
        drop(usage);
        Ok(RemoteRos2WorkReservation {
            state: Some(Arc::clone(self)),
            working_bytes,
            retained_bytes,
        })
    }
}

/// The single sealed owner for source generation, policy/profile, limits, budget, and steps.
///
/// Every later typestate moves this value. There is no API that accepts an unrelated budget after
/// admission, so a census or result cannot be rebound to another source or policy profile.
pub(crate) struct RemoteRos2AdmissionOwnerV1<'definitions, 'input, 'source, 'wire> {
    source: RemoteDefinitionsCapability<'definitions, 'input, 'source>,
    policy: FrozenRemoteDecoderPolicyV1<'wire>,
    budget_state: Arc<RemoteRos2BudgetState>,
    viewer_scope: *const RemoteViewerScopeState,
    profile_scope: *const RemoteRos2ProfileScopeV1,
    steps: RemoteRos2StepOwnershipV1,
    scratch: RemoteRos2FixedScratchV1,
}

impl RemoteRos2AdmissionOwnerV1<'_, '_, '_, '_> {
    fn ensure_current(&self) -> Result<(), RemoteRos2InitializationError> {
        self.source.ensure_current()?;
        if self.source.source.viewer_scope != self.viewer_scope
            || self.budget_state.generation != self.source.generation
            || self.budget_state.viewer_scope_identity != self.viewer_scope.addr()
            || self.budget_state.profile_scope_identity != self.profile_scope.addr()
            || self.budget_state.poisoned.load(Ordering::Acquire)
        {
            remote_ros2_fatal_control_plane("admission owner identity changed");
        }
        Ok(())
    }
}

struct RemoteRos2StepOwnershipV1 {
    remaining: u64,
    consumed: u64,
    limit_kind: RemoteRos2ResourceLimit,
}

impl RemoteRos2StepOwnershipV1 {
    fn with_limit(remaining: u64, limit_kind: RemoteRos2ResourceLimit) -> Self {
        Self {
            remaining,
            consumed: 0,
            limit_kind,
        }
    }

    fn consume(&mut self) -> Result<(), RemoteRos2InitializationError> {
        let Some(remaining) = self.remaining.checked_sub(1) else {
            return Err(RemoteRos2InitializationError::ResourceLimitExceeded(
                self.limit_kind,
            ));
        };
        self.remaining = remaining;
        self.consumed = checked_add(self.consumed, 1)?;
        Ok(())
    }

    fn step(&mut self) -> Result<(), RemoteRos2InitializationError> {
        self.consume()
    }

    fn consume_bytes(&mut self, bytes: usize) -> Result<(), RemoteRos2InitializationError> {
        for _byte in 0..bytes {
            self.consume()?;
        }
        Ok(())
    }

    fn ensure_exhausted(&self) -> Result<(), RemoteRos2InitializationError> {
        if self.remaining == 0 {
            Ok(())
        } else {
            remote_ros2_fatal_invariant("materialization did not consume its exact step owner")
        }
    }
}

pub(crate) fn begin_remote_ros2_admission_v1<'definitions, 'input, 'source, 'wire>(
    source: RemoteDefinitionsCapability<'definitions, 'input, 'source>,
    policy: FrozenRemoteDecoderPolicyV1<'wire>,
    budget: &RemoteRos2InitializationBudget<'source, 'wire>,
) -> Result<
    RemoteRos2AdmissionOwnerV1<'definitions, 'input, 'source, 'wire>,
    RemoteRos2InitializationError,
> {
    source.ensure_current()?;
    if !std::ptr::eq(source.source, budget.source)
        || source.generation != budget.generation
        || source.source.viewer_scope != budget.viewer_scope
        || !std::ptr::eq(policy.wire, budget.policy_wire)
        || budget.profile_scope.is_null()
        || budget.state.generation != budget.generation
        || budget.state.viewer_scope_identity != budget.viewer_scope.addr()
        || budget.state.profile_scope_identity != budget.profile_scope.addr()
        || budget.state.policy_wire_identity != std::ptr::from_ref(budget.policy_wire).addr()
    {
        remote_ros2_fatal_control_plane("source, policy, profile, or generation was rebound");
    }
    let max_signature_steps = budget.state.limits.max_signature_steps;
    Ok(RemoteRos2AdmissionOwnerV1 {
        source,
        policy,
        budget_state: Arc::clone(&budget.state),
        viewer_scope: budget.viewer_scope,
        profile_scope: budget.profile_scope,
        steps: RemoteRos2StepOwnershipV1::with_limit(
            max_signature_steps,
            RemoteRos2ResourceLimit::SignatureSteps,
        ),
        scratch: RemoteRos2FixedScratchV1::new(),
    })
}

struct RemoteRos2WorkReservation {
    state: Option<Arc<RemoteRos2BudgetState>>,
    working_bytes: u64,
    retained_bytes: u64,
}

impl RemoteRos2WorkReservation {
    fn narrow(&mut self, working_bytes: u64, retained_bytes: u64) {
        if working_bytes > self.working_bytes || retained_bytes > self.retained_bytes {
            if let Some(state) = &self.state {
                poison_invariant(state, "reservation widening attempted");
            }
            remote_ros2_fatal_invariant("completed reservation was reused");
        }
        let state = self
            .state
            .as_ref()
            .unwrap_or_else(|| remote_ros2_fatal_invariant("completed reservation was reused"));
        let mut usage = state.usage.lock();
        let next_working_bytes = usage
            .working_bytes
            .checked_sub(self.working_bytes - working_bytes)
            .unwrap_or_else(|| poison_invariant(state, "working reservation underflow"));
        let next_retained_bytes = usage
            .retained_bytes
            .checked_sub(self.retained_bytes - retained_bytes)
            .unwrap_or_else(|| poison_invariant(state, "retained reservation underflow"));
        usage.working_bytes = next_working_bytes;
        usage.retained_bytes = next_retained_bytes;
        self.working_bytes = working_bytes;
        self.retained_bytes = retained_bytes;
    }

    fn complete(mut self) -> RemoteRos2ResultReservation {
        let state = self
            .state
            .as_ref()
            .unwrap_or_else(|| remote_ros2_fatal_invariant("completed reservation was reused"));
        {
            let mut usage = state.usage.lock();
            let next_active_initializers = usage
                .active_initializers
                .checked_sub(1)
                .unwrap_or_else(|| poison_invariant(state, "active initializer underflow"));
            let next_working_bytes = usage
                .working_bytes
                .checked_sub(self.working_bytes)
                .unwrap_or_else(|| poison_invariant(state, "working reservation underflow"));
            usage.active_initializers = next_active_initializers;
            usage.working_bytes = next_working_bytes;
        }
        let state = self
            .state
            .take()
            .unwrap_or_else(|| remote_ros2_fatal_invariant("completed reservation was reused"));
        RemoteRos2ResultReservation {
            state,
            retained_bytes: self.retained_bytes,
        }
    }
}

impl Drop for RemoteRos2WorkReservation {
    fn drop(&mut self) {
        let Some(state) = self.state.take() else {
            return;
        };
        let mut usage = state.usage.lock();
        let Some(next) =
            checked_release_work_usage(*usage, self.working_bytes, self.retained_bytes)
        else {
            state.poisoned.store(true, Ordering::Release);
            return;
        };
        *usage = next;
    }
}

struct RemoteRos2ResultReservation {
    state: Arc<RemoteRos2BudgetState>,
    retained_bytes: u64,
}

impl Drop for RemoteRos2ResultReservation {
    fn drop(&mut self) {
        let mut usage = self.state.usage.lock();
        let Some(retained_results) = usage.retained_results.checked_sub(1) else {
            self.state.poisoned.store(true, Ordering::Release);
            return;
        };
        let Some(retained_bytes) = usage.retained_bytes.checked_sub(self.retained_bytes) else {
            self.state.poisoned.store(true, Ordering::Release);
            return;
        };
        usage.retained_results = retained_results;
        usage.retained_bytes = retained_bytes;
    }
}

fn poison_invariant(state: &RemoteRos2BudgetState, reason: &'static str) -> ! {
    state.poisoned.store(true, Ordering::Release);
    remote_ros2_fatal_control_plane(reason)
}

fn checked_release_work_usage(
    usage: RemoteRos2BudgetUsage,
    working_bytes: u64,
    retained_bytes: u64,
) -> Option<RemoteRos2BudgetUsage> {
    Some(RemoteRos2BudgetUsage {
        active_initializers: usage.active_initializers.checked_sub(1)?,
        working_bytes: usage.working_bytes.checked_sub(working_bytes)?,
        retained_results: usage.retained_results.checked_sub(1)?,
        retained_bytes: usage.retained_bytes.checked_sub(retained_bytes)?,
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RemoteRos2Census {
    schemas: u64,
    definition_bytes: u64,
    specifications: u64,
    fields: u64,
    constants: u64,
    members: u64,
    dependency_edges: u64,
    census_steps: u64,
    materialization_steps: u64,
    projection_steps: u64,
    recognition_steps: u64,
    retained_bytes: u64,
    working_bytes: u64,
}

/// Move-only token proving complete allocation-free grammar and dependency validation.
pub(crate) struct PreparedRemoteRos2CensusV1<'definitions, 'input, 'source, 'wire> {
    evidence: RemoteDecoderTopicSignatureEvidenceV1<'definitions, 'input, 'source, 'wire>,
    census: RemoteRos2Census,
    reservation: RemoteRos2WorkReservation,
    materialization_steps: RemoteRos2StepOwnershipV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SourceTextKind {
    SchemaName,
    SchemaData,
    SyntheticPadding,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SourceTextSpan {
    schema_record_index: usize,
    kind: SourceTextKind,
    start: u32,
    len: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PrimitiveTypeV1 {
    Bool,
    Byte,
    Char,
    Float32,
    Float64,
    Int8,
    Int16,
    Int32,
    Int64,
    UInt8,
    UInt16,
    UInt32,
    UInt64,
    String { bound: Option<u64> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ElementTypeV1 {
    Primitive(PrimitiveTypeV1),
    Complex(SourceTextSpan),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArraySizeV1 {
    Scalar,
    Fixed(u64),
    Bounded(u64),
    Unbounded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ParsedTypeV1 {
    element: ElementTypeV1,
    array: ArraySizeV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ParsedMemberKindV1 {
    Field,
    Constant,
    SyntheticPadding,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ParsedMemberV1 {
    specification_index: usize,
    kind: ParsedMemberKindV1,
    name: SourceTextSpan,
    ty: ParsedTypeV1,
    literal: Option<SourceTextSpan>,
}

#[derive(Debug, PartialEq, Eq)]
struct ParsedSpecificationV1 {
    schema_index: usize,
    name: SourceTextSpan,
    members: Range<usize>,
}

#[derive(Debug, PartialEq, Eq)]
struct ParsedSchemaV1 {
    schema_id: u16,
    schema_record_index: usize,
    specifications: Range<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ComplexTypeResolutionV1 {
    member_index: usize,
    target_specification_index: usize,
}

/// Sealed bounded representation consumed by later remote decoder stages.
pub(crate) struct Ros2InitializedRemoteDefinitionsV1<'definitions, 'input, 'source, 'wire> {
    source: RemoteDefinitionsCapability<'definitions, 'input, 'source>,
    policy: FrozenRemoteDecoderPolicyV1<'wire>,
    viewer_scope: *const RemoteViewerScopeState,
    profile_scope: *const RemoteRos2ProfileScopeV1,
    schemas: FixedArena<ParsedSchemaV1>,
    specifications: FixedArena<ParsedSpecificationV1>,
    members: FixedArena<ParsedMemberV1>,
    resolutions: FixedArena<ComplexTypeResolutionV1>,
    projection_steps: u64,
    recognition_steps: u64,
    reservation: RemoteRos2ResultReservation,
}

mod transition_payload {
    use super::Ros2InitializedRemoteDefinitionsV1;

    /// Sealed payload shared only through the parent module's move-only APIs.
    ///
    /// MCAP-027 and MCAP-028 are crate-root siblings of this module, so Rust privacy prevents
    /// either from reaching the raw definitions or replenishing an internal step owner.
    pub(super) struct Payload<'definitions, 'input, 'source, 'wire> {
        pub(super) initialized:
            Ros2InitializedRemoteDefinitionsV1<'definitions, 'input, 'source, 'wire>,
    }
}

/// The only MCAP-026 → MCAP-027 handoff.
///
/// MCAP-027 is a sibling module and consumes this value by move. The nested sealed payload
/// preserves the exact definitions borrow, source generation, policy, ROS result, reservation,
/// and one-shot recognition owner without exposing a raw definitions map.
pub(crate) struct RemoteRos2ToProtobufTransitionV1<'definitions, 'input, 'source, 'wire> {
    payload: transition_payload::Payload<'definitions, 'input, 'source, 'wire>,
}

impl<'definitions, 'input, 'source, 'wire>
    Ros2InitializedRemoteDefinitionsV1<'definitions, 'input, 'source, 'wire>
{
    pub(crate) fn into_protobuf_transition_v1(
        self,
    ) -> Result<
        RemoteRos2ToProtobufTransitionV1<'definitions, 'input, 'source, 'wire>,
        RemoteRos2InitializationError,
    > {
        self.source.ensure_current()?;
        Ok(RemoteRos2ToProtobufTransitionV1 {
            payload: transition_payload::Payload { initialized: self },
        })
    }
}

impl<'definitions, 'input, 'source, 'wire>
    RemoteRos2ToProtobufTransitionV1<'definitions, 'input, 'source, 'wire>
{
    fn ensure_current(&self) -> Result<(), RemoteRos2InitializationError> {
        let initialized = &self.payload.initialized;
        initialized.source.ensure_current()?;
        let state = &initialized.reservation.state;
        if initialized.source.source.viewer_scope != initialized.viewer_scope
            || state.generation != initialized.source.generation
            || state.viewer_scope_identity != initialized.viewer_scope.addr()
            || initialized.profile_scope.is_null()
            || state.profile_scope_identity != initialized.profile_scope.addr()
            || state.policy_wire_identity != std::ptr::from_ref(initialized.policy.wire).addr()
            || state.poisoned.load(Ordering::Acquire)
        {
            remote_ros2_fatal_control_plane(
                "protobuf transition source, policy, profile, or generation changed",
            );
        }
        Ok(())
    }

    fn ros2_schema_count(&self) -> usize {
        self.payload.initialized.schemas.len()
    }

    /// The only bounded borrow that MCAP-027 can use to inspect the admitted ROS projection.
    ///
    /// The iterator cannot outlive or detach from this move-only transition, and every operation
    /// revalidates the exact source generation. It never exposes the raw summary definitions.
    pub(crate) fn projection_iter_v1(
        &self,
    ) -> Result<RemoteRos2ProjectionIterV1<'_, '_, '_, '_, '_>, RemoteRos2InitializationError> {
        self.ensure_current()?;
        Ok(RemoteRos2ProjectionIterV1 {
            transition: self,
            next_schema_index: 0,
        })
    }

    /// Bounded borrowed protobuf-schema projection for the MCAP-027 sibling module.
    ///
    /// The sibling receives neither `ValidatedSummaryDefinitions` nor a raw Summary. Every yielded
    /// header/data borrow stays tied to this transition and revalidates the source generation.
    pub(crate) fn into_protobuf_projection_v1(
        self,
    ) -> Result<
        RemoteProtobufProjectionOwnerV1<'definitions, 'input, 'source, 'wire>,
        RemoteRos2InitializationError,
    > {
        self.ensure_current()?;
        let remaining_steps = self.payload.initialized.projection_steps;
        Ok(RemoteProtobufProjectionOwnerV1 {
            transition: self,
            next_record_index: 0,
            remaining_steps,
            reached_eof: false,
        })
    }
}

pub(crate) struct RemoteRos2ChannelRecognitionV1<
    'item,
    'borrow,
    'definitions,
    'input,
    'source,
    'wire,
> {
    owner: &'item RemoteRos2RecognitionIterV1<'borrow, 'definitions, 'input, 'source, 'wire>,
    channel_record_index: usize,
    recognized_by_reflection: bool,
}

impl RemoteRos2ChannelRecognitionV1<'_, '_, '_, '_, '_, '_> {
    pub(crate) fn channel_id(&self) -> Result<u16, RemoteRos2InitializationError> {
        self.owner.transition.ensure_current()?;
        let definitions = self.owner.transition.payload.initialized.source.definitions;
        match definitions.projection_record_at(self.channel_record_index) {
            Some(SummaryDefinitionProjectionRecord::Channel { id, .. }) => Ok(id),
            _ => remote_ros2_fatal_invariant("recognized channel projection changed kind"),
        }
    }

    pub(crate) fn recognized_by_reflection(&self) -> Result<bool, RemoteRos2InitializationError> {
        self.owner.transition.ensure_current()?;
        Ok(self.recognized_by_reflection)
    }
}

/// One-shot, source/policy-bound recognition capability for MCAP-028.
pub(crate) struct RemoteRos2RecognitionIterV1<'borrow, 'definitions, 'input, 'source, 'wire> {
    transition: &'borrow RemoteRos2ToProtobufTransitionV1<'definitions, 'input, 'source, 'wire>,
    next_record_index: usize,
    steps: RemoteRos2StepOwnershipV1,
}

impl<'borrow, 'definitions, 'input, 'source, 'wire>
    RemoteRos2RecognitionIterV1<'borrow, 'definitions, 'input, 'source, 'wire>
{
    pub(crate) fn next_channel<'item>(
        &'item mut self,
    ) -> Result<
        Option<
            RemoteRos2ChannelRecognitionV1<'item, 'borrow, 'definitions, 'input, 'source, 'wire>,
        >,
        RemoteRos2InitializationError,
    > {
        self.transition.ensure_current()?;
        let definitions = self.transition.payload.initialized.source.definitions;
        while self.next_record_index < definitions.source_record_count() {
            self.steps.consume()?;
            let record_index = self.next_record_index;
            self.next_record_index = self.next_record_index.checked_add(1).ok_or(
                RemoteRos2InitializationError::ResourceLimitExceeded(
                    RemoteRos2ResourceLimit::Arithmetic,
                ),
            )?;
            let Some(SummaryDefinitionProjectionRecord::Channel { id, .. }) =
                definitions.projection_record_at(record_index)
            else {
                continue;
            };
            if !is_canonical_channel_record_metered(definitions, record_index, id, &mut self.steps)?
            {
                continue;
            }
            let channel = definitions
                .channel_at_record(record_index)
                .unwrap_or_else(|| {
                    remote_ros2_fatal_invariant("channel recognition projection changed kind")
                });
            let schema = canonical_channel_schema_metered(definitions, channel, &mut self.steps)?;
            self.steps.consume_bytes(channel.message_encoding.len())?;
            let recognized_by_reflection = if !channel.message_encoding.eq_ignore_ascii_case("cdr")
            {
                false
            } else if let Some(schema) = schema {
                self.steps.consume_bytes(schema.header.encoding.len())?;
                self.steps
                    .consume_bytes(self.transition.payload.initialized.schemas.len())?;
                schema.header.encoding == ROS2_SCHEMA_ENCODING
                    && self
                        .transition
                        .payload
                        .initialized
                        .schemas
                        .as_slice()
                        .iter()
                        .any(|parsed| parsed.schema_id == schema.header.id)
            } else {
                false
            };
            return Ok(Some(RemoteRos2ChannelRecognitionV1 {
                owner: self,
                channel_record_index: record_index,
                recognized_by_reflection,
            }));
        }
        self.steps.ensure_exhausted()?;
        Ok(None)
    }
}

pub(crate) struct RemoteProtobufProjectionOwnerV1<'definitions, 'input, 'source, 'wire> {
    transition: RemoteRos2ToProtobufTransitionV1<'definitions, 'input, 'source, 'wire>,
    next_record_index: usize,
    remaining_steps: u64,
    reached_eof: bool,
}

/// Move-only proof that the exact projection owner reached EOF.
pub(crate) struct RemoteRos2ProjectionEofAuthorityV1<'definitions, 'input, 'source, 'wire> {
    transition: RemoteRos2ToProtobufTransitionV1<'definitions, 'input, 'source, 'wire>,
    recognition_steps: Option<RemoteRos2StepOwnershipV1>,
}

impl<'definitions, 'input, 'source, 'wire>
    RemoteRos2ProjectionEofAuthorityV1<'definitions, 'input, 'source, 'wire>
{
    /// Borrows recognition from the still-bound EOF owner without exposing its transition.
    #[cfg(test)]
    pub(crate) fn take_bound_recognition_for_test_v1(
        &mut self,
    ) -> Result<
        RemoteRos2RecognitionIterV1<'_, 'definitions, 'input, 'source, 'wire>,
        RemoteRos2InitializationError,
    > {
        self.transition.ensure_current()?;
        let steps = self.recognition_steps.take().ok_or(
            RemoteRos2InitializationError::ResourceLimitExceeded(
                RemoteRos2ResourceLimit::RecognitionSteps,
            ),
        )?;
        Ok(RemoteRos2RecognitionIterV1 {
            transition: &self.transition,
            next_record_index: 0,
            steps,
        })
    }

    #[cfg(test)]
    pub(crate) fn ensure_current_for_test_v1(&self) -> Result<(), RemoteRos2InitializationError> {
        self.transition.ensure_current()
    }
}

impl<'definitions, 'input, 'source, 'wire>
    RemoteProtobufProjectionOwnerV1<'definitions, 'input, 'source, 'wire>
{
    fn consume_step(&mut self) -> Result<(), RemoteRos2InitializationError> {
        self.remaining_steps = self.remaining_steps.checked_sub(1).ok_or(
            RemoteRos2InitializationError::ResourceLimitExceeded(
                RemoteRos2ResourceLimit::ProjectionSteps,
            ),
        )?;
        Ok(())
    }

    pub(crate) fn next_schema(
        &mut self,
    ) -> Result<
        Option<RemoteProtobufSchemaProjectionV1<'_, 'definitions, 'input, 'source, 'wire>>,
        RemoteRos2InitializationError,
    > {
        self.transition.ensure_current()?;
        let definitions = self.transition.payload.initialized.source.definitions;
        while self.next_record_index < definitions.source_record_count() {
            self.consume_step()?;
            let record_index = self.next_record_index;
            self.next_record_index = self.next_record_index.checked_add(1).ok_or(
                RemoteRos2InitializationError::ResourceLimitExceeded(
                    RemoteRos2ResourceLimit::Arithmetic,
                ),
            )?;
            let Some(SummaryDefinitionProjectionRecord::Schema { id, .. }) =
                definitions.projection_record_at(record_index)
            else {
                continue;
            };
            let mut canonical = true;
            for earlier in 0..record_index {
                self.consume_step()?;
                if matches!(
                    definitions.projection_record_at(earlier),
                    Some(SummaryDefinitionProjectionRecord::Schema { id: earlier_id, .. })
                        if earlier_id == id
                ) {
                    canonical = false;
                    break;
                }
            }
            if !canonical {
                continue;
            }
            let schema = definitions
                .schema_at_record(record_index)
                .unwrap_or_else(|| remote_ros2_fatal_invariant("schema projection changed kind"));
            if schema.header.encoding != "protobuf" {
                continue;
            }
            let mut observed = false;
            for channel_index in 0..definitions.source_record_count() {
                self.consume_step()?;
                let Some(SummaryDefinitionProjectionRecord::Channel { id: channel_id, .. }) =
                    definitions.projection_record_at(channel_index)
                else {
                    continue;
                };
                let mut canonical_channel = true;
                for earlier in 0..channel_index {
                    self.consume_step()?;
                    if matches!(
                        definitions.projection_record_at(earlier),
                        Some(SummaryDefinitionProjectionRecord::Channel { id, .. })
                            if id == channel_id
                    ) {
                        canonical_channel = false;
                        break;
                    }
                }
                if canonical_channel
                    && definitions
                        .channel_at_record(channel_index)
                        .is_some_and(|channel| channel.schema_id == id)
                {
                    observed = true;
                    break;
                }
            }
            if observed {
                return Ok(Some(RemoteProtobufSchemaProjectionV1 {
                    owner: self,
                    schema_record_index: record_index,
                }));
            }
        }
        self.reached_eof = true;
        Ok(None)
    }

    /// Finishes the projection only after the exact owner reached EOF.
    ///
    /// The returned move-only proof contains no result slot. Only the protobuf boundary can bind
    /// it to the result produced while consuming this owner.
    pub(crate) fn finish_eof_v1(
        self,
    ) -> Result<
        crate::remote_protobuf_projection_boundary::RemoteProtobufProjectionEofContinuationV1<
            'definitions,
            'input,
            'source,
            'wire,
        >,
        RemoteRos2InitializationError,
    > {
        self.transition.ensure_current()?;
        if !self.reached_eof {
            remote_ros2_fatal_control_plane("protobuf projection completed before EOF");
        }
        let recognition_steps = self.transition.payload.initialized.recognition_steps;
        let authority = RemoteRos2ProjectionEofAuthorityV1 {
            transition: self.transition,
            recognition_steps: Some(RemoteRos2StepOwnershipV1::with_limit(
                recognition_steps,
                RemoteRos2ResourceLimit::RecognitionSteps,
            )),
        };
        Ok(
            crate::remote_protobuf_projection_boundary::seal_remote_ros2_projection_eof_v1(
                authority,
            ),
        )
    }
}

pub(crate) struct RemoteProtobufSchemaProjectionV1<'borrow, 'definitions, 'input, 'source, 'wire> {
    owner: &'borrow RemoteProtobufProjectionOwnerV1<'definitions, 'input, 'source, 'wire>,
    schema_record_index: usize,
}

impl RemoteProtobufSchemaProjectionV1<'_, '_, '_, '_, '_> {
    fn schema(&self) -> Result<CanonicalSchemaDefinition<'_>, RemoteRos2InitializationError> {
        self.owner.transition.ensure_current()?;
        self.owner
            .transition
            .payload
            .initialized
            .source
            .definitions
            .schema_at_record(self.schema_record_index)
            .ok_or_else(|| remote_ros2_fatal_invariant("protobuf projection changed kind"))
    }

    pub(crate) fn schema_id(&self) -> Result<u16, RemoteRos2InitializationError> {
        Ok(self.schema()?.header.id)
    }

    pub(crate) fn name(&self) -> Result<&str, RemoteRos2InitializationError> {
        Ok(self.schema()?.header.name.as_str())
    }

    pub(crate) fn encoding(&self) -> Result<&str, RemoteRos2InitializationError> {
        Ok(self.schema()?.header.encoding.as_str())
    }

    pub(crate) fn data(&self) -> Result<&[u8], RemoteRos2InitializationError> {
        Ok(self.schema()?.data)
    }
}

pub(crate) struct RemoteRos2ProjectionIterV1<'borrow, 'definitions, 'input, 'source, 'wire> {
    transition: &'borrow RemoteRos2ToProtobufTransitionV1<'definitions, 'input, 'source, 'wire>,
    next_schema_index: usize,
}

impl<'borrow, 'definitions, 'input, 'source, 'wire>
    RemoteRos2ProjectionIterV1<'borrow, 'definitions, 'input, 'source, 'wire>
{
    pub(crate) fn next_schema(
        &mut self,
    ) -> Result<
        Option<RemoteRos2SchemaProjectionV1<'borrow, 'definitions, 'input, 'source, 'wire>>,
        RemoteRos2InitializationError,
    > {
        self.transition.ensure_current()?;
        if self.next_schema_index >= self.transition.payload.initialized.schemas.len() {
            return Ok(None);
        }
        let schema_index = self.next_schema_index;
        self.next_schema_index = self.next_schema_index.checked_add(1).ok_or(
            RemoteRos2InitializationError::ResourceLimitExceeded(
                RemoteRos2ResourceLimit::Arithmetic,
            ),
        )?;
        Ok(Some(RemoteRos2SchemaProjectionV1 {
            transition: self.transition,
            schema_index,
        }))
    }
}

pub(crate) struct RemoteRos2SchemaProjectionV1<'borrow, 'definitions, 'input, 'source, 'wire> {
    transition: &'borrow RemoteRos2ToProtobufTransitionV1<'definitions, 'input, 'source, 'wire>,
    schema_index: usize,
}

impl RemoteRos2SchemaProjectionV1<'_, '_, '_, '_, '_> {
    pub(crate) fn schema_id(&self) -> Result<u16, RemoteRos2InitializationError> {
        self.transition.ensure_current()?;
        Ok(self
            .transition
            .payload
            .initialized
            .schemas
            .get(self.schema_index)?
            .schema_id)
    }

    pub(crate) fn specification_count(&self) -> Result<usize, RemoteRos2InitializationError> {
        self.transition.ensure_current()?;
        let schema = self
            .transition
            .payload
            .initialized
            .schemas
            .get(self.schema_index)?;
        schema
            .specifications
            .end
            .checked_sub(schema.specifications.start)
            .ok_or_else(|| remote_ros2_fatal_invariant("specification range is reversed"))
    }

    pub(crate) fn specification_name(
        &self,
        local_index: usize,
    ) -> Result<&str, RemoteRos2InitializationError> {
        self.transition.ensure_current()?;
        let schema = self
            .transition
            .payload
            .initialized
            .schemas
            .get(self.schema_index)?;
        let index = schema.specifications.start.checked_add(local_index).ok_or(
            RemoteRos2InitializationError::ResourceLimitExceeded(
                RemoteRos2ResourceLimit::Arithmetic,
            ),
        )?;
        if index >= schema.specifications.end {
            remote_ros2_fatal_invariant("specification projection index is out of range");
        }
        let specification = self
            .transition
            .payload
            .initialized
            .specifications
            .get(index)?;
        text_for_span(
            self.transition.payload.initialized.source.definitions,
            specification.name,
        )
    }
}

#[derive(Clone, Copy)]
struct SchemaView<'a> {
    id: u16,
    record_index: usize,
    name: &'a str,
    data: &'a str,
}

#[derive(Clone, Copy)]
struct SpecView {
    name: SourceTextSpan,
    body_start: usize,
    body_end: usize,
}

#[derive(Clone, Copy)]
struct MemberView {
    kind: ParsedMemberKindV1,
    name: SourceTextSpan,
    ty: ParsedTypeV1,
    literal: Option<SourceTextSpan>,
}

#[derive(Clone, Copy)]
struct DependencyDfsFrameV1 {
    specification_index: usize,
    next_member_index: usize,
}

/// Fixed, allocation-free scratch tied to the sealed admission owner.
///
/// Large bounded traversal state lives here instead of in an unaccounted function frame.
/// Remaining scalar frames and compiler spills are checked by the locked release-Wasm verifier.
struct RemoteRos2FixedScratchV1 {
    dependency_frames: [DependencyDfsFrameV1; MAX_STACK_DEPENDENCY_DEPTH_V1 + 1],
}

impl RemoteRos2FixedScratchV1 {
    fn new() -> Self {
        Self {
            dependency_frames: [DependencyDfsFrameV1 {
                specification_index: usize::MAX,
                next_member_index: 0,
            }; MAX_STACK_DEPENDENCY_DEPTH_V1 + 1],
        }
    }
}

#[derive(Clone, Copy)]
struct TextSlice<'a> {
    text: &'a str,
    absolute_start: usize,
}

fn checked_add(left: u64, right: u64) -> Result<u64, RemoteRos2InitializationError> {
    left.checked_add(right)
        .ok_or(RemoteRos2InitializationError::ResourceLimitExceeded(
            RemoteRos2ResourceLimit::Arithmetic,
        ))
}

fn checked_usize(value: u64) -> Result<usize, RemoteRos2InitializationError> {
    usize::try_from(value).map_err(|_overflow| {
        RemoteRos2InitializationError::ResourceLimitExceeded(RemoteRos2ResourceLimit::Arithmetic)
    })
}

fn checked_u32(value: usize) -> Result<u32, RemoteRos2InitializationError> {
    u32::try_from(value).map_err(|_overflow| {
        RemoteRos2InitializationError::ResourceLimitExceeded(RemoteRos2ResourceLimit::Arithmetic)
    })
}

fn increment_limited(
    value: &mut u64,
    increment: u64,
    limit: u64,
    kind: RemoteRos2ResourceLimit,
) -> Result<(), RemoteRos2InitializationError> {
    let next = checked_add(*value, increment)?;
    if next > limit {
        return Err(RemoteRos2InitializationError::ResourceLimitExceeded(kind));
    }
    *value = next;
    Ok(())
}

fn align_up_checked(value: u64, alignment: u64) -> Result<u64, RemoteRos2InitializationError> {
    let padded = value.checked_add(alignment - 1).ok_or(
        RemoteRos2InitializationError::ResourceLimitExceeded(RemoteRos2ResourceLimit::Arithmetic),
    )?;
    Ok((padded / alignment) * alignment)
}

/// Upper bound on the wasm linear-memory footprint attributable to one allocation.
///
/// This is tied to the release artifact, not an empirical granule:
///
/// * Rust 1.95.0 `RawVec::try_reserve_exact` passes the exact array `Layout` to `System`;
/// * wasm32 `System` in commit `59807616e…` calls bundled dlmalloc 0.2.10;
/// * normal allocations use `request2size = max(16, align8(request + 4))`;
/// * alignments above 8 use dlmalloc's `memalign` over-allocation branch;
/// * a cache miss can grow linear memory by
///   `align64KiB(chunk + top_foot(40) + malloc_alignment(8))`.
///
/// Charging the whole possible `memory.grow` increment per arena also covers allocator metadata,
/// top-chunk slack, and fragmentation without relying on another session's reusable free chunks.
fn locked_wasm_allocation_footprint_v1(
    layout: Layout,
) -> Result<u64, RemoteRos2InitializationError> {
    let requested = u64::try_from(layout.size()).map_err(|_overflow| {
        RemoteRos2InitializationError::ResourceLimitExceeded(RemoteRos2ResourceLimit::Arithmetic)
    })?;
    if requested == 0 {
        return Ok(0);
    }
    let alignment = u64::try_from(layout.align()).map_err(|_overflow| {
        RemoteRos2InitializationError::ResourceLimitExceeded(RemoteRos2ResourceLimit::Arithmetic)
    })?;
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

fn arena_allocation_footprint<T>(count: u64) -> Result<u64, RemoteRos2InitializationError> {
    let count = checked_usize(count)?;
    let layout = Layout::array::<T>(count).map_err(|_overflow| {
        RemoteRos2InitializationError::ResourceLimitExceeded(RemoteRos2ResourceLimit::Arithmetic)
    })?;
    locked_wasm_allocation_footprint_v1(layout)
}

fn for_each_canonical_ros2_schema<'definitions, 'input>(
    definitions: &'definitions ValidatedSummaryDefinitions<'input>,
    steps: &mut RemoteRos2StepOwnershipV1,
    mut visit: impl FnMut(
        SchemaView<'definitions>,
        &mut RemoteRos2StepOwnershipV1,
    ) -> Result<(), RemoteRos2InitializationError>,
) -> Result<(), RemoteRos2InitializationError> {
    for projection in definitions.projection_records() {
        steps.consume()?;
        let SummaryDefinitionProjectionRecord::Schema { id, record_index } = projection else {
            continue;
        };
        if !is_canonical_schema_record_metered(definitions, record_index, id, steps)?
            || !schema_is_observed_by_canonical_channel_metered(definitions, id, steps)?
        {
            continue;
        }
        let schema = definitions
            .schema_at_record(record_index)
            .unwrap_or_else(|| remote_ros2_fatal_invariant("schema projection changed kind"));
        steps.consume_bytes(schema.header.encoding.len())?;
        if schema.header.encoding != ROS2_SCHEMA_ENCODING {
            continue;
        }
        let name = schema.header.name.as_str();
        steps.consume_bytes(schema.data.len())?;
        let data = std::str::from_utf8(schema.data)
            .map_err(|_utf8| RemoteRos2InitializationError::InvalidRemoteSchema)?;
        visit(
            SchemaView {
                id,
                record_index,
                name,
                data,
            },
            steps,
        )?;
    }
    Ok(())
}

fn is_canonical_schema_record_metered(
    definitions: &ValidatedSummaryDefinitions<'_>,
    record_index: usize,
    schema_id: u16,
    steps: &mut RemoteRos2StepOwnershipV1,
) -> Result<bool, RemoteRos2InitializationError> {
    for projection in definitions.projection_records() {
        steps.consume()?;
        if matches!(
            projection,
            SummaryDefinitionProjectionRecord::Schema {
                id,
                record_index: earlier,
            } if id == schema_id && earlier < record_index
        ) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn schema_is_observed_by_canonical_channel_metered(
    definitions: &ValidatedSummaryDefinitions<'_>,
    schema_id: u16,
    steps: &mut RemoteRos2StepOwnershipV1,
) -> Result<bool, RemoteRos2InitializationError> {
    for projection in definitions.projection_records() {
        steps.consume()?;
        let SummaryDefinitionProjectionRecord::Channel { id, record_index } = projection else {
            continue;
        };
        if is_canonical_channel_record_metered(definitions, record_index, id, steps)?
            && definitions
                .channel_at_record(record_index)
                .is_some_and(|channel| channel.schema_id == schema_id)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn span_from_data(
    schema: SchemaView<'_>,
    start: usize,
    len: usize,
) -> Result<SourceTextSpan, RemoteRos2InitializationError> {
    let end =
        start
            .checked_add(len)
            .ok_or(RemoteRos2InitializationError::ResourceLimitExceeded(
                RemoteRos2ResourceLimit::Arithmetic,
            ))?;
    if end > schema.data.len() {
        return Err(RemoteRos2InitializationError::InvalidRemoteSchema);
    }
    Ok(SourceTextSpan {
        schema_record_index: schema.record_index,
        kind: SourceTextKind::SchemaData,
        start: checked_u32(start)?,
        len: checked_u32(len)?,
    })
}

fn schema_name_span(
    schema: SchemaView<'_>,
) -> Result<SourceTextSpan, RemoteRos2InitializationError> {
    Ok(SourceTextSpan {
        schema_record_index: schema.record_index,
        kind: SourceTextKind::SchemaName,
        start: 0,
        len: checked_u32(schema.name.len())?,
    })
}

fn synthetic_padding_span(
    schema: SchemaView<'_>,
) -> Result<SourceTextSpan, RemoteRos2InitializationError> {
    Ok(SourceTextSpan {
        schema_record_index: schema.record_index,
        kind: SourceTextKind::SyntheticPadding,
        start: 0,
        len: checked_u32(SYNTHETIC_PADDING_NAME.len())?,
    })
}

#[expect(
    unsafe_code,
    reason = "the sealed census has already validated the complete source schema as UTF-8"
)]
fn text_for_span<'a>(
    definitions: &'a ValidatedSummaryDefinitions<'_>,
    span: SourceTextSpan,
) -> Result<&'a str, RemoteRos2InitializationError> {
    match span.kind {
        SourceTextKind::SyntheticPadding => Ok(SYNTHETIC_PADDING_NAME),
        SourceTextKind::SchemaName => definitions
            .schema_at_record(span.schema_record_index)
            .map(|schema| schema.header.name.as_str())
            .map(Ok)
            .unwrap_or_else(|| remote_ros2_fatal_invariant("schema-name span lost its source")),
        SourceTextKind::SchemaData => {
            let schema = definitions
                .schema_at_record(span.schema_record_index)
                .unwrap_or_else(|| remote_ros2_fatal_invariant("schema-data span lost its source"));
            let start = usize::try_from(span.start)
                .unwrap_or_else(|_overflow| remote_ros2_fatal_invariant("span start overflowed"));
            let len = usize::try_from(span.len)
                .unwrap_or_else(|_overflow| remote_ros2_fatal_invariant("span length overflowed"));
            let end = start
                .checked_add(len)
                .unwrap_or_else(|| remote_ros2_fatal_invariant("span end overflowed"));
            let bytes = schema
                .data
                .get(start..end)
                .unwrap_or_else(|| remote_ros2_fatal_invariant("span escaped source schema"));
            // The whole schema was validated as UTF-8 before this span could be created, and every
            // parser span is checked to lie on token boundaries in that same string. Re-running
            // UTF-8 validation here would be a second input-length scan outside the step owner.
            // SAFETY: `bytes` is a checked subslice of that already validated UTF-8 string.
            Ok(unsafe { std::str::from_utf8_unchecked(bytes) })
        }
    }
}

#[derive(Clone, Copy)]
struct LineView<'a> {
    text: &'a str,
    start: usize,
    next: usize,
}

fn next_line<'a>(
    input: &'a str,
    cursor: &mut usize,
    steps: &mut RemoteRos2StepOwnershipV1,
) -> Result<Option<LineView<'a>>, RemoteRos2InitializationError> {
    if *cursor >= input.len() {
        return Ok(None);
    }
    let start = *cursor;
    let bytes = input.as_bytes();
    let mut end = start;
    while end < bytes.len() && bytes[end] != b'\n' {
        steps.consume()?;
        end += 1;
    }
    if end < bytes.len() {
        steps.consume()?;
    }
    let next = if end < bytes.len() { end + 1 } else { end };
    let content_end = if end > start {
        steps.consume()?;
        if bytes[end - 1] == b'\r' {
            end - 1
        } else {
            end
        }
    } else {
        end
    };
    *cursor = next;
    Ok(Some(LineView {
        text: &input[start..content_end],
        start,
        next,
    }))
}

fn trim_ascii<'a>(
    slice: TextSlice<'a>,
    steps: &mut RemoteRos2StepOwnershipV1,
) -> Result<TextSlice<'a>, RemoteRos2InitializationError> {
    let bytes = slice.text.as_bytes();
    let mut start = 0;
    let mut end = bytes.len();
    while start < end {
        steps.consume()?;
        if bytes[start].is_ascii_whitespace() {
            start += 1;
        } else {
            break;
        }
    }
    while end > start {
        steps.consume()?;
        if bytes[end - 1].is_ascii_whitespace() {
            end -= 1;
        } else {
            break;
        }
    }
    Ok(TextSlice {
        text: &slice.text[start..end],
        absolute_start: slice.absolute_start + start,
    })
}

fn is_schema_separator(
    line: &str,
    steps: &mut RemoteRos2StepOwnershipV1,
) -> Result<bool, RemoteRos2InitializationError> {
    let trimmed = trim_ascii(
        TextSlice {
            text: line,
            absolute_start: 0,
        },
        steps,
    )?
    .text;
    if trimmed.len() < 3 {
        return Ok(false);
    }
    for byte in trimmed.bytes() {
        steps.consume()?;
        if byte != b'=' {
            return Ok(false);
        }
    }
    Ok(true)
}

fn find_next_separator(
    input: &str,
    start: usize,
    steps: &mut RemoteRos2StepOwnershipV1,
) -> Result<Option<(usize, usize)>, RemoteRos2InitializationError> {
    let mut cursor = start;
    while let Some(line) = next_line(input, &mut cursor, steps)? {
        if is_schema_separator(line.text, steps)? {
            return Ok(Some((line.start, line.next)));
        }
    }
    Ok(None)
}

struct SpecIter<'a> {
    schema: SchemaView<'a>,
    yielded_main: bool,
    next_dependency: Option<usize>,
    done: bool,
}

impl<'a> SpecIter<'a> {
    fn new(schema: SchemaView<'a>) -> Self {
        Self {
            schema,
            yielded_main: false,
            next_dependency: None,
            done: false,
        }
    }

    fn next_spec(
        &mut self,
        steps: &mut RemoteRos2StepOwnershipV1,
    ) -> Result<Option<SpecView>, RemoteRos2InitializationError> {
        if self.done {
            return Ok(None);
        }
        if !self.yielded_main {
            self.yielded_main = true;
            let (body_end, dependency) = if let Some((separator_start, separator_end)) =
                find_next_separator(self.schema.data, 0, steps)?
            {
                (separator_start, Some(separator_end))
            } else {
                self.done = true;
                (self.schema.data.len(), None)
            };
            self.next_dependency = dependency;
            return Ok(Some(SpecView {
                name: schema_name_span(self.schema)?,
                body_start: 0,
                body_end,
            }));
        }

        let dependency_start = self.next_dependency.take().ok_or(
            RemoteRos2InitializationError::UnsupportedForRemote(
                UnsupportedForRemote::GrammarFeature,
            ),
        )?;
        let mut cursor = dependency_start;
        let header = next_line(self.schema.data, &mut cursor, steps)?.ok_or(
            RemoteRos2InitializationError::UnsupportedForRemote(
                UnsupportedForRemote::GrammarFeature,
            ),
        )?;
        if is_schema_separator(header.text, steps)? {
            return Err(RemoteRos2InitializationError::UnsupportedForRemote(
                UnsupportedForRemote::GrammarFeature,
            ));
        }
        let header = trim_ascii(
            TextSlice {
                text: header.text,
                absolute_start: header.start,
            },
            steps,
        )?;
        let name = header.text.strip_prefix("MSG: ").ok_or(
            RemoteRos2InitializationError::UnsupportedForRemote(
                UnsupportedForRemote::GrammarFeature,
            ),
        )?;
        let prefix_bytes = header.text.len() - name.len();
        let name = trim_ascii(
            TextSlice {
                text: name,
                absolute_start: header.absolute_start + prefix_bytes,
            },
            steps,
        )?;
        if name.text.is_empty() {
            return Err(RemoteRos2InitializationError::UnsupportedForRemote(
                UnsupportedForRemote::GrammarFeature,
            ));
        }
        let name_span = span_from_data(self.schema, name.absolute_start, name.text.len())?;
        let (body_end, next_dependency) = if let Some((separator_start, separator_end)) =
            find_next_separator(self.schema.data, cursor, steps)?
        {
            (separator_start, Some(separator_end))
        } else {
            self.done = true;
            (self.schema.data.len(), None)
        };
        if cursor >= body_end {
            return Err(RemoteRos2InitializationError::UnsupportedForRemote(
                UnsupportedForRemote::GrammarFeature,
            ));
        }
        self.next_dependency = next_dependency;
        Ok(Some(SpecView {
            name: name_span,
            body_start: cursor,
            body_end,
        }))
    }
}

fn strip_comment_checked<'a>(
    line: TextSlice<'a>,
    steps: &mut RemoteRos2StepOwnershipV1,
) -> Result<TextSlice<'a>, RemoteRos2InitializationError> {
    let mut quote = None;
    let mut escaped = false;
    for (index, character) in line.text.char_indices() {
        steps.consume_bytes(character.len_utf8())?;
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' if quote.is_some() => escaped = true,
            '\'' | '"' if quote.is_none() => quote = Some(character),
            '\'' | '"' if quote == Some(character) => quote = None,
            '#' if quote.is_none() => {
                return trim_ascii(
                    TextSlice {
                        text: &line.text[..index],
                        absolute_start: line.absolute_start,
                    },
                    steps,
                );
            }
            _ => {}
        }
    }
    if quote.is_some() || escaped {
        return Err(RemoteRos2InitializationError::UnsupportedForRemote(
            UnsupportedForRemote::GrammarFeature,
        ));
    }
    trim_ascii(line, steps)
}

fn next_token<'a>(
    input: TextSlice<'a>,
    start: usize,
    steps: &mut RemoteRos2StepOwnershipV1,
) -> Result<Option<TextSlice<'a>>, RemoteRos2InitializationError> {
    let bytes = input.text.as_bytes();
    let mut cursor = start;
    while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
        steps.consume()?;
        cursor += 1;
    }
    if cursor >= bytes.len() || bytes[cursor] == b'=' {
        return Ok(None);
    }
    let token_start = cursor;
    while cursor < bytes.len() {
        steps.consume()?;
        let byte = bytes[cursor];
        if byte.is_ascii_whitespace()
            || (byte == b'=' && bytes.get(cursor.wrapping_sub(1)) != Some(&b'<'))
        {
            break;
        }
        cursor += 1;
    }
    Ok((cursor > token_start).then(|| TextSlice {
        text: &input.text[token_start..cursor],
        absolute_start: input.absolute_start + token_start,
    }))
}

fn token_end(input: TextSlice<'_>, token: TextSlice<'_>) -> usize {
    token.absolute_start - input.absolute_start + token.text.len()
}

fn ensure_length(
    actual: usize,
    limit: u64,
    kind: RemoteRos2ResourceLimit,
) -> Result<(), RemoteRos2InitializationError> {
    let actual = u64::try_from(actual).map_err(|_overflow| {
        RemoteRos2InitializationError::ResourceLimitExceeded(RemoteRos2ResourceLimit::Arithmetic)
    })?;
    if actual > limit {
        Err(RemoteRos2InitializationError::ResourceLimitExceeded(kind))
    } else {
        Ok(())
    }
}

fn is_valid_identifier_segment(
    segment: &str,
    steps: &mut RemoteRos2StepOwnershipV1,
) -> Result<bool, RemoteRos2InitializationError> {
    let mut bytes = segment.bytes();
    let Some(first) = bytes.next() else {
        return Ok(false);
    };
    steps.consume()?;
    if !first.is_ascii_alphabetic() {
        return Ok(false);
    }
    for byte in bytes {
        steps.consume()?;
        if !byte.is_ascii_alphanumeric() && byte != b'_' {
            return Ok(false);
        }
    }
    Ok(true)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QualifiedTypeNameValidityV1 {
    StrictSubset,
    LocalInvalid,
    OutsideRemoteSubset,
}

fn classify_qualified_type_name_v1(
    name: &str,
    steps: &mut RemoteRos2StepOwnershipV1,
) -> Result<QualifiedTypeNameValidityV1, RemoteRos2InitializationError> {
    if name.is_empty() {
        return Ok(QualifiedTypeNameValidityV1::LocalInvalid);
    }

    // Match the local parser's `rsplit_once('/')` admission before applying the stricter remote
    // identifier grammar. In particular `/Type` is locally invalid, while `//Type` and
    // `/pkg/Type` have non-empty prefixes at the final slash and are valid local names outside the
    // remote subset.
    let mut last_slash = None;
    for (index, byte) in name.bytes().enumerate() {
        steps.consume()?;
        if byte == b'/' {
            last_slash = Some(index);
        }
    }
    if let Some(index) = last_slash
        && (index == 0 || index.checked_add(1) == Some(name.len()))
    {
        return Ok(QualifiedTypeNameValidityV1::LocalInvalid);
    }
    for segment in name.split('/') {
        if segment.is_empty() {
            return Ok(QualifiedTypeNameValidityV1::OutsideRemoteSubset);
        }
        if !is_valid_identifier_segment(segment, steps)? {
            return Ok(QualifiedTypeNameValidityV1::OutsideRemoteSubset);
        }
        steps.consume()?;
    }
    Ok(QualifiedTypeNameValidityV1::StrictSubset)
}

fn is_valid_qualified_type_name(
    name: &str,
    steps: &mut RemoteRos2StepOwnershipV1,
) -> Result<bool, RemoteRos2InitializationError> {
    Ok(classify_qualified_type_name_v1(name, steps)? == QualifiedTypeNameValidityV1::StrictSubset)
}

fn is_valid_field_name(
    name: &str,
    steps: &mut RemoteRos2StepOwnershipV1,
) -> Result<bool, RemoteRos2InitializationError> {
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return Ok(false);
    };
    steps.consume()?;
    if !first.is_ascii_lowercase() {
        return Ok(false);
    }
    let mut previous_underscore = false;
    for byte in bytes {
        steps.consume()?;
        if !byte.is_ascii_lowercase() && !byte.is_ascii_digit() && byte != b'_' {
            return Ok(false);
        }
        if byte == b'_' && previous_underscore {
            return Ok(false);
        }
        previous_underscore = byte == b'_';
    }
    Ok(!previous_underscore)
}

fn is_valid_constant_name(
    name: &str,
    steps: &mut RemoteRos2StepOwnershipV1,
) -> Result<bool, RemoteRos2InitializationError> {
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return Ok(false);
    };
    steps.consume()?;
    if !first.is_ascii_uppercase() {
        return Ok(false);
    }
    let mut previous_underscore = false;
    for byte in bytes {
        steps.consume()?;
        if !byte.is_ascii_uppercase() && !byte.is_ascii_digit() && byte != b'_' {
            return Ok(false);
        }
        if byte == b'_' && previous_underscore {
            return Ok(false);
        }
        previous_underscore = byte == b'_';
    }
    Ok(!previous_underscore)
}

fn parse_u64(
    input: &str,
    steps: &mut RemoteRos2StepOwnershipV1,
) -> Result<u64, RemoteRos2InitializationError> {
    if input.is_empty() {
        return Err(RemoteRos2InitializationError::InvalidRemoteSchema);
    }
    let mut value = 0_u64;
    for byte in input.bytes() {
        steps.consume()?;
        if !byte.is_ascii_digit() {
            return Err(RemoteRos2InitializationError::InvalidRemoteSchema);
        }
        value = value
            .checked_mul(10)
            .and_then(|value| value.checked_add(u64::from(byte - b'0')))
            .ok_or(RemoteRos2InitializationError::InvalidRemoteSchema)?;
    }
    Ok(value)
}

fn parse_type_v1(
    schema: SchemaView<'_>,
    token: TextSlice<'_>,
    limits: &UnfrozenRemoteRos2LimitsV1,
    steps: &mut RemoteRos2StepOwnershipV1,
) -> Result<ParsedTypeV1, RemoteRos2InitializationError> {
    ensure_length(
        token.text.len(),
        limits.max_token_bytes,
        RemoteRos2ResourceLimit::TokenBytes,
    )?;
    // Perform one metered delimiter scan. In particular, do not compose `find`, `contains`, and
    // `ends_with`: those would repeatedly inspect caller-controlled text after only one charge.
    let mut open = None;
    let mut close = None;
    for (index, byte) in token.text.bytes().enumerate() {
        steps.consume()?;
        match byte {
            b'[' if open.replace(index).is_some() => {
                return Err(RemoteRos2InitializationError::InvalidRemoteSchema);
            }
            b']' if open.is_none() => {
                return Err(RemoteRos2InitializationError::UnsupportedForRemote(
                    UnsupportedForRemote::GrammarFeature,
                ));
            }
            b']' if close.replace(index).is_some() => {
                return Err(RemoteRos2InitializationError::InvalidRemoteSchema);
            }
            _ => {}
        }
    }
    let (base, array) = if let Some(open) = open {
        if close != Some(token.text.len() - 1) {
            return Err(RemoteRos2InitializationError::InvalidRemoteSchema);
        }
        let inner = &token.text[open + 1..token.text.len() - 1];
        let array = if inner.is_empty() {
            ArraySizeV1::Unbounded
        } else if let Some(bound) = inner.strip_prefix("<=") {
            let bound = parse_u64(bound, steps)?;
            if bound > limits.max_array_bound {
                return Err(RemoteRos2InitializationError::ResourceLimitExceeded(
                    RemoteRos2ResourceLimit::ArrayBound,
                ));
            }
            ArraySizeV1::Bounded(bound)
        } else {
            let bound = parse_u64(inner, steps)?;
            if bound > limits.max_array_bound {
                return Err(RemoteRos2InitializationError::ResourceLimitExceeded(
                    RemoteRos2ResourceLimit::ArrayBound,
                ));
            }
            ArraySizeV1::Fixed(bound)
        };
        (&token.text[..open], array)
    } else {
        if close.is_some() {
            return Err(RemoteRos2InitializationError::InvalidRemoteSchema);
        }
        (token.text, ArraySizeV1::Scalar)
    };
    if base.is_empty() {
        return Err(RemoteRos2InitializationError::InvalidRemoteSchema);
    }

    // Charge the complete bounded primitive classifier before it inspects `base`. The executable
    // classifier is a fixed V1 table, so one full scan is a conservative input-dependent bound.
    steps.consume_bytes(base.len())?;
    let primitive = match base {
        "bool" => Some(PrimitiveTypeV1::Bool),
        "byte" => Some(PrimitiveTypeV1::Byte),
        "char" => Some(PrimitiveTypeV1::Char),
        "float32" => Some(PrimitiveTypeV1::Float32),
        "float64" => Some(PrimitiveTypeV1::Float64),
        "int8" => Some(PrimitiveTypeV1::Int8),
        "int16" => Some(PrimitiveTypeV1::Int16),
        "int32" => Some(PrimitiveTypeV1::Int32),
        "int64" => Some(PrimitiveTypeV1::Int64),
        "uint8" => Some(PrimitiveTypeV1::UInt8),
        "uint16" => Some(PrimitiveTypeV1::UInt16),
        "uint32" => Some(PrimitiveTypeV1::UInt32),
        "uint64" => Some(PrimitiveTypeV1::UInt64),
        "string" => Some(PrimitiveTypeV1::String { bound: None }),
        "wstring" => {
            return Err(RemoteRos2InitializationError::UnsupportedForRemote(
                UnsupportedForRemote::Ros2Wstring,
            ));
        }
        _ => None,
    };
    let primitive = if let Some(bound) = base.strip_prefix("string<=") {
        let bound = parse_u64(bound, steps)?;
        if bound > limits.max_string_bound {
            return Err(RemoteRos2InitializationError::ResourceLimitExceeded(
                RemoteRos2ResourceLimit::StringBound,
            ));
        }
        Some(PrimitiveTypeV1::String { bound: Some(bound) })
    } else if let Some(bound) = base.strip_prefix("wstring<=") {
        parse_u64(bound, steps)?;
        return Err(RemoteRos2InitializationError::UnsupportedForRemote(
            UnsupportedForRemote::Ros2Wstring,
        ));
    } else {
        primitive
    };

    let element = if let Some(primitive) = primitive {
        ElementTypeV1::Primitive(primitive)
    } else {
        // `contains` and qualified-name validation are distinct scans and therefore have distinct
        // charges. The latter charges internally as it consumes each component byte.
        steps.consume_bytes(base.len())?;
        if base.contains("<=") {
            return Err(RemoteRos2InitializationError::UnsupportedForRemote(
                UnsupportedForRemote::GrammarFeature,
            ));
        }
        match classify_qualified_type_name_v1(base, steps)? {
            QualifiedTypeNameValidityV1::StrictSubset => {}
            QualifiedTypeNameValidityV1::LocalInvalid => {
                return Err(RemoteRos2InitializationError::InvalidRemoteSchema);
            }
            QualifiedTypeNameValidityV1::OutsideRemoteSubset => {
                return Err(RemoteRos2InitializationError::UnsupportedForRemote(
                    UnsupportedForRemote::GrammarFeature,
                ));
            }
        }
        ensure_length(
            base.len(),
            limits.max_identifier_bytes,
            RemoteRos2ResourceLimit::IdentifierBytes,
        )?;
        ElementTypeV1::Complex(span_from_data(schema, token.absolute_start, base.len())?)
    };
    Ok(ParsedTypeV1 { element, array })
}

fn parse_quoted_string<'a>(
    input: &'a str,
    steps: &mut RemoteRos2StepOwnershipV1,
) -> Result<&'a str, RemoteRos2InitializationError> {
    steps.consume_bytes(input.len())?;
    let Some(first) = input.as_bytes().first().copied() else {
        return Ok(input);
    };
    if first != b'\'' && first != b'"' {
        return Ok(input);
    }
    if input.len() < 2 || input.as_bytes().last().copied() != Some(first) {
        return Err(RemoteRos2InitializationError::UnsupportedForRemote(
            UnsupportedForRemote::GrammarFeature,
        ));
    }
    let inner = &input[1..input.len() - 1];
    let mut escaped = false;
    for byte in inner.bytes() {
        steps.consume()?;
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == first {
            return Err(RemoteRos2InitializationError::UnsupportedForRemote(
                UnsupportedForRemote::GrammarFeature,
            ));
        }
    }
    if escaped {
        return Err(RemoteRos2InitializationError::UnsupportedForRemote(
            UnsupportedForRemote::GrammarFeature,
        ));
    }
    Ok(inner)
}

fn metered_parse<T: std::str::FromStr>(
    input: &str,
    steps: &mut RemoteRos2StepOwnershipV1,
) -> Result<Option<T>, RemoteRos2InitializationError> {
    steps.consume_bytes(input.len())?;
    Ok(input.parse::<T>().ok())
}

fn require_local_integer<T>(parsed: Option<T>) -> Result<T, RemoteRos2InitializationError> {
    parsed.ok_or(RemoteRos2InitializationError::InvalidRemoteSchema)
}

fn reject_outside_remote_narrowing(
    within_remote_type: bool,
) -> Result<(), RemoteRos2InitializationError> {
    if within_remote_type {
        Ok(())
    } else {
        // The local parser accepts these values into its wider i64/u64 literal representation.
        // Keep InvalidRemoteSchema reserved for local-invalid text.
        Err(RemoteRos2InitializationError::UnsupportedForRemote(
            UnsupportedForRemote::GrammarFeature,
        ))
    }
}

fn validate_scalar_literal(
    input: &str,
    primitive: PrimitiveTypeV1,
    limits: &UnfrozenRemoteRos2LimitsV1,
    steps: &mut RemoteRos2StepOwnershipV1,
) -> Result<(), RemoteRos2InitializationError> {
    let valid = match primitive {
        PrimitiveTypeV1::Bool => {
            steps.consume_bytes(input.len())?;
            matches!(input, "true" | "false")
        }
        PrimitiveTypeV1::Byte | PrimitiveTypeV1::UInt8 => {
            let value = require_local_integer(metered_parse::<u64>(input, steps)?)?;
            reject_outside_remote_narrowing(u8::try_from(value).is_ok())?;
            true
        }
        PrimitiveTypeV1::Char | PrimitiveTypeV1::Int8 => {
            let value = require_local_integer(metered_parse::<i64>(input, steps)?)?;
            reject_outside_remote_narrowing(i8::try_from(value).is_ok())?;
            true
        }
        PrimitiveTypeV1::Int16 => {
            let value = require_local_integer(metered_parse::<i64>(input, steps)?)?;
            reject_outside_remote_narrowing(i16::try_from(value).is_ok())?;
            true
        }
        PrimitiveTypeV1::Int32 => {
            let value = require_local_integer(metered_parse::<i64>(input, steps)?)?;
            reject_outside_remote_narrowing(i32::try_from(value).is_ok())?;
            true
        }
        PrimitiveTypeV1::Int64 => metered_parse::<i64>(input, steps)?.is_some(),
        PrimitiveTypeV1::UInt16 => {
            let value = require_local_integer(metered_parse::<u64>(input, steps)?)?;
            reject_outside_remote_narrowing(u16::try_from(value).is_ok())?;
            true
        }
        PrimitiveTypeV1::UInt32 => {
            let value = require_local_integer(metered_parse::<u64>(input, steps)?)?;
            reject_outside_remote_narrowing(u32::try_from(value).is_ok())?;
            true
        }
        PrimitiveTypeV1::UInt64 => metered_parse::<u64>(input, steps)?.is_some(),
        // The local parser intentionally stores defaults for both float widths as f64.
        PrimitiveTypeV1::Float32 | PrimitiveTypeV1::Float64 => {
            metered_parse::<f64>(input, steps)?.is_some()
        }
        PrimitiveTypeV1::String { bound } => {
            let string = parse_quoted_string(input, steps)?;
            ensure_length(
                string.len(),
                limits.max_string_literal_bytes,
                RemoteRos2ResourceLimit::StringLiteralBytes,
            )?;
            if bound.is_some_and(|bound| string.len() as u64 > bound) {
                return Err(RemoteRos2InitializationError::UnsupportedForRemote(
                    UnsupportedForRemote::GrammarFeature,
                ));
            }
            true
        }
    };
    if valid {
        Ok(())
    } else {
        Err(RemoteRos2InitializationError::InvalidRemoteSchema)
    }
}

fn validate_literal_v1(
    input: &str,
    ty: ParsedTypeV1,
    limits: &UnfrozenRemoteRos2LimitsV1,
    steps: &mut RemoteRos2StepOwnershipV1,
) -> Result<(), RemoteRos2InitializationError> {
    let ElementTypeV1::Primitive(primitive) = ty.element else {
        return Err(RemoteRos2InitializationError::UnsupportedForRemote(
            UnsupportedForRemote::GrammarFeature,
        ));
    };
    match ty.array {
        ArraySizeV1::Scalar => validate_scalar_literal(input, primitive, limits, steps),
        ArraySizeV1::Fixed(expected) | ArraySizeV1::Bounded(expected) => {
            validate_array_literal(input, primitive, Some((ty.array, expected)), limits, steps)
        }
        ArraySizeV1::Unbounded => validate_array_literal(input, primitive, None, limits, steps),
    }
}

fn validate_array_literal(
    input: &str,
    primitive: PrimitiveTypeV1,
    bound: Option<(ArraySizeV1, u64)>,
    limits: &UnfrozenRemoteRos2LimitsV1,
    steps: &mut RemoteRos2StepOwnershipV1,
) -> Result<(), RemoteRos2InitializationError> {
    steps.consume_bytes(input.len())?;
    if !input.starts_with('[') || !input.ends_with(']') {
        return Err(RemoteRos2InitializationError::InvalidRemoteSchema);
    }
    let inner = &input[1..input.len() - 1];
    let mut count = 0_u64;
    let inner_trimmed = trim_ascii(
        TextSlice {
            text: inner,
            absolute_start: 0,
        },
        steps,
    )?;
    if !inner_trimmed.text.is_empty() {
        for element in inner.split(',') {
            steps.consume()?;
            let element = trim_ascii(
                TextSlice {
                    text: element,
                    absolute_start: 0,
                },
                steps,
            )?
            .text;
            if element.is_empty() {
                return Err(RemoteRos2InitializationError::UnsupportedForRemote(
                    UnsupportedForRemote::GrammarFeature,
                ));
            }
            validate_scalar_literal(element, primitive, limits, steps)?;
            count = checked_add(count, 1)?;
            if count > limits.max_array_bound {
                return Err(RemoteRos2InitializationError::ResourceLimitExceeded(
                    RemoteRos2ResourceLimit::ArrayBound,
                ));
            }
        }
    }
    if let Some((kind, expected)) = bound {
        match kind {
            ArraySizeV1::Fixed(_) if count != expected => {
                return Err(RemoteRos2InitializationError::UnsupportedForRemote(
                    UnsupportedForRemote::GrammarFeature,
                ));
            }
            ArraySizeV1::Bounded(_) if count > expected => {
                return Err(RemoteRos2InitializationError::UnsupportedForRemote(
                    UnsupportedForRemote::GrammarFeature,
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

fn parse_member_v1(
    schema: SchemaView<'_>,
    line: TextSlice<'_>,
    limits: &UnfrozenRemoteRos2LimitsV1,
    steps: &mut RemoteRos2StepOwnershipV1,
) -> Result<MemberView, RemoteRos2InitializationError> {
    let type_token =
        next_token(line, 0, steps)?.ok_or(RemoteRos2InitializationError::InvalidRemoteSchema)?;
    let type_end = token_end(line, type_token);
    let name_token = next_token(line, type_end, steps)?
        .ok_or(RemoteRos2InitializationError::InvalidRemoteSchema)?;
    ensure_length(
        name_token.text.len(),
        limits.max_identifier_bytes,
        RemoteRos2ResourceLimit::IdentifierBytes,
    )?;
    let name_end = token_end(line, name_token);
    let rest = trim_ascii(
        TextSlice {
            text: &line.text[name_end..],
            absolute_start: line.absolute_start + name_end,
        },
        steps,
    )?;
    let ty = parse_type_v1(schema, type_token, limits, steps)?;
    let constant = rest.text.starts_with('=');
    if constant && !is_valid_constant_name(name_token.text, steps)? {
        return Err(RemoteRos2InitializationError::InvalidRemoteSchema);
    }
    if !constant && !is_valid_field_name(name_token.text, steps)? {
        return Err(RemoteRos2InitializationError::UnsupportedForRemote(
            UnsupportedForRemote::GrammarFeature,
        ));
    }
    let literal = if constant {
        if !matches!(ty.array, ArraySizeV1::Scalar)
            || !matches!(ty.element, ElementTypeV1::Primitive(_))
        {
            return Err(RemoteRos2InitializationError::InvalidRemoteSchema);
        }
        let value = trim_ascii(
            TextSlice {
                text: &rest.text[1..],
                absolute_start: rest.absolute_start + 1,
            },
            steps,
        )?;
        if value.text.is_empty()
            && !matches!(
                ty.element,
                ElementTypeV1::Primitive(PrimitiveTypeV1::String { .. })
            )
        {
            return Err(RemoteRos2InitializationError::InvalidRemoteSchema);
        }
        ensure_length(
            value.text.len(),
            limits.max_default_bytes,
            RemoteRos2ResourceLimit::DefaultBytes,
        )?;
        validate_literal_v1(value.text, ty, limits, steps)?;
        Some(span_from_data(
            schema,
            value.absolute_start,
            value.text.len(),
        )?)
    } else if rest.text.is_empty() {
        None
    } else {
        ensure_length(
            rest.text.len(),
            limits.max_default_bytes,
            RemoteRos2ResourceLimit::DefaultBytes,
        )?;
        validate_literal_v1(rest.text, ty, limits, steps)?;
        Some(span_from_data(
            schema,
            rest.absolute_start,
            rest.text.len(),
        )?)
    };
    Ok(MemberView {
        kind: if constant {
            ParsedMemberKindV1::Constant
        } else {
            ParsedMemberKindV1::Field
        },
        name: span_from_data(schema, name_token.absolute_start, name_token.text.len())?,
        ty,
        literal,
    })
}

struct MemberIter<'a> {
    schema: SchemaView<'a>,
    spec: SpecView,
    cursor: usize,
    limits: &'a UnfrozenRemoteRos2LimitsV1,
}

impl<'a> MemberIter<'a> {
    fn new(schema: SchemaView<'a>, spec: SpecView, limits: &'a UnfrozenRemoteRos2LimitsV1) -> Self {
        Self {
            schema,
            spec,
            cursor: spec.body_start,
            limits,
        }
    }

    fn next_member(
        &mut self,
        steps: &mut RemoteRos2StepOwnershipV1,
    ) -> Result<Option<MemberView>, RemoteRos2InitializationError> {
        while self.cursor < self.spec.body_end {
            let line = next_line(self.schema.data, &mut self.cursor, steps)?
                .ok_or(RemoteRos2InitializationError::InvalidRemoteSchema)?;
            if line.start >= self.spec.body_end || line.next > self.spec.body_end.saturating_add(1)
            {
                return Err(RemoteRos2InitializationError::InvalidRemoteSchema);
            }
            ensure_length(
                line.text.len(),
                self.limits.max_line_bytes,
                RemoteRos2ResourceLimit::LineBytes,
            )?;
            let line = strip_comment_checked(
                TextSlice {
                    text: line.text,
                    absolute_start: line.start,
                },
                steps,
            )?;
            if line.text.is_empty() {
                continue;
            }
            if is_schema_separator(line.text, steps)? || line.text.starts_with("MSG:") {
                return Err(RemoteRos2InitializationError::InvalidRemoteSchema);
            }
            return parse_member_v1(self.schema, line, self.limits, steps).map(Some);
        }
        Ok(None)
    }
}

#[derive(Clone, Copy)]
struct SpecificationSummary {
    actual_fields: u64,
    constants: u64,
    needs_padding: bool,
}

fn summarize_specification(
    schema: SchemaView<'_>,
    spec: SpecView,
    limits: &UnfrozenRemoteRos2LimitsV1,
    meter: &mut RemoteRos2StepOwnershipV1,
) -> Result<SpecificationSummary, RemoteRos2InitializationError> {
    let mut members = MemberIter::new(schema, spec, limits);
    let mut fields = 0_u64;
    let mut constants = 0_u64;
    let mut first_constant_type = None;
    let mut enum_like = true;
    loop {
        meter.step()?;
        let Some(member) = members.next_member(meter)? else {
            break;
        };
        match member.kind {
            ParsedMemberKindV1::Field => fields = checked_add(fields, 1)?,
            ParsedMemberKindV1::Constant => {
                constants = checked_add(constants, 1)?;
                let ElementTypeV1::Primitive(primitive) = member.ty.element else {
                    return Err(RemoteRos2InitializationError::InvalidRemoteSchema);
                };
                if !matches!(
                    primitive,
                    PrimitiveTypeV1::Bool
                        | PrimitiveTypeV1::Byte
                        | PrimitiveTypeV1::Char
                        | PrimitiveTypeV1::Int8
                        | PrimitiveTypeV1::UInt8
                ) {
                    enum_like = false;
                }
                if let Some(first) = first_constant_type {
                    if first != primitive {
                        enum_like = false;
                    }
                } else {
                    first_constant_type = Some(primitive);
                }
            }
            ParsedMemberKindV1::SyntheticPadding => {
                remote_ros2_fatal_invariant("source parser emitted synthetic padding");
            }
        }
    }
    Ok(SpecificationSummary {
        actual_fields: fields,
        constants,
        needs_padding: fields == 0 && !(constants > 0 && enum_like),
    })
}

fn nth_spec_metered(
    schema: SchemaView<'_>,
    index: usize,
    meter: &mut RemoteRos2StepOwnershipV1,
) -> Result<Option<SpecView>, RemoteRos2InitializationError> {
    let mut specs = SpecIter::new(schema);
    for current in 0..=index {
        let Some(spec) = specs.next_spec(meter)? else {
            return Ok(None);
        };
        meter.step()?;
        if current == index {
            return Ok(Some(spec));
        }
    }
    Ok(None)
}

fn nth_member_metered(
    schema: SchemaView<'_>,
    spec: SpecView,
    index: usize,
    limits: &UnfrozenRemoteRos2LimitsV1,
    meter: &mut RemoteRos2StepOwnershipV1,
) -> Result<Option<MemberView>, RemoteRos2InitializationError> {
    let mut members = MemberIter::new(schema, spec, limits);
    for current in 0..=index {
        let Some(member) = members.next_member(meter)? else {
            return Ok(None);
        };
        meter.step()?;
        if current == index {
            return Ok(Some(member));
        }
    }
    Ok(None)
}

fn validate_specification_names(
    definitions: &ValidatedSummaryDefinitions<'_>,
    schema: SchemaView<'_>,
    spec_count: usize,
    limits: &UnfrozenRemoteRos2LimitsV1,
    meter: &mut RemoteRos2StepOwnershipV1,
) -> Result<(), RemoteRos2InitializationError> {
    #[cfg(target_arch = "wasm32")]
    verify_artifact_stage_identity(rerun_remote_ros2_name_stage_v1(), 0x2603);
    for left_index in 0..spec_count {
        let left = nth_spec_metered(schema, left_index, meter)?
            .ok_or(RemoteRos2InitializationError::InvalidRemoteSchema)?;
        let left_name = text_for_span(definitions, left.name)?;
        ensure_length(
            left_name.len(),
            limits.max_identifier_bytes,
            RemoteRos2ResourceLimit::IdentifierBytes,
        )?;
        if !is_valid_qualified_type_name(left_name, meter)? {
            return Err(RemoteRos2InitializationError::UnsupportedForRemote(
                UnsupportedForRemote::GrammarFeature,
            ));
        }
        for right_index in 0..left_index {
            meter.step()?;
            let right = nth_spec_metered(schema, right_index, meter)?
                .ok_or(RemoteRos2InitializationError::InvalidRemoteSchema)?;
            if metered_str_eq(left_name, text_for_span(definitions, right.name)?, meter)? {
                return Err(RemoteRos2InitializationError::UnsupportedForRemote(
                    UnsupportedForRemote::DependencyGraph,
                ));
            }
        }
    }
    Ok(())
}

fn validate_member_names(
    definitions: &ValidatedSummaryDefinitions<'_>,
    schema: SchemaView<'_>,
    spec_count: usize,
    limits: &UnfrozenRemoteRos2LimitsV1,
    meter: &mut RemoteRos2StepOwnershipV1,
) -> Result<(), RemoteRos2InitializationError> {
    for spec_index in 0..spec_count {
        let spec = nth_spec_metered(schema, spec_index, meter)?
            .ok_or(RemoteRos2InitializationError::InvalidRemoteSchema)?;
        let mut member_count = 0_usize;
        while nth_member_metered(schema, spec, member_count, limits, meter)?.is_some() {
            member_count = member_count.checked_add(1).ok_or(
                RemoteRos2InitializationError::ResourceLimitExceeded(
                    RemoteRos2ResourceLimit::Arithmetic,
                ),
            )?;
        }
        for left_index in 0..member_count {
            let left = nth_member_metered(schema, spec, left_index, limits, meter)?
                .ok_or(RemoteRos2InitializationError::InvalidRemoteSchema)?;
            let left_name = text_for_span(definitions, left.name)?;
            for right_index in 0..left_index {
                meter.step()?;
                let right = nth_member_metered(schema, spec, right_index, limits, meter)?
                    .ok_or(RemoteRos2InitializationError::InvalidRemoteSchema)?;
                if metered_str_eq(left_name, text_for_span(definitions, right.name)?, meter)? {
                    return Err(RemoteRos2InitializationError::UnsupportedForRemote(
                        UnsupportedForRemote::GrammarFeature,
                    ));
                }
            }
        }
    }
    Ok(())
}

fn type_name_matches(
    requested: &str,
    scope_name: &str,
    candidate_name: &str,
    meter: &mut RemoteRos2StepOwnershipV1,
) -> Result<bool, RemoteRos2InitializationError> {
    meter.consume_bytes(requested.len())?;
    meter.consume_bytes(scope_name.len())?;
    meter.consume_bytes(candidate_name.len())?;
    if requested.contains('/') {
        return Ok(requested == candidate_name);
    }
    if let Some((package, _remainder)) = scope_name.split_once('/') {
        Ok(candidate_name.len() == package.len() + 1 + requested.len()
            && candidate_name.starts_with(package)
            && candidate_name.as_bytes().get(package.len()) == Some(&b'/')
            && &candidate_name[package.len() + 1..] == requested)
    } else {
        Ok(candidate_name == requested)
    }
}

fn resolve_complex_type(
    definitions: &ValidatedSummaryDefinitions<'_>,
    schema: SchemaView<'_>,
    scope_spec_index: usize,
    type_span: SourceTextSpan,
    spec_count: usize,
    meter: &mut RemoteRos2StepOwnershipV1,
) -> Result<usize, RemoteRos2InitializationError> {
    #[cfg(target_arch = "wasm32")]
    verify_artifact_stage_identity(rerun_remote_ros2_complex_stage_v1(), 0x2604);
    let scope = nth_spec_metered(schema, scope_spec_index, meter)?
        .ok_or(RemoteRos2InitializationError::InvalidRemoteSchema)?;
    let scope_name = text_for_span(definitions, scope.name)?;
    let requested = text_for_span(definitions, type_span)?;
    let mut found = None;

    // The local reflection resolver contains dependent specifications, not the main definition.
    let mut candidates = SpecIter::new(schema);
    candidates
        .next_spec(meter)?
        .ok_or(RemoteRos2InitializationError::InvalidRemoteSchema)?;
    meter.step()?;
    for candidate_index in 1..spec_count {
        let candidate = candidates
            .next_spec(meter)?
            .ok_or(RemoteRos2InitializationError::InvalidRemoteSchema)?;
        meter.step()?;
        if type_name_matches(
            requested,
            scope_name,
            text_for_span(definitions, candidate.name)?,
            meter,
        )? && found.replace(candidate_index).is_some()
        {
            return Err(RemoteRos2InitializationError::UnsupportedForRemote(
                UnsupportedForRemote::DependencyGraph,
            ));
        }
    }
    found.ok_or(RemoteRos2InitializationError::UnsupportedForRemote(
        UnsupportedForRemote::DependencyGraph,
    ))
}

fn for_each_complex_member(
    schema: SchemaView<'_>,
    spec_count: usize,
    limits: &UnfrozenRemoteRos2LimitsV1,
    meter: &mut RemoteRos2StepOwnershipV1,
    mut visit: impl FnMut(
        usize,
        SourceTextSpan,
        &mut RemoteRos2StepOwnershipV1,
    ) -> Result<(), RemoteRos2InitializationError>,
) -> Result<(), RemoteRos2InitializationError> {
    for spec_index in 0..spec_count {
        let spec = nth_spec_metered(schema, spec_index, meter)?
            .ok_or(RemoteRos2InitializationError::InvalidRemoteSchema)?;
        let mut members = MemberIter::new(schema, spec, limits);
        while let Some(member) = members.next_member(meter)? {
            meter.step()?;
            if let ElementTypeV1::Complex(type_span) = member.ty.element {
                visit(spec_index, type_span, meter)?;
            }
        }
    }
    Ok(())
}

fn validate_dependency_graph(
    definitions: Option<&ValidatedSummaryDefinitions<'_>>,
    schema: SchemaView<'_>,
    spec_count: usize,
    limits: &UnfrozenRemoteRos2LimitsV1,
    meter: &mut RemoteRos2StepOwnershipV1,
    scratch: &mut RemoteRos2FixedScratchV1,
) -> Result<(), RemoteRos2InitializationError> {
    let empty = DependencyDfsFrameV1 {
        specification_index: usize::MAX,
        next_member_index: 0,
    };
    scratch.dependency_frames.fill(empty);
    let frames = &mut scratch.dependency_frames;
    for root in 0..spec_count {
        frames[0] = DependencyDfsFrameV1 {
            specification_index: root,
            next_member_index: 0,
        };
        let mut depth = 0_usize;
        loop {
            meter.step()?;
            let frame = frames
                .get_mut(depth)
                .unwrap_or_else(|| remote_ros2_fatal_invariant("dependency frame escaped scratch"));
            let spec =
                nth_spec_metered(schema, frame.specification_index, meter)?.unwrap_or_else(|| {
                    remote_ros2_fatal_invariant("dependency frame lost specification")
                });
            let member = nth_member_metered(schema, spec, frame.next_member_index, limits, meter)?;
            frame.next_member_index = frame.next_member_index.checked_add(1).ok_or(
                RemoteRos2InitializationError::ResourceLimitExceeded(
                    RemoteRos2ResourceLimit::Arithmetic,
                ),
            )?;
            let Some(member) = member else {
                if depth == 0 {
                    break;
                }
                depth -= 1;
                continue;
            };
            let ElementTypeV1::Complex(type_span) = member.ty.element else {
                continue;
            };
            let definitions = definitions
                .unwrap_or_else(|| remote_ros2_fatal_invariant("complex dependency has no source"));
            let target = resolve_complex_type(
                definitions,
                schema,
                frame.specification_index,
                type_span,
                spec_count,
                meter,
            )?;
            let next_depth = depth.checked_add(1).ok_or(
                RemoteRos2InitializationError::ResourceLimitExceeded(
                    RemoteRos2ResourceLimit::DependencyDepth,
                ),
            )?;
            if u64::try_from(next_depth).map_or(true, |value| value > limits.max_dependency_depth)
                || next_depth > MAX_STACK_DEPENDENCY_DEPTH_V1
            {
                return Err(RemoteRos2InitializationError::ResourceLimitExceeded(
                    RemoteRos2ResourceLimit::DependencyDepth,
                ));
            }
            for ancestor in &frames[..=depth] {
                meter.step()?;
                if ancestor.specification_index == target {
                    return Err(RemoteRos2InitializationError::UnsupportedForRemote(
                        UnsupportedForRemote::DependencyGraph,
                    ));
                }
            }
            frames[next_depth] = DependencyDfsFrameV1 {
                specification_index: target,
                next_member_index: 0,
            };
            depth = next_depth;
        }
    }
    Ok(())
}

fn census_one_schema(
    definitions: &ValidatedSummaryDefinitions<'_>,
    schema: SchemaView<'_>,
    limits: &UnfrozenRemoteRos2LimitsV1,
    census: &mut RemoteRos2Census,
    meter: &mut RemoteRos2StepOwnershipV1,
    scratch: &mut RemoteRos2FixedScratchV1,
) -> Result<(), RemoteRos2InitializationError> {
    increment_limited(
        &mut census.schemas,
        1,
        limits.max_schemas,
        RemoteRos2ResourceLimit::SchemaCount,
    )?;
    increment_limited(
        &mut census.definition_bytes,
        u64::try_from(schema.data.len()).map_err(|_overflow| {
            RemoteRos2InitializationError::ResourceLimitExceeded(
                RemoteRos2ResourceLimit::Arithmetic,
            )
        })?,
        limits.max_definition_bytes,
        RemoteRos2ResourceLimit::DefinitionBytes,
    )?;

    ensure_length(
        schema.name.len(),
        limits.max_identifier_bytes,
        RemoteRos2ResourceLimit::IdentifierBytes,
    )?;
    if !is_valid_qualified_type_name(schema.name, meter)? {
        return Err(RemoteRos2InitializationError::UnsupportedForRemote(
            UnsupportedForRemote::GrammarFeature,
        ));
    }

    let mut line_cursor = 0;
    while let Some(line) = next_line(schema.data, &mut line_cursor, meter)? {
        ensure_length(
            line.text.len(),
            limits.max_line_bytes,
            RemoteRos2ResourceLimit::LineBytes,
        )?;
    }

    let mut specs = SpecIter::new(schema);
    let mut spec_count = 0_usize;
    loop {
        meter.step()?;
        let Some(spec) = specs.next_spec(meter)? else {
            break;
        };
        increment_limited(
            &mut census.specifications,
            1,
            limits.max_specifications,
            RemoteRos2ResourceLimit::SpecificationCount,
        )?;
        spec_count = spec_count.checked_add(1).ok_or(
            RemoteRos2InitializationError::ResourceLimitExceeded(
                RemoteRos2ResourceLimit::Arithmetic,
            ),
        )?;
        let summary = summarize_specification(schema, spec, limits, meter)?;
        increment_limited(
            &mut census.fields,
            summary.actual_fields,
            limits.max_fields,
            RemoteRos2ResourceLimit::FieldCount,
        )?;
        increment_limited(
            &mut census.constants,
            summary.constants,
            limits.max_constants,
            RemoteRos2ResourceLimit::ConstantCount,
        )?;
        let synthetic = u64::from(summary.needs_padding);
        increment_limited(
            &mut census.fields,
            synthetic,
            limits.max_fields,
            RemoteRos2ResourceLimit::FieldCount,
        )?;
        let members = checked_add(
            checked_add(summary.actual_fields, summary.constants)?,
            synthetic,
        )?;
        increment_limited(
            &mut census.members,
            members,
            limits.max_members,
            RemoteRos2ResourceLimit::MemberCount,
        )?;
    }

    validate_specification_names(definitions, schema, spec_count, limits, meter)?;
    validate_member_names(definitions, schema, spec_count, limits, meter)?;
    for_each_complex_member(
        schema,
        spec_count,
        limits,
        meter,
        |scope_index, type_span, meter| {
            increment_limited(
                &mut census.dependency_edges,
                1,
                limits.max_dependency_edges,
                RemoteRos2ResourceLimit::DependencyEdges,
            )?;
            resolve_complex_type(
                definitions,
                schema,
                scope_index,
                type_span,
                spec_count,
                meter,
            )?;
            Ok(())
        },
    )?;
    validate_dependency_graph(
        Some(definitions),
        schema,
        spec_count,
        limits,
        meter,
        scratch,
    )?;

    Ok(())
}

fn compute_arena_peak(
    census: &mut RemoteRos2Census,
    limits: &UnfrozenRemoteRos2LimitsV1,
) -> Result<(), RemoteRos2InitializationError> {
    #[cfg(target_arch = "wasm32")]
    verify_artifact_stage_identity(rerun_remote_ros2_peak_stage_v1(), 0x2605);
    let schema_bytes = arena_allocation_footprint::<ParsedSchemaV1>(census.schemas)?;
    let specification_bytes =
        arena_allocation_footprint::<ParsedSpecificationV1>(census.specifications)?;
    let member_bytes = arena_allocation_footprint::<ParsedMemberV1>(census.members)?;
    let resolution_bytes =
        arena_allocation_footprint::<ComplexTypeResolutionV1>(census.dependency_edges)?;
    let retained = checked_add(
        checked_add(schema_bytes, specification_bytes)?,
        checked_add(member_bytes, resolution_bytes)?,
    )?;
    census.retained_bytes = checked_add(retained, remote_ros2_result_inline_bytes_v1()?)?;
    if census.retained_bytes > limits.max_retained_bytes {
        return Err(RemoteRos2InitializationError::ResourceLimitExceeded(
            RemoteRos2ResourceLimit::RetainedBytes,
        ));
    }

    census.working_bytes = checked_add(census.retained_bytes, sealed_fixed_working_bytes_v1()?)?;
    if census.working_bytes > limits.max_working_bytes {
        return Err(RemoteRos2InitializationError::ResourceLimitExceeded(
            RemoteRos2ResourceLimit::WorkingBytes,
        ));
    }
    Ok(())
}

fn remote_protobuf_projection_step_bound_v1(
    definitions: &ValidatedSummaryDefinitions<'_>,
    limits: &UnfrozenRemoteRos2LimitsV1,
) -> Result<u64, RemoteRos2InitializationError> {
    let record_count = u64::try_from(definitions.source_record_count()).map_err(|_overflow| {
        RemoteRos2InitializationError::ResourceLimitExceeded(RemoteRos2ResourceLimit::Arithmetic)
    })?;
    checked_remote_protobuf_projection_step_bound_v1(record_count, limits.max_projection_steps)
}

fn checked_remote_protobuf_projection_step_bound_v1(
    record_count: u64,
    max_projection_steps: u64,
) -> Result<u64, RemoteRos2InitializationError> {
    // The protobuf projection can perform one full canonicality scan per record and one nested
    // canonical-channel scan per candidate schema. This frozen conservative polynomial is charged
    // before an owner can exist; later stages cannot replenish it.
    let steps = record_count
        .checked_mul(record_count)
        .and_then(|square| {
            let cube = square.checked_mul(record_count)?;
            cube.checked_add(square)
        })
        .and_then(|value| value.checked_add(record_count))
        .ok_or(RemoteRos2InitializationError::ResourceLimitExceeded(
            RemoteRos2ResourceLimit::Arithmetic,
        ))?;
    if steps > max_projection_steps {
        return Err(RemoteRos2InitializationError::ResourceLimitExceeded(
            RemoteRos2ResourceLimit::ProjectionSteps,
        ));
    }
    Ok(steps)
}

fn measure_remote_ros2_recognition_steps_v1(
    definitions: &ValidatedSummaryDefinitions<'_>,
    ros2_schema_count: u64,
    limits: &UnfrozenRemoteRos2LimitsV1,
) -> Result<u64, RemoteRos2InitializationError> {
    let mut steps = RemoteRos2StepOwnershipV1::with_limit(
        limits.max_recognition_steps,
        RemoteRos2ResourceLimit::RecognitionSteps,
    );
    let ros2_schema_count = checked_usize(ros2_schema_count)?;
    for record_index in 0..definitions.source_record_count() {
        steps.consume()?;
        let Some(SummaryDefinitionProjectionRecord::Channel { id, .. }) =
            definitions.projection_record_at(record_index)
        else {
            continue;
        };
        if !is_canonical_channel_record_metered(definitions, record_index, id, &mut steps)? {
            continue;
        }
        let channel = definitions
            .channel_at_record(record_index)
            .unwrap_or_else(|| {
                remote_ros2_fatal_invariant("channel recognition census changed kind")
            });
        let schema = canonical_channel_schema_metered(definitions, channel, &mut steps)?;
        steps.consume_bytes(channel.message_encoding.len())?;
        if channel.message_encoding.eq_ignore_ascii_case("cdr")
            && let Some(schema) = schema
        {
            steps.consume_bytes(schema.header.encoding.len())?;
            steps.consume_bytes(ros2_schema_count)?;
        }
    }
    Ok(steps.consumed)
}

struct Ros2ArenaDirectoryV1 {
    schemas: FixedArena<ParsedSchemaV1>,
    specifications: FixedArena<ParsedSpecificationV1>,
    members: FixedArena<ParsedMemberV1>,
    resolutions: FixedArena<ComplexTypeResolutionV1>,
}

fn sealed_fixed_working_bytes_v1() -> Result<u64, RemoteRos2InitializationError> {
    // These are sealed owner/result objects whose storage is independent of input length.
    // Compiler-created scalar frames and spills are deliberately not guessed here; the locked
    // release-Wasm artifact verifier measures those and fails closed if its ceiling changes.
    let bytes = size_of::<RemoteRos2FixedScratchV1>()
        .checked_add(size_of::<Ros2ArenaDirectoryV1>())
        .and_then(|value| value.checked_add(size_of::<CountingMaterializationSink>()))
        .and_then(|value| {
            value.checked_add(size_of::<
                PreparedRemoteRos2CensusV1<'static, 'static, 'static, 'static>,
            >())
        })
        .ok_or(RemoteRos2InitializationError::ResourceLimitExceeded(
            RemoteRos2ResourceLimit::Arithmetic,
        ))?;
    let bytes = u64::try_from(bytes).map_err(|_overflow| {
        RemoteRos2InitializationError::ResourceLimitExceeded(RemoteRos2ResourceLimit::Arithmetic)
    })?;
    checked_add(bytes, LOCKED_REMOTE_ROS2_FRAME_SPILL_CEILING_V1)
}

fn remote_ros2_result_inline_bytes_v1() -> Result<u64, RemoteRos2InitializationError> {
    u64::try_from(size_of::<
        Ros2InitializedRemoteDefinitionsV1<'static, 'static, 'static, 'static>,
    >())
    .map_err(|_overflow| {
        RemoteRos2InitializationError::ResourceLimitExceeded(RemoteRos2ResourceLimit::Arithmetic)
    })
}

pub(crate) fn prepare_remote_ros2_census_v1<'definitions, 'input, 'source, 'wire>(
    mut evidence: RemoteDecoderTopicSignatureEvidenceV1<'definitions, 'input, 'source, 'wire>,
) -> Result<
    PreparedRemoteRos2CensusV1<'definitions, 'input, 'source, 'wire>,
    RemoteRos2InitializationError,
> {
    #[cfg(target_arch = "wasm32")]
    verify_artifact_stage_identity(rerun_remote_ros2_census_stage_v1(), 0x2602);
    evidence.owner.ensure_current()?;
    let limits = evidence.owner.budget_state.limits;
    let mut census_steps = RemoteRos2StepOwnershipV1::with_limit(
        limits.max_census_steps,
        RemoteRos2ResourceLimit::CensusSteps,
    );
    let mut census = RemoteRos2Census::default();
    let definitions = evidence.owner.source.definitions;
    let scratch = &mut evidence.owner.scratch;
    for_each_canonical_ros2_schema(definitions, &mut census_steps, |schema, steps| {
        census_one_schema(definitions, schema, &limits, &mut census, steps, scratch)
    })?;
    census.census_steps = census_steps.consumed;
    compute_arena_peak(&mut census, &limits)?;

    // This is the allocation-free dry run of the exact materialization control flow.
    // It includes the four arena allocation actions, every parser scan, and every emitted record.
    // A limit one step below the exact count therefore fails here, before the first arena exists.
    let mut materialization_steps = RemoteRos2StepOwnershipV1::with_limit(
        limits.max_materialization_steps,
        RemoteRos2ResourceLimit::MaterializationSteps,
    );
    consume_arena_allocation_steps(&mut materialization_steps)?;
    let mut counter = CountingMaterializationSink::default();
    walk_remote_ros2_materialization(
        definitions,
        &limits,
        &mut counter,
        &mut materialization_steps,
    )?;
    counter.verify(census)?;
    let exact_materialization_steps = materialization_steps.consumed;
    census.materialization_steps = exact_materialization_steps;
    census.projection_steps = remote_protobuf_projection_step_bound_v1(definitions, &limits)?;
    census.recognition_steps =
        measure_remote_ros2_recognition_steps_v1(definitions, census.schemas, &limits)?;
    evidence.owner.ensure_current()?;
    // Capacity ownership begins only after both allocation-free passes have succeeded.
    // Reserving the configured maxima before the census would let a small valid schema exclude an
    // unrelated initializer even though their exact simultaneous peaks fit in the aggregate cap.
    let reservation = evidence
        .owner
        .budget_state
        .reserve(census.working_bytes, census.retained_bytes)?;
    evidence.owner.ensure_current()?;
    Ok(PreparedRemoteRos2CensusV1 {
        evidence,
        census,
        reservation,
        materialization_steps: RemoteRos2StepOwnershipV1::with_limit(
            exact_materialization_steps,
            RemoteRos2ResourceLimit::MaterializationSteps,
        ),
    })
}

trait RemoteRos2ArenaAllocationGate {
    fn before_allocation(
        &self,
        arena_index: usize,
        layout: Layout,
    ) -> Result<(), RemoteRos2InitializationError>;

    fn after_allocation(&self, arena_index: usize);
}

struct SystemRemoteRos2ArenaAllocationGate;

impl RemoteRos2ArenaAllocationGate for SystemRemoteRos2ArenaAllocationGate {
    fn before_allocation(
        &self,
        _arena_index: usize,
        _layout: Layout,
    ) -> Result<(), RemoteRos2InitializationError> {
        Ok(())
    }

    fn after_allocation(&self, _arena_index: usize) {}
}

/// A fixed-capacity, contiguous arena whose only allocation is explicitly fallible.
///
/// The exact `Layout` is charged with [`locked_wasm_allocation_footprint_v1`] before this is called.
/// Rust 1.95.0 guarantees `try_reserve_exact` installs exactly the requested element capacity;
/// the arena never executes another reserve or growth operation.
struct FixedArena<T> {
    storage: Vec<T>,
}

impl<T> FixedArena<T> {
    fn try_new(
        capacity: usize,
        arena_index: usize,
        gate: &impl RemoteRos2ArenaAllocationGate,
    ) -> Result<Self, RemoteRos2InitializationError> {
        let layout = Layout::array::<T>(capacity).map_err(|_overflow| {
            RemoteRos2InitializationError::ResourceLimitExceeded(
                RemoteRos2ResourceLimit::Arithmetic,
            )
        })?;
        gate.before_allocation(arena_index, layout)?;
        let mut storage = Vec::new();
        storage
            .try_reserve_exact(capacity)
            .map_err(|_allocation| RemoteRos2InitializationError::FallibleAllocationFailed)?;
        gate.after_allocation(arena_index);
        Ok(Self { storage })
    }

    fn len(&self) -> usize {
        self.storage.len()
    }

    fn capacity(&self) -> usize {
        self.storage.capacity()
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "the fixed and counting sinks share one fallible materialization interface"
    )]
    fn push(&mut self, value: T) -> Result<(), RemoteRos2InitializationError> {
        if self.storage.len() >= self.storage.capacity() {
            remote_ros2_fatal_invariant("fixed arena census undercounted materialization");
        }
        self.storage.push(value);
        Ok(())
    }

    fn as_slice(&self) -> &[T] {
        self.storage.as_slice()
    }
}

impl<T> FixedArena<T> {
    fn get(&self, index: usize) -> Result<&T, RemoteRos2InitializationError> {
        self.storage
            .get(index)
            .ok_or_else(|| remote_ros2_fatal_invariant("fixed arena index is out of range"))
    }
}

fn consume_arena_allocation_steps(
    steps: &mut RemoteRos2StepOwnershipV1,
) -> Result<(), RemoteRos2InitializationError> {
    for _arena in 0..4 {
        steps.consume()?;
    }
    Ok(())
}

fn allocate_remote_ros2_arenas(
    census: RemoteRos2Census,
    gate: &impl RemoteRos2ArenaAllocationGate,
    steps: &mut RemoteRos2StepOwnershipV1,
) -> Result<Ros2ArenaDirectoryV1, RemoteRos2InitializationError> {
    steps.consume()?;
    let schemas = FixedArena::try_new(checked_usize(census.schemas)?, 0, gate)?;
    steps.consume()?;
    let specifications = FixedArena::try_new(checked_usize(census.specifications)?, 1, gate)?;
    steps.consume()?;
    let members = FixedArena::try_new(checked_usize(census.members)?, 2, gate)?;
    steps.consume()?;
    let resolutions = FixedArena::try_new(checked_usize(census.dependency_edges)?, 3, gate)?;
    Ok(Ros2ArenaDirectoryV1 {
        schemas,
        specifications,
        members,
        resolutions,
    })
}

trait RemoteRos2MaterializationSink {
    fn schema_len(&self) -> usize;
    fn specification_len(&self) -> usize;
    fn member_len(&self) -> usize;
    fn resolution_len(&self) -> usize;

    fn push_schema(&mut self, value: ParsedSchemaV1) -> Result<(), RemoteRos2InitializationError>;
    fn push_specification(
        &mut self,
        value: ParsedSpecificationV1,
    ) -> Result<(), RemoteRos2InitializationError>;
    fn push_member(&mut self, value: ParsedMemberV1) -> Result<(), RemoteRos2InitializationError>;
    fn push_resolution(
        &mut self,
        value: ComplexTypeResolutionV1,
    ) -> Result<(), RemoteRos2InitializationError>;
}

#[derive(Default)]
struct CountingMaterializationSink {
    schemas: usize,
    specifications: usize,
    members: usize,
    resolutions: usize,
}

impl CountingMaterializationSink {
    fn increment(value: &mut usize) -> Result<(), RemoteRos2InitializationError> {
        *value =
            value
                .checked_add(1)
                .ok_or(RemoteRos2InitializationError::ResourceLimitExceeded(
                    RemoteRos2ResourceLimit::Arithmetic,
                ))?;
        Ok(())
    }

    fn verify(&self, census: RemoteRos2Census) -> Result<(), RemoteRos2InitializationError> {
        if self.schemas == checked_usize(census.schemas)?
            && self.specifications == checked_usize(census.specifications)?
            && self.members == checked_usize(census.members)?
            && self.resolutions == checked_usize(census.dependency_edges)?
        {
            Ok(())
        } else {
            remote_ros2_fatal_invariant("dry-run record counts differ from grammar census")
        }
    }
}

impl RemoteRos2MaterializationSink for CountingMaterializationSink {
    fn schema_len(&self) -> usize {
        self.schemas
    }

    fn specification_len(&self) -> usize {
        self.specifications
    }

    fn member_len(&self) -> usize {
        self.members
    }

    fn resolution_len(&self) -> usize {
        self.resolutions
    }

    fn push_schema(&mut self, _value: ParsedSchemaV1) -> Result<(), RemoteRos2InitializationError> {
        Self::increment(&mut self.schemas)
    }

    fn push_specification(
        &mut self,
        _value: ParsedSpecificationV1,
    ) -> Result<(), RemoteRos2InitializationError> {
        Self::increment(&mut self.specifications)
    }

    fn push_member(&mut self, _value: ParsedMemberV1) -> Result<(), RemoteRos2InitializationError> {
        Self::increment(&mut self.members)
    }

    fn push_resolution(
        &mut self,
        _value: ComplexTypeResolutionV1,
    ) -> Result<(), RemoteRos2InitializationError> {
        Self::increment(&mut self.resolutions)
    }
}

impl RemoteRos2MaterializationSink for Ros2ArenaDirectoryV1 {
    fn schema_len(&self) -> usize {
        self.schemas.len()
    }

    fn specification_len(&self) -> usize {
        self.specifications.len()
    }

    fn member_len(&self) -> usize {
        self.members.len()
    }

    fn resolution_len(&self) -> usize {
        self.resolutions.len()
    }

    fn push_schema(&mut self, value: ParsedSchemaV1) -> Result<(), RemoteRos2InitializationError> {
        self.schemas.push(value)
    }

    fn push_specification(
        &mut self,
        value: ParsedSpecificationV1,
    ) -> Result<(), RemoteRos2InitializationError> {
        self.specifications.push(value)
    }

    fn push_member(&mut self, value: ParsedMemberV1) -> Result<(), RemoteRos2InitializationError> {
        self.members.push(value)
    }

    fn push_resolution(
        &mut self,
        value: ComplexTypeResolutionV1,
    ) -> Result<(), RemoteRos2InitializationError> {
        self.resolutions.push(value)
    }
}

fn walk_remote_ros2_materialization(
    definitions: &ValidatedSummaryDefinitions<'_>,
    limits: &UnfrozenRemoteRos2LimitsV1,
    sink: &mut impl RemoteRos2MaterializationSink,
    steps: &mut RemoteRos2StepOwnershipV1,
) -> Result<(), RemoteRos2InitializationError> {
    for_each_canonical_ros2_schema(definitions, steps, |schema, steps| {
        materialize_one_schema(Some(definitions), schema, limits, sink, steps)
    })
}

fn materialize_one_schema(
    definitions: Option<&ValidatedSummaryDefinitions<'_>>,
    schema: SchemaView<'_>,
    limits: &UnfrozenRemoteRos2LimitsV1,
    sink: &mut impl RemoteRos2MaterializationSink,
    steps: &mut RemoteRos2StepOwnershipV1,
) -> Result<(), RemoteRos2InitializationError> {
    let schema_index = sink.schema_len();
    let specification_start = sink.specification_len();
    let mut spec_count = 0_usize;
    let mut spec_counter = SpecIter::new(schema);
    loop {
        steps.consume()?;
        let Some(_spec) = spec_counter.next_spec(steps)? else {
            break;
        };
        spec_count = spec_count.checked_add(1).ok_or(
            RemoteRos2InitializationError::ResourceLimitExceeded(
                RemoteRos2ResourceLimit::Arithmetic,
            ),
        )?;
    }
    let mut specs = SpecIter::new(schema);
    for local_specification_index in 0..spec_count {
        steps.consume()?;
        let spec = specs.next_spec(steps)?.unwrap_or_else(|| {
            remote_ros2_fatal_invariant("specification count changed during materialization")
        });
        let specification_index = sink.specification_len();
        let member_start = sink.member_len();
        let mut members = MemberIter::new(schema, spec, limits);
        loop {
            steps.consume()?;
            let Some(member) = members.next_member(steps)? else {
                break;
            };
            let member_index = sink.member_len();
            steps.consume()?;
            sink.push_member(ParsedMemberV1 {
                specification_index,
                kind: member.kind,
                name: member.name,
                ty: member.ty,
                literal: member.literal,
            })?;
            if let ElementTypeV1::Complex(type_span) = member.ty.element {
                let definitions = definitions.unwrap_or_else(|| {
                    remote_ros2_fatal_invariant("complex materialization has no source")
                });
                let target = resolve_complex_type(
                    definitions,
                    schema,
                    local_specification_index,
                    type_span,
                    spec_count,
                    steps,
                )?;
                steps.consume()?;
                sink.push_resolution(ComplexTypeResolutionV1 {
                    member_index,
                    target_specification_index: specification_start.checked_add(target).ok_or(
                        RemoteRos2InitializationError::ResourceLimitExceeded(
                            RemoteRos2ResourceLimit::Arithmetic,
                        ),
                    )?,
                })?;
            }
        }
        if summarize_specification(schema, spec, limits, steps)?.needs_padding {
            steps.consume()?;
            sink.push_member(ParsedMemberV1 {
                specification_index,
                kind: ParsedMemberKindV1::SyntheticPadding,
                name: synthetic_padding_span(schema)?,
                ty: ParsedTypeV1 {
                    element: ElementTypeV1::Primitive(PrimitiveTypeV1::UInt8),
                    array: ArraySizeV1::Scalar,
                },
                literal: None,
            })?;
        }
        let member_end = sink.member_len();
        steps.consume()?;
        sink.push_specification(ParsedSpecificationV1 {
            schema_index,
            name: spec.name,
            members: member_start..member_end,
        })?;
    }
    let specification_end = sink.specification_len();
    steps.consume()?;
    sink.push_schema(ParsedSchemaV1 {
        schema_id: schema.id,
        schema_record_index: schema.record_index,
        specifications: specification_start..specification_end,
    })?;
    Ok(())
}

pub(crate) fn materialize_remote_ros2_definitions_v1<'definitions, 'input, 'source, 'wire>(
    prepared: PreparedRemoteRos2CensusV1<'definitions, 'input, 'source, 'wire>,
) -> Result<
    Ros2InitializedRemoteDefinitionsV1<'definitions, 'input, 'source, 'wire>,
    RemoteRos2InitializationError,
> {
    materialize_remote_ros2_definitions_v1_with_gate(prepared, &SystemRemoteRos2ArenaAllocationGate)
}

fn materialize_remote_ros2_definitions_v1_with_gate<'definitions, 'input, 'source, 'wire>(
    prepared: PreparedRemoteRos2CensusV1<'definitions, 'input, 'source, 'wire>,
    gate: &impl RemoteRos2ArenaAllocationGate,
) -> Result<
    Ros2InitializedRemoteDefinitionsV1<'definitions, 'input, 'source, 'wire>,
    RemoteRos2InitializationError,
> {
    let PreparedRemoteRos2CensusV1 {
        evidence,
        census,
        reservation,
        mut materialization_steps,
    } = prepared;
    evidence.owner.ensure_current()?;
    let limits = evidence.owner.budget_state.limits;
    let mut arenas = allocate_remote_ros2_arenas(census, gate, &mut materialization_steps)?;
    evidence.owner.ensure_current()?;
    let definitions = evidence.owner.source.definitions;
    walk_remote_ros2_materialization(
        definitions,
        &limits,
        &mut arenas,
        &mut materialization_steps,
    )?;
    if arenas.schemas.len() != checked_usize(census.schemas)?
        || arenas.specifications.len() != checked_usize(census.specifications)?
        || arenas.members.len() != checked_usize(census.members)?
        || arenas.resolutions.len() != checked_usize(census.dependency_edges)?
    {
        remote_ros2_fatal_invariant("materialized arena lengths differ from exact census");
    }
    materialization_steps.ensure_exhausted()?;
    evidence.owner.ensure_current()?;
    let RemoteDecoderTopicSignatureEvidenceV1 { owner } = evidence;
    let RemoteRos2AdmissionOwnerV1 {
        source,
        policy,
        steps: _steps,
        budget_state: _budget_state,
        viewer_scope,
        profile_scope,
        scratch: _scratch,
    } = owner;
    let result_reservation = reservation.complete();
    Ok(Ros2InitializedRemoteDefinitionsV1 {
        source,
        policy,
        viewer_scope,
        profile_scope,
        schemas: arenas.schemas,
        specifications: arenas.specifications,
        members: arenas.members,
        resolutions: arenas.resolutions,
        projection_steps: census.projection_steps,
        recognition_steps: census.recognition_steps,
        reservation: result_reservation,
    })
}

/// Release-Wasm executable probe used by the final artifact verifier.
///
/// This deliberately exercises the same primitive-schema parser, exact-step owner, fixed arenas,
/// `Vec::try_reserve_exact` allocation path, and installed accounting allocator as production.
/// It is not exported through wasm-bindgen and has no Viewer or source side effects.
#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub(crate) extern "C" fn rerun_remote_ros2_initializer_artifact_probe_v1_impl() -> u32 {
    let definitions = ValidatedSummaryDefinitions::for_remote_ros2_artifact_probe();
    let viewer_scope = RemoteViewerScopeState {
        _sealed_identity: 1,
    };
    let profile_scope = RemoteRos2ProfileScopeV1 {
        _sealed_identity: 1,
    };
    let source_state = RemoteDefinitionsSourceState {
        generation: Cell::new(1),
        viewer_scope: &viewer_scope,
    };
    let wire = RemoteDecoderPolicyWireV1 {
        allowlist_version: REMOTE_ALLOWLIST_VERSION_V1,
        assignment_version: REMOTE_POLICY_VERSION_V1,
        grammar_version: REMOTE_GRAMMAR_VERSION_V1,
        decoder_identities: &CANONICAL_DECODER_IDENTITIES_V1,
        fallback_identity: CANONICAL_FALLBACK_IDENTITY_V1,
    };
    let limits = UnfrozenRemoteRos2LimitsV1 {
        max_schemas: 1,
        max_definition_bytes: 128,
        max_specifications: 2,
        max_fields: 2,
        max_constants: 0,
        max_members: 2,
        max_line_bytes: 128,
        max_token_bytes: 32,
        max_identifier_bytes: 32,
        max_default_bytes: 32,
        max_string_literal_bytes: 32,
        max_string_bound: 32,
        max_array_bound: 32,
        max_dependency_edges: 1,
        max_dependency_depth: 1,
        max_signature_steps: 4_096,
        max_projection_steps: 4_096,
        max_recognition_steps: 4_096,
        max_census_steps: 32_768,
        max_materialization_steps: 32_768,
        max_retained_bytes: u64::MAX,
        max_working_bytes: u64::MAX,
    };
    let budget_state = Arc::new(RemoteRos2BudgetState {
        limits,
        capacity: RemoteRos2BudgetCapacity {
            max_active_initializers: 1,
            max_working_bytes: u64::MAX,
            max_retained_results: 1,
            max_retained_bytes: u64::MAX,
        },
        usage: Mutex::new(RemoteRos2BudgetUsage::default()),
        poisoned: AtomicBool::new(false),
        generation: 1,
        viewer_scope_identity: std::ptr::from_ref(&viewer_scope).addr(),
        profile_scope_identity: std::ptr::from_ref(&profile_scope).addr(),
        policy_wire_identity: std::ptr::from_ref(&wire).addr(),
    });
    let budget = RemoteRos2InitializationBudget {
        state: budget_state,
        source: &source_state,
        generation: 1,
        viewer_scope: &viewer_scope,
        profile_scope: &profile_scope,
        policy_wire: &wire,
    };
    let policy = match freeze_remote_decoder_policy_v1(&wire) {
        Ok(policy) => policy,
        Err(_error) => return 2,
    };
    let owner = match begin_remote_ros2_admission_v1(
        RemoteDefinitionsCapability {
            definitions: &definitions,
            source: &source_state,
            generation: 1,
        },
        policy,
        &budget,
    ) {
        Ok(owner) => owner,
        Err(_error) => return 3,
    };
    let evidence = match preflight_remote_decoder_topic_signatures_v1(owner) {
        Ok(evidence) => evidence,
        Err(_error) => return 4,
    };
    let prepared = match prepare_remote_ros2_census_v1(evidence) {
        Ok(prepared) => prepared,
        Err(_error) => return 5,
    };
    let result = match materialize_remote_ros2_definitions_v1(prepared) {
        Ok(result) => result,
        Err(_error) => return 6,
    };
    if result.schemas.len() != 1
        || result.specifications.len() != 2
        || result.members.len() != 2
        || result.resolutions.len() != 1
    {
        return 7;
    }
    let transition = match result.into_protobuf_transition_v1() {
        Ok(transition) => transition,
        Err(_error) => return 8,
    };
    if transition.ros2_schema_count() != 1 {
        return 9;
    }
    let Ok(mut projection) = transition.projection_iter_v1() else {
        return 10;
    };
    let Ok(Some(schema)) = projection.next_schema() else {
        return 11;
    };
    if schema.schema_id().ok() != Some(1) || schema.specification_count().ok() != Some(2) {
        return 12;
    }
    if schema.specification_name(1).ok() != Some("rerun/Child") {
        return 13;
    }
    drop(projection);
    drop(transition);
    if *budget.state.usage.lock() != RemoteRos2BudgetUsage::default() {
        return 3;
    }
    0
}

#[cfg(test)]
mod tests {
    use crate::remote_summary::tests::AllocationGuard;
    use crate::remote_summary::validated_summary_definitions_for_test;
    use crate::testing::{
        AdversarialMcapFixture, AdversarialMcapFixtureBuilder, DefinitionFixture, FixtureChannel,
        FixtureChunk, FixtureMessage, FixtureSchema, PartitionFixture,
    };

    use super::*;
    use crate::remote_protobuf_projection_boundary::RemoteProtobufProjectionEofContinuationV1;

    static_assertions::assert_not_impl_any!(
        RemoteDecoderTopicSignatureEvidenceV1<'static, 'static, 'static, 'static>: Clone, Copy
    );
    static_assertions::assert_not_impl_any!(
        PreparedRemoteRos2CensusV1<'static, 'static, 'static, 'static>: Clone, Copy
    );
    static_assertions::assert_not_impl_any!(
        Ros2InitializedRemoteDefinitionsV1<'static, 'static, 'static, 'static>: Clone, Copy
    );
    static_assertions::assert_not_impl_any!(
        RemoteRos2ToProtobufTransitionV1<'static, 'static, 'static, 'static>: Clone, Copy
    );
    static_assertions::assert_not_impl_any!(
        RemoteProtobufProjectionOwnerV1<'static, 'static, 'static, 'static>: Clone, Copy
    );
    static_assertions::assert_not_impl_any!(
        RemoteRos2ProjectionEofAuthorityV1<'static, 'static, 'static, 'static>: Clone, Copy
    );
    static_assertions::assert_not_impl_any!(
        RemoteProtobufProjectionEofContinuationV1<'static, 'static, 'static, 'static>: Clone, Copy
    );
    static_assertions::assert_not_impl_any!(
        RemoteRos2RecognitionIterV1<'static, 'static, 'static, 'static, 'static>: Clone, Copy
    );
    static_assertions::assert_not_impl_any!(
        RemoteRos2ChannelRecognitionV1<'static, 'static, 'static, 'static, 'static, 'static>: Clone, Copy
    );

    static CANONICAL_IDENTITIES: [&str; 4] = CANONICAL_DECODER_IDENTITIES_V1;
    static VIEWER_SCOPE: RemoteViewerScopeState = RemoteViewerScopeState {
        _sealed_identity: 1,
    };
    static OTHER_VIEWER_SCOPE: RemoteViewerScopeState = RemoteViewerScopeState {
        _sealed_identity: 2,
    };
    static PROFILE_SCOPE: RemoteRos2ProfileScopeV1 = RemoteRos2ProfileScopeV1 {
        _sealed_identity: 1,
    };
    static OTHER_PROFILE_SCOPE: RemoteRos2ProfileScopeV1 = RemoteRos2ProfileScopeV1 {
        _sealed_identity: 2,
    };

    fn complete_projection_for_test_v1<'definitions, 'input, 'source, 'wire>(
        mut projection: RemoteProtobufProjectionOwnerV1<'definitions, 'input, 'source, 'wire>,
    ) -> Result<
        (
            usize,
            RemoteProtobufProjectionEofContinuationV1<'definitions, 'input, 'source, 'wire>,
        ),
        RemoteRos2InitializationError,
    > {
        let mut schema_count = 0_usize;
        while let Some(schema) = projection.next_schema()? {
            let _ = (
                schema.schema_id()?,
                schema.name()?,
                schema.encoding()?,
                schema.data()?,
            );
            schema_count = schema_count.checked_add(1).ok_or(
                RemoteRos2InitializationError::ResourceLimitExceeded(
                    RemoteRos2ResourceLimit::Arithmetic,
                ),
            )?;
        }
        Ok((schema_count, projection.finish_eof_v1()?))
    }

    fn canonical_policy_wire() -> RemoteDecoderPolicyWireV1<'static> {
        RemoteDecoderPolicyWireV1 {
            allowlist_version: REMOTE_ALLOWLIST_VERSION_V1,
            assignment_version: REMOTE_POLICY_VERSION_V1,
            grammar_version: REMOTE_GRAMMAR_VERSION_V1,
            decoder_identities: &CANONICAL_IDENTITIES,
            fallback_identity: CANONICAL_FALLBACK_IDENTITY_V1,
        }
    }

    fn generous_limits() -> UnfrozenRemoteRos2LimitsV1 {
        UnfrozenRemoteRos2LimitsV1 {
            max_schemas: 32,
            max_definition_bytes: 1_000_000,
            max_specifications: 256,
            max_fields: 4_096,
            max_constants: 4_096,
            max_members: 8_192,
            max_line_bytes: 16_384,
            max_token_bytes: 1_024,
            max_identifier_bytes: 1_024,
            max_default_bytes: 16_384,
            max_string_literal_bytes: 16_384,
            max_string_bound: 1_000_000,
            max_array_bound: 1_000_000,
            max_dependency_edges: 4_096,
            max_dependency_depth: 32,
            max_signature_steps: 1_000_000,
            max_projection_steps: 1_000_000,
            max_recognition_steps: 1_000_000,
            max_census_steps: 1_000_000,
            max_materialization_steps: 1_000_000,
            max_retained_bytes: 16_000_000,
            max_working_bytes: 16_000_000,
        }
    }

    fn budget<'source, 'wire>(
        source: &'source RemoteDefinitionsSourceState,
        policy_wire: &'wire RemoteDecoderPolicyWireV1<'wire>,
        limits: UnfrozenRemoteRos2LimitsV1,
    ) -> RemoteRos2InitializationBudget<'source, 'wire> {
        RemoteRos2InitializationBudget {
            state: Arc::new(RemoteRos2BudgetState {
                limits,
                capacity: RemoteRos2BudgetCapacity {
                    max_active_initializers: 8,
                    max_working_bytes: 64_000_000,
                    max_retained_results: 8,
                    max_retained_bytes: 64_000_000,
                },
                usage: Mutex::new(RemoteRos2BudgetUsage::default()),
                poisoned: AtomicBool::new(false),
                generation: source.generation.get(),
                viewer_scope_identity: source.viewer_scope.addr(),
                profile_scope_identity: std::ptr::from_ref(&PROFILE_SCOPE).addr(),
                policy_wire_identity: std::ptr::from_ref(policy_wire).addr(),
            }),
            source,
            generation: source.generation.get(),
            viewer_scope: &VIEWER_SCOPE,
            profile_scope: &PROFILE_SCOPE,
            policy_wire,
        }
    }

    fn fixture(
        schemas: impl IntoIterator<Item = FixtureSchema>,
        channels: impl IntoIterator<Item = FixtureChannel>,
    ) -> AdversarialMcapFixture {
        AdversarialMcapFixtureBuilder::new()
            .with_schemas(schemas)
            .with_channels(channels)
            .with_chunks([FixtureChunk::single(FixtureMessage::new(1, 0, 1))])
            .with_partition_fixture(PartitionFixture::default())
            .build()
            .expect("the remote ROS 2 test fixture builds")
    }

    fn schema(id: u16, name: &str, encoding: &str, data: impl Into<Vec<u8>>) -> FixtureSchema {
        FixtureSchema::new(id, name, encoding).with_data(data)
    }

    fn channel(id: u16, schema_id: u16, topic: &str, message_encoding: &str) -> FixtureChannel {
        FixtureChannel::schema_less(id, topic).with_schema(schema_id, message_encoding)
    }

    fn source_state() -> RemoteDefinitionsSourceState {
        RemoteDefinitionsSourceState {
            generation: Cell::new(1),
            viewer_scope: &VIEWER_SCOPE,
        }
    }

    #[expect(
        clippy::disallowed_methods,
        reason = "host-only tests verify that internal control-plane failures do not enter the request error enum"
    )]
    fn assert_fatal_control_plane(action: impl FnOnce()) {
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(action)).is_err(),
            "control-plane invariant must use the internal fatal path"
        );
    }

    #[test]
    fn policy_must_be_exact_before_signature_or_census() {
        let mut reordered = CANONICAL_IDENTITIES;
        reordered.swap(0, 1);
        for bad in [
            RemoteDecoderPolicyWireV1 {
                allowlist_version: 99,
                ..canonical_policy_wire()
            },
            RemoteDecoderPolicyWireV1 {
                decoder_identities: &reordered,
                ..canonical_policy_wire()
            },
            RemoteDecoderPolicyWireV1 {
                decoder_identities: &[
                    CANONICAL_IDENTITIES[0],
                    CANONICAL_IDENTITIES[1],
                    CANONICAL_IDENTITIES[1],
                    CANONICAL_IDENTITIES[3],
                ],
                ..canonical_policy_wire()
            },
        ] {
            assert!(freeze_remote_decoder_policy_v1(&bad).is_err());
        }
    }

    #[test]
    fn same_topic_signature_is_order_independent_and_covers_all_encodings() {
        for reverse in [false, true] {
            let mut schemas = vec![
                schema(7, "pkg/Valid", "ros2msg", b"int32 value"),
                schema(8, "pkg/Wide", "ros2msg", b"wstring value"),
            ];
            let mut channels = vec![
                channel(1, 7, "/same", "not-cdr"),
                channel(2, 8, "/same", "not-cdr"),
            ];
            if reverse {
                schemas.reverse();
                channels.reverse();
            }
            let fixture = fixture(schemas, channels);
            let definitions = validated_summary_definitions_for_test(&fixture);
            let state = source_state();
            let wire = canonical_policy_wire();
            let budget = budget(&state, &wire, generous_limits());
            let error = preflight(&definitions, &state, &wire, &budget)
                .err()
                .expect("different same-topic schema signatures conflict");
            assert_eq!(
                error,
                RemoteRos2InitializationError::ConflictingTopicDecoderSignature
            );
        }

        let fixture = fixture(
            [
                schema(7, "pkg/A", "protobuf", [1, 2]),
                schema(8, "pkg/B", "protobuf", [1, 3]),
            ],
            [
                channel(1, 7, "/same", "protobuf"),
                channel(2, 8, "/same", "protobuf"),
            ],
        );
        let definitions = validated_summary_definitions_for_test(&fixture);
        let state = source_state();
        let wire = canonical_policy_wire();
        let budget = budget(&state, &wire, generous_limits());
        assert_eq!(
            preflight(&definitions, &state, &wire, &budget).err(),
            Some(RemoteRos2InitializationError::ConflictingTopicDecoderSignature)
        );
    }

    #[test]
    fn identical_same_topic_signatures_ignore_schema_and_channel_ids() {
        let fixture = fixture(
            [
                schema(7, "pkg/Msg", "ros2msg", b"int32 value"),
                schema(8, "pkg/Msg", "ros2msg", b"int32 value"),
            ],
            [channel(1, 7, "/same", "cdr"), channel(2, 8, "/same", "cdr")],
        );
        let definitions = validated_summary_definitions_for_test(&fixture);
        let state = source_state();
        let wire = canonical_policy_wire();
        let different_topics_budget = budget(&state, &wire, generous_limits());
        preflight(&definitions, &state, &wire, &different_topics_budget)
            .expect("IDs do not participate in the same-topic signature");
    }

    #[test]
    fn decoder_signature_field_matrix_and_canonical_duplicates_are_exact() {
        enum Difference {
            MessageEncoding,
            SchemaPresence,
            SchemaName,
            SchemaEncoding,
            SchemaData,
        }
        for difference in [
            Difference::MessageEncoding,
            Difference::SchemaPresence,
            Difference::SchemaName,
            Difference::SchemaEncoding,
            Difference::SchemaData,
        ] {
            let mut right_schema = schema(8, "pkg/Msg", "ros2msg", b"int32 value");
            let mut right_channel = channel(2, 8, "/same", "cdr");
            match difference {
                Difference::MessageEncoding => {
                    right_channel.message_encoding = "cdr-alt".to_owned();
                }
                Difference::SchemaPresence => {
                    right_channel.schema_id = 0;
                }
                Difference::SchemaName => right_schema.name = "pkg/Other".to_owned(),
                Difference::SchemaEncoding => right_schema.encoding = "protobuf".to_owned(),
                Difference::SchemaData => right_schema.data = b"uint32 value".to_vec(),
            }
            let fixture = fixture(
                [
                    schema(7, "pkg/Msg", "ros2msg", b"int32 value"),
                    right_schema,
                ],
                [channel(1, 7, "/same", "cdr"), right_channel],
            );
            let definitions = validated_summary_definitions_for_test(&fixture);
            let state = source_state();
            let wire = canonical_policy_wire();
            let budget = budget(&state, &wire, generous_limits());
            assert_eq!(
                preflight(&definitions, &state, &wire, &budget).err(),
                Some(RemoteRos2InitializationError::ConflictingTopicDecoderSignature)
            );
        }

        let different_topics = fixture(
            [
                schema(7, "pkg/A", "ros2msg", b"int32 value"),
                schema(8, "pkg/B", "protobuf", [1, 2, 3]),
            ],
            [channel(1, 7, "/a", "cdr"), channel(2, 8, "/b", "protobuf")],
        );
        let definitions = validated_summary_definitions_for_test(&different_topics);
        let state = source_state();
        let wire = canonical_policy_wire();
        let different_topics_budget = budget(&state, &wire, generous_limits());
        preflight(&definitions, &state, &wire, &different_topics_budget)
            .expect("decoder signatures are compared only within one exact topic");

        for reverse in [false, true] {
            let mut schemas = vec![
                schema(7, "pkg/Msg", "ros2msg", b"int32 value"),
                schema(8, "pkg/Msg", "ros2msg", b"int32 value"),
            ];
            let mut channels = vec![channel(1, 7, "/same", "cdr"), channel(2, 8, "/same", "cdr")];
            if reverse {
                schemas.reverse();
                channels.reverse();
            }
            let fixture = fixture(schemas, channels);
            let definitions = validated_summary_definitions_for_test(&fixture);
            let state = source_state();
            let wire = canonical_policy_wire();
            let budget = budget(&state, &wire, generous_limits());
            preflight(&definitions, &state, &wire, &budget)
                .expect("identical canonical signatures are source-order independent");
        }

        let exact_duplicates = AdversarialMcapFixtureBuilder::new()
            .with_schemas([schema(7, "pkg/Msg", "ros2msg", b"int32 value")])
            .with_channels([channel(1, 7, "/same", "cdr")])
            .with_chunks([FixtureChunk::single(FixtureMessage::new(1, 0, 1))])
            .with_definition_fixture(DefinitionFixture::ExactDuplicate)
            .with_partition_fixture(PartitionFixture::default())
            .build()
            .expect("the exact-duplicate fixture builds");
        let definitions = validated_summary_definitions_for_test(&exact_duplicates);
        let state = source_state();
        let wire = canonical_policy_wire();
        let budget = budget(&state, &wire, generous_limits());
        preflight(&definitions, &state, &wire, &budget)
            .expect("exact duplicate records do not mint a second canonical signature");
    }

    fn preflight<'definitions, 'input, 'source, 'wire>(
        definitions: &'definitions ValidatedSummaryDefinitions<'input>,
        state: &'source RemoteDefinitionsSourceState,
        wire: &'wire RemoteDecoderPolicyWireV1<'wire>,
        budget: &RemoteRos2InitializationBudget<'source, 'wire>,
    ) -> Result<
        RemoteDecoderTopicSignatureEvidenceV1<'definitions, 'input, 'source, 'wire>,
        RemoteRos2InitializationError,
    > {
        let policy = freeze_remote_decoder_policy_v1(wire).expect("test policy is canonical");
        let owner = begin_remote_ros2_admission_v1(
            RemoteDefinitionsCapability {
                definitions,
                source: state,
                generation: 1,
            },
            policy,
            budget,
        )?;
        preflight_remote_decoder_topic_signatures_v1(owner)
    }

    fn prepare<'definitions, 'input, 'source, 'wire>(
        definitions: &'definitions ValidatedSummaryDefinitions<'input>,
        state: &'source RemoteDefinitionsSourceState,
        wire: &'wire RemoteDecoderPolicyWireV1<'wire>,
        budget: &RemoteRos2InitializationBudget<'source, 'wire>,
    ) -> Result<
        PreparedRemoteRos2CensusV1<'definitions, 'input, 'source, 'wire>,
        RemoteRos2InitializationError,
    > {
        prepare_remote_ros2_census_v1(preflight(definitions, state, wire, budget)?)
    }

    #[test]
    fn allocation_free_census_validates_full_schema_and_reserves_exact_peak() {
        let fixture = fixture(
            [schema(
                7,
                "pkg/Root",
                "ros2msg",
                b"int32 count 2\nChild child\n=====\nMSG: pkg/Child\nstring<=8 label \"ok\"\n",
            )],
            [channel(1, 7, "/root", "not-cdr-but-still-observed")],
        );
        let definitions = validated_summary_definitions_for_test(&fixture);
        let state = source_state();
        let wire = canonical_policy_wire();
        let budget = budget(&state, &wire, generous_limits());
        let prepared = prepare(&definitions, &state, &wire, &budget)
            .expect("the strict admitted schema completes its census");
        assert_eq!(prepared.census.schemas, 1);
        assert_eq!(prepared.census.specifications, 2);
        assert_eq!(prepared.census.fields, 3); // count, child, label
        assert_eq!(prepared.census.dependency_edges, 1);
        assert!(prepared.census.census_steps > 0);
        assert!(prepared.census.materialization_steps > 0);
        assert_eq!(
            *budget.state.usage.lock(),
            RemoteRos2BudgetUsage {
                active_initializers: 1,
                working_bytes: prepared.census.working_bytes,
                retained_results: 1,
                retained_bytes: prepared.census.retained_bytes,
            }
        );
        drop(prepared);
        assert_eq!(*budget.state.usage.lock(), RemoteRos2BudgetUsage::default());
    }

    #[test]
    fn exact_peak_capacity_allows_small_concurrency_and_invalid_census_claims_nothing() {
        let valid_fixture = fixture(
            [schema(7, "pkg/Root", "ros2msg", b"int32 value")],
            [channel(1, 7, "/root", "cdr")],
        );
        let valid_definitions = validated_summary_definitions_for_test(&valid_fixture);
        let invalid_fixture = fixture(
            [schema(7, "pkg/Root", "ros2msg", b"wstring value")],
            [channel(1, 7, "/root", "cdr")],
        );
        let invalid_definitions = validated_summary_definitions_for_test(&invalid_fixture);
        let state = source_state();
        let wire = canonical_policy_wire();
        let measuring_budget = budget(&state, &wire, generous_limits());
        let measured = prepare(&valid_definitions, &state, &wire, &measuring_budget).unwrap();
        let exact_working = measured.census.working_bytes;
        let exact_retained = measured.census.retained_bytes;
        assert!(exact_working < measuring_budget.state.limits.max_working_bytes);
        assert!(exact_retained < measuring_budget.state.limits.max_retained_bytes);
        drop(measured);

        let mut constrained = budget(&state, &wire, generous_limits());
        let state_mut = Arc::get_mut(&mut constrained.state).unwrap();
        state_mut.capacity = RemoteRos2BudgetCapacity {
            max_active_initializers: 2,
            max_working_bytes: exact_working * 2,
            max_retained_results: 2,
            max_retained_bytes: exact_retained * 2,
        };
        let first = prepare(&valid_definitions, &state, &wire, &constrained).unwrap();
        let before_invalid = *constrained.state.usage.lock();
        assert!(matches!(
            prepare(&invalid_definitions, &state, &wire, &constrained),
            Err(RemoteRos2InitializationError::UnsupportedForRemote(
                UnsupportedForRemote::Ros2Wstring
            ))
        ));
        assert_eq!(*constrained.state.usage.lock(), before_invalid);
        let second = prepare(&valid_definitions, &state, &wire, &constrained).unwrap();
        assert_eq!(
            constrained.state.usage.lock().active_initializers,
            2,
            "exact reservations, rather than configured maxima, share aggregate capacity"
        );
        drop((first, second));
        assert_eq!(
            *constrained.state.usage.lock(),
            RemoteRos2BudgetUsage::default()
        );
    }

    #[test]
    fn malformed_wstring_dependency_and_resource_limits_are_typed_and_rollback() {
        let cases = [
            (
                b"wstring value".as_slice(),
                RemoteRos2InitializationError::UnsupportedForRemote(
                    UnsupportedForRemote::Ros2Wstring,
                ),
            ),
            (
                b"Child child\n===\nMSG: pkg/Child\nwstring[] value".as_slice(),
                RemoteRos2InitializationError::UnsupportedForRemote(
                    UnsupportedForRemote::Ros2Wstring,
                ),
            ),
            (
                b"int32 broken=[1]".as_slice(),
                RemoteRos2InitializationError::InvalidRemoteSchema,
            ),
        ];
        for (data, expected) in cases {
            let fixture = fixture(
                [schema(7, "pkg/Root", "ros2msg", data)],
                [channel(1, 7, "/root", "cdr")],
            );
            let definitions = validated_summary_definitions_for_test(&fixture);
            let state = source_state();
            let wire = canonical_policy_wire();
            let budget = budget(&state, &wire, generous_limits());
            assert_eq!(
                prepare(&definitions, &state, &wire, &budget).err(),
                Some(expected)
            );
            assert_eq!(*budget.state.usage.lock(), RemoteRos2BudgetUsage::default());
        }

        let fixture = fixture(
            [schema(7, "pkg/Root", "ros2msg", b"int32 value")],
            [channel(1, 7, "/root", "cdr")],
        );
        let definitions = validated_summary_definitions_for_test(&fixture);
        let state = source_state();
        let wire = canonical_policy_wire();
        let mut limits = generous_limits();
        limits.max_fields = 0;
        let budget = budget(&state, &wire, limits);
        assert_eq!(
            prepare(&definitions, &state, &wire, &budget).err(),
            Some(RemoteRos2InitializationError::ResourceLimitExceeded(
                RemoteRos2ResourceLimit::FieldCount
            ))
        );
        assert_eq!(*budget.state.usage.lock(), RemoteRos2BudgetUsage::default());
    }

    #[test]
    fn census_and_all_pre_reservation_failures_allocate_nothing() {
        for data in [
            b"int32 value".as_slice(),
            b"wstring value".as_slice(),
            &[0xff_u8],
        ] {
            let fixture = fixture(
                [schema(7, "pkg/Root", "ros2msg", data)],
                [channel(1, 7, "/root", "not-cdr")],
            );
            let definitions = validated_summary_definitions_for_test(&fixture);
            let state = source_state();
            let wire = canonical_policy_wire();
            let budget = budget(&state, &wire, generous_limits());
            let guard = AllocationGuard::start();
            let result = prepare(&definitions, &state, &wire, &budget);
            let allocations = AllocationGuard::count();
            drop(guard);
            assert_eq!(allocations, 0, "census must not allocate");
            drop(result);
            assert_eq!(*budget.state.usage.lock(), RemoteRos2BudgetUsage::default());
        }
    }

    #[test]
    fn strict_grammar_rejects_malformed_sections_names_types_arrays_and_literals() {
        let grammar = RemoteRos2InitializationError::UnsupportedForRemote(
            UnsupportedForRemote::GrammarFeature,
        );
        let dependency = RemoteRos2InitializationError::UnsupportedForRemote(
            UnsupportedForRemote::DependencyGraph,
        );
        let malformed: &[(&str, &[u8], RemoteRos2InitializationError)] = &[
            (
                "pkg/Root",
                b"int32",
                RemoteRos2InitializationError::InvalidRemoteSchema,
            ),
            ("pkg/Root", b"int32 value\n===\n", grammar),
            (
                "pkg/Root",
                b"int32 value\n===\nMSG: \nuint32 other",
                grammar,
            ),
            ("pkg/Root", b"unknown<=2 value", grammar),
            (
                "pkg/Root",
                b"int32[bad] value",
                RemoteRos2InitializationError::InvalidRemoteSchema,
            ),
            ("pkg/Root", b"int32[2] value [1]", grammar),
            (
                "pkg/Root",
                b"bool value maybe",
                RemoteRos2InitializationError::InvalidRemoteSchema,
            ),
            ("pkg/Root", b"string value \"unterminated", grammar),
            ("pkg/Root", b"int32 value\nuint32 value", grammar),
            ("pkg/Root", b"int32 DUP=1\nint32 DUP=2", grammar),
            (
                "pkg/Root",
                b"A value\n===\nMSG: pkg/A\nint32 one\n===\nMSG: pkg/A\nint32 two",
                dependency,
            ),
            ("bad name", b"int32 value", grammar),
            (
                "pkg/Root",
                &[0xff],
                RemoteRos2InitializationError::InvalidRemoteSchema,
            ),
        ];
        for (name, data, expected) in malformed {
            let local = std::str::from_utf8(data)
                .ok()
                .map(|data| re_ros_msg::MessageSchema::parse(name, data));
            match expected {
                RemoteRos2InitializationError::InvalidRemoteSchema => {
                    assert!(local.is_none_or(|result| result.is_err()));
                }
                RemoteRos2InitializationError::UnsupportedForRemote(_) => {
                    assert!(
                        local.is_some_and(|result| result.is_ok()),
                        "UnsupportedForRemote is reserved for local-valid input"
                    );
                }
                _ => panic!("the classification table only contains grammar outcomes"),
            }
            let fixture = fixture(
                [schema(7, name, "ros2msg", *data)],
                [channel(1, 7, "/root", "not-cdr")],
            );
            let definitions = validated_summary_definitions_for_test(&fixture);
            let state = source_state();
            let wire = canonical_policy_wire();
            let budget = budget(&state, &wire, generous_limits());
            assert_eq!(
                prepare(&definitions, &state, &wire, &budget).err(),
                Some(*expected),
                "the invalid/unsupported classification must match local validity"
            );
            assert_eq!(*budget.state.usage.lock(), RemoteRos2BudgetUsage::default());
        }
    }

    #[test]
    fn local_valid_outside_subset_and_local_invalid_literals_have_stable_classification() {
        let grammar = RemoteRos2InitializationError::UnsupportedForRemote(
            UnsupportedForRemote::GrammarFeature,
        );
        for definition in [
            "int8 value 128",
            "int16 value 32768",
            "int32 value 2147483648",
            "uint8 value 256",
            "uint16 value 65536",
            "uint32 value 4294967296",
            "int8 TOO_LARGE=128",
            "int32 UpperCase",
            "int32 bad__name",
            "int32[] values [1,,2]",
            "int32[] values [1,2,]",
            "string value \"unterminated",
        ] {
            assert!(re_ros_msg::MessageSchema::parse("pkg/Root", definition).is_ok());
            let fixture = fixture(
                [schema(7, "pkg/Root", "ros2msg", definition.as_bytes())],
                [channel(1, 7, "/root", "cdr")],
            );
            let definitions = validated_summary_definitions_for_test(&fixture);
            let state = source_state();
            let wire = canonical_policy_wire();
            let budget = budget(&state, &wire, generous_limits());
            assert_eq!(
                prepare(&definitions, &state, &wire, &budget).err(),
                Some(grammar),
                "local-valid narrow overflow is outside the remote profile: {definition}"
            );
        }

        for definition in [
            "int64 value 999999999999999999999999",
            "uint64 value -1",
            "float64 value not-a-number",
        ] {
            assert!(re_ros_msg::MessageSchema::parse("pkg/Root", definition).is_err());
            let fixture = fixture(
                [schema(7, "pkg/Root", "ros2msg", definition.as_bytes())],
                [channel(1, 7, "/root", "cdr")],
            );
            let definitions = validated_summary_definitions_for_test(&fixture);
            let state = source_state();
            let wire = canonical_policy_wire();
            let budget = budget(&state, &wire, generous_limits());
            assert_eq!(
                prepare(&definitions, &state, &wire, &budget).err(),
                Some(RemoteRos2InitializationError::InvalidRemoteSchema),
                "local-invalid literal remains invalid: {definition}"
            );
        }

        for definition in [
            "float32 value 1e100",
            "float64 value -1.7976931348623157e308",
            "float32 VALUE=3.25",
            "float64 VALUE=-0.0",
        ] {
            assert!(re_ros_msg::MessageSchema::parse("pkg/Root", definition).is_ok());
            let fixture = fixture(
                [schema(7, "pkg/Root", "ros2msg", definition.as_bytes())],
                [channel(1, 7, "/root", "cdr")],
            );
            let definitions = validated_summary_definitions_for_test(&fixture);
            let state = source_state();
            let wire = canonical_policy_wire();
            let budget = budget(&state, &wire, generous_limits());
            let prepared = prepare(&definitions, &state, &wire, &budget)
                .expect("local-valid float defaults are admitted");
            drop(prepared);
        }
    }

    #[test]
    fn local_validity_precedes_strict_subset_classification() {
        let cases = [
            (
                "int32 lower=1",
                Some(RemoteRos2InitializationError::InvalidRemoteSchema),
            ),
            (
                "int32[1] CONST=[1]",
                Some(RemoteRos2InitializationError::InvalidRemoteSchema),
            ),
            ("string EMPTY=", None),
            (
                "Foo] value",
                Some(RemoteRos2InitializationError::UnsupportedForRemote(
                    UnsupportedForRemote::GrammarFeature,
                )),
            ),
            (
                "pkg/ value",
                Some(RemoteRos2InitializationError::InvalidRemoteSchema),
            ),
            (
                "/Type value",
                Some(RemoteRos2InitializationError::InvalidRemoteSchema),
            ),
            (
                "//Type value",
                Some(RemoteRos2InitializationError::UnsupportedForRemote(
                    UnsupportedForRemote::GrammarFeature,
                )),
            ),
            (
                "///Type value",
                Some(RemoteRos2InitializationError::UnsupportedForRemote(
                    UnsupportedForRemote::GrammarFeature,
                )),
            ),
            (
                "/pkg/Type value",
                Some(RemoteRos2InitializationError::UnsupportedForRemote(
                    UnsupportedForRemote::GrammarFeature,
                )),
            ),
            (
                "pkg//Type value",
                Some(RemoteRos2InitializationError::UnsupportedForRemote(
                    UnsupportedForRemote::GrammarFeature,
                )),
            ),
        ];
        for (definition, expected) in cases {
            let local_valid = re_ros_msg::MessageSchema::parse("pkg/Root", definition).is_ok();
            assert_eq!(
                local_valid,
                !matches!(
                    expected,
                    Some(RemoteRos2InitializationError::InvalidRemoteSchema)
                ),
                "fixture must freeze the local parser's validity boundary: {definition}"
            );
            let fixture = fixture(
                [schema(7, "pkg/Root", "ros2msg", definition.as_bytes())],
                [channel(1, 7, "/root", "cdr")],
            );
            let definitions = validated_summary_definitions_for_test(&fixture);
            let state = source_state();
            let wire = canonical_policy_wire();
            let budget = budget(&state, &wire, generous_limits());
            let outcome = prepare(&definitions, &state, &wire, &budget);
            assert_eq!(outcome.as_ref().err().copied(), expected, "{definition}");
            drop(outcome);
            assert_eq!(*budget.state.usage.lock(), RemoteRos2BudgetUsage::default());
        }
    }

    #[test]
    fn every_v1_limit_accepts_exact_boundary_and_rejects_the_next_unit() {
        let definition = concat!(
            "int32 count 2\n",
            "string<=8[<=3] labels [\"a\",\"b\"]\n",
            "Child child\n",
            "uint8 FLAG=1\n",
            "===\nMSG: pkg/Child\n",
            "float64 value 1.5\n",
        );
        let fixture = fixture(
            [schema(7, "pkg/Root", "ros2msg", definition.as_bytes())],
            [channel(1, 7, "/root", "not-cdr")],
        );
        let definitions = validated_summary_definitions_for_test(&fixture);
        let state = source_state();
        let wire = canonical_policy_wire();
        let generous_budget = budget(&state, &wire, generous_limits());
        let generous = prepare(&definitions, &state, &wire, &generous_budget).unwrap();
        let census = generous.census;
        drop(generous);

        let mut exact = generous_limits();
        exact.max_schemas = census.schemas;
        exact.max_definition_bytes = census.definition_bytes;
        exact.max_specifications = census.specifications;
        exact.max_fields = census.fields;
        exact.max_constants = census.constants;
        exact.max_members = census.members;
        exact.max_line_bytes =
            u64::try_from(definition.lines().map(str::len).max().unwrap()).unwrap();
        exact.max_token_bytes = u64::try_from("string<=8[<=3]".len()).unwrap();
        exact.max_identifier_bytes = u64::try_from("pkg/Child".len()).unwrap();
        exact.max_default_bytes = u64::try_from("[\"a\",\"b\"]".len()).unwrap();
        exact.max_string_literal_bytes = 1;
        exact.max_string_bound = 8;
        exact.max_array_bound = 3;
        exact.max_dependency_edges = census.dependency_edges;
        exact.max_dependency_depth = 1;
        exact.max_census_steps = census.census_steps;
        exact.max_materialization_steps = census.materialization_steps;
        exact.max_projection_steps = census.projection_steps;
        exact.max_recognition_steps = census.recognition_steps;
        exact.max_retained_bytes = census.retained_bytes;
        exact.max_working_bytes = census.working_bytes;

        let exact_budget = budget(&state, &wire, exact);
        let exact_prepared = prepare(&definitions, &state, &wire, &exact_budget)
            .expect("every exact V1 limit boundary is admitted");
        drop(exact_prepared);
        assert_eq!(
            *exact_budget.state.usage.lock(),
            RemoteRos2BudgetUsage::default()
        );

        macro_rules! rejects_one_less {
            ($field:ident, $kind:expr) => {{
                let mut limited = exact;
                limited.$field = limited.$field.checked_sub(1).unwrap();
                let limited_budget = budget(&state, &wire, limited);
                assert_eq!(
                    prepare(&definitions, &state, &wire, &limited_budget).err(),
                    Some(RemoteRos2InitializationError::ResourceLimitExceeded($kind)),
                    "one unit below the exact {} boundary must fail",
                    stringify!($field),
                );
                assert_eq!(
                    *limited_budget.state.usage.lock(),
                    RemoteRos2BudgetUsage::default()
                );
            }};
        }

        rejects_one_less!(max_schemas, RemoteRos2ResourceLimit::SchemaCount);
        rejects_one_less!(
            max_definition_bytes,
            RemoteRos2ResourceLimit::DefinitionBytes
        );
        rejects_one_less!(
            max_specifications,
            RemoteRos2ResourceLimit::SpecificationCount
        );
        rejects_one_less!(max_fields, RemoteRos2ResourceLimit::FieldCount);
        rejects_one_less!(max_constants, RemoteRos2ResourceLimit::ConstantCount);
        rejects_one_less!(max_members, RemoteRos2ResourceLimit::MemberCount);
        rejects_one_less!(max_line_bytes, RemoteRos2ResourceLimit::LineBytes);
        rejects_one_less!(max_token_bytes, RemoteRos2ResourceLimit::TokenBytes);
        rejects_one_less!(
            max_identifier_bytes,
            RemoteRos2ResourceLimit::IdentifierBytes
        );
        rejects_one_less!(max_default_bytes, RemoteRos2ResourceLimit::DefaultBytes);
        rejects_one_less!(
            max_string_literal_bytes,
            RemoteRos2ResourceLimit::StringLiteralBytes
        );
        rejects_one_less!(max_string_bound, RemoteRos2ResourceLimit::StringBound);
        rejects_one_less!(max_array_bound, RemoteRos2ResourceLimit::ArrayBound);
        rejects_one_less!(
            max_dependency_edges,
            RemoteRos2ResourceLimit::DependencyEdges
        );
        rejects_one_less!(
            max_dependency_depth,
            RemoteRos2ResourceLimit::DependencyDepth
        );
        rejects_one_less!(max_census_steps, RemoteRos2ResourceLimit::CensusSteps);
        rejects_one_less!(
            max_materialization_steps,
            RemoteRos2ResourceLimit::MaterializationSteps
        );
        rejects_one_less!(
            max_projection_steps,
            RemoteRos2ResourceLimit::ProjectionSteps
        );
        rejects_one_less!(
            max_recognition_steps,
            RemoteRos2ResourceLimit::RecognitionSteps
        );
        rejects_one_less!(max_retained_bytes, RemoteRos2ResourceLimit::RetainedBytes);
        rejects_one_less!(max_working_bytes, RemoteRos2ResourceLimit::WorkingBytes);
    }

    #[test]
    fn missing_duplicate_and_cyclic_dependencies_fail_before_reservation() {
        for (data, expected) in [
            (
                "Missing value",
                RemoteRos2InitializationError::UnsupportedForRemote(
                    UnsupportedForRemote::DependencyGraph,
                ),
            ),
            (
                "int32 value\nuint32 value",
                RemoteRos2InitializationError::UnsupportedForRemote(
                    UnsupportedForRemote::GrammarFeature,
                ),
            ),
            (
                concat!(
                    "A root\n",
                    "===\nMSG: pkg/A\nB next\n",
                    "===\nMSG: pkg/B\nA next\n"
                ),
                RemoteRos2InitializationError::UnsupportedForRemote(
                    UnsupportedForRemote::DependencyGraph,
                ),
            ),
        ] {
            let fixture = fixture(
                [schema(7, "pkg/Root", "ros2msg", data.as_bytes())],
                [channel(1, 7, "/root", "cdr")],
            );
            let definitions = validated_summary_definitions_for_test(&fixture);
            let state = source_state();
            let wire = canonical_policy_wire();
            let budget = budget(&state, &wire, generous_limits());
            assert_eq!(
                prepare(&definitions, &state, &wire, &budget).err(),
                Some(expected)
            );
            assert_eq!(*budget.state.usage.lock(), RemoteRos2BudgetUsage::default());
        }
    }

    #[test]
    fn stale_source_fails_without_budget_ownership() {
        let fixture = fixture(
            [schema(7, "pkg/Root", "ros2msg", b"int32 value")],
            [channel(1, 7, "/root", "cdr")],
        );
        let definitions = validated_summary_definitions_for_test(&fixture);
        let state = source_state();
        state.generation.set(2);
        let wire = canonical_policy_wire();
        let budget = budget(&state, &wire, generous_limits());
        assert_eq!(
            prepare(&definitions, &state, &wire, &budget).err(),
            Some(RemoteRos2InitializationError::StaleSource)
        );
        assert_eq!(*budget.state.usage.lock(), RemoteRos2BudgetUsage::default());
    }

    struct TestAllocationGate<'a> {
        fail_before: Option<usize>,
        stale_after: Option<usize>,
        source: Option<&'a RemoteDefinitionsSourceState>,
    }

    impl RemoteRos2ArenaAllocationGate for TestAllocationGate<'_> {
        fn before_allocation(
            &self,
            arena_index: usize,
            _layout: Layout,
        ) -> Result<(), RemoteRos2InitializationError> {
            if self.fail_before == Some(arena_index) {
                Err(RemoteRos2InitializationError::FallibleAllocationFailed)
            } else {
                Ok(())
            }
        }

        fn after_allocation(&self, arena_index: usize) {
            if self.stale_after == Some(arena_index)
                && let Some(source) = self.source
            {
                source.generation.set(2);
            }
        }
    }

    #[test]
    fn materialization_uses_exact_borrowed_arenas_and_retains_only_result_budget() {
        let fixture = fixture(
            [schema(
                7,
                "pkg/Root",
                "ros2msg",
                b"int32 count\nChild child\n===\nMSG: pkg/Child\nstring label\n",
            )],
            [channel(1, 7, "/root", "cdr")],
        );
        let definitions = validated_summary_definitions_for_test(&fixture);
        let state = source_state();
        let wire = canonical_policy_wire();
        let budget = budget(&state, &wire, generous_limits());
        let prepared = prepare(&definitions, &state, &wire, &budget).unwrap();
        let retained = prepared.census.retained_bytes;
        let result = materialize_remote_ros2_definitions_v1(prepared)
            .expect("the fully preflighted schema materializes");

        assert_eq!(result.schemas.len(), 1);
        assert_eq!(result.specifications.len(), 2);
        assert_eq!(result.members.len(), 3);
        assert_eq!(result.resolutions.len(), 1);
        assert_eq!(result.schemas.capacity(), 1);
        assert_eq!(result.specifications.capacity(), 2);
        assert_eq!(
            text_for_span(&definitions, result.specifications.get(0).unwrap().name,).unwrap(),
            "pkg/Root"
        );
        assert_eq!(
            text_for_span(&definitions, result.specifications.get(1).unwrap().name,).unwrap(),
            "pkg/Child"
        );
        assert_eq!(
            result
                .resolutions
                .get(0)
                .unwrap()
                .target_specification_index,
            1
        );
        assert_eq!(
            *budget.state.usage.lock(),
            RemoteRos2BudgetUsage {
                active_initializers: 0,
                working_bytes: 0,
                retained_results: 1,
                retained_bytes: retained,
            }
        );
        drop(result);
        assert_eq!(*budget.state.usage.lock(), RemoteRos2BudgetUsage::default());
    }

    #[test]
    fn empty_ros_set_still_advances_same_source_and_policy_typestate() {
        let fixture = fixture(
            [schema(7, "pkg.Message", "protobuf", [0x0a, 0x00])],
            [channel(1, 7, "/protobuf", "protobuf")],
        );
        let definitions = validated_summary_definitions_for_test(&fixture);
        let state = source_state();
        let wire = canonical_policy_wire();
        let budget = budget(&state, &wire, generous_limits());
        let result = materialize_remote_ros2_definitions_v1(
            prepare(&definitions, &state, &wire, &budget).unwrap(),
        )
        .expect("an empty ROS 2 set still advances the ordered typestate");
        assert!(result.schemas.as_slice().is_empty());
        assert!(result.specifications.as_slice().is_empty());
        assert!(result.members.as_slice().is_empty());
        assert!(result.resolutions.as_slice().is_empty());
        assert!(std::ptr::eq(result.source.definitions, &definitions));
        assert!(std::ptr::eq(result.policy.wire, &wire));
        drop(result);
        assert_eq!(*budget.state.usage.lock(), RemoteRos2BudgetUsage::default());
    }

    #[test]
    fn every_arena_failure_and_stale_checkpoint_rolls_back_exactly() {
        for fault_index in 0..4 {
            let fixture = fixture(
                [schema(
                    7,
                    "pkg/Root",
                    "ros2msg",
                    b"Child child\n===\nMSG: pkg/Child\nint32 value",
                )],
                [channel(1, 7, "/root", "cdr")],
            );
            let definitions = validated_summary_definitions_for_test(&fixture);
            let state = source_state();
            let wire = canonical_policy_wire();
            let budget = budget(&state, &wire, generous_limits());
            let prepared = prepare(&definitions, &state, &wire, &budget).unwrap();
            let gate = TestAllocationGate {
                fail_before: Some(fault_index),
                stale_after: None,
                source: None,
            };
            assert_eq!(
                materialize_remote_ros2_definitions_v1_with_gate(prepared, &gate).err(),
                Some(RemoteRos2InitializationError::FallibleAllocationFailed)
            );
            assert_eq!(*budget.state.usage.lock(), RemoteRos2BudgetUsage::default());
        }

        for stale_index in 0..4 {
            let fixture = fixture(
                [schema(
                    7,
                    "pkg/Root",
                    "ros2msg",
                    b"Child child\n===\nMSG: pkg/Child\nint32 value",
                )],
                [channel(1, 7, "/root", "cdr")],
            );
            let definitions = validated_summary_definitions_for_test(&fixture);
            let state = source_state();
            let wire = canonical_policy_wire();
            let budget = budget(&state, &wire, generous_limits());
            let prepared = prepare(&definitions, &state, &wire, &budget).unwrap();
            let gate = TestAllocationGate {
                fail_before: None,
                stale_after: Some(stale_index),
                source: Some(&state),
            };
            assert_eq!(
                materialize_remote_ros2_definitions_v1_with_gate(prepared, &gate).err(),
                Some(RemoteRos2InitializationError::StaleSource)
            );
            assert_eq!(*budget.state.usage.lock(), RemoteRos2BudgetUsage::default());
        }
    }

    #[test]
    fn admission_owner_rejects_cross_source_policy_and_aggregate_contention() {
        let fixture = fixture(
            [schema(7, "pkg/Root", "ros2msg", b"int32 value")],
            [channel(1, 7, "/root", "cdr")],
        );
        let definitions = validated_summary_definitions_for_test(&fixture);
        let source_a = source_state();
        let source_b = source_state();
        let wire_a = canonical_policy_wire();
        let wire_b = canonical_policy_wire();
        let mut budget_a = budget(&source_a, &wire_a, generous_limits());
        Arc::get_mut(&mut budget_a.state)
            .unwrap()
            .capacity
            .max_active_initializers = 1;

        assert_fatal_control_plane(|| {
            let _owner = begin_remote_ros2_admission_v1(
                RemoteDefinitionsCapability {
                    definitions: &definitions,
                    source: &source_b,
                    generation: 1,
                },
                freeze_remote_decoder_policy_v1(&wire_a).unwrap(),
                &budget_a,
            );
        });
        assert_fatal_control_plane(|| {
            let _owner = begin_remote_ros2_admission_v1(
                RemoteDefinitionsCapability {
                    definitions: &definitions,
                    source: &source_a,
                    generation: 1,
                },
                freeze_remote_decoder_policy_v1(&wire_b).unwrap(),
                &budget_a,
            );
        });

        let first = prepare(&definitions, &source_a, &wire_a, &budget_a).unwrap();
        assert_eq!(
            prepare(&definitions, &source_a, &wire_a, &budget_a).err(),
            Some(RemoteRos2InitializationError::ResourceLimitExceeded(
                RemoteRos2ResourceLimit::ReservationCapacity,
            ))
        );
        drop(first);
        assert_eq!(
            *budget_a.state.usage.lock(),
            RemoteRos2BudgetUsage::default()
        );
    }

    #[test]
    fn signature_conflict_precedes_budget_contention_and_spends_no_initializer_ownership() {
        let fixture = fixture(
            [
                schema(7, "pkg/A", "ros2msg", b"int32 value"),
                schema(8, "pkg/B", "ros2msg", b"uint32 value"),
            ],
            [channel(1, 7, "/same", "cdr"), channel(2, 8, "/same", "cdr")],
        );
        let definitions = validated_summary_definitions_for_test(&fixture);
        let state = source_state();
        let wire = canonical_policy_wire();
        let mut constrained = budget(&state, &wire, generous_limits());
        Arc::get_mut(&mut constrained.state)
            .unwrap()
            .capacity
            .max_active_initializers = 0;

        assert_eq!(
            preflight(&definitions, &state, &wire, &constrained).err(),
            Some(RemoteRos2InitializationError::ConflictingTopicDecoderSignature)
        );
        assert_eq!(
            *constrained.state.usage.lock(),
            RemoteRos2BudgetUsage::default()
        );
    }

    #[test]
    fn budget_identity_rejects_reopen_viewer_profile_and_generation_rebinding() {
        let fixture = fixture(
            [schema(7, "pkg/Root", "ros2msg", b"int32 value")],
            [channel(1, 7, "/root", "cdr")],
        );
        let definitions = validated_summary_definitions_for_test(&fixture);
        let state = source_state();
        let wire = canonical_policy_wire();
        let mut original = budget(&state, &wire, generous_limits());

        original.viewer_scope = &OTHER_VIEWER_SCOPE;
        assert_fatal_control_plane(|| {
            let _result = preflight(&definitions, &state, &wire, &original);
        });
        original.viewer_scope = &VIEWER_SCOPE;

        original.profile_scope = &OTHER_PROFILE_SCOPE;
        assert_fatal_control_plane(|| {
            let _result = preflight(&definitions, &state, &wire, &original);
        });
        original.profile_scope = &PROFILE_SCOPE;

        state.generation.set(2);
        original.generation = 2;
        assert_fatal_control_plane(|| {
            let _owner = begin_remote_ros2_admission_v1(
                RemoteDefinitionsCapability {
                    definitions: &definitions,
                    source: &state,
                    generation: 2,
                },
                freeze_remote_decoder_policy_v1(&wire).unwrap(),
                &original,
            );
        });
        assert_eq!(
            *original.state.usage.lock(),
            RemoteRos2BudgetUsage::default()
        );
    }

    #[test]
    fn all_cpu_phases_have_exact_independent_boundaries() {
        let definition = b"Child child\n===\nMSG: pkg/Child\nstring label \"ok\"\n";
        let fixture = fixture(
            [schema(7, "pkg/Root", "ros2msg", definition)],
            [channel(1, 7, "/root", "cdr")],
        );
        let definitions = validated_summary_definitions_for_test(&fixture);
        let state = source_state();
        let wire = canonical_policy_wire();
        let generous_budget = budget(&state, &wire, generous_limits());
        let signature = preflight(&definitions, &state, &wire, &generous_budget).unwrap();
        let exact_signature_steps = signature.owner.steps.consumed;
        drop(signature);
        let prepared = prepare(&definitions, &state, &wire, &generous_budget).unwrap();
        let census = prepared.census;
        drop(prepared);

        let mut exact = generous_limits();
        exact.max_signature_steps = exact_signature_steps;
        exact.max_census_steps = census.census_steps;
        exact.max_materialization_steps = census.materialization_steps;
        exact.max_projection_steps = census.projection_steps;
        exact.max_recognition_steps = census.recognition_steps;
        let exact_budget = budget(&state, &wire, exact);
        let exact_prepared = prepare(&definitions, &state, &wire, &exact_budget).unwrap();
        let initialized = materialize_remote_ros2_definitions_v1(exact_prepared).unwrap();
        let transition = initialized.into_protobuf_transition_v1().unwrap();
        let mut projection = transition.into_protobuf_projection_v1().unwrap();
        while projection.next_schema().unwrap().is_some() {}
        let mut continuation = projection.finish_eof_v1().unwrap();
        {
            let mut recognition = continuation.take_bound_recognition_for_test_v1().unwrap();
            while recognition.next_channel().unwrap().is_some() {}
        }
        drop(continuation);
        assert_eq!(
            *exact_budget.state.usage.lock(),
            RemoteRos2BudgetUsage::default()
        );

        for (phase, limits, expected) in [
            (
                "signature",
                UnfrozenRemoteRos2LimitsV1 {
                    max_signature_steps: exact_signature_steps - 1,
                    ..exact
                },
                RemoteRos2ResourceLimit::SignatureSteps,
            ),
            (
                "census",
                UnfrozenRemoteRos2LimitsV1 {
                    max_census_steps: census.census_steps - 1,
                    ..exact
                },
                RemoteRos2ResourceLimit::CensusSteps,
            ),
            (
                "materialization",
                UnfrozenRemoteRos2LimitsV1 {
                    max_materialization_steps: census.materialization_steps - 1,
                    ..exact
                },
                RemoteRos2ResourceLimit::MaterializationSteps,
            ),
            (
                "projection",
                UnfrozenRemoteRos2LimitsV1 {
                    max_projection_steps: census.projection_steps - 1,
                    ..exact
                },
                RemoteRos2ResourceLimit::ProjectionSteps,
            ),
            (
                "recognition",
                UnfrozenRemoteRos2LimitsV1 {
                    max_recognition_steps: census.recognition_steps - 1,
                    ..exact
                },
                RemoteRos2ResourceLimit::RecognitionSteps,
            ),
        ] {
            let limited_budget = budget(&state, &wire, limits);
            let guard = AllocationGuard::start();
            let error = prepare(&definitions, &state, &wire, &limited_budget).err();
            let allocations = AllocationGuard::count();
            drop(guard);
            assert_eq!(
                error,
                Some(RemoteRos2InitializationError::ResourceLimitExceeded(
                    expected
                )),
                "{phase} exact-minus-one must fail in its allocation-free phase"
            );
            assert_eq!(allocations, 0, "{phase} failure must precede allocation");
            assert_eq!(
                *limited_budget.state.usage.lock(),
                RemoteRos2BudgetUsage::default()
            );
        }

        assert_eq!(
            checked_remote_protobuf_projection_step_bound_v1(2, 14),
            Ok(14)
        );
        assert_eq!(
            checked_remote_protobuf_projection_step_bound_v1(2, 13),
            Err(RemoteRos2InitializationError::ResourceLimitExceeded(
                RemoteRos2ResourceLimit::ProjectionSteps
            ))
        );
        assert_eq!(
            checked_remote_protobuf_projection_step_bound_v1(u64::MAX, u64::MAX),
            Err(RemoteRos2InitializationError::ResourceLimitExceeded(
                RemoteRos2ResourceLimit::Arithmetic
            ))
        );
    }

    #[test]
    fn rescans_and_dependency_ancestor_checks_have_exact_step_ownership() {
        let mut number_steps =
            RemoteRos2StepOwnershipV1::with_limit(u64::MAX, RemoteRos2ResourceLimit::CensusSteps);
        assert_eq!(parse_u64("184467", &mut number_steps).unwrap(), 184_467);
        let exact_number_steps = number_steps.consumed;
        let mut number_minus_one = RemoteRos2StepOwnershipV1::with_limit(
            exact_number_steps - 1,
            RemoteRos2ResourceLimit::CensusSteps,
        );
        assert!(matches!(
            parse_u64("184467", &mut number_minus_one),
            Err(RemoteRos2InitializationError::ResourceLimitExceeded(
                RemoteRos2ResourceLimit::CensusSteps
            ))
        ));

        let limits = generous_limits();
        let type_schema = SchemaView {
            id: 7,
            record_index: 0,
            name: "pkg/Root",
            data: "pkg/Child[<=12] children",
        };
        let token = TextSlice {
            text: "pkg/Child[<=12]",
            absolute_start: 0,
        };
        let mut type_steps =
            RemoteRos2StepOwnershipV1::with_limit(u64::MAX, RemoteRos2ResourceLimit::CensusSteps);
        parse_type_v1(type_schema, token, &limits, &mut type_steps).unwrap();
        let exact_type_steps = type_steps.consumed;
        let mut type_minus_one = RemoteRos2StepOwnershipV1::with_limit(
            exact_type_steps - 1,
            RemoteRos2ResourceLimit::CensusSteps,
        );
        assert!(matches!(
            parse_type_v1(type_schema, token, &limits, &mut type_minus_one),
            Err(RemoteRos2InitializationError::ResourceLimitExceeded(
                RemoteRos2ResourceLimit::CensusSteps
            ))
        ));

        let fixture = fixture(
            [schema(
                7,
                "pkg/Root",
                "ros2msg",
                b"Child child\n===\nMSG: pkg/Child\nint32 value",
            )],
            [channel(1, 7, "/root", "cdr")],
        );
        let definitions = validated_summary_definitions_for_test(&fixture);
        let graph_schema = SchemaView {
            id: 7,
            record_index: 0,
            name: "pkg/Root",
            data: "Child child\n===\nMSG: pkg/Child\nint32 value",
        };
        let mut graph_steps =
            RemoteRos2StepOwnershipV1::with_limit(u64::MAX, RemoteRos2ResourceLimit::CensusSteps);
        validate_dependency_graph(
            Some(&definitions),
            graph_schema,
            2,
            &limits,
            &mut graph_steps,
            &mut RemoteRos2FixedScratchV1::new(),
        )
        .unwrap();
        let exact_graph_steps = graph_steps.consumed;
        let mut graph_minus_one = RemoteRos2StepOwnershipV1::with_limit(
            exact_graph_steps - 1,
            RemoteRos2ResourceLimit::CensusSteps,
        );
        assert!(matches!(
            validate_dependency_graph(
                Some(&definitions),
                graph_schema,
                2,
                &limits,
                &mut graph_minus_one,
                &mut RemoteRos2FixedScratchV1::new(),
            ),
            Err(RemoteRos2InitializationError::ResourceLimitExceeded(
                RemoteRos2ResourceLimit::CensusSteps
            ))
        ));

        let schema_record_index = definitions
            .projection_records()
            .find_map(|record| match record {
                SummaryDefinitionProjectionRecord::Schema { record_index, .. } => {
                    Some(record_index)
                }
                SummaryDefinitionProjectionRecord::Channel { .. } => None,
            })
            .unwrap();
        let span = SourceTextSpan {
            schema_record_index,
            kind: SourceTextKind::SchemaData,
            start: 0,
            len: 5,
        };
        let guard = AllocationGuard::start();
        assert_eq!(text_for_span(&definitions, span).unwrap(), "Child");
        assert_eq!(
            AllocationGuard::count(),
            0,
            "validated spans do not rescan UTF-8"
        );
        drop(guard);
    }

    #[test]
    fn work_reservation_invariant_failures_are_atomic_and_poison_the_budget() {
        let state = source_state();
        let wire = canonical_policy_wire();
        let budget = budget(&state, &wire, generous_limits());
        let mut reservation = budget.reserve(100, 80).unwrap();
        let before = *budget.state.usage.lock();
        assert_fatal_control_plane(|| {
            reservation.narrow(101, 80);
        });
        assert_eq!(*budget.state.usage.lock(), before);
        assert!(budget.state.poisoned.load(Ordering::Acquire));
        assert_fatal_control_plane(|| {
            let _reservation = budget.reserve(1, 1);
        });
        drop(reservation);
        assert_eq!(*budget.state.usage.lock(), RemoteRos2BudgetUsage::default());
    }

    #[test]
    fn locked_wasm_allocator_contract_and_fixed_scratch_are_precharged() {
        assert_eq!(
            locked_wasm_allocation_footprint_v1(Layout::from_size_align(1, 1).unwrap()).unwrap(),
            LOCKED_WASM_DLMALLOC_PAGE_V1
        );
        assert_eq!(
            locked_wasm_allocation_footprint_v1(Layout::from_size_align(65_500, 1).unwrap())
                .unwrap(),
            2 * LOCKED_WASM_DLMALLOC_PAGE_V1
        );
        assert!(
            locked_wasm_allocation_footprint_v1(Layout::from_size_align(1, 16).unwrap()).unwrap()
                >= LOCKED_WASM_DLMALLOC_PAGE_V1
        );
        assert!(sealed_fixed_working_bytes_v1().unwrap() > 0);

        let mut census = RemoteRos2Census {
            schemas: 1,
            specifications: 2,
            members: 3,
            dependency_edges: 1,
            ..RemoteRos2Census::default()
        };
        compute_arena_peak(&mut census, &generous_limits()).unwrap();
        let exact_retained = checked_add(
            checked_add(
                arena_allocation_footprint::<ParsedSchemaV1>(1).unwrap(),
                arena_allocation_footprint::<ParsedSpecificationV1>(2).unwrap(),
            )
            .unwrap(),
            checked_add(
                arena_allocation_footprint::<ParsedMemberV1>(3).unwrap(),
                arena_allocation_footprint::<ComplexTypeResolutionV1>(1).unwrap(),
            )
            .unwrap(),
        )
        .and_then(|bytes| checked_add(bytes, remote_ros2_result_inline_bytes_v1().unwrap()))
        .unwrap();
        assert_eq!(census.retained_bytes, exact_retained);
        assert_eq!(
            census.working_bytes,
            exact_retained + sealed_fixed_working_bytes_v1().unwrap()
        );
    }

    #[test]
    fn protobuf_transition_is_move_only_token_checked_and_keeps_reservation() {
        let fixture = fixture(
            [
                schema(7, "pkg/Root", "ros2msg", b"int32 value"),
                schema(8, "pkg.Message", "protobuf", [0x0a, 0x00]),
            ],
            [
                channel(1, 7, "/root", "cdr"),
                channel(2, 8, "/protobuf", "protobuf"),
            ],
        );
        let definitions = validated_summary_definitions_for_test(&fixture);
        let state = source_state();
        let wire = canonical_policy_wire();
        let budget = budget(&state, &wire, generous_limits());
        let result = materialize_remote_ros2_definitions_v1(
            prepare(&definitions, &state, &wire, &budget).unwrap(),
        )
        .unwrap();
        let retained = budget.state.usage.lock().retained_bytes;
        let transition = result.into_protobuf_transition_v1().unwrap();
        assert_eq!(transition.ros2_schema_count(), 1);
        assert!(transition.ensure_current().is_ok());
        {
            let mut projection = transition.projection_iter_v1().unwrap();
            let schema = projection.next_schema().unwrap().unwrap();
            assert_eq!(schema.schema_id().unwrap(), 7);
            assert_eq!(schema.specification_count().unwrap(), 1);
            assert_eq!(schema.specification_name(0).unwrap(), "pkg/Root");
            assert!(projection.next_schema().unwrap().is_none());
        }
        let projection = transition.into_protobuf_projection_v1().unwrap();
        let (protobuf_schema_count, mut continuation) =
            complete_projection_for_test_v1(projection).unwrap();
        assert_eq!(protobuf_schema_count, 1);
        {
            let mut recognition = continuation.take_bound_recognition_for_test_v1().unwrap();
            let ros2 = recognition.next_channel().unwrap().unwrap();
            assert_eq!(ros2.channel_id().unwrap(), 1);
            assert!(ros2.recognized_by_reflection().unwrap());
            let protobuf = recognition.next_channel().unwrap().unwrap();
            assert_eq!(protobuf.channel_id().unwrap(), 2);
            assert!(!protobuf.recognized_by_reflection().unwrap());
            assert!(recognition.next_channel().unwrap().is_none());
        }
        assert!(matches!(
            continuation.take_bound_recognition_for_test_v1(),
            Err(RemoteRos2InitializationError::ResourceLimitExceeded(
                RemoteRos2ResourceLimit::RecognitionSteps
            ))
        ));
        assert_eq!(budget.state.usage.lock().retained_bytes, retained);
        state.generation.set(2);
        assert!(matches!(
            continuation.ensure_current_for_test_v1(),
            Err(RemoteRos2InitializationError::StaleSource)
        ));
        drop(continuation);
        assert_eq!(*budget.state.usage.lock(), RemoteRos2BudgetUsage::default());
    }

    #[test]
    fn protobuf_eof_continuation_cannot_be_created_before_projection_eof() {
        let fixture = fixture(
            [schema(8, "pkg.Message", "protobuf", [0x0a, 0x00])],
            [channel(1, 8, "/protobuf", "protobuf")],
        );
        let definitions = validated_summary_definitions_for_test(&fixture);
        let state = source_state();
        let wire = canonical_policy_wire();
        let budget = budget(&state, &wire, generous_limits());
        let initialized = materialize_remote_ros2_definitions_v1(
            prepare(&definitions, &state, &wire, &budget).unwrap(),
        )
        .unwrap();
        let projection = initialized
            .into_protobuf_transition_v1()
            .unwrap()
            .into_protobuf_projection_v1()
            .unwrap();
        assert_fatal_control_plane(move || {
            let _eof = projection.finish_eof_v1();
        });
        assert_eq!(*budget.state.usage.lock(), RemoteRos2BudgetUsage::default());
    }

    #[test]
    fn protobuf_eof_continuations_keep_distinct_source_and_policy_owners() {
        let fixture_a = fixture(
            [schema(8, "pkg/A", "protobuf", [0x0a, 0x00])],
            [channel(1, 8, "/a", "protobuf")],
        );
        let fixture_b = fixture(
            [
                schema(9, "pkg/B", "protobuf", [0x0a, 0x00]),
                schema(10, "pkg/C", "protobuf", [0x0a, 0x00]),
            ],
            [
                channel(1, 9, "/b", "protobuf"),
                channel(2, 10, "/c", "protobuf"),
            ],
        );
        let definitions_a = validated_summary_definitions_for_test(&fixture_a);
        let definitions_b = validated_summary_definitions_for_test(&fixture_b);
        let state_a = source_state();
        let state_b = source_state();
        let wire_a = canonical_policy_wire();
        let wire_b = canonical_policy_wire();
        let budget_a = budget(&state_a, &wire_a, generous_limits());
        let budget_b = budget(&state_b, &wire_b, generous_limits());

        let projection_a = materialize_remote_ros2_definitions_v1(
            prepare(&definitions_a, &state_a, &wire_a, &budget_a).unwrap(),
        )
        .unwrap()
        .into_protobuf_transition_v1()
        .unwrap()
        .into_protobuf_projection_v1()
        .unwrap();
        let projection_b = materialize_remote_ros2_definitions_v1(
            prepare(&definitions_b, &state_b, &wire_b, &budget_b).unwrap(),
        )
        .unwrap()
        .into_protobuf_transition_v1()
        .unwrap()
        .into_protobuf_projection_v1()
        .unwrap();
        let (schema_count_a, continuation_a) =
            complete_projection_for_test_v1(projection_a).unwrap();
        let (schema_count_b, continuation_b) =
            complete_projection_for_test_v1(projection_b).unwrap();
        assert_eq!(schema_count_a, 1);
        assert_eq!(schema_count_b, 2);

        state_a.generation.set(2);
        assert!(matches!(
            continuation_a.ensure_current_for_test_v1(),
            Err(RemoteRos2InitializationError::StaleSource)
        ));
        assert!(continuation_b.ensure_current_for_test_v1().is_ok());
        drop(continuation_a);
        drop(continuation_b);
        assert_eq!(
            *budget_a.state.usage.lock(),
            RemoteRos2BudgetUsage::default()
        );
        assert_eq!(
            *budget_b.state.usage.lock(),
            RemoteRos2BudgetUsage::default()
        );
    }

    fn local_type_from_remote(
        definitions: &ValidatedSummaryDefinitions<'_>,
        ty: ParsedTypeV1,
    ) -> re_ros_msg::message_spec::Type {
        use re_ros_msg::message_spec::{ArraySize, BuiltInType, ComplexType, Type};

        let element = match ty.element {
            ElementTypeV1::Primitive(primitive) => Type::BuiltIn(match primitive {
                PrimitiveTypeV1::Bool => BuiltInType::Bool,
                PrimitiveTypeV1::Byte => BuiltInType::Byte,
                PrimitiveTypeV1::Char => BuiltInType::Char,
                PrimitiveTypeV1::Float32 => BuiltInType::Float32,
                PrimitiveTypeV1::Float64 => BuiltInType::Float64,
                PrimitiveTypeV1::Int8 => BuiltInType::Int8,
                PrimitiveTypeV1::Int16 => BuiltInType::Int16,
                PrimitiveTypeV1::Int32 => BuiltInType::Int32,
                PrimitiveTypeV1::Int64 => BuiltInType::Int64,
                PrimitiveTypeV1::UInt8 => BuiltInType::UInt8,
                PrimitiveTypeV1::UInt16 => BuiltInType::UInt16,
                PrimitiveTypeV1::UInt32 => BuiltInType::UInt32,
                PrimitiveTypeV1::UInt64 => BuiltInType::UInt64,
                PrimitiveTypeV1::String { bound } => {
                    BuiltInType::String(bound.map(|bound| usize::try_from(bound).unwrap()))
                }
            }),
            ElementTypeV1::Complex(span) => {
                let name = text_for_span(definitions, span).unwrap();
                Type::Complex(if let Some((package, name)) = name.rsplit_once('/') {
                    ComplexType::Absolute {
                        package: package.to_owned(),
                        name: name.to_owned(),
                    }
                } else {
                    ComplexType::Relative {
                        name: name.to_owned(),
                    }
                })
            }
        };
        match ty.array {
            ArraySizeV1::Scalar => element,
            ArraySizeV1::Fixed(size) => Type::Array {
                ty: Box::new(element),
                size: ArraySize::Fixed(usize::try_from(size).unwrap()),
            },
            ArraySizeV1::Bounded(size) => Type::Array {
                ty: Box::new(element),
                size: ArraySize::Bounded(usize::try_from(size).unwrap()),
            },
            ArraySizeV1::Unbounded => Type::Array {
                ty: Box::new(element),
                size: ArraySize::Unbounded,
            },
        }
    }

    fn semantic_literal_from_remote(
        input: &str,
        ty: ParsedTypeV1,
    ) -> re_ros_msg::message_spec::Literal {
        use re_ros_msg::message_spec::Literal;

        if !matches!(ty.array, ArraySizeV1::Scalar) {
            let inner = input.strip_prefix('[').unwrap().strip_suffix(']').unwrap();
            let scalar = ParsedTypeV1 {
                element: ty.element,
                array: ArraySizeV1::Scalar,
            };
            return Literal::Array(
                inner
                    .split(',')
                    .map(str::trim)
                    .filter(|element| !element.is_empty())
                    .map(|element| semantic_literal_from_remote(element, scalar))
                    .collect(),
            );
        }

        match ty.element {
            ElementTypeV1::Primitive(PrimitiveTypeV1::Bool) => {
                Literal::Bool(input.parse().unwrap())
            }
            ElementTypeV1::Primitive(
                PrimitiveTypeV1::Char
                | PrimitiveTypeV1::Int8
                | PrimitiveTypeV1::Int16
                | PrimitiveTypeV1::Int32
                | PrimitiveTypeV1::Int64,
            ) => Literal::Int(input.parse().unwrap()),
            ElementTypeV1::Primitive(
                PrimitiveTypeV1::Byte
                | PrimitiveTypeV1::UInt8
                | PrimitiveTypeV1::UInt16
                | PrimitiveTypeV1::UInt32
                | PrimitiveTypeV1::UInt64,
            ) => Literal::UInt(input.parse().unwrap()),
            ElementTypeV1::Primitive(PrimitiveTypeV1::Float32 | PrimitiveTypeV1::Float64) => {
                Literal::Float(input.parse().unwrap())
            }
            ElementTypeV1::Primitive(PrimitiveTypeV1::String { .. }) => {
                let value = if (input.starts_with('"') && input.ends_with('"'))
                    || (input.starts_with('\'') && input.ends_with('\''))
                {
                    &input[1..input.len() - 1]
                } else {
                    input
                };
                Literal::String(value.to_owned())
            }
            ElementTypeV1::Complex(_) => panic!("complex defaults are outside the admitted subset"),
        }
    }

    fn test_only_local_schema_from_bounded(
        result: &Ros2InitializedRemoteDefinitionsV1<'_, '_, '_, '_>,
    ) -> re_ros_msg::MessageSchema {
        use re_ros_msg::message_spec::{Field, MessageSpecification};

        let definitions = result.source.definitions;
        let schema = result.schemas.get(0).unwrap();
        let mut specifications = Vec::new();
        for specification_index in schema.specifications.clone() {
            let specification = result.specifications.get(specification_index).unwrap();
            let mut fields = Vec::new();
            for member_index in specification.members.clone() {
                let member = *result.members.get(member_index).unwrap();
                assert_ne!(member.kind, ParsedMemberKindV1::Constant);
                assert!(member.literal.is_none());
                fields.push(Field {
                    ty: local_type_from_remote(definitions, member.ty),
                    name: text_for_span(definitions, member.name).unwrap().to_owned(),
                    default: None,
                });
            }
            specifications.push(MessageSpecification {
                name: text_for_span(definitions, specification.name)
                    .unwrap()
                    .to_owned(),
                fields,
                constants: Vec::new(),
            });
        }
        re_ros_msg::MessageSchema {
            spec: specifications.remove(0),
            dependencies: specifications,
        }
    }

    fn remote_enum_underlying(
        result: &Ros2InitializedRemoteDefinitionsV1<'_, '_, '_, '_>,
        specification_index: usize,
    ) -> re_cdr::Result<Option<PrimitiveTypeV1>> {
        let specification = result
            .specifications
            .get(specification_index)
            .map_err(|error| re_cdr::Error::Custom(error.to_string()))?;
        let mut first = None;
        let mut constants = 0_usize;
        for member_index in specification.members.clone() {
            let member = *result
                .members
                .get(member_index)
                .map_err(|error| re_cdr::Error::Custom(error.to_string()))?;
            match member.kind {
                ParsedMemberKindV1::Field | ParsedMemberKindV1::SyntheticPadding => {
                    return Ok(None);
                }
                ParsedMemberKindV1::Constant => {
                    constants += 1;
                    let ElementTypeV1::Primitive(primitive) = member.ty.element else {
                        return Err(re_cdr::Error::Custom(
                            "remote constant has a non-primitive type".to_owned(),
                        ));
                    };
                    if first.is_some_and(|first| first != primitive) {
                        return Ok(None);
                    }
                    first = Some(primitive);
                }
            }
        }
        Ok((constants > 0
            && first.is_some_and(|primitive| {
                matches!(
                    primitive,
                    PrimitiveTypeV1::Bool
                        | PrimitiveTypeV1::Byte
                        | PrimitiveTypeV1::Char
                        | PrimitiveTypeV1::Int8
                        | PrimitiveTypeV1::UInt8
                )
            }))
        .then_some(first)
        .flatten())
    }

    fn decode_remote_message(
        result: &Ros2InitializedRemoteDefinitionsV1<'_, '_, '_, '_>,
        reader: &mut re_cdr::CdrReader<'_, byteorder::LittleEndian>,
        specification_index: usize,
    ) -> re_cdr::Result<re_ros_msg::deserialize::Value> {
        let specification = result
            .specifications
            .get(specification_index)
            .map_err(|error| re_cdr::Error::Custom(error.to_string()))?;
        let mut fields = std::collections::BTreeMap::new();
        for member_index in specification.members.clone() {
            let member = *result
                .members
                .get(member_index)
                .map_err(|error| re_cdr::Error::Custom(error.to_string()))?;
            if member.kind == ParsedMemberKindV1::Constant {
                continue;
            }
            let name = text_for_span(result.source.definitions, member.name)
                .map_err(|error| re_cdr::Error::Custom(error.to_string()))?
                .to_owned();
            let value = decode_remote_value(result, reader, member_index, member.ty)?;
            fields.insert(name, value);
        }
        Ok(re_ros_msg::deserialize::Value::Message(fields))
    }

    fn decode_remote_value(
        result: &Ros2InitializedRemoteDefinitionsV1<'_, '_, '_, '_>,
        reader: &mut re_cdr::CdrReader<'_, byteorder::LittleEndian>,
        member_index: usize,
        ty: ParsedTypeV1,
    ) -> re_cdr::Result<re_ros_msg::deserialize::Value> {
        let count = match ty.array {
            ArraySizeV1::Scalar => None,
            ArraySizeV1::Fixed(count) => Some(
                usize::try_from(count).map_err(|error| re_cdr::Error::Custom(error.to_string()))?,
            ),
            ArraySizeV1::Bounded(_) | ArraySizeV1::Unbounded => {
                Some(reader.read_sequence_length()?)
            }
        };
        if let Some(count) = count {
            let fixed = matches!(ty.array, ArraySizeV1::Fixed(_));
            if let ElementTypeV1::Primitive(primitive) = ty.element {
                let array = decode_remote_primitive_array(reader, primitive, count)?;
                return Ok(if fixed {
                    re_ros_msg::deserialize::Value::PrimitiveArray(array)
                } else {
                    re_ros_msg::deserialize::Value::PrimitiveSeq(array)
                });
            }
            let mut values = Vec::with_capacity(count);
            for _element in 0..count {
                values.push(decode_remote_scalar(
                    result,
                    reader,
                    member_index,
                    ty.element,
                )?);
            }
            return Ok(if fixed {
                re_ros_msg::deserialize::Value::Array(values)
            } else {
                re_ros_msg::deserialize::Value::Sequence(values)
            });
        }
        decode_remote_scalar(result, reader, member_index, ty.element)
    }

    fn decode_remote_scalar(
        result: &Ros2InitializedRemoteDefinitionsV1<'_, '_, '_, '_>,
        reader: &mut re_cdr::CdrReader<'_, byteorder::LittleEndian>,
        member_index: usize,
        element: ElementTypeV1,
    ) -> re_cdr::Result<re_ros_msg::deserialize::Value> {
        match element {
            ElementTypeV1::Primitive(primitive) => decode_remote_primitive(reader, primitive),
            ElementTypeV1::Complex(_) => {
                let target = result
                    .resolutions
                    .as_slice()
                    .iter()
                    .find(|resolution| resolution.member_index == member_index)
                    .ok_or_else(|| {
                        re_cdr::Error::Custom("missing remote complex-type resolution".to_owned())
                    })?
                    .target_specification_index;
                if let Some(primitive) = remote_enum_underlying(result, target)? {
                    decode_remote_primitive(reader, primitive)
                } else {
                    decode_remote_message(result, reader, target)
                }
            }
        }
    }

    fn decode_remote_primitive(
        reader: &mut re_cdr::CdrReader<'_, byteorder::LittleEndian>,
        primitive: PrimitiveTypeV1,
    ) -> re_cdr::Result<re_ros_msg::deserialize::Value> {
        use re_ros_msg::deserialize::Value;
        Ok(match primitive {
            PrimitiveTypeV1::Bool => Value::Bool(reader.read_bool()?),
            PrimitiveTypeV1::Byte | PrimitiveTypeV1::Char | PrimitiveTypeV1::UInt8 => {
                Value::U8(reader.read_u8()?)
            }
            PrimitiveTypeV1::Int8 => Value::I8(reader.read_i8()?),
            PrimitiveTypeV1::Int16 => Value::I16(reader.read_i16()?),
            PrimitiveTypeV1::Int32 => Value::I32(reader.read_i32()?),
            PrimitiveTypeV1::Int64 => Value::I64(reader.read_i64()?),
            PrimitiveTypeV1::UInt16 => Value::U16(reader.read_u16()?),
            PrimitiveTypeV1::UInt32 => Value::U32(reader.read_u32()?),
            PrimitiveTypeV1::UInt64 => Value::U64(reader.read_u64()?),
            PrimitiveTypeV1::Float32 => Value::F32(reader.read_f32()?),
            PrimitiveTypeV1::Float64 => Value::F64(reader.read_f64()?),
            PrimitiveTypeV1::String { .. } => Value::String(reader.read_string()?),
        })
    }

    fn decode_remote_primitive_array(
        reader: &mut re_cdr::CdrReader<'_, byteorder::LittleEndian>,
        primitive: PrimitiveTypeV1,
        count: usize,
    ) -> re_cdr::Result<re_ros_msg::deserialize::primitive_array::PrimitiveArray> {
        use re_ros_msg::deserialize::primitive_array::PrimitiveArray;
        Ok(match primitive {
            PrimitiveTypeV1::Bool => PrimitiveArray::Bool(
                (0..count)
                    .map(|_element| reader.read_bool())
                    .collect::<re_cdr::Result<_>>()?,
            ),
            PrimitiveTypeV1::Byte | PrimitiveTypeV1::Char | PrimitiveTypeV1::UInt8 => {
                PrimitiveArray::U8(reader.read_numeric_vec(count)?)
            }
            PrimitiveTypeV1::Int8 => PrimitiveArray::I8(reader.read_numeric_vec(count)?),
            PrimitiveTypeV1::Int16 => PrimitiveArray::I16(reader.read_numeric_vec(count)?),
            PrimitiveTypeV1::Int32 => PrimitiveArray::I32(reader.read_numeric_vec(count)?),
            PrimitiveTypeV1::Int64 => PrimitiveArray::I64(reader.read_numeric_vec(count)?),
            PrimitiveTypeV1::UInt16 => PrimitiveArray::U16(reader.read_numeric_vec(count)?),
            PrimitiveTypeV1::UInt32 => PrimitiveArray::U32(reader.read_numeric_vec(count)?),
            PrimitiveTypeV1::UInt64 => PrimitiveArray::U64(reader.read_numeric_vec(count)?),
            PrimitiveTypeV1::Float32 => PrimitiveArray::F32(reader.read_numeric_vec(count)?),
            PrimitiveTypeV1::Float64 => PrimitiveArray::F64(reader.read_numeric_vec(count)?),
            PrimitiveTypeV1::String { .. } => PrimitiveArray::String(
                (0..count)
                    .map(|_element| reader.read_string())
                    .collect::<re_cdr::Result<_>>()?,
            ),
        })
    }

    #[test]
    fn admitted_schema_and_legal_payload_match_local_reflection() {
        let definition = b"Child child\n===\nMSG: pkg/Child\nint32 value\n";
        let fixture = fixture(
            [schema(7, "pkg/Root", "ros2msg", definition)],
            [channel(1, 7, "/root", "cdr")],
        );
        let definitions = validated_summary_definitions_for_test(&fixture);
        let state = source_state();
        let wire = canonical_policy_wire();
        let budget = budget(&state, &wire, generous_limits());
        let result = materialize_remote_ros2_definitions_v1(
            prepare(&definitions, &state, &wire, &budget).unwrap(),
        )
        .unwrap();
        let bounded_projection = test_only_local_schema_from_bounded(&result);
        let local =
            re_ros_msg::MessageSchema::parse("pkg/Root", std::str::from_utf8(definition).unwrap())
                .unwrap();
        assert_eq!(bounded_projection, local);

        let mut payload = vec![0, 1, 0, 0]; // little-endian CDR encapsulation
        payload.extend_from_slice(&42_i32.to_le_bytes());
        let mut remote_reader = re_cdr::CdrReader::<byteorder::LittleEndian>::new(&payload[4..]);
        let remote_value = decode_remote_message(&result, &mut remote_reader, 0).unwrap();
        let resolver = re_ros_msg::deserialize::MapResolver::new(
            local
                .dependencies
                .iter()
                .map(|dependency| (dependency.name.clone(), dependency)),
        );
        let mut local_reader = re_cdr::CdrReader::<byteorder::LittleEndian>::new(&payload[4..]);
        let local_value =
            re_ros_msg::deserialize::decode_message(&mut local_reader, &local.spec, &resolver)
                .unwrap();
        assert_eq!(remote_value, local_value);
    }

    #[test]
    fn admitted_owner_recognition_matches_local_reflection_channels() {
        use crate::MessageDecoder as _;
        use crate::decoders::McapRos2ReflectionDecoder;

        let fixture = fixture(
            [
                schema(7, "std_msgs/msg/String", "ros2msg", b"string data"),
                schema(8, "pkg.Message", "protobuf", [0x0a, 0x00]),
            ],
            [
                channel(1, 7, "/cdr", "cdr"),
                channel(2, 7, "/non_cdr", "json"),
                FixtureChannel::schema_less(3, "/schema_less"),
                channel(4, 8, "/protobuf", "protobuf"),
                channel(5, 7, "/second", "CDR"),
            ],
        );
        let definitions = validated_summary_definitions_for_test(&fixture);
        let state = source_state();
        let wire = canonical_policy_wire();
        let budget = budget(&state, &wire, generous_limits());
        let remote = materialize_remote_ros2_definitions_v1(
            prepare(&definitions, &state, &wire, &budget).unwrap(),
        )
        .unwrap();
        let transition = remote.into_protobuf_transition_v1().unwrap();
        let mut projection = transition.into_protobuf_projection_v1().unwrap();
        while projection.next_schema().unwrap().is_some() {}
        let mut continuation = projection.finish_eof_v1().unwrap();
        let remote_recognition = {
            let mut recognition = continuation.take_bound_recognition_for_test_v1().unwrap();
            let mut remote_recognition = Vec::new();
            while let Some(channel) = recognition.next_channel().unwrap() {
                remote_recognition.push((
                    channel.channel_id().unwrap(),
                    channel.recognized_by_reflection().unwrap(),
                ));
            }
            remote_recognition
        };

        let summary = fixture.read_upstream_summary().unwrap().unwrap();
        let mut local = McapRos2ReflectionDecoder::default();
        local.init(&summary).unwrap();
        assert_eq!(remote_recognition.len(), summary.channels.len());
        for (channel_id, remote_supports) in remote_recognition {
            assert_eq!(
                remote_supports,
                local.supports_channel(&summary.channels[&channel_id]),
                "remote bounded recognition must match the local owner for channel {channel_id}"
            );
        }
        drop(continuation);
        assert_eq!(*budget.state.usage.lock(), RemoteRos2BudgetUsage::default());
    }

    #[test]
    fn admitted_subset_matches_local_schema_for_constants_defaults_arrays_and_resolution() {
        let definition = concat!(
            "bool enabled true\n",
            "int32 count -2\n",
            "string<=8 label \"ok\"\n",
            "uint16[3] fixed [1,2,3]\n",
            "float32[<=2] samples [1.0,2.0]\n",
            "Child relative_child\n",
            "pkg/Child absolute_child\n",
            "uint8 MODE=2\n",
            "string NAME=\"x\"\n",
            "===\nMSG: pkg/Child\n",
            "uint64 value\n",
        );
        let fixture = fixture(
            [schema(7, "pkg/Root", "ros2msg", definition.as_bytes())],
            [channel(1, 7, "/root", "cdr")],
        );
        let definitions = validated_summary_definitions_for_test(&fixture);
        let state = source_state();
        let wire = canonical_policy_wire();
        let budget = budget(&state, &wire, generous_limits());
        let result = materialize_remote_ros2_definitions_v1(
            prepare(&definitions, &state, &wire, &budget).unwrap(),
        )
        .unwrap();
        let local = re_ros_msg::MessageSchema::parse("pkg/Root", definition).unwrap();

        assert_eq!(result.specifications.len(), 2);
        let main = result.specifications.get(0).unwrap();
        let remote_members = &result.members.as_slice()[main.members.clone()];
        assert_eq!(
            remote_members.len(),
            local.spec.fields.len() + local.spec.constants.len()
        );
        for member in remote_members {
            let name = text_for_span(&definitions, member.name).unwrap();
            match member.kind {
                ParsedMemberKindV1::Field => {
                    let field = local
                        .spec
                        .fields
                        .iter()
                        .find(|field| field.name == name)
                        .unwrap();
                    assert_eq!(local_type_from_remote(&definitions, member.ty), field.ty);
                    assert_eq!(member.literal.is_some(), field.default.is_some());
                    assert_eq!(
                        member.literal.map(|literal| semantic_literal_from_remote(
                            text_for_span(&definitions, literal).unwrap(),
                            member.ty,
                        )),
                        field.default,
                        "remote field defaults must match the local parsed value"
                    );
                }
                ParsedMemberKindV1::Constant => {
                    let constant = local
                        .spec
                        .constants
                        .iter()
                        .find(|constant| constant.name == name)
                        .unwrap();
                    assert_eq!(local_type_from_remote(&definitions, member.ty), constant.ty);
                    assert_eq!(
                        semantic_literal_from_remote(
                            text_for_span(&definitions, member.literal.unwrap()).unwrap(),
                            member.ty,
                        ),
                        constant.value,
                        "remote constants must match the local parsed value"
                    );
                }
                ParsedMemberKindV1::SyntheticPadding => {
                    panic!("a specification with fields is not padded")
                }
            }
        }
        for (name, literal) in [
            ("enabled", "true"),
            ("count", "-2"),
            ("label", "\"ok\""),
            ("fixed", "[1,2,3]"),
            ("samples", "[1.0,2.0]"),
            ("MODE", "2"),
            ("NAME", "\"x\""),
        ] {
            let member = remote_members
                .iter()
                .find(|member| text_for_span(&definitions, member.name).unwrap() == name)
                .unwrap();
            assert_eq!(
                text_for_span(&definitions, member.literal.unwrap()).unwrap(),
                literal
            );
        }
        assert_eq!(result.resolutions.len(), 2);
        assert!(
            result
                .resolutions
                .as_slice()
                .iter()
                .all(|resolution| resolution.target_specification_index == 1)
        );
        drop(result);
        assert_eq!(*budget.state.usage.lock(), RemoteRos2BudgetUsage::default());
    }

    #[test]
    fn multiple_legal_cdr_shapes_match_local_reflection_output() {
        let cases: &[(&str, &[u8])] = &[
            ("bool enabled\nuint32 count\n", &[1, 0, 0, 0, 42, 0, 0, 0]),
            ("string label\n", &[3, 0, 0, 0, b'o', b'k', 0]),
            ("uint16[3] values\n", &[1, 0, 2, 0, 3, 0]),
            ("uint8[] values\n", &[3, 0, 0, 0, 1, 2, 3]),
        ];
        for (definition, cdr_body) in cases {
            let fixture = fixture(
                [schema(7, "pkg/Root", "ros2msg", definition.as_bytes())],
                [channel(1, 7, "/root", "cdr")],
            );
            let definitions = validated_summary_definitions_for_test(&fixture);
            let state = source_state();
            let wire = canonical_policy_wire();
            let budget = budget(&state, &wire, generous_limits());
            let result = materialize_remote_ros2_definitions_v1(
                prepare(&definitions, &state, &wire, &budget).unwrap(),
            )
            .unwrap();
            let local = re_ros_msg::MessageSchema::parse("pkg/Root", definition).unwrap();
            let local_resolver = re_ros_msg::deserialize::MapResolver::new(
                local
                    .dependencies
                    .iter()
                    .map(|dependency| (dependency.name.clone(), dependency)),
            );
            let mut remote_reader = re_cdr::CdrReader::<byteorder::LittleEndian>::new(cdr_body);
            let mut local_reader = re_cdr::CdrReader::<byteorder::LittleEndian>::new(cdr_body);
            let remote_value = decode_remote_message(&result, &mut remote_reader, 0).unwrap();
            let local_value = re_ros_msg::deserialize::decode_message(
                &mut local_reader,
                &local.spec,
                &local_resolver,
            )
            .unwrap();
            assert_eq!(remote_value, local_value);
            drop(result);
            assert_eq!(*budget.state.usage.lock(), RemoteRos2BudgetUsage::default());
        }
    }
}
