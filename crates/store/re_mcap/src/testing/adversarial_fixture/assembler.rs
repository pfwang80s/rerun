use super::{
    AdversarialMcapFixture, AdversarialMcapFixtureBuilder, BTreeMap, BTreeSet, CompressionFixture,
    Cursor, DefinitionFixture, FakeComponentOutcome, FixtureChannel, FixtureChunk,
    FixtureChunkIndexLayout, FixtureChunkLayout, FixtureCrc, FixtureLayout, FixtureMessage,
    FixtureMessageIndexEntry, FixtureMessageIndexLayout, FixtureNestedCollectionLayout,
    FixtureRecordSpan, FixtureSchema, FixtureStatisticsLayout, McapFormatViolation,
    MessageIndexFault, NestedCollectionLocation, NestedCollectionShape, NestedCollectionTarget,
    PartitionGenerationFixture, PhysicalLayoutFault, RECORD_HEADER_LEN, RawTimeRange,
    RemotePolicyOutcome, StatisticsFixture, TemporalOutputFixture,
};
use mcap::records::op;
use std::io::Write as _;

#[derive(Clone, Debug)]
struct EncodedRecord {
    bytes: Vec<u8>,
    body_len: usize,
}

impl EncodedRecord {
    fn new(opcode: u8, body: Vec<u8>) -> Self {
        let body_len = body.len();
        let mut bytes = Vec::with_capacity(RECORD_HEADER_LEN + body_len);
        bytes.push(opcode);
        push_u64(&mut bytes, body_len as u64);
        bytes.extend(body);
        Self { bytes, body_len }
    }
}

fn append_record(
    output: &mut Vec<u8>,
    layout: &mut FixtureLayout,
    record: EncodedRecord,
) -> FixtureRecordSpan {
    let start = output.len();
    let body_start = start + RECORD_HEADER_LEN;
    let opcode = record.bytes[0];
    output.extend(record.bytes);
    let span = FixtureRecordSpan {
        opcode,
        start,
        body_start,
        body_len: record.body_len,
        end: output.len(),
    };
    layout.records.push(span);
    span
}

fn push_u16(output: &mut Vec<u8>, value: u16) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn push_u64(output: &mut Vec<u8>, value: u64) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn push_string(output: &mut Vec<u8>, value: &str) {
    push_u32(output, value.len() as u32);
    output.extend_from_slice(value.as_bytes());
}

fn encode_header() -> EncodedRecord {
    let mut body = Vec::new();
    push_string(&mut body, "rerun-adversarial-fixture");
    push_string(&mut body, "re_mcap::testing");
    EncodedRecord::new(op::HEADER, body)
}

fn encode_schema(schema: &FixtureSchema) -> EncodedRecord {
    let mut body = Vec::new();
    push_u16(&mut body, schema.id);
    push_string(&mut body, &schema.name);
    push_string(&mut body, &schema.encoding);
    push_u32(&mut body, schema.data.len() as u32);
    body.extend_from_slice(&schema.data);
    EncodedRecord::new(op::SCHEMA, body)
}

fn encode_channel(channel: &FixtureChannel) -> EncodedRecord {
    let mut body = Vec::new();
    push_u16(&mut body, channel.id);
    push_u16(&mut body, channel.schema_id);
    push_string(&mut body, &channel.topic);
    push_string(&mut body, &channel.message_encoding);
    push_string_map(&mut body, &channel.metadata);
    EncodedRecord::new(op::CHANNEL, body)
}

fn encode_message(message: &FixtureMessage) -> EncodedRecord {
    let mut body = Vec::new();
    push_u16(&mut body, message.channel_id);
    push_u32(&mut body, message.sequence);
    push_u64(&mut body, message.log_time);
    push_u64(&mut body, message.publish_time);
    body.extend_from_slice(&message.data);
    EncodedRecord::new(op::MESSAGE, body)
}

fn push_string_map(output: &mut Vec<u8>, map: &BTreeMap<String, String>) {
    let byte_len = map
        .iter()
        .map(|(key, value)| 8usize + key.len() + value.len())
        .sum::<usize>();
    push_u32(output, byte_len as u32);
    for (key, value) in map {
        push_string(output, key);
        push_string(output, value);
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

fn crc_field(mode: FixtureCrc, covered: &[u8]) -> u32 {
    match mode {
        FixtureCrc::Zero => 0,
        FixtureCrc::ValidNonZero => crc32(covered),
        FixtureCrc::InvalidNonZero => {
            let valid = crc32(covered);
            let invalid = valid ^ 1;
            if invalid == 0 { 1 } else { invalid }
        }
    }
}

/// A deterministic construction error caused by mutually incompatible fixture controls.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixtureBuildError(pub String);

impl std::fmt::Display for FixtureBuildError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for FixtureBuildError {}

#[derive(Clone, Debug)]
struct ChunkDescriptor {
    message_start_time: u64,
    message_end_time: u64,
    chunk_start_offset: u64,
    chunk_length: u64,
    message_index_offsets: BTreeMap<u16, u64>,
    message_index_length: u64,
    compression: String,
    compressed_size: u64,
    uncompressed_size: u64,
}

impl From<&ChunkDescriptor> for FixtureChunkIndexLayout {
    fn from(descriptor: &ChunkDescriptor) -> Self {
        Self {
            message_range: RawTimeRange::new(
                descriptor.message_start_time,
                descriptor.message_end_time,
            ),
            chunk_start_offset: descriptor.chunk_start_offset,
            chunk_length: descriptor.chunk_length,
            message_index_offsets: descriptor.message_index_offsets.clone(),
            message_index_length: descriptor.message_index_length,
            compression: descriptor.compression.clone(),
            compressed_size: descriptor.compressed_size,
            uncompressed_size: descriptor.uncompressed_size,
        }
    }
}

#[derive(Clone, Debug)]
struct BuiltChunk {
    descriptor: ChunkDescriptor,
    layout: FixtureChunkLayout,
}

#[derive(Clone, Debug)]
struct ChunkPayload {
    bytes: Vec<u8>,
    record_count: usize,
    messages: Vec<(FixtureMessage, u64)>,
}

#[derive(Clone, Copy, Debug)]
struct NestedPatch {
    target: NestedCollectionTarget,
    location: NestedCollectionLocation,
    length_offset: usize,
    content_end: usize,
    entry_count: usize,
    max_string_bytes: usize,
    retained_bytes: usize,
}

struct StatisticsOracle {
    declared_message_count: u64,
    actual_message_count: u64,
    declared_channel_count: u32,
    actual_channel_count: u32,
    declared_chunk_count: u32,
    actual_chunk_count: u32,
    channel_message_counts: BTreeMap<u16, u64>,
}

fn generated_nested_metadata(
    fixture: super::NestedCollectionFixture,
    entries: usize,
) -> BTreeMap<String, String> {
    match fixture.shape {
        NestedCollectionShape::OversizedString => [(
            "k".repeat(fixture.limits.max_string_bytes + 1),
            "v".to_owned(),
        )]
        .into(),
        NestedCollectionShape::TooManyRetainedBytes => [(
            "k".to_owned(),
            "v".repeat(fixture.limits.max_retained_bytes + 1),
        )]
        .into(),
        _ => (0..entries)
            .map(|index| (format!("k{index:04}"), format!("v{index:04}")))
            .collect(),
    }
}

fn nested_entry_count(fixture: super::NestedCollectionFixture) -> usize {
    match fixture.shape {
        NestedCollectionShape::MaximumDensity
        | NestedCollectionShape::OversizedString
        | NestedCollectionShape::WrongByteLength
        | NestedCollectionShape::PrematureEof => fixture.limits.max_entries,
        NestedCollectionShape::TooManyEntries => fixture.limits.max_entries + 1,
        NestedCollectionShape::TooManyRetainedBytes => match fixture.target {
            NestedCollectionTarget::ChannelMetadata => 1,
            NestedCollectionTarget::MessageIndexRecords => {
                fixture.limits.max_retained_bytes / 16 + 1
            }
            NestedCollectionTarget::ChunkIndexMessageIndexOffsets
            | NestedCollectionTarget::StatisticsChannelMessageCounts => {
                fixture.limits.max_retained_bytes / 10 + 1
            }
        },
    }
}

impl AdversarialMcapFixtureBuilder {
    pub fn build(mut self) -> Result<AdversarialMcapFixture, FixtureBuildError> {
        self.prepare_inputs()?;

        let (mut mcap_format_violations, remote_expected_outcomes, fake_component_outcomes) =
            self.expected_outcomes();
        let mut bytes = mcap::MAGIC.to_vec();
        let mut layout = FixtureLayout::default();
        append_record(&mut bytes, &mut layout, encode_header());

        let mut built_chunks = Vec::with_capacity(self.chunks.len());
        let mut nested_patches = Vec::new();
        for chunk_index in 0..self.chunks.len() {
            let payload = self.build_chunk_payload(chunk_index)?;
            let built = self.append_chunk(
                &mut bytes,
                &mut layout,
                chunk_index,
                &payload,
                &mut nested_patches,
            )?;
            built_chunks.push(built);
        }

        self.apply_cross_chunk_message_index_fault(&mut built_chunks)?;

        if !self.top_level_messages.is_empty() {
            for schema in &self.schemas {
                append_record(&mut bytes, &mut layout, encode_schema(schema));
            }
            for channel in &self.channels {
                append_record(&mut bytes, &mut layout, encode_channel(channel));
            }
            for message in &self.top_level_messages {
                append_record(&mut bytes, &mut layout, encode_message(message));
            }
        }

        let data_end = append_record(
            &mut bytes,
            &mut layout,
            EncodedRecord::new(op::DATA_END, 0u32.to_le_bytes().to_vec()),
        );
        layout.data_end = Some(data_end);
        layout.summary_start = bytes.len();

        let mut descriptors: Vec<_> = built_chunks
            .iter()
            .map(|chunk| chunk.descriptor.clone())
            .collect();
        self.apply_physical_layout_fault(
            &mut descriptors,
            data_end,
            layout.summary_start,
            &mut mcap_format_violations,
        )?;
        layout.chunk_indexes = descriptors
            .iter()
            .map(FixtureChunkIndexLayout::from)
            .collect();

        let summary_start = bytes.len();
        let mut summary_groups = Vec::<(u8, usize, usize)>::new();

        let schemas_start = bytes.len();
        for schema in &self.schemas {
            append_record(&mut bytes, &mut layout, encode_schema(schema));
        }
        if bytes.len() > schemas_start {
            summary_groups.push((op::SCHEMA, schemas_start, bytes.len() - schemas_start));
        }

        let channels_start = bytes.len();
        for channel in &self.channels {
            let span = append_record(&mut bytes, &mut layout, encode_channel(channel));
            nested_patches.push(NestedPatch {
                target: NestedCollectionTarget::ChannelMetadata,
                location: NestedCollectionLocation::Summary,
                length_offset: channel_metadata_length_offset(span, channel),
                content_end: span.end,
                entry_count: channel.metadata.len(),
                max_string_bytes: channel
                    .metadata
                    .iter()
                    .flat_map(|(key, value)| [key.len(), value.len()])
                    .max()
                    .unwrap_or(0),
                retained_bytes: channel
                    .metadata
                    .iter()
                    .map(|(key, value)| key.len() + value.len())
                    .sum(),
            });
        }
        if bytes.len() > channels_start {
            summary_groups.push((op::CHANNEL, channels_start, bytes.len() - channels_start));
        }

        let statistics_start = bytes.len();
        let (statistics, statistics_nested_offset, statistics_oracle) = self.encode_statistics();
        let statistics_span = append_record(&mut bytes, &mut layout, statistics);
        layout.statistics = Some(FixtureStatisticsLayout {
            record: statistics_span,
            declared_message_count: statistics_oracle.declared_message_count,
            actual_message_count: statistics_oracle.actual_message_count,
            declared_channel_count: statistics_oracle.declared_channel_count,
            actual_channel_count: statistics_oracle.actual_channel_count,
            declared_chunk_count: statistics_oracle.declared_chunk_count,
            actual_chunk_count: statistics_oracle.actual_chunk_count,
            channel_message_counts: statistics_oracle.channel_message_counts.clone(),
        });
        nested_patches.push(NestedPatch {
            target: NestedCollectionTarget::StatisticsChannelMessageCounts,
            location: NestedCollectionLocation::Summary,
            length_offset: statistics_span.body_start + statistics_nested_offset,
            content_end: statistics_span.end,
            entry_count: statistics_oracle.channel_message_counts.len(),
            max_string_bytes: 0,
            retained_bytes: statistics_oracle.channel_message_counts.len() * 10,
        });
        summary_groups.push((
            op::STATISTICS,
            statistics_start,
            bytes.len() - statistics_start,
        ));

        let chunk_indexes_start = bytes.len();
        for descriptor in &descriptors {
            let (record, nested_offset) = encode_chunk_index(descriptor);
            let span = append_record(&mut bytes, &mut layout, record);
            nested_patches.push(NestedPatch {
                target: NestedCollectionTarget::ChunkIndexMessageIndexOffsets,
                location: NestedCollectionLocation::Summary,
                length_offset: span.body_start + nested_offset,
                content_end: span.body_start
                    + nested_offset
                    + 4
                    + descriptor.message_index_offsets.len() * 10,
                entry_count: descriptor.message_index_offsets.len(),
                max_string_bytes: 0,
                retained_bytes: descriptor.message_index_offsets.len() * 10,
            });
        }
        if bytes.len() > chunk_indexes_start {
            summary_groups.push((
                op::CHUNK_INDEX,
                chunk_indexes_start,
                bytes.len() - chunk_indexes_start,
            ));
        }

        for index in 0..self.extra_summary_records {
            append_record(
                &mut bytes,
                &mut layout,
                EncodedRecord::new(0x80, vec![index as u8]),
            );
        }

        let summary_offset_start = if self.summary_offsets {
            let start = bytes.len();
            for (group_opcode, group_start, group_length) in &summary_groups {
                let mut body = Vec::new();
                body.push(*group_opcode);
                push_u64(&mut body, *group_start as u64);
                push_u64(&mut body, *group_length as u64);
                append_record(
                    &mut bytes,
                    &mut layout,
                    EncodedRecord::new(op::SUMMARY_OFFSET, body),
                );
            }
            start
        } else {
            0
        };

        let footer_start = bytes.len();
        let mut footer_prefix = Vec::new();
        footer_prefix.push(op::FOOTER);
        push_u64(&mut footer_prefix, 20);
        push_u64(&mut footer_prefix, summary_start as u64);
        push_u64(&mut footer_prefix, summary_offset_start as u64);
        let mut covered = bytes[summary_start..].to_vec();
        covered.extend_from_slice(&footer_prefix);
        let summary_crc = crc_field(self.summary_crc, &covered);
        let mut footer_body = Vec::new();
        push_u64(&mut footer_body, summary_start as u64);
        push_u64(&mut footer_body, summary_offset_start as u64);
        push_u32(&mut footer_body, summary_crc);
        let footer = append_record(
            &mut bytes,
            &mut layout,
            EncodedRecord::new(op::FOOTER, footer_body),
        );
        if footer.start != footer_start {
            return Err(FixtureBuildError(
                "Footer position changed during assembly".to_owned(),
            ));
        }
        layout.footer = Some(footer);
        bytes.extend_from_slice(mcap::MAGIC);

        self.apply_nested_collection_shape(&mut bytes, &nested_patches)?;
        if !mcap_format_violations.contains(&McapFormatViolation::PrematureEof) {
            rewrite_summary_crc(&mut bytes, &layout, self.summary_crc)?;
        }
        layout.nested_collections = nested_patches
            .iter()
            .filter_map(|patch| {
                let end = patch.length_offset.checked_add(4)?;
                let raw: [u8; 4] = bytes.get(patch.length_offset..end)?.try_into().ok()?;
                Some(FixtureNestedCollectionLayout {
                    target: patch.target,
                    length_offset: patch.length_offset,
                    content_end: patch.content_end,
                    declared_encoded_bytes: u32::from_le_bytes(raw),
                    actual_encoded_bytes: patch.content_end.saturating_sub(end),
                    entry_count: patch.entry_count,
                    max_string_bytes: patch.max_string_bytes,
                    retained_bytes: patch.retained_bytes,
                })
            })
            .collect();

        layout.chunks = built_chunks.into_iter().map(|chunk| chunk.layout).collect();
        let fixture = AdversarialMcapFixture {
            bytes,
            layout,
            mcap_format_violations,
            remote_expected_outcomes,
            fake_component_outcomes,
            cardinality: self.cardinality,
            time_type: self.time_type,
            nested_collection: self.nested_collection,
            nested_collection_location: self.nested_collection_location,
            compression: self.compression,
            chunk_crc: self.chunk_crc,
            summary_crc: self.summary_crc,
            summary_offsets: self.summary_offsets,
            message_index_fault: self.message_index_fault,
            statistics_fixture: self.statistics_fixture,
            definition_fixture: self.definition_fixture,
            physical_layout_fault: self.physical_layout_fault,
            decoder: self.decoder,
            partition: self.partition,
        };
        fixture.validate_self().map_err(FixtureBuildError)?;
        if fixture.is_mcap_format_valid() {
            fixture.validate_upstream_differential()?;
        }
        Ok(fixture)
    }

    fn prepare_inputs(&mut self) -> Result<(), FixtureBuildError> {
        if self.chunks.is_empty() {
            return Err(FixtureBuildError(
                "an adversarial MCAP fixture needs at least one Chunk".to_owned(),
            ));
        }
        if self.channels.is_empty() {
            return Err(FixtureBuildError(
                "an adversarial MCAP fixture needs at least one Channel".to_owned(),
            ));
        }

        self.validate_nested_location_and_composition()?;
        if self.compression == CompressionFixture::TruncatedInput
            && self.definition_fixture != DefinitionFixture::Matching
        {
            return Err(FixtureBuildError(
                "truncated compressed input would obscure the requested definition fixture"
                    .to_owned(),
            ));
        }
        if self.compression == CompressionFixture::TruncatedInput
            && self.chunks.iter().any(|chunk| {
                chunk
                    .messages
                    .iter()
                    .any(|message| message.publish_time > i64::MAX as u64)
            })
        {
            return Err(FixtureBuildError(
                "truncated compressed input would obscure an invalid publish_time".to_owned(),
            ));
        }

        if self.cardinality_override
            && (self.definition_fixture != DefinitionFixture::Matching
                || self.nested_collection.is_some())
        {
            return Err(FixtureBuildError(
                "explicit file cardinality cannot be combined with definition or nested-collection shape generation because both own the same records"
                    .to_owned(),
            ));
        }

        self.prepare_definition_fixture();
        self.prepare_nested_collection_fixture()?;
        self.prepare_cardinality_fixture()?;
        self.prepare_message_index_fixture()?;
        self.refresh_cardinality_metadata();
        self.prepare_partition_byte_contract()?;
        self.prepare_decoder_byte_contract()?;

        let known_channels: BTreeSet<_> = self.channels.iter().map(|channel| channel.id).collect();
        if self.definition_fixture != DefinitionFixture::UnknownReferenced {
            for message in self
                .chunks
                .iter()
                .flat_map(|chunk| &chunk.messages)
                .chain(&self.top_level_messages)
            {
                if !known_channels.contains(&message.channel_id) {
                    return Err(FixtureBuildError(format!(
                        "message references channel {} absent from the Summary fixture",
                        message.channel_id
                    )));
                }
            }
        }

        Ok(())
    }

    fn validate_nested_location_and_composition(&self) -> Result<(), FixtureBuildError> {
        let Some(nested) = self.nested_collection else {
            return Ok(());
        };
        let location = self
            .nested_collection_location
            .unwrap_or_else(|| NestedCollectionLocation::default_for(nested.target));
        let chunk_index = match location {
            NestedCollectionLocation::Summary => None,
            NestedCollectionLocation::Chunk { chunk_index }
            | NestedCollectionLocation::MessageIndexRegion { chunk_index } => Some(chunk_index),
        };
        if let Some(chunk_index) = chunk_index
            && chunk_index >= self.chunks.len()
        {
            return Err(FixtureBuildError(format!(
                "nested collection location references absent Chunk {chunk_index}"
            )));
        }
        if nested.shape != NestedCollectionShape::PrematureEof {
            return Ok(());
        }

        match location {
            NestedCollectionLocation::Chunk { chunk_index } => {
                if self.definition_fixture != DefinitionFixture::Matching {
                    return Err(FixtureBuildError(
                        "Chunk-local premature EOF would obscure the requested definition fixture"
                            .to_owned(),
                    ));
                }
                let chunk = &self.chunks[chunk_index];
                if chunk
                    .messages
                    .iter()
                    .any(|message| message.publish_time > i64::MAX as u64)
                {
                    return Err(FixtureBuildError(
                        "Chunk-local premature EOF would obscure an invalid publish_time"
                            .to_owned(),
                    ));
                }
                if !chunk.messages.is_empty()
                    && matches!(
                        self.message_index_fault,
                        MessageIndexFault::Missing
                            | MessageIndexFault::EntryTime
                            | MessageIndexFault::WrongOpcode
                    )
                {
                    return Err(FixtureBuildError(
                        "Chunk-local premature EOF would obscure the actual Message log-time extent"
                            .to_owned(),
                    ));
                }
            }
            NestedCollectionLocation::Summary
            | NestedCollectionLocation::MessageIndexRegion { .. } => {
                if self.summary_crc != FixtureCrc::ValidNonZero
                    || self.statistics_fixture != StatisticsFixture::Exact
                    || self.definition_fixture != DefinitionFixture::Matching
                    || self.physical_layout_fault != PhysicalLayoutFault::None
                    || self.message_index_fault != MessageIndexFault::None
                    || self.summary_offsets_override
                    || self.cardinality_override
                    || self.decoder_override
                    || self.partition_override
                    || self.chunks.iter().any(|chunk| chunk.index_range.is_some())
                {
                    return Err(FixtureBuildError(
                        "outer premature EOF would obscure another explicitly requested control"
                            .to_owned(),
                    ));
                }
            }
        }
        Ok(())
    }

    fn prepare_definition_fixture(&mut self) {
        let needs_schema = matches!(
            self.definition_fixture,
            DefinitionFixture::MissingReferencedSchema
                | DefinitionFixture::SchemaNameConflict
                | DefinitionFixture::SchemaEncodingConflict
                | DefinitionFixture::SchemaDataConflict
                | DefinitionFixture::ChannelSchemaConflict
        );
        if needs_schema && self.schemas.is_empty() {
            self.schemas
                .push(FixtureSchema::new(1, "fixture.Schema", "protobuf").with_data([0x0a, 0x00]));
            self.channels[0].schema_id = 1;
            self.channels[0].message_encoding = "protobuf".to_owned();
        }
        if self.definition_fixture == DefinitionFixture::MissingReferencedSchema {
            self.schemas.clear();
            self.channels[0].schema_id = 1;
        }
        if self.definition_fixture == DefinitionFixture::ChannelSchemaConflict {
            let current = self.channels[0].schema_id;
            if !self.schemas.iter().any(|schema| schema.id != current) {
                let id = current.wrapping_add(1).max(1);
                self.schemas.push(
                    FixtureSchema::new(id, "fixture.AlternateSchema", "protobuf")
                        .with_data([0x12, 0x00]),
                );
            }
        }
        if self.definition_fixture == DefinitionFixture::UnknownReferenced {
            self.chunks[0]
                .messages
                .push(FixtureMessage::new(99, u32::MAX, 2));
        }
        if self.definition_fixture == DefinitionFixture::UnselectedChannelConflict
            && !self.channels.iter().any(|channel| channel.id == 2)
        {
            self.channels
                .push(FixtureChannel::schema_less(2, "/unselected"));
        }
    }

    fn prepare_message_index_fixture(&mut self) -> Result<(), FixtureBuildError> {
        let needs_second_channel = matches!(
            self.message_index_fault,
            MessageIndexFault::MapKeyMismatch | MessageIndexFault::DuplicateDescriptorOffset
        );
        if !needs_second_channel || self.channels.len() >= 2 {
            return Ok(());
        }
        if self.cardinality_override {
            return Err(FixtureBuildError(
                "this MessageIndex fixture requires cardinality.channels >= 2".to_owned(),
            ));
        }
        let mut id = self.channels[0].id.wrapping_add(1);
        while self.channels.iter().any(|channel| channel.id == id) {
            id = id.wrapping_add(1);
        }
        self.channels
            .push(FixtureChannel::schema_less(id, "/fixture/index-alternate"));
        if self.message_index_fault == MessageIndexFault::DuplicateDescriptorOffset {
            for (chunk_index, chunk) in self.chunks.iter_mut().enumerate() {
                let log_time = chunk.actual_range().end.saturating_add(chunk_index as u64);
                chunk
                    .messages
                    .push(FixtureMessage::new(id, u32::MAX, log_time));
            }
        }
        Ok(())
    }

    fn refresh_cardinality_metadata(&mut self) {
        if self.cardinality_override {
            return;
        }
        self.cardinality.schemas = self.schemas.len();
        self.cardinality.channels = self.channels.len();
        self.cardinality.chunk_indexes = self.chunks.len()
            + usize::from(matches!(
                self.physical_layout_fault,
                PhysicalLayoutFault::DuplicateChunkIndex
                    | PhysicalLayoutFault::ConflictingDuplicateChunkIndex
            ));
        self.cardinality.messages_per_chunk = self
            .chunks
            .iter()
            .map(|chunk| chunk.messages.len())
            .max()
            .unwrap_or(0);
        let selected_channels: BTreeSet<_> = self
            .partition
            .selected_group_members
            .iter()
            .flatten()
            .copied()
            .collect();
        self.cardinality.selected_dispatches_per_scan = self
            .chunks
            .iter()
            .flat_map(|chunk| &chunk.messages)
            .filter(|message| selected_channels.contains(&message.channel_id))
            .count();
        let extra_definition_records =
            match self.definition_fixture {
                DefinitionFixture::Matching | DefinitionFixture::MissingReferencedSchema => 0,
                DefinitionFixture::ExactDuplicate => self.schemas.len() + self.channels.len(),
                DefinitionFixture::UnknownUnreferenced | DefinitionFixture::UnknownReferenced => 2,
                DefinitionFixture::SchemaNameConflict
                | DefinitionFixture::SchemaEncodingConflict
                | DefinitionFixture::SchemaDataConflict
                | DefinitionFixture::ChannelSchemaConflict
                | DefinitionFixture::ChannelTopicConflict
                | DefinitionFixture::ChannelEncodingConflict
                | DefinitionFixture::ChannelMetadataConflict
                | DefinitionFixture::UnselectedChannelConflict
                | DefinitionFixture::ChunkTailConflict => 1,
            } + usize::from(self.trailing_canonical_channel_after_conflict);
        self.cardinality.records_per_chunk = self.schemas.len()
            + self.channels.len()
            + self.cardinality.messages_per_chunk
            + extra_definition_records
            + self.extra_records_per_chunk;
        self.cardinality.summary_records = self.schemas.len()
            + self.channels.len()
            + 1
            + self.cardinality.chunk_indexes
            + self.extra_summary_records;
    }

    fn prepare_nested_collection_fixture(&mut self) -> Result<(), FixtureBuildError> {
        let Some(fixture) = self.nested_collection else {
            return Ok(());
        };
        let location = self
            .nested_collection_location
            .unwrap_or_else(|| NestedCollectionLocation::default_for(fixture.target));
        let location_is_compatible = match fixture.target {
            NestedCollectionTarget::ChannelMetadata => matches!(
                location,
                NestedCollectionLocation::Summary | NestedCollectionLocation::Chunk { .. }
            ),
            NestedCollectionTarget::MessageIndexRecords => {
                matches!(
                    location,
                    NestedCollectionLocation::MessageIndexRegion { .. }
                )
            }
            NestedCollectionTarget::ChunkIndexMessageIndexOffsets
            | NestedCollectionTarget::StatisticsChannelMessageCounts => {
                location == NestedCollectionLocation::Summary
            }
        };
        if !location_is_compatible {
            return Err(FixtureBuildError(format!(
                "nested {:?} cannot be placed at {location:?}",
                fixture.target
            )));
        }
        let entries = nested_entry_count(fixture);
        match fixture.target {
            NestedCollectionTarget::ChannelMetadata => {
                // A repeated Channel definition must remain byte-equivalent to its Summary
                // definition. The requested location owns the malformed-length patch and the
                // location assertion, not a conflicting semantic Channel value.
                self.channels[0].metadata = generated_nested_metadata(fixture, entries);
            }
            NestedCollectionTarget::MessageIndexRecords => {
                let NestedCollectionLocation::MessageIndexRegion { chunk_index } = location else {
                    return Err(FixtureBuildError(
                        "MessageIndex records require a MessageIndexRegion location".to_owned(),
                    ));
                };
                let chunk = self.chunks.get_mut(chunk_index).ok_or_else(|| {
                    FixtureBuildError(format!(
                        "nested collection location references absent Chunk {chunk_index}"
                    ))
                })?;
                chunk.messages.clear();
                for index in 0..entries {
                    chunk.messages.push(FixtureMessage::new(
                        self.channels[0].id,
                        index as u32,
                        index as u64,
                    ));
                }
            }
            NestedCollectionTarget::ChunkIndexMessageIndexOffsets
            | NestedCollectionTarget::StatisticsChannelMessageCounts => {
                self.schemas.clear();
                self.channels.clear();
                self.chunks[0].messages.clear();
                for index in 0..entries {
                    let id = u16::try_from(index + 1).map_err(|_error| {
                        FixtureBuildError("nested fixture exceeds u16 Channel IDs".to_owned())
                    })?;
                    self.channels
                        .push(FixtureChannel::schema_less(id, format!("/fixture/{index}")));
                    self.chunks[0]
                        .messages
                        .push(FixtureMessage::new(id, 0, index as u64));
                }
            }
        }
        Ok(())
    }

    fn prepare_partition_byte_contract(&mut self) -> Result<(), FixtureBuildError> {
        if !self.partition_override {
            let first_channel = self.channels.first().ok_or_else(|| {
                FixtureBuildError("partition fixture needs a serialized Channel".to_owned())
            })?;
            self.partition.selected_group_members = vec![vec![first_channel.id]];
            self.partition.selected_channel_groups = 1;
            self.partition.channels_per_selected_group = 1;
            self.partition.cursor_time = self
                .chunks
                .first()
                .map(FixtureChunk::actual_range)
                .map_or(0, |range| range.start);
            self.partition.cursor_covering_chunks = self
                .chunks
                .iter()
                .filter(|chunk| {
                    let range = chunk.index_range.unwrap_or_else(|| chunk.actual_range());
                    range.start <= self.partition.cursor_time
                        && self.partition.cursor_time <= range.end
                })
                .count();
            if matches!(
                self.physical_layout_fault,
                PhysicalLayoutFault::DuplicateChunkIndex
                    | PhysicalLayoutFault::ConflictingDuplicateChunkIndex
            ) && self.chunks.first().is_some_and(|chunk| {
                let range = chunk.index_range.unwrap_or_else(|| chunk.actual_range());
                range.start <= self.partition.cursor_time && self.partition.cursor_time <= range.end
            }) {
                self.partition.cursor_covering_chunks += 1;
            }
            return Ok(());
        }
        if self.partition.selected_channel_groups != self.partition.selected_group_members.len() {
            return Err(FixtureBuildError(
                "selected Channel-group count does not match serialized membership".to_owned(),
            ));
        }
        let max_group_channels = self
            .partition
            .selected_group_members
            .iter()
            .map(Vec::len)
            .max()
            .unwrap_or(0);
        if self.partition.channels_per_selected_group != max_group_channels {
            return Err(FixtureBuildError(
                "channels-per-group count does not match serialized membership".to_owned(),
            ));
        }
        let known_channels: BTreeSet<_> = self.channels.iter().map(|channel| channel.id).collect();
        if let Some(unknown) = self
            .partition
            .selected_group_members
            .iter()
            .flatten()
            .find(|channel_id| !known_channels.contains(channel_id))
        {
            return Err(FixtureBuildError(format!(
                "selected group references absent serialized Channel {unknown}"
            )));
        }
        let covering_chunks = self
            .chunks
            .iter()
            .filter(|chunk| {
                let range = chunk.index_range.unwrap_or_else(|| chunk.actual_range());
                range.start <= self.partition.cursor_time && self.partition.cursor_time <= range.end
            })
            .count();
        if covering_chunks != self.partition.cursor_covering_chunks {
            return Err(FixtureBuildError(format!(
                "cursor-covering Chunk count differs from serialized ranges: {covering_chunks} != {}",
                self.partition.cursor_covering_chunks
            )));
        }
        Ok(())
    }

    fn prepare_decoder_byte_contract(&mut self) -> Result<(), FixtureBuildError> {
        if !self.decoder_override {
            let channel = self.channels.first().ok_or_else(|| {
                FixtureBuildError("decoder fixture needs a serialized Channel".to_owned())
            })?;
            let schema = self
                .schemas
                .iter()
                .find(|schema| schema.id == channel.schema_id);
            self.decoder.recognition_input = super::DecoderRecognitionInput {
                channel_id: channel.id,
                schema_id: channel.schema_id,
                schema_name: schema.map(|schema| schema.name.clone()),
                schema_encoding: schema.map(|schema| schema.encoding.clone()),
                message_encoding: channel.message_encoding.clone(),
            };
            return Ok(());
        }

        let input = &self.decoder.recognition_input;
        let channel = self
            .channels
            .iter()
            .find(|channel| channel.id == input.channel_id)
            .ok_or_else(|| {
                FixtureBuildError(format!(
                    "decoder recognition references absent Channel {}",
                    input.channel_id
                ))
            })?;
        let schema = self
            .schemas
            .iter()
            .find(|schema| schema.id == channel.schema_id);
        let matches = channel.schema_id == input.schema_id
            && channel.message_encoding == input.message_encoding
            && schema.map(|schema| schema.name.as_str()) == input.schema_name.as_deref()
            && schema.map(|schema| schema.encoding.as_str()) == input.schema_encoding.as_deref();
        if !matches {
            return Err(FixtureBuildError(
                "decoder recognition input differs from serialized Channel/Schema".to_owned(),
            ));
        }
        Ok(())
    }

    fn prepare_cardinality_fixture(&mut self) -> Result<(), FixtureBuildError> {
        if !self.cardinality_override {
            return Ok(());
        }
        if self.cardinality.messages_per_chunk == 0 {
            for chunk in &mut self.chunks {
                chunk.messages.clear();
            }
        }
        if self.cardinality.messages_per_chunk > u32::MAX as usize {
            return Err(FixtureBuildError(
                "message cardinality exceeds sequence representation".to_owned(),
            ));
        }
        self.schemas.clear();
        for index in 0..self.cardinality.schemas {
            let id = u16::try_from(index + 1)
                .map_err(|_error| FixtureBuildError("schema cardinality exceeds u16".to_owned()))?;
            self.schemas.push(
                FixtureSchema::new(id, format!("fixture.Schema{index}"), "protobuf")
                    .with_data([index as u8]),
            );
        }

        self.channels.clear();
        for index in 0..self.cardinality.channels {
            let id = u16::try_from(index + 1).map_err(|_error| {
                FixtureBuildError("channel cardinality exceeds u16".to_owned())
            })?;
            let channel = if self.schemas.is_empty() {
                FixtureChannel::schema_less(id, format!("/fixture/{index}"))
            } else {
                let schema = &self.schemas[index % self.schemas.len()];
                FixtureChannel::schema_less(id, format!("/fixture/{index}"))
                    .with_schema(schema.id, "protobuf")
            };
            self.channels.push(channel);
        }
        if self.channels.is_empty() && self.cardinality.messages_per_chunk > 0 {
            return Err(FixtureBuildError(
                "nonzero messages_per_chunk requires a Channel".to_owned(),
            ));
        }

        let selected_channels: Vec<_> = self
            .partition
            .selected_group_members
            .iter()
            .flatten()
            .copied()
            .filter(|channel_id| {
                self.channels
                    .iter()
                    .any(|channel| channel.id == *channel_id)
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let unselected_channels: Vec<_> = self
            .channels
            .iter()
            .map(|channel| channel.id)
            .filter(|channel_id| !selected_channels.contains(channel_id))
            .collect();
        let total_messages = self
            .cardinality
            .chunk_indexes
            .checked_mul(self.cardinality.messages_per_chunk)
            .ok_or_else(|| FixtureBuildError("message cardinality overflow".to_owned()))?;
        if self.cardinality.selected_dispatches_per_scan > total_messages {
            return Err(FixtureBuildError(
                "selected dispatch cardinality exceeds serialized Messages".to_owned(),
            ));
        }
        if self.cardinality.selected_dispatches_per_scan > 0 && selected_channels.is_empty() {
            return Err(FixtureBuildError(
                "selected dispatch cardinality requires a serialized selected Channel".to_owned(),
            ));
        }
        if self.cardinality.selected_dispatches_per_scan < total_messages
            && unselected_channels.is_empty()
        {
            return Err(FixtureBuildError(
                "unselected serialized Messages require an unselected Channel".to_owned(),
            ));
        }

        self.chunks.clear();
        let mut global_message_index = 0usize;
        for chunk_index in 0..self.cardinality.chunk_indexes {
            let mut messages = Vec::new();
            for message_index in 0..self.cardinality.messages_per_chunk {
                let channel_id =
                    if global_message_index < self.cardinality.selected_dispatches_per_scan {
                        selected_channels[global_message_index % selected_channels.len()]
                    } else {
                        unselected_channels[(global_message_index
                            - self.cardinality.selected_dispatches_per_scan)
                            % unselected_channels.len()]
                    };
                messages.push(FixtureMessage::new(
                    channel_id,
                    message_index as u32,
                    (chunk_index * self.cardinality.messages_per_chunk + message_index) as u64,
                ));
                global_message_index += 1;
            }
            self.chunks.push(FixtureChunk::new(messages));
        }
        if self.chunks.is_empty() {
            return Err(FixtureBuildError(
                "chunk index cardinality must be nonzero for this fixture".to_owned(),
            ));
        }

        let base_chunk_records =
            self.schemas.len() + self.channels.len() + self.cardinality.messages_per_chunk;
        self.extra_records_per_chunk = self
            .cardinality
            .records_per_chunk
            .checked_sub(base_chunk_records)
            .ok_or_else(|| {
                FixtureBuildError(format!(
                    "records_per_chunk {} is below the {} required definition/message records",
                    self.cardinality.records_per_chunk, base_chunk_records
                ))
            })?;
        let base_summary_records =
            self.schemas.len() + self.channels.len() + 1 + self.cardinality.chunk_indexes;
        self.extra_summary_records = self
            .cardinality
            .summary_records
            .checked_sub(base_summary_records)
            .ok_or_else(|| {
                FixtureBuildError(format!(
                    "summary_records {} is below the {base_summary_records} required records",
                    self.cardinality.summary_records
                ))
            })?;
        Ok(())
    }

    fn expected_outcomes(
        &self,
    ) -> (
        BTreeSet<McapFormatViolation>,
        BTreeSet<RemotePolicyOutcome>,
        BTreeSet<FakeComponentOutcome>,
    ) {
        let mut format = BTreeSet::new();
        let mut remote = BTreeSet::new();
        let mut fake = BTreeSet::new();
        for chunk in &self.chunks {
            let actual = chunk.actual_range();
            if chunk.header_range.is_some_and(|range| range != actual)
                || chunk.index_range.is_some_and(|range| range != actual)
            {
                format.insert(McapFormatViolation::ChunkDeclaredTimeRange);
            }
            if chunk.messages.iter().any(|message| {
                message.log_time > i64::MAX as u64 || message.publish_time > i64::MAX as u64
            }) || chunk
                .header_range
                .is_some_and(|range| range.start > i64::MAX as u64 || range.end > i64::MAX as u64)
                || chunk.index_range.is_some_and(|range| {
                    range.start > i64::MAX as u64 || range.end > i64::MAX as u64
                })
            {
                remote.insert(RemotePolicyOutcome::InvalidTemporalValue);
            }
        }
        if self.top_level_messages.iter().any(|message| {
            message.log_time > i64::MAX as u64 || message.publish_time > i64::MAX as u64
        }) {
            remote.insert(RemotePolicyOutcome::InvalidTemporalValue);
        }
        if self.message_index_fault == MessageIndexFault::EntryTime
            && self
                .chunks
                .first()
                .and_then(|chunk| chunk.messages.first())
                .is_some_and(|message| message.log_time.wrapping_add(1) > i64::MAX as u64)
        {
            remote.insert(RemotePolicyOutcome::InvalidTemporalValue);
        }
        match self.message_index_fault {
            MessageIndexFault::None => {}
            MessageIndexFault::Missing => {
                remote.insert(RemotePolicyOutcome::MessageIndexAbsent);
            }
            MessageIndexFault::Duplicate => {
                format.insert(McapFormatViolation::MessageIndexDuplicate);
            }
            MessageIndexFault::EntryOffset => {
                format.insert(McapFormatViolation::MessageIndexEntryOffset);
            }
            MessageIndexFault::EntryTime => {
                format.insert(McapFormatViolation::MessageIndexEntryTime);
            }
            MessageIndexFault::WrongOpcode => {
                format.insert(McapFormatViolation::MessageIndexOpcode);
            }
            MessageIndexFault::MapKeyMismatch => {
                format.insert(McapFormatViolation::MessageIndexMapKey);
            }
            MessageIndexFault::RecordCrossesOwningRegion => {
                format.insert(McapFormatViolation::MessageIndexOwningRegion);
            }
            MessageIndexFault::DescriptorOffsetIntoRecordBody
            | MessageIndexFault::DescriptorOffsetIntoChunk => {
                format.insert(McapFormatViolation::MessageIndexDescriptorOffset);
            }
            MessageIndexFault::DuplicateDescriptorOffset => {
                format.insert(McapFormatViolation::MessageIndexDuplicateDescriptorOffset);
                // A map has unique Channel keys, so two distinct keys pointing at one record
                // necessarily make at least one key disagree with that record's Channel ID.
                format.insert(McapFormatViolation::MessageIndexMapKey);
            }
            MessageIndexFault::CrossChunkAlias => {
                format.insert(McapFormatViolation::MessageIndexCrossChunkAlias);
            }
        }
        if self.statistics_fixture != StatisticsFixture::Exact {
            format.insert(McapFormatViolation::StatisticsCount);
        }
        if self.summary_crc == FixtureCrc::InvalidNonZero {
            format.insert(McapFormatViolation::SummaryCrc);
        }
        if self.chunk_crc == FixtureCrc::InvalidNonZero {
            format.insert(McapFormatViolation::ChunkCrc);
        }
        match self.definition_fixture {
            DefinitionFixture::Matching
            | DefinitionFixture::ExactDuplicate
            | DefinitionFixture::UnknownUnreferenced => {}
            DefinitionFixture::UnknownReferenced => {
                format.insert(McapFormatViolation::UnknownReferencedChannel);
            }
            DefinitionFixture::MissingReferencedSchema => {
                format.insert(McapFormatViolation::MissingReferencedSchema);
            }
            DefinitionFixture::SchemaNameConflict
            | DefinitionFixture::SchemaEncodingConflict
            | DefinitionFixture::SchemaDataConflict => {
                format.insert(McapFormatViolation::ConflictingSchema);
            }
            DefinitionFixture::ChannelSchemaConflict
            | DefinitionFixture::ChannelTopicConflict
            | DefinitionFixture::ChannelEncodingConflict
            | DefinitionFixture::ChannelMetadataConflict
            | DefinitionFixture::UnselectedChannelConflict
            | DefinitionFixture::ChunkTailConflict => {
                format.insert(McapFormatViolation::ConflictingChannel);
            }
        }
        match self.physical_layout_fault {
            PhysicalLayoutFault::None | PhysicalLayoutFault::DuplicateChunkIndex => {}
            PhysicalLayoutFault::ConflictingDuplicateChunkIndex => {
                format.insert(McapFormatViolation::ConflictingChunkIndex);
            }
            _ => {
                format.insert(McapFormatViolation::PhysicalRegionOverlap);
            }
        }
        if let Some(nested) = self.nested_collection {
            match nested.shape {
                NestedCollectionShape::MaximumDensity => {}
                NestedCollectionShape::TooManyEntries => {
                    remote.insert(RemotePolicyOutcome::NestedCollectionCount);
                }
                NestedCollectionShape::OversizedString => {
                    remote.insert(RemotePolicyOutcome::NestedStringLength);
                }
                NestedCollectionShape::TooManyRetainedBytes => {
                    remote.insert(RemotePolicyOutcome::NestedRetainedBytes);
                }
                NestedCollectionShape::WrongByteLength => {
                    format.insert(McapFormatViolation::NestedByteLength);
                }
                NestedCollectionShape::PrematureEof => {
                    format.insert(McapFormatViolation::PrematureEof);
                }
            }
        }
        match self.compression {
            CompressionFixture::None | CompressionFixture::Zstd | CompressionFixture::Lz4 => {}
            CompressionFixture::DeclaredOutputTooSmall
            | CompressionFixture::DeclaredOutputTooLarge => {
                format.insert(McapFormatViolation::CompressedOutputSize);
            }
            CompressionFixture::ConcatenatedFrame => {
                remote.insert(RemotePolicyOutcome::MultipleCompressionFrames);
            }
            CompressionFixture::TrailingPayload => {
                format.insert(McapFormatViolation::CompressedTrailingPayload);
            }
            CompressionFixture::TruncatedInput => {
                format.insert(McapFormatViolation::CompressedInputTruncated);
            }
            CompressionFixture::OversizedDeclaration => {
                format.insert(McapFormatViolation::CompressedOutputSize);
                remote.insert(RemotePolicyOutcome::CompressedDeclarationLimit);
            }
            CompressionFixture::OutputFullBeforeCodecEof => {
                fake.insert(FakeComponentOutcome::CodecDidNotReachEof);
            }
            CompressionFixture::ZeroProgress => {
                fake.insert(FakeComponentOutcome::CodecZeroProgress);
            }
        }
        if self.decoder.singleton_multi_group_overlap {
            fake.insert(FakeComponentOutcome::DecoderGroupOverlap);
        }
        if self.decoder.cross_owner_reference {
            fake.insert(FakeComponentOutcome::DecoderCrossOwnerReference);
        }
        if self.decoder.temporal_output != TemporalOutputFixture::Temporal {
            fake.insert(FakeComponentOutcome::DecoderTemporalOutput);
        }
        if self
            .partition
            .selected_group_roots
            .iter()
            .copied()
            .chain([self.partition.opening_static_roots])
            .chain(
                self.partition
                    .generations
                    .iter()
                    .filter_map(|generation| match generation {
                        PartitionGenerationFixture::CompleteEmpty => None,
                        PartitionGenerationFixture::Roots {
                            root_descriptors, ..
                        } => Some(*root_descriptors),
                    }),
            )
            .any(|roots| roots > self.partition.max_roots_per_partition)
        {
            remote.insert(RemotePolicyOutcome::PartitionRootLimit);
        }
        if self
            .partition
            .external_origin_bytes_per_partition
            .iter()
            .copied()
            .chain(
                self.partition
                    .generations
                    .iter()
                    .filter_map(|generation| match generation {
                        PartitionGenerationFixture::CompleteEmpty => None,
                        PartitionGenerationFixture::Roots {
                            external_origin_bytes,
                            ..
                        } => Some(*external_origin_bytes),
                    }),
            )
            .any(|bytes| bytes > self.partition.max_external_origin_bytes_per_partition)
        {
            remote.insert(RemotePolicyOutcome::PartitionExternalOriginLimit);
        }
        if self.partition.resident_roots > self.partition.session_root_cap {
            remote.insert(RemotePolicyOutcome::SessionRootCap);
        }
        (format, remote, fake)
    }

    fn build_chunk_payload(&self, chunk_index: usize) -> Result<ChunkPayload, FixtureBuildError> {
        let mut bytes = Vec::new();
        let mut record_count = 0;
        let mut messages = Vec::new();
        let mut chunk_nested_patch = None;
        let chunk = &self.chunks[chunk_index];

        let tail_conflict = matches!(
            self.definition_fixture,
            DefinitionFixture::UnselectedChannelConflict | DefinitionFixture::ChunkTailConflict
        );

        for schema in &self.schemas {
            bytes.extend_from_slice(&encode_schema(schema).bytes);
            record_count += 1;
        }
        for (channel_index, channel) in self.channels.iter().enumerate() {
            let mut channel = channel.clone();
            let is_chunk_nested_target = channel_index == 0
                && self
                    .nested_collection
                    .is_some_and(|nested| nested.target == NestedCollectionTarget::ChannelMetadata)
                && self.nested_collection_location
                    == Some(NestedCollectionLocation::Chunk { chunk_index });
            if is_chunk_nested_target {
                let nested = self.nested_collection.expect("checked above");
                let entries = nested_entry_count(nested);
                channel.metadata = generated_nested_metadata(nested, entries);
            }
            let encoded = encode_channel(&channel);
            let record_start = bytes.len();
            bytes.extend_from_slice(&encoded.bytes);
            record_count += 1;
            if is_chunk_nested_target {
                chunk_nested_patch = Some(NestedPatch {
                    target: NestedCollectionTarget::ChannelMetadata,
                    location: NestedCollectionLocation::Chunk { chunk_index },
                    length_offset: record_start
                        + RECORD_HEADER_LEN
                        + 2
                        + 2
                        + 4
                        + channel.topic.len()
                        + 4
                        + channel.message_encoding.len(),
                    content_end: bytes.len(),
                    entry_count: channel.metadata.len(),
                    max_string_bytes: channel
                        .metadata
                        .iter()
                        .flat_map(|(key, value)| [key.len(), value.len()])
                        .max()
                        .unwrap_or(0),
                    retained_bytes: channel
                        .metadata
                        .iter()
                        .map(|(key, value)| key.len() + value.len())
                        .sum(),
                });
            }
        }

        match self.definition_fixture {
            DefinitionFixture::ExactDuplicate => {
                for schema in &self.schemas {
                    bytes.extend_from_slice(&encode_schema(schema).bytes);
                    record_count += 1;
                }
                for channel in &self.channels {
                    bytes.extend_from_slice(&encode_channel(channel).bytes);
                    record_count += 1;
                }
            }
            DefinitionFixture::UnknownUnreferenced | DefinitionFixture::UnknownReferenced => {
                let schema =
                    FixtureSchema::new(99, "unknown.Schema", "protobuf").with_data([0x0a, 0x00]);
                let channel =
                    FixtureChannel::schema_less(99, "/unknown").with_schema(99, "protobuf");
                bytes.extend_from_slice(&encode_schema(&schema).bytes);
                bytes.extend_from_slice(&encode_channel(&channel).bytes);
                record_count += 2;
            }
            DefinitionFixture::SchemaNameConflict
            | DefinitionFixture::SchemaEncodingConflict
            | DefinitionFixture::SchemaDataConflict => {
                let mut schema = self.schemas[0].clone();
                match self.definition_fixture {
                    DefinitionFixture::SchemaNameConflict => schema.name.push_str(".conflict"),
                    DefinitionFixture::SchemaEncodingConflict => {
                        schema.encoding.push_str("-conflict");
                    }
                    DefinitionFixture::SchemaDataConflict => schema.data.push(0xff),
                    _ => unreachable!(),
                }
                bytes.extend_from_slice(&encode_schema(&schema).bytes);
                record_count += 1;
            }
            DefinitionFixture::ChannelSchemaConflict
            | DefinitionFixture::ChannelTopicConflict
            | DefinitionFixture::ChannelEncodingConflict
            | DefinitionFixture::ChannelMetadataConflict => {
                let channel = self.conflicting_channel(self.channels[0].clone());
                bytes.extend_from_slice(&encode_channel(&channel).bytes);
                record_count += 1;
            }
            _ if !tail_conflict => {}
            _ => {}
        }

        for message in &chunk.messages {
            let offset = bytes.len() as u64;
            bytes.extend_from_slice(&encode_message(message).bytes);
            record_count += 1;
            messages.push((message.clone(), offset));
        }

        for index in 0..self.extra_records_per_chunk {
            bytes.extend_from_slice(&EncodedRecord::new(0x80, vec![index as u8]).bytes);
            record_count += 1;
        }

        if tail_conflict {
            let channel = if self.definition_fixture == DefinitionFixture::UnselectedChannelConflict
            {
                let channel = self
                    .channels
                    .iter()
                    .find(|channel| channel.id == 2)
                    .ok_or_else(|| {
                        FixtureBuildError(
                            "unselected conflict fixture is missing Channel 2".to_owned(),
                        )
                    })?
                    .clone();
                FixtureChannel {
                    topic: format!("{}-conflict", channel.topic),
                    ..channel
                }
            } else {
                FixtureChannel {
                    topic: format!("{}-tail-conflict", self.channels[0].topic),
                    ..self.channels[0].clone()
                }
            };
            bytes.extend_from_slice(&encode_channel(&channel).bytes);
            record_count += 1;
        }
        if self.trailing_canonical_channel_after_conflict {
            bytes.extend_from_slice(&encode_channel(&self.channels[0]).bytes);
            record_count += 1;
        }

        if let (Some(nested), Some(patch)) = (self.nested_collection, chunk_nested_patch) {
            apply_nested_shape_bytes(nested, &mut bytes, patch)?;
        }

        Ok(ChunkPayload {
            bytes,
            record_count,
            messages,
        })
    }

    fn conflicting_channel(&self, mut channel: FixtureChannel) -> FixtureChannel {
        match self.definition_fixture {
            DefinitionFixture::ChannelSchemaConflict => {
                channel.schema_id = self
                    .schemas
                    .iter()
                    .find(|schema| schema.id != channel.schema_id)
                    .map_or_else(|| channel.schema_id.wrapping_add(1), |schema| schema.id);
            }
            DefinitionFixture::ChannelTopicConflict => channel.topic.push_str("-conflict"),
            DefinitionFixture::ChannelEncodingConflict => {
                channel.message_encoding.push_str("-conflict");
            }
            DefinitionFixture::ChannelMetadataConflict => {
                channel
                    .metadata
                    .insert("conflict".to_owned(), "true".to_owned());
            }
            _ => {}
        }
        channel
    }

    fn append_chunk(
        &self,
        output: &mut Vec<u8>,
        layout: &mut FixtureLayout,
        chunk_index: usize,
        payload: &ChunkPayload,
        nested_patches: &mut Vec<NestedPatch>,
    ) -> Result<BuiltChunk, FixtureBuildError> {
        let chunk = &self.chunks[chunk_index];
        let actual_range = chunk.actual_range();
        let header_range = chunk.header_range.unwrap_or(actual_range);
        let index_range = chunk.index_range.unwrap_or(actual_range);

        let (compression, mut compressed, mut declared_uncompressed_size) =
            self.compressed_payload(chunk_index, payload)?;
        let canonical_compressed_data_len = compressed.len();
        match self.compression {
            CompressionFixture::DeclaredOutputTooSmall => {
                declared_uncompressed_size = declared_uncompressed_size.saturating_sub(1);
            }
            CompressionFixture::DeclaredOutputTooLarge => {
                declared_uncompressed_size = declared_uncompressed_size.saturating_add(1);
            }
            CompressionFixture::ConcatenatedFrame => {
                let empty_frame = zstd::bulk::compress(&[], 0).map_err(|error| {
                    FixtureBuildError(format!("empty zstd frame compression failed: {error}"))
                })?;
                compressed.extend_from_slice(&empty_frame);
            }
            CompressionFixture::TrailingPayload => compressed.extend_from_slice(b"trailing"),
            CompressionFixture::TruncatedInput => {
                compressed.pop();
            }
            CompressionFixture::OversizedDeclaration => {
                declared_uncompressed_size = u64::MAX;
            }
            _ => {}
        }

        let mut chunk_body = Vec::new();
        push_u64(&mut chunk_body, header_range.start);
        push_u64(&mut chunk_body, header_range.end);
        push_u64(&mut chunk_body, declared_uncompressed_size);
        let actual_uncompressed_crc = crc32(&payload.bytes);
        let declared_uncompressed_crc = crc_field(self.chunk_crc, &payload.bytes);
        if self.chunk_crc == FixtureCrc::ValidNonZero && declared_uncompressed_crc == 0 {
            return Err(FixtureBuildError(
                "generated Chunk payload has a zero CRC and cannot satisfy ValidNonZero".to_owned(),
            ));
        }
        push_u32(&mut chunk_body, declared_uncompressed_crc);
        push_string(&mut chunk_body, &compression);
        push_u64(&mut chunk_body, compressed.len() as u64);
        let compressed_data_offset_in_body = chunk_body.len();
        chunk_body.extend_from_slice(&compressed);

        let chunk_record = EncodedRecord::new(op::CHUNK, chunk_body);
        let chunk_span = append_record(output, layout, chunk_record);
        let compressed_data_start = chunk_span.body_start + compressed_data_offset_in_body;

        let index_region_start = output.len();
        let mut index_offsets = BTreeMap::<u16, u64>::new();
        let mut index_spans = Vec::new();
        let mut per_channel = BTreeMap::<u16, Vec<(u64, u64)>>::new();
        for (message, offset) in &payload.messages {
            per_channel
                .entry(message.channel_id)
                .or_default()
                .push((message.log_time, *offset));
        }

        if self.message_index_fault != MessageIndexFault::Missing || chunk_index != 0 {
            for (position, (&channel_id, entries)) in per_channel.iter().enumerate() {
                let mut body = Vec::new();
                push_u16(&mut body, channel_id);
                push_u32(&mut body, (entries.len() * 16) as u32);
                let mut serialized_entries = Vec::with_capacity(entries.len());
                for (entry_index, &(mut log_time, mut offset)) in entries.iter().enumerate() {
                    if chunk_index == 0 && position == 0 && entry_index == 0 {
                        if self.message_index_fault == MessageIndexFault::EntryTime {
                            log_time = log_time.wrapping_add(1);
                        }
                        if self.message_index_fault == MessageIndexFault::EntryOffset {
                            offset = offset.wrapping_add(1);
                        }
                    }
                    push_u64(&mut body, log_time);
                    push_u64(&mut body, offset);
                    serialized_entries.push(FixtureMessageIndexEntry { log_time, offset });
                }

                let opcode = if chunk_index == 0
                    && position == 0
                    && self.message_index_fault == MessageIndexFault::WrongOpcode
                {
                    op::CHUNK_INDEX
                } else {
                    op::MESSAGE_INDEX
                };
                let record = EncodedRecord::new(opcode, body);
                let span = append_record(output, layout, record.clone());
                index_spans.push(FixtureMessageIndexLayout {
                    record: span,
                    channel_id,
                    entries: serialized_entries.clone(),
                });
                nested_patches.push(NestedPatch {
                    target: NestedCollectionTarget::MessageIndexRecords,
                    location: NestedCollectionLocation::MessageIndexRegion { chunk_index },
                    length_offset: span.body_start + 2,
                    content_end: span.end,
                    entry_count: serialized_entries.len(),
                    max_string_bytes: 0,
                    retained_bytes: serialized_entries.len() * 16,
                });

                let map_key = if chunk_index == 0
                    && position == 0
                    && self.message_index_fault == MessageIndexFault::MapKeyMismatch
                {
                    self.channels
                        .iter()
                        .find(|channel| channel.id != channel_id)
                        .map_or_else(|| channel_id.wrapping_add(1), |channel| channel.id)
                } else {
                    channel_id
                };
                let record_offset = if chunk_index == 0
                    && position == 0
                    && self.message_index_fault == MessageIndexFault::DescriptorOffsetIntoRecordBody
                {
                    span.body_start as u64
                } else if chunk_index == 0
                    && position == 0
                    && self.message_index_fault == MessageIndexFault::DescriptorOffsetIntoChunk
                {
                    chunk_span.start as u64
                } else {
                    span.start as u64
                };
                index_offsets.insert(map_key, record_offset);

                if chunk_index == 0
                    && position == 0
                    && self.message_index_fault == MessageIndexFault::Duplicate
                {
                    let duplicate = append_record(output, layout, record);
                    index_spans.push(FixtureMessageIndexLayout {
                        record: duplicate,
                        channel_id,
                        entries: serialized_entries.clone(),
                    });
                    nested_patches.push(NestedPatch {
                        target: NestedCollectionTarget::MessageIndexRecords,
                        location: NestedCollectionLocation::MessageIndexRegion { chunk_index },
                        length_offset: duplicate.body_start + 2,
                        content_end: duplicate.end,
                        entry_count: serialized_entries.len(),
                        max_string_bytes: 0,
                        retained_bytes: serialized_entries.len() * 16,
                    });
                }
            }
        }

        if chunk_index == 0
            && self.message_index_fault == MessageIndexFault::DuplicateDescriptorOffset
        {
            let Some((_, &offset)) = index_offsets.first_key_value() else {
                return Err(FixtureBuildError(
                    "duplicate MessageIndex descriptor offset needs an index".to_owned(),
                ));
            };
            let duplicate_key = index_offsets.keys().copied().nth(1).ok_or_else(|| {
                FixtureBuildError(
                    "duplicate MessageIndex descriptor offset needs two Channels".to_owned(),
                )
            })?;
            index_offsets.insert(duplicate_key, offset);
        }

        let index_region_end = output.len();
        let mut message_index_length = (index_region_end - index_region_start) as u64;
        if chunk_index == 0
            && self.message_index_fault == MessageIndexFault::RecordCrossesOwningRegion
            && message_index_length > 0
        {
            message_index_length -= 1;
        }

        let descriptor = ChunkDescriptor {
            message_start_time: index_range.start,
            message_end_time: index_range.end,
            chunk_start_offset: chunk_span.start as u64,
            chunk_length: (chunk_span.end - chunk_span.start) as u64,
            message_index_offsets: index_offsets.clone(),
            message_index_length,
            compression: compression.clone(),
            compressed_size: compressed.len() as u64,
            uncompressed_size: declared_uncompressed_size,
        };
        Ok(BuiltChunk {
            descriptor,
            layout: FixtureChunkLayout {
                record: chunk_span,
                compressed_data_start,
                compressed_data_len: compressed.len(),
                canonical_compressed_data_len,
                uncompressed_record_count: payload.record_count,
                declared_header_range: header_range,
                declared_index_range: index_range,
                message_log_times: payload
                    .messages
                    .iter()
                    .map(|(message, _)| message.log_time)
                    .collect(),
                message_publish_times: payload
                    .messages
                    .iter()
                    .map(|(message, _)| message.publish_time)
                    .collect(),
                uncompressed_message_offsets: payload
                    .messages
                    .iter()
                    .map(|(_, offset)| *offset)
                    .collect(),
                message_index_records: index_spans,
                message_index_offsets: index_offsets,
                message_index_length,
                compression,
                declared_uncompressed_size,
                actual_uncompressed_size: payload.bytes.len() as u64,
                declared_uncompressed_crc,
                actual_uncompressed_crc,
            },
        })
    }

    fn compressed_payload(
        &self,
        chunk_index: usize,
        payload: &ChunkPayload,
    ) -> Result<(String, Vec<u8>, u64), FixtureBuildError> {
        let compression = match self.compression {
            CompressionFixture::None => {
                return Ok((
                    String::new(),
                    payload.bytes.clone(),
                    payload.bytes.len() as u64,
                ));
            }
            CompressionFixture::Lz4 => "lz4",
            _ => "zstd",
        };
        let can_differentiate_upstream = self.definition_fixture == DefinitionFixture::Matching
            && self.nested_collection.is_none()
            && self.extra_records_per_chunk == 0;
        if can_differentiate_upstream {
            let canonical = upstream_write_chunk(
                &self.schemas,
                &self.channels,
                &self.chunks[chunk_index].messages,
                None,
            )?;
            if canonical.bytes != payload.bytes {
                return Err(FixtureBuildError(
                    "upstream writer uncompressed record stream differs from fixture assembler"
                        .to_owned(),
                ));
            }
            let upstream_compression = if compression == "lz4" {
                mcap::Compression::Lz4
            } else {
                mcap::Compression::Zstd
            };
            let compressed = upstream_write_chunk(
                &self.schemas,
                &self.channels,
                &self.chunks[chunk_index].messages,
                Some(upstream_compression),
            )?;
            if compressed.uncompressed_size != payload.bytes.len() as u64 {
                return Err(FixtureBuildError(format!(
                    "upstream writer payload differs from fixture assembler: {} != {}",
                    compressed.uncompressed_size,
                    payload.bytes.len()
                )));
            }
            return Ok((
                compressed.compression,
                compressed.bytes,
                compressed.uncompressed_size,
            ));
        }

        let compressed = if compression == "lz4" {
            let mut encoder = lz4_flex::frame::FrameEncoder::new(Vec::new());
            encoder.write_all(&payload.bytes).map_err(|error| {
                FixtureBuildError(format!("lz4 fixture compression failed: {error}"))
            })?;
            encoder.finish().map_err(|error| {
                FixtureBuildError(format!("lz4 fixture finalization failed: {error}"))
            })?
        } else {
            zstd::bulk::compress(&payload.bytes, 0).map_err(|error| {
                FixtureBuildError(format!("zstd fixture compression failed: {error}"))
            })?
        };
        Ok((
            compression.to_owned(),
            compressed,
            payload.bytes.len() as u64,
        ))
    }

    fn apply_cross_chunk_message_index_fault(
        &self,
        chunks: &mut [BuiltChunk],
    ) -> Result<(), FixtureBuildError> {
        if self.message_index_fault != MessageIndexFault::CrossChunkAlias {
            return Ok(());
        }
        if chunks.len() < 2 {
            return Err(FixtureBuildError(
                "cross-Chunk MessageIndex alias requires at least two Chunks".to_owned(),
            ));
        }
        let Some((&channel_id, _)) = chunks[0].descriptor.message_index_offsets.first_key_value()
        else {
            return Err(FixtureBuildError(
                "cross-Chunk MessageIndex alias requires a non-empty first index".to_owned(),
            ));
        };
        let Some((_, &other_offset)) = chunks[1].descriptor.message_index_offsets.first_key_value()
        else {
            return Err(FixtureBuildError(
                "cross-Chunk MessageIndex alias requires a non-empty second index".to_owned(),
            ));
        };
        chunks[0]
            .descriptor
            .message_index_offsets
            .insert(channel_id, other_offset);
        chunks[0]
            .layout
            .message_index_offsets
            .insert(channel_id, other_offset);
        Ok(())
    }

    fn apply_physical_layout_fault(
        &self,
        descriptors: &mut Vec<ChunkDescriptor>,
        data_end: FixtureRecordSpan,
        summary_start: usize,
        format_violations: &mut BTreeSet<McapFormatViolation>,
    ) -> Result<(), FixtureBuildError> {
        let first = descriptors.first().cloned().ok_or_else(|| {
            FixtureBuildError("physical layout fixture has no ChunkIndex".to_owned())
        })?;
        match self.physical_layout_fault {
            PhysicalLayoutFault::None => {}
            PhysicalLayoutFault::DuplicateChunkIndex => descriptors.push(first),
            PhysicalLayoutFault::ConflictingDuplicateChunkIndex => {
                let mut duplicate = first;
                duplicate.compressed_size = duplicate.compressed_size.wrapping_add(1);
                descriptors.push(duplicate);
            }
            PhysicalLayoutFault::OverlappingChunkRanges => {
                let Some(second) = descriptors.get_mut(1) else {
                    return Err(FixtureBuildError(
                        "overlapping Chunk ranges require at least two Chunks".to_owned(),
                    ));
                };
                second.chunk_start_offset = first
                    .chunk_start_offset
                    .saturating_add(first.chunk_length.saturating_div(2).max(1));
            }
            PhysicalLayoutFault::ChunkOverlapsMessageIndex => {
                descriptors[0].chunk_length = first.chunk_length.saturating_add(1);
            }
            PhysicalLayoutFault::ChunkOverlapsDataEnd => {
                descriptors[0].chunk_start_offset = data_end.start as u64;
                descriptors[0].chunk_length = (data_end.end - data_end.start) as u64;
            }
            PhysicalLayoutFault::ChunkOverlapsSummary => {
                descriptors[0].chunk_start_offset = summary_start as u64;
                descriptors[0].chunk_length = 1;
            }
            PhysicalLayoutFault::MessageIndexOverlapsDataEnd => {
                let region_start = first.chunk_start_offset + first.chunk_length;
                descriptors[0].message_index_length =
                    (data_end.end as u64).saturating_sub(region_start).max(1);
            }
            PhysicalLayoutFault::MessageIndexOverlapsSummary => {
                let region_start = first.chunk_start_offset + first.chunk_length;
                descriptors[0].message_index_length = (summary_start as u64)
                    .saturating_sub(region_start)
                    .saturating_add(1);
            }
        }
        if self.physical_layout_fault == PhysicalLayoutFault::ConflictingDuplicateChunkIndex {
            format_violations.insert(McapFormatViolation::ConflictingChunkIndex);
        }
        Ok(())
    }

    fn encode_statistics(&self) -> (EncodedRecord, usize, StatisticsOracle) {
        let mut channel_message_counts = BTreeMap::<u16, u64>::new();
        let mut all_times = Vec::new();
        for message in self
            .chunks
            .iter()
            .flat_map(|chunk| &chunk.messages)
            .chain(&self.top_level_messages)
        {
            *channel_message_counts
                .entry(message.channel_id)
                .or_default() += 1;
            all_times.push(message.log_time);
        }
        let message_count = channel_message_counts.values().copied().sum::<u64>();
        let (message_start_time, message_end_time) = match (
            all_times.iter().min().copied(),
            all_times.iter().max().copied(),
        ) {
            (Some(start), Some(end)) => (start, end),
            _ => (0, 0),
        };

        let declared_message_count =
            if self.statistics_fixture == StatisticsFixture::WrongMessageCount {
                message_count.wrapping_add(1)
            } else {
                message_count
            };
        let actual_channel_count = self.channels.len() as u32;
        let declared_channel_count =
            if self.statistics_fixture == StatisticsFixture::WrongChannelCount {
                actual_channel_count.wrapping_add(1)
            } else {
                actual_channel_count
            };
        let actual_chunk_count = self.chunks.len() as u32;
        let declared_chunk_count = if self.statistics_fixture == StatisticsFixture::WrongChunkCount
        {
            actual_chunk_count.wrapping_add(1)
        } else {
            actual_chunk_count
        };

        let mut body = Vec::new();
        push_u64(&mut body, declared_message_count);
        push_u16(&mut body, self.schemas.len() as u16);
        push_u32(&mut body, declared_channel_count);
        push_u32(&mut body, 0);
        push_u32(&mut body, 0);
        push_u32(&mut body, declared_chunk_count);
        push_u64(&mut body, message_start_time);
        push_u64(&mut body, message_end_time);
        let nested_offset = body.len();
        push_u32(&mut body, (channel_message_counts.len() * 10) as u32);
        for (channel_id, count) in &channel_message_counts {
            push_u16(&mut body, *channel_id);
            push_u64(&mut body, *count);
        }
        (
            EncodedRecord::new(op::STATISTICS, body),
            nested_offset,
            StatisticsOracle {
                declared_message_count,
                actual_message_count: message_count,
                declared_channel_count,
                actual_channel_count,
                declared_chunk_count,
                actual_chunk_count,
                channel_message_counts,
            },
        )
    }

    fn apply_nested_collection_shape(
        &self,
        bytes: &mut Vec<u8>,
        patches: &[NestedPatch],
    ) -> Result<(), FixtureBuildError> {
        let Some(fixture) = self.nested_collection else {
            return Ok(());
        };
        let location = self
            .nested_collection_location
            .unwrap_or_else(|| NestedCollectionLocation::default_for(fixture.target));
        if matches!(location, NestedCollectionLocation::Chunk { .. }) {
            return Ok(());
        }
        let patch = patches
            .iter()
            .rev()
            .find(|patch| patch.target == fixture.target && patch.location == location)
            .ok_or_else(|| {
                FixtureBuildError(format!(
                    "no generated {:?} collection was available at {location:?}",
                    fixture.target,
                ))
            })?;
        apply_nested_shape_bytes(fixture, bytes, *patch)
    }
}

fn apply_nested_shape_bytes(
    fixture: super::NestedCollectionFixture,
    bytes: &mut Vec<u8>,
    patch: NestedPatch,
) -> Result<(), FixtureBuildError> {
    match fixture.shape {
        NestedCollectionShape::MaximumDensity
        | NestedCollectionShape::TooManyEntries
        | NestedCollectionShape::TooManyRetainedBytes => {}
        NestedCollectionShape::OversizedString => {
            if fixture.target != NestedCollectionTarget::ChannelMetadata {
                return Err(FixtureBuildError(
                    "only Channel.metadata contains nested strings".to_owned(),
                ));
            }
        }
        NestedCollectionShape::WrongByteLength => {
            let end = patch.length_offset + 4;
            let raw: [u8; 4] = bytes
                .get(patch.length_offset..end)
                .ok_or_else(|| FixtureBuildError("nested length is outside bytes".to_owned()))?
                .try_into()
                .expect("four-byte slice");
            let wrong = u32::from_le_bytes(raw).wrapping_add(1);
            bytes[patch.length_offset..end].copy_from_slice(&wrong.to_le_bytes());
        }
        NestedCollectionShape::PrematureEof => {
            let content_start = patch.length_offset + 4;
            let truncate_at = content_start + (patch.content_end - content_start) / 2;
            bytes.truncate(truncate_at);
        }
    }
    Ok(())
}

fn channel_metadata_length_offset(span: FixtureRecordSpan, channel: &FixtureChannel) -> usize {
    span.body_start + 2 + 2 + 4 + channel.topic.len() + 4 + channel.message_encoding.len()
}

fn encode_chunk_index(descriptor: &ChunkDescriptor) -> (EncodedRecord, usize) {
    let mut body = Vec::new();
    push_u64(&mut body, descriptor.message_start_time);
    push_u64(&mut body, descriptor.message_end_time);
    push_u64(&mut body, descriptor.chunk_start_offset);
    push_u64(&mut body, descriptor.chunk_length);
    let nested_offset = body.len();
    push_u32(
        &mut body,
        (descriptor.message_index_offsets.len() * 10) as u32,
    );
    for (channel_id, offset) in &descriptor.message_index_offsets {
        push_u16(&mut body, *channel_id);
        push_u64(&mut body, *offset);
    }
    push_u64(&mut body, descriptor.message_index_length);
    push_string(&mut body, &descriptor.compression);
    push_u64(&mut body, descriptor.compressed_size);
    push_u64(&mut body, descriptor.uncompressed_size);
    (EncodedRecord::new(op::CHUNK_INDEX, body), nested_offset)
}

fn rewrite_summary_crc(
    bytes: &mut [u8],
    layout: &FixtureLayout,
    mode: FixtureCrc,
) -> Result<(), FixtureBuildError> {
    let footer = layout
        .footer
        .ok_or_else(|| FixtureBuildError("fixture has no Footer".to_owned()))?;
    let crc_offset = footer.body_start + 16;
    if crc_offset + 4 > bytes.len() || footer.body_start + 16 > bytes.len() {
        return Err(FixtureBuildError(
            "fixture Footer CRC is outside generated bytes".to_owned(),
        ));
    }
    let covered = &bytes[layout.summary_start..footer.body_start + 16];
    let crc = crc_field(mode, covered);
    if mode == FixtureCrc::ValidNonZero && crc == 0 {
        return Err(FixtureBuildError(
            "generated Summary has a zero CRC and cannot satisfy ValidNonZero".to_owned(),
        ));
    }
    bytes[crc_offset..crc_offset + 4].copy_from_slice(&crc.to_le_bytes());
    Ok(())
}

struct UpstreamCompressedChunk {
    compression: String,
    bytes: Vec<u8>,
    uncompressed_size: u64,
}

fn upstream_write_chunk(
    schemas: &[FixtureSchema],
    channels: &[FixtureChannel],
    messages: &[FixtureMessage],
    compression: Option<mcap::Compression>,
) -> Result<UpstreamCompressedChunk, FixtureBuildError> {
    let cursor = Cursor::new(Vec::new());
    let options = mcap::WriteOptions::new()
        .compression(compression)
        .chunk_size(None)
        .calculate_chunk_crcs(true);
    let mut writer = mcap::Writer::with_options(cursor, options)
        .map_err(|error| FixtureBuildError(format!("upstream writer creation failed: {error}")))?;
    for schema in schemas {
        writer
            .add_schema_with_id(schema.id, &schema.name, &schema.encoding, &schema.data)
            .map_err(|error| FixtureBuildError(format!("upstream Schema write failed: {error}")))?;
    }
    for channel in channels {
        writer
            .add_channel_with_id(
                channel.id,
                channel.schema_id,
                &channel.topic,
                &channel.message_encoding,
                &channel.metadata,
            )
            .map_err(|error| {
                FixtureBuildError(format!("upstream Channel write failed: {error}"))
            })?;
    }
    for message in messages {
        writer
            .write_to_known_channel(
                &mcap::records::MessageHeader {
                    channel_id: message.channel_id,
                    sequence: message.sequence,
                    log_time: message.log_time,
                    publish_time: message.publish_time,
                },
                &message.data,
            )
            .map_err(|error| {
                FixtureBuildError(format!("upstream Message write failed: {error}"))
            })?;
    }
    let summary = writer
        .finish()
        .map_err(|error| FixtureBuildError(format!("upstream writer finish failed: {error}")))?;
    let bytes = writer.into_inner().into_inner();
    let descriptor = summary.chunk_indexes.first().ok_or_else(|| {
        FixtureBuildError("upstream writer produced no compressed Chunk".to_owned())
    })?;
    let start = usize::try_from(
        descriptor
            .compressed_data_offset()
            .map_err(|error| FixtureBuildError(format!("bad compressed offset: {error}")))?,
    )
    .map_err(|_error| FixtureBuildError("compressed offset does not fit usize".to_owned()))?;
    let len = usize::try_from(descriptor.compressed_size)
        .map_err(|_error| FixtureBuildError("compressed size does not fit usize".to_owned()))?;
    let compressed = bytes
        .get(start..start + len)
        .ok_or_else(|| FixtureBuildError("upstream compressed payload is truncated".to_owned()))?
        .to_vec();
    Ok(UpstreamCompressedChunk {
        compression: descriptor.compression.clone(),
        bytes: compressed,
        uncompressed_size: descriptor.uncompressed_size,
    })
}
