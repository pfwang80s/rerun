//! Sealed physical-Chunk ownership, header validation, and semantic scanning.
//!
//! The complete path is deliberately production-disarmed.
//! A future Chrome adapter may provide an exact full-record body, but it cannot construct a read
//! identity, report Chunk metadata, or bypass the pending-header transition defined here.

#![allow(dead_code)]

use std::marker::PhantomData;
use std::num::NonZeroU64;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;
use re_log_types::TimeInt;

use crate::remote_decompression::{
    ChunkCompressionCodec, ChunkDecompressionBudget, ChunkDecompressionError,
    ExactCompressedChunkInput, ExactOutputChunk, PhysicalChunkReadIdentity,
    prepare_header_validated_compressed_chunk_input,
};
use crate::remote_summary::definitions::{
    CanonicalSchemaDefinition, ChunkDefinitionAccumulator, ChunkDefinitionEvent,
    DefinitionConsistencyError, NonDefinitionChunkRecord, SummaryDefinitionLookup,
    SummaryDefinitionProjectionRecord, ValidatedSummaryDefinitions,
};
use crate::remote_summary::physical_regions::{CanonicalPhysicalRegion, ValidatedPhysicalRegions};
use crate::remote_time::{RawMcapTime, canonicalize_raw_mcap_time};

const CHUNK_FIXED_HEADER_BYTES: usize = 8 + 8 + 8 + 4;
const CHUNK_COMPRESSED_SIZE_BYTES: usize = size_of::<u64>();
const MESSAGE_HEADER_BYTES: usize = 2 + 4 + 8 + 8;
const DEFINITION_ID_DOMAIN_LEN: usize = u16::MAX as usize + 1;
const MISSING_DEFINITION_RECORD: usize = usize::MAX;

static NEXT_PHYSICAL_CHUNK_SOURCE_GENERATION: AtomicU64 = AtomicU64::new(1);

/// The only checksum policy accepted by the remote physical-Chunk path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChunkChecksumPolicy {
    ValidateIfProvided,
}

/// Marker proving that no Chunk header metadata has yet been trusted.
#[derive(Debug)]
pub(crate) struct PendingHeaderValidation;

/// Marker proving that the exact full-record body matched the canonical descriptor.
#[derive(Debug)]
pub(crate) struct HeaderValidated;

#[derive(Debug)]
struct PhysicalChunkSourceGeneration(NonZeroU64);

#[derive(Debug)]
pub(crate) struct PhysicalChunkReadGeneration(NonZeroU64);

#[derive(Clone, Copy, Debug)]
pub(crate) struct PhysicalChunkScanLimits {
    max_records_per_chunk: u64,
    max_messages_per_chunk: u64,
    max_schema_records_per_chunk: u64,
    max_channel_records_per_chunk: u64,
    max_channel_metadata_entries_per_record: u64,
    max_channel_metadata_key_bytes: u64,
    max_channel_metadata_value_bytes: u64,
    max_nested_entries_per_chunk: u64,
    max_owned_field_bytes_per_chunk: u64,
    max_message_payload_bytes_per_chunk: u64,
    max_canonical_schemas: u64,
    max_canonical_channels: u64,
    max_result_retained_bytes: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PhysicalChunkScanBudgetUsage {
    active_authorities: u64,
    authority_slots: u64,
    authority_slot_bytes: u64,
    definition_projection_bytes: u64,
    active_scans: u64,
    active_nested_bytes: u64,
    retained_results: u64,
    retained_result_bytes: u64,
}

#[derive(Clone, Copy, Debug)]
struct PhysicalChunkScanBudgetCapacity {
    max_active_authorities: u64,
    max_authority_slots: u64,
    max_authority_slot_bytes: u64,
    max_definition_projection_bytes: u64,
    max_active_scans: u64,
    max_active_nested_bytes: u64,
    max_retained_results: u64,
    max_retained_result_bytes: u64,
}

struct PhysicalChunkScanBudgetState {
    limits: PhysicalChunkScanLimits,
    capacity: PhysicalChunkScanBudgetCapacity,
    usage: Mutex<PhysicalChunkScanBudgetUsage>,
}

/// One source/profile budget for authority slots, bounded scan work, and retained census data.
pub(crate) struct PhysicalChunkScanBudget {
    state: Arc<PhysicalChunkScanBudgetState>,
}

impl std::fmt::Debug for PhysicalChunkScanBudget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PhysicalChunkScanBudget")
            .field("limits", &self.state.limits)
            .field("capacity", &self.state.capacity)
            .finish_non_exhaustive()
    }
}

impl PhysicalChunkScanBudget {
    fn reserve_authority(
        &self,
        slots: u64,
        slot_bytes: u64,
        definition_projection_bytes: u64,
    ) -> Result<PhysicalChunkAuthorityReservation, PhysicalChunkValidationError> {
        let mut usage = self.state.usage.lock();
        let next = PhysicalChunkScanBudgetUsage {
            active_authorities: checked_add(usage.active_authorities, 1)?,
            authority_slots: checked_add(usage.authority_slots, slots)?,
            authority_slot_bytes: checked_add(usage.authority_slot_bytes, slot_bytes)?,
            definition_projection_bytes: checked_add(
                usage.definition_projection_bytes,
                definition_projection_bytes,
            )?,
            ..*usage
        };
        if next.active_authorities > self.state.capacity.max_active_authorities
            || next.authority_slots > self.state.capacity.max_authority_slots
            || next.authority_slot_bytes > self.state.capacity.max_authority_slot_bytes
            || next.definition_projection_bytes
                > self.state.capacity.max_definition_projection_bytes
        {
            return Err(PhysicalChunkValidationError::AuthorityReservationLimitExceeded);
        }
        *usage = next;
        drop(usage);
        Ok(PhysicalChunkAuthorityReservation {
            state: Arc::clone(&self.state),
            slots,
            slot_bytes,
            definition_projection_bytes,
        })
    }

    fn reserve_scan(
        &self,
        nested_bytes: u64,
        result_bytes: u64,
    ) -> Result<PhysicalChunkScanWorkReservation, PhysicalChunkValidationError> {
        let mut usage = self.state.usage.lock();
        let next = PhysicalChunkScanBudgetUsage {
            active_scans: checked_add(usage.active_scans, 1)?,
            active_nested_bytes: checked_add(usage.active_nested_bytes, nested_bytes)?,
            retained_results: checked_add(usage.retained_results, 1)?,
            retained_result_bytes: checked_add(usage.retained_result_bytes, result_bytes)?,
            ..*usage
        };
        if next.active_scans > self.state.capacity.max_active_scans
            || next.active_nested_bytes > self.state.capacity.max_active_nested_bytes
            || next.retained_results > self.state.capacity.max_retained_results
            || next.retained_result_bytes > self.state.capacity.max_retained_result_bytes
        {
            return Err(PhysicalChunkValidationError::ScanReservationLimitExceeded);
        }
        *usage = next;
        drop(usage);
        Ok(PhysicalChunkScanWorkReservation {
            state: Some(Arc::clone(&self.state)),
            nested_bytes,
            result_bytes,
        })
    }
}

struct PhysicalChunkAuthorityReservation {
    state: Arc<PhysicalChunkScanBudgetState>,
    slots: u64,
    slot_bytes: u64,
    definition_projection_bytes: u64,
}

impl Drop for PhysicalChunkAuthorityReservation {
    fn drop(&mut self) {
        let mut usage = self.state.usage.lock();
        usage.active_authorities = usage
            .active_authorities
            .checked_sub(1)
            .expect("a live physical-Chunk authority owns one authority claim");
        usage.authority_slots = usage
            .authority_slots
            .checked_sub(self.slots)
            .expect("a live physical-Chunk authority owns its exact slots");
        usage.authority_slot_bytes = usage
            .authority_slot_bytes
            .checked_sub(self.slot_bytes)
            .expect("a live physical-Chunk authority owns its exact slot bytes");
        usage.definition_projection_bytes = usage
            .definition_projection_bytes
            .checked_sub(self.definition_projection_bytes)
            .expect("a live physical-Chunk authority owns its exact definition projection bytes");
    }
}

struct PhysicalChunkScanWorkReservation {
    state: Option<Arc<PhysicalChunkScanBudgetState>>,
    nested_bytes: u64,
    result_bytes: u64,
}

impl PhysicalChunkScanWorkReservation {
    fn complete(mut self) -> PhysicalChunkScanResultReservation {
        let state = self
            .state
            .take()
            .expect("a live physical-Chunk scan owns one budget state");
        {
            let mut usage = state.usage.lock();
            usage.active_scans = usage
                .active_scans
                .checked_sub(1)
                .expect("a live physical-Chunk scan owns one active scan claim");
            usage.active_nested_bytes = usage
                .active_nested_bytes
                .checked_sub(self.nested_bytes)
                .expect("a live physical-Chunk scan owns its nested-byte claim");
        }
        PhysicalChunkScanResultReservation {
            state,
            result_bytes: self.result_bytes,
        }
    }
}

impl Drop for PhysicalChunkScanWorkReservation {
    fn drop(&mut self) {
        let Some(state) = self.state.take() else {
            return;
        };
        let mut usage = state.usage.lock();
        usage.active_scans = usage
            .active_scans
            .checked_sub(1)
            .expect("a live physical-Chunk scan owns one active scan claim");
        usage.active_nested_bytes = usage
            .active_nested_bytes
            .checked_sub(self.nested_bytes)
            .expect("a live physical-Chunk scan owns its nested-byte claim");
        usage.retained_results = usage
            .retained_results
            .checked_sub(1)
            .expect("a live physical-Chunk scan owns one pending result claim");
        usage.retained_result_bytes = usage
            .retained_result_bytes
            .checked_sub(self.result_bytes)
            .expect("a live physical-Chunk scan owns its pending result bytes");
    }
}

struct PhysicalChunkScanResultReservation {
    state: Arc<PhysicalChunkScanBudgetState>,
    result_bytes: u64,
}

impl Drop for PhysicalChunkScanResultReservation {
    fn drop(&mut self) {
        let mut usage = self.state.usage.lock();
        usage.retained_results = usage
            .retained_results
            .checked_sub(1)
            .expect("a retained physical-Chunk scan owns one result claim");
        usage.retained_result_bytes = usage
            .retained_result_bytes
            .checked_sub(self.result_bytes)
            .expect("a retained physical-Chunk scan owns its result bytes");
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PhysicalChunkSlot {
    Vacant { last_generation: u64 },
    Live { generation: NonZeroU64 },
}

#[derive(Clone, Copy)]
struct ChannelDefinitionProjectionSlot {
    record_index: usize,
    dense_ordinal: usize,
}

impl ChannelDefinitionProjectionSlot {
    const MISSING: Self = Self {
        record_index: MISSING_DEFINITION_RECORD,
        dense_ordinal: usize::MAX,
    };
}

/// Immutable O(1) Schema/Channel lookup owned and budgeted by one source authority.
struct CanonicalDefinitionProjection {
    schema_record_indices: Box<[usize]>,
    channel_slots: Box<[ChannelDefinitionProjectionSlot]>,
    canonical_channel_ids: Box<[u16]>,
    #[cfg(test)]
    lookup_calls: std::sync::atomic::AtomicU64,
}

impl CanonicalDefinitionProjection {
    const MAX_SLOT_LOOKUPS_PER_QUERY: u64 = 1;

    fn retained_bytes(canonical_channels: u64) -> Result<u64, PhysicalChunkValidationError> {
        let schema_slots = (DEFINITION_ID_DOMAIN_LEN as u64)
            .checked_mul(size_of::<usize>() as u64)
            .ok_or(PhysicalChunkValidationError::ArithmeticOverflow)?;
        let channel_slots = (DEFINITION_ID_DOMAIN_LEN as u64)
            .checked_mul(size_of::<ChannelDefinitionProjectionSlot>() as u64)
            .ok_or(PhysicalChunkValidationError::ArithmeticOverflow)?;
        let channel_ids = canonical_channels
            .checked_mul(size_of::<u16>() as u64)
            .ok_or(PhysicalChunkValidationError::ArithmeticOverflow)?;
        checked_add(checked_add(schema_slots, channel_slots)?, channel_ids)
    }

    fn build(
        definitions: &ValidatedSummaryDefinitions<'_>,
        canonical_schemas: usize,
        canonical_channels: usize,
    ) -> Result<Self, PhysicalChunkValidationError> {
        let mut schema_record_indices = Vec::new();
        schema_record_indices
            .try_reserve_exact(DEFINITION_ID_DOMAIN_LEN)
            .map_err(|_allocation| PhysicalChunkValidationError::ProjectionAllocationFailed)?;
        schema_record_indices.resize(DEFINITION_ID_DOMAIN_LEN, MISSING_DEFINITION_RECORD);

        let mut channel_slots = Vec::new();
        channel_slots
            .try_reserve_exact(DEFINITION_ID_DOMAIN_LEN)
            .map_err(|_allocation| PhysicalChunkValidationError::ProjectionAllocationFailed)?;
        channel_slots.resize(
            DEFINITION_ID_DOMAIN_LEN,
            ChannelDefinitionProjectionSlot::MISSING,
        );

        let mut canonical_channel_ids = Vec::new();
        canonical_channel_ids
            .try_reserve_exact(canonical_channels)
            .map_err(|_allocation| PhysicalChunkValidationError::ProjectionAllocationFailed)?;
        let mut schemas_seen = 0_usize;
        for record in definitions.projection_records() {
            match record {
                SummaryDefinitionProjectionRecord::Schema { id, record_index } => {
                    let slot = &mut schema_record_indices[usize::from(id)];
                    if *slot == MISSING_DEFINITION_RECORD {
                        if schemas_seen == canonical_schemas {
                            return Err(PhysicalChunkValidationError::ProjectionCountMismatch);
                        }
                        *slot = record_index;
                        schemas_seen = schemas_seen
                            .checked_add(1)
                            .ok_or(PhysicalChunkValidationError::ArithmeticOverflow)?;
                    }
                }
                SummaryDefinitionProjectionRecord::Channel { id, record_index } => {
                    let slot = &mut channel_slots[usize::from(id)];
                    if slot.record_index == MISSING_DEFINITION_RECORD {
                        if canonical_channel_ids.len() == canonical_channels {
                            return Err(PhysicalChunkValidationError::ProjectionCountMismatch);
                        }
                        let dense_ordinal = canonical_channel_ids.len();
                        canonical_channel_ids.push(id);
                        *slot = ChannelDefinitionProjectionSlot {
                            record_index,
                            dense_ordinal,
                        };
                    }
                }
            }
        }
        if schemas_seen != canonical_schemas || canonical_channel_ids.len() != canonical_channels {
            return Err(PhysicalChunkValidationError::ProjectionCountMismatch);
        }
        Ok(Self {
            schema_record_indices: schema_record_indices.into_boxed_slice(),
            channel_slots: channel_slots.into_boxed_slice(),
            canonical_channel_ids: canonical_channel_ids.into_boxed_slice(),
            #[cfg(test)]
            lookup_calls: std::sync::atomic::AtomicU64::new(0),
        })
    }

    fn schema_record_index(&self, id: u16) -> Option<usize> {
        self.record_lookup();
        let record_index = self.schema_record_indices[usize::from(id)];
        (record_index != MISSING_DEFINITION_RECORD).then_some(record_index)
    }

    fn channel_slot(&self, id: u16) -> Option<ChannelDefinitionProjectionSlot> {
        self.record_lookup();
        let slot = self.channel_slots[usize::from(id)];
        (slot.record_index != MISSING_DEFINITION_RECORD).then_some(slot)
    }

    fn record_lookup(&self) {
        #[cfg(test)]
        self.lookup_calls
            .fetch_add(Self::MAX_SLOT_LOOKUPS_PER_QUERY, Ordering::Relaxed);
    }

    #[cfg(test)]
    fn lookup_calls(&self) -> u64 {
        self.lookup_calls.load(Ordering::Relaxed)
    }
}

struct ProjectedSummaryDefinitions<'projection, 'definitions, 'input> {
    projection: &'projection CanonicalDefinitionProjection,
    definitions: &'definitions ValidatedSummaryDefinitions<'input>,
}

impl SummaryDefinitionLookup for ProjectedSummaryDefinitions<'_, '_, '_> {
    fn schema(&self, id: u16) -> Option<CanonicalSchemaDefinition<'_>> {
        self.definitions
            .schema_at_record(self.projection.schema_record_index(id)?)
    }

    fn channel(&self, id: u16) -> Option<&mcap::records::Channel> {
        self.definitions
            .channel_at_record(self.projection.channel_slot(id)?.record_index)
    }
}

struct PhysicalChunkAuthorityState {
    open: bool,
    slots: Vec<PhysicalChunkSlot>,
}

struct PhysicalChunkSourceEvidence<'a> {
    physical: ValidatedPhysicalRegions<'a>,
    definition_projection: CanonicalDefinitionProjection,
    source_generation: PhysicalChunkSourceGeneration,
    decompression_budget: ChunkDecompressionBudget,
    scan_budget: PhysicalChunkScanBudget,
    state: Mutex<PhysicalChunkAuthorityState>,
    _authority_reservation: PhysicalChunkAuthorityReservation,
}

/// The unique source-scoped owner of MCAP-021 → 020 → 019 physical evidence.
pub(crate) struct PhysicalChunkSourceAuthority<'a> {
    shared: Arc<PhysicalChunkSourceEvidence<'a>>,
}

impl std::fmt::Debug for PhysicalChunkSourceAuthority<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PhysicalChunkSourceAuthority")
            .field("physical", &"<sealed MCAP-021 evidence>")
            .field("source_generation", &"<opaque>")
            .field(
                "canonical_chunks",
                &self.shared.physical.canonical_chunk_count(),
            )
            .finish_non_exhaustive()
    }
}

/// A short-lived, authority-borrowed canonical position selector.
pub(crate) struct PhysicalChunkReadSelector<'authority, 'source> {
    authority: &'authority PhysicalChunkSourceAuthority<'source>,
    canonical_ordinal: usize,
}

struct PhysicalChunkReadLeaseCore<'a> {
    shared: Arc<PhysicalChunkSourceEvidence<'a>>,
    canonical_ordinal: usize,
    source_generation: NonZeroU64,
    read_generation: NonZeroU64,
    checksum_policy: ChunkChecksumPolicy,
}

/// Move-only ownership of one canonical ordinal's live read claim.
pub(crate) struct PhysicalChunkReadLease<'a, State> {
    core: Option<PhysicalChunkReadLeaseCore<'a>>,
    _state: PhantomData<State>,
}

impl<State> std::fmt::Debug for PhysicalChunkReadLease<'_, State> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PhysicalChunkReadLease")
            .field("identity", &"<opaque move-only owner>")
            .finish_non_exhaustive()
    }
}

impl<'a> PhysicalChunkSourceAuthority<'a> {
    pub(crate) fn new(
        physical: ValidatedPhysicalRegions<'a>,
        decompression_budget: ChunkDecompressionBudget,
        scan_budget: PhysicalChunkScanBudget,
    ) -> Result<Self, PhysicalChunkValidationError> {
        let definitions = physical.definitions();
        let canonical_schemas = definitions.canonical_schema_count();
        let canonical_channels = definitions.canonical_channel_count();
        let limits = scan_budget.state.limits;
        if canonical_schemas > limits.max_canonical_schemas {
            return Err(PhysicalChunkValidationError::CanonicalSchemaLimitExceeded);
        }
        if canonical_channels > limits.max_canonical_channels {
            return Err(PhysicalChunkValidationError::CanonicalChannelLimitExceeded);
        }
        let canonical_schema_capacity = usize::try_from(canonical_schemas)
            .map_err(|_overflow| PhysicalChunkValidationError::ArithmeticOverflow)?;
        let canonical_channel_capacity = usize::try_from(canonical_channels)
            .map_err(|_overflow| PhysicalChunkValidationError::ArithmeticOverflow)?;
        let definition_projection_bytes =
            CanonicalDefinitionProjection::retained_bytes(canonical_channels)?;

        for region in physical.regions() {
            validate_expected_raw_extent(region.raw_descriptor())?;
        }

        let canonical_chunks = u64::try_from(physical.canonical_chunk_count())
            .map_err(|_overflow| PhysicalChunkValidationError::ArithmeticOverflow)?;
        let slot_bytes = canonical_chunks
            .checked_mul(size_of::<PhysicalChunkSlot>() as u64)
            .ok_or(PhysicalChunkValidationError::ArithmeticOverflow)?;
        let reservation = scan_budget.reserve_authority(
            canonical_chunks,
            slot_bytes,
            definition_projection_bytes,
        )?;
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(physical.canonical_chunk_count())
            .map_err(|_allocation| PhysicalChunkValidationError::AuthorityAllocationFailed)?;
        slots.resize(
            physical.canonical_chunk_count(),
            PhysicalChunkSlot::Vacant { last_generation: 0 },
        );
        let definition_projection = CanonicalDefinitionProjection::build(
            definitions,
            canonical_schema_capacity,
            canonical_channel_capacity,
        )?;

        let source_generation = allocate_source_generation()?;
        Ok(Self {
            shared: Arc::new(PhysicalChunkSourceEvidence {
                physical,
                definition_projection,
                source_generation: PhysicalChunkSourceGeneration(source_generation),
                decompression_budget,
                scan_budget,
                state: Mutex::new(PhysicalChunkAuthorityState { open: true, slots }),
                _authority_reservation: reservation,
            }),
        })
    }

    pub(crate) fn select(
        &self,
        canonical_ordinal: usize,
    ) -> Result<PhysicalChunkReadSelector<'_, 'a>, PhysicalChunkValidationError> {
        if self.shared.physical.region(canonical_ordinal).is_none() {
            return Err(PhysicalChunkValidationError::UnknownCanonicalOrdinal);
        }
        Ok(PhysicalChunkReadSelector {
            authority: self,
            canonical_ordinal,
        })
    }

    #[expect(
        clippy::needless_pass_by_value,
        reason = "issuing consumes the authority-borrowed selector so it cannot be replayed"
    )]
    pub(crate) fn issue(
        &self,
        selector: PhysicalChunkReadSelector<'_, 'a>,
    ) -> Result<PhysicalChunkReadLease<'a, PendingHeaderValidation>, PhysicalChunkValidationError>
    {
        if !Arc::ptr_eq(&self.shared, &selector.authority.shared) {
            return Err(PhysicalChunkValidationError::SelectorAuthorityMismatch);
        }
        let mut state = self.shared.state.lock();
        if !state.open {
            return Err(PhysicalChunkValidationError::SourceClosed);
        }
        let slot = state
            .slots
            .get_mut(selector.canonical_ordinal)
            .ok_or(PhysicalChunkValidationError::UnknownCanonicalOrdinal)?;
        let last_generation = match *slot {
            PhysicalChunkSlot::Vacant { last_generation } => last_generation,
            PhysicalChunkSlot::Live { .. } => {
                return Err(PhysicalChunkValidationError::DuplicateLiveRead);
            }
        };
        let generation = NonZeroU64::new(
            last_generation
                .checked_add(1)
                .ok_or(PhysicalChunkValidationError::ReadGenerationExhausted)?,
        )
        .ok_or(PhysicalChunkValidationError::ReadGenerationExhausted)?;
        *slot = PhysicalChunkSlot::Live { generation };
        drop(state);

        Ok(PhysicalChunkReadLease {
            core: Some(PhysicalChunkReadLeaseCore {
                shared: Arc::clone(&self.shared),
                canonical_ordinal: selector.canonical_ordinal,
                source_generation: self.shared.source_generation.0,
                read_generation: generation,
                checksum_policy: ChunkChecksumPolicy::ValidateIfProvided,
            }),
            _state: PhantomData,
        })
    }

    pub(crate) fn close(&self) {
        self.shared.state.lock().open = false;
    }
}

impl Drop for PhysicalChunkSourceAuthority<'_> {
    fn drop(&mut self) {
        self.close();
    }
}

impl<'a, State> PhysicalChunkReadLease<'a, State> {
    fn core(&self) -> &PhysicalChunkReadLeaseCore<'a> {
        self.core
            .as_ref()
            .expect("a live physical-Chunk lease retains its core")
    }

    pub(super) fn ensure_current(&self) -> Result<(), PhysicalChunkValidationError> {
        let core = self.core();
        if core.source_generation != core.shared.source_generation.0 {
            return Err(PhysicalChunkValidationError::StaleSourceGeneration);
        }
        let state = core.shared.state.lock();
        if !state.open {
            return Err(PhysicalChunkValidationError::SourceClosed);
        }
        match state.slots.get(core.canonical_ordinal) {
            Some(PhysicalChunkSlot::Live { generation }) if *generation == core.read_generation => {
                Ok(())
            }
            _ => Err(PhysicalChunkValidationError::StaleReadGeneration),
        }
    }

    fn region(&self) -> CanonicalPhysicalRegion<'_> {
        self.core()
            .shared
            .physical
            .region(self.core().canonical_ordinal)
            .expect("a live lease was issued from a canonical physical region")
    }

    fn transition<Next>(
        mut self,
    ) -> Result<PhysicalChunkReadLease<'a, Next>, PhysicalChunkValidationError> {
        self.ensure_current()?;
        Ok(PhysicalChunkReadLease {
            core: self.core.take(),
            _state: PhantomData,
        })
    }

    pub(super) fn decompression_budget(&self) -> &ChunkDecompressionBudget {
        &self.core().shared.decompression_budget
    }

    fn scan_budget(&self) -> &PhysicalChunkScanBudget {
        &self.core().shared.scan_budget
    }
}

impl<State> Drop for PhysicalChunkReadLease<'_, State> {
    fn drop(&mut self) {
        let Some(core) = self.core.take() else {
            return;
        };
        let mut state = core.shared.state.lock();
        let Some(slot) = state.slots.get_mut(core.canonical_ordinal) else {
            return;
        };
        if matches!(
            *slot,
            PhysicalChunkSlot::Live { generation } if generation == core.read_generation
        ) {
            *slot = PhysicalChunkSlot::Vacant {
                last_generation: core.read_generation.get(),
            };
        }
    }
}

impl<'a> PhysicalChunkReadIdentity<'a> {
    fn lease(
        &self,
    ) -> Result<&PhysicalChunkReadLease<'a, HeaderValidated>, PhysicalChunkValidationError> {
        match self {
            Self::Lease(lease) => Ok(lease),
            #[cfg(test)]
            Self::CodecUnit { .. } => Err(PhysicalChunkValidationError::MissingPhysicalReadLease),
        }
    }
}

/// Exact full-record bytes joined to the only pending read lease that may interpret them.
struct ExactPhysicalChunkRecord<'a> {
    bytes: Option<Box<[u8]>>,
    lease: Option<PhysicalChunkReadLease<'a, PendingHeaderValidation>>,
}

impl Drop for ExactPhysicalChunkRecord<'_> {
    fn drop(&mut self) {
        drop(self.bytes.take());
        drop(self.lease.take());
    }
}

/// A sealed result of allocation-free Chunk envelope and header validation.
pub(crate) struct HeaderValidatedPhysicalChunkRead<'a> {
    full_record: Option<Box<[u8]>>,
    lease: Option<PhysicalChunkReadLease<'a, HeaderValidated>>,
    payload: Range<usize>,
    codec: ChunkCompressionCodec,
    compressed_size: u64,
    uncompressed_size: u64,
    declared_uncompressed_crc: u32,
}

impl std::fmt::Debug for HeaderValidatedPhysicalChunkRead<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HeaderValidatedPhysicalChunkRead")
            .field("identity", &"<opaque move-only owner>")
            .field("payload", &"<private validated subrange>")
            .field("codec", &self.codec)
            .field("declared_uncompressed_crc", &"<header-derived>")
            .finish_non_exhaustive()
    }
}

impl Drop for HeaderValidatedPhysicalChunkRead<'_> {
    fn drop(&mut self) {
        drop(self.full_record.take());
        drop(self.lease.take());
    }
}

fn validate_physical_chunk_header(
    mut input: ExactPhysicalChunkRecord<'_>,
) -> Result<HeaderValidatedPhysicalChunkRead<'_>, PhysicalChunkValidationError> {
    let lease = input
        .lease
        .take()
        .expect("an exact physical record retains its pending lease");
    lease.ensure_current()?;
    let bytes = input
        .bytes
        .take()
        .expect("an exact physical record retains its backing");
    let region = lease.region();
    let descriptor = region.raw_descriptor();
    let expected_record_bytes = region
        .chunk_range()
        .end
        .checked_sub(region.chunk_range().start)
        .ok_or(PhysicalChunkValidationError::ArithmeticOverflow)?;
    let actual_record_bytes = u64::try_from(bytes.len())
        .map_err(|_overflow| PhysicalChunkValidationError::ArithmeticOverflow)?;
    if actual_record_bytes != expected_record_bytes
        || actual_record_bytes != descriptor.chunk_length
    {
        return Err(PhysicalChunkValidationError::FullRecordLengthMismatch);
    }

    let mut record = BorrowedCursor::new(&bytes);
    if record.u8()? != mcap::records::op::CHUNK {
        return Err(PhysicalChunkValidationError::WrongChunkOpcode);
    }
    let declared_body_len = record.u64()?;
    if declared_body_len
        .checked_add(crate::RECORD_HEADER_LEN as u64)
        .ok_or(PhysicalChunkValidationError::ArithmeticOverflow)?
        != actual_record_bytes
    {
        return Err(PhysicalChunkValidationError::InvalidRecordEnvelope);
    }
    let body = record.take_exact(
        usize::try_from(declared_body_len)
            .map_err(|_overflow| PhysicalChunkValidationError::ArithmeticOverflow)?,
    )?;
    record.finish()?;

    let mut header = BorrowedCursor::new(body);
    let message_start_time = header.u64()?;
    let message_end_time = header.u64()?;
    let uncompressed_size = header.u64()?;
    let declared_uncompressed_crc = header.u32()?;
    let compression = header.string()?;
    let compressed_size = header.u64()?;
    let payload_start_in_body = header.position();
    let payload_len = usize::try_from(compressed_size)
        .map_err(|_overflow| PhysicalChunkValidationError::ArithmeticOverflow)?;
    header.take_exact(payload_len)?;
    header.finish()?;

    if message_start_time != descriptor.message_start_time
        || message_end_time != descriptor.message_end_time
    {
        return Err(PhysicalChunkValidationError::HeaderExtentMismatch);
    }
    if compression != descriptor.compression {
        return Err(PhysicalChunkValidationError::HeaderCompressionMismatch);
    }
    if compressed_size != descriptor.compressed_size
        || uncompressed_size != descriptor.uncompressed_size
    {
        return Err(PhysicalChunkValidationError::HeaderSizeMismatch);
    }
    if lease.core().checksum_policy != ChunkChecksumPolicy::ValidateIfProvided {
        return Err(PhysicalChunkValidationError::ChecksumPolicyMismatch);
    }
    validate_expected_raw_extent(descriptor)?;
    lease.ensure_current()?;

    let payload_start = crate::RECORD_HEADER_LEN
        .checked_add(payload_start_in_body)
        .ok_or(PhysicalChunkValidationError::ArithmeticOverflow)?;
    let payload_end = payload_start
        .checked_add(payload_len)
        .ok_or(PhysicalChunkValidationError::ArithmeticOverflow)?;
    if payload_end != bytes.len() {
        return Err(PhysicalChunkValidationError::CompressedPayloadRangeMismatch);
    }
    let codec = ChunkCompressionCodec::from_mcap_name(compression)
        .unwrap_or(ChunkCompressionCodec::Unsupported);
    let lease = lease.transition::<HeaderValidated>()?;
    Ok(HeaderValidatedPhysicalChunkRead {
        full_record: Some(bytes),
        lease: Some(lease),
        payload: payload_start..payload_end,
        codec,
        compressed_size,
        uncompressed_size,
        declared_uncompressed_crc,
    })
}

#[cfg(test)]
fn install_exact_physical_chunk_record_for_test(
    lease: PhysicalChunkReadLease<'_, PendingHeaderValidation>,
    bytes: Box<[u8]>,
) -> Result<HeaderValidatedPhysicalChunkRead<'_>, PhysicalChunkValidationError> {
    validate_physical_chunk_header(ExactPhysicalChunkRecord {
        bytes: Some(bytes),
        lease: Some(lease),
    })
}

#[cfg(test)]
fn install_header_validated_payload_for_test(
    validated: HeaderValidatedPhysicalChunkRead<'_>,
) -> Result<ExactCompressedChunkInput<'_>, PhysicalChunkValidationError> {
    install_header_validated_payload_with_hook_for_test(validated, || {})
}

#[cfg(test)]
fn install_header_validated_payload_with_hook_for_test(
    mut validated: HeaderValidatedPhysicalChunkRead<'_>,
    after_copy: impl FnOnce(),
) -> Result<ExactCompressedChunkInput<'_>, PhysicalChunkValidationError> {
    let lease = validated
        .lease
        .take()
        .expect("a header-validated owner retains its lease");
    lease.ensure_current()?;
    let prepared = prepare_header_validated_compressed_chunk_input(
        lease,
        validated.codec,
        validated.compressed_size,
        validated.uncompressed_size,
        validated.declared_uncompressed_crc,
    )?;
    let full_record = validated
        .full_record
        .take()
        .expect("a header-validated owner retains its exact full record");
    let payload = full_record
        .get(validated.payload.clone())
        .ok_or(PhysicalChunkValidationError::CompressedPayloadRangeMismatch)?;
    let mut destination = Vec::new();
    destination
        .try_reserve_exact(payload.len())
        .map_err(|_allocation| PhysicalChunkValidationError::PayloadAllocationFailed)?;
    destination.extend_from_slice(payload);
    after_copy();
    drop(full_record);
    prepared
        .install(destination.into_boxed_slice())
        .map_err(Into::into)
}

#[derive(Clone, Copy, Debug, Default)]
struct ScanPreflight {
    records_seen: u64,
    messages_seen: u64,
    schema_records_seen: u64,
    channel_records_seen: u64,
    nested_entries: u64,
    owned_field_bytes: u64,
    message_payload_bytes: u64,
    raw_min: Option<u64>,
    raw_max: Option<u64>,
    canonical_min: Option<TimeInt>,
    canonical_max: Option<TimeInt>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ChannelSemanticCensus {
    channel_id: u16,
    message_count: u64,
    payload_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ValidatedPhysicalChunkExtent {
    KnownEmpty,
    NonEmpty {
        raw_start: u64,
        raw_end: u64,
        canonical_start: TimeInt,
        canonical_end: TimeInt,
    },
}

/// A sealed semantic result with no parser, dispatch, registration, or terminal authority.
pub(crate) struct ValidatedPhysicalChunkScan<'a> {
    // Keep retained census backing and its accounting before the lease-owning output.
    // Rust drops fields in declaration order, while `ExactOutputChunk::drop` then releases its
    // decompressed backing and accounting before releasing the physical read lease.
    channels: Vec<ChannelSemanticCensus>,
    _reservation: PhysicalChunkScanResultReservation,
    output: ExactOutputChunk<'a>,
    records_seen: u64,
    messages_seen: u64,
    message_payload_bytes: u64,
    extent: ValidatedPhysicalChunkExtent,
}

impl std::fmt::Debug for ValidatedPhysicalChunkScan<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ValidatedPhysicalChunkScan")
            .field("identity", &"<opaque lease-owning output>")
            .field("records_seen", &self.records_seen)
            .field("messages_seen", &self.messages_seen)
            .field("message_payload_bytes", &self.message_payload_bytes)
            .field("channels", &self.channels.len())
            .field("extent", &self.extent)
            .finish_non_exhaustive()
    }
}

impl ValidatedPhysicalChunkScan<'_> {
    pub(crate) fn is_current(&self) -> bool {
        self.output
            .identity()
            .lease()
            .and_then(PhysicalChunkReadLease::ensure_current)
            .is_ok()
    }
}

pub(crate) struct PhysicalChunkScanCacheEntry<'a> {
    inner: Arc<ValidatedPhysicalChunkScan<'a>>,
}

#[derive(Clone)]
pub(crate) struct PhysicalChunkScanConsumer<'a> {
    inner: Arc<ValidatedPhysicalChunkScan<'a>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PhysicalChunkCacheEviction {
    Reclaimed,
    PendingReclaim,
}

impl<'a> PhysicalChunkScanCacheEntry<'a> {
    pub(crate) fn new(
        scan: ValidatedPhysicalChunkScan<'a>,
    ) -> Result<Self, PhysicalChunkValidationError> {
        let (shared, canonical_ordinal, source_generation, read_generation) = {
            let lease = scan.output.identity().lease()?;
            let core = lease.core();
            (
                Arc::clone(&core.shared),
                core.canonical_ordinal,
                core.source_generation,
                core.read_generation,
            )
        };
        if source_generation != shared.source_generation.0 {
            return Err(PhysicalChunkValidationError::StaleSourceGeneration);
        }
        let state = shared.state.lock();
        if !state.open {
            return Err(PhysicalChunkValidationError::SourceClosed);
        }
        if !matches!(
            state.slots.get(canonical_ordinal),
            Some(PhysicalChunkSlot::Live { generation }) if *generation == read_generation
        ) {
            return Err(PhysicalChunkValidationError::StaleReadGeneration);
        }
        let inner = Arc::new(scan);
        drop(state);
        Ok(Self { inner })
    }

    pub(crate) fn consumer(&self) -> PhysicalChunkScanConsumer<'a> {
        PhysicalChunkScanConsumer {
            inner: Arc::clone(&self.inner),
        }
    }

    pub(crate) fn evict(self) -> PhysicalChunkCacheEviction {
        let outcome = if Arc::strong_count(&self.inner) == 1 {
            PhysicalChunkCacheEviction::Reclaimed
        } else {
            PhysicalChunkCacheEviction::PendingReclaim
        };
        drop(self);
        outcome
    }
}

impl PhysicalChunkScanConsumer<'_> {
    pub(crate) fn is_current(&self) -> bool {
        self.inner.is_current()
    }
}

pub(crate) fn scan_decompressed_physical_chunk(
    output: ExactOutputChunk<'_>,
) -> Result<ValidatedPhysicalChunkScan<'_>, PhysicalChunkValidationError> {
    let lease = output.identity().lease()?;
    lease.ensure_current()?;
    if !output.has_matching_lease_profile() {
        return Err(PhysicalChunkValidationError::DecompressionProfileMismatch);
    }
    let definitions = lease.core().shared.physical.definitions();
    let definition_projection = &lease.core().shared.definition_projection;
    let projected_definitions = ProjectedSummaryDefinitions {
        projection: definition_projection,
        definitions,
    };
    let limits = lease.scan_budget().state.limits;
    let preflight =
        preflight_uncompressed_records(output.bytes(), &projected_definitions, &limits)?;
    let canonical_channels = u64::try_from(definition_projection.canonical_channel_ids.len())
        .map_err(|_overflow| PhysicalChunkValidationError::ArithmeticOverflow)?;
    let result_bytes = canonical_channels
        .checked_mul(size_of::<ChannelSemanticCensus>() as u64)
        .ok_or(PhysicalChunkValidationError::ArithmeticOverflow)?;
    if result_bytes > limits.max_result_retained_bytes {
        return Err(PhysicalChunkValidationError::ResultRetainedByteLimitExceeded);
    }
    let work_reservation = lease
        .scan_budget()
        .reserve_scan(preflight.owned_field_bytes, result_bytes)?;

    let channel_capacity = usize::try_from(canonical_channels)
        .map_err(|_overflow| PhysicalChunkValidationError::ArithmeticOverflow)?;
    let mut channels = Vec::new();
    channels
        .try_reserve_exact(channel_capacity)
        .map_err(|_allocation| PhysicalChunkValidationError::ResultAllocationFailed)?;
    for &channel_id in &definition_projection.canonical_channel_ids {
        channels.push(ChannelSemanticCensus {
            channel_id,
            message_count: 0,
            payload_bytes: 0,
        });
    }
    if channels.len() != channel_capacity {
        return Err(PhysicalChunkValidationError::CanonicalChannelProjectionMismatch);
    }

    let mut accumulator = ChunkDefinitionAccumulator::new(&projected_definitions);
    let mut records = RecordSequence::new(output.bytes());
    let mut materialized_metadata_entries = 0_u64;
    while let Some(record) = records.next()? {
        match record.opcode {
            mcap::records::op::SCHEMA => {
                let parsed = mcap::parse_record(record.opcode, record.body)
                    .map_err(|_error| PhysicalChunkValidationError::RecordBodyInvalid)?;
                let mcap::records::Record::Schema { header, data } = parsed else {
                    return Err(PhysicalChunkValidationError::RecordBodyInvalid);
                };
                accumulator.observe(ChunkDefinitionEvent::Schema {
                    header: &header,
                    data: data.as_ref(),
                })?;
            }
            mcap::records::op::CHANNEL => {
                // Re-read the immutable encoded count linearly, then compare it with the bounded
                // parser's map cardinality. The borrowed pass has already established every
                // structural and UTF-8 invariant consumed by the parser, so the parser's only
                // remaining data-dependent Channel failure is its duplicate-map-key rejection.
                let encoded = preflight_channel(record.body, &projected_definitions, &limits)?;
                let parsed = mcap::parse_record(record.opcode, record.body)
                    .map_err(|_error| PhysicalChunkValidationError::DuplicateChannelMetadataKey)?;
                let mcap::records::Record::Channel(channel) = parsed else {
                    return Err(PhysicalChunkValidationError::RecordBodyInvalid);
                };
                let parsed_entries = u64::try_from(channel.metadata.len())
                    .map_err(|_overflow| PhysicalChunkValidationError::ArithmeticOverflow)?;
                if parsed_entries != encoded.entries {
                    return Err(PhysicalChunkValidationError::DuplicateChannelMetadataKey);
                }
                materialized_metadata_entries =
                    checked_add(materialized_metadata_entries, encoded.entries)?;
                accumulator.observe(ChunkDefinitionEvent::Channel(&channel))?;
            }
            mcap::records::op::MESSAGE => {
                let message = parse_message_observation(record.body)?;
                accumulator.observe(ChunkDefinitionEvent::Message {
                    channel_id: message.channel_id,
                })?;
                let channel_slot = definition_projection
                    .channel_slot(message.channel_id)
                    .ok_or(PhysicalChunkValidationError::UnknownMessageChannel)?;
                let channel = channels
                    .get_mut(channel_slot.dense_ordinal)
                    .ok_or(PhysicalChunkValidationError::CanonicalChannelProjectionMismatch)?;
                if channel.channel_id != message.channel_id {
                    return Err(PhysicalChunkValidationError::CanonicalChannelProjectionMismatch);
                }
                channel.message_count = checked_add(channel.message_count, 1)?;
                channel.payload_bytes = checked_add(channel.payload_bytes, message.payload_bytes)?;
            }
            opcode => {
                accumulator.observe(ChunkDefinitionEvent::Other(
                    NonDefinitionChunkRecord::try_from_opcode(opcode)?,
                ))?;
            }
        }
    }
    let semantic = accumulator.finish()?;
    if semantic.messages_seen() != preflight.messages_seen
        || semantic.observations_seen() != preflight.records_seen
        || materialized_metadata_entries != preflight.nested_entries
    {
        return Err(PhysicalChunkValidationError::SemanticCensusMismatch);
    }
    lease.ensure_current()?;
    let extent = validate_actual_extent(&lease.region(), preflight)?;
    Ok(ValidatedPhysicalChunkScan {
        channels,
        _reservation: work_reservation.complete(),
        output,
        records_seen: preflight.records_seen,
        messages_seen: preflight.messages_seen,
        message_payload_bytes: preflight.message_payload_bytes,
        extent,
    })
}

fn preflight_uncompressed_records(
    bytes: &[u8],
    projection: &ProjectedSummaryDefinitions<'_, '_, '_>,
    limits: &PhysicalChunkScanLimits,
) -> Result<ScanPreflight, PhysicalChunkValidationError> {
    let mut census = ScanPreflight::default();
    let mut records = RecordSequence::new(bytes);
    while let Some(record) = records.next()? {
        census.records_seen = checked_increment_limited(
            census.records_seen,
            limits.max_records_per_chunk,
            PhysicalChunkValidationError::RecordLimitExceeded,
        )?;
        match record.opcode {
            0 => return Err(PhysicalChunkValidationError::InvalidRecordOpcode),
            mcap::records::op::SCHEMA => {
                census.schema_records_seen = checked_increment_limited(
                    census.schema_records_seen,
                    limits.max_schema_records_per_chunk,
                    PhysicalChunkValidationError::SchemaRecordLimitExceeded,
                )?;
                let nested = preflight_schema(record.body, projection)?;
                census.owned_field_bytes = checked_add_limited(
                    census.owned_field_bytes,
                    nested.owned_field_bytes,
                    limits.max_owned_field_bytes_per_chunk,
                    PhysicalChunkValidationError::OwnedFieldByteLimitExceeded,
                )?;
            }
            mcap::records::op::CHANNEL => {
                census.channel_records_seen = checked_increment_limited(
                    census.channel_records_seen,
                    limits.max_channel_records_per_chunk,
                    PhysicalChunkValidationError::ChannelRecordLimitExceeded,
                )?;
                let nested = preflight_channel(record.body, projection, limits)?;
                census.nested_entries = checked_add_limited(
                    census.nested_entries,
                    nested.entries,
                    limits.max_nested_entries_per_chunk,
                    PhysicalChunkValidationError::NestedEntryLimitExceeded,
                )?;
                census.owned_field_bytes = checked_add_limited(
                    census.owned_field_bytes,
                    nested.owned_field_bytes,
                    limits.max_owned_field_bytes_per_chunk,
                    PhysicalChunkValidationError::OwnedFieldByteLimitExceeded,
                )?;
            }
            mcap::records::op::MESSAGE => {
                census.messages_seen = checked_increment_limited(
                    census.messages_seen,
                    limits.max_messages_per_chunk,
                    PhysicalChunkValidationError::MessageLimitExceeded,
                )?;
                let message = parse_message_observation(record.body)?;
                let channel = projection
                    .channel(message.channel_id)
                    .ok_or(PhysicalChunkValidationError::UnknownMessageChannel)?;
                if channel.schema_id != 0 && projection.schema(channel.schema_id).is_none() {
                    return Err(PhysicalChunkValidationError::MissingMessageSchema);
                }
                census.message_payload_bytes = checked_add_limited(
                    census.message_payload_bytes,
                    message.payload_bytes,
                    limits.max_message_payload_bytes_per_chunk,
                    PhysicalChunkValidationError::MessagePayloadByteLimitExceeded,
                )?;
                let canonical = canonicalize_raw_mcap_time(RawMcapTime::new(message.log_time))
                    .map_err(|_error| PhysicalChunkValidationError::InvalidTemporalValue)?;
                census.raw_min = Some(
                    census
                        .raw_min
                        .map_or(message.log_time, |min| min.min(message.log_time)),
                );
                census.raw_max = Some(
                    census
                        .raw_max
                        .map_or(message.log_time, |max| max.max(message.log_time)),
                );
                census.canonical_min = Some(
                    census
                        .canonical_min
                        .map_or(canonical, |min| min.min(canonical)),
                );
                census.canonical_max = Some(
                    census
                        .canonical_max
                        .map_or(canonical, |max| max.max(canonical)),
                );
            }
            opcode if (0x10..=0xff).contains(&opcode) => {}
            _ => return Err(PhysicalChunkValidationError::ContextForbiddenRecordOpcode),
        }
    }
    Ok(census)
}

#[derive(Clone, Copy, Debug)]
struct NestedPreflight {
    entries: u64,
    owned_field_bytes: u64,
}

fn preflight_schema(
    body: &[u8],
    projection: &ProjectedSummaryDefinitions<'_, '_, '_>,
) -> Result<NestedPreflight, PhysicalChunkValidationError> {
    let mut cursor = BorrowedCursor::new(body);
    let id = cursor.u16()?;
    let name = cursor.string()?;
    let encoding = cursor.string()?;
    let data_len = usize::try_from(cursor.u32()?)
        .map_err(|_overflow| PhysicalChunkValidationError::ArithmeticOverflow)?;
    let data = cursor.take_exact(data_len)?;
    cursor.finish()?;
    if let Some(summary) = projection.schema(id)
        && (summary.header.name != name
            || summary.header.encoding != encoding
            || summary.data != data)
    {
        return Err(PhysicalChunkValidationError::DefinitionConsistency);
    }
    Ok(NestedPreflight {
        entries: 0,
        owned_field_bytes: checked_add(
            checked_add(name.len() as u64, encoding.len() as u64)?,
            data.len() as u64,
        )?,
    })
}

fn preflight_channel(
    body: &[u8],
    projection: &ProjectedSummaryDefinitions<'_, '_, '_>,
    limits: &PhysicalChunkScanLimits,
) -> Result<NestedPreflight, PhysicalChunkValidationError> {
    let mut cursor = BorrowedCursor::new(body);
    let id = cursor.u16()?;
    let schema_id = cursor.u16()?;
    let topic = cursor.string()?;
    let message_encoding = cursor.string()?;
    let metadata_len = usize::try_from(cursor.u32()?)
        .map_err(|_overflow| PhysicalChunkValidationError::ArithmeticOverflow)?;
    let metadata_bytes = cursor.take_exact(metadata_len)?;
    cursor.finish()?;

    let mut metadata = BorrowedCursor::new(metadata_bytes);
    let mut entries = 0_u64;
    let mut owned_field_bytes = checked_add(topic.len() as u64, message_encoding.len() as u64)?;
    let summary = projection.channel(id);
    while !metadata.is_empty() {
        entries = checked_increment_limited(
            entries,
            limits.max_channel_metadata_entries_per_record,
            PhysicalChunkValidationError::ChannelMetadataEntryLimitExceeded,
        )?;
        let key = metadata.string_limited(
            limits.max_channel_metadata_key_bytes,
            PhysicalChunkValidationError::ChannelMetadataKeyByteLimitExceeded,
        )?;
        let value = metadata.string_limited(
            limits.max_channel_metadata_value_bytes,
            PhysicalChunkValidationError::ChannelMetadataValueByteLimitExceeded,
        )?;
        owned_field_bytes = checked_add(
            owned_field_bytes,
            checked_add(key.len() as u64, value.len() as u64)?,
        )?;
    }
    if let Some(summary) = summary
        && (summary.schema_id != schema_id
            || summary.topic != topic
            || summary.message_encoding != message_encoding)
    {
        return Err(PhysicalChunkValidationError::DefinitionConsistency);
    }
    Ok(NestedPreflight {
        entries,
        owned_field_bytes,
    })
}

#[derive(Clone, Copy, Debug)]
struct MessageObservation {
    channel_id: u16,
    log_time: u64,
    payload_bytes: u64,
}

fn parse_message_observation(
    body: &[u8],
) -> Result<MessageObservation, PhysicalChunkValidationError> {
    let mut cursor = BorrowedCursor::new(body);
    let channel_id = cursor.u16()?;
    let _sequence = cursor.u32()?;
    let log_time = cursor.u64()?;
    let _publish_time = cursor.u64()?;
    let payload_bytes = u64::try_from(cursor.remaining())
        .map_err(|_overflow| PhysicalChunkValidationError::ArithmeticOverflow)?;
    cursor.take_exact(cursor.remaining())?;
    cursor.finish()?;
    Ok(MessageObservation {
        channel_id,
        log_time,
        payload_bytes,
    })
}

fn validate_actual_extent(
    region: &CanonicalPhysicalRegion<'_>,
    census: ScanPreflight,
) -> Result<ValidatedPhysicalChunkExtent, PhysicalChunkValidationError> {
    let descriptor = region.raw_descriptor();
    if census.messages_seen == 0 {
        if descriptor.message_start_time != 0 || descriptor.message_end_time != 0 {
            return Err(PhysicalChunkValidationError::EmptyExtentMismatch);
        }
        Ok(ValidatedPhysicalChunkExtent::KnownEmpty)
    } else {
        let raw_start = census
            .raw_min
            .ok_or(PhysicalChunkValidationError::SemanticCensusMismatch)?;
        let raw_end = census
            .raw_max
            .ok_or(PhysicalChunkValidationError::SemanticCensusMismatch)?;
        if raw_start != descriptor.message_start_time || raw_end != descriptor.message_end_time {
            return Err(PhysicalChunkValidationError::NonEmptyExtentMismatch);
        }
        Ok(ValidatedPhysicalChunkExtent::NonEmpty {
            raw_start,
            raw_end,
            canonical_start: census
                .canonical_min
                .ok_or(PhysicalChunkValidationError::SemanticCensusMismatch)?,
            canonical_end: census
                .canonical_max
                .ok_or(PhysicalChunkValidationError::SemanticCensusMismatch)?,
        })
    }
}

fn validate_expected_raw_extent(
    descriptor: &mcap::records::ChunkIndex,
) -> Result<(), PhysicalChunkValidationError> {
    if descriptor.message_start_time > descriptor.message_end_time {
        return Err(PhysicalChunkValidationError::InvalidExpectedExtent);
    }
    if descriptor.message_start_time == 0 && descriptor.message_end_time == 0 {
        return Ok(());
    }
    canonicalize_raw_mcap_time(RawMcapTime::new(descriptor.message_start_time))
        .map_err(|_error| PhysicalChunkValidationError::InvalidTemporalValue)?;
    canonicalize_raw_mcap_time(RawMcapTime::new(descriptor.message_end_time))
        .map_err(|_error| PhysicalChunkValidationError::InvalidTemporalValue)?;
    Ok(())
}

struct RecordView<'a> {
    opcode: u8,
    body: &'a [u8],
}

struct RecordSequence<'a> {
    cursor: BorrowedCursor<'a>,
}

impl<'a> RecordSequence<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            cursor: BorrowedCursor::new(bytes),
        }
    }

    fn next(&mut self) -> Result<Option<RecordView<'a>>, PhysicalChunkValidationError> {
        if self.cursor.is_empty() {
            return Ok(None);
        }
        let opcode = self.cursor.u8()?;
        let body_len = usize::try_from(self.cursor.u64()?)
            .map_err(|_overflow| PhysicalChunkValidationError::ArithmeticOverflow)?;
        let body = self.cursor.take_exact(body_len)?;
        Ok(Some(RecordView { opcode, body }))
    }
}

struct BorrowedCursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> BorrowedCursor<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    const fn position(&self) -> usize {
        self.position
    }

    fn remaining(&self) -> usize {
        self.bytes.len() - self.position
    }

    fn is_empty(&self) -> bool {
        self.position == self.bytes.len()
    }

    fn take_exact(&mut self, length: usize) -> Result<&'a [u8], PhysicalChunkValidationError> {
        let end = self
            .position
            .checked_add(length)
            .ok_or(PhysicalChunkValidationError::ArithmeticOverflow)?;
        let bytes = self
            .bytes
            .get(self.position..end)
            .ok_or(PhysicalChunkValidationError::RecordBodyOutOfBounds)?;
        self.position = end;
        Ok(bytes)
    }

    fn u8(&mut self) -> Result<u8, PhysicalChunkValidationError> {
        Ok(self.take_exact(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, PhysicalChunkValidationError> {
        Ok(u16::from_le_bytes(
            self.take_exact(size_of::<u16>())?
                .try_into()
                .map_err(|_error| PhysicalChunkValidationError::RecordBodyInvalid)?,
        ))
    }

    fn u32(&mut self) -> Result<u32, PhysicalChunkValidationError> {
        Ok(u32::from_le_bytes(
            self.take_exact(size_of::<u32>())?
                .try_into()
                .map_err(|_error| PhysicalChunkValidationError::RecordBodyInvalid)?,
        ))
    }

    fn u64(&mut self) -> Result<u64, PhysicalChunkValidationError> {
        Ok(u64::from_le_bytes(
            self.take_exact(size_of::<u64>())?
                .try_into()
                .map_err(|_error| PhysicalChunkValidationError::RecordBodyInvalid)?,
        ))
    }

    fn string(&mut self) -> Result<&'a str, PhysicalChunkValidationError> {
        let length = usize::try_from(self.u32()?)
            .map_err(|_overflow| PhysicalChunkValidationError::ArithmeticOverflow)?;
        std::str::from_utf8(self.take_exact(length)?)
            .map_err(|_error| PhysicalChunkValidationError::InvalidUtf8)
    }

    fn string_limited(
        &mut self,
        max_bytes: u64,
        limit_error: PhysicalChunkValidationError,
    ) -> Result<&'a str, PhysicalChunkValidationError> {
        let raw_length = self.u32()?;
        if u64::from(raw_length) > max_bytes {
            return Err(limit_error);
        }
        let length = usize::try_from(raw_length)
            .map_err(|_overflow| PhysicalChunkValidationError::ArithmeticOverflow)?;
        std::str::from_utf8(self.take_exact(length)?)
            .map_err(|_error| PhysicalChunkValidationError::InvalidUtf8)
    }

    fn finish(self) -> Result<(), PhysicalChunkValidationError> {
        if self.position == self.bytes.len() {
            Ok(())
        } else {
            Err(PhysicalChunkValidationError::RecordTrailingBytes)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PhysicalChunkValidationError {
    ArithmeticOverflow,
    AuthorityReservationLimitExceeded,
    AuthorityAllocationFailed,
    ProjectionAllocationFailed,
    ProjectionCountMismatch,
    ScanReservationLimitExceeded,
    UnknownCanonicalOrdinal,
    SelectorAuthorityMismatch,
    DuplicateLiveRead,
    ReadGenerationExhausted,
    SourceGenerationExhausted,
    SourceClosed,
    StaleSourceGeneration,
    StaleReadGeneration,
    MissingPhysicalReadLease,
    FullRecordLengthMismatch,
    WrongChunkOpcode,
    InvalidRecordEnvelope,
    HeaderExtentMismatch,
    HeaderCompressionMismatch,
    HeaderSizeMismatch,
    ChecksumPolicyMismatch,
    CompressedPayloadRangeMismatch,
    PayloadAllocationFailed,
    DecompressionProfileMismatch,
    InvalidRecordOpcode,
    ContextForbiddenRecordOpcode,
    RecordBodyOutOfBounds,
    RecordBodyInvalid,
    RecordTrailingBytes,
    InvalidUtf8,
    RecordLimitExceeded,
    MessageLimitExceeded,
    SchemaRecordLimitExceeded,
    ChannelRecordLimitExceeded,
    ChannelMetadataEntryLimitExceeded,
    ChannelMetadataKeyByteLimitExceeded,
    ChannelMetadataValueByteLimitExceeded,
    DuplicateChannelMetadataKey,
    NestedEntryLimitExceeded,
    OwnedFieldByteLimitExceeded,
    MessagePayloadByteLimitExceeded,
    CanonicalSchemaLimitExceeded,
    CanonicalChannelLimitExceeded,
    ResultRetainedByteLimitExceeded,
    ResultAllocationFailed,
    CanonicalChannelProjectionMismatch,
    UnknownMessageChannel,
    MissingMessageSchema,
    InvalidTemporalValue,
    InvalidExpectedExtent,
    EmptyExtentMismatch,
    NonEmptyExtentMismatch,
    SemanticCensusMismatch,
    DefinitionConsistency,
    Decompression,
}

impl std::fmt::Display for PhysicalChunkValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("remote MCAP physical Chunk validation failed")
    }
}

impl std::error::Error for PhysicalChunkValidationError {}

impl From<DefinitionConsistencyError> for PhysicalChunkValidationError {
    fn from(_error: DefinitionConsistencyError) -> Self {
        Self::DefinitionConsistency
    }
}

impl From<ChunkDecompressionError> for PhysicalChunkValidationError {
    fn from(_error: ChunkDecompressionError) -> Self {
        Self::Decompression
    }
}

fn checked_add(left: u64, right: u64) -> Result<u64, PhysicalChunkValidationError> {
    left.checked_add(right)
        .ok_or(PhysicalChunkValidationError::ArithmeticOverflow)
}

fn checked_add_limited(
    left: u64,
    right: u64,
    limit: u64,
    error: PhysicalChunkValidationError,
) -> Result<u64, PhysicalChunkValidationError> {
    let value = checked_add(left, right)?;
    if value > limit {
        return Err(error);
    }
    Ok(value)
}

fn checked_increment_limited(
    current: u64,
    limit: u64,
    error: PhysicalChunkValidationError,
) -> Result<u64, PhysicalChunkValidationError> {
    checked_add_limited(current, 1, limit, error)
}

fn allocate_source_generation() -> Result<NonZeroU64, PhysicalChunkValidationError> {
    let generation = NEXT_PHYSICAL_CHUNK_SOURCE_GENERATION
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1)
        })
        .map_err(|_current| PhysicalChunkValidationError::SourceGenerationExhausted)?;
    NonZeroU64::new(generation).ok_or(PhysicalChunkValidationError::SourceGenerationExhausted)
}

#[cfg(test)]
impl PhysicalChunkScanLimits {
    fn generous() -> Self {
        Self {
            max_records_per_chunk: 100_000,
            max_messages_per_chunk: 100_000,
            max_schema_records_per_chunk: 10_000,
            max_channel_records_per_chunk: 10_000,
            max_channel_metadata_entries_per_record: 10_000,
            max_channel_metadata_key_bytes: 1024 * 1024,
            max_channel_metadata_value_bytes: 1024 * 1024,
            max_nested_entries_per_chunk: 100_000,
            max_owned_field_bytes_per_chunk: 64 * 1024 * 1024,
            max_message_payload_bytes_per_chunk: 64 * 1024 * 1024,
            max_canonical_schemas: 10_000,
            max_canonical_channels: 10_000,
            max_result_retained_bytes: 64 * 1024 * 1024,
        }
    }
}

#[cfg(test)]
impl PhysicalChunkScanBudget {
    fn for_test(limits: PhysicalChunkScanLimits) -> Self {
        Self {
            state: Arc::new(PhysicalChunkScanBudgetState {
                limits,
                capacity: PhysicalChunkScanBudgetCapacity {
                    max_active_authorities: 16,
                    max_authority_slots: 100_000,
                    max_authority_slot_bytes: 64 * 1024 * 1024,
                    max_definition_projection_bytes: 64 * 1024 * 1024,
                    max_active_scans: 16,
                    max_active_nested_bytes: 128 * 1024 * 1024,
                    max_retained_results: 32,
                    max_retained_result_bytes: 128 * 1024 * 1024,
                },
                usage: Mutex::new(PhysicalChunkScanBudgetUsage::default()),
            }),
        }
    }

    fn usage_for_test(&self) -> PhysicalChunkScanBudgetUsage {
        *self.state.usage.lock()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_decompression::{
        chunk_decompression_budget_for_test, decompress_exact_chunk,
    };
    use crate::remote_fixed_layout::OptionalCrcValidation;
    use crate::remote_summary::tests::AllocationGuard;
    use crate::remote_summary::validated_physical_regions_for_test;
    use crate::testing::{
        AdversarialMcapFixture, AdversarialMcapFixtureBuilder, CompressionFixture,
        DefinitionFixture, FixtureChannel, FixtureChunk, FixtureCrc, FixtureMessage, FixtureSchema,
        MessageIndexFault, RawTimeRange,
    };

    fn fixture_with_chunks(
        chunks: impl IntoIterator<Item = FixtureChunk>,
    ) -> AdversarialMcapFixture {
        AdversarialMcapFixtureBuilder::new()
            .with_chunks(chunks)
            .build()
            .expect("the physical-Chunk test fixture builds")
    }

    fn authority_with_limits(
        fixture: &AdversarialMcapFixture,
        limits: PhysicalChunkScanLimits,
    ) -> PhysicalChunkSourceAuthority<'_> {
        try_authority_with_limits(fixture, limits)
            .expect("the physical-Chunk test authority builds")
    }

    fn try_authority_with_limits(
        fixture: &AdversarialMcapFixture,
        limits: PhysicalChunkScanLimits,
    ) -> Result<PhysicalChunkSourceAuthority<'_>, PhysicalChunkValidationError> {
        PhysicalChunkSourceAuthority::new(
            validated_physical_regions_for_test(fixture),
            chunk_decompression_budget_for_test(),
            PhysicalChunkScanBudget::for_test(limits),
        )
    }

    fn build_authority(fixture: &AdversarialMcapFixture) -> PhysicalChunkSourceAuthority<'_> {
        authority_with_limits(fixture, PhysicalChunkScanLimits::generous())
    }

    fn full_record(fixture: &AdversarialMcapFixture, ordinal: usize) -> Box<[u8]> {
        let record = fixture
            .layout
            .chunks
            .get(ordinal)
            .expect("the fixture contains the requested Chunk")
            .record;
        fixture.bytes[record.start..record.end]
            .to_vec()
            .into_boxed_slice()
    }

    fn issue<'a>(
        authority: &PhysicalChunkSourceAuthority<'a>,
        ordinal: usize,
    ) -> PhysicalChunkReadLease<'a, PendingHeaderValidation> {
        let selector = authority.select(ordinal).expect("the ordinal exists");
        authority.issue(selector).expect("the read lease is issued")
    }

    fn decompress_fixture_chunk<'a>(
        fixture: &AdversarialMcapFixture,
        authority: &PhysicalChunkSourceAuthority<'a>,
        ordinal: usize,
    ) -> Result<ExactOutputChunk<'a>, PhysicalChunkValidationError> {
        decompress_record(authority, ordinal, full_record(fixture, ordinal))
    }

    fn decompress_record<'a>(
        authority: &PhysicalChunkSourceAuthority<'a>,
        ordinal: usize,
        record: Box<[u8]>,
    ) -> Result<ExactOutputChunk<'a>, PhysicalChunkValidationError> {
        let validated =
            install_exact_physical_chunk_record_for_test(issue(authority, ordinal), record)?;
        let input = install_header_validated_payload_for_test(validated)?;
        decompress_exact_chunk(input).map_err(Into::into)
    }

    fn scan_fixture_chunk<'a>(
        fixture: &AdversarialMcapFixture,
        authority: &PhysicalChunkSourceAuthority<'a>,
        ordinal: usize,
    ) -> Result<ValidatedPhysicalChunkScan<'a>, PhysicalChunkValidationError> {
        scan_decompressed_physical_chunk(decompress_fixture_chunk(fixture, authority, ordinal)?)
    }

    fn scan_record<'a>(
        authority: &PhysicalChunkSourceAuthority<'a>,
        ordinal: usize,
        record: Box<[u8]>,
    ) -> Result<ValidatedPhysicalChunkScan<'a>, PhysicalChunkValidationError> {
        scan_decompressed_physical_chunk(decompress_record(authority, ordinal, record)?)
    }

    fn uncompressed_record_opcode_offsets(
        fixture: &AdversarialMcapFixture,
        ordinal: usize,
    ) -> Vec<usize> {
        let layout = fixture
            .layout
            .chunks
            .get(ordinal)
            .expect("the fixture contains the requested Chunk");
        assert_eq!(layout.compression, "");
        let record_start = layout.record.start;
        let payload_start = layout
            .compressed_data_start
            .checked_sub(record_start)
            .expect("the payload belongs to its Chunk record");
        let payload_end = payload_start + layout.compressed_data_len;
        let record = full_record(fixture, ordinal);
        let mut offsets = Vec::new();
        let mut position = payload_start;
        while position < payload_end {
            offsets.push(position);
            let body_len = u64::from_le_bytes(
                record[position + 1..position + crate::RECORD_HEADER_LEN]
                    .try_into()
                    .expect("a generated record has its complete length prefix"),
            );
            position += crate::RECORD_HEADER_LEN + usize::try_from(body_len).unwrap();
        }
        assert_eq!(position, payload_end);
        offsets
    }

    fn channel_metadata_ranges(
        fixture: &AdversarialMcapFixture,
        ordinal: usize,
    ) -> Vec<(Range<usize>, Range<usize>)> {
        let record = full_record(fixture, ordinal);
        let opcode_offset = uncompressed_record_opcode_offsets(fixture, ordinal)
            .into_iter()
            .find(|&offset| record[offset] == mcap::records::op::CHANNEL)
            .expect("the generated Chunk contains a Channel record");
        let body_start = opcode_offset + crate::RECORD_HEADER_LEN;
        let body_len = usize::try_from(u64::from_le_bytes(
            record[opcode_offset + 1..body_start]
                .try_into()
                .expect("the Channel record has a complete length prefix"),
        ))
        .unwrap();
        let mut channel = BorrowedCursor::new(&record[body_start..body_start + body_len]);
        channel.u16().unwrap();
        channel.u16().unwrap();
        channel.string().unwrap();
        channel.string().unwrap();
        let metadata_len = usize::try_from(channel.u32().unwrap()).unwrap();
        let metadata_start = body_start + channel.position();
        let metadata = channel.take_exact(metadata_len).unwrap();
        channel.finish().unwrap();

        let mut cursor = BorrowedCursor::new(metadata);
        let mut ranges = Vec::new();
        while !cursor.is_empty() {
            let key_len = usize::try_from(cursor.u32().unwrap()).unwrap();
            let key_start = metadata_start + cursor.position();
            cursor.take_exact(key_len).unwrap();
            let value_len = usize::try_from(cursor.u32().unwrap()).unwrap();
            let value_start = metadata_start + cursor.position();
            cursor.take_exact(value_len).unwrap();
            ranges.push((
                key_start..key_start + key_len,
                value_start..value_start + value_len,
            ));
        }
        ranges
    }

    fn encoded_channel_body(
        id: u16,
        metadata: impl IntoIterator<Item = (&'static str, &'static str)>,
    ) -> Vec<u8> {
        fn push_string(bytes: &mut Vec<u8>, value: &str) {
            bytes.extend_from_slice(&u32::try_from(value.len()).unwrap().to_le_bytes());
            bytes.extend_from_slice(value.as_bytes());
        }

        let encoded_metadata = encoded_metadata(metadata);
        let mut body = Vec::new();
        body.extend_from_slice(&id.to_le_bytes());
        body.extend_from_slice(&0_u16.to_le_bytes());
        push_string(&mut body, "/unknown");
        push_string(&mut body, "raw");
        body.extend_from_slice(&u32::try_from(encoded_metadata.len()).unwrap().to_le_bytes());
        body.extend_from_slice(&encoded_metadata);
        body
    }

    fn encoded_metadata(
        metadata: impl IntoIterator<Item = (&'static str, &'static str)>,
    ) -> Vec<u8> {
        fn push_string(bytes: &mut Vec<u8>, value: &str) {
            bytes.extend_from_slice(&u32::try_from(value.len()).unwrap().to_le_bytes());
            bytes.extend_from_slice(value.as_bytes());
        }

        let mut bytes = Vec::new();
        for (key, value) in metadata {
            push_string(&mut bytes, key);
            push_string(&mut bytes, value);
        }
        bytes
    }

    #[test]
    fn authority_has_one_live_claim_per_ordinal_and_fresh_generations() {
        let fixture = fixture_with_chunks([
            FixtureChunk::single(FixtureMessage::new(1, 0, 1)),
            FixtureChunk::single(FixtureMessage::new(1, 1, 2)),
        ]);
        let authority = build_authority(&fixture);

        let first = issue(&authority, 0);
        assert_eq!(first.core().read_generation.get(), 1);
        let duplicate = authority.issue(authority.select(0).unwrap()).unwrap_err();
        assert_eq!(duplicate, PhysicalChunkValidationError::DuplicateLiveRead);
        let other = issue(&authority, 1);
        assert_eq!(other.core().read_generation.get(), 1);
        assert_eq!(first.core().read_generation.get(), 1);
        drop(other);
        assert_eq!(first.core().read_generation.get(), 1);
        drop(first);

        let fresh = issue(&authority, 0);
        assert_eq!(fresh.core().read_generation.get(), 2);
        drop(fresh);
        assert_eq!(
            authority.shared.state.lock().slots[0],
            PhysicalChunkSlot::Vacant { last_generation: 2 }
        );
    }

    #[test]
    fn selectors_and_stale_drops_cannot_cross_authority_or_release_a_new_claim() {
        let fixture = fixture_with_chunks([FixtureChunk::empty()]);
        let first_authority = build_authority(&fixture);
        let second_authority = build_authority(&fixture);
        let foreign_selector = first_authority.select(0).unwrap();
        assert_eq!(
            second_authority.issue(foreign_selector).unwrap_err(),
            PhysicalChunkValidationError::SelectorAuthorityMismatch
        );

        let stale = issue(&first_authority, 0);
        first_authority.shared.state.lock().slots[0] = PhysicalChunkSlot::Vacant {
            last_generation: stale.core().read_generation.get(),
        };
        let current = issue(&first_authority, 0);
        assert_eq!(current.core().read_generation.get(), 2);
        drop(stale);
        assert_eq!(
            first_authority.shared.state.lock().slots[0],
            PhysicalChunkSlot::Live {
                generation: NonZeroU64::new(2).unwrap()
            },
            "a stale generation must not release the current live claim"
        );

        let independent = issue(&second_authority, 0);
        drop(first_authority);
        assert!(independent.ensure_current().is_ok());
        drop(independent);
        drop(current);
    }

    #[test]
    fn generation_exhaustion_and_source_close_are_fail_closed() {
        let fixture = fixture_with_chunks([FixtureChunk::empty()]);
        let authority = build_authority(&fixture);
        authority.shared.state.lock().slots[0] = PhysicalChunkSlot::Vacant {
            last_generation: u64::MAX,
        };
        assert_eq!(
            authority.issue(authority.select(0).unwrap()).unwrap_err(),
            PhysicalChunkValidationError::ReadGenerationExhausted
        );
        assert_eq!(
            authority.shared.state.lock().slots[0],
            PhysicalChunkSlot::Vacant {
                last_generation: u64::MAX
            }
        );

        authority.shared.state.lock().slots[0] = PhysicalChunkSlot::Vacant { last_generation: 0 };
        let lease = issue(&authority, 0);
        authority.close();
        assert_eq!(
            lease.ensure_current(),
            Err(PhysicalChunkValidationError::SourceClosed)
        );
        drop(lease);
        assert_eq!(
            authority.issue(authority.select(0).unwrap()).unwrap_err(),
            PhysicalChunkValidationError::SourceClosed
        );
    }

    #[test]
    fn authority_slot_accounting_is_bounded_and_released_with_the_last_owner() {
        let fixture = fixture_with_chunks([FixtureChunk::empty()]);
        let limits = PhysicalChunkScanLimits::generous();
        let state = Arc::new(PhysicalChunkScanBudgetState {
            limits,
            capacity: PhysicalChunkScanBudgetCapacity {
                max_active_authorities: 1,
                max_authority_slots: 1,
                max_authority_slot_bytes: u64::MAX,
                max_definition_projection_bytes: u64::MAX,
                max_active_scans: 1,
                max_active_nested_bytes: u64::MAX,
                max_retained_results: 1,
                max_retained_result_bytes: u64::MAX,
            },
            usage: Mutex::new(PhysicalChunkScanBudgetUsage::default()),
        });
        let make_budget = || PhysicalChunkScanBudget {
            state: Arc::clone(&state),
        };
        let first = PhysicalChunkSourceAuthority::new(
            validated_physical_regions_for_test(&fixture),
            chunk_decompression_budget_for_test(),
            make_budget(),
        )
        .unwrap();
        assert_eq!(
            state.usage.lock().definition_projection_bytes,
            CanonicalDefinitionProjection::retained_bytes(1).unwrap()
        );
        assert_eq!(
            PhysicalChunkSourceAuthority::new(
                validated_physical_regions_for_test(&fixture),
                chunk_decompression_budget_for_test(),
                make_budget(),
            )
            .unwrap_err(),
            PhysicalChunkValidationError::AuthorityReservationLimitExceeded
        );
        drop(first);
        let replacement = PhysicalChunkSourceAuthority::new(
            validated_physical_regions_for_test(&fixture),
            chunk_decompression_budget_for_test(),
            make_budget(),
        )
        .unwrap();
        drop(replacement);
        assert_eq!(*state.usage.lock(), PhysicalChunkScanBudgetUsage::default());
    }

    #[test]
    fn definition_projection_is_sparse_duplicate_stable_and_constant_lookup() {
        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_schemas([
                FixtureSchema::new(1, "low.Schema", "raw"),
                FixtureSchema::new(u16::MAX, "high.Schema", "raw"),
            ])
            .with_channels([
                FixtureChannel::schema_less(1, "/low"),
                FixtureChannel::schema_less(u16::MAX, "/high").with_schema(u16::MAX, "raw"),
            ])
            .with_chunks([FixtureChunk::new([
                FixtureMessage::new(1, 0, 1),
                FixtureMessage::new(u16::MAX, 1, 2),
            ])])
            .build()
            .unwrap();
        let authority = build_authority(&fixture);
        let projection = &authority.shared.definition_projection;
        assert_eq!(projection.canonical_channel_ids.as_ref(), &[1, u16::MAX]);
        let before = projection.lookup_calls();
        assert!(projection.schema_record_index(0).is_none());
        assert!(projection.schema_record_index(u16::MAX).is_some());
        assert!(projection.channel_slot(1).is_some());
        assert!(projection.channel_slot(u16::MAX).is_some());
        assert!(projection.channel_slot(42).is_none());
        assert_eq!(
            projection.lookup_calls() - before,
            5 * CanonicalDefinitionProjection::MAX_SLOT_LOOKUPS_PER_QUERY
        );
        let before_scan = projection.lookup_calls();
        let scan = scan_fixture_chunk(&fixture, &authority, 0).unwrap();
        assert_eq!(scan.messages_seen, 2);
        assert_eq!(scan.channels.len(), 2);
        assert!(
            projection.lookup_calls() - before_scan
                <= scan.records_seen
                    * 3
                    * CanonicalDefinitionProjection::MAX_SLOT_LOOKUPS_PER_QUERY,
            "each record has a fixed lookup-operation upper bound"
        );

        let duplicate = AdversarialMcapFixtureBuilder::new()
            .with_definition_fixture(DefinitionFixture::ExactDuplicate)
            .build()
            .unwrap();
        let duplicate_authority = build_authority(&duplicate);
        assert_eq!(
            duplicate_authority
                .shared
                .definition_projection
                .canonical_channel_ids
                .len(),
            1,
            "identical duplicate definitions map to the same dense slot"
        );
        scan_fixture_chunk(&duplicate, &duplicate_authority, 0).unwrap();
    }

    #[test]
    fn canonical_definition_caps_fail_before_projection_or_chunk_work() {
        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_schemas([
                FixtureSchema::new(1, "one", "raw"),
                FixtureSchema::new(2, "two", "raw"),
            ])
            .with_channels([
                FixtureChannel::schema_less(1, "/one").with_schema(1, "raw"),
                FixtureChannel::schema_less(2, "/two").with_schema(2, "raw"),
            ])
            .with_chunks([FixtureChunk::new([
                FixtureMessage::new(1, 0, 1),
                FixtureMessage::new(2, 1, 2),
            ])])
            .build()
            .unwrap();
        let mut exact = PhysicalChunkScanLimits::generous();
        exact.max_canonical_schemas = 2;
        exact.max_canonical_channels = 2;
        exact.max_messages_per_chunk = 2;
        let authority = authority_with_limits(&fixture, exact);
        assert_eq!(
            scan_fixture_chunk(&fixture, &authority, 0)
                .unwrap()
                .messages_seen,
            2
        );

        let mut schema_limited = exact;
        schema_limited.max_canonical_schemas = 1;
        assert_eq!(
            try_authority_with_limits(&fixture, schema_limited).unwrap_err(),
            PhysicalChunkValidationError::CanonicalSchemaLimitExceeded
        );
        let mut channel_limited = exact;
        channel_limited.max_canonical_channels = 1;
        assert_eq!(
            try_authority_with_limits(&fixture, channel_limited).unwrap_err(),
            PhysicalChunkValidationError::CanonicalChannelLimitExceeded
        );
    }

    #[test]
    fn header_validator_rejects_every_descriptor_bound_field_before_decompression() {
        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_chunks([FixtureChunk::single(FixtureMessage::new(1, 0, 10))])
            .with_compression(CompressionFixture::Zstd)
            .build()
            .unwrap();
        let compression_len = fixture.layout.chunks[0].compression.len();
        let compression_len_start = crate::RECORD_HEADER_LEN + CHUNK_FIXED_HEADER_BYTES;
        let compression_start = crate::RECORD_HEADER_LEN + CHUNK_FIXED_HEADER_BYTES + 4;
        let compressed_size_start = compression_start + compression_len;
        let cases = [
            (0, 0xff, PhysicalChunkValidationError::WrongChunkOpcode),
            (1, 0xff, PhysicalChunkValidationError::InvalidRecordEnvelope),
            (
                crate::RECORD_HEADER_LEN,
                0xff,
                PhysicalChunkValidationError::HeaderExtentMismatch,
            ),
            (
                crate::RECORD_HEADER_LEN + 8,
                0xff,
                PhysicalChunkValidationError::HeaderExtentMismatch,
            ),
            (
                crate::RECORD_HEADER_LEN + 16,
                0xff,
                PhysicalChunkValidationError::HeaderSizeMismatch,
            ),
            (
                compression_len_start,
                0xff,
                PhysicalChunkValidationError::RecordBodyOutOfBounds,
            ),
            (
                compression_start,
                b'x',
                PhysicalChunkValidationError::HeaderCompressionMismatch,
            ),
            (
                compressed_size_start,
                0xff,
                PhysicalChunkValidationError::RecordBodyOutOfBounds,
            ),
        ];
        for (offset, replacement, expected) in cases {
            let authority = build_authority(&fixture);
            let mut record = full_record(&fixture, 0);
            record[offset] = replacement;
            assert_eq!(
                install_exact_physical_chunk_record_for_test(issue(&authority, 0), record)
                    .unwrap_err(),
                expected
            );
            let fresh = issue(&authority, 0);
            assert_eq!(fresh.core().read_generation.get(), 2);
        }

        let authority = build_authority(&fixture);
        let mut truncated = full_record(&fixture, 0).into_vec();
        truncated.pop();
        assert_eq!(
            install_exact_physical_chunk_record_for_test(
                issue(&authority, 0),
                truncated.into_boxed_slice(),
            )
            .unwrap_err(),
            PhysicalChunkValidationError::FullRecordLengthMismatch
        );

        let authority = build_authority(&fixture);
        let mut trailing_payload = full_record(&fixture, 0);
        trailing_payload[compressed_size_start] = trailing_payload[compressed_size_start]
            .checked_sub(1)
            .expect("the generated compressed payload is nonempty");
        assert_eq!(
            install_exact_physical_chunk_record_for_test(issue(&authority, 0), trailing_payload,)
                .unwrap_err(),
            PhysicalChunkValidationError::RecordTrailingBytes
        );
    }

    #[test]
    fn header_preflight_is_allocation_free_and_binds_the_selected_ordinal() {
        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_chunks([
                FixtureChunk::single(FixtureMessage::new(1, 0, 10)),
                FixtureChunk::single(FixtureMessage::new(1, 0, 20)),
            ])
            .with_chunk_crc(FixtureCrc::Zero)
            .build()
            .unwrap();
        let authority = build_authority(&fixture);

        assert_eq!(
            install_exact_physical_chunk_record_for_test(
                issue(&authority, 0),
                full_record(&fixture, 1),
            )
            .unwrap_err(),
            PhysicalChunkValidationError::HeaderExtentMismatch,
            "same-sized bytes from another ordinal cannot be rebound to a lease"
        );

        let lease = issue(&authority, 0);
        let record = full_record(&fixture, 0);
        let guard = AllocationGuard::start();
        let validated = install_exact_physical_chunk_record_for_test(lease, record);
        let allocations = AllocationGuard::count();
        drop(guard);
        assert_eq!(allocations, 0, "Chunk header validation must not allocate");
        drop(validated.unwrap());
    }

    #[test]
    fn crc_is_derived_only_from_header_and_has_all_three_states() {
        for (crc, expected) in [
            (FixtureCrc::Zero, OptionalCrcValidation::NotProvided),
            (FixtureCrc::ValidNonZero, OptionalCrcValidation::Verified),
        ] {
            let fixture = AdversarialMcapFixtureBuilder::new()
                .with_chunk_crc(crc)
                .build()
                .unwrap();
            let authority = build_authority(&fixture);
            let output = decompress_fixture_chunk(&fixture, &authority, 0).unwrap();
            assert_eq!(output.crc_validation(), expected);
        }

        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_chunk_crc(FixtureCrc::InvalidNonZero)
            .build()
            .unwrap();
        let authority = build_authority(&fixture);
        let validated = install_exact_physical_chunk_record_for_test(
            issue(&authority, 0),
            full_record(&fixture, 0),
        )
        .unwrap();
        let input = install_header_validated_payload_for_test(validated).unwrap();
        assert_eq!(
            decompress_exact_chunk(input).unwrap_err(),
            ChunkDecompressionError::ChunkChecksumMismatch
        );
        assert_eq!(issue(&authority, 0).core().read_generation.get(), 2);

        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_chunk_crc(FixtureCrc::ValidNonZero)
            .build()
            .unwrap();
        let crc_offset = crate::RECORD_HEADER_LEN + 8 + 8 + 8;
        let authority = build_authority(&fixture);
        let mut zero_crc_body = full_record(&fixture, 0);
        zero_crc_body[crc_offset..crc_offset + 4].fill(0);
        let zero = decompress_record(&authority, 0, zero_crc_body).unwrap();
        assert_eq!(zero.crc_validation(), OptionalCrcValidation::NotProvided);
        drop(zero);
        let verified = decompress_fixture_chunk(&fixture, &authority, 0).unwrap();
        assert_eq!(verified.crc_validation(), OptionalCrcValidation::Verified);
    }

    #[test]
    fn none_zstd_and_lz4_reach_the_same_complete_semantic_scan() {
        for compression in [
            CompressionFixture::None,
            CompressionFixture::Zstd,
            CompressionFixture::Lz4,
        ] {
            let fixture = AdversarialMcapFixtureBuilder::new()
                .with_chunks([FixtureChunk::new([
                    FixtureMessage::new(1, 0, 20).with_data([1, 2]),
                    FixtureMessage::new(1, 1, 10).with_data([3, 4, 5]),
                ])])
                .with_compression(compression)
                .build()
                .unwrap();
            let authority = build_authority(&fixture);
            let scan = scan_fixture_chunk(&fixture, &authority, 0).unwrap();
            assert_eq!(scan.messages_seen, 2);
            assert_eq!(scan.message_payload_bytes, 5);
            assert_eq!(scan.channels[0].message_count, 2);
            assert_eq!(scan.channels[0].payload_bytes, 5);
            assert_eq!(
                scan.extent,
                ValidatedPhysicalChunkExtent::NonEmpty {
                    raw_start: 10,
                    raw_end: 20,
                    canonical_start: TimeInt::try_from(10).unwrap(),
                    canonical_end: TimeInt::try_from(20).unwrap(),
                }
            );
            assert!(scan.is_current());
        }
    }

    #[test]
    fn private_records_are_skipped_but_invalid_and_context_records_fail_closed() {
        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_chunk_crc(FixtureCrc::Zero)
            .build()
            .unwrap();
        let first_opcode = uncompressed_record_opcode_offsets(&fixture, 0)[0];

        for opcode in [0x10, 0x80, 0xff] {
            let authority = build_authority(&fixture);
            let mut record = full_record(&fixture, 0);
            record[first_opcode] = opcode;
            let scan = scan_record(&authority, 0, record).unwrap();
            assert_eq!(scan.messages_seen, 1);
        }

        for (opcode, expected) in [
            (0, PhysicalChunkValidationError::InvalidRecordOpcode),
            (
                mcap::records::op::HEADER,
                PhysicalChunkValidationError::ContextForbiddenRecordOpcode,
            ),
            (
                mcap::records::op::DATA_END,
                PhysicalChunkValidationError::ContextForbiddenRecordOpcode,
            ),
        ] {
            let authority = build_authority(&fixture);
            let mut record = full_record(&fixture, 0);
            record[first_opcode] = opcode;
            assert_eq!(scan_record(&authority, 0, record).unwrap_err(), expected);
        }

        let authority = build_authority(&fixture);
        let mut record = full_record(&fixture, 0);
        record[first_opcode + 1..first_opcode + crate::RECORD_HEADER_LEN].fill(0xff);
        assert_eq!(
            scan_record(&authority, 0, record).unwrap_err(),
            PhysicalChunkValidationError::ArithmeticOverflow
        );
    }

    #[test]
    fn nested_record_preflight_rejects_truncation_utf8_lengths_and_trailing_bytes() {
        fn push_string(bytes: &mut Vec<u8>, value: &[u8]) {
            bytes.extend_from_slice(&u32::try_from(value.len()).unwrap().to_le_bytes());
            bytes.extend_from_slice(value);
        }

        let fixture = AdversarialMcapFixtureBuilder::new().build().unwrap();
        let authority = build_authority(&fixture);
        let definitions = authority.shared.physical.definitions();
        let projected = ProjectedSummaryDefinitions {
            projection: &authority.shared.definition_projection,
            definitions,
        };

        let mut schema = Vec::new();
        schema.extend_from_slice(&99_u16.to_le_bytes());
        push_string(&mut schema, b"schema");
        push_string(&mut schema, b"raw");
        schema.extend_from_slice(&2_u32.to_le_bytes());
        schema.extend_from_slice(&[1, 2]);
        assert!(preflight_schema(&schema, &projected).is_ok());

        let mut truncated = schema.clone();
        truncated.pop();
        assert_eq!(
            preflight_schema(&truncated, &projected).unwrap_err(),
            PhysicalChunkValidationError::RecordBodyOutOfBounds
        );
        let mut invalid_utf8 = schema.clone();
        invalid_utf8[2 + 4] = 0xff;
        assert_eq!(
            preflight_schema(&invalid_utf8, &projected).unwrap_err(),
            PhysicalChunkValidationError::InvalidUtf8
        );
        let mut trailing = schema;
        trailing.push(0);
        assert_eq!(
            preflight_schema(&trailing, &projected).unwrap_err(),
            PhysicalChunkValidationError::RecordTrailingBytes
        );

        let mut metadata = Vec::new();
        push_string(&mut metadata, b"key");
        push_string(&mut metadata, b"value");
        let mut channel = Vec::new();
        channel.extend_from_slice(&99_u16.to_le_bytes());
        channel.extend_from_slice(&0_u16.to_le_bytes());
        push_string(&mut channel, b"/topic");
        push_string(&mut channel, b"raw");
        let metadata_len_offset = channel.len();
        channel.extend_from_slice(&u32::try_from(metadata.len()).unwrap().to_le_bytes());
        let metadata_start = channel.len();
        channel.extend_from_slice(&metadata);
        assert!(
            preflight_channel(&channel, &projected, &PhysicalChunkScanLimits::generous()).is_ok()
        );

        let mut outer_short = channel.clone();
        let short_len = u32::try_from(metadata.len() - 1).unwrap().to_le_bytes();
        outer_short[metadata_len_offset..metadata_len_offset + 4].copy_from_slice(&short_len);
        assert_eq!(
            preflight_channel(
                &outer_short,
                &projected,
                &PhysicalChunkScanLimits::generous()
            )
            .unwrap_err(),
            PhysicalChunkValidationError::RecordTrailingBytes
        );
        let mut outer_long = channel.clone();
        let long_len = u32::try_from(metadata.len() + 1).unwrap().to_le_bytes();
        outer_long[metadata_len_offset..metadata_len_offset + 4].copy_from_slice(&long_len);
        assert_eq!(
            preflight_channel(
                &outer_long,
                &projected,
                &PhysicalChunkScanLimits::generous()
            )
            .unwrap_err(),
            PhysicalChunkValidationError::RecordBodyOutOfBounds
        );
        let mut nested_long = channel.clone();
        nested_long[metadata_start..metadata_start + 4].fill(0xff);
        let mut unbounded_field_lengths = PhysicalChunkScanLimits::generous();
        unbounded_field_lengths.max_channel_metadata_key_bytes = u64::MAX;
        unbounded_field_lengths.max_channel_metadata_value_bytes = u64::MAX;
        assert_eq!(
            preflight_channel(&nested_long, &projected, &unbounded_field_lengths).unwrap_err(),
            PhysicalChunkValidationError::RecordBodyOutOfBounds
        );
        let mut nested_utf8 = channel;
        nested_utf8[metadata_start + 4] = 0xff;
        assert_eq!(
            preflight_channel(
                &nested_utf8,
                &projected,
                &PhysicalChunkScanLimits::generous()
            )
            .unwrap_err(),
            PhysicalChunkValidationError::InvalidUtf8
        );

        assert_eq!(
            parse_message_observation(&[0; MESSAGE_HEADER_BYTES - 1]).unwrap_err(),
            PhysicalChunkValidationError::RecordBodyOutOfBounds
        );
    }

    #[test]
    fn metadata_key_and_value_limits_are_exact_and_precede_parser_allocation() {
        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_channels([
                FixtureChannel::schema_less(1, "/fixture").with_metadata("key", "value")
            ])
            .build()
            .unwrap();
        let mut exact = PhysicalChunkScanLimits::generous();
        exact.max_channel_metadata_key_bytes = 3;
        exact.max_channel_metadata_value_bytes = 5;
        let authority = authority_with_limits(&fixture, exact);
        scan_fixture_chunk(&fixture, &authority, 0).unwrap();

        let mut key_limited = exact;
        key_limited.max_channel_metadata_key_bytes = 2;
        let authority = authority_with_limits(&fixture, key_limited);
        let output = decompress_fixture_chunk(&fixture, &authority, 0).unwrap();
        let usage_before = authority.shared.scan_budget.usage_for_test();
        let guard = AllocationGuard::start();
        let error = scan_decompressed_physical_chunk(output).unwrap_err();
        let allocations = AllocationGuard::count();
        drop(guard);
        assert_eq!(
            error,
            PhysicalChunkValidationError::ChannelMetadataKeyByteLimitExceeded
        );
        assert_eq!(
            allocations, 0,
            "metadata cap failure precedes upstream parsing"
        );
        assert_eq!(authority.shared.scan_budget.usage_for_test(), usage_before);
        assert_eq!(issue(&authority, 0).core().read_generation.get(), 2);

        let mut value_limited = exact;
        value_limited.max_channel_metadata_value_bytes = 4;
        let authority = authority_with_limits(&fixture, value_limited);
        assert_eq!(
            scan_fixture_chunk(&fixture, &authority, 0).unwrap_err(),
            PhysicalChunkValidationError::ChannelMetadataValueByteLimitExceeded
        );
    }

    #[test]
    fn metadata_single_field_and_multi_record_aggregate_limits_are_independent() {
        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_channels([
                FixtureChannel::schema_less(1, "/one").with_metadata("key", "value"),
                FixtureChannel::schema_less(2, "/two").with_metadata("key", "value"),
            ])
            .with_chunks([FixtureChunk::new([
                FixtureMessage::new(1, 0, 1),
                FixtureMessage::new(2, 1, 2),
            ])])
            .build()
            .unwrap();
        let mut exact = PhysicalChunkScanLimits::generous();
        exact.max_channel_metadata_key_bytes = 3;
        exact.max_channel_metadata_value_bytes = 5;
        exact.max_owned_field_bytes_per_chunk = 30;
        let authority = authority_with_limits(&fixture, exact);
        scan_fixture_chunk(&fixture, &authority, 0).unwrap();

        let mut aggregate_limited = exact;
        aggregate_limited.max_owned_field_bytes_per_chunk = 29;
        let authority = authority_with_limits(&fixture, aggregate_limited);
        assert_eq!(
            scan_fixture_chunk(&fixture, &authority, 0).unwrap_err(),
            PhysicalChunkValidationError::OwnedFieldByteLimitExceeded
        );
    }

    #[test]
    fn duplicate_metadata_keys_cannot_mask_a_missing_canonical_pair() {
        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_channels([FixtureChannel::schema_less(1, "/fixture")
                .with_metadata("a", "1")
                .with_metadata("b", "2")])
            .with_chunk_crc(FixtureCrc::Zero)
            .build()
            .unwrap();
        let ranges = channel_metadata_ranges(&fixture, 0);
        assert_eq!(ranges.len(), 2);
        for (same_value, label) in [(true, "same value"), (false, "different value")] {
            let authority = build_authority(&fixture);
            let mut record = full_record(&fixture, 0);
            let (first_key, first_value) = &ranges[0];
            let (second_key, second_value) = &ranges[1];
            assert_eq!(first_key.len(), second_key.len());
            let first_key_bytes = record[first_key.clone()].to_vec();
            record[second_key.clone()].copy_from_slice(&first_key_bytes);
            if same_value {
                assert_eq!(first_value.len(), second_value.len());
                let first_value_bytes = record[first_value.clone()].to_vec();
                record[second_value.clone()].copy_from_slice(&first_value_bytes);
            }
            let output = decompress_record(&authority, 0, record).unwrap();
            let usage_before = authority.shared.scan_budget.usage_for_test();
            let guard = AllocationGuard::start();
            let error = scan_decompressed_physical_chunk(output).unwrap_err();
            let allocations = AllocationGuard::count();
            drop(guard);
            assert_eq!(
                error,
                PhysicalChunkValidationError::DuplicateChannelMetadataKey,
                "duplicate case: {label}"
            );
            assert!(
                (1..=16).contains(&allocations),
                "duplicate {label} materialization used {allocations} allocations"
            );
            assert_eq!(authority.shared.scan_budget.usage_for_test(), usage_before);
            assert_eq!(issue(&authority, 0).core().read_generation.get(), 2);
        }
    }

    #[test]
    fn borrowed_metadata_preflight_is_linear_and_defers_duplicate_detection() {
        let fixture = AdversarialMcapFixtureBuilder::new().build().unwrap();
        let authority = build_authority(&fixture);
        let projected = ProjectedSummaryDefinitions {
            projection: &authority.shared.definition_projection,
            definitions: authority.shared.physical.definitions(),
        };
        let high = "\u{10ffff}";
        assert!(
            preflight_channel(
                &encoded_channel_body(99, [("", "empty"), (high, "high")]),
                &projected,
                &PhysicalChunkScanLimits::generous(),
            )
            .is_ok()
        );
        for metadata in [[("", "one"), ("", "two")], [(high, "one"), (high, "two")]] {
            let body = encoded_channel_body(99, metadata);
            let guard = AllocationGuard::start();
            let nested =
                preflight_channel(&body, &projected, &PhysicalChunkScanLimits::generous()).unwrap();
            let allocations = AllocationGuard::count();
            drop(guard);
            assert_eq!(nested.entries, 2);
            assert_eq!(allocations, 0);
        }

        let production = include_str!("remote_chunk_scan.rs")
            .split("#[cfg(test)]\nmod tests")
            .next()
            .unwrap();
        let borrowed_channel_pass = production
            .split("fn preflight_channel(")
            .nth(1)
            .unwrap()
            .split("struct MessageObservation")
            .next()
            .unwrap();
        assert!(!borrowed_channel_pass.contains("ensure_unique"));
        assert!(!borrowed_channel_pass.contains("get(..current_entry_start)"));
        assert_eq!(
            borrowed_channel_pass
                .matches("while !metadata.is_empty()")
                .count(),
            1
        );
    }

    #[test]
    fn duplicate_empty_and_high_unicode_metadata_keys_fail_after_reservation() {
        let empty_key_fixture = AdversarialMcapFixtureBuilder::new()
            .with_channels([FixtureChannel::schema_less(1, "/fixture")
                .with_metadata("", "one")
                .with_metadata("x", "two")])
            .with_chunk_crc(FixtureCrc::Zero)
            .build()
            .unwrap();
        let high_a = "\u{10fffe}";
        let high_b = "\u{10ffff}";
        let high_key_fixture = AdversarialMcapFixtureBuilder::new()
            .with_channels([FixtureChannel::schema_less(1, "/fixture")
                .with_metadata(high_a, "one")
                .with_metadata(high_b, "two")])
            .with_chunk_crc(FixtureCrc::Zero)
            .build()
            .unwrap();

        for (fixture, replacement, label) in [
            (
                &empty_key_fixture,
                encoded_metadata([("", "one"), ("", "two!")]),
                "empty key",
            ),
            (
                &high_key_fixture,
                encoded_metadata([(high_a, "one"), (high_a, "two")]),
                "high Unicode key",
            ),
        ] {
            let ranges = channel_metadata_ranges(fixture, 0);
            assert_eq!(ranges.len(), 2);
            let metadata_start = ranges[0].0.start - size_of::<u32>();
            let metadata_end = ranges.last().unwrap().1.end;
            assert_eq!(replacement.len(), metadata_end - metadata_start);
            let mut record = full_record(fixture, 0);
            record[metadata_start..metadata_end].copy_from_slice(&replacement);

            let authority = build_authority(fixture);
            let usage_before = authority.shared.scan_budget.usage_for_test();
            assert_eq!(
                scan_record(&authority, 0, record).unwrap_err(),
                PhysicalChunkValidationError::DuplicateChannelMetadataKey,
                "duplicate case: {label}"
            );
            assert_eq!(authority.shared.scan_budget.usage_for_test(), usage_before);
            assert_eq!(issue(&authority, 0).core().read_generation.get(), 2);
        }
    }

    #[test]
    fn large_metadata_prefix_scans_linearly_and_detects_a_tail_duplicate() {
        const ENTRY_COUNT: usize = 2048;

        let mut channel = FixtureChannel::schema_less(1, "/fixture");
        for index in 0..ENTRY_COUNT {
            channel
                .metadata
                .insert(format!("k{index:04}"), format!("v{index:04}"));
        }
        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_channels([channel])
            .with_chunk_crc(FixtureCrc::Zero)
            .build()
            .unwrap();

        let authority = build_authority(&fixture);
        let scan = scan_fixture_chunk(&fixture, &authority, 0).unwrap();
        drop(scan);
        assert_eq!(
            authority.shared.scan_budget.usage_for_test().active_scans,
            0
        );

        let ranges = channel_metadata_ranges(&fixture, 0);
        assert_eq!(ranges.len(), ENTRY_COUNT);
        let mut record = full_record(&fixture, 0);
        let first_key = record[ranges[0].0.clone()].to_vec();
        let last_key = &ranges.last().unwrap().0;
        assert_eq!(first_key.len(), last_key.len());
        record[last_key.clone()].copy_from_slice(&first_key);

        let authority = build_authority(&fixture);
        let usage_before = authority.shared.scan_budget.usage_for_test();
        assert_eq!(
            scan_record(&authority, 0, record).unwrap_err(),
            PhysicalChunkValidationError::DuplicateChannelMetadataKey
        );
        assert_eq!(authority.shared.scan_budget.usage_for_test(), usage_before);
        assert_eq!(issue(&authority, 0).core().read_generation.get(), 2);
    }

    #[test]
    fn empty_and_nonempty_zero_are_distinct_semantic_results() {
        let fixture = fixture_with_chunks([
            FixtureChunk::empty(),
            FixtureChunk::new([FixtureMessage::new(1, 0, 0), FixtureMessage::new(1, 1, 0)]),
        ]);
        let authority = build_authority(&fixture);
        let empty = scan_fixture_chunk(&fixture, &authority, 0).unwrap();
        let zero = scan_fixture_chunk(&fixture, &authority, 1).unwrap();
        assert_eq!(empty.extent, ValidatedPhysicalChunkExtent::KnownEmpty);
        assert_eq!(
            zero.extent,
            ValidatedPhysicalChunkExtent::NonEmpty {
                raw_start: 0,
                raw_end: 0,
                canonical_start: TimeInt::try_from(0).unwrap(),
                canonical_end: TimeInt::try_from(0).unwrap(),
            }
        );
    }

    #[test]
    fn every_definition_event_including_unselected_and_tail_is_validated() {
        for accepted in [
            DefinitionFixture::Matching,
            DefinitionFixture::ExactDuplicate,
            DefinitionFixture::UnknownUnreferenced,
        ] {
            let fixture = AdversarialMcapFixtureBuilder::new()
                .with_definition_fixture(accepted)
                .build()
                .unwrap();
            let authority = build_authority(&fixture);
            scan_fixture_chunk(&fixture, &authority, 0).unwrap();
        }

        for rejected in [
            DefinitionFixture::UnknownReferenced,
            DefinitionFixture::SchemaNameConflict,
            DefinitionFixture::SchemaEncodingConflict,
            DefinitionFixture::SchemaDataConflict,
            DefinitionFixture::ChannelSchemaConflict,
            DefinitionFixture::ChannelTopicConflict,
            DefinitionFixture::ChannelEncodingConflict,
            DefinitionFixture::ChannelMetadataConflict,
            DefinitionFixture::UnselectedChannelConflict,
            DefinitionFixture::ChunkTailConflict,
        ] {
            let mut builder =
                AdversarialMcapFixtureBuilder::new().with_definition_fixture(rejected);
            if rejected == DefinitionFixture::UnknownReferenced {
                builder = builder.with_message_index_fault(MessageIndexFault::Missing);
            }
            let fixture = builder.build().unwrap();
            let authority = build_authority(&fixture);
            let expected = if rejected == DefinitionFixture::UnknownReferenced {
                PhysicalChunkValidationError::UnknownMessageChannel
            } else {
                PhysicalChunkValidationError::DefinitionConsistency
            };
            assert_eq!(
                scan_fixture_chunk(&fixture, &authority, 0).unwrap_err(),
                expected,
                "fixture: {rejected:?}"
            );
        }
    }

    #[test]
    fn invalid_unselected_time_and_actual_extent_mismatch_fail_after_crc() {
        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_channels([
                FixtureChannel::schema_less(1, "/selected"),
                FixtureChannel::schema_less(2, "/unselected"),
            ])
            .with_chunks([FixtureChunk::single(FixtureMessage::new(2, 0, u64::MAX))
                .with_header_range(RawTimeRange::new(0, 0))
                .with_index_range(RawTimeRange::new(0, 0))])
            .build()
            .unwrap();
        let authority = build_authority(&fixture);
        assert_eq!(
            scan_fixture_chunk(&fixture, &authority, 0).unwrap_err(),
            PhysicalChunkValidationError::InvalidTemporalValue
        );

        let fixture = fixture_with_chunks([FixtureChunk::single(FixtureMessage::new(1, 0, 10))
            .with_header_range(RawTimeRange::new(5, 5))
            .with_index_range(RawTimeRange::new(5, 5))]);
        let authority = build_authority(&fixture);
        assert_eq!(
            scan_fixture_chunk(&fixture, &authority, 0).unwrap_err(),
            PhysicalChunkValidationError::NonEmptyExtentMismatch
        );

        let fixture = fixture_with_chunks([FixtureChunk::empty()
            .with_header_range(RawTimeRange::new(5, 5))
            .with_index_range(RawTimeRange::new(5, 5))]);
        let authority = build_authority(&fixture);
        assert_eq!(
            scan_fixture_chunk(&fixture, &authority, 0).unwrap_err(),
            PhysicalChunkValidationError::EmptyExtentMismatch
        );
    }

    #[test]
    fn limits_stop_at_the_first_unadmitted_record_or_message() {
        let fixture = fixture_with_chunks([FixtureChunk::new([
            FixtureMessage::new(1, 0, 1),
            FixtureMessage::new(1, 1, 2),
        ])]);
        let mut limits = PhysicalChunkScanLimits::generous();
        limits.max_messages_per_chunk = 1;
        let authority = authority_with_limits(&fixture, limits);
        assert_eq!(
            scan_fixture_chunk(&fixture, &authority, 0).unwrap_err(),
            PhysicalChunkValidationError::MessageLimitExceeded
        );
        assert_eq!(issue(&authority, 0).core().read_generation.get(), 2);

        let mut limits = PhysicalChunkScanLimits::generous();
        limits.max_records_per_chunk = 1;
        let authority = authority_with_limits(&fixture, limits);
        assert_eq!(
            scan_fixture_chunk(&fixture, &authority, 0).unwrap_err(),
            PhysicalChunkValidationError::RecordLimitExceeded
        );
    }

    #[test]
    fn every_nested_payload_definition_and_result_limit_fails_before_retention() {
        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_schemas([FixtureSchema::new(1, "schema", "raw")])
            .with_channels([FixtureChannel::schema_less(1, "/fixture").with_schema(1, "raw")])
            .build()
            .unwrap();
        let mut limits = PhysicalChunkScanLimits::generous();
        limits.max_schema_records_per_chunk = 0;
        let authority = authority_with_limits(&fixture, limits);
        assert_eq!(
            scan_fixture_chunk(&fixture, &authority, 0).unwrap_err(),
            PhysicalChunkValidationError::SchemaRecordLimitExceeded
        );

        let two_metadata_entries = FixtureChannel::schema_less(1, "/fixture")
            .with_metadata("a", "1")
            .with_metadata("b", "2");
        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_channels([two_metadata_entries])
            .build()
            .unwrap();
        let mut limits = PhysicalChunkScanLimits::generous();
        limits.max_channel_metadata_entries_per_record = 1;
        let authority = authority_with_limits(&fixture, limits);
        assert_eq!(
            scan_fixture_chunk(&fixture, &authority, 0).unwrap_err(),
            PhysicalChunkValidationError::ChannelMetadataEntryLimitExceeded
        );

        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_channels([
                FixtureChannel::schema_less(1, "/one").with_metadata("a", "1"),
                FixtureChannel::schema_less(2, "/two").with_metadata("b", "2"),
            ])
            .build()
            .unwrap();
        let mut limits = PhysicalChunkScanLimits::generous();
        limits.max_nested_entries_per_chunk = 1;
        let authority = authority_with_limits(&fixture, limits);
        assert_eq!(
            scan_fixture_chunk(&fixture, &authority, 0).unwrap_err(),
            PhysicalChunkValidationError::NestedEntryLimitExceeded
        );

        let fixture = AdversarialMcapFixtureBuilder::new().build().unwrap();
        type LimitCase = (
            fn(&mut PhysicalChunkScanLimits),
            PhysicalChunkValidationError,
        );
        let cases: [LimitCase; 3] = [
            (
                |limits: &mut PhysicalChunkScanLimits| limits.max_owned_field_bytes_per_chunk = 0,
                PhysicalChunkValidationError::OwnedFieldByteLimitExceeded,
            ),
            (
                |limits: &mut PhysicalChunkScanLimits| limits.max_channel_records_per_chunk = 0,
                PhysicalChunkValidationError::ChannelRecordLimitExceeded,
            ),
            (
                |limits: &mut PhysicalChunkScanLimits| limits.max_result_retained_bytes = 0,
                PhysicalChunkValidationError::ResultRetainedByteLimitExceeded,
            ),
        ];
        for (configure, expected) in cases {
            let mut limits = PhysicalChunkScanLimits::generous();
            configure(&mut limits);
            let authority = authority_with_limits(&fixture, limits);
            assert_eq!(
                scan_fixture_chunk(&fixture, &authority, 0).unwrap_err(),
                expected
            );
        }
        let mut limits = PhysicalChunkScanLimits::generous();
        limits.max_canonical_channels = 0;
        assert_eq!(
            try_authority_with_limits(&fixture, limits).unwrap_err(),
            PhysicalChunkValidationError::CanonicalChannelLimitExceeded
        );

        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_chunks([FixtureChunk::single(
                FixtureMessage::new(1, 0, 1).with_data([1, 2]),
            )])
            .build()
            .unwrap();
        let mut limits = PhysicalChunkScanLimits::generous();
        limits.max_message_payload_bytes_per_chunk = 1;
        let authority = authority_with_limits(&fixture, limits);
        assert_eq!(
            scan_fixture_chunk(&fixture, &authority, 0).unwrap_err(),
            PhysicalChunkValidationError::MessagePayloadByteLimitExceeded
        );
    }

    #[test]
    fn retained_result_capacity_blocks_only_until_the_prior_scan_drops() {
        let fixture = fixture_with_chunks([
            FixtureChunk::single(FixtureMessage::new(1, 0, 1)),
            FixtureChunk::single(FixtureMessage::new(1, 1, 2)),
        ]);
        let limits = PhysicalChunkScanLimits::generous();
        let budget = PhysicalChunkScanBudget {
            state: Arc::new(PhysicalChunkScanBudgetState {
                limits,
                capacity: PhysicalChunkScanBudgetCapacity {
                    max_active_authorities: 1,
                    max_authority_slots: 2,
                    max_authority_slot_bytes: u64::MAX,
                    max_definition_projection_bytes: u64::MAX,
                    max_active_scans: 1,
                    max_active_nested_bytes: u64::MAX,
                    max_retained_results: 1,
                    max_retained_result_bytes: u64::MAX,
                },
                usage: Mutex::new(PhysicalChunkScanBudgetUsage::default()),
            }),
        };
        let state = Arc::clone(&budget.state);
        let authority = PhysicalChunkSourceAuthority::new(
            validated_physical_regions_for_test(&fixture),
            chunk_decompression_budget_for_test(),
            budget,
        )
        .unwrap();
        let first = scan_fixture_chunk(&fixture, &authority, 0).unwrap();
        assert_eq!(state.usage.lock().retained_results, 1);
        assert_eq!(
            scan_fixture_chunk(&fixture, &authority, 1).unwrap_err(),
            PhysicalChunkValidationError::ScanReservationLimitExceeded
        );
        assert_eq!(issue(&authority, 1).core().read_generation.get(), 2);
        drop(first);
        let second = scan_fixture_chunk(&fixture, &authority, 1).unwrap();
        assert_eq!(state.usage.lock().retained_results, 1);
        drop(second);
        drop(authority);
        assert_eq!(*state.usage.lock(), PhysicalChunkScanBudgetUsage::default());
    }

    #[test]
    fn stale_before_header_and_after_decompression_never_publish_semantics() {
        let fixture = AdversarialMcapFixtureBuilder::new().build().unwrap();
        let authority = build_authority(&fixture);
        let lease = issue(&authority, 0);
        authority.close();
        assert_eq!(
            install_exact_physical_chunk_record_for_test(lease, full_record(&fixture, 0))
                .unwrap_err(),
            PhysicalChunkValidationError::SourceClosed
        );

        let authority = build_authority(&fixture);
        let validated = install_exact_physical_chunk_record_for_test(
            issue(&authority, 0),
            full_record(&fixture, 0),
        )
        .unwrap();
        assert_eq!(
            install_header_validated_payload_with_hook_for_test(validated, || authority.close())
                .unwrap_err(),
            PhysicalChunkValidationError::Decompression,
            "a source closed after payload copy cannot enter the decompressor"
        );

        let authority = build_authority(&fixture);
        let output = decompress_fixture_chunk(&fixture, &authority, 0).unwrap();
        authority.close();
        assert_eq!(
            scan_decompressed_physical_chunk(output).unwrap_err(),
            PhysicalChunkValidationError::SourceClosed
        );
    }

    #[test]
    fn cache_eviction_waits_for_consumers_before_fresh_refetch() {
        let fixture = AdversarialMcapFixtureBuilder::new().build().unwrap();
        let authority = build_authority(&fixture);
        let scan = scan_fixture_chunk(&fixture, &authority, 0).unwrap();
        let entry = PhysicalChunkScanCacheEntry::new(scan).unwrap();
        let consumer = entry.consumer();
        assert_eq!(entry.evict(), PhysicalChunkCacheEviction::PendingReclaim);
        assert!(consumer.is_current());
        assert_eq!(
            authority.issue(authority.select(0).unwrap()).unwrap_err(),
            PhysicalChunkValidationError::DuplicateLiveRead
        );
        drop(consumer);
        assert_eq!(issue(&authority, 0).core().read_generation.get(), 2);
    }

    #[test]
    fn retained_cache_consumers_become_stale_when_the_source_closes() {
        let fixture = AdversarialMcapFixtureBuilder::new().build().unwrap();
        let authority = build_authority(&fixture);
        let entry =
            PhysicalChunkScanCacheEntry::new(scan_fixture_chunk(&fixture, &authority, 0).unwrap())
                .unwrap();
        let consumer = entry.consumer();
        authority.close();
        assert!(!consumer.is_current());
        assert_eq!(entry.evict(), PhysicalChunkCacheEviction::PendingReclaim);
        drop(consumer);
        assert_eq!(
            authority.issue(authority.select(0).unwrap()).unwrap_err(),
            PhysicalChunkValidationError::SourceClosed
        );
    }

    #[test]
    fn stale_scan_cannot_enter_cache_or_release_the_replacement_generation() {
        let fixture = AdversarialMcapFixtureBuilder::new().build().unwrap();
        let authority = build_authority(&fixture);
        let stale_scan = scan_fixture_chunk(&fixture, &authority, 0).unwrap();
        authority.shared.state.lock().slots[0] = PhysicalChunkSlot::Vacant { last_generation: 1 };
        let replacement = issue(&authority, 0);
        assert_eq!(replacement.core().read_generation.get(), 2);
        match PhysicalChunkScanCacheEntry::new(stale_scan) {
            Err(error) => assert_eq!(error, PhysicalChunkValidationError::StaleReadGeneration),
            Ok(_) => panic!("a stale semantic result must not enter the cache"),
        }
        assert!(replacement.ensure_current().is_ok());
        drop(replacement);
        assert_eq!(issue(&authority, 0).core().read_generation.get(), 3);
    }

    #[test]
    fn api_shape_has_no_metadata_crc_range_or_box_rebinding_constructor() {
        let source = include_str!("remote_chunk_scan.rs");
        let production = source
            .split("#[cfg(test)]\nmod tests")
            .next()
            .expect("production source precedes tests");
        for forbidden in [
            "PhysicalChunkReadLease::new",
            "PhysicalChunkReadIdentity::new",
            "pub(crate) fn install_exact_physical_chunk_record_for_test",
            "pub(crate) fn install_header_validated_payload_for_test",
            "payload_range(",
            "canonical_ordinal(&self)",
            "read_generation(&self)",
        ] {
            assert!(
                !production.contains(forbidden),
                "production authority surface must not contain {forbidden}"
            );
        }
        assert!(production.contains("struct PendingHeaderValidation"));
        let pending_core = production
            .split("struct PhysicalChunkReadLeaseCore")
            .nth(1)
            .expect("the pending lease core exists")
            .split("pub(crate) struct PhysicalChunkReadLease")
            .next()
            .expect("the pending lease core ends before its wrapper");
        assert!(!pending_core.contains("declared_uncompressed_crc"));
        assert!(production.contains("ChunkChecksumPolicy::ValidateIfProvided"));
        assert!(production.contains("fn validate_physical_chunk_header("));
        assert!(production.contains("Self::Lease(lease) => Ok(lease)"));
        assert!(!production.contains("MessageParser"));
        assert!(!production.contains("ValidatedChunkDispatchPlan"));
        assert!(!production.contains("AdmittedChunkDispatchPlan"));
        assert!(!production.contains(".canonical_channels()"));
        assert!(!production.contains("channels.iter_mut().find"));
        assert!(production.contains("DEFINITION_ID_DOMAIN_LEN"));
        assert!(production.contains("MAX_SLOT_LOOKUPS_PER_QUERY: u64 = 1"));

        for (owner, backing, lease) in [
            (
                "impl Drop for ExactPhysicalChunkRecord",
                "drop(self.bytes.take())",
                "drop(self.lease.take())",
            ),
            (
                "impl Drop for HeaderValidatedPhysicalChunkRead",
                "drop(self.full_record.take())",
                "drop(self.lease.take())",
            ),
        ] {
            let owner = production
                .split(owner)
                .nth(1)
                .expect("the exact physical owner has an explicit Drop implementation");
            let backing = owner.find(backing).expect("the owner drops its backing");
            let lease = owner.find(lease).expect("the owner drops its lease");
            assert!(backing < lease);
        }

        let scan_owner = production
            .split("pub(crate) struct ValidatedPhysicalChunkScan")
            .nth(1)
            .expect("the sealed scan owner exists")
            .split("impl std::fmt::Debug for ValidatedPhysicalChunkScan")
            .next()
            .unwrap();
        let census = scan_owner.find("channels:").unwrap();
        let permit = scan_owner.find("_reservation:").unwrap();
        let lease_owner = scan_owner.find("output:").unwrap();
        assert!(census < permit);
        assert!(permit < lease_owner);

        let decompression = include_str!("remote_decompression.rs");
        let prepared = decompression
            .split("pub(super) struct PreparedExactCompressedChunkInput")
            .nth(1)
            .unwrap()
            .split("impl std::fmt::Debug for PreparedExactCompressedChunkInput")
            .next()
            .unwrap();
        assert!(
            prepared.find("\n    reservation:").unwrap()
                < prepared.find("\n    identity:").unwrap()
        );
    }
}
