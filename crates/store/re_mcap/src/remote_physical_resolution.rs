//! Exactly-once ownership bridge between ambiguous-zero preparation and physical Chunk reads.
//!
//! This is production-disarmed until the Web opening adapter supplies an artifact-issued object
//! binding. It has no HTTP types and cannot inspect validator wire bytes.

use std::alloc::Layout;
use std::num::NonZeroU64;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use parking_lot::Mutex;
use re_log_types::TimeInt;

use crate::remote_chunk_scan::{
    PendingHeaderValidation, PhysicalChunkDefinitionsCapabilityV1, PhysicalChunkReadLease,
    PhysicalChunkScanBudget, PhysicalChunkSourceAuthority, PhysicalChunkSourceBindingV1,
    PhysicalChunkValidationError, PreparedPhysicalChunkAuthorityAllocationV1Bound,
    PreparedPhysicalChunkAuthorityClaimV1, PreparedPhysicalChunkAuthorityContextV1,
    ValidatedPhysicalChunkExtent, ValidatedPhysicalChunkScan,
};
use crate::remote_decompression::ChunkDecompressionBudget;
use crate::remote_summary::PreparedAmbiguousZeroBodyPlan;
use crate::remote_summary::{
    AmbiguousChunkClassification, PreparedAmbiguousZeroAggregateReservations,
    PreparedAmbiguousZeroResolutionSeed,
};
use crate::remote_time::{RawMcapTime, canonicalize_raw_mcap_time};
use re_mcap_web_contract::McapCorrelationPermitV1;

static NEXT_REMOTE_OBJECT_GENERATION_V1: AtomicU64 = AtomicU64::new(1);

// Test-only fault injection used to prove that allocations after the aggregate claim roll back
// the claim and all lower reservations. This is compiled out of every production build.
#[cfg(test)]
thread_local! {
    static FAIL_NEXT_POST_CLAIM_ALLOCATION_V1: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum RemoteObjectConsistencyClassV1 {
    StrongValidator = 1,
    DeploymentAssumed = 2,
}

pub(crate) struct RemotePhysicalObjectStateV1 {
    open: bool,
    generation: NonZeroU64,
    content_length: NonZeroU64,
    consistency: RemoteObjectConsistencyClassV1,
}

/// Opaque lower projection of one fresh Web remote-object owner.
///
/// It is intentionally non-`Clone`; URL and validator bytes never cross this boundary.
pub struct RemotePhysicalObjectBindingV1 {
    state: Arc<Mutex<RemotePhysicalObjectStateV1>>,
}

/// One borrowed byte view signed by the currently open remote object owner.
///
/// Callers cannot construct this token from a naked slice or `Box<[u8]>`; all fixed-layout,
/// Summary, and `MessageIndex` parsers consume it while it remains nested in the enclosing owner.
pub(crate) struct BoundRemoteObjectReadV1<'a> {
    object: Arc<Mutex<RemotePhysicalObjectStateV1>>,
    slice: crate::remote_fixed_layout::RemoteMcapSlice<'a>,
}

/// Sealed upper-transport issuer for reads produced by the matching validator owner.
///
/// The lower MCAP layer can only consume and match this evidence; it cannot bless caller-provided
/// byte slices itself.
struct RemoteObjectReadIssuerV1 {
    object: Arc<Mutex<RemotePhysicalObjectStateV1>>,
}

impl std::fmt::Debug for RemotePhysicalObjectBindingV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RemotePhysicalObjectBindingV1")
            .field("identity", &"<opaque fresh-per-open>")
            .finish_non_exhaustive()
    }
}

impl RemotePhysicalObjectBindingV1 {
    #[cfg(any(test, rerun_mcap_phase_a_proof_v1))]
    fn issue(
        content_length: NonZeroU64,
        consistency: RemoteObjectConsistencyClassV1,
    ) -> Result<Self, PhysicalSourceResolutionErrorV1> {
        let generation = NEXT_REMOTE_OBJECT_GENERATION_V1
            .fetch_update(
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
                |current| current.checked_add(1),
            )
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
    fn issue_for_phase_a_measurement_v1(
        content_length: NonZeroU64,
        consistency: RemoteObjectConsistencyClassV1,
    ) -> Result<Self, PhysicalSourceResolutionErrorV1> {
        Self::issue(content_length, consistency)
    }

    #[cfg(test)]
    fn issue_for_test(
        content_length: NonZeroU64,
        consistency: RemoteObjectConsistencyClassV1,
    ) -> Result<Self, PhysicalSourceResolutionErrorV1> {
        Self::issue_for_phase_a_measurement_v1(content_length, consistency)
    }

    fn ensure_open(&self) -> Result<(), PhysicalSourceResolutionErrorV1> {
        if self.state.lock().open {
            Ok(())
        } else {
            Err(PhysicalSourceResolutionErrorV1::ObjectClosed)
        }
    }

    fn content_length(&self) -> NonZeroU64 {
        self.state.lock().content_length
    }

    fn generation(&self) -> NonZeroU64 {
        self.state.lock().generation
    }

    #[cfg(test)]
    fn identity_for_test(&self) -> (NonZeroU64, NonZeroU64, RemoteObjectConsistencyClassV1) {
        let state = self.state.lock();
        (state.generation, state.content_length, state.consistency)
    }
}

impl RemoteObjectReadIssuerV1 {
    fn issue_v1<'a>(
        &self,
        slice: crate::remote_fixed_layout::RemoteMcapSlice<'a>,
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

impl<'a> BoundRemoteObjectReadV1<'a> {
    fn offset(&self) -> u64 {
        self.slice.offset()
    }

    fn bytes(&self) -> &'a [u8] {
        self.slice.bytes()
    }

    fn into_slice_v1(
        self,
        expected: &RemotePhysicalObjectBindingV1,
    ) -> Result<crate::remote_fixed_layout::RemoteMcapSlice<'a>, PhysicalSourceResolutionErrorV1>
    {
        if !Arc::ptr_eq(&self.object, &expected.state) {
            return Err(PhysicalSourceResolutionErrorV1::ObjectBindingMismatch);
        }
        expected.ensure_open()?;
        Ok(self.slice)
    }
}

struct RemotePhysicalObjectLifetimeV1 {
    state: Arc<Mutex<RemotePhysicalObjectStateV1>>,
}

impl Drop for RemotePhysicalObjectLifetimeV1 {
    fn drop(&mut self) {
        self.state.lock().open = false;
    }
}

struct RemotePhysicalSourceRegistryStateV1;

struct RemotePhysicalSourceRegistryLeaseV1 {
    state: Arc<RemotePhysicalSourceRegistryStateV1>,
}

struct RemotePhysicalEvidenceProfileV1 {
    summary_census_limits: crate::remote_summary::SummaryCensusLimits,
    summary_materialization_budget:
        crate::remote_summary::materialization::SummaryMaterializationBudget,
    physical_region_budget: crate::remote_summary::physical_regions::PhysicalRegionBudget,
    message_index_budget: crate::remote_summary::message_index::MessageIndexRegionBudget,
    ambiguous_zero_budget: crate::remote_summary::ambiguous_zero::AmbiguousZeroBudget,
    decompression_budget: ChunkDecompressionBudget,
    scan_budget: PhysicalChunkScanBudget,
    aggregate_budget_root: AggregateResolutionBudgetRootV1,
}

/// The lower authority installed atomically with the first bound object probe.
pub(crate) struct RemotePhysicalEvidenceAuthorityV1 {
    object: RemotePhysicalObjectBindingV1,
    registry: RemotePhysicalSourceRegistryLeaseV1,
    profile: RemotePhysicalEvidenceProfileV1,
}

impl RemotePhysicalEvidenceAuthorityV1 {
    fn ensure_bound_fixed_layout_v1(
        &self,
        fixed: &crate::remote_fixed_layout::ValidatedFixedLayout<'_>,
    ) -> Result<(), PhysicalSourceResolutionErrorV1> {
        self.object.ensure_open()?;
        if fixed.object_len() != self.object.content_length().get() {
            return Err(PhysicalSourceResolutionErrorV1::ObjectLengthMismatch);
        }
        Ok(())
    }
}

pub struct PreparedBoundRemotePhysicalSourceV1<ValidatorOwner, T> {
    inner: T,
    authority: RemotePhysicalEvidenceAuthorityV1,
    read_issuer: RemoteObjectReadIssuerV1,
    object_lifetime: RemotePhysicalObjectLifetimeV1,
    validator_owner: ValidatorOwner,
}

struct BoundOwnerTransitionGuardV1<ValidatorOwner> {
    authority: Option<RemotePhysicalEvidenceAuthorityV1>,
    read_issuer: Option<RemoteObjectReadIssuerV1>,
    object_lifetime: Option<RemotePhysicalObjectLifetimeV1>,
    validator_owner: Option<ValidatorOwner>,
}

impl<ValidatorOwner> BoundOwnerTransitionGuardV1<ValidatorOwner> {
    fn authority_v1(&self) -> &RemotePhysicalEvidenceAuthorityV1 {
        self.authority
            .as_ref()
            .expect("a live bound transition retains lower authority")
    }

    fn finish_v1(
        mut self,
    ) -> (
        RemotePhysicalEvidenceAuthorityV1,
        RemoteObjectReadIssuerV1,
        RemotePhysicalObjectLifetimeV1,
        ValidatorOwner,
    ) {
        (
            self.authority.take().expect("lower authority is present"),
            self.read_issuer.take().expect("read issuer is present"),
            self.object_lifetime
                .take()
                .expect("object lifetime is present"),
            self.validator_owner
                .take()
                .expect("validator owner is present"),
        )
    }
}

impl<ValidatorOwner> Drop for BoundOwnerTransitionGuardV1<ValidatorOwner> {
    fn drop(&mut self) {
        // The consumed inner backing is gone before this guard. Keep the required lower → object
        // → validator teardown order explicit instead of relying on field drop order.
        drop(self.authority.take());
        drop(self.read_issuer.take());
        drop(self.object_lifetime.take());
        drop(self.validator_owner.take());
    }
}

impl<ValidatorOwner, T> PreparedBoundRemotePhysicalSourceV1<ValidatorOwner, T> {
    fn transition<U, E>(
        self,
        transition: impl FnOnce(&RemotePhysicalEvidenceAuthorityV1, T) -> Result<U, E>,
    ) -> Result<PreparedBoundRemotePhysicalSourceV1<ValidatorOwner, U>, E> {
        let Self {
            inner,
            authority,
            read_issuer,
            object_lifetime,
            validator_owner,
        } = self;
        let guard = BoundOwnerTransitionGuardV1 {
            authority: Some(authority),
            read_issuer: Some(read_issuer),
            object_lifetime: Some(object_lifetime),
            validator_owner: Some(validator_owner),
        };
        let inner = transition(guard.authority_v1(), inner)?;
        let (authority, read_issuer, object_lifetime, validator_owner) = guard.finish_v1();
        Ok(PreparedBoundRemotePhysicalSourceV1 {
            inner,
            authority,
            read_issuer,
            object_lifetime,
            validator_owner,
        })
    }

    fn issue_read_v1<'a>(
        &self,
        slice: crate::remote_fixed_layout::RemoteMcapSlice<'a>,
    ) -> Result<BoundRemoteObjectReadV1<'a>, PhysicalSourceResolutionErrorV1> {
        self.read_issuer.issue_v1(slice)
    }
}

impl<'a, V>
    PreparedBoundRemotePhysicalSourceV1<V, crate::remote_fixed_layout::ValidatedFixedLayout<'a>>
{
    pub(crate) fn prepare_summary_v1(
        self,
        header_read: BoundRemoteObjectReadV1<'a>,
    ) -> Result<
        PreparedBoundRemotePhysicalSourceV1<V, crate::remote_summary::PreparedSummaryRecords<'a>>,
        crate::remote_summary::SummaryCensusError,
    > {
        self.transition(|authority, fixed| {
            let header_read = header_read
                .into_slice_v1(&authority.object)
                .map_err(|_error| {
                    crate::remote_summary::SummaryCensusError::HeaderReadRangeMismatch
                })?;
            crate::remote_summary::prepare_summary_records(
                fixed,
                header_read,
                &authority.profile.summary_census_limits,
            )
        })
    }
}

impl<'a, V>
    PreparedBoundRemotePhysicalSourceV1<V, crate::remote_summary::PreparedSummaryRecords<'a>>
{
    pub(crate) fn preflight_materialization_v1(
        self,
    ) -> Result<
        PreparedBoundRemotePhysicalSourceV1<
            V,
            crate::remote_summary::materialization::SummaryMaterializationToken<'a>,
        >,
        crate::remote_summary::materialization::SummaryMaterializationError,
    > {
        self.transition(|authority, prepared| {
            crate::remote_summary::materialization::preflight_summary_records(
                prepared,
                &authority.profile.summary_materialization_budget,
            )
        })
    }
}

impl<'a, V>
    PreparedBoundRemotePhysicalSourceV1<
        V,
        crate::remote_summary::materialization::SummaryMaterializationToken<'a>,
    >
{
    pub(crate) fn materialize_summary_v1(
        self,
    ) -> Result<
        PreparedBoundRemotePhysicalSourceV1<
            V,
            crate::remote_summary::materialization::MaterializedSummaryRecords<'a>,
        >,
        crate::remote_summary::materialization::SummaryMaterializationError,
    > {
        self.transition(|_, token| {
            crate::remote_summary::materialization::materialize_summary_records(token)
        })
    }
}

impl<'a, V>
    PreparedBoundRemotePhysicalSourceV1<
        V,
        crate::remote_summary::materialization::MaterializedSummaryRecords<'a>,
    >
{
    pub(crate) fn validate_definitions_v1(
        self,
    ) -> Result<
        PreparedBoundRemotePhysicalSourceV1<
            V,
            crate::remote_summary::definitions::ValidatedSummaryDefinitions<'a>,
        >,
        crate::remote_summary::definitions::DefinitionConsistencyError,
    > {
        self.transition(|_, materialized| {
            crate::remote_summary::definitions::validate_summary_definitions(materialized)
        })
    }
}

impl<'a, V>
    PreparedBoundRemotePhysicalSourceV1<
        V,
        crate::remote_summary::definitions::ValidatedSummaryDefinitions<'a>,
    >
{
    pub(crate) fn validate_physical_regions_v1(
        self,
    ) -> Result<
        PreparedBoundRemotePhysicalSourceV1<
            V,
            crate::remote_summary::physical_regions::ValidatedPhysicalRegions<'a>,
        >,
        crate::remote_summary::physical_regions::IndexConsistencyViolation,
    > {
        self.transition(|authority, definitions| {
            crate::remote_summary::physical_regions::validate_physical_regions(
                definitions,
                &authority.profile.physical_region_budget,
            )
        })
    }
}

impl<'a, V>
    PreparedBoundRemotePhysicalSourceV1<
        V,
        crate::remote_summary::physical_regions::ValidatedPhysicalRegions<'a>,
    >
{
    pub(crate) fn prepare_ambiguous_zero_v1(
        self,
    ) -> Result<
        PreparedBoundRemotePhysicalSourceV1<
            V,
            crate::remote_summary::ambiguous_zero::AmbiguousZeroStage1<'a>,
        >,
        crate::remote_summary::physical_regions::IndexConsistencyViolation,
    > {
        self.transition(|authority, physical| {
            crate::remote_summary::ambiguous_zero::prepare_ambiguous_zero_stage1(
                physical,
                &authority.profile.ambiguous_zero_budget,
                &authority.profile.message_index_budget,
            )
        })
    }
}

pub(crate) enum BoundAmbiguousZeroTurnV1<'a, V> {
    MessageIndex(
        PreparedBoundRemotePhysicalSourceV1<
            V,
            crate::remote_summary::ambiguous_zero::PreparedAmbiguousZeroMessageIndex<'a>,
        >,
    ),
    BodyPlan(PreparedBoundRemotePhysicalSourceV1<V, PreparedAmbiguousZeroBodyPlan<'a>>),
}

impl<'a, V>
    PreparedBoundRemotePhysicalSourceV1<
        V,
        crate::remote_summary::ambiguous_zero::PreparedAmbiguousZeroMessageIndex<'a>,
    >
{
    pub(crate) fn expected_message_index_range_v1(&self) -> std::ops::Range<u64> {
        self.inner.expected_range()
    }

    pub(crate) fn install_and_execute_message_index_v1(
        self,
        read: BoundRemoteObjectReadV1<'_>,
    ) -> Result<
        PreparedBoundRemotePhysicalSourceV1<
            V,
            crate::remote_summary::ambiguous_zero::AmbiguousZeroStage1<'a>,
        >,
        crate::remote_summary::physical_regions::IndexConsistencyViolation,
    > {
        self.transition(|authority, prepared| {
            let read = read.into_slice_v1(&authority.object).map_err(|_error| {
                crate::remote_summary::physical_regions::IndexConsistencyViolation::ArithmeticOverflow
            })?;
            prepared.install_raw(read.offset(), read.bytes().to_vec().into_boxed_slice())?.execute()
        })
    }
}

impl<'a, V>
    PreparedBoundRemotePhysicalSourceV1<
        V,
        crate::remote_summary::ambiguous_zero::AmbiguousZeroStage1<'a>,
    >
{
    pub(crate) fn next_ambiguous_zero_v1(
        self,
    ) -> Result<
        BoundAmbiguousZeroTurnV1<'a, V>,
        crate::remote_summary::physical_regions::IndexConsistencyViolation,
    > {
        let Self {
            inner: stage,
            authority,
            read_issuer,
            object_lifetime,
            validator_owner,
        } = self;
        let guard = BoundOwnerTransitionGuardV1 {
            authority: Some(authority),
            read_issuer: Some(read_issuer),
            object_lifetime: Some(object_lifetime),
            validator_owner: Some(validator_owner),
        };
        let next = stage.next()?;
        let (authority, read_issuer, object_lifetime, validator_owner) = guard.finish_v1();
        match next {
            crate::remote_summary::ambiguous_zero::AmbiguousZeroStage1Turn::MessageIndex(value) => {
                Ok(BoundAmbiguousZeroTurnV1::MessageIndex(
                    PreparedBoundRemotePhysicalSourceV1 {
                        inner: value,
                        authority,
                        read_issuer,
                        object_lifetime,
                        validator_owner,
                    },
                ))
            }
            crate::remote_summary::ambiguous_zero::AmbiguousZeroStage1Turn::BodyPlan(value) => Ok(
                BoundAmbiguousZeroTurnV1::BodyPlan(PreparedBoundRemotePhysicalSourceV1 {
                    inner: value,
                    authority,
                    read_issuer,
                    object_lifetime,
                    validator_owner,
                }),
            ),
        }
    }
}

impl<ValidatorOwner> PreparedBoundRemotePhysicalSourceV1<ValidatorOwner, ()> {
    #[cfg(any(test, rerun_mcap_phase_a_proof_v1))]
    fn issue_and_prepare_fixed_layout_for_phase_a_measurement_v1<'a>(
        validator_owner: ValidatorOwner,
        content_length: NonZeroU64,
        consistency: RemoteObjectConsistencyClassV1,
        initial_read: crate::remote_fixed_layout::RemoteMcapSlice<'a>,
        footer_read: crate::remote_fixed_layout::RemoteMcapSlice<'a>,
        profile: RemotePhysicalEvidenceProfileV1,
    ) -> Result<
        PreparedBoundRemotePhysicalSourceV1<
            ValidatorOwner,
            crate::remote_fixed_layout::PreparedFixedLayout,
        >,
        PhysicalSourceResolutionErrorV1,
    > {
        let lower = RemotePhysicalObjectBindingV1::issue_for_phase_a_measurement_v1(
            content_length,
            consistency,
        )?;
        let state = Arc::clone(&lower.state);
        let read_issuer = RemoteObjectReadIssuerV1 {
            object: Arc::clone(&state),
        };
        let registry = Arc::new(RemotePhysicalSourceRegistryStateV1);
        // Install the object owner before the first Header/Footer parser sees any bytes.
        let initial_read = read_issuer.issue_v1(initial_read)?.into_slice_v1(&lower)?;
        let footer_read = read_issuer.issue_v1(footer_read)?.into_slice_v1(&lower)?;
        let authority = RemotePhysicalEvidenceAuthorityV1 {
            object: lower,
            registry: RemotePhysicalSourceRegistryLeaseV1 {
                state: Arc::clone(&registry),
            },
            profile,
        };
        let fixed = crate::remote_fixed_layout::prepare_fixed_layout(
            content_length.get(),
            initial_read,
            footer_read,
        )
        .map_err(|_error| PhysicalSourceResolutionErrorV1::InvalidFixedLayout)?;
        Ok(PreparedBoundRemotePhysicalSourceV1 {
            inner: fixed,
            authority,
            read_issuer,
            object_lifetime: RemotePhysicalObjectLifetimeV1 { state },
            validator_owner,
        })
    }

    #[cfg(test)]
    fn issue_and_prepare_fixed_layout_for_test<'a>(
        validator_owner: ValidatorOwner,
        content_length: NonZeroU64,
        consistency: RemoteObjectConsistencyClassV1,
        initial_read: crate::remote_fixed_layout::RemoteMcapSlice<'a>,
        footer_read: crate::remote_fixed_layout::RemoteMcapSlice<'a>,
        profile: RemotePhysicalEvidenceProfileV1,
    ) -> Result<
        PreparedBoundRemotePhysicalSourceV1<
            ValidatorOwner,
            crate::remote_fixed_layout::PreparedFixedLayout,
        >,
        PhysicalSourceResolutionErrorV1,
    > {
        Self::issue_and_prepare_fixed_layout_for_phase_a_measurement_v1(
            validator_owner,
            content_length,
            consistency,
            initial_read,
            footer_read,
            profile,
        )
    }
}

impl<'a, V>
    PreparedBoundRemotePhysicalSourceV1<V, crate::remote_fixed_layout::PreparedFixedLayout>
{
    pub(crate) fn validate_data_end_and_summary_v1(
        self,
        read: BoundRemoteObjectReadV1<'a>,
    ) -> Result<
        PreparedBoundRemotePhysicalSourceV1<
            V,
            crate::remote_fixed_layout::ValidatedFixedLayout<'a>,
        >,
        crate::remote_fixed_layout::FixedLayoutError,
    > {
        self.transition(|authority, prepared| {
            let read = read.into_slice_v1(&authority.object).map_err(|_error| {
                crate::remote_fixed_layout::FixedLayoutError::UnexpectedReadRange
            })?;
            let fixed = prepared.validate_data_end_and_summary(read)?;
            authority
                .ensure_bound_fixed_layout_v1(&fixed)
                .map_err(|_error| {
                    crate::remote_fixed_layout::FixedLayoutError::UnexpectedReadRange
                })?;
            Ok(fixed)
        })
    }
}

impl<'a, ValidatorOwner>
    PreparedBoundRemotePhysicalSourceV1<ValidatorOwner, PreparedAmbiguousZeroBodyPlan<'a>>
{
    pub(crate) fn prepare(
        self,
    ) -> Result<
        BoundPreparedPhysicalSourceResolutionV1<'a, ValidatorOwner>,
        PhysicalSourceResolutionErrorV1,
    > {
        let Self {
            inner,
            authority,
            read_issuer,
            object_lifetime,
            validator_owner,
        } = self;
        drop(read_issuer);
        match PreparedPhysicalSourceResolutionV1::prepare_bound_parts_v1(authority, inner) {
            Ok(inner) => Ok(BoundPreparedPhysicalSourceResolutionV1 {
                inner,
                object_lifetime,
                validator_owner,
            }),
            Err(error) => {
                // The lower evidence has already torn down its byte owners and authority.
                // Keep the upper validator alive until the matching object lifetime is closed.
                drop(object_lifetime);
                drop(validator_owner);
                Err(error)
            }
        }
    }
}

pub struct BoundPreparedPhysicalSourceResolutionV1<'a, ValidatorOwner> {
    inner: PreparedPhysicalSourceResolutionV1<'a>,
    object_lifetime: RemotePhysicalObjectLifetimeV1,
    validator_owner: ValidatorOwner,
}

impl<'a, ValidatorOwner> BoundPreparedPhysicalSourceResolutionV1<'a, ValidatorOwner> {
    pub(crate) fn inner_mut_v1(&mut self) -> &mut PreparedPhysicalSourceResolutionV1<'a> {
        &mut self.inner
    }

    pub(crate) fn finalize_v1(
        self,
    ) -> Result<
        BoundResolvedPhysicalSourceAuthorityV1<'a, ValidatorOwner>,
        PhysicalSourceResolutionErrorV1,
    > {
        let Self {
            inner,
            object_lifetime,
            validator_owner,
        } = self;
        match inner.finalize_v1() {
            Ok(inner) => Ok(BoundResolvedPhysicalSourceAuthorityV1 {
                inner,
                object_lifetime,
                validator_owner,
            }),
            Err(error) => {
                drop(object_lifetime);
                drop(validator_owner);
                Err(error)
            }
        }
    }
}

pub struct BoundResolvedPhysicalSourceAuthorityV1<'a, ValidatorOwner> {
    inner: ResolvedPhysicalSourceAuthorityV1<'a>,
    object_lifetime: RemotePhysicalObjectLifetimeV1,
    validator_owner: ValidatorOwner,
}

impl<ValidatorOwner> BoundResolvedPhysicalSourceAuthorityV1<'_, ValidatorOwner> {
    pub(crate) fn source_v1(&self) -> ResolvedRemotePhysicalSourceRefV1<'_, '_> {
        let _keep_object_open = &self.object_lifetime;
        ResolvedRemotePhysicalSourceRefV1 { inner: &self.inner }
    }
}

#[cfg(target_arch = "wasm32")]
impl<'a, ValidatorOwner> BoundResolvedPhysicalSourceAuthorityV1<'a, ValidatorOwner> {
    /// Exposes the resolved physical source authority for the Web adapter's
    /// `issue_physical_receipt_v1` seam.
    ///
    /// The lease and selector types remain `pub(crate)`; this exposes only the
    /// receipt-issuing authority handle. The returned borrow keeps the bound object
    /// lifetime alive, so the remote object stays open while the authority is held.
    #[cfg(target_arch = "wasm32")]
    pub fn resolved_authority_v1(&self) -> &ResolvedPhysicalSourceAuthorityV1<'a> {
        let _keep_object_open = &self.object_lifetime;
        &self.inner
    }
}

pub(crate) struct ResolvedRemotePhysicalSourceRefV1<'owner, 'input> {
    inner: &'owner ResolvedPhysicalSourceAuthorityV1<'input>,
}

impl ResolvedRemotePhysicalSourceRefV1<'_, '_> {
    pub(crate) fn source_generation_v1(&self) -> u64 {
        self.inner.source_binding.source_generation_v1()
    }
    pub(crate) fn layout_v1(&self) -> &ResolvedCanonicalPhysicalLayoutV1 {
        &self.inner.layout
    }

    pub(crate) fn issue_lease_v1(
        &self,
        ordinal: usize,
    ) -> Result<PhysicalChunkReadLease<'_, PendingHeaderValidation>, PhysicalSourceResolutionErrorV1>
    {
        self.inner.issue_lease_v1(ordinal)
    }

    pub(crate) fn source_unit_v1(
        &self,
        canonical_ordinal: usize,
    ) -> Option<ResolvedRemotePhysicalSourceUnitRefV1<'_, '_>> {
        self.inner
            .layout
            .canonical_classification_v1(canonical_ordinal)?;
        Some(ResolvedRemotePhysicalSourceUnitRefV1 {
            source: self.inner,
            canonical_ordinal,
        })
    }
}

pub(crate) struct ResolvedRemotePhysicalSourceUnitRefV1<'owner, 'input> {
    source: &'owner ResolvedPhysicalSourceAuthorityV1<'input>,
    canonical_ordinal: usize,
}

impl ResolvedRemotePhysicalSourceUnitRefV1<'_, '_> {
    pub(crate) fn canonical_ordinal_v1(&self) -> usize {
        self.canonical_ordinal
    }

    pub(crate) fn classification_v1(&self) -> Option<CanonicalPhysicalExtentV1> {
        self.source
            .layout
            .canonical_classification_v1(self.canonical_ordinal)
    }

    pub(crate) fn canonical_interval_v1(&self) -> Option<(TimeInt, TimeInt)> {
        self.source
            .layout
            .canonical_interval_v1(self.canonical_ordinal)
    }

    pub(crate) fn metadata_v1(&self) -> Option<ResolvedRemotePhysicalSourceUnitMetadataRefV1<'_>> {
        let region = self
            .source
            .authority
            .physical_region_v1(self.canonical_ordinal)?;
        Some(ResolvedRemotePhysicalSourceUnitMetadataRefV1 {
            binding: &self.source.source_binding,
            definitions: self
                .source
                .authority
                .definitions_capability_for_remote_assignment_v1(),
            canonical_ordinal: self.canonical_ordinal,
            region,
        })
    }

    pub(crate) fn issue_lease_v1(
        &self,
    ) -> Result<PhysicalChunkReadLease<'_, PendingHeaderValidation>, PhysicalSourceResolutionErrorV1>
    {
        self.source.issue_lease_v1(self.canonical_ordinal)
    }
}

pub(crate) struct ResolvedRemotePhysicalSourceUnitMetadataRefV1<'a> {
    binding: &'a PhysicalChunkSourceBindingV1,
    definitions: PhysicalChunkDefinitionsCapabilityV1<'a, 'a>,
    canonical_ordinal: usize,
    region: crate::remote_summary::physical_regions::CanonicalPhysicalRegion<'a>,
}

pub(crate) enum ResolvedDefinitionDescriptorV1<'a> {
    Schema {
        id: u16,
        name: &'a str,
        encoding: &'a str,
        data: &'a [u8],
    },
    Channel {
        id: u16,
        schema_id: u16,
        topic: &'a str,
        message_encoding: &'a str,
    },
}

impl ResolvedRemotePhysicalSourceUnitMetadataRefV1<'_> {
    #[cfg(test)]
    pub(crate) fn definitions_capability_for_full_chain_test_v1(
        &self,
    ) -> PhysicalChunkDefinitionsCapabilityV1<'_, '_> {
        self.definitions
            .source_binding_v1()
            .ensure_matches_v1(self.binding);
        self.definitions.clone()
    }

    pub(crate) fn canonical_ordinal_v1(&self) -> usize {
        self.canonical_ordinal
    }

    pub(crate) fn chunk_range_v1(&self) -> std::ops::Range<u64> {
        self.region.chunk_range()
    }

    pub(crate) fn message_index_region_v1(&self) -> std::ops::Range<u64> {
        self.region.message_index_region()
    }

    pub(crate) fn unit_range_v1(&self) -> std::ops::Range<u64> {
        self.region.unit_range()
    }

    pub(crate) fn compression_v1(&self) -> &str {
        &self.region.raw_descriptor().compression
    }

    pub(crate) fn compressed_size_v1(&self) -> u64 {
        self.region.raw_descriptor().compressed_size
    }

    pub(crate) fn uncompressed_size_v1(&self) -> u64 {
        self.region.raw_descriptor().uncompressed_size
    }

    pub(crate) fn message_index_offsets_v1(&self) -> &std::collections::BTreeMap<u16, u64> {
        &self.region.raw_descriptor().message_index_offsets
    }

    pub(crate) fn source_is_current_v1(&self) -> bool {
        self.binding.ensure_current_v1().is_ok()
    }

    pub(crate) fn definition_descriptors_v1(
        &self,
    ) -> impl Iterator<Item = ResolvedDefinitionDescriptorV1<'_>> + '_ {
        self.definitions
            .source_binding_v1()
            .ensure_matches_v1(self.binding);
        self.definitions
            .definitions_v1()
            .projection_records()
            .filter_map(|record| {
                match record {
                crate::remote_summary::definitions::SummaryDefinitionProjectionRecord::Schema {
                    id,
                    record_index,
                } => {
                    let schema = self
                        .definitions
                        .definitions_v1()
                        .schema_at_record(record_index)?;
                    Some(ResolvedDefinitionDescriptorV1::Schema {
                        id,
                        name: &schema.header.name,
                        encoding: &schema.header.encoding,
                        data: schema.data,
                    })
                }
                crate::remote_summary::definitions::SummaryDefinitionProjectionRecord::Channel {
                    id,
                    record_index,
                } => {
                    let channel = self
                        .definitions
                        .definitions_v1()
                        .channel_at_record(record_index)?;
                    Some(ResolvedDefinitionDescriptorV1::Channel {
                        id,
                        schema_id: channel.schema_id,
                        topic: &channel.topic,
                        message_encoding: &channel.message_encoding,
                    })
                }
            }
            })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanonicalPhysicalExtentV1 {
    KnownEmpty,
    Known { start: TimeInt, end: TimeInt },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CanonicalIntervalV1 {
    start: TimeInt,
    end: TimeInt,
    canonical_ordinal: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResolutionClassificationV1 {
    Pending,
    KnownEmpty,
    NonEmpty { start: TimeInt, end: TimeInt },
}

/// Immutable layout retained by the resolved authority and available only by borrow.
pub struct ResolvedCanonicalPhysicalLayoutV1 {
    classifications: Box<[ResolutionClassificationV1]>,
    intervals: Box<[CanonicalIntervalV1]>,
    prefix_max_end: Box<[TimeInt]>,
    interval_count: usize,
    extent: CanonicalPhysicalExtentV1,
}

impl ResolvedCanonicalPhysicalLayoutV1 {
    pub fn canonical_extent_v1(&self) -> CanonicalPhysicalExtentV1 {
        self.extent
    }

    pub fn canonical_chunk_count_v1(&self) -> usize {
        self.classifications.len()
    }

    pub(crate) fn canonical_classification_v1(
        &self,
        canonical_ordinal: usize,
    ) -> Option<CanonicalPhysicalExtentV1> {
        match self.classifications.get(canonical_ordinal)? {
            ResolutionClassificationV1::Pending => None,
            ResolutionClassificationV1::KnownEmpty => Some(CanonicalPhysicalExtentV1::KnownEmpty),
            ResolutionClassificationV1::NonEmpty { start, end } => {
                Some(CanonicalPhysicalExtentV1::Known {
                    start: *start,
                    end: *end,
                })
            }
        }
    }

    pub(crate) fn canonical_interval_v1(
        &self,
        canonical_ordinal: usize,
    ) -> Option<(TimeInt, TimeInt)> {
        match self.classifications.get(canonical_ordinal)? {
            ResolutionClassificationV1::NonEmpty { start, end } => Some((*start, *end)),
            ResolutionClassificationV1::Pending | ResolutionClassificationV1::KnownEmpty => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_window_planner_test_v1(intervals: &[(i64, i64)]) -> Self {
        let count = intervals.len();
        let mut classifications = Vec::with_capacity(count);
        let mut sorted_intervals = Vec::with_capacity(count);
        let mut prefix_max_end = Vec::with_capacity(count);
        for &(start, end) in intervals {
            classifications.push(ResolutionClassificationV1::NonEmpty {
                start: TimeInt::new_temporal(start),
                end: TimeInt::new_temporal(end),
            });
        }
        for (ordinal, classification) in classifications.iter().copied().enumerate() {
            if let ResolutionClassificationV1::NonEmpty { start, end } = classification {
                sorted_intervals.push(CanonicalIntervalV1 {
                    start,
                    end,
                    canonical_ordinal: ordinal,
                });
            }
        }
        sorted_intervals.sort_unstable_by_key(|interval| {
            (interval.start, interval.end, interval.canonical_ordinal)
        });
        let mut max_end = TimeInt::MIN;
        for interval in &sorted_intervals {
            max_end = max_end.max(interval.end);
            prefix_max_end.push(max_end);
        }
        let extent = sorted_intervals.iter().fold(
            CanonicalPhysicalExtentV1::KnownEmpty,
            |extent, interval| match extent {
                CanonicalPhysicalExtentV1::KnownEmpty => CanonicalPhysicalExtentV1::Known {
                    start: interval.start,
                    end: interval.end,
                },
                CanonicalPhysicalExtentV1::Known { start, end } => {
                    CanonicalPhysicalExtentV1::Known {
                        start: start.min(interval.start),
                        end: end.max(interval.end),
                    }
                }
            },
        );
        Self {
            classifications: classifications.into_boxed_slice(),
            intervals: sorted_intervals.into_boxed_slice(),
            prefix_max_end: prefix_max_end.into_boxed_slice(),
            interval_count: count,
            extent,
        }
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

#[derive(Clone, Copy, Debug)]
pub struct PhysicalSourceResolutionLimitsV1 {
    max_canonical_chunks: u64,
    max_retained_bytes: u64,
}

impl PhysicalSourceResolutionLimitsV1 {
    #[cfg(any(test, re_mcap_locked_remote_wasm_allocator_v1))]
    pub const fn new_for_sealed_profile_v1(
        max_canonical_chunks: u64,
        max_retained_bytes: u64,
    ) -> Self {
        Self {
            max_canonical_chunks,
            max_retained_bytes,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PhysicalSourceResolutionUsageV1 {
    active: u64,
    canonical_chunks: u64,
    retained_bytes: u64,
}

struct PhysicalSourceResolutionBudgetStateV1 {
    limits: PhysicalSourceResolutionLimitsV1,
    max_active: u64,
    max_aggregate_chunks: u64,
    max_aggregate_bytes: u64,
    usage: Mutex<PhysicalSourceResolutionUsageV1>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PhysicalSourceResolutionCensusV1 {
    canonical_chunks: u64,
    retained_bytes: u64,
}

impl PhysicalSourceResolutionCensusV1 {
    fn aggregate_peak_bytes_v1(
        self,
        authority_slot_bytes: u64,
        definition_projection_bytes: u64,
        existing_evidence_bytes: u64,
    ) -> Result<u64, PhysicalSourceResolutionErrorV1> {
        self.retained_bytes
            .checked_add(authority_slot_bytes)
            .and_then(|value| value.checked_add(definition_projection_bytes))
            .and_then(|value| value.checked_add(existing_evidence_bytes))
            .ok_or(PhysicalSourceResolutionErrorV1::ArithmeticOverflow)
    }
}

impl PhysicalSourceResolutionCensusV1 {
    fn checked(
        canonical_chunks: u64,
        unresolved_chunks: u64,
    ) -> Result<Self, PhysicalSourceResolutionErrorV1> {
        let canonical = usize::try_from(canonical_chunks)
            .map_err(|_layout_error| PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?;
        let unresolved = usize::try_from(unresolved_chunks)
            .map_err(|_layout_error| PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?;
        let canonical_backing = locked_allocation_footprint_v1(
            Layout::array::<ResolutionClassificationV1>(canonical)
                .map_err(|_layout_error| PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?,
        )?
        .checked_add(locked_allocation_footprint_v1(
            Layout::array::<CanonicalIntervalV1>(canonical)
                .map_err(|_layout_error| PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?,
        )?)
        .and_then(|value| {
            locked_allocation_footprint_v1(Layout::array::<TimeInt>(canonical).ok()?)
                .ok()?
                .checked_add(value)
        })
        .ok_or(PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?;
        let unresolved_backing = locked_allocation_footprint_v1(
            Layout::array::<usize>(unresolved)
                .map_err(|_layout_error| PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?,
        )?;
        let retained_bytes = canonical_backing
            .checked_add(unresolved_backing)
            .ok_or(PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?;
        Ok(Self {
            canonical_chunks,
            retained_bytes,
        })
    }
}

fn locked_allocation_footprint_v1(layout: Layout) -> Result<u64, PhysicalSourceResolutionErrorV1> {
    // The locked Web allocator uses a fixed metadata word and rounds each backing to its required
    // alignment. Keep this helper shared by census tests rather than charging Rust pointer sizes.
    let size = layout.size().max(1);
    let aligned = size
        .checked_add(layout.align().saturating_sub(1))
        .ok_or(PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?
        / layout.align()
        * layout.align();
    u64::try_from(
        aligned
            .checked_add(size_of::<usize>())
            .ok_or(PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?,
    )
    .map_err(|_layout_error| PhysicalSourceResolutionErrorV1::ArithmeticOverflow)
}

fn locked_owned_layout_footprint_v1<T>() -> Result<u64, PhysicalSourceResolutionErrorV1> {
    locked_allocation_footprint_v1(Layout::new::<T>())
}

pub struct PhysicalSourceResolutionBudgetV1 {
    state: Arc<PhysicalSourceResolutionBudgetStateV1>,
}

/// One remote-only aggregate root for the full evidence-to-final-layout simultaneous peak.
///
/// Existing lower-stage reservations remain responsible for their own detailed limits, while
/// this root prevents independently valid stages from exceeding the source-wide peak when held
/// together by MCAP-025A.
pub struct AggregateResolutionBudgetRootV1 {
    state: Arc<AggregateResolutionBudgetStateV1>,
}

/// Allocation-free admission census for one complete 019→025A resolution.
///
/// All fields are checked before any child allocator is touched.  The census intentionally
/// counts the simultaneous peak (retained evidence, projections, authority slots and the new
/// layout) rather than only the final layout size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FullResolutionAdmissionCensusV1 {
    pub(crate) canonical_chunks: u64,
    pub(crate) unresolved_chunks: u64,
    pub(crate) retained_evidence_bytes: u64,
    pub(crate) authority_slot_bytes: u64,
    pub(crate) definition_projection_bytes: u64,
    pub(crate) layout_bytes: u64,
}

impl FullResolutionAdmissionCensusV1 {
    pub(crate) fn checked(
        canonical_chunks: u64,
        unresolved_chunks: u64,
        retained_evidence_bytes: u64,
        authority_slot_bytes: u64,
        definition_projection_bytes: u64,
    ) -> Result<Self, PhysicalSourceResolutionErrorV1> {
        let layout_bytes =
            PhysicalSourceResolutionCensusV1::checked(canonical_chunks, unresolved_chunks)?
                .retained_bytes;
        // Keep the checked sum in one place so callers cannot accidentally under-account an
        // overlap between old evidence and the newly materialized layout.
        let _peak = retained_evidence_bytes
            .checked_add(authority_slot_bytes)
            .and_then(|v| v.checked_add(definition_projection_bytes))
            .and_then(|v| v.checked_add(layout_bytes))
            .ok_or(PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?;
        Ok(Self {
            canonical_chunks,
            unresolved_chunks,
            retained_evidence_bytes,
            authority_slot_bytes,
            definition_projection_bytes,
            layout_bytes,
        })
    }

    fn peak_bytes(self) -> Result<u64, PhysicalSourceResolutionErrorV1> {
        self.retained_evidence_bytes
            .checked_add(self.authority_slot_bytes)
            .and_then(|v| v.checked_add(self.definition_projection_bytes))
            .and_then(|v| v.checked_add(self.layout_bytes))
            .ok_or(PhysicalSourceResolutionErrorV1::ArithmeticOverflow)
    }

    pub(crate) fn checked_with_lower_stage_footprints_v1(
        canonical_chunks: u64,
        unresolved_chunks: u64,
        lower: &PreparedAmbiguousZeroBodyPlan<'_>,
        authority_slot_bytes: u64,
        definition_projection_bytes: u64,
        owner_overhead_bytes: u64,
    ) -> Result<Self, PhysicalSourceResolutionErrorV1> {
        let evidence = lower
            .retained_resolution_evidence_bytes_v1()
            .map_err(|_layout_error| PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?;
        let evidence = evidence
            .checked_add(owner_overhead_bytes)
            .ok_or(PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?;
        Self::checked(
            canonical_chunks,
            unresolved_chunks,
            evidence,
            authority_slot_bytes,
            definition_projection_bytes,
        )
    }
}

struct AggregateResolutionBudgetStateV1 {
    max_active: u64,
    max_retained_bytes: u64,
    usage: Mutex<AggregateResolutionBudgetUsageV1>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct AggregateResolutionBudgetUsageV1 {
    active: u64,
    retained_bytes: u64,
}

struct AggregateResolutionReservationV1 {
    state: Arc<AggregateResolutionBudgetStateV1>,
    retained_bytes: u64,
}

/// Move-only token proving that aggregate admission succeeded.  Child allocations may only be
/// consumed while this token is held.
pub(crate) struct AggregateResolutionTransactionV1 {
    reservation: AggregateResolutionReservationV1,
    authority_claim: PreparedPhysicalChunkAuthorityClaimV1,
    census: FullResolutionAdmissionCensusV1,
}

impl AggregateResolutionTransactionV1 {
    pub(crate) fn census_v1(&self) -> FullResolutionAdmissionCensusV1 {
        self.census
    }

    fn into_parts_v1(
        self,
    ) -> (
        AggregateResolutionReservationV1,
        PreparedPhysicalChunkAuthorityClaimV1,
    ) {
        (self.reservation, self.authority_claim)
    }
}

impl Drop for AggregateResolutionReservationV1 {
    fn drop(&mut self) {
        let mut usage = self.state.usage.lock();
        usage.active -= 1;
        usage.retained_bytes -= self.retained_bytes;
    }
}

impl AggregateResolutionBudgetRootV1 {
    #[cfg(any(test, rerun_mcap_phase_a_proof_v1))]
    pub(crate) fn new_for_sealed_profile_v1(max_active: u64, max_retained_bytes: u64) -> Self {
        Self {
            state: Arc::new(AggregateResolutionBudgetStateV1 {
                max_active,
                max_retained_bytes,
                usage: Mutex::new(AggregateResolutionBudgetUsageV1::default()),
            }),
        }
    }

    fn reserve_v1(
        &self,
        retained_bytes: u64,
    ) -> Result<AggregateResolutionReservationV1, PhysicalSourceResolutionErrorV1> {
        let mut usage = self.state.usage.lock();
        let next = AggregateResolutionBudgetUsageV1 {
            active: usage
                .active
                .checked_add(1)
                .ok_or(PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?,
            retained_bytes: usage
                .retained_bytes
                .checked_add(retained_bytes)
                .ok_or(PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?,
        };
        if next.active > self.state.max_active
            || next.retained_bytes > self.state.max_retained_bytes
        {
            return Err(PhysicalSourceResolutionErrorV1::ReservationLimitExceeded);
        }
        *usage = next;
        Ok(AggregateResolutionReservationV1 {
            state: Arc::clone(&self.state),
            retained_bytes,
        })
    }

    fn claim_transaction_v1(
        &self,
        census: FullResolutionAdmissionCensusV1,
        scan_budget: &PhysicalChunkScanBudget,
        source_generation: NonZeroU64,
    ) -> Result<AggregateResolutionTransactionV1, PhysicalSourceResolutionErrorV1> {
        let retained_bytes = census.peak_bytes()?;
        let reservation = self.reserve_v1(retained_bytes)?;
        let authority_claim = PreparedPhysicalChunkAuthorityContextV1::issue_child_claim_v1(
            scan_budget,
            census.canonical_chunks,
            census.authority_slot_bytes,
            census.definition_projection_bytes,
            source_generation,
        )
        .map_err(PhysicalSourceResolutionErrorV1::Physical)?;
        Ok(AggregateResolutionTransactionV1 {
            reservation,
            authority_claim,
            census,
        })
    }

    #[cfg(test)]
    fn usage_for_test_v1(&self) -> AggregateResolutionBudgetUsageV1 {
        *self.state.usage.lock()
    }

    #[cfg(test)]
    fn clone_for_test_v1(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
        }
    }
}

struct PhysicalSourceResolutionReservationV1 {
    state: Arc<PhysicalSourceResolutionBudgetStateV1>,
    canonical_chunks: u64,
    retained_bytes: u64,
}

impl Drop for PhysicalSourceResolutionReservationV1 {
    fn drop(&mut self) {
        let mut usage = self.state.usage.lock();
        usage.active -= 1;
        usage.canonical_chunks -= self.canonical_chunks;
        usage.retained_bytes -= self.retained_bytes;
    }
}

impl PhysicalSourceResolutionBudgetV1 {
    #[cfg(any(test, re_mcap_locked_remote_wasm_allocator_v1))]
    pub(crate) fn new_for_sealed_profile_v1(
        limits: PhysicalSourceResolutionLimitsV1,
        max_active: u64,
        max_aggregate_chunks: u64,
        max_aggregate_bytes: u64,
    ) -> Self {
        Self {
            state: Arc::new(PhysicalSourceResolutionBudgetStateV1 {
                limits,
                max_active,
                max_aggregate_chunks,
                max_aggregate_bytes,
                usage: Mutex::new(PhysicalSourceResolutionUsageV1::default()),
            }),
        }
    }

    fn reserve(
        &self,
        census: PhysicalSourceResolutionCensusV1,
    ) -> Result<PhysicalSourceResolutionReservationV1, PhysicalSourceResolutionErrorV1> {
        let PhysicalSourceResolutionCensusV1 {
            canonical_chunks,
            retained_bytes,
        } = census;
        let mut usage = self.state.usage.lock();
        let next = PhysicalSourceResolutionUsageV1 {
            active: usage
                .active
                .checked_add(1)
                .ok_or(PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?,
            canonical_chunks: usage
                .canonical_chunks
                .checked_add(canonical_chunks)
                .ok_or(PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?,
            retained_bytes: usage
                .retained_bytes
                .checked_add(retained_bytes)
                .ok_or(PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?,
        };
        let limits = self.state.limits;
        if canonical_chunks > limits.max_canonical_chunks
            || retained_bytes > limits.max_retained_bytes
            || next.active > self.state.max_active
            || next.canonical_chunks > self.state.max_aggregate_chunks
            || next.retained_bytes > self.state.max_aggregate_bytes
        {
            return Err(PhysicalSourceResolutionErrorV1::ReservationLimitExceeded);
        }
        *usage = next;
        Ok(PhysicalSourceResolutionReservationV1 {
            state: Arc::clone(&self.state),
            canonical_chunks,
            retained_bytes,
        })
    }
}

/// Sealed lease-bound result of the complete MCAP-025 scan chain.
pub struct AmbiguousPhysicalExtentResolutionV1<'a> {
    scan: ValidatedPhysicalChunkScan<'a>,
    binding: PhysicalChunkSourceBindingV1,
    canonical_ordinal: usize,
    extent: ValidatedPhysicalChunkExtent,
}

impl<'a> AmbiguousPhysicalExtentResolutionV1<'a> {
    pub(crate) fn from_scan(
        scan: ValidatedPhysicalChunkScan<'a>,
    ) -> Result<Self, PhysicalSourceResolutionErrorV1> {
        let binding = scan
            .source_binding_v1()
            .map_err(PhysicalSourceResolutionErrorV1::Physical)?;
        let canonical_ordinal = scan
            .canonical_ordinal_v1()
            .map_err(PhysicalSourceResolutionErrorV1::Physical)?;
        let extent = scan.extent_for_resolution_v1();
        match extent {
            ValidatedPhysicalChunkExtent::KnownEmpty
            | ValidatedPhysicalChunkExtent::NonEmpty {
                raw_start: 0,
                raw_end: 0,
                ..
            } => {}
            ValidatedPhysicalChunkExtent::NonEmpty { .. } => {
                return Err(PhysicalSourceResolutionErrorV1::AmbiguousExtentNotZero);
            }
        }
        Ok(Self {
            scan,
            binding,
            canonical_ordinal,
            extent,
        })
    }
}

/// The move-only coordinator. No layout borrow exists before successful finalization.
pub struct PreparedPhysicalSourceResolutionV1<'a> {
    object: RemotePhysicalObjectBindingV1,
    registry: RemotePhysicalSourceRegistryLeaseV1,
    authority: PhysicalChunkSourceAuthority<'a>,
    source_binding: PhysicalChunkSourceBindingV1,
    classifications: Box<[ResolutionClassificationV1]>,
    intervals: Box<[CanonicalIntervalV1]>,
    prefix_max_end: Box<[TimeInt]>,
    unresolved: Box<[usize]>,
    next_unresolved: usize,
    reservation: AggregateResolutionReservationV1,
    aggregate_reservations: PreparedAmbiguousZeroAggregateReservations,
    close_guard: AuthorityCloseGuardV1,
}

struct AuthorityCloseGuardV1 {
    binding: PhysicalChunkSourceBindingV1,
    armed: bool,
}

impl AuthorityCloseGuardV1 {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for AuthorityCloseGuardV1 {
    fn drop(&mut self) {
        if self.armed {
            self.binding.close_v1();
        }
    }
}

impl<'a> PreparedPhysicalSourceResolutionV1<'a> {
    fn prepare_bound_parts_v1(
        authority: RemotePhysicalEvidenceAuthorityV1,
        plan: PreparedAmbiguousZeroBodyPlan<'a>,
    ) -> Result<Self, PhysicalSourceResolutionErrorV1> {
        let RemotePhysicalEvidenceAuthorityV1 {
            object,
            registry,
            profile,
        } = authority;
        let RemotePhysicalEvidenceProfileV1 {
            summary_census_limits: _,
            summary_materialization_budget: _,
            physical_region_budget: _,
            message_index_budget: _,
            ambiguous_zero_budget: _,
            decompression_budget,
            scan_budget,
            aggregate_budget_root,
        } = profile;
        object.ensure_open()?;
        // Complete all fallible semantic/order/range validation and the allocation-free census
        // while lower evidence remains untouched.  Aggregate rejection must therefore leave the
        // complete 019→023 ownership graph bitwise unchanged and perform no backing allocation.
        let physical = plan.physical();
        if physical
            .definitions()
            .materialized()
            .prepared()
            .fixed_layout()
            .object_len()
            != object.content_length().get()
        {
            return Err(PhysicalSourceResolutionErrorV1::ObjectLengthMismatch);
        }
        for region in physical.regions() {
            if region.unit_range().end > object.content_length().get() {
                return Err(PhysicalSourceResolutionErrorV1::ObjectLengthMismatch);
            }
        }
        let count = physical.canonical_chunk_count();
        let count_u64 = u64::try_from(count)
            .map_err(|_overflow| PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?;
        let unresolved_count = plan.unresolved_count();
        let ambiguous_chunks = plan.resolution_classifications_v1();
        let unresolved_u64 = u64::try_from(unresolved_count)
            .map_err(|_overflow| PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?;
        let (authority_slot_bytes, definition_projection_bytes) =
            PhysicalChunkSourceAuthority::allocation_census_v1(physical, &scan_budget)
                .map_err(PhysicalSourceResolutionErrorV1::Physical)?;
        let owner_overhead_bytes = [
            locked_owned_layout_footprint_v1::<RemotePhysicalObjectStateV1>()?,
            locked_owned_layout_footprint_v1::<RemotePhysicalSourceRegistryStateV1>()?,
            locked_owned_layout_footprint_v1::<RemotePhysicalEvidenceProfileV1>()?,
            locked_owned_layout_footprint_v1::<AggregateResolutionTransactionV1>()?,
            locked_owned_layout_footprint_v1::<PreparedPhysicalSourceResolutionV1<'a>>()?,
            locked_owned_layout_footprint_v1::<ResolvedPhysicalSourceAuthorityV1<'a>>()?,
            locked_owned_layout_footprint_v1::<AggregateResolutionBudgetStateV1>()?,
        ]
        .into_iter()
        .try_fold(0u64, |sum, value| {
            sum.checked_add(value)
                .ok_or(PhysicalSourceResolutionErrorV1::ArithmeticOverflow)
        })?;
        let full_census = FullResolutionAdmissionCensusV1::checked_with_lower_stage_footprints_v1(
            count_u64,
            unresolved_u64,
            &plan,
            authority_slot_bytes,
            definition_projection_bytes,
            owner_overhead_bytes,
        )?;
        let source_generation = object.generation();
        let zero = canonicalize_raw_mcap_time(RawMcapTime::new(0))
            .map_err(|_invalid| PhysicalSourceResolutionErrorV1::InvalidTemporalValue)?;
        let mut ambiguous_index = 0;
        let mut unresolved_index = 0;
        for ordinal in 0..count {
            let ambiguous = ambiguous_chunks.get(ambiguous_index).copied();
            let _ = if ambiguous.is_some_and(|chunk| chunk.canonical_ordinal == ordinal) {
                ambiguous_index += 1;
                let Some(ambiguous) = ambiguous else {
                    return Err(PhysicalSourceResolutionErrorV1::InvalidPlan);
                };
                match ambiguous.classification {
                    AmbiguousChunkClassification::IndexNonEmptyZero => {
                        ResolutionClassificationV1::NonEmpty {
                            start: zero,
                            end: zero,
                        }
                    }
                    AmbiguousChunkClassification::Unresolved => {
                        unresolved_index += 1;
                        ResolutionClassificationV1::Pending
                    }
                    AmbiguousChunkClassification::PendingMessageIndex => {
                        return Err(PhysicalSourceResolutionErrorV1::InvalidPlan);
                    }
                }
            } else {
                let descriptor = physical
                    .region(ordinal)
                    .ok_or(PhysicalSourceResolutionErrorV1::InvalidPlan)?
                    .raw_descriptor();
                ResolutionClassificationV1::NonEmpty {
                    start: canonicalize_raw_mcap_time(RawMcapTime::new(
                        descriptor.message_start_time,
                    ))
                    .map_err(|_invalid| PhysicalSourceResolutionErrorV1::InvalidTemporalValue)?,
                    end: canonicalize_raw_mcap_time(RawMcapTime::new(descriptor.message_end_time))
                        .map_err(|_invalid| {
                            PhysicalSourceResolutionErrorV1::InvalidTemporalValue
                        })?,
                }
            };
        }
        if ambiguous_index != ambiguous_chunks.len() || unresolved_index != unresolved_count {
            return Err(PhysicalSourceResolutionErrorV1::InvalidPlan);
        }
        let PreparedAmbiguousZeroResolutionSeed {
            physical,
            ambiguous_chunks,
            aggregate_reservations,
        } = plan.into_resolution_seed();
        let transaction = aggregate_budget_root.claim_transaction_v1(
            full_census,
            &scan_budget,
            source_generation,
        )?;
        let authority_allocation =
            PhysicalChunkSourceAuthority::prepare_context_v1(&physical, &scan_budget)
                .map_err(PhysicalSourceResolutionErrorV1::Physical)?;
        if !authority_allocation.matches_scan_budget_v1(&scan_budget) {
            return Err(PhysicalSourceResolutionErrorV1::RegistryBindingMismatch);
        }
        let mut classifications = try_filled(count, ResolutionClassificationV1::Pending)?;
        let intervals = try_filled(
            count,
            CanonicalIntervalV1 {
                start: TimeInt::MIN,
                end: TimeInt::MIN,
                canonical_ordinal: 0,
            },
        )?;
        let prefix_max_end = try_filled(count, TimeInt::MIN)?;
        let mut unresolved = try_filled(unresolved_count, 0usize)?;
        let mut classification_cursor = 0usize;
        let mut unresolved_cursor = 0usize;
        for (ordinal, classification) in classifications.iter_mut().enumerate() {
            let ambiguous = ambiguous_chunks.get(classification_cursor).copied();
            *classification = if ambiguous.is_some_and(|value| value.canonical_ordinal == ordinal) {
                classification_cursor += 1;
                match ambiguous.expect("validated ambiguous entry").classification {
                    AmbiguousChunkClassification::IndexNonEmptyZero => {
                        ResolutionClassificationV1::NonEmpty {
                            start: zero,
                            end: zero,
                        }
                    }
                    AmbiguousChunkClassification::Unresolved => {
                        unresolved[unresolved_cursor] = ordinal;
                        unresolved_cursor += 1;
                        ResolutionClassificationV1::Pending
                    }
                    AmbiguousChunkClassification::PendingMessageIndex => {
                        unreachable!("validated plan")
                    }
                }
            } else {
                let descriptor = physical
                    .region(ordinal)
                    .expect("validated ordinal")
                    .raw_descriptor();
                ResolutionClassificationV1::NonEmpty {
                    start: canonicalize_raw_mcap_time(RawMcapTime::new(
                        descriptor.message_start_time,
                    ))
                    .expect("validated time"),
                    end: canonicalize_raw_mcap_time(RawMcapTime::new(descriptor.message_end_time))
                        .expect("validated time"),
                }
            };
        }
        let (reservation, authority_claim) = transaction.into_parts_v1();
        let authority_allocation: PreparedPhysicalChunkAuthorityAllocationV1Bound =
            PhysicalChunkSourceAuthority::bind_prepared_allocation_to_claim_v1(
                &physical,
                authority_allocation,
                authority_claim,
            );
        let authority = PhysicalChunkSourceAuthority::new_preallocated_v1(
            physical,
            decompression_budget,
            scan_budget,
            authority_allocation,
            Some(
                crate::remote_chunk_scan::BoundPhysicalChunkObjectIdentityV1 {
                    state: Arc::clone(&object.state),
                    generation: source_generation,
                },
            ),
        );
        let source_binding = authority.binding_for_remote_assignment_v1();
        Ok(Self {
            object,
            registry,
            authority,
            source_binding: source_binding.clone(),
            classifications: classifications.into_boxed_slice(),
            intervals: intervals.into_boxed_slice(),
            prefix_max_end: prefix_max_end.into_boxed_slice(),
            unresolved: unresolved.into_boxed_slice(),
            next_unresolved: 0,
            reservation,
            aggregate_reservations,
            close_guard: AuthorityCloseGuardV1 {
                binding: source_binding.clone(),
                armed: true,
            },
        })
    }

    pub(crate) fn issue_next_resolution_lease_v1(
        &self,
    ) -> Result<PhysicalChunkReadLease<'a, PendingHeaderValidation>, PhysicalSourceResolutionErrorV1>
    {
        let ordinal = *self
            .unresolved
            .get(self.next_unresolved)
            .ok_or(PhysicalSourceResolutionErrorV1::ResolutionComplete)?;
        let selector = self
            .authority
            .select(ordinal)
            .map_err(PhysicalSourceResolutionErrorV1::Physical)?;
        self.authority
            .issue(selector)
            .map_err(PhysicalSourceResolutionErrorV1::Physical)
    }

    pub(crate) fn submit_resolution_v1(
        &mut self,
        result: AmbiguousPhysicalExtentResolutionV1<'a>,
    ) -> Result<(), PhysicalSourceResolutionErrorV1> {
        let expected = *self
            .unresolved
            .get(self.next_unresolved)
            .ok_or(PhysicalSourceResolutionErrorV1::ResolutionComplete)?;
        if !self.source_binding.matches_v1(&result.binding) {
            return Err(PhysicalSourceResolutionErrorV1::PhysicalSourceBindingMismatch);
        }
        if result.canonical_ordinal != expected {
            return Err(PhysicalSourceResolutionErrorV1::OutOfOrderResolution);
        }
        let classification = match result.extent {
            ValidatedPhysicalChunkExtent::KnownEmpty => ResolutionClassificationV1::KnownEmpty,
            ValidatedPhysicalChunkExtent::NonEmpty {
                canonical_start,
                canonical_end,
                raw_start: 0,
                raw_end: 0,
            } => ResolutionClassificationV1::NonEmpty {
                start: canonical_start,
                end: canonical_end,
            },
            ValidatedPhysicalChunkExtent::NonEmpty { .. } => {
                return Err(PhysicalSourceResolutionErrorV1::AmbiguousExtentNotZero);
            }
        };
        drop(result);
        self.authority
            .ensure_slot_vacant_v1(expected)
            .map_err(PhysicalSourceResolutionErrorV1::Physical)?;
        self.classifications[expected] = classification;
        self.next_unresolved += 1;
        Ok(())
    }

    pub(crate) fn finalize_v1(
        mut self,
    ) -> Result<ResolvedPhysicalSourceAuthorityV1<'a>, PhysicalSourceResolutionErrorV1> {
        if self.next_unresolved != self.unresolved.len() {
            return Err(PhysicalSourceResolutionErrorV1::ResolutionIncomplete);
        }
        if self
            .classifications
            .iter()
            .any(|value| matches!(value, ResolutionClassificationV1::Pending))
        {
            return Err(PhysicalSourceResolutionErrorV1::ResolutionIncomplete);
        }
        let mut interval_count = 0;
        let mut extent: Option<(TimeInt, TimeInt)> = None;
        for (ordinal, classification) in self.classifications.iter().copied().enumerate() {
            if let ResolutionClassificationV1::NonEmpty { start, end } = classification {
                self.intervals[interval_count] = CanonicalIntervalV1 {
                    start,
                    end,
                    canonical_ordinal: ordinal,
                };
                interval_count += 1;
                extent = Some(match extent {
                    Some((min, max)) => (min.min(start), max.max(end)),
                    None => (start, end),
                });
            }
        }
        self.intervals[..interval_count]
            .sort_unstable_by_key(|value| (value.start, value.end, value.canonical_ordinal));
        let mut max_end = TimeInt::MIN;
        for (index, interval) in self.intervals[..interval_count].iter().enumerate() {
            max_end = max_end.max(interval.end);
            self.prefix_max_end[index] = max_end;
        }
        let layout = ResolvedCanonicalPhysicalLayoutV1 {
            classifications: std::mem::take(&mut self.classifications),
            intervals: std::mem::take(&mut self.intervals),
            prefix_max_end: std::mem::take(&mut self.prefix_max_end),
            interval_count,
            extent: extent.map_or(CanonicalPhysicalExtentV1::KnownEmpty, |(start, end)| {
                CanonicalPhysicalExtentV1::Known { start, end }
            }),
        };
        let Self {
            object,
            registry,
            authority,
            source_binding,
            classifications: _,
            intervals: _,
            prefix_max_end: _,
            unresolved: _,
            next_unresolved: _,
            reservation,
            aggregate_reservations,
            close_guard,
        } = self;
        let mut close_guard = close_guard;
        close_guard.disarm();
        Ok(ResolvedPhysicalSourceAuthorityV1 {
            object,
            authority,
            source_binding,
            layout,
            _reservation: reservation,
            _registry: registry,
            _aggregate_reservations: aggregate_reservations,
        })
    }
}

pub struct ResolvedPhysicalSourceAuthorityV1<'a> {
    object: RemotePhysicalObjectBindingV1,
    authority: PhysicalChunkSourceAuthority<'a>,
    source_binding: PhysicalChunkSourceBindingV1,
    layout: ResolvedCanonicalPhysicalLayoutV1,
    _reservation: AggregateResolutionReservationV1,
    _registry: RemotePhysicalSourceRegistryLeaseV1,
    _aggregate_reservations: PreparedAmbiguousZeroAggregateReservations,
}

impl<'a> ResolvedPhysicalSourceAuthorityV1<'a> {
    pub(crate) fn layout_v1(&self) -> &ResolvedCanonicalPhysicalLayoutV1 {
        &self.layout
    }
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

fn retained_layout_bytes(count: u64) -> Result<u64, PhysicalSourceResolutionErrorV1> {
    PhysicalSourceResolutionCensusV1::checked(count, count).map(|census| census.retained_bytes)
}

fn try_filled<T: Clone>(count: usize, value: T) -> Result<Vec<T>, PhysicalSourceResolutionErrorV1> {
    #[cfg(test)]
    if FAIL_NEXT_POST_CLAIM_ALLOCATION_V1.with(|fail| fail.replace(false)) {
        return Err(PhysicalSourceResolutionErrorV1::AllocationFailed);
    }
    let mut output = Vec::new();
    output
        .try_reserve_exact(count)
        .map_err(|_allocation| PhysicalSourceResolutionErrorV1::AllocationFailed)?;
    output.resize(count, value);
    Ok(output)
}

#[cfg(any(test, rerun_mcap_phase_a_proof_v1))]
pub mod phase_a_measurement;

#[cfg(test)]
mod tests {
    use std::{num::NonZeroU64, sync::Arc};

    use static_assertions::assert_not_impl_any;

    use super::*;
    use crate::remote_chunk_scan::{
        PhysicalChunkScanLimits, install_exact_physical_chunk_record_for_test,
        install_header_validated_payload_for_test, scan_decompressed_physical_chunk,
    };
    use crate::testing::{
        AdversarialMcapFixture, AdversarialMcapFixtureBuilder, FixtureChannel, FixtureChunk,
        FixtureCrc, FixtureMessage, FixtureSchema, PartitionFixture, RawTimeRange,
    };

    assert_not_impl_any!(RemotePhysicalObjectBindingV1: Clone, Copy);
    assert_not_impl_any!(PreparedPhysicalSourceResolutionV1<'static>: Clone, Copy);
    assert_not_impl_any!(ResolvedPhysicalSourceAuthorityV1<'static>: Clone, Copy);

    fn fixture(chunks: impl IntoIterator<Item = FixtureChunk>) -> AdversarialMcapFixture {
        AdversarialMcapFixtureBuilder::new()
            .with_chunks(chunks)
            .with_summary_crc(FixtureCrc::Zero)
            .build()
            .unwrap()
    }

    fn object(fixture: &AdversarialMcapFixture) -> RemotePhysicalObjectBindingV1 {
        RemotePhysicalObjectBindingV1::issue_for_test(
            NonZeroU64::new(u64::try_from(fixture.bytes.len()).unwrap()).unwrap(),
            RemoteObjectConsistencyClassV1::StrongValidator,
        )
        .unwrap()
    }

    fn layout_budget(_chunks: u64, bytes: u64) -> AggregateResolutionBudgetRootV1 {
        // Test fixtures use the historical layout census as their public input. Account for the
        // authority's fixed projection/slot overlap in the aggregate root as well.
        AggregateResolutionBudgetRootV1::new_for_sealed_profile_v1(
            1,
            bytes.saturating_add(16 << 20),
        )
    }

    fn fail_next_post_claim_allocation() {
        FAIL_NEXT_POST_CLAIM_ALLOCATION_V1.with(|fail| fail.set(true));
    }

    fn bound_pair(
        fixture: &AdversarialMcapFixture,
        budget: AggregateResolutionBudgetRootV1,
    ) -> PreparedBoundRemotePhysicalSourceV1<&'static str, PreparedAmbiguousZeroBodyPlan<'_>> {
        use crate::remote_fixed_layout::{RemoteMcapSlice, prepare_fixed_layout};
        use crate::remote_summary::ambiguous_zero::{AmbiguousZeroBudget, AmbiguousZeroLimits};
        use crate::remote_summary::materialization::{
            NestedPreflightCensus, SummaryMaterializationBudget, SummaryMaterializationLimits,
        };
        use crate::remote_summary::message_index::{
            MessageIndexRegionBudget, MessageIndexRegionLimits,
        };
        use crate::remote_summary::physical_regions::{PhysicalRegionBudget, PhysicalRegionLimits};

        const RECORD_ENVELOPE_LEN: usize = 9;
        const FOOTER_TAIL_LEN: usize = RECORD_ENVELOPE_LEN + 20 + mcap::MAGIC.len();
        let object_len = u64::try_from(fixture.bytes.len()).unwrap();
        let tail_start = fixture.bytes.len() - FOOTER_TAIL_LEN;
        let initial_read =
            RemoteMcapSlice::new(0, &fixture.bytes[..mcap::MAGIC.len() + RECORD_ENVELOPE_LEN]);
        let footer_read = RemoteMcapSlice::new(
            u64::try_from(tail_start).unwrap(),
            &fixture.bytes[tail_start..],
        );
        let fixed = prepare_fixed_layout(object_len, initial_read, footer_read).unwrap();
        let summary_range = fixed.data_end_and_summary_range();
        let summary_read = RemoteMcapSlice::new(
            summary_range.start,
            &fixture.bytes[usize::try_from(summary_range.start).unwrap()
                ..usize::try_from(summary_range.end).unwrap()],
        );
        let _fixed = fixed
            .validate_data_end_and_summary(RemoteMcapSlice::new(
                summary_range.start,
                &fixture.bytes[usize::try_from(summary_range.start).unwrap()
                    ..usize::try_from(summary_range.end).unwrap()],
            ))
            .unwrap();
        let summary_census_limits =
            crate::remote_summary::SummaryCensusLimits::for_test(object_len, 10_000);
        let summary_materialization_budget = SummaryMaterializationBudget::for_test(
            SummaryMaterializationLimits::for_test(
                10_000,
                object_len,
                object_len,
                object_len.saturating_mul(8),
                10_000,
                10_000,
            ),
            1,
            NestedPreflightCensus {
                main_summary_records: 10_000,
                owned_string_count: 100_000,
                owned_string_bytes: object_len.saturating_mul(8),
                schema_data_bytes: object_len.saturating_mul(8),
                channel_metadata_entries: 100_000,
                fixed_map_entries: 100_000,
                nested_encoded_bytes: object_len.saturating_mul(8),
                nested_retained_bytes: object_len.saturating_mul(8),
            },
        );
        let physical_region_budget = PhysicalRegionBudget::for_test(
            PhysicalRegionLimits::for_test(
                10_000,
                object_len,
                object_len,
                object_len,
                object_len,
                object_len.saturating_mul(8),
                object_len.saturating_mul(8),
            ),
            1,
            10_000,
            object_len.saturating_mul(8),
        );
        let ambiguous_zero_budget = AmbiguousZeroBudget::for_test(
            AmbiguousZeroLimits::for_test(
                [10_000, object_len, 1_000_000, 10_000, object_len],
                [
                    10_000, object_len, object_len, object_len, 10_000, object_len, 1_000_000,
                ],
            ),
            1,
            1,
            1,
        );
        let message_index_budget = MessageIndexRegionBudget::for_test(
            MessageIndexRegionLimits::for_test(10_000, 1_000_000, object_len, object_len),
            10_000,
            object_len,
            10_000,
            1_000_000,
            1_000_000,
            object_len,
        );
        let profile = RemotePhysicalEvidenceProfileV1 {
            summary_census_limits,
            summary_materialization_budget,
            physical_region_budget,
            message_index_budget,
            ambiguous_zero_budget,
            decompression_budget: crate::remote_decompression::chunk_decompression_budget_for_test(
            ),
            scan_budget: PhysicalChunkScanBudget::for_test(PhysicalChunkScanLimits::generous()),
            aggregate_budget_root: budget,
        };
        let evidence = PreparedBoundRemotePhysicalSourceV1::<&'static str, ()>::issue_and_prepare_fixed_layout_for_test(
            "validator",
            NonZeroU64::new(object_len).unwrap(),
            RemoteObjectConsistencyClassV1::StrongValidator,
            initial_read,
            footer_read,
            profile,
        )
        .unwrap();
        let summary_read = evidence.issue_read_v1(summary_read).unwrap();
        let evidence = evidence
            .validate_data_end_and_summary_v1(summary_read)
            .unwrap();
        let header = fixture
            .layout
            .records
            .iter()
            .find(|record| record.opcode == mcap::records::op::HEADER)
            .unwrap();
        let header_read = evidence
            .issue_read_v1(RemoteMcapSlice::new(
                u64::try_from(header.body_start).unwrap(),
                &fixture.bytes[header.body_start..header.end],
            ))
            .unwrap();
        let evidence = evidence
            .prepare_summary_v1(header_read)
            .unwrap()
            .preflight_materialization_v1()
            .unwrap()
            .materialize_summary_v1()
            .unwrap();
        let mut evidence = evidence
            .validate_definitions_v1()
            .unwrap()
            .validate_physical_regions_v1()
            .unwrap()
            .prepare_ambiguous_zero_v1()
            .unwrap();
        loop {
            match evidence.next_ambiguous_zero_v1().unwrap() {
                BoundAmbiguousZeroTurnV1::MessageIndex(prepared) => {
                    let range = prepared.expected_message_index_range_v1();
                    let start = usize::try_from(range.start).unwrap();
                    let end = usize::try_from(range.end).unwrap();
                    let read = prepared
                        .issue_read_v1(RemoteMcapSlice::new(
                            range.start,
                            &fixture.bytes[start..end],
                        ))
                        .unwrap();
                    evidence = prepared.install_and_execute_message_index_v1(read).unwrap();
                }
                BoundAmbiguousZeroTurnV1::BodyPlan(plan) => return plan,
            }
        }
    }

    fn prepare(
        fixture: &AdversarialMcapFixture,
        budget: AggregateResolutionBudgetRootV1,
    ) -> BoundPreparedPhysicalSourceResolutionV1<'_, &'static str> {
        bound_pair(fixture, budget).prepare().unwrap()
    }

    fn resolve_next<'a>(
        prepared: &PreparedPhysicalSourceResolutionV1<'a>,
        fixture: &'a AdversarialMcapFixture,
        ordinal: usize,
    ) -> AmbiguousPhysicalExtentResolutionV1<'a> {
        let lease = prepared.issue_next_resolution_lease_v1().unwrap();
        let record = fixture.layout.chunks[ordinal].record;
        let validated = install_exact_physical_chunk_record_for_test(
            lease,
            fixture.bytes[record.start..record.end]
                .to_vec()
                .into_boxed_slice(),
        )
        .unwrap();
        let input = install_header_validated_payload_for_test(validated).unwrap();
        let output = crate::remote_decompression::decompress_exact_chunk(input).unwrap();
        AmbiguousPhysicalExtentResolutionV1::from_scan(
            scan_decompressed_physical_chunk(output).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn finalized_owner_typed_ros_scalar_matches_local_decoder_chunk() {
        use crate::decoders::{
            DecoderRegistry, McapRos2ReflectionDecoder, TestEmitter, TopicFilter,
        };
        use arrow::array::{Array as _, Int32Array, StructArray};

        let payload = [0, 1, 0, 0, 42, 0, 0, 0];
        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_schemas([FixtureSchema::new(7, "pkg/Root", "ros2msg").with_data(b"int32 value")])
            .with_channels([FixtureChannel::schema_less(1, "/root").with_schema(7, "cdr")])
            .with_chunks([FixtureChunk::single(
                FixtureMessage::new(1, 0, 1)
                    .with_publish_time(3)
                    .with_data(payload),
            )])
            .with_partition_fixture(PartitionFixture::default())
            .with_summary_crc(FixtureCrc::Zero)
            .build()
            .unwrap();

        let exact = retained_layout_bytes(1).unwrap();
        let resolved = bound_pair(&fixture, layout_budget(1, exact))
            .prepare()
            .unwrap()
            .finalize_v1()
            .unwrap();
        let manifest_source = resolved.source_v1();
        let metadata_source = resolved.source_v1();
        let unit = metadata_source.source_unit_v1(0).unwrap();
        let metadata = unit.metadata_v1().unwrap();
        let physical = metadata.definitions_capability_for_full_chain_test_v1();
        let record = fixture.layout.chunks[0].record;
        let validated = install_exact_physical_chunk_record_for_test(
            unit.issue_lease_v1().unwrap(),
            fixture.bytes[record.start..record.end]
                .to_vec()
                .into_boxed_slice(),
        )
        .unwrap();
        let compressed = install_header_validated_payload_for_test(validated).unwrap();
        let decompressed = crate::remote_decompression::decompress_exact_chunk(compressed).unwrap();
        let scan = scan_decompressed_physical_chunk(decompressed).unwrap();
        let cache = crate::remote_chunk_scan::PhysicalChunkScanCacheEntry::new(scan).unwrap();
        let evidence = cache.consumer().into_message_evidence_v1().unwrap();
        let remote =
            crate::remote_decoder_assignment::execute_ros_scalar_from_finalized_source_for_test_v1(
                manifest_source,
                &physical,
                evidence,
                1,
            );

        let summary = fixture.read_upstream_summary().unwrap().unwrap();
        let plan = DecoderRegistry::empty()
            .register_message_decoder::<McapRos2ReflectionDecoder>()
            .plan(&fixture.bytes, &summary, &TopicFilter::default())
            .unwrap();
        let emitter = TestEmitter::default();
        plan.run(
            &fixture.bytes,
            &summary,
            re_log_types::TimeType::TimestampNs,
            &*emitter,
        )
        .unwrap();
        let local = emitter.finish();
        let [local] = local.as_slice() else {
            panic!("the local reflection oracle must emit exactly one chunk");
        };

        // Root and row identities deliberately belong to different authorities. Everything else
        // must be byte-for-byte schema/value equivalent to the established local decoder.
        assert_eq!(remote.entity_path(), local.entity_path());
        assert_eq!(remote.timelines(), local.timelines());
        assert_eq!(remote.num_rows(), local.num_rows());
        let remote_components = remote.components().iter().collect::<Vec<_>>();
        let [(remote_component, remote_column)] = remote_components.as_slice() else {
            panic!("the remote scalar chunk must contain exactly one component");
        };
        let local_components = local.components().iter().collect::<Vec<_>>();
        let [(local_component, local_column)] = local_components.as_slice() else {
            panic!("the local scalar chunk must contain exactly one component");
        };
        assert_eq!(remote_component, local_component);
        assert_eq!(remote_column.descriptor, local_column.descriptor);
        assert_eq!(
            remote_column.list_array.data_type(),
            local_column.list_array.data_type(),
            "the complete Arrow schema, including field names/nullability, must match",
        );
        assert_eq!(
            remote_column.list_array.value_offsets(),
            local_column.list_array.value_offsets()
        );
        assert_eq!(
            remote_column.list_array.null_count(),
            local_column.list_array.null_count()
        );
        let remote_struct = remote_column
            .list_array
            .values()
            .as_any()
            .downcast_ref::<StructArray>()
            .unwrap();
        let local_struct = local_column
            .list_array
            .values()
            .as_any()
            .downcast_ref::<StructArray>()
            .unwrap();
        assert_eq!(remote_struct.data_type(), local_struct.data_type());
        assert_eq!(remote_struct.null_count(), local_struct.null_count());
        assert_eq!(
            remote_struct
                .column(0)
                .as_any()
                .downcast_ref::<Int32Array>()
                .unwrap()
                .values(),
            local_struct
                .column(0)
                .as_any()
                .downcast_ref::<Int32Array>()
                .unwrap()
                .values(),
        );
        assert_ne!(remote.id(), local.id());
        assert_ne!(
            remote.row_ids().collect::<Vec<_>>(),
            local.row_ids().collect::<Vec<_>>()
        );
    }

    #[test]
    fn terminal_dispatch_is_atomic_for_all_channels_in_one_frozen_group() {
        let int32_payload = |value: i32| {
            let mut payload = vec![0, 1, 0, 0];
            payload.extend_from_slice(&value.to_le_bytes());
            payload
        };
        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_schemas([FixtureSchema::new(7, "pkg/Root", "ros2msg").with_data(b"int32 value")])
            .with_channels([
                FixtureChannel::schema_less(1, "/alpha").with_schema(7, "cdr"),
                FixtureChannel::schema_less(2, "/beta").with_schema(7, "cdr"),
            ])
            .with_chunks([FixtureChunk::new([
                FixtureMessage::new(2, 0, 2).with_data(int32_payload(20)),
                FixtureMessage::new(1, 0, 1).with_data(int32_payload(10)),
            ])])
            .with_partition_fixture(PartitionFixture::default())
            .with_summary_crc(FixtureCrc::Zero)
            .build()
            .unwrap();

        let exact = retained_layout_bytes(1).unwrap();
        let resolved = bound_pair(&fixture, layout_budget(1, exact))
            .prepare()
            .unwrap()
            .finalize_v1()
            .unwrap();
        let manifest_source = resolved.source_v1();
        let metadata_source = resolved.source_v1();
        let unit = metadata_source.source_unit_v1(0).unwrap();
        let metadata = unit.metadata_v1().unwrap();
        let physical = metadata.definitions_capability_for_full_chain_test_v1();
        let record = fixture.layout.chunks[0].record;
        let validated = install_exact_physical_chunk_record_for_test(
            unit.issue_lease_v1().unwrap(),
            fixture.bytes[record.start..record.end]
                .to_vec()
                .into_boxed_slice(),
        )
        .unwrap();
        let compressed = install_header_validated_payload_for_test(validated).unwrap();
        let decompressed = crate::remote_decompression::decompress_exact_chunk(compressed).unwrap();
        let cache = crate::remote_chunk_scan::PhysicalChunkScanCacheEntry::new(
            scan_decompressed_physical_chunk(decompressed).unwrap(),
        )
        .unwrap();
        let chunks =
            crate::remote_decoder_assignment::execute_group_from_finalized_source_for_test_v1(
                manifest_source,
                &physical,
                cache.consumer().into_message_evidence_v1().unwrap(),
                1,
            );
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].entity_path().to_string(), "/alpha");
        assert_eq!(chunks[1].entity_path().to_string(), "/beta");
        assert_eq!(chunks[0].num_rows(), 1);
        assert_eq!(chunks[1].num_rows(), 1);
        assert_ne!(chunks[0].id(), chunks[1].id());
    }

    #[test]
    fn terminal_dispatch_filters_mixed_groups_to_the_frozen_partition() {
        let mut int32_payload = vec![0, 1, 0, 0];
        int32_payload.extend_from_slice(&10_i32.to_le_bytes());
        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_schemas([
                FixtureSchema::new(7, "pkg/Int", "ros2msg").with_data(b"int32 value"),
                FixtureSchema::new(8, "pkg/Bool", "ros2msg").with_data(b"bool value"),
            ])
            .with_channels([
                FixtureChannel::schema_less(1, "/int").with_schema(7, "cdr"),
                FixtureChannel::schema_less(2, "/bool").with_schema(8, "cdr"),
            ])
            .with_chunks([FixtureChunk::new([
                FixtureMessage::new(1, 0, 1).with_data(int32_payload),
                FixtureMessage::new(2, 0, 2).with_data(vec![0, 1, 0, 0, 1]),
            ])])
            .with_partition_fixture(PartitionFixture::default())
            .with_summary_crc(FixtureCrc::Zero)
            .build()
            .unwrap();

        let exact = retained_layout_bytes(1).unwrap();
        let resolved = bound_pair(&fixture, layout_budget(1, exact))
            .prepare()
            .unwrap()
            .finalize_v1()
            .unwrap();
        let manifest_source = resolved.source_v1();
        let metadata_source = resolved.source_v1();
        let unit = metadata_source.source_unit_v1(0).unwrap();
        let metadata = unit.metadata_v1().unwrap();
        let physical = metadata.definitions_capability_for_full_chain_test_v1();
        let record = fixture.layout.chunks[0].record;
        let validated = install_exact_physical_chunk_record_for_test(
            unit.issue_lease_v1().unwrap(),
            fixture.bytes[record.start..record.end]
                .to_vec()
                .into_boxed_slice(),
        )
        .unwrap();
        let compressed = install_header_validated_payload_for_test(validated).unwrap();
        let decompressed = crate::remote_decompression::decompress_exact_chunk(compressed).unwrap();
        let cache = crate::remote_chunk_scan::PhysicalChunkScanCacheEntry::new(
            scan_decompressed_physical_chunk(decompressed).unwrap(),
        )
        .unwrap();
        let chunks =
            crate::remote_decoder_assignment::execute_group_from_finalized_source_for_test_v1(
                manifest_source,
                &physical,
                cache.consumer().into_message_evidence_v1().unwrap(),
                1,
            );
        let [chunk] = chunks.as_slice() else {
            panic!("the frozen int32 partition must emit exactly one chunk");
        };
        assert_eq!(chunk.entity_path().to_string(), "/int");
        assert_eq!(chunk.num_rows(), 1);
    }

    #[test]
    fn terminal_dispatch_validates_zero_row_members_without_emitting_empty_chunks() {
        let mut payload = vec![0, 1, 0, 0];
        payload.extend_from_slice(&20_i32.to_le_bytes());
        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_schemas([FixtureSchema::new(7, "pkg/Root", "ros2msg").with_data(b"int32 value")])
            .with_channels([
                FixtureChannel::schema_less(1, "/alpha").with_schema(7, "cdr"),
                FixtureChannel::schema_less(2, "/beta").with_schema(7, "cdr"),
            ])
            .with_chunks([FixtureChunk::new([
                FixtureMessage::new(2, 0, 1).with_data(payload)
            ])])
            .with_partition_fixture(PartitionFixture::default())
            .with_summary_crc(FixtureCrc::Zero)
            .build()
            .unwrap();

        let exact = retained_layout_bytes(1).unwrap();
        let resolved = bound_pair(&fixture, layout_budget(1, exact))
            .prepare()
            .unwrap()
            .finalize_v1()
            .unwrap();
        let manifest_source = resolved.source_v1();
        let metadata_source = resolved.source_v1();
        let unit = metadata_source.source_unit_v1(0).unwrap();
        let metadata = unit.metadata_v1().unwrap();
        let physical = metadata.definitions_capability_for_full_chain_test_v1();
        let record = fixture.layout.chunks[0].record;
        let validated = install_exact_physical_chunk_record_for_test(
            unit.issue_lease_v1().unwrap(),
            fixture.bytes[record.start..record.end]
                .to_vec()
                .into_boxed_slice(),
        )
        .unwrap();
        let compressed = install_header_validated_payload_for_test(validated).unwrap();
        let decompressed = crate::remote_decompression::decompress_exact_chunk(compressed).unwrap();
        let cache = crate::remote_chunk_scan::PhysicalChunkScanCacheEntry::new(
            scan_decompressed_physical_chunk(decompressed).unwrap(),
        )
        .unwrap();
        let chunks =
            crate::remote_decoder_assignment::execute_group_from_finalized_source_for_test_v1(
                manifest_source,
                &physical,
                cache.consumer().into_message_evidence_v1().unwrap(),
                1,
            );
        let [chunk] = chunks.as_slice() else {
            panic!("only the non-empty canonical member emits a chunk");
        };
        assert_eq!(chunk.entity_path().to_string(), "/beta");
        assert_eq!(chunk.num_rows(), 1);
    }

    #[test]
    fn dispatched_roots_register_and_become_queryable_through_entity_db() {
        let mut payload = vec![0, 1, 0, 0];
        payload.extend_from_slice(&20_i32.to_le_bytes());
        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_schemas([FixtureSchema::new(7, "pkg/Root", "ros2msg").with_data(b"int32 value")])
            .with_channels([FixtureChannel::schema_less(1, "/int").with_schema(7, "cdr")])
            .with_chunks([FixtureChunk::new([
                FixtureMessage::new(1, 0, 1).with_data(payload)
            ])])
            .with_partition_fixture(PartitionFixture::default())
            .with_summary_crc(FixtureCrc::Zero)
            .build()
            .unwrap();

        let exact = retained_layout_bytes(1).unwrap();
        let resolved = bound_pair(&fixture, layout_budget(1, exact))
            .prepare()
            .unwrap()
            .finalize_v1()
            .unwrap();
        let metadata_source = resolved.source_v1();
        let unit = metadata_source.source_unit_v1(0).unwrap();
        let metadata = unit.metadata_v1().unwrap();
        let physical = metadata.definitions_capability_for_full_chain_test_v1();
        let record = fixture.layout.chunks[0].record;
        let validated = install_exact_physical_chunk_record_for_test(
            unit.issue_lease_v1().unwrap(),
            fixture.bytes[record.start..record.end]
                .to_vec()
                .into_boxed_slice(),
        )
        .unwrap();
        let compressed = install_header_validated_payload_for_test(validated).unwrap();
        let decompressed = crate::remote_decompression::decompress_exact_chunk(compressed).unwrap();
        let cache = crate::remote_chunk_scan::PhysicalChunkScanCacheEntry::new(
            scan_decompressed_physical_chunk(decompressed).unwrap(),
        )
        .unwrap();
        let terminal =
            crate::remote_decoder_assignment::dispatch_group_from_finalized_source_for_test_v1(
                resolved.source_v1(),
                &physical,
                cache.consumer().into_message_evidence_v1().unwrap(),
                1,
                crate::remote_decoder_assignment::RemoteDispatchTestMutationV1::None,
            );
        let crate::remote_chunk_dispatch::RemoteChunkTerminalV1::Complete(handoff) = &terminal
        else {
            panic!("the valid source must produce a complete typed handoff");
        };
        let partition = handoff.partition_v1();
        let chunks = handoff.chunks_v1().cloned().collect::<Vec<_>>();
        let registration =
            crate::remote_partition_residency::prepare_terminal_registration_for_test_v1(&terminal)
                .unwrap();
        let root_count = u64::try_from(chunks.len()).unwrap();
        let store_id = re_log_types::StoreId::random(
            re_log_types::StoreKind::Recording,
            "remote-dispatch-registration-db",
        );
        let (mut store, capability) =
            re_chunk_store::WebRemoteMcapStoreConfigV1::for_test_v1().into_store_v1(store_id);
        let mut index = crate::remote_partition_residency::RefetchableRootIndexV1::new_v1(
            partition.key_v1().session_id_v1(),
            crate::remote_manifest::RemoteRegistrationCapacityV1 {
                max_registered_partitions: 1,
                max_complete_empty_entries: 0,
                max_root_descriptors: root_count,
                max_external_origin_bytes: root_count
                    * re_chunk_store::ExternalRefetchableRootOriginV1::ENCODED_BYTES_V1,
            },
            &store,
            &capability,
            crate::remote_loaded_coverage::RemoteTemporalCoveragePlanV1::for_test_v1(
                &[Some((0, 0))],
                1,
            ),
        )
        .unwrap();
        index
            .register_commit_set_v1(&mut store, &capability, vec![registration])
            .unwrap();
        let descriptors = chunks
            .iter()
            .map(|chunk| index.root_refetch_descriptor_v1(chunk.id()).unwrap())
            .collect::<Vec<_>>();
        let mut db =
            re_entity_db::EntityDb::from_web_remote_mcap_store_v1(store, &capability).unwrap();

        for (chunk, descriptor) in chunks.iter().zip(descriptors) {
            let chunk = Arc::new(chunk.clone());
            let permit = capability
                .issue_refetch_v1(db.storage_engine().store(), descriptor)
                .unwrap();
            let events = db.add_external_refetchable_root_v1(permit, &chunk).unwrap();
            assert_eq!(
                events
                    .iter()
                    .filter(|event| event.diff.to_addition().is_some())
                    .count(),
                1
            );
            let timeline = *chunk.timelines().keys().next().unwrap();
            let component = chunk.component_descriptors().next().unwrap().component;
            let results = db.latest_at(
                &re_chunk_store::LatestAtQuery::latest(timeline),
                chunk.entity_path(),
                [component],
            );
            assert!(
                results.component_batch_raw(component).is_some(),
                "the Store event must reach QueryCache before remote add returns"
            );
        }
        assert_eq!(db.num_physical_chunks(), chunks.len());
    }

    fn run_ignored_proof_in_subprocess(test_name: &str) {
        let executable = std::env::current_exe().unwrap();
        let status = std::process::Command::new(executable)
            .args(["--ignored", "--exact", test_name, "--test-threads=1"])
            .status()
            .unwrap();
        assert!(
            status.success(),
            "isolated proof `{test_name}` failed with status {status}"
        );
    }

    #[test]
    fn runtime_identifier_exhaustion_proof_runs_in_an_isolated_process() {
        run_ignored_proof_in_subprocess(
            "remote_physical_resolution::tests::runtime_identifier_exhaustion_bounds_opening_active_and_store_failure",
        );
    }

    #[test]
    #[ignore = "exhausts the process-global module budget; run this proof in isolation"]
    fn runtime_identifier_exhaustion_bounds_opening_active_and_store_failure() {
        use re_string_interner::bounded_runtime_intern as intern;

        crate::remote_runtime_intern::ensure_disarmed_test_profile_v1();

        fn exhaust_remaining_budget() {
            fn ordinal_tag(mut ordinal: usize) -> String {
                const ALPHANUMERICS: &[u8; 62] =
                    b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
                let mut tag = Vec::new();
                loop {
                    tag.push(ALPHANUMERICS[ordinal % 62]);
                    ordinal /= 62;
                    if ordinal == 0 {
                        break;
                    }
                }
                tag.reverse();
                String::from_utf8(tag).unwrap()
            }

            let mut size = 262_144;
            let mut ordinal = 0_usize;
            loop {
                let suffix = ordinal_tag(ordinal);
                assert!(suffix.len() <= size);
                let mut raw = "x".repeat(size - suffix.len());
                raw.push_str(&suffix);
                let before_failure =
                    intern::remote_mcap_runtime_intern_snapshot().expect("budget is initialized");
                let result = intern::prepare_remote_mcap_runtime_intern(&[raw.as_str()])
                    .and_then(intern::PreparedRemoteInternBatch::commit);
                match result {
                    Ok(_) => ordinal += 1,
                    Err(
                        intern::RemoteMcapRuntimeInternError::StringBudgetExceeded
                        | intern::RemoteMcapRuntimeInternError::EntryAndCapacityBudgetExceeded
                        | intern::RemoteMcapRuntimeInternError::SideMapEntryLimitExceeded,
                    ) => {
                        assert_eq!(
                            intern::remote_mcap_runtime_intern_snapshot().unwrap(),
                            before_failure
                        );
                        if size == 1 {
                            return;
                        }
                        size /= 2;
                    }
                    Err(error) => panic!("unexpected remote exhaustion error: {error}"),
                }
            }
        }

        let mut payload = vec![0, 1, 0, 0];
        payload.extend_from_slice(&20_i32.to_le_bytes());
        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_schemas([FixtureSchema::new(7, "pkg/Root", "ros2msg").with_data(b"int32 value")])
            .with_channels([FixtureChannel::schema_less(1, "/root").with_schema(7, "cdr")])
            .with_chunks([FixtureChunk::new([
                FixtureMessage::new(1, 0, 1).with_data(payload)
            ])])
            .with_partition_fixture(PartitionFixture::default())
            .with_summary_crc(FixtureCrc::Zero)
            .build()
            .unwrap();

        let exact = retained_layout_bytes(1).unwrap();
        let resolved = bound_pair(&fixture, layout_budget(1, exact))
            .prepare()
            .unwrap()
            .finalize_v1()
            .unwrap();
        let metadata_source = resolved.source_v1();
        let unit = metadata_source.source_unit_v1(0).unwrap();
        let metadata = unit.metadata_v1().unwrap();
        let physical = metadata.definitions_capability_for_full_chain_test_v1();
        let record = fixture.layout.chunks[0].record;
        let validated = install_exact_physical_chunk_record_for_test(
            unit.issue_lease_v1().unwrap(),
            fixture.bytes[record.start..record.end]
                .to_vec()
                .into_boxed_slice(),
        )
        .unwrap();
        let compressed = install_header_validated_payload_for_test(validated).unwrap();
        let decompressed = crate::remote_decompression::decompress_exact_chunk(compressed).unwrap();
        let cache = crate::remote_chunk_scan::PhysicalChunkScanCacheEntry::new(
            scan_decompressed_physical_chunk(decompressed).unwrap(),
        )
        .unwrap();

        let initial =
            crate::remote_decoder_assignment::try_dispatch_group_from_finalized_source_for_test_v1(
                resolved.source_v1(),
                &physical,
                cache.consumer().into_message_evidence_v1().unwrap(),
                1,
                crate::remote_decoder_assignment::RemoteDispatchTestMutationV1::None,
            )
            .unwrap();
        let crate::remote_chunk_dispatch::RemoteChunkTerminalV1::Complete(handoff) = initial else {
            panic!("the initial remote source must complete before exhaustion");
        };
        let partition = handoff.partition_v1();
        let chunks = handoff.chunks_v1().cloned().collect::<Vec<_>>();
        let registration =
            crate::remote_partition_residency::prepare_terminal_registration_for_test_v1(
                &crate::remote_chunk_dispatch::RemoteChunkTerminalV1::Complete(handoff),
            )
            .unwrap();

        exhaust_remaining_budget();
        let exhausted = intern::remote_mcap_runtime_intern_snapshot().unwrap();

        let existing_only =
            crate::remote_decoder_assignment::try_dispatch_group_from_finalized_source_for_test_v1(
                resolved.source_v1(),
                &physical,
                cache.consumer().into_message_evidence_v1().unwrap(),
                1,
                crate::remote_decoder_assignment::RemoteDispatchTestMutationV1::None,
            )
            .unwrap();
        assert!(matches!(
            existing_only,
            crate::remote_chunk_dispatch::RemoteChunkTerminalV1::Complete(_)
        ));

        let active = crate::remote_decoder_assignment::try_dispatch_group_from_finalized_source_for_test_v1(
            resolved.source_v1(),
            &physical,
            cache.consumer().into_message_evidence_v1().unwrap(),
            1,
            crate::remote_decoder_assignment::RemoteDispatchTestMutationV1::ExhaustChunkRuntimeIdentifier,
        )
        .unwrap();
        let crate::remote_chunk_dispatch::RemoteChunkTerminalV1::Failed(
            crate::remote_chunk_dispatch::RemoteChunkDispatchFailureV1::RuntimeIdentifier(error),
        ) = active
        else {
            panic!("an active Chunk requiring a new identifier must fail");
        };
        assert_eq!(
            error,
            crate::remote_runtime_intern::RemoteRuntimeInternAdmissionErrorV1::BudgetExhausted
        );
        assert_eq!(
            crate::remote_runtime_intern::RemoteRuntimeInternAdmissionErrorV1::terminal_kind_v1(
                crate::remote_runtime_intern::RemoteRuntimeInternAdmissionPhaseV1::ActiveChunk
            ),
            crate::remote_runtime_intern::RemoteRuntimeInternTerminalV1::ActiveSessionFatal
        );
        assert_eq!(
            cache.consumer().into_message_evidence_v1().err(),
            Some(PhysicalChunkValidationError::SourceClosed)
        );

        let mut opening_payload = vec![0, 1, 0, 0];
        opening_payload.extend_from_slice(&21_i32.to_le_bytes());
        let opening_fixture = AdversarialMcapFixtureBuilder::new()
            .with_schemas([
                FixtureSchema::new(9, "mcap083/OpeningExhaustedSchema", "ros2msg")
                    .with_data(b"int32 value"),
            ])
            .with_channels([
                FixtureChannel::schema_less(1, "/mcap083/opening/exhausted").with_schema(9, "cdr")
            ])
            .with_chunks([FixtureChunk::new([
                FixtureMessage::new(1, 0, 1).with_data(opening_payload)
            ])])
            .with_partition_fixture(PartitionFixture::default())
            .with_summary_crc(FixtureCrc::Zero)
            .build()
            .unwrap();
        let opening_exact = retained_layout_bytes(1).unwrap();
        let opening_resolved = bound_pair(&opening_fixture, layout_budget(1, opening_exact))
            .prepare()
            .unwrap()
            .finalize_v1()
            .unwrap();
        let opening_source = opening_resolved.source_v1();
        let opening_unit = opening_source.source_unit_v1(0).unwrap();
        let opening_metadata = opening_unit.metadata_v1().unwrap();
        let opening_physical = opening_metadata.definitions_capability_for_full_chain_test_v1();
        let opening_record = opening_fixture.layout.chunks[0].record;
        let opening_validated = install_exact_physical_chunk_record_for_test(
            opening_unit.issue_lease_v1().unwrap(),
            opening_fixture.bytes[opening_record.start..opening_record.end]
                .to_vec()
                .into_boxed_slice(),
        )
        .unwrap();
        let opening_compressed =
            install_header_validated_payload_for_test(opening_validated).unwrap();
        let opening_decompressed =
            crate::remote_decompression::decompress_exact_chunk(opening_compressed).unwrap();
        let opening_cache = crate::remote_chunk_scan::PhysicalChunkScanCacheEntry::new(
            scan_decompressed_physical_chunk(opening_decompressed).unwrap(),
        )
        .unwrap();
        let opening_evidence = opening_cache.consumer().into_message_evidence_v1().unwrap();
        let opening_dispatch =
            crate::remote_decoder_assignment::try_dispatch_group_from_finalized_source_for_test_v1(
                opening_resolved.source_v1(),
                &opening_physical,
                opening_evidence,
                1,
                crate::remote_decoder_assignment::RemoteDispatchTestMutationV1::None,
            );
        assert_eq!(
            opening_dispatch.err().unwrap(),
            crate::remote_decoder_assignment::PhaseAMeasurementDispatchSetupErrorV1::SummaryRuntimeIntern
        );
        assert_eq!(
            crate::remote_runtime_intern::RemoteRuntimeInternAdmissionErrorV1::terminal_kind_v1(
                crate::remote_runtime_intern::RemoteRuntimeInternAdmissionPhaseV1::Opening,
            ),
            crate::remote_runtime_intern::RemoteRuntimeInternTerminalV1::OpeningExhausted
        );
        assert_eq!(
            opening_cache.consumer().into_message_evidence_v1().err(),
            Some(PhysicalChunkValidationError::SourceClosed)
        );

        let root_count = u64::try_from(chunks.len()).unwrap();
        let (mut store, capability) = re_chunk_store::WebRemoteMcapStoreConfigV1::for_test_v1()
            .into_store_v1(re_log_types::StoreId::random(
                re_log_types::StoreKind::Recording,
                "mcap083-runtime-exhaustion-store",
            ));
        let mut index = crate::remote_partition_residency::RefetchableRootIndexV1::new_v1(
            partition.key_v1().session_id_v1(),
            crate::remote_manifest::RemoteRegistrationCapacityV1 {
                max_registered_partitions: 1,
                max_complete_empty_entries: 0,
                max_root_descriptors: root_count,
                max_external_origin_bytes: root_count
                    * re_chunk_store::ExternalRefetchableRootOriginV1::ENCODED_BYTES_V1,
            },
            &store,
            &capability,
            crate::remote_loaded_coverage::RemoteTemporalCoveragePlanV1::for_test_v1(
                &[Some((0, 0))],
                1,
            ),
        )
        .unwrap();
        index
            .register_commit_set_v1(&mut store, &capability, vec![registration])
            .unwrap();
        let mut db =
            re_entity_db::EntityDb::from_web_remote_mcap_store_v1(store, &capability).unwrap();
        let before_store_failure = intern::remote_mcap_runtime_intern_snapshot().unwrap();
        for chunk in &chunks {
            let descriptor = index.root_refetch_descriptor_v1(chunk.id()).unwrap();
            let permit = capability
                .issue_refetch_v1(db.storage_engine().store(), descriptor)
                .unwrap();
            let mismatched = Arc::new(
                re_chunk::Chunk::builder_with_id(
                    re_chunk::ChunkId::new(),
                    "mcap083/store/mismatch",
                )
                .with_archetype(
                    re_chunk::RowId::new(),
                    re_log_types::TimePoint::STATIC,
                    &re_sdk_types::archetypes::Points3D::new([[1.0, 2.0, 3.0]]),
                )
                .build()
                .unwrap(),
            );
            assert!(
                db.add_external_refetchable_root_v1(permit, &mismatched)
                    .is_err()
            );
        }
        assert_eq!(db.num_physical_chunks(), 0);
        let after_store_failure = intern::remote_mcap_runtime_intern_snapshot().unwrap();
        assert_eq!(
            before_store_failure.burned_string_bytes,
            exhausted.burned_string_bytes
        );
        assert_eq!(
            before_store_failure.burned_entry_bytes,
            exhausted.burned_entry_bytes
        );
        assert_eq!(
            after_store_failure.burned_string_bytes,
            before_store_failure.burned_string_bytes
        );
        assert_eq!(
            after_store_failure.burned_entry_bytes,
            before_store_failure.burned_entry_bytes
        );
        assert_eq!(
            after_store_failure.remote_entries,
            before_store_failure.remote_entries
        );
    }

    #[test]
    fn all_zero_frozen_group_validates_every_owner_before_complete_empty() {
        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_schemas([
                FixtureSchema::new(7, "pkg/Int", "ros2msg").with_data(b"int32 value"),
                FixtureSchema::new(8, "pkg/Bool", "ros2msg").with_data(b"bool value"),
            ])
            .with_channels([
                FixtureChannel::schema_less(1, "/alpha").with_schema(7, "cdr"),
                FixtureChannel::schema_less(2, "/beta").with_schema(7, "cdr"),
                FixtureChannel::schema_less(3, "/other").with_schema(8, "cdr"),
            ])
            .with_chunks([FixtureChunk::new([
                FixtureMessage::new(3, 0, 1).with_data(vec![0, 1, 0, 0, 1])
            ])])
            .with_partition_fixture(PartitionFixture::default())
            .with_summary_crc(FixtureCrc::Zero)
            .build()
            .unwrap();

        let exact = retained_layout_bytes(1).unwrap();
        let resolved = bound_pair(&fixture, layout_budget(1, exact))
            .prepare()
            .unwrap()
            .finalize_v1()
            .unwrap();
        let manifest_source = resolved.source_v1();
        let metadata_source = resolved.source_v1();
        let unit = metadata_source.source_unit_v1(0).unwrap();
        let metadata = unit.metadata_v1().unwrap();
        let physical = metadata.definitions_capability_for_full_chain_test_v1();
        let record = fixture.layout.chunks[0].record;
        let validated = install_exact_physical_chunk_record_for_test(
            unit.issue_lease_v1().unwrap(),
            fixture.bytes[record.start..record.end]
                .to_vec()
                .into_boxed_slice(),
        )
        .unwrap();
        let compressed = install_header_validated_payload_for_test(validated).unwrap();
        let decompressed = crate::remote_decompression::decompress_exact_chunk(compressed).unwrap();
        let cache = crate::remote_chunk_scan::PhysicalChunkScanCacheEntry::new(
            scan_decompressed_physical_chunk(decompressed).unwrap(),
        )
        .unwrap();
        let evidence = cache.consumer().into_message_evidence_v1().unwrap();
        let terminal =
            crate::remote_decoder_assignment::dispatch_group_from_finalized_source_for_test_v1(
                manifest_source,
                &physical,
                evidence,
                1,
                crate::remote_decoder_assignment::RemoteDispatchTestMutationV1::None,
            );
        assert!(matches!(
            terminal,
            crate::remote_chunk_dispatch::RemoteChunkTerminalV1::CompleteEmpty
        ));

        let cross_wired =
            crate::remote_decoder_assignment::dispatch_group_from_finalized_source_for_test_v1(
                resolved.source_v1(),
                &physical,
                cache.consumer().into_message_evidence_v1().unwrap(),
                1,
                crate::remote_decoder_assignment::RemoteDispatchTestMutationV1::CrossWireFirstDescriptor,
            );
        assert!(matches!(
            cross_wired,
            crate::remote_chunk_dispatch::RemoteChunkTerminalV1::Failed(
                crate::remote_chunk_dispatch::RemoteChunkDispatchFailureV1::StaleSource
            )
        ));
    }

    #[test]
    fn all_zero_frozen_group_revalidates_stale_source_before_complete_empty() {
        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_schemas([
                FixtureSchema::new(7, "pkg/Int", "ros2msg").with_data(b"int32 value"),
                FixtureSchema::new(8, "pkg/Bool", "ros2msg").with_data(b"bool value"),
            ])
            .with_channels([
                FixtureChannel::schema_less(1, "/alpha").with_schema(7, "cdr"),
                FixtureChannel::schema_less(2, "/beta").with_schema(7, "cdr"),
                FixtureChannel::schema_less(3, "/other").with_schema(8, "cdr"),
            ])
            .with_chunks([FixtureChunk::new([
                FixtureMessage::new(3, 0, 1).with_data(vec![0, 1, 0, 0, 1])
            ])])
            .with_partition_fixture(PartitionFixture::default())
            .with_summary_crc(FixtureCrc::Zero)
            .build()
            .unwrap();

        let exact = retained_layout_bytes(1).unwrap();
        let resolved = bound_pair(&fixture, layout_budget(1, exact))
            .prepare()
            .unwrap()
            .finalize_v1()
            .unwrap();
        let manifest_source = resolved.source_v1();
        let metadata_source = resolved.source_v1();
        let unit = metadata_source.source_unit_v1(0).unwrap();
        let metadata = unit.metadata_v1().unwrap();
        let physical = metadata.definitions_capability_for_full_chain_test_v1();
        let record = fixture.layout.chunks[0].record;
        let validated = install_exact_physical_chunk_record_for_test(
            unit.issue_lease_v1().unwrap(),
            fixture.bytes[record.start..record.end]
                .to_vec()
                .into_boxed_slice(),
        )
        .unwrap();
        let compressed = install_header_validated_payload_for_test(validated).unwrap();
        let decompressed = crate::remote_decompression::decompress_exact_chunk(compressed).unwrap();
        let cache = crate::remote_chunk_scan::PhysicalChunkScanCacheEntry::new(
            scan_decompressed_physical_chunk(decompressed).unwrap(),
        )
        .unwrap();
        let evidence = cache.consumer().into_message_evidence_v1().unwrap();
        let terminal =
            crate::remote_decoder_assignment::dispatch_group_from_finalized_source_for_test_v1(
                manifest_source,
                &physical,
                evidence,
                1,
                crate::remote_decoder_assignment::RemoteDispatchTestMutationV1::InvalidateSource,
            );
        assert!(matches!(
            terminal,
            crate::remote_chunk_dispatch::RemoteChunkTerminalV1::Failed(_)
        ));
    }

    #[test]
    #[expect(
        clippy::disallowed_methods,
        reason = "host-only test verifies cross-source all-zero evidence uses the fatal control-plane path"
    )]
    fn all_zero_frozen_group_rejects_cross_source_evidence_before_complete_empty() {
        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_schemas([
                FixtureSchema::new(7, "pkg/Int", "ros2msg").with_data(b"int32 value"),
                FixtureSchema::new(8, "pkg/Bool", "ros2msg").with_data(b"bool value"),
            ])
            .with_channels([
                FixtureChannel::schema_less(1, "/empty").with_schema(7, "cdr"),
                FixtureChannel::schema_less(2, "/other").with_schema(8, "cdr"),
            ])
            .with_chunks([FixtureChunk::new([
                FixtureMessage::new(2, 0, 1).with_data(vec![0, 1, 0, 0, 1])
            ])])
            .with_partition_fixture(PartitionFixture::default())
            .with_summary_crc(FixtureCrc::Zero)
            .build()
            .unwrap();
        let other_fixture = fixture.clone();

        let exact = retained_layout_bytes(1).unwrap();
        let resolved = bound_pair(&fixture, layout_budget(1, exact))
            .prepare()
            .unwrap()
            .finalize_v1()
            .unwrap();
        let manifest_source = resolved.source_v1();
        let metadata_source = resolved.source_v1();
        let unit = metadata_source.source_unit_v1(0).unwrap();
        let metadata = unit.metadata_v1().unwrap();
        let physical = metadata.definitions_capability_for_full_chain_test_v1();

        let other_resolved = bound_pair(&other_fixture, layout_budget(1, exact))
            .prepare()
            .unwrap()
            .finalize_v1()
            .unwrap();
        let other_source = other_resolved.source_v1();
        let other_unit = other_source.source_unit_v1(0).unwrap();
        let other_record = other_fixture.layout.chunks[0].record;
        let other_validated = install_exact_physical_chunk_record_for_test(
            other_unit.issue_lease_v1().unwrap(),
            other_fixture.bytes[other_record.start..other_record.end]
                .to_vec()
                .into_boxed_slice(),
        )
        .unwrap();
        let other_compressed = install_header_validated_payload_for_test(other_validated).unwrap();
        let other_decompressed =
            crate::remote_decompression::decompress_exact_chunk(other_compressed).unwrap();
        let other_cache = crate::remote_chunk_scan::PhysicalChunkScanCacheEntry::new(
            scan_decompressed_physical_chunk(other_decompressed).unwrap(),
        )
        .unwrap();
        let other_evidence = other_cache.consumer().into_message_evidence_v1().unwrap();

        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::remote_decoder_assignment::dispatch_group_from_finalized_source_for_test_v1(
                manifest_source,
                &physical,
                other_evidence,
                1,
                crate::remote_decoder_assignment::RemoteDispatchTestMutationV1::None,
            )
        }));
        match outcome {
            Ok(crate::remote_chunk_dispatch::RemoteChunkTerminalV1::Failed(_)) | Err(_) => {}
            Ok(
                crate::remote_chunk_dispatch::RemoteChunkTerminalV1::Complete(_)
                | crate::remote_chunk_dispatch::RemoteChunkTerminalV1::CompleteEmpty,
            ) => panic!("cross-source all-zero evidence must fail before completion"),
        }
    }

    #[test]
    fn typed_output_locked_peak_covers_multi_row_bool_and_u64_builds() {
        fn run_case(definition: &str, payloads: &[Vec<u8>]) {
            let messages = payloads.iter().enumerate().map(|(index, payload)| {
                let sequence = u32::try_from(index).unwrap();
                let timestamp = u64::try_from(index).unwrap();
                FixtureMessage::new(1, sequence, timestamp + 1)
                    .with_publish_time(timestamp + 101)
                    .with_data(payload.clone())
            });
            let fixture = AdversarialMcapFixtureBuilder::new()
                .with_schemas([
                    FixtureSchema::new(7, "pkg/Root", "ros2msg").with_data(definition.as_bytes())
                ])
                .with_channels([FixtureChannel::schema_less(1, "/root").with_schema(7, "cdr")])
                .with_chunks([FixtureChunk::new(messages)])
                .with_partition_fixture(PartitionFixture::default())
                .with_summary_crc(FixtureCrc::Zero)
                .build()
                .unwrap();

            let exact = retained_layout_bytes(1).unwrap();
            let resolved = bound_pair(&fixture, layout_budget(1, exact))
                .prepare()
                .unwrap()
                .finalize_v1()
                .unwrap();
            let manifest_source = resolved.source_v1();
            let metadata_source = resolved.source_v1();
            let unit = metadata_source.source_unit_v1(0).unwrap();
            let metadata = unit.metadata_v1().unwrap();
            let physical = metadata.definitions_capability_for_full_chain_test_v1();
            let record = fixture.layout.chunks[0].record;
            let validated = install_exact_physical_chunk_record_for_test(
                unit.issue_lease_v1().unwrap(),
                fixture.bytes[record.start..record.end]
                    .to_vec()
                    .into_boxed_slice(),
            )
            .unwrap();
            let compressed = install_header_validated_payload_for_test(validated).unwrap();
            let decompressed =
                crate::remote_decompression::decompress_exact_chunk(compressed).unwrap();
            let scan = scan_decompressed_physical_chunk(decompressed).unwrap();
            let cache = crate::remote_chunk_scan::PhysicalChunkScanCacheEntry::new(scan).unwrap();
            let evidence = cache.consumer().into_message_evidence_v1().unwrap();
            let chunk = crate::remote_decoder_assignment::execute_ros_scalar_from_finalized_source_for_test_v1(
                manifest_source,
                &physical,
                evidence,
                1,
            );
            assert_eq!(chunk.num_rows(), payloads.len());
        }

        let int32_payloads = [1_i32, 2, 3]
            .into_iter()
            .map(|value| {
                let mut payload = vec![0, 1, 0, 0];
                payload.extend_from_slice(&value.to_le_bytes());
                payload
            })
            .collect::<Vec<_>>();
        run_case("int32 value", &int32_payloads);

        let bool_payloads = [false, true, false]
            .into_iter()
            .map(|value| vec![0, 1, 0, 0, u8::from(value)])
            .collect::<Vec<_>>();
        run_case("bool value", &bool_payloads);

        let u64_payloads = [1_u64, u64::from(u32::MAX) + 1, u64::MAX]
            .into_iter()
            .map(|value| {
                let mut payload = vec![0, 1, 0, 0];
                payload.extend_from_slice(&value.to_le_bytes());
                payload
            })
            .collect::<Vec<_>>();
        run_case("uint64 value", &u64_payloads);
    }

    #[test]
    fn finalized_owner_protobuf_nested_and_repeated_matches_local_decoder_chunk() {
        use crate::decoders::{DecoderRegistry, McapProtobufDecoder, TestEmitter, TopicFilter};
        use arrow::array::Array as _;
        use prost_reflect::prost::Message as _;
        use prost_reflect::prost_types::{
            DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
            OneofDescriptorProto, field_descriptor_proto,
        };

        let field = |name: &str,
                     number,
                     label: field_descriptor_proto::Label,
                     kind: field_descriptor_proto::Type,
                     type_name: Option<&str>| FieldDescriptorProto {
            name: Some(name.to_owned()),
            number: Some(number),
            label: Some(label as i32),
            r#type: Some(kind as i32),
            type_name: type_name.map(str::to_owned),
            ..Default::default()
        };
        let child = DescriptorProto {
            name: Some("Child".to_owned()),
            field: vec![
                field(
                    "first",
                    1,
                    field_descriptor_proto::Label::Optional,
                    field_descriptor_proto::Type::Int32,
                    None,
                ),
                field(
                    "second",
                    2,
                    field_descriptor_proto::Label::Optional,
                    field_descriptor_proto::Type::Int32,
                    None,
                ),
            ],
            ..Default::default()
        };
        let root = DescriptorProto {
            name: Some("Message".to_owned()),
            field: vec![
                field(
                    "implicit",
                    1,
                    field_descriptor_proto::Label::Optional,
                    field_descriptor_proto::Type::Int32,
                    None,
                ),
                field(
                    "nested",
                    2,
                    field_descriptor_proto::Label::Optional,
                    field_descriptor_proto::Type::Message,
                    Some(".pkg.Message.Child"),
                ),
                field(
                    "values",
                    3,
                    field_descriptor_proto::Label::Repeated,
                    field_descriptor_proto::Type::Int32,
                    None,
                ),
                FieldDescriptorProto {
                    oneof_index: Some(0),
                    proto3_optional: Some(true),
                    ..field(
                        "optional_text",
                        4,
                        field_descriptor_proto::Label::Optional,
                        field_descriptor_proto::Type::String,
                        None,
                    )
                },
            ],
            nested_type: vec![child],
            oneof_decl: vec![OneofDescriptorProto {
                name: Some("_optional_text".to_owned()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let descriptor = FileDescriptorSet {
            file: vec![FileDescriptorProto {
                name: Some("remote.proto".to_owned()),
                package: Some("pkg".to_owned()),
                message_type: vec![root],
                syntax: Some("proto3".to_owned()),
                ..Default::default()
            }],
        }
        .encode_to_vec();
        // Two singular nested occurrences must merge to {first:9, second:10}; values=[1]
        // unpacked followed by packed [2,3]; implicit and proto3 optional are absent.
        let payload = vec![
            0x12, 0x02, 0x08, 0x09, 0x12, 0x02, 0x10, 0x0a, 0x18, 0x01, 0x1a, 0x02, 0x02, 0x03,
        ];
        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_schemas([FixtureSchema::new(7, "pkg.Message", "protobuf").with_data(descriptor)])
            .with_channels([FixtureChannel::schema_less(1, "/protobuf").with_schema(7, "protobuf")])
            .with_chunks([FixtureChunk::single(
                FixtureMessage::new(1, 0, 1)
                    .with_publish_time(3)
                    .with_data(payload),
            )])
            .with_partition_fixture(PartitionFixture::default())
            .with_summary_crc(FixtureCrc::Zero)
            .build()
            .unwrap();

        let exact = retained_layout_bytes(1).unwrap();
        let resolved = bound_pair(&fixture, layout_budget(1, exact))
            .prepare()
            .unwrap()
            .finalize_v1()
            .unwrap();
        let manifest_source = resolved.source_v1();
        let metadata_source = resolved.source_v1();
        let unit = metadata_source.source_unit_v1(0).unwrap();
        let metadata = unit.metadata_v1().unwrap();
        let physical = metadata.definitions_capability_for_full_chain_test_v1();
        let record = fixture.layout.chunks[0].record;
        let validated = install_exact_physical_chunk_record_for_test(
            unit.issue_lease_v1().unwrap(),
            fixture.bytes[record.start..record.end]
                .to_vec()
                .into_boxed_slice(),
        )
        .unwrap();
        let compressed = install_header_validated_payload_for_test(validated).unwrap();
        let decompressed = crate::remote_decompression::decompress_exact_chunk(compressed).unwrap();
        let scan = scan_decompressed_physical_chunk(decompressed).unwrap();
        let cache = crate::remote_chunk_scan::PhysicalChunkScanCacheEntry::new(scan).unwrap();
        let evidence = cache.consumer().into_message_evidence_v1().unwrap();
        let remote =
            crate::remote_decoder_assignment::execute_ros_scalar_from_finalized_source_for_test_v1(
                manifest_source,
                &physical,
                evidence,
                1,
            );

        let summary = fixture.read_upstream_summary().unwrap().unwrap();
        let plan = DecoderRegistry::empty()
            .register_message_decoder::<McapProtobufDecoder>()
            .plan(&fixture.bytes, &summary, &TopicFilter::default())
            .unwrap();
        let emitter = TestEmitter::default();
        plan.run(
            &fixture.bytes,
            &summary,
            re_log_types::TimeType::TimestampNs,
            &*emitter,
        )
        .unwrap();
        let local = emitter.finish();
        let [local] = local.as_slice() else {
            panic!("the local protobuf oracle must emit exactly one chunk");
        };
        assert_eq!(remote.entity_path(), local.entity_path());
        assert_eq!(remote.timelines(), local.timelines());
        let remote_components = remote.components().iter().collect::<Vec<_>>();
        let [(remote_component, remote_column)] = remote_components.as_slice() else {
            panic!("the remote protobuf chunk must contain one component");
        };
        let local_components = local.components().iter().collect::<Vec<_>>();
        let [(local_component, local_column)] = local_components.as_slice() else {
            panic!("the local protobuf chunk must contain one component");
        };
        assert_eq!(remote_component, local_component);
        assert_eq!(remote_column.descriptor, local_column.descriptor);
        assert_eq!(
            remote_column.list_array.to_data(),
            local_column.list_array.to_data()
        );
    }

    #[test]
    fn finalized_owner_protobuf_oneof_and_enum_matches_local_decoder_chunk() {
        use crate::decoders::{DecoderRegistry, McapProtobufDecoder, TestEmitter, TopicFilter};
        use arrow::array::Array as _;
        use prost_reflect::prost::Message as _;
        use prost_reflect::prost_types::{
            DescriptorProto, EnumDescriptorProto, EnumValueDescriptorProto, FieldDescriptorProto,
            FileDescriptorProto, FileDescriptorSet, OneofDescriptorProto, field_descriptor_proto,
        };

        let field = |name: &str,
                     number,
                     kind: field_descriptor_proto::Type,
                     type_name: Option<&str>,
                     oneof_index: Option<i32>| FieldDescriptorProto {
            name: Some(name.to_owned()),
            number: Some(number),
            label: Some(field_descriptor_proto::Label::Optional as i32),
            r#type: Some(kind as i32),
            type_name: type_name.map(str::to_owned),
            oneof_index,
            ..Default::default()
        };
        let root = DescriptorProto {
            name: Some("Message".to_owned()),
            field: vec![
                field(
                    "mode",
                    1,
                    field_descriptor_proto::Type::Enum,
                    Some(".pkg.Mode"),
                    None,
                ),
                field(
                    "alpha",
                    2,
                    field_descriptor_proto::Type::Message,
                    Some(".pkg.Message.Child"),
                    Some(0),
                ),
                field(
                    "beta",
                    3,
                    field_descriptor_proto::Type::Int32,
                    None,
                    Some(0),
                ),
                FieldDescriptorProto {
                    default_value: Some("42".to_owned()),
                    ..field(
                        "threshold",
                        4,
                        field_descriptor_proto::Type::Int32,
                        None,
                        None,
                    )
                },
                FieldDescriptorProto {
                    label: Some(field_descriptor_proto::Label::Required as i32),
                    ..field(
                        "required_text",
                        5,
                        field_descriptor_proto::Type::String,
                        None,
                        None,
                    )
                },
                FieldDescriptorProto {
                    default_value: Some("line\\n\\u03bb".to_owned()),
                    ..field(
                        "escaped_default",
                        6,
                        field_descriptor_proto::Type::String,
                        None,
                        None,
                    )
                },
                FieldDescriptorProto {
                    default_value: Some("\\000\\377".to_owned()),
                    ..field(
                        "escaped_bytes_default",
                        7,
                        field_descriptor_proto::Type::Bytes,
                        None,
                        None,
                    )
                },
            ],
            oneof_decl: vec![OneofDescriptorProto {
                name: Some("choice".to_owned()),
                ..Default::default()
            }],
            nested_type: vec![DescriptorProto {
                name: Some("Child".to_owned()),
                field: vec![
                    field("x", 1, field_descriptor_proto::Type::Int32, None, None),
                    field("y", 2, field_descriptor_proto::Type::Int32, None, None),
                    field("z", 3, field_descriptor_proto::Type::Int32, None, None),
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        let mode = EnumDescriptorProto {
            name: Some("Mode".to_owned()),
            value: vec![
                EnumValueDescriptorProto {
                    name: Some("MODE_UNSPECIFIED".to_owned()),
                    number: Some(0),
                    ..Default::default()
                },
                EnumValueDescriptorProto {
                    name: Some("MODE_ACTIVE".to_owned()),
                    number: Some(1),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let descriptor = FileDescriptorSet {
            file: vec![FileDescriptorProto {
                name: Some("remote.proto".to_owned()),
                package: Some("pkg".to_owned()),
                message_type: vec![root],
                enum_type: vec![mode],
                syntax: Some("proto2".to_owned()),
                ..Default::default()
            }],
        }
        .encode_to_vec();
        // mode=MODE_ACTIVE; alpha{x=1}, beta=7, alpha{y=2}, alpha{z=3}. The final alpha suffix
        // must merge to {x:null,y:2,z:3}; the pre-switch x=1 must not leak across beta.
        let payload = vec![
            0x08, 0x01, 0x12, 0x02, 0x08, 0x01, 0x18, 0x07, 0x12, 0x02, 0x10, 0x02, 0x12, 0x02,
            0x18, 0x03, 0x2a, 0x02, b'o', b'k',
        ];
        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_schemas([FixtureSchema::new(7, "pkg.Message", "protobuf").with_data(descriptor)])
            .with_channels([FixtureChannel::schema_less(1, "/protobuf").with_schema(7, "protobuf")])
            .with_chunks([FixtureChunk::single(
                FixtureMessage::new(1, 0, 1)
                    .with_publish_time(3)
                    .with_data(payload),
            )])
            .with_partition_fixture(PartitionFixture::default())
            .with_summary_crc(FixtureCrc::Zero)
            .build()
            .unwrap();

        let exact = retained_layout_bytes(1).unwrap();
        let resolved = bound_pair(&fixture, layout_budget(1, exact))
            .prepare()
            .unwrap()
            .finalize_v1()
            .unwrap();
        let manifest_source = resolved.source_v1();
        let metadata_source = resolved.source_v1();
        let unit = metadata_source.source_unit_v1(0).unwrap();
        let metadata = unit.metadata_v1().unwrap();
        let physical = metadata.definitions_capability_for_full_chain_test_v1();
        let record = fixture.layout.chunks[0].record;
        let validated = install_exact_physical_chunk_record_for_test(
            unit.issue_lease_v1().unwrap(),
            fixture.bytes[record.start..record.end]
                .to_vec()
                .into_boxed_slice(),
        )
        .unwrap();
        let compressed = install_header_validated_payload_for_test(validated).unwrap();
        let decompressed = crate::remote_decompression::decompress_exact_chunk(compressed).unwrap();
        let scan = scan_decompressed_physical_chunk(decompressed).unwrap();
        let cache = crate::remote_chunk_scan::PhysicalChunkScanCacheEntry::new(scan).unwrap();
        let evidence = cache.consumer().into_message_evidence_v1().unwrap();
        let remote =
            crate::remote_decoder_assignment::execute_ros_scalar_from_finalized_source_for_test_v1(
                manifest_source,
                &physical,
                evidence,
                1,
            );

        let summary = fixture.read_upstream_summary().unwrap().unwrap();
        let plan = DecoderRegistry::empty()
            .register_message_decoder::<McapProtobufDecoder>()
            .plan(&fixture.bytes, &summary, &TopicFilter::default())
            .unwrap();
        let emitter = TestEmitter::default();
        plan.run(
            &fixture.bytes,
            &summary,
            re_log_types::TimeType::TimestampNs,
            &*emitter,
        )
        .unwrap();
        let local = emitter.finish();
        let [local] = local.as_slice() else {
            panic!("the local protobuf oracle must emit exactly one chunk");
        };
        assert_eq!(remote.entity_path(), local.entity_path());
        assert_eq!(remote.timelines(), local.timelines());
        let remote_components = remote.components().iter().collect::<Vec<_>>();
        let [(remote_component, remote_column)] = remote_components.as_slice() else {
            panic!("the remote protobuf chunk must contain one component");
        };
        let local_components = local.components().iter().collect::<Vec<_>>();
        let [(local_component, local_column)] = local_components.as_slice() else {
            panic!("the local protobuf chunk must contain one component");
        };
        assert_eq!(remote_component, local_component);
        assert_eq!(remote_column.descriptor, local_column.descriptor);
        assert_eq!(
            remote_column.list_array.to_data(),
            local_column.list_array.to_data()
        );
    }

    #[test]
    fn finalized_owner_protobuf_map_and_unknown_field_matches_local_decoder_chunk() {
        use crate::decoders::{DecoderRegistry, McapProtobufDecoder, TestEmitter, TopicFilter};
        use arrow::array::Array as _;
        use prost_reflect::prost::Message as _;
        use prost_reflect::prost_types::{
            DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
            MessageOptions, field_descriptor_proto,
        };

        let field = |name: &str,
                     number,
                     label: field_descriptor_proto::Label,
                     kind: field_descriptor_proto::Type,
                     type_name: Option<&str>| FieldDescriptorProto {
            name: Some(name.to_owned()),
            number: Some(number),
            label: Some(label as i32),
            r#type: Some(kind as i32),
            type_name: type_name.map(str::to_owned),
            ..Default::default()
        };
        let entry = DescriptorProto {
            name: Some("LabelsEntry".to_owned()),
            field: vec![
                field(
                    "key",
                    1,
                    field_descriptor_proto::Label::Optional,
                    field_descriptor_proto::Type::String,
                    None,
                ),
                field(
                    "value",
                    2,
                    field_descriptor_proto::Label::Optional,
                    field_descriptor_proto::Type::Message,
                    Some(".pkg.Message.Value"),
                ),
            ],
            options: Some(MessageOptions {
                map_entry: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };
        let root = DescriptorProto {
            name: Some("Message".to_owned()),
            field: vec![field(
                "labels",
                1,
                field_descriptor_proto::Label::Repeated,
                field_descriptor_proto::Type::Message,
                Some(".pkg.Message.LabelsEntry"),
            )],
            nested_type: vec![
                entry,
                DescriptorProto {
                    name: Some("Value".to_owned()),
                    field: vec![field(
                        "number",
                        1,
                        field_descriptor_proto::Label::Optional,
                        field_descriptor_proto::Type::Int32,
                        None,
                    )],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let descriptor = FileDescriptorSet {
            file: vec![FileDescriptorProto {
                name: Some("remote.proto".to_owned()),
                package: Some("pkg".to_owned()),
                message_type: vec![root],
                syntax: Some("proto3".to_owned()),
                ..Default::default()
            }],
        }
        .encode_to_vec();
        // labels={"b":<missing message>,"a":{1},"a":{5}} in reverse/duplicate key order,
        // followed by unknown field 99=7. Both decoders must apply last-key-wins, sort the map,
        // materialize the missing message value exactly like local reflection, and ignore the
        // bounded unknown application field.
        let payload = vec![
            0x0a, 0x03, 0x0a, 0x01, b'b', 0x0a, 0x07, 0x0a, 0x01, b'a', 0x12, 0x02, 0x08, 0x01,
            0x0a, 0x07, 0x0a, 0x01, b'a', 0x12, 0x02, 0x08, 0x05, 0x98, 0x06, 0x07,
        ];
        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_schemas([FixtureSchema::new(7, "pkg.Message", "protobuf").with_data(descriptor)])
            .with_channels([FixtureChannel::schema_less(1, "/protobuf").with_schema(7, "protobuf")])
            .with_chunks([FixtureChunk::single(
                FixtureMessage::new(1, 0, 1)
                    .with_publish_time(3)
                    .with_data(payload),
            )])
            .with_partition_fixture(PartitionFixture::default())
            .with_summary_crc(FixtureCrc::Zero)
            .build()
            .unwrap();

        let exact = retained_layout_bytes(1).unwrap();
        let resolved = bound_pair(&fixture, layout_budget(1, exact))
            .prepare()
            .unwrap()
            .finalize_v1()
            .unwrap();
        let manifest_source = resolved.source_v1();
        let metadata_source = resolved.source_v1();
        let unit = metadata_source.source_unit_v1(0).unwrap();
        let metadata = unit.metadata_v1().unwrap();
        let physical = metadata.definitions_capability_for_full_chain_test_v1();
        let record = fixture.layout.chunks[0].record;
        let validated = install_exact_physical_chunk_record_for_test(
            unit.issue_lease_v1().unwrap(),
            fixture.bytes[record.start..record.end]
                .to_vec()
                .into_boxed_slice(),
        )
        .unwrap();
        let compressed = install_header_validated_payload_for_test(validated).unwrap();
        let decompressed = crate::remote_decompression::decompress_exact_chunk(compressed).unwrap();
        let scan = scan_decompressed_physical_chunk(decompressed).unwrap();
        let cache = crate::remote_chunk_scan::PhysicalChunkScanCacheEntry::new(scan).unwrap();
        let evidence = cache.consumer().into_message_evidence_v1().unwrap();
        let remote =
            crate::remote_decoder_assignment::execute_ros_scalar_from_finalized_source_for_test_v1(
                manifest_source,
                &physical,
                evidence,
                1,
            );

        let summary = fixture.read_upstream_summary().unwrap().unwrap();
        let plan = DecoderRegistry::empty()
            .register_message_decoder::<McapProtobufDecoder>()
            .plan(&fixture.bytes, &summary, &TopicFilter::default())
            .unwrap();
        let emitter = TestEmitter::default();
        plan.run(
            &fixture.bytes,
            &summary,
            re_log_types::TimeType::TimestampNs,
            &*emitter,
        )
        .unwrap();
        let local = emitter.finish();
        let [local] = local.as_slice() else {
            panic!("the local protobuf oracle must emit exactly one chunk");
        };
        assert_eq!(remote.entity_path(), local.entity_path());
        assert_eq!(remote.timelines(), local.timelines());
        let remote_components = remote.components().iter().collect::<Vec<_>>();
        let [(remote_component, remote_column)] = remote_components.as_slice() else {
            panic!("the remote protobuf chunk must contain one component");
        };
        let local_components = local.components().iter().collect::<Vec<_>>();
        let [(local_component, local_column)] = local_components.as_slice() else {
            panic!("the local protobuf chunk must contain one component");
        };
        assert_eq!(remote_component, local_component);
        assert_eq!(remote_column.descriptor, local_column.descriptor);
        assert_eq!(
            remote_column.list_array.to_data(),
            local_column.list_array.to_data()
        );
    }

    #[test]
    fn object_identity_is_fresh_and_contains_only_typed_projection() {
        let first = RemotePhysicalObjectBindingV1::issue_for_test(
            NonZeroU64::new(42).unwrap(),
            RemoteObjectConsistencyClassV1::StrongValidator,
        )
        .unwrap();
        let second = RemotePhysicalObjectBindingV1::issue_for_test(
            NonZeroU64::new(42).unwrap(),
            RemoteObjectConsistencyClassV1::StrongValidator,
        )
        .unwrap();
        assert_ne!(first.identity_for_test().0, second.identity_for_test().0);
        assert_eq!(first.identity_for_test().1, NonZeroU64::new(42).unwrap());
        assert_eq!(
            format!("{first:?}"),
            "RemotePhysicalObjectBindingV1 { identity: \"<opaque fresh-per-open>\", .. }"
        );
    }

    #[test]
    fn same_length_objects_have_distinct_sealed_owners() {
        let fixture = fixture([FixtureChunk::empty()]);
        let exact = retained_layout_bytes(1).unwrap();
        let first = bound_pair(&fixture, layout_budget(1, exact));
        let second = bound_pair(&fixture, layout_budget(1, exact));
        assert!(!Arc::ptr_eq(
            &first.object_lifetime.state,
            &second.object_lifetime.state
        ));
        drop(first);
        drop(second);
    }

    #[test]
    fn profile_and_budget_are_not_parameters_of_the_resolution_transition() {
        let source = include_str!("remote_physical_resolution.rs");
        let prepare_start = source
            .find("pub(crate) fn prepare(\n        self,")
            .expect("the enclosing prepare transition exists");
        let prepare_end = source[prepare_start..]
            .find("pub struct BoundPreparedPhysicalSourceResolutionV1")
            .map(|offset| prepare_start + offset)
            .expect("the enclosing prepare transition ends before its output owner");
        let signature = &source[prepare_start..prepare_end];
        assert!(signature.contains("self,"));
        assert!(!signature.contains("decompression_budget:"));
        assert!(!signature.contains("scan_budget:"));
        assert!(!signature.contains("layout_budget:"));

        let forbidden_artifact_issuer = ["issue_for_verified", "_artifact_v1"].concat();
        assert!(!source.contains(&forbidden_artifact_issuer));
        assert!(source.contains("#[cfg(test)]\n    fn issue_for_test("));
        assert!(
            !source.contains("#[cfg(re_mcap_locked_remote_wasm_allocator_v1)]\n    pub fn issue")
        );
    }

    #[test]
    fn bound_final_owner_exposes_only_borrowed_source_projection() {
        let fixture = fixture([FixtureChunk::single(FixtureMessage::new(1, 0, 7))]);
        let exact = retained_layout_bytes(1).unwrap();
        let prepared = bound_pair(&fixture, layout_budget(1, exact))
            .prepare()
            .unwrap();
        let resolved = prepared.finalize_v1().unwrap();
        let source = resolved.source_v1();
        assert_eq!(
            source.layout_v1().canonical_classification_v1(0),
            Some(CanonicalPhysicalExtentV1::Known {
                start: canonicalize_raw_mcap_time(RawMcapTime::new(7)).unwrap(),
                end: canonicalize_raw_mcap_time(RawMcapTime::new(7)).unwrap(),
            })
        );
        assert_eq!(
            source.layout_v1().canonical_interval_v1(0),
            Some((
                canonicalize_raw_mcap_time(RawMcapTime::new(7)).unwrap(),
                canonicalize_raw_mcap_time(RawMcapTime::new(7)).unwrap(),
            ))
        );
        let unit = source.source_unit_v1(0).unwrap();
        assert_eq!(unit.canonical_ordinal_v1(), 0);
        assert_eq!(
            unit.classification_v1(),
            Some(CanonicalPhysicalExtentV1::Known {
                start: canonicalize_raw_mcap_time(RawMcapTime::new(7)).unwrap(),
                end: canonicalize_raw_mcap_time(RawMcapTime::new(7)).unwrap(),
            })
        );
        assert_eq!(
            unit.canonical_interval_v1(),
            source.layout_v1().canonical_interval_v1(0)
        );
        let lease = unit.issue_lease_v1().unwrap();
        drop(lease);
        let metadata = unit.metadata_v1().unwrap();
        let descriptors = metadata.definition_descriptors_v1().count();
        assert!(descriptors > 0);
    }

    #[test]
    fn zero_unresolved_finalizes_without_a_read_and_keeps_overlap_safe_projection() {
        let fixture = fixture([
            FixtureChunk::single(FixtureMessage::new(1, 0, 3)),
            FixtureChunk::single(FixtureMessage::new(1, 1, 10)),
        ]);
        let bytes = retained_layout_bytes(2).unwrap();
        let budget = layout_budget(2, bytes);
        let prepared = prepare(&fixture, budget);
        let resolved = prepared.finalize_v1().unwrap();
        let source = resolved.source_v1();
        assert_eq!(source.layout_v1().canonical_chunk_count_v1(), 2);
        assert_eq!(
            source.layout_v1().canonical_extent_v1(),
            CanonicalPhysicalExtentV1::Known {
                start: canonicalize_raw_mcap_time(RawMcapTime::new(3)).unwrap(),
                end: canonicalize_raw_mcap_time(RawMcapTime::new(10)).unwrap(),
            }
        );
        let lease = source.issue_lease_v1(0).unwrap();
        drop(lease);
    }

    #[test]
    fn one_and_multiple_unresolved_resolve_strictly_in_order() {
        let fixture = fixture([
            FixtureChunk::empty(),
            FixtureChunk::empty(),
            FixtureChunk::single(FixtureMessage::new(1, 2, 5)),
        ]);
        let budget = layout_budget(3, retained_layout_bytes(3).unwrap());
        let mut prepared = prepare(&fixture, budget);
        let first = resolve_next(prepared.inner_mut_v1(), &fixture, 0);
        assert_eq!(
            prepared
                .inner_mut_v1()
                .issue_next_resolution_lease_v1()
                .unwrap_err(),
            PhysicalSourceResolutionErrorV1::Physical(
                PhysicalChunkValidationError::DuplicateLiveRead
            )
        );
        prepared.inner_mut_v1().submit_resolution_v1(first).unwrap();
        let second = resolve_next(prepared.inner_mut_v1(), &fixture, 1);
        prepared
            .inner_mut_v1()
            .submit_resolution_v1(second)
            .unwrap();
        let resolved = prepared.finalize_v1().unwrap();
        assert_eq!(
            resolved.source_v1().layout_v1().canonical_extent_v1(),
            CanonicalPhysicalExtentV1::Known {
                start: canonicalize_raw_mcap_time(RawMcapTime::new(5)).unwrap(),
                end: canonicalize_raw_mcap_time(RawMcapTime::new(5)).unwrap(),
            }
        );
    }

    #[test]
    fn drop_closes_authority_and_exact_minus_one_budget_rejects_before_any_read() {
        let fixture = fixture([FixtureChunk::empty()]);
        let exact = retained_layout_bytes(1).unwrap();
        assert!(matches!(
            bound_pair(
                &fixture,
                AggregateResolutionBudgetRootV1::new_for_sealed_profile_v1(1, 0)
            )
            .prepare(),
            Err(PhysicalSourceResolutionErrorV1::ReservationLimitExceeded)
        ));

        let budget = layout_budget(1, exact);
        let mut prepared = prepare(&fixture, budget);
        let binding = prepared.inner_mut_v1().source_binding.clone();
        drop(prepared);
        assert_eq!(
            binding.ensure_current_v1(),
            Err(PhysicalChunkValidationError::SourceClosed)
        );
    }

    #[test]
    fn aggregate_reject_keeps_root_usage_zero_before_lower_claim() {
        let fixture = fixture([FixtureChunk::empty()]);
        let exact = retained_layout_bytes(1).unwrap();
        let root = AggregateResolutionBudgetRootV1::new_for_sealed_profile_v1(1, exact - 1);
        let result = bound_pair(&fixture, root.clone_for_test_v1()).prepare();
        assert!(matches!(
            result,
            Err(PhysicalSourceResolutionErrorV1::ReservationLimitExceeded)
        ));
        assert_eq!(
            root.usage_for_test_v1(),
            AggregateResolutionBudgetUsageV1::default()
        );
    }

    #[test]
    fn post_claim_allocation_failure_rolls_back_root_claim() {
        let fixture = fixture([FixtureChunk::empty()]);
        let exact = retained_layout_bytes(1).unwrap();
        let root = layout_budget(1, exact);
        fail_next_post_claim_allocation();
        let result = bound_pair(&fixture, root.clone_for_test_v1()).prepare();
        assert_eq!(
            result.err(),
            Some(PhysicalSourceResolutionErrorV1::AllocationFailed)
        );
        assert_eq!(
            root.usage_for_test_v1(),
            AggregateResolutionBudgetUsageV1::default()
        );
    }

    #[test]
    fn deployment_assumed_binding_preserves_typed_consistency_class() {
        let object = RemotePhysicalObjectBindingV1::issue_for_test(
            NonZeroU64::new(17).unwrap(),
            RemoteObjectConsistencyClassV1::DeploymentAssumed,
        )
        .unwrap();
        assert_eq!(
            object.identity_for_test().2,
            RemoteObjectConsistencyClassV1::DeploymentAssumed
        );
    }

    #[test]
    fn read_owner_and_content_length_are_checked_before_lower_parsing() {
        use crate::remote_fixed_layout::RemoteMcapSlice;

        let object = RemotePhysicalObjectBindingV1::issue_for_test(
            NonZeroU64::new(8).unwrap(),
            RemoteObjectConsistencyClassV1::StrongValidator,
        )
        .unwrap();
        let issuer = RemoteObjectReadIssuerV1 {
            object: Arc::clone(&object.state),
        };
        assert!(matches!(
            issuer.issue_v1(RemoteMcapSlice::new(7, &[0, 1])),
            Err(PhysicalSourceResolutionErrorV1::ObjectLengthMismatch)
        ));

        let other = RemotePhysicalObjectBindingV1::issue_for_test(
            NonZeroU64::new(8).unwrap(),
            RemoteObjectConsistencyClassV1::StrongValidator,
        )
        .unwrap();
        let read = RemoteObjectReadIssuerV1 {
            object: Arc::clone(&other.state),
        }
        .issue_v1(RemoteMcapSlice::new(0, &[0]))
        .unwrap();
        assert!(matches!(
            read.into_slice_v1(&object),
            Err(PhysicalSourceResolutionErrorV1::ObjectBindingMismatch)
        ));
    }

    #[test]
    fn census_accepts_exact_limit_and_rejects_one_byte_over() {
        let fixture = fixture([FixtureChunk::empty()]);
        let exact = retained_layout_bytes(1).unwrap();
        let measuring_root = layout_budget(1, exact);
        let prepared = prepare(&fixture, measuring_root.clone_for_test_v1());
        let measured = measuring_root.usage_for_test_v1().retained_bytes;
        drop(prepared);

        let exact_root = AggregateResolutionBudgetRootV1::new_for_sealed_profile_v1(1, measured);
        assert!(bound_pair(&fixture, exact_root).prepare().is_ok());

        let root = AggregateResolutionBudgetRootV1::new_for_sealed_profile_v1(1, measured - 1);
        assert_eq!(
            bound_pair(&fixture, root).prepare().err(),
            Some(PhysicalSourceResolutionErrorV1::ReservationLimitExceeded)
        );
        assert_eq!(
            FullResolutionAdmissionCensusV1::checked(u64::MAX, 0, 0, 0, 0).unwrap_err(),
            PhysicalSourceResolutionErrorV1::ArithmeticOverflow
        );
    }

    #[test]
    fn stale_resolution_result_cannot_cross_source_or_reopen_a_slot() {
        let fixture = fixture([FixtureChunk::empty()]);
        let exact = retained_layout_bytes(1).unwrap();
        let mut first = prepare(&fixture, layout_budget(1, exact));
        let foreign = resolve_next(first.inner_mut_v1(), &fixture, 0);
        let mut second = prepare(&fixture, layout_budget(1, exact));
        assert_eq!(
            second.inner_mut_v1().submit_resolution_v1(foreign),
            Err(PhysicalSourceResolutionErrorV1::PhysicalSourceBindingMismatch)
        );
        let own = resolve_next(second.inner_mut_v1(), &fixture, 0);
        second.inner_mut_v1().submit_resolution_v1(own).unwrap();
        assert!(matches!(
            second.inner_mut_v1().issue_next_resolution_lease_v1(),
            Err(PhysicalSourceResolutionErrorV1::ResolutionComplete)
        ));
    }

    #[test]
    fn duplicate_and_out_of_order_resolution_are_rejected_without_advancing_cursor() {
        let fixture = fixture([FixtureChunk::empty(), FixtureChunk::empty()]);
        let exact = retained_layout_bytes(2).unwrap();
        let mut prepared = prepare(&fixture, layout_budget(2, exact));
        let mut first = resolve_next(prepared.inner_mut_v1(), &fixture, 0);
        first.canonical_ordinal = 1;
        assert_eq!(
            prepared.inner_mut_v1().submit_resolution_v1(first),
            Err(PhysicalSourceResolutionErrorV1::OutOfOrderResolution)
        );
        let first = resolve_next(prepared.inner_mut_v1(), &fixture, 0);
        prepared.inner_mut_v1().submit_resolution_v1(first).unwrap();
        let second = resolve_next(prepared.inner_mut_v1(), &fixture, 1);
        prepared
            .inner_mut_v1()
            .submit_resolution_v1(second)
            .unwrap();
        assert!(matches!(
            prepared.inner_mut_v1().issue_next_resolution_lease_v1(),
            Err(PhysicalSourceResolutionErrorV1::ResolutionComplete)
        ));
    }

    #[test]
    fn competing_sources_are_bounded_by_root_and_release_full_usage_on_drop() {
        let fixture = fixture([FixtureChunk::empty()]);
        let exact = retained_layout_bytes(1).unwrap();
        let measurement = layout_budget(1, exact);
        let sample = prepare(&fixture, measurement.clone_for_test_v1());
        let measured = measurement.usage_for_test_v1().retained_bytes;
        drop(sample);
        let root = AggregateResolutionBudgetRootV1::new_for_sealed_profile_v1(1, measured * 2);
        let first = bound_pair(&fixture, root.clone_for_test_v1())
            .prepare()
            .unwrap();
        assert!(matches!(
            bound_pair(&fixture, root.clone_for_test_v1()).prepare(),
            Err(PhysicalSourceResolutionErrorV1::ReservationLimitExceeded)
        ));
        drop(first);
        assert_eq!(root.usage_for_test_v1().active, 0);
        assert_eq!(root.usage_for_test_v1().retained_bytes, 0);
    }

    #[test]
    fn incomplete_finalize_is_terminal_and_releases_root_reservation() {
        let fixture = fixture([FixtureChunk::empty()]);
        let exact = retained_layout_bytes(1).unwrap();
        let root = layout_budget(1, exact);
        let prepared = prepare(&fixture, root.clone_for_test_v1());
        assert!(matches!(
            prepared.finalize_v1(),
            Err(PhysicalSourceResolutionErrorV1::ResolutionIncomplete)
        ));
        assert_eq!(
            root.usage_for_test_v1(),
            AggregateResolutionBudgetUsageV1::default()
        );
    }

    #[test]
    fn decompression_failure_does_not_advance_resolution_and_drop_releases_root() {
        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_chunks([
                FixtureChunk::empty(),
                FixtureChunk::single(FixtureMessage::new(1, 0, 7)),
            ])
            .with_chunk_crc(FixtureCrc::InvalidNonZero)
            .with_summary_crc(FixtureCrc::Zero)
            .build()
            .unwrap();
        let root = layout_budget(2, retained_layout_bytes(2).unwrap());
        let mut prepared = prepare(&fixture, root.clone_for_test_v1());
        let lease = prepared
            .inner_mut_v1()
            .issue_next_resolution_lease_v1()
            .unwrap();
        let record = fixture.layout.chunks[0].record;
        let validated = install_exact_physical_chunk_record_for_test(
            lease,
            fixture.bytes[record.start..record.end]
                .to_vec()
                .into_boxed_slice(),
        )
        .unwrap();
        let input = install_header_validated_payload_for_test(validated).unwrap();
        assert_eq!(
            crate::remote_decompression::decompress_exact_chunk(input).unwrap_err(),
            crate::remote_decompression::ChunkDecompressionError::ChunkChecksumMismatch
        );
        assert!(
            prepared
                .inner_mut_v1()
                .issue_next_resolution_lease_v1()
                .is_ok()
        );
        drop(prepared);
        assert_eq!(
            root.usage_for_test_v1(),
            AggregateResolutionBudgetUsageV1::default()
        );
    }

    #[test]
    fn malformed_physical_record_does_not_advance_resolution_and_drop_releases_root() {
        let fixture = AdversarialMcapFixtureBuilder::new()
            .with_chunks([
                FixtureChunk::empty(),
                FixtureChunk::single(FixtureMessage::new(1, 0, 7)),
            ])
            .with_summary_crc(FixtureCrc::Zero)
            .build()
            .unwrap();
        let root = layout_budget(2, retained_layout_bytes(2).unwrap());
        let mut prepared = prepare(&fixture, root.clone_for_test_v1());
        let lease = prepared
            .inner_mut_v1()
            .issue_next_resolution_lease_v1()
            .unwrap();
        assert_eq!(
            install_exact_physical_chunk_record_for_test(lease, vec![0].into_boxed_slice())
                .unwrap_err(),
            PhysicalChunkValidationError::FullRecordLengthMismatch
        );
        assert!(
            prepared
                .inner_mut_v1()
                .issue_next_resolution_lease_v1()
                .is_ok()
        );
        drop(prepared);
        assert_eq!(
            root.usage_for_test_v1(),
            AggregateResolutionBudgetUsageV1::default()
        );
    }

    #[test]
    fn interval_projection_keeps_earlier_long_overlap() {
        let fixture = fixture([
            FixtureChunk::single(FixtureMessage::new(1, 0, 0))
                .with_index_range(RawTimeRange::new(1, 100)),
            FixtureChunk::single(FixtureMessage::new(1, 1, 0))
                .with_index_range(RawTimeRange::new(20, 30)),
            FixtureChunk::single(FixtureMessage::new(1, 2, 0))
                .with_index_range(RawTimeRange::new(40, 50)),
        ]);
        let budget = layout_budget(3, retained_layout_bytes(3).unwrap());
        let prepared = prepare(&fixture, budget);
        let resolved = prepared.finalize_v1().unwrap();
        let source = resolved.source_v1();
        let query = canonicalize_raw_mcap_time(RawMcapTime::new(45)).unwrap();
        assert_eq!(
            source
                .layout_v1()
                .intersecting_ordinals_v1(query, query)
                .collect::<Vec<_>>(),
            [0, 2]
        );
    }

    #[test]
    fn resolved_authority_issues_physical_receipt_and_surfaces_matching_material() {
        use re_mcap_web_contract::CorrelationFactoryV1;

        let fixture = fixture([FixtureChunk::single(FixtureMessage::new(1, 1, 0))]);
        let budget = layout_budget(1, retained_layout_bytes(1).unwrap());
        let prepared = prepare(&fixture, budget);
        let resolved = prepared.finalize_v1().unwrap();

        let (_pair, _web, mcap_permit) = CorrelationFactoryV1::new_operation_v1();
        let receipt = resolved
            .inner
            .issue_physical_receipt_v1(0, mcap_permit)
            .unwrap();

        let record = fixture.layout.chunks[0].record;
        let body = &fixture.bytes[record.start..record.end];
        let (_borrowed, material) = receipt.bind_exact_body_v1(body).unwrap();
        let _ = material;
    }
}
