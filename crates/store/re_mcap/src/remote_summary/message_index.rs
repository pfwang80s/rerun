//! Bounded, exact-sequence parsing for one descriptor-owned `MessageIndex` region.

use std::ops::Range;
use std::sync::Arc;

use parking_lot::Mutex;

use super::materialization::{
    MessageIndexPreflightLimits, SummaryMaterializationError, preflight_message_index_records,
};
use super::physical_regions::{IndexConsistencyViolation, ValidatedPhysicalRegions};

const RECORD_ENVELOPE_LEN: usize = super::RECORD_ENVELOPE_LEN;
const MESSAGE_INDEX_PREFIX_LEN: usize = size_of::<u16>() + size_of::<u32>();
const MESSAGE_INDEX_ENTRY_LEN: usize = size_of::<u64>() * 2;

/// Fixed limits for one non-preemptible `MessageIndex` region parse.
///
/// There is deliberately no production constructor while remote MCAP remains disarmed.
#[derive(Clone, Copy, Debug)]
pub(crate) struct MessageIndexRegionLimits {
    max_records_per_region: u64,
    nested: MessageIndexPreflightLimits,
    max_result_retained_bytes: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct MessageIndexRegionBudgetUsage {
    active_raw_inputs: u64,
    raw_input_bytes: u64,
    active_results: u64,
    result_records: u64,
    result_entries: u64,
    result_retained_bytes: u64,
}

#[derive(Clone, Copy, Debug)]
struct MessageIndexRegionBudgetCapacity {
    max_active_raw_inputs: u64,
    max_raw_input_bytes: u64,
    max_active_results: u64,
    max_result_records: u64,
    max_result_entries: u64,
    max_result_retained_bytes: u64,
}

struct MessageIndexRegionBudgetState {
    limits: MessageIndexRegionLimits,
    capacity: MessageIndexRegionBudgetCapacity,
    usage: Mutex<MessageIndexRegionBudgetUsage>,
}

/// Aggregate owner for queued raw-region and immutable parsed-result memory.
///
/// Its constructor remains test-only until the production remote-MCAP profile is sealed.
pub(crate) struct MessageIndexRegionBudget {
    state: Arc<MessageIndexRegionBudgetState>,
}

impl std::fmt::Debug for MessageIndexRegionBudget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MessageIndexRegionBudget")
            .field("limits", &self.state.limits)
            .field("capacity", &self.state.capacity)
            .finish_non_exhaustive()
    }
}

impl MessageIndexRegionBudget {
    fn try_reserve_raw(
        &self,
        raw_input_bytes: u64,
    ) -> Result<MessageIndexRawReservation, IndexConsistencyViolation> {
        let mut usage = self.state.usage.lock();
        let next = MessageIndexRegionBudgetUsage {
            active_raw_inputs: checked_add(usage.active_raw_inputs, 1)?,
            raw_input_bytes: checked_add(usage.raw_input_bytes, raw_input_bytes)?,
            ..*usage
        };
        if next.active_raw_inputs > self.state.capacity.max_active_raw_inputs
            || next.raw_input_bytes > self.state.capacity.max_raw_input_bytes
        {
            return Err(IndexConsistencyViolation::MessageIndexRawReservationLimitExceeded);
        }
        *usage = next;
        drop(usage);

        Ok(MessageIndexRawReservation {
            state: Arc::clone(&self.state),
            raw_input_bytes,
        })
    }
}

impl MessageIndexRegionBudgetState {
    fn try_reserve_result(
        self: &Arc<Self>,
        census: MessageIndexRegionCensus,
    ) -> Result<MessageIndexResultReservation, IndexConsistencyViolation> {
        let mut usage = self.usage.lock();
        let next = MessageIndexRegionBudgetUsage {
            active_results: checked_add(usage.active_results, 1)?,
            result_records: checked_add(usage.result_records, census.records)?,
            result_entries: checked_add(usage.result_entries, census.entries)?,
            result_retained_bytes: checked_add(usage.result_retained_bytes, census.retained_bytes)?,
            ..*usage
        };
        if next.active_results > self.capacity.max_active_results
            || next.result_records > self.capacity.max_result_records
            || next.result_entries > self.capacity.max_result_entries
            || next.result_retained_bytes > self.capacity.max_result_retained_bytes
        {
            return Err(IndexConsistencyViolation::MessageIndexResultReservationLimitExceeded);
        }
        *usage = next;
        drop(usage);

        Ok(MessageIndexResultReservation {
            state: Arc::clone(self),
            census,
        })
    }
}

/// Move-only ownership of one exact queued raw-region claim.
struct MessageIndexRawReservation {
    state: Arc<MessageIndexRegionBudgetState>,
    raw_input_bytes: u64,
}

impl Drop for MessageIndexRawReservation {
    fn drop(&mut self) {
        let mut usage = self.state.usage.lock();
        usage.active_raw_inputs = usage
            .active_raw_inputs
            .checked_sub(1)
            .expect("live MessageIndex raw input owns one active queue claim");
        usage.raw_input_bytes = usage
            .raw_input_bytes
            .checked_sub(self.raw_input_bytes)
            .expect("live MessageIndex raw input owns its exact byte claim");
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct MessageIndexRegionCensus {
    records: u64,
    entries: u64,
    retained_bytes: u64,
}

/// Move-only ownership of one immutable parsed-region allocation.
struct MessageIndexResultReservation {
    state: Arc<MessageIndexRegionBudgetState>,
    census: MessageIndexRegionCensus,
}

impl Drop for MessageIndexResultReservation {
    fn drop(&mut self) {
        let mut usage = self.state.usage.lock();
        usage.active_results = usage
            .active_results
            .checked_sub(1)
            .expect("live MessageIndex result owns one active result claim");
        usage.result_records = usage
            .result_records
            .checked_sub(self.census.records)
            .expect("live MessageIndex result owns its record claim");
        usage.result_entries = usage
            .result_entries
            .checked_sub(self.census.entries)
            .expect("live MessageIndex result owns its entry claim");
        usage.result_retained_bytes = usage
            .result_retained_bytes
            .checked_sub(self.census.retained_bytes)
            .expect("live MessageIndex result owns its retained-byte claim");
    }
}

/// Private authority to allocate the immutable view for one fully preflighted exact region.
struct MessageIndexMaterializationToken {
    census: MessageIndexRegionCensus,
    reservation: MessageIndexResultReservation,
}

/// Prepared ownership for one exact region before its Range result is installed.
///
/// Construction acquires queue capacity before the caller starts or installs external work.
pub(crate) struct PreparedMessageIndexRegionParse<'a> {
    physical: ValidatedPhysicalRegions<'a>,
    canonical_ordinal: usize,
    expected_range: Range<u64>,
    raw_reservation: MessageIndexRawReservation,
}

impl std::fmt::Debug for PreparedMessageIndexRegionParse<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedMessageIndexRegionParse")
            .field("canonical_ordinal", &self.canonical_ordinal)
            .field("expected_range", &self.expected_range)
            .finish_non_exhaustive()
    }
}

impl<'a> PreparedMessageIndexRegionParse<'a> {
    /// Installs a stable backing owner without copying its bytes.
    pub(crate) fn install_raw(
        self,
        file_start: u64,
        bytes: Box<[u8]>,
    ) -> Result<MessageIndexRegionParse<'a>, IndexConsistencyViolation> {
        let byte_len = u64::try_from(bytes.len())
            .map_err(|_overflow| IndexConsistencyViolation::ArithmeticOverflow)?;
        let actual_end = file_start
            .checked_add(byte_len)
            .ok_or(IndexConsistencyViolation::ArithmeticOverflow)?;
        if file_start != self.expected_range.start || actual_end != self.expected_range.end {
            return Err(IndexConsistencyViolation::MessageIndexRawInputMismatch);
        }
        Ok(MessageIndexRegionParse {
            physical: self.physical,
            canonical_ordinal: self.canonical_ordinal,
            expected_range: self.expected_range,
            raw: MessageIndexRawInput {
                bytes: Some(bytes),
                reservation: Some(self.raw_reservation),
            },
        })
    }
}

/// Exact raw bytes coupled to their queue reservation.
///
/// Field order and explicit `Drop` ensure the backing owner is released before its permit.
struct MessageIndexRawInput {
    bytes: Option<Box<[u8]>>,
    reservation: Option<MessageIndexRawReservation>,
}

impl MessageIndexRawInput {
    fn budget_state(
        &self,
    ) -> Result<&Arc<MessageIndexRegionBudgetState>, IndexConsistencyViolation> {
        self.reservation
            .as_ref()
            .map(|reservation| &reservation.state)
            .ok_or(IndexConsistencyViolation::MessageIndexRawInputMismatch)
    }

    fn as_slice(&self) -> Result<&[u8], IndexConsistencyViolation> {
        self.bytes
            .as_deref()
            .ok_or(IndexConsistencyViolation::MessageIndexRawInputMismatch)
    }
}

impl Drop for MessageIndexRawInput {
    fn drop(&mut self) {
        drop(self.bytes.take());
        drop(self.reservation.take());
    }
}

/// A single non-preemptible CPU work unit for one owning `MessageIndex` region.
///
/// The type owns the complete MCAP-021 capability, one exact raw backing owner, and its queue
/// reservation. One call to [`Self::execute`] can parse only this one region.
pub(crate) struct MessageIndexRegionParse<'a> {
    physical: ValidatedPhysicalRegions<'a>,
    canonical_ordinal: usize,
    expected_range: Range<u64>,
    raw: MessageIndexRawInput,
}

impl std::fmt::Debug for MessageIndexRegionParse<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MessageIndexRegionParse")
            .field("canonical_ordinal", &self.canonical_ordinal)
            .field("expected_range", &self.expected_range)
            .finish_non_exhaustive()
    }
}

impl<'a> MessageIndexRegionParse<'a> {
    pub(crate) fn execute(
        self,
    ) -> Result<ValidatedMessageIndexRegion<'a>, IndexConsistencyViolation> {
        let Self {
            physical,
            canonical_ordinal,
            expected_range,
            raw,
        } = self;
        let descriptor = descriptor_for(&physical, canonical_ordinal)?;
        let bytes = raw.as_slice()?;
        let budget_state = Arc::clone(raw.budget_state()?);
        let census = preflight_region(
            bytes,
            expected_range.start,
            descriptor,
            &budget_state.limits,
        )?;
        let materialization = MessageIndexMaterializationToken {
            census,
            reservation: budget_state.try_reserve_result(census)?,
        };
        let (channels, result_reservation) =
            materialize_region(bytes, expected_range.start, descriptor, materialization)?;
        drop(raw);

        Ok(ValidatedMessageIndexRegion {
            channels,
            physical,
            canonical_ordinal,
            expected_range,
            result_reservation,
        })
    }

    #[cfg(test)]
    pub(crate) fn raw_identity_for_test(&self) -> (*const u8, usize) {
        let bytes = self
            .raw
            .bytes
            .as_deref()
            .expect("a pending MessageIndex work unit owns its exact raw box");
        (bytes.as_ptr(), bytes.len())
    }
}

/// One immutable per-Channel view whose record-start mapping has been validated bidirectionally.
pub(crate) struct ValidatedChannelMessageIndex {
    channel_id: u16,
    absolute_record_start: u64,
    entries: Vec<mcap::records::MessageIndexEntry>,
}

impl std::fmt::Debug for ValidatedChannelMessageIndex {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ValidatedChannelMessageIndex")
            .field("channel_id", &"<bounded Channel ID>")
            .field("entries", &self.entries.len())
            .finish_non_exhaustive()
    }
}

impl ValidatedChannelMessageIndex {
    pub(crate) const fn channel_id(&self) -> u16 {
        self.channel_id
    }

    pub(crate) const fn absolute_record_start(&self) -> u64 {
        self.absolute_record_start
    }

    pub(crate) fn entries(&self) -> &[mcap::records::MessageIndexEntry] {
        &self.entries
    }
}

/// Sealed result for exactly one fully consumed owning `MessageIndex` region.
///
/// It retains the complete MCAP-019/020/021 ownership chain and immutable-result reservation.
/// The successfully consumed raw backing and queue permit are released before this result exists.
/// It carries no time-canonicalization or planner claim.
pub(crate) struct ValidatedMessageIndexRegion<'a> {
    channels: Vec<ValidatedChannelMessageIndex>,
    physical: ValidatedPhysicalRegions<'a>,
    canonical_ordinal: usize,
    expected_range: Range<u64>,
    result_reservation: MessageIndexResultReservation,
}

impl std::fmt::Debug for ValidatedMessageIndexRegion<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ValidatedMessageIndexRegion")
            .field("canonical_ordinal", &self.canonical_ordinal)
            .field("expected_range", &self.expected_range)
            .field("channels", &self.channels.len())
            .finish_non_exhaustive()
    }
}

impl<'a> ValidatedMessageIndexRegion<'a> {
    pub(crate) fn channels(&self) -> &[ValidatedChannelMessageIndex] {
        &self.channels
    }

    pub(crate) const fn physical(&self) -> &ValidatedPhysicalRegions<'a> {
        &self.physical
    }

    pub(crate) fn into_physical(self) -> ValidatedPhysicalRegions<'a> {
        let Self {
            channels,
            physical,
            result_reservation,
            ..
        } = self;
        drop(channels);
        drop(result_reservation);
        physical
    }
}

/// Claims the exact queue/raw capacity for one canonical owning region before bytes are installed.
pub(crate) fn prepare_message_index_region<'a>(
    physical: ValidatedPhysicalRegions<'a>,
    canonical_ordinal: usize,
    budget: &MessageIndexRegionBudget,
) -> Result<PreparedMessageIndexRegionParse<'a>, IndexConsistencyViolation> {
    let expected_range = physical
        .regions()
        .nth(canonical_ordinal)
        .ok_or(IndexConsistencyViolation::UnknownCanonicalRegion)?
        .message_index_region();
    let raw_bytes = expected_range
        .end
        .checked_sub(expected_range.start)
        .ok_or(IndexConsistencyViolation::ArithmeticOverflow)?;
    let raw_reservation = budget.try_reserve_raw(raw_bytes)?;

    Ok(PreparedMessageIndexRegionParse {
        physical,
        canonical_ordinal,
        expected_range,
        raw_reservation,
    })
}

fn descriptor_for<'physical>(
    physical: &'physical ValidatedPhysicalRegions<'_>,
    canonical_ordinal: usize,
) -> Result<&'physical mcap::records::ChunkIndex, IndexConsistencyViolation> {
    physical
        .regions()
        .nth(canonical_ordinal)
        .map(|region| region.raw_descriptor())
        .ok_or(IndexConsistencyViolation::UnknownCanonicalRegion)
}

fn preflight_region(
    bytes: &[u8],
    absolute_start: u64,
    descriptor: &mcap::records::ChunkIndex,
    limits: &MessageIndexRegionLimits,
) -> Result<MessageIndexRegionCensus, IndexConsistencyViolation> {
    let mut cursor = 0_usize;
    let mut records = 0_u64;
    let mut entries = 0_u64;
    while cursor < bytes.len() {
        let envelope = next_envelope(bytes, cursor)?;
        records = checked_add(records, 1)?;
        if records > limits.max_records_per_region {
            return Err(IndexConsistencyViolation::MessageIndexRecordLimitExceeded);
        }
        if envelope.opcode != mcap::records::op::MESSAGE_INDEX {
            return Err(IndexConsistencyViolation::MessageIndexRecordWrongOpcode);
        }
        let preflight = preflight_message_index_records(
            &bytes[envelope.body_range.clone()],
            entries,
            &limits.nested,
        )
        .map_err(map_nested_error)?;
        let record_start = checked_add(
            absolute_start,
            u64::try_from(envelope.start)
                .map_err(|_overflow| IndexConsistencyViolation::ArithmeticOverflow)?,
        )?;
        if descriptor.message_index_offsets.get(&preflight.channel_id) != Some(&record_start) {
            return Err(IndexConsistencyViolation::MessageIndexMappingMismatch);
        }
        entries = preflight.aggregate_entries;
        cursor = envelope.end;
    }

    let descriptor_records = u64::try_from(descriptor.message_index_offsets.len())
        .map_err(|_overflow| IndexConsistencyViolation::ArithmeticOverflow)?;
    if records != descriptor_records {
        return Err(IndexConsistencyViolation::MessageIndexMappingMismatch);
    }
    let retained_bytes = result_retained_bytes(records, entries)?;
    if retained_bytes > limits.max_result_retained_bytes {
        return Err(IndexConsistencyViolation::MessageIndexResultRetainedByteLimitExceeded);
    }

    Ok(MessageIndexRegionCensus {
        records,
        entries,
        retained_bytes,
    })
}

fn materialize_region(
    bytes: &[u8],
    absolute_start: u64,
    descriptor: &mcap::records::ChunkIndex,
    materialization: MessageIndexMaterializationToken,
) -> Result<
    (
        Vec<ValidatedChannelMessageIndex>,
        MessageIndexResultReservation,
    ),
    IndexConsistencyViolation,
> {
    let MessageIndexMaterializationToken {
        census,
        reservation,
    } = materialization;
    let record_capacity = usize::try_from(census.records)
        .map_err(|_overflow| IndexConsistencyViolation::ArithmeticOverflow)?;
    let mut channels = Vec::new();
    channels
        .try_reserve_exact(record_capacity)
        .map_err(|_error| IndexConsistencyViolation::MessageIndexResultAllocationFailed)?;

    let mut cursor = 0_usize;
    while cursor < bytes.len() {
        let envelope = next_envelope(bytes, cursor)?;
        let body = &bytes[envelope.body_range.clone()];
        let channel_id = read_u16(body, 0)?;
        let encoded_bytes = usize::try_from(read_u32(body, size_of::<u16>())?)
            .map_err(|_overflow| IndexConsistencyViolation::ArithmeticOverflow)?;
        let entry_count = encoded_bytes / MESSAGE_INDEX_ENTRY_LEN;
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(entry_count)
            .map_err(|_error| IndexConsistencyViolation::MessageIndexResultAllocationFailed)?;
        let mut entry_cursor = MESSAGE_INDEX_PREFIX_LEN;
        for _ in 0..entry_count {
            entries.push(mcap::records::MessageIndexEntry {
                log_time: read_u64(body, entry_cursor)?,
                offset: read_u64(body, entry_cursor + size_of::<u64>())?,
            });
            entry_cursor += MESSAGE_INDEX_ENTRY_LEN;
        }
        if entry_cursor != body.len() {
            return Err(IndexConsistencyViolation::MessageIndexRecordBodyInvalid);
        }
        let record_start = checked_add(
            absolute_start,
            u64::try_from(envelope.start)
                .map_err(|_overflow| IndexConsistencyViolation::ArithmeticOverflow)?,
        )?;
        if descriptor.message_index_offsets.get(&channel_id) != Some(&record_start) {
            return Err(IndexConsistencyViolation::MessageIndexMappingMismatch);
        }
        channels.push(ValidatedChannelMessageIndex {
            channel_id,
            absolute_record_start: record_start,
            entries,
        });
        cursor = envelope.end;
    }
    if channels.len() != record_capacity {
        return Err(IndexConsistencyViolation::MessageIndexMappingMismatch);
    }
    Ok((channels, reservation))
}

#[derive(Clone, Debug)]
struct RecordEnvelope {
    opcode: u8,
    start: usize,
    body_range: Range<usize>,
    end: usize,
}

fn next_envelope(bytes: &[u8], start: usize) -> Result<RecordEnvelope, IndexConsistencyViolation> {
    let envelope_end = start
        .checked_add(RECORD_ENVELOPE_LEN)
        .ok_or(IndexConsistencyViolation::ArithmeticOverflow)?;
    let envelope = bytes
        .get(start..envelope_end)
        .ok_or(IndexConsistencyViolation::MessageIndexRecordEnvelopeInvalid)?;
    let body_len = usize::try_from(u64::from_le_bytes(
        envelope[1..RECORD_ENVELOPE_LEN]
            .try_into()
            .map_err(|_error| IndexConsistencyViolation::MessageIndexRecordEnvelopeInvalid)?,
    ))
    .map_err(|_overflow| IndexConsistencyViolation::MessageIndexRecordOutOfBounds)?;
    let end = envelope_end
        .checked_add(body_len)
        .ok_or(IndexConsistencyViolation::MessageIndexRecordOutOfBounds)?;
    if end > bytes.len() {
        return Err(IndexConsistencyViolation::MessageIndexRecordOutOfBounds);
    }
    Ok(RecordEnvelope {
        opcode: envelope[0],
        start,
        body_range: envelope_end..end,
        end,
    })
}

fn result_retained_bytes(records: u64, entries: u64) -> Result<u64, IndexConsistencyViolation> {
    checked_add(
        checked_mul(
            records,
            u64::try_from(size_of::<ValidatedChannelMessageIndex>())
                .map_err(|_overflow| IndexConsistencyViolation::ArithmeticOverflow)?,
        )?,
        checked_mul(
            entries,
            u64::try_from(size_of::<mcap::records::MessageIndexEntry>())
                .map_err(|_overflow| IndexConsistencyViolation::ArithmeticOverflow)?,
        )?,
    )
}

fn map_nested_error(error: SummaryMaterializationError) -> IndexConsistencyViolation {
    match error {
        SummaryMaterializationError::ArithmeticOverflow => {
            IndexConsistencyViolation::ArithmeticOverflow
        }
        SummaryMaterializationError::MessageIndexEntryLimitExceeded => {
            IndexConsistencyViolation::MessageIndexEntryLimitExceeded
        }
        SummaryMaterializationError::MessageIndexAggregateEntryLimitExceeded => {
            IndexConsistencyViolation::MessageIndexAggregateEntryLimitExceeded
        }
        SummaryMaterializationError::RecordStructureInvalid
        | SummaryMaterializationError::RecordTrailingBytes
        | SummaryMaterializationError::NestedEncodedWidthInvalid
        | SummaryMaterializationError::StringInvalidUtf8
        | SummaryMaterializationError::ChannelMetadataEntryLimitExceeded
        | SummaryMaterializationError::ChannelMetadataKeyBytesLimitExceeded
        | SummaryMaterializationError::ChannelMetadataValueBytesLimitExceeded
        | SummaryMaterializationError::ChunkIndexMessageIndexOffsetLimitExceeded
        | SummaryMaterializationError::StatisticsChannelMessageCountLimitExceeded
        | SummaryMaterializationError::NestedRetainedBytesLimitExceeded
        | SummaryMaterializationError::DuplicateNestedKey
        | SummaryMaterializationError::PreparedCensusMismatch
        | SummaryMaterializationError::PreparedRecordRejected
        | SummaryMaterializationError::MaterializationReservationLimitExceeded => {
            IndexConsistencyViolation::MessageIndexRecordBodyInvalid
        }
    }
}

fn read_u16(bytes: &[u8], start: usize) -> Result<u16, IndexConsistencyViolation> {
    let end = start
        .checked_add(size_of::<u16>())
        .ok_or(IndexConsistencyViolation::ArithmeticOverflow)?;
    Ok(u16::from_le_bytes(
        bytes
            .get(start..end)
            .ok_or(IndexConsistencyViolation::MessageIndexRecordBodyInvalid)?
            .try_into()
            .map_err(|_error| IndexConsistencyViolation::MessageIndexRecordBodyInvalid)?,
    ))
}

fn read_u32(bytes: &[u8], start: usize) -> Result<u32, IndexConsistencyViolation> {
    let end = start
        .checked_add(size_of::<u32>())
        .ok_or(IndexConsistencyViolation::ArithmeticOverflow)?;
    Ok(u32::from_le_bytes(
        bytes
            .get(start..end)
            .ok_or(IndexConsistencyViolation::MessageIndexRecordBodyInvalid)?
            .try_into()
            .map_err(|_error| IndexConsistencyViolation::MessageIndexRecordBodyInvalid)?,
    ))
}

fn read_u64(bytes: &[u8], start: usize) -> Result<u64, IndexConsistencyViolation> {
    let end = start
        .checked_add(size_of::<u64>())
        .ok_or(IndexConsistencyViolation::ArithmeticOverflow)?;
    Ok(u64::from_le_bytes(
        bytes
            .get(start..end)
            .ok_or(IndexConsistencyViolation::MessageIndexRecordBodyInvalid)?
            .try_into()
            .map_err(|_error| IndexConsistencyViolation::MessageIndexRecordBodyInvalid)?,
    ))
}

fn checked_add(left: u64, right: u64) -> Result<u64, IndexConsistencyViolation> {
    left.checked_add(right)
        .ok_or(IndexConsistencyViolation::ArithmeticOverflow)
}

fn checked_mul(left: u64, right: u64) -> Result<u64, IndexConsistencyViolation> {
    left.checked_mul(right)
        .ok_or(IndexConsistencyViolation::ArithmeticOverflow)
}

#[cfg(test)]
impl MessageIndexRegionLimits {
    pub(crate) const fn for_test(
        max_records_per_region: u64,
        max_entries_per_record: u64,
        max_aggregate_entries: u64,
        max_result_retained_bytes: u64,
    ) -> Self {
        Self {
            max_records_per_region,
            nested: MessageIndexPreflightLimits::for_test(
                max_entries_per_record,
                max_aggregate_entries,
            ),
            max_result_retained_bytes,
        }
    }
}

#[cfg(test)]
impl MessageIndexRegionBudget {
    pub(crate) fn for_test(
        limits: MessageIndexRegionLimits,
        max_active_raw_inputs: u64,
        max_raw_input_bytes: u64,
        max_active_results: u64,
        max_result_records: u64,
        max_result_entries: u64,
        max_result_retained_bytes: u64,
    ) -> Self {
        Self {
            state: Arc::new(MessageIndexRegionBudgetState {
                limits,
                capacity: MessageIndexRegionBudgetCapacity {
                    max_active_raw_inputs,
                    max_raw_input_bytes,
                    max_active_results,
                    max_result_records,
                    max_result_entries,
                    max_result_retained_bytes,
                },
                usage: Mutex::new(MessageIndexRegionBudgetUsage::default()),
            }),
        }
    }

    pub(crate) fn usage_for_test(&self) -> (u64, u64, u64, u64, u64, u64) {
        let usage = *self.state.usage.lock();
        (
            usage.active_raw_inputs,
            usage.raw_input_bytes,
            usage.active_results,
            usage.result_records,
            usage.result_entries,
            usage.result_retained_bytes,
        )
    }
}
