//! Production-disarmed remote Store mutation arbiter for MCAP-100.
//!
//! This module owns the typed turn boundary for query-visible insertion, garbage collection, and
//! terminal cleanup. It reuses MCAP-095's sealed privileged frame authority and facade mutation
//! primitives without importing or publishing a `StoreHub`, `StoreBundle`, `EntityDb`, storage
//! engine, or ordinary Viewer mutation path.

#![allow(dead_code)]

use crate::web_remote_mcap_query::{
    CommittedPresentationTimeV1, ConsumerStorageFreeV1, PrivilegedViewerFrameContextV1,
    RemoteMutationLeaseDrainV1, RemotePresentationFacadeV1, RemotePresentationTransitionErrorV1,
};

/// Store mutation kinds that may own a remote mutation turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteMutationKindV1 {
    QueryVisibleInsertion,
    GarbageCollection,
    TerminalCleanup,
}

/// Monotonic, non-reusable remote mutation request token.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct RemoteMutationRequestIdV1(u64);

impl RemoteMutationRequestIdV1 {
    pub(crate) const fn get_v1(self) -> u64 {
        self.0
    }
}

/// Cross-frame safe points recognized by the arbiter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteMutationSafePointV1 {
    WaitingForLeases,
    BeforeFirstAddChunk,
    BetweenAddChunks,
    AfterLastAddChunkBeforeAck,
    GcWaitingForLeases,
    AfterGcBeforeReopen,
}

/// Mutation execution state.
///
/// Only the safe-point variants can be suspended, preempted, or resumed by another remote
/// controller transition. The synchronous `Running*` variants are deliberately not safe points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteMutationSubstateV1 {
    SafePoint(RemoteMutationSafePointV1),
    RunningAddChunk,
    RunningGc,
}

impl RemoteMutationSubstateV1 {
    const fn safe_point_v1(self) -> Option<RemoteMutationSafePointV1> {
        match self {
            Self::SafePoint(safe_point) => Some(safe_point),
            Self::RunningAddChunk | Self::RunningGc => None,
        }
    }
}

/// One owned remote mutation turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteMutationTurnV1 {
    kind: RemoteMutationKindV1,
    request_id: RemoteMutationRequestIdV1,
    generation: u64,
    substate: RemoteMutationSubstateV1,
}

impl RemoteMutationTurnV1 {
    fn new_v1(
        kind: RemoteMutationKindV1,
        request_id: RemoteMutationRequestIdV1,
        generation: u64,
        substate: RemoteMutationSubstateV1,
    ) -> Self {
        Self {
            kind,
            request_id,
            generation,
            substate,
        }
    }

    pub(crate) const fn kind_v1(&self) -> RemoteMutationKindV1 {
        self.kind
    }

    pub(crate) const fn request_id_v1(&self) -> RemoteMutationRequestIdV1 {
        self.request_id
    }

    pub(crate) const fn generation_v1(&self) -> u64 {
        self.generation
    }

    pub(crate) fn safe_point_v1(&self) -> Option<RemoteMutationSafePointV1> {
        self.substate.safe_point_v1()
    }
}

/// Opaque one-shot nonce used to rebind a page-hidden mutation turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct RemoteMutationResumeNonceV1(u64);

impl RemoteMutationResumeNonceV1 {
    pub(crate) const fn get_v1(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteMutationArbiterErrorV1 {
    ActiveTurnAlreadyExists,
    CloseAlreadyRequested,
    NoActiveTurn,
    NoCloseRequested,
    NotAtSafePoint,
    RunningMutationNotInterleavable,
    InvalidMutationSubstate,
    SuspendedForPageHidden,
    AlreadySuspendedForPageHidden,
    NotSuspendedForPageHidden,
    StaleResumeNonce,
    StaleCompletion,
    DuplicateCompletion,
    RequestIdExhausted,
    ResumeNonceExhausted,
    GenerationExhausted,
    LeaseDrainStillPending,
    PresentationTransition(RemotePresentationTransitionErrorV1),
}

/// Result of advancing a close request through terminal cleanup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteMutationCloseProgressV1 {
    TerminalGateStarted,
    WaitingForLeaseDrain,
    Complete,
}

/// Add-chunk boundary returned by the disarmed insertion controller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteMutationAddChunkOutcomeV1 {
    MoreChunksRemain,
    LastChunk,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RemoteMutationSuspensionV1 {
    turn: RemoteMutationTurnV1,
    resume_nonce: Option<RemoteMutationResumeNonceV1>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RemoteMutationTerminalCleanupV1 {
    request_id: RemoteMutationRequestIdV1,
    generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RemoteStoreMutationArbiterStateV1 {
    next_request_id: u64,
    generation: u64,
    next_resume_nonce: u64,
    active: Option<RemoteMutationTurnV1>,
    suspended: Option<RemoteMutationSuspensionV1>,
    close_requested: bool,
    close_request_id: Option<RemoteMutationRequestIdV1>,
    terminal_cleanup: Option<RemoteMutationTerminalCleanupV1>,
    last_completed: Option<RemoteMutationTurnV1>,
}

impl RemoteStoreMutationArbiterStateV1 {
    const fn new_v1() -> Self {
        Self {
            next_request_id: 1,
            generation: 0,
            next_resume_nonce: 1,
            active: None,
            suspended: None,
            close_requested: false,
            close_request_id: None,
            terminal_cleanup: None,
            last_completed: None,
        }
    }
}

/// The only remote Store mutation turn-ownership boundary.
///
/// It holds a sealed frame authority, the query facade, and private arbiter state. It never holds
/// or returns a physical Store handle.
pub(crate) struct RemoteStoreMutationArbiterV1<'a> {
    frame: PrivilegedViewerFrameContextV1,
    facade: &'a RemotePresentationFacadeV1,
    state: RemoteStoreMutationArbiterStateV1,
}

impl<'a> RemoteStoreMutationArbiterV1<'a> {
    pub(crate) fn new_disarmed_v1(
        frame: PrivilegedViewerFrameContextV1,
        facade: &'a RemotePresentationFacadeV1,
    ) -> Self {
        Self {
            frame,
            facade,
            state: RemoteStoreMutationArbiterStateV1::new_v1(),
        }
    }

    pub(crate) fn active_turn_v1(&self) -> Option<&RemoteMutationTurnV1> {
        self.state.active.as_ref()
    }

    pub(crate) fn suspended_turn_v1(&self) -> Option<&RemoteMutationTurnV1> {
        self.state
            .suspended
            .as_ref()
            .map(|suspension| &suspension.turn)
    }

    pub(crate) fn is_close_requested_v1(&self) -> bool {
        self.state.close_requested
    }

    pub(crate) fn request_query_visible_insertion_v1(
        &mut self,
    ) -> Result<RemoteMutationTurnV1, RemoteMutationArbiterErrorV1> {
        if self.state.close_requested {
            return Err(RemoteMutationArbiterErrorV1::CloseAlreadyRequested);
        }
        if self.state.active.is_some() {
            return Err(RemoteMutationArbiterErrorV1::ActiveTurnAlreadyExists);
        }
        if self.state.suspended.is_some() {
            return Err(RemoteMutationArbiterErrorV1::SuspendedForPageHidden);
        }

        let request_id = self.allocate_request_id_v1()?;
        let generation = self.advance_generation_v1()?;

        let drain = self
            .frame
            .remote_storage_capability_v1(self.facade)
            .begin_query_visible_insertion_v1()
            .map_err(RemoteMutationArbiterErrorV1::PresentationTransition)?;
        let substate = match drain {
            RemoteMutationLeaseDrainV1::NoLiveLeases => {
                RemoteMutationSubstateV1::SafePoint(RemoteMutationSafePointV1::BeforeFirstAddChunk)
            }
            RemoteMutationLeaseDrainV1::WaitingForLiveLeaseDrain => {
                RemoteMutationSubstateV1::SafePoint(RemoteMutationSafePointV1::WaitingForLeases)
            }
        };
        let turn = RemoteMutationTurnV1::new_v1(
            RemoteMutationKindV1::QueryVisibleInsertion,
            request_id,
            generation,
            substate,
        );
        self.state.active = Some(turn.clone());
        Ok(turn)
    }

    pub(crate) fn request_garbage_collection_v1(
        &mut self,
    ) -> Result<RemoteMutationTurnV1, RemoteMutationArbiterErrorV1> {
        if self.state.close_requested {
            return Err(RemoteMutationArbiterErrorV1::CloseAlreadyRequested);
        }
        if self.state.active.is_some() {
            return Err(RemoteMutationArbiterErrorV1::ActiveTurnAlreadyExists);
        }
        if self.state.suspended.is_some() {
            return Err(RemoteMutationArbiterErrorV1::SuspendedForPageHidden);
        }

        let request_id = self.allocate_request_id_v1()?;
        let generation = self.advance_generation_v1()?;

        let drain = self
            .frame
            .remote_storage_capability_v1(self.facade)
            .begin_garbage_collection_v1()
            .map_err(RemoteMutationArbiterErrorV1::PresentationTransition)?;
        let substate = match drain {
            RemoteMutationLeaseDrainV1::NoLiveLeases => {
                RemoteMutationSubstateV1::SafePoint(RemoteMutationSafePointV1::AfterGcBeforeReopen)
            }
            RemoteMutationLeaseDrainV1::WaitingForLiveLeaseDrain => {
                RemoteMutationSubstateV1::SafePoint(RemoteMutationSafePointV1::GcWaitingForLeases)
            }
        };
        let turn = RemoteMutationTurnV1::new_v1(
            RemoteMutationKindV1::GarbageCollection,
            request_id,
            generation,
            substate,
        );
        self.state.active = Some(turn.clone());
        Ok(turn)
    }

    pub(crate) fn drive_lease_wait_v1(
        &mut self,
    ) -> Result<RemoteMutationSafePointV1, RemoteMutationArbiterErrorV1> {
        if self.state.close_requested {
            return Err(RemoteMutationArbiterErrorV1::CloseAlreadyRequested);
        }
        let Some(turn) = self.state.active.as_ref() else {
            return Err(RemoteMutationArbiterErrorV1::NoActiveTurn);
        };

        let (waiting_safe_point, next_safe_point) = match turn.substate {
            RemoteMutationSubstateV1::SafePoint(RemoteMutationSafePointV1::WaitingForLeases) => (
                RemoteMutationSafePointV1::WaitingForLeases,
                RemoteMutationSafePointV1::BeforeFirstAddChunk,
            ),
            RemoteMutationSubstateV1::SafePoint(RemoteMutationSafePointV1::GcWaitingForLeases) => (
                RemoteMutationSafePointV1::GcWaitingForLeases,
                RemoteMutationSafePointV1::AfterGcBeforeReopen,
            ),
            _ => return Err(RemoteMutationArbiterErrorV1::InvalidMutationSubstate),
        };
        re_log::debug_assert_eq!(turn.safe_point_v1(), Some(waiting_safe_point));

        let drain_ready = self
            .frame
            .remote_storage_capability_v1(self.facade)
            .take_lease_drain_ready_v1();
        if !drain_ready && self.facade.live_lease_count_v1() != 0 {
            return Err(RemoteMutationArbiterErrorV1::LeaseDrainStillPending);
        }

        let turn = self
            .state
            .active
            .as_mut()
            .expect("active turn was checked above");
        turn.substate = RemoteMutationSubstateV1::SafePoint(next_safe_point);
        Ok(next_safe_point)
    }

    pub(crate) fn begin_add_chunk_v1(
        &mut self,
        request_id: RemoteMutationRequestIdV1,
        generation: u64,
    ) -> Result<RemoteMutationSubstateV1, RemoteMutationArbiterErrorV1> {
        if self.state.close_requested {
            return Err(RemoteMutationArbiterErrorV1::CloseAlreadyRequested);
        }
        let Some(turn) = self.state.active.as_mut() else {
            return Err(RemoteMutationArbiterErrorV1::NoActiveTurn);
        };
        if turn.request_id != request_id
            || turn.generation != generation
            || turn.kind != RemoteMutationKindV1::QueryVisibleInsertion
        {
            return Err(RemoteMutationArbiterErrorV1::StaleCompletion);
        }
        if !matches!(
            turn.substate,
            RemoteMutationSubstateV1::SafePoint(
                RemoteMutationSafePointV1::BeforeFirstAddChunk
                    | RemoteMutationSafePointV1::BetweenAddChunks,
            )
        ) {
            return Err(RemoteMutationArbiterErrorV1::InvalidMutationSubstate);
        }

        turn.substate = RemoteMutationSubstateV1::RunningAddChunk;
        Ok(turn.substate)
    }

    pub(crate) fn complete_add_chunk_v1(
        &mut self,
        request_id: RemoteMutationRequestIdV1,
        generation: u64,
        outcome: RemoteMutationAddChunkOutcomeV1,
    ) -> Result<RemoteMutationSubstateV1, RemoteMutationArbiterErrorV1> {
        let Some(turn) = self.state.active.as_mut() else {
            return Err(RemoteMutationArbiterErrorV1::NoActiveTurn);
        };
        if turn.request_id != request_id
            || turn.generation != generation
            || turn.kind != RemoteMutationKindV1::QueryVisibleInsertion
        {
            return Err(RemoteMutationArbiterErrorV1::StaleCompletion);
        }
        if turn.substate != RemoteMutationSubstateV1::RunningAddChunk {
            return Err(RemoteMutationArbiterErrorV1::InvalidMutationSubstate);
        }

        turn.substate = match outcome {
            RemoteMutationAddChunkOutcomeV1::MoreChunksRemain => {
                RemoteMutationSubstateV1::SafePoint(RemoteMutationSafePointV1::BetweenAddChunks)
            }
            RemoteMutationAddChunkOutcomeV1::LastChunk => RemoteMutationSubstateV1::SafePoint(
                RemoteMutationSafePointV1::AfterLastAddChunkBeforeAck,
            ),
        };
        if self.state.close_requested {
            Err(RemoteMutationArbiterErrorV1::CloseAlreadyRequested)
        } else {
            Ok(turn.substate)
        }
    }

    pub(crate) fn acknowledge_partitions_resident_v1(
        &self,
        request_id: RemoteMutationRequestIdV1,
        generation: u64,
    ) -> Result<(), RemoteMutationArbiterErrorV1> {
        if self.state.close_requested {
            return Err(RemoteMutationArbiterErrorV1::CloseAlreadyRequested);
        }
        let Some(turn) = self.state.active.as_ref() else {
            return Err(RemoteMutationArbiterErrorV1::NoActiveTurn);
        };
        if turn.request_id != request_id
            || turn.generation != generation
            || turn.kind != RemoteMutationKindV1::QueryVisibleInsertion
        {
            return Err(RemoteMutationArbiterErrorV1::StaleCompletion);
        }
        if turn.substate
            != RemoteMutationSubstateV1::SafePoint(
                RemoteMutationSafePointV1::AfterLastAddChunkBeforeAck,
            )
        {
            return Err(RemoteMutationArbiterErrorV1::InvalidMutationSubstate);
        }

        // A real adapter would publish partition residency here. The disarmed arbiter keeps the
        // same safe point and does not mutate the facade.
        Ok(())
    }

    pub(crate) fn complete_query_visible_insertion_v1(
        &mut self,
        request_id: RemoteMutationRequestIdV1,
        generation: u64,
    ) -> Result<(), RemoteMutationArbiterErrorV1> {
        if self.state.close_requested {
            return Err(RemoteMutationArbiterErrorV1::CloseAlreadyRequested);
        }

        let Some(turn) = self.state.active.as_ref() else {
            return self.stale_or_duplicate_completion_v1(request_id, generation);
        };
        if turn.request_id != request_id
            || turn.generation != generation
            || turn.kind != RemoteMutationKindV1::QueryVisibleInsertion
        {
            return Err(RemoteMutationArbiterErrorV1::StaleCompletion);
        }
        if turn.substate
            != RemoteMutationSubstateV1::SafePoint(
                RemoteMutationSafePointV1::AfterLastAddChunkBeforeAck,
            )
        {
            return Err(RemoteMutationArbiterErrorV1::InvalidMutationSubstate);
        }

        self.frame
            .remote_storage_capability_v1(self.facade)
            .finish_query_visible_insertion_v1()
            .map_err(RemoteMutationArbiterErrorV1::PresentationTransition)?;
        self.finish_active_turn_v1();
        Ok(())
    }

    pub(crate) fn complete_garbage_collection_v1(
        &mut self,
        request_id: RemoteMutationRequestIdV1,
        generation: u64,
        query_visible_deletion: bool,
    ) -> Result<(), RemoteMutationArbiterErrorV1> {
        if self.state.close_requested {
            return Err(RemoteMutationArbiterErrorV1::CloseAlreadyRequested);
        }

        let Some(turn) = self.state.active.as_ref() else {
            return self.stale_or_duplicate_completion_v1(request_id, generation);
        };
        if turn.request_id != request_id
            || turn.generation != generation
            || turn.kind != RemoteMutationKindV1::GarbageCollection
        {
            return Err(RemoteMutationArbiterErrorV1::StaleCompletion);
        }
        if turn.substate
            != RemoteMutationSubstateV1::SafePoint(RemoteMutationSafePointV1::AfterGcBeforeReopen)
        {
            return Err(RemoteMutationArbiterErrorV1::InvalidMutationSubstate);
        }

        self.frame
            .remote_storage_capability_v1(self.facade)
            .finish_garbage_collection_v1(query_visible_deletion)
            .map_err(RemoteMutationArbiterErrorV1::PresentationTransition)?;
        self.finish_active_turn_v1();
        Ok(())
    }

    pub(crate) fn commit_presentation_v1(
        &self,
        committed_time: Option<CommittedPresentationTimeV1>,
    ) -> Result<(), RemoteMutationArbiterErrorV1> {
        if self.state.close_requested {
            return Err(RemoteMutationArbiterErrorV1::CloseAlreadyRequested);
        }
        if self.state.active.is_some()
            || self.state.suspended.is_some()
            || self.state.terminal_cleanup.is_some()
        {
            return Err(RemoteMutationArbiterErrorV1::ActiveTurnAlreadyExists);
        }

        self.frame
            .remote_storage_capability_v1(self.facade)
            .commit_presentation_v1(committed_time)
            .map_err(RemoteMutationArbiterErrorV1::PresentationTransition)
    }

    pub(crate) fn request_close_v1(
        &mut self,
    ) -> Result<RemoteMutationRequestIdV1, RemoteMutationArbiterErrorV1> {
        if self.state.close_requested {
            return Err(RemoteMutationArbiterErrorV1::CloseAlreadyRequested);
        }

        let request_id = self.allocate_request_id_v1()?;
        self.state.close_requested = true;
        self.state.close_request_id = Some(request_id);
        Ok(request_id)
    }

    pub(crate) fn drive_close_v1(
        &mut self,
    ) -> Result<RemoteMutationCloseProgressV1, RemoteMutationArbiterErrorV1> {
        if !self.state.close_requested {
            return Err(RemoteMutationArbiterErrorV1::NoCloseRequested);
        }

        if self.state.terminal_cleanup.is_none() {
            if self
                .state
                .active
                .as_ref()
                .is_some_and(|turn| turn.safe_point_v1().is_none())
            {
                return Err(RemoteMutationArbiterErrorV1::RunningMutationNotInterleavable);
            }

            let request_id = self.state.close_request_id.expect("close request id");
            let generation = self.state.generation;
            self.state.active = None;
            self.state.suspended = None;
            self.frame
                .remote_storage_capability_v1(self.facade)
                .terminal_gate_v1();
            self.state.terminal_cleanup = Some(RemoteMutationTerminalCleanupV1 {
                request_id,
                generation,
            });
            return Ok(RemoteMutationCloseProgressV1::TerminalGateStarted);
        }

        let drain_ready = self
            .frame
            .remote_storage_capability_v1(self.facade)
            .take_lease_drain_ready_v1()
            || self.facade.live_lease_count_v1() == 0;
        if !drain_ready {
            return Ok(RemoteMutationCloseProgressV1::WaitingForLeaseDrain);
        }

        self.state.terminal_cleanup = None;
        self.state.close_requested = false;
        self.state.close_request_id = None;
        Ok(RemoteMutationCloseProgressV1::Complete)
    }

    pub(crate) fn suspend_for_page_hidden_v1(
        &mut self,
    ) -> Result<RemoteMutationResumeNonceV1, RemoteMutationArbiterErrorV1> {
        if self.state.close_requested {
            return Err(RemoteMutationArbiterErrorV1::CloseAlreadyRequested);
        }
        if self.state.suspended.is_some() {
            return Err(RemoteMutationArbiterErrorV1::AlreadySuspendedForPageHidden);
        }
        let Some(turn) = self.state.active.as_ref() else {
            return Err(RemoteMutationArbiterErrorV1::NoActiveTurn);
        };
        if turn.safe_point_v1().is_none() {
            return Err(RemoteMutationArbiterErrorV1::NotAtSafePoint);
        }

        let nonce = self.allocate_resume_nonce_v1()?;
        let turn = self.state.active.take().expect("active turn checked above");
        self.state.suspended = Some(RemoteMutationSuspensionV1 {
            turn,
            resume_nonce: Some(nonce),
        });
        Ok(nonce)
    }

    pub(crate) fn resume_from_page_hidden_v1(
        &mut self,
        nonce: RemoteMutationResumeNonceV1,
    ) -> Result<RemoteMutationTurnV1, RemoteMutationArbiterErrorV1> {
        if self.state.close_requested {
            return Err(RemoteMutationArbiterErrorV1::CloseAlreadyRequested);
        }
        let mut turn = match self.state.suspended.as_ref() {
            Some(suspension) if suspension.resume_nonce == Some(nonce) => suspension.turn.clone(),
            Some(_) => return Err(RemoteMutationArbiterErrorV1::StaleResumeNonce),
            None => return Err(RemoteMutationArbiterErrorV1::NotSuspendedForPageHidden),
        };

        let generation = self.advance_generation_v1()?;
        turn.generation = generation;
        self.state.suspended = None;
        self.state.active = Some(turn.clone());
        Ok(turn)
    }

    fn allocate_request_id_v1(
        &mut self,
    ) -> Result<RemoteMutationRequestIdV1, RemoteMutationArbiterErrorV1> {
        let request_id = self.state.next_request_id;
        self.state.next_request_id = self
            .state
            .next_request_id
            .checked_add(1)
            .ok_or(RemoteMutationArbiterErrorV1::RequestIdExhausted)?;
        Ok(RemoteMutationRequestIdV1(request_id))
    }

    fn allocate_resume_nonce_v1(
        &mut self,
    ) -> Result<RemoteMutationResumeNonceV1, RemoteMutationArbiterErrorV1> {
        let nonce = self.state.next_resume_nonce;
        self.state.next_resume_nonce = self
            .state
            .next_resume_nonce
            .checked_add(1)
            .ok_or(RemoteMutationArbiterErrorV1::ResumeNonceExhausted)?;
        Ok(RemoteMutationResumeNonceV1(nonce))
    }

    fn advance_generation_v1(&mut self) -> Result<u64, RemoteMutationArbiterErrorV1> {
        let generation = self
            .state
            .generation
            .checked_add(1)
            .ok_or(RemoteMutationArbiterErrorV1::GenerationExhausted)?;
        self.state.generation = generation;
        Ok(generation)
    }

    fn finish_active_turn_v1(&mut self) {
        if let Some(turn) = self.state.active.take() {
            self.state.last_completed = Some(turn);
        }
    }

    fn stale_or_duplicate_completion_v1(
        &self,
        request_id: RemoteMutationRequestIdV1,
        generation: u64,
    ) -> Result<(), RemoteMutationArbiterErrorV1> {
        if self
            .state
            .last_completed
            .as_ref()
            .is_some_and(|turn| turn.request_id == request_id && turn.generation == generation)
        {
            Err(RemoteMutationArbiterErrorV1::DuplicateCompletion)
        } else {
            Err(RemoteMutationArbiterErrorV1::StaleCompletion)
        }
    }
}

impl ConsumerStorageFreeV1 for RemoteMutationKindV1 {}
impl ConsumerStorageFreeV1 for RemoteMutationRequestIdV1 {}
impl ConsumerStorageFreeV1 for RemoteMutationSafePointV1 {}
impl ConsumerStorageFreeV1 for RemoteMutationSubstateV1 {}
impl ConsumerStorageFreeV1 for RemoteMutationResumeNonceV1 {}
impl ConsumerStorageFreeV1 for RemoteMutationArbiterErrorV1 where
    RemotePresentationTransitionErrorV1: ConsumerStorageFreeV1
{
}
impl ConsumerStorageFreeV1 for RemoteMutationCloseProgressV1 {}
impl ConsumerStorageFreeV1 for RemoteMutationAddChunkOutcomeV1 {}
impl ConsumerStorageFreeV1 for RemoteMutationTurnV1
where
    RemoteMutationKindV1: ConsumerStorageFreeV1,
    RemoteMutationRequestIdV1: ConsumerStorageFreeV1,
    u64: ConsumerStorageFreeV1,
    RemoteMutationSubstateV1: ConsumerStorageFreeV1,
{
}
impl ConsumerStorageFreeV1 for RemoteMutationSuspensionV1
where
    RemoteMutationTurnV1: ConsumerStorageFreeV1,
    Option<RemoteMutationResumeNonceV1>: ConsumerStorageFreeV1,
{
}
impl ConsumerStorageFreeV1 for RemoteMutationTerminalCleanupV1
where
    RemoteMutationRequestIdV1: ConsumerStorageFreeV1,
    u64: ConsumerStorageFreeV1,
{
}
impl ConsumerStorageFreeV1 for RemoteStoreMutationArbiterStateV1
where
    u64: ConsumerStorageFreeV1,
    Option<RemoteMutationTurnV1>: ConsumerStorageFreeV1,
    Option<RemoteMutationSuspensionV1>: ConsumerStorageFreeV1,
    bool: ConsumerStorageFreeV1,
    Option<RemoteMutationRequestIdV1>: ConsumerStorageFreeV1,
    Option<RemoteMutationTerminalCleanupV1>: ConsumerStorageFreeV1,
{
}
impl<'a> ConsumerStorageFreeV1 for RemoteStoreMutationArbiterV1<'a>
where
    PrivilegedViewerFrameContextV1: ConsumerStorageFreeV1,
    &'a RemotePresentationFacadeV1: ConsumerStorageFreeV1,
    RemoteStoreMutationArbiterStateV1: ConsumerStorageFreeV1,
{
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use re_chunk::{TimeInt, TimelineName};
    use re_entity_db::{EntityDb, StoreBundle};
    use re_log_types::{AbsoluteTimeRange, StoreId};
    use re_query::StorageEngine;
    use re_viewer_context::StoreHub;

    use super::*;
    use crate::web_remote_mcap_activation::RemoteRecordingUseStateV1;
    use crate::web_remote_mcap_query::{
        GatedRecordingQueryFacadeV1 as _, RemoteCanonicalIndexedExtentV1, RemoteLoadedCoverageV1,
    };

    fn store_id() -> StoreId {
        StoreId::recording("arbiter-test-app", "arbiter-test-recording")
    }

    fn timeline() -> TimelineName {
        TimelineName::log_time()
    }

    fn extent() -> AbsoluteTimeRange {
        AbsoluteTimeRange::new(0i64, 10i64)
    }

    fn committed_time(cursor: i64) -> CommittedPresentationTimeV1 {
        CommittedPresentationTimeV1 {
            timeline: timeline(),
            cursor: TimeInt::new_temporal(cursor),
        }
    }

    fn open_facade(max_live_leases: usize) -> RemotePresentationFacadeV1 {
        let facade = RemotePresentationFacadeV1::new_for_test_v1(
            91,
            store_id(),
            RemoteRecordingUseStateV1::Foreground,
            RemoteLoadedCoverageV1::new_v1(RemoteCanonicalIndexedExtentV1::Known(extent()), false),
            max_live_leases,
            Rc::new(|| {}),
        );
        let frame = PrivilegedViewerFrameContextV1::new_for_test_v1();
        let privileged = frame.remote_storage_capability_v1(&facade);
        privileged.set_opening_static_satisfied_v1(true);
        privileged
            .commit_initial_presentation_v1(Some(committed_time(3)))
            .expect("open facade");
        facade
    }

    fn make_arbiter(facade: &RemotePresentationFacadeV1) -> RemoteStoreMutationArbiterV1<'_> {
        RemoteStoreMutationArbiterV1::new_disarmed_v1(
            PrivilegedViewerFrameContextV1::new_for_test_v1(),
            facade,
        )
    }

    fn assert_storage_free_v1<T: ConsumerStorageFreeV1>() {}

    macro_rules! assert_not_storage_free_v1 {
        ($type:ty) => {
            const _: fn() = || {
                trait AmbiguousIfImpl<A> {
                    fn some_item() {}
                }

                impl<T: ?Sized> AmbiguousIfImpl<()> for T {}

                struct Invalid;

                impl<T: ?Sized + ConsumerStorageFreeV1> AmbiguousIfImpl<Invalid> for T {}

                let _ = <$type as AmbiguousIfImpl<_>>::some_item;
            };
        };
    }

    #[test]
    fn insertion_and_gc_are_mutually_exclusive() {
        let facade = open_facade(1);
        let mut arbiter = make_arbiter(&facade);

        let insertion = arbiter
            .request_query_visible_insertion_v1()
            .expect("first insertion");
        assert_eq!(
            insertion.kind_v1(),
            RemoteMutationKindV1::QueryVisibleInsertion
        );
        assert_eq!(
            arbiter.request_query_visible_insertion_v1(),
            Err(RemoteMutationArbiterErrorV1::ActiveTurnAlreadyExists)
        );
        assert_eq!(
            arbiter.request_garbage_collection_v1(),
            Err(RemoteMutationArbiterErrorV1::ActiveTurnAlreadyExists)
        );

        let facade2 = open_facade(1);
        let mut arbiter2 = make_arbiter(&facade2);
        let gc = arbiter2
            .request_garbage_collection_v1()
            .expect("first garbage collection");
        assert_eq!(gc.kind_v1(), RemoteMutationKindV1::GarbageCollection);
        assert_eq!(
            arbiter2.request_query_visible_insertion_v1(),
            Err(RemoteMutationArbiterErrorV1::ActiveTurnAlreadyExists)
        );
    }

    #[test]
    fn lease_drain_required_before_insertion_safe_point() {
        let facade = open_facade(1);
        let mut arbiter = make_arbiter(&facade);
        let lease = facade.try_lease_v1().expect("old lease");

        let turn = arbiter
            .request_query_visible_insertion_v1()
            .expect("insertion with live lease");
        assert_eq!(
            turn.safe_point_v1(),
            Some(RemoteMutationSafePointV1::WaitingForLeases)
        );
        assert_eq!(
            arbiter.drive_lease_wait_v1(),
            Err(RemoteMutationArbiterErrorV1::LeaseDrainStillPending)
        );

        drop(lease);
        assert_eq!(
            arbiter.drive_lease_wait_v1(),
            Ok(RemoteMutationSafePointV1::BeforeFirstAddChunk)
        );
    }

    #[test]
    fn lease_drain_required_before_gc_safe_point() {
        let facade = open_facade(1);
        let mut arbiter = make_arbiter(&facade);
        let lease = facade.try_lease_v1().expect("old lease");

        let turn = arbiter
            .request_garbage_collection_v1()
            .expect("gc with live lease");
        assert_eq!(
            turn.safe_point_v1(),
            Some(RemoteMutationSafePointV1::GcWaitingForLeases)
        );
        assert_eq!(
            arbiter.drive_lease_wait_v1(),
            Err(RemoteMutationArbiterErrorV1::LeaseDrainStillPending)
        );

        drop(lease);
        assert_eq!(
            arbiter.drive_lease_wait_v1(),
            Ok(RemoteMutationSafePointV1::AfterGcBeforeReopen)
        );
        arbiter
            .complete_garbage_collection_v1(turn.request_id_v1(), turn.generation_v1(), false)
            .expect("complete gc");
    }

    #[test]
    fn close_preemption_rejects_work_and_reaches_vacant() {
        let facade = open_facade(1);
        let mut arbiter = make_arbiter(&facade);
        let turn = arbiter
            .request_query_visible_insertion_v1()
            .expect("insertion turn");
        let request_id = turn.request_id_v1();
        let generation = turn.generation_v1();

        let close_id = arbiter.request_close_v1().expect("close request");
        assert_eq!(close_id.get_v1(), request_id.get_v1() + 1);
        assert_eq!(
            arbiter.request_query_visible_insertion_v1(),
            Err(RemoteMutationArbiterErrorV1::CloseAlreadyRequested)
        );
        assert_eq!(
            arbiter.request_garbage_collection_v1(),
            Err(RemoteMutationArbiterErrorV1::CloseAlreadyRequested)
        );
        assert_eq!(
            arbiter.commit_presentation_v1(Some(committed_time(9))),
            Err(RemoteMutationArbiterErrorV1::CloseAlreadyRequested)
        );
        assert_eq!(
            arbiter.complete_query_visible_insertion_v1(request_id, generation),
            Err(RemoteMutationArbiterErrorV1::CloseAlreadyRequested)
        );

        assert_eq!(
            arbiter.drive_close_v1(),
            Ok(RemoteMutationCloseProgressV1::TerminalGateStarted)
        );
        assert_eq!(
            arbiter.drive_close_v1(),
            Ok(RemoteMutationCloseProgressV1::Complete)
        );
        assert!(!arbiter.is_close_requested_v1());
        assert!(arbiter.active_turn_v1().is_none());
        assert!(arbiter.suspended_turn_v1().is_none());
        assert!(matches!(
            facade.try_lease_v1(),
            Err(crate::web_remote_mcap_query::PresentationLeaseUnavailableV1::PresentationGated)
        ));
    }

    #[test]
    fn hidden_suspension_uses_one_shot_resume_nonce() {
        let facade = open_facade(1);
        let mut arbiter = make_arbiter(&facade);
        let original = arbiter
            .request_query_visible_insertion_v1()
            .expect("insertion turn");

        let nonce = arbiter
            .suspend_for_page_hidden_v1()
            .expect("suspend at safe point");
        assert!(arbiter.active_turn_v1().is_none());
        assert!(arbiter.suspended_turn_v1().is_some());
        assert_eq!(
            arbiter.complete_query_visible_insertion_v1(
                original.request_id_v1(),
                original.generation_v1(),
            ),
            Err(RemoteMutationArbiterErrorV1::StaleCompletion)
        );

        assert_eq!(
            arbiter.resume_from_page_hidden_v1(RemoteMutationResumeNonceV1(nonce.get_v1() + 1)),
            Err(RemoteMutationArbiterErrorV1::StaleResumeNonce)
        );
        assert!(arbiter.active_turn_v1().is_none());

        let resumed = arbiter
            .resume_from_page_hidden_v1(nonce)
            .expect("one-shot resume");
        assert_eq!(resumed.request_id_v1(), original.request_id_v1());
        assert_eq!(
            resumed.safe_point_v1(),
            Some(RemoteMutationSafePointV1::BeforeFirstAddChunk)
        );
        assert_eq!(
            arbiter.resume_from_page_hidden_v1(nonce),
            Err(RemoteMutationArbiterErrorV1::NotSuspendedForPageHidden)
        );
    }

    #[test]
    fn stale_and_duplicate_completions_do_not_reopen_facade() {
        let facade = open_facade(1);
        let mut arbiter = make_arbiter(&facade);
        let turn = arbiter
            .request_query_visible_insertion_v1()
            .expect("insertion turn");
        let request_id = turn.request_id_v1();
        let generation = turn.generation_v1();

        assert_eq!(
            arbiter.begin_add_chunk_v1(request_id, generation + 1),
            Err(RemoteMutationArbiterErrorV1::StaleCompletion)
        );

        assert_eq!(
            arbiter.begin_add_chunk_v1(request_id, generation),
            Ok(RemoteMutationSubstateV1::RunningAddChunk)
        );
        assert_eq!(
            arbiter.suspend_for_page_hidden_v1(),
            Err(RemoteMutationArbiterErrorV1::NotAtSafePoint)
        );
        assert_eq!(
            arbiter.complete_add_chunk_v1(
                request_id,
                generation,
                RemoteMutationAddChunkOutcomeV1::LastChunk,
            ),
            Ok(RemoteMutationSubstateV1::SafePoint(
                RemoteMutationSafePointV1::AfterLastAddChunkBeforeAck,
            ))
        );
        arbiter
            .acknowledge_partitions_resident_v1(request_id, generation)
            .expect("ack");
        arbiter
            .complete_query_visible_insertion_v1(request_id, generation)
            .expect("complete insertion");
        assert!(arbiter.active_turn_v1().is_none());

        assert_eq!(
            arbiter.complete_query_visible_insertion_v1(request_id, generation),
            Err(RemoteMutationArbiterErrorV1::DuplicateCompletion)
        );
        assert!(facade.try_lease_v1().is_ok());
    }

    #[test]
    fn remote_mutation_arbiter_graph_is_storage_free_by_structure() {
        assert_not_storage_free_v1!(EntityDb);
        assert_not_storage_free_v1!(StoreBundle);
        assert_not_storage_free_v1!(StoreHub);
        assert_not_storage_free_v1!(StorageEngine);

        assert_storage_free_v1::<RemoteStoreMutationArbiterV1<'static>>();
        assert_storage_free_v1::<RemoteStoreMutationArbiterStateV1>();
        assert_storage_free_v1::<RemoteMutationTurnV1>();
        assert_storage_free_v1::<RemoteMutationSubstateV1>();
    }
}
