//! Descriptor-only physical ownership validation for indexed Chunk records.

use std::ops::Range;
use std::sync::Arc;

use parking_lot::Mutex;

use super::definitions::ValidatedSummaryDefinitions;
use super::materialization::{BoundedSummaryRecord, MaterializedSummaryRecords};

const DATA_END_BODY_LEN: usize = size_of::<u32>();

/// Per-object limits for descriptor-only physical validation.
///
/// There is deliberately no production constructor while remote MCAP remains disarmed.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PhysicalRegionLimits {
    max_canonical_chunks: u64,
    max_chunk_record_bytes: u64,
    max_message_index_region_bytes: u64,
    max_compression_bytes: u64,
    max_compressed_bytes: u64,
    max_uncompressed_bytes: u64,
    max_descriptor_retained_bytes: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PhysicalRegionBudgetUsage {
    active_reservations: u64,
    canonical_chunks: u64,
    descriptor_retained_bytes: u64,
}

struct PhysicalRegionBudgetState {
    limits: PhysicalRegionLimits,
    max_active_reservations: u64,
    max_aggregate_canonical_chunks: u64,
    max_aggregate_descriptor_retained_bytes: u64,
    usage: Mutex<PhysicalRegionBudgetUsage>,
}

/// Aggregate owner for the retained canonical physical-unit descriptor allocation.
///
/// Its constructor remains test-only until the Web remote-MCAP production capability is sealed.
pub(crate) struct PhysicalRegionBudget {
    state: Arc<PhysicalRegionBudgetState>,
}

impl std::fmt::Debug for PhysicalRegionBudget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PhysicalRegionBudget")
            .field("limits", &self.state.limits)
            .field(
                "max_active_reservations",
                &self.state.max_active_reservations,
            )
            .field(
                "max_aggregate_canonical_chunks",
                &self.state.max_aggregate_canonical_chunks,
            )
            .field(
                "max_aggregate_descriptor_retained_bytes",
                &self.state.max_aggregate_descriptor_retained_bytes,
            )
            .finish_non_exhaustive()
    }
}

impl PhysicalRegionBudget {
    fn limits(&self) -> &PhysicalRegionLimits {
        &self.state.limits
    }

    fn try_reserve(
        &self,
        canonical_chunks: u64,
        descriptor_retained_bytes: u64,
    ) -> Result<PhysicalRegionReservation, IndexConsistencyViolation> {
        let mut usage = self.state.usage.lock();
        let next = PhysicalRegionBudgetUsage {
            active_reservations: checked_add(usage.active_reservations, 1)?,
            canonical_chunks: checked_add(usage.canonical_chunks, canonical_chunks)?,
            descriptor_retained_bytes: checked_add(
                usage.descriptor_retained_bytes,
                descriptor_retained_bytes,
            )?,
        };
        if next.active_reservations > self.state.max_active_reservations
            || next.canonical_chunks > self.state.max_aggregate_canonical_chunks
            || next.descriptor_retained_bytes > self.state.max_aggregate_descriptor_retained_bytes
        {
            return Err(IndexConsistencyViolation::DescriptorReservationLimitExceeded);
        }
        *usage = next;
        drop(usage);

        Ok(PhysicalRegionReservation {
            state: Arc::clone(&self.state),
            canonical_chunks,
            descriptor_retained_bytes,
        })
    }
}

/// Move-only ownership of one retained canonical descriptor vector reservation.
struct PhysicalRegionReservation {
    state: Arc<PhysicalRegionBudgetState>,
    canonical_chunks: u64,
    descriptor_retained_bytes: u64,
}

impl std::fmt::Debug for PhysicalRegionReservation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PhysicalRegionReservation")
            .field("canonical_chunks", &self.canonical_chunks)
            .field("descriptor_retained_bytes", &self.descriptor_retained_bytes)
            .finish_non_exhaustive()
    }
}

impl Drop for PhysicalRegionReservation {
    fn drop(&mut self) {
        let mut usage = self.state.usage.lock();
        usage.active_reservations = usage
            .active_reservations
            .checked_sub(1)
            .expect("live physical-region reservation has one active owner");
        usage.canonical_chunks = usage
            .canonical_chunks
            .checked_sub(self.canonical_chunks)
            .expect("live physical-region reservation retains its canonical descriptors");
        usage.descriptor_retained_bytes = usage
            .descriptor_retained_bytes
            .checked_sub(self.descriptor_retained_bytes)
            .expect("live physical-region reservation retains its descriptor bytes");
    }
}

/// A descriptor-only physical-index consistency failure.
///
/// Variants deliberately omit offsets, sizes, source identities, topics, and descriptor contents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IndexConsistencyViolation {
    InvalidDataSectionEvidence,
    ConflictingChunkIndex,
    CanonicalChunkLimitExceeded,
    ChunkRecordTooShort,
    ChunkRecordByteLimitExceeded,
    MessageIndexRegionByteLimitExceeded,
    CompressionByteLimitExceeded,
    CompressedByteLimitExceeded,
    UncompressedByteLimitExceeded,
    DescriptorRetainedByteLimitExceeded,
    ArithmeticOverflow,
    MessageIndexPresenceMismatch,
    DuplicateMessageIndexOffset,
    MessageIndexOffsetOutsideOwningRegion,
    PhysicalRegionOutsideDataSection,
    PhysicalRegionOverlap,
    DescriptorReservationLimitExceeded,
    DescriptorAllocationFailed,
    UnknownCanonicalRegion,
    MessageIndexRawReservationLimitExceeded,
    MessageIndexRawInputMismatch,
    MessageIndexRecordEnvelopeInvalid,
    MessageIndexRecordWrongOpcode,
    MessageIndexRecordOutOfBounds,
    MessageIndexRecordBodyInvalid,
    MessageIndexRecordLimitExceeded,
    MessageIndexEntryLimitExceeded,
    MessageIndexAggregateEntryLimitExceeded,
    MessageIndexMappingMismatch,
    MessageIndexResultRetainedByteLimitExceeded,
    MessageIndexResultReservationLimitExceeded,
    MessageIndexResultAllocationFailed,
    AmbiguousZeroChunkLimitExceeded,
    AmbiguousZeroMessageIndexShapeInvalid,
    AmbiguousZeroMessageIndexByteLimitExceeded,
    AmbiguousZeroEntryUpperLimitExceeded,
    AmbiguousZeroMessageIndexRangeLimitExceeded,
    AmbiguousZeroRetainedByteLimitExceeded,
    AmbiguousZeroStage1ReservationLimitExceeded,
    AmbiguousZeroStage1AllocationFailed,
    AmbiguousZeroIndexTimeMismatch,
    AmbiguousZeroObservedEntryUpperExceeded,
    AmbiguousZeroUnresolvedChunkLimitExceeded,
    AmbiguousZeroChunkRangeByteLimitExceeded,
    AmbiguousZeroCompressedByteLimitExceeded,
    AmbiguousZeroUncompressedByteLimitExceeded,
    AmbiguousZeroChunkRangeLimitExceeded,
    AmbiguousZeroScanByteLimitExceeded,
    AmbiguousZeroScanRecordLimitExceeded,
    AmbiguousZeroStage2ReservationLimitExceeded,
}

impl std::fmt::Display for IndexConsistencyViolation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidDataSectionEvidence => {
                "remote MCAP data-section boundary evidence is invalid"
            }
            Self::ConflictingChunkIndex => {
                "remote MCAP contains conflicting ChunkIndex descriptors"
            }
            Self::CanonicalChunkLimitExceeded => {
                "remote MCAP canonical ChunkIndex count exceeds its limit"
            }
            Self::ChunkRecordTooShort => {
                "remote MCAP ChunkIndex declares a Chunk shorter than one record envelope"
            }
            Self::ChunkRecordByteLimitExceeded => {
                "remote MCAP Chunk record length exceeds its limit"
            }
            Self::MessageIndexRegionByteLimitExceeded => {
                "remote MCAP MessageIndex region length exceeds its limit"
            }
            Self::CompressionByteLimitExceeded => {
                "remote MCAP Chunk compression identifier exceeds its limit"
            }
            Self::CompressedByteLimitExceeded => {
                "remote MCAP compressed Chunk size exceeds its limit"
            }
            Self::UncompressedByteLimitExceeded => {
                "remote MCAP uncompressed Chunk size exceeds its limit"
            }
            Self::DescriptorRetainedByteLimitExceeded => {
                "remote MCAP physical descriptor retention exceeds its limit"
            }
            Self::ArithmeticOverflow => "remote MCAP physical descriptor arithmetic overflowed",
            Self::MessageIndexPresenceMismatch => {
                "remote MCAP MessageIndex map and owning region disagree on emptiness"
            }
            Self::DuplicateMessageIndexOffset => {
                "remote MCAP ChunkIndex repeats a physical MessageIndex offset"
            }
            Self::MessageIndexOffsetOutsideOwningRegion => {
                "remote MCAP MessageIndex offset is outside its owning region"
            }
            Self::PhysicalRegionOutsideDataSection => {
                "remote MCAP indexed physical region is outside the data section"
            }
            Self::PhysicalRegionOverlap => "remote MCAP indexed physical regions overlap",
            Self::DescriptorReservationLimitExceeded => {
                "remote MCAP physical descriptor reservation exceeds its capacity"
            }
            Self::DescriptorAllocationFailed => "remote MCAP physical descriptor allocation failed",
            Self::UnknownCanonicalRegion => {
                "remote MCAP canonical physical region selection is invalid"
            }
            Self::MessageIndexRawReservationLimitExceeded => {
                "remote MCAP MessageIndex raw-input reservation exceeds its capacity"
            }
            Self::MessageIndexRawInputMismatch => {
                "remote MCAP MessageIndex raw input does not match its owning region"
            }
            Self::MessageIndexRecordEnvelopeInvalid => {
                "remote MCAP MessageIndex record envelope is invalid"
            }
            Self::MessageIndexRecordWrongOpcode => {
                "remote MCAP MessageIndex region contains a forbidden record"
            }
            Self::MessageIndexRecordOutOfBounds => {
                "remote MCAP MessageIndex record exceeds its owning region"
            }
            Self::MessageIndexRecordBodyInvalid => {
                "remote MCAP MessageIndex record body is invalid"
            }
            Self::MessageIndexRecordLimitExceeded => {
                "remote MCAP MessageIndex record count exceeds its limit"
            }
            Self::MessageIndexEntryLimitExceeded => {
                "remote MCAP MessageIndex record entry count exceeds its limit"
            }
            Self::MessageIndexAggregateEntryLimitExceeded => {
                "remote MCAP MessageIndex region entry count exceeds its limit"
            }
            Self::MessageIndexMappingMismatch => {
                "remote MCAP MessageIndex records and descriptor map are inconsistent"
            }
            Self::MessageIndexResultRetainedByteLimitExceeded => {
                "remote MCAP MessageIndex parsed result exceeds its retained-byte limit"
            }
            Self::MessageIndexResultReservationLimitExceeded => {
                "remote MCAP MessageIndex parsed-result reservation exceeds its capacity"
            }
            Self::MessageIndexResultAllocationFailed => {
                "remote MCAP MessageIndex parsed-result allocation failed"
            }
            Self::AmbiguousZeroChunkLimitExceeded => {
                "remote MCAP ambiguous-zero Chunk count exceeds its limit"
            }
            Self::AmbiguousZeroMessageIndexShapeInvalid => {
                "remote MCAP ambiguous-zero MessageIndex region cannot contain its declared records"
            }
            Self::AmbiguousZeroMessageIndexByteLimitExceeded => {
                "remote MCAP ambiguous-zero MessageIndex bytes exceed their opening limit"
            }
            Self::AmbiguousZeroEntryUpperLimitExceeded => {
                "remote MCAP ambiguous-zero MessageIndex entry upper bound exceeds its opening limit"
            }
            Self::AmbiguousZeroMessageIndexRangeLimitExceeded => {
                "remote MCAP ambiguous-zero MessageIndex Range count exceeds its opening limit"
            }
            Self::AmbiguousZeroRetainedByteLimitExceeded => {
                "remote MCAP ambiguous-zero classification retention exceeds its opening limit"
            }
            Self::AmbiguousZeroStage1ReservationLimitExceeded => {
                "remote MCAP ambiguous-zero first-stage reservation exceeds its capacity"
            }
            Self::AmbiguousZeroStage1AllocationFailed => {
                "remote MCAP ambiguous-zero first-stage allocation failed"
            }
            Self::AmbiguousZeroIndexTimeMismatch => {
                "remote MCAP ambiguous-zero MessageIndex contains a nonzero time"
            }
            Self::AmbiguousZeroObservedEntryUpperExceeded => {
                "remote MCAP ambiguous-zero MessageIndex exceeds its reserved entry upper bound"
            }
            Self::AmbiguousZeroUnresolvedChunkLimitExceeded => {
                "remote MCAP ambiguous-zero unresolved Chunk count exceeds its limit"
            }
            Self::AmbiguousZeroChunkRangeByteLimitExceeded => {
                "remote MCAP ambiguous-zero Chunk Range bytes exceed their opening limit"
            }
            Self::AmbiguousZeroCompressedByteLimitExceeded => {
                "remote MCAP ambiguous-zero compressed bytes exceed their opening limit"
            }
            Self::AmbiguousZeroUncompressedByteLimitExceeded => {
                "remote MCAP ambiguous-zero uncompressed bytes exceed their opening limit"
            }
            Self::AmbiguousZeroChunkRangeLimitExceeded => {
                "remote MCAP ambiguous-zero Chunk Range count exceeds its opening limit"
            }
            Self::AmbiguousZeroScanByteLimitExceeded => {
                "remote MCAP ambiguous-zero scan bytes exceed their opening limit"
            }
            Self::AmbiguousZeroScanRecordLimitExceeded => {
                "remote MCAP ambiguous-zero scan record upper bound exceeds its opening limit"
            }
            Self::AmbiguousZeroStage2ReservationLimitExceeded => {
                "remote MCAP ambiguous-zero second-stage reservation exceeds its capacity"
            }
        })
    }
}

impl std::error::Error for IndexConsistencyViolation {}

#[derive(Debug)]
struct StoredPhysicalUnit {
    source_record_index: usize,
    chunk_range: Range<u64>,
    message_index_region: Range<u64>,
    unit_range: Range<u64>,
}

/// Sealed descriptor-ownership evidence for all canonical indexed physical units.
///
/// This type is non-`Clone` and owns the complete MCAP-020 definition result, including the
/// materialization permit and exact fixed-layout bytes. It proves only descriptor ranges and
/// ownership. It does not prove that any map offset is aligned to a `MessageIndex` record envelope.
pub(crate) struct ValidatedPhysicalRegions<'a> {
    definitions: ValidatedSummaryDefinitions<'a>,
    data_records_range: Range<u64>,
    units: Vec<StoredPhysicalUnit>,
    reservation: PhysicalRegionReservation,
}

impl std::fmt::Debug for ValidatedPhysicalRegions<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ValidatedPhysicalRegions")
            .field("definitions", &"<sealed MCAP-020 definition evidence>")
            .field("data_records_range", &self.data_records_range)
            .field("canonical_units", &self.units.len())
            .field("reservation", &self.reservation)
            .finish_non_exhaustive()
    }
}

impl<'a> ValidatedPhysicalRegions<'a> {
    pub(crate) const fn definitions(&self) -> &ValidatedSummaryDefinitions<'a> {
        &self.definitions
    }

    pub(crate) fn data_records_range(&self) -> Range<u64> {
        self.data_records_range.clone()
    }

    pub(crate) fn canonical_chunk_count(&self) -> usize {
        self.units.len()
    }

    /// Checked lower-stage footprint retained while MCAP-025A is admitted.
    pub(crate) fn retained_bytes_v1(&self) -> Result<u64, IndexConsistencyViolation> {
        let units = (self.units.len() as u64)
            .checked_mul(size_of::<StoredPhysicalUnit>() as u64)
            .ok_or(IndexConsistencyViolation::ArithmeticOverflow)?;
        let descriptor = self.reservation.descriptor_retained_bytes;
        let materialized = self.definitions.materialized();
        let records = (self.definitions.source_record_count() as u64)
            .checked_mul(size_of::<BoundedSummaryRecord<'a>>() as u64)
            .ok_or(IndexConsistencyViolation::ArithmeticOverflow)?;
        let nested = materialized.nested_census();
        let nested_owned = nested
            .owned_string_bytes
            .checked_add(nested.schema_data_bytes)
            .and_then(|value| value.checked_add(nested.nested_retained_bytes))
            .ok_or(IndexConsistencyViolation::ArithmeticOverflow)?;
        let prepared = materialized.prepared();
        let borrowed_backing = u64::try_from(prepared.header_body().len())
            .ok()
            .and_then(|value| {
                u64::try_from(prepared.summary_bytes().len())
                    .ok()
                    .and_then(|summary| value.checked_add(summary))
            })
            .ok_or(IndexConsistencyViolation::ArithmeticOverflow)?;
        let owner_footprint = u64::try_from(
            size_of::<Self>()
                + size_of::<ValidatedSummaryDefinitions<'a>>()
                + size_of::<MaterializedSummaryRecords<'a>>()
                + size_of::<Arc<()>>()
                + size_of::<Mutex<()>>(),
        )
        .map_err(|_| IndexConsistencyViolation::ArithmeticOverflow)?;
        units
            .checked_add(descriptor)
            .and_then(|v| v.checked_add(records))
            .and_then(|v| v.checked_add(nested_owned))
            .and_then(|v| v.checked_add(borrowed_backing))
            .and_then(|v| v.checked_add(owner_footprint))
            .ok_or(IndexConsistencyViolation::ArithmeticOverflow)
    }

    pub(crate) fn regions(&self) -> impl ExactSizeIterator<Item = CanonicalPhysicalRegion<'_>> {
        self.units.iter().map(|unit| CanonicalPhysicalRegion {
            raw_descriptor: descriptor_at(
                self.definitions.materialized().records(),
                unit.source_record_index,
            ),
            chunk_range: unit.chunk_range.clone(),
            message_index_region: unit.message_index_region.clone(),
            unit_range: unit.unit_range.clone(),
        })
    }

    pub(crate) fn region(&self, canonical_ordinal: usize) -> Option<CanonicalPhysicalRegion<'_>> {
        self.units
            .get(canonical_ordinal)
            .map(|unit| CanonicalPhysicalRegion {
                raw_descriptor: descriptor_at(
                    self.definitions.materialized().records(),
                    unit.source_record_index,
                ),
                chunk_range: unit.chunk_range.clone(),
                message_index_region: unit.message_index_region.clone(),
                unit_range: unit.unit_range.clone(),
            })
    }
}

/// One canonical descriptor and its descriptor-owned physical ranges.
///
/// `raw_descriptor.message_index_offsets` have only owning-region validation. They are not a
/// per-Channel `MessageIndex` view and have no record-envelope alignment authority.
pub(crate) struct CanonicalPhysicalRegion<'a> {
    raw_descriptor: &'a mcap::records::ChunkIndex,
    chunk_range: Range<u64>,
    message_index_region: Range<u64>,
    unit_range: Range<u64>,
}

impl std::fmt::Debug for CanonicalPhysicalRegion<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CanonicalPhysicalRegion")
            .field("raw_descriptor", &"<bounded canonical ChunkIndex>")
            .field("chunk_range", &self.chunk_range)
            .field("message_index_region", &self.message_index_region)
            .field("unit_range", &self.unit_range)
            .finish_non_exhaustive()
    }
}

impl<'a> CanonicalPhysicalRegion<'a> {
    pub(crate) const fn raw_descriptor(&self) -> &'a mcap::records::ChunkIndex {
        self.raw_descriptor
    }

    pub(crate) fn chunk_range(&self) -> Range<u64> {
        self.chunk_range.clone()
    }

    pub(crate) fn message_index_region(&self) -> Range<u64> {
        self.message_index_region.clone()
    }

    pub(crate) fn unit_range(&self) -> Range<u64> {
        self.unit_range.clone()
    }
}

/// Validates descriptor identity, range ownership, and physical non-overlap without reading any
/// Chunk or `MessageIndex` region bytes.
pub(crate) fn validate_physical_regions<'a>(
    definitions: ValidatedSummaryDefinitions<'a>,
    budget: &PhysicalRegionBudget,
) -> Result<ValidatedPhysicalRegions<'a>, IndexConsistencyViolation> {
    let data_records_range = data_records_range(&definitions)?;
    let records = definitions.materialized().records();
    let limits = budget.limits();
    let mut canonical_chunks = 0_u64;

    // This first pass is allocation-free. Every source descriptor was already retained and
    // charged by MCAP-019, including exact duplicates.
    for (source_record_index, record) in records.iter().enumerate() {
        let BoundedSummaryRecord::Known(mcap::records::Record::ChunkIndex(descriptor)) = record
        else {
            continue;
        };
        if let Some(previous) =
            find_prior_descriptor(records, descriptor.chunk_start_offset, source_record_index)
        {
            if previous != descriptor {
                return Err(IndexConsistencyViolation::ConflictingChunkIndex);
            }
            continue;
        }
        canonical_chunks = checked_add(canonical_chunks, 1)?;
        if canonical_chunks > limits.max_canonical_chunks {
            return Err(IndexConsistencyViolation::CanonicalChunkLimitExceeded);
        }
        compute_physical_unit(source_record_index, descriptor, &data_records_range, limits)?;
    }

    let retained_bytes = checked_mul(
        canonical_chunks,
        u64::try_from(size_of::<StoredPhysicalUnit>())
            .map_err(|_overflow| IndexConsistencyViolation::ArithmeticOverflow)?,
    )?;
    if retained_bytes > limits.max_descriptor_retained_bytes {
        return Err(IndexConsistencyViolation::DescriptorRetainedByteLimitExceeded);
    }
    let reservation = budget.try_reserve(canonical_chunks, retained_bytes)?;
    let canonical_chunks_usize = usize::try_from(canonical_chunks)
        .map_err(|_overflow| IndexConsistencyViolation::ArithmeticOverflow)?;
    let mut units = Vec::new();
    units
        .try_reserve_exact(canonical_chunks_usize)
        .map_err(|_error| IndexConsistencyViolation::DescriptorAllocationFailed)?;

    for (source_record_index, record) in records.iter().enumerate() {
        let BoundedSummaryRecord::Known(mcap::records::Record::ChunkIndex(descriptor)) = record
        else {
            continue;
        };
        if find_prior_descriptor(records, descriptor.chunk_start_offset, source_record_index)
            .is_none()
        {
            units.push(compute_physical_unit(
                source_record_index,
                descriptor,
                &data_records_range,
                limits,
            )?);
        }
    }
    if units.len() != canonical_chunks_usize {
        return Err(IndexConsistencyViolation::ArithmeticOverflow);
    }

    units.sort_unstable_by_key(|unit| unit.unit_range.start);
    for adjacent in units.windows(2) {
        if adjacent[0].unit_range.end > adjacent[1].unit_range.start {
            return Err(IndexConsistencyViolation::PhysicalRegionOverlap);
        }
    }

    Ok(ValidatedPhysicalRegions {
        definitions,
        data_records_range,
        units,
        reservation,
    })
}

fn data_records_range(
    definitions: &ValidatedSummaryDefinitions<'_>,
) -> Result<Range<u64>, IndexConsistencyViolation> {
    let fixed_layout = definitions.materialized().prepared().fixed_layout();
    let data_start = fixed_layout.header_body_range().end;
    let data_end_record_len = u64::try_from(super::RECORD_ENVELOPE_LEN + DATA_END_BODY_LEN)
        .map_err(|_overflow| IndexConsistencyViolation::InvalidDataSectionEvidence)?;
    let data_end = fixed_layout
        .summary_range()
        .start
        .checked_sub(data_end_record_len)
        .ok_or(IndexConsistencyViolation::InvalidDataSectionEvidence)?;
    if data_start > data_end {
        return Err(IndexConsistencyViolation::InvalidDataSectionEvidence);
    }
    Ok(data_start..data_end)
}

fn compute_physical_unit(
    source_record_index: usize,
    descriptor: &mcap::records::ChunkIndex,
    data_records_range: &Range<u64>,
    limits: &PhysicalRegionLimits,
) -> Result<StoredPhysicalUnit, IndexConsistencyViolation> {
    let record_envelope_len = u64::try_from(super::RECORD_ENVELOPE_LEN)
        .map_err(|_overflow| IndexConsistencyViolation::ArithmeticOverflow)?;
    if descriptor.chunk_length < record_envelope_len {
        return Err(IndexConsistencyViolation::ChunkRecordTooShort);
    }
    if descriptor.chunk_length > limits.max_chunk_record_bytes {
        return Err(IndexConsistencyViolation::ChunkRecordByteLimitExceeded);
    }
    if descriptor.message_index_length > limits.max_message_index_region_bytes {
        return Err(IndexConsistencyViolation::MessageIndexRegionByteLimitExceeded);
    }
    let compression_bytes = u64::try_from(descriptor.compression.len())
        .map_err(|_overflow| IndexConsistencyViolation::ArithmeticOverflow)?;
    if compression_bytes > limits.max_compression_bytes {
        return Err(IndexConsistencyViolation::CompressionByteLimitExceeded);
    }
    if descriptor.compressed_size > limits.max_compressed_bytes {
        return Err(IndexConsistencyViolation::CompressedByteLimitExceeded);
    }
    if descriptor.uncompressed_size > limits.max_uncompressed_bytes {
        return Err(IndexConsistencyViolation::UncompressedByteLimitExceeded);
    }

    let chunk_end = descriptor
        .chunk_start_offset
        .checked_add(descriptor.chunk_length)
        .ok_or(IndexConsistencyViolation::ArithmeticOverflow)?;
    let message_index_end = chunk_end
        .checked_add(descriptor.message_index_length)
        .ok_or(IndexConsistencyViolation::ArithmeticOverflow)?;
    let chunk_range = descriptor.chunk_start_offset..chunk_end;
    let message_index_region = chunk_end..message_index_end;
    let unit_range = descriptor.chunk_start_offset..message_index_end;

    if !range_within(&chunk_range, data_records_range)
        || !range_within(&message_index_region, data_records_range)
    {
        return Err(IndexConsistencyViolation::PhysicalRegionOutsideDataSection);
    }

    let has_offsets = !descriptor.message_index_offsets.is_empty();
    let has_region = descriptor.message_index_length != 0;
    if has_offsets != has_region {
        return Err(IndexConsistencyViolation::MessageIndexPresenceMismatch);
    }
    for (position, offset) in descriptor.message_index_offsets.values().enumerate() {
        if !message_index_region.contains(offset) {
            return Err(IndexConsistencyViolation::MessageIndexOffsetOutsideOwningRegion);
        }
        if descriptor
            .message_index_offsets
            .values()
            .take(position)
            .any(|previous| previous == offset)
        {
            return Err(IndexConsistencyViolation::DuplicateMessageIndexOffset);
        }
    }

    Ok(StoredPhysicalUnit {
        source_record_index,
        chunk_range,
        message_index_region,
        unit_range,
    })
}

fn find_prior_descriptor<'records>(
    records: &'records [BoundedSummaryRecord<'_>],
    chunk_start_offset: u64,
    before: usize,
) -> Option<&'records mcap::records::ChunkIndex> {
    records
        .get(..before)?
        .iter()
        .find_map(|record| match record {
            BoundedSummaryRecord::Known(mcap::records::Record::ChunkIndex(descriptor))
                if descriptor.chunk_start_offset == chunk_start_offset =>
            {
                Some(descriptor)
            }
            _ => None,
        })
}

fn descriptor_at<'records>(
    records: &'records [BoundedSummaryRecord<'_>],
    source_record_index: usize,
) -> &'records mcap::records::ChunkIndex {
    let Some(BoundedSummaryRecord::Known(mcap::records::Record::ChunkIndex(descriptor))) =
        records.get(source_record_index)
    else {
        unreachable!("physical-region descriptor index retains its immutable Summary record");
    };
    descriptor
}

const fn range_within(inner: &Range<u64>, outer: &Range<u64>) -> bool {
    inner.start >= outer.start && inner.end <= outer.end
}

fn checked_add(left: u64, right: u64) -> Result<u64, IndexConsistencyViolation> {
    left.checked_add(right)
        .ok_or(IndexConsistencyViolation::ArithmeticOverflow)
}

fn checked_mul(left: u64, right: u64) -> Result<u64, IndexConsistencyViolation> {
    left.checked_mul(right)
        .ok_or(IndexConsistencyViolation::ArithmeticOverflow)
}

impl PhysicalRegionLimits {
    #[cfg(any(test, rerun_mcap_phase_a_proof_v1))]
    pub(crate) const fn for_phase_a_measurement_v1(
        max_canonical_chunks: u64,
        max_chunk_record_bytes: u64,
        max_message_index_region_bytes: u64,
        max_compression_bytes: u64,
        max_compressed_bytes: u64,
        max_uncompressed_bytes: u64,
        max_descriptor_retained_bytes: u64,
    ) -> Self {
        Self {
            max_canonical_chunks,
            max_chunk_record_bytes,
            max_message_index_region_bytes,
            max_compression_bytes,
            max_compressed_bytes,
            max_uncompressed_bytes,
            max_descriptor_retained_bytes,
        }
    }

    #[cfg(test)]
    pub(crate) const fn for_test(
        max_canonical_chunks: u64,
        max_chunk_record_bytes: u64,
        max_message_index_region_bytes: u64,
        max_compression_bytes: u64,
        max_compressed_bytes: u64,
        max_uncompressed_bytes: u64,
        max_descriptor_retained_bytes: u64,
    ) -> Self {
        Self::for_phase_a_measurement_v1(
            max_canonical_chunks,
            max_chunk_record_bytes,
            max_message_index_region_bytes,
            max_compression_bytes,
            max_compressed_bytes,
            max_uncompressed_bytes,
            max_descriptor_retained_bytes,
        )
    }
}

impl PhysicalRegionBudget {
    #[cfg(any(test, rerun_mcap_phase_a_proof_v1))]
    pub(crate) fn for_phase_a_measurement_v1(
        limits: PhysicalRegionLimits,
        max_active_reservations: u64,
        max_aggregate_canonical_chunks: u64,
        max_aggregate_descriptor_retained_bytes: u64,
    ) -> Self {
        Self {
            state: Arc::new(PhysicalRegionBudgetState {
                limits,
                max_active_reservations,
                max_aggregate_canonical_chunks,
                max_aggregate_descriptor_retained_bytes,
                usage: Mutex::new(PhysicalRegionBudgetUsage::default()),
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        limits: PhysicalRegionLimits,
        max_active_reservations: u64,
        max_aggregate_canonical_chunks: u64,
        max_aggregate_descriptor_retained_bytes: u64,
    ) -> Self {
        Self::for_phase_a_measurement_v1(
            limits,
            max_active_reservations,
            max_aggregate_canonical_chunks,
            max_aggregate_descriptor_retained_bytes,
        )
    }

    #[cfg(test)]
    pub(crate) fn usage_for_test(&self) -> (u64, u64, u64) {
        let usage = *self.state.usage.lock();
        (
            usage.active_reservations,
            usage.canonical_chunks,
            usage.descriptor_retained_bytes,
        )
    }
}
