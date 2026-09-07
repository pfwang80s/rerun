//! Bounded, best-effort predecessor backfill for Web remote-MCAP seeks.
//!
//! This module is a pure, production-disarmed state layer. It owns no Viewer command routing,
//! Fetch/decode transport, Store facade, `EntityDb`, or storage engine. It only turns an
//! explicitly allowlisted decoder-state policy and a lazily validated `MessageIndex` view into a
//! bounded commit closure containing the complete immutable Channel-group partition.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use re_log_types::TimeInt;

/// The immutable public source description used by successful remote indexed-window seeks.
///
/// This is intentionally a fixed label rather than a single-variant capability enum. Statistics
/// and `MessageIndex` cardinalities cannot upgrade it into an exact or complete source description.
pub(crate) const REMOTE_SEEK_PUBLIC_SOURCE_LABEL_V1: &str = "BestEffortIndexedWindow";

/// Returns the fixed public source description.
///
/// There are deliberately no parameters that could feed Statistics or `MessageIndex` counts into a
/// capability upgrade.
pub(crate) const fn default_public_source_label_v1() -> &'static str {
    REMOTE_SEEK_PUBLIC_SOURCE_LABEL_V1
}

/// Decoder state policy for one remote Channel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum RemoteSeekStatePolicyV1 {
    /// Only the requested window is required. No active predecessor lookup is issued.
    #[default]
    NoDeliberateBackfill,

    /// One Message is allowed to replace the Channel's prior observable state.
    BestEffortLatestMessageBackfill,

    /// The decoder's state semantics are not representable by this MVP boundary.
    UnsupportedForRemote,
}

/// Explicit MVP decoder-state classes used to populate an allowlist.
///
/// The class is part of the decoder contract and is supplied by the caller. It is deliberately not
/// derived from a topic name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteMvpDecoderStateClassV1 {
    SingleMessageReplacement,
    WindowOnly,
    StatefulUnsupported,
}

impl RemoteMvpDecoderStateClassV1 {
    pub(crate) const fn state_policy_v1(self) -> RemoteSeekStatePolicyV1 {
        match self {
            Self::SingleMessageReplacement => {
                RemoteSeekStatePolicyV1::BestEffortLatestMessageBackfill
            }
            Self::WindowOnly => RemoteSeekStatePolicyV1::NoDeliberateBackfill,
            Self::StatefulUnsupported => RemoteSeekStatePolicyV1::UnsupportedForRemote,
        }
    }
}

/// A raw MCAP nanosecond value that must be canonicalized before temporal comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub(crate) struct RemoteRawMcapTimeV1(u64);

impl RemoteRawMcapTimeV1 {
    pub(crate) const fn new_v1(raw: u64) -> Self {
        Self(raw)
    }
}

/// Canonicalizes a raw remote-MCAP nanosecond value for the backfill cursor and candidate order.
fn canonicalize_raw_mcap_time_v1(
    raw: RemoteRawMcapTimeV1,
) -> Result<TimeInt, RemotePredecessorBackfillErrorV1> {
    let signed = i64::try_from(raw.0)
        .map_err(|_overflow| RemotePredecessorBackfillErrorV1::InvalidTemporalValue)?;
    TimeInt::try_from(signed)
        .map_err(|_static_or_invalid| RemotePredecessorBackfillErrorV1::InvalidTemporalValue)
}

/// Channel-keyed decoder-state allowlist.
///
/// The map has no topic or schema-name field. Resolution can therefore only use the explicitly
/// installed Channel ID policy, never a topic heuristic.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RemoteDecoderStateAllowlistV1 {
    policies: BTreeMap<u16, RemoteSeekStatePolicyV1>,
}

impl RemoteDecoderStateAllowlistV1 {
    pub(crate) fn new_v1() -> Self {
        Self::default()
    }

    pub(crate) fn insert_v1(
        &mut self,
        channel_id: u16,
        policy: RemoteSeekStatePolicyV1,
    ) -> Result<(), RemotePredecessorBackfillErrorV1> {
        if self.policies.insert(channel_id, policy).is_some() {
            return Err(RemotePredecessorBackfillErrorV1::DuplicateAllowlistEntry);
        }
        Ok(())
    }

    pub(crate) fn install_class_v1(
        &mut self,
        channel_id: u16,
        class: RemoteMvpDecoderStateClassV1,
    ) -> Result<(), RemotePredecessorBackfillErrorV1> {
        self.insert_v1(channel_id, class.state_policy_v1())
    }

    pub(crate) fn resolve_v1(&self, channel_id: u16) -> Option<RemoteSeekStatePolicyV1> {
        self.policies.get(&channel_id).copied()
    }
}

/// One manifest Chunk extent and its physical owning `MessageIndex` region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemotePredecessorChunkV1 {
    chunk_start_offset: u64,
    message_start_time: TimeInt,
    message_end_time: TimeInt,
    message_index_region_start: u64,
    message_index_region_end: u64,
}

impl RemotePredecessorChunkV1 {
    pub(crate) fn new_v1(
        chunk_start_offset: u64,
        message_start_time: TimeInt,
        message_end_time: TimeInt,
        message_index_region: Range<u64>,
    ) -> Result<Self, RemotePredecessorBackfillErrorV1> {
        if message_start_time.is_static() || message_end_time.is_static() {
            return Err(RemotePredecessorBackfillErrorV1::InvalidTemporalValue);
        }
        if message_start_time > message_end_time {
            return Err(RemotePredecessorBackfillErrorV1::InvalidManifest);
        }
        if message_index_region.start > message_index_region.end {
            return Err(RemotePredecessorBackfillErrorV1::InvalidManifest);
        }
        Ok(Self {
            chunk_start_offset,
            message_start_time,
            message_end_time,
            message_index_region_start: message_index_region.start,
            message_index_region_end: message_index_region.end,
        })
    }

    pub(crate) const fn chunk_start_offset_v1(self) -> u64 {
        self.chunk_start_offset
    }

    pub(crate) const fn message_start_time_v1(self) -> TimeInt {
        self.message_start_time
    }

    pub(crate) const fn message_end_time_v1(self) -> TimeInt {
        self.message_end_time
    }

    pub(crate) fn message_index_region_v1(self) -> Range<u64> {
        self.message_index_region_start..self.message_index_region_end
    }
}

/// Dense identity for one immutable Channel-group partition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct RemotePredecessorPartitionKeyV1 {
    group_id: u32,
    source_generation: u64,
}

impl RemotePredecessorPartitionKeyV1 {
    pub(crate) const fn new_v1(group_id: u32, source_generation: u64) -> Self {
        Self {
            group_id,
            source_generation,
        }
    }

    pub(crate) const fn group_id_v1(self) -> u32 {
        self.group_id
    }

    pub(crate) const fn source_generation_v1(self) -> u64 {
        self.source_generation
    }
}

/// Complete immutable Channel-group partition retained by a commit closure.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct RemotePredecessorChannelGroupPartitionV1 {
    key: RemotePredecessorPartitionKeyV1,
    channels: Box<[u16]>,
}

impl RemotePredecessorChannelGroupPartitionV1 {
    pub(crate) fn new_v1(
        key: RemotePredecessorPartitionKeyV1,
        channels: Vec<u16>,
    ) -> Result<Self, RemotePredecessorBackfillErrorV1> {
        let mut channels = channels;
        channels.sort_unstable();
        channels.dedup();
        if channels.is_empty() {
            return Err(RemotePredecessorBackfillErrorV1::EmptyChannelGroup);
        }
        Ok(Self {
            key,
            channels: channels.into_boxed_slice(),
        })
    }

    pub(crate) const fn key_v1(&self) -> RemotePredecessorPartitionKeyV1 {
        self.key
    }

    pub(crate) fn channels_v1(&self) -> &[u16] {
        &self.channels
    }
}

/// Immutable manifest subset used by predecessor backfill.
///
/// It contains only canonical Chunk extents and complete Channel-group partitions. It has no Store,
/// network, decoder, or index bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemotePredecessorManifestV1 {
    chunks: Box<[RemotePredecessorChunkV1]>,
    channel_groups: Box<[RemotePredecessorChannelGroupPartitionV1]>,
}

impl RemotePredecessorManifestV1 {
    pub(crate) fn new_v1(
        chunks: Vec<RemotePredecessorChunkV1>,
        channel_groups: Vec<RemotePredecessorChannelGroupPartitionV1>,
    ) -> Result<Self, RemotePredecessorBackfillErrorV1> {
        let mut chunk_offsets = BTreeSet::new();
        for chunk in &chunks {
            if !chunk_offsets.insert(chunk.chunk_start_offset) {
                return Err(RemotePredecessorBackfillErrorV1::DuplicateChunkOffset);
            }
        }

        let mut group_keys = BTreeSet::new();
        let mut channel_owners = BTreeMap::new();
        for group in &channel_groups {
            if !group_keys.insert(group.key) {
                return Err(RemotePredecessorBackfillErrorV1::DuplicateGroupPartition);
            }
            for channel_id in group.channels_v1() {
                if channel_owners.insert(*channel_id, group.key).is_some() {
                    return Err(RemotePredecessorBackfillErrorV1::OverlappingChannelGroup);
                }
            }
        }

        Ok(Self {
            chunks: chunks.into_boxed_slice(),
            channel_groups: channel_groups.into_boxed_slice(),
        })
    }

    pub(crate) fn chunks_v1(&self) -> &[RemotePredecessorChunkV1] {
        &self.chunks
    }

    pub(crate) fn channel_groups_v1(&self) -> &[RemotePredecessorChannelGroupPartitionV1] {
        &self.channel_groups
    }

    fn group_for_channel_v1(
        &self,
        channel_id: u16,
    ) -> Option<&RemotePredecessorChannelGroupPartitionV1> {
        self.channel_groups
            .iter()
            .find(|group| group.channels_v1().contains(&channel_id))
    }
}

/// One immutable `MessageIndex` entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemotePredecessorMessageIndexEntryV1 {
    raw_log_time: RemoteRawMcapTimeV1,
    local_record_offset: u64,
}

impl RemotePredecessorMessageIndexEntryV1 {
    pub(crate) const fn new_v1(
        raw_log_time: RemoteRawMcapTimeV1,
        local_record_offset: u64,
    ) -> Self {
        Self {
            raw_log_time,
            local_record_offset,
        }
    }
}

/// Validated per-Channel view of one owning `MessageIndex` region.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteValidatedChannelMessageIndexV1 {
    channel_id: u16,
    entries: Box<[RemotePredecessorMessageIndexEntryV1]>,
}

impl RemoteValidatedChannelMessageIndexV1 {
    pub(crate) fn new_v1(
        channel_id: u16,
        entries: Vec<RemotePredecessorMessageIndexEntryV1>,
    ) -> Result<Self, RemotePredecessorBackfillErrorV1> {
        let mut offsets = BTreeSet::new();
        for entry in &entries {
            if !offsets.insert(entry.local_record_offset) {
                return Err(RemotePredecessorBackfillErrorV1::IndexConsistencyViolation(
                    RemotePredecessorIndexViolationV1::DuplicateRecordOffset,
                ));
            }
        }
        Ok(Self {
            channel_id,
            entries: entries.into_boxed_slice(),
        })
    }

    pub(crate) const fn channel_id_v1(&self) -> u16 {
        self.channel_id
    }

    pub(crate) fn entries_v1(&self) -> &[RemotePredecessorMessageIndexEntryV1] {
        &self.entries
    }
}

/// Complete validated owning `MessageIndex` region for one Chunk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteValidatedMessageIndexRegionV1 {
    chunk_start_offset: u64,
    region: Range<u64>,
    channels: Box<[RemoteValidatedChannelMessageIndexV1]>,
}

impl RemoteValidatedMessageIndexRegionV1 {
    pub(crate) fn new_v1(
        chunk_start_offset: u64,
        region: Range<u64>,
        channels: Vec<RemoteValidatedChannelMessageIndexV1>,
    ) -> Result<Self, RemotePredecessorBackfillErrorV1> {
        if region.start > region.end {
            return Err(RemotePredecessorBackfillErrorV1::InvalidManifest);
        }
        let mut channel_ids = BTreeSet::new();
        for channel in &channels {
            if !channel_ids.insert(channel.channel_id) {
                return Err(RemotePredecessorBackfillErrorV1::IndexConsistencyViolation(
                    RemotePredecessorIndexViolationV1::DuplicateChannelIndex,
                ));
            }
        }
        Ok(Self {
            chunk_start_offset,
            region,
            channels: channels.into_boxed_slice(),
        })
    }

    pub(crate) const fn chunk_start_offset_v1(&self) -> u64 {
        self.chunk_start_offset
    }

    pub(crate) fn region_v1(&self) -> Range<u64> {
        self.region.clone()
    }

    pub(crate) fn byte_len_v1(&self) -> Result<u64, RemotePredecessorBackfillErrorV1> {
        self.region
            .end
            .checked_sub(self.region.start)
            .ok_or(RemotePredecessorBackfillErrorV1::ArithmeticOverflow)
    }

    pub(crate) fn entry_count_v1(&self) -> Result<u64, RemotePredecessorBackfillErrorV1> {
        self.channels.iter().try_fold(0_u64, |count, channel| {
            count
                .checked_add(
                    u64::try_from(channel.entries.len()).map_err(|_overflow| {
                        RemotePredecessorBackfillErrorV1::ArithmeticOverflow
                    })?,
                )
                .ok_or(RemotePredecessorBackfillErrorV1::ArithmeticOverflow)
        })
    }

    fn channel_v1(&self, channel_id: u16) -> Option<&RemoteValidatedChannelMessageIndexV1> {
        self.channels
            .iter()
            .find(|channel| channel.channel_id == channel_id)
    }
}

/// CRC evidence produced by validating a candidate Chunk's uncompressed records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteChunkChecksumEvidenceV1 {
    NotProvided,
    Verified,
    Mismatch,
}

/// One actual uncompressed record from a candidate Chunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteChunkRecordV1 {
    opcode: u8,
    channel_id: u16,
    raw_log_time: RemoteRawMcapTimeV1,
    local_record_offset: u64,
}

impl RemoteChunkRecordV1 {
    pub(crate) const fn new_v1(
        opcode: u8,
        channel_id: u16,
        raw_log_time: RemoteRawMcapTimeV1,
        local_record_offset: u64,
    ) -> Self {
        Self {
            opcode,
            channel_id,
            raw_log_time,
            local_record_offset,
        }
    }
}

/// Complete uncompressed record stream for one candidate Chunk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteValidatedChunkRecordsV1 {
    chunk_start_offset: u64,
    checksum: RemoteChunkChecksumEvidenceV1,
    records: Box<[RemoteChunkRecordV1]>,
}

impl RemoteValidatedChunkRecordsV1 {
    pub(crate) fn new_v1(
        chunk_start_offset: u64,
        checksum: RemoteChunkChecksumEvidenceV1,
        records: Vec<RemoteChunkRecordV1>,
    ) -> Result<Self, RemotePredecessorBackfillErrorV1> {
        let mut offsets = BTreeSet::new();
        for record in &records {
            if !offsets.insert(record.local_record_offset) {
                return Err(RemotePredecessorBackfillErrorV1::IndexConsistencyViolation(
                    RemotePredecessorIndexViolationV1::DuplicateRecordOffset,
                ));
            }
        }
        Ok(Self {
            chunk_start_offset,
            checksum,
            records: records.into_boxed_slice(),
        })
    }

    pub(crate) fn checksum_v1(self) -> RemoteChunkChecksumEvidenceV1 {
        self.checksum
    }
}

/// Explicit transport failure boundary used by callers that move real Range bytes into this layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemotePredecessorTransportFailureV1 {
    Retryable,
    Permanent,
}

/// Structured reasons for candidate/index or record/index disagreement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum RemotePredecessorIndexViolationV1 {
    #[error("remote predecessor MessageIndex region contains duplicate Channel indices")]
    DuplicateChannelIndex,
    #[error("remote predecessor MessageIndex contains duplicate record offsets")]
    DuplicateRecordOffset,
    #[error("remote predecessor MessageIndex region belongs to a different Chunk")]
    RegionChunkMismatch,
    #[error("remote predecessor MessageIndex region does not match the manifest region")]
    RegionRangeMismatch,
    #[error("remote predecessor candidate record is missing from the uncompressed Chunk")]
    MissingCandidateRecord,
    #[error("remote predecessor candidate record opcode is not Message")]
    CandidateOpcodeMismatch,
    #[error("remote predecessor candidate record Channel does not match the index")]
    CandidateChannelMismatch,
    #[error("remote predecessor candidate record raw log_time does not match the index")]
    CandidateRawTimeMismatch,
    #[error("remote predecessor candidate record canonical time does not match the index")]
    CandidateCanonicalTimeMismatch,
}

/// Which cumulative per-seek budget limit was exhausted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum RemotePredecessorBackfillBudgetResourceV1 {
    #[error("remote predecessor backfill Range request budget was exhausted")]
    RangeRequest,
    #[error("remote predecessor backfill MessageIndex byte budget was exhausted")]
    MessageIndexBytes,
    #[error("remote predecessor backfill parsed index-entry budget was exhausted")]
    ParsedIndexEntries,
    #[error("remote predecessor backfill visible-active-time deadline was exhausted")]
    VisibleActiveDeadline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum RemotePredecessorBackfillErrorV1 {
    #[error("remote predecessor backfill found no candidate in indexed data")]
    NotFoundInIndexedData,

    #[error("remote predecessor backfill transport is unavailable")]
    Unavailable,

    #[error("remote predecessor backfill exceeded its shared budget: {0}")]
    BudgetExhausted(RemotePredecessorBackfillBudgetResourceV1),

    #[error("remote predecessor backfill received a Channel with unsupported decoder state")]
    UnsupportedForRemote,

    #[error("remote predecessor backfill candidate/index or record/index disagreement: {0}")]
    IndexConsistencyViolation(RemotePredecessorIndexViolationV1),

    #[error("remote predecessor backfill candidate Chunk checksum mismatch")]
    ChunkChecksumMismatch,

    #[error("remote predecessor backfill received an invalid temporal value")]
    InvalidTemporalValue,

    #[error("remote predecessor backfill manifest is invalid")]
    InvalidManifest,

    #[error("remote predecessor backfill Channel does not belong to an immutable group")]
    MissingChannelGroup,

    #[error("remote predecessor backfill Channel group is empty")]
    EmptyChannelGroup,

    #[error("remote predecessor backfill allowlist contains a duplicate Channel")]
    DuplicateAllowlistEntry,

    #[error("remote predecessor backfill manifest contains duplicate Chunk offsets")]
    DuplicateChunkOffset,

    #[error("remote predecessor backfill manifest contains duplicate group partitions")]
    DuplicateGroupPartition,

    #[error("remote predecessor backfill manifest contains overlapping Channel groups")]
    OverlappingChannelGroup,

    #[error("remote predecessor backfill arithmetic overflowed")]
    ArithmeticOverflow,
}

impl RemotePredecessorBackfillErrorV1 {
    pub(crate) const fn is_degradable_terminal_outcome_v1(self) -> bool {
        matches!(
            self,
            Self::NotFoundInIndexedData | Self::Unavailable | Self::BudgetExhausted(_)
        )
    }
}

/// Cumulative shared budget for one seek across all eligible Channels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemotePredecessorBackfillBudgetV1 {
    max_range_requests: u64,
    max_message_index_bytes: u64,
    max_parsed_index_entries: u64,
    max_transport_attempts: u64,
    max_visible_active_deadline_millis: u64,
    used_range_requests: u64,
    used_message_index_bytes: u64,
    used_parsed_index_entries: u64,
    used_transport_attempts: u64,
    used_visible_active_deadline_millis: u64,
}

impl RemotePredecessorBackfillBudgetV1 {
    pub(crate) const fn new_v1(
        max_range_requests: u64,
        max_message_index_bytes: u64,
        max_parsed_index_entries: u64,
        max_transport_attempts: u64,
        max_visible_active_deadline_millis: u64,
    ) -> Self {
        Self {
            max_range_requests,
            max_message_index_bytes,
            max_parsed_index_entries,
            max_transport_attempts,
            max_visible_active_deadline_millis,
            used_range_requests: 0,
            used_message_index_bytes: 0,
            used_parsed_index_entries: 0,
            used_transport_attempts: 0,
            used_visible_active_deadline_millis: 0,
        }
    }

    pub(crate) fn try_charge_range_request_v1(
        &mut self,
    ) -> Result<(), RemotePredecessorBackfillErrorV1> {
        let next = self
            .used_range_requests
            .checked_add(1)
            .ok_or(RemotePredecessorBackfillErrorV1::ArithmeticOverflow)?;
        if next > self.max_range_requests {
            return Err(RemotePredecessorBackfillErrorV1::BudgetExhausted(
                RemotePredecessorBackfillBudgetResourceV1::RangeRequest,
            ));
        }
        self.used_range_requests = next;
        Ok(())
    }

    pub(crate) fn try_charge_message_index_bytes_v1(
        &mut self,
        bytes: u64,
    ) -> Result<(), RemotePredecessorBackfillErrorV1> {
        let next = self
            .used_message_index_bytes
            .checked_add(bytes)
            .ok_or(RemotePredecessorBackfillErrorV1::ArithmeticOverflow)?;
        if next > self.max_message_index_bytes {
            return Err(RemotePredecessorBackfillErrorV1::BudgetExhausted(
                RemotePredecessorBackfillBudgetResourceV1::MessageIndexBytes,
            ));
        }
        self.used_message_index_bytes = next;
        Ok(())
    }

    pub(crate) fn try_charge_parsed_index_entries_v1(
        &mut self,
        entries: u64,
    ) -> Result<(), RemotePredecessorBackfillErrorV1> {
        let next = self
            .used_parsed_index_entries
            .checked_add(entries)
            .ok_or(RemotePredecessorBackfillErrorV1::ArithmeticOverflow)?;
        if next > self.max_parsed_index_entries {
            return Err(RemotePredecessorBackfillErrorV1::BudgetExhausted(
                RemotePredecessorBackfillBudgetResourceV1::ParsedIndexEntries,
            ));
        }
        self.used_parsed_index_entries = next;
        Ok(())
    }

    pub(crate) fn try_charge_visible_active_deadline_v1(
        &mut self,
        millis: u64,
    ) -> Result<(), RemotePredecessorBackfillErrorV1> {
        let next = self
            .used_visible_active_deadline_millis
            .checked_add(millis)
            .ok_or(RemotePredecessorBackfillErrorV1::ArithmeticOverflow)?;
        if next > self.max_visible_active_deadline_millis {
            return Err(RemotePredecessorBackfillErrorV1::BudgetExhausted(
                RemotePredecessorBackfillBudgetResourceV1::VisibleActiveDeadline,
            ));
        }
        self.used_visible_active_deadline_millis = next;
        Ok(())
    }

    fn try_begin_transport_attempt_v1(&mut self) -> Result<(), RemotePredecessorBackfillErrorV1> {
        if self.used_transport_attempts >= self.max_transport_attempts {
            return Err(RemotePredecessorBackfillErrorV1::Unavailable);
        }
        self.used_transport_attempts = self
            .used_transport_attempts
            .checked_add(1)
            .ok_or(RemotePredecessorBackfillErrorV1::ArithmeticOverflow)?;
        Ok(())
    }
}

/// Candidate ordering tuple. Field order is intentionally fixed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RemotePredecessorCandidateOrderV1 {
    log_time: TimeInt,
    chunk_start_offset: u64,
    local_record_offset: u64,
}

impl RemotePredecessorCandidateOrderV1 {
    fn is_better_than_v1(self, other: Self) -> bool {
        self.cmp(&other) == Ordering::Greater
    }
}

impl PartialOrd for RemotePredecessorCandidateOrderV1 {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RemotePredecessorCandidateOrderV1 {
    fn cmp(&self, other: &Self) -> Ordering {
        self.log_time
            .cmp(&other.log_time)
            .then_with(|| self.chunk_start_offset.cmp(&other.chunk_start_offset))
            .then_with(|| self.local_record_offset.cmp(&other.local_record_offset))
    }
}

/// Fully validated predecessor candidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemotePredecessorCandidateV1 {
    channel_id: u16,
    log_time: TimeInt,
    chunk_start_offset: u64,
    local_record_offset: u64,
}

impl RemotePredecessorCandidateV1 {
    pub(crate) const fn channel_id_v1(self) -> u16 {
        self.channel_id
    }

    pub(crate) const fn log_time_v1(self) -> TimeInt {
        self.log_time
    }

    pub(crate) const fn chunk_start_offset_v1(self) -> u64 {
        self.chunk_start_offset
    }

    pub(crate) const fn local_record_offset_v1(self) -> u64 {
        self.local_record_offset
    }
}

/// Immutable commit closure produced by a successful backfill.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RemotePredecessorCommitClosureV1 {
    presentation_required: BTreeSet<RemotePredecessorChannelGroupPartitionV1>,
}

impl RemotePredecessorCommitClosureV1 {
    pub(crate) fn presentation_required_v1(
        &self,
    ) -> &BTreeSet<RemotePredecessorChannelGroupPartitionV1> {
        &self.presentation_required
    }
}

/// Result of one best-effort predecessor backfill.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RemotePredecessorBackfillOutcomeV1 {
    Found {
        candidate: RemotePredecessorCandidateV1,
        commit_closure: RemotePredecessorCommitClosureV1,
    },
    NotFoundInIndexedData,
    NoDeliberateBackfill,
}

impl RemotePredecessorBackfillOutcomeV1 {
    pub(crate) const fn is_degradable_terminal_outcome_v1(&self) -> bool {
        matches!(self, Self::NotFoundInIndexedData)
    }
}

fn load_index_with_retry_v1<F>(
    budget: &mut RemotePredecessorBackfillBudgetV1,
    mut loader: F,
) -> Result<RemoteValidatedMessageIndexRegionV1, RemotePredecessorBackfillErrorV1>
where
    F: FnMut() -> Result<RemoteValidatedMessageIndexRegionV1, RemotePredecessorTransportFailureV1>,
{
    loop {
        budget.try_begin_transport_attempt_v1()?;
        budget.try_charge_range_request_v1()?;
        budget.try_charge_visible_active_deadline_v1(1)?;
        match loader() {
            Ok(region) => return Ok(region),
            Err(RemotePredecessorTransportFailureV1::Permanent) => {
                return Err(RemotePredecessorBackfillErrorV1::Unavailable);
            }
            Err(RemotePredecessorTransportFailureV1::Retryable) => {}
        }
    }
}

fn load_chunk_with_retry_v1<F>(
    budget: &mut RemotePredecessorBackfillBudgetV1,
    mut loader: F,
) -> Result<RemoteValidatedChunkRecordsV1, RemotePredecessorBackfillErrorV1>
where
    F: FnMut() -> Result<RemoteValidatedChunkRecordsV1, RemotePredecessorTransportFailureV1>,
{
    loop {
        budget.try_begin_transport_attempt_v1()?;
        budget.try_charge_range_request_v1()?;
        budget.try_charge_visible_active_deadline_v1(1)?;
        match loader() {
            Ok(records) => return Ok(records),
            Err(RemotePredecessorTransportFailureV1::Permanent) => {
                return Err(RemotePredecessorBackfillErrorV1::Unavailable);
            }
            Err(RemotePredecessorTransportFailureV1::Retryable) => {}
        }
    }
}

impl RemotePredecessorManifestV1 {
    /// Performs the bounded reverse lookup for one cursor.
    ///
    /// All eligible Channels share `budget`. The index and Chunk loaders are intentionally
    /// callbacks so this production-disarmed layer never owns a network adapter.
    pub(crate) fn backfill_v1<I, C>(
        &self,
        target_channels: &[u16],
        allowlist: &RemoteDecoderStateAllowlistV1,
        cursor: TimeInt,
        budget: &mut RemotePredecessorBackfillBudgetV1,
        mut load_index: I,
        mut load_chunk: C,
    ) -> Result<RemotePredecessorBackfillOutcomeV1, RemotePredecessorBackfillErrorV1>
    where
        I: FnMut(
            &RemotePredecessorChunkV1,
        ) -> Result<
            RemoteValidatedMessageIndexRegionV1,
            RemotePredecessorTransportFailureV1,
        >,
        C: FnMut(
            &RemotePredecessorChunkV1,
        )
            -> Result<RemoteValidatedChunkRecordsV1, RemotePredecessorTransportFailureV1>,
    {
        if cursor.is_static() {
            return Err(RemotePredecessorBackfillErrorV1::InvalidTemporalValue);
        }

        let target_channels: BTreeSet<_> = target_channels.iter().copied().collect();
        if target_channels.is_empty() {
            return Ok(RemotePredecessorBackfillOutcomeV1::NoDeliberateBackfill);
        }

        let mut eligible_channels = Vec::new();
        for channel_id in target_channels {
            if self.group_for_channel_v1(channel_id).is_none() {
                return Err(RemotePredecessorBackfillErrorV1::MissingChannelGroup);
            }
            match allowlist.resolve_v1(channel_id) {
                None | Some(RemoteSeekStatePolicyV1::NoDeliberateBackfill) => {}
                Some(RemoteSeekStatePolicyV1::BestEffortLatestMessageBackfill) => {
                    eligible_channels.push(channel_id);
                }
                Some(RemoteSeekStatePolicyV1::UnsupportedForRemote) => {
                    return Err(RemotePredecessorBackfillErrorV1::UnsupportedForRemote);
                }
            }
        }

        if eligible_channels.is_empty() {
            return Ok(RemotePredecessorBackfillOutcomeV1::NoDeliberateBackfill);
        }

        let mut candidate_chunks: Vec<&RemotePredecessorChunkV1> = self
            .chunks
            .iter()
            .filter(|chunk| chunk.message_start_time <= cursor)
            .collect();
        candidate_chunks.sort_by(|left, right| {
            right
                .message_end_time
                .cmp(&left.message_end_time)
                .then_with(|| right.chunk_start_offset.cmp(&left.chunk_start_offset))
        });

        let mut loaded_message_index = BTreeMap::<u64, RemoteValidatedMessageIndexRegionV1>::new();
        let mut best: Option<(RemotePredecessorCandidateOrderV1, RemoteRawMcapTimeV1, u16)> = None;

        for chunk in candidate_chunks {
            if let Some((best_order, _, _)) = best
                && chunk.message_end_time < best_order.log_time
            {
                break;
            }

            let region = if let Some(region) = loaded_message_index.get(&chunk.chunk_start_offset) {
                region
            } else {
                let region = load_index_with_retry_v1(budget, || load_index(chunk))?;
                if region.chunk_start_offset != chunk.chunk_start_offset {
                    return Err(RemotePredecessorBackfillErrorV1::IndexConsistencyViolation(
                        RemotePredecessorIndexViolationV1::RegionChunkMismatch,
                    ));
                }
                if region.region != chunk.message_index_region_v1() {
                    return Err(RemotePredecessorBackfillErrorV1::IndexConsistencyViolation(
                        RemotePredecessorIndexViolationV1::RegionRangeMismatch,
                    ));
                }
                budget.try_charge_message_index_bytes_v1(region.byte_len_v1()?)?;
                budget.try_charge_parsed_index_entries_v1(region.entry_count_v1()?)?;
                loaded_message_index.insert(chunk.chunk_start_offset, region);
                loaded_message_index
                    .get(&chunk.chunk_start_offset)
                    .expect("the validated MessageIndex region was just inserted")
            };

            let mut chunk_candidate: Option<(
                RemotePredecessorCandidateOrderV1,
                RemoteRawMcapTimeV1,
                u16,
            )> = None;
            for channel_id in &eligible_channels {
                let Some(channel_index) = region.channel_v1(*channel_id) else {
                    continue;
                };
                for entry in channel_index.entries_v1() {
                    let log_time = canonicalize_raw_mcap_time_v1(entry.raw_log_time)?;
                    if log_time > cursor {
                        continue;
                    }
                    let order = RemotePredecessorCandidateOrderV1 {
                        log_time,
                        chunk_start_offset: chunk.chunk_start_offset,
                        local_record_offset: entry.local_record_offset,
                    };
                    if chunk_candidate
                        .as_ref()
                        .is_none_or(|(current, _, _)| order.is_better_than_v1(*current))
                    {
                        chunk_candidate = Some((order, entry.raw_log_time, *channel_id));
                    }
                }
            }

            let Some((chunk_order, raw_log_time, channel_id)) = chunk_candidate else {
                continue;
            };

            let records = load_chunk_with_retry_v1(budget, || load_chunk(chunk))?;
            if records.chunk_start_offset != chunk.chunk_start_offset {
                return Err(RemotePredecessorBackfillErrorV1::IndexConsistencyViolation(
                    RemotePredecessorIndexViolationV1::RegionChunkMismatch,
                ));
            }
            if records.checksum == RemoteChunkChecksumEvidenceV1::Mismatch {
                return Err(RemotePredecessorBackfillErrorV1::ChunkChecksumMismatch);
            }

            let actual = records
                .records
                .iter()
                .find(|record| record.local_record_offset == chunk_order.local_record_offset)
                .ok_or(RemotePredecessorBackfillErrorV1::IndexConsistencyViolation(
                    RemotePredecessorIndexViolationV1::MissingCandidateRecord,
                ))?;
            if actual.opcode != mcap::records::op::MESSAGE {
                return Err(RemotePredecessorBackfillErrorV1::IndexConsistencyViolation(
                    RemotePredecessorIndexViolationV1::CandidateOpcodeMismatch,
                ));
            }
            if actual.channel_id != channel_id {
                return Err(RemotePredecessorBackfillErrorV1::IndexConsistencyViolation(
                    RemotePredecessorIndexViolationV1::CandidateChannelMismatch,
                ));
            }
            if actual.raw_log_time != raw_log_time {
                return Err(RemotePredecessorBackfillErrorV1::IndexConsistencyViolation(
                    RemotePredecessorIndexViolationV1::CandidateRawTimeMismatch,
                ));
            }
            let actual_canonical_time = canonicalize_raw_mcap_time_v1(actual.raw_log_time)?;
            if actual_canonical_time != chunk_order.log_time {
                return Err(RemotePredecessorBackfillErrorV1::IndexConsistencyViolation(
                    RemotePredecessorIndexViolationV1::CandidateCanonicalTimeMismatch,
                ));
            }

            if best
                .as_ref()
                .is_none_or(|(current, _, _)| chunk_order.is_better_than_v1(*current))
            {
                best = Some((chunk_order, raw_log_time, channel_id));
            }
        }

        let Some((order, _, channel_id)) = best else {
            return Ok(RemotePredecessorBackfillOutcomeV1::NotFoundInIndexedData);
        };

        let group = self
            .group_for_channel_v1(channel_id)
            .cloned()
            .ok_or(RemotePredecessorBackfillErrorV1::MissingChannelGroup)?;
        let mut commit_closure = RemotePredecessorCommitClosureV1::default();
        commit_closure.presentation_required.insert(group);

        Ok(RemotePredecessorBackfillOutcomeV1::Found {
            candidate: RemotePredecessorCandidateV1 {
                channel_id,
                log_time: order.log_time,
                chunk_start_offset: order.chunk_start_offset,
                local_record_offset: order.local_record_offset,
            },
            commit_closure,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const MESSAGE_OPCODE: u8 = mcap::records::op::MESSAGE;

    fn time(value: i64) -> TimeInt {
        TimeInt::new_temporal(value)
    }

    fn chunk(
        offset: u64,
        start: i64,
        end: i64,
        index_region: Range<u64>,
    ) -> RemotePredecessorChunkV1 {
        RemotePredecessorChunkV1::new_v1(offset, time(start), time(end), index_region).unwrap()
    }

    fn group(
        group_id: u32,
        generation: u64,
        channels: &[u16],
    ) -> RemotePredecessorChannelGroupPartitionV1 {
        RemotePredecessorChannelGroupPartitionV1::new_v1(
            RemotePredecessorPartitionKeyV1::new_v1(group_id, generation),
            channels.to_vec(),
        )
        .unwrap()
    }

    fn channel_index(
        channel_id: u16,
        entries: &[(u64, u64)],
    ) -> RemoteValidatedChannelMessageIndexV1 {
        RemoteValidatedChannelMessageIndexV1::new_v1(
            channel_id,
            entries
                .iter()
                .map(|(raw, offset)| {
                    RemotePredecessorMessageIndexEntryV1::new_v1(
                        RemoteRawMcapTimeV1::new_v1(*raw),
                        *offset,
                    )
                })
                .collect(),
        )
        .unwrap()
    }

    fn region(
        chunk: &RemotePredecessorChunkV1,
        channels: Vec<RemoteValidatedChannelMessageIndexV1>,
    ) -> RemoteValidatedMessageIndexRegionV1 {
        RemoteValidatedMessageIndexRegionV1::new_v1(
            chunk.chunk_start_offset_v1(),
            chunk.message_index_region_v1(),
            channels,
        )
        .unwrap()
    }

    fn record(opcode: u8, channel_id: u16, raw: u64, offset: u64) -> RemoteChunkRecordV1 {
        RemoteChunkRecordV1::new_v1(opcode, channel_id, RemoteRawMcapTimeV1::new_v1(raw), offset)
    }

    fn records(
        chunk: &RemotePredecessorChunkV1,
        checksum: RemoteChunkChecksumEvidenceV1,
        values: Vec<RemoteChunkRecordV1>,
    ) -> RemoteValidatedChunkRecordsV1 {
        RemoteValidatedChunkRecordsV1::new_v1(chunk.chunk_start_offset_v1(), checksum, values)
            .unwrap()
    }

    fn allowlist_with(entries: &[(u16, RemoteSeekStatePolicyV1)]) -> RemoteDecoderStateAllowlistV1 {
        let mut allowlist = RemoteDecoderStateAllowlistV1::new_v1();
        for (channel, policy) in entries {
            allowlist.insert_v1(*channel, *policy).unwrap();
        }
        allowlist
    }

    fn generous_budget() -> RemotePredecessorBackfillBudgetV1 {
        RemotePredecessorBackfillBudgetV1::new_v1(100, 100_000, 100_000, 10, 100_000)
    }

    #[test]
    fn public_source_is_fixed_and_does_not_upgrade_from_statistics_or_index_counts() {
        assert_eq!(
            default_public_source_label_v1(),
            REMOTE_SEEK_PUBLIC_SOURCE_LABEL_V1
        );
        assert_eq!(default_public_source_label_v1(), "BestEffortIndexedWindow");

        assert_eq!(
            RemoteMvpDecoderStateClassV1::SingleMessageReplacement.state_policy_v1(),
            RemoteSeekStatePolicyV1::BestEffortLatestMessageBackfill
        );
        assert_eq!(
            RemoteMvpDecoderStateClassV1::WindowOnly.state_policy_v1(),
            RemoteSeekStatePolicyV1::NoDeliberateBackfill
        );
        assert_eq!(
            RemoteMvpDecoderStateClassV1::StatefulUnsupported.state_policy_v1(),
            RemoteSeekStatePolicyV1::UnsupportedForRemote
        );
    }

    #[test]
    fn allowlist_is_channel_keyed_and_stateful_classes_do_not_fake_replacement_state() {
        let allowlist = allowlist_with(&[
            (1, RemoteSeekStatePolicyV1::BestEffortLatestMessageBackfill),
            (2, RemoteSeekStatePolicyV1::UnsupportedForRemote),
        ]);
        assert_eq!(
            allowlist.resolve_v1(1),
            Some(RemoteSeekStatePolicyV1::BestEffortLatestMessageBackfill)
        );
        assert_eq!(
            allowlist.resolve_v1(2),
            Some(RemoteSeekStatePolicyV1::UnsupportedForRemote)
        );
        assert_eq!(
            RemoteMvpDecoderStateClassV1::StatefulUnsupported.state_policy_v1(),
            RemoteSeekStatePolicyV1::UnsupportedForRemote
        );
        assert_eq!(
            RemoteMvpDecoderStateClassV1::WindowOnly.state_policy_v1(),
            RemoteSeekStatePolicyV1::NoDeliberateBackfill
        );
    }

    #[test]
    fn no_deliberate_backfill_issues_no_lookup() {
        let chunk = chunk(100, 10, 20, 200..300);
        let manifest =
            RemotePredecessorManifestV1::new_v1(vec![chunk], vec![group(1, 7, &[1])]).unwrap();
        let allowlist = allowlist_with(&[(1, RemoteSeekStatePolicyV1::NoDeliberateBackfill)]);
        let index_reads = Cell::new(0_u64);
        let chunk_reads = Cell::new(0_u64);
        let mut budget = generous_budget();

        let outcome = manifest
            .backfill_v1(
                &[1],
                &allowlist,
                time(15),
                &mut budget,
                |_| {
                    index_reads.set(index_reads.get() + 1);
                    Ok(region(&chunk, vec![channel_index(1, &[(12, 1)])]))
                },
                |_| {
                    chunk_reads.set(chunk_reads.get() + 1);
                    Ok(records(
                        &chunk,
                        RemoteChunkChecksumEvidenceV1::Verified,
                        vec![record(MESSAGE_OPCODE, 1, 12, 1)],
                    ))
                },
            )
            .unwrap();

        assert_eq!(
            outcome,
            RemotePredecessorBackfillOutcomeV1::NoDeliberateBackfill
        );
        assert_eq!(index_reads.get(), 0);
        assert_eq!(chunk_reads.get(), 0);
    }

    #[test]
    fn lazy_cursor_reads_only_the_first_required_message_index_region() {
        let older = chunk(100, 10, 20, 1_000..1_010);
        let newer = chunk(200, 21, 30, 2_000..2_010);
        let manifest =
            RemotePredecessorManifestV1::new_v1(vec![older, newer], vec![group(1, 7, &[1])])
                .unwrap();
        let allowlist =
            allowlist_with(&[(1, RemoteSeekStatePolicyV1::BestEffortLatestMessageBackfill)]);
        let index_reads = Cell::new(0_u64);
        let mut budget = generous_budget();

        let outcome = manifest
            .backfill_v1(
                &[1],
                &allowlist,
                time(30),
                &mut budget,
                |chunk| {
                    index_reads.set(index_reads.get() + 1);
                    let entries = if chunk.chunk_start_offset_v1() == 200 {
                        vec![(29, 4)]
                    } else {
                        vec![(19, 2)]
                    };
                    Ok(region(chunk, vec![channel_index(1, &entries)]))
                },
                |chunk| {
                    let raw = if chunk.chunk_start_offset_v1() == 200 {
                        29
                    } else {
                        19
                    };
                    let offset = if chunk.chunk_start_offset_v1() == 200 {
                        4
                    } else {
                        2
                    };
                    Ok(records(
                        chunk,
                        RemoteChunkChecksumEvidenceV1::Verified,
                        vec![record(MESSAGE_OPCODE, 1, raw, offset)],
                    ))
                },
            )
            .unwrap();

        let RemotePredecessorBackfillOutcomeV1::Found {
            candidate,
            commit_closure,
        } = outcome
        else {
            panic!("lazy predecessor must find the newer candidate");
        };
        assert_eq!(candidate.chunk_start_offset_v1(), 200);
        assert_eq!(candidate.log_time_v1(), time(29));
        assert_eq!(commit_closure.presentation_required_v1().len(), 1);
        assert_eq!(index_reads.get(), 1);
    }

    #[test]
    fn equal_end_time_is_not_used_as_a_premature_stop_condition() {
        let earlier_file_order = chunk(100, 10, 100, 1_000..1_010);
        let later_file_order = chunk(200, 10, 100, 2_000..2_010);
        let manifest = RemotePredecessorManifestV1::new_v1(
            vec![earlier_file_order, later_file_order],
            vec![group(1, 7, &[1])],
        )
        .unwrap();
        let allowlist =
            allowlist_with(&[(1, RemoteSeekStatePolicyV1::BestEffortLatestMessageBackfill)]);
        let index_reads = Cell::new(0_u64);
        let mut budget = generous_budget();

        let outcome = manifest
            .backfill_v1(
                &[1],
                &allowlist,
                time(100),
                &mut budget,
                |chunk| {
                    index_reads.set(index_reads.get() + 1);
                    let offset = if chunk.chunk_start_offset_v1() == 200 {
                        4
                    } else {
                        2
                    };
                    Ok(region(chunk, vec![channel_index(1, &[(100, offset)])]))
                },
                |chunk| {
                    let offset = if chunk.chunk_start_offset_v1() == 200 {
                        4
                    } else {
                        2
                    };
                    Ok(records(
                        chunk,
                        RemoteChunkChecksumEvidenceV1::Verified,
                        vec![record(MESSAGE_OPCODE, 1, 100, offset)],
                    ))
                },
            )
            .unwrap();

        let RemotePredecessorBackfillOutcomeV1::Found { candidate, .. } = outcome else {
            panic!("the equal-end candidate must be found");
        };
        assert_eq!(candidate.chunk_start_offset_v1(), 200);
        assert_eq!(candidate.local_record_offset_v1(), 4);
        assert_eq!(index_reads.get(), 2);
    }

    #[test]
    fn record_opcode_channel_raw_time_and_canonical_time_are_validated() {
        let chunk = chunk(100, 10, 20, 200..220);
        let manifest =
            RemotePredecessorManifestV1::new_v1(vec![chunk], vec![group(1, 7, &[1])]).unwrap();
        let allowlist =
            allowlist_with(&[(1, RemoteSeekStatePolicyV1::BestEffortLatestMessageBackfill)]);

        for (record, expected_reason) in [
            (
                record(0x01, 1, 12, 1),
                RemotePredecessorIndexViolationV1::CandidateOpcodeMismatch,
            ),
            (
                record(MESSAGE_OPCODE, 2, 12, 1),
                RemotePredecessorIndexViolationV1::CandidateChannelMismatch,
            ),
            (
                record(MESSAGE_OPCODE, 1, 13, 1),
                RemotePredecessorIndexViolationV1::CandidateRawTimeMismatch,
            ),
        ] {
            let mut budget = generous_budget();
            let error = manifest
                .backfill_v1(
                    &[1],
                    &allowlist,
                    time(20),
                    &mut budget,
                    |chunk| Ok(region(chunk, vec![channel_index(1, &[(12, 1)])])),
                    |chunk| {
                        Ok(records(
                            chunk,
                            RemoteChunkChecksumEvidenceV1::Verified,
                            vec![record],
                        ))
                    },
                )
                .unwrap_err();
            assert_eq!(
                error,
                RemotePredecessorBackfillErrorV1::IndexConsistencyViolation(expected_reason)
            );
            assert!(!error.is_degradable_terminal_outcome_v1());
        }
    }

    #[test]
    fn checksum_mismatch_is_not_degradable() {
        let chunk = chunk(100, 10, 20, 200..220);
        let manifest =
            RemotePredecessorManifestV1::new_v1(vec![chunk], vec![group(1, 7, &[1])]).unwrap();
        let allowlist =
            allowlist_with(&[(1, RemoteSeekStatePolicyV1::BestEffortLatestMessageBackfill)]);
        let mut budget = generous_budget();

        let error = manifest
            .backfill_v1(
                &[1],
                &allowlist,
                time(20),
                &mut budget,
                |chunk| Ok(region(chunk, vec![channel_index(1, &[(12, 1)])])),
                |chunk| {
                    Ok(records(
                        chunk,
                        RemoteChunkChecksumEvidenceV1::Mismatch,
                        vec![record(MESSAGE_OPCODE, 1, 12, 1)],
                    ))
                },
            )
            .unwrap_err();
        assert_eq!(
            error,
            RemotePredecessorBackfillErrorV1::ChunkChecksumMismatch
        );
        assert!(!error.is_degradable_terminal_outcome_v1());
    }

    #[test]
    fn budget_limits_are_shared_and_degradable() {
        let chunk = chunk(100, 10, 20, 200..220);
        let manifest =
            RemotePredecessorManifestV1::new_v1(vec![chunk], vec![group(1, 7, &[1, 2])]).unwrap();
        let allowlist = allowlist_with(&[
            (1, RemoteSeekStatePolicyV1::BestEffortLatestMessageBackfill),
            (2, RemoteSeekStatePolicyV1::BestEffortLatestMessageBackfill),
        ]);

        let mut parsed_entry_budget =
            RemotePredecessorBackfillBudgetV1::new_v1(100, 100_000, 1, 10, 100_000);
        let error = manifest
            .backfill_v1(
                &[1, 2],
                &allowlist,
                time(20),
                &mut parsed_entry_budget,
                |chunk| {
                    Ok(region(
                        chunk,
                        vec![channel_index(1, &[(12, 1)]), channel_index(2, &[(13, 2)])],
                    ))
                },
                |_| unreachable!("the parsed-entry budget must fail before Chunk loading"),
            )
            .unwrap_err();
        assert_eq!(
            error,
            RemotePredecessorBackfillErrorV1::BudgetExhausted(
                RemotePredecessorBackfillBudgetResourceV1::ParsedIndexEntries
            )
        );
        assert!(error.is_degradable_terminal_outcome_v1());

        let mut range_budget =
            RemotePredecessorBackfillBudgetV1::new_v1(0, 100_000, 100, 10, 100_000);
        let error = manifest
            .backfill_v1(
                &[1],
                &allowlist,
                time(20),
                &mut range_budget,
                |_| unreachable!("the Range budget must fail before index loading"),
                |_| unreachable!("the Range budget must fail before Chunk loading"),
            )
            .unwrap_err();
        assert_eq!(
            error,
            RemotePredecessorBackfillErrorV1::BudgetExhausted(
                RemotePredecessorBackfillBudgetResourceV1::RangeRequest
            )
        );
    }

    #[test]
    fn retryable_transport_exhaustion_and_not_found_are_degradable() {
        let chunk = chunk(100, 10, 20, 200..220);
        let manifest =
            RemotePredecessorManifestV1::new_v1(vec![chunk], vec![group(1, 7, &[1])]).unwrap();
        let allowlist =
            allowlist_with(&[(1, RemoteSeekStatePolicyV1::BestEffortLatestMessageBackfill)]);
        let mut retry_budget =
            RemotePredecessorBackfillBudgetV1::new_v1(100, 100_000, 100, 2, 100_000);
        let error = manifest
            .backfill_v1(
                &[1],
                &allowlist,
                time(20),
                &mut retry_budget,
                |_| Err(RemotePredecessorTransportFailureV1::Retryable),
                |_| unreachable!("the index transport never succeeds"),
            )
            .unwrap_err();
        assert_eq!(error, RemotePredecessorBackfillErrorV1::Unavailable);
        assert!(error.is_degradable_terminal_outcome_v1());

        let mut not_found_budget = generous_budget();
        let outcome = manifest
            .backfill_v1(
                &[1],
                &allowlist,
                time(20),
                &mut not_found_budget,
                |chunk| Ok(region(chunk, vec![channel_index(1, &[])])),
                |_| unreachable!("no indexed candidate means no Chunk should be loaded"),
            )
            .unwrap();
        assert_eq!(
            outcome,
            RemotePredecessorBackfillOutcomeV1::NotFoundInIndexedData
        );
        assert!(outcome.is_degradable_terminal_outcome_v1());
    }

    #[test]
    fn unsupported_for_remote_does_not_run_a_lookup() {
        let chunk = chunk(100, 10, 20, 200..220);
        let manifest =
            RemotePredecessorManifestV1::new_v1(vec![chunk], vec![group(1, 7, &[1])]).unwrap();
        let allowlist = allowlist_with(&[(1, RemoteSeekStatePolicyV1::UnsupportedForRemote)]);
        let index_reads = Cell::new(0_u64);
        let mut budget = generous_budget();

        let error = manifest
            .backfill_v1(
                &[1],
                &allowlist,
                time(20),
                &mut budget,
                |_| {
                    index_reads.set(index_reads.get() + 1);
                    Ok(region(&chunk, vec![channel_index(1, &[(12, 1)])]))
                },
                |_| unreachable!("unsupported state must not load a Chunk"),
            )
            .unwrap_err();
        assert_eq!(
            error,
            RemotePredecessorBackfillErrorV1::UnsupportedForRemote
        );
        assert_eq!(index_reads.get(), 0);
    }

    #[test]
    fn commit_closure_contains_the_complete_channel_group_partition() {
        let chunk = chunk(100, 10, 20, 200..220);
        let manifest =
            RemotePredecessorManifestV1::new_v1(vec![chunk], vec![group(1, 7, &[1, 2, 3])])
                .unwrap();
        let allowlist =
            allowlist_with(&[(2, RemoteSeekStatePolicyV1::BestEffortLatestMessageBackfill)]);
        let mut budget = generous_budget();

        let outcome = manifest
            .backfill_v1(
                &[2],
                &allowlist,
                time(20),
                &mut budget,
                |chunk| Ok(region(chunk, vec![channel_index(2, &[(12, 1)])])),
                |chunk| {
                    Ok(records(
                        chunk,
                        RemoteChunkChecksumEvidenceV1::Verified,
                        vec![record(MESSAGE_OPCODE, 2, 12, 1)],
                    ))
                },
            )
            .unwrap();

        let RemotePredecessorBackfillOutcomeV1::Found {
            candidate,
            commit_closure,
        } = outcome
        else {
            panic!("the complete Channel-group closure must be produced");
        };
        assert_eq!(candidate.channel_id_v1(), 2);
        let partitions = commit_closure.presentation_required_v1();
        assert_eq!(partitions.len(), 1);
        let partition = partitions.iter().next().unwrap();
        assert_eq!(partition.key_v1().group_id_v1(), 1);
        assert_eq!(partition.key_v1().source_generation_v1(), 7);
        assert_eq!(partition.channels_v1(), &[1, 2, 3]);
    }

    #[test]
    fn visible_active_deadline_is_a_cumulative_shared_budget() {
        let chunk = chunk(100, 10, 20, 200..220);
        let manifest =
            RemotePredecessorManifestV1::new_v1(vec![chunk], vec![group(1, 7, &[1])]).unwrap();
        let allowlist =
            allowlist_with(&[(1, RemoteSeekStatePolicyV1::BestEffortLatestMessageBackfill)]);
        let mut budget = RemotePredecessorBackfillBudgetV1::new_v1(100, 100_000, 100, 10, 0);

        let error = manifest
            .backfill_v1(
                &[1],
                &allowlist,
                time(20),
                &mut budget,
                |_| unreachable!("the deadline must fail before index loading"),
                |_| unreachable!("the deadline must fail before Chunk loading"),
            )
            .unwrap_err();
        assert_eq!(
            error,
            RemotePredecessorBackfillErrorV1::BudgetExhausted(
                RemotePredecessorBackfillBudgetResourceV1::VisibleActiveDeadline
            )
        );
        assert!(error.is_degradable_terminal_outcome_v1());
    }

    #[test]
    fn source_contains_no_storage_or_viewer_boundary_imports() {
        let source = include_str!("remote_predecessor_backfill.rs");
        let import_lines: Vec<_> = source
            .lines()
            .filter(|line| line.trim_start().starts_with("use "))
            .collect();
        for forbidden in [
            "ViewerContext",
            "AppContext",
            "StoreHub",
            "StoreBundle",
            "EntityDb",
            "ChunkStore",
            "re_entity_db",
            "re_viewer_context",
            "re_chunk_store",
        ] {
            assert!(
                !import_lines.iter().any(|line| line.contains(forbidden)),
                "unexpected {forbidden} import"
            );
        }
    }
}
