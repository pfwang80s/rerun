//! Bounded three-layer remote-MCAP window planner.
//!
//! This module stays production-disarmed. It has no HTTP client, browser clock, retry
//! coordinator, or Store mutation side effects. It computes the disjunct physical-window demand
//! sets and then expands the two read-required layers into immutable partition identities.

use ahash::HashSet;

use re_chunk_store::{ChunkStore, WebRemoteMcapRootCapabilityV1};
use re_log_types::TimeInt;

use crate::remote_channel_group::StableDecoderGroupIdV1;
use crate::remote_loaded_coverage::CanonicalIndexedExtentV1;
use crate::remote_manifest::{
    DerivationPartitionKeyV1, ImmutableRemoteMcapManifestV1, RemoteMcapSessionIdV1,
};
use crate::remote_partition_residency::RefetchableRootIndexV1;
use crate::remote_physical_resolution::ResolvedCanonicalPhysicalLayoutV1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum RemoteWindowDemandClassV1 {
    PresentationRequired,
    PlaybackResumeRequired,
    PrefetchDesired,
}

impl RemoteWindowDemandClassV1 {
    pub(crate) const fn display_name_v1(self) -> &'static str {
        match self {
            Self::PresentationRequired => "presentation_required",
            Self::PlaybackResumeRequired => "playback_resume_required",
            Self::PrefetchDesired => "prefetch_desired",
        }
    }
}

/// Planning limits for the three-layer window demand.
///
/// These are deliberately local to `re_mcap`; they are not the same as the Web accounting scopes
/// and never borrow a metadata-opening owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteWindowDemandLimitsV1 {
    pub(crate) max_window_chunk_hits: u64,
    pub(crate) max_selected_channel_groups: u64,
    pub(crate) max_planning_cross_product_ops: u64,
    pub(crate) max_generation_partitions: u64,
}

impl RemoteWindowDemandLimitsV1 {
    pub(crate) const fn generous_disarmed_v1() -> Self {
        Self {
            max_window_chunk_hits: 1_000_000,
            max_selected_channel_groups: 256,
            max_planning_cross_product_ops: 1_000_000,
            max_generation_partitions: 1_000_000,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum RemoteWindowDemandErrorV1 {
    #[error("remote MCAP window demand arithmetic overflowed")]
    ArithmeticOverflow,

    #[error("remote MCAP window demand exceeds a resource limit")]
    ResourceLimitExceeded,

    #[error("remote MCAP window demand parameters are invalid")]
    InvalidWindow,

    #[error("remote MCAP window demand references an invalid partition")]
    InvalidPartition,

    #[error("the remote MCAP window demand phase owner does not match")]
    PhaseOwnerMismatch,

    #[error("remote MCAP prefetch demand is not bound to an exact physical-Chunk body owner")]
    PrefetchUnstarted,
}

/// Frozen caller phase identity for one demand class.
///
/// The identity includes the source generation and complete stable demand key. It is independent
/// for each of presentation, playback-resume, and prefetch, so callers cannot substitute the
/// metadata-opening owner for any later retry policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RemoteWindowDemandPhaseIdentityV1 {
    session_id: RemoteMcapSessionIdV1,
    source_generation: u64,
    class: RemoteWindowDemandClassV1,
    cursor: TimeInt,
    minimum_buffer: TimeInt,
    desired_prefetch: TimeInt,
    playing: bool,
}

impl RemoteWindowDemandPhaseIdentityV1 {
    pub(crate) const fn class_v1(&self) -> RemoteWindowDemandClassV1 {
        self.class
    }

    pub(crate) const fn source_generation_v1(self) -> u64 {
        self.source_generation
    }
}

/// Cumulative retry-policy inputs frozen with a matching phase owner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RemoteWindowDemandPolicyInputsV1 {
    pub(crate) cumulative_range_requests: u64,
    pub(crate) active_visible_deadline_millis: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteWindowDemandPhaseStateV1 {
    Uninstalled,
    PrefetchUnstarted,
    Installed(RemoteWindowDemandPolicyInputsV1),
}

/// Sealed phase-owner installation state for one demand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteWindowDemandPhaseV1 {
    identity: RemoteWindowDemandPhaseIdentityV1,
    state: RemoteWindowDemandPhaseStateV1,
}

impl RemoteWindowDemandPhaseV1 {
    pub(crate) fn new_uninstalled(
        class: RemoteWindowDemandClassV1,
        identity: RemoteWindowDemandPhaseIdentityV1,
    ) -> Self {
        Self {
            identity,
            state: if class == RemoteWindowDemandClassV1::PrefetchDesired {
                RemoteWindowDemandPhaseStateV1::PrefetchUnstarted
            } else {
                RemoteWindowDemandPhaseStateV1::Uninstalled
            },
        }
    }

    pub(crate) const fn identity_v1(&self) -> RemoteWindowDemandPhaseIdentityV1 {
        self.identity
    }

    pub(crate) const fn policy_inputs_v1(&self) -> Option<RemoteWindowDemandPolicyInputsV1> {
        match self.state {
            RemoteWindowDemandPhaseStateV1::Installed(policy_inputs) => Some(policy_inputs),
            RemoteWindowDemandPhaseStateV1::Uninstalled
            | RemoteWindowDemandPhaseStateV1::PrefetchUnstarted => None,
        }
    }

    pub(crate) const fn can_start_retry_v1(&self) -> bool {
        matches!(self.state, RemoteWindowDemandPhaseStateV1::Installed(_))
    }

    pub(crate) const fn is_prefetch_unstarted_v1(&self) -> bool {
        matches!(
            self.state,
            RemoteWindowDemandPhaseStateV1::PrefetchUnstarted
        )
    }

    pub(crate) fn install_matching_phase_owner_v1(
        &mut self,
        identity: RemoteWindowDemandPhaseIdentityV1,
        policy_inputs: RemoteWindowDemandPolicyInputsV1,
    ) -> Result<(), RemoteWindowDemandErrorV1> {
        if self.identity != identity {
            return Err(RemoteWindowDemandErrorV1::PhaseOwnerMismatch);
        }
        match self.state {
            RemoteWindowDemandPhaseStateV1::PrefetchUnstarted => {
                Err(RemoteWindowDemandErrorV1::PrefetchUnstarted)
            }
            RemoteWindowDemandPhaseStateV1::Uninstalled
            | RemoteWindowDemandPhaseStateV1::Installed(_) => {
                self.state = RemoteWindowDemandPhaseStateV1::Installed(policy_inputs);
                Ok(())
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteWindowDemandV1 {
    class: RemoteWindowDemandClassV1,
    phase: RemoteWindowDemandPhaseV1,
}

impl RemoteWindowDemandV1 {
    pub(crate) fn new_uninstalled(
        class: RemoteWindowDemandClassV1,
        identity: RemoteWindowDemandPhaseIdentityV1,
    ) -> Self {
        Self {
            class,
            phase: RemoteWindowDemandPhaseV1::new_uninstalled(class, identity),
        }
    }

    pub(crate) const fn class_v1(&self) -> RemoteWindowDemandClassV1 {
        self.class
    }

    pub(crate) const fn phase_v1(&self) -> &RemoteWindowDemandPhaseV1 {
        &self.phase
    }

    pub(crate) fn install_matching_phase_owner_v1(
        &mut self,
        identity: RemoteWindowDemandPhaseIdentityV1,
        policy_inputs: RemoteWindowDemandPolicyInputsV1,
    ) -> Result<(), RemoteWindowDemandErrorV1> {
        self.phase
            .install_matching_phase_owner_v1(identity, policy_inputs)
    }

    pub(crate) const fn can_start_retry_v1(&self) -> bool {
        self.phase.can_start_retry_v1()
    }

    /// Returns `true` when this demand is an explicitly unsupported prefetch phase.
    ///
    /// Prefetch demand currently has no sealed exact physical-Chunk body owner/lease binding
    /// available to turn it into a startable retry owner. Callers must keep this state visible
    /// instead of treating it as a generic uninstalled phase.
    pub(crate) const fn is_prefetch_unstarted_v1(&self) -> bool {
        self.phase.is_prefetch_unstarted_v1()
    }
}

/// Deduplicated physical-Chunk ordinal sets for the three demand layers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteWindowOrdinalPlanV1 {
    cursor_ordinals: Box<[usize]>,
    minimum_buffer_ordinals: Box<[usize]>,
    desired_prefetch_ordinals: Box<[usize]>,
    required_union_ordinals: Box<[usize]>,
}

impl RemoteWindowOrdinalPlanV1 {
    pub(crate) fn cursor_ordinals_v1(&self) -> &[usize] {
        &self.cursor_ordinals
    }

    pub(crate) fn minimum_buffer_ordinals_v1(&self) -> &[usize] {
        &self.minimum_buffer_ordinals
    }

    pub(crate) fn desired_prefetch_ordinals_v1(&self) -> &[usize] {
        &self.desired_prefetch_ordinals
    }

    pub(crate) fn required_union_ordinals_v1(&self) -> &[usize] {
        &self.required_union_ordinals
    }
}

/// Complete planner result after partition expansion and residency subtraction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteWindowDemandPlanV1 {
    ordinal_plan: RemoteWindowOrdinalPlanV1,
    phase_demands: Vec<RemoteWindowDemandV1>,
    presentation_required: Vec<DerivationPartitionKeyV1>,
    playback_resume_required: Vec<DerivationPartitionKeyV1>,
    satisfied_partitions: Vec<DerivationPartitionKeyV1>,
    missing_presentation: Vec<DerivationPartitionKeyV1>,
    missing_resume: Vec<DerivationPartitionKeyV1>,
    priority_2: Vec<DerivationPartitionKeyV1>,
    mutation_commit_required: Vec<DerivationPartitionKeyV1>,
}

impl RemoteWindowDemandPlanV1 {
    pub(crate) const fn ordinal_plan_v1(&self) -> &RemoteWindowOrdinalPlanV1 {
        &self.ordinal_plan
    }

    pub(crate) fn demand_for_class_v1(
        &self,
        class: RemoteWindowDemandClassV1,
    ) -> Option<&RemoteWindowDemandV1> {
        self.phase_demands
            .iter()
            .find(|demand| demand.class_v1() == class)
    }

    pub(crate) fn presentation_required_v1(&self) -> &[DerivationPartitionKeyV1] {
        &self.presentation_required
    }

    pub(crate) fn playback_resume_required_v1(&self) -> &[DerivationPartitionKeyV1] {
        &self.playback_resume_required
    }

    pub(crate) fn satisfied_partitions_v1(&self) -> &[DerivationPartitionKeyV1] {
        &self.satisfied_partitions
    }

    pub(crate) fn missing_presentation_v1(&self) -> &[DerivationPartitionKeyV1] {
        &self.missing_presentation
    }

    pub(crate) fn missing_resume_v1(&self) -> &[DerivationPartitionKeyV1] {
        &self.missing_resume
    }

    pub(crate) fn priority_2_v1(&self) -> &[DerivationPartitionKeyV1] {
        &self.priority_2
    }

    pub(crate) fn mutation_commit_required_v1(&self) -> &[DerivationPartitionKeyV1] {
        &self.mutation_commit_required
    }
}

fn half_open_window_end_v1(
    start: TimeInt,
    duration: TimeInt,
) -> Result<TimeInt, RemoteWindowDemandErrorV1> {
    let min_i = i64::MIN as i128;
    let max_i = i64::MAX as i128;
    let exclusive_end = i128::from(start.as_i64())
        .checked_add(i128::from(duration.as_i64()))
        .ok_or(RemoteWindowDemandErrorV1::ArithmeticOverflow)?;
    let inclusive_end = exclusive_end
        .checked_sub(1)
        .ok_or(RemoteWindowDemandErrorV1::InvalidWindow)?;
    if !(min_i..=max_i).contains(&inclusive_end) {
        return Err(RemoteWindowDemandErrorV1::ResourceLimitExceeded);
    }
    Ok(TimeInt::new_temporal(inclusive_end as i64))
}

fn collect_bounded_ordinals_v1(
    iter: impl Iterator<Item = usize>,
    limit: u64,
) -> Result<Vec<usize>, RemoteWindowDemandErrorV1> {
    let limit_usize = usize::try_from(limit)
        .map_err(|_error| RemoteWindowDemandErrorV1::ResourceLimitExceeded)?;
    let mut ordinals = Vec::new();
    for ordinal in iter {
        if ordinals.len() >= limit_usize {
            return Err(RemoteWindowDemandErrorV1::ResourceLimitExceeded);
        }
        ordinals
            .try_reserve(1)
            .map_err(|_error| RemoteWindowDemandErrorV1::ResourceLimitExceeded)?;
        ordinals.push(ordinal);
    }
    ordinals.sort_unstable();
    ordinals.dedup();
    Ok(ordinals)
}

fn collect_bounded_group_ids_v1(
    iter: impl Iterator<Item = StableDecoderGroupIdV1>,
    limit: u64,
) -> Result<Vec<StableDecoderGroupIdV1>, RemoteWindowDemandErrorV1> {
    let limit_usize = usize::try_from(limit)
        .map_err(|_error| RemoteWindowDemandErrorV1::ResourceLimitExceeded)?;
    let mut groups = Vec::new();
    for group in iter {
        if groups.len() >= limit_usize {
            return Err(RemoteWindowDemandErrorV1::ResourceLimitExceeded);
        }
        groups
            .try_reserve(1)
            .map_err(|_error| RemoteWindowDemandErrorV1::ResourceLimitExceeded)?;
        groups.push(group);
    }
    Ok(groups)
}

fn union_sorted_ordinals_v1(
    first: &[usize],
    second: &[usize],
) -> Result<Vec<usize>, RemoteWindowDemandErrorV1> {
    let output_limit = first
        .len()
        .checked_add(second.len())
        .ok_or(RemoteWindowDemandErrorV1::ArithmeticOverflow)?;
    let mut output = Vec::new();
    let mut first_index = 0_usize;
    let mut second_index = 0_usize;
    while first_index < first.len() || second_index < second.len() {
        let next = match (first.get(first_index), second.get(second_index)) {
            (Some(&left), Some(&right)) if left < right => {
                first_index += 1;
                left
            }
            (Some(&left), Some(&right)) if left > right => {
                second_index += 1;
                right
            }
            (Some(&left), Some(_)) => {
                first_index += 1;
                second_index += 1;
                left
            }
            (Some(&left), None) => {
                first_index += 1;
                left
            }
            (None, Some(&right)) => {
                second_index += 1;
                right
            }
            (None, None) => break,
        };
        if output.len() >= output_limit {
            return Err(RemoteWindowDemandErrorV1::ResourceLimitExceeded);
        }
        output
            .try_reserve(1)
            .map_err(|_error| RemoteWindowDemandErrorV1::ResourceLimitExceeded)?;
        output.push(next);
    }
    Ok(output)
}

fn exclude_sorted_ordinals_v1(
    values: &[usize],
    excluded: &[usize],
    limit: u64,
) -> Result<Vec<usize>, RemoteWindowDemandErrorV1> {
    let limit_usize = usize::try_from(limit)
        .map_err(|_error| RemoteWindowDemandErrorV1::ResourceLimitExceeded)?;
    let mut output = Vec::new();
    let mut excluded_index = 0_usize;
    for &value in values {
        while excluded
            .get(excluded_index)
            .is_some_and(|&candidate| candidate < value)
        {
            excluded_index += 1;
        }
        if excluded.get(excluded_index) == Some(&value) {
            continue;
        }
        if output.len() >= limit_usize {
            return Err(RemoteWindowDemandErrorV1::ResourceLimitExceeded);
        }
        output
            .try_reserve(1)
            .map_err(|_error| RemoteWindowDemandErrorV1::ResourceLimitExceeded)?;
        output.push(value);
    }
    Ok(output)
}

/// Computes the three disjoint physical-Chunk ordinal sets.
///
/// This is kept separate from partition expansion so the physical window can be differentially
/// checked against upstream's indexed reader without constructing a manifest.
pub(crate) fn plan_window_ordinals_v1(
    layout: &ResolvedCanonicalPhysicalLayoutV1,
    cursor: TimeInt,
    minimum_buffer: TimeInt,
    desired_prefetch: TimeInt,
    limits: RemoteWindowDemandLimitsV1,
) -> Result<RemoteWindowOrdinalPlanV1, RemoteWindowDemandErrorV1> {
    if cursor.is_static() || minimum_buffer.as_i64() <= 0 || desired_prefetch < minimum_buffer {
        return Err(RemoteWindowDemandErrorV1::InvalidWindow);
    }
    let minimum_end = half_open_window_end_v1(cursor, minimum_buffer)?;
    let desired_end = half_open_window_end_v1(cursor, desired_prefetch)?;

    let cursor_ordinals = collect_bounded_ordinals_v1(
        layout.intersecting_ordinals_v1(cursor, cursor),
        limits.max_window_chunk_hits,
    )?;
    let minimum_buffer_ordinals = collect_bounded_ordinals_v1(
        layout.intersecting_ordinals_v1(cursor, minimum_end),
        limits.max_window_chunk_hits,
    )?;
    let desired_ordinals = collect_bounded_ordinals_v1(
        layout.intersecting_ordinals_v1(cursor, desired_end),
        limits.max_window_chunk_hits,
    )?;
    let required_union_ordinals =
        union_sorted_ordinals_v1(&cursor_ordinals, &minimum_buffer_ordinals)?;
    let desired_prefetch_ordinals = exclude_sorted_ordinals_v1(
        &desired_ordinals,
        &required_union_ordinals,
        limits.max_window_chunk_hits,
    )?;

    Ok(RemoteWindowOrdinalPlanV1 {
        cursor_ordinals: cursor_ordinals.into_boxed_slice(),
        minimum_buffer_ordinals: minimum_buffer_ordinals.into_boxed_slice(),
        desired_prefetch_ordinals: desired_prefetch_ordinals.into_boxed_slice(),
        required_union_ordinals: required_union_ordinals.into_boxed_slice(),
    })
}

fn validate_temporal_cursor_v1(
    indexed_extent: CanonicalIndexedExtentV1,
    cursor: TimeInt,
) -> Result<(), RemoteWindowDemandErrorV1> {
    match indexed_extent {
        CanonicalIndexedExtentV1::NoIndexedMessages => {
            Err(RemoteWindowDemandErrorV1::InvalidWindow)
        }
        CanonicalIndexedExtentV1::Known(extent) if !extent.contains(cursor) => {
            Err(RemoteWindowDemandErrorV1::InvalidWindow)
        }
        CanonicalIndexedExtentV1::Known(_) => Ok(()),
    }
}

fn ordinal_plan_with_desired_prefetch_excluding_v1(
    plan: RemoteWindowOrdinalPlanV1,
    excluded_ordinals: &[usize],
    limit: u64,
) -> Result<RemoteWindowOrdinalPlanV1, RemoteWindowDemandErrorV1> {
    let desired_prefetch_ordinals = exclude_sorted_ordinals_v1(
        plan.desired_prefetch_ordinals_v1(),
        excluded_ordinals,
        limit,
    )?
    .into_boxed_slice();
    Ok(RemoteWindowOrdinalPlanV1 {
        desired_prefetch_ordinals,
        ..plan
    })
}

fn validate_backfill_partition_v1(
    partition: DerivationPartitionKeyV1,
    session_id: RemoteMcapSessionIdV1,
    source_generation: u64,
    selected_groups: &[StableDecoderGroupIdV1],
    partition_for: &impl Fn(
        usize,
        StableDecoderGroupIdV1,
    ) -> Result<DerivationPartitionKeyV1, RemoteWindowDemandErrorV1>,
) -> Result<(), RemoteWindowDemandErrorV1> {
    if partition.session_id_v1() != session_id
        || partition.source_generation_v1() != source_generation
    {
        return Err(RemoteWindowDemandErrorV1::InvalidPartition);
    }

    let group = match partition.kind_v1() {
        crate::remote_manifest::DerivationPartitionKindV1::TemporalChannelGroup(group) => group,
        crate::remote_manifest::DerivationPartitionKindV1::OpeningStatic => {
            return Err(RemoteWindowDemandErrorV1::InvalidPartition);
        }
    };
    if !selected_groups.contains(&group) {
        return Err(RemoteWindowDemandErrorV1::InvalidPartition);
    }
    let ordinal = usize::try_from(partition.source_unit_ordinal_v1())
        .map_err(|_error| RemoteWindowDemandErrorV1::ArithmeticOverflow)?;
    let expected = partition_for(ordinal, group)?;
    if expected != partition {
        return Err(RemoteWindowDemandErrorV1::InvalidPartition);
    }
    Ok(())
}

fn collect_backfill_ordinals_v1(
    backfill_additions: &[DerivationPartitionKeyV1],
    session_id: RemoteMcapSessionIdV1,
    source_generation: u64,
    selected_groups: &[StableDecoderGroupIdV1],
    partition_for: &impl Fn(
        usize,
        StableDecoderGroupIdV1,
    ) -> Result<DerivationPartitionKeyV1, RemoteWindowDemandErrorV1>,
) -> Result<Vec<usize>, RemoteWindowDemandErrorV1> {
    let mut ordinals = Vec::new();
    for &partition in backfill_additions {
        validate_backfill_partition_v1(
            partition,
            session_id,
            source_generation,
            selected_groups,
            partition_for,
        )?;
        let ordinal = usize::try_from(partition.source_unit_ordinal_v1())
            .map_err(|_error| RemoteWindowDemandErrorV1::ArithmeticOverflow)?;
        ordinals
            .try_reserve(1)
            .map_err(|_error| RemoteWindowDemandErrorV1::ResourceLimitExceeded)?;
        ordinals.push(ordinal);
    }
    ordinals.sort_unstable();
    ordinals.dedup();
    Ok(ordinals)
}

fn push_unique_partition_v1(
    output: &mut Vec<DerivationPartitionKeyV1>,
    seen: &mut HashSet<DerivationPartitionKeyV1>,
    partition: DerivationPartitionKeyV1,
    max_partitions: u64,
) -> Result<(), RemoteWindowDemandErrorV1> {
    if seen.contains(&partition) {
        return Ok(());
    }
    let output_len = u64::try_from(output.len())
        .map_err(|_error| RemoteWindowDemandErrorV1::ArithmeticOverflow)?;
    if output_len >= max_partitions {
        return Err(RemoteWindowDemandErrorV1::ResourceLimitExceeded);
    }
    output
        .try_reserve(1)
        .map_err(|_error| RemoteWindowDemandErrorV1::ResourceLimitExceeded)?;
    seen.try_reserve(1)
        .map_err(|_error| RemoteWindowDemandErrorV1::ResourceLimitExceeded)?;
    seen.insert(partition);
    output.push(partition);
    Ok(())
}

fn expand_partition_set_v1(
    ordinals: &[usize],
    selected_groups: &[StableDecoderGroupIdV1],
    include_opening_static: bool,
    opening_static: DerivationPartitionKeyV1,
    backfill_additions: &[DerivationPartitionKeyV1],
    session_id: RemoteMcapSessionIdV1,
    source_generation: u64,
    partition_for: impl Fn(
        usize,
        StableDecoderGroupIdV1,
    ) -> Result<DerivationPartitionKeyV1, RemoteWindowDemandErrorV1>,
    max_partitions: u64,
) -> Result<Vec<DerivationPartitionKeyV1>, RemoteWindowDemandErrorV1> {
    let mut output = Vec::new();
    let mut seen = HashSet::default();

    for &partition in backfill_additions {
        validate_backfill_partition_v1(
            partition,
            session_id,
            source_generation,
            selected_groups,
            &partition_for,
        )?;
    }

    if include_opening_static {
        push_unique_partition_v1(&mut output, &mut seen, opening_static, max_partitions)?;
    }
    for &ordinal in ordinals {
        for &group in selected_groups {
            let partition = partition_for(ordinal, group)?;
            push_unique_partition_v1(&mut output, &mut seen, partition, max_partitions)?;
        }
    }
    for &partition in backfill_additions {
        push_unique_partition_v1(&mut output, &mut seen, partition, max_partitions)?;
    }
    Ok(output)
}

pub(crate) fn phase_identity_v1(
    session_id: RemoteMcapSessionIdV1,
    source_generation: u64,
    class: RemoteWindowDemandClassV1,
    cursor: TimeInt,
    minimum_buffer: TimeInt,
    desired_prefetch: TimeInt,
    playing: bool,
) -> RemoteWindowDemandPhaseIdentityV1 {
    RemoteWindowDemandPhaseIdentityV1 {
        session_id,
        source_generation,
        class,
        cursor,
        minimum_buffer,
        desired_prefetch,
        playing,
    }
}

fn partition_sets_from_ordinal_plan_v1(
    ordinal_plan: &RemoteWindowOrdinalPlanV1,
    session_id: RemoteMcapSessionIdV1,
    source_generation: u64,
    cursor: TimeInt,
    minimum_buffer: TimeInt,
    desired_prefetch: TimeInt,
    selected_groups: &[StableDecoderGroupIdV1],
    opening_static: DerivationPartitionKeyV1,
    backfill_additions: &[DerivationPartitionKeyV1],
    partition_for: impl Fn(
        usize,
        StableDecoderGroupIdV1,
    ) -> Result<DerivationPartitionKeyV1, RemoteWindowDemandErrorV1>,
    is_satisfied: impl Fn(DerivationPartitionKeyV1) -> Result<bool, RemoteWindowDemandErrorV1>,
    limits: RemoteWindowDemandLimitsV1,
    playing: bool,
) -> Result<RemoteWindowDemandPlanV1, RemoteWindowDemandErrorV1> {
    let selected_group_count = u64::try_from(selected_groups.len())
        .map_err(|_error| RemoteWindowDemandErrorV1::ArithmeticOverflow)?;
    if selected_group_count > limits.max_selected_channel_groups {
        return Err(RemoteWindowDemandErrorV1::ResourceLimitExceeded);
    }
    let required_union_count = u64::try_from(ordinal_plan.required_union_ordinals_v1().len())
        .map_err(|_error| RemoteWindowDemandErrorV1::ArithmeticOverflow)?;
    let cross_product_ops = required_union_count
        .checked_mul(selected_group_count)
        .ok_or(RemoteWindowDemandErrorV1::ArithmeticOverflow)?;
    if cross_product_ops > limits.max_planning_cross_product_ops {
        return Err(RemoteWindowDemandErrorV1::ResourceLimitExceeded);
    }
    let backfill_count = u64::try_from(backfill_additions.len())
        .map_err(|_error| RemoteWindowDemandErrorV1::ArithmeticOverflow)?;
    let max_generation_partitions = cross_product_ops
        .checked_add(1)
        .and_then(|value| value.checked_add(backfill_count))
        .ok_or(RemoteWindowDemandErrorV1::ArithmeticOverflow)?;
    if max_generation_partitions > limits.max_generation_partitions {
        return Err(RemoteWindowDemandErrorV1::ResourceLimitExceeded);
    }

    let presentation_required = expand_partition_set_v1(
        ordinal_plan.cursor_ordinals_v1(),
        selected_groups,
        true,
        opening_static,
        backfill_additions,
        session_id,
        source_generation,
        &partition_for,
        max_generation_partitions,
    )?;
    let playback_resume_required = expand_partition_set_v1(
        ordinal_plan.minimum_buffer_ordinals_v1(),
        selected_groups,
        false,
        opening_static,
        &[],
        session_id,
        source_generation,
        &partition_for,
        max_generation_partitions,
    )?;

    let presentation_set = presentation_required
        .iter()
        .copied()
        .collect::<HashSet<_>>();
    let mut satisfied_set = HashSet::default();
    let mut satisfied_partitions = Vec::new();
    let mut missing_presentation = Vec::new();
    let mut missing_resume = Vec::new();

    for &partition in &presentation_required {
        if is_satisfied(partition)? {
            if satisfied_set.insert(partition) {
                satisfied_partitions
                    .try_reserve(1)
                    .map_err(|_error| RemoteWindowDemandErrorV1::ResourceLimitExceeded)?;
                satisfied_partitions.push(partition);
            }
        } else {
            missing_presentation
                .try_reserve(1)
                .map_err(|_error| RemoteWindowDemandErrorV1::ResourceLimitExceeded)?;
            missing_presentation.push(partition);
        }
    }
    for &partition in &playback_resume_required {
        if is_satisfied(partition)? {
            if satisfied_set.insert(partition) {
                satisfied_partitions
                    .try_reserve(1)
                    .map_err(|_error| RemoteWindowDemandErrorV1::ResourceLimitExceeded)?;
                satisfied_partitions.push(partition);
            }
        } else {
            missing_resume
                .try_reserve(1)
                .map_err(|_error| RemoteWindowDemandErrorV1::ResourceLimitExceeded)?;
            missing_resume.push(partition);
        }
    }

    let mut priority_2 = Vec::new();
    for &partition in &missing_resume {
        if !presentation_set.contains(&partition) {
            priority_2
                .try_reserve(1)
                .map_err(|_error| RemoteWindowDemandErrorV1::ResourceLimitExceeded)?;
            priority_2.push(partition);
        }
    }

    let mut mutation_commit_required = Vec::new();
    let mut mutation_seen = HashSet::default();
    for &partition in &missing_presentation {
        if mutation_seen.insert(partition) {
            mutation_commit_required
                .try_reserve(1)
                .map_err(|_error| RemoteWindowDemandErrorV1::ResourceLimitExceeded)?;
            mutation_commit_required.push(partition);
        }
    }
    if playing {
        for &partition in &missing_resume {
            if mutation_seen.insert(partition) {
                mutation_commit_required
                    .try_reserve(1)
                    .map_err(|_error| RemoteWindowDemandErrorV1::ResourceLimitExceeded)?;
                mutation_commit_required.push(partition);
            }
        }
    }

    let phase_demands = vec![
        RemoteWindowDemandV1::new_uninstalled(
            RemoteWindowDemandClassV1::PresentationRequired,
            phase_identity_v1(
                session_id,
                source_generation,
                RemoteWindowDemandClassV1::PresentationRequired,
                cursor,
                minimum_buffer,
                desired_prefetch,
                playing,
            ),
        ),
        RemoteWindowDemandV1::new_uninstalled(
            RemoteWindowDemandClassV1::PlaybackResumeRequired,
            phase_identity_v1(
                session_id,
                source_generation,
                RemoteWindowDemandClassV1::PlaybackResumeRequired,
                cursor,
                minimum_buffer,
                desired_prefetch,
                playing,
            ),
        ),
        RemoteWindowDemandV1::new_uninstalled(
            RemoteWindowDemandClassV1::PrefetchDesired,
            phase_identity_v1(
                session_id,
                source_generation,
                RemoteWindowDemandClassV1::PrefetchDesired,
                cursor,
                minimum_buffer,
                desired_prefetch,
                playing,
            ),
        ),
    ];

    Ok(RemoteWindowDemandPlanV1 {
        ordinal_plan: ordinal_plan.clone(),
        phase_demands,
        presentation_required,
        playback_resume_required,
        satisfied_partitions,
        missing_presentation,
        missing_resume,
        priority_2,
        mutation_commit_required,
    })
}

/// Expands the physical ordinal plan into immutable partition demand and subtracts residency.
pub(crate) fn plan_window_demands_v1(
    manifest: &ImmutableRemoteMcapManifestV1<'_, '_, '_, '_, '_>,
    root_index: &RefetchableRootIndexV1,
    store: &ChunkStore,
    capability: &WebRemoteMcapRootCapabilityV1,
    cursor: TimeInt,
    minimum_buffer: TimeInt,
    desired_prefetch: TimeInt,
    playing: bool,
    backfill_additions: &[DerivationPartitionKeyV1],
    limits: RemoteWindowDemandLimitsV1,
) -> Result<RemoteWindowDemandPlanV1, RemoteWindowDemandErrorV1> {
    validate_temporal_cursor_v1(manifest.indexed_extent_v1(), cursor)?;

    let selected_groups = collect_bounded_group_ids_v1(
        manifest.groups_v1().iter().map(|group| group.group_id()),
        limits.max_selected_channel_groups,
    )?;
    let opening_static = manifest
        .opening_static_partition_v1()
        .map_err(|_error| RemoteWindowDemandErrorV1::InvalidPartition)?
        .partition_v1()
        .key_v1();
    let partition_for = |ordinal, group| {
        let ordinal = u32::try_from(ordinal)
            .map_err(|_error| RemoteWindowDemandErrorV1::ArithmeticOverflow)?;
        manifest
            .temporal_partition_v1(ordinal, group)
            .map(|authority| authority.partition_v1().key_v1())
            .map_err(|_error| RemoteWindowDemandErrorV1::InvalidPartition)
    };
    let backfill_ordinals = collect_backfill_ordinals_v1(
        backfill_additions,
        manifest.session_id_v1(),
        manifest.source_generation_v1(),
        &selected_groups,
        &partition_for,
    )?;
    let ordinal_plan = plan_window_ordinals_v1(
        manifest.source_layout_v1(),
        cursor,
        minimum_buffer,
        desired_prefetch,
        limits,
    )?;
    let ordinal_plan = ordinal_plan_with_desired_prefetch_excluding_v1(
        ordinal_plan,
        &backfill_ordinals,
        limits.max_window_chunk_hits,
    )?;

    partition_sets_from_ordinal_plan_v1(
        &ordinal_plan,
        manifest.session_id_v1(),
        manifest.source_generation_v1(),
        cursor,
        minimum_buffer,
        desired_prefetch,
        &selected_groups,
        opening_static,
        backfill_additions,
        partition_for,
        |partition| {
            root_index
                .is_partition_fully_resident_v1(store, capability, partition)
                .map_err(|_error| RemoteWindowDemandErrorV1::InvalidPartition)
        },
        limits,
        playing,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_manifest::{DerivationPartitionKindV1, ManifestPartitionDescriptorV1};
    use crate::testing::{AdversarialMcapFixtureBuilder, FixtureChunk, FixtureMessage};

    fn time(value: i64) -> TimeInt {
        TimeInt::new_temporal(value)
    }

    fn session_id() -> RemoteMcapSessionIdV1 {
        RemoteMcapSessionIdV1::for_registration_test_v1(1)
    }

    fn temporal_partition(ordinal: u32) -> DerivationPartitionKeyV1 {
        ManifestPartitionDescriptorV1::for_registration_test_v1(
            session_id(),
            ordinal,
            DerivationPartitionKindV1::TemporalChannelGroup(
                StableDecoderGroupIdV1::first_for_assignment_test_v1(),
            ),
            1,
        )
        .0
        .key_v1()
    }

    fn opening_static_partition() -> DerivationPartitionKeyV1 {
        ManifestPartitionDescriptorV1::for_registration_test_v1(
            session_id(),
            u32::MAX,
            DerivationPartitionKindV1::OpeningStatic,
            1,
        )
        .0
        .key_v1()
    }

    fn layout(intervals: &[(i64, i64)]) -> ResolvedCanonicalPhysicalLayoutV1 {
        ResolvedCanonicalPhysicalLayoutV1::for_window_planner_test_v1(intervals)
    }

    #[test]
    fn overlapping_chunks_are_not_missed_and_layers_are_disjoint() {
        let layout = layout(&[(0, 20), (10, 30), (20, 40), (45, 50)]);
        let plan = plan_window_ordinals_v1(
            &layout,
            time(25),
            time(6),
            time(21),
            RemoteWindowDemandLimitsV1::generous_disarmed_v1(),
        )
        .unwrap();

        assert_eq!(plan.cursor_ordinals_v1(), &[1, 2]);
        assert_eq!(plan.minimum_buffer_ordinals_v1(), &[1, 2]);
        assert_eq!(plan.required_union_ordinals_v1(), &[1, 2]);
        assert_eq!(plan.desired_prefetch_ordinals_v1(), &[3]);
    }

    #[test]
    fn bounded_interval_query_fails_before_adding_next_hit() {
        let layout = layout(&[(0, 1), (1, 2), (2, 3)]);
        let limits = RemoteWindowDemandLimitsV1 {
            max_window_chunk_hits: 2,
            ..RemoteWindowDemandLimitsV1::generous_disarmed_v1()
        };
        let error =
            plan_window_ordinals_v1(&layout, time(0), time(1), time(3), limits).unwrap_err();
        assert_eq!(error, RemoteWindowDemandErrorV1::ResourceLimitExceeded);
    }

    #[test]
    fn phase_owner_installation_is_matching_and_prefetch_stays_unstarted() {
        let identity = phase_identity_v1(
            session_id(),
            7,
            RemoteWindowDemandClassV1::PrefetchDesired,
            time(1),
            time(2),
            time(3),
            true,
        );
        let mut demand = RemoteWindowDemandV1::new_uninstalled(
            RemoteWindowDemandClassV1::PrefetchDesired,
            identity,
        );
        assert!(!demand.can_start_retry_v1());

        let other = phase_identity_v1(
            session_id(),
            8,
            RemoteWindowDemandClassV1::PrefetchDesired,
            time(1),
            time(2),
            time(3),
            true,
        );
        assert_eq!(
            demand.install_matching_phase_owner_v1(
                other,
                RemoteWindowDemandPolicyInputsV1 {
                    cumulative_range_requests: 2,
                    active_visible_deadline_millis: 5,
                },
            ),
            Err(RemoteWindowDemandErrorV1::PhaseOwnerMismatch)
        );
        assert!(!demand.can_start_retry_v1());

        assert_eq!(
            demand.install_matching_phase_owner_v1(
                identity,
                RemoteWindowDemandPolicyInputsV1 {
                    cumulative_range_requests: 3,
                    active_visible_deadline_millis: 7,
                },
            ),
            Err(RemoteWindowDemandErrorV1::PrefetchUnstarted)
        );
        assert!(!demand.can_start_retry_v1());
        assert_eq!(demand.phase_v1().policy_inputs_v1(), None);
    }

    #[test]
    fn presentation_demand_becomes_retry_startable_after_matching_install() {
        let identity = phase_identity_v1(
            session_id(),
            7,
            RemoteWindowDemandClassV1::PresentationRequired,
            time(1),
            time(2),
            time(3),
            true,
        );
        let mut demand = RemoteWindowDemandV1::new_uninstalled(
            RemoteWindowDemandClassV1::PresentationRequired,
            identity,
        );
        assert!(!demand.can_start_retry_v1());

        let policy_inputs = RemoteWindowDemandPolicyInputsV1 {
            cumulative_range_requests: 3,
            active_visible_deadline_millis: 7,
        };
        demand
            .install_matching_phase_owner_v1(identity, policy_inputs)
            .unwrap();
        assert!(demand.can_start_retry_v1());
        assert_eq!(demand.phase_v1().policy_inputs_v1(), Some(policy_inputs));
    }

    #[test]
    fn desired_prefetch_excludes_backfill_partition_ordinals() {
        let base = plan_window_ordinals_v1(
            &layout(&[(0, 0), (5, 5), (10, 10)]),
            time(0),
            time(3),
            time(20),
            RemoteWindowDemandLimitsV1::generous_disarmed_v1(),
        )
        .unwrap();
        let plan = ordinal_plan_with_desired_prefetch_excluding_v1(
            base,
            &[1],
            RemoteWindowDemandLimitsV1::generous_disarmed_v1().max_window_chunk_hits,
        )
        .unwrap();
        assert_eq!(plan.cursor_ordinals_v1(), &[0]);
        assert_eq!(plan.minimum_buffer_ordinals_v1(), &[0]);
        assert_eq!(plan.required_union_ordinals_v1(), &[0]);
        assert_eq!(plan.desired_prefetch_ordinals_v1(), &[2]);
    }

    #[test]
    fn temporal_cursor_must_fall_inside_indexed_extent() {
        use re_log_types::AbsoluteTimeRange;

        assert_eq!(
            validate_temporal_cursor_v1(
                CanonicalIndexedExtentV1::Known(AbsoluteTimeRange::new(10, 20)),
                time(9),
            ),
            Err(RemoteWindowDemandErrorV1::InvalidWindow)
        );
        assert_eq!(
            validate_temporal_cursor_v1(
                CanonicalIndexedExtentV1::Known(AbsoluteTimeRange::new(10, 20)),
                time(21),
            ),
            Err(RemoteWindowDemandErrorV1::InvalidWindow)
        );
        assert_eq!(
            validate_temporal_cursor_v1(
                CanonicalIndexedExtentV1::Known(AbsoluteTimeRange::new(10, 20)),
                time(20),
            ),
            Ok(())
        );
        assert_eq!(
            validate_temporal_cursor_v1(CanonicalIndexedExtentV1::NoIndexedMessages, time(0)),
            Err(RemoteWindowDemandErrorV1::InvalidWindow)
        );
    }

    #[test]
    fn backfill_partition_identity_is_validated() {
        let selected_groups = [StableDecoderGroupIdV1::first_for_assignment_test_v1()];
        let partition_for = |ordinal, group| {
            assert_eq!(
                group,
                StableDecoderGroupIdV1::first_for_assignment_test_v1()
            );
            Ok(temporal_partition(u32::try_from(ordinal).unwrap()))
        };

        let foreign_session = DerivationPartitionKeyV1::for_window_demand_test_v1(
            RemoteMcapSessionIdV1::for_registration_test_v1(2),
            1,
            9,
            DerivationPartitionKindV1::TemporalChannelGroup(
                StableDecoderGroupIdV1::first_for_assignment_test_v1(),
            ),
        );
        assert_eq!(
            validate_backfill_partition_v1(
                foreign_session,
                session_id(),
                1,
                &selected_groups,
                &partition_for,
            ),
            Err(RemoteWindowDemandErrorV1::InvalidPartition)
        );

        let foreign_generation = DerivationPartitionKeyV1::for_window_demand_test_v1(
            session_id(),
            2,
            9,
            DerivationPartitionKindV1::TemporalChannelGroup(
                StableDecoderGroupIdV1::first_for_assignment_test_v1(),
            ),
        );
        assert_eq!(
            validate_backfill_partition_v1(
                foreign_generation,
                session_id(),
                1,
                &selected_groups,
                &partition_for,
            ),
            Err(RemoteWindowDemandErrorV1::InvalidPartition)
        );

        let foreign_kind = DerivationPartitionKeyV1::for_window_demand_test_v1(
            session_id(),
            1,
            u32::MAX,
            DerivationPartitionKindV1::OpeningStatic,
        );
        assert_eq!(
            validate_backfill_partition_v1(
                foreign_kind,
                session_id(),
                1,
                &selected_groups,
                &partition_for,
            ),
            Err(RemoteWindowDemandErrorV1::InvalidPartition)
        );

        assert_eq!(
            validate_backfill_partition_v1(
                temporal_partition(9),
                session_id(),
                1,
                &selected_groups,
                &partition_for,
            ),
            Ok(())
        );
    }

    #[test]
    fn checked_cross_product_fails_before_partition_expansion() {
        let ordinal_plan = RemoteWindowOrdinalPlanV1 {
            cursor_ordinals: vec![0, 1, 2].into_boxed_slice(),
            minimum_buffer_ordinals: vec![0, 1, 2].into_boxed_slice(),
            desired_prefetch_ordinals: vec![].into_boxed_slice(),
            required_union_ordinals: vec![0, 1, 2].into_boxed_slice(),
        };
        let selected_groups = [
            StableDecoderGroupIdV1::first_for_assignment_test_v1(),
            StableDecoderGroupIdV1::first_for_assignment_test_v1(),
        ];
        let limits = RemoteWindowDemandLimitsV1 {
            max_planning_cross_product_ops: 5,
            ..RemoteWindowDemandLimitsV1::generous_disarmed_v1()
        };
        let error = partition_sets_from_ordinal_plan_v1(
            &ordinal_plan,
            session_id(),
            1,
            time(0),
            time(1),
            time(2),
            &selected_groups,
            opening_static_partition(),
            &[],
            |ordinal, _group| Ok(temporal_partition(u32::try_from(ordinal).unwrap())),
            |_partition| Ok(false),
            limits,
            true,
        )
        .unwrap_err();
        assert_eq!(error, RemoteWindowDemandErrorV1::ResourceLimitExceeded);
    }

    #[test]
    fn long_chunk_and_half_open_boundary_are_differential_against_layout() {
        let base = plan_window_ordinals_v1(
            &layout(&[(0, 100), (101, 101)]),
            time(90),
            time(5),
            time(12),
            RemoteWindowDemandLimitsV1::generous_disarmed_v1(),
        )
        .unwrap();
        assert_eq!(base.cursor_ordinals_v1(), &[0]);
        assert_eq!(base.minimum_buffer_ordinals_v1(), &[0]);
        assert_eq!(base.required_union_ordinals_v1(), &[0]);
        assert_eq!(base.desired_prefetch_ordinals_v1(), &[1]);

        let boundary = plan_window_ordinals_v1(
            &layout(&[(0, 0), (9, 9), (10, 10)]),
            time(0),
            time(1),
            time(10),
            RemoteWindowDemandLimitsV1::generous_disarmed_v1(),
        )
        .unwrap();
        assert_eq!(boundary.cursor_ordinals_v1(), &[0]);
        assert_eq!(boundary.minimum_buffer_ordinals_v1(), &[0]);
        assert_eq!(boundary.desired_prefetch_ordinals_v1(), &[1]);
    }

    #[test]
    fn file_order_is_ignored_when_ordinals_are_time_ordered() {
        let plan = plan_window_ordinals_v1(
            &layout(&[(30, 40), (0, 10), (20, 20)]),
            time(20),
            time(1),
            time(10),
            RemoteWindowDemandLimitsV1::generous_disarmed_v1(),
        )
        .unwrap();
        assert_eq!(plan.cursor_ordinals_v1(), &[2]);
        assert_eq!(plan.minimum_buffer_ordinals_v1(), &[2]);
        assert_eq!(plan.desired_prefetch_ordinals_v1(), &[] as &[usize]);
    }

    #[test]
    fn physical_ordinal_selection_differentially_covers_all_layers() {
        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_chunks([
                FixtureChunk::single(FixtureMessage::new(1, 0, 20)),
                FixtureChunk::new([FixtureMessage::new(1, 1, 10), FixtureMessage::new(1, 2, 20)]),
                FixtureChunk::single(FixtureMessage::new(1, 3, 20)),
            ])
            .build()
            .unwrap();

        let intervals = fixture
            .layout
            .chunk_indexes
            .iter()
            .map(|index| {
                (
                    i64::try_from(index.message_range.start).unwrap(),
                    i64::try_from(index.message_range.end).unwrap(),
                )
            })
            .collect::<Vec<_>>();
        let layout = layout(&intervals);
        let cursor = 15;
        let minimum_buffer = 6;
        let desired_prefetch = 10;
        let plan = plan_window_ordinals_v1(
            &layout,
            time(cursor),
            time(minimum_buffer),
            time(desired_prefetch),
            RemoteWindowDemandLimitsV1::generous_disarmed_v1(),
        )
        .unwrap();

        let upstream = fixture
            .read_upstream_indexed(Some(cursor as u64), Some((cursor + minimum_buffer) as u64))
            .unwrap();
        let upstream_ordinals = upstream
            .chunk_payload_offsets
            .iter()
            .map(|offset| {
                fixture
                    .layout
                    .chunks
                    .iter()
                    .position(|chunk| chunk.compressed_data_start as u64 == *offset)
                    .unwrap()
            })
            .collect::<HashSet<_>>();
        let planned_ordinals = plan
            .minimum_buffer_ordinals_v1()
            .iter()
            .copied()
            .collect::<HashSet<_>>();
        assert_eq!(planned_ordinals, upstream_ordinals);

        let cursor_upstream = fixture
            .read_upstream_indexed(Some(cursor as u64), Some((cursor + 1) as u64))
            .unwrap()
            .chunk_payload_offsets
            .iter()
            .map(|offset| {
                fixture
                    .layout
                    .chunks
                    .iter()
                    .position(|chunk| chunk.compressed_data_start as u64 == *offset)
                    .unwrap()
            })
            .collect::<HashSet<_>>();
        assert_eq!(
            plan.cursor_ordinals_v1()
                .iter()
                .copied()
                .collect::<HashSet<_>>(),
            cursor_upstream
        );

        let desired_upstream = fixture
            .read_upstream_indexed(
                Some(cursor as u64),
                Some((cursor + desired_prefetch) as u64),
            )
            .unwrap()
            .chunk_payload_offsets
            .iter()
            .map(|offset| {
                fixture
                    .layout
                    .chunks
                    .iter()
                    .position(|chunk| chunk.compressed_data_start as u64 == *offset)
                    .unwrap()
            })
            .collect::<HashSet<_>>();
        let required_union = plan
            .required_union_ordinals_v1()
            .iter()
            .copied()
            .collect::<HashSet<_>>();
        let expected_prefetch = desired_upstream
            .difference(&required_union)
            .copied()
            .collect::<HashSet<_>>();
        assert_eq!(
            plan.desired_prefetch_ordinals_v1()
                .iter()
                .copied()
                .collect::<HashSet<_>>(),
            expected_prefetch
        );

        assert_eq!(plan.cursor_ordinals_v1(), &[1]);
        assert_eq!(plan.minimum_buffer_ordinals_v1(), &[0, 1, 2]);
        assert_eq!(plan.required_union_ordinals_v1(), &[0, 1, 2]);
        assert_eq!(plan.desired_prefetch_ordinals_v1(), &[] as &[usize]);
    }

    #[test]
    fn satisfied_partitions_are_removed_and_priority_two_excludes_presentation() {
        let layout = layout(&[(0, 1), (2, 3)]);
        let ordinal_plan = plan_window_ordinals_v1(
            &layout,
            time(0),
            time(3),
            time(4),
            RemoteWindowDemandLimitsV1::generous_disarmed_v1(),
        )
        .unwrap();
        let selected_groups = [StableDecoderGroupIdV1::first_for_assignment_test_v1()];
        let opening = opening_static_partition();
        let backfill = temporal_partition(9);

        let plan = partition_sets_from_ordinal_plan_v1(
            &ordinal_plan,
            session_id(),
            1,
            time(0),
            time(3),
            time(4),
            &selected_groups,
            opening,
            &[backfill],
            |ordinal, _group| Ok(temporal_partition(u32::try_from(ordinal).unwrap())),
            |partition| {
                Ok(partition == opening
                    || partition == temporal_partition(0)
                    || partition == temporal_partition(1))
            },
            RemoteWindowDemandLimitsV1::generous_disarmed_v1(),
            true,
        )
        .unwrap();

        assert_eq!(plan.presentation_required_v1().len(), 3);
        assert_eq!(plan.playback_resume_required_v1().len(), 2);
        assert_eq!(
            plan.satisfied_partitions_v1(),
            &[opening, temporal_partition(0), temporal_partition(1)]
        );
        assert_eq!(plan.missing_presentation_v1(), &[backfill]);
        assert!(plan.missing_resume_v1().is_empty());
        assert!(plan.priority_2_v1().is_empty());
        assert_eq!(plan.mutation_commit_required_v1(), &[backfill]);
    }

    #[test]
    fn physical_ordinal_selection_matches_upstream_indexed_reader() {
        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_chunks([
                FixtureChunk::single(FixtureMessage::new(1, 0, 20)),
                FixtureChunk::new([FixtureMessage::new(1, 1, 10), FixtureMessage::new(1, 2, 20)]),
                FixtureChunk::single(FixtureMessage::new(1, 3, 20)),
            ])
            .build()
            .unwrap();

        let intervals = fixture
            .layout
            .chunk_indexes
            .iter()
            .map(|index| {
                (
                    i64::try_from(index.message_range.start).unwrap(),
                    i64::try_from(index.message_range.end).unwrap(),
                )
            })
            .collect::<Vec<_>>();
        let layout = layout(&intervals);
        let cursor = 15;
        let minimum_buffer = 6;
        let desired_prefetch = 10;
        let plan = plan_window_ordinals_v1(
            &layout,
            time(cursor),
            time(minimum_buffer),
            time(desired_prefetch),
            RemoteWindowDemandLimitsV1::generous_disarmed_v1(),
        )
        .unwrap();

        let upstream = fixture
            .read_upstream_indexed(Some(cursor as u64), Some((cursor + minimum_buffer) as u64))
            .unwrap();
        let upstream_ordinals = upstream
            .chunk_payload_offsets
            .iter()
            .map(|offset| {
                fixture
                    .layout
                    .chunks
                    .iter()
                    .position(|chunk| chunk.compressed_data_start as u64 == *offset)
                    .unwrap()
            })
            .collect::<HashSet<_>>();
        let planned_ordinals = plan
            .minimum_buffer_ordinals_v1()
            .iter()
            .copied()
            .collect::<HashSet<_>>();
        assert_eq!(planned_ordinals, upstream_ordinals);
    }
}
