//! Allocation-free owned-field preflight and token-gated Summary materialization.

use std::sync::Arc;

use parking_lot::Mutex;

use super::{PreparedSummaryRecords, SummaryRecordCensus, next_record};

const FIXED_INT_MAP_ENTRY_LEN: usize = size_of::<u16>() + size_of::<u64>();
const MESSAGE_INDEX_ENTRY_LEN: usize = size_of::<u64>() + size_of::<u64>();

/// Fixed nested-collection limits projected from the Web remote resource profile.
///
/// There is deliberately no production constructor while remote MCAP remains disarmed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SummaryMaterializationLimits {
    max_channel_metadata_entries_per_record: u64,
    max_channel_metadata_key_bytes: u64,
    max_channel_metadata_value_bytes: u64,
    max_nested_retained_bytes_per_summary: u64,
    max_chunk_index_message_index_offsets: u64,
    max_statistics_channel_message_counts: u64,
}

/// Limits for the shared allocation-free `MessageIndex.records` preflight.
#[derive(Clone, Copy, Debug)]
pub(crate) struct MessageIndexPreflightLimits {
    max_entries_per_record: u64,
    max_aggregate_entries: u64,
}

/// Allocation-free census produced before materialization is allowed to allocate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct NestedPreflightCensus {
    pub(crate) main_summary_records: u64,
    pub(crate) owned_string_count: u64,
    pub(crate) owned_string_bytes: u64,
    pub(crate) schema_data_bytes: u64,
    pub(crate) channel_metadata_entries: u64,
    pub(crate) fixed_map_entries: u64,
    pub(crate) nested_encoded_bytes: u64,
    pub(crate) nested_retained_bytes: u64,
}

/// Allocation-free evidence for one `MessageIndex.records` vector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MessageIndexPreflight {
    pub(crate) channel_id: u16,
    pub(crate) entries: u64,
    pub(crate) encoded_bytes: u64,
    pub(crate) aggregate_entries: u64,
}

/// Capacity assigned to one materialization budget owner.
///
/// Every preflight census dimension is reserved independently so a bounded input cannot borrow
/// capacity from another allocation category.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SummaryMaterializationCapacity {
    max_active_reservations: u64,
    census: NestedPreflightCensus,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct SummaryMaterializationUsage {
    active_reservations: u64,
    census: NestedPreflightCensus,
}

struct SummaryMaterializationBudgetState {
    profile_id: u64,
    limits: SummaryMaterializationLimits,
    capacity: SummaryMaterializationCapacity,
    usage: Mutex<SummaryMaterializationUsage>,
}

/// Owner of the real aggregate capacity used by Summary materialization permits.
///
/// Its constructor remains test-only while the Web remote-MCAP production route is disarmed.
pub(crate) struct SummaryMaterializationBudget {
    state: Arc<SummaryMaterializationBudgetState>,
}

impl std::fmt::Debug for SummaryMaterializationBudget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SummaryMaterializationBudget")
            .field("profile_id", &self.state.profile_id)
            .field("limits", &self.state.limits)
            .field("capacity", &self.state.capacity)
            .finish_non_exhaustive()
    }
}

impl SummaryMaterializationBudget {
    fn limits(&self) -> &SummaryMaterializationLimits {
        &self.state.limits
    }

    fn try_reserve(
        &self,
        census: NestedPreflightCensus,
    ) -> Result<SummaryMaterializationReservation, SummaryMaterializationError> {
        let mut usage = self.state.usage.lock();
        let next = add_usage(*usage, census)?;
        if !usage_fits(next, self.state.capacity) {
            return Err(SummaryMaterializationError::MaterializationReservationLimitExceeded);
        }
        *usage = next;
        drop(usage);

        Ok(SummaryMaterializationReservation {
            state: Arc::clone(&self.state),
            census,
        })
    }
}

/// Non-`Clone` ownership of one complete materialization census reservation.
struct SummaryMaterializationReservation {
    state: Arc<SummaryMaterializationBudgetState>,
    census: NestedPreflightCensus,
}

impl SummaryMaterializationReservation {
    const fn census(&self) -> NestedPreflightCensus {
        self.census
    }
}

impl std::fmt::Debug for SummaryMaterializationReservation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SummaryMaterializationReservation")
            .field("profile_id", &self.state.profile_id)
            .field("census", &self.census)
            .finish_non_exhaustive()
    }
}

impl Drop for SummaryMaterializationReservation {
    fn drop(&mut self) {
        let mut usage = self.state.usage.lock();
        *usage = subtract_usage(*usage, self.census)
            .expect("live Summary materialization reservation has matching budget usage");
    }
}

/// Private authority to begin materializing the exact prepared Header/Summary input.
///
/// This type is intentionally non-`Clone`; its private fields bind the complete preflight to the
/// borrowed bytes retained by `PreparedSummaryRecords`.
pub(crate) struct SummaryMaterializationToken<'a> {
    prepared: PreparedSummaryRecords<'a>,
    reservation: SummaryMaterializationReservation,
}

impl std::fmt::Debug for SummaryMaterializationToken<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SummaryMaterializationToken")
            .field("input", &"<preflight-complete borrowed bytes>")
            .field("reservation", &self.reservation)
            .finish_non_exhaustive()
    }
}

/// One source-ordered bounded Summary record.
///
/// Unknown bodies are deliberately absent: their validated envelopes are skipped without parsing
/// or copying their bytes.
#[derive(Debug)]
pub(crate) enum BoundedSummaryRecord<'a> {
    Known(mcap::records::Record<'a>),
    Unknown { opcode: u8, source_ordinal: u64 },
}

impl BoundedSummaryRecord<'_> {
    pub(crate) fn opcode(&self) -> u8 {
        match self {
            Self::Known(record) => record.opcode(),
            Self::Unknown { opcode, .. } => *opcode,
        }
    }
}

/// Materialized Header and source-ordered bounded Summary records.
#[derive(Debug)]
pub(crate) struct MaterializedSummaryRecords<'a> {
    prepared: PreparedSummaryRecords<'a>,
    header: mcap::records::Header,
    records: Vec<BoundedSummaryRecord<'a>>,
    reservation: SummaryMaterializationReservation,
}

impl<'a> MaterializedSummaryRecords<'a> {
    pub(crate) const fn header(&self) -> &mcap::records::Header {
        &self.header
    }

    pub(crate) fn records(&self) -> &[BoundedSummaryRecord<'a>] {
        &self.records
    }

    pub(crate) const fn prepared(&self) -> &PreparedSummaryRecords<'a> {
        &self.prepared
    }

    pub(crate) const fn top_level_census(&self) -> SummaryRecordCensus {
        self.prepared.census()
    }

    pub(crate) const fn nested_census(&self) -> NestedPreflightCensus {
        self.reservation.census()
    }
}

/// A preflight or token-gated materialization failure.
///
/// Errors intentionally omit source content, offsets, declared sizes, and identifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SummaryMaterializationError {
    RecordStructureInvalid,
    RecordTrailingBytes,
    StringInvalidUtf8,
    ArithmeticOverflow,
    ChannelMetadataEntryLimitExceeded,
    ChannelMetadataKeyBytesLimitExceeded,
    ChannelMetadataValueBytesLimitExceeded,
    ChunkIndexMessageIndexOffsetLimitExceeded,
    StatisticsChannelMessageCountLimitExceeded,
    MessageIndexEntryLimitExceeded,
    MessageIndexAggregateEntryLimitExceeded,
    NestedRetainedBytesLimitExceeded,
    NestedEncodedWidthInvalid,
    DuplicateNestedKey,
    PreparedCensusMismatch,
    PreparedRecordRejected,
    MaterializationReservationLimitExceeded,
}

impl std::fmt::Display for SummaryMaterializationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::RecordStructureInvalid => "remote MCAP record structure is invalid",
            Self::RecordTrailingBytes => "remote MCAP record body has trailing bytes",
            Self::StringInvalidUtf8 => "remote MCAP record string is not valid UTF-8",
            Self::ArithmeticOverflow => "remote MCAP nested record accounting overflowed",
            Self::ChannelMetadataEntryLimitExceeded => {
                "remote MCAP Channel metadata entry count exceeds its limit"
            }
            Self::ChannelMetadataKeyBytesLimitExceeded => {
                "remote MCAP Channel metadata key exceeds its byte limit"
            }
            Self::ChannelMetadataValueBytesLimitExceeded => {
                "remote MCAP Channel metadata value exceeds its byte limit"
            }
            Self::ChunkIndexMessageIndexOffsetLimitExceeded => {
                "remote MCAP ChunkIndex message-index offset count exceeds its limit"
            }
            Self::StatisticsChannelMessageCountLimitExceeded => {
                "remote MCAP Statistics channel-message count exceeds its limit"
            }
            Self::MessageIndexEntryLimitExceeded => {
                "remote MCAP MessageIndex entry count exceeds its limit"
            }
            Self::MessageIndexAggregateEntryLimitExceeded => {
                "remote MCAP MessageIndex aggregate entry count exceeds its limit"
            }
            Self::NestedRetainedBytesLimitExceeded => {
                "remote MCAP nested collections exceed their retained-byte limit"
            }
            Self::NestedEncodedWidthInvalid => {
                "remote MCAP fixed-width nested collection has an invalid byte length"
            }
            Self::DuplicateNestedKey => "remote MCAP nested map contains a duplicate key",
            Self::PreparedCensusMismatch => {
                "remote MCAP materialization preflight does not match prepared evidence"
            }
            Self::PreparedRecordRejected => {
                "remote MCAP prepared record was rejected during materialization"
            }
            Self::MaterializationReservationLimitExceeded => {
                "remote MCAP Summary materialization reservation exceeds its capacity"
            }
        })
    }
}

impl std::error::Error for SummaryMaterializationError {}

/// Rewalks the complete prepared Header and main Summary section without allocating.
pub(crate) fn preflight_summary_records<'a>(
    prepared: PreparedSummaryRecords<'a>,
    budget: &SummaryMaterializationBudget,
) -> Result<SummaryMaterializationToken<'a>, SummaryMaterializationError> {
    let mut census = NestedPreflightCensus::default();
    let limits = budget.limits();

    preflight_header(prepared.header_body(), &mut census)?;

    let bytes = prepared.main_summary_bytes();
    let mut cursor = 0_usize;
    let mut observed = SummaryRecordCensus::default();
    while cursor < bytes.len() {
        // Stage one already validated every envelope. Still handle any impossible mismatch without
        // panicking, so the token remains the only authority that reaches materialization.
        let envelope = next_record(bytes, cursor)
            .map_err(|_error| SummaryMaterializationError::PreparedCensusMismatch)?;
        let body = bytes
            .get(envelope.body_start..envelope.end)
            .ok_or(SummaryMaterializationError::PreparedCensusMismatch)?;

        census.main_summary_records = checked_add(census.main_summary_records, 1)?;
        match envelope.opcode {
            mcap::records::op::SCHEMA => {
                observed.schema_records = checked_add(observed.schema_records, 1)?;
                preflight_schema(body, &mut census)?;
            }
            mcap::records::op::CHANNEL => {
                observed.channel_records = checked_add(observed.channel_records, 1)?;
                preflight_channel(body, limits, &mut census)?;
            }
            mcap::records::op::CHUNK_INDEX => {
                observed.chunk_index_records = checked_add(observed.chunk_index_records, 1)?;
                preflight_chunk_index(body, limits, &mut census)?;
            }
            mcap::records::op::ATTACHMENT_INDEX => {
                preflight_attachment_index(body, &mut census)?;
            }
            mcap::records::op::STATISTICS => {
                preflight_statistics(body, limits, &mut census)?;
            }
            mcap::records::op::METADATA_INDEX => {
                preflight_metadata_index(body, &mut census)?;
            }
            _ => {
                observed.unknown_records = checked_add(observed.unknown_records, 1)?;
            }
        }
        cursor = envelope.end;
    }

    observed.summary_records = census.main_summary_records;
    let expected = prepared.census();
    let expected_main_records = expected
        .summary_records
        .checked_sub(expected.summary_offset_records)
        .ok_or(SummaryMaterializationError::PreparedCensusMismatch)?;
    if expected_main_records != observed.summary_records
        || expected.schema_records != observed.schema_records
        || expected.channel_records != observed.channel_records
        || expected.chunk_index_records != observed.chunk_index_records
        || expected.unknown_records != observed.unknown_records
    {
        return Err(SummaryMaterializationError::PreparedCensusMismatch);
    }

    let reservation = budget.try_reserve(census)?;
    Ok(SummaryMaterializationToken {
        prepared,
        reservation,
    })
}

/// Materializes known records only after consuming matching whole-input preflight evidence.
pub(crate) fn materialize_summary_records(
    token: SummaryMaterializationToken<'_>,
) -> Result<MaterializedSummaryRecords<'_>, SummaryMaterializationError> {
    let SummaryMaterializationToken {
        prepared,
        reservation,
    } = token;
    let census = reservation.census();
    let mcap::records::Record::Header(header) =
        mcap::parse_record(mcap::records::op::HEADER, prepared.header_body())
            .map_err(|_error| SummaryMaterializationError::PreparedRecordRejected)?
    else {
        return Err(SummaryMaterializationError::PreparedRecordRejected);
    };

    let record_capacity = usize::try_from(census.main_summary_records)
        .map_err(|_overflow| SummaryMaterializationError::ArithmeticOverflow)?;
    let mut records = Vec::with_capacity(record_capacity);
    let bytes = prepared.main_summary_bytes();
    let mut cursor = 0_usize;
    let mut source_ordinal = 0_u64;
    while cursor < bytes.len() {
        let envelope = next_record(bytes, cursor)
            .map_err(|_error| SummaryMaterializationError::PreparedCensusMismatch)?;
        let body = bytes
            .get(envelope.body_start..envelope.end)
            .ok_or(SummaryMaterializationError::PreparedCensusMismatch)?;

        let record = match envelope.opcode {
            mcap::records::op::SCHEMA
            | mcap::records::op::CHANNEL
            | mcap::records::op::CHUNK_INDEX
            | mcap::records::op::ATTACHMENT_INDEX
            | mcap::records::op::STATISTICS
            | mcap::records::op::METADATA_INDEX => BoundedSummaryRecord::Known(
                mcap::parse_record(envelope.opcode, body)
                    .map_err(|_error| SummaryMaterializationError::PreparedRecordRejected)?,
            ),
            opcode => BoundedSummaryRecord::Unknown {
                opcode,
                source_ordinal,
            },
        };
        records.push(record);
        source_ordinal = checked_add(source_ordinal, 1)?;
        cursor = envelope.end;
    }

    if source_ordinal != census.main_summary_records {
        return Err(SummaryMaterializationError::PreparedCensusMismatch);
    }

    Ok(MaterializedSummaryRecords {
        prepared,
        header,
        records,
        reservation,
    })
}

/// Validates a complete `MessageIndex` body before any entries are reserved or pushed.
pub(crate) fn preflight_message_index_records(
    body: &[u8],
    aggregate_entries_before: u64,
    limits: &MessageIndexPreflightLimits,
) -> Result<MessageIndexPreflight, SummaryMaterializationError> {
    let mut cursor = SliceCursor::new(body);
    let channel_id = cursor.read_u16()?;
    let encoded_bytes = cursor.read_u32_as_usize()?;
    cursor.take(encoded_bytes)?;
    if encoded_bytes % MESSAGE_INDEX_ENTRY_LEN != 0 {
        return Err(SummaryMaterializationError::NestedEncodedWidthInvalid);
    }
    cursor.finish()?;

    let entries = u64::try_from(encoded_bytes / MESSAGE_INDEX_ENTRY_LEN)
        .map_err(|_overflow| SummaryMaterializationError::ArithmeticOverflow)?;
    if entries > limits.max_entries_per_record {
        return Err(SummaryMaterializationError::MessageIndexEntryLimitExceeded);
    }
    let aggregate_entries = checked_add(aggregate_entries_before, entries)?;
    if aggregate_entries > limits.max_aggregate_entries {
        return Err(SummaryMaterializationError::MessageIndexAggregateEntryLimitExceeded);
    }

    Ok(MessageIndexPreflight {
        channel_id,
        entries,
        encoded_bytes: u64::try_from(encoded_bytes)
            .map_err(|_overflow| SummaryMaterializationError::ArithmeticOverflow)?,
        aggregate_entries,
    })
}

fn preflight_header(
    body: &[u8],
    census: &mut NestedPreflightCensus,
) -> Result<(), SummaryMaterializationError> {
    let mut cursor = SliceCursor::new(body);
    observe_owned_string(cursor.read_string()?, census)?;
    observe_owned_string(cursor.read_string()?, census)?;
    cursor.finish()
}

fn preflight_schema(
    body: &[u8],
    census: &mut NestedPreflightCensus,
) -> Result<(), SummaryMaterializationError> {
    let mut cursor = SliceCursor::new(body);
    cursor.skip(size_of::<u16>())?;
    observe_owned_string(cursor.read_string()?, census)?;
    observe_owned_string(cursor.read_string()?, census)?;
    let data_len = cursor.read_u32_as_usize()?;
    cursor.take(data_len)?;
    census.schema_data_bytes = checked_add(
        census.schema_data_bytes,
        u64::try_from(data_len)
            .map_err(|_overflow| SummaryMaterializationError::ArithmeticOverflow)?,
    )?;
    cursor.finish()
}

fn preflight_channel(
    body: &[u8],
    limits: &SummaryMaterializationLimits,
    census: &mut NestedPreflightCensus,
) -> Result<(), SummaryMaterializationError> {
    let mut cursor = SliceCursor::new(body);
    cursor.skip(size_of::<u16>() * 2)?;
    observe_owned_string(cursor.read_string()?, census)?;
    observe_owned_string(cursor.read_string()?, census)?;

    let encoded_len = cursor.read_u32_as_usize()?;
    let nested = cursor.take(encoded_len)?;
    cursor.finish()?;

    let mut nested_cursor = SliceCursor::new(nested);
    let mut entries = 0_u64;
    while !nested_cursor.is_finished() {
        let entry_start = nested_cursor.position();
        let key = nested_cursor.read_string()?;
        let value = nested_cursor.read_string()?;
        entries = checked_add(entries, 1)?;
        if entries > limits.max_channel_metadata_entries_per_record {
            return Err(SummaryMaterializationError::ChannelMetadataEntryLimitExceeded);
        }
        if usize_as_u64(key.len())? > limits.max_channel_metadata_key_bytes {
            return Err(SummaryMaterializationError::ChannelMetadataKeyBytesLimitExceeded);
        }
        if usize_as_u64(value.len())? > limits.max_channel_metadata_value_bytes {
            return Err(SummaryMaterializationError::ChannelMetadataValueBytesLimitExceeded);
        }
        ensure_unique_string_key(nested, entry_start, key)?;
        observe_owned_string(key, census)?;
        observe_owned_string(value, census)?;
        census.nested_retained_bytes = checked_add(
            census.nested_retained_bytes,
            checked_add(usize_as_u64(key.len())?, usize_as_u64(value.len())?)?,
        )?;
        enforce_nested_retained_limit(census, limits)?;
    }
    nested_cursor.finish()?;

    census.channel_metadata_entries = checked_add(census.channel_metadata_entries, entries)?;
    census.nested_encoded_bytes =
        checked_add(census.nested_encoded_bytes, usize_as_u64(encoded_len)?)?;
    Ok(())
}

fn preflight_chunk_index(
    body: &[u8],
    limits: &SummaryMaterializationLimits,
    census: &mut NestedPreflightCensus,
) -> Result<(), SummaryMaterializationError> {
    let mut cursor = SliceCursor::new(body);
    cursor.skip(size_of::<u64>() * 4)?;
    preflight_fixed_int_map(
        &mut cursor,
        limits.max_chunk_index_message_index_offsets,
        SummaryMaterializationError::ChunkIndexMessageIndexOffsetLimitExceeded,
        limits,
        census,
    )?;
    cursor.skip(size_of::<u64>())?;
    observe_owned_string(cursor.read_string()?, census)?;
    cursor.skip(size_of::<u64>() * 2)?;
    cursor.finish()
}

fn preflight_attachment_index(
    body: &[u8],
    census: &mut NestedPreflightCensus,
) -> Result<(), SummaryMaterializationError> {
    let mut cursor = SliceCursor::new(body);
    cursor.skip(size_of::<u64>() * 5)?;
    observe_owned_string(cursor.read_string()?, census)?;
    observe_owned_string(cursor.read_string()?, census)?;
    cursor.finish()
}

fn preflight_statistics(
    body: &[u8],
    limits: &SummaryMaterializationLimits,
    census: &mut NestedPreflightCensus,
) -> Result<(), SummaryMaterializationError> {
    let mut cursor = SliceCursor::new(body);
    cursor.skip(size_of::<u64>() + size_of::<u16>() + size_of::<u32>() * 4)?;
    cursor.skip(size_of::<u64>() * 2)?;
    preflight_fixed_int_map(
        &mut cursor,
        limits.max_statistics_channel_message_counts,
        SummaryMaterializationError::StatisticsChannelMessageCountLimitExceeded,
        limits,
        census,
    )?;
    cursor.finish()
}

fn preflight_metadata_index(
    body: &[u8],
    census: &mut NestedPreflightCensus,
) -> Result<(), SummaryMaterializationError> {
    let mut cursor = SliceCursor::new(body);
    cursor.skip(size_of::<u64>() * 2)?;
    observe_owned_string(cursor.read_string()?, census)?;
    cursor.finish()
}

fn preflight_fixed_int_map(
    cursor: &mut SliceCursor<'_>,
    max_entries: u64,
    limit_error: SummaryMaterializationError,
    limits: &SummaryMaterializationLimits,
    census: &mut NestedPreflightCensus,
) -> Result<(), SummaryMaterializationError> {
    let encoded_len = cursor.read_u32_as_usize()?;
    let nested = cursor.take(encoded_len)?;
    if encoded_len % FIXED_INT_MAP_ENTRY_LEN != 0 {
        return Err(SummaryMaterializationError::NestedEncodedWidthInvalid);
    }
    let entries = u64::try_from(encoded_len / FIXED_INT_MAP_ENTRY_LEN)
        .map_err(|_overflow| SummaryMaterializationError::ArithmeticOverflow)?;
    if entries > max_entries {
        return Err(limit_error);
    }

    for entry in 0..usize::try_from(entries)
        .map_err(|_overflow| SummaryMaterializationError::ArithmeticOverflow)?
    {
        let start = entry
            .checked_mul(FIXED_INT_MAP_ENTRY_LEN)
            .ok_or(SummaryMaterializationError::ArithmeticOverflow)?;
        let key = u16::from_le_bytes(
            nested[start..start + size_of::<u16>()]
                .try_into()
                .expect("fixed-width map key has an exact slice"),
        );
        for previous in 0..entry {
            let previous_start = previous
                .checked_mul(FIXED_INT_MAP_ENTRY_LEN)
                .ok_or(SummaryMaterializationError::ArithmeticOverflow)?;
            let previous_key = u16::from_le_bytes(
                nested[previous_start..previous_start + size_of::<u16>()]
                    .try_into()
                    .expect("fixed-width map key has an exact slice"),
            );
            if previous_key == key {
                return Err(SummaryMaterializationError::DuplicateNestedKey);
            }
        }
    }

    census.fixed_map_entries = checked_add(census.fixed_map_entries, entries)?;
    let encoded_len = usize_as_u64(encoded_len)?;
    census.nested_encoded_bytes = checked_add(census.nested_encoded_bytes, encoded_len)?;
    census.nested_retained_bytes = checked_add(census.nested_retained_bytes, encoded_len)?;
    enforce_nested_retained_limit(census, limits)
}

fn ensure_unique_string_key(
    nested: &[u8],
    current_entry_start: usize,
    current_key: &str,
) -> Result<(), SummaryMaterializationError> {
    let mut previous = SliceCursor::new(
        nested
            .get(..current_entry_start)
            .ok_or(SummaryMaterializationError::RecordStructureInvalid)?,
    );
    while !previous.is_finished() {
        let previous_key = previous.read_string()?;
        previous.read_string()?;
        if previous_key == current_key {
            return Err(SummaryMaterializationError::DuplicateNestedKey);
        }
    }
    previous.finish()
}

fn enforce_nested_retained_limit(
    census: &NestedPreflightCensus,
    limits: &SummaryMaterializationLimits,
) -> Result<(), SummaryMaterializationError> {
    if census.nested_retained_bytes > limits.max_nested_retained_bytes_per_summary {
        return Err(SummaryMaterializationError::NestedRetainedBytesLimitExceeded);
    }
    Ok(())
}

fn observe_owned_string(
    string: &str,
    census: &mut NestedPreflightCensus,
) -> Result<(), SummaryMaterializationError> {
    census.owned_string_count = checked_add(census.owned_string_count, 1)?;
    census.owned_string_bytes =
        checked_add(census.owned_string_bytes, usize_as_u64(string.len())?)?;
    Ok(())
}

fn add_usage(
    usage: SummaryMaterializationUsage,
    census: NestedPreflightCensus,
) -> Result<SummaryMaterializationUsage, SummaryMaterializationError> {
    Ok(SummaryMaterializationUsage {
        active_reservations: checked_add(usage.active_reservations, 1)?,
        census: add_census(usage.census, census)?,
    })
}

fn subtract_usage(
    usage: SummaryMaterializationUsage,
    census: NestedPreflightCensus,
) -> Option<SummaryMaterializationUsage> {
    Some(SummaryMaterializationUsage {
        active_reservations: usage.active_reservations.checked_sub(1)?,
        census: subtract_census(usage.census, census)?,
    })
}

fn add_census(
    left: NestedPreflightCensus,
    right: NestedPreflightCensus,
) -> Result<NestedPreflightCensus, SummaryMaterializationError> {
    Ok(NestedPreflightCensus {
        main_summary_records: checked_add(left.main_summary_records, right.main_summary_records)?,
        owned_string_count: checked_add(left.owned_string_count, right.owned_string_count)?,
        owned_string_bytes: checked_add(left.owned_string_bytes, right.owned_string_bytes)?,
        schema_data_bytes: checked_add(left.schema_data_bytes, right.schema_data_bytes)?,
        channel_metadata_entries: checked_add(
            left.channel_metadata_entries,
            right.channel_metadata_entries,
        )?,
        fixed_map_entries: checked_add(left.fixed_map_entries, right.fixed_map_entries)?,
        nested_encoded_bytes: checked_add(left.nested_encoded_bytes, right.nested_encoded_bytes)?,
        nested_retained_bytes: checked_add(
            left.nested_retained_bytes,
            right.nested_retained_bytes,
        )?,
    })
}

fn subtract_census(
    left: NestedPreflightCensus,
    right: NestedPreflightCensus,
) -> Option<NestedPreflightCensus> {
    Some(NestedPreflightCensus {
        main_summary_records: left
            .main_summary_records
            .checked_sub(right.main_summary_records)?,
        owned_string_count: left
            .owned_string_count
            .checked_sub(right.owned_string_count)?,
        owned_string_bytes: left
            .owned_string_bytes
            .checked_sub(right.owned_string_bytes)?,
        schema_data_bytes: left
            .schema_data_bytes
            .checked_sub(right.schema_data_bytes)?,
        channel_metadata_entries: left
            .channel_metadata_entries
            .checked_sub(right.channel_metadata_entries)?,
        fixed_map_entries: left
            .fixed_map_entries
            .checked_sub(right.fixed_map_entries)?,
        nested_encoded_bytes: left
            .nested_encoded_bytes
            .checked_sub(right.nested_encoded_bytes)?,
        nested_retained_bytes: left
            .nested_retained_bytes
            .checked_sub(right.nested_retained_bytes)?,
    })
}

const fn usage_fits(
    usage: SummaryMaterializationUsage,
    capacity: SummaryMaterializationCapacity,
) -> bool {
    usage.active_reservations <= capacity.max_active_reservations
        && usage.census.main_summary_records <= capacity.census.main_summary_records
        && usage.census.owned_string_count <= capacity.census.owned_string_count
        && usage.census.owned_string_bytes <= capacity.census.owned_string_bytes
        && usage.census.schema_data_bytes <= capacity.census.schema_data_bytes
        && usage.census.channel_metadata_entries <= capacity.census.channel_metadata_entries
        && usage.census.fixed_map_entries <= capacity.census.fixed_map_entries
        && usage.census.nested_encoded_bytes <= capacity.census.nested_encoded_bytes
        && usage.census.nested_retained_bytes <= capacity.census.nested_retained_bytes
}

fn checked_add(left: u64, right: u64) -> Result<u64, SummaryMaterializationError> {
    left.checked_add(right)
        .ok_or(SummaryMaterializationError::ArithmeticOverflow)
}

fn usize_as_u64(value: usize) -> Result<u64, SummaryMaterializationError> {
    u64::try_from(value).map_err(|_overflow| SummaryMaterializationError::ArithmeticOverflow)
}

struct SliceCursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> SliceCursor<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    const fn position(&self) -> usize {
        self.position
    }

    fn is_finished(&self) -> bool {
        self.position == self.bytes.len()
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], SummaryMaterializationError> {
        let end = self
            .position
            .checked_add(len)
            .ok_or(SummaryMaterializationError::ArithmeticOverflow)?;
        let value = self
            .bytes
            .get(self.position..end)
            .ok_or(SummaryMaterializationError::RecordStructureInvalid)?;
        self.position = end;
        Ok(value)
    }

    fn skip(&mut self, len: usize) -> Result<(), SummaryMaterializationError> {
        self.take(len).map(|_bytes| ())
    }

    fn read_u16(&mut self) -> Result<u16, SummaryMaterializationError> {
        Ok(u16::from_le_bytes(
            self.take(size_of::<u16>())?
                .try_into()
                .expect("u16 has an exact slice"),
        ))
    }

    fn read_u32_as_usize(&mut self) -> Result<usize, SummaryMaterializationError> {
        let value = u32::from_le_bytes(
            self.take(size_of::<u32>())?
                .try_into()
                .expect("u32 has an exact slice"),
        );
        usize::try_from(value).map_err(|_overflow| SummaryMaterializationError::ArithmeticOverflow)
    }

    fn read_string(&mut self) -> Result<&'a str, SummaryMaterializationError> {
        let len = self.read_u32_as_usize()?;
        std::str::from_utf8(self.take(len)?)
            .map_err(|_error| SummaryMaterializationError::StringInvalidUtf8)
    }

    fn finish(self) -> Result<(), SummaryMaterializationError> {
        if self.position == self.bytes.len() {
            Ok(())
        } else {
            Err(SummaryMaterializationError::RecordTrailingBytes)
        }
    }
}

#[cfg(test)]
impl SummaryMaterializationLimits {
    pub(crate) const fn for_test(
        max_channel_metadata_entries_per_record: u64,
        max_channel_metadata_key_bytes: u64,
        max_channel_metadata_value_bytes: u64,
        max_nested_retained_bytes_per_summary: u64,
        max_chunk_index_message_index_offsets: u64,
        max_statistics_channel_message_counts: u64,
    ) -> Self {
        Self {
            max_channel_metadata_entries_per_record,
            max_channel_metadata_key_bytes,
            max_channel_metadata_value_bytes,
            max_nested_retained_bytes_per_summary,
            max_chunk_index_message_index_offsets,
            max_statistics_channel_message_counts,
        }
    }
}

#[cfg(test)]
impl SummaryMaterializationBudget {
    pub(crate) fn for_test(
        limits: SummaryMaterializationLimits,
        max_active_reservations: u64,
        census_capacity: NestedPreflightCensus,
    ) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};

        static NEXT_PROFILE_ID: AtomicU64 = AtomicU64::new(1);
        Self {
            state: Arc::new(SummaryMaterializationBudgetState {
                profile_id: NEXT_PROFILE_ID.fetch_add(1, Ordering::Relaxed),
                limits,
                capacity: SummaryMaterializationCapacity {
                    max_active_reservations,
                    census: census_capacity,
                },
                usage: Mutex::new(SummaryMaterializationUsage::default()),
            }),
        }
    }

    pub(super) fn usage_for_test(&self) -> (u64, NestedPreflightCensus) {
        let usage = *self.state.usage.lock();
        (usage.active_reservations, usage.census)
    }

    pub(super) fn profile_id_for_test(&self) -> u64 {
        self.state.profile_id
    }
}

#[cfg(test)]
impl SummaryMaterializationToken<'_> {
    pub(super) fn reservation_profile_id_for_test(&self) -> u64 {
        self.reservation.state.profile_id
    }

    pub(super) const fn reserved_census_for_test(&self) -> NestedPreflightCensus {
        self.reservation.census()
    }
}

#[cfg(test)]
impl MaterializedSummaryRecords<'_> {
    pub(super) fn reservation_profile_id_for_test(&self) -> u64 {
        self.reservation.state.profile_id
    }
}

#[cfg(test)]
impl MessageIndexPreflightLimits {
    pub(super) const fn for_test(max_entries_per_record: u64, max_aggregate_entries: u64) -> Self {
        Self {
            max_entries_per_record,
            max_aggregate_entries,
        }
    }
}
