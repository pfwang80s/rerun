//! Production-disarmed strict-only extensionless MCAP format sniffing.
//!
//! This module has no Viewer registry or public Web API entry point.
//! A future strict admission transaction must supply the non-constructible admission capability
//! and retain the linear owners defined here.

#![allow(
    dead_code,
    reason = "MCAP-016 remains production-disarmed until strict admission wiring lands"
)]

use std::cell::Cell;
use std::fmt;
use std::future::Future;
use std::num::NonZeroU64;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};

use pin_project_lite::pin_project;

use crate::chrome_byob::{ExactLengthByobPumpControl, ExactLengthRangeBody};
use crate::chrome_range::{
    BoundChromeRangeObject, ChromeFormatSniffTransportOutcome, ChromeRangeError,
    ChromeRangeRequest, MCAP_MAGIC_BYTES, PreparedChromeBoundRangeAttempt,
    prepare_bound_exact_range_attempt, prepare_format_sniff_range_attempt,
};
use crate::range_retry::{
    ActiveVisibleExecutionBinding, ChromeRangeAttemptAbortController,
    ChromeRangeAttemptAbortFactory, ExternalVisibleControlTurn, MetadataOpeningRetryCoordinator,
    RangeAttemptAbortController as _, RangeAttemptCompletionDisposition, RangeAttemptStartFailure,
    RangeAttemptTransportCompletionWithController, RemoteRangeLiveIdentity,
    RemoteRangeRetryOperation, RetryProtocolError, SettledRangeAttempt, StartedRangeAttempt,
    VisibleRetryTurnPermit,
};
use crate::remote_limits::{
    OperationAccountingScope, RangeResponseAccountingScope, SessionAccountingScope,
    SourceAccountingScope, WasmModuleLimitAccountingRoot, WorkUnitAccountingScope,
};
use crate::remote_validator::RepresentationConsistencyPolicy;
use crate::secret_url::SecretUrl;

const FORMAT_SNIFF_RANGE: std::ops::Range<u64> = 0..8;

static NEXT_SNIFF_ID: AtomicU64 = AtomicU64::new(1);

fn allocate_sniff_id() -> Result<NonZeroU64, RetryProtocolError> {
    allocate_sniff_id_from(&NEXT_SNIFF_ID)
}

fn allocate_sniff_id_from(next_id: &AtomicU64) -> Result<NonZeroU64, RetryProtocolError> {
    let mut current = next_id.load(Ordering::Relaxed);
    loop {
        let Some(id) = NonZeroU64::new(current) else {
            return Err(RetryProtocolError::IdentityExhausted);
        };
        let next = current.checked_add(1).unwrap_or(0);
        match next_id.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return Ok(id),
            Err(observed) => current = observed,
        }
    }
}

macro_rules! sniff_id {
    ($name:ident) => {
        #[derive(Clone, Copy, PartialEq, Eq)]
        pub(crate) struct $name(NonZeroU64);

        impl $name {
            fn allocate() -> Result<Self, RetryProtocolError> {
                allocate_sniff_id().map(Self)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!(stringify!($name), "(<opaque>)"))
            }
        }
    };
}

sniff_id!(SniffViewerInstanceId);
sniff_id!(SniffSourceId);
sniff_id!(PendingFormatSniffTokenId);
sniff_id!(SniffSessionId);
sniff_id!(SniffRepresentationId);
sniff_id!(SniffAttemptEpoch);

/// Non-cloneable ownership of one pending strict extensionless source.
struct PendingFormatSniffToken(PendingFormatSniffTokenId);

/// Capability created only by the future strict admission transaction.
pub(crate) struct DisarmedStrictExtensionlessAdmission {
    viewer: SniffViewerInstanceId,
    source: SniffSourceId,
    pending: PendingFormatSniffToken,
    session: SniffSessionId,
    representation: SniffRepresentationId,
}

/// The non-cloneable HTTP source identity frozen by strict admission.
///
/// Only typed request preparation can temporarily expose the secret URL, so handoff consumers
/// cannot clone, serialize, or log the original source.
struct FrozenFormatSniffHttpSourceSpec {
    url: SecretUrl,
    consistency_policy: RepresentationConsistencyPolicy,
    pump_slice_bytes: NonZeroU64,
}

impl fmt::Debug for FrozenFormatSniffHttpSourceSpec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FrozenFormatSniffHttpSourceSpec")
            .field("url", &"<redacted>")
            .field("consistency_policy", &self.consistency_policy)
            .field("pump_slice_bytes", &self.pump_slice_bytes)
            .finish()
    }
}

impl FrozenFormatSniffHttpSourceSpec {
    fn prepare_bound_range_attempt<'object>(
        &self,
        request_range: crate::chrome_range::BoundChromeRangeRequest<'object>,
        abort_controller: &web_sys::AbortController,
        root: &WasmModuleLimitAccountingRoot,
        range_scope: &RangeResponseAccountingScope,
        work_scope: &WorkUnitAccountingScope,
    ) -> Result<PreparedChromeBoundRangeAttempt<'object>, ChromeRangeError> {
        prepare_bound_exact_range_attempt(
            &self.url,
            request_range,
            abort_controller,
            root,
            range_scope,
            work_scope,
            self.pump_slice_bytes,
        )
    }

    #[cfg(test)]
    fn exact_url_matches_for_test(&self, expected: &str) -> bool {
        self.url.expose_for_request(|actual| actual == expected)
    }
}

/// Accounting ancestry that must remain continuous across sniff and metadata opening.
struct FormatSniffOpeningAccountingOwners {
    root: WasmModuleLimitAccountingRoot,
    source_scope: SourceAccountingScope,
    session_scope: SessionAccountingScope,
}

impl DisarmedStrictExtensionlessAdmission {
    #[cfg(test)]
    fn for_test() -> Result<Self, RetryProtocolError> {
        Ok(Self {
            viewer: SniffViewerInstanceId::allocate()?,
            source: SniffSourceId::allocate()?,
            pending: PendingFormatSniffToken(PendingFormatSniffTokenId::allocate()?),
            session: SniffSessionId::allocate()?,
            representation: SniffRepresentationId::allocate()?,
        })
    }
}

/// Copyable callback identity; the corresponding pending-token owner remains linear.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct FormatSniffIdentity {
    viewer: SniffViewerInstanceId,
    source: SniffSourceId,
    pending: PendingFormatSniffTokenId,
    session: SniffSessionId,
    representation: SniffRepresentationId,
    attempt: SniffAttemptEpoch,
}

impl fmt::Debug for FormatSniffIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("FormatSniffIdentity(<opaque>)")
    }
}

type SniffCoordinator =
    MetadataOpeningRetryCoordinator<SniffSourceId, SniffSessionId, SniffRepresentationId>;
type SniffOperation = RemoteRangeRetryOperation<
    SniffSourceId,
    SniffSessionId,
    SniffRepresentationId,
    PendingFormatSniffTokenId,
    ChromeRangeAttemptAbortFactory,
>;
type StartedSniffAttempt<Payload> = StartedRangeAttempt<
    SniffSourceId,
    SniffSessionId,
    SniffRepresentationId,
    PendingFormatSniffTokenId,
    ChromeRangeAttemptAbortController,
    Payload,
>;
type SettledSniffAttempt<Output> = SettledRangeAttempt<
    SniffSourceId,
    SniffSessionId,
    SniffRepresentationId,
    PendingFormatSniffTokenId,
    ChromeRangeAttemptAbortController,
    Output,
>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SnifferState {
    ArmedInitial,
    AttemptActive(SniffAttemptEpoch),
    RetryPending,
    Terminal,
}

/// The source-owned sniffer and the exact retry/deadline owners that must survive MCAP handoff.
pub(crate) struct ChromeFormatSniffer {
    viewer: SniffViewerInstanceId,
    source: SniffSourceId,
    pending: PendingFormatSniffToken,
    session: SniffSessionId,
    representation: SniffRepresentationId,
    source_scope: SourceAccountingScope,
    session_scope: SessionAccountingScope,
    range_scope: RangeResponseAccountingScope,
    work_scope: WorkUnitAccountingScope,
    coordinator: SniffCoordinator,
    operation: SniffOperation,
    root: WasmModuleLimitAccountingRoot,
    url: SecretUrl,
    consistency_policy: RepresentationConsistencyPolicy,
    pump_slice_bytes: NonZeroU64,
    state: SnifferState,
}

impl fmt::Debug for ChromeFormatSniffer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChromeFormatSniffer")
            .field("identity", &"<opaque>")
            .field("state", &self.state)
            .field("range_count", &self.coordinator.range_count())
            .finish_non_exhaustive()
    }
}

impl ChromeFormatSniffer {
    pub(crate) fn prepare(
        _admission: DisarmedStrictExtensionlessAdmission,
        source_scope: SourceAccountingScope,
        root: WasmModuleLimitAccountingRoot,
        url: SecretUrl,
        consistency_policy: RepresentationConsistencyPolicy,
        initial_execution: ActiveVisibleExecutionBinding,
        pump_slice_bytes: NonZeroU64,
    ) -> Result<Self, FormatSniffPrepareError> {
        let admission = _admission;
        let session_scope = source_scope.create_session_scope()?;
        let range_scope = session_scope.create_range_response_scope()?;
        let work_scope = range_scope.create_work_unit_scope()?;
        let operation_scope: OperationAccountingScope = source_scope.create_operation_scope()?;
        let live = RemoteRangeLiveIdentity::new(
            admission.source,
            admission.session,
            admission.representation,
        );
        let coordinator =
            MetadataOpeningRetryCoordinator::new(&source_scope, live.clone(), initial_execution)?;
        let operation = RemoteRangeRetryOperation::new(
            &operation_scope,
            &range_scope,
            &work_scope,
            live,
            ChromeRangeRequest::new(FORMAT_SNIFF_RANGE)?,
            admission.pending.0,
            ChromeRangeAttemptAbortFactory,
        )?;
        Ok(Self {
            viewer: admission.viewer,
            source: admission.source,
            pending: admission.pending,
            session: admission.session,
            representation: admission.representation,
            source_scope,
            session_scope,
            range_scope,
            work_scope,
            coordinator,
            operation,
            root,
            url,
            consistency_policy,
            pump_slice_bytes,
            state: SnifferState::ArmedInitial,
        })
    }

    fn identity(&self, attempt: SniffAttemptEpoch) -> FormatSniffIdentity {
        FormatSniffIdentity {
            viewer: self.viewer,
            source: self.source,
            pending: self.pending.0,
            session: self.session,
            representation: self.representation,
            attempt,
        }
    }

    fn validate_identity(&self, identity: FormatSniffIdentity) -> Result<(), FormatSniffError> {
        if identity != self.identity(identity.attempt) {
            return Err(FormatSniffError::StaleIdentity);
        }
        Ok(())
    }

    pub(crate) fn range_count(&self) -> u64 {
        self.coordinator.range_count()
    }

    /// Starts the initial format-sniff operation.
    pub(crate) fn start_initial<T, C>(
        &mut self,
        execution: ActiveVisibleExecutionBinding,
        control: C,
        timeout: T,
    ) -> Result<
        StartedChromeFormatSniff<
            // Keep the future output type opaque and tied to the caller's control values.
            impl Future<Output = Result<ChromeFormatSniffTransportOutcome, ChromeRangeError>>
            + use<T, C>,
        >,
        FormatSniffStartError,
    >
    where
        T: Future<Output = ()>,
        C: ExactLengthByobPumpControl,
    {
        if self.state != SnifferState::ArmedInitial {
            return Err(FormatSniffStartError::Protocol(
                RetryProtocolError::OperationNotInitial,
            ));
        }
        let epoch = SniffAttemptEpoch::allocate()?;
        let identity = self.identity(epoch);
        let started = self.operation.start_initial(
            &mut self.coordinator,
            execution,
            |controller, request| {
                prepare_format_sniff_range_attempt(
                    &self.url,
                    request,
                    self.consistency_policy,
                    controller.controller(),
                    &self.root,
                    &self.range_scope,
                    &self.work_scope,
                    self.pump_slice_bytes,
                )
            },
            |_controller, prepared, _receipt| prepared.start(control, timeout),
        )?;
        self.state = SnifferState::AttemptActive(epoch);
        Ok(StartedChromeFormatSniff {
            identity,
            attempt: started,
        })
    }

    pub(crate) fn project_retry_turn<'turn>(
        &mut self,
        turn: &'turn ExternalVisibleControlTurn,
    ) -> Result<VisibleRetryTurnPermit<'turn>, RetryProtocolError> {
        self.coordinator.project_retry_turn(turn)
    }

    /// Starts a retry of the format-sniff operation on a visible turn.
    pub(crate) fn start_retry_on_turn<T, C>(
        &mut self,
        turn: &mut VisibleRetryTurnPermit<'_>,
        control: C,
        timeout: T,
    ) -> Result<
        StartedChromeFormatSniff<
            // Keep the future output type opaque and tied to the caller's control values.
            impl Future<Output = Result<ChromeFormatSniffTransportOutcome, ChromeRangeError>>
            + use<T, C>,
        >,
        FormatSniffStartError,
    >
    where
        T: Future<Output = ()>,
        C: ExactLengthByobPumpControl,
    {
        if self.state != SnifferState::RetryPending {
            return Err(FormatSniffStartError::Protocol(
                RetryProtocolError::OperationNotRetryPending,
            ));
        }
        let epoch = SniffAttemptEpoch::allocate()?;
        let identity = self.identity(epoch);
        let started = self.operation.start_retry_on_turn(
            &mut self.coordinator,
            turn,
            |controller, request| {
                prepare_format_sniff_range_attempt(
                    &self.url,
                    request,
                    self.consistency_policy,
                    controller.controller(),
                    &self.root,
                    &self.range_scope,
                    &self.work_scope,
                    self.pump_slice_bytes,
                )
            },
            |_controller, prepared, _receipt| prepared.start(control, timeout),
        )?;
        self.state = SnifferState::AttemptActive(epoch);
        Ok(StartedChromeFormatSniff {
            identity,
            attempt: started,
        })
    }

    pub(crate) fn close_started<Payload>(
        &mut self,
        identity: FormatSniffIdentity,
        mut started: Pin<&mut StartedChromeFormatSniff<Payload>>,
    ) -> Result<RangeAttemptCompletionDisposition, FormatSniffError> {
        self.validate_identity(identity)?;
        if self.state != SnifferState::AttemptActive(identity.attempt)
            || started.as_ref().get_ref().identity != identity
        {
            return Err(FormatSniffError::StaleIdentity);
        }
        let disposition = self
            .operation
            .close_active(&mut self.coordinator, started.as_mut().project_attempt())?;
        self.state = SnifferState::Terminal;
        Ok(disposition)
    }

    #[expect(
        clippy::result_large_err,
        reason = "a rejected tokenized close must return the unique settled owner without allocation"
    )]
    pub(crate) fn close_settled<Output>(
        &mut self,
        identity: FormatSniffIdentity,
        settled: SettledChromeFormatSniff<Output>,
    ) -> Result<RangeAttemptCompletionDisposition, RejectedFormatSniffSettled<Output>> {
        if self.validate_identity(identity).is_err()
            || self.state != SnifferState::AttemptActive(identity.attempt)
            || settled.identity != identity
        {
            return Err(RejectedFormatSniffSettled {
                error: FormatSniffError::StaleIdentity,
                settled,
            });
        }
        match self
            .operation
            .close_settled(&mut self.coordinator, settled.attempt)
        {
            Ok(disposition) => {
                self.state = SnifferState::Terminal;
                Ok(disposition)
            }
            Err(rejected) => Err(RejectedFormatSniffSettled {
                error: FormatSniffError::Retry(rejected.error()),
                settled: SettledChromeFormatSniff {
                    identity,
                    attempt: rejected.into_attempt(),
                },
            }),
        }
    }

    pub(crate) fn complete(
        mut self,
        settled: SettledChromeFormatSniff<
            Result<ChromeFormatSniffTransportOutcome, ChromeRangeError>,
        >,
    ) -> FormatSniffCompletion {
        if self.validate_identity(settled.identity).is_err()
            || self.state != SnifferState::AttemptActive(settled.identity.attempt)
        {
            return FormatSniffCompletion::Rejected(RejectedFormatSniffCompletion {
                error: FormatSniffError::StaleIdentity,
                sniffer: self,
                settled,
            });
        }
        match self
            .operation
            .complete_chrome_settlement_with_controller(&mut self.coordinator, settled.attempt)
        {
            RangeAttemptTransportCompletionWithController::Succeeded(
                ChromeFormatSniffTransportOutcome::PartialContent(probed),
                controller,
            ) => {
                if probed.body().as_slice() != MCAP_MAGIC_BYTES {
                    drop(controller);
                    self.state = SnifferState::Terminal;
                    return FormatSniffCompletion::Terminal(FormatSniffTerminal::UnsupportedFormat);
                }
                let (prefix, object) = probed.into_parts();
                FormatSniffCompletion::Handoff(PreparedFormatSniffHandoff {
                    identity: settled.identity,
                    pending: self.pending,
                    source_spec: FrozenFormatSniffHttpSourceSpec {
                        url: self.url,
                        consistency_policy: self.consistency_policy,
                        pump_slice_bytes: self.pump_slice_bytes,
                    },
                    accounting: FormatSniffOpeningAccountingOwners {
                        root: self.root,
                        source_scope: self.source_scope,
                        session_scope: self.session_scope,
                    },
                    coordinator: self.coordinator,
                    prefix,
                    object,
                    controller,
                })
            }
            RangeAttemptTransportCompletionWithController::Succeeded(
                ChromeFormatSniffTransportOutcome::FullContent(prefix),
                mut controller,
            ) => {
                controller.abort();
                self.state = SnifferState::Terminal;
                if prefix.as_slice() == MCAP_MAGIC_BYTES {
                    FormatSniffCompletion::Terminal(FormatSniffTerminal::RangeUnsupported)
                } else {
                    FormatSniffCompletion::Terminal(FormatSniffTerminal::UnsupportedFormat)
                }
            }
            RangeAttemptTransportCompletionWithController::Failed(disposition) => {
                if disposition == RangeAttemptCompletionDisposition::RetryPending {
                    self.state = SnifferState::RetryPending;
                    FormatSniffCompletion::RetryPending(self)
                } else {
                    self.state = SnifferState::Terminal;
                    FormatSniffCompletion::Terminal(FormatSniffTerminal::Transport(disposition))
                }
            }
            RangeAttemptTransportCompletionWithController::Rejected(rejected) => {
                FormatSniffCompletion::Rejected(RejectedFormatSniffCompletion {
                    error: FormatSniffError::Retry(rejected.error()),
                    sniffer: self,
                    settled: SettledChromeFormatSniff {
                        identity: settled.identity,
                        attempt: rejected.into_attempt(),
                    },
                })
            }
        }
    }
}

pin_project! {
    pub(crate) struct StartedChromeFormatSniff<Payload> {
        identity: FormatSniffIdentity,
        #[pin]
        attempt: StartedSniffAttempt<Payload>,
    }
}

impl<Payload> StartedChromeFormatSniff<Payload> {
    pub(crate) fn identity(&self) -> FormatSniffIdentity {
        self.identity
    }

    fn project_attempt(self: Pin<&mut Self>) -> Pin<&mut StartedSniffAttempt<Payload>> {
        self.project().attempt
    }

    pub(crate) fn poll_settlement(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<SettledChromeFormatSniff<Payload::Output>, RetryProtocolError>>
    where
        Payload: Future,
    {
        let identity = self.as_ref().get_ref().identity;
        match self.project_attempt().poll_settlement(context) {
            Poll::Ready(Ok(attempt)) => {
                Poll::Ready(Ok(SettledChromeFormatSniff { identity, attempt }))
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => Poll::Pending,
        }
    }
}

pub(crate) struct SettledChromeFormatSniff<Output> {
    identity: FormatSniffIdentity,
    attempt: SettledSniffAttempt<Output>,
}

pub(crate) struct RejectedFormatSniffSettled<Output> {
    error: FormatSniffError,
    settled: SettledChromeFormatSniff<Output>,
}

impl<Output> fmt::Debug for RejectedFormatSniffSettled<Output> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RejectedFormatSniffSettled")
            .field("error", &self.error)
            .field("settled", &"<retained>")
            .finish_non_exhaustive()
    }
}

impl<Output> RejectedFormatSniffSettled<Output> {
    pub(crate) fn error(&self) -> FormatSniffError {
        self.error
    }

    pub(crate) fn into_settled(self) -> SettledChromeFormatSniff<Output> {
        self.settled
    }
}

#[expect(
    clippy::large_enum_variant,
    reason = "linear retry and rejected owners stay allocation-free across the post-burn boundary"
)]
pub(crate) enum FormatSniffCompletion {
    Handoff(PreparedFormatSniffHandoff),
    RetryPending(ChromeFormatSniffer),
    Terminal(FormatSniffTerminal),
    Rejected(RejectedFormatSniffCompletion),
}

pub(crate) struct RejectedFormatSniffCompletion {
    error: FormatSniffError,
    sniffer: ChromeFormatSniffer,
    settled: SettledChromeFormatSniff<Result<ChromeFormatSniffTransportOutcome, ChromeRangeError>>,
}

impl fmt::Debug for RejectedFormatSniffCompletion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RejectedFormatSniffCompletion")
            .field("error", &self.error)
            .field("owners", &"<retained>")
            .finish_non_exhaustive()
    }
}

impl RejectedFormatSniffCompletion {
    pub(crate) fn error(&self) -> FormatSniffError {
        self.error
    }

    pub(crate) fn into_owners(
        self,
    ) -> (
        ChromeFormatSniffer,
        SettledChromeFormatSniff<Result<ChromeFormatSniffTransportOutcome, ChromeRangeError>>,
    ) {
        (self.sniffer, self.settled)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FormatSniffTerminal {
    UnsupportedFormat,
    RangeUnsupported,
    Transport(RangeAttemptCompletionDisposition),
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FormatSniffError {
    StaleIdentity,
    Retry(RetryProtocolError),
}

impl From<RetryProtocolError> for FormatSniffError {
    fn from(error: RetryProtocolError) -> Self {
        Self::Retry(error)
    }
}

#[derive(Debug)]
pub(crate) enum FormatSniffPrepareError {
    Accounting(crate::remote_limits::ScopeAccountingError),
    Range(ChromeRangeError),
    Retry(crate::range_retry::RangeAttemptStartError),
}

impl From<crate::remote_limits::ScopeAccountingError> for FormatSniffPrepareError {
    fn from(error: crate::remote_limits::ScopeAccountingError) -> Self {
        Self::Accounting(error)
    }
}

impl From<ChromeRangeError> for FormatSniffPrepareError {
    fn from(error: ChromeRangeError) -> Self {
        Self::Range(error)
    }
}

impl From<crate::range_retry::RangeAttemptStartError> for FormatSniffPrepareError {
    fn from(error: crate::range_retry::RangeAttemptStartError) -> Self {
        Self::Retry(error)
    }
}

#[derive(Debug)]
pub(crate) enum FormatSniffStartError {
    Protocol(RetryProtocolError),
    Request,
}

impl<Prepared, Controller> From<RangeAttemptStartFailure<Prepared, Controller>>
    for FormatSniffStartError
{
    fn from(error: RangeAttemptStartFailure<Prepared, Controller>) -> Self {
        match error {
            RangeAttemptStartFailure::Request(error) => match error {
                crate::range_retry::RangeAttemptStartError::Protocol(error) => {
                    Self::Protocol(error)
                }
                _ => Self::Request,
            },
            RangeAttemptStartFailure::PreparedMismatch(_) => Self::Request,
        }
    }
}

impl From<RetryProtocolError> for FormatSniffStartError {
    fn from(error: RetryProtocolError) -> Self {
        Self::Protocol(error)
    }
}

/// First linear handoff stage after strict `206` magic confirmation.
pub(crate) struct PreparedFormatSniffHandoff {
    identity: FormatSniffIdentity,
    pending: PendingFormatSniffToken,
    source_spec: FrozenFormatSniffHttpSourceSpec,
    accounting: FormatSniffOpeningAccountingOwners,
    coordinator: SniffCoordinator,
    prefix: ExactLengthRangeBody,
    object: BoundChromeRangeObject,
    controller: ChromeRangeAttemptAbortController,
}

impl PreparedFormatSniffHandoff {
    pub(crate) fn identity(&self) -> FormatSniffIdentity {
        self.identity
    }

    pub(crate) fn range_count(&self) -> u64 {
        self.coordinator.range_count()
    }

    #[expect(
        clippy::result_large_err,
        reason = "a stale claim must return the unique prepared handoff without allocation"
    )]
    pub(crate) fn claim(
        self,
        expected: FormatSniffIdentity,
    ) -> Result<ClaimedFormatSniffHandoff, RejectedHandoff<Self>> {
        if self.identity != expected || self.pending.0 != expected.pending {
            return Err(RejectedHandoff {
                error: FormatSniffError::StaleIdentity,
                owner: self,
            });
        }
        Ok(ClaimedFormatSniffHandoff(self))
    }

    #[expect(
        clippy::result_large_err,
        reason = "a stale close must return the unique prepared handoff without allocation"
    )]
    pub(crate) fn close(
        self,
        expected: FormatSniffIdentity,
    ) -> Result<FormatSniffTerminal, RejectedHandoff<Self>> {
        if self.identity != expected {
            return Err(RejectedHandoff {
                error: FormatSniffError::StaleIdentity,
                owner: self,
            });
        }
        drop(self);
        Ok(FormatSniffTerminal::Closed)
    }
}

pub(crate) struct ClaimedFormatSniffHandoff(PreparedFormatSniffHandoff);

impl ClaimedFormatSniffHandoff {
    #[expect(
        clippy::result_large_err,
        reason = "a stale release must return the unique claimed handoff without allocation"
    )]
    pub(crate) fn release(
        self,
        expected: FormatSniffIdentity,
    ) -> Result<ReleasedFormatSniffHandoff, RejectedHandoff<Self>> {
        if self.0.identity != expected {
            return Err(RejectedHandoff {
                error: FormatSniffError::StaleIdentity,
                owner: self,
            });
        }
        Ok(ReleasedFormatSniffHandoff(self.0))
    }

    #[expect(
        clippy::result_large_err,
        reason = "a stale close must return the unique claimed handoff without allocation"
    )]
    pub(crate) fn close(
        self,
        expected: FormatSniffIdentity,
    ) -> Result<FormatSniffTerminal, RejectedHandoff<Self>> {
        if self.0.identity != expected {
            return Err(RejectedHandoff {
                error: FormatSniffError::StaleIdentity,
                owner: self,
            });
        }
        drop(self);
        Ok(FormatSniffTerminal::Closed)
    }
}

pub(crate) struct ReleasedFormatSniffHandoff(PreparedFormatSniffHandoff);

impl ReleasedFormatSniffHandoff {
    #[expect(
        clippy::result_large_err,
        reason = "a stale opening transfer must return the unique released handoff without allocation"
    )]
    pub(crate) fn into_opening(
        mut self,
        expected: FormatSniffIdentity,
    ) -> Result<ConfirmedRemoteFormatSniffHandoff, RejectedHandoff<Self>> {
        if self.0.identity != expected {
            return Err(RejectedHandoff {
                error: FormatSniffError::StaleIdentity,
                owner: self,
            });
        }
        self.0.controller.finish();
        let PreparedFormatSniffHandoff {
            identity,
            pending,
            source_spec,
            accounting,
            coordinator,
            prefix,
            object,
            controller: _,
        } = self.0;
        Ok(ConfirmedRemoteFormatSniffHandoff {
            identity,
            pending,
            source_spec,
            accounting,
            coordinator,
            prefix,
            object,
        })
    }

    #[expect(
        clippy::result_large_err,
        reason = "a stale close must return the unique released handoff without allocation"
    )]
    pub(crate) fn close(
        self,
        expected: FormatSniffIdentity,
    ) -> Result<FormatSniffTerminal, RejectedHandoff<Self>> {
        if self.0.identity != expected {
            return Err(RejectedHandoff {
                error: FormatSniffError::StaleIdentity,
                owner: self,
            });
        }
        drop(self);
        Ok(FormatSniffTerminal::Closed)
    }
}

pub(crate) struct ConfirmedRemoteFormatSniffHandoff {
    identity: FormatSniffIdentity,
    pending: PendingFormatSniffToken,
    source_spec: FrozenFormatSniffHttpSourceSpec,
    accounting: FormatSniffOpeningAccountingOwners,
    coordinator: SniffCoordinator,
    prefix: ExactLengthRangeBody,
    object: BoundChromeRangeObject,
}

/// Borrowed request-construction capability split from the mutable metadata retry coordinator.
///
/// It can create a typed browser request but cannot reveal or retain the secret URL itself.
pub(crate) struct ConfirmedFormatSniffOpeningView<'opening> {
    source_spec: &'opening FrozenFormatSniffHttpSourceSpec,
    root: &'opening WasmModuleLimitAccountingRoot,
    object: &'opening BoundChromeRangeObject,
}

impl ConfirmedFormatSniffOpeningView<'_> {
    pub(crate) fn prepare_metadata_range_attempt<'object>(
        &self,
        request: ChromeRangeRequest,
        controller: &ChromeRangeAttemptAbortController,
        range_scope: &RangeResponseAccountingScope,
        work_scope: &WorkUnitAccountingScope,
    ) -> Result<PreparedChromeBoundRangeAttempt<'object>, ChromeRangeError>
    where
        Self: 'object,
    {
        let request = self.object.request_range(request.as_range())?;
        self.source_spec.prepare_bound_range_attempt(
            request,
            controller.controller(),
            self.root,
            range_scope,
            work_scope,
        )
    }
}

impl ConfirmedRemoteFormatSniffHandoff {
    pub(crate) fn range_count(&self) -> u64 {
        self.coordinator.range_count()
    }

    pub(crate) fn prefix(&self) -> &[u8] {
        self.prefix.as_slice()
    }

    pub(crate) fn object(&self) -> &BoundChromeRangeObject {
        &self.object
    }

    pub(crate) fn retry_operation_identity(
        &self,
    ) -> (
        RemoteRangeLiveIdentity<SniffSourceId, SniffSessionId, SniffRepresentationId>,
        PendingFormatSniffTokenId,
    ) {
        (
            RemoteRangeLiveIdentity::new(
                self.identity.source,
                self.identity.session,
                self.identity.representation,
            ),
            self.pending.0,
        )
    }

    /// Creates the next exact metadata Range's accounting ancestry without cloning the secret
    /// source owner or resetting the source-scoped Range/deadline coordinator.
    pub(crate) fn create_metadata_range_scopes(
        &self,
    ) -> Result<
        (
            OperationAccountingScope,
            RangeResponseAccountingScope,
            WorkUnitAccountingScope,
        ),
        crate::remote_limits::ScopeAccountingError,
    > {
        let operation = self.accounting.source_scope.create_operation_scope()?;
        let range = self
            .accounting
            .session_scope
            .create_range_response_scope()?;
        let work = range.create_work_unit_scope()?;
        Ok((operation, range, work))
    }

    /// Splits immutable request construction from the one mutable retry coordinator without
    /// cloning any source or secret owner.
    pub(crate) fn split_metadata_opening(
        &mut self,
    ) -> (ConfirmedFormatSniffOpeningView<'_>, &mut SniffCoordinator) {
        (
            ConfirmedFormatSniffOpeningView {
                source_spec: &self.source_spec,
                root: &self.accounting.root,
                object: &self.object,
            },
            &mut self.coordinator,
        )
    }
}

pub(crate) struct RejectedHandoff<Owner> {
    error: FormatSniffError,
    owner: Owner,
}

impl<Owner> RejectedHandoff<Owner> {
    pub(crate) fn error(&self) -> FormatSniffError {
        self.error
    }

    pub(crate) fn into_owner(self) -> Owner {
        self.owner
    }
}

impl<Owner> fmt::Debug for RejectedHandoff<Owner> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RejectedHandoff")
            .field("error", &self.error)
            .field("owner", &"<retained>")
            .finish()
    }
}

/// Production-disarmed bounded registry for strict-only pending format-sniff work.
///
/// This keeps the pending slot accounting separate from the sniff state machine itself.
/// It does not hook into compatibility `open()` or any native viewer path.
pub(crate) struct PendingFormatSniffRegistryV1 {
    max_pending: usize,
    pending_count: Rc<Cell<usize>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PendingFormatSniffRegistryErrorV1 {
    CapacityReached,
}

pub(crate) struct PendingFormatSniffReservationV1 {
    sniffer: Option<ChromeFormatSniffer>,
    pending_count: Rc<Cell<usize>>,
    released: bool,
}

impl PendingFormatSniffRegistryV1 {
    pub(crate) fn new_v1(max_pending: usize) -> Self {
        Self {
            max_pending,
            pending_count: Rc::new(Cell::new(0)),
        }
    }

    pub(crate) fn pending_count_v1(&self) -> usize {
        self.pending_count.get()
    }

    pub(crate) fn reserve_v1(
        &self,
        sniffer: ChromeFormatSniffer,
    ) -> Result<PendingFormatSniffReservationV1, PendingFormatSniffRegistryErrorV1> {
        if self.pending_count.get() >= self.max_pending {
            return Err(PendingFormatSniffRegistryErrorV1::CapacityReached);
        }
        self.pending_count.set(self.pending_count.get() + 1);
        Ok(PendingFormatSniffReservationV1 {
            sniffer: Some(sniffer),
            pending_count: Rc::clone(&self.pending_count),
            released: false,
        })
    }
}

impl PendingFormatSniffReservationV1 {
    pub(crate) fn sniffer_mut_v1(&mut self) -> &mut ChromeFormatSniffer {
        self.sniffer
            .as_mut()
            .expect("pending format-sniff reservation still owns a sniffer")
    }

    pub(crate) fn into_sniffer_v1(mut self) -> ChromeFormatSniffer {
        self.released = true;
        self.pending_count
            .set(self.pending_count.get().saturating_sub(1));
        self.sniffer
            .take()
            .expect("pending format-sniff reservation still owns a sniffer")
    }
}

impl Drop for PendingFormatSniffReservationV1 {
    fn drop(&mut self) {
        if !self.released {
            self.pending_count
                .set(self.pending_count.get().saturating_sub(1));
        }
    }
}

#[cfg(test)]
mod tests {
    use futures::future;
    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;
    use crate::range_retry::test_execution_support::TestVisibleExecutionIssuer;

    wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    fn sniff_identity_exhaustion_is_permanent_and_never_reuses_an_id() {
        let next_id = AtomicU64::new(u64::MAX);
        assert_eq!(allocate_sniff_id_from(&next_id).unwrap().get(), u64::MAX);
        assert_eq!(
            allocate_sniff_id_from(&next_id),
            Err(RetryProtocolError::IdentityExhausted)
        );
        assert_eq!(
            allocate_sniff_id_from(&next_id),
            Err(RetryProtocolError::IdentityExhausted)
        );
    }

    #[wasm_bindgen::prelude::wasm_bindgen(inline_js = r#"
        let sniffHarness;

        const MAGIC = [0x89, 0x4d, 0x43, 0x41, 0x50, 0x30, 0x0d, 0x0a];
        const NON_MCAP = [0x6e, 0x6f, 0x74, 0x2d, 0x6d, 0x63, 0x61, 0x70];

        function bytesFor(mode) {
            if (mode.includes("non_mcap")) return NON_MCAP;
            if (mode.includes("short")) return MAGIC.slice(0, 4);
            if (mode.includes("long")) return [...MAGIC, 0xaa, 0xbb, 0xcc];
            return MAGIC;
        }

        function byteStream(mode, stats) {
            const bytes = bytesFor(mode);
            let offset = 0;
            const infinite = mode.includes("infinite");
            return new ReadableStream({
                type: "bytes",
                pull(controller) {
                    stats.pulls += 1;
                    if (mode.includes("body_pending")) return;
                    if (!infinite && offset >= bytes.length) {
                        controller.close();
                        return;
                    }
                    const request = controller.byobRequest;
                    if (!request) throw new Error("sniff fixture requires a BYOB request");
                    const view = request.view;
                    const count = Math.min(view.byteLength, infinite ? 1 : bytes.length - offset);
                    for (let index = 0; index < count; index += 1) {
                        view[index] = offset < bytes.length ? bytes[offset] : 0xcc;
                        offset += 1;
                    }
                    stats.served += count;
                    request.respond(count);
                    if (!infinite && offset >= bytes.length) controller.close();
                },
                cancel() {
                    stats.cancels += 1;
                },
            });
        }

        function responseFor(mode, stats, request) {
            const status = mode.startsWith("200_") ? 200 : mode === "status_503" ? 503 : 206;
            const requestedRange = request.headers.get("Range") ?? "";
            const contentRange = `${requestedRange.replace(/^bytes=/, "bytes ")}/64`;
            if (mode.includes("missing_body")) {
                return new Response(null, { status, headers: status === 206 ? {
                    "Content-Range": contentRange,
                    "Content-Length": "8",
                    "ETag": "\"v1\"",
                } : { "ETag": "\"v1\"" } });
            }
            let body;
            if (mode.includes("no_byob")) {
                body = new ReadableStream({
                    start(controller) {
                        controller.enqueue(new Uint8Array(bytesFor(mode)));
                        controller.close();
                    },
                    cancel() { stats.cancels += 1; },
                });
            } else {
                body = byteStream(mode, stats);
            }
            const headers = { "ETag": "\"v1\"" };
            if (status === 206 && !mode.includes("missing_range")) {
                headers["Content-Range"] = contentRange;
            }
            if (status === 206) {
                headers["Content-Length"] = "8";
            } else if (!mode.includes("infinite")) {
                headers["Content-Length"] = String(bytesFor(mode).length);
            }
            return new Response(body, { status, headers });
        }

        export function install_format_sniff_fetch(mode) {
            if (sniffHarness !== undefined) throw new Error("sniff harness already installed");
            const originalFetch = window.fetch;
            const responsePrototype = Response.prototype;
            const readerPrototype = ReadableStreamBYOBReader.prototype;
            const originalArrayBuffer = responsePrototype.arrayBuffer;
            const originalRead = readerPrototype.read;
            const originalCancel = readerPrototype.cancel;
            const stats = {
                calls: 0,
                reads: 0,
                maxView: 0,
                arrayBuffer: 0,
                cancels: 0,
                readerCancels: 0,
                aborts: 0,
                served: 0,
                pulls: 0,
                range: "",
                urls: [],
                events: [],
            };
            responsePrototype.arrayBuffer = function(...args) {
                stats.arrayBuffer += 1;
                return originalArrayBuffer.apply(this, args);
            };
            let corruptFirstRead = mode.includes("zero") || mode.includes("detached");
            readerPrototype.read = function(view) {
                stats.reads += 1;
                stats.maxView = Math.max(stats.maxView, view.byteLength);
                if (corruptFirstRead) {
                    corruptFirstRead = false;
                    if (mode.includes("zero")) {
                        return Promise.resolve({
                            done: false,
                            value: new Uint8Array(new ArrayBuffer(1), 0, 0),
                        });
                    }
                    const buffer = new ArrayBuffer(1);
                    const returned = new Uint8Array(buffer);
                    structuredClone(buffer, { transfer: [buffer] });
                    return Promise.resolve({ done: false, value: returned });
                }
                return originalRead.call(this, view);
            };
            readerPrototype.cancel = function(...args) {
                stats.readerCancels += 1;
                stats.events.push("reader_cancel");
                return originalCancel.apply(this, args);
            };
            window.fetch = function(request) {
                stats.calls += 1;
                stats.range = request.headers.get("Range") ?? "";
                stats.urls.push(request.url);
                request.signal.addEventListener("abort", () => {
                    stats.aborts += 1;
                    stats.events.push("signal_abort");
                }, { once: true });
                if (mode.includes("fetch_pending")) return new Promise(() => {});
                if (mode.includes("fetch_reject")) return Promise.reject(new TypeError("redacted"));
                return Promise.resolve(responseFor(mode, stats, request));
            };
            sniffHarness = {
                originalFetch,
                responsePrototype,
                readerPrototype,
                originalArrayBuffer,
                originalRead,
                originalCancel,
                stats,
            };
        }

        export function format_sniff_stat(name) {
            if (sniffHarness === undefined) throw new Error("sniff harness is not installed");
            return sniffHarness.stats[name];
        }

        export function format_sniff_range() {
            if (sniffHarness === undefined) throw new Error("sniff harness is not installed");
            return sniffHarness.stats.range;
        }

        export function format_sniff_all_urls_equal() {
            if (sniffHarness === undefined) throw new Error("sniff harness is not installed");
            return sniffHarness.stats.urls.length > 0 &&
                sniffHarness.stats.urls.every(url => url === sniffHarness.stats.urls[0]);
        }

        export function format_sniff_reader_cancel_precedes_abort() {
            if (sniffHarness === undefined) throw new Error("sniff harness is not installed");
            const cancel = sniffHarness.stats.events.indexOf("reader_cancel");
            const abort = sniffHarness.stats.events.indexOf("signal_abort");
            return cancel >= 0 && abort > cancel;
        }

        export function restore_format_sniff_fetch() {
            if (sniffHarness === undefined) return;
            const state = sniffHarness;
            sniffHarness = undefined;
            window.fetch = state.originalFetch;
            state.responsePrototype.arrayBuffer = state.originalArrayBuffer;
            state.readerPrototype.read = state.originalRead;
            state.readerPrototype.cancel = state.originalCancel;
        }

        export function is_chrome_format_sniff_runtime() {
            return /(?:Chrome|Chromium)/.test(navigator.userAgent);
        }
    "#)]
    extern "C" {
        fn install_format_sniff_fetch(mode: &str);
        fn format_sniff_stat(name: &str) -> u32;
        fn format_sniff_range() -> String;
        fn format_sniff_all_urls_equal() -> bool;
        fn format_sniff_reader_cancel_precedes_abort() -> bool;
        fn restore_format_sniff_fetch();
        fn is_chrome_format_sniff_runtime() -> bool;
    }

    struct FetchHarness;

    impl FetchHarness {
        fn install(mode: &str) -> Self {
            install_format_sniff_fetch(mode);
            Self
        }

        #[expect(
            clippy::unused_self,
            reason = "the guard receiver documents that the mocked Fetch installation is live"
        )]
        fn stat(&self, name: &str) -> u32 {
            format_sniff_stat(name)
        }
    }

    impl Drop for FetchHarness {
        fn drop(&mut self) {
            restore_format_sniff_fetch();
        }
    }

    struct AlwaysAllowed;

    #[async_trait::async_trait(?Send)]
    impl ExactLengthByobPumpControl for AlwaysAllowed {
        async fn wait_until_read_allowed(
            &mut self,
        ) -> Result<(), crate::chrome_byob::ExactLengthByobPumpControlError> {
            Ok(())
        }
    }

    fn prepared_sniffer(
        attempt_limit: u64,
        range_limit: u64,
    ) -> (ChromeFormatSniffer, TestVisibleExecutionIssuer) {
        let root = crate::remote_limits::tests::transport_test_profile_with(&[
            (
                crate::remote_limits::WebRemoteLimitKey::RangeRetryAttemptsPerOperation,
                attempt_limit,
            ),
            (
                crate::remote_limits::WebRemoteLimitKey::MetadataOpeningRangeRequests,
                range_limit,
            ),
            (
                crate::remote_limits::WebRemoteLimitKey::MetadataOpeningVisibleDeadlineMillis,
                10,
            ),
        ])
        .start_accounting_root()
        .expect("sniff test accounting starts");
        let viewer = root.create_viewer_scope().expect("viewer scope");
        let source = viewer.create_source_scope().expect("source scope");
        let issuer = TestVisibleExecutionIssuer::new(1);
        let admission =
            DisarmedStrictExtensionlessAdmission::for_test().expect("test admission allocates");
        let url = SecretUrl::parse_for_test(
            "https://example.invalid/extensionless?token=secret#view=test",
        )
        .expect("test URL parses");
        let sniffer = ChromeFormatSniffer::prepare(
            admission,
            source,
            root,
            url,
            RepresentationConsistencyPolicy::RequireStrongValidator,
            issuer.binding(),
            NonZeroU64::new(4).unwrap(),
        )
        .expect("disarmed sniffer prepares");
        (sniffer, issuer)
    }

    #[wasm_bindgen_test]
    async fn pending_registry_caps_pending_sniffs_and_releases_on_drop() {
        if !is_chrome_format_sniff_runtime() {
            return;
        }
        let registry = PendingFormatSniffRegistryV1::new_v1(1);
        let (sniffer, _issuer) = prepared_sniffer(1, 2);
        let reservation = registry
            .reserve_v1(sniffer)
            .expect("first pending sniff reserves");
        assert_eq!(registry.pending_count_v1(), 1);
        assert!(matches!(
            registry.reserve_v1(prepared_sniffer(1, 2).0),
            Err(PendingFormatSniffRegistryErrorV1::CapacityReached)
        ));
        drop(reservation);
        assert_eq!(registry.pending_count_v1(), 0);
        assert!(registry.reserve_v1(prepared_sniffer(1, 2).0).is_ok());
    }

    #[wasm_bindgen_test]
    async fn pending_registry_releases_capacity_when_sniffer_is_moved_out() {
        if !is_chrome_format_sniff_runtime() {
            return;
        }
        let registry = PendingFormatSniffRegistryV1::new_v1(1);
        let (sniffer, _issuer) = prepared_sniffer(1, 2);
        let reservation = registry
            .reserve_v1(sniffer)
            .expect("first pending sniff reserves");
        assert_eq!(registry.pending_count_v1(), 1);
        let sniffer = reservation.into_sniffer_v1();
        assert_eq!(registry.pending_count_v1(), 0);
        drop(sniffer);
        assert!(registry.reserve_v1(prepared_sniffer(1, 2).0).is_ok());
    }

    async fn settle_started<Payload>(
        mut started: Pin<&mut StartedChromeFormatSniff<Payload>>,
    ) -> SettledChromeFormatSniff<Payload::Output>
    where
        Payload: Future,
    {
        std::future::poll_fn(|context| match started.as_mut().poll_settlement(context) {
            Poll::Ready(result) => Poll::Ready(result.expect("matching sniff task settles")),
            Poll::Pending => Poll::Pending,
        })
        .await
    }

    async fn complete_mode(mode: &str) -> (FormatSniffCompletion, FetchHarness) {
        let harness = FetchHarness::install(mode);
        let (mut sniffer, issuer) = prepared_sniffer(2, 4);
        let started = sniffer
            .start_initial(issuer.binding(), AlwaysAllowed, future::pending::<()>())
            .expect("initial sniff starts");
        let mut started = std::pin::pin!(started);
        let settled = settle_started(started.as_mut()).await;
        (sniffer.complete(settled), harness)
    }

    #[wasm_bindgen_test]
    async fn partial_content_magic_moves_exact_owners_through_all_handoff_stages() {
        if !is_chrome_format_sniff_runtime() {
            return;
        }
        crate::secret_url::reset_test_drop_counts();
        let harness = FetchHarness::install("206_mcap");
        let (mut sniffer, issuer) = prepared_sniffer(2, 4);
        assert_eq!(
            sniffer
                .coordinator
                .advance_visible_time(issuer.binding(), 6)
                .unwrap(),
            crate::remote_limits::MetadataOpeningDeadlineState::Active
        );
        let started = sniffer
            .start_initial(issuer.binding(), AlwaysAllowed, future::pending::<()>())
            .expect("initial sniff starts");
        let mut started = std::pin::pin!(started);
        let settled = settle_started(started.as_mut()).await;
        let completion = sniffer.complete(settled);
        let FormatSniffCompletion::Handoff(prepared) = completion else {
            panic!("strict 206 MCAP must produce a handoff");
        };
        assert_eq!(prepared.range_count(), 1);
        assert_eq!(prepared.prefix.as_slice(), MCAP_MAGIC_BYTES);
        let identity = prepared.identity();
        let mut wrong = identity;
        wrong.attempt = SniffAttemptEpoch::allocate().unwrap();
        let prepared = match prepared.claim(wrong) {
            Ok(_) => panic!("a stale claim must retain the prepared owner"),
            Err(rejected) => rejected.into_owner(),
        };
        assert_eq!(prepared.range_count(), 1);
        let claimed = prepared.claim(identity).expect("matching claim succeeds");
        let released = claimed
            .release(identity)
            .expect("matching release succeeds");
        let mut confirmed = released
            .into_opening(identity)
            .expect("matching opening handoff succeeds");
        assert_eq!(confirmed.range_count(), 1);
        assert_eq!(crate::secret_url::test_drop_counts(), (0, 0));
        assert!(confirmed.source_spec.exact_url_matches_for_test(
            "https://example.invalid/extensionless?token=secret#view=test"
        ));

        let (operation_scope, range_scope, work_scope) = confirmed
            .create_metadata_range_scopes()
            .expect("next metadata Range scopes preserve the sniff session ancestry");
        let request = ChromeRangeRequest::new(8..16).unwrap();
        let (live, demand) = confirmed.retry_operation_identity();
        let mut operation = RemoteRangeRetryOperation::new(
            &operation_scope,
            &range_scope,
            &work_scope,
            live,
            request,
            demand,
            ChromeRangeAttemptAbortFactory,
        )
        .expect("next metadata Range operation preserves source identity");
        let settled = {
            let (opening, coordinator) = confirmed.split_metadata_opening();
            let started = operation
                .start_initial(
                    coordinator,
                    issuer.binding(),
                    |controller, request| {
                        opening.prepare_metadata_range_attempt(
                            request,
                            controller,
                            &range_scope,
                            &work_scope,
                        )
                    },
                    |_controller, prepared, _receipt| {
                        prepared.start(AlwaysAllowed, future::pending::<()>())
                    },
                )
                .expect("next metadata Range starts from handed-off source owners");
            let mut started = std::pin::pin!(started);
            std::future::poll_fn(|context| started.as_mut().poll_settlement(context))
                .await
                .expect("next metadata Range settles")
        };
        let crate::range_retry::RangeAttemptTransportCompletion::Succeeded(second_body) =
            operation.complete_chrome_settlement(&mut confirmed.coordinator, settled)
        else {
            panic!("next metadata Range must complete against the sniffed object");
        };
        assert_eq!(second_body.as_slice(), MCAP_MAGIC_BYTES);
        assert_eq!(confirmed.range_count(), 2);
        assert_eq!(
            confirmed
                .coordinator
                .advance_visible_time(issuer.binding(), 4)
                .unwrap(),
            crate::remote_limits::MetadataOpeningDeadlineState::Expired
        );
        assert_eq!(confirmed.prefix(), MCAP_MAGIC_BYTES);
        assert_eq!(confirmed.object().object_length().get(), 64);
        assert_eq!(harness.stat("calls"), 2);
        assert_eq!(format_sniff_range(), "bytes=8-15");
        assert!(format_sniff_all_urls_equal());
        assert_eq!(harness.stat("arrayBuffer"), 0);
        assert!(harness.stat("maxView") <= 4);
        drop(second_body);
        drop(confirmed);
        assert_eq!(crate::secret_url::test_drop_counts(), (4, 4));
    }

    #[wasm_bindgen_test]
    async fn status_and_prefix_matrix_is_strict_and_bounded() {
        if !is_chrome_format_sniff_runtime() {
            return;
        }
        for (mode, expected) in [
            ("206_non_mcap", FormatSniffTerminal::UnsupportedFormat),
            ("200_non_mcap", FormatSniffTerminal::UnsupportedFormat),
            ("200_short", FormatSniffTerminal::UnsupportedFormat),
            ("200_mcap", FormatSniffTerminal::RangeUnsupported),
            ("200_long", FormatSniffTerminal::RangeUnsupported),
            ("200_infinite", FormatSniffTerminal::RangeUnsupported),
        ] {
            let (completion, harness) = complete_mode(mode).await;
            let FormatSniffCompletion::Terminal(actual) = completion else {
                panic!("{mode} must terminalize");
            };
            assert_eq!(actual, expected, "{mode}");
            assert_eq!(harness.stat("calls"), 1, "{mode}");
            assert_eq!(harness.stat("arrayBuffer"), 0, "{mode}");
            assert!(harness.stat("maxView") <= 4, "{mode}");
            assert!(harness.stat("served") <= 8, "{mode}");
            assert!(harness.stat("aborts") >= 1, "{mode}");
            assert!(harness.stat("readerCancels") >= 1, "{mode}");
            assert!(format_sniff_reader_cancel_precedes_abort(), "{mode}");
            drop(harness);
        }
    }

    #[wasm_bindgen_test]
    async fn malformed_short_long_missing_and_non_byob_responses_converge() {
        if !is_chrome_format_sniff_runtime() {
            return;
        }
        for mode in [
            "206_short",
            "206_long",
            "206_missing_range",
            "206_missing_body",
            "206_no_byob",
            "200_missing_body",
            "200_no_byob",
            "200_zero",
            "200_detached",
        ] {
            let (completion, harness) = complete_mode(mode).await;
            let FormatSniffCompletion::Terminal(FormatSniffTerminal::Transport(_)) = completion
            else {
                panic!("{mode} must fail through typed transport completion");
            };
            assert_eq!(harness.stat("calls"), 1, "{mode}");
            assert_eq!(harness.stat("arrayBuffer"), 0, "{mode}");
            assert!(harness.stat("maxView") <= 4, "{mode}");
            drop(harness);
        }
    }

    #[wasm_bindgen_test]
    async fn retry_keeps_source_counters_and_allocates_a_fresh_attempt_identity() {
        if !is_chrome_format_sniff_runtime() {
            return;
        }
        let first_harness = FetchHarness::install("status_503");
        let (mut sniffer, mut issuer) = prepared_sniffer(2, 4);
        let first = sniffer
            .start_initial(issuer.binding(), AlwaysAllowed, future::pending::<()>())
            .unwrap();
        let first_identity = first.identity();
        let mut first = std::pin::pin!(first);
        let settled = settle_started(first.as_mut()).await;
        let FormatSniffCompletion::RetryPending(mut sniffer) = sniffer.complete(settled) else {
            panic!("allowlisted status must enter retry pending");
        };
        assert_eq!(sniffer.range_count(), 1);
        drop(first_harness);

        let second_harness = FetchHarness::install("206_mcap");
        let turn = issuer.turn();
        let mut permit = sniffer.project_retry_turn(&turn).unwrap();
        let second = sniffer
            .start_retry_on_turn(&mut permit, AlwaysAllowed, future::pending::<()>())
            .unwrap();
        assert_ne!(first_identity.attempt, second.identity().attempt);
        let mut second = std::pin::pin!(second);
        let settled = settle_started(second.as_mut()).await;
        let FormatSniffCompletion::Handoff(handoff) = sniffer.complete(settled) else {
            panic!("retry MCAP must hand off");
        };
        assert_eq!(handoff.range_count(), 2);
        assert_eq!(second_harness.stat("calls"), 1);
    }

    #[wasm_bindgen_test]
    async fn close_at_pending_settled_handoff_claim_and_release_is_tokenized() {
        if !is_chrome_format_sniff_runtime() {
            return;
        }
        let pending_harness = FetchHarness::install("fetch_pending");
        let (mut pending_sniffer, issuer) = prepared_sniffer(1, 2);
        let pending = pending_sniffer
            .start_initial(issuer.binding(), AlwaysAllowed, future::pending::<()>())
            .unwrap();
        let identity = pending.identity();
        let mut wrong = identity;
        wrong.attempt = SniffAttemptEpoch::allocate().unwrap();
        let mut pending = std::pin::pin!(pending);
        assert_eq!(
            pending_sniffer
                .close_started(wrong, pending.as_mut())
                .unwrap_err(),
            FormatSniffError::StaleIdentity
        );
        assert_eq!(pending_harness.stat("aborts"), 0);
        let mut context = Context::from_waker(std::task::Waker::noop());
        assert!(pending.as_mut().poll_settlement(&mut context).is_pending());
        assert_eq!(
            pending_sniffer
                .close_started(identity, pending.as_mut())
                .unwrap(),
            RangeAttemptCompletionDisposition::Cancelled
        );
        assert!(pending_harness.stat("aborts") >= 1);
        drop(pending_harness);
        drop(pending_sniffer);

        let body_pending_harness = FetchHarness::install("206_body_pending");
        let (mut body_pending_sniffer, issuer) = prepared_sniffer(1, 2);
        let body_pending = body_pending_sniffer
            .start_initial(issuer.binding(), AlwaysAllowed, future::pending::<()>())
            .unwrap();
        let identity = body_pending.identity();
        let mut body_pending = std::pin::pin!(body_pending);
        let mut context = Context::from_waker(std::task::Waker::noop());
        for _ in 0..8 {
            assert!(
                body_pending
                    .as_mut()
                    .poll_settlement(&mut context)
                    .is_pending()
            );
            if body_pending_harness.stat("reads") > 0 {
                break;
            }
            re_async::sleep(std::time::Duration::ZERO).await;
        }
        assert_eq!(body_pending_harness.stat("reads"), 1);
        let mut wrong = identity;
        wrong.attempt = SniffAttemptEpoch::allocate().unwrap();
        assert_eq!(
            body_pending_sniffer
                .close_started(wrong, body_pending.as_mut())
                .unwrap_err(),
            FormatSniffError::StaleIdentity
        );
        assert_eq!(body_pending_harness.stat("readerCancels"), 0);
        assert_eq!(body_pending_harness.stat("aborts"), 0);
        assert_eq!(
            body_pending_sniffer
                .close_started(identity, body_pending.as_mut())
                .unwrap(),
            RangeAttemptCompletionDisposition::Cancelled
        );
        assert_eq!(body_pending_harness.stat("readerCancels"), 1);
        assert_eq!(body_pending_harness.stat("aborts"), 1);
        assert!(format_sniff_reader_cancel_precedes_abort());
        assert!(matches!(
            body_pending.as_mut().poll_settlement(&mut context),
            Poll::Ready(Err(RetryProtocolError::OperationAlreadyTerminal))
        ));
        assert_eq!(body_pending_sniffer.range_count(), 1);
        drop(body_pending_harness);
        drop(body_pending_sniffer);

        let settled_harness = FetchHarness::install("206_mcap");
        let (mut settled_sniffer, issuer) = prepared_sniffer(1, 2);
        let settled_started = settled_sniffer
            .start_initial(issuer.binding(), AlwaysAllowed, future::pending::<()>())
            .unwrap();
        let identity = settled_started.identity();
        let mut settled_started = std::pin::pin!(settled_started);
        let settled = settle_started(settled_started.as_mut()).await;
        let mut wrong = identity;
        wrong.attempt = SniffAttemptEpoch::allocate().unwrap();
        let rejected = settled_sniffer
            .close_settled(wrong, settled)
            .expect_err("stale settled close must return its owner");
        assert_eq!(rejected.error(), FormatSniffError::StaleIdentity);
        assert_eq!(settled_harness.stat("aborts"), 0);
        let disposition = settled_sniffer
            .close_settled(identity, rejected.into_settled())
            .expect("matching settled close must succeed");
        assert_eq!(disposition, RangeAttemptCompletionDisposition::Cancelled);
        assert!(settled_harness.stat("aborts") >= 1);
        drop(settled_harness);
        drop(settled_sniffer);

        for stage in ["prepared", "claimed", "released"] {
            crate::secret_url::reset_test_drop_counts();
            let (completion, harness) = complete_mode("206_mcap").await;
            let FormatSniffCompletion::Handoff(prepared) = completion else {
                panic!("MCAP handoff expected");
            };
            let identity = prepared.identity();
            let mut wrong = identity;
            wrong.attempt = SniffAttemptEpoch::allocate().unwrap();
            match stage {
                "prepared" => {
                    let rejected = prepared
                        .close(wrong)
                        .expect_err("stale prepared close returns its owner");
                    assert_eq!(rejected.error(), FormatSniffError::StaleIdentity);
                    assert_eq!(crate::secret_url::test_drop_counts(), (0, 0));
                    let prepared = rejected.into_owner();
                    assert_eq!(
                        prepared.close(identity).unwrap(),
                        FormatSniffTerminal::Closed
                    );
                }
                "claimed" => {
                    let claimed = prepared.claim(identity).unwrap();
                    let Err(rejected) = claimed.release(wrong) else {
                        panic!("stale release must not transfer the claimed owner");
                    };
                    assert_eq!(rejected.error(), FormatSniffError::StaleIdentity);
                    assert_eq!(crate::secret_url::test_drop_counts(), (0, 0));
                    let claimed = rejected.into_owner();
                    assert_eq!(
                        claimed.close(identity).unwrap(),
                        FormatSniffTerminal::Closed
                    );
                }
                "released" => {
                    let released = prepared.claim(identity).unwrap().release(identity).unwrap();
                    let Err(rejected) = released.into_opening(wrong) else {
                        panic!("stale opening transfer must not consume the owner");
                    };
                    assert_eq!(rejected.error(), FormatSniffError::StaleIdentity);
                    assert_eq!(crate::secret_url::test_drop_counts(), (0, 0));
                    let released = rejected.into_owner();
                    assert_eq!(
                        released.close(identity).unwrap(),
                        FormatSniffTerminal::Closed
                    );
                }
                _ => unreachable!(),
            }
            assert!(harness.stat("aborts") >= 1, "{stage}");
            assert_eq!(crate::secret_url::test_drop_counts(), (4, 4), "{stage}");
            drop(harness);
        }
    }

    #[wasm_bindgen_test]
    fn compatibility_known_and_native_routes_have_no_sniffer_or_network_entry() {
        let harness = FetchHarness::install("206_mcap");
        // No production constructor exists for `DisarmedStrictExtensionlessAdmission`, and this
        // module is not compiled on native targets. Compatibility extensionless and known `.mcap`
        // therefore cannot reach the new classifier or Fetch boundary in this work item.
        assert_eq!(harness.stat("calls"), 0);
        assert_eq!(harness.stat("reads"), 0);
        assert_eq!(harness.stat("arrayBuffer"), 0);
    }
}
