//! Deterministic ownership and scheduling primitives for asynchronous Web Viewer tests.
//!
//! The scheduler in this module polls real futures with real wakers on one native thread.
//! Its browser lanes are an explicit test model; real browser ordering is covered by the
//! Chrome adapter in `tests/rust/test_mcap_chrome_fixture`.
//! No remote-open, lifecycle, importer or Store-mutation business state is implemented here.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{Arc, Weak as ArcWeak};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use futures::future::{AbortHandle, Abortable};
use futures::task::{ArcWake, waker};
use parking_lot::Mutex;

const RESOURCE_KIND_COUNT: usize = 9;

/// A reusable slot plus its monotonically increasing generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GenerationToken {
    pub slot: u32,
    pub generation: u64,
}

impl GenerationToken {
    pub const fn is_newer_generation_of(self, older: Self) -> bool {
        self.slot == older.slot && self.generation > older.generation
    }
}

/// Allocates generation-aware identities and rejects stale release attempts.
#[derive(Debug, Default)]
pub struct GenerationSlots {
    next_generation: BTreeMap<u32, u64>,
    occupied: BTreeMap<u32, GenerationToken>,
}

impl GenerationSlots {
    pub fn check_claim(&self, slot: u32) -> Result<(), GenerationSlotError> {
        if let Some(current) = self.occupied.get(&slot) {
            return Err(GenerationSlotError::Occupied(*current));
        }
        self.next_generation
            .get(&slot)
            .copied()
            .unwrap_or(1)
            .checked_add(1)
            .ok_or(GenerationSlotError::GenerationExhausted { slot })?;
        Ok(())
    }

    pub fn claim(&mut self, slot: u32) -> Result<GenerationToken, GenerationSlotError> {
        self.check_claim(slot)?;
        let generation = self.next_generation.get(&slot).copied().unwrap_or(1);
        let next_generation = generation
            .checked_add(1)
            .ok_or(GenerationSlotError::GenerationExhausted { slot })?;
        let token = GenerationToken { slot, generation };
        self.next_generation.insert(slot, next_generation);
        self.occupied.insert(slot, token);
        Ok(token)
    }

    pub fn release(&mut self, token: GenerationToken) -> bool {
        if self.occupied.get(&token.slot) != Some(&token) {
            return false;
        }
        self.occupied.remove(&token.slot);
        true
    }

    pub fn is_current(&self, token: GenerationToken) -> bool {
        self.occupied.get(&token.slot) == Some(&token)
    }

    pub fn occupied_count(&self) -> usize {
        self.occupied.len()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GenerationSlotError {
    #[error("generation slot is occupied")]
    Occupied(GenerationToken),

    #[error("generation counter is exhausted for slot {slot}")]
    GenerationExhausted { slot: u32 },
}

/// Execution boundaries controlled independently by native scheduler tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ExecutionLane {
    NetworkCompletion,
    BrowserMicrotask,
    BrowserMacrotask,
    AnimationFrame,
    ViewerFrame,
    StoreCallback,
}

/// Store insertion boundaries named by the design without implementing their state machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InsertionSafePoint {
    WaitingForLeases,
    BeforeFirstAddChunk,
    BetweenAddChunks,
    AfterLastAddChunkBeforeAck,
}

/// Store GC boundaries named by the design without implementing their state machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GcSafePoint {
    WaitingForLeases,
    BeforeGcCall,
    AfterGcBeforeReopen,
}

/// A typed asynchronous boundary at which a production future can be parked by a test.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AsyncBoundary {
    Prepare,
    DisarmedCommit,
    JsRelease,
    Activation,
    FetchCompletion,
    BodyBackpressure,
    LegacyApplyAck,
    Insertion(InsertionSafePoint),
    Gc(GcSafePoint),
    TeardownLateCompletion,
}

/// Distinguishes repeated visits to the same typed boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Checkpoint {
    pub boundary: AsyncBoundary,
    pub ordinal: u16,
}

impl Checkpoint {
    pub const fn once(boundary: AsyncBoundary) -> Self {
        Self {
            boundary,
            ordinal: 0,
        }
    }
}

/// Every retained resource category observed by the harness.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum ResourceKind {
    Task,
    Owner,
    Permit,
    QueueWaiter,
    QueuedItem,
    InFlightItem,
    Lease,
    Timer,
    GateWaiter,
}

impl ResourceKind {
    pub const ALL: [Self; RESOURCE_KIND_COUNT] = [
        Self::Task,
        Self::Owner,
        Self::Permit,
        Self::QueueWaiter,
        Self::QueuedItem,
        Self::InFlightItem,
        Self::Lease,
        Self::Timer,
        Self::GateWaiter,
    ];

    const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceLimit {
    pub max_count: u64,
    pub max_bytes: u64,
}

impl ResourceLimit {
    pub const UNLIMITED: Self = Self {
        max_count: u64::MAX,
        max_bytes: u64::MAX,
    };
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResourceUsage {
    pub current_count: u64,
    pub current_bytes: u64,
    pub high_water_count: u64,
    pub high_water_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceSnapshot {
    usage: [ResourceUsage; RESOURCE_KIND_COUNT],
}

impl ResourceSnapshot {
    pub fn usage(&self, kind: ResourceKind) -> ResourceUsage {
        self.usage[kind.index()]
    }

    pub fn is_live_empty(&self) -> bool {
        self.usage
            .iter()
            .all(|usage| usage.current_count == 0 && usage.current_bytes == 0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ResourceError {
    #[error("resource accounting overflow")]
    Overflow { kind: ResourceKind },

    #[error("resource limit exceeded")]
    LimitExceeded { kind: ResourceKind },
}

#[derive(Debug)]
struct ResourceTrackerState {
    limits: [ResourceLimit; RESOURCE_KIND_COUNT],
    usage: [ResourceUsage; RESOURCE_KIND_COUNT],
}

/// Shared checked accounting for all harness-owned resources.
#[derive(Clone, Debug)]
pub struct ResourceTracker(Rc<RefCell<ResourceTrackerState>>);

impl Default for ResourceTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceTracker {
    pub fn new() -> Self {
        Self(Rc::new(RefCell::new(ResourceTrackerState {
            limits: [ResourceLimit::UNLIMITED; RESOURCE_KIND_COUNT],
            usage: [ResourceUsage::default(); RESOURCE_KIND_COUNT],
        })))
    }

    pub fn set_limit(&self, kind: ResourceKind, limit: ResourceLimit) {
        self.0.borrow_mut().limits[kind.index()] = limit;
    }

    pub fn reserve(
        &self,
        kind: ResourceKind,
        count: u64,
        bytes: u64,
    ) -> Result<ResourceGuard, ResourceError> {
        let mut state = self.0.borrow_mut();
        let limit = state.limits[kind.index()];
        let usage = &mut state.usage[kind.index()];
        let next_count = usage
            .current_count
            .checked_add(count)
            .ok_or(ResourceError::Overflow { kind })?;
        let next_bytes = usage
            .current_bytes
            .checked_add(bytes)
            .ok_or(ResourceError::Overflow { kind })?;
        if next_count > limit.max_count || next_bytes > limit.max_bytes {
            return Err(ResourceError::LimitExceeded { kind });
        }
        usage.current_count = next_count;
        usage.current_bytes = next_bytes;
        usage.high_water_count = usage.high_water_count.max(next_count);
        usage.high_water_bytes = usage.high_water_bytes.max(next_bytes);
        drop(state);
        Ok(ResourceGuard {
            tracker: self.clone(),
            kind,
            count,
            bytes,
            active: true,
        })
    }

    pub fn snapshot(&self) -> ResourceSnapshot {
        ResourceSnapshot {
            usage: self.0.borrow().usage,
        }
    }

    fn release(&self, kind: ResourceKind, count: u64, bytes: u64) {
        let mut state = self.0.borrow_mut();
        let usage = &mut state.usage[kind.index()];
        usage.current_count = usage
            .current_count
            .checked_sub(count)
            .expect("resource count ownership must balance");
        usage.current_bytes = usage
            .current_bytes
            .checked_sub(bytes)
            .expect("resource byte ownership must balance");
    }

    fn reclassify(
        &self,
        old_kind: ResourceKind,
        new_kind: ResourceKind,
        count: u64,
        bytes: u64,
    ) -> Result<(), ResourceError> {
        if old_kind == new_kind {
            return Ok(());
        }
        let mut state = self.0.borrow_mut();
        let new_limit = state.limits[new_kind.index()];
        let old_usage = state.usage[old_kind.index()];
        let new_usage = state.usage[new_kind.index()];
        let next_count = new_usage
            .current_count
            .checked_add(count)
            .ok_or(ResourceError::Overflow { kind: new_kind })?;
        let next_bytes = new_usage
            .current_bytes
            .checked_add(bytes)
            .ok_or(ResourceError::Overflow { kind: new_kind })?;
        if next_count > new_limit.max_count || next_bytes > new_limit.max_bytes {
            return Err(ResourceError::LimitExceeded { kind: new_kind });
        }
        state.usage[old_kind.index()].current_count = old_usage
            .current_count
            .checked_sub(count)
            .expect("resource count ownership must balance");
        state.usage[old_kind.index()].current_bytes = old_usage
            .current_bytes
            .checked_sub(bytes)
            .expect("resource byte ownership must balance");
        let new_usage = &mut state.usage[new_kind.index()];
        new_usage.current_count = next_count;
        new_usage.current_bytes = next_bytes;
        new_usage.high_water_count = new_usage.high_water_count.max(next_count);
        new_usage.high_water_bytes = new_usage.high_water_bytes.max(next_bytes);
        Ok(())
    }
}

/// A non-cloneable RAII reservation.
#[derive(Debug)]
pub struct ResourceGuard {
    tracker: ResourceTracker,
    kind: ResourceKind,
    count: u64,
    bytes: u64,
    active: bool,
}

impl ResourceGuard {
    pub fn kind(&self) -> ResourceKind {
        self.kind
    }

    pub fn count(&self) -> u64 {
        self.count
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    pub fn reclassify(&mut self, new_kind: ResourceKind) -> Result<(), ResourceError> {
        self.tracker
            .reclassify(self.kind, new_kind, self.count, self.bytes)?;
        self.kind = new_kind;
        Ok(())
    }

    pub fn release(mut self) {
        self.release_inner();
    }

    fn release_inner(&mut self) {
        if self.active {
            self.tracker.release(self.kind, self.count, self.bytes);
            self.active = false;
        }
    }
}

impl Drop for ResourceGuard {
    fn drop(&mut self) {
        self.release_inner();
    }
}

/// Bounded, enum-only scheduler diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceEventKind {
    Spawn,
    Poll,
    WakeObserved,
    StaleWakeDropped,
    Complete,
    Cancel,
    Abort,
    Panic,
    CheckpointWait,
    CheckpointRelease,
    CompletionAccepted,
    CompletionRejected,
    QueueWait,
    QueueEnqueue,
    QueueDeliver,
    QueueAck,
    QueueDrop,
    LeaseAcquire,
    LeaseRelease,
    LeaseDrained,
    ClockAdvance,
    PageSignal,
    RepaintRequest,
    FrameAllowance,
    FrameConsumed,
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TraceEvent {
    pub sequence: u64,
    pub kind: TraceEventKind,
    pub token: Option<GenerationToken>,
    pub lane: Option<ExecutionLane>,
    pub value: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceSnapshot {
    pub events: Vec<TraceEvent>,
    pub overflowed: bool,
    pub retained_bytes: usize,
}

#[derive(Debug)]
struct TraceState {
    events: VecDeque<TraceEvent>,
    next_sequence: u64,
    max_events: usize,
    max_bytes: usize,
    retained_bytes: usize,
    overflowed: bool,
}

#[derive(Clone, Debug)]
pub struct BoundedTrace(Rc<RefCell<TraceState>>);

impl BoundedTrace {
    pub fn new(max_events: usize, max_bytes: usize) -> Self {
        Self(Rc::new(RefCell::new(TraceState {
            events: VecDeque::new(),
            next_sequence: 0,
            max_events,
            max_bytes,
            retained_bytes: 0,
            overflowed: false,
        })))
    }

    pub fn record(
        &self,
        kind: TraceEventKind,
        token: Option<GenerationToken>,
        lane: Option<ExecutionLane>,
        value: u64,
    ) {
        let mut state = self.0.borrow_mut();
        let sequence = state.next_sequence;
        let Some(next_sequence) = state.next_sequence.checked_add(1) else {
            state.overflowed = true;
            return;
        };
        let event_bytes = std::mem::size_of::<TraceEvent>();
        if state.max_events == 0 || event_bytes > state.max_bytes {
            state.overflowed = true;
            return;
        }
        while state.events.len() >= state.max_events
            || state
                .retained_bytes
                .checked_add(event_bytes)
                .is_none_or(|next| next > state.max_bytes)
        {
            if state.events.pop_front().is_none() {
                state.overflowed = true;
                return;
            }
            state.retained_bytes = state
                .retained_bytes
                .checked_sub(event_bytes)
                .expect("trace retained bytes must match retained events");
            state.overflowed = true;
        }
        state.next_sequence = next_sequence;
        state.events.push_back(TraceEvent {
            sequence,
            kind,
            token,
            lane,
            value,
        });
        state.retained_bytes = state
            .retained_bytes
            .checked_add(event_bytes)
            .expect("trace capacity check prevents byte overflow");
    }

    pub fn snapshot(&self) -> TraceSnapshot {
        let state = self.0.borrow();
        TraceSnapshot {
            events: state.events.iter().copied().collect(),
            overflowed: state.overflowed,
            retained_bytes: state.retained_bytes,
        }
    }
}

impl Default for BoundedTrace {
    fn default() -> Self {
        Self::new(512, 64 * 1024)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadyOrder {
    Fifo,
    Seeded(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ReadyEntry {
    token: GenerationToken,
    lane: ExecutionLane,
}

#[derive(Debug)]
struct ReadyState {
    accepting: bool,
    entries: VecDeque<ReadyEntry>,
    present: BTreeSet<(ExecutionLane, GenerationToken)>,
}

impl ReadyState {
    fn enqueue(&mut self, entry: ReadyEntry) {
        if !self.accepting {
            return;
        }
        if self.present.insert((entry.lane, entry.token)) {
            self.entries.push_back(entry);
        }
    }

    fn remove_at(&mut self, index: usize) -> Option<ReadyEntry> {
        let entry = self.entries.remove(index)?;
        self.present.remove(&(entry.lane, entry.token));
        Some(entry)
    }

    fn remove_token(&mut self, token: GenerationToken) {
        self.entries.retain(|entry| entry.token != token);
        self.present.retain(|(_lane, current)| *current != token);
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.present.clear();
    }
}

impl Default for ReadyState {
    fn default() -> Self {
        Self {
            accepting: true,
            entries: VecDeque::new(),
            present: BTreeSet::new(),
        }
    }
}

#[derive(Debug)]
struct TaskWake {
    entry: ReadyEntry,
    ready: Arc<Mutex<ReadyState>>,
}

impl ArcWake for TaskWake {
    fn wake_by_ref(arc_self: &Arc<Self>) {
        arc_self.ready.lock().enqueue(arc_self.entry);
    }
}

struct ScheduledTask {
    lane: ExecutionLane,
    future: Pin<Box<dyn Future<Output = ()> + 'static>>,
    completion_settlement: Rc<Cell<TaskSettlement>>,
    _resource: ResourceGuard,
}

impl std::fmt::Debug for ScheduledTask {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ScheduledTask")
            .field("lane", &self.lane)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskSettlement {
    Completed,
    Cancelled,
    Aborted,
    Panicked,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HarnessSnapshot {
    pub stopped: bool,
    pub live_tasks: usize,
    pub ready_tasks: usize,
    pub live_wakers: usize,
    pub resources: ResourceSnapshot,
    pub trace: TraceSnapshot,
}

impl HarnessSnapshot {
    pub fn is_clean(&self) -> bool {
        self.live_tasks == 0
            && self.ready_tasks == 0
            && self.live_wakers == 0
            && self.resources.is_live_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HarnessDiagnostic {
    pub steps: usize,
    pub snapshot: HarnessSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SchedulerError {
    #[error("scheduler is stopped")]
    Stopped,

    #[error("scheduler step budget exhausted")]
    StepBudgetExceeded(Box<HarnessDiagnostic>),

    #[error("scheduler has live but unrunnable futures")]
    Deadlocked(Box<HarnessDiagnostic>),

    #[error("scheduled future panicked")]
    TaskPanicked(Box<HarnessDiagnostic>),

    #[error("live waker limit exceeded")]
    WakerLimitExceeded(Box<HarnessDiagnostic>),

    #[error("automatic task slots are exhausted")]
    AutomaticSlotsExhausted,

    #[error(transparent)]
    Generation(#[from] GenerationSlotError),

    #[error(transparent)]
    Resource(#[from] ResourceError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepOutcome {
    PolledPending(GenerationToken),
    Completed(GenerationToken),
    StaleWakeDropped(GenerationToken),
    NoReadyTask,
}

/// Single-threaded deterministic executor for non-`Send` futures.
pub struct DeterministicScheduler {
    tasks: BTreeMap<GenerationToken, ScheduledTask>,
    slots: GenerationSlots,
    next_automatic_slot: Option<u32>,
    ready: Arc<Mutex<ReadyState>>,
    wakers: Vec<ArcWeak<TaskWake>>,
    settlements: BTreeMap<GenerationToken, TaskSettlement>,
    settlement_order: VecDeque<GenerationToken>,
    max_settlements: usize,
    max_live_wakers: usize,
    order: ReadyOrder,
    random_state: u64,
    stopped: bool,
    tracker: ResourceTracker,
    trace: BoundedTrace,
}

impl std::fmt::Debug for DeterministicScheduler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DeterministicScheduler")
            .field("snapshot", &self.snapshot())
            .finish()
    }
}

impl Default for DeterministicScheduler {
    fn default() -> Self {
        Self::new(ReadyOrder::Fifo)
    }
}

impl DeterministicScheduler {
    pub fn new(order: ReadyOrder) -> Self {
        let random_state = match order {
            ReadyOrder::Fifo => 0,
            ReadyOrder::Seeded(seed) => seed.max(1),
        };
        Self {
            tasks: BTreeMap::new(),
            slots: GenerationSlots::default(),
            next_automatic_slot: Some(0),
            ready: Arc::new(Mutex::new(ReadyState::default())),
            wakers: Vec::new(),
            settlements: BTreeMap::new(),
            settlement_order: VecDeque::new(),
            max_settlements: 1024,
            max_live_wakers: 1024,
            order,
            random_state,
            stopped: false,
            tracker: ResourceTracker::new(),
            trace: BoundedTrace::default(),
        }
    }

    pub fn with_limits(
        order: ReadyOrder,
        max_settlements: usize,
        max_live_wakers: usize,
        trace_events: usize,
        trace_bytes: usize,
    ) -> Self {
        let mut scheduler = Self::new(order);
        scheduler.max_settlements = max_settlements;
        scheduler.max_live_wakers = max_live_wakers;
        scheduler.trace = BoundedTrace::new(trace_events, trace_bytes);
        scheduler
    }

    pub fn tracker(&self) -> ResourceTracker {
        self.tracker.clone()
    }

    pub fn trace(&self) -> BoundedTrace {
        self.trace.clone()
    }

    pub fn spawn(
        &mut self,
        lane: ExecutionLane,
        future: impl Future<Output = ()> + 'static,
    ) -> Result<GenerationToken, SchedulerError> {
        self.spawn_automatic_with_settlement(
            lane,
            future,
            Rc::new(Cell::new(TaskSettlement::Completed)),
        )
    }

    pub fn spawn_in_slot(
        &mut self,
        slot: u32,
        lane: ExecutionLane,
        future: impl Future<Output = ()> + 'static,
    ) -> Result<GenerationToken, SchedulerError> {
        if self.stopped {
            return Err(SchedulerError::Stopped);
        }
        self.spawn_in_slot_with_settlement(
            slot,
            lane,
            future,
            Rc::new(Cell::new(TaskSettlement::Completed)),
        )
    }

    fn spawn_in_slot_with_settlement(
        &mut self,
        slot: u32,
        lane: ExecutionLane,
        future: impl Future<Output = ()> + 'static,
        completion_settlement: Rc<Cell<TaskSettlement>>,
    ) -> Result<GenerationToken, SchedulerError> {
        if self.stopped {
            return Err(SchedulerError::Stopped);
        }
        self.slots.check_claim(slot)?;
        let resource = self.tracker.reserve(ResourceKind::Task, 1, 0)?;
        let token = self
            .slots
            .claim(slot)
            .expect("prepared generation claim must commit without failure");
        self.tasks.insert(
            token,
            ScheduledTask {
                lane,
                future: Box::pin(future),
                completion_settlement,
                _resource: resource,
            },
        );
        self.ready.lock().enqueue(ReadyEntry { token, lane });
        self.trace
            .record(TraceEventKind::Spawn, Some(token), Some(lane), 0);
        Ok(token)
    }

    fn spawn_automatic_with_settlement(
        &mut self,
        lane: ExecutionLane,
        future: impl Future<Output = ()> + 'static,
        completion_settlement: Rc<Cell<TaskSettlement>>,
    ) -> Result<GenerationToken, SchedulerError> {
        let slot = self
            .next_automatic_slot
            .ok_or(SchedulerError::AutomaticSlotsExhausted)?;
        let token =
            self.spawn_in_slot_with_settlement(slot, lane, future, completion_settlement)?;
        self.next_automatic_slot = slot.checked_add(1);
        Ok(token)
    }

    pub fn spawn_abortable(
        &mut self,
        lane: ExecutionLane,
        future: impl Future<Output = ()> + 'static,
    ) -> Result<(GenerationToken, AbortHandle), SchedulerError> {
        let (abort_handle, registration) = AbortHandle::new_pair();
        let trace = self.trace.clone();
        let token_cell = Rc::new(Cell::new(None));
        let task_token_cell = token_cell.clone();
        let completion_settlement = Rc::new(Cell::new(TaskSettlement::Completed));
        let task_settlement = completion_settlement.clone();
        let task = async move {
            if Abortable::new(future, registration).await.is_err()
                && let Some(token) = task_token_cell.get()
            {
                task_settlement.set(TaskSettlement::Aborted);
                trace.record(TraceEventKind::Abort, Some(token), Some(lane), 0);
            }
        };
        let token = self.spawn_automatic_with_settlement(lane, task, completion_settlement)?;
        token_cell.set(Some(token));
        Ok((token, abort_handle))
    }

    pub fn cancel(&mut self, token: GenerationToken) -> bool {
        let Some(task) = self.tasks.remove(&token) else {
            return false;
        };
        drop(task);
        self.ready.lock().remove_token(token);
        self.slots.release(token);
        self.settle(token, TaskSettlement::Cancelled);
        self.trace.record(
            TraceEventKind::Cancel,
            Some(token),
            None,
            TaskSettlement::Cancelled as u64,
        );
        true
    }

    pub fn settlement(&self, token: GenerationToken) -> Option<TaskSettlement> {
        self.settlements.get(&token).copied()
    }

    pub fn step(&mut self) -> Result<StepOutcome, SchedulerError> {
        self.step_matching(None)
    }

    pub fn step_lane(&mut self, lane: ExecutionLane) -> Result<StepOutcome, SchedulerError> {
        self.step_matching(Some(lane))
    }

    fn step_matching(
        &mut self,
        lane_filter: Option<ExecutionLane>,
    ) -> Result<StepOutcome, SchedulerError> {
        let Some(entry) = self.pop_ready(lane_filter) else {
            return Ok(StepOutcome::NoReadyTask);
        };
        self.trace.record(
            TraceEventKind::WakeObserved,
            Some(entry.token),
            Some(entry.lane),
            0,
        );
        let Some(mut task) = self.tasks.remove(&entry.token) else {
            self.trace.record(
                TraceEventKind::StaleWakeDropped,
                Some(entry.token),
                Some(entry.lane),
                0,
            );
            return Ok(StepOutcome::StaleWakeDropped(entry.token));
        };
        if task.lane != entry.lane {
            let correct_entry = ReadyEntry {
                token: entry.token,
                lane: task.lane,
            };
            self.tasks.insert(entry.token, task);
            self.ready.lock().enqueue(correct_entry);
            self.trace.record(
                TraceEventKind::StaleWakeDropped,
                Some(entry.token),
                Some(entry.lane),
                1,
            );
            return Ok(StepOutcome::StaleWakeDropped(entry.token));
        }

        let wake = Arc::new(TaskWake {
            entry,
            ready: self.ready.clone(),
        });
        self.wakers.push(Arc::downgrade(&wake));
        let task_waker = waker(wake);
        let mut context = Context::from_waker(&task_waker);
        self.trace
            .record(TraceEventKind::Poll, Some(entry.token), Some(entry.lane), 0);
        #[expect(
            clippy::disallowed_methods,
            reason = "the harness explicitly verifies unwind cleanup"
        )]
        let poll = catch_unwind(AssertUnwindSafe(|| task.future.as_mut().poll(&mut context)));
        drop(task_waker);
        let waker_limit_exceeded_while_pending = self.live_waker_count() > self.max_live_wakers;
        match poll {
            Ok(Poll::Pending) if waker_limit_exceeded_while_pending => {
                drop(task);
                self.slots.release(entry.token);
                self.settle(entry.token, TaskSettlement::Cancelled);
                self.trace.record(
                    TraceEventKind::Cancel,
                    Some(entry.token),
                    Some(entry.lane),
                    1,
                );
                self.stop();
                Err(SchedulerError::WakerLimitExceeded(Box::new(
                    self.diagnostic(1),
                )))
            }
            Ok(Poll::Pending) => {
                self.tasks.insert(entry.token, task);
                Ok(StepOutcome::PolledPending(entry.token))
            }
            Ok(Poll::Ready(())) => {
                let settlement = task.completion_settlement.get();
                drop(task);
                self.slots.release(entry.token);
                self.settle(entry.token, settlement);
                self.trace.record(
                    TraceEventKind::Complete,
                    Some(entry.token),
                    Some(entry.lane),
                    0,
                );
                if self.live_waker_count() > self.max_live_wakers {
                    self.stop();
                    return Err(SchedulerError::WakerLimitExceeded(Box::new(
                        self.diagnostic(1),
                    )));
                }
                Ok(StepOutcome::Completed(entry.token))
            }
            Err(_panic) => {
                drop(task);
                self.slots.release(entry.token);
                self.settle(entry.token, TaskSettlement::Panicked);
                self.trace.record(
                    TraceEventKind::Panic,
                    Some(entry.token),
                    Some(entry.lane),
                    0,
                );
                self.stop();
                Err(SchedulerError::TaskPanicked(Box::new(self.diagnostic(1))))
            }
        }
    }

    pub fn run_until_idle(&mut self, step_budget: usize) -> Result<usize, SchedulerError> {
        let mut steps = 0;
        while !self.ready.lock().entries.is_empty() {
            if steps == step_budget {
                return Err(SchedulerError::StepBudgetExceeded(Box::new(
                    self.diagnostic(steps),
                )));
            }
            self.step()?;
            steps += 1;
        }
        Ok(steps)
    }

    pub fn drive_until(
        &mut self,
        step_budget: usize,
        mut predicate: impl FnMut(&Self) -> bool,
    ) -> Result<usize, SchedulerError> {
        let mut steps = 0;
        while !predicate(self) {
            if steps == step_budget {
                return Err(SchedulerError::StepBudgetExceeded(Box::new(
                    self.diagnostic(steps),
                )));
            }
            if self.ready.lock().entries.is_empty() {
                return Err(SchedulerError::Deadlocked(Box::new(self.diagnostic(steps))));
            }
            self.step()?;
            steps += 1;
        }
        Ok(steps)
    }

    pub fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        let tasks = std::mem::take(&mut self.tasks);
        for (token, task) in tasks {
            drop(task);
            self.slots.release(token);
            self.settle(token, TaskSettlement::Cancelled);
            self.trace
                .record(TraceEventKind::Cancel, Some(token), None, 0);
        }
        let mut ready = self.ready.lock();
        ready.accepting = false;
        ready.clear();
        drop(ready);
        self.prune_wakers();
        self.trace.record(TraceEventKind::Stop, None, None, 0);
    }

    /// Drops ready entries that no longer refer to a live matching generation.
    pub fn drain_stale_wakes(&self, step_budget: usize) -> Result<usize, SchedulerError> {
        let mut steps = 0;
        loop {
            let stale_index = {
                let ready = self.ready.lock();
                ready.entries.iter().position(|entry| {
                    self.tasks
                        .get(&entry.token)
                        .is_none_or(|task| task.lane != entry.lane)
                })
            };
            let Some(stale_index) = stale_index else {
                return Ok(steps);
            };
            if steps == step_budget {
                return Err(SchedulerError::StepBudgetExceeded(Box::new(
                    self.diagnostic(steps),
                )));
            }
            let entry = self
                .ready
                .lock()
                .remove_at(stale_index)
                .expect("stale ready entry was observed under the same-thread scheduler");
            self.trace.record(
                TraceEventKind::StaleWakeDropped,
                Some(entry.token),
                Some(entry.lane),
                0,
            );
            steps += 1;
        }
    }

    pub fn snapshot(&self) -> HarnessSnapshot {
        HarnessSnapshot {
            stopped: self.stopped,
            live_tasks: self.tasks.len(),
            ready_tasks: self.ready.lock().entries.len(),
            live_wakers: self.live_waker_count_readonly(),
            resources: self.tracker.snapshot(),
            trace: self.trace.snapshot(),
        }
    }

    #[track_caller]
    pub fn assert_clean(&self) {
        let snapshot = self.snapshot();
        assert!(
            snapshot.is_clean(),
            "harness retained resources: {snapshot:#?}"
        );
    }

    fn diagnostic(&self, steps: usize) -> HarnessDiagnostic {
        HarnessDiagnostic {
            steps,
            snapshot: self.snapshot(),
        }
    }

    fn settle(&mut self, token: GenerationToken, settlement: TaskSettlement) {
        if self.settlements.contains_key(&token) {
            return;
        }
        if self.max_settlements == 0 {
            return;
        }
        while self.settlements.len() >= self.max_settlements {
            let Some(oldest) = self.settlement_order.pop_front() else {
                break;
            };
            self.settlements.remove(&oldest);
        }
        self.settlements.insert(token, settlement);
        self.settlement_order.push_back(token);
    }

    fn pop_ready(&mut self, lane_filter: Option<ExecutionLane>) -> Option<ReadyEntry> {
        let mut ready = self.ready.lock();
        let matching_indices = ready
            .entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                lane_filter
                    .is_none_or(|lane| lane == entry.lane)
                    .then_some(index)
            })
            .collect::<Vec<_>>();
        if matching_indices.is_empty() {
            return None;
        }
        let choice = match self.order {
            ReadyOrder::Fifo => 0,
            ReadyOrder::Seeded(_) => {
                self.random_state ^= self.random_state << 13;
                self.random_state ^= self.random_state >> 7;
                self.random_state ^= self.random_state << 17;
                usize::try_from(self.random_state).unwrap_or(usize::MAX) % matching_indices.len()
            }
        };
        ready.remove_at(matching_indices[choice])
    }

    fn prune_wakers(&mut self) {
        self.wakers.retain(|wake| wake.strong_count() > 0);
    }

    fn live_waker_count(&mut self) -> usize {
        self.prune_wakers();
        self.live_waker_count_readonly()
    }

    fn live_waker_count_readonly(&self) -> usize {
        self.wakers.iter().map(ArcWeak::strong_count).sum()
    }
}

impl Drop for DeterministicScheduler {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Debug)]
struct GateState {
    token: GenerationToken,
    released: bool,
    cancelled: bool,
    next_waiter: u64,
    waiters: BTreeMap<u64, Waker>,
}

impl GateState {
    fn new(token: GenerationToken) -> Self {
        Self {
            token,
            released: false,
            cancelled: false,
            next_waiter: 0,
            waiters: BTreeMap::new(),
        }
    }

    fn take_wakers(&mut self) -> Vec<Waker> {
        std::mem::take(&mut self.waiters).into_values().collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CheckpointError {
    #[error("checkpoint token is stale")]
    Stale,

    #[error("checkpoint was cancelled")]
    Cancelled,

    #[error("checkpoint waiter resource limit was exceeded")]
    Resource(ResourceError),
}

/// Tokenized collection of one-shot checkpoint gates.
#[derive(Clone, Debug)]
pub struct CheckpointController {
    token: Rc<Cell<GenerationToken>>,
    cancelled: Rc<Cell<bool>>,
    gates: Rc<RefCell<BTreeMap<Checkpoint, Rc<RefCell<GateState>>>>>,
    tracker: ResourceTracker,
    trace: BoundedTrace,
}

impl CheckpointController {
    pub fn new(token: GenerationToken, tracker: ResourceTracker, trace: BoundedTrace) -> Self {
        Self {
            token: Rc::new(Cell::new(token)),
            cancelled: Rc::new(Cell::new(false)),
            gates: Rc::new(RefCell::new(BTreeMap::new())),
            tracker,
            trace,
        }
    }

    pub fn token(&self) -> GenerationToken {
        self.token.get()
    }

    pub fn wait(&self, token: GenerationToken, checkpoint: Checkpoint) -> CheckpointFuture {
        let current_token = self.token();
        let gate = if token != current_token {
            Rc::new(RefCell::new(GateState::new(current_token)))
        } else if self.cancelled.get() {
            let mut gate = GateState::new(current_token);
            gate.cancelled = true;
            Rc::new(RefCell::new(gate))
        } else {
            self.gates
                .borrow_mut()
                .entry(checkpoint)
                .or_insert_with(|| Rc::new(RefCell::new(GateState::new(current_token))))
                .clone()
        };
        CheckpointFuture {
            token,
            checkpoint,
            gate,
            tracker: self.tracker.clone(),
            trace: self.trace.clone(),
            waiter_id: None,
            waiter_resource: None,
        }
    }

    pub fn release(&self, token: GenerationToken, checkpoint: Checkpoint) -> bool {
        if token != self.token() || self.cancelled.get() {
            self.trace
                .record(TraceEventKind::CompletionRejected, Some(token), None, 0);
            return false;
        }
        let gate = self
            .gates
            .borrow_mut()
            .entry(checkpoint)
            .or_insert_with(|| Rc::new(RefCell::new(GateState::new(token))))
            .clone();
        let wakers = {
            let mut gate = gate.borrow_mut();
            if gate.released || gate.cancelled {
                return false;
            }
            gate.released = true;
            gate.take_wakers()
        };
        self.trace.record(
            TraceEventKind::CheckpointRelease,
            Some(token),
            None,
            checkpoint.ordinal.into(),
        );
        for waker in wakers {
            waker.wake();
        }
        true
    }

    pub fn cancel_all(&self, token: GenerationToken) -> bool {
        if token != self.token() || self.cancelled.replace(true) {
            return false;
        }
        let gates = self.gates.borrow().values().cloned().collect::<Vec<_>>();
        let mut wakers = Vec::new();
        for gate in gates {
            let mut gate = gate.borrow_mut();
            gate.cancelled = true;
            wakers.extend(gate.take_wakers());
        }
        for waker in wakers {
            waker.wake();
        }
        true
    }

    pub fn reset(&self, old_token: GenerationToken, new_token: GenerationToken) -> bool {
        if old_token != self.token() || !new_token.is_newer_generation_of(old_token) {
            return false;
        }
        self.cancelled.set(true);
        let gates = std::mem::take(&mut *self.gates.borrow_mut())
            .into_values()
            .collect::<Vec<_>>();
        let mut wakers = Vec::new();
        for gate in gates {
            let mut gate = gate.borrow_mut();
            gate.cancelled = true;
            wakers.extend(gate.take_wakers());
        }
        self.token.set(new_token);
        self.cancelled.set(false);
        for waker in wakers {
            waker.wake();
        }
        true
    }
}

pub struct CheckpointFuture {
    token: GenerationToken,
    checkpoint: Checkpoint,
    gate: Rc<RefCell<GateState>>,
    tracker: ResourceTracker,
    trace: BoundedTrace,
    waiter_id: Option<u64>,
    waiter_resource: Option<ResourceGuard>,
}

impl std::fmt::Debug for CheckpointFuture {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CheckpointFuture")
            .field("token", &self.token)
            .field("checkpoint", &self.checkpoint)
            .finish_non_exhaustive()
    }
}

impl Future for CheckpointFuture {
    type Output = Result<(), CheckpointError>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let result = {
            let gate = self.gate.borrow();
            if self.token != gate.token {
                Some(Err(CheckpointError::Stale))
            } else if gate.cancelled {
                Some(Err(CheckpointError::Cancelled))
            } else if gate.released {
                Some(Ok(()))
            } else {
                None
            }
        };
        if let Some(result) = result {
            self.remove_waiter();
            return Poll::Ready(result);
        }

        if self.waiter_id.is_none() {
            let resource = match self.tracker.reserve(ResourceKind::GateWaiter, 1, 0) {
                Ok(resource) => resource,
                Err(error) => return Poll::Ready(Err(CheckpointError::Resource(error))),
            };
            let waiter_id = {
                let mut gate = self.gate.borrow_mut();
                let waiter_id = gate.next_waiter;
                let Some(next_waiter) = gate.next_waiter.checked_add(1) else {
                    return Poll::Ready(Err(CheckpointError::Resource(ResourceError::Overflow {
                        kind: ResourceKind::GateWaiter,
                    })));
                };
                gate.next_waiter = next_waiter;
                gate.waiters.insert(waiter_id, context.waker().clone());
                waiter_id
            };
            self.waiter_id = Some(waiter_id);
            self.waiter_resource = Some(resource);
            self.trace.record(
                TraceEventKind::CheckpointWait,
                Some(self.token),
                None,
                self.checkpoint.ordinal.into(),
            );
        } else if let Some(waiter_id) = self.waiter_id {
            self.gate
                .borrow_mut()
                .waiters
                .insert(waiter_id, context.waker().clone());
        }
        Poll::Pending
    }
}

impl CheckpointFuture {
    fn remove_waiter(&mut self) {
        if let Some(waiter_id) = self.waiter_id.take() {
            self.gate.borrow_mut().waiters.remove(&waiter_id);
        }
        self.waiter_resource.take();
    }
}

impl Drop for CheckpointFuture {
    fn drop(&mut self) {
        self.remove_waiter();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("completion source was cancelled")]
pub struct CompletionCancelled;

#[derive(Debug)]
pub enum CompletionAttempt<T> {
    Accepted,
    Stale(T),
    Duplicate(T),
    ReceiverDropped(T),
}

#[derive(Debug)]
struct CompletionState<T> {
    token: GenerationToken,
    outcome: Option<Result<T, CompletionCancelled>>,
    terminal: bool,
    settlement_count: u8,
    receiver_alive: bool,
    waker: Option<Waker>,
    owner: Option<ResourceGuard>,
}

/// Callback-side owner for a tokenized one-shot result.
pub struct CompletionSource<T> {
    state: Rc<RefCell<CompletionState<T>>>,
    trace: BoundedTrace,
}

impl<T> std::fmt::Debug for CompletionSource<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.state.borrow();
        formatter
            .debug_struct("CompletionSource")
            .field("token", &state.token)
            .field("terminal", &state.terminal)
            .finish_non_exhaustive()
    }
}

/// Future-side receiver for a tokenized one-shot result.
pub struct CompletionFuture<T> {
    state: Rc<RefCell<CompletionState<T>>>,
}

impl<T> std::fmt::Debug for CompletionFuture<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.state.borrow();
        formatter
            .debug_struct("CompletionFuture")
            .field("token", &state.token)
            .field("terminal", &state.terminal)
            .finish_non_exhaustive()
    }
}

pub fn token_completion<T>(
    token: GenerationToken,
    retained_bytes: u64,
    tracker: &ResourceTracker,
    trace: BoundedTrace,
) -> Result<(CompletionSource<T>, CompletionFuture<T>), ResourceError> {
    let owner = tracker.reserve(ResourceKind::Owner, 1, retained_bytes)?;
    let state = Rc::new(RefCell::new(CompletionState {
        token,
        outcome: None,
        terminal: false,
        settlement_count: 0,
        receiver_alive: true,
        waker: None,
        owner: Some(owner),
    }));
    Ok((
        CompletionSource {
            state: state.clone(),
            trace,
        },
        CompletionFuture { state },
    ))
}

impl<T> CompletionSource<T> {
    pub fn token(&self) -> GenerationToken {
        self.state.borrow().token
    }

    pub fn complete(&self, token: GenerationToken, value: T) -> CompletionAttempt<T> {
        let waker = {
            let mut state = self.state.borrow_mut();
            if token != state.token {
                drop(state);
                self.trace
                    .record(TraceEventKind::CompletionRejected, Some(token), None, 0);
                return CompletionAttempt::Stale(value);
            }
            if !state.receiver_alive {
                return CompletionAttempt::ReceiverDropped(value);
            }
            if state.terminal {
                return CompletionAttempt::Duplicate(value);
            }
            state.terminal = true;
            state.settlement_count = 1;
            state.outcome = Some(Ok(value));
            state.owner.take();
            state.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
        self.trace
            .record(TraceEventKind::CompletionAccepted, Some(token), None, 0);
        CompletionAttempt::Accepted
    }

    pub fn cancel(&self, token: GenerationToken) -> bool {
        let waker = {
            let mut state = self.state.borrow_mut();
            if token != state.token || state.terminal {
                return false;
            }
            state.terminal = true;
            state.settlement_count = 1;
            state.outcome = Some(Err(CompletionCancelled));
            state.owner.take();
            state.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
        self.trace
            .record(TraceEventKind::Cancel, Some(token), None, 0);
        true
    }

    pub fn settlement_count(&self) -> u8 {
        self.state.borrow().settlement_count
    }
}

impl<T> Drop for CompletionSource<T> {
    fn drop(&mut self) {
        let token = self.state.borrow().token;
        self.cancel(token);
    }
}

impl<T> Future for CompletionFuture<T> {
    type Output = Result<T, CompletionCancelled>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.state.borrow_mut();
        if let Some(outcome) = state.outcome.take() {
            state.receiver_alive = false;
            return Poll::Ready(outcome);
        }
        state.waker = Some(context.waker().clone());
        Poll::Pending
    }
}

impl<T> Drop for CompletionFuture<T> {
    fn drop(&mut self) {
        let mut state = self.state.borrow_mut();
        state.receiver_alive = false;
        state.waker.take();
        state.outcome.take();
        if !state.terminal {
            state.terminal = true;
            state.settlement_count = 1;
            state.owner.take();
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QueueSnapshot {
    pub closed: bool,
    pub retired: bool,
    pub queued_count: u64,
    pub in_flight_count: u64,
    pub retained_count: u64,
    pub retained_bytes: u64,
    pub high_water_count: u64,
    pub high_water_bytes: u64,
    pub waiting_senders: usize,
    pub waiting_receivers: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct QueueDeliveryId {
    pub token: GenerationToken,
    pub sequence: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum QueueAckError {
    #[error("delivery generation is retired")]
    StaleGeneration,

    #[error("ack token does not match the delivery generation")]
    WrongToken,

    #[error("ack identity does not match the delivery")]
    WrongDelivery,

    #[error("delivery was already acknowledged")]
    Duplicate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum QueueResetError {
    #[error("queue reset token is stale")]
    Stale,

    #[error("queue generation is already retired")]
    AlreadyRetired,

    #[error("replacement token is not a newer generation of the same slot")]
    InvalidReplacement,
}

struct QueueEntry<T> {
    identity: QueueDeliveryId,
    item: T,
    bytes: u64,
    resource: ResourceGuard,
}

impl<T> std::fmt::Debug for QueueEntry<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("QueueEntry")
            .field("bytes", &self.bytes)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
struct QueueWaiter {
    id: u64,
    waker: Waker,
}

#[derive(Debug)]
struct QueueState<T> {
    token: GenerationToken,
    max_count: u64,
    max_bytes: u64,
    closed: bool,
    retired: bool,
    entries: VecDeque<QueueEntry<T>>,
    in_flight_count: u64,
    retained_count: u64,
    retained_bytes: u64,
    high_water_count: u64,
    high_water_bytes: u64,
    next_waiter: u64,
    next_delivery_sequence: u64,
    send_waiters: VecDeque<QueueWaiter>,
    recv_waiters: VecDeque<QueueWaiter>,
}

impl<T> QueueState<T> {
    fn can_retain(&self, bytes: u64) -> bool {
        self.retained_count < self.max_count
            && self
                .retained_bytes
                .checked_add(bytes)
                .is_some_and(|next| next <= self.max_bytes)
    }

    fn first_sender_waker(&self) -> Option<Waker> {
        self.send_waiters.front().map(|waiter| waiter.waker.clone())
    }

    fn first_receiver_waker(&self) -> Option<Waker> {
        self.recv_waiters.front().map(|waiter| waiter.waker.clone())
    }

    fn all_wakers(&self) -> Vec<Waker> {
        self.send_waiters
            .iter()
            .chain(&self.recv_waiters)
            .map(|waiter| waiter.waker.clone())
            .collect()
    }
}

/// Bounded local queue whose capacity is held until an explicitly delivered item is acknowledged.
pub struct AckQueue<T> {
    state: Rc<RefCell<QueueState<T>>>,
    tracker: ResourceTracker,
    trace: BoundedTrace,
}

impl<T> Clone for AckQueue<T> {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            tracker: self.tracker.clone(),
            trace: self.trace.clone(),
        }
    }
}

impl<T> std::fmt::Debug for AckQueue<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AckQueue")
            .field("snapshot", &self.snapshot())
            .finish()
    }
}

impl<T> AckQueue<T> {
    pub fn new(
        token: GenerationToken,
        max_count: u64,
        max_bytes: u64,
        tracker: ResourceTracker,
        trace: BoundedTrace,
    ) -> Self {
        Self {
            state: Rc::new(RefCell::new(QueueState {
                token,
                max_count,
                max_bytes,
                closed: false,
                retired: false,
                entries: VecDeque::new(),
                in_flight_count: 0,
                retained_count: 0,
                retained_bytes: 0,
                high_water_count: 0,
                high_water_bytes: 0,
                next_waiter: 0,
                next_delivery_sequence: 0,
                send_waiters: VecDeque::new(),
                recv_waiters: VecDeque::new(),
            })),
            tracker,
            trace,
        }
    }

    pub fn token(&self) -> GenerationToken {
        self.state.borrow().token
    }

    pub fn send(
        &self,
        token: GenerationToken,
        item: T,
        bytes: u64,
    ) -> Result<QueueSendFuture<T>, ResourceError> {
        let owner = self.tracker.reserve(ResourceKind::Owner, 1, bytes)?;
        Ok(QueueSendFuture {
            queue: self.clone(),
            token,
            item: Some(item),
            bytes,
            item_resource: Some(owner),
            waiter_id: None,
            waiter_resource: None,
        })
    }

    pub fn recv(&self, token: GenerationToken) -> QueueRecvFuture<T> {
        QueueRecvFuture {
            queue: self.clone(),
            token,
            waiter_id: None,
            waiter_resource: None,
        }
    }

    pub fn try_recv(
        &self,
        token: GenerationToken,
    ) -> Result<Option<QueueDelivery<T>>, QueueReceiveError> {
        let mut state = self.state.borrow_mut();
        if token != state.token || state.retired {
            return Err(QueueReceiveError::Stale);
        }
        if state.entries.is_empty() {
            return if state.closed {
                Ok(None)
            } else {
                Err(QueueReceiveError::Empty)
            };
        }
        state
            .entries
            .front_mut()
            .expect("queue front was checked")
            .resource
            .reclassify(ResourceKind::InFlightItem)
            .map_err(QueueReceiveError::Resource)?;
        let entry = state.entries.pop_front().expect("queue front was checked");
        state.in_flight_count = state
            .in_flight_count
            .checked_add(1)
            .expect("queue retained-count limit prevents overflow");
        self.trace
            .record(TraceEventKind::QueueDeliver, Some(token), None, entry.bytes);
        Ok(Some(QueueDelivery {
            queue: self.clone(),
            identity: entry.identity,
            item: Some(entry.item),
            bytes: entry.bytes,
            resource: Some(entry.resource),
            acknowledged: false,
        }))
    }

    pub fn close(&self, token: GenerationToken) -> bool {
        let (entries, wakers) = {
            let mut state = self.state.borrow_mut();
            if token != state.token || state.closed || state.retired {
                return false;
            }
            state.closed = true;
            let entries = state.entries.drain(..).collect::<Vec<_>>();
            for entry in &entries {
                state.retained_count = state
                    .retained_count
                    .checked_sub(1)
                    .expect("queued item ownership must balance");
                state.retained_bytes = state
                    .retained_bytes
                    .checked_sub(entry.bytes)
                    .expect("queued item bytes must balance");
            }
            (entries, state.all_wakers())
        };
        drop(entries);
        for waker in wakers {
            waker.wake();
        }
        true
    }

    /// Retires this generation and returns an isolated queue for `new_token`.
    pub fn reset(
        &self,
        old_token: GenerationToken,
        new_token: GenerationToken,
    ) -> Result<Self, QueueResetError> {
        if !new_token.is_newer_generation_of(old_token) {
            return Err(QueueResetError::InvalidReplacement);
        }
        let (max_count, max_bytes, entries, wakers) = {
            let mut state = self.state.borrow_mut();
            if old_token != state.token {
                return Err(QueueResetError::Stale);
            }
            if state.retired {
                return Err(QueueResetError::AlreadyRetired);
            }
            state.closed = true;
            state.retired = true;
            let entries = state.entries.drain(..).collect::<Vec<_>>();
            for entry in &entries {
                state.retained_count = state
                    .retained_count
                    .checked_sub(1)
                    .expect("queued item ownership must balance");
                state.retained_bytes = state
                    .retained_bytes
                    .checked_sub(entry.bytes)
                    .expect("queued item bytes must balance");
            }
            (
                state.max_count,
                state.max_bytes,
                entries,
                state.all_wakers(),
            )
        };
        drop(entries);
        for waker in wakers {
            waker.wake();
        }
        Ok(Self::new(
            new_token,
            max_count,
            max_bytes,
            self.tracker.clone(),
            self.trace.clone(),
        ))
    }

    pub fn snapshot(&self) -> QueueSnapshot {
        let state = self.state.borrow();
        QueueSnapshot {
            closed: state.closed,
            retired: state.retired,
            queued_count: u64::try_from(state.entries.len())
                .expect("supported targets have at most 64-bit usize"),
            in_flight_count: state.in_flight_count,
            retained_count: state.retained_count,
            retained_bytes: state.retained_bytes,
            high_water_count: state.high_water_count,
            high_water_bytes: state.high_water_bytes,
            waiting_senders: state.send_waiters.len(),
            waiting_receivers: state.recv_waiters.len(),
        }
    }

    fn remove_send_waiter(&self, waiter_id: u64) {
        let waker = {
            let mut state = self.state.borrow_mut();
            let was_front = state
                .send_waiters
                .front()
                .is_some_and(|waiter| waiter.id == waiter_id);
            if let Some(index) = state
                .send_waiters
                .iter()
                .position(|waiter| waiter.id == waiter_id)
            {
                state.send_waiters.remove(index);
            }
            was_front.then(|| state.first_sender_waker()).flatten()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    fn remove_recv_waiter(&self, waiter_id: u64) {
        let waker = {
            let mut state = self.state.borrow_mut();
            let was_front = state
                .recv_waiters
                .front()
                .is_some_and(|waiter| waiter.id == waiter_id);
            if let Some(index) = state
                .recv_waiters
                .iter()
                .position(|waiter| waiter.id == waiter_id)
            {
                state.recv_waiters.remove(index);
            }
            was_front.then(|| state.first_receiver_waker()).flatten()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

#[derive(Debug)]
pub enum QueueSendError<T> {
    Stale(T),
    Closed(T),
    ItemTooLarge(T),
    Resource(T, ResourceError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum QueueReceiveError {
    #[error("queue token is stale")]
    Stale,

    #[error("queue is empty")]
    Empty,

    #[error(transparent)]
    Resource(ResourceError),
}

pub struct QueueSendFuture<T> {
    queue: AckQueue<T>,
    token: GenerationToken,
    item: Option<T>,
    bytes: u64,
    item_resource: Option<ResourceGuard>,
    waiter_id: Option<u64>,
    waiter_resource: Option<ResourceGuard>,
}

impl<T> Unpin for QueueSendFuture<T> {}

impl<T> std::fmt::Debug for QueueSendFuture<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("QueueSendFuture")
            .field("token", &self.token)
            .field("bytes", &self.bytes)
            .finish_non_exhaustive()
    }
}

impl<T> Future for QueueSendFuture<T> {
    type Output = Result<(), QueueSendError<T>>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = &mut *self;
        let mut state = this.queue.state.borrow_mut();
        if this.token != state.token || state.retired {
            drop(state);
            this.remove_waiter();
            let item = this.item.take().expect("send future settles once");
            this.item_resource.take();
            return Poll::Ready(Err(QueueSendError::Stale(item)));
        }
        if state.closed {
            drop(state);
            this.remove_waiter();
            let item = this.item.take().expect("send future settles once");
            this.item_resource.take();
            return Poll::Ready(Err(QueueSendError::Closed(item)));
        }
        if this.bytes > state.max_bytes || state.max_count == 0 {
            drop(state);
            this.remove_waiter();
            let item = this.item.take().expect("send future settles once");
            this.item_resource.take();
            return Poll::Ready(Err(QueueSendError::ItemTooLarge(item)));
        }

        if this.waiter_id.is_none() {
            let waiter_resource = match this.queue.tracker.reserve(ResourceKind::QueueWaiter, 1, 0)
            {
                Ok(resource) => resource,
                Err(error) => {
                    drop(state);
                    let item = this.item.take().expect("send future settles once");
                    this.item_resource.take();
                    return Poll::Ready(Err(QueueSendError::Resource(item, error)));
                }
            };
            let waiter_id = state.next_waiter;
            let Some(next_waiter) = state.next_waiter.checked_add(1) else {
                drop(state);
                let item = this.item.take().expect("send future settles once");
                this.item_resource.take();
                return Poll::Ready(Err(QueueSendError::Resource(
                    item,
                    ResourceError::Overflow {
                        kind: ResourceKind::QueueWaiter,
                    },
                )));
            };
            state.next_waiter = next_waiter;
            state.send_waiters.push_back(QueueWaiter {
                id: waiter_id,
                waker: context.waker().clone(),
            });
            this.waiter_id = Some(waiter_id);
            this.waiter_resource = Some(waiter_resource);
        } else if let Some(waiter) = state
            .send_waiters
            .iter_mut()
            .find(|waiter| Some(waiter.id) == this.waiter_id)
        {
            waiter.waker = context.waker().clone();
        }

        let is_front = state
            .send_waiters
            .front()
            .is_some_and(|waiter| Some(waiter.id) == this.waiter_id);
        if !is_front || !state.can_retain(this.bytes) {
            this.queue.trace.record(
                TraceEventKind::QueueWait,
                Some(this.token),
                None,
                this.bytes,
            );
            return Poll::Pending;
        }

        let delivery_sequence = state.next_delivery_sequence;
        let Some(next_delivery_sequence) = delivery_sequence.checked_add(1) else {
            drop(state);
            this.remove_waiter();
            let item = this.item.take().expect("send future settles once");
            this.item_resource.take();
            return Poll::Ready(Err(QueueSendError::Resource(
                item,
                ResourceError::Overflow {
                    kind: ResourceKind::QueuedItem,
                },
            )));
        };

        let resource = this
            .item_resource
            .as_mut()
            .expect("send future owns item accounting");
        if let Err(error) = resource.reclassify(ResourceKind::QueuedItem) {
            drop(state);
            this.remove_waiter();
            let item = this.item.take().expect("send future settles once");
            this.item_resource.take();
            return Poll::Ready(Err(QueueSendError::Resource(item, error)));
        }
        let waiter = state
            .send_waiters
            .pop_front()
            .expect("front waiter was checked");
        assert_eq!(Some(waiter.id), this.waiter_id);
        this.waiter_id = None;
        this.waiter_resource.take();
        let resource = this
            .item_resource
            .take()
            .expect("send future owns item accounting");
        state.next_delivery_sequence = next_delivery_sequence;
        let item = this.item.take().expect("send future settles once");
        state.entries.push_back(QueueEntry {
            identity: QueueDeliveryId {
                token: this.token,
                sequence: delivery_sequence,
            },
            item,
            bytes: this.bytes,
            resource,
        });
        state.retained_count = state
            .retained_count
            .checked_add(1)
            .expect("queue capacity check prevents count overflow");
        state.retained_bytes = state
            .retained_bytes
            .checked_add(this.bytes)
            .expect("queue capacity check prevents byte overflow");
        state.high_water_count = state.high_water_count.max(state.retained_count);
        state.high_water_bytes = state.high_water_bytes.max(state.retained_bytes);
        let receiver_waker = state.first_receiver_waker();
        drop(state);
        this.queue.trace.record(
            TraceEventKind::QueueEnqueue,
            Some(this.token),
            None,
            this.bytes,
        );
        if let Some(waker) = receiver_waker {
            waker.wake();
        }
        Poll::Ready(Ok(()))
    }
}

impl<T> QueueSendFuture<T> {
    fn remove_waiter(&mut self) {
        if let Some(waiter_id) = self.waiter_id.take() {
            self.queue.remove_send_waiter(waiter_id);
        }
        self.waiter_resource.take();
    }
}

impl<T> Drop for QueueSendFuture<T> {
    fn drop(&mut self) {
        self.remove_waiter();
    }
}

pub struct QueueRecvFuture<T> {
    queue: AckQueue<T>,
    token: GenerationToken,
    waiter_id: Option<u64>,
    waiter_resource: Option<ResourceGuard>,
}

impl<T> Unpin for QueueRecvFuture<T> {}

impl<T> std::fmt::Debug for QueueRecvFuture<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("QueueRecvFuture")
            .field("token", &self.token)
            .finish_non_exhaustive()
    }
}

impl<T> Future for QueueRecvFuture<T> {
    type Output = Result<Option<QueueDelivery<T>>, QueueReceiveError>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = &mut *self;
        {
            let state = this.queue.state.borrow();
            if this.token != state.token || state.retired {
                drop(state);
                this.remove_waiter();
                return Poll::Ready(Err(QueueReceiveError::Stale));
            }
            if !state.entries.is_empty() {
                drop(state);
                this.remove_waiter();
                return Poll::Ready(this.queue.try_recv(this.token));
            }
            if state.closed {
                drop(state);
                this.remove_waiter();
                return Poll::Ready(Ok(None));
            }
        }

        let mut state = this.queue.state.borrow_mut();
        if this.waiter_id.is_none() {
            let resource = match this.queue.tracker.reserve(ResourceKind::QueueWaiter, 1, 0) {
                Ok(resource) => resource,
                Err(error) => return Poll::Ready(Err(QueueReceiveError::Resource(error))),
            };
            let waiter_id = state.next_waiter;
            let Some(next_waiter) = state.next_waiter.checked_add(1) else {
                return Poll::Ready(Err(QueueReceiveError::Resource(ResourceError::Overflow {
                    kind: ResourceKind::QueueWaiter,
                })));
            };
            state.next_waiter = next_waiter;
            state.recv_waiters.push_back(QueueWaiter {
                id: waiter_id,
                waker: context.waker().clone(),
            });
            this.waiter_id = Some(waiter_id);
            this.waiter_resource = Some(resource);
        } else if let Some(waiter) = state
            .recv_waiters
            .iter_mut()
            .find(|waiter| Some(waiter.id) == this.waiter_id)
        {
            waiter.waker = context.waker().clone();
        }
        Poll::Pending
    }
}

impl<T> QueueRecvFuture<T> {
    fn remove_waiter(&mut self) {
        if let Some(waiter_id) = self.waiter_id.take() {
            self.queue.remove_recv_waiter(waiter_id);
        }
        self.waiter_resource.take();
    }
}

impl<T> Drop for QueueRecvFuture<T> {
    fn drop(&mut self) {
        self.remove_waiter();
    }
}

pub struct QueueDelivery<T> {
    queue: AckQueue<T>,
    identity: QueueDeliveryId,
    item: Option<T>,
    bytes: u64,
    resource: Option<ResourceGuard>,
    acknowledged: bool,
}

impl<T> std::fmt::Debug for QueueDelivery<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("QueueDelivery")
            .field("identity", &self.identity)
            .field("bytes", &self.bytes)
            .field("acknowledged", &self.acknowledged)
            .finish_non_exhaustive()
    }
}

impl<T> QueueDelivery<T> {
    pub fn identity(&self) -> QueueDeliveryId {
        self.identity
    }

    pub fn item(&self) -> &T {
        self.item.as_ref().expect("delivery item is live")
    }

    pub fn item_mut(&mut self) -> &mut T {
        self.item.as_mut().expect("delivery item is live")
    }

    pub fn ack(
        &mut self,
        token: GenerationToken,
        identity: QueueDeliveryId,
    ) -> Result<T, QueueAckError> {
        if self.acknowledged || self.resource.is_none() {
            return Err(QueueAckError::Duplicate);
        }
        if token != self.identity.token {
            return Err(QueueAckError::WrongToken);
        }
        if identity != self.identity {
            return Err(QueueAckError::WrongDelivery);
        }
        {
            let state = self.queue.state.borrow();
            if state.token != self.identity.token || state.retired {
                return Err(QueueAckError::StaleGeneration);
            }
        }
        self.acknowledged = true;
        let item = self.item.take().expect("delivery item is live");
        self.release_inner();
        Ok(item)
    }

    fn release_inner(&mut self) {
        let Some(resource) = self.resource.take() else {
            return;
        };
        let sender_waker = {
            let mut state = self.queue.state.borrow_mut();
            state.in_flight_count = state
                .in_flight_count
                .checked_sub(1)
                .expect("delivery ownership must balance");
            state.retained_count = state
                .retained_count
                .checked_sub(1)
                .expect("retained delivery ownership must balance");
            state.retained_bytes = state
                .retained_bytes
                .checked_sub(self.bytes)
                .expect("retained delivery bytes must balance");
            state.first_sender_waker()
        };
        drop(resource);
        self.queue.trace.record(
            if self.acknowledged {
                TraceEventKind::QueueAck
            } else {
                TraceEventKind::QueueDrop
            },
            Some(self.identity.token),
            None,
            self.bytes,
        );
        if let Some(waker) = sender_waker {
            waker.wake();
        }
    }
}

impl<T> Drop for QueueDelivery<T> {
    fn drop(&mut self) {
        self.release_inner();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LeaseError {
    #[error("lease pool token is stale")]
    Stale,

    #[error("lease pool is closed")]
    Closed,

    #[error("lease pool resource limit was exceeded")]
    Resource(ResourceError),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LeaseSnapshot {
    pub accepting: bool,
    pub retired: bool,
    pub live_leases: u64,
    pub waiting_drains: usize,
}

#[derive(Debug)]
struct LeaseState {
    token: GenerationToken,
    accepting: bool,
    retired: bool,
    live_leases: u64,
    next_waiter: u64,
    waiters: BTreeMap<u64, Waker>,
}

impl LeaseState {
    fn new(token: GenerationToken) -> Self {
        Self {
            token,
            accepting: true,
            retired: false,
            live_leases: 0,
            next_waiter: 0,
            waiters: BTreeMap::new(),
        }
    }

    fn take_wakers(&mut self) -> Vec<Waker> {
        std::mem::take(&mut self.waiters).into_values().collect()
    }
}

/// Generation-aware query lease ownership with an explicit close-and-drain boundary.
#[derive(Clone, Debug)]
pub struct LeasePool {
    current: Rc<RefCell<Rc<RefCell<LeaseState>>>>,
    tracker: ResourceTracker,
    trace: BoundedTrace,
}

impl LeasePool {
    pub fn new(token: GenerationToken, tracker: ResourceTracker, trace: BoundedTrace) -> Self {
        Self {
            current: Rc::new(RefCell::new(Rc::new(RefCell::new(LeaseState::new(token))))),
            tracker,
            trace,
        }
    }

    pub fn token(&self) -> GenerationToken {
        self.current.borrow().borrow().token
    }

    pub fn acquire(&self, token: GenerationToken) -> Result<LeaseGuard, LeaseError> {
        let state = self.current.borrow().clone();
        {
            let state_ref = state.borrow();
            if state_ref.token != token || state_ref.retired {
                return Err(LeaseError::Stale);
            }
            if !state_ref.accepting {
                return Err(LeaseError::Closed);
            }
            state_ref
                .live_leases
                .checked_add(1)
                .ok_or(LeaseError::Resource(ResourceError::Overflow {
                    kind: ResourceKind::Lease,
                }))?;
        }
        let resource = self
            .tracker
            .reserve(ResourceKind::Lease, 1, 0)
            .map_err(LeaseError::Resource)?;
        let mut state_ref = state.borrow_mut();
        state_ref.live_leases = state_ref
            .live_leases
            .checked_add(1)
            .expect("lease count was checked before resource admission");
        drop(state_ref);
        self.trace
            .record(TraceEventKind::LeaseAcquire, Some(token), None, 0);
        Ok(LeaseGuard {
            state,
            resource: Some(resource),
            trace: self.trace.clone(),
        })
    }

    pub fn close(&self, token: GenerationToken) -> bool {
        let state = self.current.borrow().clone();
        let wakers = {
            let mut state = state.borrow_mut();
            if state.token != token || state.retired || !state.accepting {
                return false;
            }
            state.accepting = false;
            if state.live_leases == 0 {
                state.take_wakers()
            } else {
                Vec::new()
            }
        };
        for waker in wakers {
            waker.wake();
        }
        true
    }

    pub fn drain(&self, token: GenerationToken) -> LeaseDrainFuture {
        LeaseDrainFuture {
            token,
            state: self.current.borrow().clone(),
            tracker: self.tracker.clone(),
            trace: self.trace.clone(),
            waiter_id: None,
            waiter_resource: None,
        }
    }

    /// Retires the old generation without allowing its late lease drops to affect the new one.
    pub fn reset(&self, old_token: GenerationToken, new_token: GenerationToken) -> bool {
        if !new_token.is_newer_generation_of(old_token) {
            return false;
        }
        let old_state = self.current.borrow().clone();
        let wakers = {
            let mut old = old_state.borrow_mut();
            if old.token != old_token || old.retired {
                return false;
            }
            old.accepting = false;
            old.retired = true;
            old.take_wakers()
        };
        *self.current.borrow_mut() = Rc::new(RefCell::new(LeaseState::new(new_token)));
        for waker in wakers {
            waker.wake();
        }
        true
    }

    pub fn snapshot(&self) -> LeaseSnapshot {
        let state = self.current.borrow().clone();
        let state = state.borrow();
        LeaseSnapshot {
            accepting: state.accepting,
            retired: state.retired,
            live_leases: state.live_leases,
            waiting_drains: state.waiters.len(),
        }
    }
}

pub struct LeaseGuard {
    state: Rc<RefCell<LeaseState>>,
    resource: Option<ResourceGuard>,
    trace: BoundedTrace,
}

impl std::fmt::Debug for LeaseGuard {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("LeaseGuard").finish_non_exhaustive()
    }
}

impl Drop for LeaseGuard {
    fn drop(&mut self) {
        let Some(resource) = self.resource.take() else {
            return;
        };
        let (token, live_leases, wakers) = {
            let mut state = self.state.borrow_mut();
            state.live_leases = state
                .live_leases
                .checked_sub(1)
                .expect("lease ownership must balance");
            let wakers = if state.live_leases == 0 {
                state.take_wakers()
            } else {
                Vec::new()
            };
            (state.token, state.live_leases, wakers)
        };
        drop(resource);
        self.trace
            .record(TraceEventKind::LeaseRelease, Some(token), None, live_leases);
        for waker in wakers {
            waker.wake();
        }
    }
}

pub struct LeaseDrainFuture {
    token: GenerationToken,
    state: Rc<RefCell<LeaseState>>,
    tracker: ResourceTracker,
    trace: BoundedTrace,
    waiter_id: Option<u64>,
    waiter_resource: Option<ResourceGuard>,
}

impl Unpin for LeaseDrainFuture {}

impl std::fmt::Debug for LeaseDrainFuture {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LeaseDrainFuture")
            .field("token", &self.token)
            .finish_non_exhaustive()
    }
}

impl Future for LeaseDrainFuture {
    type Output = Result<(), LeaseError>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let result = {
            let state = this.state.borrow();
            if state.token != this.token || state.retired {
                Some(Err(LeaseError::Stale))
            } else if state.accepting {
                None
            } else if state.live_leases == 0 {
                Some(Ok(()))
            } else {
                None
            }
        };
        if let Some(result) = result {
            this.remove_waiter();
            if result.is_ok() {
                this.trace
                    .record(TraceEventKind::LeaseDrained, Some(this.token), None, 0);
            }
            return Poll::Ready(result);
        }

        if this.waiter_id.is_none() {
            let resource = match this.tracker.reserve(ResourceKind::GateWaiter, 1, 0) {
                Ok(resource) => resource,
                Err(error) => return Poll::Ready(Err(LeaseError::Resource(error))),
            };
            let mut state = this.state.borrow_mut();
            let waiter_id = state.next_waiter;
            let Some(next_waiter) = state.next_waiter.checked_add(1) else {
                return Poll::Ready(Err(LeaseError::Resource(ResourceError::Overflow {
                    kind: ResourceKind::GateWaiter,
                })));
            };
            state.next_waiter = next_waiter;
            state.waiters.insert(waiter_id, context.waker().clone());
            this.waiter_id = Some(waiter_id);
            this.waiter_resource = Some(resource);
        } else if let Some(waiter_id) = this.waiter_id {
            this.state
                .borrow_mut()
                .waiters
                .insert(waiter_id, context.waker().clone());
        }
        Poll::Pending
    }
}

impl LeaseDrainFuture {
    fn remove_waiter(&mut self) {
        if let Some(waiter_id) = self.waiter_id.take() {
            self.state.borrow_mut().waiters.remove(&waiter_id);
        }
        self.waiter_resource.take();
    }
}

impl Drop for LeaseDrainFuture {
    fn drop(&mut self) {
        self.remove_waiter();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ClockError {
    #[error("clock value overflowed")]
    Overflow,

    #[error("clock was cancelled")]
    Cancelled,

    #[error("clock timer resource limit was exceeded")]
    Resource(ResourceError),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClockSnapshot {
    pub monotonic_millis: u64,
    pub wall_millis: i64,
    pub pending_timers: usize,
}

#[derive(Debug)]
struct TimerEntry {
    deadline_millis: u64,
    waker: Waker,
}

#[derive(Debug, Default)]
struct ClockState {
    monotonic_millis: u64,
    wall_millis: i64,
    next_timer: u64,
    timers: BTreeMap<u64, TimerEntry>,
    cancelled: bool,
}

/// Manual clock with independently controlled monotonic and wall-clock domains.
#[derive(Clone, Debug)]
pub struct ManualClock {
    state: Rc<RefCell<ClockState>>,
    tracker: ResourceTracker,
    trace: BoundedTrace,
}

impl ManualClock {
    pub fn new(tracker: ResourceTracker, trace: BoundedTrace) -> Self {
        Self {
            state: Rc::new(RefCell::new(ClockState::default())),
            tracker,
            trace,
        }
    }

    pub fn sleep(&self, duration: Duration) -> Result<ManualSleep, ClockError> {
        let duration_millis =
            u64::try_from(duration.as_millis()).map_err(|_error| ClockError::Overflow)?;
        let deadline_millis = self
            .state
            .borrow()
            .monotonic_millis
            .checked_add(duration_millis)
            .ok_or(ClockError::Overflow)?;
        Ok(ManualSleep {
            clock: self.clone(),
            deadline_millis,
            timer_id: None,
            resource: None,
        })
    }

    pub fn advance_monotonic(&self, duration: Duration) -> Result<u64, ClockError> {
        let delta = u64::try_from(duration.as_millis()).map_err(|_error| ClockError::Overflow)?;
        let (now, due) = {
            let mut state = self.state.borrow_mut();
            state.monotonic_millis = state
                .monotonic_millis
                .checked_add(delta)
                .ok_or(ClockError::Overflow)?;
            let now = state.monotonic_millis;
            let due = state
                .timers
                .values()
                .filter(|timer| timer.deadline_millis <= now)
                .map(|timer| timer.waker.clone())
                .collect::<Vec<_>>();
            (now, due)
        };
        for waker in due {
            waker.wake();
        }
        self.trace
            .record(TraceEventKind::ClockAdvance, None, None, now);
        Ok(now)
    }

    pub fn advance_wall(&self, delta_millis: i64) -> Result<i64, ClockError> {
        let wall = {
            let mut state = self.state.borrow_mut();
            state.wall_millis = state
                .wall_millis
                .checked_add(delta_millis)
                .ok_or(ClockError::Overflow)?;
            state.wall_millis
        };
        self.trace.record(
            TraceEventKind::ClockAdvance,
            None,
            None,
            wall.unsigned_abs(),
        );
        Ok(wall)
    }

    pub fn cancel_all(&self) {
        let wakers = {
            let mut state = self.state.borrow_mut();
            state.cancelled = true;
            state
                .timers
                .values()
                .map(|timer| timer.waker.clone())
                .collect::<Vec<_>>()
        };
        for waker in wakers {
            waker.wake();
        }
    }

    pub fn snapshot(&self) -> ClockSnapshot {
        let state = self.state.borrow();
        ClockSnapshot {
            monotonic_millis: state.monotonic_millis,
            wall_millis: state.wall_millis,
            pending_timers: state.timers.len(),
        }
    }
}

pub struct ManualSleep {
    clock: ManualClock,
    deadline_millis: u64,
    timer_id: Option<u64>,
    resource: Option<ResourceGuard>,
}

impl Unpin for ManualSleep {}

impl std::fmt::Debug for ManualSleep {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ManualSleep")
            .field("deadline_millis", &self.deadline_millis)
            .finish_non_exhaustive()
    }
}

impl Future for ManualSleep {
    type Output = Result<(), ClockError>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let (cancelled, due) = {
            let state = this.clock.state.borrow();
            (
                state.cancelled,
                state.monotonic_millis >= this.deadline_millis,
            )
        };
        if cancelled || due {
            this.remove_timer();
            return if cancelled {
                Poll::Ready(Err(ClockError::Cancelled))
            } else {
                Poll::Ready(Ok(()))
            };
        }
        if let Some(timer_id) = this.timer_id {
            if let Some(timer) = this.clock.state.borrow_mut().timers.get_mut(&timer_id) {
                timer.waker = context.waker().clone();
            }
            return Poll::Pending;
        }
        let resource = match this.clock.tracker.reserve(ResourceKind::Timer, 1, 0) {
            Ok(resource) => resource,
            Err(error) => return Poll::Ready(Err(ClockError::Resource(error))),
        };
        let timer_id = {
            let mut state = this.clock.state.borrow_mut();
            let timer_id = state.next_timer;
            let Some(next_timer) = state.next_timer.checked_add(1) else {
                return Poll::Ready(Err(ClockError::Overflow));
            };
            state.next_timer = next_timer;
            state.timers.insert(
                timer_id,
                TimerEntry {
                    deadline_millis: this.deadline_millis,
                    waker: context.waker().clone(),
                },
            );
            timer_id
        };
        this.timer_id = Some(timer_id);
        this.resource = Some(resource);
        Poll::Pending
    }
}

impl ManualSleep {
    fn remove_timer(&mut self) {
        if let Some(timer_id) = self.timer_id.take() {
            self.clock.state.borrow_mut().timers.remove(&timer_id);
        }
        self.resource.take();
    }
}

impl Drop for ManualSleep {
    fn drop(&mut self) {
        self.remove_timer();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageSignal {
    VisibilityVisible,
    VisibilityHidden,
    PageHide { persisted: bool },
    PageShow { persisted: bool },
    Freeze,
    Resume,
    Stop,
}

impl PageSignal {
    const fn trace_value(self) -> u64 {
        match self {
            Self::VisibilityVisible => 0,
            Self::VisibilityHidden => 1,
            Self::PageHide { persisted: false } => 2,
            Self::PageHide { persisted: true } => 3,
            Self::PageShow { persisted: false } => 4,
            Self::PageShow { persisted: true } => 5,
            Self::Freeze => 6,
            Self::Resume => 7,
            Self::Stop => 8,
        }
    }
}

/// Bounded page lifecycle signal transport. Policy remains in the code under test.
#[derive(Clone, Debug)]
pub struct PageSignalQueue(AckQueue<PageSignal>);

impl PageSignalQueue {
    pub fn new(
        token: GenerationToken,
        max_signals: u64,
        tracker: ResourceTracker,
        trace: BoundedTrace,
    ) -> Self {
        Self(AckQueue::new(
            token,
            max_signals,
            max_signals,
            tracker,
            trace,
        ))
    }

    pub fn send(
        &self,
        token: GenerationToken,
        signal: PageSignal,
    ) -> Result<QueueSendFuture<PageSignal>, ResourceError> {
        self.0.trace.record(
            TraceEventKind::PageSignal,
            Some(token),
            None,
            signal.trace_value(),
        );
        self.0.send(token, signal, 1)
    }

    pub fn recv(&self, token: GenerationToken) -> QueueRecvFuture<PageSignal> {
        self.0.recv(token)
    }

    pub fn try_recv(
        &self,
        token: GenerationToken,
    ) -> Result<Option<QueueDelivery<PageSignal>>, QueueReceiveError> {
        self.0.try_recv(token)
    }

    pub fn close(&self, token: GenerationToken) -> bool {
        self.0.close(token)
    }

    pub fn snapshot(&self) -> QueueSnapshot {
        self.0.snapshot()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameSnapshot {
    pub active: bool,
    pub repaint_pending: bool,
    pub repaint_requests: u64,
    pub stale_repaint_requests: u64,
    pub animation_frame_allowance: u64,
    pub viewer_frame_allowance: u64,
}

#[derive(Debug)]
struct FrameState {
    token: GenerationToken,
    active: bool,
    repaint_pending: bool,
    repaint_requests: u64,
    stale_repaint_requests: u64,
    animation_frame_allowance: u64,
    viewer_frame_allowance: u64,
}

/// Separately grants browser RAF and Viewer-frame progress and models repaint latching.
#[derive(Clone, Debug)]
pub struct FrameController {
    state: Rc<RefCell<FrameState>>,
    tracker: ResourceTracker,
    trace: BoundedTrace,
}

impl FrameController {
    pub fn new(token: GenerationToken, tracker: ResourceTracker, trace: BoundedTrace) -> Self {
        Self {
            state: Rc::new(RefCell::new(FrameState {
                token,
                active: true,
                repaint_pending: false,
                repaint_requests: 0,
                stale_repaint_requests: 0,
                animation_frame_allowance: 0,
                viewer_frame_allowance: 0,
            })),
            tracker,
            trace,
        }
    }

    pub fn repaint_requester(
        &self,
        token: GenerationToken,
    ) -> Result<RepaintRequester, ResourceError> {
        Ok(RepaintRequester {
            state: self.state.clone(),
            token,
            owner: Some(self.tracker.reserve(ResourceKind::Owner, 1, 0)?),
            trace: self.trace.clone(),
        })
    }

    pub fn grant_animation_frames(&self, token: GenerationToken, count: u64) -> bool {
        let mut state = self.state.borrow_mut();
        if !state.active || state.token != token {
            return false;
        }
        let Some(next) = state.animation_frame_allowance.checked_add(count) else {
            return false;
        };
        state.animation_frame_allowance = next;
        self.trace
            .record(TraceEventKind::FrameAllowance, Some(token), None, count);
        true
    }

    pub fn grant_viewer_frames(&self, token: GenerationToken, count: u64) -> bool {
        let mut state = self.state.borrow_mut();
        if !state.active || state.token != token {
            return false;
        }
        let Some(next) = state.viewer_frame_allowance.checked_add(count) else {
            return false;
        };
        state.viewer_frame_allowance = next;
        self.trace
            .record(TraceEventKind::FrameAllowance, Some(token), None, count);
        true
    }

    pub fn consume_animation_frame(&self, token: GenerationToken) -> bool {
        self.consume(token, true)
    }

    pub fn consume_viewer_frame(&self, token: GenerationToken) -> bool {
        self.consume(token, false)
    }

    fn consume(&self, token: GenerationToken, animation: bool) -> bool {
        let mut state = self.state.borrow_mut();
        if !state.active || state.token != token {
            return false;
        }
        let allowance = if animation {
            &mut state.animation_frame_allowance
        } else {
            &mut state.viewer_frame_allowance
        };
        let Some(next) = allowance.checked_sub(1) else {
            return false;
        };
        *allowance = next;
        if animation {
            state.repaint_pending = false;
        }
        self.trace.record(
            TraceEventKind::FrameConsumed,
            Some(token),
            None,
            u64::from(animation),
        );
        true
    }

    pub fn reset(&self, old_token: GenerationToken, new_token: GenerationToken) -> bool {
        let mut state = self.state.borrow_mut();
        if !state.active || state.token != old_token || !new_token.is_newer_generation_of(old_token)
        {
            return false;
        }
        *state = FrameState {
            token: new_token,
            active: true,
            repaint_pending: false,
            repaint_requests: 0,
            stale_repaint_requests: 0,
            animation_frame_allowance: 0,
            viewer_frame_allowance: 0,
        };
        true
    }

    pub fn stop(&self, token: GenerationToken) -> bool {
        let mut state = self.state.borrow_mut();
        if state.token != token || !state.active {
            return false;
        }
        state.active = false;
        state.repaint_pending = false;
        state.animation_frame_allowance = 0;
        state.viewer_frame_allowance = 0;
        true
    }

    pub fn snapshot(&self) -> FrameSnapshot {
        let state = self.state.borrow();
        FrameSnapshot {
            active: state.active,
            repaint_pending: state.repaint_pending,
            repaint_requests: state.repaint_requests,
            stale_repaint_requests: state.stale_repaint_requests,
            animation_frame_allowance: state.animation_frame_allowance,
            viewer_frame_allowance: state.viewer_frame_allowance,
        }
    }
}

/// Production-callback-shaped repaint handle with explicit owner retention.
pub struct RepaintRequester {
    state: Rc<RefCell<FrameState>>,
    token: GenerationToken,
    owner: Option<ResourceGuard>,
    trace: BoundedTrace,
}

impl std::fmt::Debug for RepaintRequester {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RepaintRequester")
            .field("token", &self.token)
            .finish_non_exhaustive()
    }
}

impl RepaintRequester {
    pub fn request(&self) -> bool {
        let mut state = self.state.borrow_mut();
        if !state.active || state.token != self.token {
            if let Some(next) = state.stale_repaint_requests.checked_add(1) {
                state.stale_repaint_requests = next;
            }
            return false;
        }
        let Some(next_requests) = state.repaint_requests.checked_add(1) else {
            return false;
        };
        state.repaint_pending = true;
        state.repaint_requests = next_requests;
        self.trace.record(
            TraceEventKind::RepaintRequest,
            Some(self.token),
            None,
            state.repaint_requests,
        );
        true
    }

    pub fn release(mut self) {
        self.owner.take();
    }
}

/// Common composition for deterministic tests; it intentionally contains no product state machine.
#[derive(Debug)]
pub struct DeterministicWebHarness {
    pub scheduler: DeterministicScheduler,
    pub token: GenerationToken,
    pub checkpoints: CheckpointController,
    pub leases: LeasePool,
    pub clock: ManualClock,
    pub page_signals: PageSignalQueue,
    pub frames: FrameController,
    instance_slots: GenerationSlots,
}

impl DeterministicWebHarness {
    pub fn new(order: ReadyOrder) -> Self {
        let scheduler = DeterministicScheduler::new(order);
        let tracker = scheduler.tracker();
        let trace = scheduler.trace();
        let mut instance_slots = GenerationSlots::default();
        let token = instance_slots
            .claim(0)
            .expect("fresh harness instance slot must be available");
        Self {
            checkpoints: CheckpointController::new(token, tracker.clone(), trace.clone()),
            leases: LeasePool::new(token, tracker.clone(), trace.clone()),
            clock: ManualClock::new(tracker.clone(), trace.clone()),
            page_signals: PageSignalQueue::new(token, 32, tracker.clone(), trace.clone()),
            frames: FrameController::new(token, tracker, trace),
            scheduler,
            token,
            instance_slots,
        }
    }

    pub fn stop(&mut self) {
        self.checkpoints.cancel_all(self.token);
        self.leases.close(self.token);
        self.clock.cancel_all();
        self.page_signals.close(self.token);
        self.frames.stop(self.token);
        self.scheduler.stop();
        self.instance_slots.release(self.token);
    }

    pub fn snapshot(&self) -> HarnessSnapshot {
        self.scheduler.snapshot()
    }

    #[track_caller]
    pub fn assert_clean(&self) {
        self.scheduler.assert_clean();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::future::{pending, poll_fn};
    use std::rc::Rc;
    use std::sync::Arc;
    use std::task::Poll;
    use std::time::Duration;

    use futures::FutureExt as _;

    use super::*;

    thread_local! {
        static REENTRANT_ACTION: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
    }

    #[derive(Debug)]
    struct ReentrantWake;

    impl ArcWake for ReentrantWake {
        fn wake_by_ref(_arc_self: &Arc<Self>) {
            if let Some(action) = REENTRANT_ACTION.with(|slot| slot.borrow_mut().take()) {
                action();
            }
        }
    }

    fn reentrant_waker(action: impl FnOnce() + 'static) -> Waker {
        REENTRANT_ACTION.with(|slot| {
            assert!(slot.borrow().is_none());
            *slot.borrow_mut() = Some(Box::new(action));
        });
        waker(Arc::new(ReentrantWake))
    }

    fn assert_no_reentrant_action() {
        REENTRANT_ACTION.with(|slot| assert!(slot.borrow().is_none()));
    }

    fn token(slot: u32, generation: u64) -> GenerationToken {
        GenerationToken { slot, generation }
    }

    fn assert_resources_empty(tracker: &ResourceTracker) {
        let snapshot = tracker.snapshot();
        for kind in ResourceKind::ALL {
            assert_eq!(
                snapshot.usage(kind).current_count,
                0,
                "live count for {kind:?}"
            );
            assert_eq!(
                snapshot.usage(kind).current_bytes,
                0,
                "live bytes for {kind:?}"
            );
        }
    }

    #[test]
    fn generation_slots_reject_stale_release_after_reuse() {
        let mut slots = GenerationSlots::default();
        let first = slots.claim(7).unwrap();
        assert!(matches!(
            slots.claim(7),
            Err(GenerationSlotError::Occupied(current)) if current == first
        ));
        assert!(slots.release(first));
        let second = slots.claim(7).unwrap();
        assert_eq!(second.generation, first.generation + 1);
        assert!(!slots.release(first));
        assert!(slots.is_current(second));
        assert!(slots.release(second));
        assert_eq!(slots.occupied_count(), 0);
    }

    #[test]
    fn failed_spawn_admission_does_not_consume_slot_or_generation() {
        let mut scheduler = DeterministicScheduler::default();
        let tracker = scheduler.tracker();
        tracker.set_limit(
            ResourceKind::Task,
            ResourceLimit {
                max_count: 0,
                max_bytes: 0,
            },
        );
        assert!(matches!(
            scheduler.spawn(ExecutionLane::ViewerFrame, async {}),
            Err(SchedulerError::Resource(ResourceError::LimitExceeded {
                kind: ResourceKind::Task
            }))
        ));
        assert_eq!(scheduler.next_automatic_slot, Some(0));
        assert!(scheduler.slots.next_generation.is_empty());
        tracker.set_limit(ResourceKind::Task, ResourceLimit::UNLIMITED);
        let retry = scheduler
            .spawn(ExecutionLane::ViewerFrame, async {})
            .unwrap();
        assert_eq!(retry, token(0, 1));
        assert!(scheduler.cancel(retry));
        scheduler.assert_clean();

        let mut scheduler = DeterministicScheduler::default();
        let tracker = scheduler.tracker();
        tracker.set_limit(
            ResourceKind::Task,
            ResourceLimit {
                max_count: 0,
                max_bytes: 0,
            },
        );
        assert!(matches!(
            scheduler.spawn_in_slot(17, ExecutionLane::StoreCallback, async {}),
            Err(SchedulerError::Resource(ResourceError::LimitExceeded {
                kind: ResourceKind::Task
            }))
        ));
        assert!(!scheduler.slots.next_generation.contains_key(&17));
        tracker.set_limit(ResourceKind::Task, ResourceLimit::UNLIMITED);
        let retry = scheduler
            .spawn_in_slot(17, ExecutionLane::StoreCallback, async {})
            .unwrap();
        assert_eq!(retry, token(17, 1));
        assert!(scheduler.cancel(retry));
        scheduler.assert_clean();

        let mut scheduler = DeterministicScheduler::default();
        let tracker = scheduler.tracker();
        tracker.set_limit(
            ResourceKind::Task,
            ResourceLimit {
                max_count: 0,
                max_bytes: 0,
            },
        );
        assert!(matches!(
            scheduler.spawn_abortable(ExecutionLane::NetworkCompletion, pending()),
            Err(SchedulerError::Resource(ResourceError::LimitExceeded {
                kind: ResourceKind::Task
            }))
        ));
        assert_eq!(scheduler.next_automatic_slot, Some(0));
        assert!(scheduler.slots.next_generation.is_empty());
        tracker.set_limit(ResourceKind::Task, ResourceLimit::UNLIMITED);
        let retry = scheduler
            .spawn(ExecutionLane::NetworkCompletion, async {})
            .unwrap();
        assert_eq!(retry, token(0, 1));
        assert!(scheduler.cancel(retry));
        scheduler.assert_clean();
    }

    fn scheduler_order(order: ReadyOrder) -> Vec<u8> {
        let observed = Rc::new(RefCell::new(Vec::new()));
        let mut scheduler = DeterministicScheduler::new(order);
        for id in 0..8 {
            let observed = observed.clone();
            scheduler
                .spawn(ExecutionLane::ViewerFrame, async move {
                    observed.borrow_mut().push(id);
                })
                .unwrap();
        }
        scheduler.run_until_idle(8).unwrap();
        let result = observed.borrow().clone();
        scheduler.assert_clean();
        result
    }

    #[test]
    fn scheduler_lane_steps_and_seeded_order_are_deterministic() {
        assert_eq!(
            scheduler_order(ReadyOrder::Fifo),
            (0..8).collect::<Vec<_>>()
        );
        let first = scheduler_order(ReadyOrder::Seeded(0x5eed));
        let second = scheduler_order(ReadyOrder::Seeded(0x5eed));
        assert_eq!(first, second);
        assert_ne!(first, (0..8).collect::<Vec<_>>());

        let network_done = Rc::new(Cell::new(false));
        let frame_done = Rc::new(Cell::new(false));
        let mut scheduler = DeterministicScheduler::default();
        scheduler
            .spawn(ExecutionLane::NetworkCompletion, {
                let done = network_done.clone();
                async move { done.set(true) }
            })
            .unwrap();
        scheduler
            .spawn(ExecutionLane::ViewerFrame, {
                let done = frame_done.clone();
                async move { done.set(true) }
            })
            .unwrap();
        scheduler.step_lane(ExecutionLane::ViewerFrame).unwrap();
        assert!(frame_done.get());
        assert!(!network_done.get());
        scheduler
            .step_lane(ExecutionLane::NetworkCompletion)
            .unwrap();
        scheduler.assert_clean();

        let lanes = [
            ExecutionLane::NetworkCompletion,
            ExecutionLane::BrowserMicrotask,
            ExecutionLane::BrowserMacrotask,
            ExecutionLane::AnimationFrame,
            ExecutionLane::ViewerFrame,
            ExecutionLane::StoreCallback,
        ];
        let observed = Rc::new(RefCell::new(Vec::new()));
        let mut scheduler = DeterministicScheduler::default();
        for lane in lanes {
            let observed = observed.clone();
            scheduler
                .spawn(lane, async move { observed.borrow_mut().push(lane) })
                .unwrap();
        }
        for lane in lanes.into_iter().rev() {
            scheduler.step_lane(lane).unwrap();
        }
        assert_eq!(
            observed.borrow().as_slice(),
            lanes.into_iter().rev().collect::<Vec<_>>()
        );
        scheduler.assert_clean();
    }

    #[test]
    fn stale_waker_cannot_wake_reused_task_slot() {
        let captured = Rc::new(RefCell::new(None::<Waker>));
        let mut scheduler = DeterministicScheduler::default();
        let old = scheduler
            .spawn_in_slot(11, ExecutionLane::NetworkCompletion, {
                let captured = captured.clone();
                poll_fn(move |context| {
                    *captured.borrow_mut() = Some(context.waker().clone());
                    Poll::<()>::Pending
                })
            })
            .unwrap();
        assert!(matches!(
            scheduler.step().unwrap(),
            StepOutcome::PolledPending(current) if current == old
        ));
        assert!(scheduler.cancel(old));
        let new_ran = Rc::new(Cell::new(false));
        let new = scheduler
            .spawn_in_slot(11, ExecutionLane::NetworkCompletion, {
                let new_ran = new_ran.clone();
                async move { new_ran.set(true) }
            })
            .unwrap();
        captured.borrow_mut().take().unwrap().wake();
        assert!(matches!(
            scheduler.step().unwrap(),
            StepOutcome::Completed(current) if current == new
        ));
        assert!(new_ran.get());
        assert!(matches!(
            scheduler.step().unwrap(),
            StepOutcome::StaleWakeDropped(current) if current == old
        ));
        assert!(
            scheduler
                .snapshot()
                .trace
                .events
                .iter()
                .any(|event| event.kind == TraceEventKind::StaleWakeDropped)
        );
        scheduler.assert_clean();
    }

    #[test]
    fn retained_waker_cap_is_checked_after_poll_without_lost_wakes() {
        let retained = Rc::new(RefCell::new(Vec::<Waker>::new()));
        let mut scheduler =
            DeterministicScheduler::with_limits(ReadyOrder::Fifo, 16, 2, 64, 16 * 1024);
        let over_limit_task = scheduler
            .spawn(ExecutionLane::NetworkCompletion, {
                let retained = retained.clone();
                poll_fn(move |context| {
                    let mut retained = retained.borrow_mut();
                    retained.push(context.waker().clone());
                    retained.push(context.waker().clone());
                    retained.push(context.waker().clone());
                    Poll::<()>::Pending
                })
            })
            .unwrap();
        assert!(matches!(
            scheduler.step(),
            Err(SchedulerError::WakerLimitExceeded(diagnostic))
                if diagnostic.snapshot.live_tasks == 0
                    && diagnostic.snapshot.live_wakers == 3
        ));
        assert!(scheduler.snapshot().stopped);
        assert_eq!(
            scheduler.settlement(over_limit_task),
            Some(TaskSettlement::Cancelled)
        );
        assert_eq!(
            scheduler
                .snapshot()
                .resources
                .usage(ResourceKind::Task)
                .current_count,
            0
        );
        let late = retained.borrow_mut().pop().unwrap();
        late.wake_by_ref();
        drop(late);
        retained.borrow_mut().clear();
        assert_eq!(scheduler.snapshot().ready_tasks, 0);
        assert_eq!(scheduler.drain_stale_wakes(1).unwrap(), 0);
        scheduler.assert_clean();

        let mut scheduler =
            DeterministicScheduler::with_limits(ReadyOrder::Fifo, 16, 2, 64, 16 * 1024);
        let mut internally_retained = Vec::new();
        scheduler
            .spawn(
                ExecutionLane::NetworkCompletion,
                poll_fn(move |context| {
                    internally_retained.push(context.waker().clone());
                    internally_retained.push(context.waker().clone());
                    internally_retained.push(context.waker().clone());
                    Poll::Ready(())
                }),
            )
            .unwrap();
        assert!(matches!(scheduler.step(), Ok(StepOutcome::Completed(_))));
        scheduler.assert_clean();

        let retained = Rc::new(RefCell::new(Vec::<Waker>::new()));
        let mut scheduler =
            DeterministicScheduler::with_limits(ReadyOrder::Fifo, 16, 1, 64, 16 * 1024);
        let task = scheduler
            .spawn(ExecutionLane::NetworkCompletion, {
                let retained = retained.clone();
                poll_fn(move |context| {
                    retained.borrow_mut().clear();
                    let first = context.waker().clone();
                    let transient = first.clone();
                    drop(transient);
                    retained.borrow_mut().push(first);
                    Poll::<()>::Pending
                })
            })
            .unwrap();
        assert!(matches!(
            scheduler.step().unwrap(),
            StepOutcome::PolledPending(current) if current == task
        ));
        assert_eq!(scheduler.snapshot().live_wakers, 1);
        retained.borrow()[0].wake_by_ref();
        assert!(matches!(
            scheduler.step().unwrap(),
            StepOutcome::PolledPending(current) if current == task
        ));
        assert_eq!(scheduler.snapshot().live_wakers, 1);
        assert!(scheduler.cancel(task));
        retained.borrow_mut().clear();
        scheduler.assert_clean();

        let mut scheduler =
            DeterministicScheduler::with_limits(ReadyOrder::Fifo, 16, 0, 64, 16 * 1024);
        let task = scheduler
            .spawn(
                ExecutionLane::ViewerFrame,
                poll_fn(|context| {
                    context.waker().wake_by_ref();
                    Poll::<()>::Pending
                }),
            )
            .unwrap();
        assert!(matches!(
            scheduler.run_until_idle(3),
            Err(SchedulerError::StepBudgetExceeded(diagnostic)) if diagnostic.steps == 3
        ));
        assert_eq!(scheduler.snapshot().ready_tasks, 1);
        assert!(scheduler.cancel(task));
        scheduler.assert_clean();
    }

    #[test]
    fn cancel_and_stale_drain_never_poll_live_business_future() {
        let ran = Rc::new(Cell::new(false));
        let mut scheduler = DeterministicScheduler::default();
        let cancelled = scheduler
            .spawn(ExecutionLane::ViewerFrame, async {})
            .unwrap();
        assert!(scheduler.cancel(cancelled));
        scheduler.assert_clean();

        let old_waker = Rc::new(RefCell::new(None::<Waker>));
        let old = scheduler
            .spawn_in_slot(41, ExecutionLane::NetworkCompletion, {
                let old_waker = old_waker.clone();
                poll_fn(move |context| {
                    *old_waker.borrow_mut() = Some(context.waker().clone());
                    Poll::<()>::Pending
                })
            })
            .unwrap();
        scheduler.step().unwrap();
        assert!(scheduler.cancel(old));
        let late_waker = old_waker.borrow_mut().take().unwrap();
        scheduler
            .spawn(ExecutionLane::ViewerFrame, {
                let ran = ran.clone();
                async move { ran.set(true) }
            })
            .unwrap();
        late_waker.wake_by_ref();
        drop(late_waker);
        assert_eq!(scheduler.drain_stale_wakes(1).unwrap(), 1);
        assert!(!ran.get());
        assert_eq!(scheduler.snapshot().ready_tasks, 1);
        scheduler.step().unwrap();
        assert!(ran.get());
        scheduler.assert_clean();
    }

    #[test]
    fn every_named_checkpoint_controls_a_real_future_boundary() {
        let checkpoints = [
            Checkpoint::once(AsyncBoundary::Prepare),
            Checkpoint::once(AsyncBoundary::DisarmedCommit),
            Checkpoint::once(AsyncBoundary::JsRelease),
            Checkpoint::once(AsyncBoundary::Activation),
            Checkpoint::once(AsyncBoundary::FetchCompletion),
            Checkpoint::once(AsyncBoundary::BodyBackpressure),
            Checkpoint::once(AsyncBoundary::LegacyApplyAck),
            Checkpoint::once(AsyncBoundary::Insertion(
                InsertionSafePoint::WaitingForLeases,
            )),
            Checkpoint::once(AsyncBoundary::Insertion(
                InsertionSafePoint::BeforeFirstAddChunk,
            )),
            Checkpoint::once(AsyncBoundary::Insertion(
                InsertionSafePoint::BetweenAddChunks,
            )),
            Checkpoint::once(AsyncBoundary::Insertion(
                InsertionSafePoint::AfterLastAddChunkBeforeAck,
            )),
            Checkpoint::once(AsyncBoundary::Gc(GcSafePoint::WaitingForLeases)),
            Checkpoint::once(AsyncBoundary::Gc(GcSafePoint::BeforeGcCall)),
            Checkpoint::once(AsyncBoundary::Gc(GcSafePoint::AfterGcBeforeReopen)),
            Checkpoint::once(AsyncBoundary::TeardownLateCompletion),
        ];
        let mut scheduler = DeterministicScheduler::default();
        let current = token(3, 1);
        let controller = CheckpointController::new(current, scheduler.tracker(), scheduler.trace());
        let reached = Rc::new(Cell::new(0));
        scheduler
            .spawn(ExecutionLane::StoreCallback, {
                let controller = controller.clone();
                let reached = reached.clone();
                async move {
                    for checkpoint in checkpoints {
                        controller.wait(current, checkpoint).await.unwrap();
                        reached.set(reached.get() + 1);
                    }
                }
            })
            .unwrap();
        assert!(matches!(
            scheduler.step().unwrap(),
            StepOutcome::PolledPending(_)
        ));
        for (index, checkpoint) in checkpoints.into_iter().enumerate() {
            assert_eq!(reached.get(), index);
            assert!(!controller.release(token(3, 2), checkpoint));
            assert!(controller.release(current, checkpoint));
            scheduler.step().unwrap();
            assert_eq!(reached.get(), index + 1);
        }
        scheduler.assert_clean();
    }

    #[test]
    fn checkpoint_cancellation_is_sticky_until_matching_reset() {
        let tracker = ResourceTracker::new();
        let trace = BoundedTrace::default();
        let first = token(12, 1);
        let second = token(12, 2);
        let controller = CheckpointController::new(first, tracker.clone(), trace);
        assert!(controller.cancel_all(first));
        assert!(!controller.cancel_all(first));
        assert!(!controller.release(first, Checkpoint::once(AsyncBoundary::Activation)));

        let mut unseen = Box::pin(controller.wait(
            first,
            Checkpoint::once(AsyncBoundary::TeardownLateCompletion),
        ));
        let waker = futures::task::noop_waker();
        let mut context = Context::from_waker(&waker);
        assert_eq!(
            unseen.as_mut().poll(&mut context),
            Poll::Ready(Err(CheckpointError::Cancelled))
        );

        assert!(controller.reset(first, second));
        assert!(!controller.release(first, Checkpoint::once(AsyncBoundary::Activation)));
        let mut old = Box::pin(controller.wait(first, Checkpoint::once(AsyncBoundary::Activation)));
        assert_eq!(
            old.as_mut().poll(&mut context),
            Poll::Ready(Err(CheckpointError::Stale))
        );

        let checkpoint = Checkpoint::once(AsyncBoundary::Activation);
        assert!(controller.release(second, checkpoint));
        let mut current = Box::pin(controller.wait(second, checkpoint));
        assert_eq!(current.as_mut().poll(&mut context), Poll::Ready(Ok(())));
        drop((unseen, old, current));
        assert_resources_empty(&tracker);
    }

    #[test]
    fn checkpoint_reset_publishes_new_generation_before_reentrant_wake() {
        let tracker = ResourceTracker::new();
        let trace = BoundedTrace::default();
        let first = token(13, 1);
        let second = token(13, 2);
        let third = token(13, 3);
        let controller = CheckpointController::new(first, tracker.clone(), trace);
        let old_checkpoint = Checkpoint::once(AsyncBoundary::Prepare);
        let old_future = Rc::new(RefCell::new(Box::pin(
            controller.wait(first, old_checkpoint),
        )));
        let reentrant_completed = Rc::new(Cell::new(false));
        let reset_waker = reentrant_waker({
            let controller = controller.clone();
            let old_future = old_future.clone();
            let reentrant_completed = reentrant_completed.clone();
            move || {
                let waker = futures::task::noop_waker();
                let mut context = Context::from_waker(&waker);
                assert_eq!(
                    old_future.borrow_mut().as_mut().poll(&mut context),
                    Poll::Ready(Err(CheckpointError::Cancelled))
                );
                assert_eq!(controller.token(), second);
                assert!(!controller.reset(first, third));

                let next_checkpoint = Checkpoint::once(AsyncBoundary::Activation);
                let mut next_future = Box::pin(controller.wait(second, next_checkpoint));
                assert_eq!(next_future.as_mut().poll(&mut context), Poll::Pending);
                assert!(controller.reset(second, third));
                assert_eq!(
                    next_future.as_mut().poll(&mut context),
                    Poll::Ready(Err(CheckpointError::Cancelled))
                );
                assert_eq!(controller.token(), third);
                reentrant_completed.set(true);
            }
        });
        let mut context = Context::from_waker(&reset_waker);
        assert_eq!(
            old_future.borrow_mut().as_mut().poll(&mut context),
            Poll::Pending
        );
        assert!(controller.reset(first, second));
        assert!(reentrant_completed.get());
        assert_eq!(controller.token(), third);
        assert!(!controller.release(second, Checkpoint::once(AsyncBoundary::Activation)));
        assert!(controller.release(third, Checkpoint::once(AsyncBoundary::Activation)));
        assert_no_reentrant_action();
        drop((reset_waker, old_future));
        assert_resources_empty(&tracker);
    }

    #[test]
    fn callback_wakes_can_synchronously_repoll_without_refcell_borrows() {
        let tracker = ResourceTracker::new();
        let trace = BoundedTrace::default();
        let current = token(30, 1);

        let controller = CheckpointController::new(current, tracker.clone(), trace.clone());
        let checkpoint = Checkpoint::once(AsyncBoundary::JsRelease);
        let checkpoint_future =
            Rc::new(RefCell::new(Box::pin(controller.wait(current, checkpoint))));
        let checkpoint_ready = Rc::new(Cell::new(false));
        let checkpoint_waker = reentrant_waker({
            let future = checkpoint_future.clone();
            let ready = checkpoint_ready.clone();
            move || {
                let waker = futures::task::noop_waker();
                let mut context = Context::from_waker(&waker);
                assert_eq!(
                    future.borrow_mut().as_mut().poll(&mut context),
                    Poll::Ready(Ok(()))
                );
                ready.set(true);
            }
        });
        let mut context = Context::from_waker(&checkpoint_waker);
        assert_eq!(
            checkpoint_future.borrow_mut().as_mut().poll(&mut context),
            Poll::Pending
        );
        assert!(controller.release(current, checkpoint));
        assert!(checkpoint_ready.get());
        assert_no_reentrant_action();
        drop((checkpoint_waker, checkpoint_future));

        let (source, completion_future) =
            token_completion(current, 4, &tracker, trace.clone()).unwrap();
        let completion_future = Rc::new(RefCell::new(Box::pin(completion_future)));
        let completion_value = Rc::new(Cell::new(None));
        let completion_waker = reentrant_waker({
            let future = completion_future.clone();
            let value = completion_value.clone();
            move || {
                let waker = futures::task::noop_waker();
                let mut context = Context::from_waker(&waker);
                let Poll::Ready(Ok(result)) = future.borrow_mut().as_mut().poll(&mut context)
                else {
                    panic!("completion callback did not synchronously settle")
                };
                value.set(Some(result));
            }
        });
        let mut context = Context::from_waker(&completion_waker);
        assert_eq!(
            completion_future.borrow_mut().as_mut().poll(&mut context),
            Poll::Pending
        );
        assert!(matches!(
            source.complete(current, 7_u8),
            CompletionAttempt::Accepted
        ));
        assert_eq!(completion_value.get(), Some(7));
        assert_no_reentrant_action();
        drop((completion_waker, completion_future, source));

        let queue = AckQueue::new(current, 1, 1, tracker.clone(), trace.clone());
        let mut first_send = Box::pin(queue.send(current, 1_u8, 1).unwrap());
        let waker = futures::task::noop_waker();
        let mut context = Context::from_waker(&waker);
        assert!(matches!(
            first_send.as_mut().poll(&mut context),
            Poll::Ready(Ok(()))
        ));
        let mut first_delivery = queue.try_recv(current).unwrap().unwrap();
        let second_send = Rc::new(RefCell::new(Box::pin(
            queue.send(current, 2_u8, 1).unwrap(),
        )));
        let second_ready = Rc::new(Cell::new(false));
        let queue_waker = reentrant_waker({
            let second_send = second_send.clone();
            let ready = second_ready.clone();
            move || {
                let waker = futures::task::noop_waker();
                let mut context = Context::from_waker(&waker);
                assert!(matches!(
                    second_send.borrow_mut().as_mut().poll(&mut context),
                    Poll::Ready(Ok(()))
                ));
                ready.set(true);
            }
        });
        let mut context = Context::from_waker(&queue_waker);
        assert!(matches!(
            second_send.borrow_mut().as_mut().poll(&mut context),
            Poll::Pending
        ));
        let first_id = first_delivery.identity();
        assert_eq!(first_delivery.ack(current, first_id), Ok(1));
        assert!(second_ready.get());
        assert_no_reentrant_action();
        let mut second_delivery = queue.try_recv(current).unwrap().unwrap();
        let second_id = second_delivery.identity();
        assert_eq!(second_delivery.ack(current, second_id), Ok(2));
        drop((
            queue_waker,
            first_send,
            first_delivery,
            second_send,
            second_delivery,
        ));

        let leases = LeasePool::new(current, tracker.clone(), trace);
        let lease = leases.acquire(current).unwrap();
        assert!(leases.close(current));
        let drain = Rc::new(RefCell::new(Box::pin(leases.drain(current))));
        let drain_ready = Rc::new(Cell::new(false));
        let lease_waker = reentrant_waker({
            let drain = drain.clone();
            let ready = drain_ready.clone();
            move || {
                let waker = futures::task::noop_waker();
                let mut context = Context::from_waker(&waker);
                assert_eq!(
                    drain.borrow_mut().as_mut().poll(&mut context),
                    Poll::Ready(Ok(()))
                );
                ready.set(true);
            }
        });
        let mut context = Context::from_waker(&lease_waker);
        assert_eq!(
            drain.borrow_mut().as_mut().poll(&mut context),
            Poll::Pending
        );
        drop(lease);
        assert!(drain_ready.get());
        assert_no_reentrant_action();
        drop((lease_waker, drain));
        assert_resources_empty(&tracker);
    }

    #[test]
    fn completion_is_tokenized_exactly_once_and_drop_safe() {
        let tracker = ResourceTracker::new();
        let trace = BoundedTrace::default();
        let current = token(1, 4);
        let (source, future) = token_completion(current, 64, &tracker, trace.clone()).unwrap();
        let result = Rc::new(Cell::new(None));
        let mut scheduler = DeterministicScheduler::default();
        scheduler
            .spawn(ExecutionLane::NetworkCompletion, {
                let result = result.clone();
                async move { result.set(Some(future.await.unwrap())) }
            })
            .unwrap();
        scheduler.step().unwrap();
        assert!(matches!(
            source.complete(token(1, 3), 10),
            CompletionAttempt::Stale(10)
        ));
        assert!(matches!(
            source.complete(current, 20),
            CompletionAttempt::Accepted
        ));
        assert!(matches!(
            source.complete(current, 30),
            CompletionAttempt::Duplicate(30)
        ));
        scheduler.step().unwrap();
        assert_eq!(result.get(), Some(20));
        assert_eq!(source.settlement_count(), 1);
        drop(source);
        assert_resources_empty(&tracker);

        let (source, future) = token_completion(current, 8, &tracker, trace).unwrap();
        drop(future);
        assert!(matches!(
            source.complete(current, 1),
            CompletionAttempt::ReceiverDropped(1)
        ));
        assert_eq!(source.settlement_count(), 1);
        drop(source);
        assert_resources_empty(&tracker);
    }

    #[test]
    fn ack_queue_holds_capacity_until_delivery_ack_and_closes_waiters() {
        let mut scheduler = DeterministicScheduler::default();
        let tracker = scheduler.tracker();
        let current = token(2, 1);
        let queue = AckQueue::new(current, 1, 4, tracker.clone(), scheduler.trace());
        let first_done = Rc::new(Cell::new(false));
        scheduler
            .spawn(ExecutionLane::NetworkCompletion, {
                let first_done = first_done.clone();
                let send = queue.send(current, 10_u8, 4).unwrap();
                async move {
                    send.await.unwrap();
                    first_done.set(true);
                }
            })
            .unwrap();
        let second_done = Rc::new(Cell::new(false));
        scheduler
            .spawn(ExecutionLane::NetworkCompletion, {
                let second_done = second_done.clone();
                let send = queue.send(current, 20_u8, 4).unwrap();
                async move {
                    send.await.unwrap();
                    second_done.set(true);
                }
            })
            .unwrap();
        scheduler.step().unwrap();
        scheduler.step().unwrap();
        assert!(first_done.get());
        assert!(!second_done.get());
        let mut delivery = queue.try_recv(current).unwrap().unwrap();
        assert_eq!(*delivery.item(), 10);
        assert_eq!(queue.snapshot().in_flight_count, 1);
        assert_eq!(scheduler.step().unwrap(), StepOutcome::NoReadyTask);
        let delivery_id = delivery.identity();
        assert_eq!(delivery.ack(current, delivery_id), Ok(10));
        scheduler.step().unwrap();
        assert!(second_done.get());
        let mut delivery = queue.try_recv(current).unwrap().unwrap();
        let delivery_id = delivery.identity();
        assert_eq!(delivery.ack(current, delivery_id), Ok(20));
        assert_eq!(queue.snapshot().high_water_count, 1);
        assert_eq!(queue.snapshot().high_water_bytes, 4);
        assert_resources_empty(&tracker);

        let waiting = Rc::new(Cell::new(false));
        scheduler
            .spawn(ExecutionLane::NetworkCompletion, {
                let waiting = waiting.clone();
                let send = queue.send(current, 30_u8, 4).unwrap();
                async move {
                    send.await.unwrap();
                    waiting.set(true);
                }
            })
            .unwrap();
        scheduler.step().unwrap();
        scheduler
            .spawn(ExecutionLane::NetworkCompletion, {
                let send = queue.send(current, 40_u8, 4).unwrap();
                async move {
                    assert!(matches!(send.await, Err(QueueSendError::Closed(40))));
                }
            })
            .unwrap();
        scheduler.step().unwrap();
        assert!(queue.close(current));
        scheduler.run_until_idle(2).unwrap();
        assert!(waiting.get());
        assert_resources_empty(&tracker);
        scheduler.assert_clean();
    }

    #[test]
    fn queue_ack_identity_is_generation_safe_across_slot_reuse() {
        let mut scheduler = DeterministicScheduler::default();
        let tracker = scheduler.tracker();
        let first = token(21, 1);
        let second = token(21, 2);
        let queue = AckQueue::new(first, 1, 8, tracker.clone(), scheduler.trace());
        let send = queue.send(first, 10_u8, 8).unwrap();
        scheduler
            .spawn(ExecutionLane::NetworkCompletion, async move {
                send.await.unwrap();
            })
            .unwrap();
        scheduler.step().unwrap();
        let mut old_delivery = queue.try_recv(first).unwrap().unwrap();
        let old_id = old_delivery.identity();

        assert!(matches!(
            queue.reset(first, first),
            Err(QueueResetError::InvalidReplacement)
        ));
        let replacement = queue.reset(first, second).unwrap();
        assert!(queue.snapshot().retired);
        assert!(matches!(
            queue.send(first, 11_u8, 8).unwrap().now_or_never(),
            Some(Err(QueueSendError::Stale(11)))
        ));
        assert!(matches!(
            queue.try_recv(first),
            Err(QueueReceiveError::Stale)
        ));

        let send = replacement.send(second, 20_u8, 8).unwrap();
        scheduler
            .spawn(ExecutionLane::NetworkCompletion, async move {
                send.await.unwrap();
            })
            .unwrap();
        scheduler.step().unwrap();
        let mut new_delivery = replacement.try_recv(second).unwrap().unwrap();
        let new_id = new_delivery.identity();
        assert_eq!(replacement.snapshot().retained_count, 1);

        assert_eq!(
            old_delivery.ack(first, old_id),
            Err(QueueAckError::StaleGeneration)
        );
        assert_eq!(replacement.snapshot().retained_count, 1);
        drop(old_delivery);
        assert_eq!(replacement.snapshot().retained_count, 1);

        assert_eq!(
            new_delivery.ack(first, new_id),
            Err(QueueAckError::WrongToken)
        );
        assert_eq!(
            new_delivery.ack(
                second,
                QueueDeliveryId {
                    token: second,
                    sequence: new_id.sequence.checked_add(1).unwrap(),
                },
            ),
            Err(QueueAckError::WrongDelivery)
        );
        assert_eq!(replacement.snapshot().retained_count, 1);
        assert_eq!(new_delivery.ack(second, new_id), Ok(20));
        assert_eq!(
            new_delivery.ack(second, new_id),
            Err(QueueAckError::Duplicate)
        );
        assert_eq!(replacement.snapshot().retained_count, 0);
        drop(new_delivery);
        assert_eq!(replacement.snapshot().retained_count, 0);
        assert_resources_empty(&tracker);
        scheduler.assert_clean();
    }

    #[test]
    fn lease_drain_waits_for_last_lease_and_isolates_reused_generation() {
        let mut scheduler = DeterministicScheduler::default();
        let tracker = scheduler.tracker();
        let first = token(5, 1);
        let pool = LeasePool::new(first, tracker.clone(), scheduler.trace());
        let lease_a = pool.acquire(first).unwrap();
        let lease_b = pool.acquire(first).unwrap();
        assert!(pool.close(first));
        let drained = Rc::new(Cell::new(false));
        scheduler
            .spawn(ExecutionLane::StoreCallback, {
                let drained = drained.clone();
                let drain = pool.drain(first);
                async move {
                    drain.await.unwrap();
                    drained.set(true);
                }
            })
            .unwrap();
        scheduler.step().unwrap();
        drop(lease_a);
        assert_eq!(scheduler.step().unwrap(), StepOutcome::NoReadyTask);
        drop(lease_b);
        scheduler.step().unwrap();
        assert!(drained.get());

        let second = token(5, 2);
        assert!(pool.reset(first, second));
        let old_drain = pool.drain(first);
        let new_lease = pool.acquire(second).unwrap();
        scheduler
            .spawn(ExecutionLane::StoreCallback, async move {
                assert_eq!(old_drain.await, Err(LeaseError::Stale));
            })
            .unwrap();
        scheduler.step().unwrap();
        assert_eq!(pool.snapshot().live_leases, 1);
        drop(new_lease);
        assert_resources_empty(&tracker);
        scheduler.assert_clean();
    }

    #[test]
    fn manual_clock_separates_wall_time_and_monotonic_timers() {
        let mut scheduler = DeterministicScheduler::default();
        let tracker = scheduler.tracker();
        let clock = ManualClock::new(tracker.clone(), scheduler.trace());
        let woke = Rc::new(Cell::new(false));
        scheduler
            .spawn(ExecutionLane::BrowserMacrotask, {
                let sleep = clock.sleep(Duration::from_millis(10)).unwrap();
                let woke = woke.clone();
                async move {
                    sleep.await.unwrap();
                    woke.set(true);
                }
            })
            .unwrap();
        scheduler.step().unwrap();
        assert_eq!(clock.snapshot().pending_timers, 1);
        clock.advance_wall(50_000).unwrap();
        assert_eq!(scheduler.step().unwrap(), StepOutcome::NoReadyTask);
        clock.advance_monotonic(Duration::from_millis(9)).unwrap();
        assert_eq!(scheduler.step().unwrap(), StepOutcome::NoReadyTask);
        clock.advance_monotonic(Duration::from_millis(1)).unwrap();
        scheduler.step().unwrap();
        assert!(woke.get());
        assert_eq!(clock.snapshot().pending_timers, 0);
        assert_resources_empty(&tracker);
        scheduler.assert_clean();
    }

    #[test]
    fn page_signals_frames_and_repaint_are_independently_controlled() {
        let mut scheduler = DeterministicScheduler::default();
        let tracker = scheduler.tracker();
        let trace = scheduler.trace();
        let current = token(9, 1);
        let signals = PageSignalQueue::new(current, 2, tracker.clone(), trace.clone());
        for signal in [PageSignal::VisibilityHidden, PageSignal::Resume] {
            let send = signals.send(current, signal).unwrap();
            scheduler
                .spawn(ExecutionLane::BrowserMacrotask, async move {
                    send.await.unwrap();
                })
                .unwrap();
            scheduler.step().unwrap();
        }
        let mut first = signals.try_recv(current).unwrap().unwrap();
        assert_eq!(*first.item(), PageSignal::VisibilityHidden);
        let first_id = first.identity();
        first.ack(current, first_id).unwrap();
        let mut second = signals.try_recv(current).unwrap().unwrap();
        assert_eq!(*second.item(), PageSignal::Resume);
        let second_id = second.identity();
        second.ack(current, second_id).unwrap();

        let frames = FrameController::new(current, tracker.clone(), trace);
        let requester = frames.repaint_requester(current).unwrap();
        assert!(requester.request());
        assert!(requester.request());
        assert!(frames.snapshot().repaint_pending);
        assert!(frames.grant_animation_frames(current, 1));
        assert!(frames.grant_viewer_frames(current, 2));
        assert!(frames.consume_viewer_frame(current));
        assert!(frames.snapshot().repaint_pending);
        assert!(frames.consume_animation_frame(current));
        assert!(!frames.snapshot().repaint_pending);
        assert!(!frames.consume_animation_frame(current));
        let next = token(9, 2);
        assert!(frames.reset(current, next));
        assert!(!requester.request());
        assert_eq!(frames.snapshot().stale_repaint_requests, 1);
        requester.release();
        assert_resources_empty(&tracker);
        scheduler.assert_clean();
    }

    #[test]
    fn abort_stop_panic_and_late_callback_release_all_ownership() {
        let mut scheduler = DeterministicScheduler::default();
        let (aborted, abort) = scheduler
            .spawn_abortable(ExecutionLane::NetworkCompletion, pending())
            .unwrap();
        scheduler.step().unwrap();
        abort.abort();
        scheduler.step().unwrap();
        assert_eq!(scheduler.settlement(aborted), Some(TaskSettlement::Aborted));
        scheduler.assert_clean();

        let tracker = scheduler.tracker();
        let current = token(6, 1);
        let (source, future) =
            token_completion::<u8>(current, 32, &tracker, scheduler.trace()).unwrap();
        scheduler
            .spawn(ExecutionLane::NetworkCompletion, async move {
                let _settlement = future.await;
            })
            .unwrap();
        scheduler.step().unwrap();
        scheduler.stop();
        assert!(matches!(
            source.complete(current, 1),
            CompletionAttempt::ReceiverDropped(1)
        ));
        drop(source);
        scheduler.assert_clean();

        let retained_waker = Rc::new(RefCell::new(None::<Waker>));
        let mut scheduler = DeterministicScheduler::default();
        scheduler
            .spawn(ExecutionLane::NetworkCompletion, {
                let retained_waker = retained_waker.clone();
                poll_fn(move |context| {
                    *retained_waker.borrow_mut() = Some(context.waker().clone());
                    Poll::<()>::Pending
                })
            })
            .unwrap();
        scheduler.step().unwrap();
        scheduler.stop();
        let late_waker = retained_waker.borrow_mut().take().unwrap();
        late_waker.wake_by_ref();
        assert_eq!(scheduler.snapshot().ready_tasks, 0);
        drop(late_waker);
        assert_eq!(scheduler.drain_stale_wakes(1).unwrap(), 0);
        scheduler.assert_clean();

        let mut scheduler = DeterministicScheduler::default();
        scheduler
            .spawn(ExecutionLane::ViewerFrame, async move {
                panic!("controlled harness panic")
            })
            .unwrap();
        assert!(matches!(
            scheduler.step(),
            Err(SchedulerError::TaskPanicked(_))
        ));
        scheduler.assert_clean();
    }

    #[test]
    fn step_budget_deadlock_and_trace_are_bounded_and_sanitized() {
        let mut scheduler = DeterministicScheduler::with_limits(
            ReadyOrder::Fifo,
            4,
            4,
            3,
            std::mem::size_of::<TraceEvent>() * 3,
        );
        scheduler
            .spawn(
                ExecutionLane::ViewerFrame,
                poll_fn(|context| {
                    context.waker().wake_by_ref();
                    Poll::<()>::Pending
                }),
            )
            .unwrap();
        assert!(matches!(
            scheduler.run_until_idle(5),
            Err(SchedulerError::StepBudgetExceeded(diagnostic)) if diagnostic.steps == 5
        ));
        assert!(scheduler.snapshot().trace.overflowed);
        assert!(scheduler.snapshot().trace.events.len() <= 3);
        scheduler.stop();
        scheduler.assert_clean();

        let mut scheduler = DeterministicScheduler::default();
        scheduler
            .spawn(ExecutionLane::ViewerFrame, pending())
            .unwrap();
        scheduler.step().unwrap();
        assert!(matches!(
            scheduler.drive_until(4, |_| false),
            Err(SchedulerError::Deadlocked(_))
        ));
        scheduler.stop();
        scheduler.assert_clean();
    }

    #[test]
    fn resource_limits_and_atomic_reclassification_preserve_high_water() {
        let tracker = ResourceTracker::new();
        tracker.set_limit(
            ResourceKind::Owner,
            ResourceLimit {
                max_count: 1,
                max_bytes: 8,
            },
        );
        tracker.set_limit(
            ResourceKind::QueuedItem,
            ResourceLimit {
                max_count: 0,
                max_bytes: 0,
            },
        );
        let mut owner = tracker.reserve(ResourceKind::Owner, 1, 8).unwrap();
        assert_eq!(
            tracker.reserve(ResourceKind::Owner, 1, 1).unwrap_err(),
            ResourceError::LimitExceeded {
                kind: ResourceKind::Owner
            }
        );
        assert_eq!(
            owner.reclassify(ResourceKind::QueuedItem),
            Err(ResourceError::LimitExceeded {
                kind: ResourceKind::QueuedItem
            })
        );
        assert_eq!(owner.kind(), ResourceKind::Owner);
        assert_eq!(
            tracker.snapshot().usage(ResourceKind::Owner).current_bytes,
            8
        );
        let permits = tracker.reserve(ResourceKind::Permit, 2, 16).unwrap();
        assert_eq!(
            tracker
                .snapshot()
                .usage(ResourceKind::Permit)
                .high_water_count,
            2
        );
        assert_eq!(
            tracker
                .snapshot()
                .usage(ResourceKind::Permit)
                .high_water_bytes,
            16
        );
        drop(permits);
        drop(owner);
        assert_eq!(
            tracker
                .snapshot()
                .usage(ResourceKind::Owner)
                .high_water_bytes,
            8
        );
        assert_resources_empty(&tracker);
    }

    #[test]
    fn composed_harness_teardown_is_explicit_and_zero_live() {
        let mut harness = DeterministicWebHarness::new(ReadyOrder::Seeded(7));
        let requester = harness.frames.repaint_requester(harness.token).unwrap();
        assert!(requester.request());
        let lease = harness.leases.acquire(harness.token).unwrap();
        harness.stop();
        assert!(!requester.request());
        drop(lease);
        requester.release();
        harness.assert_clean();
    }
}
