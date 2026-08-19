//! Production-disarmed remote-MCAP Viewer frame slice.
//!
//! This module owns the bounded ordering and page-state boundary required by MCAP-104. It does not
//! import or construct a `ViewerContext`, `AppContext`, `StoreHub`, `StoreBundle`, `EntityDb`,
//! storage engine, network transport, real query lease, or real mutation transport. Work is
//! represented as storage-free queue items and the frame driver only advances one item per bounded
//! allowance slot.
//!
//! CPU kind, mutation kind, and mutation safe-point enums are reused from
//! `web_remote_mcap_cpu` and `web_remote_mcap_mutation_arbiter`. The frame driver's `RemoteFrame*`
//! queue items are ordering-only wrappers because those modules expose lifetime- and
//! capability-sealed work values that cannot cross this storage-free frame boundary.

#![allow(dead_code)]

use std::collections::VecDeque;

use crate::web_remote_mcap_activation::RemoteRecordingUseStateV1;
use crate::web_remote_mcap_cpu::RemoteCpuWorkKindV1;
use crate::web_remote_mcap_mutation_arbiter::{
    RemoteMutationKindV1 as RemoteMutationWorkKindV1,
    RemoteMutationSafePointV1 as RemoteMutationBoundaryV1,
};
use crate::web_remote_mcap_query::ConsumerStorageFreeV1;

const REMOTE_FRAME_PHASE_COUNT: usize = 10;

/// The fixed remote-MCAP frame ordering described by design section 16.3.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum RemoteFramePhaseV1 {
    SnapshotPageAndOwner,
    HiddenCloseAndCallbackConvergence,
    RevalidateVisibleOwner,
    MergeVisibleCommandAndUseState,
    ConsumeFetchCompletion,
    RunSingleRemoteCpuWork,
    AdvanceSingleMutationSafePoint,
    CommitPresentationRevision,
    AcquireQueryLease,
    FinishRemoteMetricsAndRepaint,
}

/// Structural evidence that the slice is fixed and nonrecursive.
pub(crate) struct RemoteFrameOrderingV1;

impl RemoteFrameOrderingV1 {
    pub(crate) const fn phases_v1() -> [RemoteFramePhaseV1; REMOTE_FRAME_PHASE_COUNT] {
        [
            RemoteFramePhaseV1::SnapshotPageAndOwner,
            RemoteFramePhaseV1::HiddenCloseAndCallbackConvergence,
            RemoteFramePhaseV1::RevalidateVisibleOwner,
            RemoteFramePhaseV1::MergeVisibleCommandAndUseState,
            RemoteFramePhaseV1::ConsumeFetchCompletion,
            RemoteFramePhaseV1::RunSingleRemoteCpuWork,
            RemoteFramePhaseV1::AdvanceSingleMutationSafePoint,
            RemoteFramePhaseV1::CommitPresentationRevision,
            RemoteFramePhaseV1::AcquireQueryLease,
            RemoteFramePhaseV1::FinishRemoteMetricsAndRepaint,
        ]
    }

    pub(crate) fn preserves_ordering_v1(observed: &[RemoteFramePhaseV1]) -> bool {
        let phases = Self::phases_v1();
        let mut next_expected = 0;
        for phase in observed {
            while next_expected < phases.len() && phases[next_expected] != *phase {
                next_expected += 1;
            }
            if next_expected == phases.len() {
                return false;
            }
            next_expected += 1;
        }
        true
    }
}

/// One page execution epoch. Zero is intentionally not a valid epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct BrowserExecutionEpochV1(u64);

impl BrowserExecutionEpochV1 {
    pub(crate) const fn new_v1(value: u64) -> Option<Self> {
        if value == 0 { None } else { Some(Self(value)) }
    }

    #[cfg(test)]
    pub(crate) const fn new_for_test_v1(value: u64) -> Self {
        Self(value)
    }

    pub(crate) const fn get_v1(self) -> u64 {
        self.0
    }

    fn next_v1(self) -> Result<Self, RemoteFrameDriverErrorV1> {
        self.0
            .checked_add(1)
            .and_then(Self::new_v1)
            .ok_or(RemoteFrameDriverErrorV1::EpochExhausted)
    }
}

/// A fresh baseline bound to one visible execution epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BrowserClockBaselineV1 {
    epoch: BrowserExecutionEpochV1,
    sequence: u64,
}

impl BrowserClockBaselineV1 {
    fn initial_v1(epoch: BrowserExecutionEpochV1) -> Self {
        Self { epoch, sequence: 1 }
    }

    fn allocate_after_v1(self) -> Result<Self, RemoteFrameDriverErrorV1> {
        self.sequence
            .checked_add(1)
            .map(|sequence| Self {
                epoch: self.epoch,
                sequence,
            })
            .ok_or(RemoteFrameDriverErrorV1::ClockBaselineExhausted)
    }

    pub(crate) const fn epoch_v1(self) -> BrowserExecutionEpochV1 {
        self.epoch
    }

    pub(crate) const fn sequence_v1(self) -> u64 {
        self.sequence
    }
}

/// One-shot resume nonce. Zero is not a valid nonce.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct RemoteResumeNonceV1(u64);

impl RemoteResumeNonceV1 {
    pub(crate) const fn get_v1(self) -> u64 {
        self.0
    }
}

/// Page-lifecycle reason carried by terminal teardown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemotePageTerminationReasonV1 {
    PageHide,
    Freeze,
    ExplicitClose,
}

/// Exact remote page execution state from design section 8.5.1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChromePageExecutionStateV1 {
    VisibleRunning {
        epoch: BrowserExecutionEpochV1,
        clock_baseline: BrowserClockBaselineV1,
    },
    HiddenSuspended {
        epoch: BrowserExecutionEpochV1,
        remote_wake_pending: bool,
    },
    VisibleRevalidating {
        from_epoch: BrowserExecutionEpochV1,
        to_epoch: BrowserExecutionEpochV1,
        resume_nonce: RemoteResumeNonceV1,
    },
    RemoteTerminating {
        epoch: BrowserExecutionEpochV1,
        reason: RemotePageTerminationReasonV1,
    },
}

/// Compatibility alias for callers that name the concept as remote page execution state.
pub(crate) type RemotePageExecutionStateV1 = ChromePageExecutionStateV1;

impl ChromePageExecutionStateV1 {
    pub(crate) const fn epoch_v1(self) -> BrowserExecutionEpochV1 {
        match self {
            Self::VisibleRunning { epoch, .. }
            | Self::HiddenSuspended { epoch, .. }
            | Self::VisibleRevalidating {
                to_epoch: epoch, ..
            }
            | Self::RemoteTerminating { epoch, .. } => epoch,
        }
    }

    pub(crate) const fn is_visible_running_v1(self) -> bool {
        matches!(self, Self::VisibleRunning { .. })
    }
}

/// Ingress classes that are deliberately never routed into the remote frame slice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteIngressSourceKindV1 {
    RemoteMcap,
    OrdinaryReceiver,
    LogChannel,
    GrpcMessageProxy,
    Redap,
    System,
    Ui,
    NonremoteQuery,
    Native,
    Local,
    Legacy,
}

impl RemoteIngressSourceKindV1 {
    pub(crate) const fn enters_remote_frame_slice_v1(self) -> bool {
        matches!(self, Self::RemoteMcap)
    }
}

/// Immutable frame snapshot consumed by the bounded slice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteFrameSnapshotV1 {
    pub(crate) ingress: RemoteIngressSourceKindV1,
    pub(crate) page_state: ChromePageExecutionStateV1,
    pub(crate) use_state: RemoteRecordingUseStateV1,
    pub(crate) exact_handle_command: Option<RemoteExactHandleCommandV1>,
    pub(crate) navigation_frozen: bool,
    pub(crate) requested_generation_in_flight: bool,
    pub(crate) buffering: bool,
    pub(crate) close_request: Option<RemoteCloseRequestV1>,
    pub(crate) store_event: Option<RemoteStoreEventV1>,
    pub(crate) resume_nonce: Option<RemoteResumeNonceV1>,
    pub(crate) resume_proof: Option<RemoteResumeProofV1>,
}

impl RemoteFrameSnapshotV1 {
    pub(crate) const fn candidate_clock_held_v1(&self) -> bool {
        self.navigation_frozen || self.requested_generation_in_flight || self.buffering
    }
}

/// Exact-handle control command accepted only in a visible running turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteExactHandleCommandV1 {
    SetUseState(RemoteRecordingUseStateV1),
    DiscreteNavigation {
        command_generation: u64,
        canonical_target_delta: i64,
    },
    Close {
        source_generation: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemotePendingIntentV1 {
    command_generation: u64,
    canonical_target_delta: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteCloseRequestV1 {
    pub(crate) source_generation: u64,
    pub(crate) reason: RemotePageTerminationReasonV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteStoreEventV1 {
    MetadataStatus,
    TemporalDataArrived,
}

impl RemoteStoreEventV1 {
    const fn is_control_plane_only_v1(self) -> bool {
        matches!(self, Self::MetadataStatus)
    }
}

/// Validation facts checked before any page-hidden owner can be rebound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteResumeProofV1 {
    pub(crate) slot_token: u64,
    pub(crate) source_token: u64,
    pub(crate) store_generation: u64,
    pub(crate) validator_generation: u64,
    pub(crate) reservation_generation: u64,
    pub(crate) completion_slot_generation: u64,
    pub(crate) pending_lease_generation: u64,
    pub(crate) full_range: RemoteByteRangeV1,
    pub(crate) suspended_turn_generation: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteByteRangeV1 {
    start: u64,
    end_exclusive: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemotePhysicalChunkCompletionV1 {
    pub(crate) source_generation: u64,
    pub(crate) read_generation: u64,
    pub(crate) attempt_generation: u64,
    pub(crate) canonical_ordinal: u32,
    pub(crate) full_range: RemoteByteRangeV1,
    pub(crate) lease_generation: u64,
    pub(crate) body_owner_generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteFrameRetryPendingV1 {
    pub(crate) source_generation: u64,
    pub(crate) operation_generation: u64,
    pub(crate) attempt_generation: u64,
    pub(crate) canonical_ordinal: u32,
    pub(crate) full_range: RemoteByteRangeV1,
    pub(crate) deadline_remaining_micros: u64,
}

/// Frame-ordering wrapper for a fetch/browser callback payload.
///
/// The underlying CPU and retry identities live in `web_remote_mcap_cpu`; this wrapper only
/// carries the storage-free payload through the frame-slice ordering boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteFrameFetchCompletionV1 {
    PhysicalChunkReady(RemotePhysicalChunkCompletionV1),
    RetryPending(RemoteFrameRetryPendingV1),
    MetadataStatus,
    CloseAck,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteFrameCpuWorkV1 {
    pub(crate) kind: RemoteCpuWorkKindV1,
    pub(crate) source_generation: u64,
    pub(crate) canonical_ordinal: u32,
    pub(crate) phase_generation: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteFrameMutationWorkV1 {
    pub(crate) kind: RemoteMutationWorkKindV1,
    pub(crate) generation: u64,
    pub(crate) boundary: RemoteMutationBoundaryV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemotePresentationCommitV1 {
    pub(crate) source_generation: u64,
    pub(crate) cursor_delta: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteQueryLeaseRequestV1 {
    pub(crate) source_generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteSuspendedOwnerV1 {
    Fetch(RemotePhysicalChunkCompletionV1),
    Mutation(RemoteFrameMutationWorkV1),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RemoteCallbackOwnerV1 {
    generation: u64,
}

/// Per-frame bounded remote work allowance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RemoteFrameAllowanceV1 {
    pub(crate) fetch_completions: usize,
    pub(crate) retry_attempts: usize,
    pub(crate) cpu_work_units: usize,
    pub(crate) mutation_safe_points: usize,
    pub(crate) presentation_commits: usize,
    pub(crate) query_leases: usize,
    pub(crate) callback_ownership_convergence: usize,
}

impl RemoteFrameAllowanceV1 {
    pub(crate) const fn one_of_each_v1() -> Self {
        Self {
            fetch_completions: 1,
            retry_attempts: 1,
            cpu_work_units: 1,
            mutation_safe_points: 1,
            presentation_commits: 1,
            query_leases: 1,
            callback_ownership_convergence: 1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteFrameResumeRejectionV1 {
    StaleNonce,
    ProofMismatch,
    NotForeground,
    ExplicitClose,
    NoResumeProof,
    ClockBaselineExhausted,
    InternalStateMismatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteFrameEventV1 {
    CloseRequested,
    CallbackOwnershipConverged,
    PageResumed {
        rebound_owner: bool,
        clock_baseline: BrowserClockBaselineV1,
    },
    ResumeRejected(RemoteFrameResumeRejectionV1),
    UseStateChanged {
        from: RemoteRecordingUseStateV1,
        to: RemoteRecordingUseStateV1,
    },
    CandidateClockHoldUpdated {
        held: bool,
    },
    PendingIntentUpdated,
    RetryAttemptReady,
    FetchRetryPendingQueued,
    FetchCompletionReady {
        canonical_ordinal: u32,
    },
    FetchCompletionRejected {
        canonical_ordinal: u32,
    },
    MetadataStatusProcessed,
    CpuWorkConsumed {
        kind: RemoteCpuWorkKindV1,
        canonical_ordinal: u32,
    },
    MutationSafePointAdvanced {
        kind: RemoteMutationWorkKindV1,
        boundary: RemoteMutationBoundaryV1,
    },
    PresentationCommitApplied,
    QueryLeaseAcquired,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RemoteFrameReportV1 {
    phases: Vec<RemoteFramePhaseV1>,
    events: Vec<RemoteFrameEventV1>,
    request_repaint: bool,
    remote_work_remains: bool,
    cursor_delta: bool,
}

impl RemoteFrameReportV1 {
    pub(crate) fn phases_v1(&self) -> &[RemoteFramePhaseV1] {
        &self.phases
    }

    pub(crate) fn events_v1(&self) -> &[RemoteFrameEventV1] {
        &self.events
    }

    pub(crate) const fn request_repaint_v1(&self) -> bool {
        self.request_repaint
    }

    pub(crate) const fn remote_work_remains_v1(&self) -> bool {
        self.remote_work_remains
    }

    pub(crate) const fn cursor_delta_v1(&self) -> bool {
        self.cursor_delta
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RemoteFrameOutcomeV1 {
    Driven(RemoteFrameReportV1),
    NotRemoteMcap { ingress: RemoteIngressSourceKindV1 },
    StalePageSnapshot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteBrowserCallbackOutcomeV1 {
    pub(crate) completion_enqueued: bool,
    pub(crate) request_repaint: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RemoteFrameHighWaterV1 {
    frame_sequence: u64,
    retained_fetch_completions: usize,
    retained_cpu_work: usize,
    retained_mutation_work: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteFrameDriverErrorV1 {
    EpochExhausted,
    FrameSequenceExhausted,
    ClockBaselineExhausted,
    ResumeNonceExhausted,
    NotVisibleRunning,
    NotHiddenSuspended,
    NotVisibleRevalidating,
    AlreadyHidden,
    AlreadyTerminating,
    StaleResumeNonce,
    ResumeProofMismatch,
}

struct RemoteMcapFrameDriverStateV1 {
    page_state: ChromePageExecutionStateV1,
    use_state: RemoteRecordingUseStateV1,
    owner_proof: RemoteResumeProofV1,
    next_resume_nonce: u64,
    clock_baseline: BrowserClockBaselineV1,
    candidate_clock_held: bool,
    latest_pending_intent: Option<RemotePendingIntentV1>,
    retry_admission_eligible: bool,
    retry_admission_frame_sequence: Option<u64>,
    suspended_owner: Option<RemoteSuspendedOwnerV1>,
    resumed_owner: Option<RemoteSuspendedOwnerV1>,
    close_requested: Option<RemoteCloseRequestV1>,
    fetch_completions: VecDeque<RemoteFrameFetchCompletionV1>,
    retry_pending: VecDeque<RemoteFrameRetryPendingV1>,
    cpu_work: VecDeque<RemoteFrameCpuWorkV1>,
    mutation_work: VecDeque<RemoteFrameMutationWorkV1>,
    presentation_commits: VecDeque<RemotePresentationCommitV1>,
    query_lease_requests: VecDeque<RemoteQueryLeaseRequestV1>,
    callback_owners: VecDeque<RemoteCallbackOwnerV1>,
    high_water: RemoteFrameHighWaterV1,
}

/// Storage-free, production-disarmed remote-MCAP frame-slice controller.
pub(crate) struct RemoteMcapFrameDriverV1 {
    state: RemoteMcapFrameDriverStateV1,
}

impl RemoteMcapFrameDriverV1 {
    pub(crate) fn new_disarmed_v1(
        epoch: BrowserExecutionEpochV1,
        use_state: RemoteRecordingUseStateV1,
        owner_proof: RemoteResumeProofV1,
    ) -> Self {
        let clock_baseline = BrowserClockBaselineV1::initial_v1(epoch);
        Self {
            state: RemoteMcapFrameDriverStateV1 {
                page_state: ChromePageExecutionStateV1::VisibleRunning {
                    epoch,
                    clock_baseline,
                },
                use_state,
                owner_proof,
                next_resume_nonce: 1,
                clock_baseline,
                candidate_clock_held: false,
                latest_pending_intent: None,
                retry_admission_eligible: true,
                retry_admission_frame_sequence: None,
                suspended_owner: None,
                resumed_owner: None,
                close_requested: None,
                fetch_completions: VecDeque::new(),
                retry_pending: VecDeque::new(),
                cpu_work: VecDeque::new(),
                mutation_work: VecDeque::new(),
                presentation_commits: VecDeque::new(),
                query_lease_requests: VecDeque::new(),
                callback_owners: VecDeque::new(),
                high_water: RemoteFrameHighWaterV1::default(),
            },
        }
    }

    pub(crate) const fn page_state_v1(&self) -> ChromePageExecutionStateV1 {
        self.state.page_state
    }

    pub(crate) fn snapshot_v1(&self) -> RemoteFrameSnapshotV1 {
        let resume_nonce = match self.state.page_state {
            ChromePageExecutionStateV1::VisibleRevalidating { resume_nonce, .. } => {
                Some(resume_nonce)
            }
            _ => None,
        };
        RemoteFrameSnapshotV1 {
            ingress: RemoteIngressSourceKindV1::RemoteMcap,
            page_state: self.state.page_state,
            use_state: self.state.use_state,
            exact_handle_command: None,
            navigation_frozen: false,
            requested_generation_in_flight: false,
            buffering: false,
            close_request: self.state.close_requested,
            store_event: None,
            resume_nonce,
            resume_proof: Some(self.state.owner_proof),
        }
    }

    pub(crate) fn transition_to_hidden_v1(
        &mut self,
    ) -> Result<ChromePageExecutionStateV1, RemoteFrameDriverErrorV1> {
        let next_epoch = match self.state.page_state {
            ChromePageExecutionStateV1::VisibleRunning { epoch, .. }
            | ChromePageExecutionStateV1::VisibleRevalidating {
                from_epoch: epoch, ..
            } => epoch.next_v1()?,
            ChromePageExecutionStateV1::HiddenSuspended { .. } => {
                return Err(RemoteFrameDriverErrorV1::AlreadyHidden);
            }
            ChromePageExecutionStateV1::RemoteTerminating { .. } => {
                return Err(RemoteFrameDriverErrorV1::AlreadyTerminating);
            }
        };
        if self.state.suspended_owner.is_none() {
            self.state.suspended_owner = self.state.resumed_owner.take().or_else(|| {
                self.state
                    .mutation_work
                    .pop_front()
                    .map(RemoteSuspendedOwnerV1::Mutation)
            });
        }
        self.state.resumed_owner = None;
        self.state.page_state = ChromePageExecutionStateV1::HiddenSuspended {
            epoch: next_epoch,
            remote_wake_pending: false,
        };
        Ok(self.state.page_state)
    }

    pub(crate) fn begin_resume_v1(
        &mut self,
    ) -> Result<RemoteResumeNonceV1, RemoteFrameDriverErrorV1> {
        let ChromePageExecutionStateV1::HiddenSuspended {
            epoch: from_epoch, ..
        } = self.state.page_state
        else {
            return Err(RemoteFrameDriverErrorV1::NotHiddenSuspended);
        };
        let to_epoch = from_epoch.next_v1()?;
        let value = self.state.next_resume_nonce;
        self.state.next_resume_nonce = self
            .state
            .next_resume_nonce
            .checked_add(1)
            .ok_or(RemoteFrameDriverErrorV1::ResumeNonceExhausted)?;
        let resume_nonce = RemoteResumeNonceV1(value);
        self.state.page_state = ChromePageExecutionStateV1::VisibleRevalidating {
            from_epoch,
            to_epoch,
            resume_nonce,
        };
        Ok(resume_nonce)
    }

    pub(crate) fn resume_v1(
        &mut self,
        nonce: RemoteResumeNonceV1,
        proof: RemoteResumeProofV1,
    ) -> Result<RemoteResumeResultV1, RemoteFrameDriverErrorV1> {
        let ChromePageExecutionStateV1::VisibleRevalidating {
            resume_nonce: expected_nonce,
            to_epoch: epoch,
            ..
        } = self.state.page_state
        else {
            return Err(RemoteFrameDriverErrorV1::NotVisibleRevalidating);
        };
        if nonce != expected_nonce {
            return Err(RemoteFrameDriverErrorV1::StaleResumeNonce);
        }
        if proof != self.state.owner_proof {
            return Err(RemoteFrameDriverErrorV1::ResumeProofMismatch);
        }
        if self.state.use_state != RemoteRecordingUseStateV1::Foreground {
            return Err(RemoteFrameDriverErrorV1::NotVisibleRunning);
        }
        if self.state.close_requested.is_some() {
            return Err(RemoteFrameDriverErrorV1::NotVisibleRunning);
        }

        let rebound_owner = self.state.suspended_owner.take();
        self.state.resumed_owner = rebound_owner;
        let clock_baseline = self.state.clock_baseline.allocate_after_v1()?;
        self.state.clock_baseline = clock_baseline;
        self.state.retry_admission_eligible = true;
        self.state.retry_admission_frame_sequence = None;
        self.state.page_state = ChromePageExecutionStateV1::VisibleRunning {
            epoch,
            clock_baseline,
        };
        Ok(RemoteResumeResultV1 {
            rebound_owner: rebound_owner.is_some(),
            clock_baseline,
        })
    }

    pub(crate) fn transition_to_terminating_v1(
        &mut self,
        reason: RemotePageTerminationReasonV1,
    ) -> Result<ChromePageExecutionStateV1, RemoteFrameDriverErrorV1> {
        match self.state.page_state {
            ChromePageExecutionStateV1::RemoteTerminating { .. } => {
                Err(RemoteFrameDriverErrorV1::AlreadyTerminating)
            }
            ChromePageExecutionStateV1::VisibleRunning { epoch, .. }
            | ChromePageExecutionStateV1::HiddenSuspended { epoch, .. }
            | ChromePageExecutionStateV1::VisibleRevalidating {
                from_epoch: epoch, ..
            } => {
                self.state.page_state =
                    ChromePageExecutionStateV1::RemoteTerminating { epoch, reason };
                self.clear_remote_work_v1();
                Ok(self.state.page_state)
            }
        }
    }

    fn clear_remote_work_v1(&mut self) {
        self.state.fetch_completions.clear();
        self.state.retry_pending.clear();
        self.state.cpu_work.clear();
        self.state.mutation_work.clear();
        self.state.presentation_commits.clear();
        self.state.query_lease_requests.clear();
        self.state.callback_owners.clear();
        self.state.suspended_owner = None;
        self.state.resumed_owner = None;
    }

    pub(crate) fn enqueue_fetch_completion_v1(
        &mut self,
        completion: RemoteFrameFetchCompletionV1,
    ) -> RemoteBrowserCallbackOutcomeV1 {
        self.state.fetch_completions.push_back(completion);
        RemoteBrowserCallbackOutcomeV1 {
            completion_enqueued: true,
            request_repaint: true,
        }
    }

    pub(crate) fn enqueue_retry_pending_v1(
        &mut self,
        retry: RemoteFrameRetryPendingV1,
    ) -> RemoteBrowserCallbackOutcomeV1 {
        self.state.retry_pending.push_back(retry);
        RemoteBrowserCallbackOutcomeV1 {
            completion_enqueued: true,
            request_repaint: true,
        }
    }

    pub(crate) fn enqueue_cpu_work_v1(&mut self, work: RemoteFrameCpuWorkV1) {
        self.state.cpu_work.push_back(work);
    }

    pub(crate) fn enqueue_mutation_work_v1(&mut self, work: RemoteFrameMutationWorkV1) {
        self.state.mutation_work.push_back(work);
    }

    pub(crate) fn enqueue_presentation_commit_v1(&mut self, commit: RemotePresentationCommitV1) {
        self.state.presentation_commits.push_back(commit);
    }

    pub(crate) fn enqueue_query_lease_request_v1(&mut self, request: RemoteQueryLeaseRequestV1) {
        self.state.query_lease_requests.push_back(request);
    }

    pub(crate) fn enqueue_callback_owner_v1(&mut self, generation: u64) {
        self.state
            .callback_owners
            .push_back(RemoteCallbackOwnerV1 { generation });
    }

    pub(crate) fn drive_frame_v1(
        &mut self,
        snapshot: RemoteFrameSnapshotV1,
        allowance: RemoteFrameAllowanceV1,
    ) -> Result<RemoteFrameOutcomeV1, RemoteFrameDriverErrorV1> {
        if !snapshot.ingress.enters_remote_frame_slice_v1() {
            return Ok(RemoteFrameOutcomeV1::NotRemoteMcap {
                ingress: snapshot.ingress,
            });
        }
        if snapshot.page_state != self.state.page_state {
            return Ok(RemoteFrameOutcomeV1::StalePageSnapshot);
        }

        let mut report = RemoteFrameReportV1::default();
        report.phases.push(RemoteFramePhaseV1::SnapshotPageAndOwner);
        let frame_sequence = self
            .state
            .high_water
            .frame_sequence
            .checked_add(1)
            .ok_or(RemoteFrameDriverErrorV1::FrameSequenceExhausted)?;
        self.state.high_water.frame_sequence = frame_sequence;

        if let Some(close_request) = snapshot.close_request {
            self.state.close_requested = Some(close_request);
            report.events.push(RemoteFrameEventV1::CloseRequested);
        }

        match snapshot.page_state {
            ChromePageExecutionStateV1::RemoteTerminating { .. } => {
                self.drive_terminating_v1(&snapshot, allowance, &mut report);
            }
            ChromePageExecutionStateV1::HiddenSuspended { .. } => {
                self.drive_hidden_v1(&snapshot, allowance, &mut report);
            }
            ChromePageExecutionStateV1::VisibleRevalidating { .. } => {
                self.drive_revalidating_v1(&snapshot, allowance, &mut report);
            }
            ChromePageExecutionStateV1::VisibleRunning { .. } => {
                self.drive_visible_running_v1(&snapshot, allowance, frame_sequence, &mut report);
            }
        }

        report.remote_work_remains = self.remote_work_remains_v1();
        report.request_repaint = report.remote_work_remains || !report.events.is_empty();
        self.record_high_water_v1(&mut report);
        Ok(RemoteFrameOutcomeV1::Driven(report))
    }

    fn drive_terminating_v1(
        &mut self,
        _snapshot: &RemoteFrameSnapshotV1,
        allowance: RemoteFrameAllowanceV1,
        report: &mut RemoteFrameReportV1,
    ) {
        report
            .phases
            .push(RemoteFramePhaseV1::HiddenCloseAndCallbackConvergence);
        self.converge_bounded_callback_ownership_v1(allowance, report);
        report
            .phases
            .push(RemoteFramePhaseV1::FinishRemoteMetricsAndRepaint);
    }

    fn drive_hidden_v1(
        &mut self,
        _snapshot: &RemoteFrameSnapshotV1,
        allowance: RemoteFrameAllowanceV1,
        report: &mut RemoteFrameReportV1,
    ) {
        report
            .phases
            .push(RemoteFramePhaseV1::HiddenCloseAndCallbackConvergence);
        self.converge_bounded_callback_ownership_v1(allowance, report);
        report
            .phases
            .push(RemoteFramePhaseV1::FinishRemoteMetricsAndRepaint);
    }

    fn drive_revalidating_v1(
        &mut self,
        snapshot: &RemoteFrameSnapshotV1,
        _allowance: RemoteFrameAllowanceV1,
        report: &mut RemoteFrameReportV1,
    ) {
        report
            .phases
            .push(RemoteFramePhaseV1::RevalidateVisibleOwner);
        if snapshot.use_state != RemoteRecordingUseStateV1::Foreground {
            report.events.push(RemoteFrameEventV1::ResumeRejected(
                RemoteFrameResumeRejectionV1::NotForeground,
            ));
        } else if snapshot.close_request.is_some() || self.state.close_requested.is_some() {
            report.events.push(RemoteFrameEventV1::ResumeRejected(
                RemoteFrameResumeRejectionV1::ExplicitClose,
            ));
        } else {
            let (Some(nonce), Some(proof)) = (snapshot.resume_nonce, snapshot.resume_proof) else {
                report.events.push(RemoteFrameEventV1::ResumeRejected(
                    RemoteFrameResumeRejectionV1::NoResumeProof,
                ));
                report
                    .phases
                    .push(RemoteFramePhaseV1::FinishRemoteMetricsAndRepaint);
                return;
            };

            match self.resume_v1(nonce, proof) {
                Ok(result) => {
                    report.events.push(RemoteFrameEventV1::PageResumed {
                        rebound_owner: result.rebound_owner,
                        clock_baseline: result.clock_baseline,
                    });
                }
                Err(RemoteFrameDriverErrorV1::StaleResumeNonce) => {
                    report.events.push(RemoteFrameEventV1::ResumeRejected(
                        RemoteFrameResumeRejectionV1::StaleNonce,
                    ));
                }
                Err(RemoteFrameDriverErrorV1::ResumeProofMismatch) => {
                    report.events.push(RemoteFrameEventV1::ResumeRejected(
                        RemoteFrameResumeRejectionV1::ProofMismatch,
                    ));
                }
                Err(RemoteFrameDriverErrorV1::ClockBaselineExhausted) => {
                    report.events.push(RemoteFrameEventV1::ResumeRejected(
                        RemoteFrameResumeRejectionV1::ClockBaselineExhausted,
                    ));
                }
                Err(_) => {
                    report.events.push(RemoteFrameEventV1::ResumeRejected(
                        RemoteFrameResumeRejectionV1::InternalStateMismatch,
                    ));
                }
            }
        }
        report
            .phases
            .push(RemoteFramePhaseV1::FinishRemoteMetricsAndRepaint);
    }

    fn drive_visible_running_v1(
        &mut self,
        snapshot: &RemoteFrameSnapshotV1,
        allowance: RemoteFrameAllowanceV1,
        frame_sequence: u64,
        report: &mut RemoteFrameReportV1,
    ) {
        report
            .phases
            .push(RemoteFramePhaseV1::MergeVisibleCommandAndUseState);
        self.merge_exact_handle_and_use_state_v1(snapshot, report);

        if let Some(close_request) = self.state.close_requested {
            let Ok(_) = self.transition_to_terminating_v1(close_request.reason) else {
                return;
            };
            report
                .phases
                .push(RemoteFramePhaseV1::HiddenCloseAndCallbackConvergence);
            self.converge_bounded_callback_ownership_v1(allowance, report);
            report
                .phases
                .push(RemoteFramePhaseV1::FinishRemoteMetricsAndRepaint);
            return;
        }

        if self.state.use_state != RemoteRecordingUseStateV1::Foreground {
            Self::process_control_plane_only_v1(snapshot.store_event, report);
            report
                .phases
                .push(RemoteFramePhaseV1::FinishRemoteMetricsAndRepaint);
            return;
        }

        if let Some(owner) = self.state.resumed_owner.take() {
            self.recover_resumed_owner_v1(owner, report);
            report
                .phases
                .push(RemoteFramePhaseV1::FinishRemoteMetricsAndRepaint);
            return;
        }

        report
            .phases
            .push(RemoteFramePhaseV1::ConsumeFetchCompletion);
        self.state.retry_admission_eligible = true;
        self.state.retry_admission_frame_sequence = None;
        self.consume_retry_attempt_v1(allowance, frame_sequence, report);
        self.consume_fetch_completion_v1(allowance, report);

        report
            .phases
            .push(RemoteFramePhaseV1::RunSingleRemoteCpuWork);
        self.consume_single_cpu_work_v1(allowance, report);

        report
            .phases
            .push(RemoteFramePhaseV1::AdvanceSingleMutationSafePoint);
        self.consume_single_mutation_safe_point_v1(allowance, report);

        report
            .phases
            .push(RemoteFramePhaseV1::CommitPresentationRevision);
        self.consume_presentation_commit_v1(allowance, report);

        report.phases.push(RemoteFramePhaseV1::AcquireQueryLease);
        self.consume_query_lease_request_v1(allowance, report);

        report
            .phases
            .push(RemoteFramePhaseV1::FinishRemoteMetricsAndRepaint);
    }

    fn recover_resumed_owner_v1(
        &self,
        owner: RemoteSuspendedOwnerV1,
        report: &mut RemoteFrameReportV1,
    ) {
        match owner {
            RemoteSuspendedOwnerV1::Mutation(work) => {
                report
                    .phases
                    .push(RemoteFramePhaseV1::AdvanceSingleMutationSafePoint);
                report
                    .events
                    .push(RemoteFrameEventV1::MutationSafePointAdvanced {
                        kind: work.kind,
                        boundary: work.boundary,
                    });
            }
            RemoteSuspendedOwnerV1::Fetch(physical) => {
                report
                    .phases
                    .push(RemoteFramePhaseV1::ConsumeFetchCompletion);
                if physical_matches_proof_v1(physical, self.state.owner_proof) {
                    report
                        .events
                        .push(RemoteFrameEventV1::FetchCompletionReady {
                            canonical_ordinal: physical.canonical_ordinal,
                        });
                } else {
                    report
                        .events
                        .push(RemoteFrameEventV1::FetchCompletionRejected {
                            canonical_ordinal: physical.canonical_ordinal,
                        });
                }
            }
        }
    }

    fn merge_exact_handle_and_use_state_v1(
        &mut self,
        snapshot: &RemoteFrameSnapshotV1,
        report: &mut RemoteFrameReportV1,
    ) {
        if let Some(command) = snapshot.exact_handle_command {
            match command {
                RemoteExactHandleCommandV1::SetUseState(to) => {
                    let from = self.state.use_state;
                    if from != to {
                        self.state.use_state = to;
                        report
                            .events
                            .push(RemoteFrameEventV1::UseStateChanged { from, to });
                    }
                }
                RemoteExactHandleCommandV1::DiscreteNavigation {
                    command_generation,
                    canonical_target_delta,
                } => {
                    self.state.latest_pending_intent = Some(RemotePendingIntentV1 {
                        command_generation,
                        canonical_target_delta,
                    });
                    report.events.push(RemoteFrameEventV1::PendingIntentUpdated);
                }
                RemoteExactHandleCommandV1::Close { source_generation } => {
                    self.state.close_requested = Some(RemoteCloseRequestV1 {
                        source_generation,
                        reason: RemotePageTerminationReasonV1::ExplicitClose,
                    });
                    report.events.push(RemoteFrameEventV1::CloseRequested);
                }
            }
        }

        if self.state.use_state != snapshot.use_state {
            let from = self.state.use_state;
            let to = snapshot.use_state;
            self.state.use_state = to;
            report
                .events
                .push(RemoteFrameEventV1::UseStateChanged { from, to });
        }

        let held = snapshot.candidate_clock_held_v1();
        if self.state.candidate_clock_held != held {
            self.state.candidate_clock_held = held;
            report
                .events
                .push(RemoteFrameEventV1::CandidateClockHoldUpdated { held });
        }
    }

    fn consume_retry_attempt_v1(
        &mut self,
        allowance: RemoteFrameAllowanceV1,
        frame_sequence: u64,
        report: &mut RemoteFrameReportV1,
    ) {
        if allowance.retry_attempts == 0 || !self.state.retry_admission_eligible {
            return;
        }
        if self.state.retry_admission_frame_sequence == Some(frame_sequence) {
            return;
        }
        if let Some(_retry) = self.state.retry_pending.pop_front() {
            self.state.retry_admission_eligible = false;
            self.state.retry_admission_frame_sequence = Some(frame_sequence);
            report.events.push(RemoteFrameEventV1::RetryAttemptReady);
        }
    }

    fn consume_fetch_completion_v1(
        &mut self,
        allowance: RemoteFrameAllowanceV1,
        report: &mut RemoteFrameReportV1,
    ) {
        if allowance.fetch_completions == 0 {
            return;
        }
        let Some(completion) = self.state.fetch_completions.pop_front() else {
            return;
        };

        match completion {
            RemoteFrameFetchCompletionV1::PhysicalChunkReady(physical) => {
                if physical_matches_proof_v1(physical, self.state.owner_proof) {
                    report
                        .events
                        .push(RemoteFrameEventV1::FetchCompletionReady {
                            canonical_ordinal: physical.canonical_ordinal,
                        });
                } else {
                    report
                        .events
                        .push(RemoteFrameEventV1::FetchCompletionRejected {
                            canonical_ordinal: physical.canonical_ordinal,
                        });
                }
            }
            RemoteFrameFetchCompletionV1::RetryPending(retry) => {
                self.state.retry_pending.push_back(retry);
                report
                    .events
                    .push(RemoteFrameEventV1::FetchRetryPendingQueued);
            }
            RemoteFrameFetchCompletionV1::MetadataStatus => {
                report
                    .events
                    .push(RemoteFrameEventV1::MetadataStatusProcessed);
            }
            RemoteFrameFetchCompletionV1::CloseAck => {
                report.events.push(RemoteFrameEventV1::CloseRequested);
            }
        }
    }

    fn consume_single_cpu_work_v1(
        &mut self,
        allowance: RemoteFrameAllowanceV1,
        report: &mut RemoteFrameReportV1,
    ) {
        if allowance.cpu_work_units == 0 {
            return;
        }
        if let Some(work) = self.state.cpu_work.pop_front() {
            report.events.push(RemoteFrameEventV1::CpuWorkConsumed {
                kind: work.kind,
                canonical_ordinal: work.canonical_ordinal,
            });
        }
    }

    fn consume_single_mutation_safe_point_v1(
        &mut self,
        allowance: RemoteFrameAllowanceV1,
        report: &mut RemoteFrameReportV1,
    ) {
        if allowance.mutation_safe_points == 0 {
            return;
        }
        if let Some(work) = self.state.mutation_work.pop_front() {
            report
                .events
                .push(RemoteFrameEventV1::MutationSafePointAdvanced {
                    kind: work.kind,
                    boundary: work.boundary,
                });
        }
    }

    fn consume_presentation_commit_v1(
        &mut self,
        allowance: RemoteFrameAllowanceV1,
        report: &mut RemoteFrameReportV1,
    ) {
        if allowance.presentation_commits == 0 {
            return;
        }
        if let Some(commit) = self.state.presentation_commits.pop_front() {
            report.cursor_delta = commit.cursor_delta;
            report
                .events
                .push(RemoteFrameEventV1::PresentationCommitApplied);
        }
    }

    fn consume_query_lease_request_v1(
        &mut self,
        allowance: RemoteFrameAllowanceV1,
        report: &mut RemoteFrameReportV1,
    ) {
        if allowance.query_leases == 0 {
            return;
        }
        if self.state.query_lease_requests.pop_front().is_some() {
            report.events.push(RemoteFrameEventV1::QueryLeaseAcquired);
        }
    }

    fn process_control_plane_only_v1(
        store_event: Option<RemoteStoreEventV1>,
        report: &mut RemoteFrameReportV1,
    ) {
        if store_event.is_some_and(RemoteStoreEventV1::is_control_plane_only_v1) {
            report
                .events
                .push(RemoteFrameEventV1::MetadataStatusProcessed);
        }
    }

    fn converge_bounded_callback_ownership_v1(
        &mut self,
        allowance: RemoteFrameAllowanceV1,
        report: &mut RemoteFrameReportV1,
    ) {
        if allowance.callback_ownership_convergence == 0 {
            return;
        }
        if self.state.callback_owners.pop_front().is_some() {
            report
                .events
                .push(RemoteFrameEventV1::CallbackOwnershipConverged);
        }
    }

    fn remote_work_remains_v1(&self) -> bool {
        !self.state.fetch_completions.is_empty()
            || !self.state.retry_pending.is_empty()
            || !self.state.cpu_work.is_empty()
            || !self.state.mutation_work.is_empty()
            || !self.state.presentation_commits.is_empty()
            || !self.state.query_lease_requests.is_empty()
            || !self.state.callback_owners.is_empty()
            || self.state.suspended_owner.is_some()
            || self.state.resumed_owner.is_some()
    }

    fn record_high_water_v1(&mut self, report: &mut RemoteFrameReportV1) {
        self.state.high_water.retained_fetch_completions = self.state.fetch_completions.len();
        self.state.high_water.retained_cpu_work = self.state.cpu_work.len();
        self.state.high_water.retained_mutation_work = self.state.mutation_work.len();
        report.remote_work_remains = self.remote_work_remains_v1();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteResumeResultV1 {
    rebound_owner: bool,
    clock_baseline: BrowserClockBaselineV1,
}

impl RemoteResumeResultV1 {
    pub(crate) const fn rebound_owner_v1(self) -> bool {
        self.rebound_owner
    }

    pub(crate) const fn clock_baseline_v1(self) -> BrowserClockBaselineV1 {
        self.clock_baseline
    }
}

fn physical_matches_proof_v1(
    physical: RemotePhysicalChunkCompletionV1,
    proof: RemoteResumeProofV1,
) -> bool {
    let Ok(reservation_generation) = u32::try_from(proof.reservation_generation) else {
        return false;
    };
    physical.source_generation == proof.source_token
        && physical.read_generation == proof.store_generation
        && physical.attempt_generation == proof.validator_generation
        && physical.canonical_ordinal == reservation_generation
        && physical.full_range == proof.full_range
        && physical.lease_generation == proof.pending_lease_generation
        && physical.body_owner_generation == proof.slot_token
}

impl ConsumerStorageFreeV1 for VecDeque<RemoteFrameFetchCompletionV1> {}
impl ConsumerStorageFreeV1 for VecDeque<RemoteFrameRetryPendingV1> {}
impl ConsumerStorageFreeV1 for VecDeque<RemoteFrameCpuWorkV1> {}
impl ConsumerStorageFreeV1 for VecDeque<RemoteFrameMutationWorkV1> {}
impl ConsumerStorageFreeV1 for VecDeque<RemotePresentationCommitV1> {}
impl ConsumerStorageFreeV1 for VecDeque<RemoteQueryLeaseRequestV1> {}
impl ConsumerStorageFreeV1 for VecDeque<RemoteCallbackOwnerV1> {}

impl ConsumerStorageFreeV1 for BrowserExecutionEpochV1 {}
impl ConsumerStorageFreeV1 for BrowserClockBaselineV1 {}
impl ConsumerStorageFreeV1 for RemoteResumeNonceV1 {}
impl ConsumerStorageFreeV1 for RemotePageTerminationReasonV1 {}
impl ConsumerStorageFreeV1 for ChromePageExecutionStateV1 {}
impl ConsumerStorageFreeV1 for RemoteIngressSourceKindV1 {}
impl ConsumerStorageFreeV1 for RemoteExactHandleCommandV1 {}
impl ConsumerStorageFreeV1 for RemotePendingIntentV1 {}
impl ConsumerStorageFreeV1 for RemoteCloseRequestV1 {}
impl ConsumerStorageFreeV1 for RemoteStoreEventV1 {}
impl ConsumerStorageFreeV1 for RemoteResumeProofV1 {}
impl ConsumerStorageFreeV1 for RemoteByteRangeV1 {}
impl ConsumerStorageFreeV1 for RemotePhysicalChunkCompletionV1 {}
impl ConsumerStorageFreeV1 for RemoteFrameRetryPendingV1 {}
impl ConsumerStorageFreeV1 for RemoteFrameFetchCompletionV1 {}
impl ConsumerStorageFreeV1 for RemoteCpuWorkKindV1 {}
impl ConsumerStorageFreeV1 for RemoteFrameCpuWorkV1 {}
impl ConsumerStorageFreeV1 for RemoteFrameMutationWorkV1 {}
impl ConsumerStorageFreeV1 for RemotePresentationCommitV1 {}
impl ConsumerStorageFreeV1 for RemoteQueryLeaseRequestV1 {}
impl ConsumerStorageFreeV1 for RemoteSuspendedOwnerV1 {}
impl ConsumerStorageFreeV1 for RemoteCallbackOwnerV1 {}
impl ConsumerStorageFreeV1 for RemoteFrameAllowanceV1 {}
impl ConsumerStorageFreeV1 for RemoteFrameResumeRejectionV1 {}
impl ConsumerStorageFreeV1 for RemoteFrameEventV1 {}
impl ConsumerStorageFreeV1 for RemoteFrameReportV1 {}
impl ConsumerStorageFreeV1 for RemoteFrameOutcomeV1 {}
impl ConsumerStorageFreeV1 for RemoteBrowserCallbackOutcomeV1 {}
impl ConsumerStorageFreeV1 for RemoteFrameHighWaterV1 {}
impl ConsumerStorageFreeV1 for RemoteFrameDriverErrorV1 {}
impl ConsumerStorageFreeV1 for RemoteResumeResultV1 {}
impl ConsumerStorageFreeV1 for RemoteMcapFrameDriverStateV1 {}
impl ConsumerStorageFreeV1 for RemoteMcapFrameDriverV1 {}
impl ConsumerStorageFreeV1 for RemoteFrameSnapshotV1 {}

#[cfg(test)]
mod tests {
    use re_entity_db::{EntityDb, StoreBundle};
    use re_query::StorageEngine;
    use re_viewer_context::StoreHub;

    use super::*;

    fn epoch(value: u64) -> BrowserExecutionEpochV1 {
        BrowserExecutionEpochV1::new_for_test_v1(value)
    }

    fn proof() -> RemoteResumeProofV1 {
        RemoteResumeProofV1 {
            slot_token: 11,
            source_token: 12,
            store_generation: 13,
            validator_generation: 14,
            reservation_generation: 7,
            completion_slot_generation: 15,
            pending_lease_generation: 15,
            full_range: RemoteByteRangeV1 {
                start: 700,
                end_exclusive: 799,
            },
            suspended_turn_generation: Some(16),
        }
    }

    fn physical(canonical_ordinal: u32) -> RemoteFrameFetchCompletionV1 {
        RemoteFrameFetchCompletionV1::PhysicalChunkReady(RemotePhysicalChunkCompletionV1 {
            source_generation: 12,
            read_generation: 13,
            attempt_generation: 14,
            canonical_ordinal,
            full_range: RemoteByteRangeV1 {
                start: u64::from(canonical_ordinal) * 100,
                end_exclusive: u64::from(canonical_ordinal) * 100 + 99,
            },
            lease_generation: 15,
            body_owner_generation: 11,
        })
    }

    fn retry(canonical_ordinal: u32) -> RemoteFrameRetryPendingV1 {
        RemoteFrameRetryPendingV1 {
            source_generation: 12,
            operation_generation: 100,
            attempt_generation: 1,
            canonical_ordinal,
            full_range: RemoteByteRangeV1 {
                start: u64::from(canonical_ordinal) * 100,
                end_exclusive: u64::from(canonical_ordinal) * 100 + 99,
            },
            deadline_remaining_micros: 1_000,
        }
    }

    fn cpu(kind: RemoteCpuWorkKindV1, canonical_ordinal: u32) -> RemoteFrameCpuWorkV1 {
        RemoteFrameCpuWorkV1 {
            kind,
            source_generation: 12,
            canonical_ordinal,
            phase_generation: 1,
        }
    }

    fn mutation() -> RemoteFrameMutationWorkV1 {
        RemoteFrameMutationWorkV1 {
            kind: RemoteMutationWorkKindV1::QueryVisibleInsertion,
            generation: 1,
            boundary: RemoteMutationBoundaryV1::BeforeFirstAddChunk,
        }
    }

    fn commit(cursor_delta: bool) -> RemotePresentationCommitV1 {
        RemotePresentationCommitV1 {
            source_generation: 12,
            cursor_delta,
        }
    }

    fn query_request() -> RemoteQueryLeaseRequestV1 {
        RemoteQueryLeaseRequestV1 {
            source_generation: 12,
        }
    }

    fn driver(use_state: RemoteRecordingUseStateV1) -> RemoteMcapFrameDriverV1 {
        RemoteMcapFrameDriverV1::new_disarmed_v1(epoch(1), use_state, proof())
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
    fn frame_ordering_is_fixed_and_does_not_gate_nonremote_ingress() {
        let expected = RemoteFrameOrderingV1::phases_v1();
        assert_eq!(
            expected,
            [
                RemoteFramePhaseV1::SnapshotPageAndOwner,
                RemoteFramePhaseV1::HiddenCloseAndCallbackConvergence,
                RemoteFramePhaseV1::RevalidateVisibleOwner,
                RemoteFramePhaseV1::MergeVisibleCommandAndUseState,
                RemoteFramePhaseV1::ConsumeFetchCompletion,
                RemoteFramePhaseV1::RunSingleRemoteCpuWork,
                RemoteFramePhaseV1::AdvanceSingleMutationSafePoint,
                RemoteFramePhaseV1::CommitPresentationRevision,
                RemoteFramePhaseV1::AcquireQueryLease,
                RemoteFramePhaseV1::FinishRemoteMetricsAndRepaint,
            ]
        );
        assert!(RemoteFrameOrderingV1::preserves_ordering_v1(&expected));

        let mut driver = driver(RemoteRecordingUseStateV1::Foreground);
        driver.enqueue_cpu_work_v1(cpu(RemoteCpuWorkKindV1::OpeningParse, 1));
        let queued_before = driver.state.cpu_work.len();

        for ingress in [
            RemoteIngressSourceKindV1::OrdinaryReceiver,
            RemoteIngressSourceKindV1::LogChannel,
            RemoteIngressSourceKindV1::GrpcMessageProxy,
            RemoteIngressSourceKindV1::Redap,
            RemoteIngressSourceKindV1::System,
            RemoteIngressSourceKindV1::Ui,
            RemoteIngressSourceKindV1::NonremoteQuery,
            RemoteIngressSourceKindV1::Native,
            RemoteIngressSourceKindV1::Local,
            RemoteIngressSourceKindV1::Legacy,
        ] {
            let mut snapshot = driver.snapshot_v1();
            snapshot.ingress = ingress;
            assert_eq!(
                driver
                    .drive_frame_v1(snapshot, RemoteFrameAllowanceV1::one_of_each_v1())
                    .unwrap(),
                RemoteFrameOutcomeV1::NotRemoteMcap { ingress }
            );
            assert_eq!(driver.state.cpu_work.len(), queued_before);
        }
    }

    #[test]
    fn hidden_drive_only_converges_callbacks_and_never_touches_temporal_work() {
        let mut driver = driver(RemoteRecordingUseStateV1::Foreground);
        driver.enqueue_fetch_completion_v1(physical(1));
        driver.enqueue_cpu_work_v1(cpu(RemoteCpuWorkKindV1::PhysicalChunkValidation, 1));
        driver.enqueue_mutation_work_v1(mutation());
        driver.enqueue_presentation_commit_v1(commit(true));
        driver.enqueue_query_lease_request_v1(query_request());
        driver.enqueue_retry_pending_v1(retry(1));
        driver.enqueue_callback_owner_v1(1);

        driver.transition_to_hidden_v1().unwrap();
        let snapshot = driver.snapshot_v1();
        let RemoteFrameOutcomeV1::Driven(report) = driver
            .drive_frame_v1(snapshot, RemoteFrameAllowanceV1::one_of_each_v1())
            .unwrap()
        else {
            panic!("remote hidden drive should be driven");
        };

        assert!(
            report
                .events
                .contains(&RemoteFrameEventV1::CallbackOwnershipConverged)
        );
        assert!(!report.events.iter().any(|event| matches!(
            event,
            RemoteFrameEventV1::FetchCompletionReady { .. }
                | RemoteFrameEventV1::FetchRetryPendingQueued
                | RemoteFrameEventV1::CpuWorkConsumed { .. }
                | RemoteFrameEventV1::MutationSafePointAdvanced { .. }
                | RemoteFrameEventV1::PresentationCommitApplied
                | RemoteFrameEventV1::QueryLeaseAcquired
                | RemoteFrameEventV1::RetryAttemptReady
        )));
        assert_eq!(driver.state.fetch_completions.len(), 1);
        assert_eq!(driver.state.retry_pending.len(), 1);
        assert_eq!(driver.state.cpu_work.len(), 1);
        assert_eq!(driver.state.mutation_work.len(), 0);
        assert_eq!(driver.state.presentation_commits.len(), 1);
        assert_eq!(driver.state.query_lease_requests.len(), 1);
        assert_eq!(
            driver.state.suspended_owner,
            Some(RemoteSuspendedOwnerV1::Mutation(mutation()))
        );
        assert!(!report.cursor_delta_v1());
    }

    #[test]
    fn revalidation_rebinds_once_and_rejects_stale_nonce_without_work() {
        let mut driver = driver(RemoteRecordingUseStateV1::Foreground);
        driver.state.suspended_owner = Some(RemoteSuspendedOwnerV1::Mutation(mutation()));
        driver.transition_to_hidden_v1().unwrap();
        let nonce = driver.begin_resume_v1().unwrap();

        let mut stale_snapshot = driver.snapshot_v1();
        stale_snapshot.resume_nonce = Some(RemoteResumeNonceV1(nonce.get_v1() + 1));
        let RemoteFrameOutcomeV1::Driven(stale_report) = driver
            .drive_frame_v1(stale_snapshot, RemoteFrameAllowanceV1::one_of_each_v1())
            .unwrap()
        else {
            panic!("stale resume should still be driven as a rejected revalidation");
        };
        assert!(
            stale_report
                .events
                .contains(&RemoteFrameEventV1::ResumeRejected(
                    RemoteFrameResumeRejectionV1::StaleNonce,
                ))
        );
        assert_eq!(
            driver.page_state_v1(),
            ChromePageExecutionStateV1::VisibleRevalidating {
                from_epoch: epoch(2),
                to_epoch: epoch(3),
                resume_nonce: nonce,
            }
        );
        assert!(driver.state.suspended_owner.is_some());

        let mut resume_snapshot = driver.snapshot_v1();
        resume_snapshot.resume_nonce = Some(nonce);
        let RemoteFrameOutcomeV1::Driven(resume_report) = driver
            .drive_frame_v1(resume_snapshot, RemoteFrameAllowanceV1::one_of_each_v1())
            .unwrap()
        else {
            panic!("valid resume should be driven");
        };
        assert!(resume_report.events.iter().any(|event| matches!(
            event,
            RemoteFrameEventV1::PageResumed {
                rebound_owner: true,
                ..
            }
        )));
        assert!(driver.state.suspended_owner.is_none());
        assert!(driver.state.resumed_owner.is_some());
        assert!(driver.page_state_v1().is_visible_running_v1());
    }

    #[test]
    fn visible_running_allows_one_retry_and_one_cpu_phase_per_frame() {
        let mut driver = driver(RemoteRecordingUseStateV1::Foreground);
        driver.enqueue_retry_pending_v1(retry(1));
        driver.enqueue_retry_pending_v1(retry(2));
        driver.enqueue_cpu_work_v1(cpu(RemoteCpuWorkKindV1::PhysicalChunkValidation, 1));
        driver.enqueue_cpu_work_v1(cpu(RemoteCpuWorkKindV1::PhysicalChunkDispatchDecode, 1));

        let snapshot = driver.snapshot_v1();
        let RemoteFrameOutcomeV1::Driven(report) = driver
            .drive_frame_v1(snapshot, RemoteFrameAllowanceV1::one_of_each_v1())
            .unwrap()
        else {
            panic!("visible running should be driven");
        };

        assert_eq!(
            report
                .events
                .iter()
                .filter(|event| matches!(event, RemoteFrameEventV1::RetryAttemptReady))
                .count(),
            1
        );
        assert_eq!(
            report
                .events
                .iter()
                .filter(|event| matches!(event, RemoteFrameEventV1::CpuWorkConsumed { .. }))
                .count(),
            1
        );
        assert_eq!(driver.state.retry_pending.len(), 1);
        assert_eq!(driver.state.cpu_work.len(), 1);

        let snapshot = driver.snapshot_v1();
        let RemoteFrameOutcomeV1::Driven(second_report) = driver
            .drive_frame_v1(snapshot, RemoteFrameAllowanceV1::one_of_each_v1())
            .unwrap()
        else {
            panic!("second visible running should be driven");
        };
        assert_eq!(
            second_report
                .events
                .iter()
                .filter(|event| matches!(event, RemoteFrameEventV1::RetryAttemptReady))
                .count(),
            1
        );
        assert_eq!(driver.state.retry_pending.len(), 0);
    }

    #[test]
    fn catalog_only_and_inactive_produce_no_temporal_delta() {
        for use_state in [
            RemoteRecordingUseStateV1::CatalogOnly,
            RemoteRecordingUseStateV1::Inactive,
        ] {
            let mut driver = driver(use_state);
            driver.enqueue_fetch_completion_v1(physical(1));
            driver.enqueue_cpu_work_v1(cpu(RemoteCpuWorkKindV1::MessageIndexParse, 1));
            driver.enqueue_mutation_work_v1(mutation());
            driver.enqueue_presentation_commit_v1(commit(true));
            driver.enqueue_query_lease_request_v1(query_request());

            let mut snapshot = driver.snapshot_v1();
            snapshot.store_event = Some(RemoteStoreEventV1::MetadataStatus);
            let RemoteFrameOutcomeV1::Driven(report) = driver
                .drive_frame_v1(snapshot, RemoteFrameAllowanceV1::one_of_each_v1())
                .unwrap()
            else {
                panic!("control-plane drive should be driven");
            };

            assert!(
                report
                    .events_v1()
                    .iter()
                    .any(|event| matches!(event, RemoteFrameEventV1::MetadataStatusProcessed))
            );
            assert!(!report.events_v1().iter().any(|event| matches!(
                event,
                RemoteFrameEventV1::FetchCompletionReady { .. }
                    | RemoteFrameEventV1::CpuWorkConsumed { .. }
                    | RemoteFrameEventV1::MutationSafePointAdvanced { .. }
                    | RemoteFrameEventV1::PresentationCommitApplied
                    | RemoteFrameEventV1::QueryLeaseAcquired
            )));
            assert!(!report.cursor_delta_v1());
            assert_eq!(driver.state.fetch_completions.len(), 1);
            assert_eq!(driver.state.cpu_work.len(), 1);
            assert_eq!(driver.state.mutation_work.len(), 1);
            assert_eq!(driver.state.presentation_commits.len(), 1);
            assert_eq!(driver.state.query_lease_requests.len(), 1);
        }
    }

    #[test]
    fn browser_callback_only_enqueues_and_requests_repaint() {
        let mut driver = driver(RemoteRecordingUseStateV1::Foreground);
        let outcome = driver.enqueue_fetch_completion_v1(physical(1));
        assert!(outcome.completion_enqueued);
        assert!(outcome.request_repaint);
        assert_eq!(driver.state.fetch_completions.len(), 1);
        assert!(driver.state.cpu_work.is_empty());
        assert!(driver.state.retry_pending.is_empty());

        let retry_outcome = driver
            .enqueue_fetch_completion_v1(RemoteFrameFetchCompletionV1::RetryPending(retry(2)));
        assert!(retry_outcome.completion_enqueued);
        assert!(retry_outcome.request_repaint);
        assert_eq!(driver.state.fetch_completions.len(), 2);
        assert!(driver.state.retry_pending.is_empty());
    }

    #[test]
    fn exhausted_allowance_returns_frame_without_draining_backlog() {
        let mut driver = driver(RemoteRecordingUseStateV1::Foreground);
        driver.enqueue_cpu_work_v1(cpu(RemoteCpuWorkKindV1::OpeningParse, 1));
        driver.enqueue_cpu_work_v1(cpu(RemoteCpuWorkKindV1::OpeningParse, 2));

        let mut allowance = RemoteFrameAllowanceV1::one_of_each_v1();
        allowance.cpu_work_units = 0;
        let snapshot = driver.snapshot_v1();
        let RemoteFrameOutcomeV1::Driven(report) =
            driver.drive_frame_v1(snapshot, allowance).unwrap()
        else {
            panic!("exhausted frame should be driven");
        };
        assert!(
            report
                .events
                .iter()
                .all(|event| !matches!(event, RemoteFrameEventV1::CpuWorkConsumed { .. }))
        );
        assert_eq!(driver.state.cpu_work.len(), 2);
        assert!(report.request_repaint_v1());
        assert!(report.remote_work_remains_v1());
    }

    #[test]
    fn remaining_work_requests_repaint_after_one_item_is_consumed() {
        let mut driver = driver(RemoteRecordingUseStateV1::Foreground);
        driver.enqueue_cpu_work_v1(cpu(RemoteCpuWorkKindV1::OpeningParse, 1));
        driver.enqueue_cpu_work_v1(cpu(RemoteCpuWorkKindV1::OpeningParse, 2));

        let snapshot = driver.snapshot_v1();
        let RemoteFrameOutcomeV1::Driven(report) = driver
            .drive_frame_v1(snapshot, RemoteFrameAllowanceV1::one_of_each_v1())
            .unwrap()
        else {
            panic!("frame should be driven");
        };
        assert_eq!(driver.state.cpu_work.len(), 1);
        assert!(report.request_repaint_v1());
        assert!(report.remote_work_remains_v1());
    }

    #[test]
    fn close_request_preempts_all_visible_running_temporal_work() {
        for close_command in [false, true] {
            let mut driver = driver(RemoteRecordingUseStateV1::Foreground);
            driver.enqueue_retry_pending_v1(retry(1));
            driver.enqueue_fetch_completion_v1(physical(1));
            driver.enqueue_cpu_work_v1(cpu(RemoteCpuWorkKindV1::PhysicalChunkValidation, 1));
            driver.enqueue_mutation_work_v1(mutation());
            driver.enqueue_presentation_commit_v1(commit(true));
            driver.enqueue_query_lease_request_v1(query_request());

            let mut snapshot = driver.snapshot_v1();
            if close_command {
                snapshot.exact_handle_command = Some(RemoteExactHandleCommandV1::Close {
                    source_generation: 1,
                });
            } else {
                snapshot.close_request = Some(RemoteCloseRequestV1 {
                    source_generation: 1,
                    reason: RemotePageTerminationReasonV1::ExplicitClose,
                });
            }

            let RemoteFrameOutcomeV1::Driven(report) = driver
                .drive_frame_v1(snapshot, RemoteFrameAllowanceV1::one_of_each_v1())
                .unwrap()
            else {
                panic!("close request should still drive one terminating frame");
            };

            assert!(
                report
                    .events
                    .iter()
                    .any(|event| matches!(event, RemoteFrameEventV1::CloseRequested))
            );
            assert!(!report.events.iter().any(|event| matches!(
                event,
                RemoteFrameEventV1::RetryAttemptReady
                    | RemoteFrameEventV1::FetchCompletionReady { .. }
                    | RemoteFrameEventV1::CpuWorkConsumed { .. }
                    | RemoteFrameEventV1::MutationSafePointAdvanced { .. }
                    | RemoteFrameEventV1::PresentationCommitApplied
                    | RemoteFrameEventV1::QueryLeaseAcquired
            )));
            assert!(matches!(
                driver.page_state_v1(),
                ChromePageExecutionStateV1::RemoteTerminating {
                    reason: RemotePageTerminationReasonV1::ExplicitClose,
                    ..
                }
            ));
            assert!(driver.state.fetch_completions.is_empty());
            assert!(driver.state.retry_pending.is_empty());
            assert!(driver.state.cpu_work.is_empty());
            assert!(driver.state.mutation_work.is_empty());
            assert!(driver.state.presentation_commits.is_empty());
            assert!(driver.state.query_lease_requests.is_empty());
        }
    }

    #[test]
    fn terminating_transition_drains_remote_work_and_stops_repaint() {
        let mut driver = driver(RemoteRecordingUseStateV1::Foreground);
        driver.enqueue_retry_pending_v1(retry(1));
        driver.enqueue_fetch_completion_v1(physical(1));
        driver.enqueue_cpu_work_v1(cpu(RemoteCpuWorkKindV1::PhysicalChunkValidation, 1));
        driver.enqueue_mutation_work_v1(mutation());
        driver.enqueue_presentation_commit_v1(commit(true));
        driver.enqueue_query_lease_request_v1(query_request());
        driver.enqueue_callback_owner_v1(1);

        driver
            .transition_to_terminating_v1(RemotePageTerminationReasonV1::PageHide)
            .unwrap();
        assert!(driver.state.fetch_completions.is_empty());
        assert!(driver.state.retry_pending.is_empty());
        assert!(driver.state.cpu_work.is_empty());
        assert!(driver.state.mutation_work.is_empty());
        assert!(driver.state.presentation_commits.is_empty());
        assert!(driver.state.query_lease_requests.is_empty());
        assert!(driver.state.callback_owners.is_empty());

        let snapshot = driver.snapshot_v1();
        let RemoteFrameOutcomeV1::Driven(report) = driver
            .drive_frame_v1(snapshot, RemoteFrameAllowanceV1::one_of_each_v1())
            .unwrap()
        else {
            panic!("terminating frame should be driven");
        };

        assert!(!report.events.iter().any(|event| matches!(
            event,
            RemoteFrameEventV1::RetryAttemptReady
                | RemoteFrameEventV1::FetchCompletionReady { .. }
                | RemoteFrameEventV1::CpuWorkConsumed { .. }
                | RemoteFrameEventV1::MutationSafePointAdvanced { .. }
                | RemoteFrameEventV1::PresentationCommitApplied
                | RemoteFrameEventV1::QueryLeaseAcquired
        )));
        assert!(!report.remote_work_remains_v1());
        assert!(!report.request_repaint_v1());
    }

    #[test]
    fn hidden_resume_moves_mutation_front_and_recovers_it_as_sole_frame_work() {
        let mut driver = driver(RemoteRecordingUseStateV1::Foreground);
        let original_work = RemoteFrameMutationWorkV1 {
            kind: RemoteMutationWorkKindV1::QueryVisibleInsertion,
            generation: 1,
            boundary: RemoteMutationBoundaryV1::BeforeFirstAddChunk,
        };
        driver.enqueue_mutation_work_v1(original_work);

        driver.transition_to_hidden_v1().unwrap();
        assert!(driver.state.mutation_work.is_empty());
        assert_eq!(
            driver.state.suspended_owner,
            Some(RemoteSuspendedOwnerV1::Mutation(original_work))
        );

        let nonce = driver.begin_resume_v1().unwrap();
        let mut resume_snapshot = driver.snapshot_v1();
        resume_snapshot.resume_nonce = Some(nonce);
        let RemoteFrameOutcomeV1::Driven(resume_report) = driver
            .drive_frame_v1(resume_snapshot, RemoteFrameAllowanceV1::one_of_each_v1())
            .unwrap()
        else {
            panic!("valid resume should be driven");
        };
        assert!(resume_report.events.iter().any(|event| matches!(
            event,
            RemoteFrameEventV1::PageResumed {
                rebound_owner: true,
                ..
            }
        )));
        assert!(driver.state.suspended_owner.is_none());
        assert_eq!(
            driver.state.resumed_owner,
            Some(RemoteSuspendedOwnerV1::Mutation(original_work))
        );

        let bypass_mutation = RemoteFrameMutationWorkV1 {
            kind: RemoteMutationWorkKindV1::QueryVisibleInsertion,
            generation: 2,
            boundary: RemoteMutationBoundaryV1::AfterLastAddChunkBeforeAck,
        };
        driver.enqueue_mutation_work_v1(bypass_mutation);
        driver.enqueue_cpu_work_v1(cpu(RemoteCpuWorkKindV1::OpeningParse, 1));
        driver.enqueue_presentation_commit_v1(commit(true));

        let recovery_snapshot = driver.snapshot_v1();
        let RemoteFrameOutcomeV1::Driven(recovery_report) = driver
            .drive_frame_v1(recovery_snapshot, RemoteFrameAllowanceV1::one_of_each_v1())
            .unwrap()
        else {
            panic!("resumed owner recovery frame should be driven");
        };

        assert!(recovery_report.events.iter().any(|event| matches!(
            event,
            RemoteFrameEventV1::MutationSafePointAdvanced {
                kind: RemoteMutationWorkKindV1::QueryVisibleInsertion,
                boundary: RemoteMutationBoundaryV1::BeforeFirstAddChunk,
            }
        )));
        assert!(!recovery_report.events.iter().any(|event| matches!(
            event,
            RemoteFrameEventV1::CpuWorkConsumed { .. }
                | RemoteFrameEventV1::PresentationCommitApplied
                | RemoteFrameEventV1::RetryAttemptReady
        )));
        assert!(driver.state.resumed_owner.is_none());
        assert_eq!(driver.state.mutation_work.len(), 1);
        assert_eq!(driver.state.cpu_work.len(), 1);
        assert_eq!(driver.state.presentation_commits.len(), 1);
    }

    #[test]
    fn physical_completion_proof_rejects_non_u32_reservation_generation() {
        let mut overflowing_proof = proof();
        overflowing_proof.reservation_generation = u64::from(u32::MAX) + 1;

        for canonical_ordinal in [0, 1, 7] {
            let physical = RemotePhysicalChunkCompletionV1 {
                source_generation: overflowing_proof.source_token,
                read_generation: overflowing_proof.store_generation,
                attempt_generation: overflowing_proof.validator_generation,
                canonical_ordinal,
                full_range: overflowing_proof.full_range,
                lease_generation: overflowing_proof.pending_lease_generation,
                body_owner_generation: overflowing_proof.slot_token,
            };
            assert!(!physical_matches_proof_v1(physical, overflowing_proof));
        }
    }

    #[test]
    fn frame_driver_reuses_existing_cpu_and_mutation_source_of_truth() {
        fn cpu_kind_roundtrip(
            value: RemoteCpuWorkKindV1,
        ) -> crate::web_remote_mcap_cpu::RemoteCpuWorkKindV1 {
            value
        }

        fn mutation_kind_roundtrip(
            value: RemoteMutationWorkKindV1,
        ) -> crate::web_remote_mcap_mutation_arbiter::RemoteMutationKindV1 {
            value
        }

        fn mutation_boundary_roundtrip(
            value: RemoteMutationBoundaryV1,
        ) -> crate::web_remote_mcap_mutation_arbiter::RemoteMutationSafePointV1 {
            value
        }

        for kind in [
            RemoteCpuWorkKindV1::OpeningParse,
            RemoteCpuWorkKindV1::MessageIndexParse,
            RemoteCpuWorkKindV1::PhysicalChunkValidation,
            RemoteCpuWorkKindV1::PhysicalChunkDispatchDecode,
        ] {
            assert_eq!(cpu_kind_roundtrip(kind), kind);
        }

        for kind in [
            RemoteMutationWorkKindV1::QueryVisibleInsertion,
            RemoteMutationWorkKindV1::GarbageCollection,
            RemoteMutationWorkKindV1::TerminalCleanup,
        ] {
            assert_eq!(mutation_kind_roundtrip(kind), kind);
        }

        for boundary in [
            RemoteMutationBoundaryV1::WaitingForLeases,
            RemoteMutationBoundaryV1::BeforeFirstAddChunk,
            RemoteMutationBoundaryV1::BetweenAddChunks,
            RemoteMutationBoundaryV1::AfterLastAddChunkBeforeAck,
            RemoteMutationBoundaryV1::GcWaitingForLeases,
            RemoteMutationBoundaryV1::AfterGcBeforeReopen,
        ] {
            assert_eq!(mutation_boundary_roundtrip(boundary), boundary);
        }
    }

    #[test]
    fn resume_clock_baseline_exhaustion_has_dedicated_rejection() {
        let mut driver = driver(RemoteRecordingUseStateV1::Foreground);
        driver.transition_to_hidden_v1().unwrap();
        let nonce = driver.begin_resume_v1().unwrap();
        driver.state.clock_baseline = BrowserClockBaselineV1 {
            epoch: epoch(3),
            sequence: u64::MAX,
        };

        let mut snapshot = driver.snapshot_v1();
        snapshot.resume_nonce = Some(nonce);
        let RemoteFrameOutcomeV1::Driven(report) = driver
            .drive_frame_v1(snapshot, RemoteFrameAllowanceV1::one_of_each_v1())
            .unwrap()
        else {
            panic!("resume rejection frame should be driven");
        };

        assert!(report.events.iter().any(|event| matches!(
            event,
            RemoteFrameEventV1::ResumeRejected(
                RemoteFrameResumeRejectionV1::ClockBaselineExhausted
            )
        )));
        assert!(!report.events.iter().any(|event| matches!(
            event,
            RemoteFrameEventV1::ResumeRejected(RemoteFrameResumeRejectionV1::ExplicitClose)
        )));
        assert!(matches!(
            driver.page_state_v1(),
            ChromePageExecutionStateV1::VisibleRevalidating { .. }
        ));
    }

    #[test]
    fn frame_sequence_overflow_returns_checked_error() {
        let mut driver = driver(RemoteRecordingUseStateV1::Foreground);
        driver.state.high_water.frame_sequence = u64::MAX;
        let snapshot = driver.snapshot_v1();

        assert_eq!(
            driver.drive_frame_v1(snapshot, RemoteFrameAllowanceV1::one_of_each_v1()),
            Err(RemoteFrameDriverErrorV1::FrameSequenceExhausted)
        );
    }

    #[test]
    fn frame_slice_is_storage_free_by_structure() {
        assert_not_storage_free_v1!(EntityDb);
        assert_not_storage_free_v1!(StoreBundle);
        assert_not_storage_free_v1!(StoreHub);
        assert_not_storage_free_v1!(StorageEngine);

        assert_storage_free_v1::<RemoteMcapFrameDriverV1>();
        assert_storage_free_v1::<RemoteMcapFrameDriverStateV1>();
        assert_storage_free_v1::<RemoteFrameSnapshotV1>();
        assert_storage_free_v1::<ChromePageExecutionStateV1>();
        assert_storage_free_v1::<RemoteFrameReportV1>();
    }
}
