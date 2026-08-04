use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;

const RECORD_HEADER_LEN: usize = 9;

/// An inclusive raw MCAP time range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawTimeRange {
    pub start: u64,
    pub end: u64,
}

impl RawTimeRange {
    pub const fn new(start: u64, end: u64) -> Self {
        Self { start, end }
    }
}

/// A schema definition used by an adversarial fixture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixtureSchema {
    pub id: u16,
    pub name: String,
    pub encoding: String,
    pub data: Vec<u8>,
}

impl FixtureSchema {
    pub fn new(id: u16, name: impl Into<String>, encoding: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            encoding: encoding.into(),
            data: Vec::new(),
        }
    }

    pub fn with_data(mut self, data: impl Into<Vec<u8>>) -> Self {
        self.data = data.into();
        self
    }
}

/// A channel definition used by an adversarial fixture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixtureChannel {
    pub id: u16,
    pub schema_id: u16,
    pub topic: String,
    pub message_encoding: String,
    pub metadata: BTreeMap<String, String>,
}

impl FixtureChannel {
    pub fn schema_less(id: u16, topic: impl Into<String>) -> Self {
        Self {
            id,
            schema_id: 0,
            topic: topic.into(),
            message_encoding: "raw".to_owned(),
            metadata: BTreeMap::new(),
        }
    }

    pub fn with_schema(mut self, schema_id: u16, encoding: impl Into<String>) -> Self {
        self.schema_id = schema_id;
        self.message_encoding = encoding.into();
        self
    }

    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }
}

/// A raw message used by an adversarial fixture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixtureMessage {
    pub channel_id: u16,
    pub sequence: u32,
    pub log_time: u64,
    pub publish_time: u64,
    pub data: Vec<u8>,
}

impl FixtureMessage {
    pub fn new(channel_id: u16, sequence: u32, log_time: u64) -> Self {
        Self {
            channel_id,
            sequence,
            log_time,
            publish_time: log_time,
            data: vec![sequence as u8],
        }
    }

    pub fn with_publish_time(mut self, publish_time: u64) -> Self {
        self.publish_time = publish_time;
        self
    }

    pub fn with_data(mut self, data: impl Into<Vec<u8>>) -> Self {
        self.data = data.into();
        self
    }
}

/// One physical chunk and the independently controlled ranges declared by its header and index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixtureChunk {
    pub messages: Vec<FixtureMessage>,
    pub header_range: Option<RawTimeRange>,
    pub index_range: Option<RawTimeRange>,
}

impl FixtureChunk {
    pub fn new(messages: impl IntoIterator<Item = FixtureMessage>) -> Self {
        Self {
            messages: messages.into_iter().collect(),
            header_range: None,
            index_range: None,
        }
    }

    pub fn empty() -> Self {
        Self::new([])
    }

    pub fn single(message: FixtureMessage) -> Self {
        Self::new([message])
    }

    pub fn with_header_range(mut self, range: RawTimeRange) -> Self {
        self.header_range = Some(range);
        self
    }

    pub fn with_index_range(mut self, range: RawTimeRange) -> Self {
        self.index_range = Some(range);
        self
    }

    fn actual_range(&self) -> RawTimeRange {
        let mut times = self.messages.iter().map(|message| message.log_time);
        let Some(first) = times.next() else {
            return RawTimeRange::new(0, 0);
        };
        times.fold(RawTimeRange::new(first, first), |range, time| {
            RawTimeRange::new(range.start.min(time), range.end.max(time))
        })
    }
}

/// CRC field behavior for independently controlled chunk and summary checksums.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FixtureCrc {
    Zero,
    #[default]
    ValidNonZero,
    InvalidNonZero,
}

/// A single `MessageIndex` or owning-region defect.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MessageIndexFault {
    #[default]
    None,
    Missing,
    Duplicate,
    EntryOffset,
    EntryTime,
    WrongOpcode,
    MapKeyMismatch,
    RecordCrossesOwningRegion,
    DescriptorOffsetIntoRecordBody,
    DescriptorOffsetIntoChunk,
    DuplicateDescriptorOffset,
    CrossChunkAlias,
}

/// A definition relationship between Summary records and definitions inside a Chunk.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DefinitionFixture {
    #[default]
    Matching,
    ExactDuplicate,
    UnknownUnreferenced,
    UnknownReferenced,
    MissingReferencedSchema,
    SchemaNameConflict,
    SchemaEncodingConflict,
    SchemaDataConflict,
    ChannelSchemaConflict,
    ChannelTopicConflict,
    ChannelEncodingConflict,
    ChannelMetadataConflict,
    UnselectedChannelConflict,
    ChunkTailConflict,
}

/// Physical descriptor mutations that do not require overlapping the actual backing vector.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PhysicalLayoutFault {
    #[default]
    None,
    DuplicateChunkIndex,
    ConflictingDuplicateChunkIndex,
    OverlappingChunkRanges,
    ChunkOverlapsMessageIndex,
    ChunkOverlapsDataEnd,
    ChunkOverlapsSummary,
    MessageIndexOverlapsDataEnd,
    MessageIndexOverlapsSummary,
}

/// Summary Statistics behavior.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StatisticsFixture {
    #[default]
    Exact,
    WrongMessageCount,
    WrongChunkCount,
    WrongChannelCount,
}

/// The nested collection whose byte length, count, or retention pressure is under test.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum NestedCollectionTarget {
    ChannelMetadata,
    ChunkIndexMessageIndexOffsets,
    StatisticsChannelMessageCounts,
    MessageIndexRecords,
}

/// A resource profile local to one nested collection fixture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NestedCollectionLimits {
    pub max_entries: usize,
    pub max_string_bytes: usize,
    pub max_retained_bytes: usize,
}

impl Default for NestedCollectionLimits {
    fn default() -> Self {
        Self {
            max_entries: 4,
            max_string_bytes: 16,
            max_retained_bytes: 64,
        }
    }
}

/// A boundary or malformed shape for one nested MCAP collection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NestedCollectionShape {
    MaximumDensity,
    TooManyEntries,
    OversizedString,
    TooManyRetainedBytes,
    WrongByteLength,
    PrematureEof,
}

/// Controls one nested collection independently from the file-level record counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NestedCollectionFixture {
    pub target: NestedCollectionTarget,
    pub shape: NestedCollectionShape,
    pub limits: NestedCollectionLimits,
}

/// The physical record stream that owns a nested collection mutation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NestedCollectionLocation {
    Summary,
    Chunk { chunk_index: usize },
    MessageIndexRegion { chunk_index: usize },
}

impl NestedCollectionLocation {
    fn default_for(target: NestedCollectionTarget) -> Self {
        if target == NestedCollectionTarget::MessageIndexRecords {
            Self::MessageIndexRegion { chunk_index: 0 }
        } else {
            Self::Summary
        }
    }
}

/// Static compressed-stream shapes plus codec-only scheduling states used by later decoder tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CompressionFixture {
    #[default]
    None,
    Zstd,
    Lz4,
    DeclaredOutputTooSmall,
    DeclaredOutputTooLarge,
    ConcatenatedFrame,
    TrailingPayload,
    TruncatedInput,
    OversizedDeclaration,
    OutputFullBeforeCodecEof,
    ZeroProgress,
}

/// The interpretation frozen by an HTTP MCAP open request for the same raw time values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum McapTimeTypeFixture {
    #[default]
    TimestampNs,
    DurationNs,
}

/// Cardinalities that are consumed independently by parser, dispatcher, and partition tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixtureCardinality {
    pub summary_records: usize,
    pub schemas: usize,
    pub channels: usize,
    pub chunk_indexes: usize,
    pub records_per_chunk: usize,
    pub messages_per_chunk: usize,
    pub selected_dispatches_per_scan: usize,
}

impl Default for FixtureCardinality {
    fn default() -> Self {
        Self {
            summary_records: 3,
            schemas: 0,
            channels: 1,
            chunk_indexes: 1,
            records_per_chunk: 2,
            messages_per_chunk: 1,
            selected_dispatches_per_scan: 1,
        }
    }
}

/// Which decoders recognize a channel in a later assignment-policy test.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FixtureDecoder {
    SemanticRos,
    RosReflection,
    Raw,
}

/// The exact serialized Channel/Schema fields presented to fake recognition candidates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecoderRecognitionInput {
    pub channel_id: u16,
    pub schema_id: u16,
    pub schema_name: Option<String>,
    pub schema_encoding: Option<String>,
    pub message_encoding: String,
}

/// The output shape returned by a fake temporal parser.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TemporalOutputFixture {
    #[default]
    Temporal,
    MissingCanonicalLogTimeline,
    WrongTimelineType,
    MessageDerivedStatic,
}

/// Assignment data intentionally kept separate from the serialized MCAP bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecoderAssignmentFixture {
    pub recognition_input: DecoderRecognitionInput,
    pub recognized_by: BTreeSet<FixtureDecoder>,
    pub identical_duplicate_groups: usize,
    pub singleton_multi_group_overlap: bool,
    pub cross_owner_reference: bool,
    pub allowlist_serialization: Vec<FixtureDecoder>,
    pub temporal_output: TemporalOutputFixture,
}

impl Default for DecoderAssignmentFixture {
    fn default() -> Self {
        Self {
            recognition_input: DecoderRecognitionInput {
                channel_id: 1,
                schema_id: 0,
                schema_name: None,
                schema_encoding: None,
                message_encoding: "raw".to_owned(),
            },
            recognized_by: std::iter::once(FixtureDecoder::Raw).collect(),
            identical_duplicate_groups: 0,
            singleton_multi_group_overlap: false,
            cross_owner_reference: false,
            allowlist_serialization: vec![
                FixtureDecoder::SemanticRos,
                FixtureDecoder::RosReflection,
                FixtureDecoder::Raw,
            ],
            temporal_output: TemporalOutputFixture::Temporal,
        }
    }
}

/// Partition/root pressure parameters consumed by later manifest and registration tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartitionFixture {
    pub max_roots_per_partition: usize,
    pub max_external_origin_bytes_per_partition: usize,
    pub session_root_cap: usize,
    pub selected_group_roots: Vec<usize>,
    pub opening_static_roots: usize,
    pub external_origin_bytes_per_partition: Vec<usize>,
    pub cursor_time: u64,
    pub cursor_covering_chunks: usize,
    pub selected_channel_groups: usize,
    pub channels_per_selected_group: usize,
    pub selected_group_members: Vec<Vec<u16>>,
    pub generations: Vec<PartitionGenerationFixture>,
    pub resident_roots: usize,
    pub rows_per_root: usize,
    pub unsorted_timeline: bool,
}

impl Default for PartitionFixture {
    fn default() -> Self {
        Self {
            max_roots_per_partition: 4,
            max_external_origin_bytes_per_partition: 64,
            session_root_cap: 8,
            selected_group_roots: vec![1],
            opening_static_roots: 1,
            external_origin_bytes_per_partition: vec![0],
            cursor_time: 1,
            cursor_covering_chunks: 1,
            selected_channel_groups: 1,
            channels_per_selected_group: 1,
            selected_group_members: vec![vec![1]],
            generations: vec![PartitionGenerationFixture::Roots {
                root_descriptors: 1,
                external_origin_bytes: 0,
            }],
            resident_roots: 1,
            rows_per_root: 1,
            unsorted_timeline: false,
        }
    }
}

/// One non-overlapping window generation in a partition fixture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartitionGenerationFixture {
    CompleteEmpty,
    Roots {
        root_descriptors: usize,
        external_origin_bytes: usize,
    },
}

/// A defect in the serialized MCAP representation itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum McapFormatViolation {
    ChunkDeclaredTimeRange,
    MessageIndexDuplicate,
    MessageIndexEntryOffset,
    MessageIndexEntryTime,
    MessageIndexOpcode,
    MessageIndexMapKey,
    MessageIndexOwningRegion,
    MessageIndexDescriptorOffset,
    MessageIndexDuplicateDescriptorOffset,
    MessageIndexCrossChunkAlias,
    StatisticsCount,
    SummaryCrc,
    ChunkCrc,
    MissingReferencedSchema,
    UnknownReferencedChannel,
    ConflictingSchema,
    ConflictingChannel,
    ConflictingChunkIndex,
    PhysicalRegionOverlap,
    NestedByteLength,
    PrematureEof,
    CompressedOutputSize,
    CompressedTrailingPayload,
    CompressedInputTruncated,
}

/// A remote-opening policy observation or rejection that does not define MCAP syntax validity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RemotePolicyOutcome {
    InvalidTemporalValue,
    MessageIndexAbsent,
    NestedCollectionCount,
    NestedStringLength,
    NestedRetainedBytes,
    CompressedDeclarationLimit,
    MultipleCompressionFrames,
    PartitionRootLimit,
    PartitionExternalOriginLimit,
    SessionRootCap,
}

/// An expected result from a separately injected fake decoder or scheduler component.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FakeComponentOutcome {
    CodecDidNotReachEof,
    CodecZeroProgress,
    DecoderGroupOverlap,
    DecoderCrossOwnerReference,
    DecoderTemporalOutput,
}

/// Byte span of a generated record, including its opcode and length prefix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixtureRecordSpan {
    pub opcode: u8,
    pub start: usize,
    pub body_start: usize,
    pub body_len: usize,
    pub end: usize,
}

/// Stable assembler diagnostics for locating generated records in targeted tests.
///
/// This layout is not a validation oracle; `validate_self` re-parses `bytes` independently.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FixtureLayout {
    pub records: Vec<FixtureRecordSpan>,
    pub chunks: Vec<FixtureChunkLayout>,
    pub chunk_indexes: Vec<FixtureChunkIndexLayout>,
    pub statistics: Option<FixtureStatisticsLayout>,
    pub nested_collections: Vec<FixtureNestedCollectionLayout>,
    pub data_end: Option<FixtureRecordSpan>,
    pub summary_start: usize,
    pub footer: Option<FixtureRecordSpan>,
}

/// Stable physical and uncompressed offsets for one generated chunk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixtureChunkLayout {
    pub record: FixtureRecordSpan,
    pub compressed_data_start: usize,
    pub compressed_data_len: usize,
    pub canonical_compressed_data_len: usize,
    pub uncompressed_record_count: usize,
    pub declared_header_range: RawTimeRange,
    pub declared_index_range: RawTimeRange,
    pub message_log_times: Vec<u64>,
    pub message_publish_times: Vec<u64>,
    pub uncompressed_message_offsets: Vec<u64>,
    pub message_index_records: Vec<FixtureMessageIndexLayout>,
    pub message_index_offsets: BTreeMap<u16, u64>,
    pub message_index_length: u64,
    pub compression: String,
    pub declared_uncompressed_size: u64,
    pub actual_uncompressed_size: u64,
    pub declared_uncompressed_crc: u32,
    pub actual_uncompressed_crc: u32,
}

/// One generated `MessageIndex` record and the entries actually serialized in its body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixtureMessageIndexLayout {
    pub record: FixtureRecordSpan,
    pub channel_id: u16,
    pub entries: Vec<FixtureMessageIndexEntry>,
}

/// One entry actually serialized into a generated `MessageIndex`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixtureMessageIndexEntry {
    pub log_time: u64,
    pub offset: u64,
}

/// A canonical projection of one serialized `ChunkIndex` descriptor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixtureChunkIndexLayout {
    pub message_range: RawTimeRange,
    pub chunk_start_offset: u64,
    pub chunk_length: u64,
    pub message_index_offsets: BTreeMap<u16, u64>,
    pub message_index_length: u64,
    pub compression: String,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
}

/// Declared Statistics fields and their assembler-derived values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixtureStatisticsLayout {
    pub record: FixtureRecordSpan,
    pub declared_message_count: u64,
    pub actual_message_count: u64,
    pub declared_channel_count: u32,
    pub actual_channel_count: u32,
    pub declared_chunk_count: u32,
    pub actual_chunk_count: u32,
    pub channel_message_counts: BTreeMap<u16, u64>,
}

/// Location and resource facts for one serialized nested collection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixtureNestedCollectionLayout {
    pub target: NestedCollectionTarget,
    pub length_offset: usize,
    pub content_end: usize,
    pub declared_encoded_bytes: u32,
    pub actual_encoded_bytes: usize,
    pub entry_count: usize,
    pub max_string_bytes: usize,
    pub retained_bytes: usize,
}

/// A complete fixture plus its byte-independent decoder and partition inputs.
#[derive(Clone, Debug)]
pub struct AdversarialMcapFixture {
    pub bytes: Vec<u8>,
    pub layout: FixtureLayout,
    pub mcap_format_violations: BTreeSet<McapFormatViolation>,
    pub remote_expected_outcomes: BTreeSet<RemotePolicyOutcome>,
    pub fake_component_outcomes: BTreeSet<FakeComponentOutcome>,
    pub cardinality: FixtureCardinality,
    pub time_type: McapTimeTypeFixture,
    pub nested_collection: Option<NestedCollectionFixture>,
    pub nested_collection_location: Option<NestedCollectionLocation>,
    pub compression: CompressionFixture,
    pub chunk_crc: FixtureCrc,
    pub summary_crc: FixtureCrc,
    pub summary_offsets: bool,
    pub message_index_fault: MessageIndexFault,
    pub statistics_fixture: StatisticsFixture,
    pub definition_fixture: DefinitionFixture,
    pub physical_layout_fault: PhysicalLayoutFault,
    pub decoder: DecoderAssignmentFixture,
    pub partition: PartitionFixture,
}

impl AdversarialMcapFixture {
    pub fn is_mcap_format_valid(&self) -> bool {
        self.mcap_format_violations.is_empty()
    }

    pub fn has_only_format_violation(&self, violation: McapFormatViolation) -> bool {
        self.mcap_format_violations.len() == 1 && self.mcap_format_violations.contains(&violation)
    }

    pub fn has_only_remote_outcome(&self, outcome: RemotePolicyOutcome) -> bool {
        self.remote_expected_outcomes.len() == 1 && self.remote_expected_outcomes.contains(&outcome)
    }

    pub fn has_only_fake_component_outcome(&self, outcome: FakeComponentOutcome) -> bool {
        self.fake_component_outcomes.len() == 1 && self.fake_component_outcomes.contains(&outcome)
    }

    /// Re-inspects serialized bytes with an implementation independent from the assembler.
    pub fn validate_self(&self) -> Result<(), String> {
        reference::validate_fixture(self).map_err(|error| error.0)
    }

    /// Runs the upstream `SummaryReader` used by `re_mcap::read_summary` against this fixture.
    pub fn read_upstream_summary(&self) -> Result<Option<mcap::Summary>, FixtureBuildError> {
        crate::read_summary(Cursor::new(&self.bytes))
            .map_err(|error| FixtureBuildError(format!("upstream SummaryReader failed: {error}")))
    }

    /// Runs upstream's indexed reader and records both selected Chunk payloads and messages.
    pub fn read_upstream_indexed(
        &self,
        start: Option<u64>,
        end: Option<u64>,
    ) -> Result<UpstreamIndexedSelection, FixtureBuildError> {
        let summary = self
            .read_upstream_summary()?
            .ok_or_else(|| FixtureBuildError("fixture has no upstream Summary".to_owned()))?;
        let mut options = mcap::sans_io::IndexedReaderOptions::new();
        options.start = start;
        options.end = end;
        options.order = mcap::sans_io::indexed_reader::ReadOrder::LogTime;
        let mut reader = mcap::sans_io::IndexedReader::new_with_options(&summary, options)
            .map_err(|error| {
                FixtureBuildError(format!("upstream indexed setup failed: {error}"))
            })?;
        let mut selection = UpstreamIndexedSelection::default();
        loop {
            let Some(event) = reader.next_event() else {
                break;
            };
            match event.map_err(|error| {
                FixtureBuildError(format!("upstream indexed read failed: {error}"))
            })? {
                mcap::sans_io::IndexedReadEvent::ReadChunkRequest { offset, length } => {
                    let start = usize::try_from(offset).map_err(|_error| {
                        FixtureBuildError("upstream read offset does not fit usize".to_owned())
                    })?;
                    let chunk = self.bytes.get(start..start + length).ok_or_else(|| {
                        FixtureBuildError("upstream Chunk request is outside fixture".to_owned())
                    })?;
                    selection.chunk_payload_offsets.push(offset);
                    reader
                        .insert_chunk_record_data(offset, chunk)
                        .map_err(|error| {
                            FixtureBuildError(format!("upstream Chunk insertion failed: {error}"))
                        })?;
                }
                mcap::sans_io::IndexedReadEvent::Message { header, data } => {
                    selection.messages.push(UpstreamIndexedMessage {
                        channel_id: header.channel_id,
                        sequence: header.sequence,
                        log_time: header.log_time,
                        publish_time: header.publish_time,
                        data: data.to_vec(),
                    });
                }
            }
        }
        Ok(selection)
    }

    /// Compares every upstream Summary/indexed field against an independent byte inspection.
    pub fn validate_upstream_differential(&self) -> Result<(), FixtureBuildError> {
        reference::validate_upstream_differential(self)
    }
}

/// Observable result of driving upstream's sans-I/O indexed reader.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UpstreamIndexedSelection {
    pub chunk_payload_offsets: Vec<u64>,
    pub messages: Vec<UpstreamIndexedMessage>,
}

/// Owned message projection emitted by upstream's indexed reader.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpstreamIndexedMessage {
    pub channel_id: u16,
    pub sequence: u32,
    pub log_time: u64,
    pub publish_time: u64,
    pub data: Vec<u8>,
}

/// Composable builder for valid boundary files and narrowly malformed MCAP files.
#[derive(Clone, Debug)]
pub struct AdversarialMcapFixtureBuilder {
    schemas: Vec<FixtureSchema>,
    channels: Vec<FixtureChannel>,
    chunks: Vec<FixtureChunk>,
    top_level_messages: Vec<FixtureMessage>,
    chunk_crc: FixtureCrc,
    summary_crc: FixtureCrc,
    summary_offsets: bool,
    summary_offsets_override: bool,
    message_index_fault: MessageIndexFault,
    definition_fixture: DefinitionFixture,
    physical_layout_fault: PhysicalLayoutFault,
    trailing_canonical_channel_after_conflict: bool,
    statistics_fixture: StatisticsFixture,
    nested_collection: Option<NestedCollectionFixture>,
    nested_collection_location: Option<NestedCollectionLocation>,
    compression: CompressionFixture,
    cardinality: FixtureCardinality,
    cardinality_override: bool,
    extra_records_per_chunk: usize,
    extra_summary_records: usize,
    time_type: McapTimeTypeFixture,
    decoder: DecoderAssignmentFixture,
    decoder_override: bool,
    partition: PartitionFixture,
    partition_override: bool,
}

impl Default for AdversarialMcapFixtureBuilder {
    fn default() -> Self {
        Self {
            schemas: Vec::new(),
            channels: vec![FixtureChannel::schema_less(1, "/fixture")],
            chunks: vec![FixtureChunk::single(FixtureMessage::new(1, 0, 1))],
            top_level_messages: Vec::new(),
            chunk_crc: FixtureCrc::ValidNonZero,
            summary_crc: FixtureCrc::ValidNonZero,
            summary_offsets: true,
            summary_offsets_override: false,
            message_index_fault: MessageIndexFault::None,
            definition_fixture: DefinitionFixture::Matching,
            physical_layout_fault: PhysicalLayoutFault::None,
            trailing_canonical_channel_after_conflict: false,
            statistics_fixture: StatisticsFixture::Exact,
            nested_collection: None,
            nested_collection_location: None,
            compression: CompressionFixture::None,
            cardinality: FixtureCardinality::default(),
            cardinality_override: false,
            extra_records_per_chunk: 0,
            extra_summary_records: 0,
            time_type: McapTimeTypeFixture::TimestampNs,
            decoder: DecoderAssignmentFixture::default(),
            decoder_override: false,
            partition: PartitionFixture::default(),
            partition_override: false,
        }
    }
}

impl AdversarialMcapFixtureBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_schemas(mut self, schemas: impl IntoIterator<Item = FixtureSchema>) -> Self {
        self.schemas = schemas.into_iter().collect();
        self
    }

    pub fn with_channels(mut self, channels: impl IntoIterator<Item = FixtureChannel>) -> Self {
        self.channels = channels.into_iter().collect();
        self
    }

    pub fn with_chunks(mut self, chunks: impl IntoIterator<Item = FixtureChunk>) -> Self {
        self.chunks = chunks.into_iter().collect();
        self
    }

    pub fn with_top_level_messages(
        mut self,
        messages: impl IntoIterator<Item = FixtureMessage>,
    ) -> Self {
        self.top_level_messages = messages.into_iter().collect();
        self
    }

    pub fn with_chunk_crc(mut self, crc: FixtureCrc) -> Self {
        self.chunk_crc = crc;
        self
    }

    pub fn with_summary_crc(mut self, crc: FixtureCrc) -> Self {
        self.summary_crc = crc;
        self
    }

    pub fn with_summary_offsets(mut self, enabled: bool) -> Self {
        self.summary_offsets = enabled;
        self.summary_offsets_override = true;
        self
    }

    pub fn with_message_index_fault(mut self, fault: MessageIndexFault) -> Self {
        self.message_index_fault = fault;
        self
    }

    pub fn with_definition_fixture(mut self, fixture: DefinitionFixture) -> Self {
        self.definition_fixture = fixture;
        self
    }

    pub fn with_physical_layout_fault(mut self, fault: PhysicalLayoutFault) -> Self {
        self.physical_layout_fault = fault;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_trailing_canonical_channel_after_conflict(mut self) -> Self {
        self.trailing_canonical_channel_after_conflict = true;
        self
    }

    pub fn with_statistics_fixture(mut self, fixture: StatisticsFixture) -> Self {
        self.statistics_fixture = fixture;
        self
    }

    pub fn with_nested_collection(mut self, fixture: NestedCollectionFixture) -> Self {
        self.nested_collection_location =
            Some(NestedCollectionLocation::default_for(fixture.target));
        self.nested_collection = Some(fixture);
        self
    }

    pub fn with_nested_collection_at(
        mut self,
        fixture: NestedCollectionFixture,
        location: NestedCollectionLocation,
    ) -> Self {
        self.nested_collection = Some(fixture);
        self.nested_collection_location = Some(location);
        self
    }

    pub fn with_compression(mut self, compression: CompressionFixture) -> Self {
        self.compression = compression;
        self
    }

    pub fn with_cardinality(mut self, cardinality: FixtureCardinality) -> Self {
        self.cardinality = cardinality;
        self.cardinality_override = true;
        self
    }

    pub fn with_time_type(mut self, time_type: McapTimeTypeFixture) -> Self {
        self.time_type = time_type;
        self
    }

    pub fn with_decoder_fixture(mut self, fixture: DecoderAssignmentFixture) -> Self {
        self.decoder = fixture;
        self.decoder_override = true;
        self
    }

    pub fn with_partition_fixture(mut self, fixture: PartitionFixture) -> Self {
        self.partition = fixture;
        self.partition_override = true;
        self
    }
}

mod assembler;
mod reference;

pub use assembler::FixtureBuildError;
pub use reference::*;

#[cfg(test)]
#[path = "adversarial_fixture_tests.rs"]
mod tests;
