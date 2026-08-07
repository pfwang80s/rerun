//! Semantic consistency validation for Summary and Chunk-local definitions.

use super::materialization::{BoundedSummaryRecord, MaterializedSummaryRecords};

/// One canonical Schema borrowed from the source-ordered materialized Summary.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CanonicalSchemaDefinition<'a> {
    pub(crate) header: &'a mcap::records::SchemaHeader,
    pub(crate) data: &'a [u8],
}

/// Validated canonical Summary definitions plus their original evidence and multiplicity.
///
/// Canonical lookup is a bounded linear walk over the already bounded source records. This avoids
/// allocating a second, unreserved ID map while retaining every duplicate for evidence and budget
/// accounting.
#[derive(Debug)]
pub(crate) struct ValidatedSummaryDefinitions<'a> {
    materialized: MaterializedSummaryRecords<'a>,
    canonical_schema_count: u64,
    canonical_channel_count: u64,
}

impl<'a> ValidatedSummaryDefinitions<'a> {
    pub(crate) const fn materialized(&self) -> &MaterializedSummaryRecords<'a> {
        &self.materialized
    }

    pub(crate) const fn canonical_schema_count(&self) -> u64 {
        self.canonical_schema_count
    }

    pub(crate) const fn canonical_channel_count(&self) -> u64 {
        self.canonical_channel_count
    }

    pub(crate) fn schema(&self, id: u16) -> Option<CanonicalSchemaDefinition<'_>> {
        find_schema(
            self.materialized.records(),
            id,
            self.materialized.records().len(),
        )
    }

    pub(crate) fn channel(&self, id: u16) -> Option<&mcap::records::Channel> {
        find_channel(
            self.materialized.records(),
            id,
            self.materialized.records().len(),
        )
    }
}

/// A definition/reference consistency failure.
///
/// Errors deliberately omit IDs, names, topics, metadata, record positions, and source content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DefinitionConsistencyError {
    ConflictingSchemaDefinition,
    ConflictingChannelDefinition,
    MissingReferencedSchema,
    MissingMessageIndexChannel,
    UnknownReferencedChannel,
    DefinitionRecordMisroutedAsOther,
    DefinitionCountOverflow,
}

impl std::fmt::Display for DefinitionConsistencyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::ConflictingSchemaDefinition => {
                "remote MCAP contains conflicting Schema definitions"
            }
            Self::ConflictingChannelDefinition => {
                "remote MCAP contains conflicting Channel definitions"
            }
            Self::MissingReferencedSchema => {
                "remote MCAP Channel references a Schema absent from the Summary"
            }
            Self::MissingMessageIndexChannel => {
                "remote MCAP ChunkIndex references a Channel absent from the Summary"
            }
            Self::UnknownReferencedChannel => {
                "remote MCAP Message references a Channel absent from the Summary"
            }
            Self::DefinitionRecordMisroutedAsOther => {
                "remote MCAP definition record was routed through the non-definition event"
            }
            Self::DefinitionCountOverflow => "remote MCAP definition count overflowed",
        })
    }
}

impl std::error::Error for DefinitionConsistencyError {}

/// Validates and logically canonicalizes all materialized Summary Schema and Channel records.
pub(crate) fn validate_summary_definitions(
    materialized: MaterializedSummaryRecords<'_>,
) -> Result<ValidatedSummaryDefinitions<'_>, DefinitionConsistencyError> {
    let records = materialized.records();
    let mut canonical_schema_count = 0_u64;
    let mut canonical_channel_count = 0_u64;

    for (source_index, record) in records.iter().enumerate() {
        match record {
            BoundedSummaryRecord::Known(mcap::records::Record::Schema { header, data }) => {
                if let Some(previous) = find_schema(records, header.id, source_index) {
                    if previous.header.name != header.name
                        || previous.header.encoding != header.encoding
                        || previous.data != data.as_ref()
                    {
                        return Err(DefinitionConsistencyError::ConflictingSchemaDefinition);
                    }
                } else {
                    canonical_schema_count = checked_increment(canonical_schema_count)?;
                }
            }
            BoundedSummaryRecord::Known(mcap::records::Record::Channel(channel)) => {
                if let Some(previous) = find_channel(records, channel.id, source_index) {
                    if previous != channel {
                        return Err(DefinitionConsistencyError::ConflictingChannelDefinition);
                    }
                } else {
                    canonical_channel_count = checked_increment(canonical_channel_count)?;
                }
            }
            _ => {}
        }
    }

    // Reference closure happens only after every group has been validated because Summary opcode
    // groups may occur in any order.
    for (source_index, record) in records.iter().enumerate() {
        let BoundedSummaryRecord::Known(mcap::records::Record::Channel(channel)) = record else {
            continue;
        };
        if find_channel(records, channel.id, source_index).is_some() {
            continue;
        }
        if channel.schema_id != 0
            && find_schema(records, channel.schema_id, records.len()).is_none()
        {
            return Err(DefinitionConsistencyError::MissingReferencedSchema);
        }
    }

    // Every original ChunkIndex participates in reference closure. ChunkIndex canonicalization is
    // deliberately deferred to MCAP-021, so duplicate records cannot hide a later bad key.
    for record in records {
        let BoundedSummaryRecord::Known(mcap::records::Record::ChunkIndex(chunk_index)) = record
        else {
            continue;
        };
        for channel_id in chunk_index.message_index_offsets.keys() {
            if find_channel(records, *channel_id, records.len()).is_none() {
                return Err(DefinitionConsistencyError::MissingMessageIndexChannel);
            }
        }
    }

    Ok(ValidatedSummaryDefinitions {
        materialized,
        canonical_schema_count,
        canonical_channel_count,
    })
}

fn find_schema<'records>(
    records: &'records [BoundedSummaryRecord<'_>],
    id: u16,
    before: usize,
) -> Option<CanonicalSchemaDefinition<'records>> {
    records
        .get(..before)?
        .iter()
        .find_map(|record| match record {
            BoundedSummaryRecord::Known(mcap::records::Record::Schema { header, data })
                if header.id == id =>
            {
                Some(CanonicalSchemaDefinition {
                    header,
                    data: data.as_ref(),
                })
            }
            _ => None,
        })
}

fn find_channel<'records>(
    records: &'records [BoundedSummaryRecord<'_>],
    id: u16,
    before: usize,
) -> Option<&'records mcap::records::Channel> {
    records
        .get(..before)?
        .iter()
        .find_map(|record| match record {
            BoundedSummaryRecord::Known(mcap::records::Record::Channel(channel))
                if channel.id == id =>
            {
                Some(channel)
            }
            _ => None,
        })
}

fn checked_increment(value: u64) -> Result<u64, DefinitionConsistencyError> {
    value
        .checked_add(1)
        .ok_or(DefinitionConsistencyError::DefinitionCountOverflow)
}

/// Proof that an `Other` event does not carry a Schema, Channel, or Message opcode.
///
/// This is only an event-routing guard. It does not validate whether the opcode is otherwise legal
/// in a Chunk; exact envelope and context validation belong to the MCAP-025 scanner.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct NonDefinitionChunkRecord {
    opcode: u8,
}

impl NonDefinitionChunkRecord {
    pub(crate) fn try_from_opcode(opcode: u8) -> Result<Self, DefinitionConsistencyError> {
        if matches!(
            opcode,
            mcap::records::op::SCHEMA | mcap::records::op::CHANNEL | mcap::records::op::MESSAGE
        ) {
            return Err(DefinitionConsistencyError::DefinitionRecordMisroutedAsOther);
        }
        Ok(Self { opcode })
    }
}

/// One exhaustively classified semantic event from a future exact Chunk scanner.
pub(crate) enum ChunkDefinitionEvent<'record> {
    Schema {
        header: &'record mcap::records::SchemaHeader,
        data: &'record [u8],
    },
    Channel(&'record mcap::records::Channel),
    Message {
        channel_id: u16,
    },
    Other(NonDefinitionChunkRecord),
}

/// Reference-driven semantic accumulator for Chunk-local definitions and Message references.
///
/// This type has no Chunk identity or exact-exhaustion witness. Its result is not a validation or
/// publication capability. MCAP-025 must own this accumulator inside its exact scanner and combine
/// the semantic result with sealed source identity and exhaustion evidence before publication.
pub(crate) struct ChunkDefinitionAccumulator<'summary, 'input> {
    summary: &'summary ValidatedSummaryDefinitions<'input>,
    observations_seen: u64,
    messages_seen: u64,
    first_error: Option<DefinitionConsistencyError>,
}

impl std::fmt::Debug for ChunkDefinitionAccumulator<'_, '_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ChunkDefinitionAccumulator")
            .field("summary", &"<validated definition capability>")
            .field("observations_seen", &self.observations_seen)
            .field("messages_seen", &self.messages_seen)
            .field("first_error", &self.first_error)
            .finish_non_exhaustive()
    }
}

impl<'summary, 'input> ChunkDefinitionAccumulator<'summary, 'input> {
    pub(crate) const fn new(summary: &'summary ValidatedSummaryDefinitions<'input>) -> Self {
        Self {
            summary,
            observations_seen: 0,
            messages_seen: 0,
            first_error: None,
        }
    }

    pub(crate) fn observe(
        &mut self,
        event: ChunkDefinitionEvent<'_>,
    ) -> Result<(), DefinitionConsistencyError> {
        self.begin_observation()?;
        match event {
            ChunkDefinitionEvent::Schema { header, data } => self.observe_schema(header, data),
            ChunkDefinitionEvent::Channel(channel) => self.observe_channel(channel),
            ChunkDefinitionEvent::Message { channel_id } => self.observe_message(channel_id),
            ChunkDefinitionEvent::Other(other) => {
                let _opcode = other.opcode;
                Ok(())
            }
        }
    }

    fn observe_schema(
        &mut self,
        header: &mcap::records::SchemaHeader,
        data: &[u8],
    ) -> Result<(), DefinitionConsistencyError> {
        let Some(summary) = self.summary.schema(header.id) else {
            // Summary-unknown and unreferenced Chunk-local definitions are intentionally ignored.
            return Ok(());
        };
        if summary.header.name != header.name
            || summary.header.encoding != header.encoding
            || summary.data != data
        {
            return self.fail(DefinitionConsistencyError::ConflictingSchemaDefinition);
        }
        Ok(())
    }

    fn observe_channel(
        &mut self,
        channel: &mcap::records::Channel,
    ) -> Result<(), DefinitionConsistencyError> {
        let Some(summary) = self.summary.channel(channel.id) else {
            // Standalone closure is deliberately not required for Summary-unknown local Channels.
            return Ok(());
        };
        if summary != channel {
            return self.fail(DefinitionConsistencyError::ConflictingChannelDefinition);
        }
        Ok(())
    }

    fn observe_message(&mut self, channel_id: u16) -> Result<(), DefinitionConsistencyError> {
        self.messages_seen = match checked_increment(self.messages_seen) {
            Ok(messages_seen) => messages_seen,
            Err(error) => return self.fail(error),
        };
        if self.summary.channel(channel_id).is_none() {
            return self.fail(DefinitionConsistencyError::UnknownReferencedChannel);
        }
        Ok(())
    }

    fn begin_observation(&mut self) -> Result<(), DefinitionConsistencyError> {
        if let Some(error) = self.first_error {
            return Err(error);
        }
        self.observations_seen = match checked_increment(self.observations_seen) {
            Ok(observations_seen) => observations_seen,
            Err(error) => return self.fail(error),
        };
        Ok(())
    }

    fn fail(
        &mut self,
        error: DefinitionConsistencyError,
    ) -> Result<(), DefinitionConsistencyError> {
        Err(*self.first_error.get_or_insert(error))
    }

    pub(crate) fn finish(
        self,
    ) -> Result<ChunkDefinitionSemanticSummary<'summary, 'input>, DefinitionConsistencyError> {
        if let Some(error) = self.first_error {
            return Err(error);
        }
        Ok(ChunkDefinitionSemanticSummary {
            summary: self.summary,
            observations_seen: self.observations_seen,
            messages_seen: self.messages_seen,
        })
    }
}

/// Non-authoritative summary of the semantic events observed by one accumulator.
///
/// A prefix can produce this summary. It proves neither Chunk identity nor exact exhaustion and
/// must never authorize decode, partition publication, or Store mutation by itself.
pub(crate) struct ChunkDefinitionSemanticSummary<'summary, 'input> {
    summary: &'summary ValidatedSummaryDefinitions<'input>,
    observations_seen: u64,
    messages_seen: u64,
}

impl std::fmt::Debug for ChunkDefinitionSemanticSummary<'_, '_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ChunkDefinitionSemanticSummary")
            .field("summary", &"<validated definition capability>")
            .field("observations_seen", &self.observations_seen)
            .field("messages_seen", &self.messages_seen)
            .finish_non_exhaustive()
    }
}

impl<'summary, 'input> ChunkDefinitionSemanticSummary<'summary, 'input> {
    pub(crate) const fn summary(&self) -> &'summary ValidatedSummaryDefinitions<'input> {
        self.summary
    }

    pub(crate) const fn observations_seen(&self) -> u64 {
        self.observations_seen
    }

    pub(crate) const fn messages_seen(&self) -> u64 {
        self.messages_seen
    }
}
