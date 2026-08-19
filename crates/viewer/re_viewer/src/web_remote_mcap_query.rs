//! Production-disarmed remote MCAP query facade and revision leases.
//!
//! This module owns the query-isolation state machine required by MCAP-095. It does not hold or
//! publish a `StoreHub`, `EntityDb`, storage engine, or ordinary Viewer query capability. Real
//! Store mutation and Viewer wiring are intentionally left to later work items.

#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use re_chunk::{RangeQuery, TimeInt, TimelineName};
use re_log_types::{AbsoluteTimeRange, StoreId};

use crate::web_remote_mcap_activation::RemoteRecordingUseStateV1;

type FacadeRepaintCallback = Rc<dyn Fn()>;

pub(crate) trait ConsumerStorageFreeV1 {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct PresentationEpochV1(u64);

impl PresentationEpochV1 {
    const fn initial_v1() -> Self {
        Self(0)
    }

    const fn from_u64_v1(value: u64) -> Self {
        Self(value)
    }

    const fn get_v1(self) -> u64 {
        self.0
    }

    const fn next_v1(self) -> Result<Self, RemotePresentationTransitionErrorV1> {
        match self.0.checked_add(1) {
            Some(value) => Ok(Self(value)),
            None => Err(RemotePresentationTransitionErrorV1::PresentationEpochExhausted),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct PresentationFacadeInstanceIdV1(u64);

impl PresentationFacadeInstanceIdV1 {
    const fn from_u64_v1(value: u64) -> Self {
        Self(value)
    }

    const fn get_v1(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RemotePresentationFacadeInstanceAllocatorV1 {
    next: u64,
}

impl RemotePresentationFacadeInstanceAllocatorV1 {
    const fn new_v1(first: u64) -> Self {
        Self { next: first }
    }

    fn allocate_v1(
        &mut self,
    ) -> Result<PresentationFacadeInstanceIdV1, RemotePresentationTransitionErrorV1> {
        let value = self.next;
        if value == 0 {
            return Err(RemotePresentationTransitionErrorV1::FacadeInstanceExhausted);
        }
        self.next = self
            .next
            .checked_add(1)
            .ok_or(RemotePresentationTransitionErrorV1::FacadeInstanceExhausted)?;
        Ok(PresentationFacadeInstanceIdV1::from_u64_v1(value))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct PresentationRevisionV1 {
    facade_instance: PresentationFacadeInstanceIdV1,
    store_id: StoreId,
    epoch: PresentationEpochV1,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CommittedPresentationTimeV1 {
    pub(crate) timeline: TimelineName,
    pub(crate) cursor: TimeInt,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PresentationQuerySnapshotV1 {
    revision: PresentationRevisionV1,
    committed_time: Option<CommittedPresentationTimeV1>,
}

impl PresentationQuerySnapshotV1 {
    pub(crate) fn committed_time_v1(&self) -> Option<&CommittedPresentationTimeV1> {
        self.committed_time.as_ref()
    }

    fn revision_v1(&self) -> &PresentationRevisionV1 {
        &self.revision
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RevisionTaggedV1<T> {
    revision: PresentationRevisionV1,
    pub(crate) value: T,
}

impl<T> RevisionTaggedV1<T> {
    pub(crate) fn from_snapshot_v1(snapshot: &PresentationQuerySnapshotV1, value: T) -> Self {
        Self {
            revision: snapshot.revision.clone(),
            value,
        }
    }

    fn revision_v1(&self) -> &PresentationRevisionV1 {
        &self.revision
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CompleteIndexedCoverageV1 {
    NoIndexedMessages,
    Incomplete,
    Complete,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RemoteCanonicalIndexedExtentV1 {
    NoIndexedMessages,
    Known(AbsoluteTimeRange),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RemotePresentationMutationKindV1 {
    QueryVisibleInsertion,
    GarbageCollection,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteLoadedCoverageV1 {
    canonical_extent: RemoteCanonicalIndexedExtentV1,
    loaded_ranges: Vec<AbsoluteTimeRange>,
    opening_static_satisfied: bool,
}

impl RemoteLoadedCoverageV1 {
    pub(crate) fn new_v1(
        canonical_extent: RemoteCanonicalIndexedExtentV1,
        opening_static_satisfied: bool,
    ) -> Self {
        Self {
            canonical_extent,
            loaded_ranges: Vec::new(),
            opening_static_satisfied,
        }
    }

    pub(crate) fn set_canonical_extent_v1(&mut self, extent: RemoteCanonicalIndexedExtentV1) {
        self.canonical_extent = extent;
    }

    pub(crate) fn set_opening_static_satisfied_v1(&mut self, satisfied: bool) {
        self.opening_static_satisfied = satisfied;
    }

    pub(crate) fn replace_loaded_ranges_v1(
        &mut self,
        ranges: impl IntoIterator<Item = AbsoluteTimeRange>,
    ) {
        self.loaded_ranges = normalize_loaded_ranges_v1(ranges);
    }

    pub(crate) fn complete_indexed_coverage_v1(&self) -> CompleteIndexedCoverageV1 {
        if !self.opening_static_satisfied {
            return CompleteIndexedCoverageV1::Incomplete;
        }

        match &self.canonical_extent {
            RemoteCanonicalIndexedExtentV1::NoIndexedMessages => {
                CompleteIndexedCoverageV1::NoIndexedMessages
            }
            RemoteCanonicalIndexedExtentV1::Known(extent) if extent.is_empty() => {
                CompleteIndexedCoverageV1::Complete
            }
            RemoteCanonicalIndexedExtentV1::Known(extent) => {
                if loaded_ranges_cover_extent_v1(&self.loaded_ranges, *extent) {
                    CompleteIndexedCoverageV1::Complete
                } else {
                    CompleteIndexedCoverageV1::Incomplete
                }
            }
        }
    }

    pub(crate) fn loaded_ranges_v1(&self) -> &[AbsoluteTimeRange] {
        &self.loaded_ranges
    }
}

fn normalize_loaded_ranges_v1(
    ranges: impl IntoIterator<Item = AbsoluteTimeRange>,
) -> Vec<AbsoluteTimeRange> {
    let mut ranges: Vec<_> = ranges
        .into_iter()
        .filter(|range| !range.is_empty())
        .collect();
    ranges.sort_by(|left, right| {
        left.min
            .cmp(&right.min)
            .then_with(|| left.max.cmp(&right.max))
    });

    let mut merged: Vec<AbsoluteTimeRange> = Vec::with_capacity(ranges.len());
    for range in ranges {
        let Some(last) = merged.last_mut() else {
            merged.push(range);
            continue;
        };
        if range.min <= last.max.inc() {
            last.max = last.max.max(range.max);
        } else {
            merged.push(range);
        }
    }
    merged
}

fn loaded_ranges_cover_extent_v1(
    loaded_ranges: &[AbsoluteTimeRange],
    extent: AbsoluteTimeRange,
) -> bool {
    let mut covered_end = None::<TimeInt>;
    for range in loaded_ranges {
        if range.max < extent.min {
            continue;
        }
        if range.min > extent.max {
            break;
        }

        let Some(current_end) = covered_end else {
            if range.min > extent.min {
                return false;
            }
            covered_end = Some(range.max);
            continue;
        };

        if range.min > current_end.inc() {
            return false;
        }
        covered_end = Some(current_end.max(range.max));
    }

    covered_end.is_some_and(|end| end >= extent.max)
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RemotePresentationStateV1 {
    InitialPresentationGated {
        snapshot: PresentationQuerySnapshotV1,
    },
    Open {
        snapshot: PresentationQuerySnapshotV1,
    },
    ClosedForMutation {
        snapshot: PresentationQuerySnapshotV1,
        mutation_kind: RemotePresentationMutationKindV1,
    },
    TerminalGated {
        revision: PresentationRevisionV1,
    },
}

impl RemotePresentationStateV1 {
    fn revision_v1(&self) -> &PresentationRevisionV1 {
        match self {
            Self::InitialPresentationGated { snapshot }
            | Self::Open { snapshot }
            | Self::ClosedForMutation { snapshot, .. } => &snapshot.revision,
            Self::TerminalGated { revision } => revision,
        }
    }

    fn snapshot_v1(&self) -> Option<&PresentationQuerySnapshotV1> {
        match self {
            Self::InitialPresentationGated { snapshot }
            | Self::Open { snapshot }
            | Self::ClosedForMutation { snapshot, .. } => Some(snapshot),
            Self::TerminalGated { .. } => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PresentationLeaseUnavailableV1 {
    RecordingNotForeground,
    PresentationGated,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RemoteRangeQueryUnavailableV1 {
    RecordingNotForeground,
    PresentationGated,
    IndexedCoverageIncomplete {
        indexed_extent: RemoteCanonicalIndexedExtentV1,
        loaded_ranges: Vec<AbsoluteTimeRange>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemotePresentationTransitionErrorV1 {
    PresentationEpochExhausted,
    FacadeInstanceExhausted,
    InvalidPresentationState,
    OpeningStaticNotSatisfied,
    LeasesStillLive,
    LeaseCapacityExhausted,
    LeaseIdentityExhausted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteMutationLeaseDrainV1 {
    NoLiveLeases,
    WaitingForLiveLeaseDrain,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct PresentationLeaseIdentityV1 {
    counter_generation: u64,
    sequence: u64,
    acquired_revision: PresentationRevisionV1,
}

struct PresentationLeaseCounterStateV1 {
    generation: u64,
    next_sequence: u64,
    max_live_leases: usize,
    live: BTreeSet<PresentationLeaseIdentityV1>,
    drain_waiter: Option<PresentationRevisionV1>,
    lease_drained: Option<PresentationRevisionV1>,
}

struct PresentationLeaseCounterV1 {
    state: RefCell<PresentationLeaseCounterStateV1>,
    repaint: FacadeRepaintCallback,
}

impl PresentationLeaseCounterV1 {
    fn new_v1(max_live_leases: usize, repaint: FacadeRepaintCallback) -> Self {
        Self {
            state: RefCell::new(PresentationLeaseCounterStateV1 {
                generation: 1,
                next_sequence: 1,
                max_live_leases,
                live: BTreeSet::new(),
                drain_waiter: None,
                lease_drained: None,
            }),
            repaint,
        }
    }

    fn live_lease_count_v1(&self) -> usize {
        self.state.borrow().live.len()
    }

    fn counter_generation_v1(&self) -> u64 {
        self.state.borrow().generation
    }

    fn arm_drain_waiter_v1(&self, revision: PresentationRevisionV1) {
        let mut state = self.state.borrow_mut();
        state.drain_waiter = Some(revision);
    }

    fn clear_drain_v1(&self) {
        let mut state = self.state.borrow_mut();
        state.drain_waiter = None;
        state.lease_drained = None;
    }

    fn take_lease_drained_revision_v1(&self) -> Option<PresentationRevisionV1> {
        self.state.borrow_mut().lease_drained.take()
    }

    fn acquire_v1(
        &self,
        revision: PresentationRevisionV1,
    ) -> Result<PresentationLeaseGuardV1<'_>, RemotePresentationTransitionErrorV1> {
        let identity = {
            let mut state = self.state.borrow_mut();
            if state.live.len() >= state.max_live_leases {
                return Err(RemotePresentationTransitionErrorV1::LeaseCapacityExhausted);
            }

            let sequence = state.next_sequence;
            let next_sequence = state
                .next_sequence
                .checked_add(1)
                .ok_or(RemotePresentationTransitionErrorV1::LeaseIdentityExhausted)?;
            let identity = PresentationLeaseIdentityV1 {
                counter_generation: state.generation,
                sequence,
                acquired_revision: revision,
            };
            if !state.live.insert(identity.clone()) {
                return Err(RemotePresentationTransitionErrorV1::LeaseIdentityExhausted);
            }
            state.next_sequence = next_sequence;
            identity
        };

        Ok(PresentationLeaseGuardV1 {
            counter: self,
            identity: Some(identity),
        })
    }

    fn reinitialize_v1(&self) -> Result<(), RemotePresentationTransitionErrorV1> {
        let mut state = self.state.borrow_mut();
        if !state.live.is_empty() {
            return Err(RemotePresentationTransitionErrorV1::LeasesStillLive);
        }
        state.generation = state
            .generation
            .checked_add(1)
            .ok_or(RemotePresentationTransitionErrorV1::LeaseIdentityExhausted)?;
        state.next_sequence = 1;
        state.drain_waiter = None;
        state.lease_drained = None;
        Ok(())
    }

    fn remove_identity_v1(&self, identity: &PresentationLeaseIdentityV1) -> (bool, bool) {
        let mut state = self.state.borrow_mut();
        let removed = state.live.remove(identity);

        let should_repaint = if removed && state.live.is_empty() {
            if let Some(revision) = state.drain_waiter.take() {
                state.lease_drained = Some(revision);
                true
            } else {
                false
            }
        } else {
            false
        };

        (removed, should_repaint)
    }

    fn release_v1(&self, identity: &PresentationLeaseIdentityV1) {
        let (removed, should_repaint) = self.remove_identity_v1(identity);
        re_log::debug_assert!(removed, "stale or duplicate presentation lease release");

        if should_repaint {
            (self.repaint)();
        }
    }
}

pub(crate) struct PresentationQueryLeaseV1<'a> {
    pub(crate) snapshot: PresentationQuerySnapshotV1,
    guard: PresentationLeaseGuardV1<'a>,
}

pub(crate) struct PresentationLeaseGuardV1<'a> {
    counter: &'a PresentationLeaseCounterV1,
    identity: Option<PresentationLeaseIdentityV1>,
}

impl Drop for PresentationLeaseGuardV1<'_> {
    fn drop(&mut self) {
        if let Some(identity) = self.identity.take() {
            self.counter.release_v1(&identity);
        }
    }
}

pub(crate) struct RemotePresentationFacadeV1 {
    facade_instance: PresentationFacadeInstanceIdV1,
    store_id: StoreId,
    use_state: RefCell<RemoteRecordingUseStateV1>,
    state: RefCell<RemotePresentationStateV1>,
    leases: PresentationLeaseCounterV1,
    coverage: RefCell<RemoteLoadedCoverageV1>,
    repaint_handle: FacadeRepaintCallback,
    query_probe: RefCell<FacadeRepaintCallback>,
}

impl RemotePresentationFacadeV1 {
    pub(crate) fn new_initial_presentation_gated_v1(
        facade_instance: PresentationFacadeInstanceIdV1,
        store_id: StoreId,
        use_state: RemoteRecordingUseStateV1,
        coverage: RemoteLoadedCoverageV1,
        max_live_leases: usize,
        repaint: FacadeRepaintCallback,
    ) -> Self {
        let initial_revision = PresentationRevisionV1 {
            facade_instance,
            store_id: store_id.clone(),
            epoch: PresentationEpochV1::initial_v1(),
        };
        let snapshot = PresentationQuerySnapshotV1 {
            revision: initial_revision,
            committed_time: None,
        };
        let repaint_handle = Rc::clone(&repaint);
        Self {
            facade_instance,
            store_id,
            use_state: RefCell::new(use_state),
            state: RefCell::new(RemotePresentationStateV1::InitialPresentationGated { snapshot }),
            leases: PresentationLeaseCounterV1::new_v1(max_live_leases, repaint),
            coverage: RefCell::new(coverage),
            repaint_handle,
            query_probe: RefCell::new(Rc::new(|| {})),
        }
    }

    pub(crate) fn use_state_v1(&self) -> RemoteRecordingUseStateV1 {
        *self.use_state.borrow()
    }

    fn state_v1(&self) -> RemotePresentationStateV1 {
        self.state.borrow().clone()
    }

    fn snapshot_v1(&self) -> Option<PresentationQuerySnapshotV1> {
        self.state.borrow().snapshot_v1().cloned()
    }

    pub(crate) fn coverage_v1(&self) -> RemoteLoadedCoverageV1 {
        self.coverage.borrow().clone()
    }

    fn replace_coverage_v1(&self, coverage: RemoteLoadedCoverageV1) {
        *self.coverage.borrow_mut() = coverage;
    }

    pub(crate) fn live_lease_count_v1(&self) -> usize {
        self.leases.live_lease_count_v1()
    }

    pub(crate) fn lease_counter_generation_v1(&self) -> u64 {
        self.leases.counter_generation_v1()
    }

    fn take_lease_drained_revision_v1(&self) -> Option<PresentationRevisionV1> {
        self.leases.take_lease_drained_revision_v1()
    }

    pub(crate) fn reinitialize_lease_counter_v1(
        &self,
    ) -> Result<(), RemotePresentationTransitionErrorV1> {
        self.leases.reinitialize_v1()
    }

    fn set_use_state_v1(
        &self,
        use_state: RemoteRecordingUseStateV1,
    ) -> Result<PresentationRevisionV1, RemotePresentationTransitionErrorV1> {
        if *self.use_state.borrow() == use_state {
            return Ok(self.state.borrow().revision_v1().clone());
        }

        if *self.use_state.borrow() == RemoteRecordingUseStateV1::Foreground {
            self.advance_epoch_v1()?;
        }
        *self.use_state.borrow_mut() = use_state;
        Ok(self.state.borrow().revision_v1().clone())
    }

    fn set_opening_static_satisfied_v1(&self, satisfied: bool) {
        self.coverage
            .borrow_mut()
            .set_opening_static_satisfied_v1(satisfied);
    }

    fn commit_initial_presentation_v1(
        &self,
        committed_time: Option<CommittedPresentationTimeV1>,
    ) -> Result<PresentationRevisionV1, RemotePresentationTransitionErrorV1> {
        if self.leases.live_lease_count_v1() != 0 {
            return Err(RemotePresentationTransitionErrorV1::LeasesStillLive);
        }
        if !self.coverage.borrow().opening_static_satisfied {
            return Err(RemotePresentationTransitionErrorV1::OpeningStaticNotSatisfied);
        }

        let initial_snapshot = match &*self.state.borrow() {
            RemotePresentationStateV1::InitialPresentationGated { snapshot } => snapshot.clone(),
            _ => return Err(RemotePresentationTransitionErrorV1::InvalidPresentationState),
        };

        let committed_time = if matches!(
            self.coverage.borrow().canonical_extent,
            RemoteCanonicalIndexedExtentV1::NoIndexedMessages
        ) {
            None
        } else {
            committed_time
        };
        let revision = self.make_revision_v1(initial_snapshot.revision.epoch.next_v1()?);
        let snapshot = PresentationQuerySnapshotV1 {
            revision: revision.clone(),
            committed_time,
        };
        *self.state.borrow_mut() = RemotePresentationStateV1::Open { snapshot };
        Ok(revision)
    }

    fn commit_presentation_v1(
        &self,
        committed_time: Option<CommittedPresentationTimeV1>,
    ) -> Result<PresentationRevisionV1, RemotePresentationTransitionErrorV1> {
        let current_snapshot = match &*self.state.borrow() {
            RemotePresentationStateV1::Open { snapshot } => snapshot.clone(),
            _ => return Err(RemotePresentationTransitionErrorV1::InvalidPresentationState),
        };

        let revision = self.make_revision_v1(current_snapshot.revision.epoch.next_v1()?);
        let snapshot = PresentationQuerySnapshotV1 {
            revision: revision.clone(),
            committed_time,
        };
        *self.state.borrow_mut() = RemotePresentationStateV1::Open { snapshot };
        Ok(revision)
    }

    fn begin_mutation_v1(
        &self,
        mutation_kind: RemotePresentationMutationKindV1,
    ) -> Result<Option<PresentationRevisionV1>, RemotePresentationTransitionErrorV1> {
        let current_snapshot = match &*self.state.borrow() {
            RemotePresentationStateV1::Open { snapshot } => snapshot.clone(),
            _ => return Err(RemotePresentationTransitionErrorV1::InvalidPresentationState),
        };

        let revision = match mutation_kind {
            RemotePresentationMutationKindV1::QueryVisibleInsertion => {
                self.make_revision_v1(current_snapshot.revision.epoch.next_v1()?)
            }
            RemotePresentationMutationKindV1::GarbageCollection => {
                current_snapshot.revision.clone()
            }
        };
        let snapshot = PresentationQuerySnapshotV1 {
            revision: revision.clone(),
            committed_time: current_snapshot.committed_time,
        };
        *self.state.borrow_mut() = RemotePresentationStateV1::ClosedForMutation {
            snapshot,
            mutation_kind,
        };

        if self.leases.live_lease_count_v1() == 0 {
            Ok(None)
        } else {
            self.leases.arm_drain_waiter_v1(revision.clone());
            Ok(Some(revision))
        }
    }

    fn reopen_after_mutation_v1(
        &self,
        query_visible_deletion: bool,
    ) -> Result<PresentationRevisionV1, RemotePresentationTransitionErrorV1> {
        if self.leases.live_lease_count_v1() != 0 {
            return Err(RemotePresentationTransitionErrorV1::LeasesStillLive);
        }

        let (closed_snapshot, mutation_kind) = match &*self.state.borrow() {
            RemotePresentationStateV1::ClosedForMutation {
                snapshot,
                mutation_kind,
            } => (snapshot.clone(), *mutation_kind),
            _ => return Err(RemotePresentationTransitionErrorV1::InvalidPresentationState),
        };

        if query_visible_deletion
            && mutation_kind != RemotePresentationMutationKindV1::GarbageCollection
        {
            return Err(RemotePresentationTransitionErrorV1::InvalidPresentationState);
        }

        let snapshot = if query_visible_deletion {
            let revision = self.make_revision_v1(closed_snapshot.revision.epoch.next_v1()?);
            PresentationQuerySnapshotV1 {
                revision: revision.clone(),
                committed_time: closed_snapshot.committed_time.clone(),
            }
        } else {
            closed_snapshot
        };
        self.leases.clear_drain_v1();
        *self.state.borrow_mut() = RemotePresentationStateV1::Open { snapshot };
        Ok(self.state.borrow().revision_v1().clone())
    }

    fn terminal_gate_v1(&self) -> PresentationRevisionV1 {
        let revision = self.state.borrow().revision_v1().clone();
        *self.state.borrow_mut() = RemotePresentationStateV1::TerminalGated {
            revision: revision.clone(),
        };
        revision
    }

    fn make_revision_v1(&self, epoch: PresentationEpochV1) -> PresentationRevisionV1 {
        PresentationRevisionV1 {
            facade_instance: self.facade_instance,
            store_id: self.store_id.clone(),
            epoch,
        }
    }

    fn advance_epoch_v1(&self) -> Result<(), RemotePresentationTransitionErrorV1> {
        let current_revision = self.state.borrow().revision_v1().clone();
        let next_revision = self.make_revision_v1(current_revision.epoch.next_v1()?);

        let current_state = self.state.borrow().clone();
        let mut state = self.state.borrow_mut();
        match &current_state {
            RemotePresentationStateV1::InitialPresentationGated { snapshot } => {
                *state = RemotePresentationStateV1::InitialPresentationGated {
                    snapshot: PresentationQuerySnapshotV1 {
                        revision: next_revision,
                        committed_time: snapshot.committed_time.clone(),
                    },
                };
            }
            RemotePresentationStateV1::Open { snapshot } => {
                *state = RemotePresentationStateV1::Open {
                    snapshot: PresentationQuerySnapshotV1 {
                        revision: next_revision,
                        committed_time: snapshot.committed_time.clone(),
                    },
                };
            }
            RemotePresentationStateV1::ClosedForMutation {
                snapshot,
                mutation_kind,
            } => {
                *state = RemotePresentationStateV1::ClosedForMutation {
                    snapshot: PresentationQuerySnapshotV1 {
                        revision: next_revision,
                        committed_time: snapshot.committed_time.clone(),
                    },
                    mutation_kind: *mutation_kind,
                };
            }
            RemotePresentationStateV1::TerminalGated { .. } => {
                *state = RemotePresentationStateV1::TerminalGated {
                    revision: next_revision,
                };
            }
        }
        Ok(())
    }

    fn is_query_ready_v1(&self) -> bool {
        *self.use_state.borrow() == RemoteRecordingUseStateV1::Foreground
            && matches!(
                &*self.state.borrow(),
                RemotePresentationStateV1::Open { .. }
            )
    }

    fn acquire_query_lease_v1(
        &self,
    ) -> Result<PresentationQueryLeaseV1<'_>, PresentationLeaseUnavailableV1> {
        if !self.is_query_ready_v1() {
            return Err(
                if *self.use_state.borrow() == RemoteRecordingUseStateV1::Foreground {
                    PresentationLeaseUnavailableV1::PresentationGated
                } else {
                    PresentationLeaseUnavailableV1::RecordingNotForeground
                },
            );
        }

        let snapshot = self
            .state
            .borrow()
            .snapshot_v1()
            .cloned()
            .expect("open facade always has a snapshot");
        let guard = self
            .leases
            .acquire_v1(snapshot.revision.clone())
            .map_err(remote_lease_error_from_transition_v1)?;
        Ok(PresentationQueryLeaseV1 { snapshot, guard })
    }

    fn acquire_complete_range_lease_v1(
        &self,
        _query: &RangeQuery,
    ) -> Result<PresentationQueryLeaseV1<'_>, RemoteRangeQueryUnavailableV1> {
        if !self.is_query_ready_v1() {
            return Err(
                if *self.use_state.borrow() == RemoteRecordingUseStateV1::Foreground {
                    RemoteRangeQueryUnavailableV1::PresentationGated
                } else {
                    RemoteRangeQueryUnavailableV1::RecordingNotForeground
                },
            );
        }

        let coverage = self.coverage.borrow().clone();
        if coverage.complete_indexed_coverage_v1() != CompleteIndexedCoverageV1::Complete {
            return Err(RemoteRangeQueryUnavailableV1::IndexedCoverageIncomplete {
                indexed_extent: coverage.canonical_extent,
                loaded_ranges: coverage.loaded_ranges,
            });
        }

        let snapshot = self
            .state
            .borrow()
            .snapshot_v1()
            .cloned()
            .expect("open facade always has a snapshot");
        let guard = self
            .leases
            .acquire_v1(snapshot.revision.clone())
            .map_err(remote_range_lease_error_from_transition_v1)?;
        Ok(PresentationQueryLeaseV1 { snapshot, guard })
    }

    pub(crate) fn is_current_v1(&self, snapshot: &PresentationQuerySnapshotV1) -> bool {
        self.is_query_ready_v1() && self.state.borrow().revision_v1() == snapshot.revision_v1()
    }

    pub(crate) fn is_current_result_v1<T>(&self, result: &RevisionTaggedV1<T>) -> bool {
        self.is_query_ready_v1() && self.state.borrow().revision_v1() == result.revision_v1()
    }

    pub(crate) fn set_query_probe_v1(&self, probe: FacadeRepaintCallback) {
        *self.query_probe.borrow_mut() = probe;
    }

    fn privileged_set_use_state_v1(
        &self,
        use_state: RemoteRecordingUseStateV1,
    ) -> Result<(), RemotePresentationTransitionErrorV1> {
        self.set_use_state_v1(use_state).map(|_| ())
    }

    fn privileged_commit_initial_presentation_v1(
        &self,
        committed_time: Option<CommittedPresentationTimeV1>,
    ) -> Result<(), RemotePresentationTransitionErrorV1> {
        self.commit_initial_presentation_v1(committed_time)
            .map(|_| ())
    }

    fn privileged_commit_presentation_v1(
        &self,
        committed_time: Option<CommittedPresentationTimeV1>,
    ) -> Result<(), RemotePresentationTransitionErrorV1> {
        self.commit_presentation_v1(committed_time).map(|_| ())
    }

    fn privileged_begin_query_visible_insertion_v1(
        &self,
    ) -> Result<RemoteMutationLeaseDrainV1, RemotePresentationTransitionErrorV1> {
        self.begin_mutation_v1(RemotePresentationMutationKindV1::QueryVisibleInsertion)
            .map(|drain_revision| {
                if drain_revision.is_some() {
                    RemoteMutationLeaseDrainV1::WaitingForLiveLeaseDrain
                } else {
                    RemoteMutationLeaseDrainV1::NoLiveLeases
                }
            })
    }

    fn privileged_finish_query_visible_insertion_v1(
        &self,
    ) -> Result<(), RemotePresentationTransitionErrorV1> {
        self.reopen_after_mutation_v1(false).map(|_| ())
    }

    fn privileged_begin_garbage_collection_v1(
        &self,
    ) -> Result<RemoteMutationLeaseDrainV1, RemotePresentationTransitionErrorV1> {
        self.begin_mutation_v1(RemotePresentationMutationKindV1::GarbageCollection)
            .map(|drain_revision| {
                if drain_revision.is_some() {
                    RemoteMutationLeaseDrainV1::WaitingForLiveLeaseDrain
                } else {
                    RemoteMutationLeaseDrainV1::NoLiveLeases
                }
            })
    }

    fn privileged_finish_garbage_collection_v1(
        &self,
        query_visible_deletion: bool,
    ) -> Result<(), RemotePresentationTransitionErrorV1> {
        self.reopen_after_mutation_v1(query_visible_deletion)
            .map(|_| ())
    }

    fn privileged_terminal_gate_v1(&self) {
        self.terminal_gate_v1();
    }

    fn privileged_take_lease_drain_ready_v1(&self) -> bool {
        self.take_lease_drained_revision_v1().is_some()
    }
}

mod privileged_private {
    pub(crate) struct PrivilegedFrameContextSealV1;
    pub(crate) struct RemoteStorageCapabilitySealV1;
}

/// Sealed frame-level authority for creating privileged remote storage capabilities.
///
/// There is intentionally no public constructor. Consumer code cannot obtain or name the private
/// factory token, so it cannot construct this type.
pub(crate) struct PrivilegedViewerFrameContextV1 {
    _sealed: privileged_private::PrivilegedFrameContextSealV1,
}

impl PrivilegedViewerFrameContextV1 {
    fn new_privileged_viewer_frame_context_v1() -> Self {
        Self {
            _sealed: privileged_private::PrivilegedFrameContextSealV1,
        }
    }

    pub(crate) fn remote_storage_capability_v1<'a>(
        &self,
        facade: &'a RemotePresentationFacadeV1,
    ) -> PrivilegedRemoteStorageCapabilityV1<'a> {
        let _ = &self._sealed;
        PrivilegedRemoteStorageCapabilityV1 {
            facade,
            _sealed: privileged_private::RemoteStorageCapabilitySealV1,
        }
    }

    #[cfg(test)]
    pub(crate) fn new_for_test_v1() -> Self {
        Self::new_privileged_viewer_frame_context_v1()
    }
}

/// Sealed privileged storage control-plane capability.
///
/// Every method returns an owned outcome and never returns a borrow to the facade, an `EntityDb`,
/// storage engine, or a handle an asynchronous task could retain.
pub(crate) struct PrivilegedRemoteStorageCapabilityV1<'a> {
    facade: &'a RemotePresentationFacadeV1,
    _sealed: privileged_private::RemoteStorageCapabilitySealV1,
}

impl PrivilegedRemoteStorageCapabilityV1<'_> {
    pub(crate) fn set_use_state_v1(
        &self,
        use_state: RemoteRecordingUseStateV1,
    ) -> Result<(), RemotePresentationTransitionErrorV1> {
        self.facade.privileged_set_use_state_v1(use_state)
    }

    pub(crate) fn set_opening_static_satisfied_v1(&self, satisfied: bool) {
        self.facade.set_opening_static_satisfied_v1(satisfied);
    }

    pub(crate) fn replace_loaded_coverage_v1(&self, coverage: RemoteLoadedCoverageV1) {
        self.facade.replace_coverage_v1(coverage);
    }

    pub(crate) fn commit_initial_presentation_v1(
        &self,
        committed_time: Option<CommittedPresentationTimeV1>,
    ) -> Result<(), RemotePresentationTransitionErrorV1> {
        self.facade
            .privileged_commit_initial_presentation_v1(committed_time)
    }

    pub(crate) fn commit_presentation_v1(
        &self,
        committed_time: Option<CommittedPresentationTimeV1>,
    ) -> Result<(), RemotePresentationTransitionErrorV1> {
        self.facade
            .privileged_commit_presentation_v1(committed_time)
    }

    pub(crate) fn begin_query_visible_insertion_v1(
        &self,
    ) -> Result<RemoteMutationLeaseDrainV1, RemotePresentationTransitionErrorV1> {
        self.facade.privileged_begin_query_visible_insertion_v1()
    }

    pub(crate) fn finish_query_visible_insertion_v1(
        &self,
    ) -> Result<(), RemotePresentationTransitionErrorV1> {
        self.facade.privileged_finish_query_visible_insertion_v1()
    }

    pub(crate) fn begin_garbage_collection_v1(
        &self,
    ) -> Result<RemoteMutationLeaseDrainV1, RemotePresentationTransitionErrorV1> {
        self.facade.privileged_begin_garbage_collection_v1()
    }

    pub(crate) fn finish_garbage_collection_v1(
        &self,
        query_visible_deletion: bool,
    ) -> Result<(), RemotePresentationTransitionErrorV1> {
        self.facade
            .privileged_finish_garbage_collection_v1(query_visible_deletion)
    }

    pub(crate) fn terminal_gate_v1(&self) {
        self.facade.privileged_terminal_gate_v1();
    }

    pub(crate) fn take_lease_drain_ready_v1(&self) -> bool {
        self.facade.privileged_take_lease_drain_ready_v1()
    }
}

impl ConsumerStorageFreeV1 for u64 {}
impl ConsumerStorageFreeV1 for usize {}
impl ConsumerStorageFreeV1 for bool {}
impl ConsumerStorageFreeV1 for StoreId {}
impl ConsumerStorageFreeV1 for TimelineName {}
impl ConsumerStorageFreeV1 for TimeInt {}
impl ConsumerStorageFreeV1 for AbsoluteTimeRange {}
impl ConsumerStorageFreeV1 for RangeQuery {}
impl ConsumerStorageFreeV1 for RemoteRecordingUseStateV1 {}
impl ConsumerStorageFreeV1 for dyn Fn() {}

impl<T: ConsumerStorageFreeV1> ConsumerStorageFreeV1 for Option<T> {}
impl<T: ConsumerStorageFreeV1> ConsumerStorageFreeV1 for Vec<T> {}
impl<T: ConsumerStorageFreeV1> ConsumerStorageFreeV1 for RefCell<T> {}
impl<T: ConsumerStorageFreeV1 + ?Sized> ConsumerStorageFreeV1 for Rc<T> {}
impl<T: ConsumerStorageFreeV1 + ?Sized> ConsumerStorageFreeV1 for &T {}
impl<T: ConsumerStorageFreeV1 + ?Sized> ConsumerStorageFreeV1 for &mut T {}
impl<T: ConsumerStorageFreeV1> ConsumerStorageFreeV1 for BTreeSet<T> {}

impl ConsumerStorageFreeV1 for PresentationEpochV1 {}
impl ConsumerStorageFreeV1 for PresentationFacadeInstanceIdV1 {}
impl ConsumerStorageFreeV1 for RemotePresentationFacadeInstanceAllocatorV1 {}
impl ConsumerStorageFreeV1 for PresentationLeaseUnavailableV1 {}
impl ConsumerStorageFreeV1 for RemotePresentationTransitionErrorV1 {}
impl ConsumerStorageFreeV1 for RemoteMutationLeaseDrainV1 {}
impl ConsumerStorageFreeV1 for CompleteIndexedCoverageV1 {}
impl ConsumerStorageFreeV1 for RemotePresentationMutationKindV1 {}

impl ConsumerStorageFreeV1 for RemoteCanonicalIndexedExtentV1 where
    AbsoluteTimeRange: ConsumerStorageFreeV1
{
}

impl ConsumerStorageFreeV1 for RemoteLoadedCoverageV1
where
    RemoteCanonicalIndexedExtentV1: ConsumerStorageFreeV1,
    Vec<AbsoluteTimeRange>: ConsumerStorageFreeV1,
    bool: ConsumerStorageFreeV1,
{
}

impl ConsumerStorageFreeV1 for PresentationRevisionV1
where
    PresentationFacadeInstanceIdV1: ConsumerStorageFreeV1,
    StoreId: ConsumerStorageFreeV1,
    PresentationEpochV1: ConsumerStorageFreeV1,
{
}

impl ConsumerStorageFreeV1 for CommittedPresentationTimeV1
where
    TimelineName: ConsumerStorageFreeV1,
    TimeInt: ConsumerStorageFreeV1,
{
}

impl ConsumerStorageFreeV1 for PresentationQuerySnapshotV1
where
    PresentationRevisionV1: ConsumerStorageFreeV1,
    Option<CommittedPresentationTimeV1>: ConsumerStorageFreeV1,
{
}

impl<T> ConsumerStorageFreeV1 for RevisionTaggedV1<T>
where
    PresentationRevisionV1: ConsumerStorageFreeV1,
    T: ConsumerStorageFreeV1,
{
}

impl ConsumerStorageFreeV1 for RemoteRangeQueryUnavailableV1
where
    RemoteCanonicalIndexedExtentV1: ConsumerStorageFreeV1,
    Vec<AbsoluteTimeRange>: ConsumerStorageFreeV1,
{
}

impl ConsumerStorageFreeV1 for RemotePresentationStateV1
where
    PresentationQuerySnapshotV1: ConsumerStorageFreeV1,
    RemotePresentationMutationKindV1: ConsumerStorageFreeV1,
    PresentationRevisionV1: ConsumerStorageFreeV1,
{
}

impl ConsumerStorageFreeV1 for PresentationLeaseIdentityV1
where
    u64: ConsumerStorageFreeV1,
    PresentationRevisionV1: ConsumerStorageFreeV1,
{
}

impl ConsumerStorageFreeV1 for PresentationLeaseCounterStateV1
where
    u64: ConsumerStorageFreeV1,
    usize: ConsumerStorageFreeV1,
    BTreeSet<PresentationLeaseIdentityV1>: ConsumerStorageFreeV1,
    Option<PresentationRevisionV1>: ConsumerStorageFreeV1,
{
}

impl ConsumerStorageFreeV1 for PresentationLeaseCounterV1
where
    RefCell<PresentationLeaseCounterStateV1>: ConsumerStorageFreeV1,
    FacadeRepaintCallback: ConsumerStorageFreeV1,
{
}

impl<'a> ConsumerStorageFreeV1 for PresentationLeaseGuardV1<'a>
where
    &'a PresentationLeaseCounterV1: ConsumerStorageFreeV1,
    Option<PresentationLeaseIdentityV1>: ConsumerStorageFreeV1,
{
}

impl<'a> ConsumerStorageFreeV1 for PresentationQueryLeaseV1<'a>
where
    PresentationQuerySnapshotV1: ConsumerStorageFreeV1,
    PresentationLeaseGuardV1<'a>: ConsumerStorageFreeV1,
{
}

impl ConsumerStorageFreeV1 for RemotePresentationFacadeV1
where
    PresentationFacadeInstanceIdV1: ConsumerStorageFreeV1,
    StoreId: ConsumerStorageFreeV1,
    RefCell<RemoteRecordingUseStateV1>: ConsumerStorageFreeV1,
    RefCell<RemotePresentationStateV1>: ConsumerStorageFreeV1,
    PresentationLeaseCounterV1: ConsumerStorageFreeV1,
    RefCell<RemoteLoadedCoverageV1>: ConsumerStorageFreeV1,
    FacadeRepaintCallback: ConsumerStorageFreeV1,
    RefCell<FacadeRepaintCallback>: ConsumerStorageFreeV1,
{
}

impl ConsumerStorageFreeV1 for privileged_private::PrivilegedFrameContextSealV1 {}
impl ConsumerStorageFreeV1 for privileged_private::RemoteStorageCapabilitySealV1 {}

impl ConsumerStorageFreeV1 for PrivilegedViewerFrameContextV1 where
    privileged_private::PrivilegedFrameContextSealV1: ConsumerStorageFreeV1
{
}

impl<'a> ConsumerStorageFreeV1 for PrivilegedRemoteStorageCapabilityV1<'a>
where
    &'a RemotePresentationFacadeV1: ConsumerStorageFreeV1,
    privileged_private::RemoteStorageCapabilitySealV1: ConsumerStorageFreeV1,
{
}

#[cfg(test)]
impl RemotePresentationFacadeV1 {
    pub(crate) fn new_for_test_v1(
        facade_instance: u64,
        store_id: StoreId,
        use_state: RemoteRecordingUseStateV1,
        coverage: RemoteLoadedCoverageV1,
        max_live_leases: usize,
        repaint: Rc<dyn Fn()>,
    ) -> Self {
        Self::new_initial_presentation_gated_v1(
            PresentationFacadeInstanceIdV1::from_u64_v1(facade_instance),
            store_id,
            use_state,
            coverage,
            max_live_leases,
            repaint,
        )
    }
}

pub(crate) trait GatedRecordingQueryFacadeV1 {
    fn try_lease_v1(&self) -> Result<PresentationQueryLeaseV1<'_>, PresentationLeaseUnavailableV1>;

    fn try_complete_range_lease_v1(
        &self,
        query: &RangeQuery,
    ) -> Result<PresentationQueryLeaseV1<'_>, RemoteRangeQueryUnavailableV1>;

    fn is_current_v1(&self, snapshot: &PresentationQuerySnapshotV1) -> bool;

    fn is_current_result_v1<T>(&self, result: &RevisionTaggedV1<T>) -> bool;
}

impl GatedRecordingQueryFacadeV1 for RemotePresentationFacadeV1 {
    fn try_lease_v1(&self) -> Result<PresentationQueryLeaseV1<'_>, PresentationLeaseUnavailableV1> {
        self.acquire_query_lease_v1()
    }

    fn try_complete_range_lease_v1(
        &self,
        query: &RangeQuery,
    ) -> Result<PresentationQueryLeaseV1<'_>, RemoteRangeQueryUnavailableV1> {
        self.acquire_complete_range_lease_v1(query)
    }

    fn is_current_v1(&self, snapshot: &PresentationQuerySnapshotV1) -> bool {
        self.is_current_v1(snapshot)
    }

    fn is_current_result_v1<T>(&self, result: &RevisionTaggedV1<T>) -> bool {
        self.is_current_result_v1(result)
    }
}

fn remote_lease_error_from_transition_v1(
    error: RemotePresentationTransitionErrorV1,
) -> PresentationLeaseUnavailableV1 {
    re_log::debug_assert!(
        matches!(
            error,
            RemotePresentationTransitionErrorV1::LeaseCapacityExhausted
                | RemotePresentationTransitionErrorV1::LeaseIdentityExhausted
        ),
        "unexpected transition error while acquiring lease: {error:?}"
    );
    PresentationLeaseUnavailableV1::PresentationGated
}

fn remote_range_lease_error_from_transition_v1(
    error: RemotePresentationTransitionErrorV1,
) -> RemoteRangeQueryUnavailableV1 {
    re_log::debug_assert!(
        matches!(
            error,
            RemotePresentationTransitionErrorV1::LeaseCapacityExhausted
                | RemotePresentationTransitionErrorV1::LeaseIdentityExhausted
        ),
        "unexpected transition error while acquiring range lease: {error:?}"
    );
    RemoteRangeQueryUnavailableV1::PresentationGated
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn store_id() -> StoreId {
        StoreId::recording("facade-test-app", "facade-test-recording")
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

    fn repaint_handle() -> (FacadeRepaintCallback, Rc<Cell<usize>>) {
        let count = Rc::new(Cell::new(0));
        let count_for_closure = Rc::clone(&count);
        (
            Rc::new(move || {
                count_for_closure.set(count_for_closure.get() + 1);
            }),
            count,
        )
    }

    fn initial_facade(max_live_leases: usize) -> (RemotePresentationFacadeV1, Rc<Cell<usize>>) {
        let (repaint, repaint_count) = repaint_handle();
        let facade = RemotePresentationFacadeV1::new_initial_presentation_gated_v1(
            PresentationFacadeInstanceIdV1::from_u64_v1(1),
            store_id(),
            RemoteRecordingUseStateV1::Foreground,
            RemoteLoadedCoverageV1::new_v1(RemoteCanonicalIndexedExtentV1::Known(extent()), false),
            max_live_leases,
            repaint,
        );
        (facade, repaint_count)
    }

    fn initial_no_indexed_facade(
        max_live_leases: usize,
    ) -> (RemotePresentationFacadeV1, Rc<Cell<usize>>) {
        let (repaint, repaint_count) = repaint_handle();
        let facade = RemotePresentationFacadeV1::new_initial_presentation_gated_v1(
            PresentationFacadeInstanceIdV1::from_u64_v1(2),
            store_id(),
            RemoteRecordingUseStateV1::Foreground,
            RemoteLoadedCoverageV1::new_v1(
                RemoteCanonicalIndexedExtentV1::NoIndexedMessages,
                false,
            ),
            max_live_leases,
            repaint,
        );
        (facade, repaint_count)
    }

    fn open_facade(facade: &RemotePresentationFacadeV1, cursor: i64) {
        facade.set_opening_static_satisfied_v1(true);
        facade
            .commit_initial_presentation_v1(Some(committed_time(cursor)))
            .expect("open facade");
    }

    #[test]
    fn initial_gate_rejects_leases_and_current_results() {
        let (facade, _) = initial_facade(1);
        let initial_snapshot = facade.snapshot_v1().expect("snapshot");

        assert!(matches!(
            facade.try_lease_v1(),
            Err(PresentationLeaseUnavailableV1::PresentationGated)
        ));
        assert!(matches!(
            facade.try_complete_range_lease_v1(&RangeQuery::new(timeline(), extent())),
            Err(RemoteRangeQueryUnavailableV1::PresentationGated)
        ));
        assert!(!facade.is_current_v1(&initial_snapshot));
    }

    #[test]
    fn initial_complete_empty_can_open_without_premature_lease() {
        let (facade, _) = initial_facade(1);
        let old_revision = facade.snapshot_v1().expect("snapshot").revision;

        let error = facade
            .commit_initial_presentation_v1(Some(committed_time(0)))
            .expect_err("opening static is not satisfied");
        assert_eq!(
            error,
            RemotePresentationTransitionErrorV1::OpeningStaticNotSatisfied
        );

        facade.set_opening_static_satisfied_v1(true);
        let revision = facade
            .commit_initial_presentation_v1(Some(committed_time(0)))
            .expect("initial CompleteEmpty should open");
        assert!(revision.epoch.get_v1() > old_revision.epoch.get_v1());
        assert!(facade.try_lease_v1().is_ok());
    }

    #[test]
    fn no_indexed_messages_initial_open_forces_missing_temporal_cursor() {
        let (facade, _) = initial_no_indexed_facade(1);
        facade.set_opening_static_satisfied_v1(true);

        facade
            .commit_initial_presentation_v1(Some(committed_time(5)))
            .expect("NoIndexedMessages should open");

        let snapshot = facade.snapshot_v1().expect("snapshot");
        assert_eq!(snapshot.committed_time, None);
        assert!(matches!(
            facade.state_v1(),
            RemotePresentationStateV1::Open { .. }
        ));

        let lease = facade.try_lease_v1().expect("static lease");
        assert_eq!(lease.snapshot.committed_time, None);
        assert!(facade.is_current_v1(&lease.snapshot));
    }

    #[test]
    fn open_lease_carries_current_revision_and_frozen_snapshot() {
        let (facade, _) = initial_facade(1);
        open_facade(&facade, 0);

        let lease = facade.try_lease_v1().expect("lease");
        let frozen = lease.snapshot.clone();
        assert_eq!(
            lease.snapshot.revision,
            facade.snapshot_v1().expect("snapshot").revision
        );
        assert_eq!(lease.snapshot.committed_time, Some(committed_time(0)));

        facade
            .commit_presentation_v1(Some(committed_time(7)))
            .expect("advance presentation");
        assert_eq!(lease.snapshot, frozen);
        assert!(!facade.is_current_v1(&frozen));
    }

    #[test]
    fn non_foreground_state_never_issues_leases() {
        let (facade, _) = initial_facade(1);
        open_facade(&facade, 0);

        facade
            .set_use_state_v1(RemoteRecordingUseStateV1::CatalogOnly)
            .expect("leave foreground");

        assert!(matches!(
            facade.try_lease_v1(),
            Err(PresentationLeaseUnavailableV1::RecordingNotForeground)
        ));
        assert!(matches!(
            facade.try_complete_range_lease_v1(&RangeQuery::new(timeline(), extent())),
            Err(RemoteRangeQueryUnavailableV1::RecordingNotForeground)
        ));
        let snapshot = facade.snapshot_v1().expect("snapshot");
        assert!(!facade.is_current_v1(&snapshot));
    }

    #[test]
    fn foreground_loss_advances_revision_and_invalidates_old_results() {
        let (facade, _) = initial_facade(1);
        open_facade(&facade, 0);
        let open_snapshot = facade.snapshot_v1().expect("snapshot");

        facade
            .set_use_state_v1(RemoteRecordingUseStateV1::CatalogOnly)
            .expect("leave foreground");
        assert!(!facade.is_current_v1(&open_snapshot));

        facade
            .set_use_state_v1(RemoteRecordingUseStateV1::Foreground)
            .expect("return foreground");
        assert!(!facade.is_current_v1(&open_snapshot));
    }

    #[test]
    fn inactive_use_state_rejects_queries_after_open() {
        let (facade, _) = initial_facade(1);
        open_facade(&facade, 0);

        facade
            .set_use_state_v1(RemoteRecordingUseStateV1::Inactive)
            .expect("inactive");
        assert!(matches!(
            facade.try_lease_v1(),
            Err(PresentationLeaseUnavailableV1::RecordingNotForeground)
        ));
        let snapshot = facade.snapshot_v1().expect("snapshot");
        assert!(!facade.is_current_v1(&snapshot));
    }

    #[test]
    fn leases_are_unique_and_released_exactly_once() {
        let (facade, repaint_count) = initial_facade(2);
        open_facade(&facade, 0);

        let lease1 = facade.try_lease_v1().expect("lease1");
        let lease2 = facade.try_lease_v1().expect("lease2");
        assert_eq!(facade.live_lease_count_v1(), 2);

        drop(lease1);
        assert_eq!(facade.live_lease_count_v1(), 1);
        assert_eq!(repaint_count.get(), 0);

        drop(lease2);
        assert_eq!(facade.live_lease_count_v1(), 0);
        assert_eq!(repaint_count.get(), 0);
    }

    #[test]
    fn last_lease_wakes_drain_waiter() {
        let (facade, repaint_count) = initial_facade(1);
        open_facade(&facade, 0);

        let lease = facade.try_lease_v1().expect("lease");
        let waiting_revision = facade
            .begin_mutation_v1(RemotePresentationMutationKindV1::QueryVisibleInsertion)
            .expect("mutation begins")
            .expect("live lease requires drain");
        assert_eq!(repaint_count.get(), 0);

        drop(lease);
        assert_eq!(repaint_count.get(), 1);
        assert_eq!(
            facade.take_lease_drained_revision_v1(),
            Some(waiting_revision)
        );
    }

    #[test]
    fn mutation_waits_for_old_lease_then_reopens_with_same_counter() {
        let (facade, repaint_count) = initial_facade(1);
        open_facade(&facade, 0);
        let lease = facade.try_lease_v1().expect("lease");
        let generation = facade.lease_counter_generation_v1();

        let waiting_revision = facade
            .begin_mutation_v1(RemotePresentationMutationKindV1::QueryVisibleInsertion)
            .expect("mutation begins")
            .expect("live lease requires drain");
        assert!(matches!(
            facade.state_v1(),
            RemotePresentationStateV1::ClosedForMutation { .. }
        ));
        assert_eq!(repaint_count.get(), 0);

        drop(lease);
        assert_eq!(repaint_count.get(), 1);
        assert_eq!(
            facade.take_lease_drained_revision_v1(),
            Some(waiting_revision.clone())
        );

        facade
            .reopen_after_mutation_v1(false)
            .expect("reopen after drain");
        assert_eq!(facade.lease_counter_generation_v1(), generation);
        assert!(matches!(
            facade.state_v1(),
            RemotePresentationStateV1::Open { .. }
        ));
        let reopened_snapshot = facade.snapshot_v1().expect("snapshot");
        assert_eq!(reopened_snapshot.revision, waiting_revision);
        assert!(facade.is_current_v1(&reopened_snapshot));
    }

    #[test]
    fn query_visible_insertion_advances_epoch_when_closing() {
        let (facade, _) = initial_facade(1);
        open_facade(&facade, 0);
        let open_snapshot = facade.snapshot_v1().expect("snapshot");

        facade
            .begin_mutation_v1(RemotePresentationMutationKindV1::QueryVisibleInsertion)
            .expect("insertion mutation");
        let closed_snapshot = facade.snapshot_v1().expect("snapshot");
        assert!(closed_snapshot.revision.epoch.get_v1() > open_snapshot.revision.epoch.get_v1());
        assert!(!facade.is_current_v1(&open_snapshot));

        facade
            .reopen_after_mutation_v1(false)
            .expect("reopen insertion");
        assert!(!facade.is_current_v1(&open_snapshot));
        assert!(facade.is_current_v1(&facade.snapshot_v1().expect("snapshot")));
    }

    #[test]
    fn no_op_gc_preserves_epoch_and_reopens_same_publication() {
        let (facade, _) = initial_facade(1);
        open_facade(&facade, 0);
        let open_snapshot = facade.snapshot_v1().expect("snapshot");

        facade
            .begin_mutation_v1(RemotePresentationMutationKindV1::GarbageCollection)
            .expect("gc mutation");
        let closed_snapshot = facade.snapshot_v1().expect("snapshot");
        assert_eq!(closed_snapshot.revision, open_snapshot.revision);
        assert!(!facade.is_current_v1(&open_snapshot));

        facade
            .reopen_after_mutation_v1(false)
            .expect("reopen no-op gc");
        assert!(facade.is_current_v1(&open_snapshot));
    }

    #[test]
    fn query_visible_gc_deletion_advances_epoch_before_reopen() {
        let (facade, _) = initial_facade(1);
        open_facade(&facade, 0);
        let open_snapshot = facade.snapshot_v1().expect("snapshot");

        facade
            .begin_mutation_v1(RemotePresentationMutationKindV1::GarbageCollection)
            .expect("gc mutation");
        let closed_snapshot = facade.snapshot_v1().expect("snapshot");
        assert_eq!(closed_snapshot.revision, open_snapshot.revision);

        facade
            .reopen_after_mutation_v1(true)
            .expect("reopen query-visible deletion");
        let reopened_snapshot = facade.snapshot_v1().expect("snapshot");
        assert!(reopened_snapshot.revision.epoch.get_v1() > open_snapshot.revision.epoch.get_v1());
        assert!(!facade.is_current_v1(&open_snapshot));
        assert!(facade.is_current_v1(&reopened_snapshot));
    }

    #[test]
    fn query_visible_deletion_requires_gc_mutation() {
        let (facade, _) = initial_facade(1);
        open_facade(&facade, 0);

        facade
            .begin_mutation_v1(RemotePresentationMutationKindV1::QueryVisibleInsertion)
            .expect("insertion mutation");
        assert_eq!(
            facade.reopen_after_mutation_v1(true),
            Err(RemotePresentationTransitionErrorV1::InvalidPresentationState)
        );
    }

    #[test]
    fn stale_and_duplicate_release_never_underflow() {
        let (repaint, _) = repaint_handle();
        let counter = PresentationLeaseCounterV1::new_v1(1, repaint);
        let revision = PresentationRevisionV1 {
            facade_instance: PresentationFacadeInstanceIdV1::from_u64_v1(1),
            store_id: store_id(),
            epoch: PresentationEpochV1::from_u64_v1(1),
        };
        let stale = PresentationLeaseIdentityV1 {
            counter_generation: 1,
            sequence: 999,
            acquired_revision: revision.clone(),
        };

        let (removed, should_repaint) = counter.remove_identity_v1(&stale);
        assert!(!removed);
        assert!(!should_repaint);
        assert_eq!(counter.live_lease_count_v1(), 0);

        let guard = counter.acquire_v1(revision).expect("live lease");
        let identity = guard.identity.as_ref().expect("identity").clone();
        assert_eq!(counter.live_lease_count_v1(), 1);
        drop(guard);

        let (removed, should_repaint) = counter.remove_identity_v1(&identity);
        assert!(!removed);
        assert!(!should_repaint);
        assert_eq!(counter.live_lease_count_v1(), 0);
    }

    #[test]
    fn closed_and_terminal_states_reject_queries() {
        let (facade, _) = initial_facade(1);
        open_facade(&facade, 0);
        let open_snapshot = facade.snapshot_v1().expect("snapshot");

        facade
            .begin_mutation_v1(RemotePresentationMutationKindV1::QueryVisibleInsertion)
            .expect("close for mutation");
        assert!(matches!(
            facade.try_lease_v1(),
            Err(PresentationLeaseUnavailableV1::PresentationGated)
        ));
        assert!(!facade.is_current_v1(&open_snapshot));

        facade
            .reopen_after_mutation_v1(false)
            .expect("reopen after mutation");
        assert!(!facade.is_current_v1(&open_snapshot));

        facade.terminal_gate_v1();
        assert!(matches!(
            facade.try_lease_v1(),
            Err(PresentationLeaseUnavailableV1::PresentationGated)
        ));
        assert!(!facade.is_current_v1(&open_snapshot));
    }

    #[test]
    fn incomplete_range_never_issues_store_lease() {
        let (facade, _) = initial_facade(1);
        open_facade(&facade, 0);
        let (probe, probe_count) = repaint_handle();
        facade.set_query_probe_v1(probe);

        let result = facade.try_complete_range_lease_v1(&RangeQuery::new(timeline(), extent()));
        match result {
            Err(RemoteRangeQueryUnavailableV1::IndexedCoverageIncomplete {
                indexed_extent,
                loaded_ranges,
            }) => {
                assert_eq!(
                    indexed_extent,
                    RemoteCanonicalIndexedExtentV1::Known(extent())
                );
                assert!(loaded_ranges.is_empty());
            }
            Err(other) => panic!("expected incomplete coverage error, got {other:?}"),
            Ok(_) => panic!("incomplete coverage must not produce a lease"),
        }
        assert_eq!(facade.live_lease_count_v1(), 0);
        assert_eq!(probe_count.get(), 0);
    }

    #[test]
    fn complete_coverage_allows_range_lease() {
        let (facade, _) = initial_facade(1);
        open_facade(&facade, 0);

        let mut coverage = facade.coverage_v1();
        coverage.replace_loaded_ranges_v1([extent()]);
        facade.replace_coverage_v1(coverage);

        let lease = facade
            .try_complete_range_lease_v1(&RangeQuery::new(timeline(), extent()))
            .expect("complete range lease");
        assert_eq!(facade.live_lease_count_v1(), 1);
        assert!(facade.is_current_v1(&lease.snapshot));
    }

    #[test]
    fn ordinary_lease_does_not_imply_complete_range() {
        let (facade, _) = initial_facade(1);
        open_facade(&facade, 0);
        let snapshot = facade.snapshot_v1().expect("snapshot");

        let lease = facade.try_lease_v1().expect("ordinary lease");
        assert_eq!(lease.snapshot.revision, snapshot.revision);
        drop(lease);

        let mut coverage =
            RemoteLoadedCoverageV1::new_v1(RemoteCanonicalIndexedExtentV1::NoIndexedMessages, true);
        coverage.replace_loaded_ranges_v1([extent()]);
        facade.replace_coverage_v1(coverage);

        let result = facade.try_complete_range_lease_v1(&RangeQuery::new(timeline(), extent()));
        if let Err(RemoteRangeQueryUnavailableV1::IndexedCoverageIncomplete {
            indexed_extent,
            ..
        }) = result
        {
            assert_eq!(
                indexed_extent,
                RemoteCanonicalIndexedExtentV1::NoIndexedMessages
            );
        } else {
            panic!("NoIndexedMessages must reject range query");
        }
        assert_eq!(facade.live_lease_count_v1(), 0);
    }

    #[test]
    fn epoch_and_instance_allocators_are_checked() {
        assert_eq!(
            PresentationEpochV1::from_u64_v1(u64::MAX).next_v1(),
            Err(RemotePresentationTransitionErrorV1::PresentationEpochExhausted)
        );

        let mut zero_allocator = RemotePresentationFacadeInstanceAllocatorV1::new_v1(0);
        assert_eq!(
            zero_allocator.allocate_v1(),
            Err(RemotePresentationTransitionErrorV1::FacadeInstanceExhausted)
        );

        let mut allocator = RemotePresentationFacadeInstanceAllocatorV1::new_v1(u64::MAX - 1);
        let first = allocator.allocate_v1().expect("first instance");
        assert_eq!(first.get_v1(), u64::MAX - 1);
        assert_eq!(
            allocator.allocate_v1(),
            Err(RemotePresentationTransitionErrorV1::FacadeInstanceExhausted)
        );
    }

    #[test]
    fn lease_counter_admission_is_bounded() {
        let (facade, _) = initial_facade(1);
        open_facade(&facade, 0);

        let _lease = facade.try_lease_v1().expect("first lease");
        assert!(matches!(
            facade.try_lease_v1(),
            Err(PresentationLeaseUnavailableV1::PresentationGated)
        ));
    }

    #[test]
    fn counter_generation_only_advances_when_live_set_is_empty() {
        let (facade, _) = initial_facade(1);
        open_facade(&facade, 0);

        let generation = facade.lease_counter_generation_v1();
        assert!(matches!(facade.reinitialize_lease_counter_v1(), Ok(())));
        assert_eq!(facade.lease_counter_generation_v1(), generation + 1);
    }
}
