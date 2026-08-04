use std::collections::{BTreeMap, BTreeSet};
use std::io::Read as _;

use mcap::records::op;

use super::{
    AdversarialMcapFixture, CompressionFixture, DefinitionFixture, FakeComponentOutcome,
    FixtureBuildError, FixtureCardinality, FixtureCrc, McapFormatViolation, MessageIndexFault,
    NestedCollectionLocation, NestedCollectionShape, NestedCollectionTarget, RawTimeRange,
    RemotePolicyOutcome, StatisticsFixture, UpstreamIndexedMessage, UpstreamIndexedSelection,
};

const RECORD_HEADER_LEN: usize = 9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceSection {
    Data,
    Summary,
    Chunk { chunk_index: usize },
    MessageIndex { chunk_index: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReferenceRecordSpan {
    pub opcode: u8,
    pub start: usize,
    pub body_start: usize,
    pub declared_body_len: usize,
    pub available_end: usize,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReferenceSchema {
    pub id: u16,
    pub name: String,
    pub encoding: String,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct ReferenceChannel {
    pub id: u16,
    pub schema_id: u16,
    pub topic: String,
    pub message_encoding: String,
    pub metadata: BTreeMap<String, String>,
    pub record_start: usize,
}

impl PartialEq for ReferenceChannel {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.schema_id == other.schema_id
            && self.topic == other.topic
            && self.message_encoding == other.message_encoding
            && self.metadata == other.metadata
    }
}

impl Eq for ReferenceChannel {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReferenceMessage {
    pub channel_id: u16,
    pub sequence: u32,
    pub log_time: u64,
    pub publish_time: u64,
    pub data: Vec<u8>,
    pub record_offset: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReferenceMessageIndexEntry {
    pub log_time: u64,
    pub offset: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReferenceMessageIndex {
    pub span: ReferenceRecordSpan,
    pub chunk_index: usize,
    pub channel_id: u16,
    pub entries: Vec<ReferenceMessageIndexEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReferenceChunk {
    pub span: ReferenceRecordSpan,
    pub message_range: RawTimeRange,
    pub declared_uncompressed_size: u64,
    pub declared_uncompressed_crc: u32,
    pub compression: String,
    pub compressed_data_start: usize,
    pub compressed_data: Vec<u8>,
    pub first_zstd_frame_len: Option<usize>,
    pub uncompressed_data: Option<Vec<u8>>,
    pub decompression_error: Option<String>,
    pub actual_uncompressed_crc: Option<u32>,
    pub inner_records: Vec<ReferenceRecordSpan>,
    pub schemas: Vec<ReferenceSchema>,
    pub channels: Vec<ReferenceChannel>,
    pub messages: Vec<ReferenceMessage>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReferenceChunkIndex {
    pub message_range: RawTimeRange,
    pub chunk_start_offset: u64,
    pub chunk_length: u64,
    pub message_index_offsets: BTreeMap<u16, u64>,
    pub message_index_length: u64,
    pub compression: String,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReferenceStatistics {
    pub message_count: u64,
    pub schema_count: u16,
    pub channel_count: u32,
    pub attachment_count: u32,
    pub metadata_count: u32,
    pub chunk_count: u32,
    pub message_start_time: u64,
    pub message_end_time: u64,
    pub channel_message_counts: BTreeMap<u16, u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReferenceFooter {
    pub span: ReferenceRecordSpan,
    pub summary_start: u64,
    pub summary_offset_start: u64,
    pub summary_crc: u32,
    pub actual_summary_crc: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReferenceNestedCollection {
    pub target: NestedCollectionTarget,
    pub section: ReferenceSection,
    pub record_start: usize,
    pub declared_encoded_bytes: u32,
    pub available_encoded_bytes: usize,
    pub entry_count: usize,
    pub max_string_bytes: usize,
    pub retained_bytes: usize,
    pub element_width_valid: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReferenceInspection {
    pub starts_with_magic: bool,
    pub ends_with_magic: bool,
    pub records: Vec<ReferenceRecordSpan>,
    pub truncated_record: Option<ReferenceRecordSpan>,
    pub chunks: Vec<ReferenceChunk>,
    pub message_indexes: Vec<ReferenceMessageIndex>,
    pub chunk_indexes: Vec<ReferenceChunkIndex>,
    pub summary_schemas: Vec<ReferenceSchema>,
    pub summary_channels: Vec<ReferenceChannel>,
    pub statistics: Option<ReferenceStatistics>,
    pub top_level_messages: Vec<ReferenceMessage>,
    pub nested_collections: Vec<ReferenceNestedCollection>,
    pub summary_offset_count: usize,
    pub data_end: Option<ReferenceRecordSpan>,
    pub footer: Option<ReferenceFooter>,
}

impl ReferenceInspection {
    pub fn inspect(bytes: &[u8]) -> Result<Self, FixtureBuildError> {
        let starts_with_magic = bytes.starts_with(mcap::MAGIC);
        let ends_with_magic = bytes.ends_with(mcap::MAGIC);
        let records_start = usize::from(starts_with_magic) * mcap::MAGIC.len();
        let records_end = bytes
            .len()
            .saturating_sub(usize::from(ends_with_magic) * mcap::MAGIC.len());
        let walked = walk_records(
            bytes.get(records_start..records_end).unwrap_or_default(),
            records_start,
        )?;
        let footer_raw = walked
            .records
            .iter()
            .rev()
            .find(|record| record.span.opcode == op::FOOTER && !record.span.truncated);
        let footer_fields = footer_raw
            .map(|record| parse_footer(&record.body))
            .transpose()?;
        let inferred_summary_start = walked
            .records
            .iter()
            .find(|record| record.span.opcode == op::DATA_END && !record.span.truncated)
            .map(|record| record.span.available_end as u64);
        let summary_start = footer_fields
            .map(|footer| footer.0)
            .or(inferred_summary_start)
            .unwrap_or(u64::MAX);

        let mut inspection = Self {
            starts_with_magic,
            ends_with_magic,
            records: walked.records.iter().map(|record| record.span).collect(),
            truncated_record: walked.truncated,
            ..Self::default()
        };

        for raw in &walked.records {
            let section = if raw.span.start as u64 >= summary_start {
                ReferenceSection::Summary
            } else {
                ReferenceSection::Data
            };
            if raw.span.truncated {
                let section = if raw.span.opcode == op::MESSAGE_INDEX {
                    ReferenceSection::MessageIndex {
                        chunk_index: owning_chunk_index(&inspection.chunks, raw.span.start),
                    }
                } else {
                    section
                };
                if let Some(nested) = parse_truncated_nested_collection(raw, section)? {
                    inspection.nested_collections.push(nested);
                }
                continue;
            }
            match raw.span.opcode {
                op::SCHEMA if section == ReferenceSection::Summary => {
                    inspection.summary_schemas.push(parse_schema(&raw.body)?);
                }
                op::CHANNEL if section == ReferenceSection::Summary => {
                    let (channel, nested) = parse_channel(&raw.body, raw.span.start, section)?;
                    inspection.summary_channels.push(channel);
                    inspection.nested_collections.push(nested);
                }
                op::MESSAGE if section == ReferenceSection::Data => {
                    inspection
                        .top_level_messages
                        .push(parse_message(&raw.body, raw.span.start as u64)?);
                }
                op::CHUNK => {
                    let chunk_index = inspection.chunks.len();
                    let chunk = parse_chunk(raw, chunk_index, &mut inspection.nested_collections)?;
                    inspection.chunks.push(chunk);
                }
                op::MESSAGE_INDEX => {
                    let chunk_index = owning_chunk_index(&inspection.chunks, raw.span.start);
                    let (index, nested) = parse_message_index(
                        &raw.body,
                        raw.span,
                        ReferenceSection::MessageIndex { chunk_index },
                    )?;
                    inspection.message_indexes.push(index);
                    inspection.nested_collections.push(nested);
                }
                op::CHUNK_INDEX if section == ReferenceSection::Summary => {
                    let (index, nested) = parse_chunk_index(&raw.body, raw.span.start, section)?;
                    inspection.chunk_indexes.push(index);
                    inspection.nested_collections.push(nested);
                }
                op::STATISTICS if section == ReferenceSection::Summary => {
                    let (statistics, nested) =
                        parse_statistics(&raw.body, raw.span.start, section)?;
                    inspection.statistics = Some(statistics);
                    inspection.nested_collections.push(nested);
                }
                op::SUMMARY_OFFSET => inspection.summary_offset_count += 1,
                op::DATA_END => inspection.data_end = Some(raw.span),
                _ => {}
            }
        }

        if let (Some(raw), Some((summary_start, summary_offset_start, summary_crc))) =
            (footer_raw, footer_fields)
        {
            let covered_end = raw
                .span
                .body_start
                .checked_add(16)
                .ok_or_else(|| FixtureBuildError("Footer CRC coverage overflow".to_owned()))?;
            let covered = bytes
                .get(summary_start as usize..covered_end)
                .ok_or_else(|| {
                    FixtureBuildError("Footer CRC coverage is outside bytes".to_owned())
                })?;
            inspection.footer = Some(ReferenceFooter {
                span: raw.span,
                summary_start,
                summary_offset_start,
                summary_crc,
                actual_summary_crc: crc32fast::hash(covered),
            });
        }
        Ok(inspection)
    }

    pub fn canonical_summary_schemas(
        &self,
    ) -> Result<BTreeMap<u16, ReferenceSchema>, FixtureBuildError> {
        canonical_by_id(&self.summary_schemas, |schema| schema.id, "Schema")
    }

    pub fn canonical_summary_channels(
        &self,
    ) -> Result<BTreeMap<u16, ReferenceChannel>, FixtureBuildError> {
        canonical_by_id(&self.summary_channels, |channel| channel.id, "Channel")
    }

    pub fn expected_indexed_selection(
        &self,
        start: Option<u64>,
        end: Option<u64>,
    ) -> UpstreamIndexedSelection {
        let mut selected_indexes: Vec<_> = self
            .chunk_indexes
            .iter()
            .filter(|index| {
                start.is_none_or(|start| index.message_range.end >= start)
                    && end.is_none_or(|end| index.message_range.start < end)
            })
            .collect();
        // `IndexedReader` requests Chunks in declared log-time order, preserving Summary order
        // for equal starts. This is intentionally independent from physical Chunk order.
        selected_indexes.sort_by_key(|index| index.message_range.start);
        let chunks_by_start: BTreeMap<_, _> = self
            .chunks
            .iter()
            .map(|chunk| (chunk.span.start as u64, chunk))
            .collect();
        let mut messages: Vec<_> = selected_indexes
            .iter()
            .filter_map(|index| chunks_by_start.get(&index.chunk_start_offset).copied())
            .flat_map(|chunk| &chunk.messages)
            .filter(|message| {
                start.is_none_or(|start| message.log_time >= start)
                    && end.is_none_or(|end| message.log_time < end)
            })
            .map(|message| UpstreamIndexedMessage {
                channel_id: message.channel_id,
                sequence: message.sequence,
                log_time: message.log_time,
                publish_time: message.publish_time,
                data: message.data.clone(),
            })
            .collect();
        messages.sort_by_key(|message| message.log_time);
        let chunk_payload_offsets = selected_indexes
            .into_iter()
            .filter_map(|index| chunks_by_start.get(&index.chunk_start_offset))
            .map(|chunk| chunk.compressed_data_start as u64)
            .collect();
        UpstreamIndexedSelection {
            chunk_payload_offsets,
            messages,
        }
    }
}

pub(super) fn validate_fixture(fixture: &AdversarialMcapFixture) -> Result<(), FixtureBuildError> {
    let inspection = ReferenceInspection::inspect(&fixture.bytes)?;
    if !inspection.starts_with_magic {
        return Err(FixtureBuildError(
            "reference inspector found no leading MCAP magic".to_owned(),
        ));
    }
    let expects_premature_eof = fixture
        .mcap_format_violations
        .contains(&McapFormatViolation::PrematureEof);
    if expects_premature_eof {
        let inner_truncated = inspection
            .chunks
            .iter()
            .flat_map(|chunk| &chunk.inner_records)
            .any(|record| record.truncated);
        if inspection.truncated_record.is_none() && !inner_truncated {
            return Err(FixtureBuildError(
                "PrematureEof fixture did not truncate a serialized record".to_owned(),
            ));
        }
    }
    if !expects_premature_eof
        && (!inspection.ends_with_magic || inspection.truncated_record.is_some())
    {
        return Err(FixtureBuildError(
            "reference inspector found an unexpected truncated MCAP".to_owned(),
        ));
    }

    validate_crc(fixture, &inspection)?;
    validate_nested(fixture, &inspection)?;
    validate_compression(fixture, &inspection)?;
    validate_time_controls(fixture, &inspection)?;
    if expects_premature_eof && inspection.truncated_record.is_some() {
        validate_attached_outcomes(fixture, &inspection)?;
        return Ok(());
    }
    validate_summary_offsets(fixture, &inspection)?;
    validate_message_indexes(fixture, &inspection)?;
    validate_statistics(fixture, &inspection)?;
    validate_definitions(fixture, &inspection)?;
    validate_physical_layout(fixture, &inspection)?;
    validate_cardinality(fixture.cardinality, &inspection)?;
    validate_decoder_and_partition_bytes(fixture, &inspection)?;
    validate_attached_outcomes(fixture, &inspection)?;
    Ok(())
}

fn validate_time_controls(
    fixture: &AdversarialMcapFixture,
    inspection: &ReferenceInspection,
) -> Result<(), FixtureBuildError> {
    let mut declared_range_mismatch = false;
    let mut invalid_temporal_value = false;
    for (position, chunk) in inspection.chunks.iter().enumerate() {
        invalid_temporal_value |= raw_range_is_invalid(chunk.message_range);
        let descriptor = inspection.chunk_indexes.get(position);
        if let Some(descriptor) = descriptor {
            invalid_temporal_value |= raw_range_is_invalid(descriptor.message_range);
        }

        let actual_log_times: Vec<_> = if chunk_record_stream_is_complete(chunk) {
            chunk
                .messages
                .iter()
                .map(|message| message.log_time)
                .collect()
        } else {
            message_indexes_for_chunk(inspection, position)
                .into_iter()
                .flat_map(|index| index.entries.iter().map(|entry| entry.log_time))
                .collect()
        };
        let actual_range = raw_time_extent(&actual_log_times);
        declared_range_mismatch |= chunk.message_range != actual_range;
        if let Some(descriptor) = descriptor {
            declared_range_mismatch |= descriptor.message_range != actual_range;
        }

        for message in &chunk.messages {
            invalid_temporal_value |=
                message.log_time > i64::MAX as u64 || message.publish_time > i64::MAX as u64;
        }
        if fixture.message_index_fault == MessageIndexFault::None
            && chunk_record_stream_is_complete(chunk)
            && !fixture.nested_collection.is_some_and(|nested| {
                nested.shape == NestedCollectionShape::PrematureEof
                    && fixture.nested_collection_location
                        == Some(NestedCollectionLocation::MessageIndexRegion {
                            chunk_index: position,
                        })
            })
        {
            let mut message_times: Vec<_> = chunk
                .messages
                .iter()
                .map(|message| message.log_time)
                .collect();
            let mut index_times: Vec<_> = message_indexes_for_chunk(inspection, position)
                .into_iter()
                .flat_map(|index| index.entries.iter().map(|entry| entry.log_time))
                .collect();
            message_times.sort_unstable();
            index_times.sort_unstable();
            if message_times != index_times {
                return Err(FixtureBuildError(
                    "MessageIndex log times differ from serialized Messages".to_owned(),
                ));
            }
        }
    }
    for index in &inspection.message_indexes {
        invalid_temporal_value |= index
            .entries
            .iter()
            .any(|entry| entry.log_time > i64::MAX as u64);
    }
    invalid_temporal_value |= inspection.top_level_messages.iter().any(|message| {
        message.log_time > i64::MAX as u64 || message.publish_time > i64::MAX as u64
    });

    let labeled_range_mismatch = fixture
        .mcap_format_violations
        .contains(&McapFormatViolation::ChunkDeclaredTimeRange);
    if labeled_range_mismatch != declared_range_mismatch {
        return Err(FixtureBuildError(
            "Chunk declared-range label differs from independently parsed time extent".to_owned(),
        ));
    }
    let labeled_invalid_temporal = fixture
        .remote_expected_outcomes
        .contains(&RemotePolicyOutcome::InvalidTemporalValue);
    if labeled_invalid_temporal != invalid_temporal_value {
        return Err(FixtureBuildError(
            "InvalidTemporalValue outcome differs from independently parsed raw times".to_owned(),
        ));
    }
    Ok(())
}

fn raw_range_is_invalid(range: RawTimeRange) -> bool {
    range.start > i64::MAX as u64 || range.end > i64::MAX as u64
}

fn raw_time_extent(times: &[u64]) -> RawTimeRange {
    let Some((&first, rest)) = times.split_first() else {
        return RawTimeRange::new(0, 0);
    };
    rest.iter()
        .fold(RawTimeRange::new(first, first), |range, time| {
            RawTimeRange::new(range.start.min(*time), range.end.max(*time))
        })
}

fn message_indexes_for_chunk(
    inspection: &ReferenceInspection,
    chunk_index: usize,
) -> Vec<&ReferenceMessageIndex> {
    inspection
        .message_indexes
        .iter()
        .filter(|index| index.chunk_index == chunk_index)
        .collect()
}

fn validate_summary_offsets(
    fixture: &AdversarialMcapFixture,
    inspection: &ReferenceInspection,
) -> Result<(), FixtureBuildError> {
    let footer = inspection
        .footer
        .as_ref()
        .ok_or_else(|| FixtureBuildError("SummaryOffset control has no Footer".to_owned()))?;
    let present = footer.summary_offset_start != 0 && inspection.summary_offset_count > 0;
    let absent = footer.summary_offset_start == 0 && inspection.summary_offset_count == 0;
    if (fixture.summary_offsets && !present) || (!fixture.summary_offsets && !absent) {
        return Err(FixtureBuildError(
            "serialized SummaryOffset presence differs from requested control".to_owned(),
        ));
    }
    Ok(())
}

pub(super) fn validate_upstream_differential(
    fixture: &AdversarialMcapFixture,
) -> Result<(), FixtureBuildError> {
    let reference = ReferenceInspection::inspect(&fixture.bytes)?;
    let summary = fixture
        .read_upstream_summary()?
        .ok_or_else(|| FixtureBuildError("format-valid fixture has no Summary".to_owned()))?;
    let schemas = reference.canonical_summary_schemas()?;
    if summary.schemas.len() != schemas.len() {
        return Err(FixtureBuildError(format!(
            "upstream/reference Schema count differs: {} != {}",
            summary.schemas.len(),
            schemas.len()
        )));
    }
    for (id, expected) in schemas {
        let actual = summary
            .schemas
            .get(&id)
            .ok_or_else(|| FixtureBuildError(format!("upstream Summary omitted Schema {id}")))?;
        if actual.id != expected.id
            || actual.name != expected.name
            || actual.encoding != expected.encoding
            || actual.data.as_ref() != expected.data
        {
            return Err(FixtureBuildError(format!(
                "upstream/reference Schema {id} differs"
            )));
        }
    }

    let channels = reference.canonical_summary_channels()?;
    if summary.channels.len() != channels.len() {
        return Err(FixtureBuildError(format!(
            "upstream/reference Channel count differs: {} != {}",
            summary.channels.len(),
            channels.len()
        )));
    }
    for (id, expected) in channels {
        let actual = summary
            .channels
            .get(&id)
            .ok_or_else(|| FixtureBuildError(format!("upstream Summary omitted Channel {id}")))?;
        if actual.id != expected.id
            || actual.topic != expected.topic
            || actual.message_encoding != expected.message_encoding
            || actual.metadata != expected.metadata
            || actual.schema.as_ref().map(|schema| schema.id)
                != (expected.schema_id != 0).then_some(expected.schema_id)
        {
            return Err(FixtureBuildError(format!(
                "upstream/reference Channel {id} differs"
            )));
        }
    }

    let expected_statistics = reference
        .statistics
        .as_ref()
        .ok_or_else(|| FixtureBuildError("reference projection has no Statistics".to_owned()))?;
    let actual_statistics = summary
        .stats
        .as_ref()
        .ok_or_else(|| FixtureBuildError("upstream Summary has no Statistics".to_owned()))?;
    if actual_statistics.message_count != expected_statistics.message_count
        || actual_statistics.schema_count != expected_statistics.schema_count
        || actual_statistics.channel_count != expected_statistics.channel_count
        || actual_statistics.attachment_count != expected_statistics.attachment_count
        || actual_statistics.metadata_count != expected_statistics.metadata_count
        || actual_statistics.chunk_count != expected_statistics.chunk_count
        || actual_statistics.message_start_time != expected_statistics.message_start_time
        || actual_statistics.message_end_time != expected_statistics.message_end_time
        || actual_statistics.channel_message_counts != expected_statistics.channel_message_counts
    {
        return Err(FixtureBuildError(
            "upstream/reference Statistics differs".to_owned(),
        ));
    }

    if summary.chunk_indexes.len() != reference.chunk_indexes.len() {
        return Err(FixtureBuildError(format!(
            "upstream/reference ChunkIndex count differs: {} != {}",
            summary.chunk_indexes.len(),
            reference.chunk_indexes.len()
        )));
    }
    for (actual, expected) in summary.chunk_indexes.iter().zip(&reference.chunk_indexes) {
        if actual.message_start_time != expected.message_range.start
            || actual.message_end_time != expected.message_range.end
            || actual.chunk_start_offset != expected.chunk_start_offset
            || actual.chunk_length != expected.chunk_length
            || actual.message_index_offsets != expected.message_index_offsets
            || actual.message_index_length != expected.message_index_length
            || actual.compression != expected.compression
            || actual.compressed_size != expected.compressed_size
            || actual.uncompressed_size != expected.uncompressed_size
        {
            return Err(FixtureBuildError(
                "upstream/reference ChunkIndex differs".to_owned(),
            ));
        }
    }
    if !summary.attachment_indexes.is_empty() || !summary.metadata_indexes.is_empty() {
        return Err(FixtureBuildError(
            "upstream Summary found unexpected attachment/metadata indexes".to_owned(),
        ));
    }

    let expected = reference.expected_indexed_selection(None, None);
    let actual = fixture.read_upstream_indexed(None, None)?;
    if actual != expected {
        return Err(FixtureBuildError(format!(
            "upstream/reference indexed selection differs: {actual:?} != {expected:?}"
        )));
    }
    Ok(())
}

fn validate_crc(
    fixture: &AdversarialMcapFixture,
    inspection: &ReferenceInspection,
) -> Result<(), FixtureBuildError> {
    if let Some(footer) = inspection.footer.as_ref() {
        let summary_matches = footer.summary_crc == footer.actual_summary_crc;
        match fixture.summary_crc {
            FixtureCrc::Zero if footer.summary_crc != 0 => {
                return Err(FixtureBuildError(
                    "reference Summary CRC is not zero".to_owned(),
                ));
            }
            FixtureCrc::ValidNonZero if footer.summary_crc == 0 || !summary_matches => {
                return Err(FixtureBuildError(
                    "reference Summary CRC is not valid and nonzero".to_owned(),
                ));
            }
            FixtureCrc::InvalidNonZero if footer.summary_crc == 0 || summary_matches => {
                return Err(FixtureBuildError(
                    "reference Summary CRC is not an invalid nonzero checksum".to_owned(),
                ));
            }
            _ => {}
        }
    } else if !fixture
        .mcap_format_violations
        .contains(&McapFormatViolation::PrematureEof)
    {
        return Err(FixtureBuildError(
            "reference inspector found no Footer".to_owned(),
        ));
    }
    for chunk in &inspection.chunks {
        let Some(actual) = chunk.actual_uncompressed_crc else {
            if fixture.mcap_format_violations.iter().any(|violation| {
                matches!(
                    violation,
                    McapFormatViolation::CompressedInputTruncated
                        | McapFormatViolation::CompressedOutputSize
                        | McapFormatViolation::CompressedTrailingPayload
                )
            }) {
                continue;
            }
            return Err(FixtureBuildError(
                "reference inspector could not compute Chunk CRC".to_owned(),
            ));
        };
        match fixture.chunk_crc {
            FixtureCrc::Zero if chunk.declared_uncompressed_crc != 0 => {
                return Err(FixtureBuildError(
                    "reference Chunk CRC is not zero".to_owned(),
                ));
            }
            FixtureCrc::ValidNonZero
                if chunk.declared_uncompressed_crc == 0
                    || chunk.declared_uncompressed_crc != actual =>
            {
                return Err(FixtureBuildError(
                    "reference Chunk CRC is not valid and nonzero".to_owned(),
                ));
            }
            FixtureCrc::InvalidNonZero
                if chunk.declared_uncompressed_crc == 0
                    || chunk.declared_uncompressed_crc == actual =>
            {
                return Err(FixtureBuildError(
                    "reference Chunk CRC is not an invalid nonzero checksum".to_owned(),
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_message_indexes(
    fixture: &AdversarialMcapFixture,
    inspection: &ReferenceInspection,
) -> Result<(), FixtureBuildError> {
    let Some(descriptor) = inspection.chunk_indexes.first() else {
        return Ok(());
    };
    let Some(chunk) = inspection
        .chunks
        .iter()
        .find(|chunk| chunk.span.start as u64 == descriptor.chunk_start_offset)
    else {
        return Ok(());
    };
    let region_start = descriptor.chunk_start_offset + descriptor.chunk_length;
    let region_end = region_start.saturating_add(descriptor.message_index_length);
    let records: Vec<_> = inspection
        .records
        .iter()
        .filter(|record| {
            (record.start as u64) >= region_start && (record.start as u64) < region_end
        })
        .collect();
    let indexes: Vec<_> = inspection
        .message_indexes
        .iter()
        .filter(|index| {
            (index.span.start as u64) >= region_start && (index.span.start as u64) < region_end
        })
        .collect();
    match fixture.message_index_fault {
        MessageIndexFault::None => {}
        MessageIndexFault::Missing => {
            if descriptor.message_index_length != 0
                || !descriptor.message_index_offsets.is_empty()
                || !records.is_empty()
            {
                return Err(FixtureBuildError(
                    "MessageIndexAbsent outcome has serialized index evidence".to_owned(),
                ));
            }
        }
        MessageIndexFault::Duplicate => {
            let mut channels = BTreeSet::new();
            if indexes
                .iter()
                .all(|index| channels.insert(index.channel_id))
            {
                return Err(FixtureBuildError(
                    "duplicate MessageIndex mutation is absent from bytes".to_owned(),
                ));
            }
        }
        MessageIndexFault::EntryOffset => {
            let valid_offsets: BTreeSet<_> = chunk
                .messages
                .iter()
                .map(|message| message.record_offset)
                .collect();
            if indexes
                .iter()
                .flat_map(|index| &index.entries)
                .all(|entry| valid_offsets.contains(&entry.offset))
            {
                return Err(FixtureBuildError(
                    "MessageIndex entry-offset mutation is absent from bytes".to_owned(),
                ));
            }
        }
        MessageIndexFault::EntryTime => {
            let messages: BTreeMap<_, _> = chunk
                .messages
                .iter()
                .map(|message| (message.record_offset, message.log_time))
                .collect();
            if indexes
                .iter()
                .flat_map(|index| &index.entries)
                .all(|entry| {
                    messages
                        .get(&entry.offset)
                        .is_some_and(|time| *time == entry.log_time)
                })
            {
                return Err(FixtureBuildError(
                    "MessageIndex entry-time mutation is absent from bytes".to_owned(),
                ));
            }
        }
        MessageIndexFault::WrongOpcode => {
            if records
                .iter()
                .all(|record| record.opcode == op::MESSAGE_INDEX)
            {
                return Err(FixtureBuildError(
                    "MessageIndex opcode mutation is absent from bytes".to_owned(),
                ));
            }
        }
        MessageIndexFault::MapKeyMismatch => {
            if descriptor
                .message_index_offsets
                .iter()
                .all(|(channel_id, offset)| {
                    indexes.iter().any(|index| {
                        index.span.start as u64 == *offset && index.channel_id == *channel_id
                    })
                })
            {
                return Err(FixtureBuildError(
                    "MessageIndex map-key mutation is absent from bytes".to_owned(),
                ));
            }
        }
        MessageIndexFault::RecordCrossesOwningRegion => {
            if records
                .iter()
                .all(|record| (record.body_start + record.declared_body_len) as u64 <= region_end)
            {
                return Err(FixtureBuildError(
                    "MessageIndex owning-region mutation is absent from bytes".to_owned(),
                ));
            }
        }
        MessageIndexFault::DescriptorOffsetIntoRecordBody
        | MessageIndexFault::DescriptorOffsetIntoChunk => {
            let starts: BTreeSet<_> = records.iter().map(|record| record.start as u64).collect();
            if descriptor
                .message_index_offsets
                .values()
                .all(|offset| starts.contains(offset))
            {
                return Err(FixtureBuildError(
                    "MessageIndex descriptor-offset mutation is absent from bytes".to_owned(),
                ));
            }
        }
        MessageIndexFault::DuplicateDescriptorOffset => {
            let mut offsets = BTreeSet::new();
            if descriptor
                .message_index_offsets
                .values()
                .all(|offset| offsets.insert(*offset))
            {
                return Err(FixtureBuildError(
                    "duplicate MessageIndex descriptor offset is absent from bytes".to_owned(),
                ));
            }
        }
        MessageIndexFault::CrossChunkAlias => {
            if descriptor
                .message_index_offsets
                .values()
                .all(|offset| *offset >= region_start && *offset < region_end)
            {
                return Err(FixtureBuildError(
                    "cross-Chunk MessageIndex alias is absent from bytes".to_owned(),
                ));
            }
        }
    }
    Ok(())
}

fn validate_statistics(
    fixture: &AdversarialMcapFixture,
    inspection: &ReferenceInspection,
) -> Result<(), FixtureBuildError> {
    let statistics = inspection
        .statistics
        .as_ref()
        .ok_or_else(|| FixtureBuildError("reference Statistics is absent".to_owned()))?;
    let chunk_stream_incomplete = inspection
        .chunks
        .iter()
        .any(|chunk| !chunk_record_stream_is_complete(chunk));
    // When an intentionally malformed compressed payload cannot be decoded, its otherwise valid
    // MessageIndex records are the independent serialized oracle for the hidden Message records.
    // Treating the failed decode as an empty Chunk would manufacture a Statistics violation.
    let actual_message_count = if chunk_stream_incomplete {
        inspection
            .message_indexes
            .iter()
            .map(|index| index.entries.len() as u64)
            .sum::<u64>()
            + inspection.top_level_messages.len() as u64
    } else {
        inspection
            .chunks
            .iter()
            .map(|chunk| chunk.messages.len() as u64)
            .sum::<u64>()
            + inspection.top_level_messages.len() as u64
    };
    let actual_channel_count = inspection.canonical_summary_channels()?.len() as u32;
    let actual_chunk_count = inspection.chunks.len() as u32;
    let exact_message = statistics.message_count == actual_message_count;
    let exact_channel = statistics.channel_count == actual_channel_count;
    let exact_chunk = statistics.chunk_count == actual_chunk_count;
    let matches = match fixture.statistics_fixture {
        StatisticsFixture::Exact => exact_message && exact_channel && exact_chunk,
        StatisticsFixture::WrongMessageCount => {
            statistics.message_count == actual_message_count.wrapping_add(1)
                && exact_channel
                && exact_chunk
        }
        StatisticsFixture::WrongChannelCount => {
            exact_message
                && statistics.channel_count == actual_channel_count.wrapping_add(1)
                && exact_chunk
        }
        StatisticsFixture::WrongChunkCount => {
            exact_message
                && exact_channel
                && statistics.chunk_count == actual_chunk_count.wrapping_add(1)
        }
    };
    if !matches {
        return Err(FixtureBuildError(
            "serialized Statistics do not match the exact requested count control".to_owned(),
        ));
    }
    Ok(())
}

fn chunk_record_stream_is_complete(chunk: &ReferenceChunk) -> bool {
    chunk.uncompressed_data.is_some() && chunk.inner_records.iter().all(|record| !record.truncated)
}

fn validate_definitions(
    fixture: &AdversarialMcapFixture,
    inspection: &ReferenceInspection,
) -> Result<(), FixtureBuildError> {
    let summary_schemas = inspection.canonical_summary_schemas();
    let summary_channels = inspection.canonical_summary_channels();
    let summary_schema_conflict = summary_schemas.is_err();
    let summary_channel_conflict = summary_channels.is_err();
    let summary_schemas = summary_schemas.unwrap_or_default();
    let summary_channels = summary_channels.unwrap_or_default();
    let chunk_schema_conflict = inspection
        .chunks
        .iter()
        .flat_map(|chunk| &chunk.schemas)
        .any(|schema| {
            summary_schemas
                .get(&schema.id)
                .is_some_and(|summary| summary != schema)
        });
    let chunk_channel_conflict = inspection
        .chunks
        .iter()
        .flat_map(|chunk| &chunk.channels)
        .any(|channel| {
            summary_channels
                .get(&channel.id)
                .is_some_and(|summary| summary != channel)
        });
    let conflicting_schema = summary_schema_conflict || chunk_schema_conflict;
    let conflicting_channel = summary_channel_conflict || chunk_channel_conflict;
    if fixture
        .mcap_format_violations
        .contains(&McapFormatViolation::ConflictingSchema)
        != conflicting_schema
    {
        return Err(FixtureBuildError(
            "reference Schema conflict differs from expectation".to_owned(),
        ));
    }
    if fixture
        .mcap_format_violations
        .contains(&McapFormatViolation::ConflictingChannel)
        != conflicting_channel
    {
        return Err(FixtureBuildError(format!(
            "reference Channel conflict differs from expectation: summary_conflict={summary_channel_conflict}, chunk_conflict={chunk_channel_conflict}, chunk_channels={:?}",
            inspection
                .chunks
                .iter()
                .map(|chunk| &chunk.channels)
                .collect::<Vec<_>>()
        )));
    }

    let missing_schema = summary_channels
        .values()
        .any(|channel| channel.schema_id != 0 && !summary_schemas.contains_key(&channel.schema_id));
    if fixture
        .mcap_format_violations
        .contains(&McapFormatViolation::MissingReferencedSchema)
        != missing_schema
    {
        return Err(FixtureBuildError(
            "reference missing-Schema evidence differs from expectation".to_owned(),
        ));
    }
    let unknown_referenced_channel = inspection
        .chunks
        .iter()
        .flat_map(|chunk| &chunk.messages)
        .chain(&inspection.top_level_messages)
        .any(|message| !summary_channels.contains_key(&message.channel_id));
    if fixture
        .mcap_format_violations
        .contains(&McapFormatViolation::UnknownReferencedChannel)
        != unknown_referenced_channel
    {
        return Err(FixtureBuildError(
            "reference unknown-Channel evidence differs from expectation".to_owned(),
        ));
    }
    validate_exact_definition_control(fixture, inspection, &summary_schemas, &summary_channels)
}

fn validate_exact_definition_control(
    fixture: &AdversarialMcapFixture,
    inspection: &ReferenceInspection,
    summary_schemas: &BTreeMap<u16, ReferenceSchema>,
    summary_channels: &BTreeMap<u16, ReferenceChannel>,
) -> Result<(), FixtureBuildError> {
    let complete_chunks: Vec<_> = inspection
        .chunks
        .iter()
        .filter(|chunk| chunk_record_stream_is_complete(chunk))
        .collect();
    if complete_chunks.is_empty()
        && (fixture
            .mcap_format_violations
            .contains(&McapFormatViolation::PrematureEof)
            || fixture.compression == CompressionFixture::TruncatedInput)
    {
        return Ok(());
    }
    if complete_chunks.is_empty() {
        return Err(FixtureBuildError(
            "definition control has no complete Chunk evidence".to_owned(),
        ));
    }
    let selected_channels: BTreeSet<_> = fixture
        .partition
        .selected_group_members
        .iter()
        .flatten()
        .copied()
        .collect();

    for chunk in complete_chunks {
        let known_schemas_exact_once = summary_schemas.values().all(|summary| {
            chunk
                .schemas
                .iter()
                .filter(|schema| *schema == summary)
                .count()
                == 1
        });
        let known_channels_exact_once = summary_channels.values().all(|summary| {
            chunk
                .channels
                .iter()
                .filter(|channel| *channel == summary)
                .count()
                == 1
        });
        let exact = match fixture.definition_fixture {
            DefinitionFixture::Matching => {
                known_schemas_exact_once
                    && known_channels_exact_once
                    && chunk.schemas.len() == summary_schemas.len()
                    && chunk.channels.len() == summary_channels.len()
                    && summary_channels.values().all(|channel| {
                        channel.schema_id == 0 || summary_schemas.contains_key(&channel.schema_id)
                    })
            }
            DefinitionFixture::ExactDuplicate => {
                chunk.schemas.len() == summary_schemas.len() * 2
                    && chunk.channels.len() == summary_channels.len() * 2
                    && summary_schemas.values().all(|summary| {
                        chunk
                            .schemas
                            .iter()
                            .filter(|schema| *schema == summary)
                            .count()
                            == 2
                    })
                    && summary_channels.values().all(|summary| {
                        chunk
                            .channels
                            .iter()
                            .filter(|channel| *channel == summary)
                            .count()
                            == 2
                    })
            }
            DefinitionFixture::UnknownUnreferenced | DefinitionFixture::UnknownReferenced => {
                let unknown_schemas: Vec<_> = chunk
                    .schemas
                    .iter()
                    .filter(|schema| !summary_schemas.contains_key(&schema.id))
                    .collect();
                let unknown_channels: Vec<_> = chunk
                    .channels
                    .iter()
                    .filter(|channel| !summary_channels.contains_key(&channel.id))
                    .collect();
                let referenced = unknown_channels.iter().any(|unknown| {
                    chunk
                        .messages
                        .iter()
                        .any(|message| message.channel_id == unknown.id)
                });
                known_schemas_exact_once
                    && known_channels_exact_once
                    && unknown_schemas.len() == 1
                    && unknown_channels.len() == 1
                    && unknown_channels[0].schema_id == unknown_schemas[0].id
                    && (fixture.definition_fixture == DefinitionFixture::UnknownReferenced)
                        == referenced
            }
            DefinitionFixture::MissingReferencedSchema => {
                known_channels_exact_once
                    && chunk.channels.iter().any(|channel| {
                        channel.schema_id != 0 && !summary_schemas.contains_key(&channel.schema_id)
                    })
                    && chunk.schemas.len() == summary_schemas.len()
                    && chunk.channels.len() == summary_channels.len()
            }
            DefinitionFixture::SchemaNameConflict
            | DefinitionFixture::SchemaEncodingConflict
            | DefinitionFixture::SchemaDataConflict => {
                known_schemas_exact_once
                    && known_channels_exact_once
                    && exact_schema_conflict(
                        fixture.definition_fixture,
                        &chunk.schemas,
                        summary_schemas,
                    )
            }
            DefinitionFixture::ChannelSchemaConflict
            | DefinitionFixture::ChannelTopicConflict
            | DefinitionFixture::ChannelEncodingConflict
            | DefinitionFixture::ChannelMetadataConflict
            | DefinitionFixture::UnselectedChannelConflict
            | DefinitionFixture::ChunkTailConflict => {
                known_schemas_exact_once
                    && known_channels_exact_once
                    && exact_channel_conflict(
                        fixture.definition_fixture,
                        chunk,
                        summary_channels,
                        &selected_channels,
                    )
            }
        };
        if !exact {
            return Err(FixtureBuildError(format!(
                "serialized definitions do not match requested {:?}",
                fixture.definition_fixture
            )));
        }
    }
    Ok(())
}

fn exact_schema_conflict(
    requested: DefinitionFixture,
    chunk_schemas: &[ReferenceSchema],
    summary_schemas: &BTreeMap<u16, ReferenceSchema>,
) -> bool {
    let conflicts: Vec<_> = chunk_schemas
        .iter()
        .filter_map(|schema| {
            let summary = summary_schemas.get(&schema.id)?;
            (schema != summary).then_some((schema, summary))
        })
        .collect();
    if conflicts.len() != 1 || chunk_schemas.len() != summary_schemas.len() + 1 {
        return false;
    }
    let (actual, summary) = conflicts[0];
    let name = actual.name != summary.name;
    let encoding = actual.encoding != summary.encoding;
    let data = actual.data != summary.data;
    match requested {
        DefinitionFixture::SchemaNameConflict => name && !encoding && !data,
        DefinitionFixture::SchemaEncodingConflict => !name && encoding && !data,
        DefinitionFixture::SchemaDataConflict => !name && !encoding && data,
        _ => false,
    }
}

fn exact_channel_conflict(
    requested: DefinitionFixture,
    chunk: &ReferenceChunk,
    summary_channels: &BTreeMap<u16, ReferenceChannel>,
    selected_channels: &BTreeSet<u16>,
) -> bool {
    let conflicts: Vec<_> = chunk
        .channels
        .iter()
        .filter_map(|channel| {
            let summary = summary_channels.get(&channel.id)?;
            (channel != summary).then_some((channel, summary))
        })
        .collect();
    if conflicts.len() != 1 || chunk.channels.len() != summary_channels.len() + 1 {
        return false;
    }
    let (actual, summary) = conflicts[0];
    let schema = actual.schema_id != summary.schema_id;
    let topic = actual.topic != summary.topic;
    let encoding = actual.message_encoding != summary.message_encoding;
    let metadata = actual.metadata != summary.metadata;
    let exact_field = match requested {
        DefinitionFixture::ChannelSchemaConflict => schema && !topic && !encoding && !metadata,
        DefinitionFixture::ChannelTopicConflict
        | DefinitionFixture::UnselectedChannelConflict
        | DefinitionFixture::ChunkTailConflict => !schema && topic && !encoding && !metadata,
        DefinitionFixture::ChannelEncodingConflict => !schema && !topic && encoding && !metadata,
        DefinitionFixture::ChannelMetadataConflict => !schema && !topic && !encoding && metadata,
        _ => false,
    };
    if !exact_field {
        return false;
    }
    let conflict_is_last_record = chunk
        .inner_records
        .last()
        .is_some_and(|record| record.opcode == op::CHANNEL && record.start == actual.record_start);
    match requested {
        DefinitionFixture::UnselectedChannelConflict => !selected_channels.contains(&actual.id),
        DefinitionFixture::ChunkTailConflict => {
            conflict_is_last_record
                && chunk
                    .messages
                    .iter()
                    .any(|message| message.channel_id == actual.id)
        }
        DefinitionFixture::ChannelTopicConflict => !conflict_is_last_record,
        _ => true,
    }
}

fn validate_nested(
    fixture: &AdversarialMcapFixture,
    inspection: &ReferenceInspection,
) -> Result<(), FixtureBuildError> {
    let Some(expected) = fixture.nested_collection else {
        return Ok(());
    };
    let location = fixture
        .nested_collection_location
        .unwrap_or_else(|| NestedCollectionLocation::default_for(expected.target));
    let section = match location {
        NestedCollectionLocation::Summary => ReferenceSection::Summary,
        NestedCollectionLocation::Chunk { chunk_index } => ReferenceSection::Chunk { chunk_index },
        NestedCollectionLocation::MessageIndexRegion { chunk_index } => {
            ReferenceSection::MessageIndex { chunk_index }
        }
    };
    let collection = inspection
        .nested_collections
        .iter()
        .rev()
        .find(|collection| collection.target == expected.target && collection.section == section)
        .ok_or_else(|| FixtureBuildError("reference nested collection is absent".to_owned()))?;
    let matches = match expected.shape {
        NestedCollectionShape::MaximumDensity => {
            collection.entry_count == expected.limits.max_entries
                && collection.declared_encoded_bytes as usize == collection.available_encoded_bytes
                && collection.element_width_valid
        }
        NestedCollectionShape::TooManyEntries => {
            collection.entry_count > expected.limits.max_entries
        }
        NestedCollectionShape::OversizedString => {
            collection.max_string_bytes > expected.limits.max_string_bytes
        }
        NestedCollectionShape::TooManyRetainedBytes => {
            collection.retained_bytes > expected.limits.max_retained_bytes
        }
        NestedCollectionShape::WrongByteLength => {
            collection.declared_encoded_bytes as usize != collection.available_encoded_bytes
                || !collection.element_width_valid
        }
        NestedCollectionShape::PrematureEof => {
            collection.available_encoded_bytes < collection.declared_encoded_bytes as usize
        }
    };
    if !matches {
        return Err(FixtureBuildError(format!(
            "reference nested {:?} evidence does not satisfy {:?}",
            expected.target, expected.shape
        )));
    }
    Ok(())
}

fn validate_compression(
    fixture: &AdversarialMcapFixture,
    inspection: &ReferenceInspection,
) -> Result<(), FixtureBuildError> {
    for chunk in &inspection.chunks {
        let actual_size = chunk
            .uncompressed_data
            .as_ref()
            .map(|bytes| bytes.len() as u64);
        let matches = match fixture.compression {
            CompressionFixture::None => chunk.compression.is_empty(),
            CompressionFixture::Zstd
            | CompressionFixture::OutputFullBeforeCodecEof
            | CompressionFixture::ZeroProgress => {
                chunk.compression == "zstd" && actual_size == Some(chunk.declared_uncompressed_size)
            }
            CompressionFixture::Lz4 => {
                chunk.compression == "lz4" && actual_size == Some(chunk.declared_uncompressed_size)
            }
            CompressionFixture::DeclaredOutputTooSmall => {
                actual_size.is_some_and(|actual| chunk.declared_uncompressed_size < actual)
            }
            CompressionFixture::DeclaredOutputTooLarge => actual_size.is_some_and(|actual| {
                chunk.declared_uncompressed_size > actual
                    && chunk.declared_uncompressed_size != u64::MAX
            }),
            CompressionFixture::ConcatenatedFrame => {
                chunk
                    .first_zstd_frame_len
                    .is_some_and(|first| first < chunk.compressed_data.len())
                    && actual_size == Some(chunk.declared_uncompressed_size)
                    && (chunk.declared_uncompressed_crc == 0
                        || chunk.actual_uncompressed_crc == Some(chunk.declared_uncompressed_crc))
            }
            CompressionFixture::TrailingPayload => chunk
                .first_zstd_frame_len
                .is_some_and(|first| first < chunk.compressed_data.len()),
            CompressionFixture::TruncatedInput => chunk.decompression_error.is_some(),
            CompressionFixture::OversizedDeclaration => {
                chunk.declared_uncompressed_size == u64::MAX
            }
        };
        if !matches {
            return Err(FixtureBuildError(format!(
                "reference compression evidence does not satisfy {:?}",
                fixture.compression
            )));
        }
    }
    Ok(())
}

fn validate_physical_layout(
    fixture: &AdversarialMcapFixture,
    inspection: &ReferenceInspection,
) -> Result<(), FixtureBuildError> {
    let conflicting_duplicate =
        inspection
            .chunk_indexes
            .iter()
            .enumerate()
            .any(|(position, index)| {
                inspection.chunk_indexes[position + 1..]
                    .iter()
                    .any(|other| {
                        index.chunk_start_offset == other.chunk_start_offset && index != other
                    })
            });
    if fixture
        .mcap_format_violations
        .contains(&McapFormatViolation::ConflictingChunkIndex)
        != conflicting_duplicate
    {
        return Err(FixtureBuildError(
            "reference conflicting ChunkIndex evidence differs from expectation".to_owned(),
        ));
    }
    let data_start = inspection
        .records
        .iter()
        .find(|record| record.opcode == op::HEADER)
        .map(|record| record.available_end as u64)
        .unwrap_or(0);
    let data_end = inspection
        .data_end
        .map(|record| record.start as u64)
        .unwrap_or(u64::MAX);
    let mut by_chunk_start = BTreeMap::new();
    for index in &inspection.chunk_indexes {
        by_chunk_start
            .entry(index.chunk_start_offset)
            .or_insert(index);
    }
    let mut regions = Vec::new();
    let mut physical_overlap = false;
    for index in by_chunk_start.values() {
        let chunk_end = index.chunk_start_offset.saturating_add(index.chunk_length);
        let message_index_end = chunk_end.saturating_add(index.message_index_length);
        physical_overlap |= index.chunk_start_offset < data_start
            || chunk_end > data_end
            || message_index_end > data_end;
        if index.chunk_length > 0 {
            regions.push((index.chunk_start_offset, chunk_end));
        }
        if index.message_index_length > 0 {
            regions.push((chunk_end, message_index_end));
        }
    }
    regions.sort_unstable();
    physical_overlap |= regions.windows(2).any(|window| window[0].1 > window[1].0);
    if fixture
        .mcap_format_violations
        .contains(&McapFormatViolation::PhysicalRegionOverlap)
        != physical_overlap
    {
        return Err(FixtureBuildError(
            "reference physical-region overlap differs from expectation".to_owned(),
        ));
    }
    validate_exact_physical_control(fixture.physical_layout_fault, inspection)
}

fn validate_exact_physical_control(
    requested: super::PhysicalLayoutFault,
    inspection: &ReferenceInspection,
) -> Result<(), FixtureBuildError> {
    let first_index = inspection.chunk_indexes.first();
    let first_chunk = inspection.chunks.first();
    let data_end = inspection.data_end;
    let summary_start = inspection.footer.map(|footer| footer.summary_start);
    let exact =
        match requested {
            super::PhysicalLayoutFault::None => {
                inspection.chunk_indexes.len() == inspection.chunks.len()
                    && inspection.chunk_indexes.iter().zip(&inspection.chunks).all(
                        |(index, chunk)| {
                            index.chunk_start_offset == chunk.span.start as u64
                                && index.chunk_length
                                    == (chunk.span.available_end - chunk.span.start) as u64
                        },
                    )
                    && data_end.is_some_and(|data_end| {
                        inspection.chunk_indexes.iter().all(|index| {
                            index
                                .chunk_start_offset
                                .saturating_add(index.chunk_length)
                                .saturating_add(index.message_index_length)
                                <= data_end.start as u64
                        })
                    })
            }
            super::PhysicalLayoutFault::DuplicateChunkIndex => {
                inspection.chunk_indexes.len() == inspection.chunks.len() + 1
                    && inspection
                        .chunk_indexes
                        .iter()
                        .enumerate()
                        .any(|(position, index)| {
                            inspection.chunk_indexes[position + 1..]
                                .iter()
                                .any(|other| index == other)
                        })
            }
            super::PhysicalLayoutFault::ConflictingDuplicateChunkIndex => {
                inspection.chunk_indexes.len() == inspection.chunks.len() + 1
                    && inspection
                        .chunk_indexes
                        .iter()
                        .enumerate()
                        .any(|(position, index)| {
                            inspection.chunk_indexes[position + 1..]
                                .iter()
                                .any(|other| {
                                    conflicting_duplicate_is_compressed_size_only(index, other)
                                })
                        })
            }
            super::PhysicalLayoutFault::OverlappingChunkRanges => {
                let Some(first) = first_index else {
                    return Err(FixtureBuildError("missing first ChunkIndex".to_owned()));
                };
                let first_end = first.chunk_start_offset.saturating_add(first.chunk_length);
                inspection.chunk_indexes.iter().skip(1).any(|other| {
                    other.chunk_start_offset > first.chunk_start_offset
                        && other.chunk_start_offset < first_end
                        && !inspection
                            .chunks
                            .iter()
                            .any(|chunk| chunk.span.start as u64 == other.chunk_start_offset)
                })
            }
            super::PhysicalLayoutFault::ChunkOverlapsMessageIndex => {
                first_index.zip(first_chunk).is_some_and(|(index, chunk)| {
                    index.chunk_start_offset == chunk.span.start as u64
                        && index.chunk_length
                            == (chunk.span.available_end - chunk.span.start) as u64 + 1
                        && inspection.message_indexes.iter().any(|message_index| {
                            message_index.span.start == chunk.span.available_end
                        })
                })
            }
            super::PhysicalLayoutFault::ChunkOverlapsDataEnd => {
                first_index.zip(data_end).is_some_and(|(index, data_end)| {
                    index.chunk_start_offset == data_end.start as u64
                        && index.chunk_length == (data_end.available_end - data_end.start) as u64
                })
            }
            super::PhysicalLayoutFault::ChunkOverlapsSummary => first_index
                .zip(summary_start)
                .is_some_and(|(index, summary_start)| {
                    index.chunk_start_offset == summary_start && index.chunk_length == 1
                }),
            super::PhysicalLayoutFault::MessageIndexOverlapsDataEnd => {
                first_index.zip(data_end).is_some_and(|(index, data_end)| {
                    let region_start = index.chunk_start_offset.saturating_add(index.chunk_length);
                    region_start < data_end.available_end as u64
                        && region_start.saturating_add(index.message_index_length)
                            == data_end.available_end as u64
                })
            }
            super::PhysicalLayoutFault::MessageIndexOverlapsSummary => first_index
                .zip(summary_start)
                .is_some_and(|(index, summary_start)| {
                    let region_start = index.chunk_start_offset.saturating_add(index.chunk_length);
                    region_start <= summary_start
                        && region_start.saturating_add(index.message_index_length)
                            == summary_start.saturating_add(1)
                }),
        };
    if !exact {
        return Err(FixtureBuildError(format!(
            "serialized physical layout does not match requested {requested:?}"
        )));
    }
    Ok(())
}

fn conflicting_duplicate_is_compressed_size_only(
    first: &ReferenceChunkIndex,
    second: &ReferenceChunkIndex,
) -> bool {
    if first.chunk_start_offset != second.chunk_start_offset
        || second.compressed_size != first.compressed_size.wrapping_add(1)
    {
        return false;
    }
    let mut normalized = second.clone();
    normalized.compressed_size = first.compressed_size;
    normalized == *first
}

fn validate_cardinality(
    expected: FixtureCardinality,
    inspection: &ReferenceInspection,
) -> Result<(), FixtureBuildError> {
    let Some(footer) = inspection.footer else {
        return Ok(());
    };
    let summary_records = inspection
        .records
        .iter()
        .filter(|record| {
            record.start as u64 >= footer.summary_start
                && !matches!(record.opcode, op::SUMMARY_OFFSET | op::FOOTER)
        })
        .count();
    let all_chunk_streams_complete = inspection
        .chunks
        .iter()
        .all(chunk_record_stream_is_complete);
    let max_records_per_chunk = inspection
        .chunks
        .iter()
        .map(|chunk| chunk.inner_records.len())
        .max()
        .unwrap_or(0);
    let max_messages_per_chunk = inspection
        .chunks
        .iter()
        .map(|chunk| {
            if chunk_record_stream_is_complete(chunk) {
                return chunk.messages.len();
            }
            let Some(index) = inspection
                .chunk_indexes
                .iter()
                .find(|index| index.chunk_start_offset == chunk.span.start as u64)
            else {
                return 0;
            };
            let region_start = index.chunk_start_offset.saturating_add(index.chunk_length);
            let region_end = region_start.saturating_add(index.message_index_length);
            inspection
                .message_indexes
                .iter()
                .filter(|message_index| {
                    let start = message_index.span.start as u64;
                    start >= region_start && start < region_end
                })
                .map(|message_index| message_index.entries.len())
                .sum()
        })
        .max()
        .unwrap_or(0);
    if summary_records != expected.summary_records
        || inspection.canonical_summary_schemas()?.len() != expected.schemas
        || inspection.canonical_summary_channels()?.len() != expected.channels
        || inspection.chunk_indexes.len() != expected.chunk_indexes
        // An intentionally truncated frame makes its inner record count unobservable. Its
        // MessageIndex still independently freezes Message cardinality; all decodable fixtures
        // continue to prove complete inner-record cardinality directly from bytes.
        || (all_chunk_streams_complete && max_records_per_chunk != expected.records_per_chunk)
        || max_messages_per_chunk != expected.messages_per_chunk
    {
        return Err(FixtureBuildError(format!(
            "reference byte cardinality differs from frozen fixture values for compression {:?}: expected={expected:?}, actual_summary_records={summary_records}, actual_schemas={}, actual_channels={}, actual_chunk_indexes={}, actual_max_records_per_chunk={max_records_per_chunk}, actual_max_messages_per_chunk={max_messages_per_chunk}, decompression_errors={:?}",
            inspection
                .chunks
                .first()
                .map(|chunk| chunk.compression.as_str()),
            inspection.canonical_summary_schemas()?.len(),
            inspection.canonical_summary_channels()?.len(),
            inspection.chunk_indexes.len(),
            inspection
                .chunks
                .iter()
                .map(|chunk| chunk.decompression_error.as_deref())
                .collect::<Vec<_>>(),
        )));
    }
    Ok(())
}

fn validate_attached_outcomes(
    fixture: &AdversarialMcapFixture,
    _inspection: &ReferenceInspection,
) -> Result<(), FixtureBuildError> {
    let expected_missing = fixture.message_index_fault == MessageIndexFault::Missing;
    if fixture
        .remote_expected_outcomes
        .contains(&RemotePolicyOutcome::MessageIndexAbsent)
        != expected_missing
    {
        return Err(FixtureBuildError(
            "MessageIndexAbsent outcome is not tied to its serialized control".to_owned(),
        ));
    }
    if fixture.compression == CompressionFixture::OutputFullBeforeCodecEof
        && !fixture
            .fake_component_outcomes
            .contains(&FakeComponentOutcome::CodecDidNotReachEof)
    {
        return Err(FixtureBuildError(
            "fake codec EOF outcome is absent".to_owned(),
        ));
    }
    if fixture.compression == CompressionFixture::ZeroProgress
        && !fixture
            .fake_component_outcomes
            .contains(&FakeComponentOutcome::CodecZeroProgress)
    {
        return Err(FixtureBuildError(
            "fake codec zero-progress outcome is absent".to_owned(),
        ));
    }
    if fixture.compression == CompressionFixture::ConcatenatedFrame
        && !fixture
            .remote_expected_outcomes
            .contains(&RemotePolicyOutcome::MultipleCompressionFrames)
    {
        return Err(FixtureBuildError(
            "multiple-frame remote-policy outcome is absent".to_owned(),
        ));
    }
    Ok(())
}

fn validate_decoder_and_partition_bytes(
    fixture: &AdversarialMcapFixture,
    inspection: &ReferenceInspection,
) -> Result<(), FixtureBuildError> {
    let channels = inspection.canonical_summary_channels()?;
    let schemas = inspection.canonical_summary_schemas()?;
    let input = &fixture.decoder.recognition_input;
    let channel = channels.get(&input.channel_id).ok_or_else(|| {
        FixtureBuildError(format!(
            "reference decoder input Channel {} is absent",
            input.channel_id
        ))
    })?;
    let schema = schemas.get(&channel.schema_id);
    if channel.schema_id != input.schema_id
        || channel.message_encoding != input.message_encoding
        || schema.map(|schema| schema.name.as_str()) != input.schema_name.as_deref()
        || schema.map(|schema| schema.encoding.as_str()) != input.schema_encoding.as_deref()
    {
        return Err(FixtureBuildError(
            "reference decoder input differs from serialized Channel/Schema".to_owned(),
        ));
    }

    let groups = &fixture.partition.selected_group_members;
    if groups.len() != fixture.partition.selected_channel_groups
        || groups.iter().map(Vec::len).max().unwrap_or(0)
            != fixture.partition.channels_per_selected_group
    {
        return Err(FixtureBuildError(
            "reference selected-group shape differs from frozen counts".to_owned(),
        ));
    }
    let selected_channels: BTreeSet<_> = groups.iter().flatten().copied().collect();
    if let Some(unknown) = selected_channels
        .iter()
        .find(|channel_id| !channels.contains_key(channel_id))
    {
        return Err(FixtureBuildError(format!(
            "reference selected group contains absent Channel {unknown}"
        )));
    }
    let covering_chunks = inspection
        .chunk_indexes
        .iter()
        .filter(|index| {
            index.message_range.start <= fixture.partition.cursor_time
                && fixture.partition.cursor_time <= index.message_range.end
        })
        .count();
    if covering_chunks != fixture.partition.cursor_covering_chunks {
        return Err(FixtureBuildError(format!(
            "reference cursor coverage differs: {covering_chunks} != {}",
            fixture.partition.cursor_covering_chunks
        )));
    }
    let selected_dispatches: usize = inspection
        .chunks
        .iter()
        .map(|chunk| {
            if chunk_record_stream_is_complete(chunk) {
                return chunk
                    .messages
                    .iter()
                    .filter(|message| selected_channels.contains(&message.channel_id))
                    .count();
            }
            let Some(index) = inspection
                .chunk_indexes
                .iter()
                .find(|index| index.chunk_start_offset == chunk.span.start as u64)
            else {
                return 0;
            };
            let region_start = index.chunk_start_offset.saturating_add(index.chunk_length);
            let region_end = region_start.saturating_add(index.message_index_length);
            inspection
                .message_indexes
                .iter()
                .filter(|message_index| {
                    let start = message_index.span.start as u64;
                    start >= region_start
                        && start < region_end
                        && selected_channels.contains(&message_index.channel_id)
                })
                .map(|message_index| message_index.entries.len())
                .sum()
        })
        .sum();
    if selected_dispatches != fixture.cardinality.selected_dispatches_per_scan {
        return Err(FixtureBuildError(format!(
            "reference selected dispatches differ: {selected_dispatches} != {}",
            fixture.cardinality.selected_dispatches_per_scan
        )));
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct RawRecord {
    span: ReferenceRecordSpan,
    body: Vec<u8>,
}

struct WalkedRecords {
    records: Vec<RawRecord>,
    truncated: Option<ReferenceRecordSpan>,
}

fn walk_records(bytes: &[u8], base: usize) -> Result<WalkedRecords, FixtureBuildError> {
    let mut records = Vec::new();
    let mut position = 0usize;
    let mut truncated = None;
    while position < bytes.len() {
        if bytes.len() - position < RECORD_HEADER_LEN {
            truncated = Some(ReferenceRecordSpan {
                opcode: bytes[position],
                start: base + position,
                body_start: base + position + RECORD_HEADER_LEN,
                declared_body_len: 0,
                available_end: base + bytes.len(),
                truncated: true,
            });
            break;
        }
        let opcode = bytes[position];
        let declared_body_len = usize::try_from(u64::from_le_bytes(
            bytes[position + 1..position + RECORD_HEADER_LEN]
                .try_into()
                .expect("record length has fixed width"),
        ))
        .map_err(|_error| FixtureBuildError("record length does not fit usize".to_owned()))?;
        let body_start = position + RECORD_HEADER_LEN;
        let declared_end = body_start.checked_add(declared_body_len).ok_or_else(|| {
            FixtureBuildError("record end overflow in reference inspector".to_owned())
        })?;
        let available_end = declared_end.min(bytes.len());
        let span = ReferenceRecordSpan {
            opcode,
            start: base + position,
            body_start: base + body_start,
            declared_body_len,
            available_end: base + available_end,
            truncated: declared_end > bytes.len(),
        };
        let raw = RawRecord {
            span,
            body: bytes[body_start..available_end].to_vec(),
        };
        records.push(raw);
        if span.truncated {
            truncated = Some(span);
            break;
        }
        position = declared_end;
    }
    Ok(WalkedRecords { records, truncated })
}

fn parse_footer(body: &[u8]) -> Result<(u64, u64, u32), FixtureBuildError> {
    let mut reader = ReferenceCursor::new(body);
    Ok((reader.u64()?, reader.u64()?, reader.u32()?))
}

fn parse_schema(body: &[u8]) -> Result<ReferenceSchema, FixtureBuildError> {
    let mut reader = ReferenceCursor::new(body);
    Ok(ReferenceSchema {
        id: reader.u16()?,
        name: reader.string()?,
        encoding: reader.string()?,
        data: reader.length_prefixed_bytes()?.to_vec(),
    })
}

fn parse_channel(
    body: &[u8],
    record_start: usize,
    section: ReferenceSection,
) -> Result<(ReferenceChannel, ReferenceNestedCollection), FixtureBuildError> {
    let mut reader = ReferenceCursor::new(body);
    let id = reader.u16()?;
    let schema_id = reader.u16()?;
    let topic = reader.string()?;
    let message_encoding = reader.string()?;
    let collection = reader.collection()?;
    let mut metadata = BTreeMap::new();
    let mut entries = ReferenceCursor::new(collection.available);
    let mut max_string_bytes = 0;
    let mut retained_bytes = 0;
    while !entries.is_empty() {
        let key = entries.string()?;
        let value = entries.string()?;
        max_string_bytes = max_string_bytes.max(key.len()).max(value.len());
        retained_bytes += key.len() + value.len();
        metadata.insert(key, value);
    }
    let nested = ReferenceNestedCollection {
        target: NestedCollectionTarget::ChannelMetadata,
        section,
        record_start,
        declared_encoded_bytes: collection.declared,
        available_encoded_bytes: collection.available.len(),
        entry_count: metadata.len(),
        max_string_bytes,
        retained_bytes,
        element_width_valid: true,
    };
    Ok((
        ReferenceChannel {
            id,
            schema_id,
            topic,
            message_encoding,
            metadata,
            record_start,
        },
        nested,
    ))
}

fn parse_message(body: &[u8], record_offset: u64) -> Result<ReferenceMessage, FixtureBuildError> {
    let mut reader = ReferenceCursor::new(body);
    Ok(ReferenceMessage {
        channel_id: reader.u16()?,
        sequence: reader.u32()?,
        log_time: reader.u64()?,
        publish_time: reader.u64()?,
        data: reader.rest().to_vec(),
        record_offset,
    })
}

fn parse_chunk(
    raw: &RawRecord,
    chunk_index: usize,
    nested_collections: &mut Vec<ReferenceNestedCollection>,
) -> Result<ReferenceChunk, FixtureBuildError> {
    let mut reader = ReferenceCursor::new(&raw.body);
    let message_range = RawTimeRange::new(reader.u64()?, reader.u64()?);
    let declared_uncompressed_size = reader.u64()?;
    let declared_uncompressed_crc = reader.u32()?;
    let compression = reader.string()?;
    let compressed_size = usize::try_from(reader.u64()?)
        .map_err(|_error| FixtureBuildError("compressed size does not fit usize".to_owned()))?;
    let compressed_data_start = raw.span.body_start + reader.position();
    let compressed_data = reader.take_at_most(compressed_size).to_vec();
    let first_zstd_frame_len = if compression == "zstd" {
        zstd::zstd_safe::find_frame_compressed_size(&compressed_data).ok()
    } else {
        None
    };
    let (partial_uncompressed_data, decompression_error) =
        decompress_with_partial(&compression, &compressed_data);
    let uncompressed_data = (!partial_uncompressed_data.is_empty() || compressed_data.is_empty())
        .then_some(partial_uncompressed_data);
    let actual_uncompressed_crc = uncompressed_data.as_deref().map(crc32fast::hash);
    let mut inner_records = Vec::new();
    let mut schemas = Vec::new();
    let mut channels = Vec::new();
    let mut messages = Vec::new();
    if let Some(uncompressed) = &uncompressed_data {
        let walked = walk_records(uncompressed, 0)?;
        inner_records = walked.records.iter().map(|record| record.span).collect();
        if let Some(truncated) = walked.truncated {
            inner_records.push(truncated);
        }
        for inner in walked.records {
            if inner.span.truncated {
                if let Some(nested) = parse_truncated_nested_collection(
                    &inner,
                    ReferenceSection::Chunk { chunk_index },
                )? {
                    nested_collections.push(nested);
                }
                continue;
            }
            let section = ReferenceSection::Chunk { chunk_index };
            match inner.span.opcode {
                op::SCHEMA => schemas.push(parse_schema(&inner.body)?),
                op::CHANNEL => {
                    let (channel, nested) = parse_channel(&inner.body, inner.span.start, section)?;
                    channels.push(channel);
                    nested_collections.push(nested);
                }
                op::MESSAGE => {
                    messages.push(parse_message(&inner.body, inner.span.start as u64)?);
                }
                _ => {}
            }
        }
    }
    Ok(ReferenceChunk {
        span: raw.span,
        message_range,
        declared_uncompressed_size,
        declared_uncompressed_crc,
        compression,
        compressed_data_start,
        compressed_data,
        first_zstd_frame_len,
        uncompressed_data,
        decompression_error,
        actual_uncompressed_crc,
        inner_records,
        schemas,
        channels,
        messages,
    })
}

fn parse_message_index(
    body: &[u8],
    span: ReferenceRecordSpan,
    section: ReferenceSection,
) -> Result<(ReferenceMessageIndex, ReferenceNestedCollection), FixtureBuildError> {
    let mut reader = ReferenceCursor::new(body);
    let channel_id = reader.u16()?;
    let collection = reader.fixed_collection(16)?;
    let element_width_valid = collection.declared % 16 == 0;
    let mut entries = ReferenceCursor::new(collection.available);
    let mut records = Vec::new();
    while entries.remaining() >= 16 {
        records.push(ReferenceMessageIndexEntry {
            log_time: entries.u64()?,
            offset: entries.u64()?,
        });
    }
    let nested = ReferenceNestedCollection {
        target: NestedCollectionTarget::MessageIndexRecords,
        section,
        record_start: span.start,
        declared_encoded_bytes: collection.declared,
        available_encoded_bytes: collection.available.len(),
        entry_count: records.len(),
        max_string_bytes: 0,
        retained_bytes: records.len() * 16,
        element_width_valid,
    };
    Ok((
        ReferenceMessageIndex {
            span,
            chunk_index: match section {
                ReferenceSection::MessageIndex { chunk_index } => chunk_index,
                _ => {
                    return Err(FixtureBuildError(
                        "MessageIndex parsed outside an owning region".to_owned(),
                    ));
                }
            },
            channel_id,
            entries: records,
        },
        nested,
    ))
}

fn parse_truncated_nested_collection(
    raw: &RawRecord,
    section: ReferenceSection,
) -> Result<Option<ReferenceNestedCollection>, FixtureBuildError> {
    let mut reader = ReferenceCursor::new(&raw.body);
    let (target, collection, element_width_valid) = match raw.span.opcode {
        op::CHANNEL => {
            reader.u16()?;
            reader.u16()?;
            reader.string()?;
            reader.string()?;
            (
                NestedCollectionTarget::ChannelMetadata,
                reader.collection()?,
                true,
            )
        }
        op::MESSAGE_INDEX => {
            reader.u16()?;
            let collection = reader.fixed_collection(16)?;
            (
                NestedCollectionTarget::MessageIndexRecords,
                collection,
                collection.declared % 16 == 0,
            )
        }
        op::CHUNK_INDEX => {
            reader.u64()?;
            reader.u64()?;
            reader.u64()?;
            reader.u64()?;
            let collection = reader.fixed_collection(10)?;
            (
                NestedCollectionTarget::ChunkIndexMessageIndexOffsets,
                collection,
                collection.declared % 10 == 0,
            )
        }
        op::STATISTICS => {
            reader.u64()?;
            reader.u16()?;
            reader.u32()?;
            reader.u32()?;
            reader.u32()?;
            reader.u32()?;
            reader.u64()?;
            reader.u64()?;
            let collection = reader.fixed_collection(10)?;
            (
                NestedCollectionTarget::StatisticsChannelMessageCounts,
                collection,
                collection.declared % 10 == 0,
            )
        }
        _ => return Ok(None),
    };
    if collection.available.len() >= collection.declared as usize {
        return Err(FixtureBuildError(
            "truncated nested record still contains its full declared collection".to_owned(),
        ));
    }
    Ok(Some(ReferenceNestedCollection {
        target,
        section,
        record_start: raw.span.start,
        declared_encoded_bytes: collection.declared,
        available_encoded_bytes: collection.available.len(),
        entry_count: 0,
        max_string_bytes: 0,
        retained_bytes: collection.available.len(),
        element_width_valid,
    }))
}

fn parse_chunk_index(
    body: &[u8],
    record_start: usize,
    section: ReferenceSection,
) -> Result<(ReferenceChunkIndex, ReferenceNestedCollection), FixtureBuildError> {
    let mut reader = ReferenceCursor::new(body);
    let message_range = RawTimeRange::new(reader.u64()?, reader.u64()?);
    let chunk_start_offset = reader.u64()?;
    let chunk_length = reader.u64()?;
    let collection = reader.fixed_collection(10)?;
    let element_width_valid = collection.declared % 10 == 0;
    let mut entries = ReferenceCursor::new(collection.available);
    let mut message_index_offsets = BTreeMap::new();
    while entries.remaining() >= 10 {
        message_index_offsets.insert(entries.u16()?, entries.u64()?);
    }
    let nested = ReferenceNestedCollection {
        target: NestedCollectionTarget::ChunkIndexMessageIndexOffsets,
        section,
        record_start,
        declared_encoded_bytes: collection.declared,
        available_encoded_bytes: collection.available.len(),
        entry_count: message_index_offsets.len(),
        max_string_bytes: 0,
        retained_bytes: message_index_offsets.len() * 10,
        element_width_valid,
    };
    let message_index_length = reader.u64()?;
    let compression = reader.string()?;
    let compressed_size = reader.u64()?;
    let uncompressed_size = reader.u64()?;
    Ok((
        ReferenceChunkIndex {
            message_range,
            chunk_start_offset,
            chunk_length,
            message_index_offsets,
            message_index_length,
            compression,
            compressed_size,
            uncompressed_size,
        },
        nested,
    ))
}

fn parse_statistics(
    body: &[u8],
    record_start: usize,
    section: ReferenceSection,
) -> Result<(ReferenceStatistics, ReferenceNestedCollection), FixtureBuildError> {
    let mut reader = ReferenceCursor::new(body);
    let message_count = reader.u64()?;
    let schema_count = reader.u16()?;
    let channel_count = reader.u32()?;
    let attachment_count = reader.u32()?;
    let metadata_count = reader.u32()?;
    let chunk_count = reader.u32()?;
    let message_start_time = reader.u64()?;
    let message_end_time = reader.u64()?;
    let collection = reader.fixed_collection(10)?;
    let element_width_valid = collection.declared % 10 == 0;
    let mut entries = ReferenceCursor::new(collection.available);
    let mut channel_message_counts = BTreeMap::new();
    while entries.remaining() >= 10 {
        channel_message_counts.insert(entries.u16()?, entries.u64()?);
    }
    let nested = ReferenceNestedCollection {
        target: NestedCollectionTarget::StatisticsChannelMessageCounts,
        section,
        record_start,
        declared_encoded_bytes: collection.declared,
        available_encoded_bytes: collection.available.len(),
        entry_count: channel_message_counts.len(),
        max_string_bytes: 0,
        retained_bytes: channel_message_counts.len() * 10,
        element_width_valid,
    };
    Ok((
        ReferenceStatistics {
            message_count,
            schema_count,
            channel_count,
            attachment_count,
            metadata_count,
            chunk_count,
            message_start_time,
            message_end_time,
            channel_message_counts,
        },
        nested,
    ))
}

fn decompress_with_partial(compression: &str, bytes: &[u8]) -> (Vec<u8>, Option<String>) {
    match compression {
        "" => (bytes.to_vec(), None),
        "zstd" => match zstd::stream::read::Decoder::new(std::io::Cursor::new(bytes)) {
            Ok(mut decoder) => {
                let mut output = Vec::new();
                let error = decoder
                    .read_to_end(&mut output)
                    .err()
                    .map(|error| format!("zstd reference decode failed: {error}"));
                (output, error)
            }
            Err(error) => (
                Vec::new(),
                Some(format!(
                    "zstd reference decoder initialization failed: {error}"
                )),
            ),
        },
        "lz4" => {
            let mut output = Vec::new();
            let error = lz4_flex::frame::FrameDecoder::new(bytes)
                .read_to_end(&mut output)
                .err()
                .map(|error| format!("lz4 reference decode failed: {error}"));
            (output, error)
        }
        other => (
            Vec::new(),
            Some(format!("unsupported reference compression {other:?}")),
        ),
    }
}

fn owning_chunk_index(chunks: &[ReferenceChunk], record_start: usize) -> usize {
    chunks
        .iter()
        .enumerate()
        .rev()
        .find(|(_index, chunk)| chunk.span.available_end <= record_start)
        .map_or(0, |(index, _chunk)| index)
}

fn canonical_by_id<T: Clone + PartialEq>(
    records: &[T],
    id: impl Fn(&T) -> u16,
    label: &str,
) -> Result<BTreeMap<u16, T>, FixtureBuildError> {
    let mut canonical = BTreeMap::new();
    for record in records {
        match canonical.get(&id(record)) {
            Some(existing) if existing != record => {
                return Err(FixtureBuildError(format!(
                    "conflicting {label} in reference projection"
                )));
            }
            Some(_) => {}
            None => {
                canonical.insert(id(record), record.clone());
            }
        }
    }
    Ok(canonical)
}

#[derive(Clone, Copy)]
struct ReferenceCollection<'a> {
    declared: u32,
    available: &'a [u8],
}

struct ReferenceCursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> ReferenceCursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn position(&self) -> usize {
        self.position
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.position)
    }

    fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], FixtureBuildError> {
        let end = self
            .position
            .checked_add(length)
            .ok_or_else(|| FixtureBuildError("reference cursor position overflow".to_owned()))?;
        let bytes = self.bytes.get(self.position..end).ok_or_else(|| {
            FixtureBuildError("reference cursor reached premature EOF".to_owned())
        })?;
        self.position = end;
        Ok(bytes)
    }

    fn take_at_most(&mut self, length: usize) -> &'a [u8] {
        let available = length.min(self.remaining());
        let start = self.position;
        self.position += available;
        &self.bytes[start..start + available]
    }

    fn rest(&mut self) -> &'a [u8] {
        self.take_at_most(self.remaining())
    }

    fn u16(&mut self) -> Result<u16, FixtureBuildError> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().expect("u16 has fixed width"),
        ))
    }

    fn u32(&mut self) -> Result<u32, FixtureBuildError> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("u32 has fixed width"),
        ))
    }

    fn u64(&mut self) -> Result<u64, FixtureBuildError> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().expect("u64 has fixed width"),
        ))
    }

    fn string(&mut self) -> Result<String, FixtureBuildError> {
        let bytes = self.length_prefixed_bytes()?;
        String::from_utf8(bytes.to_vec())
            .map_err(|error| FixtureBuildError(format!("reference string is not UTF-8: {error}")))
    }

    fn length_prefixed_bytes(&mut self) -> Result<&'a [u8], FixtureBuildError> {
        let length = self.u32()? as usize;
        self.take(length)
    }

    fn collection(&mut self) -> Result<ReferenceCollection<'a>, FixtureBuildError> {
        let declared = self.u32()?;
        let available = self.take_at_most(declared as usize);
        Ok(ReferenceCollection {
            declared,
            available,
        })
    }

    fn fixed_collection(
        &mut self,
        element_width: usize,
    ) -> Result<ReferenceCollection<'a>, FixtureBuildError> {
        let declared = self.u32()?;
        let structurally_complete = declared as usize - declared as usize % element_width;
        let available = self.take_at_most(structurally_complete);
        Ok(ReferenceCollection {
            declared,
            available,
        })
    }
}
