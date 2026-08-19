//! Production-disarmed remote MCAP failure classification and terminal cleanup boundary.
//!
//! This module freezes MCAP-103's failure scopes without importing a Viewer, `StoreHub`,
//! `StoreBundle`, `EntityDb`, storage engine, or ordinary transport implementation.

#![allow(dead_code)]

use crate::web_remote_mcap_query::ConsumerStorageFreeV1;

const MAX_TERMINAL_CLEANUP_TRIGGERS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteMcapFailureClassificationV1 {
    Admission,
    FormatSniff,
    Network,
    Validation,
    Decode,
    IdentifierAdmission,
    Presentation,
    Resource,
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteMcapFailureScopeV1 {
    OptionalWork,
    CurrentSeek,
    SessionFatal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteMcapFailurePhaseV1 {
    MetadataOpening,
    Prefetch,
    PhaseBSeek,
    Refetch,
    PostMutation,
    PresentationCommit,
    InitialPresentation,
    Cleanup,
    Admission,
    FormatSniff,
    IdentifierAdmission,
    Resource,
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteMcapFailureRetryabilityV1 {
    Retryable,
    NonRetryable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteMcapDemandOwnerV1 {
    Prefetch,
    CurrentSeek,
    Refetch,
    PostMutation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteMcapFailureOwnerV1 {
    MetadataOpening,
    Prefetch,
    CurrentSeek,
    Refetch,
    PostMutation,
}

impl RemoteMcapDemandOwnerV1 {
    const fn into_failure_owner_v1(self) -> RemoteMcapFailureOwnerV1 {
        match self {
            Self::Prefetch => RemoteMcapFailureOwnerV1::Prefetch,
            Self::CurrentSeek => RemoteMcapFailureOwnerV1::CurrentSeek,
            Self::Refetch => RemoteMcapFailureOwnerV1::Refetch,
            Self::PostMutation => RemoteMcapFailureOwnerV1::PostMutation,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteMcapFailureV1 {
    pub(crate) classification: RemoteMcapFailureClassificationV1,
    pub(crate) scope: Option<RemoteMcapFailureScopeV1>,
    pub(crate) owner: RemoteMcapFailureOwnerV1,
    pub(crate) phase: RemoteMcapFailurePhaseV1,
    pub(crate) retryability: RemoteMcapFailureRetryabilityV1,
    pub(crate) label: &'static str,
}

impl RemoteMcapFailureV1 {
    const fn new_v1(
        classification: RemoteMcapFailureClassificationV1,
        scope: Option<RemoteMcapFailureScopeV1>,
        owner: RemoteMcapFailureOwnerV1,
        phase: RemoteMcapFailurePhaseV1,
        retryability: RemoteMcapFailureRetryabilityV1,
        label: &'static str,
    ) -> Self {
        Self {
            classification,
            scope,
            owner,
            phase,
            retryability,
            label,
        }
    }

    const fn optional_work_v1(
        classification: RemoteMcapFailureClassificationV1,
        owner: RemoteMcapFailureOwnerV1,
        phase: RemoteMcapFailurePhaseV1,
        retryability: RemoteMcapFailureRetryabilityV1,
        label: &'static str,
    ) -> Self {
        Self::new_v1(
            classification,
            Some(RemoteMcapFailureScopeV1::OptionalWork),
            owner,
            phase,
            retryability,
            label,
        )
    }

    const fn current_seek_v1(
        classification: RemoteMcapFailureClassificationV1,
        owner: RemoteMcapFailureOwnerV1,
        phase: RemoteMcapFailurePhaseV1,
        retryability: RemoteMcapFailureRetryabilityV1,
        label: &'static str,
    ) -> Self {
        Self::new_v1(
            classification,
            Some(RemoteMcapFailureScopeV1::CurrentSeek),
            owner,
            phase,
            retryability,
            label,
        )
    }

    const fn session_fatal_v1(
        classification: RemoteMcapFailureClassificationV1,
        owner: RemoteMcapFailureOwnerV1,
        phase: RemoteMcapFailurePhaseV1,
        label: &'static str,
    ) -> Self {
        Self::new_v1(
            classification,
            Some(RemoteMcapFailureScopeV1::SessionFatal),
            owner,
            phase,
            RemoteMcapFailureRetryabilityV1::NonRetryable,
            label,
        )
    }

    const fn unscoped_v1(
        classification: RemoteMcapFailureClassificationV1,
        owner: RemoteMcapFailureOwnerV1,
        phase: RemoteMcapFailurePhaseV1,
        retryability: RemoteMcapFailureRetryabilityV1,
        label: &'static str,
    ) -> Self {
        Self::new_v1(classification, None, owner, phase, retryability, label)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemotePrefetchFailureReasonV1 {
    ObjectChanged,
    ValidatorViolation,
    IndexCorruption,
    TransportUnavailable,
    BudgetExhausted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemotePhaseBFailureReasonV1 {
    NetworkTimeout,
    AttemptExhausted,
    RangeCountExhausted,
    ActiveVisibleDeadlineExhausted,
    ValidatorViolation,
    LengthChanged,
    Http412,
    Http416,
    ObjectChanged,
    IndexCorruption,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemotePostMutationFailureReasonV1 {
    Insertion,
    ResidencyAck,
    PresentationCommit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemotePreMutationSeekTransitionV1 {
    RollbackToCommittedCursor(RemoteMcapFailureV1),
    InitialPresentationFailed(RemoteMcapFailureV1),
    SessionFatal(RemoteMcapFailureV1),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteFailureSourceKindV1 {
    RemoteMcap,
    Legacy,
    LogChannel,
    GrpcMessageProxy,
    Redap,
    Local,
    Native,
}

impl RemoteFailureSourceKindV1 {
    pub(crate) const fn observes_remote_failure_v1(self) -> bool {
        matches!(self, Self::RemoteMcap)
    }

    pub(crate) const fn consumes_remote_budget_v1(self) -> bool {
        matches!(self, Self::RemoteMcap)
    }

    pub(crate) const fn accepts_remote_terminal_cleanup_v1(self) -> bool {
        matches!(self, Self::RemoteMcap)
    }

    pub(crate) const fn route_failure_v1(
        self,
        failure: RemoteMcapFailureV1,
    ) -> Option<RemoteMcapFailureV1> {
        if self.observes_remote_failure_v1() {
            Some(failure)
        } else {
            None
        }
    }
}

pub(crate) struct RemoteMcapFailureClassifierV1;

impl RemoteMcapFailureClassifierV1 {
    pub(crate) const fn classify_prefetch_v1(
        reason: RemotePrefetchFailureReasonV1,
    ) -> RemoteMcapFailureV1 {
        match reason {
            RemotePrefetchFailureReasonV1::ObjectChanged => RemoteMcapFailureV1::session_fatal_v1(
                RemoteMcapFailureClassificationV1::Resource,
                RemoteMcapFailureOwnerV1::Prefetch,
                RemoteMcapFailurePhaseV1::Prefetch,
                "remote prefetch object changed",
            ),
            RemotePrefetchFailureReasonV1::ValidatorViolation => {
                RemoteMcapFailureV1::session_fatal_v1(
                    RemoteMcapFailureClassificationV1::Validation,
                    RemoteMcapFailureOwnerV1::Prefetch,
                    RemoteMcapFailurePhaseV1::Prefetch,
                    "remote prefetch validator violation",
                )
            }
            RemotePrefetchFailureReasonV1::IndexCorruption => {
                RemoteMcapFailureV1::session_fatal_v1(
                    RemoteMcapFailureClassificationV1::Validation,
                    RemoteMcapFailureOwnerV1::Prefetch,
                    RemoteMcapFailurePhaseV1::Prefetch,
                    "remote prefetch deterministic index corruption",
                )
            }
            RemotePrefetchFailureReasonV1::TransportUnavailable => {
                RemoteMcapFailureV1::optional_work_v1(
                    RemoteMcapFailureClassificationV1::Network,
                    RemoteMcapFailureOwnerV1::Prefetch,
                    RemoteMcapFailurePhaseV1::Prefetch,
                    RemoteMcapFailureRetryabilityV1::Retryable,
                    "remote prefetch transport unavailable",
                )
            }
            RemotePrefetchFailureReasonV1::BudgetExhausted => {
                RemoteMcapFailureV1::optional_work_v1(
                    RemoteMcapFailureClassificationV1::Resource,
                    RemoteMcapFailureOwnerV1::Prefetch,
                    RemoteMcapFailurePhaseV1::Prefetch,
                    RemoteMcapFailureRetryabilityV1::NonRetryable,
                    "remote prefetch budget exhausted",
                )
            }
        }
    }

    pub(crate) const fn classify_metadata_opening_exhaustion_v1() -> RemoteMcapFailureV1 {
        RemoteMcapFailureV1::unscoped_v1(
            RemoteMcapFailureClassificationV1::Resource,
            RemoteMcapFailureOwnerV1::MetadataOpening,
            RemoteMcapFailurePhaseV1::MetadataOpening,
            RemoteMcapFailureRetryabilityV1::NonRetryable,
            "remote metadata opening retry exhaustion",
        )
    }

    pub(crate) const fn classify_phase_b_v1(
        owner: RemoteMcapDemandOwnerV1,
        reason: RemotePhaseBFailureReasonV1,
    ) -> RemoteMcapFailureV1 {
        let failure_owner = owner.into_failure_owner_v1();
        match reason {
            RemotePhaseBFailureReasonV1::NetworkTimeout => RemoteMcapFailureV1::current_seek_v1(
                RemoteMcapFailureClassificationV1::Network,
                failure_owner,
                RemoteMcapFailurePhaseV1::PhaseBSeek,
                RemoteMcapFailureRetryabilityV1::Retryable,
                "remote phase-B network timeout",
            ),
            RemotePhaseBFailureReasonV1::AttemptExhausted => RemoteMcapFailureV1::current_seek_v1(
                RemoteMcapFailureClassificationV1::Resource,
                failure_owner,
                RemoteMcapFailurePhaseV1::PhaseBSeek,
                RemoteMcapFailureRetryabilityV1::NonRetryable,
                "remote phase-B attempt exhaustion",
            ),
            RemotePhaseBFailureReasonV1::RangeCountExhausted => {
                RemoteMcapFailureV1::current_seek_v1(
                    RemoteMcapFailureClassificationV1::Resource,
                    failure_owner,
                    RemoteMcapFailurePhaseV1::PhaseBSeek,
                    RemoteMcapFailureRetryabilityV1::NonRetryable,
                    "remote phase-B Range count exhaustion",
                )
            }
            RemotePhaseBFailureReasonV1::ActiveVisibleDeadlineExhausted => {
                RemoteMcapFailureV1::current_seek_v1(
                    RemoteMcapFailureClassificationV1::Resource,
                    failure_owner,
                    RemoteMcapFailurePhaseV1::PhaseBSeek,
                    RemoteMcapFailureRetryabilityV1::NonRetryable,
                    "remote phase-B active-visible deadline exhaustion",
                )
            }
            RemotePhaseBFailureReasonV1::ValidatorViolation => {
                RemoteMcapFailureV1::session_fatal_v1(
                    RemoteMcapFailureClassificationV1::Validation,
                    failure_owner,
                    RemoteMcapFailurePhaseV1::PhaseBSeek,
                    "remote phase-B validator violation",
                )
            }
            RemotePhaseBFailureReasonV1::LengthChanged => RemoteMcapFailureV1::session_fatal_v1(
                RemoteMcapFailureClassificationV1::Validation,
                failure_owner,
                RemoteMcapFailurePhaseV1::PhaseBSeek,
                "remote phase-B validator length changed",
            ),
            RemotePhaseBFailureReasonV1::Http412 => RemoteMcapFailureV1::session_fatal_v1(
                RemoteMcapFailureClassificationV1::Network,
                failure_owner,
                RemoteMcapFailurePhaseV1::PhaseBSeek,
                "remote phase-B HTTP 412",
            ),
            RemotePhaseBFailureReasonV1::Http416 => RemoteMcapFailureV1::current_seek_v1(
                RemoteMcapFailureClassificationV1::Network,
                failure_owner,
                RemoteMcapFailurePhaseV1::PhaseBSeek,
                RemoteMcapFailureRetryabilityV1::NonRetryable,
                "remote phase-B HTTP 416",
            ),
            RemotePhaseBFailureReasonV1::ObjectChanged => RemoteMcapFailureV1::session_fatal_v1(
                RemoteMcapFailureClassificationV1::Resource,
                failure_owner,
                RemoteMcapFailurePhaseV1::PhaseBSeek,
                "remote phase-B object changed",
            ),
            RemotePhaseBFailureReasonV1::IndexCorruption => RemoteMcapFailureV1::session_fatal_v1(
                RemoteMcapFailureClassificationV1::Validation,
                failure_owner,
                RemoteMcapFailurePhaseV1::PhaseBSeek,
                "remote phase-B deterministic index corruption",
            ),
        }
    }

    pub(crate) const fn classify_pre_mutation_seek_v1(
        has_committed_cursor: bool,
        reason: RemotePhaseBFailureReasonV1,
    ) -> RemotePreMutationSeekTransitionV1 {
        let failure = Self::classify_phase_b_v1(RemoteMcapDemandOwnerV1::CurrentSeek, reason);
        match failure.scope {
            Some(RemoteMcapFailureScopeV1::SessionFatal) => {
                RemotePreMutationSeekTransitionV1::SessionFatal(failure)
            }
            _ if has_committed_cursor => {
                RemotePreMutationSeekTransitionV1::RollbackToCommittedCursor(failure)
            }
            _ => RemotePreMutationSeekTransitionV1::InitialPresentationFailed(failure),
        }
    }

    pub(crate) const fn classify_post_mutation_v1(
        reason: RemotePostMutationFailureReasonV1,
    ) -> RemoteTerminalCauseV1 {
        let label = match reason {
            RemotePostMutationFailureReasonV1::Insertion => {
                "remote post-insertion insertion failure"
            }
            RemotePostMutationFailureReasonV1::ResidencyAck => {
                "remote post-insertion residency ack failure"
            }
            RemotePostMutationFailureReasonV1::PresentationCommit => {
                "remote post-insertion presentation commit failure"
            }
        };
        let failure = RemoteMcapFailureV1::unscoped_v1(
            RemoteMcapFailureClassificationV1::Presentation,
            RemoteMcapFailureOwnerV1::PostMutation,
            RemoteMcapFailurePhaseV1::PostMutation,
            RemoteMcapFailureRetryabilityV1::NonRetryable,
            label,
        );
        RemoteTerminalCauseV1::PresentationCommitPoisoned(failure)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteTerminalCauseV1 {
    SessionFatal(RemoteMcapFailureV1),
    PresentationCommitPoisoned(RemoteMcapFailureV1),
    OpeningFailure(RemoteMcapFailureV1),
    ExplicitClose,
    ViewerStopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteCleanupTriggerKindV1 {
    ExplicitClose,
    MemoryPressure,
    BackgroundPolicy,
    PageTeardown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteCleanupTriggerV1 {
    kind: RemoteCleanupTriggerKindV1,
    sequence: u64,
}

impl RemoteCleanupTriggerV1 {
    pub(crate) const fn kind_v1(&self) -> RemoteCleanupTriggerKindV1 {
        self.kind
    }

    pub(crate) const fn sequence_v1(&self) -> u64 {
        self.sequence
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteCleanupTokenV1(u64);

impl RemoteCleanupTokenV1 {
    #[cfg(test)]
    const fn get_v1(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteWorkShutdownStateV1 {
    pub(crate) query_closed: bool,
    pub(crate) update_closed: bool,
    pub(crate) refetch_closed: bool,
}

impl RemoteWorkShutdownStateV1 {
    const fn new_v1() -> Self {
        Self {
            query_closed: false,
            update_closed: false,
            refetch_closed: false,
        }
    }

    const fn is_complete_v1(self) -> bool {
        self.query_closed && self.update_closed && self.refetch_closed
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteCleanupStateV1 {
    NotStarted,
    WaitingForCleanup { token: RemoteCleanupTokenV1 },
    Complete { token: RemoteCleanupTokenV1 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteTerminalCleanupProgressV1 {
    WorkClosed,
    CleanupStarted(RemoteCleanupTokenV1),
    WaitingForCleanup(RemoteCleanupTokenV1),
    AlreadyComplete(RemoteCleanupTokenV1),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteTerminalSnapshotV1 {
    pub(crate) sequence: u64,
    pub(crate) cause: RemoteTerminalCauseV1,
    pub(crate) cleanup_triggers: Vec<RemoteCleanupTriggerV1>,
    pub(crate) work_shutdown: RemoteWorkShutdownStateV1,
    pub(crate) cleanup: RemoteCleanupStateV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteTerminalLatchErrorV1 {
    AlreadyTerminal,
    NoTerminalCause,
    CleanupTriggerLimitReached,
    SequenceExhausted,
    CleanupTokenExhausted,
    CleanupNotStarted,
    CleanupTokenMismatch,
    CleanupAlreadyComplete,
}

pub(crate) struct RemoteTerminalLatchV1 {
    next_sequence: u64,
    last_sequence: u64,
    next_cleanup_token: u64,
    cause: Option<RemoteTerminalCauseV1>,
    cleanup_triggers: Vec<RemoteCleanupTriggerV1>,
    work_shutdown: RemoteWorkShutdownStateV1,
    cleanup: RemoteCleanupStateV1,
}

impl RemoteTerminalLatchV1 {
    pub(crate) const fn new_disarmed_v1() -> Self {
        Self {
            next_sequence: 1,
            last_sequence: 0,
            next_cleanup_token: 1,
            cause: None,
            cleanup_triggers: Vec::new(),
            work_shutdown: RemoteWorkShutdownStateV1::new_v1(),
            cleanup: RemoteCleanupStateV1::NotStarted,
        }
    }

    pub(crate) fn record_terminal_cause_v1(
        &mut self,
        cause: RemoteTerminalCauseV1,
    ) -> Result<RemoteTerminalSnapshotV1, RemoteTerminalLatchErrorV1> {
        if self.cause.is_some() {
            return Err(RemoteTerminalLatchErrorV1::AlreadyTerminal);
        }
        self.cause = Some(cause);
        self.advance_sequence_v1()?;
        Ok(self
            .snapshot_v1()
            .expect("terminal cause was just recorded"))
    }

    pub(crate) fn record_cleanup_trigger_v1(
        &mut self,
        kind: RemoteCleanupTriggerKindV1,
    ) -> Result<RemoteCleanupTriggerV1, RemoteTerminalLatchErrorV1> {
        if self.cause.is_none() {
            return Err(RemoteTerminalLatchErrorV1::NoTerminalCause);
        }
        if self.cleanup_triggers.len() >= MAX_TERMINAL_CLEANUP_TRIGGERS {
            return Err(RemoteTerminalLatchErrorV1::CleanupTriggerLimitReached);
        }
        let sequence = self.advance_sequence_v1()?;
        let trigger = RemoteCleanupTriggerV1 { kind, sequence };
        self.cleanup_triggers.push(trigger);
        Ok(trigger)
    }

    pub(crate) fn begin_terminal_cleanup_v1(
        &mut self,
    ) -> Result<RemoteTerminalCleanupProgressV1, RemoteTerminalLatchErrorV1> {
        if self.cause.is_none() {
            return Err(RemoteTerminalLatchErrorV1::NoTerminalCause);
        }

        if !self.work_shutdown.is_complete_v1() {
            self.work_shutdown = RemoteWorkShutdownStateV1 {
                query_closed: true,
                update_closed: true,
                refetch_closed: true,
            };
            return Ok(RemoteTerminalCleanupProgressV1::WorkClosed);
        }

        match self.cleanup {
            RemoteCleanupStateV1::NotStarted => {
                let token = self.allocate_cleanup_token_v1()?;
                self.cleanup = RemoteCleanupStateV1::WaitingForCleanup { token };
                Ok(RemoteTerminalCleanupProgressV1::CleanupStarted(token))
            }
            RemoteCleanupStateV1::WaitingForCleanup { token } => {
                Ok(RemoteTerminalCleanupProgressV1::WaitingForCleanup(token))
            }
            RemoteCleanupStateV1::Complete { token } => {
                Ok(RemoteTerminalCleanupProgressV1::AlreadyComplete(token))
            }
        }
    }

    pub(crate) fn snapshot_v1(&self) -> Option<RemoteTerminalSnapshotV1> {
        self.cause.map(|cause| RemoteTerminalSnapshotV1 {
            sequence: self.last_sequence,
            cause,
            cleanup_triggers: self.cleanup_triggers.clone(),
            work_shutdown: self.work_shutdown,
            cleanup: self.cleanup,
        })
    }

    pub(crate) const fn has_terminal_cause_v1(&self) -> bool {
        self.cause.is_some()
    }

    pub(crate) const fn is_terminal_cleanup_complete_v1(&self) -> bool {
        matches!(self.cleanup, RemoteCleanupStateV1::Complete { .. })
    }

    #[cfg(test)]
    pub(crate) fn complete_cleanup_for_test_v1(
        &mut self,
        expected: RemoteCleanupTokenV1,
    ) -> Result<RemoteTerminalSnapshotV1, RemoteTerminalLatchErrorV1> {
        let token = match self.cleanup {
            RemoteCleanupStateV1::NotStarted => {
                return Err(RemoteTerminalLatchErrorV1::CleanupNotStarted);
            }
            RemoteCleanupStateV1::WaitingForCleanup { token } if token == expected => token,
            RemoteCleanupStateV1::WaitingForCleanup { .. } => {
                return Err(RemoteTerminalLatchErrorV1::CleanupTokenMismatch);
            }
            RemoteCleanupStateV1::Complete { .. } => {
                return Err(RemoteTerminalLatchErrorV1::CleanupAlreadyComplete);
            }
        };
        self.cleanup = RemoteCleanupStateV1::Complete { token };
        Ok(self.snapshot_v1().expect("terminal cause exists"))
    }

    fn advance_sequence_v1(&mut self) -> Result<u64, RemoteTerminalLatchErrorV1> {
        let sequence = self.next_sequence;
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or(RemoteTerminalLatchErrorV1::SequenceExhausted)?;
        self.last_sequence = sequence;
        Ok(sequence)
    }

    fn allocate_cleanup_token_v1(
        &mut self,
    ) -> Result<RemoteCleanupTokenV1, RemoteTerminalLatchErrorV1> {
        let token = self.next_cleanup_token;
        self.next_cleanup_token = self
            .next_cleanup_token
            .checked_add(1)
            .ok_or(RemoteTerminalLatchErrorV1::CleanupTokenExhausted)?;
        Ok(RemoteCleanupTokenV1(token))
    }
}

impl ConsumerStorageFreeV1 for str {}
impl ConsumerStorageFreeV1 for RemoteMcapFailureClassificationV1 {}
impl ConsumerStorageFreeV1 for RemoteMcapFailureScopeV1 {}
impl ConsumerStorageFreeV1 for RemoteMcapFailurePhaseV1 {}
impl ConsumerStorageFreeV1 for RemoteMcapFailureRetryabilityV1 {}
impl ConsumerStorageFreeV1 for RemoteMcapDemandOwnerV1 {}
impl ConsumerStorageFreeV1 for RemoteMcapFailureOwnerV1 {}
impl ConsumerStorageFreeV1 for RemoteMcapFailureV1
where
    RemoteMcapFailureClassificationV1: ConsumerStorageFreeV1,
    Option<RemoteMcapFailureScopeV1>: ConsumerStorageFreeV1,
    RemoteMcapFailureOwnerV1: ConsumerStorageFreeV1,
    RemoteMcapFailurePhaseV1: ConsumerStorageFreeV1,
    RemoteMcapFailureRetryabilityV1: ConsumerStorageFreeV1,
    &'static str: ConsumerStorageFreeV1,
{
}
impl ConsumerStorageFreeV1 for RemotePrefetchFailureReasonV1 {}
impl ConsumerStorageFreeV1 for RemotePhaseBFailureReasonV1 {}
impl ConsumerStorageFreeV1 for RemotePostMutationFailureReasonV1 {}
impl ConsumerStorageFreeV1 for RemotePreMutationSeekTransitionV1 where
    RemoteMcapFailureV1: ConsumerStorageFreeV1
{
}
impl ConsumerStorageFreeV1 for RemoteFailureSourceKindV1 {}
impl ConsumerStorageFreeV1 for RemoteTerminalCauseV1 where RemoteMcapFailureV1: ConsumerStorageFreeV1
{}
impl ConsumerStorageFreeV1 for RemoteCleanupTriggerKindV1 {}
impl ConsumerStorageFreeV1 for RemoteCleanupTriggerV1
where
    RemoteCleanupTriggerKindV1: ConsumerStorageFreeV1,
    u64: ConsumerStorageFreeV1,
{
}
impl ConsumerStorageFreeV1 for RemoteCleanupTokenV1 {}
impl ConsumerStorageFreeV1 for RemoteWorkShutdownStateV1 where bool: ConsumerStorageFreeV1 {}
impl ConsumerStorageFreeV1 for RemoteCleanupStateV1 where
    Option<RemoteCleanupTokenV1>: ConsumerStorageFreeV1
{
}
impl ConsumerStorageFreeV1 for RemoteTerminalCleanupProgressV1 where
    Option<RemoteCleanupTokenV1>: ConsumerStorageFreeV1
{
}
impl ConsumerStorageFreeV1 for RemoteTerminalSnapshotV1
where
    u64: ConsumerStorageFreeV1,
    RemoteTerminalCauseV1: ConsumerStorageFreeV1,
    Vec<RemoteCleanupTriggerV1>: ConsumerStorageFreeV1,
    RemoteWorkShutdownStateV1: ConsumerStorageFreeV1,
    RemoteCleanupStateV1: ConsumerStorageFreeV1,
{
}
impl ConsumerStorageFreeV1 for RemoteTerminalLatchErrorV1 {}
impl ConsumerStorageFreeV1 for RemoteTerminalLatchV1
where
    u64: ConsumerStorageFreeV1,
    Option<RemoteTerminalCauseV1>: ConsumerStorageFreeV1,
    Vec<RemoteCleanupTriggerV1>: ConsumerStorageFreeV1,
    RemoteWorkShutdownStateV1: ConsumerStorageFreeV1,
    RemoteCleanupStateV1: ConsumerStorageFreeV1,
{
}

#[cfg(test)]
mod tests {
    use re_entity_db::{EntityDb, StoreBundle};
    use re_query::StorageEngine;
    use re_viewer_context::StoreHub;

    use super::*;

    fn failure_for_test_v1(
        classification: RemoteMcapFailureClassificationV1,
        scope: Option<RemoteMcapFailureScopeV1>,
        owner: RemoteMcapFailureOwnerV1,
        phase: RemoteMcapFailurePhaseV1,
        retryability: RemoteMcapFailureRetryabilityV1,
    ) -> RemoteMcapFailureV1 {
        RemoteMcapFailureV1::new_v1(classification, scope, owner, phase, retryability, "test")
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
    fn top_level_classification_and_three_scopes_are_frozen() {
        let classifications = [
            RemoteMcapFailureClassificationV1::Admission,
            RemoteMcapFailureClassificationV1::FormatSniff,
            RemoteMcapFailureClassificationV1::Network,
            RemoteMcapFailureClassificationV1::Validation,
            RemoteMcapFailureClassificationV1::Decode,
            RemoteMcapFailureClassificationV1::IdentifierAdmission,
            RemoteMcapFailureClassificationV1::Presentation,
            RemoteMcapFailureClassificationV1::Resource,
            RemoteMcapFailureClassificationV1::Closed,
        ];
        for classification in classifications {
            let failure = failure_for_test_v1(
                classification,
                None,
                RemoteMcapFailureOwnerV1::MetadataOpening,
                RemoteMcapFailurePhaseV1::MetadataOpening,
                RemoteMcapFailureRetryabilityV1::NonRetryable,
            );
            assert_eq!(failure.classification, classification);
        }

        assert_eq!(
            RemoteMcapFailureV1::optional_work_v1(
                RemoteMcapFailureClassificationV1::Network,
                RemoteMcapFailureOwnerV1::Prefetch,
                RemoteMcapFailurePhaseV1::Prefetch,
                RemoteMcapFailureRetryabilityV1::Retryable,
                "optional",
            )
            .scope,
            Some(RemoteMcapFailureScopeV1::OptionalWork)
        );
        assert_eq!(
            RemoteMcapFailureV1::current_seek_v1(
                RemoteMcapFailureClassificationV1::Network,
                RemoteMcapFailureOwnerV1::CurrentSeek,
                RemoteMcapFailurePhaseV1::PhaseBSeek,
                RemoteMcapFailureRetryabilityV1::Retryable,
                "current",
            )
            .scope,
            Some(RemoteMcapFailureScopeV1::CurrentSeek)
        );
        assert_eq!(
            RemoteMcapFailureV1::session_fatal_v1(
                RemoteMcapFailureClassificationV1::Validation,
                RemoteMcapFailureOwnerV1::CurrentSeek,
                RemoteMcapFailurePhaseV1::PhaseBSeek,
                "fatal",
            )
            .scope,
            Some(RemoteMcapFailureScopeV1::SessionFatal)
        );
    }

    #[test]
    fn metadata_opening_exhaustion_is_opening_scoped_not_session_fatal() {
        let failure = RemoteMcapFailureClassifierV1::classify_metadata_opening_exhaustion_v1();

        assert_eq!(failure.owner, RemoteMcapFailureOwnerV1::MetadataOpening);
        assert_eq!(failure.phase, RemoteMcapFailurePhaseV1::MetadataOpening);
        assert_eq!(failure.scope, None);
        assert_eq!(
            failure.retryability,
            RemoteMcapFailureRetryabilityV1::NonRetryable
        );
        assert_ne!(failure.scope, Some(RemoteMcapFailureScopeV1::SessionFatal));
        assert_ne!(failure.scope, Some(RemoteMcapFailureScopeV1::CurrentSeek));
    }

    #[test]
    fn phase_b_exhaustions_are_classified_by_demand_owner() {
        for owner in [
            RemoteMcapDemandOwnerV1::CurrentSeek,
            RemoteMcapDemandOwnerV1::Refetch,
            RemoteMcapDemandOwnerV1::PostMutation,
        ] {
            for reason in [
                RemotePhaseBFailureReasonV1::AttemptExhausted,
                RemotePhaseBFailureReasonV1::RangeCountExhausted,
                RemotePhaseBFailureReasonV1::ActiveVisibleDeadlineExhausted,
            ] {
                let failure = RemoteMcapFailureClassifierV1::classify_phase_b_v1(owner, reason);
                assert_eq!(failure.owner, owner.into_failure_owner_v1());
                assert_eq!(failure.scope, Some(RemoteMcapFailureScopeV1::CurrentSeek));
                assert_eq!(
                    failure.retryability,
                    RemoteMcapFailureRetryabilityV1::NonRetryable
                );
            }

            let timeout = RemoteMcapFailureClassifierV1::classify_phase_b_v1(
                owner,
                RemotePhaseBFailureReasonV1::NetworkTimeout,
            );
            assert_eq!(timeout.owner, owner.into_failure_owner_v1());
            assert_eq!(
                timeout.retryability,
                RemoteMcapFailureRetryabilityV1::Retryable
            );

            let http_416 = RemoteMcapFailureClassifierV1::classify_phase_b_v1(
                owner,
                RemotePhaseBFailureReasonV1::Http416,
            );
            assert_eq!(http_416.owner, owner.into_failure_owner_v1());
            assert_eq!(http_416.scope, Some(RemoteMcapFailureScopeV1::CurrentSeek));
            assert_eq!(
                http_416.retryability,
                RemoteMcapFailureRetryabilityV1::NonRetryable
            );
        }
    }

    #[test]
    fn fatal_phase_b_reasons_never_enter_retryable_or_optional_work() {
        let fatal_reasons = [
            RemotePhaseBFailureReasonV1::ValidatorViolation,
            RemotePhaseBFailureReasonV1::LengthChanged,
            RemotePhaseBFailureReasonV1::Http412,
            RemotePhaseBFailureReasonV1::ObjectChanged,
            RemotePhaseBFailureReasonV1::IndexCorruption,
        ];

        for owner in [
            RemoteMcapDemandOwnerV1::CurrentSeek,
            RemoteMcapDemandOwnerV1::Refetch,
            RemoteMcapDemandOwnerV1::PostMutation,
        ] {
            for reason in fatal_reasons {
                let failure = RemoteMcapFailureClassifierV1::classify_phase_b_v1(owner, reason);
                assert_eq!(failure.scope, Some(RemoteMcapFailureScopeV1::SessionFatal));
                assert_eq!(
                    failure.retryability,
                    RemoteMcapFailureRetryabilityV1::NonRetryable
                );
                assert_ne!(failure.scope, Some(RemoteMcapFailureScopeV1::CurrentSeek));
                assert_ne!(failure.scope, Some(RemoteMcapFailureScopeV1::OptionalWork));
            }
        }
    }

    #[test]
    fn prefetch_fatal_reasons_cannot_be_downgraded_by_optional_demand() {
        for reason in [
            RemotePrefetchFailureReasonV1::ObjectChanged,
            RemotePrefetchFailureReasonV1::ValidatorViolation,
            RemotePrefetchFailureReasonV1::IndexCorruption,
        ] {
            let failure = RemoteMcapFailureClassifierV1::classify_prefetch_v1(reason);
            assert_eq!(failure.scope, Some(RemoteMcapFailureScopeV1::SessionFatal));
            assert_eq!(
                failure.retryability,
                RemoteMcapFailureRetryabilityV1::NonRetryable
            );
        }

        assert_eq!(
            RemoteMcapFailureClassifierV1::classify_prefetch_v1(
                RemotePrefetchFailureReasonV1::TransportUnavailable,
            )
            .scope,
            Some(RemoteMcapFailureScopeV1::OptionalWork)
        );
    }

    #[test]
    fn pre_mutation_seek_failure_rolls_back_or_fails_initial_without_new_retry_operation() {
        let rollback = RemoteMcapFailureClassifierV1::classify_pre_mutation_seek_v1(
            true,
            RemotePhaseBFailureReasonV1::AttemptExhausted,
        );
        assert!(matches!(
            rollback,
            RemotePreMutationSeekTransitionV1::RollbackToCommittedCursor(_)
        ));

        let initial = RemoteMcapFailureClassifierV1::classify_pre_mutation_seek_v1(
            false,
            RemotePhaseBFailureReasonV1::AttemptExhausted,
        );
        assert!(matches!(
            initial,
            RemotePreMutationSeekTransitionV1::InitialPresentationFailed(_)
        ));

        for reason in [
            RemotePhaseBFailureReasonV1::ValidatorViolation,
            RemotePhaseBFailureReasonV1::LengthChanged,
            RemotePhaseBFailureReasonV1::Http412,
            RemotePhaseBFailureReasonV1::ObjectChanged,
            RemotePhaseBFailureReasonV1::IndexCorruption,
        ] {
            let with_cursor =
                RemoteMcapFailureClassifierV1::classify_pre_mutation_seek_v1(true, reason);
            assert!(matches!(
                with_cursor,
                RemotePreMutationSeekTransitionV1::SessionFatal(failure)
                    if failure.scope == Some(RemoteMcapFailureScopeV1::SessionFatal)
            ));

            let without_cursor =
                RemoteMcapFailureClassifierV1::classify_pre_mutation_seek_v1(false, reason);
            assert!(matches!(
                without_cursor,
                RemotePreMutationSeekTransitionV1::SessionFatal(failure)
                    if failure.scope == Some(RemoteMcapFailureScopeV1::SessionFatal)
            ));
        }
    }

    #[test]
    fn post_insertion_failure_is_presentation_commit_poisoned() {
        for reason in [
            RemotePostMutationFailureReasonV1::Insertion,
            RemotePostMutationFailureReasonV1::ResidencyAck,
            RemotePostMutationFailureReasonV1::PresentationCommit,
        ] {
            let cause = RemoteMcapFailureClassifierV1::classify_post_mutation_v1(reason);
            assert!(matches!(
                cause,
                RemoteTerminalCauseV1::PresentationCommitPoisoned(failure)
                    if failure.classification == RemoteMcapFailureClassificationV1::Presentation
                        && failure.owner == RemoteMcapFailureOwnerV1::PostMutation
                        && failure.scope.is_none()
            ));
        }
    }

    #[test]
    fn terminal_cause_is_first_write_wins_and_later_events_are_triggers() {
        let fatal = RemoteMcapFailureClassifierV1::classify_prefetch_v1(
            RemotePrefetchFailureReasonV1::IndexCorruption,
        );
        let mut latch = RemoteTerminalLatchV1::new_disarmed_v1();

        let first = latch
            .record_terminal_cause_v1(RemoteTerminalCauseV1::SessionFatal(fatal))
            .expect("first terminal cause");
        assert_eq!(first.sequence, 1);
        assert!(matches!(
            first.cause,
            RemoteTerminalCauseV1::SessionFatal(_)
        ));

        assert_eq!(
            latch.record_terminal_cause_v1(RemoteTerminalCauseV1::ExplicitClose),
            Err(RemoteTerminalLatchErrorV1::AlreadyTerminal)
        );

        let close_trigger = latch
            .record_cleanup_trigger_v1(RemoteCleanupTriggerKindV1::ExplicitClose)
            .expect("close trigger");
        assert_eq!(close_trigger.sequence_v1(), 2);
        let pressure_trigger = latch
            .record_cleanup_trigger_v1(RemoteCleanupTriggerKindV1::MemoryPressure)
            .expect("pressure trigger");
        assert_eq!(pressure_trigger.sequence_v1(), 3);

        let snapshot = latch.snapshot_v1().expect("snapshot");
        assert!(matches!(
            snapshot.cause,
            RemoteTerminalCauseV1::SessionFatal(_)
        ));
        assert_eq!(snapshot.cleanup_triggers.len(), 2);
        assert_eq!(
            snapshot.cleanup_triggers[0].kind_v1(),
            RemoteCleanupTriggerKindV1::ExplicitClose
        );
        assert_eq!(
            snapshot.cleanup_triggers[1].kind_v1(),
            RemoteCleanupTriggerKindV1::MemoryPressure
        );
    }

    #[test]
    fn terminal_latch_closes_work_before_tokenized_cleanup() {
        let fatal = RemoteMcapFailureClassifierV1::classify_prefetch_v1(
            RemotePrefetchFailureReasonV1::ObjectChanged,
        );
        let mut latch = RemoteTerminalLatchV1::new_disarmed_v1();
        latch
            .record_terminal_cause_v1(RemoteTerminalCauseV1::SessionFatal(fatal))
            .expect("record cause");

        assert!(!latch.is_terminal_cleanup_complete_v1());
        assert_eq!(
            latch.begin_terminal_cleanup_v1(),
            Ok(RemoteTerminalCleanupProgressV1::WorkClosed)
        );
        let after_work_shutdown = latch.snapshot_v1().expect("snapshot");
        assert!(after_work_shutdown.work_shutdown.query_closed);
        assert!(after_work_shutdown.work_shutdown.update_closed);
        assert!(after_work_shutdown.work_shutdown.refetch_closed);
        assert_eq!(
            after_work_shutdown.cleanup,
            RemoteCleanupStateV1::NotStarted
        );
        assert!(!latch.is_terminal_cleanup_complete_v1());

        let token = match latch.begin_terminal_cleanup_v1() {
            Ok(RemoteTerminalCleanupProgressV1::CleanupStarted(token)) => token,
            other => panic!("expected cleanup start, got {other:?}"),
        };
        assert!(!latch.is_terminal_cleanup_complete_v1());

        assert_eq!(
            latch.begin_terminal_cleanup_v1(),
            Ok(RemoteTerminalCleanupProgressV1::WaitingForCleanup(token))
        );
        assert!(!latch.is_terminal_cleanup_complete_v1());

        latch
            .complete_cleanup_for_test_v1(token)
            .expect("tokenized cleanup completes");
        assert!(latch.is_terminal_cleanup_complete_v1());
    }

    #[test]
    fn cleanup_completion_is_token_checked_and_not_repeatable() {
        let fatal = RemoteMcapFailureClassifierV1::classify_prefetch_v1(
            RemotePrefetchFailureReasonV1::IndexCorruption,
        );
        let mut latch = RemoteTerminalLatchV1::new_disarmed_v1();
        latch
            .record_terminal_cause_v1(RemoteTerminalCauseV1::SessionFatal(fatal))
            .expect("record cause");
        assert_eq!(
            latch.begin_terminal_cleanup_v1(),
            Ok(RemoteTerminalCleanupProgressV1::WorkClosed)
        );
        let token = match latch.begin_terminal_cleanup_v1() {
            Ok(RemoteTerminalCleanupProgressV1::CleanupStarted(token)) => token,
            other => panic!("expected cleanup start, got {other:?}"),
        };

        assert_eq!(
            latch.complete_cleanup_for_test_v1(RemoteCleanupTokenV1(token.get_v1() + 1)),
            Err(RemoteTerminalLatchErrorV1::CleanupTokenMismatch)
        );
        latch
            .complete_cleanup_for_test_v1(token)
            .expect("correct token");
        assert_eq!(
            latch.complete_cleanup_for_test_v1(token),
            Err(RemoteTerminalLatchErrorV1::CleanupAlreadyComplete)
        );
    }

    #[test]
    fn non_remote_sources_do_not_observe_or_accept_remote_cleanup() {
        let fatal = RemoteMcapFailureClassifierV1::classify_prefetch_v1(
            RemotePrefetchFailureReasonV1::ObjectChanged,
        );
        for source in [
            RemoteFailureSourceKindV1::Legacy,
            RemoteFailureSourceKindV1::LogChannel,
            RemoteFailureSourceKindV1::GrpcMessageProxy,
            RemoteFailureSourceKindV1::Redap,
            RemoteFailureSourceKindV1::Local,
            RemoteFailureSourceKindV1::Native,
        ] {
            assert!(!source.observes_remote_failure_v1());
            assert!(!source.consumes_remote_budget_v1());
            assert!(!source.accepts_remote_terminal_cleanup_v1());
            assert_eq!(source.route_failure_v1(fatal), None);
        }

        assert!(RemoteFailureSourceKindV1::RemoteMcap.observes_remote_failure_v1());
        assert!(RemoteFailureSourceKindV1::RemoteMcap.consumes_remote_budget_v1());
        assert!(RemoteFailureSourceKindV1::RemoteMcap.accepts_remote_terminal_cleanup_v1());
        assert_eq!(
            RemoteFailureSourceKindV1::RemoteMcap.route_failure_v1(fatal),
            Some(fatal)
        );
    }

    #[test]
    fn remote_failure_graph_is_storage_free_by_structure() {
        assert_not_storage_free_v1!(EntityDb);
        assert_not_storage_free_v1!(StoreBundle);
        assert_not_storage_free_v1!(StoreHub);
        assert_not_storage_free_v1!(StorageEngine);

        assert_storage_free_v1::<RemoteMcapFailureV1>();
        assert_storage_free_v1::<RemotePreMutationSeekTransitionV1>();
        assert_storage_free_v1::<RemoteTerminalCauseV1>();
        assert_storage_free_v1::<RemoteTerminalSnapshotV1>();
        assert_storage_free_v1::<RemoteTerminalLatchV1>();
    }
}
