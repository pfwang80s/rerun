use super::*;
use mcap::records::op;

fn build(builder: AdversarialMcapFixtureBuilder) -> AdversarialMcapFixture {
    builder.build().expect("fixture should build")
}

fn assert_build_error(builder: AdversarialMcapFixtureBuilder, expected: &str) {
    let error = builder
        .build()
        .expect_err("fixture controls should conflict");
    assert!(
        error.0.contains(expected),
        "expected {expected:?} in {error:?}"
    );
}

#[test]
fn baseline_differentiates_summary_and_indexed_selection_against_upstream() {
    let schema = FixtureSchema::new(7, "fixture.Point", "protobuf").with_data([0x0a, 0x00]);
    let channel = FixtureChannel::schema_less(1, "/fixture/points")
        .with_schema(7, "protobuf")
        .with_metadata("frame", "map");
    let chunks = [
        FixtureChunk::single(FixtureMessage::new(1, 0, 20)),
        FixtureChunk::single(FixtureMessage::new(1, 1, 10)),
        FixtureChunk::single(FixtureMessage::new(1, 2, 20)),
    ];
    let fixture = build(
        AdversarialMcapFixtureBuilder::new()
            .with_schemas([schema.clone()])
            .with_channels([channel.clone()])
            .with_chunks(chunks),
    );
    assert!(fixture.is_mcap_format_valid());

    let summary = fixture
        .read_upstream_summary()
        .expect("summary read")
        .expect("summary present");
    let upstream_schema = summary.schemas.get(&schema.id).expect("Schema 7");
    assert_eq!(upstream_schema.id, schema.id);
    assert_eq!(upstream_schema.name, schema.name);
    assert_eq!(upstream_schema.encoding, schema.encoding);
    assert_eq!(upstream_schema.data.as_ref(), schema.data);
    let upstream_channel = summary.channels.get(&channel.id).expect("Channel 1");
    assert_eq!(upstream_channel.id, channel.id);
    assert_eq!(upstream_channel.topic, channel.topic);
    assert_eq!(upstream_channel.message_encoding, channel.message_encoding);
    assert_eq!(upstream_channel.metadata, channel.metadata);
    assert_eq!(
        upstream_channel.schema.as_ref().expect("Channel Schema").id,
        channel.schema_id
    );
    assert_eq!(summary.channels.len(), 1);
    assert_eq!(summary.schemas.len(), 1);
    assert_eq!(summary.chunk_indexes.len(), 3);
    let statistics = summary.stats.as_ref().expect("Statistics");
    assert_eq!(statistics.message_count, 3);
    assert_eq!(statistics.schema_count, 1);
    assert_eq!(statistics.channel_count, 1);
    assert_eq!(statistics.attachment_count, 0);
    assert_eq!(statistics.metadata_count, 0);
    assert_eq!(statistics.chunk_count, 3);
    assert_eq!(statistics.message_start_time, 10);
    assert_eq!(statistics.message_end_time, 20);
    assert_eq!(statistics.channel_message_counts, [(1, 3)].into());
    for (upstream, expected) in summary
        .chunk_indexes
        .iter()
        .zip(&fixture.layout.chunk_indexes)
    {
        assert_eq!(upstream.message_start_time, expected.message_range.start);
        assert_eq!(upstream.message_end_time, expected.message_range.end);
        assert_eq!(upstream.chunk_start_offset, expected.chunk_start_offset);
        assert_eq!(upstream.chunk_length, expected.chunk_length);
        assert_eq!(
            upstream.message_index_offsets,
            expected.message_index_offsets
        );
        assert_eq!(upstream.message_index_length, expected.message_index_length);
        assert_eq!(upstream.compression, expected.compression);
        assert_eq!(upstream.compressed_size, expected.compressed_size);
        assert_eq!(upstream.uncompressed_size, expected.uncompressed_size);
    }
    assert!(summary.attachment_indexes.is_empty());
    assert!(summary.metadata_indexes.is_empty());

    let selection = fixture
        .read_upstream_indexed(Some(15), Some(21))
        .expect("indexed selection");
    let expected_payloads = vec![
        fixture.layout.chunks[0].compressed_data_start as u64,
        fixture.layout.chunks[2].compressed_data_start as u64,
    ];
    assert_eq!(selection.chunk_payload_offsets, expected_payloads);
    assert_eq!(
        selection
            .messages
            .iter()
            .map(|message| (
                message.channel_id,
                message.sequence,
                message.log_time,
                message.publish_time,
                message.data.clone(),
            ))
            .collect::<Vec<_>>(),
        vec![(1, 0, 20, 20, vec![0]), (1, 2, 20, 20, vec![2])]
    );
}

#[test]
fn legal_crc_and_summary_offset_modes_are_accepted_upstream() {
    for chunk_crc in [FixtureCrc::Zero, FixtureCrc::ValidNonZero] {
        for summary_crc in [FixtureCrc::Zero, FixtureCrc::ValidNonZero] {
            for summary_offsets in [false, true] {
                let fixture = build(
                    AdversarialMcapFixtureBuilder::new()
                        .with_chunk_crc(chunk_crc)
                        .with_summary_crc(summary_crc)
                        .with_summary_offsets(summary_offsets),
                );
                assert!(fixture.is_mcap_format_valid());
                assert!(
                    fixture
                        .read_upstream_summary()
                        .expect("summary read")
                        .is_some()
                );
                assert_eq!(
                    fixture
                        .read_upstream_indexed(None, None)
                        .expect("indexed read")
                        .messages
                        .len(),
                    1
                );
                let footer = mcap::read::footer(&fixture.bytes).expect("Footer");
                assert_eq!(footer.summary_start, fixture.layout.summary_start as u64);
                if summary_offsets {
                    assert_ne!(footer.summary_offset_start, 0);
                    assert_eq!(
                        fixture.bytes[footer.summary_offset_start as usize],
                        op::SUMMARY_OFFSET
                    );
                } else {
                    assert_eq!(footer.summary_offset_start, 0);
                }
                let mut wrong_summary_offset_control = fixture.clone();
                wrong_summary_offset_control.summary_offsets = !summary_offsets;
                assert!(wrong_summary_offset_control.validate_self().is_err());
            }
        }
    }
}

#[test]
fn invalid_crc_modes_have_only_their_checksum_violation() {
    let invalid_summary =
        build(AdversarialMcapFixtureBuilder::new().with_summary_crc(FixtureCrc::InvalidNonZero));
    assert!(invalid_summary.has_only_format_violation(McapFormatViolation::SummaryCrc));
    let footer = mcap::read::footer(&invalid_summary.bytes).expect("Footer");
    assert_ne!(footer.summary_crc, 0);
    let reference = ReferenceInspection::inspect(&invalid_summary.bytes).expect("reference");
    let footer = reference.footer.expect("reference Footer");
    assert_ne!(footer.summary_crc, footer.actual_summary_crc);
    assert!(invalid_summary.read_upstream_summary().is_ok());

    let invalid_chunk =
        build(AdversarialMcapFixtureBuilder::new().with_chunk_crc(FixtureCrc::InvalidNonZero));
    assert!(invalid_chunk.has_only_format_violation(McapFormatViolation::ChunkCrc));
    let chunk = &invalid_chunk.layout.chunks[0];
    assert_ne!(chunk.declared_uncompressed_crc, 0);
    assert_ne!(
        chunk.declared_uncompressed_crc,
        chunk.actual_uncompressed_crc
    );
    assert!(invalid_chunk.read_upstream_summary().is_ok());
    // Upstream's indexed reader currently does not validate the Chunk CRC. The independent byte
    // inspector above is therefore the checksum oracle rather than this permissive behavior.
    assert!(invalid_chunk.read_upstream_indexed(None, None).is_ok());
}

#[test]
fn legal_upstream_compression_frames_remain_index_readable() {
    for compression in [CompressionFixture::Zstd, CompressionFixture::Lz4] {
        let fixture = build(
            AdversarialMcapFixtureBuilder::new()
                .with_compression(compression)
                .with_chunk_crc(FixtureCrc::ValidNonZero),
        );
        assert!(fixture.is_mcap_format_valid());
        assert_eq!(
            fixture
                .read_upstream_indexed(None, None)
                .expect("indexed read")
                .messages
                .len(),
            1
        );
    }
}

#[test]
fn chunk_time_order_top_level_and_raw_time_controls_are_independent() {
    let chunks = [
        FixtureChunk::single(FixtureMessage::new(1, 0, 30)),
        FixtureChunk::new([FixtureMessage::new(1, 1, 10), FixtureMessage::new(1, 2, 20)]),
        FixtureChunk::single(FixtureMessage::new(1, 3, 20)),
    ];
    let fixture = build(
        AdversarialMcapFixtureBuilder::new()
            .with_chunks(chunks)
            .with_top_level_messages([FixtureMessage::new(1, 4, 15)]),
    );
    assert!(fixture.is_mcap_format_valid());
    assert_eq!(
        fixture
            .layout
            .chunks
            .iter()
            .map(|chunk| chunk.declared_index_range)
            .collect::<Vec<_>>(),
        vec![
            RawTimeRange::new(30, 30),
            RawTimeRange::new(10, 20),
            RawTimeRange::new(20, 20),
        ]
    );
    assert!(
        fixture
            .layout
            .records
            .iter()
            .any(|record| record.opcode == op::MESSAGE)
    );

    let empty = build(AdversarialMcapFixtureBuilder::new().with_chunks([FixtureChunk::empty()]));
    let time_zero = build(
        AdversarialMcapFixtureBuilder::new()
            .with_chunks([FixtureChunk::single(FixtureMessage::new(1, 0, 0))]),
    );
    assert!(empty.is_mcap_format_valid());
    assert!(time_zero.is_mcap_format_valid());
    assert!(empty.layout.chunks[0].message_log_times.is_empty());
    assert_eq!(time_zero.layout.chunks[0].message_log_times, vec![0]);
    assert_eq!(
        empty.layout.chunks[0].declared_header_range,
        RawTimeRange::new(0, 0)
    );
    assert_eq!(
        time_zero.layout.chunks[0].declared_header_range,
        RawTimeRange::new(0, 0)
    );

    let max = build(
        AdversarialMcapFixtureBuilder::new().with_chunks([FixtureChunk::single(
            FixtureMessage::new(1, 0, i64::MAX as u64).with_publish_time(i64::MAX as u64),
        )]),
    );
    assert!(max.is_mcap_format_valid());
    let mut falsely_invalid = max.clone();
    falsely_invalid
        .remote_expected_outcomes
        .insert(RemotePolicyOutcome::InvalidTemporalValue);
    assert!(falsely_invalid.validate_self().is_err());
    for message in [
        FixtureMessage::new(1, 0, i64::MAX as u64 + 1),
        FixtureMessage::new(1, 0, 1).with_publish_time(i64::MAX as u64 + 1),
    ] {
        let invalid = build(
            AdversarialMcapFixtureBuilder::new().with_chunks([FixtureChunk::single(message)]),
        );
        assert!(invalid.is_mcap_format_valid());
        assert!(invalid.has_only_remote_outcome(RemotePolicyOutcome::InvalidTemporalValue));
        invalid
            .validate_upstream_differential()
            .expect("raw u64 remains valid MCAP");
        let mut missing_outcome = invalid.clone();
        missing_outcome
            .remote_expected_outcomes
            .remove(&RemotePolicyOutcome::InvalidTemporalValue);
        assert!(missing_outcome.validate_self().is_err());
    }

    let unselected_publish = build(
        AdversarialMcapFixtureBuilder::new()
            .with_channels([
                FixtureChannel::schema_less(1, "/selected"),
                FixtureChannel::schema_less(2, "/unselected"),
            ])
            .with_chunks([FixtureChunk::new([
                FixtureMessage::new(1, 0, 1),
                FixtureMessage::new(2, 1, 2).with_publish_time(i64::MAX as u64 + 1),
            ])]),
    );
    assert!(unselected_publish.has_only_remote_outcome(RemotePolicyOutcome::InvalidTemporalValue));

    let index_only_invalid = build(
        AdversarialMcapFixtureBuilder::new()
            .with_chunks([FixtureChunk::single(FixtureMessage::new(
                1,
                0,
                i64::MAX as u64,
            ))])
            .with_message_index_fault(MessageIndexFault::EntryTime),
    );
    assert!(
        index_only_invalid
            .remote_expected_outcomes
            .contains(&RemotePolicyOutcome::InvalidTemporalValue)
    );

    for time_type in [
        McapTimeTypeFixture::TimestampNs,
        McapTimeTypeFixture::DurationNs,
    ] {
        let boot_relative = build(
            AdversarialMcapFixtureBuilder::new()
                .with_time_type(time_type)
                .with_chunks([FixtureChunk::new([
                    FixtureMessage::new(1, 0, 0),
                    FixtureMessage::new(1, 1, 60_000_000_000),
                ])]),
        );
        assert!(boot_relative.is_mcap_format_valid());
        assert_eq!(boot_relative.time_type, time_type);
    }
}

#[test]
fn declared_chunk_range_can_be_the_only_violation() {
    let fixture = build(
        AdversarialMcapFixtureBuilder::new().with_chunks([FixtureChunk::new([
            FixtureMessage::new(1, 0, 10),
            FixtureMessage::new(1, 1, 20),
        ])
        .with_header_range(RawTimeRange::new(0, 30))
        .with_index_range(RawTimeRange::new(0, 30))]),
    );
    assert!(fixture.has_only_format_violation(McapFormatViolation::ChunkDeclaredTimeRange));
    let reference = ReferenceInspection::inspect(&fixture.bytes).expect("reference");
    assert_eq!(reference.chunks[0].message_range, RawTimeRange::new(0, 30));
    assert_eq!(
        reference.chunk_indexes[0].message_range,
        RawTimeRange::new(0, 30)
    );
    assert_eq!(
        reference.chunks[0]
            .messages
            .iter()
            .map(|message| message.log_time)
            .collect::<Vec<_>>(),
        vec![10, 20]
    );
    assert_eq!(
        reference.message_indexes[0]
            .entries
            .iter()
            .map(|entry| entry.log_time)
            .collect::<Vec<_>>(),
        vec![10, 20]
    );
    let mut missing_label = fixture.clone();
    missing_label
        .mcap_format_violations
        .remove(&McapFormatViolation::ChunkDeclaredTimeRange);
    assert!(missing_label.validate_self().is_err());
    let mut wrong_label = fixture.clone();
    wrong_label.mcap_format_violations =
        std::iter::once(McapFormatViolation::StatisticsCount).collect();
    assert!(wrong_label.validate_self().is_err());
}

#[test]
fn every_message_index_fault_has_precise_typed_evidence() {
    let missing = build(
        AdversarialMcapFixtureBuilder::new().with_message_index_fault(MessageIndexFault::Missing),
    );
    assert!(missing.is_mcap_format_valid());
    assert!(missing.has_only_remote_outcome(RemotePolicyOutcome::MessageIndexAbsent));
    missing
        .validate_upstream_differential()
        .expect("missing MessageIndex is valid MCAP");

    let cases = [
        (
            MessageIndexFault::Duplicate,
            McapFormatViolation::MessageIndexDuplicate,
        ),
        (
            MessageIndexFault::EntryOffset,
            McapFormatViolation::MessageIndexEntryOffset,
        ),
        (
            MessageIndexFault::EntryTime,
            McapFormatViolation::MessageIndexEntryTime,
        ),
        (
            MessageIndexFault::WrongOpcode,
            McapFormatViolation::MessageIndexOpcode,
        ),
        (
            MessageIndexFault::MapKeyMismatch,
            McapFormatViolation::MessageIndexMapKey,
        ),
        (
            MessageIndexFault::RecordCrossesOwningRegion,
            McapFormatViolation::MessageIndexOwningRegion,
        ),
        (
            MessageIndexFault::DescriptorOffsetIntoRecordBody,
            McapFormatViolation::MessageIndexDescriptorOffset,
        ),
        (
            MessageIndexFault::DescriptorOffsetIntoChunk,
            McapFormatViolation::MessageIndexDescriptorOffset,
        ),
        (
            MessageIndexFault::CrossChunkAlias,
            McapFormatViolation::MessageIndexCrossChunkAlias,
        ),
    ];
    for (fault, violation) in cases {
        let chunks = if fault == MessageIndexFault::CrossChunkAlias {
            vec![
                FixtureChunk::single(FixtureMessage::new(1, 0, 1)),
                FixtureChunk::single(FixtureMessage::new(1, 1, 2)),
            ]
        } else {
            vec![FixtureChunk::single(FixtureMessage::new(1, 0, 1))]
        };
        let fixture = build(
            AdversarialMcapFixtureBuilder::new()
                .with_chunks(chunks)
                .with_message_index_fault(fault),
        );
        assert!(
            fixture.has_only_format_violation(violation),
            "unexpected labels for {fault:?}: {:?}",
            fixture.mcap_format_violations
        );
    }

    let duplicate_offset = build(
        AdversarialMcapFixtureBuilder::new()
            .with_message_index_fault(MessageIndexFault::DuplicateDescriptorOffset),
    );
    assert_eq!(
        duplicate_offset.mcap_format_violations,
        [
            McapFormatViolation::MessageIndexMapKey,
            McapFormatViolation::MessageIndexDuplicateDescriptorOffset,
        ]
        .into_iter()
        .collect()
    );
}

#[test]
fn definition_scenarios_cover_schema_less_duplicates_unknown_and_conflicts() {
    for scenario in [
        DefinitionFixture::Matching,
        DefinitionFixture::ExactDuplicate,
        DefinitionFixture::UnknownUnreferenced,
    ] {
        let fixture = build(AdversarialMcapFixtureBuilder::new().with_definition_fixture(scenario));
        assert!(fixture.is_mcap_format_valid(), "{scenario:?}");
        assert!(
            fixture
                .read_upstream_summary()
                .expect("summary read")
                .is_some()
        );
        let mut wrong_target = fixture.clone();
        wrong_target.definition_fixture = match scenario {
            DefinitionFixture::Matching => DefinitionFixture::ExactDuplicate,
            DefinitionFixture::ExactDuplicate => DefinitionFixture::Matching,
            DefinitionFixture::UnknownUnreferenced => DefinitionFixture::UnknownReferenced,
            _ => unreachable!(),
        };
        assert!(wrong_target.validate_self().is_err(), "{scenario:?}");
    }

    let cases = [
        (
            DefinitionFixture::UnknownReferenced,
            McapFormatViolation::UnknownReferencedChannel,
        ),
        (
            DefinitionFixture::MissingReferencedSchema,
            McapFormatViolation::MissingReferencedSchema,
        ),
        (
            DefinitionFixture::SchemaNameConflict,
            McapFormatViolation::ConflictingSchema,
        ),
        (
            DefinitionFixture::SchemaEncodingConflict,
            McapFormatViolation::ConflictingSchema,
        ),
        (
            DefinitionFixture::SchemaDataConflict,
            McapFormatViolation::ConflictingSchema,
        ),
        (
            DefinitionFixture::ChannelSchemaConflict,
            McapFormatViolation::ConflictingChannel,
        ),
        (
            DefinitionFixture::ChannelTopicConflict,
            McapFormatViolation::ConflictingChannel,
        ),
        (
            DefinitionFixture::ChannelEncodingConflict,
            McapFormatViolation::ConflictingChannel,
        ),
        (
            DefinitionFixture::ChannelMetadataConflict,
            McapFormatViolation::ConflictingChannel,
        ),
        (
            DefinitionFixture::UnselectedChannelConflict,
            McapFormatViolation::ConflictingChannel,
        ),
        (
            DefinitionFixture::ChunkTailConflict,
            McapFormatViolation::ConflictingChannel,
        ),
    ];
    for (scenario, violation) in cases {
        let fixture = build(AdversarialMcapFixtureBuilder::new().with_definition_fixture(scenario));
        assert!(fixture.has_only_format_violation(violation), "{scenario:?}");
        let mut wrong_target = fixture.clone();
        wrong_target.definition_fixture = match scenario {
            DefinitionFixture::UnknownReferenced => DefinitionFixture::UnknownUnreferenced,
            DefinitionFixture::MissingReferencedSchema => DefinitionFixture::Matching,
            DefinitionFixture::SchemaNameConflict => DefinitionFixture::SchemaEncodingConflict,
            DefinitionFixture::SchemaEncodingConflict => DefinitionFixture::SchemaDataConflict,
            DefinitionFixture::SchemaDataConflict => DefinitionFixture::SchemaNameConflict,
            DefinitionFixture::ChannelSchemaConflict => DefinitionFixture::ChannelTopicConflict,
            DefinitionFixture::ChannelTopicConflict => DefinitionFixture::ChannelEncodingConflict,
            DefinitionFixture::ChannelEncodingConflict => {
                DefinitionFixture::ChannelMetadataConflict
            }
            DefinitionFixture::ChannelMetadataConflict => DefinitionFixture::ChannelSchemaConflict,
            DefinitionFixture::UnselectedChannelConflict => DefinitionFixture::ChunkTailConflict,
            DefinitionFixture::ChunkTailConflict => DefinitionFixture::UnselectedChannelConflict,
            _ => unreachable!(),
        };
        assert!(wrong_target.validate_self().is_err(), "{scenario:?}");
    }

    let mut selected_without_messages = build(
        AdversarialMcapFixtureBuilder::new()
            .with_definition_fixture(DefinitionFixture::UnselectedChannelConflict),
    );
    selected_without_messages.partition.selected_group_members = vec![vec![1, 2]];
    selected_without_messages.partition.selected_channel_groups = 1;
    selected_without_messages
        .partition
        .channels_per_selected_group = 2;
    assert!(selected_without_messages.validate_self().is_err());

    assert_build_error(
        AdversarialMcapFixtureBuilder::new()
            .with_definition_fixture(DefinitionFixture::ChunkTailConflict)
            .with_trailing_canonical_channel_after_conflict(),
        "serialized definitions do not match requested ChunkTailConflict",
    );
}

#[test]
fn statistics_and_physical_layout_controls_are_self_identifying() {
    for statistics in [
        StatisticsFixture::WrongMessageCount,
        StatisticsFixture::WrongChunkCount,
        StatisticsFixture::WrongChannelCount,
    ] {
        let fixture =
            build(AdversarialMcapFixtureBuilder::new().with_statistics_fixture(statistics));
        assert!(fixture.has_only_format_violation(McapFormatViolation::StatisticsCount));
        let mut wrong_target = fixture.clone();
        wrong_target.statistics_fixture = match statistics {
            StatisticsFixture::WrongMessageCount => StatisticsFixture::WrongChannelCount,
            StatisticsFixture::WrongChunkCount | StatisticsFixture::WrongChannelCount => {
                StatisticsFixture::WrongMessageCount
            }
            StatisticsFixture::Exact => unreachable!(),
        };
        assert!(wrong_target.validate_self().is_err(), "{statistics:?}");
    }

    let exact_duplicate = build(
        AdversarialMcapFixtureBuilder::new()
            .with_physical_layout_fault(PhysicalLayoutFault::DuplicateChunkIndex),
    );
    assert!(exact_duplicate.is_mcap_format_valid());
    assert_eq!(
        exact_duplicate
            .read_upstream_summary()
            .expect("summary read")
            .expect("summary")
            .chunk_indexes
            .len(),
        2
    );
    let mut wrong_duplicate = exact_duplicate.clone();
    wrong_duplicate.physical_layout_fault = PhysicalLayoutFault::None;
    assert!(wrong_duplicate.validate_self().is_err());

    let faults = [
        PhysicalLayoutFault::ConflictingDuplicateChunkIndex,
        PhysicalLayoutFault::ChunkOverlapsMessageIndex,
        PhysicalLayoutFault::ChunkOverlapsDataEnd,
        PhysicalLayoutFault::ChunkOverlapsSummary,
        PhysicalLayoutFault::MessageIndexOverlapsDataEnd,
        PhysicalLayoutFault::MessageIndexOverlapsSummary,
    ];
    for fault in faults {
        let fixture = build(AdversarialMcapFixtureBuilder::new().with_physical_layout_fault(fault));
        let violation = if fault == PhysicalLayoutFault::ConflictingDuplicateChunkIndex {
            McapFormatViolation::ConflictingChunkIndex
        } else {
            McapFormatViolation::PhysicalRegionOverlap
        };
        assert!(fixture.has_only_format_violation(violation), "{fault:?}");
        let mut wrong_target = fixture.clone();
        wrong_target.physical_layout_fault =
            if fault == PhysicalLayoutFault::ConflictingDuplicateChunkIndex {
                PhysicalLayoutFault::DuplicateChunkIndex
            } else {
                PhysicalLayoutFault::ChunkOverlapsSummary
            };
        if wrong_target.physical_layout_fault == fault {
            wrong_target.physical_layout_fault = PhysicalLayoutFault::ChunkOverlapsDataEnd;
        }
        assert!(wrong_target.validate_self().is_err(), "{fault:?}");
    }
    let overlapping = build(
        AdversarialMcapFixtureBuilder::new()
            .with_chunks([
                FixtureChunk::single(FixtureMessage::new(1, 0, 1)),
                FixtureChunk::single(FixtureMessage::new(1, 1, 2)),
            ])
            .with_physical_layout_fault(PhysicalLayoutFault::OverlappingChunkRanges),
    );
    assert!(overlapping.has_only_format_violation(McapFormatViolation::PhysicalRegionOverlap));
    let mut wrong_overlap = overlapping.clone();
    wrong_overlap.physical_layout_fault = PhysicalLayoutFault::ChunkOverlapsMessageIndex;
    assert!(wrong_overlap.validate_self().is_err());
    let mut falsely_canonical = overlapping.clone();
    falsely_canonical.physical_layout_fault = PhysicalLayoutFault::None;
    assert!(falsely_canonical.validate_self().is_err());
}

#[test]
fn premature_eof_is_location_exact_and_rejects_obscured_controls() {
    let limits = NestedCollectionLimits {
        max_entries: 3,
        max_string_bytes: 16,
        max_retained_bytes: 64,
    };
    let nested = NestedCollectionFixture {
        target: NestedCollectionTarget::ChannelMetadata,
        shape: NestedCollectionShape::PrematureEof,
        limits,
    };
    let chunk_local = build(
        AdversarialMcapFixtureBuilder::new()
            .with_chunk_crc(FixtureCrc::InvalidNonZero)
            .with_nested_collection_at(nested, NestedCollectionLocation::Chunk { chunk_index: 0 }),
    );
    assert_eq!(
        chunk_local.mcap_format_violations,
        [
            McapFormatViolation::ChunkCrc,
            McapFormatViolation::PrematureEof
        ]
        .into_iter()
        .collect()
    );
    let mut wrong_location = chunk_local.clone();
    wrong_location.nested_collection_location = Some(NestedCollectionLocation::Summary);
    assert!(wrong_location.validate_self().is_err());
    let chunk_local_invalid_log = build(
        AdversarialMcapFixtureBuilder::new()
            .with_chunks([FixtureChunk::single(
                FixtureMessage::new(1, 0, i64::MAX as u64 + 1).with_publish_time(1),
            )])
            .with_nested_collection_at(nested, NestedCollectionLocation::Chunk { chunk_index: 0 }),
    );
    assert!(
        chunk_local_invalid_log
            .remote_expected_outcomes
            .contains(&RemotePolicyOutcome::InvalidTemporalValue)
    );
    assert_build_error(
        AdversarialMcapFixtureBuilder::new()
            .with_chunks([FixtureChunk::single(
                FixtureMessage::new(1, 0, 1).with_publish_time(i64::MAX as u64 + 1),
            )])
            .with_nested_collection_at(nested, NestedCollectionLocation::Chunk { chunk_index: 0 }),
        "would obscure an invalid publish_time",
    );

    let summary_premature = |builder: AdversarialMcapFixtureBuilder| {
        builder.with_nested_collection_at(nested, NestedCollectionLocation::Summary)
    };
    let outer_with_chunk_crc = build(summary_premature(
        AdversarialMcapFixtureBuilder::new().with_chunk_crc(FixtureCrc::InvalidNonZero),
    ));
    assert_eq!(
        outer_with_chunk_crc.mcap_format_violations,
        [
            McapFormatViolation::ChunkCrc,
            McapFormatViolation::PrematureEof
        ]
        .into_iter()
        .collect()
    );
    assert_build_error(
        summary_premature(
            AdversarialMcapFixtureBuilder::new().with_summary_crc(FixtureCrc::InvalidNonZero),
        ),
        "outer premature EOF would obscure",
    );
    assert_build_error(
        summary_premature(AdversarialMcapFixtureBuilder::new().with_summary_offsets(false)),
        "outer premature EOF would obscure",
    );
    assert_build_error(
        summary_premature(
            AdversarialMcapFixtureBuilder::new()
                .with_message_index_fault(MessageIndexFault::EntryOffset),
        ),
        "outer premature EOF would obscure",
    );
    assert_build_error(
        summary_premature(
            AdversarialMcapFixtureBuilder::new()
                .with_definition_fixture(DefinitionFixture::ChannelTopicConflict),
        ),
        "outer premature EOF would obscure",
    );
    assert_build_error(
        summary_premature(
            AdversarialMcapFixtureBuilder::new()
                .with_physical_layout_fault(PhysicalLayoutFault::ChunkOverlapsDataEnd),
        ),
        "outer premature EOF would obscure",
    );

    let message_index_premature = NestedCollectionFixture {
        target: NestedCollectionTarget::MessageIndexRecords,
        shape: NestedCollectionShape::PrematureEof,
        limits,
    };
    assert_build_error(
        AdversarialMcapFixtureBuilder::new()
            .with_summary_crc(FixtureCrc::InvalidNonZero)
            .with_nested_collection_at(
                message_index_premature,
                NestedCollectionLocation::MessageIndexRegion { chunk_index: 0 },
            ),
        "outer premature EOF would obscure",
    );
    assert_build_error(
        AdversarialMcapFixtureBuilder::new()
            .with_message_index_fault(MessageIndexFault::EntryTime)
            .with_nested_collection_at(
                message_index_premature,
                NestedCollectionLocation::MessageIndexRegion { chunk_index: 0 },
            ),
        "outer premature EOF would obscure",
    );
    assert_build_error(
        AdversarialMcapFixtureBuilder::new()
            .with_definition_fixture(DefinitionFixture::ChannelMetadataConflict)
            .with_nested_collection_at(
                message_index_premature,
                NestedCollectionLocation::MessageIndexRegion { chunk_index: 0 },
            ),
        "outer premature EOF would obscure",
    );
    assert_build_error(
        AdversarialMcapFixtureBuilder::new()
            .with_physical_layout_fault(PhysicalLayoutFault::MessageIndexOverlapsSummary)
            .with_nested_collection_at(
                message_index_premature,
                NestedCollectionLocation::MessageIndexRegion { chunk_index: 0 },
            ),
        "outer premature EOF would obscure",
    );
}

#[test]
fn message_index_nested_location_owns_the_requested_chunk() {
    let nested = NestedCollectionFixture {
        target: NestedCollectionTarget::MessageIndexRecords,
        shape: NestedCollectionShape::MaximumDensity,
        limits: NestedCollectionLimits {
            max_entries: 3,
            max_string_bytes: 16,
            max_retained_bytes: 64,
        },
    };
    let fixture = build(
        AdversarialMcapFixtureBuilder::new()
            .with_chunks([
                FixtureChunk::single(FixtureMessage::new(1, 0, 10)),
                FixtureChunk::single(FixtureMessage::new(1, 1, 20)),
            ])
            .with_compression(CompressionFixture::Zstd)
            .with_nested_collection_at(
                nested,
                NestedCollectionLocation::MessageIndexRegion { chunk_index: 1 },
            ),
    );
    let reference = ReferenceInspection::inspect(&fixture.bytes).expect("reference");
    let chunk_zero = reference
        .nested_collections
        .iter()
        .find(|collection| {
            collection.target == NestedCollectionTarget::MessageIndexRecords
                && collection.section == (ReferenceSection::MessageIndex { chunk_index: 0 })
        })
        .expect("Chunk 0 MessageIndex");
    let chunk_one = reference
        .nested_collections
        .iter()
        .find(|collection| {
            collection.target == NestedCollectionTarget::MessageIndexRecords
                && collection.section == (ReferenceSection::MessageIndex { chunk_index: 1 })
        })
        .expect("Chunk 1 MessageIndex");
    assert_eq!(chunk_zero.entry_count, 1);
    assert_eq!(chunk_one.entry_count, 3);

    assert_build_error(
        AdversarialMcapFixtureBuilder::new().with_nested_collection_at(
            nested,
            NestedCollectionLocation::MessageIndexRegion { chunk_index: 1 },
        ),
        "references absent Chunk 1",
    );
}

#[test]
fn nested_collection_matrix_covers_all_targets_and_boundary_shapes() {
    let limits = NestedCollectionLimits {
        max_entries: 3,
        max_string_bytes: 8,
        max_retained_bytes: 24,
    };
    let targets = [
        NestedCollectionTarget::ChannelMetadata,
        NestedCollectionTarget::ChunkIndexMessageIndexOffsets,
        NestedCollectionTarget::StatisticsChannelMessageCounts,
        NestedCollectionTarget::MessageIndexRecords,
    ];
    for target in targets {
        let maximum = build(AdversarialMcapFixtureBuilder::new().with_nested_collection(
            NestedCollectionFixture {
                target,
                shape: NestedCollectionShape::MaximumDensity,
                limits,
            },
        ));
        assert!(maximum.is_mcap_format_valid(), "maximum {target:?}");
        assert!(
            maximum
                .read_upstream_summary()
                .expect("summary read")
                .is_some()
        );

        for (shape, outcome) in [
            (
                NestedCollectionShape::TooManyEntries,
                RemotePolicyOutcome::NestedCollectionCount,
            ),
            (
                NestedCollectionShape::TooManyRetainedBytes,
                RemotePolicyOutcome::NestedRetainedBytes,
            ),
        ] {
            let fixture = build(AdversarialMcapFixtureBuilder::new().with_nested_collection(
                NestedCollectionFixture {
                    target,
                    shape,
                    limits,
                },
            ));
            assert!(fixture.is_mcap_format_valid(), "{target:?} {shape:?}");
            assert!(fixture.has_only_remote_outcome(outcome));
            fixture
                .validate_upstream_differential()
                .expect("resource-policy outcome remains valid MCAP");
        }
        for (shape, violation) in [
            (
                NestedCollectionShape::WrongByteLength,
                McapFormatViolation::NestedByteLength,
            ),
            (
                NestedCollectionShape::PrematureEof,
                McapFormatViolation::PrematureEof,
            ),
        ] {
            let fixture = build(AdversarialMcapFixtureBuilder::new().with_nested_collection(
                NestedCollectionFixture {
                    target,
                    shape,
                    limits,
                },
            ));
            assert!(fixture.has_only_format_violation(violation));
        }
    }
    let oversized_string = build(AdversarialMcapFixtureBuilder::new().with_nested_collection(
        NestedCollectionFixture {
            target: NestedCollectionTarget::ChannelMetadata,
            shape: NestedCollectionShape::OversizedString,
            limits,
        },
    ));
    assert!(oversized_string.is_mcap_format_valid());
    assert!(oversized_string.has_only_remote_outcome(RemotePolicyOutcome::NestedStringLength));
}

#[test]
fn compression_adversarial_states_have_precise_expectations() {
    let format_cases = [
        (
            CompressionFixture::DeclaredOutputTooSmall,
            McapFormatViolation::CompressedOutputSize,
        ),
        (
            CompressionFixture::DeclaredOutputTooLarge,
            McapFormatViolation::CompressedOutputSize,
        ),
        (
            CompressionFixture::TrailingPayload,
            McapFormatViolation::CompressedTrailingPayload,
        ),
        (
            CompressionFixture::TruncatedInput,
            McapFormatViolation::CompressedInputTruncated,
        ),
    ];
    for (compression, violation) in format_cases {
        let fixture = build(AdversarialMcapFixtureBuilder::new().with_compression(compression));
        assert!(
            fixture.has_only_format_violation(violation),
            "{compression:?}"
        );
    }

    let canonical =
        build(AdversarialMcapFixtureBuilder::new().with_compression(CompressionFixture::Zstd));
    let concatenated = build(
        AdversarialMcapFixtureBuilder::new()
            .with_compression(CompressionFixture::ConcatenatedFrame),
    );
    concatenated
        .validate_upstream_differential()
        .expect("upstream accepts concatenated zstd frames");
    assert!(concatenated.is_mcap_format_valid());
    assert!(concatenated.has_only_remote_outcome(RemotePolicyOutcome::MultipleCompressionFrames));
    let canonical = ReferenceInspection::inspect(&canonical.bytes)
        .expect("canonical reference")
        .chunks
        .into_iter()
        .next()
        .expect("canonical Chunk");
    let concatenated = ReferenceInspection::inspect(&concatenated.bytes)
        .expect("concatenated reference")
        .chunks
        .into_iter()
        .next()
        .expect("concatenated Chunk");
    assert_eq!(concatenated.uncompressed_data, canonical.uncompressed_data);
    assert_eq!(
        concatenated.declared_uncompressed_size,
        canonical.declared_uncompressed_size
    );
    assert_eq!(
        concatenated.declared_uncompressed_crc,
        canonical.declared_uncompressed_crc
    );
    assert_eq!(
        concatenated.actual_uncompressed_crc,
        canonical.actual_uncompressed_crc
    );
    let first_frame_len = concatenated
        .first_zstd_frame_len
        .expect("first zstd frame length");
    assert!(first_frame_len < concatenated.compressed_data.len());
    assert!(
        zstd::stream::decode_all(&concatenated.compressed_data[first_frame_len..])
            .expect("second zstd frame")
            .is_empty()
    );

    let oversized = build(
        AdversarialMcapFixtureBuilder::new()
            .with_compression(CompressionFixture::OversizedDeclaration),
    );
    assert!(oversized.has_only_format_violation(McapFormatViolation::CompressedOutputSize));
    assert!(oversized.has_only_remote_outcome(RemotePolicyOutcome::CompressedDeclarationLimit));

    for (compression, outcome) in [
        (
            CompressionFixture::OutputFullBeforeCodecEof,
            FakeComponentOutcome::CodecDidNotReachEof,
        ),
        (
            CompressionFixture::ZeroProgress,
            FakeComponentOutcome::CodecZeroProgress,
        ),
    ] {
        let fixture = build(AdversarialMcapFixtureBuilder::new().with_compression(compression));
        assert!(fixture.is_mcap_format_valid());
        assert!(fixture.has_only_fake_component_outcome(outcome));
        fixture
            .validate_upstream_differential()
            .expect("fake codec schedule uses canonical MCAP bytes");
    }
}

#[test]
fn cardinality_decoder_and_partition_parameters_are_composable() {
    let cardinality = FixtureCardinality {
        summary_records: 9,
        schemas: 2,
        channels: 3,
        chunk_indexes: 2,
        records_per_chunk: 8,
        messages_per_chunk: 2,
        selected_dispatches_per_scan: 4,
    };
    let mut decoder = DecoderAssignmentFixture {
        recognition_input: DecoderRecognitionInput {
            channel_id: 1,
            schema_id: 1,
            schema_name: Some("fixture.Schema0".to_owned()),
            schema_encoding: Some("protobuf".to_owned()),
            message_encoding: "protobuf".to_owned(),
        },
        identical_duplicate_groups: 2,
        ..DecoderAssignmentFixture::default()
    };
    decoder.allowlist_serialization.reverse();
    let partition = PartitionFixture {
        selected_group_roots: vec![2, 3],
        opening_static_roots: 2,
        external_origin_bytes_per_partition: vec![4, 8],
        cursor_time: 1,
        cursor_covering_chunks: 1,
        selected_channel_groups: 2,
        channels_per_selected_group: 2,
        selected_group_members: vec![vec![1, 2], vec![3]],
        generations: vec![
            PartitionGenerationFixture::CompleteEmpty,
            PartitionGenerationFixture::Roots {
                root_descriptors: 3,
                external_origin_bytes: 8,
            },
        ],
        resident_roots: 7,
        rows_per_root: 1024,
        unsorted_timeline: true,
        ..PartitionFixture::default()
    };
    let fixture = build(
        AdversarialMcapFixtureBuilder::new()
            .with_cardinality(cardinality)
            .with_decoder_fixture(decoder.clone())
            .with_partition_fixture(partition.clone()),
    );
    assert!(fixture.is_mcap_format_valid());
    assert_eq!(fixture.cardinality, cardinality);
    assert_eq!(fixture.decoder, decoder);
    assert_eq!(fixture.partition, partition);
    assert_eq!(
        fixture.decoder.recognized_by,
        std::iter::once(FixtureDecoder::Raw).collect()
    );
    assert!(fixture.mcap_format_violations.is_empty());
    assert!(fixture.remote_expected_outcomes.is_empty());
    assert!(fixture.fake_component_outcomes.is_empty());
    let summary = fixture
        .read_upstream_summary()
        .expect("summary read")
        .expect("summary");
    assert_eq!(summary.schemas.len(), 2);
    assert_eq!(summary.channels.len(), 3);
    assert_eq!(summary.chunk_indexes.len(), 2);
    assert_eq!(
        fixture
            .read_upstream_indexed(None, None)
            .expect("indexed")
            .messages
            .len(),
        4
    );

    let overlap = DecoderAssignmentFixture {
        singleton_multi_group_overlap: true,
        ..DecoderAssignmentFixture::default()
    };
    assert!(
        build(AdversarialMcapFixtureBuilder::new().with_decoder_fixture(overlap))
            .has_only_fake_component_outcome(FakeComponentOutcome::DecoderGroupOverlap)
    );
    let cross_owner = DecoderAssignmentFixture {
        cross_owner_reference: true,
        ..DecoderAssignmentFixture::default()
    };
    assert!(
        build(AdversarialMcapFixtureBuilder::new().with_decoder_fixture(cross_owner))
            .has_only_fake_component_outcome(FakeComponentOutcome::DecoderCrossOwnerReference)
    );
    for output in [
        TemporalOutputFixture::MissingCanonicalLogTimeline,
        TemporalOutputFixture::WrongTimelineType,
        TemporalOutputFixture::MessageDerivedStatic,
    ] {
        let temporal = DecoderAssignmentFixture {
            temporal_output: output,
            ..DecoderAssignmentFixture::default()
        };
        assert!(
            build(AdversarialMcapFixtureBuilder::new().with_decoder_fixture(temporal))
                .has_only_fake_component_outcome(FakeComponentOutcome::DecoderTemporalOutput)
        );
    }

    let roots = PartitionFixture {
        max_roots_per_partition: 1,
        selected_group_roots: vec![2],
        ..PartitionFixture::default()
    };
    assert!(
        build(AdversarialMcapFixtureBuilder::new().with_partition_fixture(roots))
            .has_only_remote_outcome(RemotePolicyOutcome::PartitionRootLimit)
    );
    let external = PartitionFixture {
        max_external_origin_bytes_per_partition: 1,
        external_origin_bytes_per_partition: vec![2],
        ..PartitionFixture::default()
    };
    assert!(
        build(AdversarialMcapFixtureBuilder::new().with_partition_fixture(external))
            .has_only_remote_outcome(RemotePolicyOutcome::PartitionExternalOriginLimit)
    );
    let session = PartitionFixture {
        session_root_cap: 1,
        resident_roots: 2,
        ..PartitionFixture::default()
    };
    assert!(
        build(AdversarialMcapFixtureBuilder::new().with_partition_fixture(session))
            .has_only_remote_outcome(RemotePolicyOutcome::SessionRootCap)
    );

    let generation_roots = PartitionFixture {
        max_roots_per_partition: 1,
        generations: vec![PartitionGenerationFixture::Roots {
            root_descriptors: 2,
            external_origin_bytes: 0,
        }],
        ..PartitionFixture::default()
    };
    assert!(
        build(AdversarialMcapFixtureBuilder::new().with_partition_fixture(generation_roots))
            .has_only_remote_outcome(RemotePolicyOutcome::PartitionRootLimit)
    );
    let generation_external = PartitionFixture {
        max_external_origin_bytes_per_partition: 1,
        generations: vec![PartitionGenerationFixture::Roots {
            root_descriptors: 1,
            external_origin_bytes: 2,
        }],
        ..PartitionFixture::default()
    };
    assert!(
        build(AdversarialMcapFixtureBuilder::new().with_partition_fixture(generation_external))
            .has_only_remote_outcome(RemotePolicyOutcome::PartitionExternalOriginLimit)
    );

    let ros_schema = FixtureSchema::new(7, "sensor_msgs/msg/Imu", "ros2msg")
        .with_data(b"string frame_id".to_vec());
    let ros_channel = FixtureChannel::schema_less(1, "/imu").with_schema(7, "cdr");
    let recognition = DecoderAssignmentFixture {
        recognition_input: DecoderRecognitionInput {
            channel_id: 1,
            schema_id: 7,
            schema_name: Some("sensor_msgs/msg/Imu".to_owned()),
            schema_encoding: Some("ros2msg".to_owned()),
            message_encoding: "cdr".to_owned(),
        },
        recognized_by: [
            FixtureDecoder::SemanticRos,
            FixtureDecoder::RosReflection,
            FixtureDecoder::Raw,
        ]
        .into_iter()
        .collect(),
        ..DecoderAssignmentFixture::default()
    };
    let recognized = build(
        AdversarialMcapFixtureBuilder::new()
            .with_schemas([ros_schema])
            .with_channels([ros_channel])
            .with_decoder_fixture(recognition),
    );
    assert!(recognized.is_mcap_format_valid());
    assert_eq!(recognized.decoder.recognized_by.len(), 3);
}

#[test]
fn cardinality_controls_each_have_independent_evidence() {
    let summary_records = build(AdversarialMcapFixtureBuilder::new().with_cardinality(
        FixtureCardinality {
            summary_records: 4,
            ..FixtureCardinality::default()
        },
    ));
    assert_eq!(
        summary_records
            .layout
            .records
            .iter()
            .filter(|record| {
                record.start >= summary_records.layout.summary_start
                    && !matches!(record.opcode, op::SUMMARY_OFFSET | op::FOOTER)
            })
            .count(),
        4
    );

    let schemas = build(AdversarialMcapFixtureBuilder::new().with_cardinality(
        FixtureCardinality {
            summary_records: 4,
            schemas: 1,
            records_per_chunk: 3,
            ..FixtureCardinality::default()
        },
    ));
    assert_eq!(
        schemas
            .read_upstream_summary()
            .expect("Summary")
            .expect("present")
            .schemas
            .len(),
        1
    );

    let channels = build(AdversarialMcapFixtureBuilder::new().with_cardinality(
        FixtureCardinality {
            summary_records: 4,
            channels: 2,
            records_per_chunk: 3,
            ..FixtureCardinality::default()
        },
    ));
    assert_eq!(
        channels
            .read_upstream_summary()
            .expect("Summary")
            .expect("present")
            .channels
            .len(),
        2
    );

    let chunk_indexes = build(AdversarialMcapFixtureBuilder::new().with_cardinality(
        FixtureCardinality {
            summary_records: 5,
            channels: 2,
            chunk_indexes: 2,
            records_per_chunk: 3,
            ..FixtureCardinality::default()
        },
    ));
    assert_eq!(chunk_indexes.layout.chunk_indexes.len(), 2);
    assert_eq!(
        chunk_indexes
            .read_upstream_indexed(None, None)
            .expect("indexed")
            .messages
            .len(),
        2
    );

    let records_per_chunk = build(AdversarialMcapFixtureBuilder::new().with_cardinality(
        FixtureCardinality {
            records_per_chunk: 4,
            ..FixtureCardinality::default()
        },
    ));
    assert_eq!(
        records_per_chunk.layout.chunks[0].uncompressed_record_count,
        4
    );

    let messages_per_chunk = build(AdversarialMcapFixtureBuilder::new().with_cardinality(
        FixtureCardinality {
            summary_records: 4,
            channels: 2,
            records_per_chunk: 4,
            messages_per_chunk: 2,
            ..FixtureCardinality::default()
        },
    ));
    assert_eq!(
        messages_per_chunk.layout.chunks[0].message_log_times.len(),
        2
    );

    let attached = FixtureCardinality {
        summary_records: 4,
        channels: 2,
        records_per_chunk: 9,
        messages_per_chunk: 7,
        selected_dispatches_per_scan: 7,
        ..FixtureCardinality::default()
    };
    let attached_fixture = build(AdversarialMcapFixtureBuilder::new().with_cardinality(attached));
    assert_eq!(attached_fixture.cardinality.selected_dispatches_per_scan, 7);
    let reference = ReferenceInspection::inspect(&attached_fixture.bytes).expect("reference");
    assert_eq!(
        reference
            .chunks
            .iter()
            .flat_map(|chunk| &chunk.messages)
            .filter(|message| message.channel_id == 1)
            .count(),
        7
    );
    assert!(attached_fixture.is_mcap_format_valid());
}

#[test]
fn controls_with_shared_record_ownership_fail_instead_of_overwriting() {
    assert_build_error(
        AdversarialMcapFixtureBuilder::new()
            .with_cardinality(FixtureCardinality::default())
            .with_definition_fixture(DefinitionFixture::ChannelTopicConflict),
        "explicit file cardinality cannot be combined",
    );
    assert_build_error(
        AdversarialMcapFixtureBuilder::new()
            .with_cardinality(FixtureCardinality::default())
            .with_nested_collection(NestedCollectionFixture {
                target: NestedCollectionTarget::ChannelMetadata,
                shape: NestedCollectionShape::MaximumDensity,
                limits: NestedCollectionLimits::default(),
            }),
        "explicit file cardinality cannot be combined",
    );
    assert_build_error(
        AdversarialMcapFixtureBuilder::new()
            .with_message_index_fault(MessageIndexFault::CrossChunkAlias),
        "requires at least two Chunks",
    );
    assert_build_error(
        AdversarialMcapFixtureBuilder::new()
            .with_cardinality(FixtureCardinality::default())
            .with_message_index_fault(MessageIndexFault::DuplicateDescriptorOffset),
        "requires cardinality.channels >= 2",
    );
}

#[test]
fn compressed_chunks_compose_with_definitions_and_nested_locations() {
    let conflict = build(
        AdversarialMcapFixtureBuilder::new()
            .with_compression(CompressionFixture::Zstd)
            .with_definition_fixture(DefinitionFixture::ChannelTopicConflict),
    );
    assert!(conflict.has_only_format_violation(McapFormatViolation::ConflictingChannel));

    let limits = NestedCollectionLimits {
        max_entries: 3,
        max_string_bytes: 16,
        max_retained_bytes: 64,
    };
    for compression in [CompressionFixture::Zstd, CompressionFixture::Lz4] {
        let nested = NestedCollectionFixture {
            target: NestedCollectionTarget::ChannelMetadata,
            shape: NestedCollectionShape::MaximumDensity,
            limits,
        };
        let fixture = build(
            AdversarialMcapFixtureBuilder::new()
                .with_compression(compression)
                .with_nested_collection_at(
                    nested,
                    NestedCollectionLocation::Chunk { chunk_index: 0 },
                ),
        );
        assert!(fixture.is_mcap_format_valid(), "{compression:?}");
        let reference = ReferenceInspection::inspect(&fixture.bytes).expect("reference");
        let collection = reference
            .nested_collections
            .iter()
            .find(|collection| {
                collection.target == NestedCollectionTarget::ChannelMetadata
                    && collection.section == (ReferenceSection::Chunk { chunk_index: 0 })
            })
            .expect("Chunk Channel.metadata");
        assert_eq!(collection.entry_count, limits.max_entries);
    }

    let wrong_length = build(
        AdversarialMcapFixtureBuilder::new()
            .with_compression(CompressionFixture::Zstd)
            .with_nested_collection_at(
                NestedCollectionFixture {
                    target: NestedCollectionTarget::ChannelMetadata,
                    shape: NestedCollectionShape::WrongByteLength,
                    limits,
                },
                NestedCollectionLocation::Chunk { chunk_index: 0 },
            ),
    );
    assert!(wrong_length.has_only_format_violation(McapFormatViolation::NestedByteLength));
}

#[test]
fn orthogonal_mutations_survive_composition_without_overwriting_labels() {
    let fixture = build(
        AdversarialMcapFixtureBuilder::new()
            .with_summary_crc(FixtureCrc::InvalidNonZero)
            .with_message_index_fault(MessageIndexFault::EntryOffset)
            .with_definition_fixture(DefinitionFixture::ChannelTopicConflict),
    );
    assert_eq!(
        fixture.mcap_format_violations,
        [
            McapFormatViolation::MessageIndexEntryOffset,
            McapFormatViolation::SummaryCrc,
            McapFormatViolation::ConflictingChannel,
        ]
        .into_iter()
        .collect()
    );
}
