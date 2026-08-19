//! Supersedable/CommitLocked ownership for Web remote-MCAP seek commits.
//!
//! This module is the pure production-disarmed state layer for the MCAP-094 transition. It owns no
//! Viewer command routing, Fetch/decode transport, `EntityDb` handle, Store mutation arbiter, or
//! public facade. It consumes only the MCAP-093 requested/committed navigation types and leaves
//! wiring to the future frame driver.

#![allow(dead_code)]

use std::collections::BTreeSet;

use re_log_types::{Duration, TimeInt};

use crate::remote_navigation::{
    CommittedPresentationTimeV1, PendingNavigationIntentV1, RemoteCandidateClockInputV1,
    RemoteLoopModeV1, RemoteNavigationAdapterV1, RemoteNavigationCommandV1,
    RemoteNavigationDemandKeyV1, RemoteNavigationErrorV1, RemoteNavigationTriggerV1,
    RemoteNavigationUiStateV1, RemotePlayStateV1,
};

/// Opaque generation identity for one remote navigation demand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct RemoteSeekGenerationV1(u64);

impl RemoteSeekGenerationV1 {
    pub(crate) const fn initial_v1() -> Self {
        Self(0)
    }

    pub(crate) const fn from_u64_v1(value: u64) -> Self {
        Self(value)
    }

    pub(crate) const fn as_u64_v1(self) -> u64 {
        self.0
    }

    pub(crate) const fn next_v1(self) -> Result<Self, RemoteSeekErrorV1> {
        match self.0.checked_add(1) {
            Some(value) => Ok(Self(value)),
            None => Err(RemoteSeekErrorV1::ArithmeticOverflow),
        }
    }
}

/// Opaque batch identity used by the state-machine tests and future adapter binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct RemoteSeekBatchIdV1(u64);

impl RemoteSeekBatchIdV1 {
    pub(crate) const fn from_u64_v1(value: u64) -> Self {
        Self(value)
    }

    pub(crate) const fn as_u64_v1(self) -> u64 {
        self.0
    }
}

/// Immutable commit-set identity frozen before the first physical Store insertion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteSeekCommitSetV1 {
    generation: RemoteSeekGenerationV1,
    batch_ids: BTreeSet<RemoteSeekBatchIdV1>,
}

impl RemoteSeekCommitSetV1 {
    pub(crate) fn new_v1(
        generation: RemoteSeekGenerationV1,
        batch_ids: BTreeSet<RemoteSeekBatchIdV1>,
    ) -> Result<Self, RemoteSeekErrorV1> {
        if batch_ids.is_empty() {
            return Err(RemoteSeekErrorV1::EmptyCommitSet);
        }
        Ok(Self {
            generation,
            batch_ids,
        })
    }

    pub(crate) const fn generation_v1(&self) -> RemoteSeekGenerationV1 {
        self.generation
    }

    pub(crate) fn batch_ids_v1(&self) -> &BTreeSet<RemoteSeekBatchIdV1> {
        &self.batch_ids
    }
}

/// Frozen `(generation, target, commit_set)` passed into `CommitLocked`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteSeekCommitLockV1 {
    generation: RemoteSeekGenerationV1,
    target: TimeInt,
    commit_set: RemoteSeekCommitSetV1,
}

impl RemoteSeekCommitLockV1 {
    pub(crate) fn new_v1(
        generation: RemoteSeekGenerationV1,
        target: TimeInt,
        commit_set: RemoteSeekCommitSetV1,
    ) -> Result<Self, RemoteSeekErrorV1> {
        if target.is_static() {
            return Err(RemoteSeekErrorV1::InvalidTarget);
        }
        if commit_set.generation_v1() != generation {
            return Err(RemoteSeekErrorV1::CommitSetGenerationMismatch);
        }
        Ok(Self {
            generation,
            target,
            commit_set,
        })
    }

    pub(crate) const fn generation_v1(&self) -> RemoteSeekGenerationV1 {
        self.generation
    }

    pub(crate) const fn target_v1(&self) -> TimeInt {
        self.target
    }

    pub(crate) const fn commit_set_v1(&self) -> &RemoteSeekCommitSetV1 {
        &self.commit_set
    }
}

/// Pre-mutation failures that can still be rolled back to the committed cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteSeekPreMutationFailureV1 {
    RetryExhausted,
    TemporalDecode,
    RegistrationPreflight,
}

/// Post-first-insertion failures that irreversibly poison the remote Store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteSeekPostMutationFailureV1 {
    Insertion,
    Residency,
    Ack,
    PresentationCommit,
}

/// First-terminal-cause-wins latch reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteSeekTerminalCauseV1 {
    InitialPresentationFailed,
    PresentationCommitPoisoned(RemoteSeekPostMutationFailureV1),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteSeekTerminalFailureV1 {
    cause: RemoteSeekTerminalCauseV1,
}

impl RemoteSeekTerminalFailureV1 {
    pub(crate) const fn initial_presentation_failed_v1() -> Self {
        Self {
            cause: RemoteSeekTerminalCauseV1::InitialPresentationFailed,
        }
    }

    pub(crate) const fn presentation_commit_poisoned_v1(
        cause: RemoteSeekPostMutationFailureV1,
    ) -> Self {
        Self {
            cause: RemoteSeekTerminalCauseV1::PresentationCommitPoisoned(cause),
        }
    }

    pub(crate) const fn cause_v1(&self) -> RemoteSeekTerminalCauseV1 {
        self.cause
    }

    pub(crate) const fn is_initial_presentation_failed_v1(&self) -> bool {
        matches!(
            self.cause,
            RemoteSeekTerminalCauseV1::InitialPresentationFailed
        )
    }
}

/// How a work result may be retained while `Supersedable`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteSeekResultDispositionV1 {
    CurrentGeneration,
    SupersededCacheOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteSeekWorkResultV1 {
    generation: RemoteSeekGenerationV1,
    batch_id: RemoteSeekBatchIdV1,
}

impl RemoteSeekWorkResultV1 {
    pub(crate) const fn new_v1(
        generation: RemoteSeekGenerationV1,
        batch_id: RemoteSeekBatchIdV1,
    ) -> Self {
        Self {
            generation,
            batch_id,
        }
    }
}

/// Open/waiting state where a new demand can still supersede the current one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteSeekSupersedableStateV1 {
    generation: RemoteSeekGenerationV1,
    work_epoch: u64,
    initial_presentation: bool,
    active_demand_key: Option<RemoteNavigationDemandKeyV1>,
    pending_navigation_intent: Option<PendingNavigationIntentV1>,
    staging_batches: BTreeSet<RemoteSeekBatchIdV1>,
    pins: BTreeSet<RemoteSeekBatchIdV1>,
    reservation_bytes: u64,
    reusable_unloaded_registration_metadata: BTreeSet<RemoteSeekBatchIdV1>,
    cache_only_results: BTreeSet<RemoteSeekBatchIdV1>,
    facade_was_closed: bool,
}

impl RemoteSeekSupersedableStateV1 {
    fn initial_presentation_v1(
        generation: RemoteSeekGenerationV1,
        work_epoch: u64,
        active_demand_key: RemoteNavigationDemandKeyV1,
    ) -> Self {
        Self {
            generation,
            work_epoch,
            initial_presentation: true,
            active_demand_key: Some(active_demand_key),
            pending_navigation_intent: None,
            staging_batches: BTreeSet::new(),
            pins: BTreeSet::new(),
            reservation_bytes: 0,
            reusable_unloaded_registration_metadata: BTreeSet::new(),
            cache_only_results: BTreeSet::new(),
            facade_was_closed: false,
        }
    }

    fn committed_presentation_v1() -> Self {
        Self {
            generation: RemoteSeekGenerationV1::initial_v1(),
            work_epoch: 0,
            initial_presentation: false,
            active_demand_key: None,
            pending_navigation_intent: None,
            staging_batches: BTreeSet::new(),
            pins: BTreeSet::new(),
            reservation_bytes: 0,
            reusable_unloaded_registration_metadata: BTreeSet::new(),
            cache_only_results: BTreeSet::new(),
            facade_was_closed: false,
        }
    }
}

/// Frozen commit ownership after the facade is closed and before the first `add_chunk`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteSeekCommitLockedStateV1 {
    lock: RemoteSeekCommitLockV1,
    work_epoch: u64,
    initial_presentation: bool,
    committed_demand_key: RemoteNavigationDemandKeyV1,
    pending_navigation_intent: Option<PendingNavigationIntentV1>,
    physical_mutation_started: bool,
    reusable_unloaded_registration_metadata: BTreeSet<RemoteSeekBatchIdV1>,
    facade_was_closed_before_mutation: bool,
}

/// Recoverable pre-mutation seek failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteSeekCurrentSeekFailureV1 {
    failed_generation: RemoteSeekGenerationV1,
    work_epoch: u64,
    failure: RemoteSeekPreMutationFailureV1,
    reusable_unloaded_registration_metadata: BTreeSet<RemoteSeekBatchIdV1>,
    facade_reopened: bool,
    closure_revalidated: bool,
}

/// Coordinator ownership state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RemoteSeekCoordinatorStateV1 {
    Supersedable(RemoteSeekSupersedableStateV1),
    CommitLocked(RemoteSeekCommitLockedStateV1),
    CurrentSeekFailedBeforeMutation(RemoteSeekCurrentSeekFailureV1),
    InitialPresentationFailed(RemoteSeekTerminalFailureV1),
    PresentationCommitPoisoned(RemoteSeekTerminalFailureV1),
}

impl RemoteSeekCoordinatorStateV1 {
    pub(crate) const fn is_supersedable_v1(&self) -> bool {
        matches!(self, Self::Supersedable(_))
    }

    pub(crate) const fn is_commit_locked_v1(&self) -> bool {
        matches!(self, Self::CommitLocked(_))
    }

    pub(crate) const fn is_current_seek_failed_before_mutation_v1(&self) -> bool {
        matches!(self, Self::CurrentSeekFailedBeforeMutation(_))
    }

    pub(crate) const fn is_initial_presentation_failed_v1(&self) -> bool {
        matches!(self, Self::InitialPresentationFailed(_))
    }

    pub(crate) const fn is_presentation_commit_poisoned_v1(&self) -> bool {
        matches!(self, Self::PresentationCommitPoisoned(_))
    }

    pub(crate) const fn generation_v1(&self) -> Option<RemoteSeekGenerationV1> {
        match self {
            Self::Supersedable(state) => Some(state.generation),
            Self::CommitLocked(state) => Some(state.lock.generation),
            Self::CurrentSeekFailedBeforeMutation(state) => Some(state.failed_generation),
            Self::InitialPresentationFailed(_) | Self::PresentationCommitPoisoned(_) => None,
        }
    }

    pub(crate) const fn work_epoch_v1(&self) -> Option<u64> {
        match self {
            Self::Supersedable(state) => Some(state.work_epoch),
            Self::CommitLocked(state) => Some(state.work_epoch),
            Self::CurrentSeekFailedBeforeMutation(state) => Some(state.work_epoch),
            Self::InitialPresentationFailed(_) | Self::PresentationCommitPoisoned(_) => None,
        }
    }

    pub(crate) fn terminal_failure_v1(&self) -> Option<&RemoteSeekTerminalFailureV1> {
        match self {
            Self::InitialPresentationFailed(failure)
            | Self::PresentationCommitPoisoned(failure) => Some(failure),
            _ => None,
        }
    }

    pub(crate) fn current_seek_failure_v1(&self) -> Option<&RemoteSeekCurrentSeekFailureV1> {
        match self {
            Self::CurrentSeekFailedBeforeMutation(failure) => Some(failure),
            _ => None,
        }
    }

    pub(crate) fn commit_lock_v1(&self) -> Option<&RemoteSeekCommitLockV1> {
        match self {
            Self::CommitLocked(state) => Some(&state.lock),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum RemoteSeekErrorV1 {
    #[error("remote navigation state machine failed: {0}")]
    Navigation(RemoteNavigationErrorV1),

    #[error("remote seek arithmetic overflowed")]
    ArithmeticOverflow,

    #[error("remote seek received an invalid generation")]
    InvalidGeneration,

    #[error("remote seek commit set is empty")]
    EmptyCommitSet,

    #[error("remote seek commit set generation does not match its lock")]
    CommitSetGenerationMismatch,

    #[error("remote seek commit set does not match the staged batch set")]
    CommitSetMismatch,

    #[error("remote seek commit target does not match the active demand")]
    TargetMismatch,

    #[error("remote seek expected an initial presentation state")]
    NotInitialPresentation,

    #[error("remote seek expected a committed presentation state")]
    NotCommittedPresentation,

    #[error("remote seek expected Supersedable ownership")]
    NotSupersedable,

    #[error("remote seek expected CommitLocked ownership")]
    NotCommitLocked,

    #[error("remote seek physical mutation has not started")]
    MutationNotStarted,

    #[error("remote seek physical mutation has already started")]
    MutationAlreadyStarted,

    #[error("remote seek terminal latch is already frozen")]
    TerminalAlreadyFrozen,

    #[error("remote seek terminal state rejects navigation commands")]
    TerminalState,

    #[error("remote seek received a static target")]
    InvalidTarget,

    #[error("remote seek failure transition is invalid in the current state")]
    InvalidFailureTransition,
}

/// Pure coordinator for MCAP-094.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteSeekCoordinatorV1 {
    navigation: RemoteNavigationAdapterV1,
    state: RemoteSeekCoordinatorStateV1,
}

impl RemoteSeekCoordinatorV1 {
    pub(crate) fn new_initial_presentation_v1(
        navigation: RemoteNavigationAdapterV1,
        initial_intent: PendingNavigationIntentV1,
    ) -> Result<Self, RemoteSeekErrorV1> {
        if navigation.committed_time_v1().is_some() {
            return Err(RemoteSeekErrorV1::NotInitialPresentation);
        }

        let mut navigation = navigation;
        apply_intent_to_navigation_v1(&mut navigation, initial_intent)?;
        navigation.set_requested_generation_in_flight_v1(true);

        let generation = RemoteSeekGenerationV1::initial_v1().next_v1()?;
        let work_epoch = 1;
        Ok(Self {
            navigation,
            state: RemoteSeekCoordinatorStateV1::Supersedable(
                RemoteSeekSupersedableStateV1::initial_presentation_v1(
                    generation,
                    work_epoch,
                    initial_intent.demand_key,
                ),
            ),
        })
    }

    pub(crate) fn new_committed_v1(
        navigation: RemoteNavigationAdapterV1,
    ) -> Result<Self, RemoteSeekErrorV1> {
        if navigation.committed_time_v1().is_none() {
            return Err(RemoteSeekErrorV1::NotCommittedPresentation);
        }
        Ok(Self {
            navigation,
            state: RemoteSeekCoordinatorStateV1::Supersedable(
                RemoteSeekSupersedableStateV1::committed_presentation_v1(),
            ),
        })
    }

    pub(crate) fn state_v1(&self) -> &RemoteSeekCoordinatorStateV1 {
        &self.state
    }

    pub(crate) fn navigation_ui_state_v1(&self) -> &RemoteNavigationUiStateV1 {
        self.navigation.ui_state_v1()
    }

    pub(crate) const fn committed_time_v1(&self) -> Option<CommittedPresentationTimeV1> {
        self.navigation.committed_time_v1()
    }

    pub(crate) const fn play_state_v1(&self) -> RemotePlayStateV1 {
        self.navigation.play_state_v1()
    }

    pub(crate) const fn active_navigation_intent_v1(&self) -> Option<PendingNavigationIntentV1> {
        self.navigation.requested_v1()
    }

    pub(crate) fn pending_navigation_intent_v1(&self) -> Option<PendingNavigationIntentV1> {
        match &self.state {
            RemoteSeekCoordinatorStateV1::Supersedable(state) => state.pending_navigation_intent,
            RemoteSeekCoordinatorStateV1::CommitLocked(state) => state.pending_navigation_intent,
            RemoteSeekCoordinatorStateV1::CurrentSeekFailedBeforeMutation(_)
            | RemoteSeekCoordinatorStateV1::InitialPresentationFailed(_)
            | RemoteSeekCoordinatorStateV1::PresentationCommitPoisoned(_) => None,
        }
    }

    pub(crate) fn generation_v1(&self) -> Option<RemoteSeekGenerationV1> {
        self.state.generation_v1()
    }

    pub(crate) fn work_epoch_v1(&self) -> Option<u64> {
        self.state.work_epoch_v1()
    }

    pub(crate) fn physical_mutation_started_v1(&self) -> bool {
        match &self.state {
            RemoteSeekCoordinatorStateV1::CommitLocked(state) => state.physical_mutation_started,
            _ => false,
        }
    }

    pub(crate) fn commit_lock_v1(&self) -> Option<&RemoteSeekCommitLockV1> {
        self.state.commit_lock_v1()
    }

    pub(crate) fn terminal_failure_v1(&self) -> Option<&RemoteSeekTerminalFailureV1> {
        self.state.terminal_failure_v1()
    }

    pub(crate) fn current_seek_failure_v1(&self) -> Option<&RemoteSeekCurrentSeekFailureV1> {
        self.state.current_seek_failure_v1()
    }

    pub(crate) fn staging_batch_count_v1(&self) -> usize {
        match &self.state {
            RemoteSeekCoordinatorStateV1::Supersedable(state) => state.staging_batches.len(),
            RemoteSeekCoordinatorStateV1::CommitLocked(state) => {
                state.lock.commit_set.batch_ids_v1().len()
            }
            _ => 0,
        }
    }

    pub(crate) fn pin_count_v1(&self) -> usize {
        match &self.state {
            RemoteSeekCoordinatorStateV1::Supersedable(state) => state.pins.len(),
            _ => 0,
        }
    }

    pub(crate) fn reservation_bytes_v1(&self) -> u64 {
        match &self.state {
            RemoteSeekCoordinatorStateV1::Supersedable(state) => state.reservation_bytes,
            _ => 0,
        }
    }

    pub(crate) fn reusable_unloaded_count_v1(&self) -> usize {
        match &self.state {
            RemoteSeekCoordinatorStateV1::Supersedable(state) => {
                state.reusable_unloaded_registration_metadata.len()
            }
            RemoteSeekCoordinatorStateV1::CommitLocked(state) => {
                state.reusable_unloaded_registration_metadata.len()
            }
            RemoteSeekCoordinatorStateV1::CurrentSeekFailedBeforeMutation(state) => {
                state.reusable_unloaded_registration_metadata.len()
            }
            RemoteSeekCoordinatorStateV1::InitialPresentationFailed(_)
            | RemoteSeekCoordinatorStateV1::PresentationCommitPoisoned(_) => 0,
        }
    }

    pub(crate) fn cache_only_result_count_v1(&self) -> usize {
        match &self.state {
            RemoteSeekCoordinatorStateV1::Supersedable(state) => state.cache_only_results.len(),
            _ => 0,
        }
    }

    pub(crate) fn requested_generation_in_flight_v1(&self) -> bool {
        self.navigation.requested_generation_in_flight_v1()
    }

    pub(crate) fn buffering_v1(&self) -> bool {
        self.navigation.buffering_v1()
    }

    pub(crate) fn set_requested_generation_in_flight_v1(&mut self, in_flight: bool) {
        self.navigation
            .set_requested_generation_in_flight_v1(in_flight);
    }

    pub(crate) fn set_buffering_v1(&mut self, buffering: bool) {
        self.navigation.set_buffering_v1(buffering);
    }

    pub(crate) fn accept_navigation_commands_v1(
        &mut self,
        commands: &[RemoteNavigationCommandV1],
    ) -> Result<&RemoteSeekCoordinatorStateV1, RemoteSeekErrorV1> {
        match self.state {
            RemoteSeekCoordinatorStateV1::Supersedable(_) => {
                self.accept_navigation_commands_supersedable_v1(commands)
            }
            RemoteSeekCoordinatorStateV1::CommitLocked(_) => {
                self.accept_navigation_commands_locked_v1(commands)
            }
            RemoteSeekCoordinatorStateV1::CurrentSeekFailedBeforeMutation(_) => {
                self.accept_navigation_commands_failed_v1(commands)
            }
            RemoteSeekCoordinatorStateV1::InitialPresentationFailed(_)
            | RemoteSeekCoordinatorStateV1::PresentationCommitPoisoned(_) => {
                Err(RemoteSeekErrorV1::TerminalState)
            }
        }
    }

    pub(crate) fn record_work_result_v1(
        &mut self,
        result: RemoteSeekWorkResultV1,
    ) -> Result<RemoteSeekResultDispositionV1, RemoteSeekErrorV1> {
        let state = self.state.supersedable_mut_v1()?;
        if result.generation == state.generation {
            state.staging_batches.insert(result.batch_id);
            Ok(RemoteSeekResultDispositionV1::CurrentGeneration)
        } else if result.generation < state.generation {
            state.cache_only_results.insert(result.batch_id);
            Ok(RemoteSeekResultDispositionV1::SupersededCacheOnly)
        } else {
            Err(RemoteSeekErrorV1::InvalidGeneration)
        }
    }

    pub(crate) fn record_pin_v1(
        &mut self,
        batch_id: RemoteSeekBatchIdV1,
    ) -> Result<(), RemoteSeekErrorV1> {
        self.state.supersedable_mut_v1()?.pins.insert(batch_id);
        Ok(())
    }

    pub(crate) fn reserve_bytes_v1(&mut self, bytes: u64) -> Result<(), RemoteSeekErrorV1> {
        let state = self.state.supersedable_mut_v1()?;
        state.reservation_bytes = state
            .reservation_bytes
            .checked_add(bytes)
            .ok_or(RemoteSeekErrorV1::ArithmeticOverflow)?;
        Ok(())
    }

    pub(crate) fn record_reusable_unloaded_v1(
        &mut self,
        batch_id: RemoteSeekBatchIdV1,
    ) -> Result<(), RemoteSeekErrorV1> {
        self.state
            .supersedable_mut_v1()?
            .reusable_unloaded_registration_metadata
            .insert(batch_id);
        Ok(())
    }

    pub(crate) fn mark_facade_closed_for_mutation_v1(
        &mut self,
    ) -> Result<&RemoteSeekCoordinatorStateV1, RemoteSeekErrorV1> {
        self.state.supersedable_mut_v1()?.facade_was_closed = true;
        Ok(self.state_v1())
    }

    pub(crate) fn freeze_commit_v1(
        &mut self,
        lock: RemoteSeekCommitLockV1,
    ) -> Result<&RemoteSeekCoordinatorStateV1, RemoteSeekErrorV1> {
        let supersedable = match &self.state {
            RemoteSeekCoordinatorStateV1::Supersedable(state) => state.clone(),
            _ => return Err(RemoteSeekErrorV1::NotSupersedable),
        };

        if lock.generation_v1() != supersedable.generation {
            return Err(RemoteSeekErrorV1::InvalidGeneration);
        }
        if lock.commit_set_v1().generation_v1() != supersedable.generation {
            return Err(RemoteSeekErrorV1::CommitSetGenerationMismatch);
        }
        if lock.commit_set_v1().batch_ids_v1() != &supersedable.staging_batches {
            return Err(RemoteSeekErrorV1::CommitSetMismatch);
        }
        let active_target = self
            .navigation
            .requested_v1()
            .map(|intent| intent.canonical_target)
            .ok_or(RemoteSeekErrorV1::TargetMismatch)?;
        if active_target != lock.target_v1() {
            return Err(RemoteSeekErrorV1::TargetMismatch);
        }
        let committed_demand_key = supersedable
            .active_demand_key
            .ok_or(RemoteSeekErrorV1::TargetMismatch)?;

        self.state = RemoteSeekCoordinatorStateV1::CommitLocked(RemoteSeekCommitLockedStateV1 {
            lock,
            work_epoch: supersedable.work_epoch,
            initial_presentation: supersedable.initial_presentation,
            committed_demand_key,
            pending_navigation_intent: supersedable.pending_navigation_intent,
            physical_mutation_started: false,
            reusable_unloaded_registration_metadata: supersedable
                .reusable_unloaded_registration_metadata,
            facade_was_closed_before_mutation: supersedable.facade_was_closed,
        });
        Ok(self.state_v1())
    }

    pub(crate) fn begin_physical_mutation_v1(
        &mut self,
    ) -> Result<&RemoteSeekCoordinatorStateV1, RemoteSeekErrorV1> {
        let state = self.state.commit_locked_mut_v1()?;
        if state.physical_mutation_started {
            return Err(RemoteSeekErrorV1::MutationAlreadyStarted);
        }
        state.physical_mutation_started = true;
        Ok(self.state_v1())
    }

    pub(crate) fn complete_commit_v1(
        &mut self,
    ) -> Result<&RemoteSeekCoordinatorStateV1, RemoteSeekErrorV1> {
        let locked = match &self.state {
            RemoteSeekCoordinatorStateV1::CommitLocked(state) => state.clone(),
            _ => return Err(RemoteSeekErrorV1::NotCommitLocked),
        };
        if !locked.physical_mutation_started {
            return Err(RemoteSeekErrorV1::MutationNotStarted);
        }

        let committed = CommittedPresentationTimeV1::new_v1(
            self.navigation.canonical_timeline_v1(),
            locked.lock.target_v1(),
        );
        self.navigation
            .commit_presentation_v1(Some(committed))
            .map_err(RemoteSeekErrorV1::Navigation)?;

        let committed_demand_key = locked.committed_demand_key;
        let pending = locked.pending_navigation_intent;
        let next_demand_key = if let Some(pending) = pending {
            apply_intent_to_navigation_v1(&mut self.navigation, pending)?;
            let demand_key = self
                .navigation
                .requested_v1()
                .map(|intent| intent.demand_key);
            if demand_key == Some(committed_demand_key) {
                self.navigation
                    .commit_presentation_v1(Some(committed))
                    .map_err(RemoteSeekErrorV1::Navigation)?;
                None
            } else {
                demand_key
            }
        } else {
            None
        };

        let generation = if next_demand_key.is_some() {
            locked.lock.generation_v1().next_v1()?
        } else {
            locked.lock.generation_v1()
        };
        let work_epoch = if generation == locked.lock.generation_v1() {
            locked.work_epoch
        } else {
            locked
                .work_epoch
                .checked_add(1)
                .ok_or(RemoteSeekErrorV1::ArithmeticOverflow)?
        };

        if next_demand_key.is_some() {
            self.navigation.set_requested_generation_in_flight_v1(true);
            self.navigation.set_buffering_v1(false);
        } else {
            self.navigation.set_requested_generation_in_flight_v1(false);
        }

        self.state = RemoteSeekCoordinatorStateV1::Supersedable(RemoteSeekSupersedableStateV1 {
            generation,
            work_epoch,
            initial_presentation: false,
            active_demand_key: next_demand_key,
            pending_navigation_intent: None,
            staging_batches: BTreeSet::new(),
            pins: BTreeSet::new(),
            reservation_bytes: 0,
            reusable_unloaded_registration_metadata: locked.reusable_unloaded_registration_metadata,
            cache_only_results: BTreeSet::new(),
            facade_was_closed: false,
        });
        Ok(self.state_v1())
    }

    pub(crate) fn fail_current_seek_before_mutation_v1(
        &mut self,
        failure: RemoteSeekPreMutationFailureV1,
    ) -> Result<&RemoteSeekCoordinatorStateV1, RemoteSeekErrorV1> {
        match &self.state {
            RemoteSeekCoordinatorStateV1::InitialPresentationFailed(_)
            | RemoteSeekCoordinatorStateV1::PresentationCommitPoisoned(_) => {
                return Err(RemoteSeekErrorV1::TerminalAlreadyFrozen);
            }
            RemoteSeekCoordinatorStateV1::CommitLocked(state)
                if state.physical_mutation_started =>
            {
                return Err(RemoteSeekErrorV1::MutationAlreadyStarted);
            }
            RemoteSeekCoordinatorStateV1::CurrentSeekFailedBeforeMutation(_) => {
                return Err(RemoteSeekErrorV1::InvalidFailureTransition);
            }
            _ => {}
        }

        let snapshot = self.failure_snapshot_v1()?;
        if snapshot.initial_presentation {
            self.reset_navigation_for_initial_failure_v1()?;
            self.state = RemoteSeekCoordinatorStateV1::InitialPresentationFailed(
                RemoteSeekTerminalFailureV1::initial_presentation_failed_v1(),
            );
            return Ok(self.state_v1());
        }

        self.rollback_navigation_to_committed_paused_v1()?;
        let work_epoch = snapshot
            .work_epoch
            .checked_add(1)
            .ok_or(RemoteSeekErrorV1::ArithmeticOverflow)?;
        self.state = RemoteSeekCoordinatorStateV1::CurrentSeekFailedBeforeMutation(
            RemoteSeekCurrentSeekFailureV1 {
                failed_generation: snapshot.generation,
                work_epoch,
                failure,
                reusable_unloaded_registration_metadata: snapshot.reusable_unloaded,
                facade_reopened: snapshot.facade_was_closed,
                closure_revalidated: true,
            },
        );
        Ok(self.state_v1())
    }

    pub(crate) fn record_presentation_commit_failure_v1(
        &mut self,
        cause: RemoteSeekPostMutationFailureV1,
    ) -> Result<&RemoteSeekCoordinatorStateV1, RemoteSeekErrorV1> {
        match &self.state {
            RemoteSeekCoordinatorStateV1::InitialPresentationFailed(_)
            | RemoteSeekCoordinatorStateV1::PresentationCommitPoisoned(_) => {
                return Err(RemoteSeekErrorV1::TerminalAlreadyFrozen);
            }
            RemoteSeekCoordinatorStateV1::CommitLocked(state) => {
                if !state.physical_mutation_started {
                    return Err(RemoteSeekErrorV1::MutationNotStarted);
                }
            }
            _ => return Err(RemoteSeekErrorV1::NotCommitLocked),
        }

        self.state = RemoteSeekCoordinatorStateV1::PresentationCommitPoisoned(
            RemoteSeekTerminalFailureV1::presentation_commit_poisoned_v1(cause),
        );
        self.navigation.set_requested_generation_in_flight_v1(false);
        self.navigation.set_buffering_v1(false);
        self.navigation.set_frozen_v1(true);
        Ok(self.state_v1())
    }

    fn accept_navigation_commands_supersedable_v1(
        &mut self,
        commands: &[RemoteNavigationCommandV1],
    ) -> Result<&RemoteSeekCoordinatorStateV1, RemoteSeekErrorV1> {
        let facade_was_closed = self.state.supersedable_v1()?.facade_was_closed;
        if facade_was_closed {
            let base_intent = self
                .state
                .supersedable_v1()?
                .pending_navigation_intent
                .or_else(|| self.navigation.requested_v1());
            let pending = compute_pending_intent_v1(&self.navigation, base_intent, commands)?;
            if pending.is_some() {
                self.state.supersedable_mut_v1()?.pending_navigation_intent = pending;
            }
            return Ok(self.state_v1());
        }

        self.navigation
            .merge_commands_v1(commands)
            .map_err(RemoteSeekErrorV1::Navigation)?;
        let Some(new_demand_key) = self
            .navigation
            .requested_v1()
            .map(|intent| intent.demand_key)
        else {
            return Ok(self.state_v1());
        };

        let demand_changed = {
            let state = self.state.supersedable_v1()?;
            state.active_demand_key != Some(new_demand_key)
        };
        if !demand_changed {
            self.navigation.set_requested_generation_in_flight_v1(true);
            return Ok(self.state_v1());
        }

        let state = self.state.supersedable_mut_v1()?;
        state.generation = state.generation.next_v1()?;
        state.work_epoch = state
            .work_epoch
            .checked_add(1)
            .ok_or(RemoteSeekErrorV1::ArithmeticOverflow)?;
        state.active_demand_key = Some(new_demand_key);
        state.pending_navigation_intent = None;
        state.staging_batches.clear();
        state.pins.clear();
        state.reservation_bytes = 0;
        state.facade_was_closed = false;
        self.navigation.set_requested_generation_in_flight_v1(true);
        self.navigation.set_buffering_v1(false);
        Ok(self.state_v1())
    }

    fn accept_navigation_commands_locked_v1(
        &mut self,
        commands: &[RemoteNavigationCommandV1],
    ) -> Result<&RemoteSeekCoordinatorStateV1, RemoteSeekErrorV1> {
        let base_intent = match &self.state {
            RemoteSeekCoordinatorStateV1::CommitLocked(state) => state.pending_navigation_intent,
            _ => return Err(RemoteSeekErrorV1::NotCommitLocked),
        };
        let pending = compute_pending_intent_v1(&self.navigation, base_intent, commands)?;
        if pending.is_some() {
            self.state.commit_locked_mut_v1()?.pending_navigation_intent = pending;
        }
        Ok(self.state_v1())
    }

    fn accept_navigation_commands_failed_v1(
        &mut self,
        commands: &[RemoteNavigationCommandV1],
    ) -> Result<&RemoteSeekCoordinatorStateV1, RemoteSeekErrorV1> {
        self.navigation
            .merge_commands_v1(commands)
            .map_err(RemoteSeekErrorV1::Navigation)?;
        let Some(new_demand_key) = self
            .navigation
            .requested_v1()
            .map(|intent| intent.demand_key)
        else {
            return Ok(self.state_v1());
        };

        let failure = match &self.state {
            RemoteSeekCoordinatorStateV1::CurrentSeekFailedBeforeMutation(failure) => {
                failure.clone()
            }
            _ => return Err(RemoteSeekErrorV1::InvalidFailureTransition),
        };
        let generation = failure.failed_generation.next_v1()?;
        let work_epoch = failure
            .work_epoch
            .checked_add(1)
            .ok_or(RemoteSeekErrorV1::ArithmeticOverflow)?;
        self.state = RemoteSeekCoordinatorStateV1::Supersedable(RemoteSeekSupersedableStateV1 {
            generation,
            work_epoch,
            initial_presentation: false,
            active_demand_key: Some(new_demand_key),
            pending_navigation_intent: None,
            staging_batches: BTreeSet::new(),
            pins: BTreeSet::new(),
            reservation_bytes: 0,
            reusable_unloaded_registration_metadata: failure
                .reusable_unloaded_registration_metadata,
            cache_only_results: BTreeSet::new(),
            facade_was_closed: false,
        });
        self.navigation.set_requested_generation_in_flight_v1(true);
        self.navigation.set_buffering_v1(false);
        Ok(self.state_v1())
    }

    fn rollback_navigation_to_committed_paused_v1(&mut self) -> Result<(), RemoteSeekErrorV1> {
        let timeline = self.navigation.canonical_timeline_v1();
        let indexed_extent = self.navigation.indexed_extent_v1();
        let committed = self
            .navigation
            .committed_time_v1()
            .ok_or(RemoteSeekErrorV1::NotCommittedPresentation)?;
        let playback_speed = self.navigation.playback_speed_v1();

        let mut navigation =
            RemoteNavigationAdapterV1::new_v1(timeline, RemotePlayStateV1::Paused, Some(committed))
                .map_err(RemoteSeekErrorV1::Navigation)?;
        navigation
            .preview_update_v1(
                indexed_extent,
                RemoteLoopModeV1::Clamp,
                RemoteCandidateClockInputV1::new_v1(Duration::from_nanos(0), true),
            )
            .map_err(RemoteSeekErrorV1::Navigation)?;
        if playback_speed != 1 {
            let speed = i64::try_from(playback_speed)
                .map_err(|_error| RemoteSeekErrorV1::ArithmeticOverflow)?;
            navigation
                .merge_commands_v1(&[RemoteNavigationCommandV1::SetPlaybackSpeed {
                    timeline,
                    speed,
                }])
                .map_err(RemoteSeekErrorV1::Navigation)?;
        }
        self.navigation = navigation;
        Ok(())
    }

    fn reset_navigation_for_initial_failure_v1(&mut self) -> Result<(), RemoteSeekErrorV1> {
        let timeline = self.navigation.canonical_timeline_v1();
        let indexed_extent = self.navigation.indexed_extent_v1();
        let mut navigation =
            RemoteNavigationAdapterV1::new_v1(timeline, RemotePlayStateV1::Paused, None)
                .map_err(RemoteSeekErrorV1::Navigation)?;
        navigation
            .preview_update_v1(
                indexed_extent,
                RemoteLoopModeV1::Clamp,
                RemoteCandidateClockInputV1::new_v1(Duration::from_nanos(0), true),
            )
            .map_err(RemoteSeekErrorV1::Navigation)?;
        self.navigation = navigation;
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct RemoteSeekFailureSnapshotV1 {
    initial_presentation: bool,
    generation: RemoteSeekGenerationV1,
    work_epoch: u64,
    reusable_unloaded: BTreeSet<RemoteSeekBatchIdV1>,
    facade_was_closed: bool,
}

impl RemoteSeekCoordinatorV1 {
    fn failure_snapshot_v1(&self) -> Result<RemoteSeekFailureSnapshotV1, RemoteSeekErrorV1> {
        match &self.state {
            RemoteSeekCoordinatorStateV1::Supersedable(state) => Ok(RemoteSeekFailureSnapshotV1 {
                initial_presentation: state.initial_presentation,
                generation: state.generation,
                work_epoch: state.work_epoch,
                reusable_unloaded: state.reusable_unloaded_registration_metadata.clone(),
                facade_was_closed: state.facade_was_closed,
            }),
            RemoteSeekCoordinatorStateV1::CommitLocked(state) => Ok(RemoteSeekFailureSnapshotV1 {
                initial_presentation: state.initial_presentation,
                generation: state.lock.generation,
                work_epoch: state.work_epoch,
                reusable_unloaded: state.reusable_unloaded_registration_metadata.clone(),
                facade_was_closed: state.facade_was_closed_before_mutation,
            }),
            RemoteSeekCoordinatorStateV1::CurrentSeekFailedBeforeMutation(_) => {
                Err(RemoteSeekErrorV1::InvalidFailureTransition)
            }
            RemoteSeekCoordinatorStateV1::InitialPresentationFailed(_)
            | RemoteSeekCoordinatorStateV1::PresentationCommitPoisoned(_) => {
                Err(RemoteSeekErrorV1::TerminalAlreadyFrozen)
            }
        }
    }
}

impl RemoteSeekCoordinatorStateV1 {
    fn supersedable_v1(&self) -> Result<&RemoteSeekSupersedableStateV1, RemoteSeekErrorV1> {
        match self {
            Self::Supersedable(state) => Ok(state),
            _ => Err(RemoteSeekErrorV1::NotSupersedable),
        }
    }

    fn supersedable_mut_v1(
        &mut self,
    ) -> Result<&mut RemoteSeekSupersedableStateV1, RemoteSeekErrorV1> {
        match self {
            Self::Supersedable(state) => Ok(state),
            _ => Err(RemoteSeekErrorV1::NotSupersedable),
        }
    }

    fn commit_locked_mut_v1(
        &mut self,
    ) -> Result<&mut RemoteSeekCommitLockedStateV1, RemoteSeekErrorV1> {
        match self {
            Self::CommitLocked(state) => Ok(state),
            _ => Err(RemoteSeekErrorV1::NotCommitLocked),
        }
    }
}

fn apply_intent_to_navigation_v1(
    navigation: &mut RemoteNavigationAdapterV1,
    intent: PendingNavigationIntentV1,
) -> Result<(), RemoteSeekErrorV1> {
    let timeline = navigation.canonical_timeline_v1();
    let play_command = match intent.play_state {
        RemotePlayStateV1::Paused => RemoteNavigationCommandV1::Pause { timeline },
        RemotePlayStateV1::Playing => RemoteNavigationCommandV1::Play { timeline },
    };
    navigation
        .merge_commands_v1(&[
            RemoteNavigationCommandV1::Seek {
                timeline,
                target: intent.canonical_target,
            },
            play_command,
        ])
        .map_err(RemoteSeekErrorV1::Navigation)?;
    Ok(())
}

fn compute_pending_intent_v1(
    base_navigation: &RemoteNavigationAdapterV1,
    base_intent: Option<PendingNavigationIntentV1>,
    commands: &[RemoteNavigationCommandV1],
) -> Result<Option<PendingNavigationIntentV1>, RemoteSeekErrorV1> {
    if !commands
        .iter()
        .any(|command| command.trigger_v1().is_some())
    {
        return Ok(base_intent);
    }

    let mut navigation = if let Some(base_intent) = base_intent {
        adapter_with_pending_intent_v1(base_navigation, base_intent)?
    } else {
        base_navigation.clone()
    };
    navigation
        .merge_commands_v1(commands)
        .map_err(RemoteSeekErrorV1::Navigation)?;
    Ok(navigation.requested_v1())
}

fn adapter_with_pending_intent_v1(
    base_navigation: &RemoteNavigationAdapterV1,
    intent: PendingNavigationIntentV1,
) -> Result<RemoteNavigationAdapterV1, RemoteSeekErrorV1> {
    let mut navigation = RemoteNavigationAdapterV1::new_v1(
        base_navigation.canonical_timeline_v1(),
        intent.play_state,
        base_navigation.committed_time_v1(),
    )
    .map_err(RemoteSeekErrorV1::Navigation)?;
    navigation
        .preview_update_v1(
            base_navigation.indexed_extent_v1(),
            RemoteLoopModeV1::Clamp,
            RemoteCandidateClockInputV1::new_v1(Duration::from_nanos(0), true),
        )
        .map_err(RemoteSeekErrorV1::Navigation)?;
    apply_intent_to_navigation_v1(&mut navigation, intent)?;
    Ok(navigation)
}

#[cfg(test)]
mod tests {
    use re_log_types::{AbsoluteTimeRange, TimeType, Timeline};

    use super::*;

    fn canonical_timeline() -> Timeline {
        Timeline::new("message_log_time", TimeType::DurationNs)
    }

    fn extent() -> crate::remote_loaded_coverage::CanonicalIndexedExtentV1 {
        crate::remote_loaded_coverage::CanonicalIndexedExtentV1::Known(AbsoluteTimeRange::new(
            0, 100,
        ))
    }

    fn committed_at(cursor: i64) -> CommittedPresentationTimeV1 {
        CommittedPresentationTimeV1::new_v1(canonical_timeline(), TimeInt::new_temporal(cursor))
    }

    fn navigation() -> RemoteNavigationAdapterV1 {
        RemoteNavigationAdapterV1::from_extent_v1(
            canonical_timeline(),
            extent(),
            Some(committed_at(10)),
        )
        .expect("known extent adapter should construct")
    }

    fn command(command: RemoteNavigationCommandV1) -> Vec<RemoteNavigationCommandV1> {
        vec![command]
    }

    fn seek(target: i64) -> Vec<RemoteNavigationCommandV1> {
        command(RemoteNavigationCommandV1::Seek {
            timeline: canonical_timeline(),
            target: TimeInt::new_temporal(target),
        })
    }

    fn batch_id(value: u64) -> RemoteSeekBatchIdV1 {
        RemoteSeekBatchIdV1::from_u64_v1(value)
    }

    fn commit_set(generation: RemoteSeekGenerationV1, batch_ids: &[u64]) -> RemoteSeekCommitSetV1 {
        RemoteSeekCommitSetV1::new_v1(
            generation,
            batch_ids
                .iter()
                .copied()
                .map(RemoteSeekBatchIdV1::from_u64_v1)
                .collect(),
        )
        .expect("test commit set should be non-empty")
    }

    fn seeded_supersedable() -> RemoteSeekCoordinatorV1 {
        let mut coordinator =
            RemoteSeekCoordinatorV1::new_committed_v1(navigation()).expect("committed coordinator");
        coordinator
            .accept_navigation_commands_v1(&seek(30))
            .expect("seek should be accepted");
        coordinator
            .record_work_result_v1(RemoteSeekWorkResultV1::new_v1(
                coordinator.generation_v1().expect("current generation"),
                batch_id(1),
            ))
            .expect("current generation result should be staged");
        coordinator
            .record_pin_v1(batch_id(1))
            .expect("pin should be added");
        coordinator.reserve_bytes_v1(128).expect("reservation");
        coordinator
            .record_reusable_unloaded_v1(batch_id(9))
            .expect("reusable metadata should be recorded");
        coordinator
    }

    #[test]
    fn commit_locked_commands_only_replace_latest_pending_intent() {
        let mut coordinator = seeded_supersedable();
        coordinator
            .mark_facade_closed_for_mutation_v1()
            .expect("facade close marker");
        let generation = coordinator.generation_v1().expect("generation");
        let lock = RemoteSeekCommitLockV1::new_v1(
            generation,
            TimeInt::new_temporal(30),
            commit_set(generation, &[1]),
        )
        .expect("commit lock");
        coordinator
            .freeze_commit_v1(lock.clone())
            .expect("freeze commit");
        let active_before = coordinator.active_navigation_intent_v1();
        let commit_before = coordinator.commit_lock_v1().expect("commit lock").clone();

        coordinator
            .accept_navigation_commands_v1(&seek(40))
            .expect("locked seek pending");
        coordinator
            .accept_navigation_commands_v1(&seek(50))
            .expect("latest-wins locked seek pending");

        assert_eq!(
            coordinator
                .pending_navigation_intent_v1()
                .map(|intent| intent.canonical_target),
            Some(TimeInt::new_temporal(50))
        );
        assert_eq!(coordinator.active_navigation_intent_v1(), active_before);
        assert_eq!(
            coordinator.commit_lock_v1().expect("commit lock"),
            &commit_before
        );
        assert!(coordinator.state_v1().is_commit_locked_v1());
    }

    #[test]
    fn superseded_result_is_cache_only_and_does_not_enter_commit_set() {
        let mut coordinator =
            RemoteSeekCoordinatorV1::new_committed_v1(navigation()).expect("committed coordinator");
        let first_generation = coordinator
            .accept_navigation_commands_v1(&seek(30))
            .expect("first seek")
            .generation_v1()
            .expect("first generation");
        coordinator
            .record_work_result_v1(RemoteSeekWorkResultV1::new_v1(
                first_generation,
                batch_id(1),
            ))
            .expect("first result");

        let second_generation = coordinator
            .accept_navigation_commands_v1(&seek(40))
            .expect("second seek")
            .generation_v1()
            .expect("second generation");
        assert_ne!(first_generation, second_generation);

        let disposition = coordinator
            .record_work_result_v1(RemoteSeekWorkResultV1::new_v1(
                first_generation,
                batch_id(2),
            ))
            .expect("stale result should be cacheable");
        assert_eq!(
            disposition,
            RemoteSeekResultDispositionV1::SupersededCacheOnly
        );
        assert_eq!(coordinator.cache_only_result_count_v1(), 1);
        assert_eq!(coordinator.staging_batch_count_v1(), 0);
    }

    #[test]
    fn current_seek_failure_rolls_back_and_reopens_pre_mutation_state() {
        let mut coordinator = seeded_supersedable();
        coordinator
            .mark_facade_closed_for_mutation_v1()
            .expect("facade close marker");
        let generation = coordinator.generation_v1().expect("generation");
        let lock = RemoteSeekCommitLockV1::new_v1(
            generation,
            TimeInt::new_temporal(30),
            commit_set(generation, &[1]),
        )
        .expect("commit lock");
        coordinator.freeze_commit_v1(lock).expect("freeze commit");
        coordinator.set_requested_generation_in_flight_v1(true);
        coordinator.set_buffering_v1(true);

        coordinator
            .fail_current_seek_before_mutation_v1(
                RemoteSeekPreMutationFailureV1::RegistrationPreflight,
            )
            .expect("pre-mutation failure should roll back");

        assert!(
            coordinator
                .state_v1()
                .is_current_seek_failed_before_mutation_v1()
        );
        assert_eq!(coordinator.committed_time_v1(), Some(committed_at(10)));
        assert_eq!(coordinator.play_state_v1(), RemotePlayStateV1::Paused);
        assert_eq!(coordinator.active_navigation_intent_v1(), None);
        assert_eq!(coordinator.pending_navigation_intent_v1(), None);
        assert!(!coordinator.requested_generation_in_flight_v1());
        assert!(!coordinator.buffering_v1());
        assert_eq!(coordinator.staging_batch_count_v1(), 0);
        assert_eq!(coordinator.pin_count_v1(), 0);
        assert_eq!(coordinator.reservation_bytes_v1(), 0);
        assert_eq!(coordinator.reusable_unloaded_count_v1(), 1);
        let failure = coordinator
            .current_seek_failure_v1()
            .expect("current failure");
        assert!(failure.facade_reopened);
        assert!(failure.closure_revalidated);
    }

    #[test]
    fn initial_presentation_failure_is_terminal_and_never_open() {
        let initial_intent = PendingNavigationIntentV1::new_v1(
            TimeInt::new_temporal(0),
            RemotePlayStateV1::Playing,
            RemoteNavigationTriggerV1::Seek,
        )
        .expect("initial intent");
        let mut coordinator = RemoteSeekCoordinatorV1::new_initial_presentation_v1(
            RemoteNavigationAdapterV1::from_extent_v1(canonical_timeline(), extent(), None)
                .expect("initial navigation"),
            initial_intent,
        )
        .expect("initial coordinator");
        coordinator.set_buffering_v1(true);

        coordinator
            .fail_current_seek_before_mutation_v1(RemoteSeekPreMutationFailureV1::RetryExhausted)
            .expect("initial failure should become terminal");

        assert!(coordinator.state_v1().is_initial_presentation_failed_v1());
        assert!(
            coordinator
                .terminal_failure_v1()
                .expect("terminal latch")
                .is_initial_presentation_failed_v1()
        );
        assert_eq!(coordinator.committed_time_v1(), None);
        assert_eq!(coordinator.active_navigation_intent_v1(), None);
        assert!(!coordinator.requested_generation_in_flight_v1());
        assert!(!coordinator.buffering_v1());
        assert_eq!(coordinator.staging_batch_count_v1(), 0);
        assert_eq!(coordinator.pin_count_v1(), 0);
        assert_eq!(coordinator.reservation_bytes_v1(), 0);

        let error = coordinator
            .accept_navigation_commands_v1(&seek(20))
            .expect_err("initial terminal must reject navigation");
        assert_eq!(error, RemoteSeekErrorV1::TerminalState);
    }

    #[test]
    fn presentation_commit_poison_is_first_cause_wins() {
        let mut coordinator = seeded_supersedable();
        let generation = coordinator.generation_v1().expect("generation");
        let lock = RemoteSeekCommitLockV1::new_v1(
            generation,
            TimeInt::new_temporal(30),
            commit_set(generation, &[1]),
        )
        .expect("commit lock");
        coordinator.freeze_commit_v1(lock).expect("freeze commit");
        coordinator
            .begin_physical_mutation_v1()
            .expect("mutation start");

        coordinator
            .record_presentation_commit_failure_v1(RemoteSeekPostMutationFailureV1::Insertion)
            .expect("poison");
        let first = coordinator
            .terminal_failure_v1()
            .expect("terminal")
            .cause_v1();

        let error = coordinator
            .record_presentation_commit_failure_v1(RemoteSeekPostMutationFailureV1::Residency)
            .expect_err("first terminal cause wins");
        assert_eq!(error, RemoteSeekErrorV1::TerminalAlreadyFrozen);
        assert_eq!(
            coordinator
                .terminal_failure_v1()
                .expect("terminal")
                .cause_v1(),
            first
        );
        assert_eq!(
            coordinator
                .accept_navigation_commands_v1(&seek(40))
                .expect_err("poisoned terminal rejects navigation"),
            RemoteSeekErrorV1::TerminalState
        );
    }

    #[test]
    fn successful_locked_commit_applies_latest_pending_and_advances_generation_when_demand_differs()
    {
        let mut coordinator = seeded_supersedable();
        let generation = coordinator.generation_v1().expect("generation");
        let lock = RemoteSeekCommitLockV1::new_v1(
            generation,
            TimeInt::new_temporal(30),
            commit_set(generation, &[1]),
        )
        .expect("commit lock");
        coordinator.freeze_commit_v1(lock).expect("freeze commit");
        coordinator
            .accept_navigation_commands_v1(&seek(50))
            .expect("pending latest wins");
        coordinator
            .begin_physical_mutation_v1()
            .expect("mutation start");

        coordinator
            .complete_commit_v1()
            .expect("commit should complete");

        assert!(coordinator.state_v1().is_supersedable_v1());
        assert_eq!(coordinator.committed_time_v1(), Some(committed_at(30)));
        assert_eq!(
            coordinator
                .active_navigation_intent_v1()
                .map(|intent| intent.canonical_target),
            Some(TimeInt::new_temporal(50))
        );
        assert_eq!(
            coordinator
                .generation_v1()
                .expect("next generation")
                .as_u64_v1(),
            generation.as_u64_v1() + 1
        );
    }

    #[test]
    fn failed_state_requires_explicit_navigation_to_start_next_demand() {
        let mut coordinator = seeded_supersedable();
        let failed_generation = coordinator.generation_v1().expect("generation");
        coordinator
            .fail_current_seek_before_mutation_v1(RemoteSeekPreMutationFailureV1::RetryExhausted)
            .expect("pre-mutation failure");

        coordinator
            .accept_navigation_commands_v1(&command(RemoteNavigationCommandV1::SetPlaybackSpeed {
                timeline: canonical_timeline(),
                speed: 2,
            }))
            .expect("speed command should not restart demand");
        assert!(
            coordinator
                .state_v1()
                .is_current_seek_failed_before_mutation_v1()
        );

        coordinator
            .accept_navigation_commands_v1(&seek(45))
            .expect("explicit seek should restart demand");
        assert!(coordinator.state_v1().is_supersedable_v1());
        assert_eq!(
            coordinator
                .generation_v1()
                .expect("new generation")
                .as_u64_v1(),
            failed_generation.as_u64_v1() + 1
        );
    }
}
