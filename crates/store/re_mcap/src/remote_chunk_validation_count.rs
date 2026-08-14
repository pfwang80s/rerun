//! Exact first-pass validation/count plans for Web remote MCAP.
//!
//! This stage consumes only MCAP-025 message evidence and the sealed MCAP-029 manifest owner.
//! It never constructs a parser, Arrow builder, or local decoder initializer.

#![allow(dead_code)]
#![allow(clippy::map_err_ignore)]

use std::alloc::Layout;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::remote_channel_group::StableDecoderGroupIdV1;
use crate::remote_chunk_scan::{
    PhysicalChunkMessageEvidenceV1, PhysicalChunkValidationError, ValidatedPhysicalChunkExtent,
};
use crate::remote_manifest::ManifestTemporalPartitionAuthorityV1;

const REMOTE_VALIDATION_COUNT_PROFILE_VERSION_V1: u16 = 1;
const REMOTE_DECODER_RESOURCE_BOUND_VERSION_V1: u16 = 1;
const LOCKED_WASM_DLMALLOC_ALIGNMENT_V1: u64 = 8;
const LOCKED_WASM_DLMALLOC_CHUNK_OVERHEAD_V1: u64 = 4;
const LOCKED_WASM_DLMALLOC_MIN_CHUNK_V1: u64 = 16;
const LOCKED_WASM_DLMALLOC_TOP_FOOT_V1: u64 = 40;
const LOCKED_WASM_DLMALLOC_PAGE_V1: u64 = 64 * 1024;
const REMOTE_TYPED_OUTPUT_CENSUS_VERSION_V1: u16 = 1;
const CANONICAL_MESSAGE_TIMELINE_COUNT_V1: usize = 2;
const TYPED_COMPONENT_COUNT_V1: usize = 1;
const MAX_SCALAR_WIDTH_BYTES_V1: usize = std::mem::size_of::<u64>();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RemoteTypedOutputPeakCensusV1 {
    profile_version: u16,
    simultaneous_bytes: u64,
}

impl RemoteTypedOutputPeakCensusV1 {
    fn from_layouts_v1(
        rows: u64,
        retained_metadata_bytes: u64,
        payload_bytes: u64,
        protobuf: Option<crate::remote_typed_output::RemoteTypedProtobufCensusV1>,
    ) -> Result<Self, RemoteValidationCountErrorV1> {
        let rows = usize::try_from(rows).map_err(|_| arithmetic_error())?;
        let rows_plus_one = rows.checked_add(1).ok_or_else(arithmetic_error)?;
        let validity_bytes = rows.checked_add(7).ok_or_else(arithmetic_error)? / 8;
        let map_columns = CANONICAL_MESSAGE_TIMELINE_COUNT_V1
            .checked_add(TYPED_COMPONENT_COUNT_V1)
            .ok_or_else(arithmetic_error)?;
        let map_slots = map_columns
            .checked_next_power_of_two()
            .and_then(|slots| slots.checked_mul(2))
            .ok_or_else(arithmetic_error)?;

        let mut total = 0_u64;
        let mut charge = |bytes: u64| -> Result<(), RemoteValidationCountErrorV1> {
            total = total.checked_add(bytes).ok_or_else(arithmetic_error)?;
            Ok(())
        };
        let array = |layout: Layout| locked_footprint(layout);
        let repeated =
            |layout: Layout, count: usize| -> Result<u64, RemoteValidationCountErrorV1> {
                array(layout)?
                    .checked_mul(u64::try_from(count).map_err(|_| arithmetic_error())?)
                    .ok_or_else(arithmetic_error)
            };

        // Fixed owners and map tables retained simultaneously while `ChunkBuilder::build`
        // materializes the final `Chunk`.
        charge(array(Layout::new::<re_chunk::ChunkBuilder>())?)?;
        charge(array(Layout::new::<re_chunk::Chunk>())?)?;
        charge(array(Layout::new::<re_chunk::ChunkComponents>())?)?;
        charge(array(
            Layout::array::<usize>(map_slots).map_err(|_| arithmetic_error())?,
        )?)?;

        // `ChunkBuilder` staging: row IDs, two timeline value vectors, and one component slot
        // vector. Every row also retains its independently allocated scalar array and one-child
        // Struct field vector until concatenation completes.
        charge(array(
            Layout::array::<re_chunk::RowId>(rows).map_err(|_| arithmetic_error())?,
        )?)?;
        charge(repeated(
            Layout::array::<i64>(rows).map_err(|_| arithmetic_error())?,
            CANONICAL_MESSAGE_TIMELINE_COUNT_V1,
        )?)?;
        charge(array(
            Layout::array::<Option<arrow::array::ArrayRef>>(rows)
                .map_err(|_| arithmetic_error())?,
        )?)?;
        charge(repeated(
            Layout::array::<u8>(MAX_SCALAR_WIDTH_BYTES_V1).map_err(|_| arithmetic_error())?,
            rows,
        )?)?;
        charge(repeated(
            Layout::array::<arrow::array::ArrayRef>(TYPED_COMPONENT_COUNT_V1)
                .map_err(|_| arithmetic_error())?,
            rows,
        )?)?;

        // Each retained row owns a primitive Arrow array and its wrapping struct array.
        // In addition to the value and child-reference buffers above, those arrays retain
        // separate `ArrayData`, Arc control, and `Field`/`Fields` heap owners until component
        // concatenation completes.
        charge(repeated(Layout::new::<arrow::array::ArrayData>(), rows)?)?;
        charge(repeated(
            Layout::new::<std::sync::Arc<arrow::array::ArrayData>>(),
            rows,
        )?)?;
        charge(repeated(Layout::new::<arrow::datatypes::Field>(), rows)?)?;
        charge(repeated(Layout::new::<arrow::datatypes::Fields>(), rows)?)?;
        charge(repeated(Layout::new::<arrow::array::ArrayData>(), rows)?)?;
        charge(repeated(
            Layout::new::<std::sync::Arc<arrow::array::ArrayData>>(),
            rows,
        )?)?;

        // Final Arrow/Chunk backing overlaps all staging above at the build safe point.
        charge(array(
            Layout::array::<[u8; 16]>(rows).map_err(|_| arithmetic_error())?,
        )?)?;
        charge(repeated(
            Layout::array::<i64>(rows).map_err(|_| arithmetic_error())?,
            CANONICAL_MESSAGE_TIMELINE_COUNT_V1,
        )?)?;
        charge(array(
            Layout::array::<u8>(
                rows.checked_mul(MAX_SCALAR_WIDTH_BYTES_V1)
                    .ok_or_else(arithmetic_error)?,
            )
            .map_err(|_| arithmetic_error())?,
        )?)?;
        charge(array(
            Layout::array::<i32>(rows_plus_one).map_err(|_| arithmetic_error())?,
        )?)?;
        charge(repeated(
            Layout::array::<u8>(validity_bytes).map_err(|_| arithmetic_error())?,
            TYPED_COMPONENT_COUNT_V1 + 1,
        )?)?;

        // `ChunkBuilder::build`, `arrays_to_list_array`, and `Chunk::from_native_row_ids`
        // overlap these fixed heap owners at the final construction safe point. They remain
        // distinct allocations even where their payload bytes were already charged above.
        charge(repeated(Layout::new::<usize>(), 2)?)?; // staging map RawTables
        charge(array(Layout::new::<Vec<&dyn arrow::array::Array>>())?)?; // sparse refs
        charge(array(Layout::new::<Vec<&dyn arrow::array::Array>>())?)?; // dense refs
        charge(array(Layout::new::<arrow::buffer::Buffer>())?)?; // concatenated child values
        charge(array(Layout::new::<arrow::array::ArrayData>())?)?; // concatenated child data
        charge(array(
            Layout::new::<std::sync::Arc<arrow::array::ArrayData>>(),
        )?)?;
        charge(array(Layout::new::<arrow::buffer::Buffer>())?)?; // list offsets
        charge(array(Layout::new::<arrow::buffer::Buffer>())?)?; // list null bitmap
        charge(array(
            Layout::new::<std::sync::Arc<arrow::datatypes::Field>>(),
        )?)?;
        charge(array(Layout::new::<arrow::buffer::Buffer>())?)?; // native row IDs
        charge(array(Layout::new::<arrow::array::ArrayData>())?)?; // row-ID Arrow data
        charge(repeated(
            Layout::new::<arrow::buffer::Buffer>(),
            CANONICAL_MESSAGE_TIMELINE_COUNT_V1,
        )?)?;
        charge(repeated(Layout::new::<usize>(), 2)?)?; // final map RawTables

        // Entity/field/component/archetype strings may be cloned by the builder and retained by
        // the completed chunk at the overlap point. Charge both owners from sealed descriptor
        // lengths rather than a fixed allowance.
        let metadata = usize::try_from(retained_metadata_bytes).map_err(|_| arithmetic_error())?;
        charge(repeated(
            Layout::array::<u8>(metadata).map_err(|_| arithmetic_error())?,
            2,
        )?)?;

        if let Some(protobuf) = protobuf {
            let payload = usize::try_from(payload_bytes).map_err(|_| arithmetic_error())?;
            let field_count = usize::try_from(protobuf.fields).map_err(|_| arithmetic_error())?;
            let wrapper_count =
                usize::try_from(protobuf.real_oneofs).map_err(|_| arithmetic_error())?;
            let nodes = field_count
                .checked_add(wrapper_count)
                .ok_or_else(arithmetic_error)?;
            let row_nodes = rows.checked_mul(nodes).ok_or_else(arithmetic_error)?;
            let value_occurrences = rows.checked_add(payload).ok_or_else(arithmetic_error)?;
            let recursive_visits = nodes
                .checked_mul(value_occurrences)
                .ok_or_else(arithmetic_error)?;

            // Recursive protobuf builders create one Arrow field/builder owner for every direct
            // field and real-oneof wrapper on every row. Nested message/list/map traversal also
            // materializes bounded grouped-field and entry-order scratch. A wire byte can begin
            // at most one nested value or collection element, so `rows + payload_bytes` is the
            // closed occurrence bound used for those transient pointer arenas.
            for layout in [
                Layout::new::<arrow::datatypes::Field>(),
                Layout::new::<arrow::array::ArrayData>(),
                Layout::new::<std::sync::Arc<arrow::array::ArrayData>>(),
                Layout::new::<arrow::buffer::Buffer>(),
                Layout::new::<Box<dyn arrow::array::ArrayBuilder>>(),
            ] {
                charge(repeated(layout, row_nodes)?)?;
            }
            charge(repeated(
                Layout::new::<&crate::remote_typed_output::RemoteTypedProtobufFieldV1>(),
                recursive_visits,
            )?)?;
            charge(repeated(
                Layout::new::<crate::remote_protobuf_descriptor::RemoteNormalizedFieldV1>(),
                value_occurrences,
            )?)?;

            // Variable-width string/bytes, list offsets, map entry scratch, and final concat may
            // overlap. Charge four payload-sized byte buffers and two offset buffers; payload is
            // a strict upper bound for all decoded variable data and element cardinality.
            charge(repeated(
                Layout::array::<u8>(payload).map_err(|_| arithmetic_error())?,
                4,
            )?)?;
            let offsets = payload
                .checked_add(rows)
                .and_then(|value| value.checked_add(1))
                .ok_or_else(arithmetic_error)?;
            charge(repeated(
                Layout::array::<i32>(offsets).map_err(|_| arithmetic_error())?,
                2,
            )?)?;

            let metadata_owners = rows
                .checked_mul(2)
                .and_then(|count| count.checked_add(1))
                .ok_or_else(arithmetic_error)?;
            charge(repeated(
                Layout::array::<u8>(metadata).map_err(|_| arithmetic_error())?,
                metadata_owners,
            )?)?;
        }

        Ok(Self {
            profile_version: REMOTE_TYPED_OUTPUT_CENSUS_VERSION_V1,
            simultaneous_bytes: total,
        })
    }
}

fn typed_output_peak_bytes_v1(
    rows: u64,
    retained_metadata_bytes: u64,
    payload_bytes: u64,
    protobuf: Option<crate::remote_typed_output::RemoteTypedProtobufCensusV1>,
) -> Result<u64, RemoteValidationCountErrorV1> {
    let census = RemoteTypedOutputPeakCensusV1::from_layouts_v1(
        rows,
        retained_metadata_bytes,
        payload_bytes,
        protobuf,
    )?;
    if census.profile_version != REMOTE_TYPED_OUTPUT_CENSUS_VERSION_V1 {
        return Err(arithmetic_error());
    }
    Ok(census.simultaneous_bytes)
}

#[cfg(test)]
pub(crate) fn typed_output_peak_for_descriptor_test_v1(
    rows: u64,
    payload_bytes: u64,
    descriptor: &crate::remote_typed_output::RemoteTypedOutputDescriptorV1,
) -> Result<u64, RemoteValidationCountErrorV1> {
    let retained_metadata_bytes = descriptor
        .retained_metadata_bytes_v1()
        .ok_or_else(arithmetic_error)?;
    typed_output_peak_bytes_v1(
        rows,
        retained_metadata_bytes,
        payload_bytes,
        descriptor.protobuf_census_v1(),
    )
}

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

fn ensure_matching_source_unit_ordinal_v1(
    evidence_ordinal: usize,
    authority_ordinal: u32,
) -> Result<(), RemoteValidationCountErrorV1> {
    if u32::try_from(evidence_ordinal).ok() == Some(authority_ordinal) {
        Ok(())
    } else {
        Err(RemoteValidationCountErrorV1::ManifestMismatch)
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
    output_bytes: u64,
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

/// A plan-owned reservation for typed Arrow/Chunk output.
///
/// It charges the same aggregate root as validation plans, so independently issued descriptors
/// cannot create private per-descriptor capacity. The reservation remains live through the sealed
/// typed-chunk handoff.
pub(crate) struct RemoteTypedOutputReservationV1 {
    state: Arc<RemoteValidationCountBudgetStateV1>,
    bytes: u64,
}

impl Drop for RemoteTypedOutputReservationV1 {
    fn drop(&mut self) {
        let mut usage = self.state.usage.lock();
        usage.output_bytes = usage
            .output_bytes
            .checked_sub(self.bytes)
            .expect("typed output budget underflowed");
    }
}

fn reserve_typed_output_state_v1(
    state: &Arc<RemoteValidationCountBudgetStateV1>,
    rows: u64,
    retained_metadata_bytes: u64,
    payload_bytes: u64,
    protobuf: Option<crate::remote_typed_output::RemoteTypedProtobufCensusV1>,
) -> Result<RemoteTypedOutputReservationV1, RemoteValidationCountErrorV1> {
    let bytes = typed_output_peak_bytes_v1(rows, retained_metadata_bytes, payload_bytes, protobuf)?;
    let mut usage = state.usage.lock();
    let next = usage
        .output_bytes
        .checked_add(bytes)
        .ok_or_else(arithmetic_error)?;
    let combined_next = usage
        .combined_retained_bytes
        .checked_add(next)
        .ok_or_else(arithmetic_error)?;
    if combined_next > state.max_aggregate_combined_retained_bytes {
        return Err(RemoteValidationCountErrorV1::ResourceLimitExceeded(
            RemoteValidationCountResourceLimitV1::ReservationCapacity,
        ));
    }
    usage.output_bytes = next;
    drop(usage);
    Ok(RemoteTypedOutputReservationV1 {
        state: Arc::clone(state),
        bytes,
    })
}

impl RemoteValidationCountBudgetV1 {
    #[cfg(any(test, rerun_mcap_phase_a_proof_v1))]
    pub(crate) fn new_for_phase_a_measurement_v1(
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

    #[cfg(test)]
    pub(crate) fn new_for_test_v1(
        limits: UnfrozenRemoteValidationCountLimitsV1,
        max_active_plans: u64,
        max_aggregate_retained_bytes: u64,
        max_aggregate_combined_retained_bytes: u64,
    ) -> Self {
        Self::new_for_phase_a_measurement_v1(
            limits,
            max_active_plans,
            max_aggregate_retained_bytes,
            max_aggregate_combined_retained_bytes,
        )
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
            output_bytes: usage.output_bytes,
        };
        if next.active_plans > self.state.max_active_plans
            || next.retained_bytes > self.state.max_aggregate_retained_bytes
            || next
                .combined_retained_bytes
                .checked_add(next.output_bytes)
                .ok_or_else(arithmetic_error)?
                > self.state.max_aggregate_combined_retained_bytes
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
    pub(crate) fn reserve_typed_output_for_test_v1(
        &self,
        rows: u64,
    ) -> Result<RemoteTypedOutputReservationV1, RemoteValidationCountErrorV1> {
        reserve_typed_output_state_v1(&self.state, rows, 0, 0, None)
    }

    #[cfg(test)]
    pub(crate) fn is_idle_for_test_v1(&self) -> bool {
        *self.state.usage.lock() == RemoteValidationCountBudgetUsageV1::default()
    }
}

#[cfg(test)]
impl UnfrozenRemoteValidationCountLimitsV1 {
    #[cfg(any(test, rerun_mcap_phase_a_proof_v1))]
    pub(crate) const fn generous_for_phase_a_measurement_v1() -> Self {
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

    #[cfg(test)]
    pub(crate) const fn generous_for_test_v1() -> Self {
        Self::generous_for_phase_a_measurement_v1()
    }
}

#[cfg(test)]
mod typed_output_peak_tests {
    use super::*;

    #[test]
    fn locked_census_is_explicit_and_overflow_closed() {
        let empty = RemoteTypedOutputPeakCensusV1::from_layouts_v1(0, 0, 0, None).unwrap();
        let one = RemoteTypedOutputPeakCensusV1::from_layouts_v1(1, 0, 0, None).unwrap();
        let with_metadata = RemoteTypedOutputPeakCensusV1::from_layouts_v1(1, 17, 0, None).unwrap();
        assert_eq!(empty.profile_version, REMOTE_TYPED_OUTPUT_CENSUS_VERSION_V1);
        assert!(empty.simultaneous_bytes > 0);
        assert!(one.simultaneous_bytes > empty.simultaneous_bytes);
        assert!(with_metadata.simultaneous_bytes > one.simultaneous_bytes);
        assert!(typed_output_peak_bytes_v1(u64::MAX, 0, 0, None).is_err());
    }

    #[test]
    fn exact_output_limit_accepts_and_one_byte_short_rejects() {
        let limits = UnfrozenRemoteValidationCountLimitsV1::generous_for_test_v1();
        let retained = typed_output_peak_bytes_v1(2, 0, 0, None).unwrap();
        let exact = RemoteValidationCountBudgetV1::new_for_test_v1(
            limits.with_combined_limit_for_test_v1(retained),
            1,
            0,
            retained,
        );
        let reservation = exact.reserve_typed_output_for_test_v1(2).unwrap();
        drop(reservation);
        assert!(exact.is_idle_for_test_v1());

        let short = RemoteValidationCountBudgetV1::new_for_test_v1(
            limits.with_combined_limit_for_test_v1(retained - 1),
            1,
            0,
            retained - 1,
        );
        assert!(matches!(
            short.reserve_typed_output_for_test_v1(2),
            Err(RemoteValidationCountErrorV1::ResourceLimitExceeded(
                RemoteValidationCountResourceLimitV1::ReservationCapacity
            ))
        ));
    }

    #[test]
    fn recursive_protobuf_exact_output_limit_accepts_and_one_byte_short_rejects() {
        let limits = UnfrozenRemoteValidationCountLimitsV1::generous_for_test_v1();
        let protobuf = crate::remote_typed_output::RemoteTypedProtobufCensusV1 {
            fields: 9,
            repeated_fields: 2,
            message_fields: 2,
            map_fields: 1,
            real_oneofs: 1,
            enum_fields: 1,
            enum_values: 3,
        };
        let retained = typed_output_peak_bytes_v1(3, 97, 64, Some(protobuf)).unwrap();
        let exact = RemoteValidationCountBudgetV1::new_for_test_v1(
            limits.with_combined_limit_for_test_v1(retained),
            1,
            0,
            retained,
        );
        let reservation =
            reserve_typed_output_state_v1(&exact.state, 3, 97, 64, Some(protobuf)).unwrap();
        drop(reservation);
        assert!(exact.is_idle_for_test_v1());

        let short = RemoteValidationCountBudgetV1::new_for_test_v1(
            limits.with_combined_limit_for_test_v1(retained - 1),
            1,
            0,
            retained - 1,
        );
        assert!(matches!(
            reserve_typed_output_state_v1(&short.state, 3, 97, 64, Some(protobuf)),
            Err(RemoteValidationCountErrorV1::ResourceLimitExceeded(
                RemoteValidationCountResourceLimitV1::ReservationCapacity
            ))
        ));
        assert!(short.is_idle_for_test_v1());
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

impl ExactChannelDispatchCountV1 {
    pub(crate) const fn channel_id_v1(self) -> u16 {
        self.channel_id
    }

    pub(crate) const fn message_count_v1(self) -> u64 {
        self.message_count
    }

    pub(crate) const fn payload_bytes_v1(self) -> u64 {
        self.payload_bytes
    }

    pub(crate) const fn selected_group_v1(self) -> StableDecoderGroupIdV1 {
        self.selected_group
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DecoderDispatchResourceBoundV1 {
    version: u16,
    group_id: StableDecoderGroupIdV1,
    exact_num_rows: u64,
    exact_payload_bytes: u64,
}

pub(crate) struct ValidatedChunkDispatchPlanV1<'manifest, 'a, 'definitions, 'input, 'source, 'wire>
{
    evidence: PhysicalChunkMessageEvidenceV1<'input>,
    authority: &'manifest ManifestTemporalPartitionAuthorityV1<
        'manifest,
        'a,
        'definitions,
        'input,
        'source,
        'wire,
    >,
    channels: Box<[ExactChannelDispatchCountV1]>,
    selected_groups: Box<[StableDecoderGroupIdV1]>,
    resource_bounds: Box<[DecoderDispatchResourceBoundV1]>,
    extent: ValidatedPhysicalChunkExtent,
    _reservation: RemoteValidationCountReservationV1,
}

impl ValidatedChunkDispatchPlanV1<'_, '_, '_, '_, '_, '_> {
    pub(crate) fn ensure_current_v1(&self) -> Result<(), RemoteValidationCountErrorV1> {
        self.evidence
            .ensure_current_v1()
            .map_err(map_physical_error)
    }

    pub(crate) fn channels_v1(&self) -> &[ExactChannelDispatchCountV1] {
        &self.channels
    }

    pub(crate) fn evidence_v1(&self) -> &PhysicalChunkMessageEvidenceV1<'_> {
        &self.evidence
    }

    pub(crate) fn expected_rows_v1(&self) -> u64 {
        self.channels.iter().fold(0_u64, |total, channel| {
            total
                .checked_add(channel.message_count)
                .unwrap_or_else(|| unreachable!("validated message-count sum overflowed"))
        })
    }

    pub(crate) fn expected_payload_bytes_v1(&self) -> u64 {
        self.channels.iter().fold(0_u64, |total, channel| {
            total
                .checked_add(channel.payload_bytes)
                .unwrap_or_else(|| unreachable!("validated payload-byte sum overflowed"))
        })
    }

    pub(crate) fn bind_executable_factory_v1(
        &self,
        channel_id: u16,
    ) -> Result<
        crate::remote_protobuf_descriptor::RemoteExecutableFactoryV1<'_, '_, '_, '_, '_>,
        crate::remote_protobuf_descriptor::RemoteExecutableAdapterErrorV1,
    > {
        if !self.channels.iter().any(|row| row.channel_id == channel_id) {
            return Err(
                crate::remote_protobuf_descriptor::RemoteExecutableAdapterErrorV1::ConfigMismatch,
            );
        }
        self.authority.bind_executable_factory_v1(channel_id)
    }

    pub(crate) fn authority_v1(
        &self,
    ) -> &ManifestTemporalPartitionAuthorityV1<'_, '_, '_, '_, '_, '_> {
        self.authority
    }

    pub(crate) fn reserve_typed_output_v1(
        &self,
        rows: u64,
        payload_bytes: u64,
        descriptor: &crate::remote_typed_output::RemoteTypedOutputDescriptorV1,
    ) -> Result<RemoteTypedOutputReservationV1, RemoteValidationCountErrorV1> {
        // V1 only admits one scalar component per row. This bound intentionally includes the
        // simultaneous builder staging, two timeline cells, row ids, Arrow buffers, chunk
        // metadata, and allocator overhead. MCAP-088 may tighten this measured profile, but may
        // not raise it without changing the profile version.
        let retained_metadata_bytes = descriptor
            .retained_metadata_bytes_v1()
            .ok_or_else(arithmetic_error)?;
        reserve_typed_output_state_v1(
            &self._reservation.state,
            rows,
            retained_metadata_bytes,
            payload_bytes,
            descriptor.protobuf_census_v1(),
        )
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

pub(crate) fn validate_and_count_with_authority_v1<
    'manifest,
    'a,
    'definitions,
    'input,
    'source,
    'wire,
>(
    evidence: PhysicalChunkMessageEvidenceV1<'input>,
    authority: &'manifest ManifestTemporalPartitionAuthorityV1<
        'manifest,
        'a,
        'definitions,
        'input,
        'source,
        'wire,
    >,
    budget: &RemoteValidationCountBudgetV1,
) -> Result<
    ValidatedChunkDispatchPlanV1<'manifest, 'a, 'definitions, 'input, 'source, 'wire>,
    RemoteValidationCountErrorV1,
> {
    if budget.state.limits.profile_version != REMOTE_VALIDATION_COUNT_PROFILE_VERSION_V1 {
        return Err(RemoteValidationCountErrorV1::ResourceLimitExceeded(
            RemoteValidationCountResourceLimitV1::Arithmetic,
        ));
    }
    evidence.ensure_current_v1().map_err(map_physical_error)?;
    let evidence_ordinal = evidence
        .canonical_ordinal_v1()
        .map_err(map_physical_error)?;
    ensure_matching_source_unit_ordinal_v1(evidence_ordinal, authority.source_unit_ordinal_v1())?;
    let manifest = authority.groups_v1();
    manifest
        .ensure_current_for_validation_v1()
        .map_err(|_error| RemoteValidationCountErrorV1::StaleSource)?;
    manifest
        .ensure_matches_physical_evidence_v1(&evidence)
        .map_err(|_error| RemoteValidationCountErrorV1::ManifestMismatch)?;
    let census = evidence.channel_census_v1().map_err(map_physical_error)?;
    let assignments = manifest.assignments_v1();
    if census.len() != assignments.len() {
        return Err(RemoteValidationCountErrorV1::ManifestMismatch);
    }
    let mut channel_len = 0_usize;
    let mut exact_messages = 0_u64;
    let mut exact_payload = 0_u64;
    for (channel, assignment) in census.zip(assignments) {
        if channel.channel_id() != assignment.channel_id() {
            return Err(RemoteValidationCountErrorV1::ManifestMismatch);
        }
        if authority.matches_group_v1(assignment.group_id()) {
            channel_len = channel_len.checked_add(1).ok_or_else(arithmetic_error)?;
            exact_messages = checked_add(exact_messages, channel.message_count())?;
            exact_payload = checked_add(exact_payload, channel.payload_bytes())?;
        }
    }
    if channel_len != authority.channels_v1().len() {
        return Err(RemoteValidationCountErrorV1::ManifestMismatch);
    }
    let channel_count = u64::try_from(channel_len).map_err(|_error| arithmetic_error())?;
    if channel_count > budget.state.limits.max_channels {
        return Err(RemoteValidationCountErrorV1::ResourceLimitExceeded(
            RemoteValidationCountResourceLimitV1::ChannelCount,
        ));
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
    let group_count = 1_usize;
    let group_count_u64 = 1_u64;
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
    for channel_id in authority.channels_v1() {
        let (channel, assignment) = evidence
            .channel_census_v1()
            .map_err(map_physical_error)?
            .zip(assignments)
            .find(|(channel, assignment)| {
                channel.channel_id() == *channel_id
                    && assignment.channel_id() == *channel_id
                    && authority.matches_group_v1(assignment.group_id())
            })
            .ok_or(RemoteValidationCountErrorV1::ManifestMismatch)?;
        channels.push(ExactChannelDispatchCountV1 {
            channel_id: channel.channel_id(),
            message_count: channel.message_count(),
            payload_bytes: channel.payload_bytes(),
            selected_group: assignment.group_id(),
        });
    }
    selected_groups.push(authority.group_id_v1());
    resource_bounds.push(DecoderDispatchResourceBoundV1 {
        version: REMOTE_DECODER_RESOURCE_BOUND_VERSION_V1,
        group_id: authority.group_id_v1(),
        exact_num_rows: exact_messages,
        exact_payload_bytes: exact_payload,
    });
    evidence.ensure_current_v1().map_err(map_physical_error)?;
    manifest
        .ensure_current_for_validation_v1()
        .map_err(|_error| RemoteValidationCountErrorV1::StaleSource)?;
    let extent = evidence.extent_v1().map_err(map_physical_error)?;
    Ok(ValidatedChunkDispatchPlanV1 {
        evidence,
        authority,
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
    .ok_or_else(arithmetic_error)?
    .checked_add(locked_footprint(
        Layout::array::<crate::remote_chunk_dispatch::RemoteTypedChunkHandoffV1>(channels)
            .map_err(|_error| arithmetic_error())?,
    )?)
    .ok_or_else(arithmetic_error)?;
    Ok(bytes)
}

pub(crate) fn locked_footprint(layout: Layout) -> Result<u64, RemoteValidationCountErrorV1> {
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

#[cfg(test)]
pub(crate) fn locked_footprint_for_test_v1(
    layout: Layout,
) -> Result<u64, RemoteValidationCountErrorV1> {
    locked_footprint(layout)
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
    fn cross_ordinal_evidence_is_rejected_before_reservation() {
        assert_eq!(ensure_matching_source_unit_ordinal_v1(0, 0), Ok(()));
        assert_eq!(
            ensure_matching_source_unit_ordinal_v1(0, 1),
            Err(RemoteValidationCountErrorV1::ManifestMismatch)
        );
        assert_eq!(
            ensure_matching_source_unit_ordinal_v1(usize::MAX, u32::MAX),
            Err(RemoteValidationCountErrorV1::ManifestMismatch)
        );

        let source = include_str!("remote_chunk_validation_count.rs");
        let ordinal_check = source
            .find(
                "ensure_matching_source_unit_ordinal_v1(evidence_ordinal, authority.source_unit_ordinal_v1())?",
            )
            .expect("validation checks the sealed evidence/authority ordinal");
        let reservation = source
            .find("let reservation = budget.reserve")
            .expect("validation eventually reserves retained ownership");
        assert!(ordinal_check < reservation);
    }

    #[test]
    fn aggregate_terminal_handoff_reservation_is_exact_and_one_byte_short_fails_before_work() {
        let limits = UnfrozenRemoteValidationCountLimitsV1::generous_for_test_v1();
        let retained = exact_retained_bytes(2, 1).unwrap();
        let expected = locked_footprint(Layout::array::<ExactChannelDispatchCountV1>(2).unwrap())
            .unwrap()
            .checked_add(
                locked_footprint(Layout::array::<StableDecoderGroupIdV1>(1).unwrap()).unwrap(),
            )
            .and_then(|bytes| {
                bytes.checked_add(
                    locked_footprint(Layout::array::<DecoderDispatchResourceBoundV1>(1).unwrap())
                        .unwrap(),
                )
            })
            .and_then(|bytes| {
                bytes.checked_add(
                    locked_footprint(
                        Layout::array::<crate::remote_chunk_dispatch::RemoteTypedChunkHandoffV1>(2)
                            .unwrap(),
                    )
                    .unwrap(),
                )
            })
            .unwrap();
        assert_eq!(retained, expected);
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

        let source = include_str!("remote_chunk_validation_count.rs");
        let reservation = source
            .find("let reservation = budget.reserve")
            .expect("validation reserves the aggregate terminal handoff peak");
        let first_result_allocation = source
            .find("let mut channels = Vec::new()")
            .expect("validation materializes the per-channel result after reservation");
        assert!(reservation < first_result_allocation);
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
        assert!(production.contains("ManifestTemporalPartitionAuthorityV1"));
        assert!(production.contains("REMOTE_DECODER_RESOURCE_BOUND_VERSION_V1"));
    }
}
