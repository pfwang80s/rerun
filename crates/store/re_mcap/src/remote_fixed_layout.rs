//! Fixed-layout and checksum validation for Web remote-MCAP inputs.
//!
//! This module only validates the fixed file edges, the `DataEnd` boundary, and checksums.
//! It does not parse Summary collections, decompress Chunks, or scan Chunk records.

use std::ops::Range;

const MAGIC_LEN: usize = mcap::MAGIC.len();
const RECORD_ENVELOPE_LEN: usize = super::RECORD_HEADER_LEN;
const HEADER_PREFIX_LEN: usize = MAGIC_LEN + RECORD_ENVELOPE_LEN;
const DATA_END_BODY_LEN: usize = 4;
const DATA_END_RECORD_LEN: usize = RECORD_ENVELOPE_LEN + DATA_END_BODY_LEN;
const FOOTER_BODY_LEN: usize = 20;
const FOOTER_RECORD_LEN: usize = RECORD_ENVELOPE_LEN + FOOTER_BODY_LEN;
const FOOTER_TAIL_LEN: usize = FOOTER_RECORD_LEN + MAGIC_LEN;
const FOOTER_CRC_PREFIX_LEN: usize = RECORD_ENVELOPE_LEN + 16;

/// A byte slice whose absolute position in the remote object is known.
#[derive(Clone, Copy, Debug)]
pub struct RemoteMcapSlice<'a> {
    offset: u64,
    bytes: &'a [u8],
}

impl<'a> RemoteMcapSlice<'a> {
    /// Associates `bytes` with its absolute object offset.
    pub const fn new(offset: u64, bytes: &'a [u8]) -> Self {
        Self { offset, bytes }
    }
}

/// The validation state of an optional MCAP CRC field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptionalCrcValidation {
    /// The declared CRC was zero, which means that no CRC was provided.
    NotProvided,

    /// The declared nonzero CRC matched the bytes required by the MCAP specification.
    Verified,
}

/// The data-section CRC state established by a partial remote read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataSectionCrcValidation {
    /// The full data section was not available, so its CRC was deliberately not checked.
    NotVerifiedByPartialRead,
}

/// A fixed-layout validation failure.
///
/// Variants deliberately omit offsets, lengths, CRC values, and source identifiers so the error
/// can cross a public status boundary without disclosing request data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixedLayoutError {
    /// The object cannot contain the required fixed records.
    FileTooShort,

    /// A supplied partial read did not exactly cover the required object range.
    UnexpectedReadRange,

    /// The leading MCAP magic was absent or malformed.
    InvalidStartingMagic,

    /// The trailing MCAP magic was absent or malformed.
    InvalidTrailingMagic,

    /// The first record was not a Header record.
    InvalidHeaderOpcode,

    /// The Header record range overflowed or overlapped the `DataEnd` record.
    InvalidHeaderRange,

    /// The fixed tail did not contain a Footer record.
    InvalidFooterOpcode,

    /// The Footer record did not declare its required 20-byte body.
    InvalidFooterBodyLength,

    /// The Footer did not declare the Summary required by the remote-MCAP MVP.
    SummaryRequired,

    /// The Summary or its preceding `DataEnd` record was outside the data-to-Footer range.
    InvalidSummaryRange,

    /// A nonzero Summary Offset start was outside the Summary-to-Footer range.
    InvalidSummaryOffsetRange,

    /// The record immediately preceding the Summary was not a `DataEnd` record.
    InvalidDataEndOpcode,

    /// The `DataEnd` record did not declare its required four-byte body.
    InvalidDataEndBodyLength,

    /// A declared nonzero Summary CRC did not match its specification-defined coverage.
    SummaryChecksumMismatch,
}

impl std::fmt::Display for FixedLayoutError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::FileTooShort => "remote MCAP object is too short for its fixed records",
            Self::UnexpectedReadRange => {
                "remote MCAP partial read does not match the required range"
            }
            Self::InvalidStartingMagic => "remote MCAP starting magic is invalid",
            Self::InvalidTrailingMagic => "remote MCAP trailing magic is invalid",
            Self::InvalidHeaderOpcode => "remote MCAP Header opcode is invalid",
            Self::InvalidHeaderRange => "remote MCAP Header range is invalid",
            Self::InvalidFooterOpcode => "remote MCAP Footer opcode is invalid",
            Self::InvalidFooterBodyLength => "remote MCAP Footer body length is invalid",
            Self::SummaryRequired => "remote MCAP Summary is required",
            Self::InvalidSummaryRange => "remote MCAP Summary range is invalid",
            Self::InvalidSummaryOffsetRange => "remote MCAP Summary Offset range is invalid",
            Self::InvalidDataEndOpcode => "remote MCAP DataEnd opcode is invalid",
            Self::InvalidDataEndBodyLength => "remote MCAP DataEnd body length is invalid",
            Self::SummaryChecksumMismatch => "remote MCAP Summary checksum does not match",
        })
    }
}

impl std::error::Error for FixedLayoutError {}

/// A validated edge layout that defines the one continuous follow-up read.
///
/// This value is not a complete MCAP index.
/// Call [`PreparedFixedLayout::validate_data_end_and_summary`] with the exact requested range
/// before parsing any Summary record.
#[derive(Debug)]
pub struct PreparedFixedLayout {
    header_body_range: Range<u64>,
    data_end_and_summary_range: Range<u64>,
    summary_start: u64,
    summary_offset_start: Option<u64>,
    footer_crc_prefix: [u8; FOOTER_CRC_PREFIX_LEN],
    declared_summary_crc: u32,
}

impl PreparedFixedLayout {
    /// The exact Header body range that a bounded Header parser must read.
    pub fn header_body_range(&self) -> Range<u64> {
        self.header_body_range.clone()
    }

    /// The exact continuous range containing `DataEnd`, Summary, and Summary Offsets.
    ///
    /// Its end is the Footer opcode and is therefore exclusive.
    pub fn data_end_and_summary_range(&self) -> Range<u64> {
        self.data_end_and_summary_range.clone()
    }

    /// The absolute start of the Summary section.
    pub const fn summary_start(&self) -> u64 {
        self.summary_start
    }

    /// The absolute start of Summary Offset records, or `None` when they were not emitted.
    pub const fn summary_offset_start(&self) -> Option<u64> {
        self.summary_offset_start
    }

    /// Validates the fixed `DataEnd` record and the Summary CRC over one exact continuous read.
    ///
    /// The CRC coverage is `[summary_start, footer_body_start + 16)`, including Summary record
    /// envelopes, Summary Offset records, the Footer envelope, and the first two Footer fields.
    pub fn validate_data_end_and_summary(
        self,
        read: RemoteMcapSlice<'_>,
    ) -> Result<ValidatedFixedLayout<'_>, FixedLayoutError> {
        validate_exact_read(&read, &self.data_end_and_summary_range)?;

        let data_end = read
            .bytes
            .get(..DATA_END_RECORD_LEN)
            .ok_or(FixedLayoutError::UnexpectedReadRange)?;
        if data_end[0] != mcap::records::op::DATA_END {
            return Err(FixedLayoutError::InvalidDataEndOpcode);
        }
        let data_end_body_len = u64::try_from(DATA_END_BODY_LEN)
            .map_err(|_overflow| FixedLayoutError::InvalidDataEndBodyLength)?;
        if read_u64(&data_end[1..RECORD_ENVELOPE_LEN]) != data_end_body_len {
            return Err(FixedLayoutError::InvalidDataEndBodyLength);
        }

        let summary_bytes = read
            .bytes
            .get(DATA_END_RECORD_LEN..)
            .ok_or(FixedLayoutError::UnexpectedReadRange)?;
        let summary_crc = if self.declared_summary_crc == 0 {
            OptionalCrcValidation::NotProvided
        } else {
            let mut hasher = crc32fast::Hasher::new();
            hasher.update(summary_bytes);
            hasher.update(&self.footer_crc_prefix);
            if hasher.finalize() != self.declared_summary_crc {
                return Err(FixedLayoutError::SummaryChecksumMismatch);
            }
            OptionalCrcValidation::Verified
        };

        Ok(ValidatedFixedLayout {
            header_body_range: self.header_body_range,
            summary_range: self.summary_start..self.data_end_and_summary_range.end,
            summary_bytes,
            summary_offset_start: self.summary_offset_start,
            summary_crc,
            data_section_crc: DataSectionCrcValidation::NotVerifiedByPartialRead,
        })
    }
}

/// The fixed object layout after `DataEnd` and Summary CRC validation.
///
/// Summary collections remain unparsed.
pub struct ValidatedFixedLayout<'a> {
    header_body_range: Range<u64>,
    summary_range: Range<u64>,
    summary_bytes: &'a [u8],
    summary_offset_start: Option<u64>,
    summary_crc: OptionalCrcValidation,
    data_section_crc: DataSectionCrcValidation,
}

impl std::fmt::Debug for ValidatedFixedLayout<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ValidatedFixedLayout")
            .field("header_body_range", &self.header_body_range)
            .field("summary_range", &self.summary_range)
            .field("summary_offset_start", &self.summary_offset_start)
            .field("summary_crc", &self.summary_crc)
            .field("data_section_crc", &self.data_section_crc)
            .finish_non_exhaustive()
    }
}

impl<'a> ValidatedFixedLayout<'a> {
    /// The exact Header body range.
    pub fn header_body_range(&self) -> Range<u64> {
        self.header_body_range.clone()
    }

    /// The Summary and Summary Offset range, ending at the Footer opcode.
    pub fn summary_range(&self) -> Range<u64> {
        self.summary_range.clone()
    }

    /// The exact Summary and Summary Offset bytes whose `DataEnd` boundary and CRC were checked.
    ///
    /// A later Summary parser must consume this slice instead of accepting another raw buffer.
    pub const fn summary_bytes(&self) -> &'a [u8] {
        self.summary_bytes
    }

    /// Consumes the validation evidence and transfers its exact Summary bytes to a parser.
    pub const fn into_summary_bytes(self) -> &'a [u8] {
        self.summary_bytes
    }

    /// The absolute start of Summary Offset records, or `None` when absent.
    pub const fn summary_offset_start(&self) -> Option<u64> {
        self.summary_offset_start
    }

    /// The result of validating the optional Summary CRC.
    pub const fn summary_crc(&self) -> OptionalCrcValidation {
        self.summary_crc
    }

    /// The deliberately unverified data-section CRC state.
    pub const fn data_section_crc(&self) -> DataSectionCrcValidation {
        self.data_section_crc
    }
}

/// Validates the MCAP fixed edges and prepares the one continuous Summary read.
///
/// `start` must cover exactly the leading magic and Header envelope.
/// `tail` must cover exactly the final Footer record and trailing magic.
/// No Header body, Summary collection, or data-section byte is parsed by this function.
/// The caller must separately prove that all reads came from the same bound remote object;
/// this content validator deliberately does not introduce source or session identity.
pub fn prepare_fixed_layout(
    object_len: u64,
    start: RemoteMcapSlice<'_>,
    tail: RemoteMcapSlice<'_>,
) -> Result<PreparedFixedLayout, FixedLayoutError> {
    let minimum_len = u64::try_from(HEADER_PREFIX_LEN + DATA_END_RECORD_LEN + 1 + FOOTER_TAIL_LEN)
        .map_err(|_overflow| FixedLayoutError::FileTooShort)?;
    if object_len < minimum_len {
        return Err(FixedLayoutError::FileTooShort);
    }

    let header_prefix_end = u64::try_from(HEADER_PREFIX_LEN)
        .map_err(|_overflow| FixedLayoutError::UnexpectedReadRange)?;
    validate_exact_read(&start, &(0..header_prefix_end))?;
    if start.bytes[..MAGIC_LEN] != *mcap::MAGIC {
        return Err(FixedLayoutError::InvalidStartingMagic);
    }
    if start.bytes[MAGIC_LEN] != mcap::records::op::HEADER {
        return Err(FixedLayoutError::InvalidHeaderOpcode);
    }

    let tail_len =
        u64::try_from(FOOTER_TAIL_LEN).map_err(|_overflow| FixedLayoutError::FileTooShort)?;
    let tail_start = object_len
        .checked_sub(tail_len)
        .ok_or(FixedLayoutError::FileTooShort)?;
    validate_exact_read(&tail, &(tail_start..object_len))?;
    if tail.bytes[FOOTER_RECORD_LEN..] != *mcap::MAGIC {
        return Err(FixedLayoutError::InvalidTrailingMagic);
    }
    if tail.bytes[0] != mcap::records::op::FOOTER {
        return Err(FixedLayoutError::InvalidFooterOpcode);
    }
    let footer_body_len =
        u64::try_from(FOOTER_BODY_LEN).map_err(|_overflow| FixedLayoutError::FileTooShort)?;
    if read_u64(&tail.bytes[1..RECORD_ENVELOPE_LEN]) != footer_body_len {
        return Err(FixedLayoutError::InvalidFooterBodyLength);
    }

    let footer_body = &tail.bytes[RECORD_ENVELOPE_LEN..FOOTER_RECORD_LEN];
    let summary_start = read_u64(&footer_body[..8]);
    if summary_start == 0 {
        return Err(FixedLayoutError::SummaryRequired);
    }
    let data_end_record_len = u64::try_from(DATA_END_RECORD_LEN)
        .map_err(|_overflow| FixedLayoutError::InvalidSummaryRange)?;
    let data_end_start = summary_start
        .checked_sub(data_end_record_len)
        .ok_or(FixedLayoutError::InvalidSummaryRange)?;
    let footer_start = tail_start;
    if summary_start >= footer_start {
        return Err(FixedLayoutError::InvalidSummaryRange);
    }

    let summary_offset_start = match read_u64(&footer_body[8..16]) {
        0 => None,
        offset if (summary_start..footer_start).contains(&offset) => Some(offset),
        _ => return Err(FixedLayoutError::InvalidSummaryOffsetRange),
    };

    let header_body_start = header_prefix_end;
    let header_body_len = read_u64(&start.bytes[MAGIC_LEN + 1..HEADER_PREFIX_LEN]);
    let header_body_end = header_body_start
        .checked_add(header_body_len)
        .ok_or(FixedLayoutError::InvalidHeaderRange)?;
    if header_body_end > data_end_start {
        return Err(FixedLayoutError::InvalidHeaderRange);
    }

    let mut footer_crc_prefix = [0_u8; FOOTER_CRC_PREFIX_LEN];
    footer_crc_prefix.copy_from_slice(&tail.bytes[..FOOTER_CRC_PREFIX_LEN]);
    let declared_summary_crc = read_u32(&footer_body[16..20]);

    Ok(PreparedFixedLayout {
        header_body_range: header_body_start..header_body_end,
        data_end_and_summary_range: data_end_start..footer_start,
        summary_start,
        summary_offset_start,
        footer_crc_prefix,
        declared_summary_crc,
    })
}

/// A checksum-validated, exact-size uncompressed Chunk output.
///
/// The private fields prevent downstream scanners from constructing this capability directly.
#[derive(Clone, Copy, Debug)]
pub struct ValidatedChunkOutput<'a> {
    bytes: &'a [u8],
    checksum: OptionalCrcValidation,
}

impl<'a> ValidatedChunkOutput<'a> {
    /// Returns the exact final uncompressed output for a subsequent bounded scanner.
    pub const fn bytes(self) -> &'a [u8] {
        self.bytes
    }

    /// Returns the result of validating the optional uncompressed Chunk CRC.
    pub const fn checksum(self) -> OptionalCrcValidation {
        self.checksum
    }
}

/// A final uncompressed Chunk output validation failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChunkOutputValidationError {
    /// The final output length did not exactly equal the Chunk declaration.
    OutputLengthMismatch,

    /// A declared nonzero Chunk CRC did not match the exact final output.
    ChunkChecksumMismatch,
}

impl std::fmt::Display for ChunkOutputValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::OutputLengthMismatch => {
                "remote MCAP Chunk output does not match its declared length"
            }
            Self::ChunkChecksumMismatch => "remote MCAP Chunk checksum does not match",
        })
    }
}

impl std::error::Error for ChunkOutputValidationError {}

/// Validates the exact final output length and optional uncompressed Chunk CRC.
///
/// A codec driver must call this only after proving codec EOF and complete compressed-input
/// consumption.
/// The returned capability is intended to be the only input accepted by the later Chunk scanner.
pub fn validate_chunk_output(
    declared_uncompressed_size: u64,
    declared_uncompressed_crc: u32,
    final_output: &[u8],
) -> Result<ValidatedChunkOutput<'_>, ChunkOutputValidationError> {
    let actual_len = u64::try_from(final_output.len())
        .map_err(|_overflow| ChunkOutputValidationError::OutputLengthMismatch)?;
    if actual_len != declared_uncompressed_size {
        return Err(ChunkOutputValidationError::OutputLengthMismatch);
    }

    let checksum = if declared_uncompressed_crc == 0 {
        OptionalCrcValidation::NotProvided
    } else if crc32fast::hash(final_output) == declared_uncompressed_crc {
        OptionalCrcValidation::Verified
    } else {
        return Err(ChunkOutputValidationError::ChunkChecksumMismatch);
    };

    Ok(ValidatedChunkOutput {
        bytes: final_output,
        checksum,
    })
}

fn validate_exact_read(
    read: &RemoteMcapSlice<'_>,
    expected: &Range<u64>,
) -> Result<(), FixedLayoutError> {
    let expected_len = expected
        .end
        .checked_sub(expected.start)
        .ok_or(FixedLayoutError::UnexpectedReadRange)?;
    let actual_len = u64::try_from(read.bytes.len())
        .map_err(|_overflow| FixedLayoutError::UnexpectedReadRange)?;
    let actual_end = read
        .offset
        .checked_add(actual_len)
        .ok_or(FixedLayoutError::UnexpectedReadRange)?;
    if read.offset != expected.start || actual_end != expected.end || actual_len != expected_len {
        return Err(FixedLayoutError::UnexpectedReadRange);
    }
    Ok(())
}

fn read_u64(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(
        bytes
            .try_into()
            .expect("fixed-layout u64 field has an exact slice"),
    )
}

fn read_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(
        bytes
            .try_into()
            .expect("fixed-layout u32 field has an exact slice"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{AdversarialMcapFixtureBuilder, FixtureCrc};

    fn fixture(
        summary_crc: FixtureCrc,
        summary_offsets: bool,
    ) -> crate::testing::AdversarialMcapFixture {
        AdversarialMcapFixtureBuilder::new()
            .with_summary_crc(summary_crc)
            .with_summary_offsets(summary_offsets)
            .build()
            .expect("valid adversarial fixture")
    }

    fn prepare(bytes: &[u8]) -> Result<PreparedFixedLayout, FixedLayoutError> {
        let object_len = u64::try_from(bytes.len()).expect("fixture length fits u64");
        let tail_start = bytes.len() - FOOTER_TAIL_LEN;
        prepare_fixed_layout(
            object_len,
            RemoteMcapSlice::new(0, &bytes[..HEADER_PREFIX_LEN]),
            RemoteMcapSlice::new(
                u64::try_from(tail_start).expect("fixture offset fits u64"),
                &bytes[tail_start..],
            ),
        )
    }

    fn validate(bytes: &[u8]) -> Result<ValidatedFixedLayout<'_>, FixedLayoutError> {
        let prepared = prepare(bytes)?;
        let range = prepared.data_end_and_summary_range();
        let start = usize::try_from(range.start).expect("fixture offset fits usize");
        let end = usize::try_from(range.end).expect("fixture offset fits usize");
        prepared
            .validate_data_end_and_summary(RemoteMcapSlice::new(range.start, &bytes[start..end]))
    }

    fn future_summary_parser_input(validated: ValidatedFixedLayout<'_>) -> &[u8] {
        validated.into_summary_bytes()
    }

    #[test]
    fn valid_fixture_edges_and_crc_states_match_the_independent_oracle() {
        for summary_crc in [FixtureCrc::Zero, FixtureCrc::ValidNonZero] {
            for summary_offsets in [false, true] {
                let fixture = fixture(summary_crc, summary_offsets);
                let footer = mcap::read::footer(&fixture.bytes).expect("reference Footer fields");
                let data_end = fixture.layout.data_end.expect("fixture DataEnd");
                let footer_span = fixture.layout.footer.expect("fixture Footer");
                let prepared = prepare(&fixture.bytes).unwrap();

                assert_eq!(
                    prepared.header_body_range().start,
                    u64::try_from(HEADER_PREFIX_LEN).unwrap()
                );
                assert_eq!(prepared.summary_start(), footer.summary_start);
                assert_eq!(
                    prepared.summary_offset_start(),
                    (footer.summary_offset_start != 0).then_some(footer.summary_offset_start)
                );

                let range = prepared.data_end_and_summary_range();
                let start = usize::try_from(range.start).unwrap();
                let end = usize::try_from(range.end).unwrap();
                let validated = prepared
                    .validate_data_end_and_summary(RemoteMcapSlice::new(
                        range.start,
                        &fixture.bytes[start..end],
                    ))
                    .unwrap();

                assert_eq!(
                    validated.header_body_range().start,
                    u64::try_from(HEADER_PREFIX_LEN).unwrap()
                );
                assert_eq!(
                    validated.summary_range(),
                    footer.summary_start..u64::try_from(footer_span.start).unwrap()
                );
                assert_eq!(
                    validated.summary_offset_start(),
                    (footer.summary_offset_start != 0).then_some(footer.summary_offset_start)
                );
                assert_eq!(
                    validated.summary_crc(),
                    match summary_crc {
                        FixtureCrc::Zero => OptionalCrcValidation::NotProvided,
                        FixtureCrc::ValidNonZero => OptionalCrcValidation::Verified,
                        FixtureCrc::InvalidNonZero => unreachable!(),
                    }
                );
                assert_eq!(
                    validated.data_section_crc(),
                    DataSectionCrcValidation::NotVerifiedByPartialRead
                );
                assert_eq!(
                    u64::try_from(data_end.start).unwrap(),
                    footer.summary_start - u64::try_from(DATA_END_RECORD_LEN).unwrap()
                );
            }
        }
    }

    #[test]
    fn validated_capability_exposes_only_the_exact_checked_summary_subslice() {
        let fixture = fixture(FixtureCrc::ValidNonZero, true);
        let bytes = fixture.bytes;
        let data_end = fixture.layout.data_end.expect("fixture DataEnd");
        let footer = fixture.layout.footer.expect("fixture Footer");
        let expected = &bytes[fixture.layout.summary_start..footer.start];
        let unrelated_same_bytes = bytes.clone();

        let validated = validate(&bytes).unwrap();
        let parser_input = future_summary_parser_input(validated);

        assert_eq!(parser_input, expected);
        assert!(std::ptr::eq(parser_input.as_ptr(), expected.as_ptr()));
        assert!(!std::ptr::eq(
            parser_input.as_ptr(),
            unrelated_same_bytes[fixture.layout.summary_start..footer.start].as_ptr()
        ));
        assert_eq!(data_end.end, fixture.layout.summary_start);
        assert_eq!(parser_input.len(), footer.start - data_end.end);
    }

    #[test]
    fn independently_valid_same_range_buffers_cannot_be_cross_wired() {
        let first = fixture(FixtureCrc::ValidNonZero, true);
        let mut second_bytes = first.bytes.clone();
        let footer = first.layout.footer.expect("fixture Footer");
        second_bytes[first.layout.summary_start] ^= 0x80;
        let second_crc =
            crc32fast::hash(&second_bytes[first.layout.summary_start..footer.body_start + 16]);
        assert_ne!(second_crc, 0);
        second_bytes[footer.body_start + 16..footer.body_start + 20]
            .copy_from_slice(&second_crc.to_le_bytes());

        let first_prepared = prepare(&first.bytes).unwrap();
        let first_range = first_prepared.data_end_and_summary_range();
        let start = usize::try_from(first_range.start).unwrap();
        let end = usize::try_from(first_range.end).unwrap();
        assert_eq!(
            first_prepared
                .validate_data_end_and_summary(RemoteMcapSlice::new(
                    first_range.start,
                    &second_bytes[start..end],
                ))
                .unwrap_err(),
            FixedLayoutError::SummaryChecksumMismatch
        );

        let second_validated = validate(&second_bytes).unwrap();
        let expected = &second_bytes[first.layout.summary_start..footer.start];
        assert!(std::ptr::eq(
            second_validated.summary_bytes().as_ptr(),
            expected.as_ptr()
        ));
    }

    #[test]
    fn summary_checksum_uses_the_entire_specification_coverage() {
        let fixture = fixture(FixtureCrc::ValidNonZero, true);
        let summary_start = fixture.layout.summary_start;
        let footer = fixture.layout.footer.expect("fixture Footer");

        let mut changed_summary_envelope = fixture.bytes.clone();
        changed_summary_envelope[summary_start] ^= 0x80;
        assert_eq!(
            validate(&changed_summary_envelope).unwrap_err(),
            FixedLayoutError::SummaryChecksumMismatch
        );

        let mut changed_footer_field = fixture.bytes.clone();
        let offset_field = footer.body_start + 8;
        let old_offset = read_u64(&changed_footer_field[offset_field..offset_field + 8]);
        let replacement = old_offset + 1;
        assert!(replacement < u64::try_from(footer.start).unwrap());
        changed_footer_field[offset_field..offset_field + 8]
            .copy_from_slice(&replacement.to_le_bytes());
        assert_eq!(
            validate(&changed_footer_field).unwrap_err(),
            FixedLayoutError::SummaryChecksumMismatch
        );
    }

    #[test]
    fn a_zero_summary_crc_is_not_claimed_as_verified() {
        let fixture = fixture(FixtureCrc::Zero, true);
        let mut bytes = fixture.bytes;
        bytes[fixture.layout.summary_start] ^= 0x80;
        assert_eq!(
            validate(&bytes).unwrap().summary_crc(),
            OptionalCrcValidation::NotProvided
        );
    }

    #[test]
    fn invalid_nonzero_summary_crc_is_typed_and_redacted() {
        let fixture = fixture(FixtureCrc::InvalidNonZero, true);
        let error = validate(&fixture.bytes).unwrap_err();
        assert_eq!(error, FixedLayoutError::SummaryChecksumMismatch);
        assert_eq!(
            error.to_string(),
            "remote MCAP Summary checksum does not match"
        );
    }

    #[test]
    fn data_end_crc_is_deliberately_not_checked_by_the_partial_read() {
        let fixture = fixture(FixtureCrc::ValidNonZero, true);
        let mut bytes = fixture.bytes;
        let data_end = fixture.layout.data_end.expect("fixture DataEnd");
        bytes[data_end.body_start..data_end.end].copy_from_slice(&0xfeed_beef_u32.to_le_bytes());

        let validated = validate(&bytes).unwrap();
        assert_eq!(
            validated.data_section_crc(),
            DataSectionCrcValidation::NotVerifiedByPartialRead
        );
        assert_eq!(validated.summary_crc(), OptionalCrcValidation::Verified);
    }

    #[test]
    fn every_fixed_opcode_and_body_length_is_checked() {
        let fixture = fixture(FixtureCrc::Zero, true);

        let mut bad_header_opcode = fixture.bytes.clone();
        bad_header_opcode[MAGIC_LEN] = mcap::records::op::CHANNEL;
        assert_eq!(
            prepare(&bad_header_opcode).unwrap_err(),
            FixedLayoutError::InvalidHeaderOpcode
        );

        let footer = fixture.layout.footer.expect("fixture Footer");
        let mut bad_footer_opcode = fixture.bytes.clone();
        bad_footer_opcode[footer.start] = mcap::records::op::HEADER;
        assert_eq!(
            prepare(&bad_footer_opcode).unwrap_err(),
            FixedLayoutError::InvalidFooterOpcode
        );

        let mut bad_footer_length = fixture.bytes.clone();
        bad_footer_length[footer.start + 1..footer.body_start]
            .copy_from_slice(&19_u64.to_le_bytes());
        assert_eq!(
            prepare(&bad_footer_length).unwrap_err(),
            FixedLayoutError::InvalidFooterBodyLength
        );

        let data_end = fixture.layout.data_end.expect("fixture DataEnd");
        let mut bad_data_end_opcode = fixture.bytes.clone();
        bad_data_end_opcode[data_end.start] = mcap::records::op::CHANNEL;
        assert_eq!(
            validate(&bad_data_end_opcode).unwrap_err(),
            FixedLayoutError::InvalidDataEndOpcode
        );

        let mut bad_data_end_length = fixture.bytes;
        bad_data_end_length[data_end.start + 1..data_end.body_start]
            .copy_from_slice(&3_u64.to_le_bytes());
        assert_eq!(
            validate(&bad_data_end_length).unwrap_err(),
            FixedLayoutError::InvalidDataEndBodyLength
        );
    }

    #[test]
    fn leading_and_trailing_magic_are_checked_independently() {
        let fixture = fixture(FixtureCrc::Zero, false);

        let mut bad_start = fixture.bytes.clone();
        bad_start[0] ^= 1;
        assert_eq!(
            prepare(&bad_start).unwrap_err(),
            FixedLayoutError::InvalidStartingMagic
        );

        let mut bad_end = fixture.bytes;
        *bad_end.last_mut().unwrap() ^= 1;
        assert_eq!(
            prepare(&bad_end).unwrap_err(),
            FixedLayoutError::InvalidTrailingMagic
        );
    }

    #[test]
    fn summary_and_header_ranges_use_checked_arithmetic() {
        let fixture = fixture(FixtureCrc::Zero, true);
        let footer = fixture.layout.footer.expect("fixture Footer");

        let set_summary_start = |bytes: &mut [u8], value: u64| {
            bytes[footer.body_start..footer.body_start + 8].copy_from_slice(&value.to_le_bytes());
        };
        let set_summary_offset = |bytes: &mut [u8], value: u64| {
            bytes[footer.body_start + 8..footer.body_start + 16]
                .copy_from_slice(&value.to_le_bytes());
        };

        let mut missing = fixture.bytes.clone();
        set_summary_start(&mut missing, 0);
        assert_eq!(
            prepare(&missing).unwrap_err(),
            FixedLayoutError::SummaryRequired
        );

        let mut underflow = fixture.bytes.clone();
        set_summary_start(
            &mut underflow,
            u64::try_from(DATA_END_RECORD_LEN - 1).unwrap(),
        );
        assert_eq!(
            prepare(&underflow).unwrap_err(),
            FixedLayoutError::InvalidSummaryRange
        );

        let mut after_footer = fixture.bytes.clone();
        set_summary_start(&mut after_footer, u64::try_from(footer.start).unwrap());
        assert_eq!(
            prepare(&after_footer).unwrap_err(),
            FixedLayoutError::InvalidSummaryRange
        );

        let mut offset_before_summary = fixture.bytes.clone();
        set_summary_offset(
            &mut offset_before_summary,
            u64::try_from(fixture.layout.summary_start - 1).unwrap(),
        );
        assert_eq!(
            prepare(&offset_before_summary).unwrap_err(),
            FixedLayoutError::InvalidSummaryOffsetRange
        );

        let mut offset_at_footer = fixture.bytes.clone();
        set_summary_offset(&mut offset_at_footer, u64::try_from(footer.start).unwrap());
        assert_eq!(
            prepare(&offset_at_footer).unwrap_err(),
            FixedLayoutError::InvalidSummaryOffsetRange
        );

        let mut header_overflow = fixture.bytes.clone();
        header_overflow[MAGIC_LEN + 1..HEADER_PREFIX_LEN].copy_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(
            prepare(&header_overflow).unwrap_err(),
            FixedLayoutError::InvalidHeaderRange
        );

        let mut header_overlap = fixture.bytes;
        let data_end = header_overlap.len() - FOOTER_TAIL_LEN - 1;
        let overlapping_len = u64::try_from(data_end - HEADER_PREFIX_LEN + 1).unwrap();
        header_overlap[MAGIC_LEN + 1..HEADER_PREFIX_LEN]
            .copy_from_slice(&overlapping_len.to_le_bytes());
        assert_eq!(
            prepare(&header_overlap).unwrap_err(),
            FixedLayoutError::InvalidHeaderRange
        );
    }

    #[test]
    fn every_partial_read_must_cover_its_exact_absolute_range() {
        let fixture = fixture(FixtureCrc::Zero, true);
        let bytes = &fixture.bytes;
        let object_len = u64::try_from(bytes.len()).unwrap();
        let tail_start = bytes.len() - FOOTER_TAIL_LEN;

        assert_eq!(
            prepare_fixed_layout(
                object_len,
                RemoteMcapSlice::new(1, &bytes[..HEADER_PREFIX_LEN]),
                RemoteMcapSlice::new(u64::try_from(tail_start).unwrap(), &bytes[tail_start..]),
            )
            .unwrap_err(),
            FixedLayoutError::UnexpectedReadRange
        );
        assert_eq!(
            prepare_fixed_layout(
                object_len,
                RemoteMcapSlice::new(0, &bytes[..HEADER_PREFIX_LEN]),
                RemoteMcapSlice::new(u64::try_from(tail_start - 1).unwrap(), &bytes[tail_start..]),
            )
            .unwrap_err(),
            FixedLayoutError::UnexpectedReadRange
        );

        let prepared = prepare(bytes).unwrap();
        let range = prepared.data_end_and_summary_range();
        let start = usize::try_from(range.start).unwrap();
        let end = usize::try_from(range.end).unwrap();
        assert_eq!(
            prepared
                .validate_data_end_and_summary(RemoteMcapSlice::new(
                    range.start + 1,
                    &bytes[start..end],
                ))
                .unwrap_err(),
            FixedLayoutError::UnexpectedReadRange
        );
    }

    #[test]
    fn too_short_objects_are_rejected_before_slicing() {
        let bytes = [0_u8; FOOTER_TAIL_LEN];
        assert_eq!(
            prepare_fixed_layout(
                0,
                RemoteMcapSlice::new(0, &bytes[..HEADER_PREFIX_LEN]),
                RemoteMcapSlice::new(0, &bytes[..FOOTER_TAIL_LEN]),
            )
            .unwrap_err(),
            FixedLayoutError::FileTooShort
        );
    }

    #[test]
    fn chunk_checksum_is_gated_by_exact_length_and_returns_a_scanner_capability() {
        let output = b"exact final uncompressed chunk";
        let declared_size = u64::try_from(output.len()).unwrap();
        let declared_crc = crc32fast::hash(output);

        let without_crc = validate_chunk_output(declared_size, 0, output).unwrap();
        assert_eq!(without_crc.bytes(), output);
        assert_eq!(without_crc.checksum(), OptionalCrcValidation::NotProvided);

        let verified = validate_chunk_output(declared_size, declared_crc, output).unwrap();
        assert_eq!(verified.bytes(), output);
        assert_eq!(verified.checksum(), OptionalCrcValidation::Verified);

        assert_eq!(
            validate_chunk_output(declared_size + 1, declared_crc, output).unwrap_err(),
            ChunkOutputValidationError::OutputLengthMismatch
        );
        assert_eq!(
            validate_chunk_output(declared_size - 1, declared_crc, output).unwrap_err(),
            ChunkOutputValidationError::OutputLengthMismatch
        );
        assert_eq!(
            validate_chunk_output(declared_size, declared_crc ^ 1, output).unwrap_err(),
            ChunkOutputValidationError::ChunkChecksumMismatch
        );
    }
}
