//! Bounded protobuf descriptor initialization for the Web remote-MCAP path.
//!
//! This module is crate-private and production-disarmed.
//! It consumes only the one-shot protobuf projection produced by the bounded ROS 2 initializer,
//! validates every descriptor wire byte before its first input-dependent allocation, and retains
//! the opaque post-EOF source/policy authority in the combined result.

#![allow(dead_code)]

use std::alloc::Layout;
use std::sync::Arc;

use sha2::{Digest as _, Sha256};
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;

use crate::remote_protobuf_projection_boundary::{
    RemoteProtobufProfileScopeV1, RemoteProtobufProjectionEofContinuationV1,
};
use crate::remote_ros2_reflection::{
    FrozenRemoteDecoderPolicyDescriptorViewV1, FrozenRemoteDecoderSlotV1,
    RemoteMcapSelectedMembershipV1, RemoteRos2ChannelRecognitionV1, RemoteRos2InitializationError,
    RemoteRos2RecognitionIterV1, RemoteViewerScopeState,
};

const REMOTE_PROTOBUF_PROFILE_VERSION_V1: u16 = 1;
// These are allocation-free projection ceilings, not the eventual production resource profile.
// The whole projection is carried by the sealed admission owner so no input-dependent allocation
// can precede the complete wire/graph census. Keep the object below the locked Wasm frame ceiling;
// MCAP-088 can only choose profile limits at or below these semantic V1 ceilings.
const MAX_INLINE_PROTOBUF_SCHEMAS_V1: usize = 8;
const MAX_INLINE_PROTOBUF_FILES_V1: usize = 16;
const MAX_INLINE_PROTOBUF_MESSAGES_V1: usize = 32;
const MAX_INLINE_PROTOBUF_ENUMS_V1: usize = 16;
const MAX_INLINE_PROTOBUF_SYMBOLS_V1: usize =
    MAX_INLINE_PROTOBUF_MESSAGES_V1 + MAX_INLINE_PROTOBUF_ENUMS_V1;
const MAX_INLINE_PROTOBUF_DEPENDENCIES_V1: usize = 64;
const MAX_INLINE_PROTOBUF_RESOLUTIONS_V1: usize = 512;
const MAX_STACK_DESCRIPTOR_DEPTH_V1: usize = 64;

// Rust 1.95.0 vendors dlmalloc 0.2.10 for wasm32 `System`.
const LOCKED_WASM_DLMALLOC_ALIGNMENT_V1: u64 = 8;
const LOCKED_WASM_DLMALLOC_CHUNK_OVERHEAD_V1: u64 = 4;
const LOCKED_WASM_DLMALLOC_MIN_CHUNK_V1: u64 = 16;
const LOCKED_WASM_DLMALLOC_TOP_FOOT_V1: u64 = 40;
const LOCKED_WASM_DLMALLOC_PAGE_V1: u64 = 64 * 1024;
const LOCKED_REMOTE_PROTOBUF_FRAME_SPILL_CEILING_V1: u64 = 16 * 1024;

macro_rules! remote_protobuf_artifact_stage_anchor {
    ($name:ident, $identity:literal) => {
        #[cfg(target_arch = "wasm32")]
        #[inline(never)]
        #[unsafe(no_mangle)]
        pub(crate) extern "C" fn $name() -> u32 {
            std::hint::black_box($identity)
        }
    };
}

remote_protobuf_artifact_stage_anchor!(rerun_remote_protobuf_wire_stage_v1, 0x2701_u32);
remote_protobuf_artifact_stage_anchor!(rerun_remote_protobuf_graph_stage_v1, 0x2702_u32);
remote_protobuf_artifact_stage_anchor!(rerun_remote_protobuf_peak_stage_v1, 0x2703_u32);
remote_protobuf_artifact_stage_anchor!(rerun_remote_protobuf_arena_stage_v1, 0x2704_u32);
remote_protobuf_artifact_stage_anchor!(rerun_remote_protobuf_recognition_stage_v1, 0x2705_u32);

#[cfg(target_arch = "wasm32")]
fn verify_artifact_stage_identity(actual: u32, expected: u32) {
    if actual != expected {
        protobuf_fatal_invariant("protobuf artifact stage identity changed");
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnsupportedRemoteProtobufV1 {
    UnknownDescriptorField,
    DescriptorFeature,
    DescriptorOption,
    DuplicateSingularField,
    RelativeTypeName,
    DependencyGraph,
    RecursiveBuilderGraph,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteProtobufResourceLimitV1 {
    SchemaCount,
    DescriptorBytes,
    FileCount,
    MessageCount,
    FieldCount,
    EnumCount,
    EnumValueCount,
    OneofCount,
    DependencyCount,
    OptionCount,
    StringBytes,
    DefaultBytes,
    SingleStringBytes,
    DescriptorDepth,
    ImportDepth,
    CensusSteps,
    MaterializationSteps,
    ResolutionSteps,
    RecognitionSteps,
    RetainedBytes,
    WorkingBytes,
    ReservationCapacity,
    Arithmetic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteProtobufInitializationErrorV1 {
    InvalidRemoteSchema,
    UnsupportedForRemote(UnsupportedRemoteProtobufV1),
    ResourceLimitExceeded(RemoteProtobufResourceLimitV1),
    FallibleAllocationFailed,
    StaleSource,
}

impl std::fmt::Display for RemoteProtobufInitializationErrorV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidRemoteSchema => "remote protobuf descriptor is invalid",
            Self::UnsupportedForRemote(UnsupportedRemoteProtobufV1::UnknownDescriptorField) => {
                "remote protobuf descriptor contains a field outside the V1 profile"
            }
            Self::UnsupportedForRemote(UnsupportedRemoteProtobufV1::DescriptorFeature) => {
                "remote protobuf descriptor uses a feature outside the V1 profile"
            }
            Self::UnsupportedForRemote(UnsupportedRemoteProtobufV1::DescriptorOption) => {
                "remote protobuf descriptor uses an option outside the V1 profile"
            }
            Self::UnsupportedForRemote(UnsupportedRemoteProtobufV1::DuplicateSingularField) => {
                "remote protobuf descriptor repeats a singular wire field"
            }
            Self::UnsupportedForRemote(UnsupportedRemoteProtobufV1::RelativeTypeName) => {
                "remote protobuf descriptor uses a relative type name outside the V1 profile"
            }
            Self::UnsupportedForRemote(UnsupportedRemoteProtobufV1::DependencyGraph) => {
                "remote protobuf descriptor dependency graph is unsupported"
            }
            Self::UnsupportedForRemote(UnsupportedRemoteProtobufV1::RecursiveBuilderGraph) => {
                "remote protobuf descriptor has a recursive builder graph"
            }
            Self::ResourceLimitExceeded(_) => "remote protobuf descriptor exceeds a resource limit",
            Self::FallibleAllocationFailed => "remote protobuf descriptor allocation failed",
            Self::StaleSource => "remote protobuf source is stale",
        })
    }
}

impl std::error::Error for RemoteProtobufInitializationErrorV1 {}

fn map_ros_error(error: RemoteRos2InitializationError) -> RemoteProtobufInitializationErrorV1 {
    match error {
        RemoteRos2InitializationError::StaleSource => {
            RemoteProtobufInitializationErrorV1::StaleSource
        }
        RemoteRos2InitializationError::ResourceLimitExceeded(_) => {
            RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                RemoteProtobufResourceLimitV1::CensusSteps,
            )
        }
        RemoteRos2InitializationError::InvalidRemoteSchema
        | RemoteRos2InitializationError::UnsupportedForRemote(_)
        | RemoteRos2InitializationError::FallibleAllocationFailed
        | RemoteRos2InitializationError::ConflictingTopicDecoderSignature
        | RemoteRos2InitializationError::SemanticConfigConflict => {
            RemoteProtobufInitializationErrorV1::InvalidRemoteSchema
        }
    }
}

fn map_ros_recognition_error(
    error: RemoteRos2InitializationError,
) -> RemoteProtobufInitializationErrorV1 {
    match error {
        RemoteRos2InitializationError::StaleSource => {
            RemoteProtobufInitializationErrorV1::StaleSource
        }
        RemoteRos2InitializationError::ResourceLimitExceeded(_) => {
            RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                RemoteProtobufResourceLimitV1::RecognitionSteps,
            )
        }
        RemoteRos2InitializationError::InvalidRemoteSchema
        | RemoteRos2InitializationError::UnsupportedForRemote(_)
        | RemoteRos2InitializationError::FallibleAllocationFailed
        | RemoteRos2InitializationError::ConflictingTopicDecoderSignature
        | RemoteRos2InitializationError::SemanticConfigConflict => {
            RemoteProtobufInitializationErrorV1::InvalidRemoteSchema
        }
    }
}

#[cold]
#[track_caller]
fn protobuf_fatal_invariant(reason: &'static str) -> ! {
    panic!("Fatal remote protobuf initializer invariant: {reason}")
}

#[cold]
#[track_caller]
fn protobuf_fatal_control_plane(reason: &'static str) -> ! {
    panic!("Fatal remote protobuf initializer control-plane mismatch: {reason}")
}

/// Remote-only measurement profile.
///
/// All values remain unfrozen until MCAP-088; no ordinary product constructor exists.
#[derive(Clone, Copy, Debug)]
pub(crate) struct UnfrozenRemoteProtobufLimitsV1 {
    profile_version: u16,
    max_schemas: u64,
    max_descriptor_bytes: u64,
    max_files: u64,
    max_messages: u64,
    max_fields: u64,
    max_enums: u64,
    max_enum_values: u64,
    max_oneofs: u64,
    max_dependencies: u64,
    max_options: u64,
    max_string_bytes: u64,
    max_default_bytes: u64,
    max_single_string_bytes: u64,
    max_descriptor_depth: u64,
    max_import_depth: u64,
    max_census_steps: u64,
    max_materialization_steps: u64,
    max_resolution_steps: u64,
    max_retained_bytes: u64,
    max_working_bytes: u64,
}

#[cfg(test)]
impl UnfrozenRemoteProtobufLimitsV1 {
    pub(crate) fn generous_for_assignment_test_v1() -> Self {
        Self {
            profile_version: REMOTE_PROTOBUF_PROFILE_VERSION_V1,
            max_schemas: MAX_INLINE_PROTOBUF_SCHEMAS_V1 as u64,
            max_descriptor_bytes: 1_000_000,
            max_files: MAX_INLINE_PROTOBUF_FILES_V1 as u64,
            max_messages: MAX_INLINE_PROTOBUF_MESSAGES_V1 as u64,
            max_fields: MAX_INLINE_PROTOBUF_RESOLUTIONS_V1 as u64,
            max_enums: MAX_INLINE_PROTOBUF_ENUMS_V1 as u64,
            max_enum_values: 1_024,
            max_oneofs: 512,
            max_dependencies: MAX_INLINE_PROTOBUF_DEPENDENCIES_V1 as u64,
            max_options: 512,
            max_string_bytes: 1_000_000,
            max_default_bytes: 1_000_000,
            max_single_string_bytes: 64_000,
            max_descriptor_depth: 32,
            max_import_depth: 32,
            max_census_steps: 10_000_000,
            max_materialization_steps: 10_000_000,
            max_resolution_steps: 10_000_000,
            max_retained_bytes: 64_000_000,
            max_working_bytes: 64_000_000,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RemoteProtobufBudgetUsageV1 {
    active_initializers: u64,
    working_bytes: u64,
    retained_results: u64,
    retained_bytes: u64,
}

#[derive(Clone, Copy, Debug)]
struct RemoteProtobufBudgetCapacityV1 {
    max_active_initializers: u64,
    max_working_bytes: u64,
    max_retained_results: u64,
    max_retained_bytes: u64,
}

struct RemoteProtobufBudgetStateV1 {
    limits: UnfrozenRemoteProtobufLimitsV1,
    capacity: RemoteProtobufBudgetCapacityV1,
    usage: Mutex<RemoteProtobufBudgetUsageV1>,
    poisoned: AtomicBool,
    viewer_scope_identity: usize,
    profile_scope_identity: usize,
}

/// Viewer/profile aggregate budget used by the production-disarmed protobuf initializer.
pub(crate) struct RemoteProtobufInitializationBudgetV1<'viewer, 'profile> {
    state: Arc<RemoteProtobufBudgetStateV1>,
    viewer_scope: &'viewer RemoteViewerScopeState,
    profile_scope: &'profile RemoteProtobufProfileScopeV1,
}

impl std::fmt::Debug for RemoteProtobufInitializationBudgetV1<'_, '_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RemoteProtobufInitializationBudgetV1")
            .field("limits", &self.state.limits)
            .field("capacity", &self.state.capacity)
            .finish_non_exhaustive()
    }
}

impl RemoteProtobufBudgetStateV1 {
    fn reserve(
        self: &Arc<Self>,
        working_bytes: u64,
        retained_bytes: u64,
    ) -> Result<RemoteProtobufWorkReservationV1, RemoteProtobufInitializationErrorV1> {
        if self.poisoned.load(Ordering::Acquire) {
            protobuf_fatal_control_plane("protobuf initializer budget is poisoned");
        }
        if working_bytes > self.limits.max_working_bytes {
            return Err(RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                RemoteProtobufResourceLimitV1::WorkingBytes,
            ));
        }
        if retained_bytes > self.limits.max_retained_bytes {
            return Err(RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                RemoteProtobufResourceLimitV1::RetainedBytes,
            ));
        }
        let mut usage = self.usage.lock();
        let next = RemoteProtobufBudgetUsageV1 {
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
            return Err(RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                RemoteProtobufResourceLimitV1::ReservationCapacity,
            ));
        }
        *usage = next;
        drop(usage);
        Ok(RemoteProtobufWorkReservationV1 {
            state: Some(Arc::clone(self)),
            working_bytes,
            retained_bytes,
        })
    }
}

struct RemoteProtobufWorkReservationV1 {
    state: Option<Arc<RemoteProtobufBudgetStateV1>>,
    working_bytes: u64,
    retained_bytes: u64,
}

impl RemoteProtobufWorkReservationV1 {
    fn complete(mut self) -> RemoteProtobufResultReservationV1 {
        let state = self
            .state
            .take()
            .unwrap_or_else(|| protobuf_fatal_invariant("completed reservation was reused"));
        let mut usage = state.usage.lock();
        usage.active_initializers = usage
            .active_initializers
            .checked_sub(1)
            .unwrap_or_else(|| poison_budget(&state, "active initializer accounting underflowed"));
        usage.working_bytes = usage
            .working_bytes
            .checked_sub(self.working_bytes)
            .unwrap_or_else(|| poison_budget(&state, "working-byte accounting underflowed"));
        drop(usage);
        RemoteProtobufResultReservationV1 {
            state,
            retained_bytes: self.retained_bytes,
        }
    }
}

impl Drop for RemoteProtobufWorkReservationV1 {
    fn drop(&mut self) {
        let Some(state) = self.state.take() else {
            return;
        };
        let mut usage = state.usage.lock();
        let Some(next) =
            checked_release_work_usage(*usage, self.working_bytes, self.retained_bytes)
        else {
            drop(usage);
            poison_budget(&state, "work reservation accounting underflowed");
        };
        *usage = next;
    }
}

struct RemoteProtobufResultReservationV1 {
    state: Arc<RemoteProtobufBudgetStateV1>,
    retained_bytes: u64,
}

impl Drop for RemoteProtobufResultReservationV1 {
    fn drop(&mut self) {
        let mut usage = self.state.usage.lock();
        let Some(retained_results) = usage.retained_results.checked_sub(1) else {
            drop(usage);
            poison_budget(&self.state, "result count accounting underflowed");
        };
        let Some(retained_bytes) = usage.retained_bytes.checked_sub(self.retained_bytes) else {
            drop(usage);
            poison_budget(&self.state, "result byte accounting underflowed");
        };
        usage.retained_results = retained_results;
        usage.retained_bytes = retained_bytes;
    }
}

fn poison_budget(state: &RemoteProtobufBudgetStateV1, reason: &'static str) -> ! {
    state.poisoned.store(true, Ordering::Release);
    protobuf_fatal_control_plane(reason)
}

fn checked_release_work_usage(
    usage: RemoteProtobufBudgetUsageV1,
    working_bytes: u64,
    retained_bytes: u64,
) -> Option<RemoteProtobufBudgetUsageV1> {
    Some(RemoteProtobufBudgetUsageV1 {
        active_initializers: usage.active_initializers.checked_sub(1)?,
        working_bytes: usage.working_bytes.checked_sub(working_bytes)?,
        retained_results: usage.retained_results.checked_sub(1)?,
        retained_bytes: usage.retained_bytes.checked_sub(retained_bytes)?,
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RemoteProtobufCensusV1 {
    schemas: u64,
    descriptor_bytes: u64,
    files: u64,
    messages: u64,
    fields: u64,
    enums: u64,
    enum_values: u64,
    oneofs: u64,
    dependencies: u64,
    options: u64,
    string_bytes: u64,
    default_bytes: u64,
    census_steps: u64,
    materialization_steps: u64,
    resolution_steps: u64,
    retained_bytes: u64,
    working_bytes: u64,
}

struct RemoteProtobufStepOwnerV1 {
    remaining: u64,
    consumed: u64,
    limit: RemoteProtobufResourceLimitV1,
}

impl RemoteProtobufStepOwnerV1 {
    fn new(remaining: u64, limit: RemoteProtobufResourceLimitV1) -> Self {
        Self {
            remaining,
            consumed: 0,
            limit,
        }
    }

    fn consume(&mut self) -> Result<(), RemoteProtobufInitializationErrorV1> {
        self.remaining = self.remaining.checked_sub(1).ok_or(
            RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(self.limit),
        )?;
        self.consumed = checked_add(self.consumed, 1)?;
        Ok(())
    }

    fn consume_bytes(&mut self, bytes: usize) -> Result<(), RemoteProtobufInitializationErrorV1> {
        for _ in 0..bytes {
            self.consume()?;
        }
        Ok(())
    }

    fn ensure_exhausted(&self) {
        if self.remaining != 0 {
            protobuf_fatal_invariant("protobuf materialization did not consume its exact steps");
        }
    }
}

fn checked_add(left: u64, right: u64) -> Result<u64, RemoteProtobufInitializationErrorV1> {
    left.checked_add(right)
        .ok_or(RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
            RemoteProtobufResourceLimitV1::Arithmetic,
        ))
}

fn checked_mul(left: u64, right: u64) -> Result<u64, RemoteProtobufInitializationErrorV1> {
    left.checked_mul(right)
        .ok_or(RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
            RemoteProtobufResourceLimitV1::Arithmetic,
        ))
}

fn checked_usize(value: u64) -> Result<usize, RemoteProtobufInitializationErrorV1> {
    usize::try_from(value).map_err(|_overflow| {
        RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
            RemoteProtobufResourceLimitV1::Arithmetic,
        )
    })
}

fn checked_u32(value: usize) -> Result<u32, RemoteProtobufInitializationErrorV1> {
    u32::try_from(value).map_err(|_overflow| {
        RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
            RemoteProtobufResourceLimitV1::Arithmetic,
        )
    })
}

fn increment_limited(
    value: &mut u64,
    add: u64,
    limit: u64,
    kind: RemoteProtobufResourceLimitV1,
) -> Result<(), RemoteProtobufInitializationErrorV1> {
    let next = checked_add(*value, add)?;
    if next > limit {
        return Err(RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
            kind,
        ));
    }
    *value = next;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ByteSpanV1 {
    schema_index: u16,
    start: u32,
    len: u32,
}

impl ByteSpanV1 {
    fn end(self) -> Result<usize, RemoteProtobufInitializationErrorV1> {
        let start = usize::try_from(self.start).map_err(|_overflow| {
            RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                RemoteProtobufResourceLimitV1::Arithmetic,
            )
        })?;
        let len = usize::try_from(self.len).map_err(|_overflow| {
            RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                RemoteProtobufResourceLimitV1::Arithmetic,
            )
        })?;
        start
            .checked_add(len)
            .ok_or(RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                RemoteProtobufResourceLimitV1::Arithmetic,
            ))
    }
}

#[derive(Clone, Copy)]
struct SchemaInputV1<'definitions> {
    schema_id: u16,
    name: &'definitions str,
    data: &'definitions [u8],
}

#[derive(Clone, Copy)]
struct InlineListV1<T: Copy, const N: usize> {
    items: [Option<T>; N],
    len: usize,
}

impl<T: Copy, const N: usize> InlineListV1<T, N> {
    fn new() -> Self {
        Self {
            items: [None; N],
            len: 0,
        }
    }

    fn push(
        &mut self,
        value: T,
        kind: RemoteProtobufResourceLimitV1,
    ) -> Result<usize, RemoteProtobufInitializationErrorV1> {
        let Some(slot) = self.items.get_mut(self.len) else {
            return Err(RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                kind,
            ));
        };
        *slot = Some(value);
        let index = self.len;
        self.len = self.len.checked_add(1).ok_or(
            RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                RemoteProtobufResourceLimitV1::Arithmetic,
            ),
        )?;
        Ok(index)
    }

    fn get(&self, index: usize) -> Result<T, RemoteProtobufInitializationErrorV1> {
        self.items
            .get(index)
            .and_then(|item| *item)
            .ok_or_else(|| protobuf_fatal_invariant("inline protobuf projection index escaped"))
    }

    fn as_slice(&self) -> &[Option<T>] {
        &self.items[..self.len]
    }
}

#[derive(Clone, Copy)]
struct WireReaderV1<'a> {
    schema_index: u16,
    bytes: &'a [u8],
    base: usize,
    position: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WireValueV1 {
    Varint(u64),
    Fixed64,
    Bytes(ByteSpanV1),
    Fixed32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WireFieldV1 {
    number: u32,
    value: WireValueV1,
}

impl<'a> WireReaderV1<'a> {
    fn root(
        schema_index: usize,
        bytes: &'a [u8],
    ) -> Result<Self, RemoteProtobufInitializationErrorV1> {
        Ok(Self {
            schema_index: u16::try_from(schema_index).map_err(|_overflow| {
                RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                    RemoteProtobufResourceLimitV1::Arithmetic,
                )
            })?,
            bytes,
            base: 0,
            position: 0,
        })
    }

    fn from_span(
        inputs: &InlineListV1<SchemaInputV1<'a>, MAX_INLINE_PROTOBUF_SCHEMAS_V1>,
        span: ByteSpanV1,
    ) -> Result<Self, RemoteProtobufInitializationErrorV1> {
        let input = inputs.get(usize::from(span.schema_index))?;
        let start = usize::try_from(span.start).map_err(|_overflow| {
            RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                RemoteProtobufResourceLimitV1::Arithmetic,
            )
        })?;
        let end = span.end()?;
        let bytes = input
            .data
            .get(start..end)
            .ok_or(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema)?;
        Ok(Self {
            schema_index: span.schema_index,
            bytes,
            base: start,
            position: 0,
        })
    }

    fn next(
        &mut self,
        steps: &mut RemoteProtobufStepOwnerV1,
    ) -> Result<Option<WireFieldV1>, RemoteProtobufInitializationErrorV1> {
        if self.position == self.bytes.len() {
            return Ok(None);
        }
        let key = self.read_varint(steps)?;
        let number = u32::try_from(key >> 3)
            .map_err(|_overflow| RemoteProtobufInitializationErrorV1::InvalidRemoteSchema)?;
        if number == 0 {
            return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
        }
        let wire_type = u8::try_from(key & 0x07)
            .map_err(|_overflow| RemoteProtobufInitializationErrorV1::InvalidRemoteSchema)?;
        let value = match wire_type {
            0 => WireValueV1::Varint(self.read_varint(steps)?),
            1 => {
                self.take_exact(8, steps)?;
                WireValueV1::Fixed64
            }
            2 => {
                let len = self.read_varint(steps)?;
                let len = usize::try_from(len).map_err(|_overflow| {
                    RemoteProtobufInitializationErrorV1::InvalidRemoteSchema
                })?;
                let relative_start = self.position;
                self.take_exact(len, steps)?;
                let absolute_start = self.base.checked_add(relative_start).ok_or(
                    RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                        RemoteProtobufResourceLimitV1::Arithmetic,
                    ),
                )?;
                WireValueV1::Bytes(ByteSpanV1 {
                    schema_index: self.schema_index,
                    start: checked_u32(absolute_start)?,
                    len: checked_u32(len)?,
                })
            }
            5 => {
                self.take_exact(4, steps)?;
                WireValueV1::Fixed32
            }
            3 | 4 | 6 | 7 => {
                return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
            }
            _ => unreachable!(),
        };
        Ok(Some(WireFieldV1 { number, value }))
    }

    fn read_varint(
        &mut self,
        steps: &mut RemoteProtobufStepOwnerV1,
    ) -> Result<u64, RemoteProtobufInitializationErrorV1> {
        let mut value = 0_u64;
        for byte_index in 0..10_u32 {
            steps.consume()?;
            let byte = *self
                .bytes
                .get(self.position)
                .ok_or(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema)?;
            self.position = self.position.checked_add(1).ok_or(
                RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                    RemoteProtobufResourceLimitV1::Arithmetic,
                ),
            )?;
            if byte_index == 9 && byte > 1 {
                return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
            }
            value |= u64::from(byte & 0x7f) << (byte_index * 7);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema)
    }

    fn take_exact(
        &mut self,
        len: usize,
        steps: &mut RemoteProtobufStepOwnerV1,
    ) -> Result<(), RemoteProtobufInitializationErrorV1> {
        let end = self
            .position
            .checked_add(len)
            .ok_or(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema)?;
        if end > self.bytes.len() {
            return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
        }
        steps.consume_bytes(len)?;
        self.position = end;
        Ok(())
    }
}

fn expected_bytes(value: WireValueV1) -> Result<ByteSpanV1, RemoteProtobufInitializationErrorV1> {
    match value {
        WireValueV1::Bytes(span) => Ok(span),
        WireValueV1::Varint(_) | WireValueV1::Fixed64 | WireValueV1::Fixed32 => {
            Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema)
        }
    }
}

fn expected_varint(value: WireValueV1) -> Result<u64, RemoteProtobufInitializationErrorV1> {
    match value {
        WireValueV1::Varint(value) => Ok(value),
        WireValueV1::Fixed64 | WireValueV1::Bytes(_) | WireValueV1::Fixed32 => {
            Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema)
        }
    }
}

fn span_bytes<'a>(
    inputs: &InlineListV1<SchemaInputV1<'a>, MAX_INLINE_PROTOBUF_SCHEMAS_V1>,
    span: ByteSpanV1,
) -> Result<&'a [u8], RemoteProtobufInitializationErrorV1> {
    let input = inputs.get(usize::from(span.schema_index))?;
    let start = usize::try_from(span.start).map_err(|_overflow| {
        RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
            RemoteProtobufResourceLimitV1::Arithmetic,
        )
    })?;
    input
        .data
        .get(start..span.end()?)
        .ok_or(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema)
}

fn span_str<'a>(
    inputs: &InlineListV1<SchemaInputV1<'a>, MAX_INLINE_PROTOBUF_SCHEMAS_V1>,
    span: ByteSpanV1,
) -> Result<&'a str, RemoteProtobufInitializationErrorV1> {
    std::str::from_utf8(span_bytes(inputs, span)?)
        .map_err(|_utf8| RemoteProtobufInitializationErrorV1::InvalidRemoteSchema)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProtobufSyntaxV1 {
    Proto2,
    Proto3,
}

#[derive(Clone, Copy)]
struct FileProjectionV1 {
    schema_index: u16,
    body: ByteSpanV1,
    name: ByteSpanV1,
    package: Option<ByteSpanV1>,
    syntax: ProtobufSyntaxV1,
}

#[derive(Clone, Copy)]
struct MessageProjectionV1 {
    file_index: u32,
    parent_message: Option<u32>,
    body: ByteSpanV1,
    name: ByteSpanV1,
    map_entry: bool,
    direct_fields: u32,
    direct_oneofs: u32,
}

#[derive(Clone, Copy)]
struct EnumProjectionV1 {
    file_index: u32,
    parent_message: Option<u32>,
    body: ByteSpanV1,
    name: ByteSpanV1,
    direct_values: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SymbolKindV1 {
    Message,
    Enum,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SymbolProjectionV1 {
    kind: SymbolKindV1,
    node_index: u32,
}

#[derive(Clone, Copy)]
struct DependencyProjectionV1 {
    file_index: u32,
    name: ByteSpanV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FieldResolutionProjectionV1 {
    message_index: u32,
    field_ordinal: u32,
    symbol_index: u32,
}

struct DescriptorProjectionV1<'definitions> {
    inputs: InlineListV1<SchemaInputV1<'definitions>, MAX_INLINE_PROTOBUF_SCHEMAS_V1>,
    files: InlineListV1<FileProjectionV1, MAX_INLINE_PROTOBUF_FILES_V1>,
    messages: InlineListV1<MessageProjectionV1, MAX_INLINE_PROTOBUF_MESSAGES_V1>,
    enums: InlineListV1<EnumProjectionV1, MAX_INLINE_PROTOBUF_ENUMS_V1>,
    symbols: InlineListV1<SymbolProjectionV1, MAX_INLINE_PROTOBUF_SYMBOLS_V1>,
    sorted_symbols: InlineListV1<u32, MAX_INLINE_PROTOBUF_SYMBOLS_V1>,
    dependencies: InlineListV1<DependencyProjectionV1, MAX_INLINE_PROTOBUF_DEPENDENCIES_V1>,
    sorted_dependencies: InlineListV1<u32, MAX_INLINE_PROTOBUF_DEPENDENCIES_V1>,
    resolutions: InlineListV1<FieldResolutionProjectionV1, MAX_INLINE_PROTOBUF_RESOLUTIONS_V1>,
    named_messages: InlineListV1<u32, MAX_INLINE_PROTOBUF_SCHEMAS_V1>,
}

impl DescriptorProjectionV1<'_> {
    fn new() -> Self {
        Self {
            inputs: InlineListV1::new(),
            files: InlineListV1::new(),
            messages: InlineListV1::new(),
            enums: InlineListV1::new(),
            symbols: InlineListV1::new(),
            sorted_symbols: InlineListV1::new(),
            dependencies: InlineListV1::new(),
            sorted_dependencies: InlineListV1::new(),
            resolutions: InlineListV1::new(),
            named_messages: InlineListV1::new(),
        }
    }
}

fn unsupported_field() -> RemoteProtobufInitializationErrorV1 {
    RemoteProtobufInitializationErrorV1::UnsupportedForRemote(
        UnsupportedRemoteProtobufV1::UnknownDescriptorField,
    )
}

fn unsupported_feature() -> RemoteProtobufInitializationErrorV1 {
    RemoteProtobufInitializationErrorV1::UnsupportedForRemote(
        UnsupportedRemoteProtobufV1::DescriptorFeature,
    )
}

fn unsupported_option() -> RemoteProtobufInitializationErrorV1 {
    RemoteProtobufInitializationErrorV1::UnsupportedForRemote(
        UnsupportedRemoteProtobufV1::DescriptorOption,
    )
}

fn set_once<T: Copy>(
    slot: &mut Option<T>,
    value: T,
) -> Result<(), RemoteProtobufInitializationErrorV1> {
    if slot.replace(value).is_some() {
        return Err(RemoteProtobufInitializationErrorV1::UnsupportedForRemote(
            UnsupportedRemoteProtobufV1::DuplicateSingularField,
        ));
    }
    Ok(())
}

fn admit_string_span(
    projection: &DescriptorProjectionV1<'_>,
    span: ByteSpanV1,
    census: &mut RemoteProtobufCensusV1,
    limits: &UnfrozenRemoteProtobufLimitsV1,
    steps: &mut RemoteProtobufStepOwnerV1,
    is_default: bool,
) -> Result<(), RemoteProtobufInitializationErrorV1> {
    let bytes = span_bytes(&projection.inputs, span)?;
    if u64::try_from(bytes.len()).map_err(|_overflow| {
        RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
            RemoteProtobufResourceLimitV1::Arithmetic,
        )
    })? > limits.max_single_string_bytes
    {
        return Err(RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
            RemoteProtobufResourceLimitV1::SingleStringBytes,
        ));
    }
    std::str::from_utf8(bytes)
        .map_err(|_utf8| RemoteProtobufInitializationErrorV1::InvalidRemoteSchema)?;
    steps.consume_bytes(bytes.len())?;
    increment_limited(
        &mut census.string_bytes,
        u64::try_from(bytes.len()).map_err(|_overflow| {
            RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                RemoteProtobufResourceLimitV1::Arithmetic,
            )
        })?,
        limits.max_string_bytes,
        RemoteProtobufResourceLimitV1::StringBytes,
    )?;
    if is_default {
        increment_limited(
            &mut census.default_bytes,
            u64::try_from(bytes.len()).map_err(|_overflow| {
                RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                    RemoteProtobufResourceLimitV1::Arithmetic,
                )
            })?,
            limits.max_default_bytes,
            RemoteProtobufResourceLimitV1::DefaultBytes,
        )?;
    }
    Ok(())
}

fn valid_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first == b'_' || first.is_ascii_alphabetic())
        && bytes.all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
}

fn valid_qualified_identifier(value: &str, allow_leading_dot: bool) -> bool {
    let value = if allow_leading_dot {
        value.strip_prefix('.').unwrap_or(value)
    } else {
        value
    };
    !value.is_empty() && value.split('.').all(valid_identifier)
}

fn validate_identifier_span(
    projection: &DescriptorProjectionV1<'_>,
    span: ByteSpanV1,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<(), RemoteProtobufInitializationErrorV1> {
    steps.consume_bytes(span_bytes(&projection.inputs, span)?.len())?;
    if valid_identifier(span_str(&projection.inputs, span)?) {
        Ok(())
    } else {
        Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema)
    }
}

fn validate_qualified_span(
    projection: &DescriptorProjectionV1<'_>,
    span: ByteSpanV1,
    allow_leading_dot: bool,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<(), RemoteProtobufInitializationErrorV1> {
    steps.consume_bytes(span_bytes(&projection.inputs, span)?.len())?;
    if valid_qualified_identifier(span_str(&projection.inputs, span)?, allow_leading_dot) {
        Ok(())
    } else {
        Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema)
    }
}

fn collect_descriptor_set_v1(
    schema_index: usize,
    projection: &mut DescriptorProjectionV1<'_>,
    census: &mut RemoteProtobufCensusV1,
    limits: &UnfrozenRemoteProtobufLimitsV1,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<(), RemoteProtobufInitializationErrorV1> {
    let input = projection.inputs.get(schema_index)?;
    let mut reader = WireReaderV1::root(schema_index, input.data)?;
    let files_before = projection.files.len;
    while let Some(field) = reader.next(steps)? {
        match field.number {
            1 => collect_file_v1(
                schema_index,
                expected_bytes(field.value)?,
                projection,
                census,
                limits,
                steps,
            )?,
            _ => return Err(unsupported_field()),
        }
    }
    if projection.files.len == files_before {
        return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
    }
    Ok(())
}

fn collect_file_v1(
    schema_index: usize,
    body: ByteSpanV1,
    projection: &mut DescriptorProjectionV1<'_>,
    census: &mut RemoteProtobufCensusV1,
    limits: &UnfrozenRemoteProtobufLimitsV1,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<(), RemoteProtobufInitializationErrorV1> {
    let mut name = None;
    let mut package = None;
    let mut syntax = None;
    let mut header = WireReaderV1::from_span(&projection.inputs, body)?;
    while let Some(field) = header.next(steps)? {
        match field.number {
            1 => set_once(&mut name, expected_bytes(field.value)?)?,
            2 => set_once(&mut package, expected_bytes(field.value)?)?,
            3..=5 => {
                let _ = expected_bytes(field.value)?;
            }
            12 => set_once(&mut syntax, expected_bytes(field.value)?)?,
            6 | 7 | 8 | 9 | 10 | 11 | 14 => return Err(unsupported_feature()),
            _ => return Err(unsupported_field()),
        }
    }
    let name = name.ok_or(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema)?;
    admit_string_span(projection, name, census, limits, steps, false)?;
    if span_str(&projection.inputs, name)?.is_empty() {
        return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
    }
    if let Some(package) = package {
        admit_string_span(projection, package, census, limits, steps, false)?;
        validate_qualified_span(projection, package, false, steps)?;
    }
    let syntax = match syntax {
        None => ProtobufSyntaxV1::Proto2,
        Some(span) => {
            admit_string_span(projection, span, census, limits, steps, false)?;
            match span_str(&projection.inputs, span)? {
                "proto2" => ProtobufSyntaxV1::Proto2,
                "proto3" => ProtobufSyntaxV1::Proto3,
                _ => return Err(unsupported_feature()),
            }
        }
    };
    increment_limited(
        &mut census.files,
        1,
        limits.max_files,
        RemoteProtobufResourceLimitV1::FileCount,
    )?;
    let file_index = projection.files.push(
        FileProjectionV1 {
            schema_index: u16::try_from(schema_index).map_err(|_overflow| {
                RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                    RemoteProtobufResourceLimitV1::Arithmetic,
                )
            })?,
            body,
            name,
            package,
            syntax,
        },
        RemoteProtobufResourceLimitV1::FileCount,
    )?;

    let mut reader = WireReaderV1::from_span(&projection.inputs, body)?;
    while let Some(field) = reader.next(steps)? {
        match field.number {
            1 | 2 | 12 => {}
            3 => {
                let dependency = expected_bytes(field.value)?;
                admit_string_span(projection, dependency, census, limits, steps, false)?;
                if span_str(&projection.inputs, dependency)?.is_empty() {
                    return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
                }
                increment_limited(
                    &mut census.dependencies,
                    1,
                    limits.max_dependencies,
                    RemoteProtobufResourceLimitV1::DependencyCount,
                )?;
                projection.dependencies.push(
                    DependencyProjectionV1 {
                        file_index: checked_u32(file_index)?,
                        name: dependency,
                    },
                    RemoteProtobufResourceLimitV1::DependencyCount,
                )?;
            }
            4 => collect_message_v1(
                file_index,
                None,
                expected_bytes(field.value)?,
                1,
                projection,
                census,
                limits,
                steps,
            )?,
            5 => collect_enum_v1(
                file_index,
                None,
                expected_bytes(field.value)?,
                projection,
                census,
                limits,
                steps,
            )?,
            6 | 7 | 8 | 9 | 10 | 11 | 14 => return Err(unsupported_feature()),
            _ => return Err(unsupported_field()),
        }
    }
    Ok(())
}

fn collect_message_v1(
    file_index: usize,
    parent_message: Option<usize>,
    body: ByteSpanV1,
    depth: u64,
    projection: &mut DescriptorProjectionV1<'_>,
    census: &mut RemoteProtobufCensusV1,
    limits: &UnfrozenRemoteProtobufLimitsV1,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<(), RemoteProtobufInitializationErrorV1> {
    if depth > limits.max_descriptor_depth || depth > MAX_STACK_DESCRIPTOR_DEPTH_V1 as u64 {
        return Err(RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
            RemoteProtobufResourceLimitV1::DescriptorDepth,
        ));
    }
    let mut name = None;
    let mut map_entry = false;
    let mut direct_fields = 0_u32;
    let mut direct_oneofs = 0_u32;
    let mut header = WireReaderV1::from_span(&projection.inputs, body)?;
    while let Some(field) = header.next(steps)? {
        match field.number {
            1 => set_once(&mut name, expected_bytes(field.value)?)?,
            2 => {
                let _ = expected_bytes(field.value)?;
                direct_fields = direct_fields.checked_add(1).ok_or(
                    RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                        RemoteProtobufResourceLimitV1::Arithmetic,
                    ),
                )?;
            }
            3 | 4 => {
                let _ = expected_bytes(field.value)?;
            }
            8 => {
                let _ = expected_bytes(field.value)?;
                direct_oneofs = direct_oneofs.checked_add(1).ok_or(
                    RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                        RemoteProtobufResourceLimitV1::Arithmetic,
                    ),
                )?;
            }
            7 => {
                map_entry = parse_message_options_v1(
                    expected_bytes(field.value)?,
                    projection,
                    census,
                    limits,
                    steps,
                )?;
            }
            5 | 6 | 9 | 10 => return Err(unsupported_feature()),
            _ => return Err(unsupported_field()),
        }
    }
    let name = name.ok_or(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema)?;
    admit_string_span(projection, name, census, limits, steps, false)?;
    validate_identifier_span(projection, name, steps)?;
    increment_limited(
        &mut census.messages,
        1,
        limits.max_messages,
        RemoteProtobufResourceLimitV1::MessageCount,
    )?;
    let message_index = projection.messages.push(
        MessageProjectionV1 {
            file_index: checked_u32(file_index)?,
            parent_message: parent_message.map(checked_u32).transpose()?,
            body,
            name,
            map_entry,
            direct_fields,
            direct_oneofs,
        },
        RemoteProtobufResourceLimitV1::MessageCount,
    )?;
    let symbol_index = projection.symbols.push(
        SymbolProjectionV1 {
            kind: SymbolKindV1::Message,
            node_index: checked_u32(message_index)?,
        },
        RemoteProtobufResourceLimitV1::MessageCount,
    )?;
    let _ = symbol_index;

    let mut field_ordinal = 0_u32;
    let mut reader = WireReaderV1::from_span(&projection.inputs, body)?;
    while let Some(field) = reader.next(steps)? {
        match field.number {
            1 | 7 => {}
            2 => {
                collect_field_v1(
                    message_index,
                    field_ordinal,
                    expected_bytes(field.value)?,
                    projection,
                    census,
                    limits,
                    steps,
                )?;
                field_ordinal = field_ordinal.checked_add(1).ok_or(
                    RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                        RemoteProtobufResourceLimitV1::Arithmetic,
                    ),
                )?;
            }
            3 => collect_message_v1(
                file_index,
                Some(message_index),
                expected_bytes(field.value)?,
                checked_add(depth, 1)?,
                projection,
                census,
                limits,
                steps,
            )?,
            4 => collect_enum_v1(
                file_index,
                Some(message_index),
                expected_bytes(field.value)?,
                projection,
                census,
                limits,
                steps,
            )?,
            8 => collect_oneof_v1(
                expected_bytes(field.value)?,
                projection,
                census,
                limits,
                steps,
            )?,
            5 | 6 | 9 | 10 => return Err(unsupported_feature()),
            _ => return Err(unsupported_field()),
        }
    }
    if field_ordinal != direct_fields {
        protobuf_fatal_invariant("message field count changed between descriptor passes");
    }
    Ok(())
}

fn parse_message_options_v1(
    body: ByteSpanV1,
    projection: &DescriptorProjectionV1<'_>,
    census: &mut RemoteProtobufCensusV1,
    limits: &UnfrozenRemoteProtobufLimitsV1,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<bool, RemoteProtobufInitializationErrorV1> {
    increment_limited(
        &mut census.options,
        1,
        limits.max_options,
        RemoteProtobufResourceLimitV1::OptionCount,
    )?;
    let mut map_entry = None;
    let mut reader = WireReaderV1::from_span(&projection.inputs, body)?;
    while let Some(field) = reader.next(steps)? {
        match field.number {
            7 => {
                let value = expected_varint(field.value)?;
                if value > 1 {
                    return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
                }
                set_once(&mut map_entry, value != 0)?;
            }
            _ => return Err(unsupported_option()),
        }
    }
    Ok(map_entry.unwrap_or(false))
}

fn collect_enum_v1(
    file_index: usize,
    parent_message: Option<usize>,
    body: ByteSpanV1,
    projection: &mut DescriptorProjectionV1<'_>,
    census: &mut RemoteProtobufCensusV1,
    limits: &UnfrozenRemoteProtobufLimitsV1,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<(), RemoteProtobufInitializationErrorV1> {
    let mut name = None;
    let mut direct_values = 0_u32;
    let mut header = WireReaderV1::from_span(&projection.inputs, body)?;
    while let Some(field) = header.next(steps)? {
        match field.number {
            1 => set_once(&mut name, expected_bytes(field.value)?)?,
            2 => {
                let _ = expected_bytes(field.value)?;
                direct_values = direct_values.checked_add(1).ok_or(
                    RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                        RemoteProtobufResourceLimitV1::Arithmetic,
                    ),
                )?;
            }
            3..=5 => return Err(unsupported_feature()),
            _ => return Err(unsupported_field()),
        }
    }
    let name = name.ok_or(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema)?;
    admit_string_span(projection, name, census, limits, steps, false)?;
    validate_identifier_span(projection, name, steps)?;
    increment_limited(
        &mut census.enums,
        1,
        limits.max_enums,
        RemoteProtobufResourceLimitV1::EnumCount,
    )?;
    let enum_index = projection.enums.push(
        EnumProjectionV1 {
            file_index: checked_u32(file_index)?,
            parent_message: parent_message.map(checked_u32).transpose()?,
            body,
            name,
            direct_values,
        },
        RemoteProtobufResourceLimitV1::EnumCount,
    )?;
    projection.symbols.push(
        SymbolProjectionV1 {
            kind: SymbolKindV1::Enum,
            node_index: checked_u32(enum_index)?,
        },
        RemoteProtobufResourceLimitV1::EnumCount,
    )?;

    let mut reader = WireReaderV1::from_span(&projection.inputs, body)?;
    let mut value_ordinal = 0_u32;
    while let Some(field) = reader.next(steps)? {
        match field.number {
            1 => {}
            2 => {
                collect_enum_value_v1(
                    expected_bytes(field.value)?,
                    projection,
                    census,
                    limits,
                    steps,
                )?;
                value_ordinal = value_ordinal.checked_add(1).ok_or(
                    RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                        RemoteProtobufResourceLimitV1::Arithmetic,
                    ),
                )?;
            }
            3..=5 => return Err(unsupported_feature()),
            _ => return Err(unsupported_field()),
        }
    }
    if value_ordinal != direct_values || direct_values == 0 {
        return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
    }
    Ok(())
}

fn collect_enum_value_v1(
    body: ByteSpanV1,
    projection: &DescriptorProjectionV1<'_>,
    census: &mut RemoteProtobufCensusV1,
    limits: &UnfrozenRemoteProtobufLimitsV1,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<(), RemoteProtobufInitializationErrorV1> {
    let mut name = None;
    let mut number = None;
    let mut reader = WireReaderV1::from_span(&projection.inputs, body)?;
    while let Some(field) = reader.next(steps)? {
        match field.number {
            1 => set_once(&mut name, expected_bytes(field.value)?)?,
            2 => set_once(&mut number, expected_varint(field.value)?)?,
            3 => return Err(unsupported_option()),
            _ => return Err(unsupported_field()),
        }
    }
    let name = name.ok_or(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema)?;
    admit_string_span(projection, name, census, limits, steps, false)?;
    validate_identifier_span(projection, name, steps)?;
    let number = number.unwrap_or(0);
    let _ = i32::try_from(number.cast_signed())
        .map_err(|_overflow| RemoteProtobufInitializationErrorV1::InvalidRemoteSchema)?;
    increment_limited(
        &mut census.enum_values,
        1,
        limits.max_enum_values,
        RemoteProtobufResourceLimitV1::EnumValueCount,
    )
}

fn collect_oneof_v1(
    body: ByteSpanV1,
    projection: &DescriptorProjectionV1<'_>,
    census: &mut RemoteProtobufCensusV1,
    limits: &UnfrozenRemoteProtobufLimitsV1,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<(), RemoteProtobufInitializationErrorV1> {
    let mut name = None;
    let mut reader = WireReaderV1::from_span(&projection.inputs, body)?;
    while let Some(field) = reader.next(steps)? {
        match field.number {
            1 => set_once(&mut name, expected_bytes(field.value)?)?,
            2 => return Err(unsupported_option()),
            _ => return Err(unsupported_field()),
        }
    }
    let name = name.ok_or(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema)?;
    admit_string_span(projection, name, census, limits, steps, false)?;
    validate_identifier_span(projection, name, steps)?;
    increment_limited(
        &mut census.oneofs,
        1,
        limits.max_oneofs,
        RemoteProtobufResourceLimitV1::OneofCount,
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FieldKindV1 {
    Double,
    Float,
    Int64,
    UInt64,
    Int32,
    Fixed64,
    Fixed32,
    Bool,
    String,
    Message,
    Bytes,
    UInt32,
    Enum,
    SFixed32,
    SFixed64,
    SInt32,
    SInt64,
}

impl FieldKindV1 {
    fn from_descriptor(value: u64) -> Result<Self, RemoteProtobufInitializationErrorV1> {
        Ok(match value {
            1 => Self::Double,
            2 => Self::Float,
            3 => Self::Int64,
            4 => Self::UInt64,
            5 => Self::Int32,
            6 => Self::Fixed64,
            7 => Self::Fixed32,
            8 => Self::Bool,
            9 => Self::String,
            10 => return Err(unsupported_feature()), // proto2 group
            11 => Self::Message,
            12 => Self::Bytes,
            13 => Self::UInt32,
            14 => Self::Enum,
            15 => Self::SFixed32,
            16 => Self::SFixed64,
            17 => Self::SInt32,
            18 => Self::SInt64,
            _ => return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema),
        })
    }

    fn requires_type_name(self) -> bool {
        matches!(self, Self::Message | Self::Enum)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FieldLabelV1 {
    Optional,
    Required,
    Repeated,
}

impl FieldLabelV1 {
    fn from_descriptor(value: u64) -> Result<Self, RemoteProtobufInitializationErrorV1> {
        match value {
            1 => Ok(Self::Optional),
            2 => Ok(Self::Required),
            3 => Ok(Self::Repeated),
            _ => Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema),
        }
    }
}

#[derive(Clone, Copy)]
struct FieldHeaderV1 {
    name: ByteSpanV1,
    number: i32,
    label: FieldLabelV1,
    explicit_kind: Option<FieldKindV1>,
    type_name: Option<ByteSpanV1>,
    default: Option<ByteSpanV1>,
    oneof_index: Option<u32>,
    proto3_optional: bool,
}

fn parse_field_header_v1(
    body: ByteSpanV1,
    projection: &DescriptorProjectionV1<'_>,
    census: &mut RemoteProtobufCensusV1,
    limits: &UnfrozenRemoteProtobufLimitsV1,
    steps: &mut RemoteProtobufStepOwnerV1,
    account: bool,
) -> Result<FieldHeaderV1, RemoteProtobufInitializationErrorV1> {
    let mut name = None;
    let mut number = None;
    let mut label = None;
    let mut kind = None;
    let mut type_name = None;
    let mut default = None;
    let mut oneof_index = None;
    let mut proto3_optional = None;
    let mut reader = WireReaderV1::from_span(&projection.inputs, body)?;
    while let Some(field) = reader.next(steps)? {
        match field.number {
            1 => set_once(&mut name, expected_bytes(field.value)?)?,
            3 => set_once(&mut number, expected_varint(field.value)?)?,
            4 => set_once(&mut label, expected_varint(field.value)?)?,
            5 => set_once(&mut kind, expected_varint(field.value)?)?,
            6 => set_once(&mut type_name, expected_bytes(field.value)?)?,
            7 => set_once(&mut default, expected_bytes(field.value)?)?,
            9 => set_once(&mut oneof_index, expected_varint(field.value)?)?,
            17 => set_once(&mut proto3_optional, expected_varint(field.value)?)?,
            2 | 8 | 10 => return Err(unsupported_feature()),
            _ => return Err(unsupported_field()),
        }
    }
    let name = name.ok_or(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema)?;
    if account {
        admit_string_span(projection, name, census, limits, steps, false)?;
    } else {
        let _ = span_str(&projection.inputs, name)?;
    }
    validate_identifier_span(projection, name, steps)?;
    let number = number.ok_or(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema)?;
    let number = i32::try_from(number)
        .map_err(|_overflow| RemoteProtobufInitializationErrorV1::InvalidRemoteSchema)?;
    if number <= 0 || number > 536_870_911 || (19_000..=19_999).contains(&number) {
        return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
    }
    let label = label
        .map(FieldLabelV1::from_descriptor)
        .transpose()?
        .unwrap_or(FieldLabelV1::Optional);
    let explicit_kind = kind.map(FieldKindV1::from_descriptor).transpose()?;
    if type_name.is_none() && explicit_kind.is_some_and(FieldKindV1::requires_type_name) {
        return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
    }
    if type_name.is_some() && explicit_kind.is_some_and(|kind| !kind.requires_type_name()) {
        return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
    }
    if let Some(type_name) = type_name {
        if account {
            admit_string_span(projection, type_name, census, limits, steps, false)?;
        } else {
            let _ = span_str(&projection.inputs, type_name)?;
        }
        validate_qualified_span(projection, type_name, true, steps)?;
    }
    if let Some(default) = default {
        if account {
            admit_string_span(projection, default, census, limits, steps, true)?;
        } else {
            let _ = span_str(&projection.inputs, default)?;
        }
    }
    let oneof_index = oneof_index
        .map(|value| {
            u32::try_from(value)
                .map_err(|_overflow| RemoteProtobufInitializationErrorV1::InvalidRemoteSchema)
        })
        .transpose()?;
    let proto3_optional = match proto3_optional {
        None | Some(0) => false,
        Some(1) => true,
        Some(_) => return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema),
    };
    if account {
        increment_limited(
            &mut census.fields,
            1,
            limits.max_fields,
            RemoteProtobufResourceLimitV1::FieldCount,
        )?;
    }
    Ok(FieldHeaderV1 {
        name,
        number,
        label,
        explicit_kind,
        type_name,
        default,
        oneof_index,
        proto3_optional,
    })
}

fn collect_field_v1(
    _message_index: usize,
    _field_ordinal: u32,
    body: ByteSpanV1,
    projection: &DescriptorProjectionV1<'_>,
    census: &mut RemoteProtobufCensusV1,
    limits: &UnfrozenRemoteProtobufLimitsV1,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<(), RemoteProtobufInitializationErrorV1> {
    let _ = parse_field_header_v1(body, projection, census, limits, steps, true)?;
    Ok(())
}

#[derive(Clone, Copy)]
struct SymbolPathV1 {
    spans: [Option<ByteSpanV1>; MAX_STACK_DESCRIPTOR_DEPTH_V1 + 2],
    len: usize,
}

impl SymbolPathV1 {
    fn new() -> Self {
        Self {
            spans: [None; MAX_STACK_DESCRIPTOR_DEPTH_V1 + 2],
            len: 0,
        }
    }

    fn push(&mut self, span: ByteSpanV1) -> Result<(), RemoteProtobufInitializationErrorV1> {
        let Some(slot) = self.spans.get_mut(self.len) else {
            return Err(RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                RemoteProtobufResourceLimitV1::DescriptorDepth,
            ));
        };
        *slot = Some(span);
        self.len = self.len.checked_add(1).ok_or(
            RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                RemoteProtobufResourceLimitV1::Arithmetic,
            ),
        )?;
        Ok(())
    }

    fn get(&self, index: usize) -> Result<ByteSpanV1, RemoteProtobufInitializationErrorV1> {
        self.spans
            .get(index)
            .and_then(|span| *span)
            .ok_or_else(|| protobuf_fatal_invariant("protobuf symbol path index escaped"))
    }
}

fn symbol_path_v1(
    projection: &DescriptorProjectionV1<'_>,
    symbol_index: usize,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<SymbolPathV1, RemoteProtobufInitializationErrorV1> {
    let symbol = projection.symbols.get(symbol_index)?;
    let (file_index, parent_message, own_name) = match symbol.kind {
        SymbolKindV1::Message => {
            let message = projection
                .messages
                .get(usize::try_from(symbol.node_index).map_err(|_overflow| {
                    RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                        RemoteProtobufResourceLimitV1::Arithmetic,
                    )
                })?)?;
            (message.file_index, message.parent_message, message.name)
        }
        SymbolKindV1::Enum => {
            let value = projection
                .enums
                .get(usize::try_from(symbol.node_index).map_err(|_overflow| {
                    RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                        RemoteProtobufResourceLimitV1::Arithmetic,
                    )
                })?)?;
            (value.file_index, value.parent_message, value.name)
        }
    };
    let file = projection
        .files
        .get(usize::try_from(file_index).map_err(|_overflow| {
            RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                RemoteProtobufResourceLimitV1::Arithmetic,
            )
        })?)?;
    let mut path = SymbolPathV1::new();
    if let Some(package) = file.package {
        path.push(package)?;
    }
    let mut ancestors = [None; MAX_STACK_DESCRIPTOR_DEPTH_V1];
    let mut ancestor_len = 0_usize;
    let mut cursor = parent_message;
    while let Some(message_index) = cursor {
        steps.consume()?;
        let Some(slot) = ancestors.get_mut(ancestor_len) else {
            return Err(RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                RemoteProtobufResourceLimitV1::DescriptorDepth,
            ));
        };
        *slot = Some(message_index);
        ancestor_len = ancestor_len.checked_add(1).ok_or(
            RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                RemoteProtobufResourceLimitV1::Arithmetic,
            ),
        )?;
        cursor = projection
            .messages
            .get(usize::try_from(message_index).map_err(|_overflow| {
                RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                    RemoteProtobufResourceLimitV1::Arithmetic,
                )
            })?)?
            .parent_message;
    }
    for reverse_index in (0..ancestor_len).rev() {
        steps.consume()?;
        let message_index = ancestors[reverse_index]
            .ok_or_else(|| protobuf_fatal_invariant("protobuf ancestor scratch lost an entry"))?;
        path.push(
            projection
                .messages
                .get(usize::try_from(message_index).map_err(|_overflow| {
                    RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                        RemoteProtobufResourceLimitV1::Arithmetic,
                    )
                })?)?
                .name,
        )?;
    }
    path.push(own_name)?;
    Ok(path)
}

fn path_total_len_v1(
    projection: &DescriptorProjectionV1<'_>,
    path: &SymbolPathV1,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<usize, RemoteProtobufInitializationErrorV1> {
    let mut len = path.len.saturating_sub(1);
    for index in 0..path.len {
        steps.consume()?;
        len = len
            .checked_add(span_bytes(&projection.inputs, path.get(index)?)?.len())
            .ok_or(RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                RemoteProtobufResourceLimitV1::Arithmetic,
            ))?;
    }
    Ok(len)
}

fn path_byte_v1(
    projection: &DescriptorProjectionV1<'_>,
    path: &SymbolPathV1,
    mut position: usize,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<u8, RemoteProtobufInitializationErrorV1> {
    for index in 0..path.len {
        steps.consume()?;
        let bytes = span_bytes(&projection.inputs, path.get(index)?)?;
        if position < bytes.len() {
            return Ok(bytes[position]);
        }
        position = position.checked_sub(bytes.len()).ok_or_else(|| {
            protobuf_fatal_invariant("protobuf path position accounting underflowed")
        })?;
        if index + 1 < path.len {
            if position == 0 {
                return Ok(b'.');
            }
            position = position.checked_sub(1).ok_or_else(|| {
                protobuf_fatal_invariant("protobuf path separator accounting underflowed")
            })?;
        }
    }
    protobuf_fatal_invariant("protobuf path byte index escaped")
}

fn compare_symbol_paths_v1(
    projection: &DescriptorProjectionV1<'_>,
    left: usize,
    right: usize,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<std::cmp::Ordering, RemoteProtobufInitializationErrorV1> {
    let left_path = symbol_path_v1(projection, left, steps)?;
    let right_path = symbol_path_v1(projection, right, steps)?;
    let left_len = path_total_len_v1(projection, &left_path, steps)?;
    let right_len = path_total_len_v1(projection, &right_path, steps)?;
    for position in 0..left_len.min(right_len) {
        steps.consume()?;
        let ordering = path_byte_v1(projection, &left_path, position, steps)?.cmp(&path_byte_v1(
            projection,
            &right_path,
            position,
            steps,
        )?);
        if ordering != std::cmp::Ordering::Equal {
            return Ok(ordering);
        }
    }
    steps.consume()?;
    Ok(left_len.cmp(&right_len))
}

fn symbol_matches_text_v1(
    projection: &DescriptorProjectionV1<'_>,
    symbol_index: usize,
    text: &str,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<bool, RemoteProtobufInitializationErrorV1> {
    let text = text.strip_prefix('.').unwrap_or(text).as_bytes();
    let path = symbol_path_v1(projection, symbol_index, steps)?;
    let len = path_total_len_v1(projection, &path, steps)?;
    if len != text.len() {
        steps.consume()?;
        return Ok(false);
    }
    for (position, expected) in text.iter().copied().enumerate() {
        steps.consume()?;
        if path_byte_v1(projection, &path, position, steps)? != expected {
            return Ok(false);
        }
    }
    Ok(true)
}

fn symbol_schema_index_v1(
    projection: &DescriptorProjectionV1<'_>,
    symbol_index: usize,
) -> Result<u16, RemoteProtobufInitializationErrorV1> {
    let symbol = projection.symbols.get(symbol_index)?;
    let file_index = match symbol.kind {
        SymbolKindV1::Message => {
            projection
                .messages
                .get(usize::try_from(symbol.node_index).map_err(|_overflow| {
                    RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                        RemoteProtobufResourceLimitV1::Arithmetic,
                    )
                })?)?
                .file_index
        }
        SymbolKindV1::Enum => {
            projection
                .enums
                .get(usize::try_from(symbol.node_index).map_err(|_overflow| {
                    RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                        RemoteProtobufResourceLimitV1::Arithmetic,
                    )
                })?)?
                .file_index
        }
    };
    Ok(projection
        .files
        .get(usize::try_from(file_index).map_err(|_overflow| {
            RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                RemoteProtobufResourceLimitV1::Arithmetic,
            )
        })?)?
        .schema_index)
}

fn sort_and_validate_symbols_v1(
    projection: &mut DescriptorProjectionV1<'_>,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<(), RemoteProtobufInitializationErrorV1> {
    for symbol_index in 0..projection.symbols.len {
        steps.consume()?;
        let mut insert_at = projection.sorted_symbols.len;
        for existing_position in 0..projection.sorted_symbols.len {
            steps.consume()?;
            let existing = usize::try_from(projection.sorted_symbols.get(existing_position)?)
                .map_err(|_overflow| {
                    RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                        RemoteProtobufResourceLimitV1::Arithmetic,
                    )
                })?;
            if symbol_schema_index_v1(projection, existing)?
                != symbol_schema_index_v1(projection, symbol_index)?
            {
                continue;
            }
            match compare_symbol_paths_v1(projection, symbol_index, existing, steps)? {
                std::cmp::Ordering::Less => {
                    insert_at = existing_position;
                    break;
                }
                std::cmp::Ordering::Equal => {
                    return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
                }
                std::cmp::Ordering::Greater => {}
            }
        }
        projection.sorted_symbols.push(
            checked_u32(symbol_index)?,
            RemoteProtobufResourceLimitV1::MessageCount,
        )?;
        for position in (insert_at + 1..projection.sorted_symbols.len).rev() {
            steps.consume()?;
            projection.sorted_symbols.items[position] =
                projection.sorted_symbols.items[position - 1];
        }
        projection.sorted_symbols.items[insert_at] = Some(checked_u32(symbol_index)?);
    }
    Ok(())
}

fn compare_spans_v1(
    projection: &DescriptorProjectionV1<'_>,
    left: ByteSpanV1,
    right: ByteSpanV1,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<std::cmp::Ordering, RemoteProtobufInitializationErrorV1> {
    let left = span_bytes(&projection.inputs, left)?;
    let right = span_bytes(&projection.inputs, right)?;
    for (left, right) in left.iter().copied().zip(right.iter().copied()) {
        steps.consume()?;
        let ordering = left.cmp(&right);
        if ordering != std::cmp::Ordering::Equal {
            return Ok(ordering);
        }
    }
    steps.consume()?;
    Ok(left.len().cmp(&right.len()))
}

fn validate_files_and_dependencies_v1(
    projection: &mut DescriptorProjectionV1<'_>,
    limits: &UnfrozenRemoteProtobufLimitsV1,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<(), RemoteProtobufInitializationErrorV1> {
    for left_index in 0..projection.files.len {
        let left = projection.files.get(left_index)?;
        for right_index in left_index + 1..projection.files.len {
            steps.consume()?;
            let right = projection.files.get(right_index)?;
            if left.schema_index == right.schema_index
                && compare_spans_v1(projection, left.name, right.name, steps)?
                    == std::cmp::Ordering::Equal
            {
                return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
            }
        }
    }

    for dependency_index in 0..projection.dependencies.len {
        steps.consume()?;
        let dependency = projection.dependencies.get(dependency_index)?;
        let file = projection
            .files
            .get(usize::try_from(dependency.file_index).map_err(|_overflow| {
                RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                    RemoteProtobufResourceLimitV1::Arithmetic,
                )
            })?)?;
        let mut found = false;
        for candidate_index in 0..projection.files.len {
            steps.consume()?;
            let candidate = projection.files.get(candidate_index)?;
            if candidate.schema_index == file.schema_index
                && compare_spans_v1(projection, dependency.name, candidate.name, steps)?
                    == std::cmp::Ordering::Equal
            {
                if candidate_index
                    == usize::try_from(dependency.file_index).map_err(|_overflow| {
                        RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                            RemoteProtobufResourceLimitV1::Arithmetic,
                        )
                    })?
                {
                    return Err(RemoteProtobufInitializationErrorV1::UnsupportedForRemote(
                        UnsupportedRemoteProtobufV1::DependencyGraph,
                    ));
                }
                found = true;
                break;
            }
        }
        if !found {
            return Err(RemoteProtobufInitializationErrorV1::UnsupportedForRemote(
                UnsupportedRemoteProtobufV1::DependencyGraph,
            ));
        }
        for earlier_index in 0..dependency_index {
            steps.consume()?;
            let earlier = projection.dependencies.get(earlier_index)?;
            if earlier.file_index == dependency.file_index
                && compare_spans_v1(projection, earlier.name, dependency.name, steps)?
                    == std::cmp::Ordering::Equal
            {
                return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
            }
        }

        let mut insert_at = projection.sorted_dependencies.len;
        for position in 0..projection.sorted_dependencies.len {
            steps.consume()?;
            let existing_index = usize::try_from(projection.sorted_dependencies.get(position)?)
                .map_err(|_overflow| {
                    RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                        RemoteProtobufResourceLimitV1::Arithmetic,
                    )
                })?;
            let existing = projection.dependencies.get(existing_index)?;
            let ordering = existing
                .file_index
                .cmp(&dependency.file_index)
                .then(compare_spans_v1(
                    projection,
                    existing.name,
                    dependency.name,
                    steps,
                )?);
            if ordering == std::cmp::Ordering::Greater {
                insert_at = position;
                break;
            }
        }
        projection.sorted_dependencies.push(
            checked_u32(dependency_index)?,
            RemoteProtobufResourceLimitV1::DependencyCount,
        )?;
        for position in (insert_at + 1..projection.sorted_dependencies.len).rev() {
            steps.consume()?;
            projection.sorted_dependencies.items[position] =
                projection.sorted_dependencies.items[position - 1];
        }
        projection.sorted_dependencies.items[insert_at] = Some(checked_u32(dependency_index)?);
    }

    for file_index in 0..projection.files.len {
        let mut visiting = [usize::MAX; MAX_STACK_DESCRIPTOR_DEPTH_V1 + 1];
        validate_import_depth_v1(projection, file_index, 0, &mut visiting, limits, steps)?;
    }
    Ok(())
}

fn validate_import_depth_v1(
    projection: &DescriptorProjectionV1<'_>,
    file_index: usize,
    depth: usize,
    visiting: &mut [usize; MAX_STACK_DESCRIPTOR_DEPTH_V1 + 1],
    limits: &UnfrozenRemoteProtobufLimitsV1,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<(), RemoteProtobufInitializationErrorV1> {
    steps.consume()?;
    if depth as u64 > limits.max_import_depth || depth > MAX_STACK_DESCRIPTOR_DEPTH_V1 {
        return Err(RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
            RemoteProtobufResourceLimitV1::ImportDepth,
        ));
    }
    for ancestor in &visiting[..depth] {
        steps.consume()?;
        if *ancestor == file_index {
            return Err(RemoteProtobufInitializationErrorV1::UnsupportedForRemote(
                UnsupportedRemoteProtobufV1::DependencyGraph,
            ));
        }
    }
    visiting[depth] = file_index;
    let file = projection.files.get(file_index)?;
    for dependency_index in 0..projection.dependencies.len {
        steps.consume()?;
        let dependency = projection.dependencies.get(dependency_index)?;
        if usize::try_from(dependency.file_index).ok() != Some(file_index) {
            continue;
        }
        for target_index in 0..projection.files.len {
            steps.consume()?;
            let target = projection.files.get(target_index)?;
            if target.schema_index == file.schema_index
                && compare_spans_v1(projection, dependency.name, target.name, steps)?
                    == std::cmp::Ordering::Equal
            {
                validate_import_depth_v1(
                    projection,
                    target_index,
                    depth + 1,
                    visiting,
                    limits,
                    steps,
                )?;
                break;
            }
        }
    }
    visiting[depth] = usize::MAX;
    Ok(())
}

fn file_directly_imports_v1(
    projection: &DescriptorProjectionV1<'_>,
    from_file_index: usize,
    target_file_index: usize,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<bool, RemoteProtobufInitializationErrorV1> {
    if from_file_index == target_file_index {
        return Ok(true);
    }
    let target = projection.files.get(target_file_index)?;
    for dependency_index in 0..projection.dependencies.len {
        steps.consume()?;
        let dependency = projection.dependencies.get(dependency_index)?;
        if usize::try_from(dependency.file_index).ok() == Some(from_file_index)
            && compare_spans_v1(projection, dependency.name, target.name, steps)?
                == std::cmp::Ordering::Equal
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn validate_named_messages_v1(
    projection: &mut DescriptorProjectionV1<'_>,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<(), RemoteProtobufInitializationErrorV1> {
    for schema_index in 0..projection.inputs.len {
        let input = projection.inputs.get(schema_index)?;
        steps.consume_bytes(input.name.len())?;
        if !valid_qualified_identifier(input.name, false) {
            return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
        }
        let mut found = None;
        for symbol_index in 0..projection.symbols.len {
            steps.consume()?;
            let symbol = projection.symbols.get(symbol_index)?;
            if symbol.kind == SymbolKindV1::Message
                && symbol_schema_index_v1(projection, symbol_index)?
                    == u16::try_from(schema_index).map_err(|_overflow| {
                        RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                            RemoteProtobufResourceLimitV1::Arithmetic,
                        )
                    })?
                && symbol_matches_text_v1(projection, symbol_index, input.name, steps)?
                && found.replace(symbol_index).is_some()
            {
                protobuf_fatal_invariant("duplicate symbols survived validation");
            }
        }
        let found = found.ok_or(RemoteProtobufInitializationErrorV1::UnsupportedForRemote(
            UnsupportedRemoteProtobufV1::DependencyGraph,
        ))?;
        projection.named_messages.push(
            checked_u32(found)?,
            RemoteProtobufResourceLimitV1::SchemaCount,
        )?;
    }
    Ok(())
}

fn symbol_file_index_v1(
    projection: &DescriptorProjectionV1<'_>,
    symbol_index: usize,
) -> Result<usize, RemoteProtobufInitializationErrorV1> {
    let symbol = projection.symbols.get(symbol_index)?;
    let index = match symbol.kind {
        SymbolKindV1::Message => {
            projection
                .messages
                .get(usize::try_from(symbol.node_index).map_err(|_overflow| {
                    RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                        RemoteProtobufResourceLimitV1::Arithmetic,
                    )
                })?)?
                .file_index
        }
        SymbolKindV1::Enum => {
            projection
                .enums
                .get(usize::try_from(symbol.node_index).map_err(|_overflow| {
                    RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                        RemoteProtobufResourceLimitV1::Arithmetic,
                    )
                })?)?
                .file_index
        }
    };
    usize::try_from(index).map_err(|_overflow| {
        RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
            RemoteProtobufResourceLimitV1::Arithmetic,
        )
    })
}

fn resolve_type_v1(
    projection: &DescriptorProjectionV1<'_>,
    message_index: usize,
    type_name: ByteSpanV1,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<usize, RemoteProtobufInitializationErrorV1> {
    let text = span_str(&projection.inputs, type_name)?;
    if !text.starts_with('.') {
        return Err(RemoteProtobufInitializationErrorV1::UnsupportedForRemote(
            UnsupportedRemoteProtobufV1::RelativeTypeName,
        ));
    }
    let schema_index = projection
        .files
        .get(
            usize::try_from(projection.messages.get(message_index)?.file_index).map_err(
                |_overflow| {
                    RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                        RemoteProtobufResourceLimitV1::Arithmetic,
                    )
                },
            )?,
        )?
        .schema_index;
    let mut found = None;
    for symbol_index in 0..projection.symbols.len {
        steps.consume()?;
        if symbol_schema_index_v1(projection, symbol_index)? == schema_index
            && symbol_matches_text_v1(projection, symbol_index, text, steps)?
            && found.replace(symbol_index).is_some()
        {
            protobuf_fatal_invariant("duplicate protobuf symbols survived preflight");
        }
    }
    let found = found.ok_or(RemoteProtobufInitializationErrorV1::UnsupportedForRemote(
        UnsupportedRemoteProtobufV1::DependencyGraph,
    ))?;
    let from_file = usize::try_from(projection.messages.get(message_index)?.file_index).map_err(
        |_overflow| {
            RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                RemoteProtobufResourceLimitV1::Arithmetic,
            )
        },
    )?;
    let target_file = symbol_file_index_v1(projection, found)?;
    if !file_directly_imports_v1(projection, from_file, target_file, steps)? {
        return Err(RemoteProtobufInitializationErrorV1::UnsupportedForRemote(
            UnsupportedRemoteProtobufV1::DependencyGraph,
        ));
    }
    Ok(found)
}

fn direct_field_span_v1(
    projection: &DescriptorProjectionV1<'_>,
    message_index: usize,
    ordinal: usize,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<ByteSpanV1, RemoteProtobufInitializationErrorV1> {
    let message = projection.messages.get(message_index)?;
    let mut reader = WireReaderV1::from_span(&projection.inputs, message.body)?;
    let mut current = 0_usize;
    while let Some(field) = reader.next(steps)? {
        if field.number == 2 {
            if current == ordinal {
                return expected_bytes(field.value);
            }
            current = current.checked_add(1).ok_or(
                RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                    RemoteProtobufResourceLimitV1::Arithmetic,
                ),
            )?;
        }
    }
    protobuf_fatal_invariant("protobuf direct field ordinal escaped census")
}

fn field_kind_from_symbol_v1(
    projection: &DescriptorProjectionV1<'_>,
    symbol_index: usize,
) -> Result<FieldKindV1, RemoteProtobufInitializationErrorV1> {
    Ok(match projection.symbols.get(symbol_index)?.kind {
        SymbolKindV1::Message => FieldKindV1::Message,
        SymbolKindV1::Enum => FieldKindV1::Enum,
    })
}

fn effective_field_kind_v1(
    projection: &DescriptorProjectionV1<'_>,
    field: FieldHeaderV1,
    resolved_symbol: Option<usize>,
) -> Result<FieldKindV1, RemoteProtobufInitializationErrorV1> {
    match (field.type_name, resolved_symbol) {
        (None, None) => Ok(field.explicit_kind.unwrap_or(FieldKindV1::Double)),
        (Some(_), Some(symbol_index)) => {
            let resolved = field_kind_from_symbol_v1(projection, symbol_index)?;
            if field
                .explicit_kind
                .is_some_and(|explicit| explicit != resolved)
            {
                return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
            }
            Ok(resolved)
        }
        (None, Some(_)) | (Some(_), None) => {
            protobuf_fatal_invariant("protobuf field resolution disagrees with its type name")
        }
    }
}

fn validate_default_value_v1(
    projection: &DescriptorProjectionV1<'_>,
    field: FieldHeaderV1,
    kind: FieldKindV1,
    resolved_symbol: Option<usize>,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<(), RemoteProtobufInitializationErrorV1> {
    let Some(default) = field.default else {
        return Ok(());
    };
    if field.label == FieldLabelV1::Repeated || field.oneof_index.is_some() {
        return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
    }
    let value = span_str(&projection.inputs, default)?;
    steps.consume_bytes(value.len())?;
    let valid = match kind {
        FieldKindV1::Bool => value == "true" || value == "false",
        FieldKindV1::Int32 | FieldKindV1::SInt32 | FieldKindV1::SFixed32 => {
            value.parse::<i32>().is_ok()
        }
        FieldKindV1::UInt32 | FieldKindV1::Fixed32 => value.parse::<u32>().is_ok(),
        FieldKindV1::Int64 | FieldKindV1::SInt64 | FieldKindV1::SFixed64 => {
            value.parse::<i64>().is_ok()
        }
        FieldKindV1::UInt64 | FieldKindV1::Fixed64 => value.parse::<u64>().is_ok(),
        FieldKindV1::Float => {
            matches!(value, "inf" | "-inf" | "nan") || value.parse::<f32>().is_ok()
        }
        FieldKindV1::Double => {
            matches!(value, "inf" | "-inf" | "nan") || value.parse::<f64>().is_ok()
        }
        FieldKindV1::Enum => {
            let symbol_index = resolved_symbol
                .ok_or_else(|| protobuf_fatal_invariant("enum default has no resolved symbol"))?;
            let symbol = projection.symbols.get(symbol_index)?;
            if symbol.kind != SymbolKindV1::Enum {
                protobuf_fatal_invariant("enum default resolved to another symbol kind");
            }
            let enum_index = usize::try_from(symbol.node_index).map_err(|_overflow| {
                RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                    RemoteProtobufResourceLimitV1::Arithmetic,
                )
            })?;
            let enumeration = projection.enums.get(enum_index)?;
            let mut found = false;
            for ordinal in 0..usize::try_from(enumeration.direct_values).map_err(|_overflow| {
                RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                    RemoteProtobufResourceLimitV1::Arithmetic,
                )
            })? {
                let candidate = enum_value_header_v1(projection, enum_index, ordinal, steps)?;
                if compare_spans_v1(projection, candidate.name, default, steps)?
                    == std::cmp::Ordering::Equal
                {
                    found = true;
                    break;
                }
            }
            found
        }
        FieldKindV1::Message => {
            return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
        }
        // Descriptor string/bytes defaults use protobuf's C-escape grammar. V1 deliberately
        // rejects it until an allocation-free unescaper and exact decoded-size bound exist.
        FieldKindV1::String | FieldKindV1::Bytes => return Err(unsupported_feature()),
    };
    if valid {
        Ok(())
    } else {
        Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema)
    }
}

fn validate_fields_and_resolve_types_v1(
    projection: &mut DescriptorProjectionV1<'_>,
    limits: &UnfrozenRemoteProtobufLimitsV1,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<(), RemoteProtobufInitializationErrorV1> {
    let mut no_accounting = RemoteProtobufCensusV1::default();
    for message_index in 0..projection.messages.len {
        steps.consume()?;
        let message = projection.messages.get(message_index)?;
        let file = projection
            .files
            .get(usize::try_from(message.file_index).map_err(|_overflow| {
                RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                    RemoteProtobufResourceLimitV1::Arithmetic,
                )
            })?)?;
        for field_ordinal in 0..usize::try_from(message.direct_fields).map_err(|_overflow| {
            RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                RemoteProtobufResourceLimitV1::Arithmetic,
            )
        })? {
            let body = direct_field_span_v1(projection, message_index, field_ordinal, steps)?;
            let field =
                parse_field_header_v1(body, projection, &mut no_accounting, limits, steps, false)?;
            if file.syntax == ProtobufSyntaxV1::Proto3
                && (field.label == FieldLabelV1::Required || field.default.is_some())
            {
                return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
            }
            if let Some(oneof_index) = field.oneof_index
                && oneof_index >= message.direct_oneofs
            {
                return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
            }
            if field.oneof_index.is_some() && field.label != FieldLabelV1::Optional {
                return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
            }
            if field.proto3_optional
                && (file.syntax != ProtobufSyntaxV1::Proto3
                    || field.label != FieldLabelV1::Optional
                    || field.oneof_index.is_none())
            {
                return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
            }
            if !field.proto3_optional
                && field.oneof_index.is_some()
                && field.label == FieldLabelV1::Repeated
            {
                return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
            }
            for earlier_ordinal in 0..field_ordinal {
                let earlier_body =
                    direct_field_span_v1(projection, message_index, earlier_ordinal, steps)?;
                let earlier = parse_field_header_v1(
                    earlier_body,
                    projection,
                    &mut no_accounting,
                    limits,
                    steps,
                    false,
                )?;
                if earlier.number == field.number
                    || compare_spans_v1(projection, earlier.name, field.name, steps)?
                        == std::cmp::Ordering::Equal
                {
                    return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
                }
            }
            let resolved_symbol = if let Some(type_name) = field.type_name {
                let symbol = resolve_type_v1(projection, message_index, type_name, steps)?;
                projection.resolutions.push(
                    FieldResolutionProjectionV1 {
                        message_index: checked_u32(message_index)?,
                        field_ordinal: checked_u32(field_ordinal)?,
                        symbol_index: checked_u32(symbol)?,
                    },
                    RemoteProtobufResourceLimitV1::FieldCount,
                )?;
                Some(symbol)
            } else {
                None
            };
            let kind = effective_field_kind_v1(projection, field, resolved_symbol)?;
            validate_default_value_v1(projection, field, kind, resolved_symbol, steps)?;
        }
    }
    Ok(())
}

fn resolution_for_field_v1(
    projection: &DescriptorProjectionV1<'_>,
    message_index: usize,
    field_ordinal: usize,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<Option<usize>, RemoteProtobufInitializationErrorV1> {
    for index in 0..projection.resolutions.len {
        steps.consume()?;
        let resolution = projection.resolutions.get(index)?;
        if usize::try_from(resolution.message_index).ok() == Some(message_index)
            && usize::try_from(resolution.field_ordinal).ok() == Some(field_ordinal)
        {
            return usize::try_from(resolution.symbol_index)
                .map(Some)
                .map_err(|_overflow| {
                    RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                        RemoteProtobufResourceLimitV1::Arithmetic,
                    )
                });
        }
    }
    Ok(None)
}

fn validate_builder_graph_v1(
    projection: &DescriptorProjectionV1<'_>,
    limits: &UnfrozenRemoteProtobufLimitsV1,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<(), RemoteProtobufInitializationErrorV1> {
    for root in 0..projection.messages.len {
        let mut stack = [usize::MAX; MAX_STACK_DESCRIPTOR_DEPTH_V1 + 1];
        validate_builder_graph_from_v1(projection, root, 0, &mut stack, limits, steps)?;
    }
    Ok(())
}

fn validate_builder_graph_from_v1(
    projection: &DescriptorProjectionV1<'_>,
    message_index: usize,
    depth: usize,
    stack: &mut [usize; MAX_STACK_DESCRIPTOR_DEPTH_V1 + 1],
    limits: &UnfrozenRemoteProtobufLimitsV1,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<(), RemoteProtobufInitializationErrorV1> {
    steps.consume()?;
    if depth as u64 > limits.max_descriptor_depth || depth > MAX_STACK_DESCRIPTOR_DEPTH_V1 {
        return Err(RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
            RemoteProtobufResourceLimitV1::DescriptorDepth,
        ));
    }
    for ancestor in &stack[..depth] {
        steps.consume()?;
        if *ancestor == message_index {
            return Err(RemoteProtobufInitializationErrorV1::UnsupportedForRemote(
                UnsupportedRemoteProtobufV1::RecursiveBuilderGraph,
            ));
        }
    }
    stack[depth] = message_index;
    let message = projection.messages.get(message_index)?;
    for field_ordinal in 0..usize::try_from(message.direct_fields).map_err(|_overflow| {
        RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
            RemoteProtobufResourceLimitV1::Arithmetic,
        )
    })? {
        let Some(symbol_index) =
            resolution_for_field_v1(projection, message_index, field_ordinal, steps)?
        else {
            continue;
        };
        let symbol = projection.symbols.get(symbol_index)?;
        if symbol.kind == SymbolKindV1::Message {
            validate_builder_graph_from_v1(
                projection,
                usize::try_from(symbol.node_index).map_err(|_overflow| {
                    RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                        RemoteProtobufResourceLimitV1::Arithmetic,
                    )
                })?,
                depth + 1,
                stack,
                limits,
                steps,
            )?;
        }
    }
    stack[depth] = usize::MAX;
    Ok(())
}

fn oneof_name_v1(
    projection: &DescriptorProjectionV1<'_>,
    message_index: usize,
    ordinal: usize,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<ByteSpanV1, RemoteProtobufInitializationErrorV1> {
    let message = projection.messages.get(message_index)?;
    let mut reader = WireReaderV1::from_span(&projection.inputs, message.body)?;
    let mut current = 0_usize;
    while let Some(field) = reader.next(steps)? {
        if field.number != 8 {
            continue;
        }
        if current == ordinal {
            let body = expected_bytes(field.value)?;
            let mut name = None;
            let mut oneof = WireReaderV1::from_span(&projection.inputs, body)?;
            while let Some(field) = oneof.next(steps)? {
                match field.number {
                    1 => set_once(&mut name, expected_bytes(field.value)?)?,
                    2 => return Err(unsupported_option()),
                    _ => return Err(unsupported_field()),
                }
            }
            return name.ok_or(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
        }
        current += 1;
    }
    protobuf_fatal_invariant("protobuf oneof ordinal escaped census")
}

fn validate_oneofs_v1(
    projection: &DescriptorProjectionV1<'_>,
    limits: &UnfrozenRemoteProtobufLimitsV1,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<(), RemoteProtobufInitializationErrorV1> {
    let mut no_accounting = RemoteProtobufCensusV1::default();
    for message_index in 0..projection.messages.len {
        steps.consume()?;
        let message = projection.messages.get(message_index)?;
        let mut saw_synthetic = false;
        for ordinal in 0..usize::try_from(message.direct_oneofs).map_err(|_overflow| {
            RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                RemoteProtobufResourceLimitV1::Arithmetic,
            )
        })? {
            let name = oneof_name_v1(projection, message_index, ordinal, steps)?;
            for earlier in 0..ordinal {
                if compare_spans_v1(
                    projection,
                    name,
                    oneof_name_v1(projection, message_index, earlier, steps)?,
                    steps,
                )? == std::cmp::Ordering::Equal
                {
                    return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
                }
            }
            let mut field_count = 0_u64;
            let mut has_proto3_optional = false;
            for field_ordinal in 0..usize::try_from(message.direct_fields).map_err(|_overflow| {
                RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                    RemoteProtobufResourceLimitV1::Arithmetic,
                )
            })? {
                let field = parse_field_header_v1(
                    direct_field_span_v1(projection, message_index, field_ordinal, steps)?,
                    projection,
                    &mut no_accounting,
                    limits,
                    steps,
                    false,
                )?;
                if field.oneof_index == Some(checked_u32(ordinal)?) {
                    field_count = checked_add(field_count, 1)?;
                    has_proto3_optional |= field.proto3_optional;
                }
            }
            if field_count == 0 || (has_proto3_optional && field_count != 1) {
                return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
            }
            if has_proto3_optional {
                saw_synthetic = true;
            } else if saw_synthetic {
                return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct EnumValueHeaderV1 {
    name: ByteSpanV1,
    number: i32,
}

fn enum_value_header_v1(
    projection: &DescriptorProjectionV1<'_>,
    enum_index: usize,
    ordinal: usize,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<EnumValueHeaderV1, RemoteProtobufInitializationErrorV1> {
    let value = projection.enums.get(enum_index)?;
    let mut reader = WireReaderV1::from_span(&projection.inputs, value.body)?;
    let mut current = 0_usize;
    while let Some(field) = reader.next(steps)? {
        if field.number != 2 {
            continue;
        }
        if current == ordinal {
            let body = expected_bytes(field.value)?;
            let mut name = None;
            let mut number = None;
            let mut value_reader = WireReaderV1::from_span(&projection.inputs, body)?;
            while let Some(field) = value_reader.next(steps)? {
                match field.number {
                    1 => set_once(&mut name, expected_bytes(field.value)?)?,
                    2 => set_once(&mut number, expected_varint(field.value)?)?,
                    3 => return Err(unsupported_option()),
                    _ => return Err(unsupported_field()),
                }
            }
            let number = number.unwrap_or(0);
            return Ok(EnumValueHeaderV1 {
                name: name.ok_or(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema)?,
                number: i32::try_from(number.cast_signed()).map_err(|_overflow| {
                    RemoteProtobufInitializationErrorV1::InvalidRemoteSchema
                })?,
            });
        }
        current += 1;
    }
    protobuf_fatal_invariant("protobuf enum value ordinal escaped census")
}

fn validate_enums_v1(
    projection: &DescriptorProjectionV1<'_>,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<(), RemoteProtobufInitializationErrorV1> {
    for enum_index in 0..projection.enums.len {
        steps.consume()?;
        let value = projection.enums.get(enum_index)?;
        let file = projection
            .files
            .get(usize::try_from(value.file_index).map_err(|_overflow| {
                RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                    RemoteProtobufResourceLimitV1::Arithmetic,
                )
            })?)?;
        let count = usize::try_from(value.direct_values).map_err(|_overflow| {
            RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                RemoteProtobufResourceLimitV1::Arithmetic,
            )
        })?;
        if file.syntax == ProtobufSyntaxV1::Proto3
            && enum_value_header_v1(projection, enum_index, 0, steps)?.number != 0
        {
            return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
        }
        for ordinal in 0..count {
            let current = enum_value_header_v1(projection, enum_index, ordinal, steps)?;
            for earlier in 0..ordinal {
                let earlier = enum_value_header_v1(projection, enum_index, earlier, steps)?;
                if current.number == earlier.number
                    || compare_spans_v1(projection, current.name, earlier.name, steps)?
                        == std::cmp::Ordering::Equal
                {
                    return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
                }
            }
        }
    }
    Ok(())
}

fn validate_map_entries_v1(
    projection: &DescriptorProjectionV1<'_>,
    limits: &UnfrozenRemoteProtobufLimitsV1,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<(), RemoteProtobufInitializationErrorV1> {
    let mut no_accounting = RemoteProtobufCensusV1::default();
    for message_index in 0..projection.messages.len {
        steps.consume()?;
        let message = projection.messages.get(message_index)?;
        if !message.map_entry {
            continue;
        }
        let mut has_nested_message = false;
        for candidate_index in 0..projection.messages.len {
            steps.consume()?;
            let candidate = projection.messages.get(candidate_index)?;
            if usize::try_from(candidate.parent_message.unwrap_or(u32::MAX)).ok()
                == Some(message_index)
            {
                has_nested_message = true;
                break;
            }
        }
        let mut has_nested_enum = false;
        for candidate_index in 0..projection.enums.len {
            steps.consume()?;
            let candidate = projection.enums.get(candidate_index)?;
            if usize::try_from(candidate.parent_message.unwrap_or(u32::MAX)).ok()
                == Some(message_index)
            {
                has_nested_enum = true;
                break;
            }
        }
        if message.parent_message.is_none()
            || message.direct_fields != 2
            || message.direct_oneofs != 0
            || has_nested_message
            || has_nested_enum
        {
            return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
        }
        let key = parse_field_header_v1(
            direct_field_span_v1(projection, message_index, 0, steps)?,
            projection,
            &mut no_accounting,
            limits,
            steps,
            false,
        )?;
        let value = parse_field_header_v1(
            direct_field_span_v1(projection, message_index, 1, steps)?,
            projection,
            &mut no_accounting,
            limits,
            steps,
            false,
        )?;
        let key_resolution = resolution_for_field_v1(projection, message_index, 0, steps)?;
        let value_resolution = resolution_for_field_v1(projection, message_index, 1, steps)?;
        let key_kind = effective_field_kind_v1(projection, key, key_resolution)?;
        let _value_kind = effective_field_kind_v1(projection, value, value_resolution)?;
        if key.number != 1
            || value.number != 2
            || span_str(&projection.inputs, key.name)? != "key"
            || span_str(&projection.inputs, value.name)? != "value"
            || key.label != FieldLabelV1::Optional
            || value.label != FieldLabelV1::Optional
            || key.default.is_some()
            || value.default.is_some()
            || key.oneof_index.is_some()
            || value.oneof_index.is_some()
            || key.proto3_optional
            || value.proto3_optional
            || matches!(
                key_kind,
                FieldKindV1::Double
                    | FieldKindV1::Float
                    | FieldKindV1::Bytes
                    | FieldKindV1::Message
                    | FieldKindV1::Enum
            )
        {
            return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
        }
        let mut references = 0_u64;
        for resolution_index in 0..projection.resolutions.len {
            steps.consume()?;
            let resolution = projection.resolutions.get(resolution_index)?;
            let symbol =
                projection
                    .symbols
                    .get(
                        usize::try_from(resolution.symbol_index).map_err(|_overflow| {
                            RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                                RemoteProtobufResourceLimitV1::Arithmetic,
                            )
                        })?,
                    )?;
            if symbol.kind == SymbolKindV1::Message
                && usize::try_from(symbol.node_index).ok() == Some(message_index)
            {
                let owner_index =
                    usize::try_from(resolution.message_index).map_err(|_overflow| {
                        RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                            RemoteProtobufResourceLimitV1::Arithmetic,
                        )
                    })?;
                if message
                    .parent_message
                    .and_then(|parent| usize::try_from(parent).ok())
                    != Some(owner_index)
                {
                    return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
                }
                let field_ordinal =
                    usize::try_from(resolution.field_ordinal).map_err(|_overflow| {
                        RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                            RemoteProtobufResourceLimitV1::Arithmetic,
                        )
                    })?;
                let field = parse_field_header_v1(
                    direct_field_span_v1(projection, owner_index, field_ordinal, steps)?,
                    projection,
                    &mut no_accounting,
                    limits,
                    steps,
                    false,
                )?;
                if field.label != FieldLabelV1::Repeated {
                    return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
                }
                references = checked_add(references, 1)?;
            }
        }
        if references != 1 {
            return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum NamespaceScopeV1 {
    Message(usize),
    TopLevel(usize),
}

#[derive(Clone, Copy)]
struct NamespaceMemberV1 {
    name: ByteSpanV1,
    field_syntax: Option<ProtobufSyntaxV1>,
}

fn same_top_level_scope_v1(
    projection: &DescriptorProjectionV1<'_>,
    left_file_index: usize,
    right_file_index: usize,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<bool, RemoteProtobufInitializationErrorV1> {
    let left = projection.files.get(left_file_index)?;
    let right = projection.files.get(right_file_index)?;
    if left.schema_index != right.schema_index {
        steps.consume()?;
        return Ok(false);
    }
    match (left.package, right.package) {
        (None, None) => {
            steps.consume()?;
            Ok(true)
        }
        (Some(left), Some(right)) => {
            Ok(compare_spans_v1(projection, left, right, steps)? == std::cmp::Ordering::Equal)
        }
        (None, Some(_)) | (Some(_), None) => {
            steps.consume()?;
            Ok(false)
        }
    }
}

fn namespace_scope_contains_v1(
    projection: &DescriptorProjectionV1<'_>,
    scope: NamespaceScopeV1,
    file_index: usize,
    parent_message: Option<u32>,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<bool, RemoteProtobufInitializationErrorV1> {
    match scope {
        NamespaceScopeV1::Message(owner) => {
            steps.consume()?;
            Ok(parent_message.and_then(|parent| usize::try_from(parent).ok()) == Some(owner))
        }
        NamespaceScopeV1::TopLevel(scope_file) => {
            if parent_message.is_some() {
                steps.consume()?;
                Ok(false)
            } else {
                same_top_level_scope_v1(projection, scope_file, file_index, steps)
            }
        }
    }
}

#[derive(Clone, Copy)]
struct NamespaceMemberIterV1 {
    scope: NamespaceScopeV1,
    field_ordinal: usize,
    oneof_ordinal: usize,
    message_index: usize,
    enum_index: usize,
    active_enum: Option<usize>,
    enum_value_ordinal: usize,
}

impl NamespaceMemberIterV1 {
    fn new(scope: NamespaceScopeV1) -> Self {
        Self {
            scope,
            field_ordinal: 0,
            oneof_ordinal: 0,
            message_index: 0,
            enum_index: 0,
            active_enum: None,
            enum_value_ordinal: 0,
        }
    }

    fn next(
        &mut self,
        projection: &DescriptorProjectionV1<'_>,
        limits: &UnfrozenRemoteProtobufLimitsV1,
        steps: &mut RemoteProtobufStepOwnerV1,
    ) -> Result<Option<NamespaceMemberV1>, RemoteProtobufInitializationErrorV1> {
        if let NamespaceScopeV1::Message(message_index) = self.scope {
            let message = projection.messages.get(message_index)?;
            let field_count = usize::try_from(message.direct_fields).map_err(|_overflow| {
                RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                    RemoteProtobufResourceLimitV1::Arithmetic,
                )
            })?;
            if self.field_ordinal < field_count {
                let ordinal = self.field_ordinal;
                self.field_ordinal = self.field_ordinal.checked_add(1).ok_or(
                    RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                        RemoteProtobufResourceLimitV1::Arithmetic,
                    ),
                )?;
                let mut no_accounting = RemoteProtobufCensusV1::default();
                let field = parse_field_header_v1(
                    direct_field_span_v1(projection, message_index, ordinal, steps)?,
                    projection,
                    &mut no_accounting,
                    limits,
                    steps,
                    false,
                )?;
                let file = projection
                    .files
                    .get(usize::try_from(message.file_index).map_err(|_overflow| {
                        RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                            RemoteProtobufResourceLimitV1::Arithmetic,
                        )
                    })?)?;
                return Ok(Some(NamespaceMemberV1 {
                    name: field.name,
                    field_syntax: Some(file.syntax),
                }));
            }
            let oneof_count = usize::try_from(message.direct_oneofs).map_err(|_overflow| {
                RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                    RemoteProtobufResourceLimitV1::Arithmetic,
                )
            })?;
            if self.oneof_ordinal < oneof_count {
                let ordinal = self.oneof_ordinal;
                self.oneof_ordinal = self.oneof_ordinal.checked_add(1).ok_or(
                    RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                        RemoteProtobufResourceLimitV1::Arithmetic,
                    ),
                )?;
                return Ok(Some(NamespaceMemberV1 {
                    name: oneof_name_v1(projection, message_index, ordinal, steps)?,
                    field_syntax: None,
                }));
            }
        }

        while self.message_index < projection.messages.len {
            let candidate_index = self.message_index;
            self.message_index = self.message_index.checked_add(1).ok_or(
                RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                    RemoteProtobufResourceLimitV1::Arithmetic,
                ),
            )?;
            steps.consume()?;
            let candidate = projection.messages.get(candidate_index)?;
            let file_index = usize::try_from(candidate.file_index).map_err(|_overflow| {
                RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                    RemoteProtobufResourceLimitV1::Arithmetic,
                )
            })?;
            if namespace_scope_contains_v1(
                projection,
                self.scope,
                file_index,
                candidate.parent_message,
                steps,
            )? {
                return Ok(Some(NamespaceMemberV1 {
                    name: candidate.name,
                    field_syntax: None,
                }));
            }
        }

        loop {
            if let Some(enum_index) = self.active_enum {
                let enumeration = projection.enums.get(enum_index)?;
                let value_count =
                    usize::try_from(enumeration.direct_values).map_err(|_overflow| {
                        RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                            RemoteProtobufResourceLimitV1::Arithmetic,
                        )
                    })?;
                if self.enum_value_ordinal < value_count {
                    let ordinal = self.enum_value_ordinal;
                    self.enum_value_ordinal = self.enum_value_ordinal.checked_add(1).ok_or(
                        RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                            RemoteProtobufResourceLimitV1::Arithmetic,
                        ),
                    )?;
                    return Ok(Some(NamespaceMemberV1 {
                        name: enum_value_header_v1(projection, enum_index, ordinal, steps)?.name,
                        field_syntax: None,
                    }));
                }
                self.active_enum = None;
                self.enum_value_ordinal = 0;
            }
            if self.enum_index >= projection.enums.len {
                return Ok(None);
            }
            let candidate_index = self.enum_index;
            self.enum_index = self.enum_index.checked_add(1).ok_or(
                RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                    RemoteProtobufResourceLimitV1::Arithmetic,
                ),
            )?;
            steps.consume()?;
            let candidate = projection.enums.get(candidate_index)?;
            let file_index = usize::try_from(candidate.file_index).map_err(|_overflow| {
                RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                    RemoteProtobufResourceLimitV1::Arithmetic,
                )
            })?;
            if namespace_scope_contains_v1(
                projection,
                self.scope,
                file_index,
                candidate.parent_message,
                steps,
            )? {
                self.active_enum = Some(candidate_index);
                return Ok(Some(NamespaceMemberV1 {
                    name: candidate.name,
                    field_syntax: None,
                }));
            }
        }
    }
}

fn normalized_field_names_equal_v1(
    projection: &DescriptorProjectionV1<'_>,
    left: ByteSpanV1,
    right: ByteSpanV1,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<bool, RemoteProtobufInitializationErrorV1> {
    // Protobuf's lower-without-underscores key is stricter than equality of the implicit JSON
    // lowerCamelCase name: every JSON-name collision also has the same lowercase, underscore-free
    // bytes. Rejecting this key therefore covers both namespaces without materializing either
    // derived string.
    fn next_normalized(
        bytes: &[u8],
        position: &mut usize,
        steps: &mut RemoteProtobufStepOwnerV1,
    ) -> Result<Option<u8>, RemoteProtobufInitializationErrorV1> {
        while let Some(byte) = bytes.get(*position).copied() {
            steps.consume()?;
            *position = position.checked_add(1).ok_or(
                RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                    RemoteProtobufResourceLimitV1::Arithmetic,
                ),
            )?;
            if byte != b'_' {
                return Ok(Some(byte.to_ascii_lowercase()));
            }
        }
        Ok(None)
    }

    let left = span_bytes(&projection.inputs, left)?;
    let right = span_bytes(&projection.inputs, right)?;
    let mut left_position = 0;
    let mut right_position = 0;
    loop {
        match (
            next_normalized(left, &mut left_position, steps)?,
            next_normalized(right, &mut right_position, steps)?,
        ) {
            (None, None) => return Ok(true),
            (Some(left), Some(right)) if left == right => {}
            _ => return Ok(false),
        }
    }
}

fn implicit_json_field_names_equal_v1(
    projection: &DescriptorProjectionV1<'_>,
    left: ByteSpanV1,
    right: ByteSpanV1,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<bool, RemoteProtobufInitializationErrorV1> {
    fn next_json_byte(
        bytes: &[u8],
        position: &mut usize,
        capitalize_next: &mut bool,
        steps: &mut RemoteProtobufStepOwnerV1,
    ) -> Result<Option<u8>, RemoteProtobufInitializationErrorV1> {
        while let Some(byte) = bytes.get(*position).copied() {
            steps.consume()?;
            *position = position.checked_add(1).ok_or(
                RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                    RemoteProtobufResourceLimitV1::Arithmetic,
                ),
            )?;
            if byte == b'_' {
                *capitalize_next = true;
                continue;
            }
            let byte = if *capitalize_next {
                *capitalize_next = false;
                byte.to_ascii_uppercase()
            } else {
                byte
            };
            return Ok(Some(byte));
        }
        Ok(None)
    }

    let left = span_bytes(&projection.inputs, left)?;
    let right = span_bytes(&projection.inputs, right)?;
    let mut left_position = 0;
    let mut right_position = 0;
    let mut left_capitalize_next = false;
    let mut right_capitalize_next = false;
    loop {
        match (
            next_json_byte(left, &mut left_position, &mut left_capitalize_next, steps)?,
            next_json_byte(
                right,
                &mut right_position,
                &mut right_capitalize_next,
                steps,
            )?,
        ) {
            (None, None) => return Ok(true),
            (Some(left), Some(right)) if left == right => {}
            _ => return Ok(false),
        }
    }
}

fn validate_namespace_scope_v1(
    projection: &DescriptorProjectionV1<'_>,
    scope: NamespaceScopeV1,
    limits: &UnfrozenRemoteProtobufLimitsV1,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<(), RemoteProtobufInitializationErrorV1> {
    let mut outer = NamespaceMemberIterV1::new(scope);
    while let Some(left) = outer.next(projection, limits, steps)? {
        let mut inner = outer;
        while let Some(right) = inner.next(projection, limits, steps)? {
            let field_collision = match (left.field_syntax, right.field_syntax) {
                (Some(ProtobufSyntaxV1::Proto3), Some(ProtobufSyntaxV1::Proto3)) => {
                    normalized_field_names_equal_v1(projection, left.name, right.name, steps)?
                }
                (Some(ProtobufSyntaxV1::Proto2), Some(ProtobufSyntaxV1::Proto2)) => {
                    implicit_json_field_names_equal_v1(projection, left.name, right.name, steps)?
                }
                (None | Some(_), None) | (None, Some(_)) => false,
                (Some(_), Some(_)) => protobuf_fatal_invariant(
                    "protobuf fields in one message retained different file syntaxes",
                ),
            };
            if compare_spans_v1(projection, left.name, right.name, steps)?
                == std::cmp::Ordering::Equal
                || field_collision
            {
                return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
            }
        }
    }
    Ok(())
}

fn validate_package_type_collisions_v1(
    projection: &DescriptorProjectionV1<'_>,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<(), RemoteProtobufInitializationErrorV1> {
    for file_index in 0..projection.files.len {
        steps.consume()?;
        let file = projection.files.get(file_index)?;
        let Some(package) = file.package else {
            continue;
        };
        let package = span_str(&projection.inputs, package)?;
        for (position, byte) in package.bytes().enumerate() {
            steps.consume()?;
            if byte != b'.' && position + 1 != package.len() {
                continue;
            }
            let prefix_end = if byte == b'.' { position } else { position + 1 };
            let prefix = package.get(..prefix_end).ok_or_else(|| {
                protobuf_fatal_invariant("validated protobuf package lost an ASCII boundary")
            })?;
            let prefix_bytes = prefix.as_bytes();
            for symbol_index in 0..projection.symbols.len {
                steps.consume()?;
                if symbol_schema_index_v1(projection, symbol_index)? == file.schema_index
                    && symbol_matches_text_v1(projection, symbol_index, prefix, steps)?
                {
                    return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
                }
            }
            for enum_index in 0..projection.enums.len {
                steps.consume()?;
                let enumeration = projection.enums.get(enum_index)?;
                if enumeration.parent_message.is_some() {
                    continue;
                }
                let enum_file_index =
                    usize::try_from(enumeration.file_index).map_err(|_overflow| {
                        RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                            RemoteProtobufResourceLimitV1::Arithmetic,
                        )
                    })?;
                let enum_file = projection.files.get(enum_file_index)?;
                if enum_file.schema_index != file.schema_index {
                    continue;
                }
                let enum_package = enum_file
                    .package
                    .map(|package| span_bytes(&projection.inputs, package))
                    .transpose()?;
                for ordinal in
                    0..usize::try_from(enumeration.direct_values).map_err(|_overflow| {
                        RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                            RemoteProtobufResourceLimitV1::Arithmetic,
                        )
                    })?
                {
                    let value = enum_value_header_v1(projection, enum_index, ordinal, steps)?;
                    let value = span_bytes(&projection.inputs, value.name)?;
                    let namespace_len = enum_package
                        .map(|package| {
                            package.len().checked_add(1).ok_or(
                                RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                                    RemoteProtobufResourceLimitV1::Arithmetic,
                                ),
                            )
                        })
                        .transpose()?
                        .unwrap_or(0);
                    let candidate_len = namespace_len.checked_add(value.len()).ok_or(
                        RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                            RemoteProtobufResourceLimitV1::Arithmetic,
                        ),
                    )?;
                    if candidate_len != prefix_bytes.len() {
                        steps.consume()?;
                        continue;
                    }
                    let mut candidate_position = 0_usize;
                    let mut matches = true;
                    if let Some(enum_package) = enum_package {
                        for byte in enum_package {
                            steps.consume()?;
                            matches &= prefix_bytes.get(candidate_position).copied() == Some(*byte);
                            candidate_position = candidate_position.checked_add(1).ok_or(
                                RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                                    RemoteProtobufResourceLimitV1::Arithmetic,
                                ),
                            )?;
                        }
                        steps.consume()?;
                        matches &= prefix_bytes.get(candidate_position).copied() == Some(b'.');
                        candidate_position = candidate_position.checked_add(1).ok_or(
                            RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                                RemoteProtobufResourceLimitV1::Arithmetic,
                            ),
                        )?;
                    }
                    for byte in value {
                        steps.consume()?;
                        matches &= prefix_bytes.get(candidate_position).copied() == Some(*byte);
                        candidate_position = candidate_position.checked_add(1).ok_or(
                            RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                                RemoteProtobufResourceLimitV1::Arithmetic,
                            ),
                        )?;
                    }
                    if matches {
                        return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
                    }
                }
            }
        }
    }
    Ok(())
}

fn validate_namespace_v1(
    projection: &DescriptorProjectionV1<'_>,
    limits: &UnfrozenRemoteProtobufLimitsV1,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<(), RemoteProtobufInitializationErrorV1> {
    validate_package_type_collisions_v1(projection, steps)?;
    for message_index in 0..projection.messages.len {
        validate_namespace_scope_v1(
            projection,
            NamespaceScopeV1::Message(message_index),
            limits,
            steps,
        )?;
    }
    for file_index in 0..projection.files.len {
        let mut already_validated = false;
        for earlier_file in 0..file_index {
            steps.consume()?;
            if same_top_level_scope_v1(projection, file_index, earlier_file, steps)? {
                already_validated = true;
                break;
            }
        }
        if !already_validated {
            validate_namespace_scope_v1(
                projection,
                NamespaceScopeV1::TopLevel(file_index),
                limits,
                steps,
            )?;
        }
    }
    Ok(())
}

fn validate_descriptor_projection_v1(
    projection: &mut DescriptorProjectionV1<'_>,
    limits: &UnfrozenRemoteProtobufLimitsV1,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<(), RemoteProtobufInitializationErrorV1> {
    sort_and_validate_symbols_v1(projection, steps)?;
    validate_files_and_dependencies_v1(projection, limits, steps)?;
    validate_named_messages_v1(projection, steps)?;
    validate_fields_and_resolve_types_v1(projection, limits, steps)?;
    validate_oneofs_v1(projection, limits, steps)?;
    validate_enums_v1(projection, steps)?;
    validate_namespace_v1(projection, limits, steps)?;
    validate_map_entries_v1(projection, limits, steps)?;
    validate_builder_graph_v1(projection, limits, steps)
}

#[derive(Clone, Copy)]
struct BoundedProtobufFieldV1 {
    message_index: u32,
    ordinal: u32,
    name: ByteSpanV1,
    number: i32,
    label: FieldLabelV1,
    kind: FieldKindV1,
    type_name: Option<ByteSpanV1>,
    default: Option<ByteSpanV1>,
    oneof_index: Option<u32>,
    proto3_optional: bool,
    resolved_symbol: Option<u32>,
}

#[derive(Clone, Copy)]
struct BoundedProtobufEnumValueV1 {
    enum_index: u32,
    ordinal: u32,
    name: ByteSpanV1,
    number: i32,
}

#[derive(Clone, Copy)]
struct BoundedProtobufOneofV1 {
    message_index: u32,
    ordinal: u32,
    name: ByteSpanV1,
}

#[derive(Clone, Copy)]
enum MaterializationRecordV1<'definitions> {
    Schema(SchemaInputV1<'definitions>),
    File(FileProjectionV1),
    Message(MessageProjectionV1),
    Field(BoundedProtobufFieldV1),
    Enum(EnumProjectionV1),
    EnumValue(BoundedProtobufEnumValueV1),
    Oneof(BoundedProtobufOneofV1),
    Symbol(SymbolProjectionV1),
    SortedSymbol(u32),
    Dependency(DependencyProjectionV1),
    SortedDependency(u32),
    Resolution(FieldResolutionProjectionV1),
    NamedMessage(u32),
}

fn walk_materialization_v1<'definitions>(
    projection: &DescriptorProjectionV1<'definitions>,
    limits: &UnfrozenRemoteProtobufLimitsV1,
    steps: &mut RemoteProtobufStepOwnerV1,
    mut emit: impl FnMut(
        MaterializationRecordV1<'definitions>,
    ) -> Result<(), RemoteProtobufInitializationErrorV1>,
) -> Result<(), RemoteProtobufInitializationErrorV1> {
    let mut no_accounting = RemoteProtobufCensusV1::default();
    for index in 0..projection.inputs.len {
        steps.consume()?;
        emit(MaterializationRecordV1::Schema(
            projection.inputs.get(index)?,
        ))?;
    }
    for index in 0..projection.files.len {
        steps.consume()?;
        emit(MaterializationRecordV1::File(projection.files.get(index)?))?;
    }
    for message_index in 0..projection.messages.len {
        steps.consume()?;
        let message = projection.messages.get(message_index)?;
        emit(MaterializationRecordV1::Message(message))?;
        for ordinal in 0..usize::try_from(message.direct_fields).map_err(|_overflow| {
            RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                RemoteProtobufResourceLimitV1::Arithmetic,
            )
        })? {
            let body = direct_field_span_v1(projection, message_index, ordinal, steps)?;
            let field =
                parse_field_header_v1(body, projection, &mut no_accounting, limits, steps, false)?;
            let resolved_symbol =
                resolution_for_field_v1(projection, message_index, ordinal, steps)?;
            let kind = effective_field_kind_v1(projection, field, resolved_symbol)?;
            steps.consume()?;
            emit(MaterializationRecordV1::Field(BoundedProtobufFieldV1 {
                message_index: checked_u32(message_index)?,
                ordinal: checked_u32(ordinal)?,
                name: field.name,
                number: field.number,
                label: field.label,
                kind,
                type_name: field.type_name,
                default: field.default,
                oneof_index: field.oneof_index,
                proto3_optional: field.proto3_optional,
                resolved_symbol: resolved_symbol.map(checked_u32).transpose()?,
            }))?;
        }
        for ordinal in 0..usize::try_from(message.direct_oneofs).map_err(|_overflow| {
            RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                RemoteProtobufResourceLimitV1::Arithmetic,
            )
        })? {
            steps.consume()?;
            emit(MaterializationRecordV1::Oneof(BoundedProtobufOneofV1 {
                message_index: checked_u32(message_index)?,
                ordinal: checked_u32(ordinal)?,
                name: oneof_name_v1(projection, message_index, ordinal, steps)?,
            }))?;
        }
    }
    for enum_index in 0..projection.enums.len {
        steps.consume()?;
        let value = projection.enums.get(enum_index)?;
        emit(MaterializationRecordV1::Enum(value))?;
        for ordinal in 0..usize::try_from(value.direct_values).map_err(|_overflow| {
            RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                RemoteProtobufResourceLimitV1::Arithmetic,
            )
        })? {
            let header = enum_value_header_v1(projection, enum_index, ordinal, steps)?;
            steps.consume()?;
            emit(MaterializationRecordV1::EnumValue(
                BoundedProtobufEnumValueV1 {
                    enum_index: checked_u32(enum_index)?,
                    ordinal: checked_u32(ordinal)?,
                    name: header.name,
                    number: header.number,
                },
            ))?;
        }
    }
    for index in 0..projection.symbols.len {
        steps.consume()?;
        emit(MaterializationRecordV1::Symbol(
            projection.symbols.get(index)?,
        ))?;
    }
    for index in 0..projection.sorted_symbols.len {
        steps.consume()?;
        emit(MaterializationRecordV1::SortedSymbol(
            projection.sorted_symbols.get(index)?,
        ))?;
    }
    for index in 0..projection.dependencies.len {
        steps.consume()?;
        emit(MaterializationRecordV1::Dependency(
            projection.dependencies.get(index)?,
        ))?;
    }
    for index in 0..projection.sorted_dependencies.len {
        steps.consume()?;
        emit(MaterializationRecordV1::SortedDependency(
            projection.sorted_dependencies.get(index)?,
        ))?;
    }
    for index in 0..projection.resolutions.len {
        steps.consume()?;
        emit(MaterializationRecordV1::Resolution(
            projection.resolutions.get(index)?,
        ))?;
    }
    for index in 0..projection.named_messages.len {
        steps.consume()?;
        emit(MaterializationRecordV1::NamedMessage(
            projection.named_messages.get(index)?,
        ))?;
    }
    Ok(())
}

#[derive(Default)]
struct MaterializationCountsV1 {
    schemas: u64,
    files: u64,
    messages: u64,
    fields: u64,
    enums: u64,
    enum_values: u64,
    oneofs: u64,
    symbols: u64,
    sorted_symbols: u64,
    dependencies: u64,
    sorted_dependencies: u64,
    resolutions: u64,
    named_messages: u64,
}

impl MaterializationCountsV1 {
    fn observe(
        &mut self,
        record: MaterializationRecordV1<'_>,
    ) -> Result<(), RemoteProtobufInitializationErrorV1> {
        let counter = match record {
            MaterializationRecordV1::Schema(_) => &mut self.schemas,
            MaterializationRecordV1::File(_) => &mut self.files,
            MaterializationRecordV1::Message(_) => &mut self.messages,
            MaterializationRecordV1::Field(_) => &mut self.fields,
            MaterializationRecordV1::Enum(_) => &mut self.enums,
            MaterializationRecordV1::EnumValue(_) => &mut self.enum_values,
            MaterializationRecordV1::Oneof(_) => &mut self.oneofs,
            MaterializationRecordV1::Symbol(_) => &mut self.symbols,
            MaterializationRecordV1::SortedSymbol(_) => &mut self.sorted_symbols,
            MaterializationRecordV1::Dependency(_) => &mut self.dependencies,
            MaterializationRecordV1::SortedDependency(_) => &mut self.sorted_dependencies,
            MaterializationRecordV1::Resolution(_) => &mut self.resolutions,
            MaterializationRecordV1::NamedMessage(_) => &mut self.named_messages,
        };
        *counter = checked_add(*counter, 1)?;
        Ok(())
    }

    fn verify(
        &self,
        census: RemoteProtobufCensusV1,
        projection: &DescriptorProjectionV1<'_>,
    ) -> Result<(), RemoteProtobufInitializationErrorV1> {
        let exact = self.schemas == census.schemas
            && self.files == census.files
            && self.messages == census.messages
            && self.fields == census.fields
            && self.enums == census.enums
            && self.enum_values == census.enum_values
            && self.oneofs == census.oneofs
            && self.symbols == checked_add(census.messages, census.enums)?
            && self.sorted_symbols == self.symbols
            && self.dependencies == census.dependencies
            && self.sorted_dependencies == census.dependencies
            && self.resolutions
                == u64::try_from(projection.resolutions.len).map_err(|_overflow| {
                    RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                        RemoteProtobufResourceLimitV1::Arithmetic,
                    )
                })?
            && self.named_messages == census.schemas;
        if exact {
            Ok(())
        } else {
            protobuf_fatal_invariant("protobuf dry-run counts differ from census")
        }
    }
}

trait RemoteProtobufAllocationGateV1 {
    fn before_allocation(
        &self,
        arena_index: usize,
        layout: Layout,
    ) -> Result<(), RemoteProtobufInitializationErrorV1>;

    fn after_allocation(&self, arena_index: usize);

    fn before_result(&self) -> Result<(), RemoteProtobufInitializationErrorV1> {
        Ok(())
    }
}

struct SystemRemoteProtobufAllocationGateV1;

impl RemoteProtobufAllocationGateV1 for SystemRemoteProtobufAllocationGateV1 {
    fn before_allocation(
        &self,
        _arena_index: usize,
        _layout: Layout,
    ) -> Result<(), RemoteProtobufInitializationErrorV1> {
        Ok(())
    }

    fn after_allocation(&self, _arena_index: usize) {}
}

struct FixedProtobufArenaV1<T> {
    storage: Vec<T>,
}

impl<T> FixedProtobufArenaV1<T> {
    fn try_new(
        capacity: usize,
        arena_index: usize,
        gate: &impl RemoteProtobufAllocationGateV1,
    ) -> Result<Self, RemoteProtobufInitializationErrorV1> {
        let layout = Layout::array::<T>(capacity).map_err(|_overflow| {
            RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                RemoteProtobufResourceLimitV1::Arithmetic,
            )
        })?;
        gate.before_allocation(arena_index, layout)?;
        let mut storage = Vec::new();
        storage
            .try_reserve_exact(capacity)
            .map_err(|_allocation| RemoteProtobufInitializationErrorV1::FallibleAllocationFailed)?;
        gate.after_allocation(arena_index);
        Ok(Self { storage })
    }

    fn push(&mut self, value: T) {
        if self.storage.len() >= self.storage.capacity() {
            protobuf_fatal_invariant("protobuf fixed arena census undercounted materialization");
        }
        self.storage.push(value);
    }

    fn as_slice(&self) -> &[T] {
        self.storage.as_slice()
    }
}

struct BoundedProtobufDescriptorGraphV1<'definitions> {
    schemas: FixedProtobufArenaV1<SchemaInputV1<'definitions>>,
    files: FixedProtobufArenaV1<FileProjectionV1>,
    messages: FixedProtobufArenaV1<MessageProjectionV1>,
    fields: FixedProtobufArenaV1<BoundedProtobufFieldV1>,
    enums: FixedProtobufArenaV1<EnumProjectionV1>,
    enum_values: FixedProtobufArenaV1<BoundedProtobufEnumValueV1>,
    oneofs: FixedProtobufArenaV1<BoundedProtobufOneofV1>,
    symbols: FixedProtobufArenaV1<SymbolProjectionV1>,
    sorted_symbols: FixedProtobufArenaV1<u32>,
    dependencies: FixedProtobufArenaV1<DependencyProjectionV1>,
    sorted_dependencies: FixedProtobufArenaV1<u32>,
    resolutions: FixedProtobufArenaV1<FieldResolutionProjectionV1>,
    named_messages: FixedProtobufArenaV1<u32>,
}

/// Sealed executable decoder/config identity derived only inside the bounded initializer module.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct FrozenRemoteExecutableConfigV1 {
    kind: u8,
    schema_handle: u16,
    canonical_digest: [u8; 16],
    max_roots_per_partition: u32,
    max_external_origin_bytes_per_partition: u64,
}

impl FrozenRemoteExecutableConfigV1 {
    pub(crate) const fn canonical_digest_v1(self) -> [u8; 16] {
        self.canonical_digest
    }
    pub(crate) const fn registration_contract_v1(self) -> (u32, u64) {
        (
            self.max_roots_per_partition,
            self.max_external_origin_bytes_per_partition,
        )
    }

    #[cfg(test)]
    pub(crate) fn new_for_channel_group_test_v1(kind: u8, schema_handle: u16) -> Self {
        let mut digest = [0; 16];
        digest[0] = kind;
        digest[1..3].copy_from_slice(&schema_handle.to_le_bytes());
        Self {
            kind,
            schema_handle,
            canonical_digest: digest,
            max_roots_per_partition: 1,
            max_external_origin_bytes_per_partition: 128,
        }
    }
}

impl BoundedProtobufDescriptorGraphV1<'_> {
    fn has_schema_id(&self, schema_id: u16) -> bool {
        self.schemas
            .as_slice()
            .iter()
            .any(|schema| schema.schema_id == schema_id)
    }

    fn schema_shape_v1(&self, schema_id: u16) -> usize {
        self.schemas
            .as_slice()
            .iter()
            .position(|schema| schema.schema_id == schema_id)
            .map_or(0, |index| index.saturating_add(1))
    }

    fn verify_lengths(
        &self,
        census: RemoteProtobufCensusV1,
        projection: &DescriptorProjectionV1<'_>,
    ) -> Result<(), RemoteProtobufInitializationErrorV1> {
        let exact = self.schemas.storage.len() == checked_usize(census.schemas)?
            && self.files.storage.len() == checked_usize(census.files)?
            && self.messages.storage.len() == checked_usize(census.messages)?
            && self.fields.storage.len() == checked_usize(census.fields)?
            && self.enums.storage.len() == checked_usize(census.enums)?
            && self.enum_values.storage.len() == checked_usize(census.enum_values)?
            && self.oneofs.storage.len() == checked_usize(census.oneofs)?
            && self.symbols.storage.len()
                == checked_usize(checked_add(census.messages, census.enums)?)?
            && self.sorted_symbols.storage.len() == self.symbols.storage.len()
            && self.dependencies.storage.len() == checked_usize(census.dependencies)?
            && self.sorted_dependencies.storage.len() == self.dependencies.storage.len()
            && self.resolutions.storage.len() == projection.resolutions.len
            && self.named_messages.storage.len() == checked_usize(census.schemas)?;
        if exact {
            Ok(())
        } else {
            protobuf_fatal_invariant("protobuf fixed arena lengths differ from census")
        }
    }

    fn canonical_digest_for_schema_v1(
        &self,
        schema_id: u16,
        policy_versions: (u16, u16, u16),
    ) -> Result<[u8; 16], RemoteProtobufInitializationErrorV1> {
        let mut hasher = Sha256::new();
        hasher.update(b"rerun.remote-mcap.protobuf-config.v2\0");
        hasher.update(policy_versions.0.to_le_bytes());
        hasher.update(policy_versions.1.to_le_bytes());
        hasher.update(policy_versions.2.to_le_bytes());
        hasher.update(schema_id.to_le_bytes());
        for schema in self.schemas.as_slice() {
            hasher.update(schema.schema_id.to_le_bytes());
            hasher.update((schema.name.len() as u64).to_le_bytes());
            hasher.update(schema.name.as_bytes());
        }
        for file in self.files.as_slice() {
            hasher.update(file.schema_index.to_le_bytes());
            self.hash_span_v1(&mut hasher, file.name)?;
            self.hash_option_span_v1(&mut hasher, file.package)?;
            hasher.update([match file.syntax {
                ProtobufSyntaxV1::Proto2 => 2,
                ProtobufSyntaxV1::Proto3 => 3,
            }]);
        }
        for message in self.messages.as_slice() {
            hasher.update(message.file_index.to_le_bytes());
            hash_option_u32(&mut hasher, message.parent_message);
            self.hash_span_v1(&mut hasher, message.name)?;
            hasher.update([message.map_entry as u8]);
            hasher.update(message.direct_fields.to_le_bytes());
            hasher.update(message.direct_oneofs.to_le_bytes());
        }
        for value in self.enums.as_slice() {
            hasher.update(value.file_index.to_le_bytes());
            hash_option_u32(&mut hasher, value.parent_message);
            self.hash_span_v1(&mut hasher, value.name)?;
            hasher.update(value.direct_values.to_le_bytes());
        }
        for field in self.fields.as_slice() {
            hasher.update(field.message_index.to_le_bytes());
            hasher.update(field.ordinal.to_le_bytes());
            self.hash_span_v1(&mut hasher, field.name)?;
            hasher.update(field.number.to_le_bytes());
            hasher.update([field.label as u8, field.kind as u8]);
            self.hash_option_span_v1(&mut hasher, field.type_name)?;
            self.hash_option_span_v1(&mut hasher, field.default)?;
            hash_option_u32(&mut hasher, field.oneof_index);
            hasher.update([field.proto3_optional as u8]);
            hash_option_u32(&mut hasher, field.resolved_symbol);
        }
        for value in self.enum_values.as_slice() {
            hasher.update(value.enum_index.to_le_bytes());
            hasher.update(value.ordinal.to_le_bytes());
            self.hash_span_v1(&mut hasher, value.name)?;
            hasher.update(value.number.to_le_bytes());
        }
        for value in self.oneofs.as_slice() {
            hasher.update(value.message_index.to_le_bytes());
            hasher.update(value.ordinal.to_le_bytes());
            self.hash_span_v1(&mut hasher, value.name)?;
        }
        for value in self.symbols.as_slice() {
            hasher.update([value.kind as u8]);
            hasher.update(value.node_index.to_le_bytes());
        }
        for value in self.sorted_symbols.as_slice() {
            hasher.update(value.to_le_bytes());
        }
        for value in self.dependencies.as_slice() {
            hasher.update(value.file_index.to_le_bytes());
            self.hash_span_v1(&mut hasher, value.name)?;
        }
        for value in self.sorted_dependencies.as_slice() {
            hasher.update(value.to_le_bytes());
        }
        for value in self.resolutions.as_slice() {
            hasher.update(value.message_index.to_le_bytes());
            hasher.update(value.field_ordinal.to_le_bytes());
            hasher.update(value.symbol_index.to_le_bytes());
        }
        for value in self.named_messages.as_slice() {
            hasher.update(value.to_le_bytes());
        }
        let full = hasher.finalize();
        let mut digest = [0; 16];
        digest.copy_from_slice(&full[..16]);
        Ok(digest)
    }

    fn hash_span_v1(
        &self,
        hasher: &mut Sha256,
        span: ByteSpanV1,
    ) -> Result<(), RemoteProtobufInitializationErrorV1> {
        let schema = self
            .schemas
            .as_slice()
            .get(usize::from(span.schema_index))
            .ok_or(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema)?;
        let bytes = schema
            .data
            .get(
                usize::try_from(span.start)
                    .map_err(|_overflow| RemoteProtobufInitializationErrorV1::InvalidRemoteSchema)?
                    ..span.end()?,
            )
            .ok_or(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema)?;
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
        Ok(())
    }

    fn hash_option_span_v1(
        &self,
        hasher: &mut Sha256,
        span: Option<ByteSpanV1>,
    ) -> Result<(), RemoteProtobufInitializationErrorV1> {
        if let Some(value) = span {
            hasher.update([1]);
            self.hash_span_v1(hasher, value)
        } else {
            hasher.update([0]);
            Ok(())
        }
    }
}

fn config_for_schema_id_v1(
    schema_id: u16,
    configs: &[(u16, FrozenRemoteExecutableConfigV1)],
) -> Result<FrozenRemoteExecutableConfigV1, RemoteProtobufInitializationErrorV1> {
    configs
        .iter()
        .find(|(id, _)| *id == schema_id)
        .map(|(_, config)| *config)
        .ok_or(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema)
}

fn hash_option_u32(hasher: &mut Sha256, value: Option<u32>) {
    match value {
        Some(value) => {
            hasher.update([1]);
            hasher.update(value.to_le_bytes());
        }
        None => hasher.update([0]),
    }
}

const REMOTE_PROTOBUF_GRAPH_ARENA_COUNT_V1: usize = 13;
const REMOTE_PROTOBUF_ARENA_COUNT_V1: usize = 14;

fn allocate_graph_v1<'definitions>(
    census: RemoteProtobufCensusV1,
    projection: &DescriptorProjectionV1<'definitions>,
    gate: &impl RemoteProtobufAllocationGateV1,
    steps: &mut RemoteProtobufStepOwnerV1,
) -> Result<BoundedProtobufDescriptorGraphV1<'definitions>, RemoteProtobufInitializationErrorV1> {
    let mut index = 0_usize;
    macro_rules! arena {
        ($ty:ty, $count:expr) => {{
            steps.consume()?;
            let arena = FixedProtobufArenaV1::<$ty>::try_new(checked_usize($count)?, index, gate)?;
            index = index.checked_add(1).ok_or(
                RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                    RemoteProtobufResourceLimitV1::Arithmetic,
                ),
            )?;
            arena
        }};
    }
    let graph = BoundedProtobufDescriptorGraphV1 {
        schemas: arena!(SchemaInputV1<'definitions>, census.schemas),
        files: arena!(FileProjectionV1, census.files),
        messages: arena!(MessageProjectionV1, census.messages),
        fields: arena!(BoundedProtobufFieldV1, census.fields),
        enums: arena!(EnumProjectionV1, census.enums),
        enum_values: arena!(BoundedProtobufEnumValueV1, census.enum_values),
        oneofs: arena!(BoundedProtobufOneofV1, census.oneofs),
        symbols: arena!(
            SymbolProjectionV1,
            checked_add(census.messages, census.enums)?
        ),
        sorted_symbols: arena!(u32, checked_add(census.messages, census.enums)?),
        dependencies: arena!(DependencyProjectionV1, census.dependencies),
        sorted_dependencies: arena!(u32, census.dependencies),
        resolutions: arena!(
            FieldResolutionProjectionV1,
            u64::try_from(projection.resolutions.len).map_err(|_overflow| {
                RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                    RemoteProtobufResourceLimitV1::Arithmetic,
                )
            })?
        ),
        named_messages: arena!(u32, census.schemas),
    };
    if index != REMOTE_PROTOBUF_GRAPH_ARENA_COUNT_V1 {
        protobuf_fatal_invariant("protobuf arena directory count changed");
    }
    Ok(graph)
}

fn push_materialization_record_v1<'definitions>(
    graph: &mut BoundedProtobufDescriptorGraphV1<'definitions>,
    record: MaterializationRecordV1<'definitions>,
) {
    match record {
        MaterializationRecordV1::Schema(value) => graph.schemas.push(value),
        MaterializationRecordV1::File(value) => graph.files.push(value),
        MaterializationRecordV1::Message(value) => graph.messages.push(value),
        MaterializationRecordV1::Field(value) => graph.fields.push(value),
        MaterializationRecordV1::Enum(value) => graph.enums.push(value),
        MaterializationRecordV1::EnumValue(value) => graph.enum_values.push(value),
        MaterializationRecordV1::Oneof(value) => graph.oneofs.push(value),
        MaterializationRecordV1::Symbol(value) => graph.symbols.push(value),
        MaterializationRecordV1::SortedSymbol(value) => graph.sorted_symbols.push(value),
        MaterializationRecordV1::Dependency(value) => graph.dependencies.push(value),
        MaterializationRecordV1::SortedDependency(value) => {
            graph.sorted_dependencies.push(value);
        }
        MaterializationRecordV1::Resolution(value) => graph.resolutions.push(value),
        MaterializationRecordV1::NamedMessage(value) => graph.named_messages.push(value),
    }
}

fn align_up_checked(
    value: u64,
    alignment: u64,
) -> Result<u64, RemoteProtobufInitializationErrorV1> {
    if alignment == 0 || !alignment.is_power_of_two() {
        protobuf_fatal_invariant("protobuf allocator alignment is invalid");
    }
    let mask = alignment - 1;
    Ok(checked_add(value, mask)? & !mask)
}

fn locked_wasm_allocation_footprint_v1(
    layout: Layout,
) -> Result<u64, RemoteProtobufInitializationErrorV1> {
    let requested = u64::try_from(layout.size()).map_err(|_overflow| {
        RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
            RemoteProtobufResourceLimitV1::Arithmetic,
        )
    })?;
    if requested == 0 {
        return Ok(0);
    }
    let alignment = u64::try_from(layout.align()).map_err(|_overflow| {
        RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
            RemoteProtobufResourceLimitV1::Arithmetic,
        )
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

fn arena_allocation_footprint_v1<T>(
    count: u64,
) -> Result<u64, RemoteProtobufInitializationErrorV1> {
    let layout = Layout::array::<T>(checked_usize(count)?).map_err(|_overflow| {
        RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
            RemoteProtobufResourceLimitV1::Arithmetic,
        )
    })?;
    locked_wasm_allocation_footprint_v1(layout)
}

fn checked_retained_bytes_v1(
    census: RemoteProtobufCensusV1,
    projection: &DescriptorProjectionV1<'_>,
) -> Result<u64, RemoteProtobufInitializationErrorV1> {
    let mut retained = 0_u64;
    macro_rules! charge {
        ($ty:ty, $count:expr) => {
            retained = checked_add(retained, arena_allocation_footprint_v1::<$ty>($count)?)?;
        };
    }
    charge!(SchemaInputV1<'static>, census.schemas);
    charge!(FileProjectionV1, census.files);
    charge!(MessageProjectionV1, census.messages);
    charge!(BoundedProtobufFieldV1, census.fields);
    charge!(EnumProjectionV1, census.enums);
    charge!(BoundedProtobufEnumValueV1, census.enum_values);
    charge!(BoundedProtobufOneofV1, census.oneofs);
    charge!(
        SymbolProjectionV1,
        checked_add(census.messages, census.enums)?
    );
    charge!(u32, checked_add(census.messages, census.enums)?);
    charge!(DependencyProjectionV1, census.dependencies);
    charge!(u32, census.dependencies);
    charge!(
        FieldResolutionProjectionV1,
        u64::try_from(projection.resolutions.len).map_err(|_overflow| {
            RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                RemoteProtobufResourceLimitV1::Arithmetic,
            )
        })?
    );
    charge!(u32, census.schemas);
    charge!((u16, FrozenRemoteExecutableConfigV1), census.schemas);
    checked_add(
        retained,
        u64::try_from(std::mem::size_of::<
            BoundedRemoteDecoderInitializersV1<'static, 'static, 'static, 'static>,
        >())
        .map_err(|_overflow| {
            RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                RemoteProtobufResourceLimitV1::Arithmetic,
            )
        })?,
    )
}

fn compute_peak_v1(
    census: &mut RemoteProtobufCensusV1,
    projection: &DescriptorProjectionV1<'_>,
    limits: &UnfrozenRemoteProtobufLimitsV1,
) -> Result<(), RemoteProtobufInitializationErrorV1> {
    #[cfg(target_arch = "wasm32")]
    verify_artifact_stage_identity(rerun_remote_protobuf_peak_stage_v1(), 0x2703);
    census.retained_bytes = checked_retained_bytes_v1(*census, projection)?;
    if census.retained_bytes > limits.max_retained_bytes {
        return Err(RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
            RemoteProtobufResourceLimitV1::RetainedBytes,
        ));
    }
    let fixed = std::mem::size_of::<DescriptorProjectionV1<'static>>()
        .checked_add(std::mem::size_of::<MaterializationCountsV1>())
        .and_then(|value| {
            value.checked_add(std::mem::size_of::<
                PreparedRemoteProtobufCensusV1<'static, 'static, 'static, 'static>,
            >())
        })
        .ok_or(RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
            RemoteProtobufResourceLimitV1::Arithmetic,
        ))?;
    if std::mem::size_of::<DescriptorProjectionV1<'static>>()
        > usize::try_from(LOCKED_REMOTE_PROTOBUF_FRAME_SPILL_CEILING_V1).map_err(|_overflow| {
            RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                RemoteProtobufResourceLimitV1::Arithmetic,
            )
        })?
    {
        protobuf_fatal_invariant("protobuf allocation-free projection exceeds the frame ceiling");
    }
    census.working_bytes = checked_add(
        census.retained_bytes,
        checked_add(
            u64::try_from(fixed).map_err(|_overflow| {
                RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                    RemoteProtobufResourceLimitV1::Arithmetic,
                )
            })?,
            LOCKED_REMOTE_PROTOBUF_FRAME_SPILL_CEILING_V1,
        )?,
    )?;
    if census.working_bytes > limits.max_working_bytes {
        return Err(RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
            RemoteProtobufResourceLimitV1::WorkingBytes,
        ));
    }
    Ok(())
}

fn validate_limit_profile_v1(limits: &UnfrozenRemoteProtobufLimitsV1) {
    if limits.profile_version != REMOTE_PROTOBUF_PROFILE_VERSION_V1
        || limits.max_schemas > MAX_INLINE_PROTOBUF_SCHEMAS_V1 as u64
        || limits.max_files > MAX_INLINE_PROTOBUF_FILES_V1 as u64
        || limits.max_messages > MAX_INLINE_PROTOBUF_MESSAGES_V1 as u64
        || limits.max_enums > MAX_INLINE_PROTOBUF_ENUMS_V1 as u64
        || match checked_add(limits.max_messages, limits.max_enums) {
            Ok(symbols) => symbols > MAX_INLINE_PROTOBUF_SYMBOLS_V1 as u64,
            Err(_) => true,
        }
        || limits.max_dependencies > MAX_INLINE_PROTOBUF_DEPENDENCIES_V1 as u64
        || limits.max_fields > MAX_INLINE_PROTOBUF_RESOLUTIONS_V1 as u64
        || limits.max_descriptor_depth > MAX_STACK_DESCRIPTOR_DEPTH_V1 as u64
        || limits.max_import_depth > MAX_STACK_DESCRIPTOR_DEPTH_V1 as u64
    {
        protobuf_fatal_control_plane("protobuf resource profile exceeds its V1 structural shape");
    }
}

impl<'viewer, 'profile> RemoteProtobufInitializationBudgetV1<'viewer, 'profile> {
    #[cfg(any(test, re_mcap_locked_remote_wasm_allocator_v1))]
    pub(crate) fn new_disarmed_v1(
        viewer_scope: &'viewer RemoteViewerScopeState,
        profile_scope: &'profile RemoteProtobufProfileScopeV1,
        limits: UnfrozenRemoteProtobufLimitsV1,
        max_active_initializers: u64,
        max_aggregate_working_bytes: u64,
        max_retained_results: u64,
        max_aggregate_retained_bytes: u64,
    ) -> Self {
        validate_limit_profile_v1(&limits);
        Self {
            state: Arc::new(RemoteProtobufBudgetStateV1 {
                limits,
                capacity: RemoteProtobufBudgetCapacityV1 {
                    max_active_initializers,
                    max_working_bytes: max_aggregate_working_bytes,
                    max_retained_results,
                    max_retained_bytes: max_aggregate_retained_bytes,
                },
                usage: Mutex::new(RemoteProtobufBudgetUsageV1::default()),
                poisoned: AtomicBool::new(false),
                viewer_scope_identity: std::ptr::from_ref(viewer_scope).addr(),
                profile_scope_identity: std::ptr::from_ref(profile_scope).addr(),
            }),
            viewer_scope,
            profile_scope,
        }
    }

    #[cfg(test)]
    pub(crate) fn is_idle_for_assignment_test_v1(&self) -> bool {
        *self.state.usage.lock() == RemoteProtobufBudgetUsageV1::default()
    }
}

pub(crate) struct PreparedRemoteProtobufCensusV1<'definitions, 'input, 'source, 'wire> {
    continuation: RemoteProtobufProjectionEofContinuationV1<'definitions, 'input, 'source, 'wire>,
    projection: DescriptorProjectionV1<'definitions>,
    census: RemoteProtobufCensusV1,
    reservation: RemoteProtobufWorkReservationV1,
    materialization_steps: RemoteProtobufStepOwnerV1,
    viewer_scope: *const RemoteViewerScopeState,
    profile_scope: *const RemoteProtobufProfileScopeV1,
}

pub(crate) fn prepare_remote_protobuf_census_v1<'definitions, 'input, 'source, 'wire>(
    transition: crate::remote_ros2_reflection::RemoteRos2ToProtobufTransitionV1<
        'definitions,
        'input,
        'source,
        'wire,
    >,
    budget: &RemoteProtobufInitializationBudgetV1<'_, '_>,
) -> Result<
    PreparedRemoteProtobufCensusV1<'definitions, 'input, 'source, 'wire>,
    RemoteProtobufInitializationErrorV1,
> {
    #[cfg(target_arch = "wasm32")]
    verify_artifact_stage_identity(rerun_remote_protobuf_wire_stage_v1(), 0x2701);
    validate_limit_profile_v1(&budget.state.limits);
    if budget.state.viewer_scope_identity != std::ptr::from_ref(budget.viewer_scope).addr()
        || budget.state.profile_scope_identity != std::ptr::from_ref(budget.profile_scope).addr()
        || budget.state.poisoned.load(Ordering::Acquire)
    {
        protobuf_fatal_control_plane("protobuf budget scope identity changed");
    }
    let mut owner = transition
        .into_protobuf_projection_v1()
        .map_err(map_ros_error)?;
    owner
        .ensure_protobuf_profile_current_v1(budget.viewer_scope, budget.profile_scope)
        .map_err(map_ros_error)?;
    let limits = budget.state.limits;
    let mut projection = DescriptorProjectionV1::new();
    let mut census = RemoteProtobufCensusV1::default();
    let mut census_steps = RemoteProtobufStepOwnerV1::new(
        limits.max_census_steps,
        RemoteProtobufResourceLimitV1::CensusSteps,
    );
    while let Some(schema) = owner.next_schema().map_err(map_ros_error)? {
        let encoding = schema.encoding().map_err(map_ros_error)?;
        census_steps.consume_bytes(encoding.len())?;
        if encoding != "protobuf" {
            protobuf_fatal_invariant("protobuf projection yielded another schema encoding");
        }
        let name = schema.name().map_err(map_ros_error)?;
        census_steps.consume_bytes(name.len())?;
        if !valid_qualified_identifier(name, false) {
            return Err(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema);
        }
        let data = schema.data().map_err(map_ros_error)?;
        increment_limited(
            &mut census.schemas,
            1,
            limits.max_schemas,
            RemoteProtobufResourceLimitV1::SchemaCount,
        )?;
        increment_limited(
            &mut census.descriptor_bytes,
            u64::try_from(data.len()).map_err(|_overflow| {
                RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                    RemoteProtobufResourceLimitV1::Arithmetic,
                )
            })?,
            limits.max_descriptor_bytes,
            RemoteProtobufResourceLimitV1::DescriptorBytes,
        )?;
        if name.len() as u64 > limits.max_single_string_bytes {
            return Err(RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                RemoteProtobufResourceLimitV1::SingleStringBytes,
            ));
        }
        increment_limited(
            &mut census.string_bytes,
            name.len() as u64,
            limits.max_string_bytes,
            RemoteProtobufResourceLimitV1::StringBytes,
        )?;
        let schema_index = projection.inputs.push(
            SchemaInputV1 {
                schema_id: schema.schema_id().map_err(map_ros_error)?,
                name,
                data,
            },
            RemoteProtobufResourceLimitV1::SchemaCount,
        )?;
        collect_descriptor_set_v1(
            schema_index,
            &mut projection,
            &mut census,
            &limits,
            &mut census_steps,
        )?;
    }
    let continuation = owner.finish_eof_v1().map_err(map_ros_error)?;
    continuation
        .ensure_profile_current_v1(budget.viewer_scope, budget.profile_scope)
        .map_err(map_ros_error)?;
    census.census_steps = census_steps.consumed;

    let mut resolution_steps = RemoteProtobufStepOwnerV1::new(
        limits.max_resolution_steps,
        RemoteProtobufResourceLimitV1::ResolutionSteps,
    );
    #[cfg(target_arch = "wasm32")]
    verify_artifact_stage_identity(rerun_remote_protobuf_graph_stage_v1(), 0x2702);
    validate_descriptor_projection_v1(&mut projection, &limits, &mut resolution_steps)?;
    census.resolution_steps = resolution_steps.consumed;

    let mut dry_run_steps = RemoteProtobufStepOwnerV1::new(
        limits.max_materialization_steps,
        RemoteProtobufResourceLimitV1::MaterializationSteps,
    );
    for _arena in 0..REMOTE_PROTOBUF_GRAPH_ARENA_COUNT_V1 {
        dry_run_steps.consume()?;
    }
    let mut counts = MaterializationCountsV1::default();
    walk_materialization_v1(&projection, &limits, &mut dry_run_steps, |record| {
        counts.observe(record)
    })?;
    counts.verify(census, &projection)?;
    census.materialization_steps = dry_run_steps.consumed;
    compute_peak_v1(&mut census, &projection, &limits)?;
    continuation
        .ensure_profile_current_v1(budget.viewer_scope, budget.profile_scope)
        .map_err(map_ros_error)?;
    let reservation = budget
        .state
        .reserve(census.working_bytes, census.retained_bytes)?;
    continuation
        .ensure_profile_current_v1(budget.viewer_scope, budget.profile_scope)
        .map_err(map_ros_error)?;
    Ok(PreparedRemoteProtobufCensusV1 {
        continuation,
        projection,
        census,
        reservation,
        materialization_steps: RemoteProtobufStepOwnerV1::new(
            census.materialization_steps,
            RemoteProtobufResourceLimitV1::MaterializationSteps,
        ),
        viewer_scope: budget.viewer_scope,
        profile_scope: budget.profile_scope,
    })
}

pub(crate) struct BoundedRemoteDecoderInitializersV1<'definitions, 'input, 'source, 'wire> {
    continuation: RemoteProtobufProjectionEofContinuationV1<'definitions, 'input, 'source, 'wire>,
    protobuf: BoundedProtobufDescriptorGraphV1<'definitions>,
    executable_configs: FixedProtobufArenaV1<(u16, FrozenRemoteExecutableConfigV1)>,
    membership_projected: bool,
    _reservation: RemoteProtobufResultReservationV1,
}

impl<'definitions, 'input, 'source, 'wire>
    BoundedRemoteDecoderInitializersV1<'definitions, 'input, 'source, 'wire>
{
    pub(crate) fn protobuf_schema_count(&self) -> usize {
        self.protobuf.schemas.as_slice().len()
    }

    pub(crate) fn selected_count_for_assignment_v1(
        &self,
    ) -> Result<usize, RemoteProtobufInitializationErrorV1> {
        self.continuation
            .selected_count_for_assignment_v1()
            .map_err(map_ros_recognition_error)
    }

    pub(crate) fn canonical_channel_count_for_assignment_v1(
        &self,
    ) -> Result<u64, RemoteProtobufInitializationErrorV1> {
        self.continuation
            .canonical_channel_count_for_assignment_v1()
            .map_err(map_ros_recognition_error)
    }

    pub(crate) fn project_selected_membership_for_assignment_v1(
        &mut self,
    ) -> Result<RemoteMcapSelectedMembershipV1, RemoteProtobufInitializationErrorV1> {
        if self.membership_projected {
            protobuf_fatal_control_plane("semantic-config membership was projected twice");
        }
        let membership = self
            .continuation
            .project_selected_membership_for_assignment_v1()
            .map_err(map_ros_recognition_error)?;
        self.membership_projected = true;
        Ok(membership)
    }

    pub(crate) fn take_recognition_v1(
        &mut self,
    ) -> Result<
        BoundedRemoteRecognitionIterV1<'_, 'definitions, 'input, 'source, 'wire>,
        RemoteProtobufInitializationErrorV1,
    > {
        #[cfg(target_arch = "wasm32")]
        verify_artifact_stage_identity(rerun_remote_protobuf_recognition_stage_v1(), 0x2705);
        let protobuf = &self.protobuf;
        let ros2 = self
            .continuation
            .take_bound_recognition_v1()
            .map_err(map_ros_recognition_error)?;
        Ok(BoundedRemoteRecognitionIterV1 {
            ros2,
            protobuf,
            executable_configs: &self.executable_configs,
        })
    }

    pub(crate) fn ensure_current_for_assignment_v1(
        &self,
    ) -> Result<(), RemoteProtobufInitializationErrorV1> {
        self.continuation
            .ensure_current_for_assignment_v1()
            .map_err(map_ros_recognition_error)
    }

    pub(crate) fn combined_retained_bytes_for_assignment_v1(
        &self,
    ) -> Result<u64, RemoteProtobufInitializationErrorV1> {
        let ros2 = self
            .continuation
            .retained_bytes_for_assignment_v1()
            .map_err(map_ros_recognition_error)?;
        ros2.checked_add(self._reservation.retained_bytes).ok_or(
            RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                RemoteProtobufResourceLimitV1::Arithmetic,
            ),
        )
    }

    pub(crate) fn policy_descriptor_for_assignment_v1(
        &self,
    ) -> Result<
        FrozenRemoteDecoderPolicyDescriptorViewV1<'_, 'wire>,
        RemoteProtobufInitializationErrorV1,
    > {
        self.continuation
            .policy_descriptor_for_assignment_v1()
            .map_err(map_ros_recognition_error)
    }
}

pub(crate) struct BoundedRemoteRecognitionIterV1<'borrow, 'definitions, 'input, 'source, 'wire> {
    ros2: RemoteRos2RecognitionIterV1<'borrow, 'definitions, 'input, 'source, 'wire>,
    protobuf: &'borrow BoundedProtobufDescriptorGraphV1<'definitions>,
    executable_configs: &'borrow FixedProtobufArenaV1<(u16, FrozenRemoteExecutableConfigV1)>,
}

pub(crate) struct BoundedRemoteChannelRecognitionV1<
    'item,
    'borrow,
    'definitions,
    'input,
    'source,
    'wire,
> {
    ros2: RemoteRos2ChannelRecognitionV1<'item, 'borrow, 'definitions, 'input, 'source, 'wire>,
    protobuf: &'item BoundedProtobufDescriptorGraphV1<'definitions>,
    executable_configs: &'item [(u16, FrozenRemoteExecutableConfigV1)],
}

impl BoundedRemoteChannelRecognitionV1<'_, '_, '_, '_, '_, '_> {
    pub(crate) fn channel_id(&self) -> Result<u16, RemoteProtobufInitializationErrorV1> {
        self.ros2.channel_id().map_err(map_ros_recognition_error)
    }

    pub(crate) fn recognized_by_ros2_reflection(
        &self,
    ) -> Result<bool, RemoteProtobufInitializationErrorV1> {
        self.ros2
            .recognized_by_reflection()
            .map_err(map_ros_recognition_error)
    }

    pub(crate) fn recognized_by_builtin_semantic(
        &self,
    ) -> Result<bool, RemoteProtobufInitializationErrorV1> {
        self.ros2
            .recognized_by_builtin_semantic()
            .map_err(map_ros_recognition_error)
    }

    pub(crate) fn recognized_by_protobuf(
        &self,
    ) -> Result<bool, RemoteProtobufInitializationErrorV1> {
        Ok(self
            .ros2
            .protobuf_schema_id()
            .map_err(map_ros_recognition_error)?
            .is_some_and(|schema_id| self.protobuf.has_schema_id(schema_id)))
    }

    pub(crate) fn resolve_bound_owner_v1(
        &self,
        recognized: [bool; 3],
    ) -> Result<Option<FrozenRemoteDecoderSlotV1>, RemoteProtobufInitializationErrorV1> {
        self.ros2
            .resolve_bound_owner_v1(recognized)
            .map_err(map_ros_recognition_error)
    }

    pub(crate) fn frozen_executable_config_v1(
        &self,
        owner: crate::remote_decoder_assignment::RemoteDecoderOwnerV1,
    ) -> Result<FrozenRemoteExecutableConfigV1, RemoteProtobufInitializationErrorV1> {
        let schema_id = match owner {
            crate::remote_decoder_assignment::RemoteDecoderOwnerV1::Ros2Reflection => self
                .ros2
                .ros2_schema_id_v1()
                .map_err(map_ros_recognition_error)?
                .ok_or(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema)?,
            crate::remote_decoder_assignment::RemoteDecoderOwnerV1::Protobuf => self
                .ros2
                .protobuf_schema_id()
                .map_err(map_ros_recognition_error)?
                .filter(|schema_id| self.protobuf.has_schema_id(*schema_id))
                .ok_or(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema)?,
            crate::remote_decoder_assignment::RemoteDecoderOwnerV1::Raw => 0,
        };
        let kind = match owner {
            crate::remote_decoder_assignment::RemoteDecoderOwnerV1::Ros2Reflection => 1,
            crate::remote_decoder_assignment::RemoteDecoderOwnerV1::Protobuf => 2,
            crate::remote_decoder_assignment::RemoteDecoderOwnerV1::Raw => 3,
        };
        let policy_versions = self
            .ros2
            .policy_versions_v1()
            .map_err(map_ros_recognition_error)?;
        let mut hasher = Sha256::new();
        hasher.update(b"rerun.remote-mcap.executable-config.v1\0");
        hasher.update(policy_versions.0.to_le_bytes());
        hasher.update(policy_versions.1.to_le_bytes());
        hasher.update(policy_versions.2.to_le_bytes());
        hasher.update([kind]);
        match owner {
            crate::remote_decoder_assignment::RemoteDecoderOwnerV1::Ros2Reflection => {
                let digest = self
                    .ros2
                    .ros2_executable_config_digest_v1()
                    .map_err(map_ros_recognition_error)?
                    .ok_or(RemoteProtobufInitializationErrorV1::InvalidRemoteSchema)?;
                hasher.update(digest);
            }
            crate::remote_decoder_assignment::RemoteDecoderOwnerV1::Protobuf => {
                return config_for_schema_id_v1(schema_id, self.executable_configs);
            }
            crate::remote_decoder_assignment::RemoteDecoderOwnerV1::Raw => {}
        }
        let full_digest = hasher.finalize();
        let mut digest = [0; 16];
        digest.copy_from_slice(&full_digest[..16]);
        Ok(FrozenRemoteExecutableConfigV1 {
            kind,
            schema_handle: schema_id,
            canonical_digest: digest,
            // All three currently admitted remote-specific parsers finalize exactly one temporal
            // Chunk. The external-origin descriptor is a fixed-width V1 encoding.
            max_roots_per_partition: 1,
            max_external_origin_bytes_per_partition: 128,
        })
    }
}

impl<'borrow, 'definitions, 'input, 'source, 'wire>
    BoundedRemoteRecognitionIterV1<'borrow, 'definitions, 'input, 'source, 'wire>
{
    pub(crate) fn next_channel<'item>(
        &'item mut self,
    ) -> Result<
        Option<
            BoundedRemoteChannelRecognitionV1<'item, 'borrow, 'definitions, 'input, 'source, 'wire>,
        >,
        RemoteProtobufInitializationErrorV1,
    > {
        let protobuf = self.protobuf;
        let executable_configs = self.executable_configs.as_slice();
        let Some(ros2) = self
            .ros2
            .next_channel()
            .map_err(map_ros_recognition_error)?
        else {
            return Ok(None);
        };
        Ok(Some(BoundedRemoteChannelRecognitionV1 {
            ros2,
            protobuf,
            executable_configs,
        }))
    }

    pub(crate) fn next_matching_channel<'item>(
        &'item mut self,
        matches: impl FnMut(u16) -> bool,
    ) -> Result<
        Option<
            BoundedRemoteChannelRecognitionV1<'item, 'borrow, 'definitions, 'input, 'source, 'wire>,
        >,
        RemoteProtobufInitializationErrorV1,
    > {
        let protobuf = self.protobuf;
        let executable_configs = self.executable_configs.as_slice();
        let Some(ros2) = self
            .ros2
            .next_matching_channel(matches)
            .map_err(map_ros_recognition_error)?
        else {
            return Ok(None);
        };
        Ok(Some(BoundedRemoteChannelRecognitionV1 {
            ros2,
            protobuf,
            executable_configs,
        }))
    }
}

pub(crate) fn initialize_remote_protobuf_v1<'definitions, 'input, 'source, 'wire>(
    prepared: PreparedRemoteProtobufCensusV1<'definitions, 'input, 'source, 'wire>,
) -> Result<
    BoundedRemoteDecoderInitializersV1<'definitions, 'input, 'source, 'wire>,
    RemoteProtobufInitializationErrorV1,
> {
    initialize_remote_protobuf_with_gate_v1(prepared, &SystemRemoteProtobufAllocationGateV1)
}

fn initialize_remote_protobuf_with_gate_v1<'definitions, 'input, 'source, 'wire>(
    mut prepared: PreparedRemoteProtobufCensusV1<'definitions, 'input, 'source, 'wire>,
    gate: &impl RemoteProtobufAllocationGateV1,
) -> Result<
    BoundedRemoteDecoderInitializersV1<'definitions, 'input, 'source, 'wire>,
    RemoteProtobufInitializationErrorV1,
> {
    #[cfg(target_arch = "wasm32")]
    verify_artifact_stage_identity(rerun_remote_protobuf_arena_stage_v1(), 0x2704);
    prepared
        .continuation
        .ensure_profile_current_v1(prepared.viewer_scope, prepared.profile_scope)
        .map_err(map_ros_error)?;
    let mut graph = allocate_graph_v1(
        prepared.census,
        &prepared.projection,
        gate,
        &mut prepared.materialization_steps,
    )?;
    walk_materialization_v1(
        &prepared.projection,
        &prepared.reservation.state.as_ref().map_or_else(
            || protobuf_fatal_invariant("protobuf reservation disappeared before materialization"),
            |state| state.limits,
        ),
        &mut prepared.materialization_steps,
        |record| {
            push_materialization_record_v1(&mut graph, record);
            Ok(())
        },
    )?;
    prepared.materialization_steps.ensure_exhausted();
    graph.verify_lengths(prepared.census, &prepared.projection)?;
    prepared
        .continuation
        .ensure_profile_current_v1(prepared.viewer_scope, prepared.profile_scope)
        .map_err(map_ros_error)?;
    let reservation = prepared.reservation.complete();
    let mut executable_configs = FixedProtobufArenaV1::try_new(
        graph.schemas.as_slice().len(),
        REMOTE_PROTOBUF_GRAPH_ARENA_COUNT_V1,
        gate,
    )?;
    let policy_versions = prepared
        .continuation
        .policy_descriptor_for_assignment_v1()
        .map_err(map_ros_error)?
        .canonical_versions_for_manifest_v1();
    for schema in graph.schemas.as_slice() {
        let short = graph.canonical_digest_for_schema_v1(schema.schema_id, policy_versions)?;
        executable_configs.push((
            schema.schema_id,
            FrozenRemoteExecutableConfigV1 {
                kind: 2,
                schema_handle: schema.schema_id,
                canonical_digest: short,
                max_roots_per_partition: 1,
                max_external_origin_bytes_per_partition: 128,
            },
        ));
    }
    gate.before_result()?;
    Ok(BoundedRemoteDecoderInitializersV1 {
        continuation: prepared.continuation,
        protobuf: graph,
        executable_configs,
        membership_projected: false,
        _reservation: reservation,
    })
}

/// Executes the exact production-disarmed protobuf path for the release-Wasm artifact verifier.
#[cfg(target_arch = "wasm32")]
pub(crate) fn run_remote_protobuf_artifact_probe_v1<'definitions, 'input, 'source, 'wire>(
    transition: crate::remote_ros2_reflection::RemoteRos2ToProtobufTransitionV1<
        'definitions,
        'input,
        'source,
        'wire,
    >,
    viewer_scope: &RemoteViewerScopeState,
    profile_scope: &RemoteProtobufProfileScopeV1,
) -> u32 {
    let limits = UnfrozenRemoteProtobufLimitsV1 {
        profile_version: REMOTE_PROTOBUF_PROFILE_VERSION_V1,
        max_schemas: 1,
        max_descriptor_bytes: 256,
        max_files: 1,
        max_messages: 2,
        max_fields: 4,
        max_enums: 1,
        max_enum_values: 2,
        max_oneofs: 1,
        max_dependencies: 1,
        max_options: 1,
        max_string_bytes: 256,
        max_default_bytes: 64,
        max_single_string_bytes: 64,
        max_descriptor_depth: 4,
        max_import_depth: 4,
        max_census_steps: 32_768,
        max_materialization_steps: 32_768,
        max_resolution_steps: 32_768,
        max_retained_bytes: u64::MAX,
        max_working_bytes: u64::MAX,
    };
    let budget = RemoteProtobufInitializationBudgetV1::new_disarmed_v1(
        viewer_scope,
        profile_scope,
        limits,
        1,
        u64::MAX,
        1,
        u64::MAX,
    );
    let prepared = match prepare_remote_protobuf_census_v1(transition, &budget) {
        Ok(prepared) => prepared,
        Err(_error) => return 21,
    };
    if prepared.census.schemas != 1
        || prepared.census.files != 1
        || prepared.census.messages != 1
        || prepared.census.fields != 1
    {
        return 22;
    }
    let result = match initialize_remote_protobuf_v1(prepared) {
        Ok(result) => result,
        Err(_error) => return 23,
    };
    if result.protobuf_schema_count() != 1 {
        return 24;
    }
    let assignment_status =
        crate::remote_decoder_assignment::run_remote_assignment_artifact_probe_v1(result);
    if assignment_status != 0 {
        return 25 + assignment_status;
    }
    if *budget.state.usage.lock() != RemoteProtobufBudgetUsageV1::default() {
        return 29;
    }
    0
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use crate::remote_ros2_reflection::{
        RemoteDecoderPolicyWireV1, RemoteDefinitionsCapability, RemoteDefinitionsSourceState,
        RemoteRos2InitializationBudget, RemoteRos2ProfileScopeV1, RemoteRos2ToProtobufTransitionV1,
        begin_remote_ros2_admission_v1, freeze_remote_decoder_policy_v1,
        materialize_remote_ros2_definitions_v1, preflight_remote_decoder_topic_signatures_v1,
        prepare_remote_ros2_census_v1,
    };
    use crate::remote_summary::definitions::ValidatedSummaryDefinitions;
    use crate::remote_summary::validated_summary_definitions_for_test;
    use crate::testing::{
        AdversarialMcapFixture, AdversarialMcapFixtureBuilder, FixtureChannel, FixtureChunk,
        FixtureMessage, FixtureSchema, PartitionFixture,
    };

    use super::*;

    #[expect(
        clippy::disallowed_methods,
        reason = "host-only tests verify that scope mismatches use the internal fatal path"
    )]
    fn assert_fatal_control_plane(action: impl FnOnce()) {
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(action)).is_err(),
            "control-plane mismatch must use the internal fatal path"
        );
    }

    static_assertions::assert_not_impl_any!(
        PreparedRemoteProtobufCensusV1<'static, 'static, 'static, 'static>: Clone, Copy
    );
    static_assertions::assert_not_impl_any!(
        BoundedRemoteDecoderInitializersV1<'static, 'static, 'static, 'static>: Clone, Copy
    );
    static_assertions::assert_not_impl_any!(
        BoundedRemoteRecognitionIterV1<'static, 'static, 'static, 'static, 'static>: Clone, Copy
    );
    static_assertions::assert_not_impl_any!(
        BoundedRemoteChannelRecognitionV1<
            'static,
            'static,
            'static,
            'static,
            'static,
            'static
        >: Clone, Copy
    );

    fn push_varint(bytes: &mut Vec<u8>, mut value: u64) {
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                bytes.push(byte);
                break;
            }
            bytes.push(byte | 0x80);
        }
    }

    fn bytes_field(number: u32, value: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        push_varint(&mut bytes, (u64::from(number) << 3) | 2);
        push_varint(&mut bytes, value.len() as u64);
        bytes.extend_from_slice(value);
        bytes
    }

    fn string_field(number: u32, value: &str) -> Vec<u8> {
        bytes_field(number, value.as_bytes())
    }

    fn varint_field(number: u32, value: u64) -> Vec<u8> {
        let mut bytes = Vec::new();
        push_varint(&mut bytes, u64::from(number) << 3);
        push_varint(&mut bytes, value);
        bytes
    }

    fn concat(parts: impl IntoIterator<Item = Vec<u8>>) -> Vec<u8> {
        parts.into_iter().flatten().collect()
    }

    fn scalar_field(name: &str, number: u64, kind: u64) -> Vec<u8> {
        scalar_field_with_label(name, number, kind, 1)
    }

    fn scalar_field_with_label(name: &str, number: u64, kind: u64, label: u64) -> Vec<u8> {
        concat([
            string_field(1, name),
            varint_field(3, number),
            varint_field(4, label),
            varint_field(5, kind),
        ])
    }

    fn named_field(name: &str, number: u64, kind: u64, type_name: &str) -> Vec<u8> {
        named_field_with_label(name, number, kind, type_name, 1)
    }

    fn named_field_with_label(
        name: &str,
        number: u64,
        kind: u64,
        type_name: &str,
        label: u64,
    ) -> Vec<u8> {
        concat([
            string_field(1, name),
            varint_field(3, number),
            varint_field(4, label),
            varint_field(5, kind),
            string_field(6, type_name),
        ])
    }

    fn scalar_field_with_default(name: &str, number: u64, kind: u64, default: &str) -> Vec<u8> {
        concat([scalar_field(name, number, kind), string_field(7, default)])
    }

    fn oneof_field(name: &str, number: u64, kind: u64, oneof_index: u64) -> Vec<u8> {
        concat([
            scalar_field(name, number, kind),
            varint_field(9, oneof_index),
        ])
    }

    fn proto3_optional_field(name: &str, number: u64, kind: u64, oneof_index: u64) -> Vec<u8> {
        concat([
            scalar_field(name, number, kind),
            varint_field(9, oneof_index),
            varint_field(17, 1),
        ])
    }

    fn message(name: &str, fields: impl IntoIterator<Item = Vec<u8>>) -> Vec<u8> {
        let mut bytes = string_field(1, name);
        for field in fields {
            bytes.extend(bytes_field(2, &field));
        }
        bytes
    }

    fn enum_descriptor(name: &str, values: &[(&str, i32)]) -> Vec<u8> {
        let mut bytes = string_field(1, name);
        for (name, number) in values {
            let value = concat([
                string_field(1, name),
                varint_field(2, u64::from_ne_bytes(i64::from(*number).to_ne_bytes())),
            ]);
            bytes.extend(bytes_field(2, &value));
        }
        bytes
    }

    fn descriptor_set_with_file(file: &[u8]) -> Vec<u8> {
        bytes_field(1, file)
    }

    fn descriptor_set(
        package: &str,
        file_name: &str,
        messages: impl IntoIterator<Item = Vec<u8>>,
        syntax: &str,
    ) -> Vec<u8> {
        let mut file = concat([
            string_field(1, file_name),
            string_field(2, package),
            string_field(12, syntax),
        ]);
        for message in messages {
            file.extend(bytes_field(4, &message));
        }
        bytes_field(1, &file)
    }

    fn simple_descriptor() -> Vec<u8> {
        descriptor_set(
            "pkg",
            "simple.proto",
            [message("Message", [scalar_field("value", 1, 5)])],
            "proto3",
        )
    }

    fn fixture(data: Vec<u8>, schema_name: &str) -> AdversarialMcapFixture {
        fixture_with_encoding(data, schema_name, "protobuf", "protobuf")
    }

    fn fixture_with_encoding(
        data: Vec<u8>,
        schema_name: &str,
        schema_encoding: &str,
        message_encoding: &str,
    ) -> AdversarialMcapFixture {
        AdversarialMcapFixtureBuilder::new()
            .with_schemas([FixtureSchema::new(7, schema_name, schema_encoding).with_data(data)])
            .with_channels([
                FixtureChannel::schema_less(1, "/protobuf").with_schema(7, message_encoding)
            ])
            .with_chunks([FixtureChunk::single(FixtureMessage::new(1, 0, 1))])
            .with_partition_fixture(PartitionFixture::default())
            .build()
            .expect("protobuf fixture builds")
    }

    fn limits() -> UnfrozenRemoteProtobufLimitsV1 {
        UnfrozenRemoteProtobufLimitsV1 {
            profile_version: REMOTE_PROTOBUF_PROFILE_VERSION_V1,
            max_schemas: MAX_INLINE_PROTOBUF_SCHEMAS_V1 as u64,
            max_descriptor_bytes: 1_000_000,
            max_files: MAX_INLINE_PROTOBUF_FILES_V1 as u64,
            max_messages: MAX_INLINE_PROTOBUF_MESSAGES_V1 as u64,
            max_fields: MAX_INLINE_PROTOBUF_RESOLUTIONS_V1 as u64,
            max_enums: MAX_INLINE_PROTOBUF_ENUMS_V1 as u64,
            max_enum_values: 1_024,
            max_oneofs: 512,
            max_dependencies: MAX_INLINE_PROTOBUF_DEPENDENCIES_V1 as u64,
            max_options: 512,
            max_string_bytes: 1_000_000,
            max_default_bytes: 1_000_000,
            max_single_string_bytes: 64_000,
            max_descriptor_depth: 32,
            max_import_depth: 32,
            max_census_steps: 10_000_000,
            max_materialization_steps: 10_000_000,
            max_resolution_steps: 10_000_000,
            max_retained_bytes: 64_000_000,
            max_working_bytes: 64_000_000,
        }
    }

    fn transition<'definitions, 'input, 'source, 'wire>(
        definitions: &'definitions ValidatedSummaryDefinitions<'input>,
        source: &'source RemoteDefinitionsSourceState,
        wire: &'wire RemoteDecoderPolicyWireV1<'wire>,
        viewer: &RemoteViewerScopeState,
        ros_profile: &RemoteRos2ProfileScopeV1,
    ) -> RemoteRos2ToProtobufTransitionV1<'definitions, 'input, 'source, 'wire> {
        let physical = crate::remote_chunk_scan::PhysicalChunkDefinitionsCapabilityV1::new_unscanned_for_test_with_binding_v1(
            definitions,
            source.physical_source_binding_for_test_v1(),
        );
        transition_with_physical(&physical, source, wire, viewer, ros_profile)
    }

    fn transition_with_physical<'definitions, 'input, 'source, 'wire>(
        physical: &crate::remote_chunk_scan::PhysicalChunkDefinitionsCapabilityV1<
            'definitions,
            'input,
        >,
        source: &'source RemoteDefinitionsSourceState,
        wire: &'wire RemoteDecoderPolicyWireV1<'wire>,
        viewer: &RemoteViewerScopeState,
        ros_profile: &RemoteRos2ProfileScopeV1,
    ) -> RemoteRos2ToProtobufTransitionV1<'definitions, 'input, 'source, 'wire> {
        let ros_budget = RemoteRos2InitializationBudget::new_for_protobuf_test_v1(
            source,
            viewer,
            ros_profile,
            wire,
        );
        transition_with_physical_and_ros_budget(physical, source, wire, &ros_budget)
    }

    fn transition_with_physical_and_ros_budget<'definitions, 'input, 'source, 'wire>(
        physical: &crate::remote_chunk_scan::PhysicalChunkDefinitionsCapabilityV1<
            'definitions,
            'input,
        >,
        source: &'source RemoteDefinitionsSourceState,
        wire: &'wire RemoteDecoderPolicyWireV1<'wire>,
        ros_budget: &RemoteRos2InitializationBudget<'source, 'wire>,
    ) -> RemoteRos2ToProtobufTransitionV1<'definitions, 'input, 'source, 'wire> {
        let policy = freeze_remote_decoder_policy_v1(wire).unwrap();
        let source = RemoteDefinitionsCapability::new_for_protobuf_test_v1(
            physical, source, &policy, ros_budget,
        )
        .unwrap();
        let owner = begin_remote_ros2_admission_v1(source, policy, ros_budget).unwrap();
        let evidence = preflight_remote_decoder_topic_signatures_v1(owner).unwrap();
        let prepared = prepare_remote_ros2_census_v1(evidence).unwrap();
        materialize_remote_ros2_definitions_v1(prepared)
            .unwrap()
            .into_protobuf_transition_v1()
            .unwrap()
    }

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
                crate::remote_chunk_scan::PhysicalChunkSourceBindingV1::new_unscanned_for_assignment_test_v1(),
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

        fn protobuf_budget(&self) -> RemoteProtobufInitializationBudgetV1<'_, '_> {
            self.protobuf_budget_with_limits(limits())
        }

        fn protobuf_budget_with_limits(
            &self,
            limits: UnfrozenRemoteProtobufLimitsV1,
        ) -> RemoteProtobufInitializationBudgetV1<'_, '_> {
            RemoteProtobufInitializationBudgetV1::new_disarmed_v1(
                &self.viewer,
                &self.protobuf_profile,
                limits,
                8,
                256_000_000,
                8,
                256_000_000,
            )
        }
    }

    fn prepared<'definitions, 'input, 'source>(
        definitions: &'definitions ValidatedSummaryDefinitions<'input>,
        context: &'source StableContext,
        budget: &RemoteProtobufInitializationBudgetV1<'_, '_>,
    ) -> Result<
        PreparedRemoteProtobufCensusV1<'definitions, 'input, 'source, 'source>,
        RemoteProtobufInitializationErrorV1,
    > {
        let transition = transition(
            definitions,
            &context.source,
            &context.wire,
            &context.viewer,
            &context.ros_profile,
        );
        prepare_remote_protobuf_census_v1(transition, budget)
    }

    fn prepared_with_physical<'definitions, 'input, 'source>(
        physical: &crate::remote_chunk_scan::PhysicalChunkDefinitionsCapabilityV1<
            'definitions,
            'input,
        >,
        context: &'source StableContext,
        budget: &RemoteProtobufInitializationBudgetV1<'_, '_>,
    ) -> Result<
        PreparedRemoteProtobufCensusV1<'definitions, 'input, 'source, 'source>,
        RemoteProtobufInitializationErrorV1,
    > {
        let transition = transition_with_physical(
            physical,
            &context.source,
            &context.wire,
            &context.viewer,
            &context.ros_profile,
        );
        prepare_remote_protobuf_census_v1(transition, budget)
    }

    fn prepared_with_physical_and_ros_budget<'definitions, 'input, 'source>(
        physical: &crate::remote_chunk_scan::PhysicalChunkDefinitionsCapabilityV1<
            'definitions,
            'input,
        >,
        context: &'source StableContext,
        ros_budget: &RemoteRos2InitializationBudget<'source, 'source>,
        budget: &RemoteProtobufInitializationBudgetV1<'_, '_>,
    ) -> Result<
        PreparedRemoteProtobufCensusV1<'definitions, 'input, 'source, 'source>,
        RemoteProtobufInitializationErrorV1,
    > {
        let transition = transition_with_physical_and_ros_budget(
            physical,
            &context.source,
            &context.wire,
            ros_budget,
        );
        prepare_remote_protobuf_census_v1(transition, budget)
    }

    fn assert_prepare_error(
        data: Vec<u8>,
        schema_name: &str,
        expected: RemoteProtobufInitializationErrorV1,
    ) {
        let fixture = fixture(data, schema_name);
        let definitions = validated_summary_definitions_for_test(&fixture);
        let context = StableContext::new();
        let budget = context.protobuf_budget();
        let error = match prepared(&definitions, &context, &budget) {
            Ok(_prepared) => panic!("invalid or unsupported descriptor unexpectedly passed"),
            Err(error) => error,
        };
        assert_eq!(error, expected);
        assert_eq!(
            *budget.state.usage.lock(),
            RemoteProtobufBudgetUsageV1::default()
        );
    }

    #[test]
    fn admitted_descriptor_matches_local_pool_and_recognition() {
        let data = simple_descriptor();
        assert!(prost_reflect::DescriptorPool::decode(data.as_slice()).is_ok());
        let fixture = fixture(data, "pkg.Message");
        let definitions = validated_summary_definitions_for_test(&fixture);
        let context = StableContext::new();
        let budget = context.protobuf_budget();
        let prepared = prepared(&definitions, &context, &budget).unwrap();
        assert_eq!(prepared.census.schemas, 1);
        assert_eq!(prepared.census.files, 1);
        assert_eq!(prepared.census.messages, 1);
        assert_eq!(prepared.census.fields, 1);
        let mut result = initialize_remote_protobuf_v1(prepared).unwrap();
        assert_eq!(result.protobuf_schema_count(), 1);
        let mut recognition = result.take_recognition_v1().unwrap();
        let channel = recognition.next_channel().unwrap().unwrap();
        assert_eq!(channel.channel_id().unwrap(), 1);
        assert!(!channel.recognized_by_ros2_reflection().unwrap());
        assert!(channel.recognized_by_protobuf().unwrap());
        assert!(recognition.next_channel().unwrap().is_none());
        let _recognition_is_finished = recognition;
        drop(result);
        assert_eq!(
            *budget.state.usage.lock(),
            RemoteProtobufBudgetUsageV1::default()
        );
    }

    #[test]
    fn malformed_and_subset_failures_precede_reservation() {
        let cases = [
            (
                vec![0x0a, 0x80],
                RemoteProtobufInitializationErrorV1::InvalidRemoteSchema,
            ),
            (
                concat([simple_descriptor(), varint_field(2, 1)]),
                RemoteProtobufInitializationErrorV1::UnsupportedForRemote(
                    UnsupportedRemoteProtobufV1::UnknownDescriptorField,
                ),
            ),
        ];
        for (data, expected) in cases {
            let fixture = fixture(data, "pkg.Message");
            let definitions = validated_summary_definitions_for_test(&fixture);
            let context = StableContext::new();
            let budget = context.protobuf_budget();
            let error = match prepared(&definitions, &context, &budget) {
                Ok(_prepared) => panic!("invalid descriptor unexpectedly passed census"),
                Err(error) => error,
            };
            assert_eq!(error, expected);
            assert_eq!(
                *budget.state.usage.lock(),
                RemoteProtobufBudgetUsageV1::default()
            );
        }
    }

    struct FailAllocationGate {
        fail_at: usize,
        allocations: Cell<usize>,
    }

    impl RemoteProtobufAllocationGateV1 for FailAllocationGate {
        fn before_allocation(
            &self,
            arena_index: usize,
            _layout: Layout,
        ) -> Result<(), RemoteProtobufInitializationErrorV1> {
            assert_eq!(arena_index, self.allocations.get());
            if arena_index == self.fail_at {
                return Err(RemoteProtobufInitializationErrorV1::FallibleAllocationFailed);
            }
            Ok(())
        }

        fn after_allocation(&self, arena_index: usize) {
            self.allocations.set(arena_index + 1);
        }

        fn before_result(&self) -> Result<(), RemoteProtobufInitializationErrorV1> {
            if self.fail_at == REMOTE_PROTOBUF_ARENA_COUNT_V1 {
                Err(RemoteProtobufInitializationErrorV1::FallibleAllocationFailed)
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn every_allocation_and_result_failure_rolls_back_exactly() {
        let fixture = fixture(simple_descriptor(), "pkg.Message");
        let definitions = validated_summary_definitions_for_test(&fixture);
        for fail_at in 0..=REMOTE_PROTOBUF_ARENA_COUNT_V1 {
            let context = StableContext::new();
            let budget = context.protobuf_budget();
            let prepared = prepared(&definitions, &context, &budget).unwrap();
            let error = match initialize_remote_protobuf_with_gate_v1(
                prepared,
                &FailAllocationGate {
                    fail_at,
                    allocations: Cell::new(0),
                },
            ) {
                Ok(_result) => panic!("injected allocation failure unexpectedly succeeded"),
                Err(error) => error,
            };
            assert_eq!(
                error,
                RemoteProtobufInitializationErrorV1::FallibleAllocationFailed
            );
            assert_eq!(
                *budget.state.usage.lock(),
                RemoteProtobufBudgetUsageV1::default()
            );
        }
    }

    #[test]
    fn source_invalidation_is_stale_and_releases_capacity() {
        let fixture = fixture(simple_descriptor(), "pkg.Message");
        let definitions = validated_summary_definitions_for_test(&fixture);
        let context = StableContext::new();
        let budget = context.protobuf_budget();
        let prepared = prepared(&definitions, &context, &budget).unwrap();
        context.source.invalidate_for_protobuf_test_v1();
        let error = match initialize_remote_protobuf_v1(prepared) {
            Ok(_result) => panic!("stale source unexpectedly initialized"),
            Err(error) => error,
        };
        assert_eq!(error, RemoteProtobufInitializationErrorV1::StaleSource);
        assert_eq!(
            *budget.state.usage.lock(),
            RemoteProtobufBudgetUsageV1::default()
        );
    }

    #[test]
    fn recognition_rows_revalidate_the_bound_source_on_every_accessor() {
        let fixture = fixture(simple_descriptor(), "pkg.Message");
        let definitions = validated_summary_definitions_for_test(&fixture);
        let context = StableContext::new();
        let budget = context.protobuf_budget();
        let mut result =
            initialize_remote_protobuf_v1(prepared(&definitions, &context, &budget).unwrap())
                .unwrap();
        {
            let mut recognition = result.take_recognition_v1().unwrap();
            let channel = recognition.next_channel().unwrap().unwrap();
            assert_eq!(channel.channel_id().unwrap(), 1);
            context.source.invalidate_for_protobuf_test_v1();
            assert_eq!(
                channel.channel_id(),
                Err(RemoteProtobufInitializationErrorV1::StaleSource)
            );
            assert_eq!(
                channel.recognized_by_ros2_reflection(),
                Err(RemoteProtobufInitializationErrorV1::StaleSource)
            );
            assert_eq!(
                channel.recognized_by_protobuf(),
                Err(RemoteProtobufInitializationErrorV1::StaleSource)
            );
        }
        drop(result);
        assert_eq!(
            *budget.state.usage.lock(),
            RemoteProtobufBudgetUsageV1::default()
        );
    }

    #[test]
    fn projection_storage_stays_below_locked_frame_ceiling() {
        assert!(
            std::mem::size_of::<DescriptorProjectionV1<'static>>()
                <= LOCKED_REMOTE_PROTOBUF_FRAME_SPILL_CEILING_V1 as usize
        );
    }

    #[test]
    fn local_descriptor_initializer_is_not_called_by_remote_production_module() {
        let source = include_str!("remote_protobuf_descriptor.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();
        assert!(!production.contains("DescriptorPool::decode"));
        assert!(!production.contains("MessageSchema::parse"));
        assert!(
            prost_reflect::DescriptorPool::decode(
                crate::remote_summary::definitions::REMOTE_PROTOBUF_ARTIFACT_DESCRIPTOR_V1,
            )
            .is_ok()
        );
    }

    #[test]
    fn named_type_resolution_is_bounded_and_matches_local_pool() {
        let child = message("Child", [scalar_field("value", 1, 9)]);
        let parent = message("Parent", [named_field("child", 1, 11, ".pkg.Child")]);
        let data = descriptor_set("pkg", "named.proto", [child, parent], "proto3");
        assert!(prost_reflect::DescriptorPool::decode(data.as_slice()).is_ok());
        let fixture = fixture(data, "pkg.Parent");
        let definitions = validated_summary_definitions_for_test(&fixture);
        let context = StableContext::new();
        let budget = context.protobuf_budget();
        let prepared = prepared(&definitions, &context, &budget).unwrap();
        assert_eq!(prepared.projection.resolutions.len, 1);
        let result = initialize_remote_protobuf_v1(prepared).unwrap();
        assert_eq!(result.protobuf.fields.as_slice().len(), 2);
        assert_eq!(
            result
                .protobuf
                .fields
                .as_slice()
                .iter()
                .filter(|field| field.resolved_symbol.is_some())
                .count(),
            1
        );
    }

    #[test]
    fn relative_type_names_are_conservatively_rejected_without_shadow_fallback() {
        let nested = message("Foo", []);
        let outer = concat([
            string_field(1, "Outer"),
            bytes_field(2, &named_field("value", 1, 11, "Foo")),
            bytes_field(3, &nested),
        ]);
        let shadowed = descriptor_set(
            "pkg",
            "shadowed.proto",
            [message("Foo", []), outer],
            "proto3",
        );
        assert!(prost_reflect::DescriptorPool::decode(shadowed.as_slice()).is_ok());
        assert_prepare_error(
            shadowed,
            "pkg.Outer",
            RemoteProtobufInitializationErrorV1::UnsupportedForRemote(
                UnsupportedRemoteProtobufV1::RelativeTypeName,
            ),
        );

        let multi_package = descriptor_set(
            "multi.component.pkg",
            "relative.proto",
            [
                message("Target", []),
                message("Owner", [named_field("value", 1, 11, "Target")]),
            ],
            "proto3",
        );
        assert!(prost_reflect::DescriptorPool::decode(multi_package.as_slice()).is_ok());
        assert_prepare_error(
            multi_package,
            "multi.component.pkg.Owner",
            RemoteProtobufInitializationErrorV1::UnsupportedForRemote(
                UnsupportedRemoteProtobufV1::RelativeTypeName,
            ),
        );

        let absolute_multi_package = descriptor_set(
            "multi.component.pkg",
            "absolute.proto",
            [
                message("Target", []),
                message(
                    "Owner",
                    [named_field("value", 1, 11, ".multi.component.pkg.Target")],
                ),
            ],
            "proto3",
        );
        assert!(prost_reflect::DescriptorPool::decode(absolute_multi_package.as_slice()).is_ok());
        let fixture = fixture(absolute_multi_package, "multi.component.pkg.Owner");
        let definitions = validated_summary_definitions_for_test(&fixture);
        let context = StableContext::new();
        let budget = context.protobuf_budget();
        let prepared = prepared(&definitions, &context, &budget).unwrap();
        assert_eq!(prepared.projection.resolutions.len, 1);
        let result = initialize_remote_protobuf_v1(prepared).unwrap();
        assert_eq!(
            result
                .protobuf
                .fields
                .as_slice()
                .iter()
                .filter(|field| field.resolved_symbol.is_some())
                .count(),
            1
        );
    }

    #[test]
    fn protobuf_scalar_defaults_and_resolved_type_kind_match_local_pool() {
        let default_scalar = concat([string_field(1, "value"), varint_field(3, 1)]);
        let resolved_without_explicit_kind = concat([
            string_field(1, "child"),
            varint_field(3, 2),
            string_field(6, ".pkg.Child"),
        ]);
        let value_without_number = string_field(1, "ZERO");
        let enumeration = concat([
            string_field(1, "State"),
            bytes_field(2, &value_without_number),
        ]);
        let mut file = concat([
            string_field(1, "defaults.proto"),
            string_field(2, "pkg"),
            string_field(12, "proto3"),
            bytes_field(4, &message("Child", [])),
            bytes_field(
                4,
                &message("Owner", [default_scalar, resolved_without_explicit_kind]),
            ),
            bytes_field(5, &enumeration),
        ]);
        file.rotate_left(0);
        let data = descriptor_set_with_file(&file);
        assert!(prost_reflect::DescriptorPool::decode(data.as_slice()).is_ok());
        let fixture = fixture(data, "pkg.Owner");
        let definitions = validated_summary_definitions_for_test(&fixture);
        let context = StableContext::new();
        let budget = context.protobuf_budget();
        let result =
            initialize_remote_protobuf_v1(prepared(&definitions, &context, &budget).unwrap())
                .unwrap();
        let owner_fields = result
            .protobuf
            .fields
            .as_slice()
            .iter()
            .filter(|field| field.message_index == 1)
            .collect::<Vec<_>>();
        assert_eq!(owner_fields.len(), 2);
        assert_eq!(owner_fields[0].label, FieldLabelV1::Optional);
        assert_eq!(owner_fields[0].kind, FieldKindV1::Double);
        assert_eq!(owner_fields[1].label, FieldLabelV1::Optional);
        assert_eq!(owner_fields[1].kind, FieldKindV1::Message);
        assert_eq!(result.protobuf.enum_values.as_slice()[0].number, 0);
    }

    #[test]
    fn explicit_type_kind_must_match_the_resolved_symbol() {
        let data = descriptor_set(
            "pkg",
            "mismatch.proto",
            [
                message("Child", []),
                message("Owner", [named_field("child", 1, 14, ".pkg.Child")]),
            ],
            "proto3",
        );
        let local = prost_reflect::DescriptorPool::decode(data.as_slice()).unwrap();
        let owner = local.get_message_by_name("pkg.Owner").unwrap();
        assert!(matches!(
            owner.get_field_by_name("child").unwrap().kind(),
            prost_reflect::Kind::Message(_)
        ));
        assert_prepare_error(
            data,
            "pkg.Owner",
            RemoteProtobufInitializationErrorV1::InvalidRemoteSchema,
        );
    }

    #[test]
    fn proto2_nested_enum_oneof_and_map_subset_matches_local_pool() {
        let proto2 = descriptor_set(
            "pkg",
            "defaults.proto",
            [message(
                "Defaults",
                [scalar_field_with_default("value", 1, 5, "42")],
            )],
            "proto2",
        );
        assert!(prost_reflect::DescriptorPool::decode(proto2.as_slice()).is_ok());

        let oneof = concat([
            string_field(1, "Choice"),
            bytes_field(2, &oneof_field("number", 1, 5, 0)),
            bytes_field(2, &oneof_field("text", 2, 9, 0)),
            bytes_field(8, &string_field(1, "choice")),
        ]);
        let enum_value = enum_descriptor("Color", &[("UNKNOWN", 0), ("RED", 1)]);
        let map_entry_options = varint_field(7, 1);
        let map_entry = concat([
            string_field(1, "LabelsEntry"),
            bytes_field(2, &scalar_field("key", 1, 9)),
            bytes_field(2, &scalar_field("value", 2, 5)),
            bytes_field(7, &map_entry_options),
        ]);
        let container = concat([
            string_field(1, "Container"),
            bytes_field(
                2,
                &named_field_with_label("labels", 1, 11, ".pkg.Container.LabelsEntry", 3),
            ),
            bytes_field(3, &map_entry),
        ]);
        let mut file = concat([
            string_field(1, "complex.proto"),
            string_field(2, "pkg"),
            string_field(12, "proto3"),
            bytes_field(4, &oneof),
            bytes_field(4, &container),
            bytes_field(5, &enum_value),
        ]);
        // Keep canonical field order irrelevant to the strict wire walker.
        file.rotate_left(0);
        let complex = descriptor_set_with_file(&file);
        assert!(prost_reflect::DescriptorPool::decode(complex.as_slice()).is_ok());

        for (data, schema_name, expected) in [
            (proto2, "pkg.Defaults", (1_u64, 0_u64, 0_u64)),
            (complex, "pkg.Container", (3_u64, 1_u64, 1_u64)),
        ] {
            let fixture = fixture(data, schema_name);
            let definitions = validated_summary_definitions_for_test(&fixture);
            let context = StableContext::new();
            let budget = context.protobuf_budget();
            let prepared = prepared(&definitions, &context, &budget).unwrap();
            assert_eq!(prepared.census.messages, expected.0);
            assert_eq!(prepared.census.enums, expected.1);
            assert_eq!(prepared.census.oneofs, expected.2);
            let result = initialize_remote_protobuf_v1(prepared).unwrap();
            assert_eq!(result.protobuf_schema_count(), 1);
        }
    }

    #[test]
    fn oneof_labels_and_synthetic_order_are_frozen_conservatively() {
        for label in [2, 3] {
            let invalid = concat([
                string_field(1, "Choice"),
                bytes_field(
                    2,
                    &concat([
                        scalar_field_with_label("value", 1, 5, label),
                        varint_field(9, 0),
                    ]),
                ),
                bytes_field(8, &string_field(1, "choice")),
            ]);
            let data = descriptor_set("pkg", "label.proto", [invalid], "proto3");
            assert!(prost_reflect::DescriptorPool::decode(data.as_slice()).is_ok());
            assert_prepare_error(
                data,
                "pkg.Choice",
                RemoteProtobufInitializationErrorV1::InvalidRemoteSchema,
            );
        }

        let synthetic_before_real = concat([
            string_field(1, "Choice"),
            bytes_field(2, &proto3_optional_field("optional", 1, 5, 0)),
            bytes_field(2, &oneof_field("real", 2, 5, 1)),
            bytes_field(8, &string_field(1, "_optional")),
            bytes_field(8, &string_field(1, "choice")),
        ]);
        let invalid = descriptor_set(
            "pkg",
            "synthetic_before.proto",
            [synthetic_before_real],
            "proto3",
        );
        assert!(prost_reflect::DescriptorPool::decode(invalid.as_slice()).is_ok());
        assert_prepare_error(
            invalid,
            "pkg.Choice",
            RemoteProtobufInitializationErrorV1::InvalidRemoteSchema,
        );

        let real_before_synthetic = concat([
            string_field(1, "Choice"),
            bytes_field(2, &oneof_field("real", 1, 5, 0)),
            bytes_field(2, &proto3_optional_field("optional", 2, 5, 1)),
            bytes_field(8, &string_field(1, "choice")),
            bytes_field(8, &string_field(1, "_optional")),
        ]);
        let valid = descriptor_set(
            "pkg",
            "synthetic_after.proto",
            [real_before_synthetic],
            "proto3",
        );
        assert!(prost_reflect::DescriptorPool::decode(valid.as_slice()).is_ok());
        let fixture = fixture(valid, "pkg.Choice");
        let definitions = validated_summary_definitions_for_test(&fixture);
        let context = StableContext::new();
        let budget = context.protobuf_budget();
        assert!(prepared(&definitions, &context, &budget).is_ok());
    }

    #[test]
    fn map_entry_has_one_repeated_reference_from_its_direct_parent() {
        fn map_entry() -> Vec<u8> {
            concat([
                string_field(1, "LabelsEntry"),
                bytes_field(2, &scalar_field("key", 1, 9)),
                bytes_field(2, &scalar_field("value", 2, 5)),
                bytes_field(7, &varint_field(7, 1)),
            ])
        }

        let owner_without_reference =
            concat([string_field(1, "Owner"), bytes_field(3, &map_entry())]);
        let wrong_parent = message(
            "Other",
            [named_field_with_label(
                "labels",
                1,
                11,
                ".pkg.Owner.LabelsEntry",
                3,
            )],
        );
        let invalid = descriptor_set(
            "pkg",
            "wrong_parent.proto",
            [owner_without_reference, wrong_parent],
            "proto3",
        );
        assert_prepare_error(
            invalid,
            "pkg.Other",
            RemoteProtobufInitializationErrorV1::InvalidRemoteSchema,
        );

        let duplicate_references = concat([
            string_field(1, "Owner"),
            bytes_field(
                2,
                &named_field_with_label("labels", 1, 11, ".pkg.Owner.LabelsEntry", 3),
            ),
            bytes_field(
                2,
                &named_field_with_label("other_labels", 2, 11, ".pkg.Owner.LabelsEntry", 3),
            ),
            bytes_field(3, &map_entry()),
        ]);
        let invalid = descriptor_set(
            "pkg",
            "multiple_map_refs.proto",
            [duplicate_references],
            "proto3",
        );
        assert_prepare_error(
            invalid,
            "pkg.Owner",
            RemoteProtobufInitializationErrorV1::InvalidRemoteSchema,
        );
    }

    #[test]
    fn duplicate_missing_and_recursive_graphs_fail_before_reservation() {
        let duplicate_fields = descriptor_set(
            "pkg",
            "duplicate.proto",
            [message(
                "Message",
                [scalar_field("value", 1, 5), scalar_field("other", 1, 5)],
            )],
            "proto3",
        );
        let missing_type = descriptor_set(
            "pkg",
            "missing.proto",
            [message(
                "Message",
                [named_field("missing", 1, 11, ".pkg.Missing")],
            )],
            "proto3",
        );
        let recursive = descriptor_set(
            "pkg",
            "recursive.proto",
            [message(
                "Message",
                [named_field("self_ref", 1, 11, ".pkg.Message")],
            )],
            "proto3",
        );
        let duplicate_messages = descriptor_set(
            "pkg",
            "symbols.proto",
            [message("Message", []), message("Message", [])],
            "proto3",
        );
        for (data, schema_name, expected) in [
            (
                duplicate_fields,
                "pkg.Message",
                RemoteProtobufInitializationErrorV1::InvalidRemoteSchema,
            ),
            (
                missing_type,
                "pkg.Message",
                RemoteProtobufInitializationErrorV1::UnsupportedForRemote(
                    UnsupportedRemoteProtobufV1::DependencyGraph,
                ),
            ),
            (
                recursive,
                "pkg.Message",
                RemoteProtobufInitializationErrorV1::UnsupportedForRemote(
                    UnsupportedRemoteProtobufV1::RecursiveBuilderGraph,
                ),
            ),
            (
                duplicate_messages,
                "pkg.Message",
                RemoteProtobufInitializationErrorV1::InvalidRemoteSchema,
            ),
        ] {
            let fixture = fixture(data, schema_name);
            let definitions = validated_summary_definitions_for_test(&fixture);
            let context = StableContext::new();
            let budget = context.protobuf_budget();
            let error = match prepared(&definitions, &context, &budget) {
                Ok(_prepared) => panic!("invalid graph unexpectedly passed census"),
                Err(error) => error,
            };
            assert_eq!(error, expected);
            assert_eq!(
                *budget.state.usage.lock(),
                RemoteProtobufBudgetUsageV1::default()
            );
        }
    }

    #[test]
    fn strict_subset_rejects_service_options_and_unknown_wire() {
        let base_file = concat([
            string_field(1, "unsupported.proto"),
            string_field(2, "pkg"),
            bytes_field(4, &message("Message", [])),
            string_field(12, "proto3"),
        ]);
        let cases = [
            (
                descriptor_set_with_file(&concat([
                    base_file.clone(),
                    bytes_field(6, &string_field(1, "Service")),
                ])),
                UnsupportedRemoteProtobufV1::DescriptorFeature,
            ),
            (
                descriptor_set_with_file(&concat([base_file.clone(), bytes_field(8, &[])])),
                UnsupportedRemoteProtobufV1::DescriptorFeature,
            ),
            (
                descriptor_set_with_file(&concat([base_file, bytes_field(99, &[])])),
                UnsupportedRemoteProtobufV1::UnknownDescriptorField,
            ),
        ];
        for (data, kind) in cases {
            let fixture = fixture(data, "pkg.Message");
            let definitions = validated_summary_definitions_for_test(&fixture);
            let context = StableContext::new();
            let budget = context.protobuf_budget();
            let error = match prepared(&definitions, &context, &budget) {
                Ok(_prepared) => panic!("unsupported descriptor unexpectedly passed"),
                Err(error) => error,
            };
            assert_eq!(
                error,
                RemoteProtobufInitializationErrorV1::UnsupportedForRemote(kind)
            );
            assert_eq!(
                *budget.state.usage.lock(),
                RemoteProtobufBudgetUsageV1::default()
            );
        }
    }

    #[test]
    fn invalid_defaults_are_invalid_while_unescaped_string_defaults_are_unsupported() {
        let enumeration = enum_descriptor("State", &[("ZERO", 0)]);
        let cases = [
            descriptor_set(
                "pkg",
                "bad_int.proto",
                [message(
                    "Message",
                    [scalar_field_with_default("value", 1, 5, "not-an-int")],
                )],
                "proto2",
            ),
            descriptor_set(
                "pkg",
                "bad_bool.proto",
                [message(
                    "Message",
                    [scalar_field_with_default("value", 1, 8, "truthy")],
                )],
                "proto2",
            ),
            {
                let mut file = concat([
                    string_field(1, "bad_enum.proto"),
                    string_field(2, "pkg"),
                    string_field(12, "proto2"),
                    bytes_field(
                        4,
                        &message(
                            "Message",
                            [concat([
                                named_field("value", 1, 14, ".pkg.State"),
                                string_field(7, "MISSING"),
                            ])],
                        ),
                    ),
                    bytes_field(5, &enumeration),
                ]);
                file.rotate_left(0);
                descriptor_set_with_file(&file)
            },
        ];
        for data in cases {
            assert!(prost_reflect::DescriptorPool::decode(data.as_slice()).is_err());
            assert_prepare_error(
                data,
                "pkg.Message",
                RemoteProtobufInitializationErrorV1::InvalidRemoteSchema,
            );
        }

        for (kind, default) in [(9, "hello"), (12, "bytes")] {
            let data = descriptor_set(
                "pkg",
                "string_default.proto",
                [message(
                    "Message",
                    [scalar_field_with_default("value", 1, kind, default)],
                )],
                "proto2",
            );
            assert!(prost_reflect::DescriptorPool::decode(data.as_slice()).is_ok());
            assert_prepare_error(
                data,
                "pkg.Message",
                RemoteProtobufInitializationErrorV1::UnsupportedForRemote(
                    UnsupportedRemoteProtobufV1::DescriptorFeature,
                ),
            );
        }
    }

    #[test]
    fn duplicate_singular_wire_fields_are_uniformly_unsupported() {
        let duplicate_file_name = descriptor_set_with_file(&concat([
            string_field(1, "first.proto"),
            string_field(1, "second.proto"),
            string_field(2, "pkg"),
            string_field(12, "proto3"),
            bytes_field(4, &message("Message", [])),
        ]));
        assert!(prost_reflect::DescriptorPool::decode(duplicate_file_name.as_slice()).is_ok());
        assert_prepare_error(
            duplicate_file_name,
            "pkg.Message",
            RemoteProtobufInitializationErrorV1::UnsupportedForRemote(
                UnsupportedRemoteProtobufV1::DuplicateSingularField,
            ),
        );

        let duplicate_field_name = concat([
            string_field(1, "first"),
            string_field(1, "second"),
            varint_field(3, 1),
            varint_field(4, 1),
            varint_field(5, 5),
        ]);
        let data = descriptor_set(
            "pkg",
            "duplicate_field.proto",
            [message("Message", [duplicate_field_name])],
            "proto3",
        );
        assert!(prost_reflect::DescriptorPool::decode(data.as_slice()).is_ok());
        assert_prepare_error(
            data,
            "pkg.Message",
            RemoteProtobufInitializationErrorV1::UnsupportedForRemote(
                UnsupportedRemoteProtobufV1::DuplicateSingularField,
            ),
        );
    }

    #[test]
    fn complete_v1_namespace_collisions_fail_before_reservation() {
        let field_type = descriptor_set_with_file(&concat([
            string_field(1, "field_type.proto"),
            string_field(2, "pkg"),
            string_field(12, "proto3"),
            bytes_field(
                4,
                &concat([
                    string_field(1, "Owner"),
                    bytes_field(2, &scalar_field("Child", 1, 5)),
                    bytes_field(3, &message("Child", [])),
                ]),
            ),
        ]));

        let oneof_type = descriptor_set_with_file(&concat([
            string_field(1, "oneof_type.proto"),
            string_field(2, "pkg"),
            string_field(12, "proto3"),
            bytes_field(
                4,
                &concat([
                    string_field(1, "Owner"),
                    bytes_field(2, &oneof_field("value", 1, 5, 0)),
                    bytes_field(3, &message("Choice", [])),
                    bytes_field(8, &string_field(1, "Choice")),
                ]),
            ),
        ]));

        let package_type = concat([
            bytes_field(
                1,
                &concat([
                    string_field(1, "root.proto"),
                    string_field(12, "proto3"),
                    bytes_field(4, &message("foo", [])),
                ]),
            ),
            bytes_field(
                1,
                &concat([
                    string_field(1, "nested.proto"),
                    string_field(2, "foo.bar"),
                    string_field(12, "proto3"),
                    bytes_field(4, &message("Value", [])),
                ]),
            ),
        ]);

        let package_enum_value = concat([
            bytes_field(
                1,
                &concat([
                    string_field(1, "enum_root.proto"),
                    string_field(12, "proto3"),
                    bytes_field(5, &enum_descriptor("State", &[("foo", 0)])),
                ]),
            ),
            bytes_field(
                1,
                &concat([
                    string_field(1, "enum_nested.proto"),
                    string_field(2, "foo.bar"),
                    string_field(12, "proto3"),
                    bytes_field(4, &message("Value", [])),
                ]),
            ),
        ]);

        let normalized_fields = descriptor_set(
            "pkg",
            "normalized.proto",
            [message(
                "Owner",
                [scalar_field("foo_bar", 1, 5), scalar_field("fooBar", 2, 5)],
            )],
            "proto3",
        );

        let enum_value_field = descriptor_set_with_file(&concat([
            string_field(1, "enum_value.proto"),
            string_field(2, "pkg"),
            string_field(12, "proto3"),
            bytes_field(
                4,
                &concat([
                    string_field(1, "Owner"),
                    bytes_field(2, &scalar_field("VALUE", 1, 5)),
                    bytes_field(4, &enum_descriptor("State", &[("VALUE", 0)])),
                ]),
            ),
        ]));

        for (data, schema_name) in [
            (field_type, "pkg.Owner"),
            (oneof_type, "pkg.Owner"),
            (package_type, "foo.bar.Value"),
            (package_enum_value, "foo.bar.Value"),
            (normalized_fields, "pkg.Owner"),
            (enum_value_field, "pkg.Owner"),
        ] {
            assert!(prost_reflect::DescriptorPool::decode(data.as_slice()).is_err());
            assert_prepare_error(
                data,
                schema_name,
                RemoteProtobufInitializationErrorV1::InvalidRemoteSchema,
            );
        }
    }

    #[test]
    fn field_collision_keys_follow_the_owner_file_syntax() {
        let proto2_distinct_json_names = descriptor_set(
            "pkg",
            "proto2_distinct.proto",
            [message(
                "Owner",
                [scalar_field("foo_bar", 1, 5), scalar_field("FooBar", 2, 5)],
            )],
            "proto2",
        );
        assert!(
            prost_reflect::DescriptorPool::decode(proto2_distinct_json_names.as_slice()).is_ok()
        );
        let fixture = fixture(proto2_distinct_json_names, "pkg.Owner");
        let definitions = validated_summary_definitions_for_test(&fixture);
        let context = StableContext::new();
        let budget = context.protobuf_budget();
        assert!(prepared(&definitions, &context, &budget).is_ok());

        let proto2_same_json_name = descriptor_set(
            "pkg",
            "proto2_collision.proto",
            [message(
                "Owner",
                [scalar_field("foo_bar", 1, 5), scalar_field("fooBar", 2, 5)],
            )],
            "proto2",
        );
        assert!(prost_reflect::DescriptorPool::decode(proto2_same_json_name.as_slice()).is_err());
        assert_prepare_error(
            proto2_same_json_name,
            "pkg.Owner",
            RemoteProtobufInitializationErrorV1::InvalidRemoteSchema,
        );

        let proto3_lowercase_collision = descriptor_set(
            "pkg",
            "proto3_collision.proto",
            [message(
                "Owner",
                [scalar_field("foo_bar", 1, 5), scalar_field("FooBar", 2, 5)],
            )],
            "proto3",
        );
        assert!(
            prost_reflect::DescriptorPool::decode(proto3_lowercase_collision.as_slice()).is_err()
        );
        assert_prepare_error(
            proto3_lowercase_collision,
            "pkg.Owner",
            RemoteProtobufInitializationErrorV1::InvalidRemoteSchema,
        );
    }

    #[test]
    fn wire_reader_rejects_truncation_overflow_groups_and_length_overrun() {
        let malformed = [
            vec![0x80],
            vec![0x0a, 0x02, 0x01],
            vec![0x0b],
            vec![0x0c],
            vec![0x0e],
            vec![0x0f],
            vec![
                0x0a, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x02,
            ],
        ];
        for data in malformed {
            let fixture = fixture(data, "pkg.Message");
            let definitions = validated_summary_definitions_for_test(&fixture);
            let context = StableContext::new();
            let budget = context.protobuf_budget();
            let error = match prepared(&definitions, &context, &budget) {
                Ok(_prepared) => panic!("malformed wire unexpectedly passed"),
                Err(error) => error,
            };
            assert_eq!(
                error,
                RemoteProtobufInitializationErrorV1::InvalidRemoteSchema
            );
            assert_eq!(
                *budget.state.usage.lock(),
                RemoteProtobufBudgetUsageV1::default()
            );
        }
    }

    #[test]
    fn empty_protobuf_projection_advances_the_same_typestate() {
        let fixture = fixture_with_encoding(Vec::new(), "schema", "jsonschema", "json");
        let definitions = validated_summary_definitions_for_test(&fixture);
        let context = StableContext::new();
        let budget = context.protobuf_budget();
        let prepared = prepared(&definitions, &context, &budget).unwrap();
        assert_eq!(prepared.census.schemas, 0);
        let mut result = initialize_remote_protobuf_v1(prepared).unwrap();
        assert_eq!(result.protobuf_schema_count(), 0);
        let mut recognition = result.take_recognition_v1().unwrap();
        let channel = recognition.next_channel().unwrap().unwrap();
        assert!(!channel.recognized_by_ros2_reflection().unwrap());
        assert!(!channel.recognized_by_protobuf().unwrap());
        assert!(recognition.next_channel().unwrap().is_none());
    }

    #[test]
    fn exact_resource_limits_fail_before_reservation() {
        let data = simple_descriptor();
        let fixture = fixture(data.clone(), "pkg.Message");
        let definitions = validated_summary_definitions_for_test(&fixture);
        let mut cases = Vec::new();

        let mut value = limits();
        value.max_schemas = 0;
        cases.push((value, RemoteProtobufResourceLimitV1::SchemaCount));
        let mut value = limits();
        value.max_descriptor_bytes = data.len() as u64 - 1;
        cases.push((value, RemoteProtobufResourceLimitV1::DescriptorBytes));
        let mut value = limits();
        value.max_files = 0;
        cases.push((value, RemoteProtobufResourceLimitV1::FileCount));
        let mut value = limits();
        value.max_messages = 0;
        cases.push((value, RemoteProtobufResourceLimitV1::MessageCount));
        let mut value = limits();
        value.max_fields = 0;
        cases.push((value, RemoteProtobufResourceLimitV1::FieldCount));
        let mut value = limits();
        value.max_string_bytes = 1;
        cases.push((value, RemoteProtobufResourceLimitV1::StringBytes));
        let mut value = limits();
        value.max_single_string_bytes = 1;
        cases.push((value, RemoteProtobufResourceLimitV1::SingleStringBytes));
        let mut value = limits();
        value.max_descriptor_depth = 0;
        cases.push((value, RemoteProtobufResourceLimitV1::DescriptorDepth));
        let mut value = limits();
        value.max_census_steps = 0;
        cases.push((value, RemoteProtobufResourceLimitV1::CensusSteps));
        let mut value = limits();
        value.max_resolution_steps = 0;
        cases.push((value, RemoteProtobufResourceLimitV1::ResolutionSteps));
        let mut value = limits();
        value.max_materialization_steps = 0;
        cases.push((value, RemoteProtobufResourceLimitV1::MaterializationSteps));
        let mut value = limits();
        value.max_retained_bytes = 0;
        cases.push((value, RemoteProtobufResourceLimitV1::RetainedBytes));
        let mut value = limits();
        value.max_working_bytes = 0;
        cases.push((value, RemoteProtobufResourceLimitV1::WorkingBytes));

        for (limits, expected) in cases {
            let context = StableContext::new();
            let budget = context.protobuf_budget_with_limits(limits);
            let error = match prepared(&definitions, &context, &budget) {
                Ok(_prepared) => panic!("resource-constrained descriptor unexpectedly passed"),
                Err(error) => error,
            };
            assert_eq!(
                error,
                RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(expected)
            );
            assert_eq!(
                *budget.state.usage.lock(),
                RemoteProtobufBudgetUsageV1::default()
            );
        }
    }

    #[test]
    fn retained_result_capacity_is_reusable_only_after_drop() {
        let fixture = fixture(simple_descriptor(), "pkg.Message");
        let definitions = validated_summary_definitions_for_test(&fixture);
        let context = StableContext::new();
        let physical =
            crate::remote_chunk_scan::PhysicalChunkDefinitionsCapabilityV1::new_unscanned_for_test_with_binding_v1(
                &definitions,
                context.source.physical_source_binding_for_test_v1(),
            );
        let ros_budget = RemoteRos2InitializationBudget::new_for_protobuf_test_v1(
            &context.source,
            &context.viewer,
            &context.ros_profile,
            &context.wire,
        );
        let budget = RemoteProtobufInitializationBudgetV1::new_disarmed_v1(
            &context.viewer,
            &context.protobuf_profile,
            limits(),
            1,
            256_000_000,
            1,
            256_000_000,
        );
        let first = initialize_remote_protobuf_v1(
            prepared_with_physical_and_ros_budget(&physical, &context, &ros_budget, &budget)
                .unwrap(),
        )
        .unwrap();
        let error = match prepared_with_physical_and_ros_budget(
            &physical,
            &context,
            &ros_budget,
            &budget,
        ) {
            Ok(_prepared) => panic!("second retained result exceeded the aggregate cap"),
            Err(error) => error,
        };
        assert_eq!(
            error,
            RemoteProtobufInitializationErrorV1::ResourceLimitExceeded(
                RemoteProtobufResourceLimitV1::ReservationCapacity
            )
        );
        drop(first);
        let second = initialize_remote_protobuf_v1(
            prepared_with_physical_and_ros_budget(&physical, &context, &ros_budget, &budget)
                .unwrap(),
        )
        .unwrap();
        drop(second);
        assert_eq!(
            *budget.state.usage.lock(),
            RemoteProtobufBudgetUsageV1::default()
        );
    }

    #[test]
    fn wrong_profile_is_a_fatal_control_plane_mismatch_without_reservation() {
        let fixture = fixture(simple_descriptor(), "pkg.Message");
        let definitions = validated_summary_definitions_for_test(&fixture);
        let context = StableContext::new();
        let other_profile = RemoteProtobufProfileScopeV1::new_disarmed_v1();
        let budget = RemoteProtobufInitializationBudgetV1::new_disarmed_v1(
            &context.viewer,
            &other_profile,
            limits(),
            1,
            256_000_000,
            1,
            256_000_000,
        );
        assert_fatal_control_plane(|| {
            let transition = transition(
                &definitions,
                &context.source,
                &context.wire,
                &context.viewer,
                &context.ros_profile,
            );
            assert!(prepare_remote_protobuf_census_v1(transition, &budget).is_err());
        });
        assert_eq!(
            *budget.state.usage.lock(),
            RemoteProtobufBudgetUsageV1::default()
        );
    }
}
