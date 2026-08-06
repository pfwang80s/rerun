//! Production-disarmed retry ownership for remote-MCAP metadata Range requests.
//!
//! This module deliberately stops at the transport/controller seam. It does not install a Viewer
//! source, page listener, seek/prefetch/GC policy, or production limits profile.

use std::cell::Cell;
use std::fmt;
use std::marker::PhantomData;
use std::num::NonZeroU64;
use std::ops::Range;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};

use pin_project_lite::pin_project;

use crate::chrome_range::{ChromeRangeError, ChromeRangeRequest, PreparedChromeRangeIdentity};
use crate::remote_limits::{
    CommittedRangeAttemptBurn, MetadataOpeningActiveVisibleDeadlineOwner,
    MetadataOpeningDeadlineState, MetadataOpeningRangeBudgetOwner, OperationAccountingScope,
    RangeAttemptAccountingBinding, RangeAttemptBurnError, RangeResponseAccountingScope,
    RangeRetryAttemptBudget, ScopeAccountingError, SourceAccountingScope, WorkUnitAccountingScope,
};

static NEXT_RANGE_OPERATION_NONCE: AtomicU64 = AtomicU64::new(1);

/// A complete live source/session/representation identity supplied by the future source registry.
///
/// The generic fields avoid inventing a second public identity space before `OpenSourceToken` and
/// `RemoteMcapSessionId` land. Diagnostics never expose any field.
#[derive(Clone, PartialEq, Eq)]
pub struct RemoteRangeLiveIdentity<Source, Session, Representation> {
    source: Source,
    session: Session,
    representation: Representation,
}

impl<Source, Session, Representation> RemoteRangeLiveIdentity<Source, Session, Representation> {
    pub fn new(source: Source, session: Session, representation: Representation) -> Self {
        Self {
            source,
            session,
            representation,
        }
    }
}

impl<Source, Session, Representation> fmt::Debug
    for RemoteRangeLiveIdentity<Source, Session, Representation>
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RemoteRangeLiveIdentity(<opaque>)")
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct RangeOperationNonce(NonZeroU64);

impl fmt::Debug for RangeOperationNonce {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RangeOperationNonce(<opaque>)")
    }
}

fn allocate_operation_nonce() -> Result<RangeOperationNonce, RetryProtocolError> {
    let mut current = NEXT_RANGE_OPERATION_NONCE.load(Ordering::Relaxed);
    loop {
        let Some(nonce) = NonZeroU64::new(current) else {
            return Err(RetryProtocolError::IdentityExhausted);
        };
        let next = current.checked_add(1).unwrap_or(0);
        match NEXT_RANGE_OPERATION_NONCE.compare_exchange_weak(
            current,
            next,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return Ok(RangeOperationNonce(nonce)),
            Err(observed) => current = observed,
        }
    }
}

/// One never-reused attempt identity within an exact Range operation.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct RangeAttemptToken {
    operation: RangeOperationNonce,
    attempt: NonZeroU64,
}

impl RangeAttemptToken {
    pub const fn ordinal(self) -> u64 {
        self.attempt.get()
    }
}

impl fmt::Debug for RangeAttemptToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RangeAttemptToken(<opaque>)")
    }
}

/// A callback identity frozen when one physical Fetch starts.
#[derive(Clone, PartialEq, Eq)]
pub struct RangeAttemptCallbackIdentity<Source, Session, Representation, Demand> {
    live: RemoteRangeLiveIdentity<Source, Session, Representation>,
    operation: RangeOperationNonce,
    attempt: RangeAttemptToken,
    demand: Demand,
}

impl<Source, Session, Representation, Demand> fmt::Debug
    for RangeAttemptCallbackIdentity<Source, Session, Representation, Demand>
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RangeAttemptCallbackIdentity(<opaque>)")
    }
}

/// A terminal condition owned by this narrow source/session seam.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetrySessionTerminal {
    ObjectChanged,
    ExplicitlyClosed,
    PageTerminated,
}

/// Result of trying to publish a matching object-change failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectChangeDisposition {
    Latched,
    ExistingTerminalPreserved,
    StaleIdentity,
    StaleAttempt,
}

/// Source/session fatal latch shared by its metadata Range operations.
pub struct RemoteRangeSessionGate<Source, Session, Representation> {
    live: RemoteRangeLiveIdentity<Source, Session, Representation>,
    can_refetch_chunks: bool,
    terminal: Option<RetrySessionTerminal>,
}

impl<Source, Session, Representation> fmt::Debug
    for RemoteRangeSessionGate<Source, Session, Representation>
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteRangeSessionGate")
            .field("live", &"<opaque>")
            .field("can_refetch_chunks", &self.can_refetch_chunks)
            .field("terminal", &self.terminal)
            .finish()
    }
}

impl<Source: PartialEq, Session: PartialEq, Representation: PartialEq>
    RemoteRangeSessionGate<Source, Session, Representation>
{
    fn new(live: RemoteRangeLiveIdentity<Source, Session, Representation>) -> Self {
        Self {
            live,
            can_refetch_chunks: true,
            terminal: None,
        }
    }

    pub const fn can_refetch_chunks(&self) -> bool {
        self.can_refetch_chunks
    }

    pub const fn terminal(&self) -> Option<RetrySessionTerminal> {
        self.terminal
    }

    fn latch_object_changed(
        &mut self,
        callback_live: &RemoteRangeLiveIdentity<Source, Session, Representation>,
    ) -> ObjectChangeDisposition {
        if callback_live != &self.live {
            return ObjectChangeDisposition::StaleIdentity;
        }
        if self.terminal.is_some() {
            return ObjectChangeDisposition::ExistingTerminalPreserved;
        }

        // Ordering is normative: refetch capability is revoked before the fatal latch is visible.
        self.can_refetch_chunks = false;
        self.terminal = Some(RetrySessionTerminal::ObjectChanged);
        ObjectChangeDisposition::Latched
    }

    pub fn close(&mut self) {
        self.can_refetch_chunks = false;
        self.terminal
            .get_or_insert(RetrySessionTerminal::ExplicitlyClosed);
    }

    fn page_terminate(&mut self) {
        self.can_refetch_chunks = false;
        self.terminal
            .get_or_insert(RetrySessionTerminal::PageTerminated);
    }
}

/// Low-cardinality failure returned to the metadata-opening owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetadataOpeningFailureSignal {
    NonRetryable(ChromeRangeError),
    RetryAttemptsExhausted,
    MetadataRangeLimitExhausted,
    ActiveVisibleDeadlineExpired,
    ResourceLimit,
}

/// An admission/protocol condition that does not itself terminalize metadata opening.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetryProtocolError {
    IdentityExhausted,
    OperationNotInitial,
    OperationNotRetryPending,
    OperationAlreadyTerminal,
    RetryNotYetEligible,
    Hidden,
    AlreadyHidden,
    PageTerminated,
    StaleExecution,
    StaleTurn,
    TurnAlreadyProjected,
    TurnAlreadyClaimed,
    AttemptMismatch,
    ActiveAttemptRequired,
    AttemptOwnerDepleted,
    TimeoutNotConfirmed,
}

impl fmt::Display for RetryProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::IdentityExhausted => "remote Range retry identity exhausted",
            Self::OperationNotInitial => "remote Range operation is not initial",
            Self::OperationNotRetryPending => "remote Range operation is not retry-pending",
            Self::OperationAlreadyTerminal => "remote Range operation is terminal",
            Self::RetryNotYetEligible => "remote Range retry is not eligible in this turn",
            Self::Hidden => "remote Range retry is suspended while hidden",
            Self::AlreadyHidden => "remote Range retry execution is already hidden",
            Self::PageTerminated => "remote Range retry execution is terminated",
            Self::StaleExecution => "remote Range retry execution identity is stale",
            Self::StaleTurn => "remote Range retry control-turn identity is stale",
            Self::TurnAlreadyProjected => "remote Range retry turn already has a live projection",
            Self::TurnAlreadyClaimed => "remote Range retry turn is already claimed",
            Self::AttemptMismatch => "remote Range retry attempt identity does not match",
            Self::ActiveAttemptRequired => "remote Range retry active attempt owner is required",
            Self::AttemptOwnerDepleted => "remote Range retry attempt owner is depleted",
            Self::TimeoutNotConfirmed => "remote Range timeout lacks active-visible confirmation",
        })
    }
}

impl std::error::Error for RetryProtocolError {}

/// Failure to start one physical attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeAttemptStartError {
    Protocol(RetryProtocolError),
    OpeningFailed(MetadataOpeningFailureSignal),
    ObjectChanged(ObjectChangeDisposition),
    AccountingAncestryMismatch,
}

impl fmt::Display for RangeAttemptStartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Protocol(error) => error.fmt(formatter),
            Self::OpeningFailed(signal) => {
                write!(formatter, "remote metadata opening failed: {signal:?}")
            }
            Self::ObjectChanged(disposition) => {
                write!(formatter, "remote metadata object changed: {disposition:?}")
            }
            Self::AccountingAncestryMismatch => {
                formatter.write_str("remote Range accounting ancestry does not match")
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreparedAttemptMismatchKind {
    ExactRange,
    AbortController,
    AccountingAncestry,
}

/// A zero-burn prepared-owner rejection.
///
/// Both owners are returned so cleanup remains explicit; dropping this value aborts the returned
/// controller through its normal owner and drops the prepared transport without changing the
/// operation or its retry-turn projection.
pub(crate) struct RejectedPreparedAttempt<Prepared, Controller> {
    kind: PreparedAttemptMismatchKind,
    prepared: Prepared,
    controller: Controller,
}

impl<Prepared, Controller> RejectedPreparedAttempt<Prepared, Controller> {
    fn kind(&self) -> PreparedAttemptMismatchKind {
        self.kind
    }

    fn into_owners(self) -> (Prepared, Controller) {
        (self.prepared, self.controller)
    }
}

impl<Prepared, Controller> fmt::Debug for RejectedPreparedAttempt<Prepared, Controller> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RejectedPreparedAttempt")
            .field("kind", &self.kind)
            .field("prepared", &"<retained>")
            .field("controller", &"<retained>")
            .finish()
    }
}

pub(crate) enum RangeAttemptStartFailure<Prepared, Controller> {
    Request(RangeAttemptStartError),
    PreparedMismatch(RejectedPreparedAttempt<Prepared, Controller>),
}

impl<Prepared, Controller> fmt::Debug for RangeAttemptStartFailure<Prepared, Controller> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request(error) => formatter.debug_tuple("Request").field(error).finish(),
            Self::PreparedMismatch(rejected) => rejected.fmt(formatter),
        }
    }
}

impl<Prepared, Controller> From<RangeAttemptStartError>
    for RangeAttemptStartFailure<Prepared, Controller>
{
    fn from(error: RangeAttemptStartError) -> Self {
        Self::Request(error)
    }
}

impl std::error::Error for RangeAttemptStartError {}

impl From<RetryProtocolError> for RangeAttemptStartError {
    fn from(error: RetryProtocolError) -> Self {
        Self::Protocol(error)
    }
}

fn map_burn_failure(error: RangeAttemptBurnError) -> MetadataOpeningFailureSignal {
    match error {
        RangeAttemptBurnError::AttemptLimitExhausted => {
            MetadataOpeningFailureSignal::RetryAttemptsExhausted
        }
        RangeAttemptBurnError::MetadataOpeningRangeLimitExhausted => {
            MetadataOpeningFailureSignal::MetadataRangeLimitExhausted
        }
        RangeAttemptBurnError::ArithmeticOverflow
        | RangeAttemptBurnError::AccountingUnavailable(_) => {
            MetadataOpeningFailureSignal::ResourceLimit
        }
    }
}

pub(crate) trait RangeAttemptAbortController {
    fn abort(&mut self);
    fn finish(&mut self);
}

pub(crate) trait RangeAttemptAbortFactory {
    type Controller: RangeAttemptAbortController;

    fn create(&mut self) -> Result<Self::Controller, ChromeRangeError>;
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct AbstractExecutionIssuerId(NonZeroU64);

impl fmt::Debug for AbstractExecutionIssuerId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AbstractExecutionIssuerId(<opaque>)")
    }
}

/// Opaque proof that an external page-execution issuer currently considers execution visible.
///
/// MCAP-015 has no production constructor for this type. The test-only issuer below exercises the
/// seam; MCAP-065 will become the sole production issuer.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct ActiveVisibleExecutionBinding {
    issuer: AbstractExecutionIssuerId,
    epoch: NonZeroU64,
    baseline: NonZeroU64,
}

impl fmt::Debug for ActiveVisibleExecutionBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ActiveVisibleExecutionBinding(<opaque>)")
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct HiddenExecutionEvidence {
    previous: ActiveVisibleExecutionBinding,
    suspension: NonZeroU64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct VisibleExecutionRebindEvidence {
    suspension: NonZeroU64,
    current: ActiveVisibleExecutionBinding,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct PageTerminationEvidence {
    issuer: AbstractExecutionIssuerId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TurnProjectionState {
    Available,
    Projected,
    Claimed,
}

/// One externally issued visible control turn.
///
/// It may lend at most one live retry projection. A projection whose preflight fails is returned;
/// after a physical attempt claims it, the turn remains permanently consumed across all sources.
/// The external issuer must allocate `sequence` monotonically without reuse for its entire
/// lifetime, including across hide/resume epochs. A per-visible-epoch sequence is invalid because
/// retry eligibility intentionally spans those epochs.
pub(crate) struct ExternalVisibleControlTurn {
    binding: ActiveVisibleExecutionBinding,
    sequence: NonZeroU64,
    projection: Cell<TurnProjectionState>,
}

impl fmt::Debug for ExternalVisibleControlTurn {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExternalVisibleControlTurn")
            .field("binding", &"<opaque>")
            .field("projection", &self.projection.get())
            .finish_non_exhaustive()
    }
}

/// One non-cloneable projection of an externally issued control turn.
pub(crate) struct VisibleRetryTurnPermit<'turn> {
    binding: ActiveVisibleExecutionBinding,
    sequence: NonZeroU64,
    projection: &'turn Cell<TurnProjectionState>,
    claimed: bool,
}

impl fmt::Debug for VisibleRetryTurnPermit<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VisibleRetryTurnPermit")
            .field("execution", &"<opaque>")
            .field("claimed", &self.claimed)
            .finish_non_exhaustive()
    }
}

impl VisibleRetryTurnPermit<'_> {
    fn claim(&mut self) -> Result<(), RetryProtocolError> {
        if self.claimed || self.projection.get() != TurnProjectionState::Projected {
            return Err(RetryProtocolError::TurnAlreadyClaimed);
        }
        self.projection.set(TurnProjectionState::Claimed);
        self.claimed = true;
        Ok(())
    }
}

impl Drop for VisibleRetryTurnPermit<'_> {
    fn drop(&mut self) {
        if !self.claimed && self.projection.get() == TurnProjectionState::Projected {
            self.projection.set(TurnProjectionState::Available);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExecutionState {
    Visible(ActiveVisibleExecutionBinding),
    Hidden {
        issuer: AbstractExecutionIssuerId,
        suspension: NonZeroU64,
    },
    Terminated {
        issuer: AbstractExecutionIssuerId,
    },
}

impl fmt::Debug for ExecutionState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Visible(_) => "Visible(<opaque>)",
            Self::Hidden { .. } => "Hidden(<opaque>)",
            Self::Terminated { .. } => "Terminated(<opaque>)",
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ActiveVisibleTimeoutEvidence {
    binding: ActiveVisibleExecutionBinding,
    attempt: RangeAttemptToken,
}

/// A matching timeout confirmed by both the external execution issuer and source deadline owner.
#[derive(Clone, Copy, PartialEq, Eq)]
struct ConfirmedActiveVisibleTimeout {
    binding: ActiveVisibleExecutionBinding,
    attempt: RangeAttemptToken,
}

/// Source-owned metadata retry limits and abstract visible execution seam.
pub struct MetadataOpeningRetryCoordinator<Source, Session, Representation> {
    session_gate: RemoteRangeSessionGate<Source, Session, Representation>,
    ranges: MetadataOpeningRangeBudgetOwner,
    deadline: MetadataOpeningActiveVisibleDeadlineOwner,
    execution: ExecutionState,
    last_observed_control_turn: u64,
}

impl<Source, Session, Representation> fmt::Debug
    for MetadataOpeningRetryCoordinator<Source, Session, Representation>
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MetadataOpeningRetryCoordinator")
            .field("session_gate", &"<opaque>")
            .field("ranges", &self.ranges)
            .field("deadline", &self.deadline)
            .field("execution", &self.execution)
            .finish_non_exhaustive()
    }
}

impl<Source: Clone + PartialEq, Session: Clone + PartialEq, Representation: Clone + PartialEq>
    MetadataOpeningRetryCoordinator<Source, Session, Representation>
{
    pub(crate) fn new(
        source_scope: &SourceAccountingScope,
        live: RemoteRangeLiveIdentity<Source, Session, Representation>,
        initial_execution: ActiveVisibleExecutionBinding,
    ) -> Result<Self, ScopeAccountingError> {
        let (ranges, deadline) = source_scope.metadata_opening_range_and_deadline_owners()?;
        Ok(Self {
            session_gate: RemoteRangeSessionGate::new(live),
            ranges,
            deadline,
            execution: ExecutionState::Visible(initial_execution),
            last_observed_control_turn: 0,
        })
    }

    pub const fn range_count(&self) -> u64 {
        self.ranges.burned()
    }

    pub const fn deadline_state(&self) -> MetadataOpeningDeadlineState {
        self.deadline.state()
    }

    pub const fn can_refetch_chunks(&self) -> bool {
        self.session_gate.can_refetch_chunks()
    }

    pub const fn terminal(&self) -> Option<RetrySessionTerminal> {
        self.session_gate.terminal()
    }

    pub fn advance_visible_time(
        &mut self,
        binding: ActiveVisibleExecutionBinding,
        elapsed_millis: u64,
    ) -> Result<MetadataOpeningDeadlineState, RetryProtocolError> {
        self.validate_visible_binding(binding)?;
        self.deadline
            .advance_visible(elapsed_millis)
            .map_err(|_error| RetryProtocolError::StaleExecution)
    }

    pub(crate) fn project_retry_turn<'turn>(
        &mut self,
        turn: &'turn ExternalVisibleControlTurn,
    ) -> Result<VisibleRetryTurnPermit<'turn>, RetryProtocolError> {
        self.validate_visible_binding(turn.binding)?;
        match turn.projection.get() {
            TurnProjectionState::Available => {}
            TurnProjectionState::Projected => {
                return Err(RetryProtocolError::TurnAlreadyProjected);
            }
            TurnProjectionState::Claimed => return Err(RetryProtocolError::TurnAlreadyClaimed),
        }
        let sequence = turn.sequence.get();
        if sequence < self.last_observed_control_turn {
            return Err(RetryProtocolError::StaleTurn);
        }
        self.last_observed_control_turn = sequence;
        turn.projection.set(TurnProjectionState::Projected);
        Ok(VisibleRetryTurnPermit {
            binding: turn.binding,
            sequence: turn.sequence,
            projection: &turn.projection,
            claimed: false,
        })
    }

    fn apply_hidden_evidence(
        &mut self,
        evidence: HiddenExecutionEvidence,
    ) -> Result<(), RetryProtocolError> {
        let ExecutionState::Visible(current) = self.execution else {
            return Err(match self.execution {
                ExecutionState::Hidden { .. } => RetryProtocolError::AlreadyHidden,
                ExecutionState::Terminated { .. } => RetryProtocolError::PageTerminated,
                ExecutionState::Visible(_) => unreachable!(),
            });
        };
        if evidence.previous != current {
            return Err(RetryProtocolError::StaleExecution);
        }
        self.execution = ExecutionState::Hidden {
            issuer: current.issuer,
            suspension: evidence.suspension,
        };
        Ok(())
    }

    fn apply_visible_rebind_evidence(
        &mut self,
        evidence: VisibleExecutionRebindEvidence,
    ) -> Result<(), RetryProtocolError> {
        let ExecutionState::Hidden { issuer, suspension } = self.execution else {
            return Err(match self.execution {
                ExecutionState::Visible(_) => RetryProtocolError::StaleExecution,
                ExecutionState::Terminated { .. } => RetryProtocolError::PageTerminated,
                ExecutionState::Hidden { .. } => unreachable!(),
            });
        };
        if evidence.suspension != suspension || evidence.current.issuer != issuer {
            return Err(RetryProtocolError::StaleExecution);
        }
        self.execution = ExecutionState::Visible(evidence.current);
        Ok(())
    }

    fn apply_page_termination(
        &mut self,
        evidence: PageTerminationEvidence,
    ) -> Result<(), RetryProtocolError> {
        let issuer = match self.execution {
            ExecutionState::Visible(binding) => binding.issuer,
            ExecutionState::Hidden { issuer, .. } => issuer,
            ExecutionState::Terminated { issuer } if evidence.issuer == issuer => return Ok(()),
            ExecutionState::Terminated { .. } => return Err(RetryProtocolError::StaleExecution),
        };
        if evidence.issuer != issuer {
            return Err(RetryProtocolError::StaleExecution);
        }
        self.execution = ExecutionState::Terminated { issuer };
        self.session_gate.page_terminate();
        Ok(())
    }

    fn latch_explicit_close(&mut self) {
        self.session_gate.close();
    }

    fn validate_visible_binding(
        &self,
        binding: ActiveVisibleExecutionBinding,
    ) -> Result<(), RetryProtocolError> {
        match self.execution {
            ExecutionState::Visible(current) if current == binding => Ok(()),
            ExecutionState::Visible(_) => Err(RetryProtocolError::StaleExecution),
            ExecutionState::Hidden { .. } => Err(RetryProtocolError::Hidden),
            ExecutionState::Terminated { .. } => Err(RetryProtocolError::PageTerminated),
        }
    }

    fn confirm_active_visible_timeout<Demand>(
        &self,
        evidence: ActiveVisibleTimeoutEvidence,
        callback: &RangeAttemptCallbackIdentity<Source, Session, Representation, Demand>,
    ) -> Result<ConfirmedActiveVisibleTimeout, RetryProtocolError> {
        self.validate_visible_binding(evidence.binding)?;
        if evidence.attempt != callback.attempt || callback.live != self.session_gate.live {
            return Err(RetryProtocolError::AttemptMismatch);
        }
        Ok(ConfirmedActiveVisibleTimeout {
            binding: evidence.binding,
            attempt: evidence.attempt,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OperationState {
    Initial,
    Active { token: RangeAttemptToken },
    RetryPending { eligible_after_turn: u64 },
    Succeeded,
    OpeningFailed,
    SessionFatal,
    Cancelled,
}

#[derive(Clone)]
struct OperationStateOwner(Rc<Cell<OperationState>>);

impl OperationStateOwner {
    fn new() -> Self {
        Self(Rc::new(Cell::new(OperationState::Initial)))
    }

    fn get(&self) -> OperationState {
        self.0.get()
    }

    fn set(&self, state: OperationState) {
        self.0.set(state);
    }
}

/// Last-drop guard shared by an operation and its unique physical-attempt owner.
///
/// Attempt fields are declared so payload/output and the abort controller drop before this guard.
/// Consequently an abandoned task cannot leave the operation `Active`, and cancellation cannot
/// become observable before its physical resources have been released.
struct ActiveOperationStateGuard {
    state: OperationStateOwner,
    token: RangeAttemptToken,
    armed: bool,
}

impl ActiveOperationStateGuard {
    fn new(state: OperationStateOwner, token: RangeAttemptToken) -> Self {
        Self {
            state,
            token,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }

    fn transfer(&mut self) -> Self {
        assert!(self.armed, "active operation guard transfers exactly once");
        self.armed = false;
        Self::new(self.state.clone(), self.token)
    }
}

impl Drop for ActiveOperationStateGuard {
    fn drop(&mut self) {
        if self.armed && self.state.get() == (OperationState::Active { token: self.token }) {
            self.state.set(OperationState::Cancelled);
        }
    }
}

type FactoryStartedRangeAttempt<Source, Session, Representation, Demand, Factory, Payload> =
    StartedRangeAttempt<
        Source,
        Session,
        Representation,
        Demand,
        <Factory as RangeAttemptAbortFactory>::Controller,
        Payload,
    >;

type FactoryAttemptStartResult<
    Source,
    Session,
    Representation,
    Demand,
    Factory,
    Prepared,
    Payload,
> = Result<
    FactoryStartedRangeAttempt<Source, Session, Representation, Demand, Factory, Payload>,
    RangeAttemptStartFailure<Prepared, <Factory as RangeAttemptAbortFactory>::Controller>,
>;

type FactorySettledRangeAttempt<Source, Session, Representation, Demand, Factory, Output> =
    SettledRangeAttempt<
        Source,
        Session,
        Representation,
        Demand,
        <Factory as RangeAttemptAbortFactory>::Controller,
        Output,
    >;

type RejectedFactorySettledRangeAttempt<Source, Session, Representation, Demand, Factory, Output> =
    RejectedRangeAttempt<
        FactorySettledRangeAttempt<Source, Session, Representation, Demand, Factory, Output>,
    >;

type SettledAttemptCompletionResult<Source, Session, Representation, Demand, Factory, Output> =
    Result<
        RangeAttemptCompletionDisposition,
        RejectedFactorySettledRangeAttempt<
            Source,
            Session,
            Representation,
            Demand,
            Factory,
            Output,
        >,
    >;

type FactoryAttemptSuccessResult<Source, Session, Representation, Demand, Factory, Payload> =
    Result<
        Payload,
        RangeAttemptSuccessError<
            FactorySettledRangeAttempt<Source, Session, Representation, Demand, Factory, Payload>,
        >,
    >;

type AttemptSettlementResult<Source, Session, Representation, Demand, Controller, Output> = Result<
    SettledRangeAttempt<Source, Session, Representation, Demand, Controller, Output>,
    RetryProtocolError,
>;

#[cfg(target_arch = "wasm32")]
type FactoryChromeSettlementCompletion<Source, Session, Representation, Demand, Factory, Success> =
    RangeAttemptTransportCompletion<
        Success,
        FactorySettledRangeAttempt<
            Source,
            Session,
            Representation,
            Demand,
            Factory,
            Result<Success, ChromeRangeError>,
        >,
    >;

#[cfg(target_arch = "wasm32")]
type FactoryChromeSettlementWithController<
    Source,
    Session,
    Representation,
    Demand,
    Factory,
    Success,
> = RangeAttemptTransportCompletionWithController<
    Success,
    <Factory as RangeAttemptAbortFactory>::Controller,
    FactorySettledRangeAttempt<
        Source,
        Session,
        Representation,
        Demand,
        Factory,
        Result<Success, ChromeRangeError>,
    >,
>;

/// A protocol rejection that returns the still-live attempt owner to the caller.
pub(crate) struct RejectedRangeAttempt<Attempt> {
    error: RetryProtocolError,
    attempt: Attempt,
}

impl<Attempt> RejectedRangeAttempt<Attempt> {
    pub(crate) fn error(&self) -> RetryProtocolError {
        self.error
    }

    pub(crate) fn into_attempt(self) -> Attempt {
        self.attempt
    }
}

impl<Attempt> fmt::Debug for RejectedRangeAttempt<Attempt> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RejectedRangeAttempt")
            .field("error", &self.error)
            .field("attempt", &"<retained>")
            .finish()
    }
}

/// A success callback that either consumed its matching owner or returned a cross-wired owner.
pub(crate) enum RangeAttemptSuccessError<Attempt> {
    Consumed(RangeAttemptCompletionDisposition),
    Rejected(RejectedRangeAttempt<Attempt>),
}

impl<Attempt> fmt::Debug for RangeAttemptSuccessError<Attempt> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Consumed(disposition) => formatter
                .debug_tuple("Consumed")
                .field(disposition)
                .finish(),
            Self::Rejected(rejected) => rejected.fmt(formatter),
        }
    }
}

/// A production-disarmed exact Range operation.
pub struct RemoteRangeRetryOperation<Source, Session, Representation, Demand, Factory>
where
    Factory: RangeAttemptAbortFactory,
{
    live: RemoteRangeLiveIdentity<Source, Session, Representation>,
    exact_range: ChromeRangeRequest,
    nonce: RangeOperationNonce,
    demand: Demand,
    attempts: RangeRetryAttemptBudget,
    expected_accounting: RangeAttemptAccountingBinding,
    abort_factory: Factory,
    state: OperationStateOwner,
}

impl<Source, Session, Representation, Demand, Factory> fmt::Debug
    for RemoteRangeRetryOperation<Source, Session, Representation, Demand, Factory>
where
    Factory: RangeAttemptAbortFactory,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteRangeRetryOperation")
            .field("live", &"<opaque>")
            .field("exact_range", &self.exact_range)
            .field("nonce", &self.nonce)
            .field("attempts", &self.attempts)
            .field("state", &self.state.get())
            .finish_non_exhaustive()
    }
}

impl<Source, Session, Representation, Demand, Factory>
    RemoteRangeRetryOperation<Source, Session, Representation, Demand, Factory>
where
    Source: Clone + PartialEq,
    Session: Clone + PartialEq,
    Representation: Clone + PartialEq,
    Demand: Clone + PartialEq,
    Factory: RangeAttemptAbortFactory,
{
    pub(crate) fn new(
        operation_scope: &OperationAccountingScope,
        range_scope: &RangeResponseAccountingScope,
        work_scope: &WorkUnitAccountingScope,
        live: RemoteRangeLiveIdentity<Source, Session, Representation>,
        exact_range: ChromeRangeRequest,
        demand: Demand,
        abort_factory: Factory,
    ) -> Result<Self, RangeAttemptStartError> {
        let attempts = operation_scope
            .range_retry_attempt_budget()
            .map_err(|_error| {
                RangeAttemptStartError::OpeningFailed(MetadataOpeningFailureSignal::ResourceLimit)
            })?;
        let expected_accounting = attempts
            .bind_metadata_opening_transport(range_scope, work_scope)
            .map_err(|_error| RangeAttemptStartError::AccountingAncestryMismatch)?;
        Ok(Self {
            live,
            exact_range,
            nonce: allocate_operation_nonce()?,
            demand,
            attempts,
            expected_accounting,
            abort_factory,
            state: OperationStateOwner::new(),
        })
    }

    fn state(&self) -> OperationState {
        self.state.get()
    }

    fn set_state(&self, state: OperationState) {
        self.state.set(state);
    }

    pub const fn attempts_burned(&self) -> u64 {
        self.attempts.burned()
    }

    pub fn exact_range(&self) -> Range<u64> {
        self.exact_range.as_range()
    }

    pub fn supersede_demand(&mut self, demand: Demand) {
        self.demand = demand;
    }

    pub fn start_initial<Prepared, Payload>(
        &mut self,
        coordinator: &mut MetadataOpeningRetryCoordinator<Source, Session, Representation>,
        execution: ActiveVisibleExecutionBinding,
        preflight: impl FnOnce(
            &Factory::Controller,
            ChromeRangeRequest,
        ) -> Result<Prepared, ChromeRangeError>,
        start_fetch: impl FnOnce(&Factory::Controller, Prepared, CommittedRangeAttemptBurn) -> Payload,
    ) -> FactoryAttemptStartResult<
        Source,
        Session,
        Representation,
        Demand,
        Factory,
        Prepared,
        Payload,
    >
    where
        Prepared: PreparedChromeRangeIdentity<Factory::Controller>,
    {
        if self.state() != OperationState::Initial {
            return Err(
                RangeAttemptStartError::from(RetryProtocolError::OperationNotInitial).into(),
            );
        }
        self.start_attempt(coordinator, execution, None, preflight, start_fetch)
    }

    pub fn start_retry_on_turn<Prepared, Payload>(
        &mut self,
        coordinator: &mut MetadataOpeningRetryCoordinator<Source, Session, Representation>,
        turn: &mut VisibleRetryTurnPermit<'_>,
        preflight: impl FnOnce(
            &Factory::Controller,
            ChromeRangeRequest,
        ) -> Result<Prepared, ChromeRangeError>,
        start_fetch: impl FnOnce(&Factory::Controller, Prepared, CommittedRangeAttemptBurn) -> Payload,
    ) -> FactoryAttemptStartResult<
        Source,
        Session,
        Representation,
        Demand,
        Factory,
        Prepared,
        Payload,
    >
    where
        Prepared: PreparedChromeRangeIdentity<Factory::Controller>,
    {
        let OperationState::RetryPending {
            eligible_after_turn,
        } = self.state()
        else {
            return Err(RangeAttemptStartError::from(match self.state() {
                OperationState::Succeeded
                | OperationState::OpeningFailed
                | OperationState::SessionFatal
                | OperationState::Cancelled => RetryProtocolError::OperationAlreadyTerminal,
                _ => RetryProtocolError::OperationNotRetryPending,
            })
            .into());
        };
        coordinator
            .validate_visible_binding(turn.binding)
            .map_err(RangeAttemptStartError::from)?;
        if turn.sequence.get() <= eligible_after_turn {
            return Err(
                RangeAttemptStartError::from(RetryProtocolError::RetryNotYetEligible).into(),
            );
        }
        if turn.claimed {
            return Err(
                RangeAttemptStartError::from(RetryProtocolError::TurnAlreadyClaimed).into(),
            );
        }
        self.start_attempt(
            coordinator,
            turn.binding,
            Some(turn),
            preflight,
            start_fetch,
        )
    }

    fn start_attempt<Prepared, Payload>(
        &mut self,
        coordinator: &mut MetadataOpeningRetryCoordinator<Source, Session, Representation>,
        execution: ActiveVisibleExecutionBinding,
        turn: Option<&mut VisibleRetryTurnPermit<'_>>,
        preflight: impl FnOnce(
            &Factory::Controller,
            ChromeRangeRequest,
        ) -> Result<Prepared, ChromeRangeError>,
        start_fetch: impl FnOnce(&Factory::Controller, Prepared, CommittedRangeAttemptBurn) -> Payload,
    ) -> FactoryAttemptStartResult<
        Source,
        Session,
        Representation,
        Demand,
        Factory,
        Prepared,
        Payload,
    >
    where
        Prepared: PreparedChromeRangeIdentity<Factory::Controller>,
    {
        coordinator
            .validate_visible_binding(execution)
            .map_err(RangeAttemptStartError::from)?;
        if coordinator.session_gate.live != self.live {
            return Err(RangeAttemptStartError::from(RetryProtocolError::AttemptMismatch).into());
        }
        if coordinator.terminal().is_some() {
            self.set_state(OperationState::Cancelled);
            return Err(
                RangeAttemptStartError::from(RetryProtocolError::OperationAlreadyTerminal).into(),
            );
        }
        if coordinator.deadline_state() == MetadataOpeningDeadlineState::Expired {
            self.set_state(OperationState::OpeningFailed);
            return Err(RangeAttemptStartError::OpeningFailed(
                MetadataOpeningFailureSignal::ActiveVisibleDeadlineExpired,
            )
            .into());
        }

        let mut controller = match self.abort_factory.create() {
            Ok(controller) => controller,
            Err(error) => return Err(self.fail_before_fetch(coordinator, error).into()),
        };
        let prepared = match preflight(&controller, self.exact_range) {
            Ok(prepared) => prepared,
            Err(error) => {
                controller.abort();
                return Err(self.fail_before_fetch(coordinator, error).into());
            }
        };
        let mismatch = if prepared.prepared_range() != self.exact_range {
            Some(PreparedAttemptMismatchKind::ExactRange)
        } else if !prepared.matches_abort_controller(&controller) {
            Some(PreparedAttemptMismatchKind::AbortController)
        } else if prepared.prepared_accounting_binding() != self.expected_accounting {
            Some(PreparedAttemptMismatchKind::AccountingAncestry)
        } else {
            None
        };
        if let Some(kind) = mismatch {
            return Err(RangeAttemptStartFailure::PreparedMismatch(
                RejectedPreparedAttempt {
                    kind,
                    prepared,
                    controller,
                },
            ));
        }
        let burn = match self
            .attempts
            .prepare_metadata_opening_attempt(&mut coordinator.ranges)
        {
            Ok(burn) => burn,
            Err(error) => {
                controller.abort();
                self.set_state(OperationState::OpeningFailed);
                return Err(RangeAttemptStartError::OpeningFailed(map_burn_failure(error)).into());
            }
        };
        let token = RangeAttemptToken {
            operation: self.nonce,
            attempt: burn.next_attempt(),
        };
        let callback = RangeAttemptCallbackIdentity {
            live: self.live.clone(),
            operation: self.nonce,
            attempt: token,
            demand: self.demand.clone(),
        };
        if let Some(turn) = turn {
            turn.claim().map_err(RangeAttemptStartError::from)?;
        }
        let receipt = burn.commit();
        self.set_state(OperationState::Active { token });
        // `start_fetch` is the only side-effecting boundary and runs after the composite burn.
        let payload = start_fetch(&controller, prepared, receipt);
        Ok(StartedRangeAttempt {
            payload: Some(payload),
            controller: Some(controller),
            callback: Some(callback),
            state_guard: ActiveOperationStateGuard::new(self.state.clone(), token),
            _not_send_or_sync: PhantomData,
        })
    }

    fn fail_before_fetch(
        &self,
        coordinator: &mut MetadataOpeningRetryCoordinator<Source, Session, Representation>,
        error: ChromeRangeError,
    ) -> RangeAttemptStartError {
        if matches!(
            error,
            ChromeRangeError::ObjectChanged | ChromeRangeError::PreconditionFailed
        ) {
            let disposition = coordinator.session_gate.latch_object_changed(&self.live);
            return match disposition {
                ObjectChangeDisposition::Latched => {
                    self.set_state(OperationState::SessionFatal);
                    RangeAttemptStartError::ObjectChanged(disposition)
                }
                ObjectChangeDisposition::ExistingTerminalPreserved => {
                    self.set_state(OperationState::Cancelled);
                    RangeAttemptStartError::Protocol(RetryProtocolError::OperationAlreadyTerminal)
                }
                ObjectChangeDisposition::StaleIdentity | ObjectChangeDisposition::StaleAttempt => {
                    self.set_state(OperationState::Cancelled);
                    RangeAttemptStartError::Protocol(RetryProtocolError::AttemptMismatch)
                }
            };
        }

        self.set_state(OperationState::OpeningFailed);
        RangeAttemptStartError::OpeningFailed(MetadataOpeningFailureSignal::NonRetryable(error))
    }

    pub fn complete_failure<Payload>(
        &self,
        coordinator: &mut MetadataOpeningRetryCoordinator<Source, Session, Representation>,
        attempt: FactorySettledRangeAttempt<
            Source,
            Session,
            Representation,
            Demand,
            Factory,
            Payload,
        >,
        error: ChromeRangeError,
    ) -> SettledAttemptCompletionResult<Source, Session, Representation, Demand, Factory, Payload>
    {
        if let Err(error) = self.validate_settled_completion_attempt(coordinator, &attempt) {
            return Err(RejectedRangeAttempt { error, attempt });
        }
        let demand_is_current = attempt.callback_identity().demand == self.demand;
        // Release Response/reader/output/permits and abort ownership before publishing pending.
        attempt.abort_and_transition(OperationState::Cancelled);

        if coordinator.terminal().is_some() {
            self.set_state(OperationState::Cancelled);
            return Ok(RangeAttemptCompletionDisposition::Stale);
        }

        if matches!(
            error,
            ChromeRangeError::ObjectChanged | ChromeRangeError::PreconditionFailed
        ) {
            let disposition = coordinator.session_gate.latch_object_changed(&self.live);
            self.set_state(match disposition {
                ObjectChangeDisposition::Latched
                | ObjectChangeDisposition::ExistingTerminalPreserved => {
                    OperationState::SessionFatal
                }
                ObjectChangeDisposition::StaleIdentity | ObjectChangeDisposition::StaleAttempt => {
                    OperationState::Cancelled
                }
            });
            return Ok(RangeAttemptCompletionDisposition::ObjectChanged(
                disposition,
            ));
        }

        if !demand_is_current {
            self.set_state(OperationState::Cancelled);
            return Ok(RangeAttemptCompletionDisposition::Stale);
        }
        if coordinator.deadline_state() == MetadataOpeningDeadlineState::Expired {
            self.set_state(OperationState::OpeningFailed);
            return Ok(RangeAttemptCompletionDisposition::OpeningFailed(
                MetadataOpeningFailureSignal::ActiveVisibleDeadlineExpired,
            ));
        }
        if is_retryable_attempt_failure(error) {
            self.set_state(OperationState::RetryPending {
                eligible_after_turn: coordinator.last_observed_control_turn,
            });
            Ok(RangeAttemptCompletionDisposition::RetryPending)
        } else {
            self.set_state(OperationState::OpeningFailed);
            Ok(RangeAttemptCompletionDisposition::OpeningFailed(
                MetadataOpeningFailureSignal::NonRetryable(error),
            ))
        }
    }

    /// Completes a timeout only after the external issuer and source deadline owner confirm it.
    ///
    /// A stale/hidden/mismatched confirmation returns the live attempt owner unchanged, allowing
    /// the caller to keep a hidden body pump parked or route a cross-wired callback correctly.
    fn complete_active_visible_timeout<Payload>(
        &self,
        coordinator: &MetadataOpeningRetryCoordinator<Source, Session, Representation>,
        mut attempt: Pin<
            &mut FactoryStartedRangeAttempt<
                Source,
                Session,
                Representation,
                Demand,
                Factory,
                Payload,
            >,
        >,
        evidence: ActiveVisibleTimeoutEvidence,
    ) -> Result<RangeAttemptCompletionDisposition, RetryProtocolError> {
        let OperationState::Active { token } = self.state() else {
            return Err(RetryProtocolError::OperationAlreadyTerminal);
        };
        let callback = attempt.as_ref().get_ref().callback_identity()?;
        let confirmation = coordinator.confirm_active_visible_timeout(evidence, callback)?;
        if confirmation.binding != evidence.binding
            || confirmation.attempt != token
            || callback.operation != self.nonce
            || callback.attempt != token
            || callback.live != self.live
        {
            return Err(RetryProtocolError::AttemptMismatch);
        }
        let callback = callback.clone();

        if coordinator.terminal().is_some() || callback.demand != self.demand {
            attempt
                .as_mut()
                .abort_and_transition(OperationState::Cancelled);
            return Ok(RangeAttemptCompletionDisposition::Stale);
        }
        if coordinator.deadline_state() == MetadataOpeningDeadlineState::Expired {
            attempt
                .as_mut()
                .abort_and_transition(OperationState::OpeningFailed);
            return Ok(RangeAttemptCompletionDisposition::OpeningFailed(
                MetadataOpeningFailureSignal::ActiveVisibleDeadlineExpired,
            ));
        }
        attempt
            .as_mut()
            .abort_and_transition(OperationState::RetryPending {
                eligible_after_turn: coordinator.last_observed_control_turn,
            });
        Ok(RangeAttemptCompletionDisposition::RetryPending)
    }

    /// Consumes a matching pending task when the source-wide active-visible opening deadline
    /// reaches its exact limit.
    fn complete_source_deadline_expiry<Payload>(
        &self,
        coordinator: &MetadataOpeningRetryCoordinator<Source, Session, Representation>,
        binding: ActiveVisibleExecutionBinding,
        mut attempt: Pin<
            &mut FactoryStartedRangeAttempt<
                Source,
                Session,
                Representation,
                Demand,
                Factory,
                Payload,
            >,
        >,
    ) -> Result<RangeAttemptCompletionDisposition, RetryProtocolError> {
        coordinator.validate_visible_binding(binding)?;
        self.validate_started_completion_attempt(coordinator, attempt.as_ref().get_ref())?;
        if coordinator.deadline_state() != MetadataOpeningDeadlineState::Expired {
            return Err(RetryProtocolError::TimeoutNotConfirmed);
        }
        if coordinator.terminal().is_some()
            || attempt.as_ref().get_ref().callback_identity()?.demand != self.demand
        {
            attempt
                .as_mut()
                .abort_and_transition(OperationState::Cancelled);
            return Ok(RangeAttemptCompletionDisposition::Stale);
        }
        attempt
            .as_mut()
            .abort_and_transition(OperationState::OpeningFailed);
        Ok(RangeAttemptCompletionDisposition::OpeningFailed(
            MetadataOpeningFailureSignal::ActiveVisibleDeadlineExpired,
        ))
    }

    /// Applies a confirmed timeout after the transport has settled but before its output has been
    /// committed by the frame owner.
    fn complete_settled_active_visible_timeout<Output>(
        &self,
        coordinator: &MetadataOpeningRetryCoordinator<Source, Session, Representation>,
        attempt: FactorySettledRangeAttempt<
            Source,
            Session,
            Representation,
            Demand,
            Factory,
            Output,
        >,
        evidence: ActiveVisibleTimeoutEvidence,
    ) -> SettledAttemptCompletionResult<Source, Session, Representation, Demand, Factory, Output>
    {
        if let Err(error) = self.validate_settled_completion_attempt(coordinator, &attempt) {
            return Err(RejectedRangeAttempt { error, attempt });
        }
        let callback = attempt.callback_identity().clone();
        let confirmation = match coordinator.confirm_active_visible_timeout(evidence, &callback) {
            Ok(confirmation) => confirmation,
            Err(error) => return Err(RejectedRangeAttempt { error, attempt }),
        };
        let OperationState::Active { token } = self.state() else {
            return Err(RejectedRangeAttempt {
                error: RetryProtocolError::OperationAlreadyTerminal,
                attempt,
            });
        };
        if confirmation.binding != evidence.binding
            || confirmation.attempt != token
            || callback.operation != self.nonce
            || callback.attempt != token
            || callback.live != self.live
        {
            return Err(RejectedRangeAttempt {
                error: RetryProtocolError::AttemptMismatch,
                attempt,
            });
        }

        if coordinator.terminal().is_some() || callback.demand != self.demand {
            attempt.abort_and_transition(OperationState::Cancelled);
            return Ok(RangeAttemptCompletionDisposition::Stale);
        }
        if coordinator.deadline_state() == MetadataOpeningDeadlineState::Expired {
            attempt.abort_and_transition(OperationState::OpeningFailed);
            return Ok(RangeAttemptCompletionDisposition::OpeningFailed(
                MetadataOpeningFailureSignal::ActiveVisibleDeadlineExpired,
            ));
        }
        attempt.abort_and_transition(OperationState::RetryPending {
            eligible_after_turn: coordinator.last_observed_control_turn,
        });
        Ok(RangeAttemptCompletionDisposition::RetryPending)
    }

    /// Applies exact source-deadline expiry to a settled, not-yet-committed transport owner.
    fn complete_settled_source_deadline_expiry<Output>(
        &self,
        coordinator: &MetadataOpeningRetryCoordinator<Source, Session, Representation>,
        binding: ActiveVisibleExecutionBinding,
        attempt: FactorySettledRangeAttempt<
            Source,
            Session,
            Representation,
            Demand,
            Factory,
            Output,
        >,
    ) -> SettledAttemptCompletionResult<Source, Session, Representation, Demand, Factory, Output>
    {
        if let Err(error) = coordinator.validate_visible_binding(binding) {
            return Err(RejectedRangeAttempt { error, attempt });
        }
        if let Err(error) = self.validate_settled_completion_attempt(coordinator, &attempt) {
            return Err(RejectedRangeAttempt { error, attempt });
        }
        if coordinator.deadline_state() != MetadataOpeningDeadlineState::Expired {
            return Err(RejectedRangeAttempt {
                error: RetryProtocolError::TimeoutNotConfirmed,
                attempt,
            });
        }
        if coordinator.terminal().is_some() || attempt.callback_identity().demand != self.demand {
            attempt.abort_and_transition(OperationState::Cancelled);
            return Ok(RangeAttemptCompletionDisposition::Stale);
        }
        attempt.abort_and_transition(OperationState::OpeningFailed);
        Ok(RangeAttemptCompletionDisposition::OpeningFailed(
            MetadataOpeningFailureSignal::ActiveVisibleDeadlineExpired,
        ))
    }

    /// Closes a settled, not-yet-committed transport owner without publishing its output.
    pub(crate) fn close_settled<Output>(
        &self,
        coordinator: &mut MetadataOpeningRetryCoordinator<Source, Session, Representation>,
        attempt: FactorySettledRangeAttempt<
            Source,
            Session,
            Representation,
            Demand,
            Factory,
            Output,
        >,
    ) -> SettledAttemptCompletionResult<Source, Session, Representation, Demand, Factory, Output>
    {
        if let Err(error) = self.validate_settled_completion_attempt(coordinator, &attempt) {
            return Err(RejectedRangeAttempt { error, attempt });
        }
        coordinator.latch_explicit_close();
        attempt.abort_and_transition(OperationState::Cancelled);
        Ok(RangeAttemptCompletionDisposition::Cancelled)
    }

    /// Terminates a settled, not-yet-committed transport owner on a matching page transition.
    fn page_terminate_settled<Output>(
        &self,
        coordinator: &mut MetadataOpeningRetryCoordinator<Source, Session, Representation>,
        evidence: PageTerminationEvidence,
        attempt: FactorySettledRangeAttempt<
            Source,
            Session,
            Representation,
            Demand,
            Factory,
            Output,
        >,
    ) -> SettledAttemptCompletionResult<Source, Session, Representation, Demand, Factory, Output>
    {
        if let Err(error) = self.validate_settled_completion_attempt(coordinator, &attempt) {
            return Err(RejectedRangeAttempt { error, attempt });
        }
        if let Err(error) = coordinator.apply_page_termination(evidence) {
            return Err(RejectedRangeAttempt { error, attempt });
        }
        attempt.abort_and_transition(OperationState::Cancelled);
        Ok(RangeAttemptCompletionDisposition::Cancelled)
    }

    /// Rejects a callback that no longer owns the attempt's response/controller resources.
    ///
    /// Browser APIs may report more than one settlement signal around abort/timeout races. Only
    /// the callback that still carries [`StartedRangeAttempt`] may advance this operation. This
    /// token-only seam exists so every later signal can be classified and dropped without
    /// cloning the resource owner or relatching a fatal condition.
    pub fn reject_unowned_failure_callback(
        &self,
        coordinator: &MetadataOpeningRetryCoordinator<Source, Session, Representation>,
        callback: &RangeAttemptCallbackIdentity<Source, Session, Representation, Demand>,
        error: ChromeRangeError,
    ) -> RangeAttemptCompletionDisposition {
        if matches!(
            error,
            ChromeRangeError::ObjectChanged | ChromeRangeError::PreconditionFailed
        ) {
            let disposition =
                if callback.live != coordinator.session_gate.live || callback.live != self.live {
                    ObjectChangeDisposition::StaleIdentity
                } else {
                    ObjectChangeDisposition::StaleAttempt
                };
            RangeAttemptCompletionDisposition::ObjectChanged(disposition)
        } else {
            RangeAttemptCompletionDisposition::Stale
        }
    }

    pub fn complete_success<Payload>(
        &self,
        coordinator: &MetadataOpeningRetryCoordinator<Source, Session, Representation>,
        attempt: FactorySettledRangeAttempt<
            Source,
            Session,
            Representation,
            Demand,
            Factory,
            Payload,
        >,
    ) -> FactoryAttemptSuccessResult<Source, Session, Representation, Demand, Factory, Payload>
    {
        if let Err(error) = self.validate_settled_completion_attempt(coordinator, &attempt) {
            return Err(RangeAttemptSuccessError::Rejected(RejectedRangeAttempt {
                error,
                attempt,
            }));
        }
        if attempt.callback_identity().demand != self.demand || coordinator.terminal().is_some() {
            attempt.abort_and_transition(OperationState::Cancelled);
            return Err(RangeAttemptSuccessError::Consumed(
                RangeAttemptCompletionDisposition::Stale,
            ));
        }
        let Some(output) = attempt.into_success_output() else {
            self.set_state(OperationState::OpeningFailed);
            return Err(RangeAttemptSuccessError::Consumed(
                RangeAttemptCompletionDisposition::OpeningFailed(
                    MetadataOpeningFailureSignal::ResourceLimit,
                ),
            ));
        };
        Ok(output)
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn complete_chrome_settlement<Success>(
        &self,
        coordinator: &mut MetadataOpeningRetryCoordinator<Source, Session, Representation>,
        attempt: FactorySettledRangeAttempt<
            Source,
            Session,
            Representation,
            Demand,
            Factory,
            Result<Success, ChromeRangeError>,
        >,
    ) -> FactoryChromeSettlementCompletion<Source, Session, Representation, Demand, Factory, Success>
    {
        if let Err(error) = self.validate_settled_completion_attempt(coordinator, &attempt) {
            return RangeAttemptTransportCompletion::Rejected(RejectedRangeAttempt {
                error,
                attempt,
            });
        }

        let observed_error = attempt
            .output
            .as_ref()
            .and_then(|output| output.as_ref().err())
            .copied();
        if let Some(error) = observed_error {
            return match self.complete_failure(coordinator, attempt, error) {
                Ok(disposition) => RangeAttemptTransportCompletion::Failed(disposition),
                Err(rejected) => RangeAttemptTransportCompletion::Rejected(rejected),
            };
        }

        if attempt.callback_identity().demand != self.demand || coordinator.terminal().is_some() {
            attempt.abort_and_transition(OperationState::Cancelled);
            return RangeAttemptTransportCompletion::Failed(
                RangeAttemptCompletionDisposition::Stale,
            );
        }

        match attempt.into_success_output() {
            Some(Ok(output)) => RangeAttemptTransportCompletion::Succeeded(output),
            Some(Err(error)) => {
                self.set_state(OperationState::OpeningFailed);
                RangeAttemptTransportCompletion::Failed(
                    RangeAttemptCompletionDisposition::OpeningFailed(
                        MetadataOpeningFailureSignal::NonRetryable(error),
                    ),
                )
            }
            None => {
                self.set_state(OperationState::OpeningFailed);
                RangeAttemptTransportCompletion::Failed(
                    RangeAttemptCompletionDisposition::OpeningFailed(
                        MetadataOpeningFailureSignal::ResourceLimit,
                    ),
                )
            }
        }
    }

    /// Completes a Chrome settlement while transferring the matching successful attempt's abort
    /// capability to a higher-level handoff owner.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn complete_chrome_settlement_with_controller<Success>(
        &self,
        coordinator: &mut MetadataOpeningRetryCoordinator<Source, Session, Representation>,
        attempt: FactorySettledRangeAttempt<
            Source,
            Session,
            Representation,
            Demand,
            Factory,
            Result<Success, ChromeRangeError>,
        >,
    ) -> FactoryChromeSettlementWithController<
        Source,
        Session,
        Representation,
        Demand,
        Factory,
        Success,
    > {
        if let Err(error) = self.validate_settled_completion_attempt(coordinator, &attempt) {
            return RangeAttemptTransportCompletionWithController::Rejected(RejectedRangeAttempt {
                error,
                attempt,
            });
        }

        let observed_error = attempt
            .output
            .as_ref()
            .and_then(|output| output.as_ref().err())
            .copied();
        if let Some(error) = observed_error {
            return match self.complete_failure(coordinator, attempt, error) {
                Ok(disposition) => {
                    RangeAttemptTransportCompletionWithController::Failed(disposition)
                }
                Err(rejected) => RangeAttemptTransportCompletionWithController::Rejected(rejected),
            };
        }

        if attempt.callback_identity().demand != self.demand || coordinator.terminal().is_some() {
            attempt.abort_and_transition(OperationState::Cancelled);
            return RangeAttemptTransportCompletionWithController::Failed(
                RangeAttemptCompletionDisposition::Stale,
            );
        }

        match attempt.into_success_output_and_controller() {
            Some((Ok(output), controller)) => {
                RangeAttemptTransportCompletionWithController::Succeeded(output, controller)
            }
            Some((Err(error), mut controller)) => {
                controller.abort();
                self.set_state(OperationState::OpeningFailed);
                RangeAttemptTransportCompletionWithController::Failed(
                    RangeAttemptCompletionDisposition::OpeningFailed(
                        MetadataOpeningFailureSignal::NonRetryable(error),
                    ),
                )
            }
            None => {
                self.set_state(OperationState::OpeningFailed);
                RangeAttemptTransportCompletionWithController::Failed(
                    RangeAttemptCompletionDisposition::OpeningFailed(
                        MetadataOpeningFailureSignal::ResourceLimit,
                    ),
                )
            }
        }
    }

    fn validate_owned_attempt<Payload>(
        &self,
        attempt: &FactoryStartedRangeAttempt<
            Source,
            Session,
            Representation,
            Demand,
            Factory,
            Payload,
        >,
    ) -> Result<(), RetryProtocolError> {
        let OperationState::Active { token } = self.state() else {
            return Err(RetryProtocolError::ActiveAttemptRequired);
        };
        let callback = attempt.callback_identity()?;
        if callback.operation != self.nonce
            || callback.attempt != token
            || callback.live != self.live
        {
            return Err(RetryProtocolError::AttemptMismatch);
        }
        Ok(())
    }

    fn validate_started_completion_attempt<Payload>(
        &self,
        coordinator: &MetadataOpeningRetryCoordinator<Source, Session, Representation>,
        attempt: &FactoryStartedRangeAttempt<
            Source,
            Session,
            Representation,
            Demand,
            Factory,
            Payload,
        >,
    ) -> Result<(), RetryProtocolError> {
        self.validate_owned_attempt(attempt)?;
        if attempt.callback_identity()?.live != coordinator.session_gate.live {
            return Err(RetryProtocolError::AttemptMismatch);
        }
        Ok(())
    }

    fn validate_settled_completion_attempt<Output>(
        &self,
        coordinator: &MetadataOpeningRetryCoordinator<Source, Session, Representation>,
        attempt: &FactorySettledRangeAttempt<
            Source,
            Session,
            Representation,
            Demand,
            Factory,
            Output,
        >,
    ) -> Result<(), RetryProtocolError> {
        let OperationState::Active { token } = self.state() else {
            return Err(RetryProtocolError::ActiveAttemptRequired);
        };
        let callback = attempt.callback_identity();
        if callback.operation != self.nonce
            || callback.attempt != token
            || callback.live != self.live
            || callback.live != coordinator.session_gate.live
        {
            return Err(RetryProtocolError::AttemptMismatch);
        }
        Ok(())
    }

    fn cancel_active<Payload>(
        &self,
        mut attempt: Pin<
            &mut FactoryStartedRangeAttempt<
                Source,
                Session,
                Representation,
                Demand,
                Factory,
                Payload,
            >,
        >,
    ) -> Result<RangeAttemptCompletionDisposition, RetryProtocolError> {
        self.validate_owned_attempt(attempt.as_ref().get_ref())?;
        attempt
            .as_mut()
            .abort_and_transition(OperationState::Cancelled);
        Ok(RangeAttemptCompletionDisposition::Cancelled)
    }

    pub(crate) fn close_active<Payload>(
        &self,
        coordinator: &mut MetadataOpeningRetryCoordinator<Source, Session, Representation>,
        mut attempt: Pin<
            &mut FactoryStartedRangeAttempt<
                Source,
                Session,
                Representation,
                Demand,
                Factory,
                Payload,
            >,
        >,
    ) -> Result<RangeAttemptCompletionDisposition, RetryProtocolError> {
        self.validate_started_completion_attempt(coordinator, attempt.as_ref().get_ref())?;
        coordinator.latch_explicit_close();
        self.cancel_active(attempt.as_mut())
    }

    fn page_terminate_active<Payload>(
        &self,
        coordinator: &mut MetadataOpeningRetryCoordinator<Source, Session, Representation>,
        evidence: PageTerminationEvidence,
        mut attempt: Pin<
            &mut FactoryStartedRangeAttempt<
                Source,
                Session,
                Representation,
                Demand,
                Factory,
                Payload,
            >,
        >,
    ) -> Result<RangeAttemptCompletionDisposition, RetryProtocolError> {
        self.validate_started_completion_attempt(coordinator, attempt.as_ref().get_ref())?;
        coordinator.apply_page_termination(evidence)?;
        self.cancel_active(attempt.as_mut())
    }

    fn close_inactive(
        &self,
        coordinator: &mut MetadataOpeningRetryCoordinator<Source, Session, Representation>,
    ) -> Result<(), RetryProtocolError> {
        if matches!(self.state(), OperationState::Active { .. }) {
            return Err(RetryProtocolError::ActiveAttemptRequired);
        }
        if coordinator.session_gate.live != self.live {
            return Err(RetryProtocolError::AttemptMismatch);
        }
        coordinator.latch_explicit_close();
        self.set_state(OperationState::Cancelled);
        Ok(())
    }

    fn page_terminate_inactive(
        &self,
        coordinator: &mut MetadataOpeningRetryCoordinator<Source, Session, Representation>,
        evidence: PageTerminationEvidence,
    ) -> Result<(), RetryProtocolError> {
        if matches!(self.state(), OperationState::Active { .. }) {
            return Err(RetryProtocolError::ActiveAttemptRequired);
        }
        if coordinator.session_gate.live != self.live {
            return Err(RetryProtocolError::AttemptMismatch);
        }
        coordinator.apply_page_termination(evidence)?;
        self.set_state(OperationState::Cancelled);
        Ok(())
    }
}

pin_project! {
    /// One pollable physical Fetch task plus all ownership required for synchronous cancellation.
    ///
    /// Field order is normative: a pending transport/output is dropped in place before its
    /// controller, and both disappear before the matching operation-state guard runs.
    pub struct StartedRangeAttempt<Source, Session, Representation, Demand, Controller, Payload>
    where
        Controller: RangeAttemptAbortController,
    {
        #[pin]
        payload: Option<Payload>,
        controller: Option<Controller>,
        callback: Option<RangeAttemptCallbackIdentity<Source, Session, Representation, Demand>>,
        state_guard: ActiveOperationStateGuard,
        _not_send_or_sync: PhantomData<*mut ()>,
    }
}

impl<Source, Session, Representation, Demand, Controller, Payload> fmt::Debug
    for StartedRangeAttempt<Source, Session, Representation, Demand, Controller, Payload>
where
    Controller: RangeAttemptAbortController,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StartedRangeAttempt")
            .field("callback", &"<opaque>")
            .field("owns_controller", &self.controller.is_some())
            .field("owns_payload", &self.payload.is_some())
            .finish_non_exhaustive()
    }
}

impl<Source, Session, Representation, Demand, Controller, Payload>
    StartedRangeAttempt<Source, Session, Representation, Demand, Controller, Payload>
where
    Controller: RangeAttemptAbortController,
{
    pub fn token(&self) -> Result<RangeAttemptToken, RetryProtocolError> {
        Ok(self.callback_identity()?.attempt)
    }

    pub fn callback_identity(
        &self,
    ) -> Result<
        &RangeAttemptCallbackIdentity<Source, Session, Representation, Demand>,
        RetryProtocolError,
    > {
        self.callback
            .as_ref()
            .ok_or(RetryProtocolError::AttemptOwnerDepleted)
    }

    /// Polls the transport without moving it out of this unique, synchronously cancellable owner.
    pub(crate) fn poll_settlement(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<
        AttemptSettlementResult<
            Source,
            Session,
            Representation,
            Demand,
            Controller,
            Payload::Output,
        >,
    >
    where
        Payload: std::future::Future,
    {
        let mut this = self.project();
        let output = {
            let Some(payload) = this.payload.as_mut().as_pin_mut() else {
                return Poll::Ready(Err(RetryProtocolError::OperationAlreadyTerminal));
            };
            match payload.poll(context) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(output) => output,
            }
        };
        this.payload.set(None);
        Poll::Ready(Ok(SettledRangeAttempt {
            output: Some(output),
            controller: this.controller.take(),
            callback: this
                .callback
                .take()
                .expect("started attempt owns its callback identity"),
            state_guard: this.state_guard.transfer(),
            _not_send_or_sync: PhantomData,
        }))
    }

    #[cfg(test)]
    fn settle_immediate(
        mut self,
    ) -> SettledRangeAttempt<Source, Session, Representation, Demand, Controller, Payload> {
        SettledRangeAttempt {
            output: self.payload.take(),
            controller: self.controller.take(),
            callback: self
                .callback
                .take()
                .expect("started attempt owns its callback identity"),
            state_guard: self.state_guard.transfer(),
            _not_send_or_sync: PhantomData,
        }
    }

    fn abort_and_transition(self: Pin<&mut Self>, state: OperationState) {
        let mut this = self.project();
        this.payload.set(None);
        if let Some(controller) = this.controller.as_mut() {
            controller.abort();
        }
        drop(this.controller.take());
        drop(this.callback.take());
        this.state_guard.state.set(state);
        this.state_guard.disarm();
    }
}

/// A settled physical attempt whose callback identity, controller, and output remain inseparable.
pub(crate) struct SettledRangeAttempt<Source, Session, Representation, Demand, Controller, Output>
where
    Controller: RangeAttemptAbortController,
{
    output: Option<Output>,
    controller: Option<Controller>,
    callback: RangeAttemptCallbackIdentity<Source, Session, Representation, Demand>,
    state_guard: ActiveOperationStateGuard,
    _not_send_or_sync: PhantomData<*mut ()>,
}

impl<Source, Session, Representation, Demand, Controller, Output> fmt::Debug
    for SettledRangeAttempt<Source, Session, Representation, Demand, Controller, Output>
where
    Controller: RangeAttemptAbortController,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SettledRangeAttempt")
            .field("callback", &"<opaque>")
            .field("owns_controller", &self.controller.is_some())
            .field("owns_output", &self.output.is_some())
            .finish_non_exhaustive()
    }
}

impl<Source, Session, Representation, Demand, Controller, Output>
    SettledRangeAttempt<Source, Session, Representation, Demand, Controller, Output>
where
    Controller: RangeAttemptAbortController,
{
    fn callback_identity(
        &self,
    ) -> &RangeAttemptCallbackIdentity<Source, Session, Representation, Demand> {
        &self.callback
    }

    fn abort_and_transition(mut self, state: OperationState) {
        // Ordering is normative: response/output permits disappear before retry becomes visible.
        drop(self.output.take());
        if let Some(controller) = self.controller.as_mut() {
            controller.abort();
        }
        drop(self.controller.take());
        self.state_guard.state.set(state);
        self.state_guard.disarm();
    }

    fn into_success_output(mut self) -> Option<Output> {
        if let Some(controller) = self.controller.as_mut() {
            controller.finish();
        }
        drop(self.controller.take());
        self.state_guard.state.set(OperationState::Succeeded);
        self.state_guard.disarm();
        self.output.take()
    }

    #[cfg(target_arch = "wasm32")]
    fn into_success_output_and_controller(mut self) -> Option<(Output, Controller)> {
        let output = self.output.take()?;
        let controller = self.controller.take()?;
        self.state_guard.state.set(OperationState::Succeeded);
        self.state_guard.disarm();
        Some((output, controller))
    }
}

/// State transition produced after consuming and cleaning up one attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeAttemptCompletionDisposition {
    RetryPending,
    OpeningFailed(MetadataOpeningFailureSignal),
    ObjectChanged(ObjectChangeDisposition),
    Cancelled,
    Stale,
}

/// Completion of a typed Chrome transport future without separating its output from ownership.
#[cfg(target_arch = "wasm32")]
pub(crate) enum RangeAttemptTransportCompletion<Success, Attempt> {
    Succeeded(Success),
    Failed(RangeAttemptCompletionDisposition),
    Rejected(RejectedRangeAttempt<Attempt>),
}

/// Completion that preserves the successful physical attempt's abort capability for handoff.
#[cfg(target_arch = "wasm32")]
pub(crate) enum RangeAttemptTransportCompletionWithController<Success, Controller, Attempt> {
    Succeeded(Success, Controller),
    Failed(RangeAttemptCompletionDisposition),
    Rejected(RejectedRangeAttempt<Attempt>),
}

#[cfg(target_arch = "wasm32")]
impl<Success, Attempt> fmt::Debug for RangeAttemptTransportCompletion<Success, Attempt> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Succeeded(_) => formatter.write_str("Succeeded(<retained>)"),
            Self::Failed(disposition) => {
                formatter.debug_tuple("Failed").field(disposition).finish()
            }
            Self::Rejected(rejected) => rejected.fmt(formatter),
        }
    }
}

fn is_retryable_attempt_failure(error: ChromeRangeError) -> bool {
    match error {
        ChromeRangeError::BrowserFetchUnavailable => true,
        ChromeRangeError::UnexpectedHttpStatus(429 | 500 | 502 | 503 | 504) => true,
        ChromeRangeError::InvalidRequestRange
        | ChromeRangeError::BrowserAdapterUnavailable
        | ChromeRangeError::RangeUnsupported
        | ChromeRangeError::AuthorizationRejected
        | ChromeRangeError::ObjectUnavailable
        | ChromeRangeError::PreconditionFailed
        | ChromeRangeError::RangeNotSatisfiable
        | ChromeRangeError::UnexpectedHttpStatus(_)
        | ChromeRangeError::RequiredResponseHeaderUnavailable(_)
        | ChromeRangeError::InvalidContentRange
        | ChromeRangeError::InvalidContentLength
        | ChromeRangeError::UnsupportedContentEncoding
        | ChromeRangeError::InvalidValidatorHeader
        | ChromeRangeError::StrongValidatorRequired
        | ChromeRangeError::ObjectChanged
        | ChromeRangeError::ResourceLimit => false,
        #[cfg(target_arch = "wasm32")]
        ChromeRangeError::BodyPump(_) => false,
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) struct ChromeRangeAttemptAbortFactory;

#[cfg(target_arch = "wasm32")]
pub(crate) struct ChromeRangeAttemptAbortController {
    controller: web_sys::AbortController,
    abort_on_drop: bool,
}

#[cfg(target_arch = "wasm32")]
impl ChromeRangeAttemptAbortController {
    pub(crate) fn controller(&self) -> &web_sys::AbortController {
        &self.controller
    }
}

#[cfg(target_arch = "wasm32")]
impl RangeAttemptAbortController for ChromeRangeAttemptAbortController {
    fn abort(&mut self) {
        if self.abort_on_drop {
            self.controller.abort();
            self.abort_on_drop = false;
        }
    }

    fn finish(&mut self) {
        self.abort_on_drop = false;
    }
}

#[cfg(target_arch = "wasm32")]
impl Drop for ChromeRangeAttemptAbortController {
    fn drop(&mut self) {
        self.abort();
    }
}

#[cfg(target_arch = "wasm32")]
impl RangeAttemptAbortFactory for ChromeRangeAttemptAbortFactory {
    type Controller = ChromeRangeAttemptAbortController;

    fn create(&mut self) -> Result<Self::Controller, ChromeRangeError> {
        Ok(ChromeRangeAttemptAbortController {
            controller: web_sys::AbortController::new()
                .map_err(|_error| ChromeRangeError::BrowserAdapterUnavailable)?,
            abort_on_drop: true,
        })
    }
}

/// Constructors for opaque execution evidence, compiled only into test artifacts.
#[cfg(all(test, target_arch = "wasm32"))]
pub(crate) mod test_execution_support {
    use super::*;

    pub(crate) struct TestVisibleExecutionIssuer {
        binding: ActiveVisibleExecutionBinding,
        next_turn: u64,
    }

    impl TestVisibleExecutionIssuer {
        pub(crate) fn new(identity: u64) -> Self {
            Self {
                binding: ActiveVisibleExecutionBinding {
                    issuer: AbstractExecutionIssuerId(
                        NonZeroU64::new(identity).expect("test issuer identity is non-zero"),
                    ),
                    epoch: NonZeroU64::MIN,
                    baseline: NonZeroU64::MIN,
                },
                next_turn: 0,
            }
        }

        pub(crate) const fn binding(&self) -> ActiveVisibleExecutionBinding {
            self.binding
        }

        pub(crate) fn turn(&mut self) -> ExternalVisibleControlTurn {
            self.next_turn = self
                .next_turn
                .checked_add(1)
                .expect("test turn identity does not overflow");
            ExternalVisibleControlTurn {
                binding: self.binding,
                sequence: NonZeroU64::new(self.next_turn).expect("test turn identity is non-zero"),
                projection: Cell::new(TurnProjectionState::Available),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use super::*;
    #[cfg(target_arch = "wasm32")]
    use crate::chrome_byob::ExactLengthByobPumpError;
    use crate::remote_limits::WebRemoteLimitKey;
    use crate::remote_limits::tests::test_profile_with;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Source(u64);
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Session(u64);
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Representation(u64);
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Demand(u64);

    struct PendingPayload {
        dropped: Rc<Cell<bool>>,
        _pin: std::marker::PhantomPinned,
    }

    impl std::future::Future for PendingPayload {
        type Output = u64;

        fn poll(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Self::Output> {
            Poll::Pending
        }
    }

    impl Drop for PendingPayload {
        fn drop(&mut self) {
            self.dropped.set(true);
        }
    }

    struct ReadyOutput {
        dropped: Rc<Cell<bool>>,
    }

    impl Drop for ReadyOutput {
        fn drop(&mut self) {
            self.dropped.set(true);
        }
    }

    struct ReadyPayload {
        output: Option<ReadyOutput>,
    }

    impl std::future::Future for ReadyPayload {
        type Output = ReadyOutput;

        fn poll(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Self::Output> {
            Poll::Ready(
                self.output
                    .take()
                    .expect("ready test payload is polled exactly once"),
            )
        }
    }

    struct TestPreparedRangeIdentity {
        range: ChromeRangeRequest,
        controller_identity: u64,
        accounting: RangeAttemptAccountingBinding,
    }

    impl TestPreparedRangeIdentity {
        fn matching(controller: &FakeAbortController, range: ChromeRangeRequest) -> Self {
            Self {
                range,
                controller_identity: controller.identity,
                accounting: controller.accounting,
            }
        }
    }

    impl PreparedChromeRangeIdentity<FakeAbortController> for TestPreparedRangeIdentity {
        fn prepared_range(&self) -> ChromeRangeRequest {
            self.range
        }

        fn matches_abort_controller(&self, controller: &FakeAbortController) -> bool {
            self.controller_identity == controller.identity
        }

        fn prepared_accounting_binding(&self) -> RangeAttemptAccountingBinding {
            self.accounting
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum FakeExecutionState {
        Visible(ActiveVisibleExecutionBinding),
        Hidden { suspension: NonZeroU64 },
        Terminated,
    }

    /// Test-only model of the future MCAP-065 issuer.
    struct FakeAbstractExecutionIssuer {
        issuer: AbstractExecutionIssuerId,
        next_epoch: u64,
        next_baseline: u64,
        next_suspension: u64,
        next_turn: u64,
        state: FakeExecutionState,
    }

    impl FakeAbstractExecutionIssuer {
        fn new(identity: u64) -> Self {
            let issuer = AbstractExecutionIssuerId(NonZeroU64::new(identity).unwrap());
            let initial = ActiveVisibleExecutionBinding {
                issuer,
                epoch: NonZeroU64::MIN,
                baseline: NonZeroU64::MIN,
            };
            Self {
                issuer,
                next_epoch: 1,
                next_baseline: 1,
                next_suspension: 0,
                next_turn: 0,
                state: FakeExecutionState::Visible(initial),
            }
        }

        fn binding(&self) -> ActiveVisibleExecutionBinding {
            let FakeExecutionState::Visible(binding) = self.state else {
                panic!("test issuer is not visible");
            };
            binding
        }

        fn turn(&mut self) -> ExternalVisibleControlTurn {
            self.next_turn = self.next_turn.checked_add(1).unwrap();
            ExternalVisibleControlTurn {
                binding: self.binding(),
                sequence: NonZeroU64::new(self.next_turn).unwrap(),
                projection: Cell::new(TurnProjectionState::Available),
            }
        }

        fn hide(&mut self) -> HiddenExecutionEvidence {
            let previous = self.binding();
            self.next_suspension = self.next_suspension.checked_add(1).unwrap();
            let suspension = NonZeroU64::new(self.next_suspension).unwrap();
            self.state = FakeExecutionState::Hidden { suspension };
            HiddenExecutionEvidence {
                previous,
                suspension,
            }
        }

        fn resume(&mut self) -> VisibleExecutionRebindEvidence {
            let FakeExecutionState::Hidden { suspension } = self.state else {
                panic!("test issuer is not hidden");
            };
            self.next_epoch = self.next_epoch.checked_add(1).unwrap();
            self.next_baseline = self.next_baseline.checked_add(1).unwrap();
            let current = ActiveVisibleExecutionBinding {
                issuer: self.issuer,
                epoch: NonZeroU64::new(self.next_epoch).unwrap(),
                baseline: NonZeroU64::new(self.next_baseline).unwrap(),
            };
            self.state = FakeExecutionState::Visible(current);
            VisibleExecutionRebindEvidence {
                suspension,
                current,
            }
        }

        fn timeout(&self, attempt: RangeAttemptToken) -> ActiveVisibleTimeoutEvidence {
            ActiveVisibleTimeoutEvidence {
                binding: self.binding(),
                attempt,
            }
        }

        fn terminate(&mut self) -> PageTerminationEvidence {
            self.state = FakeExecutionState::Terminated;
            PageTerminationEvidence {
                issuer: self.issuer,
            }
        }
    }

    #[derive(Default)]
    struct FakeAbortState {
        created: u64,
        aborted: u64,
        finished: u64,
        next_identity: u64,
    }

    #[derive(Clone)]
    struct FakeAbortFactory(Rc<RefCell<FakeAbortState>>, RangeAttemptAccountingBinding);

    struct FakeAbortController {
        identity: u64,
        state: Rc<RefCell<FakeAbortState>>,
        accounting: RangeAttemptAccountingBinding,
        abort_on_drop: bool,
    }

    impl RangeAttemptAbortController for FakeAbortController {
        fn abort(&mut self) {
            if self.abort_on_drop {
                self.state.borrow_mut().aborted += 1;
                self.abort_on_drop = false;
            }
        }

        fn finish(&mut self) {
            if self.abort_on_drop {
                self.state.borrow_mut().finished += 1;
                self.abort_on_drop = false;
            }
        }
    }

    impl Drop for FakeAbortController {
        fn drop(&mut self) {
            self.abort();
        }
    }

    impl RangeAttemptAbortFactory for FakeAbortFactory {
        type Controller = FakeAbortController;

        fn create(&mut self) -> Result<Self::Controller, ChromeRangeError> {
            let identity = {
                let mut state = self.0.borrow_mut();
                state.created += 1;
                state.next_identity += 1;
                state.next_identity
            };
            Ok(FakeAbortController {
                identity,
                state: Rc::clone(&self.0),
                accounting: self.1,
                abort_on_drop: true,
            })
        }
    }

    struct Fixture {
        _root: crate::remote_limits::WasmModuleLimitAccountingRoot,
        source_scope: SourceAccountingScope,
        range_scope: RangeResponseAccountingScope,
        work_scope: WorkUnitAccountingScope,
        operation_scope: OperationAccountingScope,
        coordinator: MetadataOpeningRetryCoordinator<Source, Session, Representation>,
        execution: FakeAbstractExecutionIssuer,
        aborts: Rc<RefCell<FakeAbortState>>,
    }

    impl Fixture {
        fn new(attempts: u64, ranges: u64, deadline: u64) -> Self {
            let root = test_profile_with(&[
                (WebRemoteLimitKey::RangeRetryAttemptsPerOperation, attempts),
                (WebRemoteLimitKey::MetadataOpeningRangeRequests, ranges),
                (
                    WebRemoteLimitKey::MetadataOpeningVisibleDeadlineMillis,
                    deadline,
                ),
            ])
            .start_accounting_root()
            .unwrap();
            let viewer = root.create_viewer_scope().unwrap();
            let source_scope = viewer.create_source_scope().unwrap();
            let session_scope = source_scope.create_session_scope().unwrap();
            let range_scope = session_scope.create_range_response_scope().unwrap();
            let work_scope = range_scope.create_work_unit_scope().unwrap();
            let operation_scope = source_scope.create_operation_scope().unwrap();
            let live = live(1, 1, 1);
            let execution = FakeAbstractExecutionIssuer::new(1);
            let coordinator =
                MetadataOpeningRetryCoordinator::new(&source_scope, live, execution.binding())
                    .unwrap();
            Self {
                _root: root,
                source_scope,
                range_scope,
                work_scope,
                operation_scope,
                coordinator,
                execution,
                aborts: Rc::new(RefCell::new(FakeAbortState::default())),
            }
        }

        fn abort_factory(&self) -> FakeAbortFactory {
            FakeAbortFactory(
                Rc::clone(&self.aborts),
                self.range_scope
                    .range_attempt_accounting_binding(&self.work_scope)
                    .unwrap(),
            )
        }

        fn operation(
            &self,
            demand: u64,
        ) -> RemoteRangeRetryOperation<Source, Session, Representation, Demand, FakeAbortFactory>
        {
            RemoteRangeRetryOperation::new(
                &self.operation_scope,
                &self.range_scope,
                &self.work_scope,
                live(1, 1, 1),
                ChromeRangeRequest::new(0..8).unwrap(),
                Demand(demand),
                self.abort_factory(),
            )
            .unwrap()
        }

        fn fresh_operation(
            &self,
            demand: u64,
            exact_range: Range<u64>,
        ) -> RemoteRangeRetryOperation<Source, Session, Representation, Demand, FakeAbortFactory>
        {
            let operation_scope = self.source_scope.create_operation_scope().unwrap();
            RemoteRangeRetryOperation::new(
                &operation_scope,
                &self.range_scope,
                &self.work_scope,
                live(1, 1, 1),
                ChromeRangeRequest::new(exact_range).unwrap(),
                Demand(demand),
                self.abort_factory(),
            )
            .unwrap()
        }
    }

    fn live(
        source: u64,
        session: u64,
        representation: u64,
    ) -> RemoteRangeLiveIdentity<Source, Session, Representation> {
        RemoteRangeLiveIdentity::new(
            Source(source),
            Session(session),
            Representation(representation),
        )
    }

    fn start(
        operation: &mut RemoteRangeRetryOperation<
            Source,
            Session,
            Representation,
            Demand,
            FakeAbortFactory,
        >,
        coordinator: &mut MetadataOpeningRetryCoordinator<Source, Session, Representation>,
    ) -> StartedRangeAttempt<Source, Session, Representation, Demand, FakeAbortController, u64>
    {
        operation
            .start_initial(
                coordinator,
                ActiveVisibleExecutionBinding {
                    issuer: AbstractExecutionIssuerId(NonZeroU64::MIN),
                    epoch: NonZeroU64::MIN,
                    baseline: NonZeroU64::MIN,
                },
                |controller, request| Ok(TestPreparedRangeIdentity::matching(controller, request)),
                |controller, _prepared, receipt| {
                    assert_eq!(receipt.attempt().get(), 1);
                    controller.identity
                },
            )
            .unwrap()
    }

    fn start_pending(
        operation: &mut RemoteRangeRetryOperation<
            Source,
            Session,
            Representation,
            Demand,
            FakeAbortFactory,
        >,
        fixture: &mut Fixture,
        dropped: Rc<Cell<bool>>,
    ) -> StartedRangeAttempt<
        Source,
        Session,
        Representation,
        Demand,
        FakeAbortController,
        PendingPayload,
    > {
        operation
            .start_initial(
                &mut fixture.coordinator,
                fixture.execution.binding(),
                |controller, request| Ok(TestPreparedRangeIdentity::matching(controller, request)),
                |_controller, _prepared, _receipt| PendingPayload {
                    dropped,
                    _pin: std::marker::PhantomPinned,
                },
            )
            .unwrap()
    }

    fn start_ready(
        operation: &mut RemoteRangeRetryOperation<
            Source,
            Session,
            Representation,
            Demand,
            FakeAbortFactory,
        >,
        fixture: &mut Fixture,
        output_dropped: Rc<Cell<bool>>,
    ) -> StartedRangeAttempt<
        Source,
        Session,
        Representation,
        Demand,
        FakeAbortController,
        ReadyPayload,
    > {
        operation
            .start_initial(
                &mut fixture.coordinator,
                fixture.execution.binding(),
                |controller, request| Ok(TestPreparedRangeIdentity::matching(controller, request)),
                |_controller, _prepared, _receipt| ReadyPayload {
                    output: Some(ReadyOutput {
                        dropped: output_dropped,
                    }),
                },
            )
            .unwrap()
    }

    fn poll_ready<Source, Session, Representation, Demand, Controller>(
        mut attempt: Pin<
            &mut StartedRangeAttempt<
                Source,
                Session,
                Representation,
                Demand,
                Controller,
                ReadyPayload,
            >,
        >,
    ) -> SettledRangeAttempt<Source, Session, Representation, Demand, Controller, ReadyOutput>
    where
        Controller: RangeAttemptAbortController,
    {
        let mut context = Context::from_waker(std::task::Waker::noop());
        match attempt.as_mut().poll_settlement(&mut context) {
            Poll::Ready(Ok(settled)) => settled,
            Poll::Ready(Err(error)) => panic!("ready attempt failed to settle: {error}"),
            Poll::Pending => panic!("ready attempt unexpectedly remained pending"),
        }
    }

    fn assert_pending<Source, Session, Representation, Demand, Controller>(
        attempt: Pin<
            &mut StartedRangeAttempt<
                Source,
                Session,
                Representation,
                Demand,
                Controller,
                PendingPayload,
            >,
        >,
    ) where
        Controller: RangeAttemptAbortController,
    {
        let mut context = Context::from_waker(std::task::Waker::noop());
        assert!(matches!(
            attempt.poll_settlement(&mut context),
            Poll::Pending
        ));
    }

    #[test]
    fn zero_burn_preflight_failure_and_settlement_failure_accounting_are_distinct() {
        let mut fixture = Fixture::new(2, 2, 100);
        let mut failed_preflight = fixture.operation(1);
        assert!(matches!(
            failed_preflight.start_initial(
                &mut fixture.coordinator,
                fixture.execution.binding(),
                |_controller, _request| {
                    Err::<TestPreparedRangeIdentity, _>(ChromeRangeError::InvalidRequestRange)
                },
                |_controller, _prepared, _receipt| (),
            ),
            Err(RangeAttemptStartFailure::Request(
                RangeAttemptStartError::OpeningFailed(MetadataOpeningFailureSignal::NonRetryable(
                    ChromeRangeError::InvalidRequestRange
                ))
            ))
        ));
        assert_eq!(failed_preflight.attempts_burned(), 0);
        assert_eq!(fixture.coordinator.range_count(), 0);

        let operation_scope = fixture.source_scope.create_operation_scope().unwrap();
        let mut operation = RemoteRangeRetryOperation::new(
            &operation_scope,
            &fixture.range_scope,
            &fixture.work_scope,
            live(1, 1, 1),
            ChromeRangeRequest::new(0..8).unwrap(),
            Demand(2),
            fixture.abort_factory(),
        )
        .unwrap();
        let started = start(&mut operation, &mut fixture.coordinator);
        assert_eq!(operation.attempts_burned(), 1);
        assert_eq!(fixture.coordinator.range_count(), 1);
        assert_eq!(
            operation
                .complete_failure(
                    &mut fixture.coordinator,
                    started.settle_immediate(),
                    ChromeRangeError::BrowserFetchUnavailable,
                )
                .unwrap(),
            RangeAttemptCompletionDisposition::RetryPending
        );
        assert_eq!(operation.attempts_burned(), 1);
        assert_eq!(fixture.coordinator.range_count(), 1);
    }

    #[test]
    fn object_change_at_any_pre_fetch_boundary_is_zero_burn_session_fatal() {
        for error in [
            ChromeRangeError::ObjectChanged,
            ChromeRangeError::PreconditionFailed,
        ] {
            let mut fixture = Fixture::new(2, 2, 100);
            let mut operation = fixture.operation(1);
            assert!(matches!(
                operation.start_initial(
                    &mut fixture.coordinator,
                    fixture.execution.binding(),
                    |_controller, _request| Err::<TestPreparedRangeIdentity, _>(error),
                    |_controller, _prepared, _receipt| {
                        panic!("Fetch boundary must remain disarmed")
                    },
                ),
                Err(RangeAttemptStartFailure::Request(
                    RangeAttemptStartError::ObjectChanged(ObjectChangeDisposition::Latched)
                ))
            ));
            assert_eq!(operation.attempts_burned(), 0);
            assert_eq!(fixture.coordinator.range_count(), 0);
            assert!(!fixture.coordinator.can_refetch_chunks());
            assert_eq!(
                fixture.coordinator.terminal(),
                Some(RetrySessionTerminal::ObjectChanged)
            );
            let state = fixture.aborts.borrow();
            assert_eq!(state.created, 1);
            assert_eq!(state.aborted, 1);
            assert_eq!(state.finished, 0);
        }
    }

    #[test]
    fn prepared_range_mismatch_is_zero_burn_for_initial_and_retry_attempts() {
        let mut initial_fixture = Fixture::new(2, 2, 100);
        let mut initial = initial_fixture.operation(1);
        assert!(matches!(
            initial.start_initial(
                &mut initial_fixture.coordinator,
                initial_fixture.execution.binding(),
                |controller, authoritative| {
                    assert_eq!(authoritative.as_range(), 0..8);
                    let mut prepared =
                        TestPreparedRangeIdentity::matching(controller, authoritative);
                    prepared.range = ChromeRangeRequest::new(8..16).unwrap();
                    Ok(prepared)
                },
                |_controller, _prepared, _receipt| {
                    panic!("mismatched initial Range must not cross Fetch")
                },
            ),
            Err(RangeAttemptStartFailure::PreparedMismatch(
                RejectedPreparedAttempt {
                    kind: PreparedAttemptMismatchKind::ExactRange,
                    ..
                }
            ))
        ));
        assert_eq!(initial.attempts_burned(), 0);
        assert_eq!(initial_fixture.coordinator.range_count(), 0);
        assert_eq!(initial_fixture.aborts.borrow().aborted, 1);
        assert_eq!(initial.state(), OperationState::Initial);
        let mut correct_initial = start(&mut initial, &mut initial_fixture.coordinator);
        initial
            .cancel_active(Pin::new(&mut correct_initial))
            .unwrap();

        let mut retry_fixture = Fixture::new(3, 3, 100);
        let mut retry = retry_fixture.operation(1);
        let first = start(&mut retry, &mut retry_fixture.coordinator);
        retry
            .complete_failure(
                &mut retry_fixture.coordinator,
                first.settle_immediate(),
                ChromeRangeError::BrowserFetchUnavailable,
            )
            .unwrap();
        let external_turn = retry_fixture.execution.turn();
        let mut permit = retry_fixture
            .coordinator
            .project_retry_turn(&external_turn)
            .unwrap();
        assert!(matches!(
            retry.start_retry_on_turn(
                &mut retry_fixture.coordinator,
                &mut permit,
                |controller, authoritative| {
                    assert_eq!(authoritative.as_range(), 0..8);
                    let mut prepared =
                        TestPreparedRangeIdentity::matching(controller, authoritative);
                    prepared.range = ChromeRangeRequest::new(16..24).unwrap();
                    Ok(prepared)
                },
                |_controller, _prepared, _receipt| {
                    panic!("mismatched retry Range must not cross Fetch")
                },
            ),
            Err(RangeAttemptStartFailure::PreparedMismatch(
                RejectedPreparedAttempt {
                    kind: PreparedAttemptMismatchKind::ExactRange,
                    ..
                }
            ))
        ));
        assert_eq!(retry.attempts_burned(), 1);
        assert_eq!(retry_fixture.coordinator.range_count(), 1);
        assert_eq!(retry_fixture.aborts.borrow().aborted, 2);
        assert_eq!(
            retry.state(),
            OperationState::RetryPending {
                eligible_after_turn: 0
            }
        );
        let mut correct_retry = retry
            .start_retry_on_turn(
                &mut retry_fixture.coordinator,
                &mut permit,
                |controller, request| Ok(TestPreparedRangeIdentity::matching(controller, request)),
                |controller, _prepared, _receipt| controller.identity,
            )
            .expect("the same unclaimed turn starts a corrected prepared owner");
        retry.cancel_active(Pin::new(&mut correct_retry)).unwrap();
    }

    #[test]
    fn prepared_controller_and_accounting_cross_wires_return_both_owners_without_state_change() {
        let mut fixture = Fixture::new(3, 3, 100);
        let mut operation = fixture.operation(1);
        let rejection = operation
            .start_initial(
                &mut fixture.coordinator,
                fixture.execution.binding(),
                |controller, request| {
                    let mut prepared = TestPreparedRangeIdentity::matching(controller, request);
                    prepared.controller_identity += 1;
                    Ok(prepared)
                },
                |_controller, _prepared, _receipt| panic!("wrong controller must not cross Fetch"),
            )
            .unwrap_err();
        let RangeAttemptStartFailure::PreparedMismatch(rejection) = rejection else {
            panic!("controller mismatch must retain both prepared owners")
        };
        assert_eq!(
            rejection.kind(),
            PreparedAttemptMismatchKind::AbortController
        );
        let (prepared, controller) = rejection.into_owners();
        assert_eq!(prepared.range, ChromeRangeRequest::new(0..8).unwrap());
        assert_eq!(controller.identity, 1);
        drop(controller);
        assert_eq!(operation.state(), OperationState::Initial);
        assert_eq!(operation.attempts_burned(), 0);
        assert_eq!(fixture.coordinator.range_count(), 0);
        assert_eq!(fixture.aborts.borrow().aborted, 1);

        let other_session = fixture.source_scope.create_session_scope().unwrap();
        let other_range = other_session.create_range_response_scope().unwrap();
        let other_work = other_range.create_work_unit_scope().unwrap();
        let wrong_accounting = other_range
            .range_attempt_accounting_binding(&other_work)
            .unwrap();
        assert!(matches!(
            operation.start_initial(
                &mut fixture.coordinator,
                fixture.execution.binding(),
                |controller, request| {
                    let mut prepared = TestPreparedRangeIdentity::matching(controller, request);
                    prepared.accounting = wrong_accounting;
                    Ok(prepared)
                },
                |_controller, _prepared, _receipt| {
                    panic!("wrong accounting scope must not cross Fetch")
                },
            ),
            Err(RangeAttemptStartFailure::PreparedMismatch(
                RejectedPreparedAttempt {
                    kind: PreparedAttemptMismatchKind::AccountingAncestry,
                    ..
                }
            ))
        ));
        assert_eq!(operation.state(), OperationState::Initial);
        assert_eq!(operation.attempts_burned(), 0);
        assert_eq!(fixture.coordinator.range_count(), 0);
        assert_eq!(fixture.aborts.borrow().aborted, 2);

        let other_root = test_profile_with(&[]).start_accounting_root().unwrap();
        let other_viewer = other_root.create_viewer_scope().unwrap();
        let other_source = other_viewer.create_source_scope().unwrap();
        let other_session = other_source.create_session_scope().unwrap();
        let other_range = other_session.create_range_response_scope().unwrap();
        let other_work = other_range.create_work_unit_scope().unwrap();
        let other_operation = other_source.create_operation_scope().unwrap();
        assert!(matches!(
            RemoteRangeRetryOperation::new(
                &other_operation,
                &fixture.range_scope,
                &fixture.work_scope,
                live(2, 2, 2),
                ChromeRangeRequest::new(0..8).unwrap(),
                Demand(1),
                FakeAbortFactory(
                    Rc::clone(&fixture.aborts),
                    other_range
                        .range_attempt_accounting_binding(&other_work)
                        .unwrap(),
                ),
            ),
            Err(RangeAttemptStartError::AccountingAncestryMismatch)
        ));
    }

    #[test]
    fn wrong_source_coordinator_never_cancels_initial_or_retry_pending_operation() {
        let mut fixture = Fixture::new(3, 3, 100);
        let other_root = test_profile_with(&[]).start_accounting_root().unwrap();
        let other_viewer = other_root.create_viewer_scope().unwrap();
        let other_source = other_viewer.create_source_scope().unwrap();
        let mut other_coordinator = MetadataOpeningRetryCoordinator::new(
            &other_source,
            live(2, 1, 1),
            fixture.execution.binding(),
        )
        .unwrap();

        let mut initial = fixture.operation(1);
        assert!(matches!(
            initial.start_initial(
                &mut other_coordinator,
                fixture.execution.binding(),
                |_controller, _request| -> Result<TestPreparedRangeIdentity, ChromeRangeError> {
                    panic!("wrong-source start must fail before controller/preflight")
                },
                |_controller, _prepared, _receipt| (),
            ),
            Err(RangeAttemptStartFailure::Request(
                RangeAttemptStartError::Protocol(RetryProtocolError::AttemptMismatch)
            ))
        ));
        assert_eq!(initial.attempts_burned(), 0);
        assert_eq!(other_coordinator.range_count(), 0);
        let mut initial_attempt = start(&mut initial, &mut fixture.coordinator);
        initial
            .cancel_active(Pin::new(&mut initial_attempt))
            .unwrap();

        let mut pending = fixture.fresh_operation(2, 8..16);
        let first = start(&mut pending, &mut fixture.coordinator);
        pending
            .complete_failure(
                &mut fixture.coordinator,
                first.settle_immediate(),
                ChromeRangeError::BrowserFetchUnavailable,
            )
            .unwrap();
        let external_turn = fixture.execution.turn();
        let mut wrong_permit = other_coordinator
            .project_retry_turn(&external_turn)
            .unwrap();
        assert!(matches!(
            pending.start_retry_on_turn(
                &mut other_coordinator,
                &mut wrong_permit,
                |_controller, _request| -> Result<TestPreparedRangeIdentity, ChromeRangeError> {
                    panic!("wrong-source retry must fail before controller/preflight")
                },
                |_controller, _prepared, _receipt| (),
            ),
            Err(RangeAttemptStartFailure::Request(
                RangeAttemptStartError::Protocol(RetryProtocolError::AttemptMismatch)
            ))
        ));
        assert_eq!(pending.attempts_burned(), 1);
        assert_eq!(other_coordinator.range_count(), 0);
        drop(wrong_permit);
        let mut correct_permit = fixture
            .coordinator
            .project_retry_turn(&external_turn)
            .unwrap();
        let mut retry = pending
            .start_retry_on_turn(
                &mut fixture.coordinator,
                &mut correct_permit,
                |controller, request| Ok(TestPreparedRangeIdentity::matching(controller, request)),
                |controller, _prepared, _receipt| controller.identity,
            )
            .unwrap();
        pending.cancel_active(Pin::new(&mut retry)).unwrap();
    }

    #[test]
    fn wrong_source_close_and_page_termination_are_zero_effect_active_and_inactive() {
        let mut fixture = Fixture::new(3, 4, 100);
        let other_root = test_profile_with(&[]).start_accounting_root().unwrap();
        let other_viewer = other_root.create_viewer_scope().unwrap();
        let other_source = other_viewer.create_source_scope().unwrap();
        let mut other_coordinator = MetadataOpeningRetryCoordinator::new(
            &other_source,
            live(2, 1, 1),
            fixture.execution.binding(),
        )
        .unwrap();

        let mut active_close = fixture.operation(1);
        let mut active_close_attempt = start(&mut active_close, &mut fixture.coordinator);
        let rejection = active_close
            .close_active(&mut other_coordinator, Pin::new(&mut active_close_attempt))
            .unwrap_err();
        assert_eq!(rejection, RetryProtocolError::AttemptMismatch);
        assert_eq!(fixture.coordinator.terminal(), None);
        assert_eq!(other_coordinator.terminal(), None);
        assert_eq!(fixture.aborts.borrow().aborted, 0);
        active_close
            .close_active(
                &mut fixture.coordinator,
                Pin::new(&mut active_close_attempt),
            )
            .unwrap();
        assert_eq!(
            fixture.coordinator.terminal(),
            Some(RetrySessionTerminal::ExplicitlyClosed)
        );
        assert_eq!(other_coordinator.terminal(), None);

        let mut page_fixture = Fixture::new(3, 4, 100);
        let page_other_root = test_profile_with(&[]).start_accounting_root().unwrap();
        let page_other_viewer = page_other_root.create_viewer_scope().unwrap();
        let page_other_source = page_other_viewer.create_source_scope().unwrap();
        let mut page_other = MetadataOpeningRetryCoordinator::new(
            &page_other_source,
            live(2, 1, 1),
            page_fixture.execution.binding(),
        )
        .unwrap();
        let mut active_page = page_fixture.operation(1);
        let mut active_page_attempt = start(&mut active_page, &mut page_fixture.coordinator);
        let page_evidence = page_fixture.execution.terminate();
        let rejection = active_page
            .page_terminate_active(
                &mut page_other,
                page_evidence,
                Pin::new(&mut active_page_attempt),
            )
            .unwrap_err();
        assert_eq!(rejection, RetryProtocolError::AttemptMismatch);
        assert_eq!(page_fixture.coordinator.terminal(), None);
        assert_eq!(page_other.terminal(), None);
        active_page
            .page_terminate_active(
                &mut page_fixture.coordinator,
                page_evidence,
                Pin::new(&mut active_page_attempt),
            )
            .unwrap();
        assert_eq!(
            page_fixture.coordinator.terminal(),
            Some(RetrySessionTerminal::PageTerminated)
        );
        assert_eq!(page_other.terminal(), None);

        let inactive_close = fixture.fresh_operation(2, 16..24);
        assert_eq!(
            inactive_close.close_inactive(&mut other_coordinator),
            Err(RetryProtocolError::AttemptMismatch)
        );
        assert_eq!(inactive_close.state(), OperationState::Initial);
        assert_eq!(other_coordinator.terminal(), None);

        let inactive_page = page_fixture.fresh_operation(2, 16..24);
        assert_eq!(
            inactive_page.page_terminate_inactive(&mut page_other, page_evidence),
            Err(RetryProtocolError::AttemptMismatch)
        );
        assert_eq!(inactive_page.state(), OperationState::Initial);
        assert_eq!(page_other.terminal(), None);
    }

    #[test]
    fn retry_is_next_visible_turn_only_and_each_turn_has_one_global_claim() {
        let mut fixture = Fixture::new(3, 4, 100);
        let mut operation = fixture.operation(1);
        assert_eq!(operation.exact_range(), 0..8);
        let started = start(&mut operation, &mut fixture.coordinator);
        assert_eq!(
            format!("{:?}", started.callback_identity().unwrap()),
            "RangeAttemptCallbackIdentity(<opaque>)"
        );
        operation
            .complete_failure(
                &mut fixture.coordinator,
                started.settle_immediate(),
                ChromeRangeError::UnexpectedHttpStatus(503),
            )
            .unwrap();

        let operation_scope = fixture.source_scope.create_operation_scope().unwrap();
        let mut other = RemoteRangeRetryOperation::new(
            &operation_scope,
            &fixture.range_scope,
            &fixture.work_scope,
            live(1, 1, 1),
            ChromeRangeRequest::new(8..16).unwrap(),
            Demand(2),
            fixture.abort_factory(),
        )
        .unwrap();
        let other_started = start(&mut other, &mut fixture.coordinator);
        other
            .complete_failure(
                &mut fixture.coordinator,
                other_started.settle_immediate(),
                ChromeRangeError::BrowserFetchUnavailable,
            )
            .unwrap();

        let external_turn = fixture.execution.turn();
        let mut turn = fixture
            .coordinator
            .project_retry_turn(&external_turn)
            .unwrap();
        let retry = operation
            .start_retry_on_turn(
                &mut fixture.coordinator,
                &mut turn,
                |controller, request| Ok(TestPreparedRangeIdentity::matching(controller, request)),
                |controller, _prepared, receipt| {
                    assert_eq!(receipt.attempt().get(), 2);
                    controller.identity
                },
            )
            .unwrap();
        assert_eq!(retry.token().unwrap().ordinal(), 2);
        assert_eq!(operation.attempts_burned(), 2);
        assert_eq!(fixture.coordinator.range_count(), 3);

        assert!(matches!(
            other.start_retry_on_turn(
                &mut fixture.coordinator,
                &mut turn,
                |controller, request| {
                    Ok(TestPreparedRangeIdentity::matching(controller, request))
                },
                |_controller, _prepared, _receipt| (),
            ),
            Err(RangeAttemptStartFailure::Request(
                RangeAttemptStartError::Protocol(RetryProtocolError::TurnAlreadyClaimed)
            ))
        ));
        let mut retry = retry;
        operation.cancel_active(Pin::new(&mut retry)).unwrap();
    }

    #[test]
    fn external_turn_claim_is_global_across_sources_and_cannot_be_replayed() {
        let root = test_profile_with(&[
            (WebRemoteLimitKey::RangeRetryAttemptsPerOperation, 2),
            (WebRemoteLimitKey::MetadataOpeningRangeRequests, 2),
        ])
        .start_accounting_root()
        .unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let source_a = viewer.create_source_scope().unwrap();
        let source_b = viewer.create_source_scope().unwrap();
        let session_a = source_a.create_session_scope().unwrap();
        let range_a = session_a.create_range_response_scope().unwrap();
        let work_a = range_a.create_work_unit_scope().unwrap();
        let session_b = source_b.create_session_scope().unwrap();
        let range_b = session_b.create_range_response_scope().unwrap();
        let work_b = range_b.create_work_unit_scope().unwrap();
        let operation_a_scope = source_a.create_operation_scope().unwrap();
        let operation_b_scope = source_b.create_operation_scope().unwrap();
        let mut execution = FakeAbstractExecutionIssuer::new(1);
        let binding = execution.binding();
        let mut coordinator_a =
            MetadataOpeningRetryCoordinator::new(&source_a, live(1, 1, 1), binding).unwrap();
        let mut coordinator_b =
            MetadataOpeningRetryCoordinator::new(&source_b, live(2, 2, 2), binding).unwrap();
        let aborts = Rc::new(RefCell::new(FakeAbortState::default()));
        let mut operation_a = RemoteRangeRetryOperation::new(
            &operation_a_scope,
            &range_a,
            &work_a,
            live(1, 1, 1),
            ChromeRangeRequest::new(0..8).unwrap(),
            Demand(1),
            FakeAbortFactory(
                Rc::clone(&aborts),
                range_a.range_attempt_accounting_binding(&work_a).unwrap(),
            ),
        )
        .unwrap();
        let mut operation_b = RemoteRangeRetryOperation::new(
            &operation_b_scope,
            &range_b,
            &work_b,
            live(2, 2, 2),
            ChromeRangeRequest::new(8..16).unwrap(),
            Demand(2),
            FakeAbortFactory(
                Rc::clone(&aborts),
                range_b.range_attempt_accounting_binding(&work_b).unwrap(),
            ),
        )
        .unwrap();
        let attempt_a = operation_a
            .start_initial(
                &mut coordinator_a,
                binding,
                |controller, request| Ok(TestPreparedRangeIdentity::matching(controller, request)),
                |_, _prepared, _| (),
            )
            .unwrap();
        operation_a
            .complete_failure(
                &mut coordinator_a,
                attempt_a.settle_immediate(),
                ChromeRangeError::BrowserFetchUnavailable,
            )
            .unwrap();
        let attempt_b = operation_b
            .start_initial(
                &mut coordinator_b,
                binding,
                |controller, request| Ok(TestPreparedRangeIdentity::matching(controller, request)),
                |_, _prepared, _| (),
            )
            .unwrap();
        operation_b
            .complete_failure(
                &mut coordinator_b,
                attempt_b.settle_immediate(),
                ChromeRangeError::BrowserFetchUnavailable,
            )
            .unwrap();

        let external_turn = execution.turn();
        let mut permit_a = coordinator_a.project_retry_turn(&external_turn).unwrap();
        let retry_a = operation_a
            .start_retry_on_turn(
                &mut coordinator_a,
                &mut permit_a,
                |controller, request| Ok(TestPreparedRangeIdentity::matching(controller, request)),
                |_, _prepared, _| (),
            )
            .unwrap();
        assert!(matches!(
            coordinator_b.project_retry_turn(&external_turn),
            Err(RetryProtocolError::TurnAlreadyClaimed)
        ));
        assert!(matches!(
            coordinator_a.project_retry_turn(&external_turn),
            Err(RetryProtocolError::TurnAlreadyClaimed)
        ));
        let mut retry_a = retry_a;
        operation_a.cancel_active(Pin::new(&mut retry_a)).unwrap();
    }

    #[test]
    fn cross_wired_attempt_is_returned_without_latching_close_or_losing_abort_owner() {
        let mut fixture = Fixture::new(2, 3, 100);
        let mut first = fixture.operation(1);
        let mut first_attempt = start(&mut first, &mut fixture.coordinator);
        let second_scope = fixture.source_scope.create_operation_scope().unwrap();
        let mut second = RemoteRangeRetryOperation::new(
            &second_scope,
            &fixture.range_scope,
            &fixture.work_scope,
            live(1, 1, 1),
            ChromeRangeRequest::new(8..16).unwrap(),
            Demand(2),
            fixture.abort_factory(),
        )
        .unwrap();
        let mut second_attempt = start(&mut second, &mut fixture.coordinator);

        let rejection = first
            .close_active(&mut fixture.coordinator, Pin::new(&mut second_attempt))
            .unwrap_err();
        assert_eq!(rejection, RetryProtocolError::AttemptMismatch);
        assert_eq!(fixture.coordinator.terminal(), None);
        second
            .close_active(&mut fixture.coordinator, Pin::new(&mut second_attempt))
            .unwrap();
        first.cancel_active(Pin::new(&mut first_attempt)).unwrap();
        assert_eq!(fixture.aborts.borrow().aborted, 2);
    }

    #[test]
    fn cross_wired_failure_and_object_change_return_the_live_owner() {
        for error in [
            ChromeRangeError::BrowserFetchUnavailable,
            ChromeRangeError::ObjectChanged,
        ] {
            let mut fixture = Fixture::new(2, 3, 100);
            let mut first = fixture.operation(1);
            let first_attempt = start(&mut first, &mut fixture.coordinator);
            let mut second = fixture.fresh_operation(2, 8..16);
            let second_attempt = start(&mut second, &mut fixture.coordinator);

            let rejection = first
                .complete_failure(
                    &mut fixture.coordinator,
                    second_attempt.settle_immediate(),
                    error,
                )
                .unwrap_err();
            assert_eq!(rejection.error(), RetryProtocolError::AttemptMismatch);
            assert_eq!(fixture.coordinator.terminal(), None);
            assert!(fixture.coordinator.can_refetch_chunks());
            assert_eq!(fixture.aborts.borrow().aborted, 0);

            let expected = if error == ChromeRangeError::ObjectChanged {
                RangeAttemptCompletionDisposition::ObjectChanged(ObjectChangeDisposition::Latched)
            } else {
                RangeAttemptCompletionDisposition::RetryPending
            };
            assert_eq!(
                second
                    .complete_failure(&mut fixture.coordinator, rejection.into_attempt(), error,)
                    .unwrap(),
                expected
            );
            let mut first_attempt = first_attempt;
            first.cancel_active(Pin::new(&mut first_attempt)).unwrap();
            assert_eq!(fixture.aborts.borrow().aborted, 2);
        }
    }

    #[test]
    fn cross_wired_success_returns_the_live_owner() {
        let mut fixture = Fixture::new(1, 2, 100);
        let mut first = fixture.operation(1);
        let first_attempt = start(&mut first, &mut fixture.coordinator);
        let mut second = fixture.fresh_operation(2, 8..16);
        let second_attempt = start(&mut second, &mut fixture.coordinator);

        let Err(RangeAttemptSuccessError::Rejected(rejection)) =
            first.complete_success(&fixture.coordinator, second_attempt.settle_immediate())
        else {
            panic!("cross-wired success must return the untouched owner");
        };
        assert_eq!(rejection.error(), RetryProtocolError::AttemptMismatch);
        assert_eq!(fixture.coordinator.terminal(), None);
        assert_eq!(fixture.aborts.borrow().aborted, 0);
        assert_eq!(
            second
                .complete_success(&fixture.coordinator, rejection.into_attempt())
                .unwrap(),
            2
        );
        assert_eq!(
            first
                .complete_success(&fixture.coordinator, first_attempt.settle_immediate())
                .unwrap(),
            1
        );
        let state = fixture.aborts.borrow();
        assert_eq!(state.finished, 2);
        assert_eq!(state.aborted, 0);
    }

    #[test]
    fn hidden_retry_does_not_burn_and_resume_rebinds_once() {
        let mut fixture = Fixture::new(3, 3, 10);
        let mut operation = fixture.operation(1);
        let started = start(&mut operation, &mut fixture.coordinator);
        operation
            .complete_failure(
                &mut fixture.coordinator,
                started.settle_immediate(),
                ChromeRangeError::BrowserFetchUnavailable,
            )
            .unwrap();
        let old_binding = fixture.execution.binding();
        let hidden = fixture.execution.hide();
        fixture.coordinator.apply_hidden_evidence(hidden).unwrap();
        assert_eq!(
            fixture.coordinator.advance_visible_time(old_binding, 10),
            Err(RetryProtocolError::Hidden)
        );
        assert_eq!(fixture.coordinator.range_count(), 1);
        assert_eq!(operation.attempts_burned(), 1);
        let stale_turn = ExternalVisibleControlTurn {
            binding: old_binding,
            sequence: NonZeroU64::MIN,
            projection: Cell::new(TurnProjectionState::Available),
        };
        assert!(matches!(
            fixture.coordinator.project_retry_turn(&stale_turn),
            Err(RetryProtocolError::Hidden)
        ));

        let visible = fixture.execution.resume();
        fixture
            .coordinator
            .apply_visible_rebind_evidence(visible)
            .unwrap();
        assert_eq!(
            fixture.coordinator.apply_visible_rebind_evidence(visible),
            Err(RetryProtocolError::StaleExecution)
        );
        let external_turn = fixture.execution.turn();
        let mut turn = fixture
            .coordinator
            .project_retry_turn(&external_turn)
            .unwrap();
        let retry = operation
            .start_retry_on_turn(
                &mut fixture.coordinator,
                &mut turn,
                |controller, request| Ok(TestPreparedRangeIdentity::matching(controller, request)),
                |controller, _prepared, _receipt| controller.identity,
            )
            .unwrap();
        let mut retry = retry;
        operation.cancel_active(Pin::new(&mut retry)).unwrap();
        assert_eq!(fixture.coordinator.range_count(), 2);
    }

    #[test]
    fn pre_hide_turn_projection_cannot_be_claimed_after_monotonic_resume() {
        let mut fixture = Fixture::new(3, 3, 100);
        let mut operation = fixture.operation(1);
        let started = start(&mut operation, &mut fixture.coordinator);
        operation
            .complete_failure(
                &mut fixture.coordinator,
                started.settle_immediate(),
                ChromeRangeError::BrowserFetchUnavailable,
            )
            .unwrap();

        let old_turn = fixture.execution.turn();
        let mut old_permit = fixture.coordinator.project_retry_turn(&old_turn).unwrap();
        let hidden = fixture.execution.hide();
        fixture.coordinator.apply_hidden_evidence(hidden).unwrap();
        let visible = fixture.execution.resume();
        fixture
            .coordinator
            .apply_visible_rebind_evidence(visible)
            .unwrap();
        assert!(matches!(
            operation.start_retry_on_turn(
                &mut fixture.coordinator,
                &mut old_permit,
                |_controller, _request| -> Result<TestPreparedRangeIdentity, ChromeRangeError> {
                    panic!("stale execution must fail before preflight")
                },
                |_controller, _prepared, _receipt| (),
            ),
            Err(RangeAttemptStartFailure::Request(
                RangeAttemptStartError::Protocol(RetryProtocolError::StaleExecution)
            ))
        ));
        assert_eq!(operation.attempts_burned(), 1);
        assert_eq!(fixture.coordinator.range_count(), 1);
        drop(old_permit);
        assert!(matches!(
            fixture.coordinator.project_retry_turn(&old_turn),
            Err(RetryProtocolError::StaleExecution)
        ));

        let resumed_turn = fixture.execution.turn();
        let mut resumed_permit = fixture
            .coordinator
            .project_retry_turn(&resumed_turn)
            .unwrap();
        let retry = operation
            .start_retry_on_turn(
                &mut fixture.coordinator,
                &mut resumed_permit,
                |controller, request| Ok(TestPreparedRangeIdentity::matching(controller, request)),
                |controller, _prepared, _receipt| controller.identity,
            )
            .unwrap();
        let mut retry = retry;
        operation.cancel_active(Pin::new(&mut retry)).unwrap();
    }

    #[test]
    fn source_metadata_owners_survive_sniff_handoff_and_are_shared_by_exact_operations() {
        fn handoff<Source, Session, Representation>(
            coordinator: MetadataOpeningRetryCoordinator<Source, Session, Representation>,
        ) -> MetadataOpeningRetryCoordinator<Source, Session, Representation> {
            coordinator
        }

        let mut fixture = Fixture::new(2, 3, 10);
        let mut sniff = fixture.operation(1);
        let sniff_attempt = start(&mut sniff, &mut fixture.coordinator);
        assert_eq!(sniff.attempts_burned(), 1);
        assert_eq!(fixture.coordinator.range_count(), 1);
        assert_eq!(
            fixture
                .coordinator
                .advance_visible_time(fixture.execution.binding(), 6)
                .unwrap(),
            MetadataOpeningDeadlineState::Active
        );
        let abort_factory = fixture.abort_factory();
        let mut opening = handoff(fixture.coordinator);

        let opening_scope = fixture.source_scope.create_operation_scope().unwrap();
        let mut header = RemoteRangeRetryOperation::new(
            &opening_scope,
            &fixture.range_scope,
            &fixture.work_scope,
            live(1, 1, 1),
            ChromeRangeRequest::new(8..16).unwrap(),
            Demand(2),
            abort_factory,
        )
        .unwrap();
        let header_attempt = start(&mut header, &mut opening);
        assert_eq!(header.attempts_burned(), 1);
        assert_eq!(sniff.attempts_burned(), 1);
        assert_eq!(opening.range_count(), 2);
        assert_eq!(
            opening
                .advance_visible_time(fixture.execution.binding(), 4)
                .unwrap(),
            MetadataOpeningDeadlineState::Expired
        );

        let mut sniff_attempt = sniff_attempt;
        let mut header_attempt = header_attempt;
        sniff.cancel_active(Pin::new(&mut sniff_attempt)).unwrap();
        header.cancel_active(Pin::new(&mut header_attempt)).unwrap();
    }

    #[test]
    fn failed_retry_preflight_does_not_claim_the_visible_turn() {
        let mut fixture = Fixture::new(3, 5, 100);
        let mut first = fixture.operation(1);
        let first_started = start(&mut first, &mut fixture.coordinator);
        first
            .complete_failure(
                &mut fixture.coordinator,
                first_started.settle_immediate(),
                ChromeRangeError::BrowserFetchUnavailable,
            )
            .unwrap();

        let second_scope = fixture.source_scope.create_operation_scope().unwrap();
        let mut second = RemoteRangeRetryOperation::new(
            &second_scope,
            &fixture.range_scope,
            &fixture.work_scope,
            live(1, 1, 1),
            ChromeRangeRequest::new(8..16).unwrap(),
            Demand(2),
            fixture.abort_factory(),
        )
        .unwrap();
        let second_started = start(&mut second, &mut fixture.coordinator);
        second
            .complete_failure(
                &mut fixture.coordinator,
                second_started.settle_immediate(),
                ChromeRangeError::BrowserFetchUnavailable,
            )
            .unwrap();

        let before = fixture.coordinator.range_count();
        let external_turn = fixture.execution.turn();
        let mut turn = fixture
            .coordinator
            .project_retry_turn(&external_turn)
            .unwrap();
        assert!(matches!(
            first.start_retry_on_turn(
                &mut fixture.coordinator,
                &mut turn,
                |_controller, _request| {
                    Err::<TestPreparedRangeIdentity, _>(ChromeRangeError::InvalidValidatorHeader)
                },
                |_controller, _prepared, _receipt| (),
            ),
            Err(RangeAttemptStartFailure::Request(
                RangeAttemptStartError::OpeningFailed(MetadataOpeningFailureSignal::NonRetryable(
                    ChromeRangeError::InvalidValidatorHeader
                ))
            ))
        ));
        assert!(!turn.claimed);
        assert_eq!(fixture.coordinator.range_count(), before);
        assert_eq!(first.attempts_burned(), 1);

        drop(turn);
        let mut turn = fixture
            .coordinator
            .project_retry_turn(&external_turn)
            .unwrap();
        let second_retry = second
            .start_retry_on_turn(
                &mut fixture.coordinator,
                &mut turn,
                |controller, request| Ok(TestPreparedRangeIdentity::matching(controller, request)),
                |_controller, _prepared, _receipt| (),
            )
            .unwrap();
        let mut second_retry = second_retry;
        second.cancel_active(Pin::new(&mut second_retry)).unwrap();
    }

    #[test]
    fn response_owner_is_dropped_before_retry_becomes_pending_or_rearms() {
        struct RetainedOwner(Rc<Cell<bool>>);
        impl Drop for RetainedOwner {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }

        let mut fixture = Fixture::new(2, 2, 100);
        let mut operation = fixture.operation(1);
        let released = Rc::new(Cell::new(false));
        let started = operation
            .start_initial(
                &mut fixture.coordinator,
                fixture.execution.binding(),
                |controller, request| Ok(TestPreparedRangeIdentity::matching(controller, request)),
                |_controller, _prepared, _receipt| RetainedOwner(Rc::clone(&released)),
            )
            .unwrap();
        assert!(!released.get());
        assert_eq!(
            operation
                .complete_failure(
                    &mut fixture.coordinator,
                    started.settle_immediate(),
                    ChromeRangeError::UnexpectedHttpStatus(429),
                )
                .unwrap(),
            RangeAttemptCompletionDisposition::RetryPending
        );
        assert!(released.get());

        let external_turn = fixture.execution.turn();
        let mut turn = fixture
            .coordinator
            .project_retry_turn(&external_turn)
            .unwrap();
        let retry = operation
            .start_retry_on_turn(
                &mut fixture.coordinator,
                &mut turn,
                |controller, request| {
                    assert!(released.get());
                    Ok(TestPreparedRangeIdentity::matching(controller, request))
                },
                |_controller, _prepared, _receipt| (),
            )
            .unwrap();
        let mut retry = retry;
        operation.cancel_active(Pin::new(&mut retry)).unwrap();
    }

    #[test]
    fn settled_success_keeps_output_bound_until_success_transition() {
        struct RetainedSuccess(Rc<Cell<bool>>);
        impl Drop for RetainedSuccess {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }

        let mut fixture = Fixture::new(2, 2, 100);
        let mut operation = fixture.operation(1);
        let released = Rc::new(Cell::new(false));
        let started = operation
            .start_initial(
                &mut fixture.coordinator,
                fixture.execution.binding(),
                |controller, request| Ok(TestPreparedRangeIdentity::matching(controller, request)),
                |_controller, _prepared, _receipt| RetainedSuccess(Rc::clone(&released)),
            )
            .unwrap();
        let settled = started.settle_immediate();
        assert!(!released.get());
        let unrelated_turn = fixture.execution.turn();
        let mut permit = fixture
            .coordinator
            .project_retry_turn(&unrelated_turn)
            .unwrap();
        assert!(matches!(
            operation.start_retry_on_turn(
                &mut fixture.coordinator,
                &mut permit,
                |controller, request| {
                    Ok(TestPreparedRangeIdentity::matching(controller, request))
                },
                |_controller, _prepared, _receipt| (),
            ),
            Err(RangeAttemptStartFailure::Request(
                RangeAttemptStartError::Protocol(RetryProtocolError::OperationNotRetryPending)
            ))
        ));
        assert!(!released.get());
        let output = operation
            .complete_success(&fixture.coordinator, settled)
            .unwrap();
        assert!(!released.get());
        assert_eq!(operation.state(), OperationState::Succeeded);
        drop(output);
        assert!(released.get());
        let state = fixture.aborts.borrow();
        assert_eq!(state.finished, 1);
        assert_eq!(state.aborted, 0);
    }

    #[test]
    fn retry_classifier_is_exact_and_does_not_split_byob_read_failure() {
        for error in [
            ChromeRangeError::BrowserFetchUnavailable,
            ChromeRangeError::UnexpectedHttpStatus(429),
            ChromeRangeError::UnexpectedHttpStatus(500),
            ChromeRangeError::UnexpectedHttpStatus(502),
            ChromeRangeError::UnexpectedHttpStatus(503),
            ChromeRangeError::UnexpectedHttpStatus(504),
        ] {
            assert!(is_retryable_attempt_failure(error), "{error:?}");
        }
        for error in [
            ChromeRangeError::BrowserAdapterUnavailable,
            ChromeRangeError::UnexpectedHttpStatus(408),
            ChromeRangeError::UnexpectedHttpStatus(501),
            ChromeRangeError::UnexpectedHttpStatus(505),
            ChromeRangeError::AuthorizationRejected,
            ChromeRangeError::ObjectUnavailable,
            ChromeRangeError::PreconditionFailed,
            ChromeRangeError::RangeNotSatisfiable,
            ChromeRangeError::InvalidContentRange,
            ChromeRangeError::InvalidContentLength,
            ChromeRangeError::InvalidValidatorHeader,
        ] {
            assert!(!is_retryable_attempt_failure(error), "{error:?}");
        }
    }

    #[cfg(target_arch = "wasm32")]
    #[test]
    fn raw_byob_timeout_read_and_adapter_failures_are_not_retryable() {
        for error in [
            ExactLengthByobPumpError::Timeout,
            ExactLengthByobPumpError::ReadFailed,
            ExactLengthByobPumpError::BrowserFetchUnavailable,
        ] {
            assert!(!is_retryable_attempt_failure(ChromeRangeError::BodyPump(
                error
            )));
        }
    }

    #[test]
    fn only_matching_active_visible_timeout_confirmation_can_enter_retry_pending() {
        let mut fixture = Fixture::new(2, 2, 100);
        let mut operation = fixture.operation(1);
        let mut started = start(&mut operation, &mut fixture.coordinator);
        let timeout = fixture.execution.timeout(started.token().unwrap());
        assert_eq!(
            operation
                .complete_active_visible_timeout(
                    &fixture.coordinator,
                    Pin::new(&mut started),
                    timeout,
                )
                .unwrap(),
            RangeAttemptCompletionDisposition::RetryPending
        );

        let mut hidden_fixture = Fixture::new(2, 2, 100);
        let mut hidden_operation = hidden_fixture.operation(1);
        let mut hidden_attempt = start(&mut hidden_operation, &mut hidden_fixture.coordinator);
        let stale_timeout = hidden_fixture
            .execution
            .timeout(hidden_attempt.token().unwrap());
        let hidden = hidden_fixture.execution.hide();
        hidden_fixture
            .coordinator
            .apply_hidden_evidence(hidden)
            .unwrap();
        let rejection = hidden_operation
            .complete_active_visible_timeout(
                &hidden_fixture.coordinator,
                Pin::new(&mut hidden_attempt),
                stale_timeout,
            )
            .unwrap_err();
        assert_eq!(rejection, RetryProtocolError::Hidden);
        hidden_operation
            .cancel_active(Pin::new(&mut hidden_attempt))
            .unwrap();
    }

    #[test]
    fn pending_task_owner_is_synchronously_closed_or_page_terminated() {
        let mut close_fixture = Fixture::new(2, 2, 100);
        let mut close_operation = close_fixture.operation(1);
        let close_dropped = Rc::new(Cell::new(false));
        let mut close_attempt = std::pin::pin!(start_pending(
            &mut close_operation,
            &mut close_fixture,
            Rc::clone(&close_dropped),
        ));
        assert_pending(close_attempt.as_mut());
        close_operation
            .close_active(&mut close_fixture.coordinator, close_attempt.as_mut())
            .unwrap();
        assert!(close_dropped.get());
        assert_eq!(close_fixture.aborts.borrow().aborted, 1);
        assert_eq!(close_operation.state(), OperationState::Cancelled);
        assert_eq!(
            close_fixture.coordinator.terminal(),
            Some(RetrySessionTerminal::ExplicitlyClosed)
        );

        let mut page_fixture = Fixture::new(2, 2, 100);
        let mut page_operation = page_fixture.operation(1);
        let page_dropped = Rc::new(Cell::new(false));
        let mut page_attempt = std::pin::pin!(start_pending(
            &mut page_operation,
            &mut page_fixture,
            Rc::clone(&page_dropped),
        ));
        assert_pending(page_attempt.as_mut());
        let evidence = page_fixture.execution.terminate();
        page_operation
            .page_terminate_active(
                &mut page_fixture.coordinator,
                evidence,
                page_attempt.as_mut(),
            )
            .unwrap();
        assert!(page_dropped.get());
        assert_eq!(page_fixture.aborts.borrow().aborted, 1);
        assert_eq!(page_operation.state(), OperationState::Cancelled);
        assert_eq!(
            page_fixture.coordinator.terminal(),
            Some(RetrySessionTerminal::PageTerminated)
        );
    }

    #[test]
    fn pending_task_timeout_and_exact_source_deadline_release_before_transition() {
        let mut timeout_fixture = Fixture::new(2, 2, 100);
        let mut timeout_operation = timeout_fixture.operation(1);
        let timeout_dropped = Rc::new(Cell::new(false));
        let mut timeout_attempt = std::pin::pin!(start_pending(
            &mut timeout_operation,
            &mut timeout_fixture,
            Rc::clone(&timeout_dropped),
        ));
        assert_pending(timeout_attempt.as_mut());
        let evidence = timeout_fixture
            .execution
            .timeout(timeout_attempt.as_ref().token().unwrap());
        assert_eq!(
            timeout_operation
                .complete_active_visible_timeout(
                    &timeout_fixture.coordinator,
                    timeout_attempt.as_mut(),
                    evidence,
                )
                .unwrap(),
            RangeAttemptCompletionDisposition::RetryPending
        );
        assert!(timeout_dropped.get());
        assert_eq!(timeout_fixture.aborts.borrow().aborted, 1);

        let mut deadline_fixture = Fixture::new(2, 2, 100);
        let mut deadline_operation = deadline_fixture.operation(1);
        let deadline_dropped = Rc::new(Cell::new(false));
        let mut deadline_attempt = std::pin::pin!(start_pending(
            &mut deadline_operation,
            &mut deadline_fixture,
            Rc::clone(&deadline_dropped),
        ));
        assert_pending(deadline_attempt.as_mut());
        assert_eq!(
            deadline_fixture
                .coordinator
                .advance_visible_time(deadline_fixture.execution.binding(), 100)
                .unwrap(),
            MetadataOpeningDeadlineState::Expired
        );
        assert_eq!(
            deadline_operation
                .complete_source_deadline_expiry(
                    &deadline_fixture.coordinator,
                    deadline_fixture.execution.binding(),
                    deadline_attempt.as_mut(),
                )
                .unwrap(),
            RangeAttemptCompletionDisposition::OpeningFailed(
                MetadataOpeningFailureSignal::ActiveVisibleDeadlineExpired
            )
        );
        assert!(deadline_dropped.get());
        assert_eq!(deadline_fixture.aborts.borrow().aborted, 1);
        assert_eq!(deadline_operation.state(), OperationState::OpeningFailed);
    }

    #[test]
    fn settled_owner_supports_close_page_timeout_and_source_deadline_terminalization() {
        let mut close_fixture = Fixture::new(2, 2, 100);
        let mut close_operation = close_fixture.operation(1);
        let close_output_dropped = Rc::new(Cell::new(false));
        let mut close_attempt = std::pin::pin!(start_ready(
            &mut close_operation,
            &mut close_fixture,
            Rc::clone(&close_output_dropped),
        ));
        let close_timeout = close_fixture
            .execution
            .timeout(close_attempt.as_ref().token().unwrap());
        let close_settled = poll_ready(close_attempt.as_mut());
        assert_eq!(
            close_operation
                .close_settled(&mut close_fixture.coordinator, close_settled)
                .unwrap(),
            RangeAttemptCompletionDisposition::Cancelled
        );
        assert!(close_output_dropped.get());
        assert_eq!(close_fixture.aborts.borrow().aborted, 1);
        assert_eq!(close_operation.state(), OperationState::Cancelled);
        assert_eq!(
            close_operation.complete_active_visible_timeout(
                &close_fixture.coordinator,
                close_attempt.as_mut(),
                close_timeout,
            ),
            Err(RetryProtocolError::OperationAlreadyTerminal)
        );

        let mut page_fixture = Fixture::new(2, 2, 100);
        let mut page_operation = page_fixture.operation(1);
        let page_output_dropped = Rc::new(Cell::new(false));
        let mut page_attempt = std::pin::pin!(start_ready(
            &mut page_operation,
            &mut page_fixture,
            Rc::clone(&page_output_dropped),
        ));
        let page_settled = poll_ready(page_attempt.as_mut());
        let page_evidence = page_fixture.execution.terminate();
        assert_eq!(
            page_operation
                .page_terminate_settled(&mut page_fixture.coordinator, page_evidence, page_settled,)
                .unwrap(),
            RangeAttemptCompletionDisposition::Cancelled
        );
        assert!(page_output_dropped.get());
        assert_eq!(page_fixture.aborts.borrow().aborted, 1);
        assert_eq!(page_operation.state(), OperationState::Cancelled);

        let mut timeout_fixture = Fixture::new(2, 2, 100);
        let mut timeout_operation = timeout_fixture.operation(1);
        let timeout_output_dropped = Rc::new(Cell::new(false));
        let mut timeout_attempt = std::pin::pin!(start_ready(
            &mut timeout_operation,
            &mut timeout_fixture,
            Rc::clone(&timeout_output_dropped),
        ));
        let timeout_evidence = timeout_fixture
            .execution
            .timeout(timeout_attempt.as_ref().token().unwrap());
        let timeout_settled = poll_ready(timeout_attempt.as_mut());
        assert_eq!(
            timeout_operation
                .complete_settled_active_visible_timeout(
                    &timeout_fixture.coordinator,
                    timeout_settled,
                    timeout_evidence,
                )
                .unwrap(),
            RangeAttemptCompletionDisposition::RetryPending
        );
        assert!(timeout_output_dropped.get());
        assert_eq!(timeout_fixture.aborts.borrow().aborted, 1);
        assert!(matches!(
            timeout_operation.state(),
            OperationState::RetryPending { .. }
        ));

        let mut deadline_fixture = Fixture::new(2, 2, 100);
        let mut deadline_operation = deadline_fixture.operation(1);
        let deadline_output_dropped = Rc::new(Cell::new(false));
        let mut deadline_attempt = std::pin::pin!(start_ready(
            &mut deadline_operation,
            &mut deadline_fixture,
            Rc::clone(&deadline_output_dropped),
        ));
        let deadline_settled = poll_ready(deadline_attempt.as_mut());
        let binding = deadline_fixture.execution.binding();
        assert_eq!(
            deadline_fixture
                .coordinator
                .advance_visible_time(binding, 100)
                .unwrap(),
            MetadataOpeningDeadlineState::Expired
        );
        assert_eq!(
            deadline_operation
                .complete_settled_source_deadline_expiry(
                    &deadline_fixture.coordinator,
                    binding,
                    deadline_settled,
                )
                .unwrap(),
            RangeAttemptCompletionDisposition::OpeningFailed(
                MetadataOpeningFailureSignal::ActiveVisibleDeadlineExpired
            )
        );
        assert!(deadline_output_dropped.get());
        assert_eq!(deadline_fixture.aborts.borrow().aborted, 1);
        assert_eq!(deadline_operation.state(), OperationState::OpeningFailed);
    }

    #[test]
    fn settled_transfer_leaves_a_typed_depleted_shell_and_double_poll_is_safe() {
        let mut fixture = Fixture::new(1, 1, 100);
        let mut operation = fixture.operation(1);
        let output_dropped = Rc::new(Cell::new(false));
        let mut attempt = std::pin::pin!(start_ready(
            &mut operation,
            &mut fixture,
            Rc::clone(&output_dropped),
        ));
        let settled = poll_ready(attempt.as_mut());

        assert_eq!(
            attempt.as_ref().token(),
            Err(RetryProtocolError::AttemptOwnerDepleted)
        );
        assert_eq!(
            attempt.as_ref().callback_identity(),
            Err(RetryProtocolError::AttemptOwnerDepleted)
        );
        let mut context = Context::from_waker(std::task::Waker::noop());
        assert!(matches!(
            attempt.as_mut().poll_settlement(&mut context),
            Poll::Ready(Err(RetryProtocolError::OperationAlreadyTerminal))
        ));
        assert_eq!(
            operation.state(),
            OperationState::Active {
                token: settled.callback_identity().attempt,
            }
        );

        drop(settled);
        assert!(output_dropped.get());
        assert_eq!(fixture.aborts.borrow().aborted, 1);
        assert_eq!(operation.state(), OperationState::Cancelled);
    }

    #[test]
    fn active_visible_timeout_at_exact_source_deadline_is_terminal_not_retryable() {
        let mut fixture = Fixture::new(2, 2, 100);
        let mut operation = fixture.operation(1);
        let payload_dropped = Rc::new(Cell::new(false));
        let mut attempt = std::pin::pin!(start_pending(
            &mut operation,
            &mut fixture,
            Rc::clone(&payload_dropped),
        ));
        assert_pending(attempt.as_mut());
        let evidence = fixture.execution.timeout(attempt.as_ref().token().unwrap());
        assert_eq!(
            fixture
                .coordinator
                .advance_visible_time(fixture.execution.binding(), 100)
                .unwrap(),
            MetadataOpeningDeadlineState::Expired
        );

        assert_eq!(
            operation
                .complete_active_visible_timeout(&fixture.coordinator, attempt.as_mut(), evidence,)
                .unwrap(),
            RangeAttemptCompletionDisposition::OpeningFailed(
                MetadataOpeningFailureSignal::ActiveVisibleDeadlineExpired
            )
        );
        assert!(payload_dropped.get());
        assert_eq!(fixture.aborts.borrow().aborted, 1);
        assert_eq!(operation.state(), OperationState::OpeningFailed);

        let turn = fixture.execution.turn();
        let mut permit = fixture
            .coordinator
            .project_retry_turn(&turn)
            .expect("a later visible turn can be projected after terminal expiry");
        assert!(matches!(
            operation.start_retry_on_turn(
                &mut fixture.coordinator,
                &mut permit,
                |_controller, _request| -> Result<TestPreparedRangeIdentity, ChromeRangeError> {
                    panic!("deadline expiry must not prepare a retry")
                },
                |_controller, _prepared, _receipt| (),
            ),
            Err(RangeAttemptStartFailure::Request(
                RangeAttemptStartError::Protocol(RetryProtocolError::OperationAlreadyTerminal)
            ))
        ));
        assert_eq!(operation.attempts_burned(), 1);
        assert_eq!(fixture.coordinator.range_count(), 1);
    }

    #[test]
    fn dropping_pending_task_cancels_matching_active_state_after_resource_release() {
        let mut fixture = Fixture::new(2, 2, 100);
        let mut operation = fixture.operation(1);
        let dropped = Rc::new(Cell::new(false));
        {
            let mut attempt = std::pin::pin!(start_pending(
                &mut operation,
                &mut fixture,
                Rc::clone(&dropped),
            ));
            assert_pending(attempt.as_mut());
        }
        assert!(dropped.get());
        assert_eq!(fixture.aborts.borrow().aborted, 1);
        assert_eq!(operation.state(), OperationState::Cancelled);
    }

    #[test]
    fn hidden_time_is_not_charged_and_deadline_close_page_races_are_first_transition_wins() {
        let mut hidden_fixture = Fixture::new(2, 2, 100);
        let hidden_binding = hidden_fixture.execution.binding();
        let hidden = hidden_fixture.execution.hide();
        hidden_fixture
            .coordinator
            .apply_hidden_evidence(hidden)
            .unwrap();
        assert_eq!(
            hidden_fixture
                .coordinator
                .advance_visible_time(hidden_binding, 100),
            Err(RetryProtocolError::Hidden)
        );
        assert_eq!(
            hidden_fixture.coordinator.deadline_state(),
            MetadataOpeningDeadlineState::Active
        );

        let mut close_fixture = Fixture::new(2, 2, 100);
        let mut close_operation = close_fixture.operation(1);
        let mut close_attempt = std::pin::pin!(start_pending(
            &mut close_operation,
            &mut close_fixture,
            Rc::new(Cell::new(false)),
        ));
        let binding = close_fixture.execution.binding();
        close_operation
            .close_active(&mut close_fixture.coordinator, close_attempt.as_mut())
            .unwrap();
        assert_eq!(
            close_operation.complete_source_deadline_expiry(
                &close_fixture.coordinator,
                binding,
                close_attempt.as_mut(),
            ),
            Err(RetryProtocolError::ActiveAttemptRequired)
        );
        assert_eq!(
            close_fixture.coordinator.terminal(),
            Some(RetrySessionTerminal::ExplicitlyClosed)
        );

        let mut expiry_fixture = Fixture::new(2, 2, 100);
        let mut expiry_operation = expiry_fixture.operation(1);
        let mut expiry_attempt = std::pin::pin!(start_pending(
            &mut expiry_operation,
            &mut expiry_fixture,
            Rc::new(Cell::new(false)),
        ));
        let binding = expiry_fixture.execution.binding();
        expiry_fixture
            .coordinator
            .advance_visible_time(binding, 100)
            .unwrap();
        expiry_operation
            .complete_source_deadline_expiry(
                &expiry_fixture.coordinator,
                binding,
                expiry_attempt.as_mut(),
            )
            .unwrap();
        let page_evidence = expiry_fixture.execution.terminate();
        assert_eq!(
            expiry_operation.page_terminate_active(
                &mut expiry_fixture.coordinator,
                page_evidence,
                expiry_attempt.as_mut(),
            ),
            Err(RetryProtocolError::ActiveAttemptRequired)
        );
        assert_eq!(expiry_operation.state(), OperationState::OpeningFailed);
        assert_eq!(expiry_fixture.coordinator.terminal(), None);
    }

    #[test]
    fn object_change_bypasses_demand_staleness_but_not_live_or_attempt_identity() {
        let mut fixture = Fixture::new(3, 3, 100);
        let mut operation = fixture.operation(1);
        let started = start(&mut operation, &mut fixture.coordinator);
        operation.supersede_demand(Demand(2));
        assert_eq!(
            operation
                .complete_failure(
                    &mut fixture.coordinator,
                    started.settle_immediate(),
                    ChromeRangeError::ObjectChanged,
                )
                .unwrap(),
            RangeAttemptCompletionDisposition::ObjectChanged(ObjectChangeDisposition::Latched)
        );
        assert!(!fixture.coordinator.can_refetch_chunks());
        assert_eq!(
            fixture.coordinator.terminal(),
            Some(RetrySessionTerminal::ObjectChanged)
        );

        let mut stale_fixture = Fixture::new(2, 2, 100);
        let mut stale_operation = stale_fixture.operation(1);
        let stale_started = start(&mut stale_operation, &mut stale_fixture.coordinator);
        let other_root = test_profile_with(&[]).start_accounting_root().unwrap();
        let other_viewer = other_root.create_viewer_scope().unwrap();
        let other_source = other_viewer.create_source_scope().unwrap();
        let mut other_coordinator = MetadataOpeningRetryCoordinator::new(
            &other_source,
            live(1, 2, 1),
            stale_fixture.execution.binding(),
        )
        .unwrap();
        let rejection = stale_operation
            .complete_failure(
                &mut other_coordinator,
                stale_started.settle_immediate(),
                ChromeRangeError::ObjectChanged,
            )
            .unwrap_err();
        assert_eq!(rejection.error(), RetryProtocolError::AttemptMismatch);
        assert_eq!(other_coordinator.terminal(), None);
        assert!(other_coordinator.can_refetch_chunks());
        stale_operation
            .complete_failure(
                &mut stale_fixture.coordinator,
                rejection.into_attempt(),
                ChromeRangeError::BrowserFetchUnavailable,
            )
            .unwrap();
        stale_operation
            .close_inactive(&mut stale_fixture.coordinator)
            .unwrap();
    }

    #[test]
    fn late_object_change_from_released_attempt_cannot_poison_current_retry() {
        let mut fixture = Fixture::new(3, 3, 100);
        let mut operation = fixture.operation(1);
        let first = start(&mut operation, &mut fixture.coordinator);
        let late_first_callback = first.callback_identity().unwrap().clone();
        assert_eq!(
            operation
                .complete_failure(
                    &mut fixture.coordinator,
                    first.settle_immediate(),
                    ChromeRangeError::BrowserFetchUnavailable,
                )
                .unwrap(),
            RangeAttemptCompletionDisposition::RetryPending
        );

        let external_turn = fixture.execution.turn();
        let mut turn = fixture
            .coordinator
            .project_retry_turn(&external_turn)
            .unwrap();
        let second = operation
            .start_retry_on_turn(
                &mut fixture.coordinator,
                &mut turn,
                |controller, request| Ok(TestPreparedRangeIdentity::matching(controller, request)),
                |controller, _prepared, receipt| {
                    assert_eq!(receipt.attempt().get(), 2);
                    controller.identity
                },
            )
            .unwrap();
        assert_eq!(
            operation.reject_unowned_failure_callback(
                &fixture.coordinator,
                &late_first_callback,
                ChromeRangeError::ObjectChanged,
            ),
            RangeAttemptCompletionDisposition::ObjectChanged(ObjectChangeDisposition::StaleAttempt)
        );
        assert_eq!(fixture.coordinator.terminal(), None);
        assert!(fixture.coordinator.can_refetch_chunks());
        let mut second = second;
        operation.cancel_active(Pin::new(&mut second)).unwrap();
    }

    #[test]
    fn close_wins_over_late_object_change_and_page_termination_never_rearms() {
        let mut fixture = Fixture::new(2, 2, 100);
        let mut operation = fixture.operation(1);
        let mut started = start(&mut operation, &mut fixture.coordinator);
        let late_callback = started.callback_identity().unwrap().clone();
        let aborted_before = fixture.aborts.borrow().aborted;
        assert_eq!(
            operation
                .close_active(&mut fixture.coordinator, Pin::new(&mut started))
                .unwrap(),
            RangeAttemptCompletionDisposition::Cancelled
        );
        assert_eq!(fixture.aborts.borrow().aborted, aborted_before + 1);
        assert_eq!(
            operation.reject_unowned_failure_callback(
                &fixture.coordinator,
                &late_callback,
                ChromeRangeError::ObjectChanged,
            ),
            RangeAttemptCompletionDisposition::ObjectChanged(ObjectChangeDisposition::StaleAttempt)
        );
        assert_eq!(
            fixture.coordinator.terminal(),
            Some(RetrySessionTerminal::ExplicitlyClosed)
        );

        let mut page_fixture = Fixture::new(2, 2, 100);
        let mut page_operation = page_fixture.operation(1);
        let page_started = start(&mut page_operation, &mut page_fixture.coordinator);
        page_operation
            .complete_failure(
                &mut page_fixture.coordinator,
                page_started.settle_immediate(),
                ChromeRangeError::BrowserFetchUnavailable,
            )
            .unwrap();
        let page_evidence = page_fixture.execution.terminate();
        page_operation
            .page_terminate_inactive(&mut page_fixture.coordinator, page_evidence)
            .unwrap();
        let stale_turn = ExternalVisibleControlTurn {
            binding: ActiveVisibleExecutionBinding {
                issuer: AbstractExecutionIssuerId(NonZeroU64::MIN),
                epoch: NonZeroU64::MIN,
                baseline: NonZeroU64::MIN,
            },
            sequence: NonZeroU64::MIN,
            projection: Cell::new(TurnProjectionState::Available),
        };
        assert!(matches!(
            page_fixture.coordinator.project_retry_turn(&stale_turn),
            Err(RetryProtocolError::PageTerminated)
        ));
        assert_eq!(
            page_fixture.coordinator.terminal(),
            Some(RetrySessionTerminal::PageTerminated)
        );

        let mut fatal_fixture = Fixture::new(2, 2, 100);
        let mut fatal_operation = fatal_fixture.operation(1);
        let fatal_attempt = start(&mut fatal_operation, &mut fatal_fixture.coordinator);
        fatal_operation
            .complete_failure(
                &mut fatal_fixture.coordinator,
                fatal_attempt.settle_immediate(),
                ChromeRangeError::ObjectChanged,
            )
            .unwrap();
        fatal_operation
            .close_inactive(&mut fatal_fixture.coordinator)
            .unwrap();
        assert_eq!(
            fatal_fixture.coordinator.terminal(),
            Some(RetrySessionTerminal::ObjectChanged)
        );
    }

    #[test]
    fn existing_close_or_page_terminal_drops_owning_late_object_change_as_stale() {
        for terminate_page in [false, true] {
            let mut fixture = Fixture::new(2, 3, 100);
            let mut terminal_owner = fixture.operation(1);
            let mut terminal_attempt = start(&mut terminal_owner, &mut fixture.coordinator);
            let mut late = fixture.fresh_operation(2, 8..16);
            let late_attempt = start(&mut late, &mut fixture.coordinator);

            let expected_terminal = if terminate_page {
                let evidence = fixture.execution.terminate();
                terminal_owner
                    .page_terminate_active(
                        &mut fixture.coordinator,
                        evidence,
                        Pin::new(&mut terminal_attempt),
                    )
                    .unwrap();
                RetrySessionTerminal::PageTerminated
            } else {
                terminal_owner
                    .close_active(&mut fixture.coordinator, Pin::new(&mut terminal_attempt))
                    .unwrap();
                RetrySessionTerminal::ExplicitlyClosed
            };
            assert_eq!(
                late.complete_failure(
                    &mut fixture.coordinator,
                    late_attempt.settle_immediate(),
                    ChromeRangeError::ObjectChanged,
                )
                .unwrap(),
                RangeAttemptCompletionDisposition::Stale
            );
            assert_eq!(fixture.coordinator.terminal(), Some(expected_terminal));
            assert_eq!(fixture.aborts.borrow().aborted, 2);
        }
    }

    #[test]
    fn metadata_exhaustion_is_an_opening_signal_not_a_session_fatal() {
        let mut fixture = Fixture::new(1, 2, 100);
        let mut operation = fixture.operation(1);
        let started = start(&mut operation, &mut fixture.coordinator);
        operation
            .complete_failure(
                &mut fixture.coordinator,
                started.settle_immediate(),
                ChromeRangeError::UnexpectedHttpStatus(500),
            )
            .unwrap();
        let external_turn = fixture.execution.turn();
        let mut turn = fixture
            .coordinator
            .project_retry_turn(&external_turn)
            .unwrap();
        assert!(matches!(
            operation.start_retry_on_turn(
                &mut fixture.coordinator,
                &mut turn,
                |controller, request| {
                    Ok(TestPreparedRangeIdentity::matching(controller, request))
                },
                |_controller, _prepared, _receipt| (),
            ),
            Err(RangeAttemptStartFailure::Request(
                RangeAttemptStartError::OpeningFailed(
                    MetadataOpeningFailureSignal::RetryAttemptsExhausted
                )
            ))
        ));
        assert_eq!(fixture.coordinator.terminal(), None);
    }

    #[test]
    fn metadata_range_exhaustion_does_not_burn_the_next_operation_attempt() {
        let mut fixture = Fixture::new(3, 1, 100);
        let mut operation = fixture.operation(1);
        let started = start(&mut operation, &mut fixture.coordinator);
        operation
            .complete_failure(
                &mut fixture.coordinator,
                started.settle_immediate(),
                ChromeRangeError::UnexpectedHttpStatus(504),
            )
            .unwrap();
        let external_turn = fixture.execution.turn();
        let mut turn = fixture
            .coordinator
            .project_retry_turn(&external_turn)
            .unwrap();
        assert!(matches!(
            operation.start_retry_on_turn(
                &mut fixture.coordinator,
                &mut turn,
                |controller, request| {
                    Ok(TestPreparedRangeIdentity::matching(controller, request))
                },
                |_controller, _prepared, _receipt| (),
            ),
            Err(RangeAttemptStartFailure::Request(
                RangeAttemptStartError::OpeningFailed(
                    MetadataOpeningFailureSignal::MetadataRangeLimitExhausted
                )
            ))
        ));
        assert_eq!(operation.attempts_burned(), 1);
        assert_eq!(fixture.coordinator.range_count(), 1);
        assert_eq!(fixture.coordinator.terminal(), None);
    }

    #[test]
    fn page_termination_invalidates_old_turns_and_active_owner_is_abortable() {
        let mut fixture = Fixture::new(2, 3, 100);
        let mut pending = fixture.operation(1);
        let pending_started = start(&mut pending, &mut fixture.coordinator);
        pending
            .complete_failure(
                &mut fixture.coordinator,
                pending_started.settle_immediate(),
                ChromeRangeError::BrowserFetchUnavailable,
            )
            .unwrap();
        let external_turn = fixture.execution.turn();
        let mut old_turn = fixture
            .coordinator
            .project_retry_turn(&external_turn)
            .unwrap();

        let active_scope = fixture.source_scope.create_operation_scope().unwrap();
        let mut active = RemoteRangeRetryOperation::new(
            &active_scope,
            &fixture.range_scope,
            &fixture.work_scope,
            live(1, 1, 1),
            ChromeRangeRequest::new(8..16).unwrap(),
            Demand(2),
            fixture.abort_factory(),
        )
        .unwrap();
        let mut active_started = start(&mut active, &mut fixture.coordinator);
        let aborted_before = fixture.aborts.borrow().aborted;
        let page_evidence = fixture.execution.terminate();
        active
            .page_terminate_active(
                &mut fixture.coordinator,
                page_evidence,
                Pin::new(&mut active_started),
            )
            .unwrap();
        assert_eq!(fixture.aborts.borrow().aborted, aborted_before + 1);
        assert!(matches!(
            pending.start_retry_on_turn(
                &mut fixture.coordinator,
                &mut old_turn,
                |controller, request| {
                    Ok(TestPreparedRangeIdentity::matching(controller, request))
                },
                |_controller, _prepared, _receipt| (),
            ),
            Err(RangeAttemptStartFailure::Request(
                RangeAttemptStartError::Protocol(RetryProtocolError::PageTerminated)
            ))
        ));
        assert_eq!(
            fixture.coordinator.terminal(),
            Some(RetrySessionTerminal::PageTerminated)
        );
    }

    #[test]
    fn retry_debug_and_errors_never_render_caller_identity() {
        #[derive(Clone, PartialEq, Eq)]
        struct Secret(&'static str);
        impl fmt::Debug for Secret {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.0)
            }
        }

        let root = test_profile_with(&[]).start_accounting_root().unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let source_scope = viewer.create_source_scope().unwrap();
        let session_scope = source_scope.create_session_scope().unwrap();
        let range_scope = session_scope.create_range_response_scope().unwrap();
        let work_scope = range_scope.create_work_unit_scope().unwrap();
        let operation_scope = source_scope.create_operation_scope().unwrap();
        let live = RemoteRangeLiveIdentity::new(
            Secret("secret-source"),
            Secret("secret-session"),
            Secret("secret-representation"),
        );
        let execution = FakeAbstractExecutionIssuer::new(987_654_319);
        let mut coordinator =
            MetadataOpeningRetryCoordinator::new(&source_scope, live.clone(), execution.binding())
                .unwrap();
        let operation = RemoteRangeRetryOperation::new(
            &operation_scope,
            &range_scope,
            &work_scope,
            live,
            ChromeRangeRequest::new(0..8).unwrap(),
            Secret("secret-demand"),
            FakeAbortFactory(
                Rc::new(RefCell::new(FakeAbortState::default())),
                range_scope
                    .range_attempt_accounting_binding(&work_scope)
                    .unwrap(),
            ),
        )
        .unwrap();
        for rendered in [format!("{coordinator:?}"), format!("{operation:?}")] {
            assert!(!rendered.contains("secret-"), "{rendered}");
        }
        let binding = execution.binding();
        let turn = ExternalVisibleControlTurn {
            binding,
            sequence: NonZeroU64::new(987_654_321).unwrap(),
            projection: Cell::new(TurnProjectionState::Available),
        };
        let permit = coordinator.project_retry_turn(&turn).unwrap();
        for rendered in [
            format!("{turn:?}"),
            format!("{permit:?}"),
            format!("{coordinator:?}"),
        ] {
            assert!(!rendered.contains("987654321"), "{rendered}");
            assert!(!rendered.contains("987654319"), "{rendered}");
        }
        drop(permit);
        coordinator.execution = ExecutionState::Hidden {
            issuer: binding.issuer,
            suspension: NonZeroU64::new(987_654_323).unwrap(),
        };
        let rendered = format!("{coordinator:?}");
        assert!(!rendered.contains("987654323"), "{rendered}");
        assert!(
            !RangeAttemptStartError::OpeningFailed(MetadataOpeningFailureSignal::NonRetryable(
                ChromeRangeError::BrowserFetchUnavailable
            ))
            .to_string()
            .contains("secret-")
        );
    }

    #[test]
    fn successful_attempt_moves_payload_without_aborting() {
        let mut fixture = Fixture::new(1, 1, 100);
        let mut operation = fixture.operation(1);
        let started = start(&mut operation, &mut fixture.coordinator);
        let controller_identity = operation
            .complete_success(&fixture.coordinator, started.settle_immediate())
            .unwrap();
        assert_eq!(controller_identity, 1);
        let state = fixture.aborts.borrow();
        assert_eq!(state.created, 1);
        assert_eq!(state.finished, 1);
        assert_eq!(state.aborted, 0);
    }
}
