//! Always-compiled seam types for Web remote-MCAP physical resolution.
//!
//! This module owns the types that the always-compiled `remote_chunk_scan` authority and the
//! wasm32 adapter seam reference, plus their transitive dependencies. The full resolution
//! graph (evidence authority, prepared source, ref chain, summary/validation, census/budget
//! machinery) remains in `remote_physical_resolution` and is gated to host-test / Phase-A /
//! locked profiles only.
//!
//! Fields are `pub(crate)` so the gated full-graph module can keep its `impl` blocks in a
//! different module while this module stays dependency-neutral with respect to that graph.

#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
use std::marker::PhantomData;
#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
use std::num::NonZeroU64;
#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
use std::sync::Arc;
#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
use parking_lot::Mutex;
#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
use re_log_types::TimeInt;

#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
use crate::remote_chunk_scan::{
    PendingHeaderValidation, PhysicalChunkReadLease, PhysicalChunkSourceAuthority,
    PhysicalChunkSourceBindingV1, PhysicalChunkValidationError,
};
#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
use crate::remote_fixed_layout::RemoteMcapSlice;
#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
use crate::remote_summary::ambiguous_zero::PreparedAmbiguousZeroAggregateReservations;
#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
use re_mcap_web_contract::McapCorrelationPermitV1;

#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
static NEXT_REMOTE_OBJECT_GENERATION_V1: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
pub enum RemoteObjectConsistencyClassV1 {
    StrongValidator = 1,
    DeploymentAssumed = 2,
}

pub(crate) struct RemotePhysicalObjectStateV1 {
    #[cfg(any(
        all(test, not(target_arch = "wasm32")),
        rerun_mcap_phase_a_proof_v1,
        re_mcap_locked_remote_wasm_allocator_v1
    ))]
    pub(crate) open: bool,
    #[cfg(any(
        all(test, not(target_arch = "wasm32")),
        rerun_mcap_phase_a_proof_v1,
        re_mcap_locked_remote_wasm_allocator_v1
    ))]
    pub(crate) generation: NonZeroU64,
    #[cfg(any(
        all(test, not(target_arch = "wasm32")),
        rerun_mcap_phase_a_proof_v1,
        re_mcap_locked_remote_wasm_allocator_v1
    ))]
    pub(crate) content_length: NonZeroU64,
    #[cfg(any(
        all(test, not(target_arch = "wasm32")),
        rerun_mcap_phase_a_proof_v1,
        re_mcap_locked_remote_wasm_allocator_v1
    ))]
    pub(crate) consistency: RemoteObjectConsistencyClassV1,
}

/// Opaque lower projection of one fresh Web remote-object owner.
///
/// It is intentionally non-`Clone`; URL and validator bytes never cross this boundary.
#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
pub struct RemotePhysicalObjectBindingV1 {
    pub(crate) state: Arc<Mutex<RemotePhysicalObjectStateV1>>,
}

/// One borrowed byte view signed by the currently open remote object owner.
///
/// Callers cannot construct this token from a naked slice or `Box<[u8]>`; all fixed-layout,
/// Summary, and `MessageIndex` parsers consume it while it remains nested in the enclosing owner.
#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
pub(crate) struct BoundRemoteObjectReadV1<'a> {
    pub(crate) object: Arc<Mutex<RemotePhysicalObjectStateV1>>,
    pub(crate) slice: RemoteMcapSlice<'a>,
}

/// Sealed upper-transport issuer for reads produced by the matching validator owner.
///
/// The lower MCAP layer can only consume and match this evidence; it cannot bless caller-provided
/// byte slices itself.
#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
pub(crate) struct RemoteObjectReadIssuerV1 {
    pub(crate) object: Arc<Mutex<RemotePhysicalObjectStateV1>>,
}

#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
impl std::fmt::Debug for RemotePhysicalObjectBindingV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RemotePhysicalObjectBindingV1")
            .field("identity", &"<opaque fresh-per-open>")
            .finish_non_exhaustive()
    }
}

#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
impl RemotePhysicalObjectBindingV1 {
    #[cfg(any(test, rerun_mcap_phase_a_proof_v1))]
    pub(crate) fn issue(
        content_length: NonZeroU64,
        consistency: RemoteObjectConsistencyClassV1,
    ) -> Result<Self, PhysicalSourceResolutionErrorV1> {
        let generation = NEXT_REMOTE_OBJECT_GENERATION_V1
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map_err(|_current| PhysicalSourceResolutionErrorV1::GenerationExhausted)
            .and_then(|value| {
                NonZeroU64::new(value).ok_or(PhysicalSourceResolutionErrorV1::GenerationExhausted)
            })?;
        Ok(Self {
            state: Arc::new(Mutex::new(RemotePhysicalObjectStateV1 {
                open: true,
                generation,
                content_length,
                consistency,
            })),
        })
    }

    #[cfg(any(test, rerun_mcap_phase_a_proof_v1))]
    pub(crate) fn issue_for_phase_a_measurement_v1(
        content_length: NonZeroU64,
        consistency: RemoteObjectConsistencyClassV1,
    ) -> Result<Self, PhysicalSourceResolutionErrorV1> {
        Self::issue(content_length, consistency)
    }

    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(crate) fn issue_for_test(
        content_length: NonZeroU64,
        consistency: RemoteObjectConsistencyClassV1,
    ) -> Result<Self, PhysicalSourceResolutionErrorV1> {
        Self::issue_for_phase_a_measurement_v1(content_length, consistency)
    }

    pub(crate) fn ensure_open(&self) -> Result<(), PhysicalSourceResolutionErrorV1> {
        if self.state.lock().open {
            Ok(())
        } else {
            Err(PhysicalSourceResolutionErrorV1::ObjectClosed)
        }
    }

    pub(crate) fn content_length(&self) -> NonZeroU64 {
        self.state.lock().content_length
    }

    pub(crate) fn generation(&self) -> NonZeroU64 {
        self.state.lock().generation
    }

    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(crate) fn identity_for_test(
        &self,
    ) -> (NonZeroU64, NonZeroU64, RemoteObjectConsistencyClassV1) {
        let state = self.state.lock();
        (state.generation, state.content_length, state.consistency)
    }
}

#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
impl RemoteObjectReadIssuerV1 {
    pub(crate) fn issue_v1<'a>(
        &self,
        slice: RemoteMcapSlice<'a>,
    ) -> Result<BoundRemoteObjectReadV1<'a>, PhysicalSourceResolutionErrorV1> {
        if !self.object.lock().open {
            return Err(PhysicalSourceResolutionErrorV1::ObjectClosed);
        }
        let end = slice
            .offset()
            .checked_add(
                u64::try_from(slice.bytes().len())
                    .map_err(|_overflow| PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?,
            )
            .ok_or(PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?;
        if end > self.object.lock().content_length.get() {
            return Err(PhysicalSourceResolutionErrorV1::ObjectLengthMismatch);
        }
        Ok(BoundRemoteObjectReadV1 {
            object: Arc::clone(&self.object),
            slice,
        })
    }
}

#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
impl<'a> BoundRemoteObjectReadV1<'a> {
    #[cfg(re_mcap_locked_remote_wasm_allocator_v1)]
    pub(crate) fn offset(&self) -> u64 {
        self.slice.offset()
    }

    #[cfg(re_mcap_locked_remote_wasm_allocator_v1)]
    pub(crate) fn bytes(&self) -> &'a [u8] {
        self.slice.bytes()
    }

    pub(crate) fn into_slice_v1(
        self,
        expected: &RemotePhysicalObjectBindingV1,
    ) -> Result<RemoteMcapSlice<'a>, PhysicalSourceResolutionErrorV1> {
        if !Arc::ptr_eq(&self.object, &expected.state) {
            return Err(PhysicalSourceResolutionErrorV1::ObjectBindingMismatch);
        }
        expected.ensure_open()?;
        Ok(self.slice)
    }
}

#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
pub(crate) struct RemotePhysicalObjectLifetimeV1 {
    pub(crate) state: Arc<Mutex<RemotePhysicalObjectStateV1>>,
}

#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
impl Drop for RemotePhysicalObjectLifetimeV1 {
    fn drop(&mut self) {
        self.state.lock().open = false;
    }
}

#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
pub(crate) struct RemotePhysicalSourceRegistryStateV1;

#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
pub(crate) struct RemotePhysicalSourceRegistryLeaseV1 {
    #[cfg(re_mcap_locked_remote_wasm_allocator_v1)]
    pub(crate) state: Arc<RemotePhysicalSourceRegistryStateV1>,
}

#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanonicalPhysicalExtentV1 {
    KnownEmpty,
    Known { start: TimeInt, end: TimeInt },
}

#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CanonicalIntervalV1 {
    pub(crate) start: TimeInt,
    pub(crate) end: TimeInt,
    pub(crate) canonical_ordinal: usize,
}

#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResolutionClassificationV1 {
    Pending,
    KnownEmpty,
    NonEmpty { start: TimeInt, end: TimeInt },
}

/// Immutable layout retained by the resolved authority and available only by borrow.
#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
pub struct ResolvedCanonicalPhysicalLayoutV1 {
    pub(crate) classifications: Box<[ResolutionClassificationV1]>,
    pub(crate) intervals: Box<[CanonicalIntervalV1]>,
    pub(crate) prefix_max_end: Box<[TimeInt]>,
    pub(crate) interval_count: usize,
    pub(crate) extent: CanonicalPhysicalExtentV1,
}

#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
impl ResolvedCanonicalPhysicalLayoutV1 {
    pub fn canonical_extent_v1(&self) -> CanonicalPhysicalExtentV1 {
        self.extent
    }

    pub fn canonical_chunk_count_v1(&self) -> usize {
        self.classifications.len()
    }

    pub fn intersecting_ordinals_v1(
        &self,
        query_start: TimeInt,
        query_end: TimeInt,
    ) -> impl Iterator<Item = usize> + '_ {
        let intervals = &self.intervals[..self.interval_count];
        let upper = intervals.partition_point(|interval| interval.start <= query_end);
        let lower = self.prefix_max_end[..upper].partition_point(|max_end| *max_end < query_start);
        intervals[lower..upper]
            .iter()
            .filter(move |interval| interval.end >= query_start)
            .map(|interval| interval.canonical_ordinal)
    }
}

#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
pub(crate) struct AggregateResolutionBudgetStateV1 {
    pub(crate) max_active: u64,
    pub(crate) max_retained_bytes: u64,
    pub(crate) usage: Mutex<AggregateResolutionBudgetUsageV1>,
}

#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct AggregateResolutionBudgetUsageV1 {
    pub(crate) active: u64,
    pub(crate) retained_bytes: u64,
}

#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
pub(crate) struct AggregateResolutionReservationV1 {
    pub(crate) state: Arc<AggregateResolutionBudgetStateV1>,
    pub(crate) retained_bytes: u64,
}

#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
impl Drop for AggregateResolutionReservationV1 {
    fn drop(&mut self) {
        let mut usage = self.state.usage.lock();
        usage.active -= 1;
        usage.retained_bytes -= self.retained_bytes;
    }
}

#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
pub struct BoundResolvedPhysicalSourceAuthorityV1<'a, ValidatorOwner> {
    pub(crate) inner: ResolvedPhysicalSourceAuthorityV1<'a>,
    pub(crate) object_lifetime: RemotePhysicalObjectLifetimeV1,
    pub(crate) validator_owner: PhantomData<ValidatorOwner>,
}

#[cfg(target_arch = "wasm32")]
#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
impl<'a, ValidatorOwner> BoundResolvedPhysicalSourceAuthorityV1<'a, ValidatorOwner> {
    /// Exposes the resolved physical source authority for the Web adapter's
    /// `issue_physical_receipt_v1` seam.
    ///
    /// The lease and selector types remain `pub(crate)`; this exposes only the
    /// receipt-issuing authority handle. The returned borrow keeps the bound object
    /// lifetime alive, so the remote object stays open while the authority is held.
    pub fn resolved_authority_v1(&self) -> &ResolvedPhysicalSourceAuthorityV1<'a> {
        let _keep_object_open = &self.object_lifetime;
        &self.inner
    }
}

#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
pub struct ResolvedPhysicalSourceAuthorityV1<'a> {
    pub(crate) object: RemotePhysicalObjectBindingV1,
    pub(crate) authority: PhysicalChunkSourceAuthority<'a>,
    pub(crate) source_binding: PhysicalChunkSourceBindingV1,
    pub(crate) layout: ResolvedCanonicalPhysicalLayoutV1,
    pub(crate) _reservation: AggregateResolutionReservationV1,
    pub(crate) _registry: RemotePhysicalSourceRegistryLeaseV1,
    pub(crate) _aggregate_reservations: PreparedAmbiguousZeroAggregateReservations,
}

#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
impl<'a> ResolvedPhysicalSourceAuthorityV1<'a> {
    pub(crate) fn issue_lease_v1(
        &self,
        ordinal: usize,
    ) -> Result<PhysicalChunkReadLease<'a, PendingHeaderValidation>, PhysicalSourceResolutionErrorV1>
    {
        self.object.ensure_open()?;
        self.source_binding
            .ensure_current_v1()
            .map_err(PhysicalSourceResolutionErrorV1::Physical)?;
        let selector = self
            .authority
            .select(ordinal)
            .map_err(PhysicalSourceResolutionErrorV1::Physical)?;
        self.authority
            .issue(selector)
            .map_err(PhysicalSourceResolutionErrorV1::Physical)
    }

    /// Issues a producer-issued physical receipt for one canonical chunk read.
    ///
    /// The adapter-facing seam revalidates the live object/source binding, selects the
    /// canonical chunk by ordinal, and issues a live read lease before consuming the MCAP
    /// correlation permit. The returned [`crate::web_body_handoff::WebPhysicalReceiptV1`]
    /// keeps the lease and the physical identity private, so the adapter never names the
    /// lease, the selector, the raw body, the cache, the reservation, or the scalar identity.
    pub fn issue_physical_receipt_v1(
        &self,
        ordinal: usize,
        permit: McapCorrelationPermitV1,
    ) -> Result<crate::web_body_handoff::WebPhysicalReceiptV1<'a>, PhysicalSourceResolutionErrorV1>
    {
        let lease = self.issue_lease_v1(ordinal)?;
        Ok(crate::web_body_handoff::WebPhysicalReceiptV1::from_lease_v1(lease, permit))
    }
}

#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhysicalSourceResolutionErrorV1 {
    ArithmeticOverflow,
    GenerationExhausted,
    ReservationLimitExceeded,
    AllocationFailed,
    ObjectClosed,
    ObjectLengthMismatch,
    ObjectBindingMismatch,
    RegistryBindingMismatch,
    PhysicalSourceBindingMismatch,
    InvalidPlan,
    InvalidFixedLayout,
    InvalidTemporalValue,
    OutOfOrderResolution,
    ResolutionComplete,
    ResolutionIncomplete,
    AlreadyFinalized,
    AmbiguousExtentNotZero,
    Physical(PhysicalChunkValidationError),
}
