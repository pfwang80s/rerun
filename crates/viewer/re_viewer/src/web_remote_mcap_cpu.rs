//! Production-disarmed Web remote-MCAP Fetch completion and CPU work driver.

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

#[cfg(target_arch = "wasm32")]
use std::cell::Cell;
#[cfg(target_arch = "wasm32")]
use std::rc::Rc;

use parking_lot::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct RemoteByteRangeV1 {
    start: u64,
    end_exclusive: u64,
}

/// Source-wide execution identity.
///
/// Per-read identity is deliberately not stored here: one driver must host multiple ordinals and
/// ranges from the same active attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct RemoteDriverIdentityV1 {
    source_generation: u64,
    activity_epoch: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct RemoteOperationIdentityV1 {
    operation_nonce: u64,
    attempt_generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RemoteOperationSettlementV1 {
    InFlight,
    RetryPending { next_attempt_generation: u64 },
    Succeeded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RemoteOperationStateV1 {
    attempt_generation: u64,
    settlement: RemoteOperationSettlementV1,
}

impl RemoteOperationIdentityV1 {
    fn next_attempt_v1(self) -> Option<Self> {
        Some(Self {
            attempt_generation: self.attempt_generation.checked_add(1)?,
            ..self
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum RemoteBudgetProfileV1 {
    UnfrozenPhaseACandidate,
}

/// Exact predicate for one sealed CPU work owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct RemoteWorkIdentityV1 {
    driver: RemoteDriverIdentityV1,
    operation: RemoteOperationIdentityV1,
    read_generation: u64,
    canonical_ordinal: u32,
    full_range: RemoteByteRangeV1,
    budget_profile: RemoteBudgetProfileV1,
    phase_generation: u32,
}

impl RemoteWorkIdentityV1 {
    fn is_direct_phase_successor_of_v1(self, predecessor: Self) -> bool {
        self.driver == predecessor.driver
            && self.operation != predecessor.operation
            && self.read_generation == predecessor.read_generation
            && self.canonical_ordinal == predecessor.canonical_ordinal
            && self.full_range == predecessor.full_range
            && self.budget_profile == predecessor.budget_profile
            && predecessor
                .phase_generation
                .checked_add(1)
                .is_some_and(|next| self.phase_generation == next)
    }

    #[cfg(rerun_mcap_phase_a_proof_v1)]
    fn for_phase_a_measurement_v1(input_bytes: u64, kind: RemoteCpuWorkKindV1) -> Self {
        Self {
            driver: RemoteDriverIdentityV1 {
                source_generation: 1,
                activity_epoch: 1,
            },
            operation: RemoteOperationIdentityV1 {
                operation_nonce: kind.index() as u64 + 1,
                attempt_generation: 1,
            },
            read_generation: 1,
            canonical_ordinal: 0,
            full_range: RemoteByteRangeV1 {
                start: 0,
                end_exclusive: input_bytes,
            },
            budget_profile: RemoteBudgetProfileV1::UnfrozenPhaseACandidate,
            phase_generation: 1,
        }
    }

    #[cfg(test)]
    fn for_test_v1(
        source_generation: u64,
        attempt_generation: u64,
        canonical_ordinal: u32,
    ) -> Self {
        Self {
            driver: RemoteDriverIdentityV1 {
                source_generation,
                activity_epoch: 2,
            },
            operation: RemoteOperationIdentityV1 {
                operation_nonce: u64::from(canonical_ordinal) + 100,
                attempt_generation,
            },
            read_generation: 3,
            canonical_ordinal,
            full_range: RemoteByteRangeV1 {
                start: canonical_ordinal as u64 * 100,
                end_exclusive: canonical_ordinal as u64 * 100 + 99,
            },
            budget_profile: RemoteBudgetProfileV1::UnfrozenPhaseACandidate,
            phase_generation: 1,
        }
    }
}

#[cfg(rerun_mcap_phase_a_proof_v1)]
pub(crate) struct PhaseADriverMeasurementV1 {
    pub(crate) pipeline: re_mcap::phase_a_measurement::PhaseAMeasurementOutputV1,
    pub(crate) max_duration_micros: u64,
    pub(crate) overflowed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteCpuWorkKindV1 {
    OpeningParse,
    MessageIndexParse,
    PhysicalChunkValidation,
    PhysicalChunkDispatchDecode,
}

impl RemoteCpuWorkKindV1 {
    const COUNT: usize = 4;

    const fn index(self) -> usize {
        match self {
            Self::OpeningParse => 0,
            Self::MessageIndexParse => 1,
            Self::PhysicalChunkValidation => 2,
            Self::PhysicalChunkDispatchDecode => 3,
        }
    }
}

#[cfg(not(any(test, rerun_mcap_phase_a_proof_v1)))]
enum ProductionDisarmedCpuCapabilityV1 {}

#[cfg(any(test, rerun_mcap_phase_a_proof_v1))]
struct DisarmedCpuExecutionV1<'work> {
    run: Box<
        dyn FnOnce() -> Result<RemoteCpuExecutionOutputV1<'work>, RemoteCpuExecutionErrorV1>
            + 'work,
    >,
}

struct SealedCpuWorkV1<'work> {
    identity: RemoteWorkIdentityV1,
    input_bytes: u64,
    expected_output_bytes: u64,
    predecessor: Option<RemoteWorkIdentityV1>,
    #[cfg(not(any(test, rerun_mcap_phase_a_proof_v1)))]
    capability: ProductionDisarmedCpuCapabilityV1,
    #[cfg(any(test, rerun_mcap_phase_a_proof_v1))]
    execution: DisarmedCpuExecutionV1<'work>,
}

trait RemoteCpuRetainedOutputV1 {
    fn retained_bytes_v1(&self) -> u64;
}

struct RemoteCpuExecutionOutputV1<'work> {
    owner: Box<dyn RemoteCpuRetainedOutputV1 + 'work>,
    successor: Option<RemoteCpuSuccessorV1<'work>>,
}

type RemoteCpuSuccessorV1<'work> = Box<
    dyn FnOnce() -> Result<RemoteCpuExecutionOutputV1<'work>, RemoteCpuExecutionErrorV1> + 'work,
>;

impl RemoteCpuExecutionOutputV1<'_> {
    fn retained_bytes_v1(&self) -> u64 {
        self.owner.retained_bytes_v1()
    }
}

struct RemoteByteOutputV1(Box<[u8]>);

impl RemoteCpuRetainedOutputV1 for RemoteByteOutputV1 {
    fn retained_bytes_v1(&self) -> u64 {
        u64::try_from(self.0.len()).expect("a materialized output length fits u64")
    }
}

#[derive(Default)]
struct RemoteCompletionInputBudgetUsageV1 {
    items: usize,
    bytes: u64,
}

struct RemoteCompletionInputBudgetStateV1 {
    max_items: usize,
    max_bytes: u64,
    usage: Mutex<RemoteCompletionInputBudgetUsageV1>,
}

struct RemoteCompletionInputReservationV1 {
    state: Arc<RemoteCompletionInputBudgetStateV1>,
    bytes: u64,
}

impl Drop for RemoteCompletionInputReservationV1 {
    fn drop(&mut self) {
        let mut usage = self.state.usage.lock();
        usage.items = usage
            .items
            .checked_sub(1)
            .expect("an input completion reservation owns one item");
        usage.bytes = usage
            .bytes
            .checked_sub(self.bytes)
            .expect("an input completion reservation owns its retained bytes");
    }
}

#[derive(Default)]
struct RemoteReadyOutputBudgetUsageV1 {
    items: usize,
    bytes: u64,
}

struct RemoteReadyOutputBudgetStateV1 {
    max_items: usize,
    max_bytes: u64,
    usage: Mutex<RemoteReadyOutputBudgetUsageV1>,
}

struct RemoteReadyOutputReservationV1 {
    state: Arc<RemoteReadyOutputBudgetStateV1>,
    bytes: u64,
}

impl Drop for RemoteReadyOutputReservationV1 {
    fn drop(&mut self) {
        let mut usage = self.state.usage.lock();
        usage.items = usage
            .items
            .checked_sub(1)
            .expect("a ready output reservation owns one item");
        usage.bytes = usage
            .bytes
            .checked_sub(self.bytes)
            .expect("a ready output reservation owns its expected bytes");
    }
}

struct ReservedCpuWorkV1<'work> {
    work: RemoteCpuWorkV1<'work>,
    input_reservation: RemoteCompletionInputReservationV1,
    output_reservation: RemoteReadyOutputReservationV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteCpuExecutionErrorV1 {
    BoundViolation,
    LayoutOverflow,
    AllocationFailed,
}

macro_rules! declare_cpu_phase {
    ($work:ident, $result:ident) => {
        pub(crate) struct $work<'work>(SealedCpuWorkV1<'work>);

        pub(crate) struct $result<'work> {
            // Field order is normative: output backing drops before the overlapping input/output
            // accounting permits.
            output: RemoteCpuExecutionOutputV1<'work>,
            input_reservation: RemoteCompletionInputReservationV1,
            output_reservation: RemoteReadyOutputReservationV1,
            identity: RemoteWorkIdentityV1,
        }

        impl<'work> $work<'work> {
            #[cfg(test)]
            fn for_test_v1(
                identity: RemoteWorkIdentityV1,
                retained_input_bytes: u64,
                expected_output_bytes: u64,
                actual_output_bytes: u64,
                run: impl FnOnce() + 'static,
            ) -> Self {
                Self(SealedCpuWorkV1 {
                    identity,
                    input_bytes: retained_input_bytes,
                    expected_output_bytes,
                    predecessor: None,
                    execution: DisarmedCpuExecutionV1 {
                        run: Box::new(move || {
                            run();
                            let output_len = usize::try_from(actual_output_bytes)
                                .map_err(|_overflow| RemoteCpuExecutionErrorV1::LayoutOverflow)?;
                            let mut backing = Vec::new();
                            backing
                                .try_reserve_exact(output_len)
                                .map_err(|_allocation| {
                                    RemoteCpuExecutionErrorV1::AllocationFailed
                                })?;
                            backing.resize(output_len, 0);
                            Ok(RemoteCpuExecutionOutputV1 {
                                owner: Box::new(RemoteByteOutputV1(backing.into_boxed_slice())),
                                successor: None,
                            })
                        }),
                    },
                })
            }

            #[cfg(rerun_mcap_phase_a_proof_v1)]
            fn for_phase_a_measurement_v1(
                identity: RemoteWorkIdentityV1,
                retained_input_bytes: u64,
                expected_output_bytes: u64,
                run: impl FnOnce()
                    -> Result<RemoteCpuExecutionOutputV1<'work>, RemoteCpuExecutionErrorV1>
                + 'work,
            ) -> Self {
                Self(SealedCpuWorkV1 {
                    identity,
                    input_bytes: retained_input_bytes,
                    expected_output_bytes,
                    predecessor: None,
                    execution: DisarmedCpuExecutionV1 { run: Box::new(run) },
                })
            }

            fn execute_reserved_v1(
                self,
                input_reservation: RemoteCompletionInputReservationV1,
                output_reservation: RemoteReadyOutputReservationV1,
            ) -> Result<($result<'work>, u64), RemoteCpuExecutionErrorV1> {
                #[cfg(not(any(test, rerun_mcap_phase_a_proof_v1)))]
                {
                    let _input_reservation = input_reservation;
                    let _output_reservation = output_reservation;
                    match self.0.capability {}
                }

                #[cfg(any(test, rerun_mcap_phase_a_proof_v1))]
                {
                    if self.0.predecessor.is_some_and(|predecessor| {
                        !self.0.identity.is_direct_phase_successor_of_v1(predecessor)
                    }) {
                        return Err(RemoteCpuExecutionErrorV1::BoundViolation);
                    }
                    let DisarmedCpuExecutionV1 { run } = self.0.execution;
                    let output = run()?;
                    let actual_output_bytes = output.retained_bytes_v1();
                    if actual_output_bytes > self.0.expected_output_bytes
                        || actual_output_bytes > output_reservation.bytes
                    {
                        return Err(RemoteCpuExecutionErrorV1::BoundViolation);
                    }
                    Ok((
                        $result {
                            output,
                            input_reservation,
                            output_reservation,
                            identity: self.0.identity,
                        },
                        self.0.input_bytes,
                    ))
                }
            }
        }
    };
}

declare_cpu_phase!(RemoteOpeningParseWorkV1, RemoteOpeningParseResultV1);
declare_cpu_phase!(
    RemoteMessageIndexParseWorkV1,
    RemoteMessageIndexParseResultV1
);
declare_cpu_phase!(
    RemotePhysicalChunkValidationWorkV1,
    RemotePhysicalChunkValidationResultV1
);
declare_cpu_phase!(
    RemotePhysicalChunkDispatchDecodeWorkV1,
    RemotePhysicalChunkDispatchDecodeResultV1
);

pub(crate) enum RemoteCpuWorkV1<'work> {
    OpeningParse(RemoteOpeningParseWorkV1<'work>),
    MessageIndexParse(RemoteMessageIndexParseWorkV1<'work>),
    PhysicalChunkValidation(RemotePhysicalChunkValidationWorkV1<'work>),
    PhysicalChunkDispatchDecode(RemotePhysicalChunkDispatchDecodeWorkV1<'work>),
}

impl<'work> RemoteCpuWorkV1<'work> {
    fn core_v1(&self) -> &SealedCpuWorkV1<'work> {
        match self {
            Self::OpeningParse(work) => &work.0,
            Self::MessageIndexParse(work) => &work.0,
            Self::PhysicalChunkValidation(work) => &work.0,
            Self::PhysicalChunkDispatchDecode(work) => &work.0,
        }
    }

    fn kind_v1(&self) -> RemoteCpuWorkKindV1 {
        match self {
            Self::OpeningParse(_) => RemoteCpuWorkKindV1::OpeningParse,
            Self::MessageIndexParse(_) => RemoteCpuWorkKindV1::MessageIndexParse,
            Self::PhysicalChunkValidation(_) => RemoteCpuWorkKindV1::PhysicalChunkValidation,
            Self::PhysicalChunkDispatchDecode(_) => {
                RemoteCpuWorkKindV1::PhysicalChunkDispatchDecode
            }
        }
    }

    fn execute_reserved_v1(
        self,
        input_reservation: RemoteCompletionInputReservationV1,
        reservation: RemoteReadyOutputReservationV1,
    ) -> Result<(RemoteUnpublishedCpuResultV1<'work>, u64), RemoteCpuExecutionErrorV1> {
        match self {
            Self::OpeningParse(work) => work
                .execute_reserved_v1(input_reservation, reservation)
                .map(|(result, input)| (RemoteUnpublishedCpuResultV1::OpeningParse(result), input)),
            Self::MessageIndexParse(work) => work
                .execute_reserved_v1(input_reservation, reservation)
                .map(|(result, input)| {
                    (
                        RemoteUnpublishedCpuResultV1::MessageIndexParse(result),
                        input,
                    )
                }),
            Self::PhysicalChunkValidation(work) => work
                .execute_reserved_v1(input_reservation, reservation)
                .map(|(result, input)| {
                    (
                        RemoteUnpublishedCpuResultV1::PhysicalChunkValidation(result),
                        input,
                    )
                }),
            Self::PhysicalChunkDispatchDecode(work) => work
                .execute_reserved_v1(input_reservation, reservation)
                .map(|(result, input)| {
                    (
                        RemoteUnpublishedCpuResultV1::PhysicalChunkDispatchDecode(result),
                        input,
                    )
                }),
        }
    }
}

pub(crate) enum RemoteUnpublishedCpuResultV1<'work> {
    OpeningParse(RemoteOpeningParseResultV1<'work>),
    MessageIndexParse(RemoteMessageIndexParseResultV1<'work>),
    PhysicalChunkValidation(RemotePhysicalChunkValidationResultV1<'work>),
    PhysicalChunkDispatchDecode(RemotePhysicalChunkDispatchDecodeResultV1<'work>),
}

/// Move-only validation authority for one exact follow-up dispatch phase.
///
/// The successor closure owns the real lower-stage validation owner.
/// Consequently, that owner first enters the validation ready queue and remains live until this
/// token is consumed into the distinct dispatch work item.
struct RemoteCpuPhaseTokenV1<'work> {
    predecessor: RemoteWorkIdentityV1,
    successor_identity: RemoteWorkIdentityV1,
    successor: RemoteCpuSuccessorV1<'work>,
}

impl RemoteUnpublishedCpuResultV1<'_> {
    fn identity_v1(&self) -> RemoteWorkIdentityV1 {
        match self {
            Self::OpeningParse(result) => result.identity,
            Self::MessageIndexParse(result) => result.identity,
            Self::PhysicalChunkValidation(result) => result.identity,
            Self::PhysicalChunkDispatchDecode(result) => result.identity,
        }
    }

    fn output_bytes_v1(&self) -> u64 {
        match self {
            Self::OpeningParse(result) => result.output.retained_bytes_v1(),
            Self::MessageIndexParse(result) => result.output.retained_bytes_v1(),
            Self::PhysicalChunkValidation(result) => result.output.retained_bytes_v1(),
            Self::PhysicalChunkDispatchDecode(result) => result.output.retained_bytes_v1(),
        }
    }
}

impl<'work> RemoteUnpublishedCpuResultV1<'work> {
    #[expect(
        clippy::result_large_err,
        reason = "failure must return the original move-only ready owner without allocation"
    )]
    fn into_physical_dispatch_token_v1(
        self,
        successor_operation: RemoteOperationIdentityV1,
    ) -> Result<RemoteCpuPhaseTokenV1<'work>, Self> {
        let Self::PhysicalChunkValidation(result) = self else {
            return Err(self);
        };
        let RemotePhysicalChunkValidationResultV1 {
            mut output,
            input_reservation,
            output_reservation,
            identity,
        } = result;
        let Some(successor) = output.successor.take() else {
            return Err(Self::PhysicalChunkValidation(
                RemotePhysicalChunkValidationResultV1 {
                    output,
                    input_reservation,
                    output_reservation,
                    identity,
                },
            ));
        };
        let Some(phase_generation) = identity.phase_generation.checked_add(1) else {
            return Err(Self::PhysicalChunkValidation(
                RemotePhysicalChunkValidationResultV1 {
                    output,
                    input_reservation,
                    output_reservation,
                    identity,
                },
            ));
        };
        let successor_identity = RemoteWorkIdentityV1 {
            operation: successor_operation,
            phase_generation,
            ..identity
        };
        drop(output);
        drop(input_reservation);
        drop(output_reservation);
        Ok(RemoteCpuPhaseTokenV1 {
            predecessor: identity,
            successor_identity,
            successor,
        })
    }
}

#[cfg(not(test))]
enum ProductionDisarmedRetryCapabilityV1 {}

pub(crate) struct RemoteRetryPendingV1 {
    driver: RemoteDriverIdentityV1,
    completed_operation: RemoteOperationIdentityV1,
    next_operation: RemoteOperationIdentityV1,
    eligible_frame: u64,
    #[cfg(not(test))]
    capability: ProductionDisarmedRetryCapabilityV1,
}

impl RemoteRetryPendingV1 {
    #[cfg(test)]
    fn for_test_v1(completed_work: RemoteWorkIdentityV1, callback_frame: u64) -> Option<Self> {
        Some(Self {
            driver: completed_work.driver,
            completed_operation: completed_work.operation,
            next_operation: completed_work.operation.next_attempt_v1()?,
            eligible_frame: callback_frame.checked_add(1)?,
        })
    }
}

/// Move-only exactly-once authority returned after the driver has already burned the attempt.
pub(crate) struct RemoteRetryAttemptV1 {
    driver: RemoteDriverIdentityV1,
    operation: RemoteOperationIdentityV1,
    #[cfg(not(test))]
    capability: ProductionDisarmedRetryCapabilityV1,
}

impl RemoteRetryAttemptV1 {
    pub(crate) const fn driver_identity_v1(&self) -> RemoteDriverIdentityV1 {
        self.driver
    }

    pub(crate) const fn operation_identity_v1(&self) -> RemoteOperationIdentityV1 {
        self.operation
    }
}

pub(crate) enum RemoteFetchCompletionV1<'work> {
    Cpu(RemoteCpuWorkV1<'work>),
    RetryPending(RemoteRetryPendingV1),
}

impl RemoteFetchCompletionV1<'_> {
    fn driver_identity_v1(&self) -> RemoteDriverIdentityV1 {
        match self {
            Self::Cpu(work) => work.core_v1().identity.driver,
            Self::RetryPending(retry) => retry.driver,
        }
    }
}

enum QueuedFetchCompletionV1<'work> {
    Cpu {
        work: RemoteCpuWorkV1<'work>,
        input_reservation: RemoteCompletionInputReservationV1,
    },
    RetryPending(RemoteRetryPendingV1),
}

impl QueuedFetchCompletionV1<'_> {
    fn driver_identity_v1(&self) -> RemoteDriverIdentityV1 {
        match self {
            Self::Cpu { work, .. } => work.core_v1().identity.driver,
            Self::RetryPending(retry) => retry.driver,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteCpuDriverLimitsV1 {
    max_completion_items: usize,
    max_completion_retained_bytes: u64,
    max_ready_items: usize,
    max_ready_retained_bytes: u64,
}

pub(crate) enum RemoteFetchCompletionEnqueueErrorV1<'work> {
    CountLimit(RemoteFetchCompletionV1<'work>),
    RetainedBytesLimit(RemoteFetchCompletionV1<'work>),
    ArithmeticOverflow(RemoteFetchCompletionV1<'work>),
    StaleDriver(RemoteFetchCompletionV1<'work>),
    DuplicateWork(RemoteFetchCompletionV1<'work>),
    DuplicateSettlement(RemoteFetchCompletionV1<'work>),
}

impl std::fmt::Debug for RemoteFetchCompletionEnqueueErrorV1<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::CountLimit(_) => "CountLimit(..)",
            Self::RetainedBytesLimit(_) => "RetainedBytesLimit(..)",
            Self::ArithmeticOverflow(_) => "ArithmeticOverflow(..)",
            Self::StaleDriver(_) => "StaleDriver(..)",
            Self::DuplicateWork(_) => "DuplicateWork(..)",
            Self::DuplicateSettlement(_) => "DuplicateSettlement(..)",
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RemoteCpuWorkMetricsV1 {
    completed_count: u64,
    input_bytes: u64,
    output_bytes: u64,
    max_duration_micros: u64,
    overflowed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteCpuMetricsSnapshotV1 {
    by_kind: [RemoteCpuWorkMetricsV1; RemoteCpuWorkKindV1::COUNT],
}

impl RemoteCpuMetricsSnapshotV1 {
    fn get_v1(&self, kind: RemoteCpuWorkKindV1) -> RemoteCpuWorkMetricsV1 {
        self.by_kind[kind.index()]
    }
}

pub(crate) enum RemoteCpuDriveOutcomeV1 {
    Idle,
    FrameAllowanceExhausted,
    RetryNotYetEligible,
    DroppedStale,
    RetryAttemptReady(RemoteRetryAttemptV1),
    ReadyReservationLimit {
        kind: RemoteCpuWorkKindV1,
    },
    WorkFailed {
        kind: RemoteCpuWorkKindV1,
        error: RemoteCpuExecutionErrorV1,
    },
    Completed {
        kind: RemoteCpuWorkKindV1,
        output_bytes: u64,
    },
}

pub(crate) struct RemoteMcapCpuDriverV1<'work> {
    identity: RemoteDriverIdentityV1,
    limits: RemoteCpuDriverLimitsV1,
    operations: BTreeMap<u64, RemoteOperationStateV1>,
    registered_work: BTreeSet<RemoteWorkIdentityV1>,
    completed_work: BTreeSet<RemoteWorkIdentityV1>,
    completions: VecDeque<QueuedFetchCompletionV1<'work>>,
    input_budget: Arc<RemoteCompletionInputBudgetStateV1>,
    ready: VecDeque<RemoteUnpublishedCpuResultV1<'work>>,
    ready_budget: Arc<RemoteReadyOutputBudgetStateV1>,
    last_drive_frame: Option<u64>,
    metrics: [RemoteCpuWorkMetricsV1; RemoteCpuWorkKindV1::COUNT],
}

impl<'work> RemoteMcapCpuDriverV1<'work> {
    fn new_v1(identity: RemoteDriverIdentityV1, limits: RemoteCpuDriverLimitsV1) -> Self {
        Self {
            identity,
            limits,
            operations: BTreeMap::new(),
            registered_work: BTreeSet::new(),
            completed_work: BTreeSet::new(),
            completions: VecDeque::new(),
            input_budget: Arc::new(RemoteCompletionInputBudgetStateV1 {
                max_items: limits.max_completion_items,
                max_bytes: limits.max_completion_retained_bytes,
                usage: Mutex::new(RemoteCompletionInputBudgetUsageV1::default()),
            }),
            ready: VecDeque::new(),
            ready_budget: Arc::new(RemoteReadyOutputBudgetStateV1 {
                max_items: limits.max_ready_items,
                max_bytes: limits.max_ready_retained_bytes,
                usage: Mutex::new(RemoteReadyOutputBudgetUsageV1::default()),
            }),
            last_drive_frame: None,
            metrics: [RemoteCpuWorkMetricsV1::default(); RemoteCpuWorkKindV1::COUNT],
        }
    }

    fn register_in_flight_operation_v1(
        &mut self,
        operation: RemoteOperationIdentityV1,
    ) -> Result<(), ()> {
        match self.operations.get(&operation.operation_nonce) {
            None => {
                self.operations.insert(
                    operation.operation_nonce,
                    RemoteOperationStateV1 {
                        attempt_generation: operation.attempt_generation,
                        settlement: RemoteOperationSettlementV1::InFlight,
                    },
                );
                Ok(())
            }
            Some(state)
                if state.attempt_generation == operation.attempt_generation
                    && state.settlement == RemoteOperationSettlementV1::InFlight =>
            {
                Ok(())
            }
            Some(_) => Err(()),
        }
    }

    /// Fetch callbacks may only enqueue sealed ownership and request a repaint.
    #[expect(
        clippy::result_large_err,
        reason = "preflight rejection must return the original move-only completion without allocation"
    )]
    fn enqueue_from_fetch_callback_v1(
        &mut self,
        completion: RemoteFetchCompletionV1<'work>,
        request_repaint: impl FnOnce(),
    ) -> Result<(), RemoteFetchCompletionEnqueueErrorV1<'work>> {
        if completion.driver_identity_v1() != self.identity {
            return Err(RemoteFetchCompletionEnqueueErrorV1::StaleDriver(completion));
        }
        if self.completions.len() >= self.limits.max_completion_items {
            return Err(RemoteFetchCompletionEnqueueErrorV1::CountLimit(completion));
        }
        let queued = match completion {
            RemoteFetchCompletionV1::Cpu(work) => {
                let identity = work.core_v1().identity;
                if self.registered_work.contains(&identity)
                    || self.completed_work.contains(&identity)
                {
                    return Err(RemoteFetchCompletionEnqueueErrorV1::DuplicateWork(
                        RemoteFetchCompletionV1::Cpu(work),
                    ));
                }
                let Some(operation) = self.operations.get(&identity.operation.operation_nonce)
                else {
                    return Err(RemoteFetchCompletionEnqueueErrorV1::StaleDriver(
                        RemoteFetchCompletionV1::Cpu(work),
                    ));
                };
                if operation.attempt_generation != identity.operation.attempt_generation {
                    return Err(RemoteFetchCompletionEnqueueErrorV1::StaleDriver(
                        RemoteFetchCompletionV1::Cpu(work),
                    ));
                }
                if operation.settlement != RemoteOperationSettlementV1::InFlight {
                    return Err(RemoteFetchCompletionEnqueueErrorV1::DuplicateSettlement(
                        RemoteFetchCompletionV1::Cpu(work),
                    ));
                }
                let retained_input_bytes = work.core_v1().input_bytes;
                let mut usage = self.input_budget.usage.lock();
                let Some(items) = usage.items.checked_add(1) else {
                    return Err(RemoteFetchCompletionEnqueueErrorV1::ArithmeticOverflow(
                        RemoteFetchCompletionV1::Cpu(work),
                    ));
                };
                let Some(bytes) = usage.bytes.checked_add(retained_input_bytes) else {
                    return Err(RemoteFetchCompletionEnqueueErrorV1::ArithmeticOverflow(
                        RemoteFetchCompletionV1::Cpu(work),
                    ));
                };
                if items > self.input_budget.max_items {
                    return Err(RemoteFetchCompletionEnqueueErrorV1::CountLimit(
                        RemoteFetchCompletionV1::Cpu(work),
                    ));
                }
                if bytes > self.input_budget.max_bytes {
                    return Err(RemoteFetchCompletionEnqueueErrorV1::RetainedBytesLimit(
                        RemoteFetchCompletionV1::Cpu(work),
                    ));
                }
                usage.items = items;
                usage.bytes = bytes;
                drop(usage);
                self.operations
                    .get_mut(&identity.operation.operation_nonce)
                    .expect("validated operation remains installed")
                    .settlement = RemoteOperationSettlementV1::Succeeded;
                self.registered_work.insert(identity);
                QueuedFetchCompletionV1::Cpu {
                    work,
                    input_reservation: RemoteCompletionInputReservationV1 {
                        state: Arc::clone(&self.input_budget),
                        bytes: retained_input_bytes,
                    },
                }
            }
            RemoteFetchCompletionV1::RetryPending(retry) => {
                let Some(operation) = self
                    .operations
                    .get_mut(&retry.completed_operation.operation_nonce)
                else {
                    return Err(RemoteFetchCompletionEnqueueErrorV1::StaleDriver(
                        RemoteFetchCompletionV1::RetryPending(retry),
                    ));
                };
                if operation.attempt_generation != retry.completed_operation.attempt_generation {
                    return Err(RemoteFetchCompletionEnqueueErrorV1::StaleDriver(
                        RemoteFetchCompletionV1::RetryPending(retry),
                    ));
                }
                if operation.settlement != RemoteOperationSettlementV1::InFlight {
                    return Err(RemoteFetchCompletionEnqueueErrorV1::DuplicateSettlement(
                        RemoteFetchCompletionV1::RetryPending(retry),
                    ));
                }
                operation.settlement = RemoteOperationSettlementV1::RetryPending {
                    next_attempt_generation: retry.next_operation.attempt_generation,
                };
                QueuedFetchCompletionV1::RetryPending(retry)
            }
        };
        self.completions.push_back(queued);
        request_repaint();
        Ok(())
    }

    fn rebind_activity_v1(&mut self, identity: RemoteDriverIdentityV1) {
        self.identity = identity;
        self.collect_stale_backlogs_v1();
    }

    fn collect_stale_backlogs_v1(&mut self) {
        self.completions
            .retain(|completion| completion.driver_identity_v1() == self.identity);
        self.ready
            .retain(|result| result.identity_v1().driver == self.identity);
        self.registered_work
            .retain(|identity| identity.driver == self.identity);
        self.completed_work
            .retain(|identity| identity.driver == self.identity);
        if self
            .registered_work
            .iter()
            .all(|identity| identity.driver != self.identity)
        {
            self.operations.clear();
        }
    }

    fn reserve_ready_output_v1(&self, bytes: u64) -> Option<RemoteReadyOutputReservationV1> {
        let mut usage = self.ready_budget.usage.lock();
        let items = usage.items.checked_add(1)?;
        let retained = usage.bytes.checked_add(bytes)?;
        if items > self.ready_budget.max_items || retained > self.ready_budget.max_bytes {
            return None;
        }
        usage.items = items;
        usage.bytes = retained;
        drop(usage);
        Some(RemoteReadyOutputReservationV1 {
            state: Arc::clone(&self.ready_budget),
            bytes,
        })
    }

    fn drive_cpu_v1(&mut self, frame_sequence: u64) -> RemoteCpuDriveOutcomeV1 {
        if self.last_drive_frame == Some(frame_sequence) {
            return RemoteCpuDriveOutcomeV1::FrameAllowanceExhausted;
        }
        self.last_drive_frame = Some(frame_sequence);
        let Some(completion) = self.completions.pop_front() else {
            return RemoteCpuDriveOutcomeV1::Idle;
        };
        if completion.driver_identity_v1() != self.identity {
            return RemoteCpuDriveOutcomeV1::DroppedStale;
        }

        match completion {
            QueuedFetchCompletionV1::RetryPending(retry) => {
                if frame_sequence < retry.eligible_frame {
                    self.completions
                        .push_front(QueuedFetchCompletionV1::RetryPending(retry));
                    return RemoteCpuDriveOutcomeV1::RetryNotYetEligible;
                }
                if retry.driver != self.identity {
                    return RemoteCpuDriveOutcomeV1::DroppedStale;
                }

                let Some(operation) = self
                    .operations
                    .get_mut(&retry.completed_operation.operation_nonce)
                else {
                    return RemoteCpuDriveOutcomeV1::DroppedStale;
                };
                if operation.attempt_generation != retry.completed_operation.attempt_generation
                    || operation.settlement
                        != (RemoteOperationSettlementV1::RetryPending {
                            next_attempt_generation: retry.next_operation.attempt_generation,
                        })
                {
                    return RemoteCpuDriveOutcomeV1::DroppedStale;
                }

                // Burning the matching attempt is one non-fallible mutation before authority
                // escape. A settled attempt never deletes ready output or unrelated ownership.
                operation.attempt_generation = retry.next_operation.attempt_generation;
                operation.settlement = RemoteOperationSettlementV1::InFlight;
                RemoteCpuDriveOutcomeV1::RetryAttemptReady(RemoteRetryAttemptV1 {
                    driver: retry.driver,
                    operation: retry.next_operation,
                    #[cfg(not(test))]
                    capability: retry.capability,
                })
            }
            QueuedFetchCompletionV1::Cpu {
                work,
                input_reservation,
            } => {
                let identity = work.core_v1().identity;
                let kind = work.kind_v1();
                if identity.driver != self.identity || !self.registered_work.contains(&identity) {
                    self.registered_work.remove(&identity);
                    return RemoteCpuDriveOutcomeV1::DroppedStale;
                }
                let Some(operation) = self.operations.get(&identity.operation.operation_nonce)
                else {
                    self.registered_work.remove(&identity);
                    return RemoteCpuDriveOutcomeV1::DroppedStale;
                };
                if operation.attempt_generation != identity.operation.attempt_generation
                    || operation.settlement != RemoteOperationSettlementV1::Succeeded
                {
                    self.registered_work.remove(&identity);
                    return RemoteCpuDriveOutcomeV1::DroppedStale;
                }
                let Some(output_reservation) =
                    self.reserve_ready_output_v1(work.core_v1().expected_output_bytes)
                else {
                    self.completions.push_front(QueuedFetchCompletionV1::Cpu {
                        work,
                        input_reservation,
                    });
                    return RemoteCpuDriveOutcomeV1::ReadyReservationLimit { kind };
                };
                let reserved = ReservedCpuWorkV1 {
                    work,
                    input_reservation,
                    output_reservation,
                };
                let started = web_time::Instant::now();
                let result = reserved
                    .work
                    .execute_reserved_v1(reserved.input_reservation, reserved.output_reservation);
                let duration_micros = u64::try_from(started.elapsed().as_micros()).ok();
                let (result, input_bytes) = match result {
                    Ok(result) => result,
                    Err(error) => {
                        self.registered_work.remove(&identity);
                        return RemoteCpuDriveOutcomeV1::WorkFailed { kind, error };
                    }
                };
                let output_bytes = result.output_bytes_v1();
                self.record_metrics_v1(kind, input_bytes, output_bytes, duration_micros);

                // The exact per-work registry predicate is checked again after CPU execution and
                // before the unpublished result becomes visible to the ready consumer.
                if result.identity_v1() != identity
                    || identity.driver != self.identity
                    || !self.registered_work.remove(&identity)
                {
                    return RemoteCpuDriveOutcomeV1::DroppedStale;
                }
                let Some(operation) = self.operations.get(&identity.operation.operation_nonce)
                else {
                    return RemoteCpuDriveOutcomeV1::DroppedStale;
                };
                if operation.attempt_generation != identity.operation.attempt_generation
                    || operation.settlement != RemoteOperationSettlementV1::Succeeded
                    || !self.completed_work.insert(identity)
                {
                    return RemoteCpuDriveOutcomeV1::DroppedStale;
                }
                self.ready.push_back(result);
                RemoteCpuDriveOutcomeV1::Completed { kind, output_bytes }
            }
        }
    }

    fn record_metrics_v1(
        &mut self,
        kind: RemoteCpuWorkKindV1,
        input_bytes: u64,
        output_bytes: u64,
        duration_micros: Option<u64>,
    ) {
        let metrics = &mut self.metrics[kind.index()];
        let (count, count_overflow) = metrics.completed_count.overflowing_add(1);
        let (input, input_overflow) = metrics.input_bytes.overflowing_add(input_bytes);
        let (output, output_overflow) = metrics.output_bytes.overflowing_add(output_bytes);
        metrics.completed_count = if count_overflow { u64::MAX } else { count };
        metrics.input_bytes = if input_overflow { u64::MAX } else { input };
        metrics.output_bytes = if output_overflow { u64::MAX } else { output };
        if let Some(duration) = duration_micros {
            metrics.max_duration_micros = metrics.max_duration_micros.max(duration);
        }
        metrics.overflowed |=
            count_overflow || input_overflow || output_overflow || duration_micros.is_none();
    }

    fn pop_ready_v1(&mut self) -> Option<RemoteUnpublishedCpuResultV1<'work>> {
        self.ready.pop_front()
    }

    fn metrics_v1(&self) -> RemoteCpuMetricsSnapshotV1 {
        RemoteCpuMetricsSnapshotV1 {
            by_kind: self.metrics,
        }
    }

    #[cfg(test)]
    fn usage_for_test_v1(&self) -> ((usize, u64), (usize, u64), usize) {
        let input = self.input_budget.usage.lock();
        let ready = self.ready_budget.usage.lock();
        (
            (input.items, input.bytes),
            (ready.items, ready.bytes),
            self.registered_work.len(),
        )
    }
}

#[cfg(rerun_mcap_phase_a_proof_v1)]
struct PhaseAActualOutputOwnerV1<Owner> {
    owner: Owner,
    retained_bytes: u64,
}

#[cfg(rerun_mcap_phase_a_proof_v1)]
struct PhaseARetainedByteDeclarationV1(u64);

#[cfg(rerun_mcap_phase_a_proof_v1)]
impl RemoteCpuRetainedOutputV1 for PhaseARetainedByteDeclarationV1 {
    fn retained_bytes_v1(&self) -> u64 {
        self.0
    }
}

#[cfg(rerun_mcap_phase_a_proof_v1)]
impl<Owner> RemoteCpuRetainedOutputV1 for PhaseAActualOutputOwnerV1<Owner> {
    fn retained_bytes_v1(&self) -> u64 {
        let _keep_owner_live = &self.owner;
        self.retained_bytes
    }
}

#[cfg(rerun_mcap_phase_a_proof_v1)]
pub(crate) fn drive_phase_a_measurement_work_v1<'work, Owner>(
    kind: RemoteCpuWorkKindV1,
    retained_input_bytes: u64,
    expected_output_bytes: u64,
    run: impl FnOnce() -> Result<
        (
            re_mcap::phase_a_measurement::PhaseAMeasurementOutputV1,
            Owner,
        ),
        RemoteCpuExecutionErrorV1,
    > + 'work,
) -> Result<PhaseADriverMeasurementV1, RemoteCpuExecutionErrorV1>
where
    Owner: 'work,
{
    use std::cell::RefCell;
    use std::rc::Rc;

    let identity = RemoteWorkIdentityV1::for_phase_a_measurement_v1(retained_input_bytes, kind);
    let result = Rc::new(RefCell::new(None));
    let result_for_work = Rc::clone(&result);
    let execute = move || {
        let (output, owner) = run()?;
        let output_bytes = output.output_bytes;
        *result_for_work.borrow_mut() = Some(output);
        Ok(RemoteCpuExecutionOutputV1 {
            owner: Box::new(PhaseAActualOutputOwnerV1 {
                owner,
                retained_bytes: output_bytes,
            }),
            successor: None,
        })
    };
    let work = match kind {
        RemoteCpuWorkKindV1::OpeningParse => {
            RemoteCpuWorkV1::OpeningParse(RemoteOpeningParseWorkV1::for_phase_a_measurement_v1(
                identity,
                retained_input_bytes,
                expected_output_bytes,
                execute,
            ))
        }
        RemoteCpuWorkKindV1::MessageIndexParse => RemoteCpuWorkV1::MessageIndexParse(
            RemoteMessageIndexParseWorkV1::for_phase_a_measurement_v1(
                identity,
                retained_input_bytes,
                expected_output_bytes,
                execute,
            ),
        ),
        RemoteCpuWorkKindV1::PhysicalChunkValidation => RemoteCpuWorkV1::PhysicalChunkValidation(
            RemotePhysicalChunkValidationWorkV1::for_phase_a_measurement_v1(
                identity,
                retained_input_bytes,
                expected_output_bytes,
                execute,
            ),
        ),
        RemoteCpuWorkKindV1::PhysicalChunkDispatchDecode => {
            RemoteCpuWorkV1::PhysicalChunkDispatchDecode(
                RemotePhysicalChunkDispatchDecodeWorkV1::for_phase_a_measurement_v1(
                    identity,
                    retained_input_bytes,
                    expected_output_bytes,
                    execute,
                ),
            )
        }
    };
    let limits = RemoteCpuDriverLimitsV1 {
        max_completion_items: 1,
        max_completion_retained_bytes: retained_input_bytes,
        max_ready_items: 1,
        max_ready_retained_bytes: expected_output_bytes,
    };
    let mut driver = RemoteMcapCpuDriverV1::new_v1(identity.driver, limits);
    driver
        .register_in_flight_operation_v1(identity.operation)
        .map_err(|()| RemoteCpuExecutionErrorV1::BoundViolation)?;
    driver
        .enqueue_from_fetch_callback_v1(RemoteFetchCompletionV1::Cpu(work), || {})
        .map_err(|_error| RemoteCpuExecutionErrorV1::BoundViolation)?;
    match driver.drive_cpu_v1(1) {
        RemoteCpuDriveOutcomeV1::Completed { .. } => {}
        RemoteCpuDriveOutcomeV1::WorkFailed { error, .. } => return Err(error),
        _ => return Err(RemoteCpuExecutionErrorV1::BoundViolation),
    }
    drop(driver.pop_ready_v1());
    let metrics = driver.metrics_v1().get_v1(kind);
    let pipeline = result
        .borrow_mut()
        .take()
        .ok_or(RemoteCpuExecutionErrorV1::BoundViolation)?;
    Ok(PhaseADriverMeasurementV1 {
        pipeline,
        max_duration_micros: metrics.max_duration_micros.max(1),
        overflowed: metrics.overflowed,
    })
}

/// Runs physical validation and dispatch as two exact, ordered CPU turns.
///
/// The validation result owns the lower validation owner through its move-only successor closure.
/// Only consuming that ready result yields the token from which the dispatch work can be built.
#[cfg(rerun_mcap_phase_a_proof_v1)]
pub(crate) fn drive_phase_a_validation_dispatch_measurement_v1<
    'work,
    ValidationOwner,
    DispatchOwner,
>(
    retained_input_bytes: u64,
    validation_expected_output_bytes: u64,
    dispatch_expected_output_bytes: u64,
    validate: impl FnOnce() -> Result<
        (
            re_mcap::phase_a_measurement::PhaseAMeasurementOutputV1,
            ValidationOwner,
        ),
        RemoteCpuExecutionErrorV1,
    > + 'work,
    dispatch: impl FnOnce(
        ValidationOwner,
    ) -> Result<
        (
            re_mcap::phase_a_measurement::PhaseAMeasurementOutputV1,
            DispatchOwner,
        ),
        RemoteCpuExecutionErrorV1,
    > + 'work,
) -> Result<PhaseADriverMeasurementV1, RemoteCpuExecutionErrorV1>
where
    ValidationOwner: 'work,
    DispatchOwner: 'work,
{
    use std::cell::RefCell;
    use std::rc::Rc;

    let validation_identity = RemoteWorkIdentityV1::for_phase_a_measurement_v1(
        retained_input_bytes,
        RemoteCpuWorkKindV1::PhysicalChunkValidation,
    );
    let dispatch_operation = RemoteOperationIdentityV1 {
        operation_nonce: RemoteCpuWorkKindV1::PhysicalChunkDispatchDecode.index() as u64 + 1,
        attempt_generation: 1,
    };
    let dispatch_result = Rc::new(RefCell::new(None));
    let dispatch_result_for_work = Rc::clone(&dispatch_result);
    let validation_work = RemoteCpuWorkV1::PhysicalChunkValidation(
        RemotePhysicalChunkValidationWorkV1::for_phase_a_measurement_v1(
            validation_identity,
            retained_input_bytes,
            validation_expected_output_bytes,
            move || {
                let (validation_measurement, validation_owner) = validate()?;
                let validation_output_bytes = validation_measurement.output_bytes;
                let successor = move || {
                    let (measurement, owner) = dispatch(validation_owner)?;
                    let output_bytes = measurement.output_bytes;
                    *dispatch_result_for_work.borrow_mut() = Some(measurement);
                    Ok(RemoteCpuExecutionOutputV1 {
                        owner: Box::new(PhaseAActualOutputOwnerV1 {
                            owner,
                            retained_bytes: output_bytes,
                        }),
                        successor: None,
                    })
                };
                Ok(RemoteCpuExecutionOutputV1 {
                    // The actual validation owner is retained by `successor`, which is part of
                    // this ready result. This owner accounts the lower stage's declared output.
                    owner: Box::new(PhaseARetainedByteDeclarationV1(validation_output_bytes)),
                    successor: Some(Box::new(successor)),
                })
            },
        ),
    );
    let limits = RemoteCpuDriverLimitsV1 {
        max_completion_items: 1,
        max_completion_retained_bytes: retained_input_bytes,
        max_ready_items: 1,
        max_ready_retained_bytes: validation_expected_output_bytes
            .max(dispatch_expected_output_bytes),
    };
    let mut driver = RemoteMcapCpuDriverV1::new_v1(validation_identity.driver, limits);
    driver
        .register_in_flight_operation_v1(validation_identity.operation)
        .map_err(|()| RemoteCpuExecutionErrorV1::BoundViolation)?;
    driver
        .enqueue_from_fetch_callback_v1(RemoteFetchCompletionV1::Cpu(validation_work), || {})
        .map_err(|_error| RemoteCpuExecutionErrorV1::BoundViolation)?;
    match driver.drive_cpu_v1(1) {
        RemoteCpuDriveOutcomeV1::Completed { .. } => {}
        RemoteCpuDriveOutcomeV1::WorkFailed { error, .. } => return Err(error),
        _ => return Err(RemoteCpuExecutionErrorV1::BoundViolation),
    }
    let token = driver
        .pop_ready_v1()
        .ok_or(RemoteCpuExecutionErrorV1::BoundViolation)?
        .into_physical_dispatch_token_v1(dispatch_operation)
        .map_err(|_result| RemoteCpuExecutionErrorV1::BoundViolation)?;
    let RemoteCpuPhaseTokenV1 {
        predecessor,
        successor_identity,
        successor,
    } = token;
    let dispatch_work = RemoteCpuWorkV1::PhysicalChunkDispatchDecode(
        RemotePhysicalChunkDispatchDecodeWorkV1(SealedCpuWorkV1 {
            identity: successor_identity,
            input_bytes: retained_input_bytes,
            expected_output_bytes: dispatch_expected_output_bytes,
            predecessor: Some(predecessor),
            execution: DisarmedCpuExecutionV1 { run: successor },
        }),
    );
    driver
        .register_in_flight_operation_v1(dispatch_operation)
        .map_err(|()| RemoteCpuExecutionErrorV1::BoundViolation)?;
    driver
        .enqueue_from_fetch_callback_v1(RemoteFetchCompletionV1::Cpu(dispatch_work), || {})
        .map_err(|_error| RemoteCpuExecutionErrorV1::BoundViolation)?;
    match driver.drive_cpu_v1(2) {
        RemoteCpuDriveOutcomeV1::Completed { .. } => {}
        RemoteCpuDriveOutcomeV1::WorkFailed { error, .. } => return Err(error),
        _ => return Err(RemoteCpuExecutionErrorV1::BoundViolation),
    }
    drop(driver.pop_ready_v1());
    let metrics = driver
        .metrics_v1()
        .get_v1(RemoteCpuWorkKindV1::PhysicalChunkDispatchDecode);
    let pipeline = dispatch_result
        .borrow_mut()
        .take()
        .ok_or(RemoteCpuExecutionErrorV1::BoundViolation)?;
    Ok(PhaseADriverMeasurementV1 {
        pipeline,
        max_duration_micros: metrics.max_duration_micros.max(1),
        overflowed: metrics.overflowed,
    })
}

#[cfg(target_arch = "wasm32")]
pub(crate) struct WasmPhysicalFetchCompletionOwnerV1<'a> {
    // Field order is normative: transport backing drops before the pending lower lease.
    body: Option<re_web::chrome_byob::ExactLengthRangeBody>,
    pending: Option<re_mcap::web_body_handoff::WebPendingPhysicalChunkReadV1<'a>>,
    authority: RemoteRangeCompletionAuthorityV1,
}

#[cfg(target_arch = "wasm32")]
struct RemoteRangeAuthorityStateV1 {
    current: Cell<Option<RemoteWorkIdentityV1>>,
}

/// Registry-side authority which remains live while an attempt/completion may be accepted.
#[cfg(target_arch = "wasm32")]
pub(crate) struct RemoteRangeOperationGuardV1 {
    state: Rc<RemoteRangeAuthorityStateV1>,
}

/// Move-only authority owned by the Range operation until exactly one completion is accepted.
#[cfg(target_arch = "wasm32")]
pub(crate) struct RemoteRangeAttemptAuthorityV1 {
    state: Rc<RemoteRangeAuthorityStateV1>,
    identity: RemoteWorkIdentityV1,
}

/// Move-only completion authority consumed by the upper body owner.
#[cfg(target_arch = "wasm32")]
pub(crate) struct RemoteRangeCompletionAuthorityV1 {
    state: Rc<RemoteRangeAuthorityStateV1>,
    identity: RemoteWorkIdentityV1,
}

#[cfg(target_arch = "wasm32")]
impl RemoteRangeAttemptAuthorityV1 {
    #[cfg(rerun_mcap_phase_a_proof_v1)]
    fn for_phase_a_measurement_v1(
        identity: RemoteWorkIdentityV1,
    ) -> (RemoteRangeOperationGuardV1, Self) {
        let state = Rc::new(RemoteRangeAuthorityStateV1 {
            current: Cell::new(Some(identity)),
        });
        (
            RemoteRangeOperationGuardV1 {
                state: Rc::clone(&state),
            },
            Self { state, identity },
        )
    }

    fn settle_success_v1(self) -> Result<RemoteRangeCompletionAuthorityV1, ()> {
        if self.state.current.get() != Some(self.identity) {
            return Err(());
        }
        Ok(RemoteRangeCompletionAuthorityV1 {
            state: self.state,
            identity: self.identity,
        })
    }
}

#[cfg(target_arch = "wasm32")]
impl Drop for RemoteRangeOperationGuardV1 {
    fn drop(&mut self) {
        self.state.current.set(None);
    }
}

#[cfg(target_arch = "wasm32")]
impl RemoteRangeCompletionAuthorityV1 {
    fn identity_v1(&self) -> RemoteWorkIdentityV1 {
        self.identity
    }

    fn ensure_current_v1(&self) -> Result<(), ()> {
        (self.state.current.get() == Some(self.identity))
            .then_some(())
            .ok_or(())
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) struct WasmPhysicalBodyOwnerAdapterV1<'a> {
    owner: WasmPhysicalFetchCompletionOwnerV1<'a>,
    overlap_budget: re_mcap::web_body_handoff::WebPhysicalCopyOverlapBudgetV1,
}

#[cfg(target_arch = "wasm32")]
pub(crate) enum WasmPhysicalBodyAdapterErrorV1 {
    Bind(re_mcap::web_body_handoff::WebPhysicalCompletionBindErrorV1),
    Handoff(re_mcap::web_body_handoff::WebPhysicalBodyHandoffErrorV1),
}

#[cfg(target_arch = "wasm32")]
impl<'a> WasmPhysicalBodyOwnerAdapterV1<'a> {
    /// The unique upper adapter owns the transport body and opaque lower pending lease together.
    pub(crate) fn new_disarmed_v1(
        body: re_web::chrome_byob::ExactLengthRangeBody,
        pending: re_mcap::web_body_handoff::WebPendingPhysicalChunkReadV1<'a>,
        authority: RemoteRangeCompletionAuthorityV1,
    ) -> Result<Self, re_mcap::web_body_handoff::WebPhysicalCompletionBindErrorV1> {
        let identity = authority.identity_v1();
        let pending_identity = pending.identity_v1()?;
        let (range_start, range_end_exclusive) = pending_identity.full_range_v1();
        if identity.driver.source_generation != pending_identity.source_generation_v1()
            || identity.read_generation != pending_identity.read_generation_v1()
            || identity.canonical_ordinal != pending_identity.canonical_ordinal_v1()
            || identity.full_range
                != (RemoteByteRangeV1 {
                    start: range_start,
                    end_exclusive: range_end_exclusive,
                })
            || identity.budget_profile != RemoteBudgetProfileV1::UnfrozenPhaseACandidate
            || pending_identity.budget_profile_v1()
                != re_mcap::web_body_handoff::WebPhysicalBudgetProfileV1::UnfrozenPhaseACandidate
        {
            return Err(
                re_mcap::web_body_handoff::WebPhysicalCompletionBindErrorV1::CrossCombination,
            );
        }
        Ok(Self {
            owner: WasmPhysicalFetchCompletionOwnerV1 {
                body: Some(body),
                pending: Some(pending),
                authority,
            },
            overlap_budget:
                re_mcap::web_body_handoff::WebPhysicalCopyOverlapBudgetV1::new_unfrozen_phase_a_v1(),
        })
    }

    pub(crate) fn execute_v1(
        self,
    ) -> Result<
        re_mcap::web_body_handoff::WebPhysicalScanCacheEntryV1<'a>,
        WasmPhysicalBodyAdapterErrorV1,
    > {
        let WasmPhysicalFetchCompletionOwnerV1 {
            mut body,
            mut pending,
            authority,
        } = self.owner;
        let body = body
            .take()
            .expect("a live upper completion owner retains its exact body");
        let borrowed = pending
            .take()
            .expect("a live upper completion owner retains its pending lease")
            .bind_borrowed_exact_body_v1(
                body.as_slice(),
                re_mcap::web_body_handoff::WebPhysicalBudgetProfileV1::UnfrozenPhaseACandidate,
            )
            .map_err(WasmPhysicalBodyAdapterErrorV1::Bind)?;
        let completed = borrowed
            .process_explicit_copy_v1(&self.overlap_budget, |_safe_point| {
                authority.ensure_current_v1()
            })
            .map_err(WasmPhysicalBodyAdapterErrorV1::Handoff)?;
        drop(body);
        Ok(completed.into_cache_after_body_drop_v1())
    }
}

#[cfg(all(target_arch = "wasm32", rerun_mcap_phase_a_proof_v1))]
pub(crate) fn execute_phase_a_physical_body_adapter_v1<'a>(
    body: re_web::chrome_byob::ExactLengthRangeBody,
    pending: re_mcap::web_body_handoff::WebPendingPhysicalChunkReadV1<'a>,
) -> Result<
    re_mcap::web_body_handoff::WebPhysicalScanCacheEntryV1<'a>,
    WasmPhysicalBodyAdapterErrorV1,
> {
    let pending_identity = pending
        .identity_v1()
        .map_err(WasmPhysicalBodyAdapterErrorV1::Bind)?;
    let (start, end_exclusive) = pending_identity.full_range_v1();
    let identity = RemoteWorkIdentityV1 {
        driver: RemoteDriverIdentityV1 {
            source_generation: pending_identity.source_generation_v1(),
            activity_epoch: 1,
        },
        operation: RemoteOperationIdentityV1 {
            operation_nonce: 1,
            attempt_generation: 1,
        },
        read_generation: pending_identity.read_generation_v1(),
        canonical_ordinal: pending_identity.canonical_ordinal_v1(),
        full_range: RemoteByteRangeV1 {
            start,
            end_exclusive,
        },
        budget_profile: RemoteBudgetProfileV1::UnfrozenPhaseACandidate,
        phase_generation: 1,
    };
    let (_operation_guard, attempt) =
        RemoteRangeAttemptAuthorityV1::for_phase_a_measurement_v1(identity);
    let authority = attempt.settle_success_v1().map_err(|()| {
        WasmPhysicalBodyAdapterErrorV1::Bind(
            re_mcap::web_body_handoff::WebPhysicalCompletionBindErrorV1::StaleLease,
        )
    })?;
    WasmPhysicalBodyOwnerAdapterV1::new_disarmed_v1(body, pending, authority)?.execute_v1()
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use super::*;

    fn limits() -> RemoteCpuDriverLimitsV1 {
        RemoteCpuDriverLimitsV1 {
            max_completion_items: 8,
            max_completion_retained_bytes: 64,
            max_ready_items: 8,
            max_ready_retained_bytes: 64,
        }
    }

    fn work(
        identity: RemoteWorkIdentityV1,
        kind: RemoteCpuWorkKindV1,
        retained_input_bytes: u64,
        expected_output_bytes: u64,
        actual_output_bytes: u64,
        run: impl FnOnce() + 'static,
    ) -> RemoteFetchCompletionV1<'static> {
        let core = |constructor: fn(SealedCpuWorkV1<'static>) -> RemoteCpuWorkV1<'static>| {
            constructor(SealedCpuWorkV1 {
                identity,
                input_bytes: retained_input_bytes,
                expected_output_bytes,
                predecessor: None,
                execution: DisarmedCpuExecutionV1 {
                    run: Box::new(move || {
                        run();
                        let output_len = usize::try_from(actual_output_bytes)
                            .map_err(|_overflow| RemoteCpuExecutionErrorV1::LayoutOverflow)?;
                        let mut backing = Vec::new();
                        backing
                            .try_reserve_exact(output_len)
                            .map_err(|_allocation| RemoteCpuExecutionErrorV1::AllocationFailed)?;
                        backing.resize(output_len, 0);
                        Ok(RemoteCpuExecutionOutputV1 {
                            owner: Box::new(RemoteByteOutputV1(backing.into_boxed_slice())),
                            successor: None,
                        })
                    }),
                },
            })
        };
        let work = match kind {
            RemoteCpuWorkKindV1::OpeningParse => {
                core(|core| RemoteCpuWorkV1::OpeningParse(RemoteOpeningParseWorkV1(core)))
            }
            RemoteCpuWorkKindV1::MessageIndexParse => {
                core(|core| RemoteCpuWorkV1::MessageIndexParse(RemoteMessageIndexParseWorkV1(core)))
            }
            RemoteCpuWorkKindV1::PhysicalChunkValidation => core(|core| {
                RemoteCpuWorkV1::PhysicalChunkValidation(RemotePhysicalChunkValidationWorkV1(core))
            }),
            RemoteCpuWorkKindV1::PhysicalChunkDispatchDecode => core(|core| {
                RemoteCpuWorkV1::PhysicalChunkDispatchDecode(
                    RemotePhysicalChunkDispatchDecodeWorkV1(core),
                )
            }),
        };
        RemoteFetchCompletionV1::Cpu(work)
    }

    #[expect(
        clippy::result_large_err,
        reason = "the test helper preserves the production move-only rejection owner"
    )]
    fn enqueue_new(
        driver: &mut RemoteMcapCpuDriverV1<'static>,
        completion: RemoteFetchCompletionV1<'static>,
    ) -> Result<(), RemoteFetchCompletionEnqueueErrorV1<'static>> {
        let operation = match &completion {
            RemoteFetchCompletionV1::Cpu(work) => work.core_v1().identity.operation,
            RemoteFetchCompletionV1::RetryPending(retry) => retry.completed_operation,
        };
        driver
            .register_in_flight_operation_v1(operation)
            .expect("test operation is fresh");
        driver.enqueue_from_fetch_callback_v1(completion, || {})
    }

    #[test]
    fn multiple_ordinals_share_driver_and_execute_fifo_one_per_frame() {
        let first = RemoteWorkIdentityV1::for_test_v1(1, 1, 1);
        let second = RemoteWorkIdentityV1::for_test_v1(1, 1, 2);
        let mut driver = RemoteMcapCpuDriverV1::new_v1(first.driver, limits());
        let order = Rc::new(RefCell::new(Vec::new()));
        for (identity, kind) in [
            (first, RemoteCpuWorkKindV1::OpeningParse),
            (second, RemoteCpuWorkKindV1::PhysicalChunkValidation),
        ] {
            let order = order.clone();
            enqueue_new(
                &mut driver,
                work(identity, kind, 1, 2, 2, move || {
                    order.borrow_mut().push(identity.canonical_ordinal);
                }),
            )
            .unwrap();
        }
        assert!(matches!(
            driver.drive_cpu_v1(1),
            RemoteCpuDriveOutcomeV1::Completed { .. }
        ));
        assert!(matches!(
            driver.drive_cpu_v1(1),
            RemoteCpuDriveOutcomeV1::FrameAllowanceExhausted
        ));
        assert!(matches!(
            driver.drive_cpu_v1(2),
            RemoteCpuDriveOutcomeV1::Completed { .. }
        ));
        assert_eq!(*order.borrow(), [1, 2]);
    }

    #[test]
    fn retry_burns_attempt_and_clears_duplicate_ownership_before_escape() {
        let identity = RemoteWorkIdentityV1::for_test_v1(2, 4, 1);
        let mut driver = RemoteMcapCpuDriverV1::new_v1(identity.driver, limits());
        enqueue_new(
            &mut driver,
            RemoteFetchCompletionV1::RetryPending(
                RemoteRetryPendingV1::for_test_v1(identity, 10).unwrap(),
            ),
        )
        .unwrap();
        assert!(matches!(
            driver.enqueue_from_fetch_callback_v1(
                RemoteFetchCompletionV1::RetryPending(
                    RemoteRetryPendingV1::for_test_v1(identity, 10).unwrap(),
                ),
                || {},
            ),
            Err(RemoteFetchCompletionEnqueueErrorV1::DuplicateSettlement(_))
        ));
        let RemoteCpuDriveOutcomeV1::RetryAttemptReady(attempt) = driver.drive_cpu_v1(11) else {
            panic!("the next eligible driver turn must burn exactly one retry attempt");
        };
        assert_eq!(driver.identity, attempt.driver_identity_v1());
        assert_eq!(attempt.operation_identity_v1().attempt_generation, 5);
        assert_eq!(driver.usage_for_test_v1().0, (0, 0));
        assert!(matches!(
            driver.drive_cpu_v1(12),
            RemoteCpuDriveOutcomeV1::Idle
        ));
    }

    #[test]
    fn first_success_rejects_late_success_and_retry_settlements() {
        let identity = RemoteWorkIdentityV1::for_test_v1(20, 1, 1);
        let mut driver = RemoteMcapCpuDriverV1::new_v1(identity.driver, limits());
        enqueue_new(
            &mut driver,
            work(identity, RemoteCpuWorkKindV1::OpeningParse, 1, 1, 1, || {}),
        )
        .unwrap();

        let late_identity = RemoteWorkIdentityV1 {
            phase_generation: identity.phase_generation + 1,
            ..identity
        };
        assert!(matches!(
            driver.enqueue_from_fetch_callback_v1(
                work(
                    late_identity,
                    RemoteCpuWorkKindV1::OpeningParse,
                    1,
                    1,
                    1,
                    || {},
                ),
                || {},
            ),
            Err(RemoteFetchCompletionEnqueueErrorV1::DuplicateSettlement(_))
        ));
        assert!(matches!(
            driver.enqueue_from_fetch_callback_v1(
                RemoteFetchCompletionV1::RetryPending(
                    RemoteRetryPendingV1::for_test_v1(identity, 0).unwrap(),
                ),
                || {},
            ),
            Err(RemoteFetchCompletionEnqueueErrorV1::DuplicateSettlement(_))
        ));
        assert!(matches!(
            driver.drive_cpu_v1(1),
            RemoteCpuDriveOutcomeV1::Completed { .. }
        ));
        assert_eq!(driver.completed_work, BTreeSet::from([identity]));
    }

    #[test]
    fn first_retry_rejects_late_success_settlement() {
        let identity = RemoteWorkIdentityV1::for_test_v1(21, 1, 1);
        let mut driver = RemoteMcapCpuDriverV1::new_v1(identity.driver, limits());
        enqueue_new(
            &mut driver,
            RemoteFetchCompletionV1::RetryPending(
                RemoteRetryPendingV1::for_test_v1(identity, 0).unwrap(),
            ),
        )
        .unwrap();

        assert!(matches!(
            driver.enqueue_from_fetch_callback_v1(
                work(identity, RemoteCpuWorkKindV1::OpeningParse, 1, 1, 1, || {},),
                || {},
            ),
            Err(RemoteFetchCompletionEnqueueErrorV1::DuplicateSettlement(_))
        ));
        assert!(matches!(
            driver.drive_cpu_v1(1),
            RemoteCpuDriveOutcomeV1::RetryAttemptReady(_)
        ));
    }

    #[test]
    fn retry_burns_only_matching_operation_and_preserves_other_ready_and_work() {
        let retried = RemoteWorkIdentityV1::for_test_v1(7, 1, 1);
        let other = RemoteWorkIdentityV1::for_test_v1(7, 1, 2);
        let mut driver = RemoteMcapCpuDriverV1::new_v1(retried.driver, limits());
        driver
            .register_in_flight_operation_v1(retried.operation)
            .unwrap();
        enqueue_new(
            &mut driver,
            work(
                other,
                RemoteCpuWorkKindV1::PhysicalChunkValidation,
                1,
                1,
                1,
                || {},
            ),
        )
        .unwrap();
        driver
            .enqueue_from_fetch_callback_v1(
                RemoteFetchCompletionV1::RetryPending(
                    RemoteRetryPendingV1::for_test_v1(retried, 0).unwrap(),
                ),
                || {},
            )
            .unwrap();

        assert!(matches!(
            driver.drive_cpu_v1(1),
            RemoteCpuDriveOutcomeV1::Completed { .. }
        ));
        let ready_before_retry = driver.usage_for_test_v1().1;
        assert!(matches!(
            driver.drive_cpu_v1(2),
            RemoteCpuDriveOutcomeV1::RetryAttemptReady(_)
        ));
        assert_eq!(driver.usage_for_test_v1().1, ready_before_retry);
        assert!(driver.pop_ready_v1().is_some());
    }

    #[test]
    fn exact_work_registry_rejects_duplicate_and_activity_rebind_clears_stale() {
        let identity = RemoteWorkIdentityV1::for_test_v1(3, 1, 1);
        let mut driver = RemoteMcapCpuDriverV1::new_v1(identity.driver, limits());
        enqueue_new(
            &mut driver,
            work(
                identity,
                RemoteCpuWorkKindV1::MessageIndexParse,
                1,
                1,
                1,
                || {},
            ),
        )
        .unwrap();
        assert!(matches!(
            driver.enqueue_from_fetch_callback_v1(
                work(
                    identity,
                    RemoteCpuWorkKindV1::MessageIndexParse,
                    1,
                    1,
                    1,
                    || {},
                ),
                || {},
            ),
            Err(RemoteFetchCompletionEnqueueErrorV1::DuplicateWork(_))
        ));
        driver.rebind_activity_v1(RemoteDriverIdentityV1 {
            activity_epoch: identity.driver.activity_epoch + 1,
            ..identity.driver
        });
        assert_eq!(driver.usage_for_test_v1(), ((0, 0), (0, 0), 0));
    }

    #[test]
    fn output_is_reserved_before_execution_and_over_bound_never_publishes() {
        let identity = RemoteWorkIdentityV1::for_test_v1(4, 1, 1);
        let mut capped = limits();
        capped.max_ready_retained_bytes = 2;
        let ran = Rc::new(Cell::new(0));
        let mut driver = RemoteMcapCpuDriverV1::new_v1(identity.driver, capped);
        let ran_for_work = ran.clone();
        enqueue_new(
            &mut driver,
            work(
                identity,
                RemoteCpuWorkKindV1::OpeningParse,
                1,
                3,
                3,
                move || ran_for_work.set(1),
            ),
        )
        .unwrap();
        assert!(matches!(
            driver.drive_cpu_v1(1),
            RemoteCpuDriveOutcomeV1::ReadyReservationLimit { .. }
        ));
        assert_eq!(ran.get(), 0);
        assert_eq!(driver.usage_for_test_v1(), ((1, 1), (0, 0), 1));

        let mut driver = RemoteMcapCpuDriverV1::new_v1(identity.driver, limits());
        enqueue_new(
            &mut driver,
            work(identity, RemoteCpuWorkKindV1::OpeningParse, 1, 2, 3, || {}),
        )
        .unwrap();
        assert!(matches!(
            driver.drive_cpu_v1(1),
            RemoteCpuDriveOutcomeV1::WorkFailed {
                error: RemoteCpuExecutionErrorV1::BoundViolation,
                ..
            }
        ));
        assert_eq!(driver.usage_for_test_v1().1, (0, 0));
    }

    #[test]
    fn ready_backing_holds_permit_until_consumer_drops_result() {
        let identity = RemoteWorkIdentityV1::for_test_v1(5, 1, 1);
        let mut driver = RemoteMcapCpuDriverV1::new_v1(identity.driver, limits());
        enqueue_new(
            &mut driver,
            work(
                identity,
                RemoteCpuWorkKindV1::PhysicalChunkDispatchDecode,
                1,
                8,
                4,
                || {},
            ),
        )
        .unwrap();
        assert!(matches!(
            driver.drive_cpu_v1(1),
            RemoteCpuDriveOutcomeV1::Completed {
                output_bytes: 4,
                ..
            }
        ));
        assert_eq!(driver.usage_for_test_v1().0, (1, 1));
        assert_eq!(driver.usage_for_test_v1().1, (1, 8));
        let result = driver.pop_ready_v1().unwrap();
        assert_eq!(driver.usage_for_test_v1().0, (1, 1));
        assert_eq!(driver.usage_for_test_v1().1, (1, 8));
        drop(result);
        assert_eq!(driver.usage_for_test_v1().0, (0, 0));
        assert_eq!(driver.usage_for_test_v1().1, (0, 0));
    }

    #[test]
    fn telemetry_overflow_never_changes_successful_completion() {
        let identity = RemoteWorkIdentityV1::for_test_v1(6, 1, 1);
        let mut driver = RemoteMcapCpuDriverV1::new_v1(identity.driver, limits());
        driver.metrics[RemoteCpuWorkKindV1::OpeningParse.index()].output_bytes = u64::MAX;
        enqueue_new(
            &mut driver,
            work(identity, RemoteCpuWorkKindV1::OpeningParse, 1, 1, 1, || {}),
        )
        .unwrap();
        assert!(matches!(
            driver.drive_cpu_v1(1),
            RemoteCpuDriveOutcomeV1::Completed { .. }
        ));
        let metrics = driver
            .metrics_v1()
            .get_v1(RemoteCpuWorkKindV1::OpeningParse);
        assert_eq!(metrics.output_bytes, u64::MAX);
        assert!(metrics.overflowed);
    }
}
