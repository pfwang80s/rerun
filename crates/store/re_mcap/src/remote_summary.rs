//! Allocation-free preparation for Web remote-MCAP Header and Summary records.
//!
//! This is the first of three deliberately separate parsing stages.
//! It validates only flat Header lengths, record envelopes, section boundaries, and top-level
//! counts.
//! No upstream record parser or allocation-bearing collection is used here.

// This module is intentionally production-disarmed until the remaining remote-MCAP parsing
// stages seal its input capability and consume its evidence.
#![allow(dead_code)]

use crate::remote_fixed_layout::{RemoteMcapSlice, ValidatedFixedLayout};

const RECORD_ENVELOPE_LEN: usize = super::RECORD_HEADER_LEN;
const SUMMARY_OFFSET_BODY_LEN: u64 = 1 + 8 + 8;
const HEADER_STRING_FIELDS: usize = 2;

/// Fixed scalar limits projected from the Web remote resource profile.
///
/// No production constructor exists before the private remote capability is sealed.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SummaryCensusLimits {
    max_header_body_bytes: u64,
    max_summary_bytes: u64,
    max_summary_records: u64,
    max_schema_records: u64,
    max_channel_records: u64,
    max_chunk_index_records: u64,
}

/// Allocation-free top-level record counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SummaryRecordCensus {
    pub(crate) summary_records: u64,
    pub(crate) schema_records: u64,
    pub(crate) channel_records: u64,
    pub(crate) chunk_index_records: u64,
    pub(crate) summary_offset_records: u64,
    pub(crate) unknown_records: u64,
}

/// Sealed evidence that Header and Summary top-level preparation completed without allocation.
///
/// The fields are private, the type is not `Clone`, and no public API can construct it.
/// MCAP-019 must consume this value and rewalk these exact borrowed inputs before materialization.
pub(crate) struct PreparedSummaryRecords<'a> {
    fixed_layout: ValidatedFixedLayout<'a>,
    header_body: &'a [u8],
    census: SummaryRecordCensus,
}

impl std::fmt::Debug for PreparedSummaryRecords<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedSummaryRecords")
            .field("header_body", &"<validated bytes>")
            .field("summary_body", &"<validated bytes>")
            .field("census", &self.census)
            .finish_non_exhaustive()
    }
}

impl<'a> PreparedSummaryRecords<'a> {
    pub(crate) const fn header_body(&self) -> &'a [u8] {
        self.header_body
    }

    pub(crate) const fn summary_bytes(&self) -> &'a [u8] {
        self.fixed_layout.summary_bytes()
    }

    pub(crate) const fn census(&self) -> SummaryRecordCensus {
        self.census
    }

    pub(crate) fn into_fixed_layout(self) -> ValidatedFixedLayout<'a> {
        self.fixed_layout
    }
}

/// An allocation-free Header/Summary preparation failure.
///
/// Errors omit offsets, declared lengths, record contents, and source identifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SummaryCensusError {
    HeaderReadRangeMismatch,
    HeaderBytesLimitExceeded,
    HeaderLengthPrefixTruncated,
    HeaderFieldOutOfBounds,
    HeaderTrailingBytes,
    SummaryBytesLimitExceeded,
    SummaryOffsetBoundaryInvalid,
    RecordEnvelopeTruncated,
    RecordRangeOverflow,
    RecordBodyOutOfBounds,
    InvalidRecordOpcode,
    ForbiddenSummaryRecordOpcode,
    RepeatedSummaryOpcodeGroup,
    SummaryRecordLimitExceeded,
    SchemaRecordLimitExceeded,
    ChannelRecordLimitExceeded,
    ChunkIndexRecordLimitExceeded,
    UnexpectedSummaryOffsetRecord,
    InvalidSummaryOffsetOpcode,
    InvalidSummaryOffsetBodyLength,
}

impl std::fmt::Display for SummaryCensusError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::HeaderReadRangeMismatch => {
                "remote MCAP Header body read does not match its required range"
            }
            Self::HeaderBytesLimitExceeded => "remote MCAP Header body exceeds its byte limit",
            Self::HeaderLengthPrefixTruncated => {
                "remote MCAP Header string length prefix is truncated"
            }
            Self::HeaderFieldOutOfBounds => "remote MCAP Header string is outside its record body",
            Self::HeaderTrailingBytes => "remote MCAP Header body has trailing bytes",
            Self::SummaryBytesLimitExceeded => "remote MCAP Summary exceeds its byte limit",
            Self::SummaryOffsetBoundaryInvalid => "remote MCAP Summary Offset boundary is invalid",
            Self::RecordEnvelopeTruncated => "remote MCAP Summary record envelope is truncated",
            Self::RecordRangeOverflow => "remote MCAP Summary record range overflowed",
            Self::RecordBodyOutOfBounds => "remote MCAP Summary record body is out of bounds",
            Self::InvalidRecordOpcode => "remote MCAP record opcode is invalid",
            Self::ForbiddenSummaryRecordOpcode => {
                "remote MCAP Summary contains a record type forbidden in that section"
            }
            Self::RepeatedSummaryOpcodeGroup => {
                "remote MCAP Summary record group is not contiguous"
            }
            Self::SummaryRecordLimitExceeded => {
                "remote MCAP Summary record count exceeds its limit"
            }
            Self::SchemaRecordLimitExceeded => "remote MCAP Schema record count exceeds its limit",
            Self::ChannelRecordLimitExceeded => {
                "remote MCAP Channel record count exceeds its limit"
            }
            Self::ChunkIndexRecordLimitExceeded => {
                "remote MCAP ChunkIndex record count exceeds its limit"
            }
            Self::UnexpectedSummaryOffsetRecord => {
                "remote MCAP Summary Offset record is outside its section"
            }
            Self::InvalidSummaryOffsetOpcode => {
                "remote MCAP Summary Offset section contains another record type"
            }
            Self::InvalidSummaryOffsetBodyLength => {
                "remote MCAP Summary Offset record body length is invalid"
            }
        })
    }
}

impl std::error::Error for SummaryCensusError {}

/// Performs the allocation-free first stage over exact Header and validated Summary bytes.
pub(crate) fn prepare_summary_records<'a>(
    fixed_layout: ValidatedFixedLayout<'a>,
    header_read: RemoteMcapSlice<'a>,
    limits: &SummaryCensusLimits,
) -> Result<PreparedSummaryRecords<'a>, SummaryCensusError> {
    let header_range = fixed_layout.header_body_range();
    validate_exact_read(header_read, header_range.start, header_range.end)?;
    validate_header_body(header_read.bytes(), limits.max_header_body_bytes)?;

    let summary_bytes = fixed_layout.summary_bytes();
    let summary_len = u64::try_from(summary_bytes.len())
        .map_err(|_overflow| SummaryCensusError::SummaryBytesLimitExceeded)?;
    if summary_len > limits.max_summary_bytes {
        return Err(SummaryCensusError::SummaryBytesLimitExceeded);
    }

    let summary_range = fixed_layout.summary_range();
    let main_end = if let Some(offset_start) = fixed_layout.summary_offset_start() {
        let relative = offset_start
            .checked_sub(summary_range.start)
            .ok_or(SummaryCensusError::SummaryOffsetBoundaryInvalid)?;
        let relative = usize::try_from(relative)
            .map_err(|_overflow| SummaryCensusError::SummaryOffsetBoundaryInvalid)?;
        if relative >= summary_bytes.len() {
            return Err(SummaryCensusError::SummaryOffsetBoundaryInvalid);
        }
        relative
    } else {
        summary_bytes.len()
    };

    let mut census = SummaryRecordCensus::default();
    scan_summary_records(&summary_bytes[..main_end], limits, &mut census)?;
    if fixed_layout.summary_offset_start().is_some() {
        scan_summary_offsets(&summary_bytes[main_end..], limits, &mut census)?;
    }

    Ok(PreparedSummaryRecords {
        fixed_layout,
        header_body: header_read.bytes(),
        census,
    })
}

fn validate_exact_read(
    read: RemoteMcapSlice<'_>,
    expected_start: u64,
    expected_end: u64,
) -> Result<(), SummaryCensusError> {
    let expected_len = expected_end
        .checked_sub(expected_start)
        .ok_or(SummaryCensusError::HeaderReadRangeMismatch)?;
    let actual_len = u64::try_from(read.bytes().len())
        .map_err(|_overflow| SummaryCensusError::HeaderReadRangeMismatch)?;
    let actual_end = read
        .offset()
        .checked_add(actual_len)
        .ok_or(SummaryCensusError::HeaderReadRangeMismatch)?;
    if read.offset() != expected_start || actual_end != expected_end || actual_len != expected_len {
        return Err(SummaryCensusError::HeaderReadRangeMismatch);
    }
    Ok(())
}

fn validate_header_body(
    header_body: &[u8],
    max_header_body_bytes: u64,
) -> Result<(), SummaryCensusError> {
    let actual_len = u64::try_from(header_body.len())
        .map_err(|_overflow| SummaryCensusError::HeaderBytesLimitExceeded)?;
    if actual_len > max_header_body_bytes {
        return Err(SummaryCensusError::HeaderBytesLimitExceeded);
    }

    let mut cursor = 0_usize;
    for _field in 0..HEADER_STRING_FIELDS {
        let prefix_end = cursor
            .checked_add(size_of::<u32>())
            .ok_or(SummaryCensusError::HeaderFieldOutOfBounds)?;
        let prefix = header_body
            .get(cursor..prefix_end)
            .ok_or(SummaryCensusError::HeaderLengthPrefixTruncated)?;
        let field_len = u32::from_le_bytes(
            prefix
                .try_into()
                .expect("Header string length prefix has an exact slice"),
        );
        let field_len = usize::try_from(field_len)
            .map_err(|_overflow| SummaryCensusError::HeaderFieldOutOfBounds)?;
        let field_end = prefix_end
            .checked_add(field_len)
            .ok_or(SummaryCensusError::HeaderFieldOutOfBounds)?;
        header_body
            .get(prefix_end..field_end)
            .ok_or(SummaryCensusError::HeaderFieldOutOfBounds)?;
        cursor = field_end;
    }
    if cursor != header_body.len() {
        return Err(SummaryCensusError::HeaderTrailingBytes);
    }
    Ok(())
}

fn scan_summary_records(
    bytes: &[u8],
    limits: &SummaryCensusLimits,
    census: &mut SummaryRecordCensus,
) -> Result<(), SummaryCensusError> {
    let mut cursor = 0_usize;
    let mut groups = SummaryOpcodeGroups::new();
    while cursor < bytes.len() {
        let record = next_record(bytes, cursor)?;
        validate_main_summary_opcode(record.opcode)?;
        groups.observe(record.opcode)?;
        observe_record(record.opcode, limits, census)?;
        cursor = record.end;
    }
    Ok(())
}

fn scan_summary_offsets(
    bytes: &[u8],
    limits: &SummaryCensusLimits,
    census: &mut SummaryRecordCensus,
) -> Result<(), SummaryCensusError> {
    let mut cursor = 0_usize;
    while cursor < bytes.len() {
        let record = next_record(bytes, cursor)?;
        if record.opcode != mcap::records::op::SUMMARY_OFFSET {
            return Err(SummaryCensusError::InvalidSummaryOffsetOpcode);
        }
        if record.body_len != SUMMARY_OFFSET_BODY_LEN {
            return Err(SummaryCensusError::InvalidSummaryOffsetBodyLength);
        }
        observe_record(record.opcode, limits, census)?;
        census.summary_offset_records = census
            .summary_offset_records
            .checked_add(1)
            .ok_or(SummaryCensusError::SummaryRecordLimitExceeded)?;
        cursor = record.end;
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct RecordEnvelope {
    opcode: u8,
    body_len: u64,
    end: usize,
}

fn next_record(bytes: &[u8], start: usize) -> Result<RecordEnvelope, SummaryCensusError> {
    let envelope_end = start
        .checked_add(RECORD_ENVELOPE_LEN)
        .ok_or(SummaryCensusError::RecordRangeOverflow)?;
    let envelope = bytes
        .get(start..envelope_end)
        .ok_or(SummaryCensusError::RecordEnvelopeTruncated)?;
    let opcode = envelope[0];
    if opcode == 0 {
        return Err(SummaryCensusError::InvalidRecordOpcode);
    }
    let body_len = u64::from_le_bytes(
        envelope[1..]
            .try_into()
            .expect("record body length has an exact slice"),
    );
    let body_len_usize =
        usize::try_from(body_len).map_err(|_overflow| SummaryCensusError::RecordRangeOverflow)?;
    let end = envelope_end
        .checked_add(body_len_usize)
        .ok_or(SummaryCensusError::RecordRangeOverflow)?;
    bytes
        .get(envelope_end..end)
        .ok_or(SummaryCensusError::RecordBodyOutOfBounds)?;
    if end <= start {
        return Err(SummaryCensusError::RecordRangeOverflow);
    }
    Ok(RecordEnvelope {
        opcode,
        body_len,
        end,
    })
}

struct SummaryOpcodeGroups {
    seen: [bool; 256],
    current: Option<u8>,
}

impl SummaryOpcodeGroups {
    const fn new() -> Self {
        Self {
            seen: [false; 256],
            current: None,
        }
    }

    fn observe(&mut self, opcode: u8) -> Result<(), SummaryCensusError> {
        if self.current == Some(opcode) {
            return Ok(());
        }

        let seen = &mut self.seen[usize::from(opcode)];
        if *seen {
            return Err(SummaryCensusError::RepeatedSummaryOpcodeGroup);
        }
        *seen = true;
        self.current = Some(opcode);
        Ok(())
    }
}

fn validate_main_summary_opcode(opcode: u8) -> Result<(), SummaryCensusError> {
    match opcode {
        mcap::records::op::SCHEMA
        | mcap::records::op::CHANNEL
        | mcap::records::op::CHUNK_INDEX
        | mcap::records::op::ATTACHMENT_INDEX
        | mcap::records::op::STATISTICS
        | mcap::records::op::METADATA_INDEX
        | 0x10..=u8::MAX => Ok(()),
        mcap::records::op::SUMMARY_OFFSET => Err(SummaryCensusError::UnexpectedSummaryOffsetRecord),
        0 => Err(SummaryCensusError::InvalidRecordOpcode),
        _ => Err(SummaryCensusError::ForbiddenSummaryRecordOpcode),
    }
}

fn observe_record(
    opcode: u8,
    limits: &SummaryCensusLimits,
    census: &mut SummaryRecordCensus,
) -> Result<(), SummaryCensusError> {
    census.summary_records = checked_increment(
        census.summary_records,
        limits.max_summary_records,
        SummaryCensusError::SummaryRecordLimitExceeded,
    )?;
    match opcode {
        mcap::records::op::SCHEMA => {
            census.schema_records = checked_increment(
                census.schema_records,
                limits.max_schema_records,
                SummaryCensusError::SchemaRecordLimitExceeded,
            )?;
        }
        mcap::records::op::CHANNEL => {
            census.channel_records = checked_increment(
                census.channel_records,
                limits.max_channel_records,
                SummaryCensusError::ChannelRecordLimitExceeded,
            )?;
        }
        mcap::records::op::CHUNK_INDEX => {
            census.chunk_index_records = checked_increment(
                census.chunk_index_records,
                limits.max_chunk_index_records,
                SummaryCensusError::ChunkIndexRecordLimitExceeded,
            )?;
        }
        opcode if !is_known_opcode(opcode) => {
            census.unknown_records = census
                .unknown_records
                .checked_add(1)
                .ok_or(SummaryCensusError::SummaryRecordLimitExceeded)?;
        }
        _ => {}
    }
    Ok(())
}

const fn is_known_opcode(opcode: u8) -> bool {
    opcode >= mcap::records::op::HEADER && opcode <= mcap::records::op::DATA_END
}

fn checked_increment(
    current: u64,
    limit: u64,
    error: SummaryCensusError,
) -> Result<u64, SummaryCensusError> {
    let next = current.checked_add(1).ok_or(error)?;
    if next > limit {
        return Err(error);
    }
    Ok(next)
}

#[cfg(test)]
mod tests {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    use super::*;
    use crate::remote_fixed_layout::{RemoteMcapSlice, prepare_fixed_layout};
    use crate::testing::{
        AdversarialMcapFixture, AdversarialMcapFixtureBuilder, FixtureCardinality, FixtureCrc,
    };

    struct TrackingAllocator;

    thread_local! {
        static TRACK_THIS_THREAD: Cell<bool> = const { Cell::new(false) };
        static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
    }

    #[expect(
        unsafe_code,
        reason = "the test-only allocator delegates unchanged to System after counting calls"
    )]
    // SAFETY: Every allocation operation is delegated to `System` with the original arguments;
    // the wrapper only increments a thread-local counter before allocation or reallocation.
    unsafe impl GlobalAlloc for TrackingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            record_allocation();
            // SAFETY: This wrapper preserves `System`'s allocation contract unchanged.
            unsafe { System.alloc(layout) }
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            record_allocation();
            // SAFETY: This wrapper preserves `System`'s allocation contract unchanged.
            unsafe { System.alloc_zeroed(layout) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            // SAFETY: `ptr` and `layout` came from the matching `System` allocation call.
            unsafe { System.dealloc(ptr, layout) }
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            record_allocation();
            // SAFETY: This wrapper preserves `System`'s reallocation contract unchanged.
            unsafe { System.realloc(ptr, layout, new_size) }
        }
    }

    #[global_allocator]
    static GLOBAL_ALLOCATOR: TrackingAllocator = TrackingAllocator;

    fn record_allocation() {
        let _ignored = TRACK_THIS_THREAD.try_with(|tracking| {
            if tracking.get() {
                let _ignored = ALLOCATIONS.try_with(|count| {
                    count.set(count.get().saturating_add(1));
                });
            }
        });
    }

    struct AllocationGuard;

    impl AllocationGuard {
        fn start() -> Self {
            ALLOCATIONS.with(|count| count.set(0));
            TRACK_THIS_THREAD.with(|tracking| tracking.set(true));
            Self
        }

        fn count() -> u64 {
            ALLOCATIONS.with(Cell::get)
        }
    }

    impl Drop for AllocationGuard {
        fn drop(&mut self) {
            TRACK_THIS_THREAD.with(|tracking| tracking.set(false));
        }
    }

    fn fixture(summary_offsets: bool) -> AdversarialMcapFixture {
        AdversarialMcapFixtureBuilder::new()
            .with_summary_crc(FixtureCrc::Zero)
            .with_summary_offsets(summary_offsets)
            .build()
            .expect("valid adversarial fixture")
    }

    fn generous_limits(fixture: &AdversarialMcapFixture) -> SummaryCensusLimits {
        SummaryCensusLimits {
            max_header_body_bytes: u64::try_from(fixture.bytes.len()).unwrap(),
            max_summary_bytes: u64::try_from(fixture.bytes.len()).unwrap(),
            max_summary_records: 1_000,
            max_schema_records: 1_000,
            max_channel_records: 1_000,
            max_chunk_index_records: 1_000,
        }
    }

    fn fixed_layout(bytes: &[u8]) -> ValidatedFixedLayout<'_> {
        const HEADER_PREFIX_LEN: usize = mcap::MAGIC.len() + RECORD_ENVELOPE_LEN;
        const FOOTER_TAIL_LEN: usize = RECORD_ENVELOPE_LEN + 20 + mcap::MAGIC.len();

        let object_len = u64::try_from(bytes.len()).unwrap();
        let tail_start = bytes.len() - FOOTER_TAIL_LEN;
        let prepared = prepare_fixed_layout(
            object_len,
            RemoteMcapSlice::new(0, &bytes[..HEADER_PREFIX_LEN]),
            RemoteMcapSlice::new(u64::try_from(tail_start).unwrap(), &bytes[tail_start..]),
        )
        .unwrap();
        let summary_range = prepared.data_end_and_summary_range();
        let start = usize::try_from(summary_range.start).unwrap();
        let end = usize::try_from(summary_range.end).unwrap();
        prepared
            .validate_data_end_and_summary(RemoteMcapSlice::new(
                summary_range.start,
                &bytes[start..end],
            ))
            .unwrap()
    }

    fn prepare<'a>(
        fixture: &'a AdversarialMcapFixture,
        limits: &SummaryCensusLimits,
    ) -> Result<PreparedSummaryRecords<'a>, SummaryCensusError> {
        let fixed = fixed_layout(&fixture.bytes);
        let header = fixture
            .layout
            .records
            .iter()
            .find(|record| record.opcode == mcap::records::op::HEADER)
            .expect("fixture Header");
        prepare_with_allocation_check(
            fixed,
            RemoteMcapSlice::new(
                u64::try_from(header.body_start).unwrap(),
                &fixture.bytes[header.body_start..header.end],
            ),
            limits,
        )
    }

    fn prepare_with_allocation_check<'a>(
        fixed_layout: ValidatedFixedLayout<'a>,
        header_read: RemoteMcapSlice<'a>,
        limits: &SummaryCensusLimits,
    ) -> Result<PreparedSummaryRecords<'a>, SummaryCensusError> {
        let guard = AllocationGuard::start();
        let result = prepare_summary_records(fixed_layout, header_read, limits);
        let allocations = AllocationGuard::count();
        drop(guard);
        assert_eq!(allocations, 0, "Summary preparation must not allocate");
        result
    }

    fn summary_records(fixture: &AdversarialMcapFixture) -> impl Iterator<Item = u8> + '_ {
        let footer = fixture.layout.footer.expect("fixture Footer");
        fixture
            .layout
            .records
            .iter()
            .filter(move |record| {
                record.start >= fixture.layout.summary_start && record.end <= footer.start
            })
            .map(|record| record.opcode)
    }

    fn fixture_with_main_opcodes(opcodes: &[u8]) -> AdversarialMcapFixture {
        assert!(opcodes.len() >= FixtureCardinality::default().summary_records);
        let mut fixture = AdversarialMcapFixtureBuilder::new()
            .with_cardinality(FixtureCardinality {
                summary_records: opcodes.len(),
                ..FixtureCardinality::default()
            })
            .with_summary_crc(FixtureCrc::Zero)
            .with_summary_offsets(false)
            .build()
            .unwrap();
        let summary_start = fixture.layout.summary_start;
        let main_end = fixture.layout.footer.unwrap().start;
        let layout = &fixture.layout;
        let bytes = &mut fixture.bytes;
        let mut records = layout
            .records
            .iter()
            .filter(|record| record.start >= summary_start && record.end <= main_end);
        for opcode in opcodes {
            let record = records.next().expect("fixture Summary record");
            bytes[record.start] = *opcode;
        }
        assert!(records.next().is_none());
        fixture
    }

    #[test]
    fn valid_offset_modes_match_the_independent_fixture_layout() {
        for offsets in [false, true] {
            let fixture = fixture(offsets);
            let prepared = prepare(&fixture, &generous_limits(&fixture)).unwrap();
            let opcodes: Vec<_> = summary_records(&fixture).collect();
            let expected = SummaryRecordCensus {
                summary_records: u64::try_from(opcodes.len()).unwrap(),
                schema_records: u64::try_from(
                    opcodes
                        .iter()
                        .filter(|opcode| **opcode == mcap::records::op::SCHEMA)
                        .count(),
                )
                .unwrap(),
                channel_records: u64::try_from(
                    opcodes
                        .iter()
                        .filter(|opcode| **opcode == mcap::records::op::CHANNEL)
                        .count(),
                )
                .unwrap(),
                chunk_index_records: u64::try_from(
                    opcodes
                        .iter()
                        .filter(|opcode| **opcode == mcap::records::op::CHUNK_INDEX)
                        .count(),
                )
                .unwrap(),
                summary_offset_records: u64::try_from(
                    opcodes
                        .iter()
                        .filter(|opcode| **opcode == mcap::records::op::SUMMARY_OFFSET)
                        .count(),
                )
                .unwrap(),
                unknown_records: u64::try_from(
                    opcodes
                        .iter()
                        .filter(|opcode| !is_known_opcode(**opcode))
                        .count(),
                )
                .unwrap(),
            };
            assert_eq!(prepared.census(), expected);
        }
    }

    #[test]
    fn exact_header_and_summary_borrows_cannot_be_replaced_after_preparation() {
        let fixture = fixture(true);
        let unrelated = fixture.bytes.clone();
        let prepared = prepare(&fixture, &generous_limits(&fixture)).unwrap();
        let header = fixture
            .layout
            .records
            .iter()
            .find(|record| record.opcode == mcap::records::op::HEADER)
            .unwrap();
        let footer = fixture.layout.footer.unwrap();

        assert!(std::ptr::eq(
            prepared.header_body().as_ptr(),
            fixture.bytes[header.body_start..header.end].as_ptr()
        ));
        assert!(std::ptr::eq(
            prepared.summary_bytes().as_ptr(),
            fixture.bytes[fixture.layout.summary_start..footer.start].as_ptr()
        ));
        assert!(!std::ptr::eq(
            prepared.header_body().as_ptr(),
            unrelated[header.body_start..header.end].as_ptr()
        ));
        assert!(!std::ptr::eq(
            prepared.summary_bytes().as_ptr(),
            unrelated[fixture.layout.summary_start..footer.start].as_ptr()
        ));
    }

    #[test]
    fn preparation_performs_zero_allocations_for_success_and_failure() {
        let cardinality = FixtureCardinality {
            summary_records: 8,
            schemas: 2,
            channels: 2,
            chunk_indexes: 2,
            records_per_chunk: 5,
            messages_per_chunk: 1,
            selected_dispatches_per_scan: 1,
        };
        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_cardinality(cardinality)
            .with_summary_crc(FixtureCrc::Zero)
            .build()
            .unwrap();
        let limits = generous_limits(&fixture);
        let fixed = fixed_layout(&fixture.bytes);
        let header = fixture
            .layout
            .records
            .iter()
            .find(|record| record.opcode == mcap::records::op::HEADER)
            .unwrap();
        let header_read = RemoteMcapSlice::new(
            u64::try_from(header.body_start).unwrap(),
            &fixture.bytes[header.body_start..header.end],
        );

        let guard = AllocationGuard::start();
        let prepared = prepare_summary_records(fixed, header_read, &limits).unwrap();
        let allocations = AllocationGuard::count();
        drop(guard);
        assert_eq!(allocations, 0);
        assert_eq!(prepared.census().unknown_records, 1);

        let mut failing_limits = generous_limits(&fixture);
        failing_limits.max_summary_records = 0;
        let fixed = fixed_layout(&fixture.bytes);
        let guard = AllocationGuard::start();
        let error = prepare_summary_records(fixed, header_read, &failing_limits).unwrap_err();
        let allocations = AllocationGuard::count();
        drop(guard);
        assert_eq!(allocations, 0);
        assert_eq!(error, SummaryCensusError::SummaryRecordLimitExceeded);
    }

    #[test]
    fn all_scalar_caps_accept_exact_boundary_and_reject_the_next_record() {
        let cardinality = FixtureCardinality {
            summary_records: 7,
            schemas: 2,
            channels: 2,
            chunk_indexes: 2,
            records_per_chunk: 5,
            messages_per_chunk: 1,
            selected_dispatches_per_scan: 1,
        };
        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_cardinality(cardinality)
            .with_summary_crc(FixtureCrc::Zero)
            .build()
            .unwrap();
        let exact = SummaryCensusLimits {
            max_header_body_bytes: 1_000,
            max_summary_bytes: u64::try_from(fixed_layout(&fixture.bytes).summary_bytes().len())
                .unwrap(),
            max_summary_records: 11,
            max_schema_records: 2,
            max_channel_records: 2,
            max_chunk_index_records: 2,
        };
        let prepared = prepare(&fixture, &exact).unwrap();
        let census = prepared.census();
        assert_eq!(census.schema_records, 2);
        assert_eq!(census.channel_records, 2);
        assert_eq!(census.chunk_index_records, 2);
        assert_eq!(census.summary_records, exact.max_summary_records);

        for (field, expected) in [
            ("summary", SummaryCensusError::SummaryRecordLimitExceeded),
            ("schema", SummaryCensusError::SchemaRecordLimitExceeded),
            ("channel", SummaryCensusError::ChannelRecordLimitExceeded),
            (
                "chunk_index",
                SummaryCensusError::ChunkIndexRecordLimitExceeded,
            ),
        ] {
            let limits = SummaryCensusLimits {
                max_summary_records: if field == "summary" {
                    exact.max_summary_records - 1
                } else {
                    exact.max_summary_records
                },
                max_schema_records: if field == "schema" { 1 } else { 2 },
                max_channel_records: if field == "channel" { 1 } else { 2 },
                max_chunk_index_records: if field == "chunk_index" { 1 } else { 2 },
                ..exact
            };
            assert_eq!(prepare(&fixture, &limits).unwrap_err(), expected);
        }

        let mut header_limited = exact;
        header_limited.max_header_body_bytes = 0;
        assert_eq!(
            prepare(&fixture, &header_limited).unwrap_err(),
            SummaryCensusError::HeaderBytesLimitExceeded
        );
        let mut summary_limited = exact;
        summary_limited.max_summary_bytes -= 1;
        assert_eq!(
            prepare(&fixture, &summary_limited).unwrap_err(),
            SummaryCensusError::SummaryBytesLimitExceeded
        );
    }

    #[test]
    fn malformed_header_lengths_and_exact_read_are_rejected_without_materialization() {
        let fixture = fixture(true);
        let fixed = fixed_layout(&fixture.bytes);
        let header = fixture
            .layout
            .records
            .iter()
            .find(|record| record.opcode == mcap::records::op::HEADER)
            .unwrap();
        let limits = generous_limits(&fixture);
        assert_eq!(
            prepare_with_allocation_check(
                fixed,
                RemoteMcapSlice::new(
                    u64::try_from(header.body_start + 1).unwrap(),
                    &fixture.bytes[header.body_start..header.end],
                ),
                &limits,
            )
            .unwrap_err(),
            SummaryCensusError::HeaderReadRangeMismatch
        );

        let mut bytes = fixture.bytes.clone();
        bytes[header.body_start..header.body_start + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        let out_of_bounds = AdversarialMcapFixture {
            bytes,
            ..fixture.clone()
        };
        assert_eq!(
            prepare(&out_of_bounds, &generous_limits(&out_of_bounds)).unwrap_err(),
            SummaryCensusError::HeaderFieldOutOfBounds
        );

        let header_body_len = header.end - header.body_start;
        let mut prefix_truncated = fixture.clone();
        prefix_truncated.bytes[header.body_start..header.body_start + 4]
            .copy_from_slice(&u32::try_from(header_body_len - 4).unwrap().to_le_bytes());
        assert_eq!(
            prepare(&prefix_truncated, &generous_limits(&prefix_truncated)).unwrap_err(),
            SummaryCensusError::HeaderLengthPrefixTruncated
        );

        let first_field_len = usize::try_from(u32::from_le_bytes(
            fixture.bytes[header.body_start..header.body_start + 4]
                .try_into()
                .unwrap(),
        ))
        .unwrap();
        let second_prefix = header.body_start + 4 + first_field_len;
        let second_field_len = u32::from_le_bytes(
            fixture.bytes[second_prefix..second_prefix + 4]
                .try_into()
                .unwrap(),
        );
        let mut trailing = fixture;
        trailing.bytes[second_prefix..second_prefix + 4]
            .copy_from_slice(&(second_field_len - 1).to_le_bytes());
        assert_eq!(
            prepare(&trailing, &generous_limits(&trailing)).unwrap_err(),
            SummaryCensusError::HeaderTrailingBytes
        );
    }

    #[test]
    fn malformed_summary_envelopes_and_section_rules_are_rejected() {
        let fixture = fixture(true);
        let footer = fixture.layout.footer.unwrap();
        let first_summary_offset = fixture
            .layout
            .records
            .iter()
            .find(|record| record.opcode == mcap::records::op::SUMMARY_OFFSET)
            .copied()
            .unwrap();

        let mutate = |offset: usize, replacement: &[u8]| {
            let mut changed = fixture.clone();
            changed.bytes[offset..offset + replacement.len()].copy_from_slice(replacement);
            changed
        };

        let wrong_opcode = mutate(first_summary_offset.start, &[mcap::records::op::CHANNEL]);
        assert_eq!(
            prepare(&wrong_opcode, &generous_limits(&wrong_opcode)).unwrap_err(),
            SummaryCensusError::InvalidSummaryOffsetOpcode
        );

        let zero_opcode = mutate(first_summary_offset.start, &[0]);
        assert_eq!(
            prepare(&zero_opcode, &generous_limits(&zero_opcode)).unwrap_err(),
            SummaryCensusError::InvalidRecordOpcode
        );

        let wrong_body_len = mutate(first_summary_offset.start + 1, &18_u64.to_le_bytes());
        assert_eq!(
            prepare(&wrong_body_len, &generous_limits(&wrong_body_len)).unwrap_err(),
            SummaryCensusError::InvalidSummaryOffsetBodyLength
        );

        let overflow = mutate(fixture.layout.summary_start + 1, &u64::MAX.to_le_bytes());
        assert_eq!(
            prepare(&overflow, &generous_limits(&overflow)).unwrap_err(),
            SummaryCensusError::RecordRangeOverflow
        );

        let last_before_offsets = fixture
            .layout
            .records
            .iter()
            .rfind(|record| {
                record.start >= fixture.layout.summary_start
                    && record.end <= first_summary_offset.start
            })
            .copied()
            .unwrap();
        let too_long = u64::try_from(last_before_offsets.body_len + 1).unwrap();
        let truncated = mutate(last_before_offsets.start + 1, &too_long.to_le_bytes());
        assert_eq!(
            prepare(&truncated, &generous_limits(&truncated)).unwrap_err(),
            SummaryCensusError::RecordBodyOutOfBounds
        );

        let mut boundary_inside_record = fixture.clone();
        let split = u64::try_from(first_summary_offset.start + 1).unwrap();
        boundary_inside_record.bytes[footer.body_start + 8..footer.body_start + 16]
            .copy_from_slice(&split.to_le_bytes());
        assert_eq!(
            prepare(
                &boundary_inside_record,
                &generous_limits(&boundary_inside_record)
            )
            .unwrap_err(),
            SummaryCensusError::RecordEnvelopeTruncated
        );
    }

    #[test]
    fn absent_offsets_reject_summary_offset_opcode_and_empty_unknown_records_make_progress() {
        let cardinality = FixtureCardinality {
            summary_records: 4,
            ..FixtureCardinality::default()
        };
        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_cardinality(cardinality)
            .with_summary_crc(FixtureCrc::Zero)
            .with_summary_offsets(false)
            .build()
            .unwrap();
        let unknown = fixture
            .layout
            .records
            .iter()
            .find(|record| record.opcode >= 0x80)
            .copied()
            .unwrap();
        let mut zero_body_unknown = fixture.clone();
        zero_body_unknown.bytes[unknown.start + 1..unknown.body_start]
            .copy_from_slice(&0_u64.to_le_bytes());
        zero_body_unknown.bytes.remove(unknown.body_start);
        assert!(prepare(&zero_body_unknown, &generous_limits(&zero_body_unknown)).is_ok());

        let mut forbidden = fixture;
        forbidden.bytes[unknown.start] = mcap::records::op::SUMMARY_OFFSET;
        assert_eq!(
            prepare(&forbidden, &generous_limits(&forbidden)).unwrap_err(),
            SummaryCensusError::UnexpectedSummaryOffsetRecord
        );
    }

    #[test]
    fn allowed_standard_future_and_private_opcodes_form_arbitrary_contiguous_groups() {
        let fixture = fixture_with_main_opcodes(&[
            0xff,
            0xff,
            mcap::records::op::SCHEMA,
            mcap::records::op::SCHEMA,
            0x10,
            0x10,
            mcap::records::op::CHANNEL,
            mcap::records::op::CHUNK_INDEX,
            mcap::records::op::ATTACHMENT_INDEX,
            mcap::records::op::STATISTICS,
            mcap::records::op::METADATA_INDEX,
            0x7f,
            0x7f,
            0x80,
            0x80,
        ]);
        let census = prepare(&fixture, &generous_limits(&fixture))
            .unwrap()
            .census();

        assert_eq!(census.summary_records, 15);
        assert_eq!(census.schema_records, 2);
        assert_eq!(census.channel_records, 1);
        assert_eq!(census.chunk_index_records, 1);
        assert_eq!(census.unknown_records, 8);
    }

    #[test]
    fn standard_future_and_private_groups_cannot_reappear() {
        for opcode in [mcap::records::op::SCHEMA, 0x10, 0x7f, 0x80, u8::MAX] {
            let fixture =
                fixture_with_main_opcodes(&[opcode, mcap::records::op::STATISTICS, opcode]);
            assert_eq!(
                prepare(&fixture, &generous_limits(&fixture)).unwrap_err(),
                SummaryCensusError::RepeatedSummaryOpcodeGroup,
                "repeated opcode group {opcode:#04x}"
            );
        }
    }

    #[test]
    fn zero_and_every_context_forbidden_standard_opcode_are_rejected() {
        let zero =
            fixture_with_main_opcodes(&[0, mcap::records::op::SCHEMA, mcap::records::op::CHANNEL]);
        assert_eq!(
            prepare(&zero, &generous_limits(&zero)).unwrap_err(),
            SummaryCensusError::InvalidRecordOpcode
        );

        for opcode in [
            mcap::records::op::HEADER,
            mcap::records::op::FOOTER,
            mcap::records::op::MESSAGE,
            mcap::records::op::CHUNK,
            mcap::records::op::MESSAGE_INDEX,
            mcap::records::op::ATTACHMENT,
            mcap::records::op::METADATA,
            mcap::records::op::DATA_END,
        ] {
            let fixture = fixture_with_main_opcodes(&[
                opcode,
                mcap::records::op::SCHEMA,
                mcap::records::op::CHANNEL,
            ]);
            assert_eq!(
                prepare(&fixture, &generous_limits(&fixture)).unwrap_err(),
                SummaryCensusError::ForbiddenSummaryRecordOpcode,
                "context-forbidden opcode {opcode:#04x}"
            );
        }

        let summary_offset = fixture_with_main_opcodes(&[
            mcap::records::op::SUMMARY_OFFSET,
            mcap::records::op::SCHEMA,
            mcap::records::op::CHANNEL,
        ]);
        assert_eq!(
            prepare(&summary_offset, &generous_limits(&summary_offset)).unwrap_err(),
            SummaryCensusError::UnexpectedSummaryOffsetRecord
        );
    }

    #[test]
    fn implementation_has_no_upstream_materialization_calls() {
        let source = include_str!("remote_summary.rs");
        let production_source = source
            .split("#[cfg(test)]")
            .next()
            .expect("production source precedes the test module");
        for forbidden in ["parse_record(", "SummaryReader", "into_owned("] {
            assert!(!production_source.contains(forbidden));
        }
    }
}
