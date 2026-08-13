//! Two-stage aggregate admission for ambiguous zero-time indexed Chunks.

use std::ops::{Range, RangeInclusive};
use std::sync::Arc;

use parking_lot::Mutex;

use super::message_index::{
    MessageIndexRegionBudget, MessageIndexRegionBudgetBinding, MessageIndexRegionParse,
    PreparedMessageIndexRegionParse, prepare_message_index_region_with_binding,
};
use super::physical_regions::{
    CanonicalPhysicalRegion, IndexConsistencyViolation, ValidatedPhysicalRegions,
};

const MESSAGE_INDEX_RECORD_FIXED_BYTES: u64 =
    super::RECORD_ENVELOPE_LEN as u64 + size_of::<u16>() as u64 + size_of::<u32>() as u64;
const MESSAGE_INDEX_ENTRY_BYTES: u64 = size_of::<u64>() as u64 * 2;
const MIN_RECORD_ENVELOPE_BYTES: u64 = super::RECORD_ENVELOPE_LEN as u64;

#[derive(Clone, Copy, Debug)]
pub(crate) struct AmbiguousZeroLimits {
    max_ambiguous_chunks: u64,
    max_message_index_bytes: u64,
    max_message_index_entries_upper: u64,
    max_message_index_range_requests: u64,
    max_classification_retained_bytes: u64,
    max_unresolved_chunks: u64,
    max_chunk_range_bytes: u64,
    max_compressed_bytes: u64,
    max_uncompressed_bytes: u64,
    max_chunk_range_requests: u64,
    max_scan_bytes: u64,
    max_scan_records_upper: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct AmbiguousZeroStage1Census {
    ambiguous_chunks: u64,
    message_index_bytes: u64,
    message_index_entries_upper: u64,
    message_index_range_requests: u64,
    classification_retained_bytes: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct AmbiguousZeroStage2Census {
    unresolved_chunks: u64,
    chunk_range_bytes: u64,
    compressed_bytes: u64,
    uncompressed_bytes: u64,
    chunk_range_requests: u64,
    scan_bytes: u64,
    scan_records_upper: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct AmbiguousZeroBudgetUsage {
    active_stage1: u64,
    stage1: AmbiguousZeroStage1Census,
    active_stage2: u64,
    stage2: AmbiguousZeroStage2Census,
}

#[derive(Clone, Copy, Debug)]
struct AmbiguousZeroBudgetCapacity {
    max_active_stage1: u64,
    stage1: AmbiguousZeroStage1Census,
    max_active_stage2: u64,
    stage2: AmbiguousZeroStage2Census,
}

struct AmbiguousZeroBudgetState {
    limits: AmbiguousZeroLimits,
    capacity: AmbiguousZeroBudgetCapacity,
    usage: Mutex<AmbiguousZeroBudgetUsage>,
}

/// Aggregate opening budget shared by all ambiguous-zero flows using one sealed profile.
///
/// There is deliberately no production constructor while remote MCAP remains disarmed.
pub(crate) struct AmbiguousZeroBudget {
    state: Arc<AmbiguousZeroBudgetState>,
}

impl std::fmt::Debug for AmbiguousZeroBudget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AmbiguousZeroBudget")
            .field("limits", &self.state.limits)
            .field("capacity", &self.state.capacity)
            .finish_non_exhaustive()
    }
}

impl AmbiguousZeroBudgetState {
    fn try_reserve_stage1(
        self: &Arc<Self>,
        census: AmbiguousZeroStage1Census,
    ) -> Result<AmbiguousZeroStage1Reservation, IndexConsistencyViolation> {
        let mut usage = self.usage.lock();
        let next = AmbiguousZeroBudgetUsage {
            active_stage1: checked_add(usage.active_stage1, 1)?,
            stage1: add_stage1(usage.stage1, census)?,
            ..*usage
        };
        if next.active_stage1 > self.capacity.max_active_stage1
            || !stage1_fits(next.stage1, self.capacity.stage1)
        {
            return Err(IndexConsistencyViolation::AmbiguousZeroStage1ReservationLimitExceeded);
        }
        *usage = next;
        drop(usage);

        Ok(AmbiguousZeroStage1Reservation {
            state: Arc::clone(self),
            census,
        })
    }

    fn try_reserve_stage2(
        self: &Arc<Self>,
        census: AmbiguousZeroStage2Census,
    ) -> Result<AmbiguousZeroStage2Reservation, IndexConsistencyViolation> {
        let mut usage = self.usage.lock();
        let next = AmbiguousZeroBudgetUsage {
            active_stage2: checked_add(usage.active_stage2, 1)?,
            stage2: add_stage2(usage.stage2, census)?,
            ..*usage
        };
        if next.active_stage2 > self.capacity.max_active_stage2
            || !stage2_fits(next.stage2, self.capacity.stage2)
        {
            return Err(IndexConsistencyViolation::AmbiguousZeroStage2ReservationLimitExceeded);
        }
        *usage = next;
        drop(usage);

        Ok(AmbiguousZeroStage2Reservation {
            state: Arc::clone(self),
            census,
        })
    }
}

struct AmbiguousZeroStage1Reservation {
    state: Arc<AmbiguousZeroBudgetState>,
    census: AmbiguousZeroStage1Census,
}

impl Drop for AmbiguousZeroStage1Reservation {
    fn drop(&mut self) {
        let mut usage = self.state.usage.lock();
        usage.active_stage1 = usage
            .active_stage1
            .checked_sub(1)
            .expect("live ambiguous-zero stage 1 owns one active reservation");
        usage.stage1 = sub_stage1(usage.stage1, self.census);
    }
}

struct AmbiguousZeroStage2Reservation {
    state: Arc<AmbiguousZeroBudgetState>,
    census: AmbiguousZeroStage2Census,
}

impl Drop for AmbiguousZeroStage2Reservation {
    fn drop(&mut self) {
        let mut usage = self.state.usage.lock();
        usage.active_stage2 = usage
            .active_stage2
            .checked_sub(1)
            .expect("live ambiguous-zero stage 2 owns one active reservation");
        usage.stage2 = sub_stage2(usage.stage2, self.census);
    }
}

struct AmbiguousZeroMaterializationToken {
    census: AmbiguousZeroStage1Census,
    reservation: AmbiguousZeroStage1Reservation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AmbiguousChunkClassification {
    PendingMessageIndex,
    Unresolved,
    IndexNonEmptyZero,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct AmbiguousChunkState {
    pub(crate) canonical_ordinal: usize,
    pub(crate) classification: AmbiguousChunkClassification,
}

struct AmbiguousZeroStage1State {
    chunks: Vec<AmbiguousChunkState>,
    next_chunk: usize,
    observed_entries: u64,
    message_index_budget: MessageIndexRegionBudgetBinding,
    reservation: AmbiguousZeroStage1Reservation,
}

/// Move-only first-stage flow.
///
/// Every call to [`Self::next`] produces at most one full-region parser work unit.
pub(crate) struct AmbiguousZeroStage1<'a> {
    physical: ValidatedPhysicalRegions<'a>,
    state: AmbiguousZeroStage1State,
}

impl std::fmt::Debug for AmbiguousZeroStage1<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AmbiguousZeroStage1")
            .field("ambiguous_chunks", &self.state.chunks.len())
            .field("next_chunk", &self.state.next_chunk)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub(crate) enum AmbiguousZeroStage1Turn<'a> {
    MessageIndex(PreparedAmbiguousZeroMessageIndex<'a>),
    BodyPlan(PreparedAmbiguousZeroBodyPlan<'a>),
}

impl<'a> AmbiguousZeroStage1<'a> {
    pub(crate) fn next(mut self) -> Result<AmbiguousZeroStage1Turn<'a>, IndexConsistencyViolation> {
        loop {
            let Some(chunk) = self.state.chunks.get(self.state.next_chunk) else {
                return prepare_stage2(self).map(AmbiguousZeroStage1Turn::BodyPlan);
            };
            match chunk.classification {
                AmbiguousChunkClassification::PendingMessageIndex => {
                    let canonical_ordinal = chunk.canonical_ordinal;
                    let Self { physical, state } = self;
                    let parse = prepare_message_index_region_with_binding(
                        physical,
                        canonical_ordinal,
                        &state.message_index_budget,
                    )?;
                    return Ok(AmbiguousZeroStage1Turn::MessageIndex(
                        PreparedAmbiguousZeroMessageIndex { parse, state },
                    ));
                }
                AmbiguousChunkClassification::Unresolved
                | AmbiguousChunkClassification::IndexNonEmptyZero => {
                    self.state.next_chunk += 1;
                }
            }
        }
    }
}

pub(crate) struct PreparedAmbiguousZeroMessageIndex<'a> {
    parse: PreparedMessageIndexRegionParse<'a>,
    state: AmbiguousZeroStage1State,
}

impl std::fmt::Debug for PreparedAmbiguousZeroMessageIndex<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedAmbiguousZeroMessageIndex")
            .field("expected_range", &self.parse.expected_range())
            .finish_non_exhaustive()
    }
}

impl<'a> PreparedAmbiguousZeroMessageIndex<'a> {
    pub(crate) fn expected_range(&self) -> Range<u64> {
        self.parse.expected_range()
    }

    pub(crate) fn install_raw(
        self,
        file_start: u64,
        bytes: Box<[u8]>,
    ) -> Result<AmbiguousZeroMessageIndexParse<'a>, IndexConsistencyViolation> {
        Ok(AmbiguousZeroMessageIndexParse {
            parse: self.parse.install_raw(file_start, bytes)?,
            state: self.state,
        })
    }
}

pub(crate) struct AmbiguousZeroMessageIndexParse<'a> {
    parse: MessageIndexRegionParse<'a>,
    state: AmbiguousZeroStage1State,
}

impl std::fmt::Debug for AmbiguousZeroMessageIndexParse<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AmbiguousZeroMessageIndexParse")
            .finish_non_exhaustive()
    }
}

impl<'a> AmbiguousZeroMessageIndexParse<'a> {
    pub(crate) fn execute(self) -> Result<AmbiguousZeroStage1<'a>, IndexConsistencyViolation> {
        let validated = self.parse.execute()?;
        let mut state = self.state;
        let mut region_entries = 0_u64;
        for channel in validated.channels() {
            for entry in channel.entries() {
                if entry.log_time != 0 {
                    return Err(IndexConsistencyViolation::AmbiguousZeroIndexTimeMismatch);
                }
                region_entries = checked_add(region_entries, 1)?;
            }
        }
        state.observed_entries = checked_add(state.observed_entries, region_entries)?;
        if state.observed_entries > state.reservation.census.message_index_entries_upper {
            return Err(IndexConsistencyViolation::AmbiguousZeroObservedEntryUpperExceeded);
        }
        let chunk = state
            .chunks
            .get_mut(state.next_chunk)
            .ok_or(IndexConsistencyViolation::UnknownCanonicalRegion)?;
        if chunk.classification != AmbiguousChunkClassification::PendingMessageIndex {
            return Err(IndexConsistencyViolation::UnknownCanonicalRegion);
        }
        chunk.classification = if region_entries == 0 {
            AmbiguousChunkClassification::Unresolved
        } else {
            AmbiguousChunkClassification::IndexNonEmptyZero
        };
        state.next_chunk += 1;
        let physical = validated.into_physical();
        Ok(AmbiguousZeroStage1 { physical, state })
    }
}

/// Sealed output of the second aggregate preflight.
///
/// It proves only which ambiguous Chunks still require a bounded body scan and which were proven
/// nonempty-at-zero by a validated `MessageIndex`. It cannot classify an unresolved Chunk as empty,
/// decompress, scan, or construct final manifest authority.
pub(crate) struct PreparedAmbiguousZeroBodyPlan<'a> {
    physical: ValidatedPhysicalRegions<'a>,
    chunks: Vec<AmbiguousChunkState>,
    stage1_reservation: AmbiguousZeroStage1Reservation,
    stage2_reservation: AmbiguousZeroStage2Reservation,
}

/// Move-only seed consumed exclusively by MCAP-025A.
///
/// Keeping this transition in the defining module prevents a second owner from being assembled
/// from a borrowed physical-region view and copied classification scalars.
pub(crate) struct PreparedAmbiguousZeroResolutionSeed<'a> {
    pub(crate) physical: ValidatedPhysicalRegions<'a>,
    pub(crate) ambiguous_chunks: Vec<AmbiguousChunkState>,
    pub(crate) aggregate_reservations: PreparedAmbiguousZeroAggregateReservations,
}

pub(crate) struct PreparedAmbiguousZeroAggregateReservations {
    _stage1: AmbiguousZeroStage1Reservation,
    _stage2: AmbiguousZeroStage2Reservation,
}

impl std::fmt::Debug for PreparedAmbiguousZeroBodyPlan<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedAmbiguousZeroBodyPlan")
            .field("index_nonempty_zero", &self.index_nonempty_zero_count())
            .field("unresolved", &self.unresolved_count())
            .finish_non_exhaustive()
    }
}

impl<'a> PreparedAmbiguousZeroBodyPlan<'a> {
    pub(crate) const fn physical(&self) -> &ValidatedPhysicalRegions<'a> {
        &self.physical
    }

    pub(crate) fn index_nonempty_zero_count(&self) -> usize {
        self.chunks
            .iter()
            .filter(|chunk| chunk.classification == AmbiguousChunkClassification::IndexNonEmptyZero)
            .count()
    }

    pub(crate) fn unresolved_count(&self) -> usize {
        self.chunks
            .iter()
            .filter(|chunk| chunk.classification == AmbiguousChunkClassification::Unresolved)
            .count()
    }

    pub(crate) fn resolution_classifications_v1(&self) -> &[AmbiguousChunkState] {
        &self.chunks
    }

    pub(crate) fn retained_resolution_evidence_bytes_v1(
        &self,
    ) -> Result<u64, IndexConsistencyViolation> {
        let physical = self.physical.retained_bytes_v1()?;
        let classifications = (self.chunks.len() as u64)
            .checked_mul(size_of::<AmbiguousChunkState>() as u64)
            .ok_or(IndexConsistencyViolation::ArithmeticOverflow)?;
        physical
            .checked_add(classifications)
            .ok_or(IndexConsistencyViolation::ArithmeticOverflow)
    }

    pub(crate) fn index_nonempty_zero_ordinals(&self) -> impl Iterator<Item = usize> + '_ {
        self.index_nonempty_zero()
            .map(|resolution| resolution.canonical_ordinal())
    }

    pub(crate) fn index_nonempty_zero(
        &self,
    ) -> impl Iterator<Item = AmbiguousZeroIndexResolution> + '_ {
        self.chunks.iter().filter_map(|chunk| {
            (chunk.classification == AmbiguousChunkClassification::IndexNonEmptyZero).then_some(
                AmbiguousZeroIndexResolution {
                    canonical_ordinal: chunk.canonical_ordinal,
                    raw_start: 0,
                    raw_end: 0,
                },
            )
        })
    }

    /// Iterates bounded descriptors only; these are not Range-request claims.
    pub(crate) fn unresolved_chunks(
        &self,
    ) -> impl Iterator<Item = AmbiguousZeroUnresolvedChunk<'_>> {
        self.chunks
            .iter()
            .filter(|chunk| chunk.classification == AmbiguousChunkClassification::Unresolved)
            .map(|chunk| {
                let region = self
                    .physical
                    .region(chunk.canonical_ordinal)
                    .expect("sealed ambiguous-zero ordinal belongs to retained physical evidence");
                AmbiguousZeroUnresolvedChunk {
                    canonical_ordinal: chunk.canonical_ordinal,
                    chunk_range: region.chunk_range(),
                    descriptor: region.raw_descriptor(),
                }
            })
    }

    pub(crate) fn into_resolution_seed(self) -> PreparedAmbiguousZeroResolutionSeed<'a> {
        let Self {
            physical,
            chunks,
            stage1_reservation,
            stage2_reservation,
        } = self;
        // Reuse MCAP-023's already-reserved ambiguous classification backing rather than
        // allocating a second canonical-sized projection before MCAP-025A admission.
        re_log::debug_assert!(chunks.iter().all(|chunk| {
            chunk.classification != AmbiguousChunkClassification::PendingMessageIndex
        }));
        let ambiguous_chunks = chunks;
        PreparedAmbiguousZeroResolutionSeed {
            physical,
            ambiguous_chunks,
            aggregate_reservations: PreparedAmbiguousZeroAggregateReservations {
                _stage1: stage1_reservation,
                _stage2: stage2_reservation,
            },
        }
    }

    #[cfg(test)]
    pub(crate) fn reservation_census_for_test(&self) -> ([u64; 5], [u64; 7]) {
        (
            stage1_values(self.stage1_reservation.census),
            stage2_values(self.stage2_reservation.census),
        )
    }
}

/// Raw, pre-canonicalization proof that one indexed ambiguous Chunk is nonempty at time zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AmbiguousZeroIndexResolution {
    canonical_ordinal: usize,
    raw_start: u64,
    raw_end: u64,
}

impl AmbiguousZeroIndexResolution {
    pub(crate) const fn canonical_ordinal(self) -> usize {
        self.canonical_ordinal
    }

    pub(crate) fn raw_time_extent(self) -> RangeInclusive<u64> {
        self.raw_start..=self.raw_end
    }
}

pub(crate) struct AmbiguousZeroUnresolvedChunk<'a> {
    canonical_ordinal: usize,
    chunk_range: Range<u64>,
    descriptor: &'a mcap::records::ChunkIndex,
}

impl std::fmt::Debug for AmbiguousZeroUnresolvedChunk<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AmbiguousZeroUnresolvedChunk")
            .field("canonical_ordinal", &self.canonical_ordinal)
            .field(
                "chunk_range_bytes",
                &(self.chunk_range.end - self.chunk_range.start),
            )
            .finish_non_exhaustive()
    }
}

impl AmbiguousZeroUnresolvedChunk<'_> {
    pub(crate) const fn canonical_ordinal(&self) -> usize {
        self.canonical_ordinal
    }

    pub(crate) fn chunk_range(&self) -> Range<u64> {
        self.chunk_range.clone()
    }

    pub(crate) fn compression(&self) -> &str {
        &self.descriptor.compression
    }

    pub(crate) const fn compressed_size(&self) -> u64 {
        self.descriptor.compressed_size
    }

    pub(crate) const fn uncompressed_size(&self) -> u64 {
        self.descriptor.uncompressed_size
    }
}

pub(crate) fn prepare_ambiguous_zero_stage1<'a>(
    physical: ValidatedPhysicalRegions<'a>,
    budget: &AmbiguousZeroBudget,
    message_index_budget: &MessageIndexRegionBudget,
) -> Result<AmbiguousZeroStage1<'a>, IndexConsistencyViolation> {
    let census = preflight_stage1(&physical, &budget.state.limits)?;
    let token = AmbiguousZeroMaterializationToken {
        census,
        reservation: budget.state.try_reserve_stage1(census)?,
    };
    let (chunks, reservation) = materialize_stage1(&physical, token)?;
    Ok(AmbiguousZeroStage1 {
        physical,
        state: AmbiguousZeroStage1State {
            chunks,
            next_chunk: 0,
            observed_entries: 0,
            message_index_budget: message_index_budget.bind(),
            reservation,
        },
    })
}

fn preflight_stage1(
    physical: &ValidatedPhysicalRegions<'_>,
    limits: &AmbiguousZeroLimits,
) -> Result<AmbiguousZeroStage1Census, IndexConsistencyViolation> {
    let mut census = AmbiguousZeroStage1Census::default();
    for region in physical.regions().filter(is_ambiguous_zero) {
        census.ambiguous_chunks = checked_add(census.ambiguous_chunks, 1)?;
        let region_bytes = range_len(region.message_index_region())?;
        let record_count = u64::try_from(region.raw_descriptor().message_index_offsets.len())
            .map_err(|_overflow| IndexConsistencyViolation::ArithmeticOverflow)?;
        if region_bytes > 0 {
            let fixed_bytes = checked_mul(record_count, MESSAGE_INDEX_RECORD_FIXED_BYTES)?;
            if region_bytes < fixed_bytes {
                return Err(IndexConsistencyViolation::AmbiguousZeroMessageIndexShapeInvalid);
            }
            // Do not subtract descriptor-declared record overhead here: a malformed region may
            // contain fewer records than its map and therefore process more entries before the
            // final bidirectional mapping check fails. Whole-region bytes / entry width remains a
            // conservative upper bound for both successful and eventually rejected shapes.
            let entries_upper = region_bytes / MESSAGE_INDEX_ENTRY_BYTES;
            census.message_index_entries_upper =
                checked_add(census.message_index_entries_upper, entries_upper)?;
            census.message_index_bytes = checked_add(census.message_index_bytes, region_bytes)?;
            census.message_index_range_requests =
                checked_add(census.message_index_range_requests, 1)?;
        }
    }
    census.classification_retained_bytes = checked_mul(
        census.ambiguous_chunks,
        u64::try_from(size_of::<AmbiguousChunkState>())
            .map_err(|_overflow| IndexConsistencyViolation::ArithmeticOverflow)?,
    )?;
    check_stage1_limits(census, limits)?;
    Ok(census)
}

fn materialize_stage1(
    physical: &ValidatedPhysicalRegions<'_>,
    token: AmbiguousZeroMaterializationToken,
) -> Result<(Vec<AmbiguousChunkState>, AmbiguousZeroStage1Reservation), IndexConsistencyViolation> {
    let AmbiguousZeroMaterializationToken {
        census,
        reservation,
    } = token;
    let capacity = usize::try_from(census.ambiguous_chunks)
        .map_err(|_overflow| IndexConsistencyViolation::ArithmeticOverflow)?;
    let mut chunks = Vec::new();
    chunks
        .try_reserve_exact(capacity)
        .map_err(|_error| IndexConsistencyViolation::AmbiguousZeroStage1AllocationFailed)?;
    for (canonical_ordinal, region) in physical.regions().enumerate() {
        if !is_ambiguous_zero(&region) {
            continue;
        }
        let classification = if region.message_index_region().is_empty() {
            AmbiguousChunkClassification::Unresolved
        } else {
            AmbiguousChunkClassification::PendingMessageIndex
        };
        chunks.push(AmbiguousChunkState {
            canonical_ordinal,
            classification,
        });
    }
    if chunks.len() != capacity {
        return Err(IndexConsistencyViolation::AmbiguousZeroStage1AllocationFailed);
    }
    Ok((chunks, reservation))
}

fn prepare_stage2(
    stage1: AmbiguousZeroStage1<'_>,
) -> Result<PreparedAmbiguousZeroBodyPlan<'_>, IndexConsistencyViolation> {
    let AmbiguousZeroStage1 { physical, state } = stage1;
    if state
        .chunks
        .iter()
        .any(|chunk| chunk.classification == AmbiguousChunkClassification::PendingMessageIndex)
    {
        return Err(IndexConsistencyViolation::UnknownCanonicalRegion);
    }
    let census = preflight_stage2(&physical, &state.chunks, &state.reservation.state.limits)?;
    let stage2_reservation = state.reservation.state.try_reserve_stage2(census)?;
    Ok(PreparedAmbiguousZeroBodyPlan {
        physical,
        chunks: state.chunks,
        stage1_reservation: state.reservation,
        stage2_reservation,
    })
}

fn preflight_stage2(
    physical: &ValidatedPhysicalRegions<'_>,
    chunks: &[AmbiguousChunkState],
    limits: &AmbiguousZeroLimits,
) -> Result<AmbiguousZeroStage2Census, IndexConsistencyViolation> {
    let mut census = AmbiguousZeroStage2Census::default();
    for chunk in chunks {
        if chunk.classification != AmbiguousChunkClassification::Unresolved {
            continue;
        }
        let region = physical
            .region(chunk.canonical_ordinal)
            .ok_or(IndexConsistencyViolation::UnknownCanonicalRegion)?;
        let descriptor = region.raw_descriptor();
        census.unresolved_chunks = checked_add(census.unresolved_chunks, 1)?;
        census.chunk_range_bytes =
            checked_add(census.chunk_range_bytes, range_len(region.chunk_range())?)?;
        census.compressed_bytes = checked_add(census.compressed_bytes, descriptor.compressed_size)?;
        census.uncompressed_bytes =
            checked_add(census.uncompressed_bytes, descriptor.uncompressed_size)?;
        census.chunk_range_requests = checked_add(census.chunk_range_requests, 1)?;
        census.scan_bytes = checked_add(census.scan_bytes, descriptor.uncompressed_size)?;
        census.scan_records_upper = checked_add(
            census.scan_records_upper,
            div_ceil(descriptor.uncompressed_size, MIN_RECORD_ENVELOPE_BYTES)?,
        )?;
    }
    check_stage2_limits(census, limits)?;
    Ok(census)
}

fn is_ambiguous_zero(region: &CanonicalPhysicalRegion<'_>) -> bool {
    let descriptor = region.raw_descriptor();
    descriptor.message_start_time == 0 && descriptor.message_end_time == 0
}

fn check_stage1_limits(
    census: AmbiguousZeroStage1Census,
    limits: &AmbiguousZeroLimits,
) -> Result<(), IndexConsistencyViolation> {
    if census.ambiguous_chunks > limits.max_ambiguous_chunks {
        return Err(IndexConsistencyViolation::AmbiguousZeroChunkLimitExceeded);
    }
    if census.message_index_bytes > limits.max_message_index_bytes {
        return Err(IndexConsistencyViolation::AmbiguousZeroMessageIndexByteLimitExceeded);
    }
    if census.message_index_entries_upper > limits.max_message_index_entries_upper {
        return Err(IndexConsistencyViolation::AmbiguousZeroEntryUpperLimitExceeded);
    }
    if census.message_index_range_requests > limits.max_message_index_range_requests {
        return Err(IndexConsistencyViolation::AmbiguousZeroMessageIndexRangeLimitExceeded);
    }
    if census.classification_retained_bytes > limits.max_classification_retained_bytes {
        return Err(IndexConsistencyViolation::AmbiguousZeroRetainedByteLimitExceeded);
    }
    Ok(())
}

fn check_stage2_limits(
    census: AmbiguousZeroStage2Census,
    limits: &AmbiguousZeroLimits,
) -> Result<(), IndexConsistencyViolation> {
    if census.unresolved_chunks > limits.max_unresolved_chunks {
        return Err(IndexConsistencyViolation::AmbiguousZeroUnresolvedChunkLimitExceeded);
    }
    if census.chunk_range_bytes > limits.max_chunk_range_bytes {
        return Err(IndexConsistencyViolation::AmbiguousZeroChunkRangeByteLimitExceeded);
    }
    if census.compressed_bytes > limits.max_compressed_bytes {
        return Err(IndexConsistencyViolation::AmbiguousZeroCompressedByteLimitExceeded);
    }
    if census.uncompressed_bytes > limits.max_uncompressed_bytes {
        return Err(IndexConsistencyViolation::AmbiguousZeroUncompressedByteLimitExceeded);
    }
    if census.chunk_range_requests > limits.max_chunk_range_requests {
        return Err(IndexConsistencyViolation::AmbiguousZeroChunkRangeLimitExceeded);
    }
    if census.scan_bytes > limits.max_scan_bytes {
        return Err(IndexConsistencyViolation::AmbiguousZeroScanByteLimitExceeded);
    }
    if census.scan_records_upper > limits.max_scan_records_upper {
        return Err(IndexConsistencyViolation::AmbiguousZeroScanRecordLimitExceeded);
    }
    Ok(())
}

fn add_stage1(
    left: AmbiguousZeroStage1Census,
    right: AmbiguousZeroStage1Census,
) -> Result<AmbiguousZeroStage1Census, IndexConsistencyViolation> {
    Ok(AmbiguousZeroStage1Census {
        ambiguous_chunks: checked_add(left.ambiguous_chunks, right.ambiguous_chunks)?,
        message_index_bytes: checked_add(left.message_index_bytes, right.message_index_bytes)?,
        message_index_entries_upper: checked_add(
            left.message_index_entries_upper,
            right.message_index_entries_upper,
        )?,
        message_index_range_requests: checked_add(
            left.message_index_range_requests,
            right.message_index_range_requests,
        )?,
        classification_retained_bytes: checked_add(
            left.classification_retained_bytes,
            right.classification_retained_bytes,
        )?,
    })
}

fn add_stage2(
    left: AmbiguousZeroStage2Census,
    right: AmbiguousZeroStage2Census,
) -> Result<AmbiguousZeroStage2Census, IndexConsistencyViolation> {
    Ok(AmbiguousZeroStage2Census {
        unresolved_chunks: checked_add(left.unresolved_chunks, right.unresolved_chunks)?,
        chunk_range_bytes: checked_add(left.chunk_range_bytes, right.chunk_range_bytes)?,
        compressed_bytes: checked_add(left.compressed_bytes, right.compressed_bytes)?,
        uncompressed_bytes: checked_add(left.uncompressed_bytes, right.uncompressed_bytes)?,
        chunk_range_requests: checked_add(left.chunk_range_requests, right.chunk_range_requests)?,
        scan_bytes: checked_add(left.scan_bytes, right.scan_bytes)?,
        scan_records_upper: checked_add(left.scan_records_upper, right.scan_records_upper)?,
    })
}

fn sub_stage1(
    left: AmbiguousZeroStage1Census,
    right: AmbiguousZeroStage1Census,
) -> AmbiguousZeroStage1Census {
    AmbiguousZeroStage1Census {
        ambiguous_chunks: left
            .ambiguous_chunks
            .checked_sub(right.ambiguous_chunks)
            .expect("live stage 1 reservation owns its ambiguous Chunk count"),
        message_index_bytes: left
            .message_index_bytes
            .checked_sub(right.message_index_bytes)
            .expect("live stage 1 reservation owns its MessageIndex bytes"),
        message_index_entries_upper: left
            .message_index_entries_upper
            .checked_sub(right.message_index_entries_upper)
            .expect("live stage 1 reservation owns its entry upper bound"),
        message_index_range_requests: left
            .message_index_range_requests
            .checked_sub(right.message_index_range_requests)
            .expect("live stage 1 reservation owns its Range request count"),
        classification_retained_bytes: left
            .classification_retained_bytes
            .checked_sub(right.classification_retained_bytes)
            .expect("live stage 1 reservation owns its classification bytes"),
    }
}

fn sub_stage2(
    left: AmbiguousZeroStage2Census,
    right: AmbiguousZeroStage2Census,
) -> AmbiguousZeroStage2Census {
    AmbiguousZeroStage2Census {
        unresolved_chunks: left
            .unresolved_chunks
            .checked_sub(right.unresolved_chunks)
            .expect("live stage 2 reservation owns its unresolved Chunk count"),
        chunk_range_bytes: left
            .chunk_range_bytes
            .checked_sub(right.chunk_range_bytes)
            .expect("live stage 2 reservation owns its Chunk Range bytes"),
        compressed_bytes: left
            .compressed_bytes
            .checked_sub(right.compressed_bytes)
            .expect("live stage 2 reservation owns its compressed bytes"),
        uncompressed_bytes: left
            .uncompressed_bytes
            .checked_sub(right.uncompressed_bytes)
            .expect("live stage 2 reservation owns its uncompressed bytes"),
        chunk_range_requests: left
            .chunk_range_requests
            .checked_sub(right.chunk_range_requests)
            .expect("live stage 2 reservation owns its Range request count"),
        scan_bytes: left
            .scan_bytes
            .checked_sub(right.scan_bytes)
            .expect("live stage 2 reservation owns its scan bytes"),
        scan_records_upper: left
            .scan_records_upper
            .checked_sub(right.scan_records_upper)
            .expect("live stage 2 reservation owns its scan record upper bound"),
    }
}

fn stage1_fits(value: AmbiguousZeroStage1Census, cap: AmbiguousZeroStage1Census) -> bool {
    value.ambiguous_chunks <= cap.ambiguous_chunks
        && value.message_index_bytes <= cap.message_index_bytes
        && value.message_index_entries_upper <= cap.message_index_entries_upper
        && value.message_index_range_requests <= cap.message_index_range_requests
        && value.classification_retained_bytes <= cap.classification_retained_bytes
}

fn stage2_fits(value: AmbiguousZeroStage2Census, cap: AmbiguousZeroStage2Census) -> bool {
    value.unresolved_chunks <= cap.unresolved_chunks
        && value.chunk_range_bytes <= cap.chunk_range_bytes
        && value.compressed_bytes <= cap.compressed_bytes
        && value.uncompressed_bytes <= cap.uncompressed_bytes
        && value.chunk_range_requests <= cap.chunk_range_requests
        && value.scan_bytes <= cap.scan_bytes
        && value.scan_records_upper <= cap.scan_records_upper
}

fn range_len(range: Range<u64>) -> Result<u64, IndexConsistencyViolation> {
    range
        .end
        .checked_sub(range.start)
        .ok_or(IndexConsistencyViolation::ArithmeticOverflow)
}

fn div_ceil(value: u64, divisor: u64) -> Result<u64, IndexConsistencyViolation> {
    if value == 0 {
        return Ok(0);
    }
    checked_add((value - 1) / divisor, 1)
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
impl AmbiguousZeroLimits {
    pub(crate) const fn for_test(stage1: [u64; 5], stage2: [u64; 7]) -> Self {
        Self {
            max_ambiguous_chunks: stage1[0],
            max_message_index_bytes: stage1[1],
            max_message_index_entries_upper: stage1[2],
            max_message_index_range_requests: stage1[3],
            max_classification_retained_bytes: stage1[4],
            max_unresolved_chunks: stage2[0],
            max_chunk_range_bytes: stage2[1],
            max_compressed_bytes: stage2[2],
            max_uncompressed_bytes: stage2[3],
            max_chunk_range_requests: stage2[4],
            max_scan_bytes: stage2[5],
            max_scan_records_upper: stage2[6],
        }
    }
}

#[cfg(test)]
impl AmbiguousZeroBudget {
    pub(crate) fn for_test(
        limits: AmbiguousZeroLimits,
        max_active_stage1: u64,
        max_active_stage2: u64,
        aggregate_scale: u64,
    ) -> Self {
        let stage1 = AmbiguousZeroStage1Census {
            ambiguous_chunks: limits.max_ambiguous_chunks,
            message_index_bytes: limits.max_message_index_bytes,
            message_index_entries_upper: limits.max_message_index_entries_upper,
            message_index_range_requests: limits.max_message_index_range_requests,
            classification_retained_bytes: limits.max_classification_retained_bytes,
        };
        let stage2 = AmbiguousZeroStage2Census {
            unresolved_chunks: limits.max_unresolved_chunks,
            chunk_range_bytes: limits.max_chunk_range_bytes,
            compressed_bytes: limits.max_compressed_bytes,
            uncompressed_bytes: limits.max_uncompressed_bytes,
            chunk_range_requests: limits.max_chunk_range_requests,
            scan_bytes: limits.max_scan_bytes,
            scan_records_upper: limits.max_scan_records_upper,
        };
        Self {
            state: Arc::new(AmbiguousZeroBudgetState {
                limits,
                capacity: AmbiguousZeroBudgetCapacity {
                    max_active_stage1,
                    stage1: scale_stage1(stage1, aggregate_scale),
                    max_active_stage2,
                    stage2: scale_stage2(stage2, aggregate_scale),
                },
                usage: Mutex::new(AmbiguousZeroBudgetUsage::default()),
            }),
        }
    }

    pub(crate) fn usage_for_test(&self) -> [u64; 14] {
        let usage = *self.state.usage.lock();
        [
            usage.active_stage1,
            usage.stage1.ambiguous_chunks,
            usage.stage1.message_index_bytes,
            usage.stage1.message_index_entries_upper,
            usage.stage1.message_index_range_requests,
            usage.stage1.classification_retained_bytes,
            usage.active_stage2,
            usage.stage2.unresolved_chunks,
            usage.stage2.chunk_range_bytes,
            usage.stage2.compressed_bytes,
            usage.stage2.uncompressed_bytes,
            usage.stage2.chunk_range_requests,
            usage.stage2.scan_bytes,
            usage.stage2.scan_records_upper,
        ]
    }
}

#[cfg(test)]
fn stage1_values(census: AmbiguousZeroStage1Census) -> [u64; 5] {
    [
        census.ambiguous_chunks,
        census.message_index_bytes,
        census.message_index_entries_upper,
        census.message_index_range_requests,
        census.classification_retained_bytes,
    ]
}

#[cfg(test)]
fn stage2_values(census: AmbiguousZeroStage2Census) -> [u64; 7] {
    [
        census.unresolved_chunks,
        census.chunk_range_bytes,
        census.compressed_bytes,
        census.uncompressed_bytes,
        census.chunk_range_requests,
        census.scan_bytes,
        census.scan_records_upper,
    ]
}

#[cfg(test)]
fn scale_stage1(value: AmbiguousZeroStage1Census, scale: u64) -> AmbiguousZeroStage1Census {
    AmbiguousZeroStage1Census {
        ambiguous_chunks: value.ambiguous_chunks.checked_mul(scale).unwrap(),
        message_index_bytes: value.message_index_bytes.checked_mul(scale).unwrap(),
        message_index_entries_upper: value
            .message_index_entries_upper
            .checked_mul(scale)
            .unwrap(),
        message_index_range_requests: value
            .message_index_range_requests
            .checked_mul(scale)
            .unwrap(),
        classification_retained_bytes: value
            .classification_retained_bytes
            .checked_mul(scale)
            .unwrap(),
    }
}

#[cfg(test)]
fn scale_stage2(value: AmbiguousZeroStage2Census, scale: u64) -> AmbiguousZeroStage2Census {
    AmbiguousZeroStage2Census {
        unresolved_chunks: value.unresolved_chunks.checked_mul(scale).unwrap(),
        chunk_range_bytes: value.chunk_range_bytes.checked_mul(scale).unwrap(),
        compressed_bytes: value.compressed_bytes.checked_mul(scale).unwrap(),
        uncompressed_bytes: value.uncompressed_bytes.checked_mul(scale).unwrap(),
        chunk_range_requests: value.chunk_range_requests.checked_mul(scale).unwrap(),
        scan_bytes: value.scan_bytes.checked_mul(scale).unwrap(),
        scan_records_upper: value.scan_records_upper.checked_mul(scale).unwrap(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_arithmetic_is_checked_before_any_reservation() {
        let stage1 = AmbiguousZeroStage1Census {
            message_index_bytes: u64::MAX,
            ..Default::default()
        };
        assert_eq!(
            add_stage1(
                stage1,
                AmbiguousZeroStage1Census {
                    message_index_bytes: 1,
                    ..Default::default()
                },
            )
            .unwrap_err(),
            IndexConsistencyViolation::ArithmeticOverflow
        );

        let stage2 = AmbiguousZeroStage2Census {
            compressed_bytes: u64::MAX,
            ..Default::default()
        };
        assert_eq!(
            add_stage2(
                stage2,
                AmbiguousZeroStage2Census {
                    compressed_bytes: 1,
                    ..Default::default()
                },
            )
            .unwrap_err(),
            IndexConsistencyViolation::ArithmeticOverflow
        );
        assert_eq!(
            checked_mul(u64::MAX, 2).unwrap_err(),
            IndexConsistencyViolation::ArithmeticOverflow
        );
        assert_eq!(
            div_ceil(u64::MAX, MIN_RECORD_ENVELOPE_BYTES).unwrap(),
            u64::MAX / MIN_RECORD_ENVELOPE_BYTES + 1
        );
    }
}
