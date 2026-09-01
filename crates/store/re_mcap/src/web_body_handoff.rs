//! Web-only sealed handoff from an exact Range body to the physical-Chunk cache.

use std::sync::Arc;

use parking_lot::Mutex;

#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
use re_mcap_web_contract::McapCorrelationMaterialV1;
#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
use re_mcap_web_contract::McapCorrelationPermitV1;

use crate::remote_chunk_scan::{
    PendingHeaderValidation, PhysicalChunkReadLease, PhysicalChunkScanCacheEntry,
};

const MAX_PHASE_A_FULL_RECORD_BYTES_V1: u64 = 16 * 1024 * 1024 + 64 * 1024;
const MAX_PHASE_A_DESTINATION_BYTES_V1: u64 = 16 * 1024 * 1024;
const MAX_PHASE_A_OVERLAP_BYTES_V1: u64 =
    MAX_PHASE_A_FULL_RECORD_BYTES_V1 + MAX_PHASE_A_DESTINATION_BYTES_V1;
const PHASE_A_RELEASE_BENCHMARK_CANDIDATE_MICROS_V1: u64 = 8_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebPhysicalBodyStrategyV1 {
    ZeroCopyV1,
    ExplicitCopyV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebPhysicalBodyProfileStatusV1 {
    /// Release-Wasm Chrome measurements have not yet sealed this profile.
    Unfrozen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WebPhysicalBodyPhaseAProfileV1 {
    pub status: WebPhysicalBodyProfileStatusV1,
    pub strategy: WebPhysicalBodyStrategyV1,
    pub max_full_record_bytes: u64,
    pub max_destination_bytes: u64,
    pub max_simultaneous_overlap_bytes: u64,

    /// A benchmark target only.
    ///
    /// This value is telemetry metadata and must never participate in admission or correctness.
    pub release_benchmark_candidate_micros: u64,
}

pub const WEB_PHYSICAL_BODY_PHASE_A_CANDIDATE_PROFILE_V1: WebPhysicalBodyPhaseAProfileV1 =
    WebPhysicalBodyPhaseAProfileV1 {
        status: WebPhysicalBodyProfileStatusV1::Unfrozen,
        strategy: WebPhysicalBodyStrategyV1::ZeroCopyV1,
        max_full_record_bytes: MAX_PHASE_A_FULL_RECORD_BYTES_V1,
        max_destination_bytes: MAX_PHASE_A_DESTINATION_BYTES_V1,
        max_simultaneous_overlap_bytes: MAX_PHASE_A_OVERLAP_BYTES_V1,
        release_benchmark_candidate_micros: PHASE_A_RELEASE_BENCHMARK_CANDIDATE_MICROS_V1,
    };

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct WebPhysicalCopyOverlapUsageV1 {
    active: u64,
    simultaneous_bytes: u64,
    high_water_bytes: u64,
}

struct WebPhysicalCopyOverlapStateV1 {
    max_full_record_bytes: u64,
    max_destination_bytes: u64,
    max_simultaneous_overlap_bytes: u64,
    usage: Mutex<WebPhysicalCopyOverlapUsageV1>,
}

pub struct WebPhysicalCopyOverlapBudgetV1 {
    state: Arc<WebPhysicalCopyOverlapStateV1>,
}

impl WebPhysicalCopyOverlapBudgetV1 {
    pub fn new_unfrozen_phase_a_v1() -> Self {
        Self {
            state: Arc::new(WebPhysicalCopyOverlapStateV1 {
                max_full_record_bytes: WEB_PHYSICAL_BODY_PHASE_A_CANDIDATE_PROFILE_V1
                    .max_full_record_bytes,
                max_destination_bytes: WEB_PHYSICAL_BODY_PHASE_A_CANDIDATE_PROFILE_V1
                    .max_destination_bytes,
                max_simultaneous_overlap_bytes: WEB_PHYSICAL_BODY_PHASE_A_CANDIDATE_PROFILE_V1
                    .max_simultaneous_overlap_bytes,
                usage: Mutex::new(WebPhysicalCopyOverlapUsageV1::default()),
            }),
        }
    }

    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(crate) fn new_for_test_v1(max_simultaneous_overlap_bytes: u64) -> Self {
        Self {
            state: Arc::new(WebPhysicalCopyOverlapStateV1 {
                max_full_record_bytes: u64::MAX,
                max_destination_bytes: u64::MAX,
                max_simultaneous_overlap_bytes,
                usage: Mutex::new(WebPhysicalCopyOverlapUsageV1::default()),
            }),
        }
    }

    fn reserve_v1(
        &self,
        full_record_bytes: u64,
        destination_bytes: u64,
    ) -> Result<WebPhysicalCopyOverlapReservationV1, WebPhysicalBodyHandoffErrorV1> {
        let simultaneous_bytes = full_record_bytes
            .checked_add(destination_bytes)
            .ok_or(WebPhysicalBodyHandoffErrorV1::ResourceLimitExceeded)?;
        if full_record_bytes > self.state.max_full_record_bytes
            || destination_bytes > self.state.max_destination_bytes
            || simultaneous_bytes > self.state.max_simultaneous_overlap_bytes
        {
            return Err(WebPhysicalBodyHandoffErrorV1::ResourceLimitExceeded);
        }
        let mut usage = self.state.usage.lock();
        let active = usage
            .active
            .checked_add(1)
            .ok_or(WebPhysicalBodyHandoffErrorV1::ResourceLimitExceeded)?;
        let retained = usage
            .simultaneous_bytes
            .checked_add(simultaneous_bytes)
            .ok_or(WebPhysicalBodyHandoffErrorV1::ResourceLimitExceeded)?;
        if active > 1 || retained > self.state.max_simultaneous_overlap_bytes {
            return Err(WebPhysicalBodyHandoffErrorV1::ResourceLimitExceeded);
        }
        usage.active = active;
        usage.simultaneous_bytes = retained;
        usage.high_water_bytes = usage.high_water_bytes.max(retained);
        drop(usage);
        Ok(WebPhysicalCopyOverlapReservationV1 {
            state: Arc::clone(&self.state),
            simultaneous_bytes,
        })
    }

    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(crate) fn usage_for_test_v1(&self) -> (u64, u64, u64) {
        let usage = *self.state.usage.lock();
        (
            usage.active,
            usage.simultaneous_bytes,
            usage.high_water_bytes,
        )
    }
}

struct WebPhysicalCopyOverlapReservationV1 {
    state: Arc<WebPhysicalCopyOverlapStateV1>,
    simultaneous_bytes: u64,
}

impl Drop for WebPhysicalCopyOverlapReservationV1 {
    fn drop(&mut self) {
        let mut usage = self.state.usage.lock();
        usage.active = usage
            .active
            .checked_sub(1)
            .expect("physical body overlap active usage underflowed");
        usage.simultaneous_bytes = usage
            .simultaneous_bytes
            .checked_sub(self.simultaneous_bytes)
            .expect("physical body overlap byte usage underflowed");
    }
}

/// Move-only physical authority issued from a live canonical MCAP read lease.
pub struct WebPhysicalReceiptV1<'a> {
    lease: Option<PhysicalChunkReadLease<'a, PendingHeaderValidation>>,
    #[cfg(any(
        all(test, not(target_arch = "wasm32")),
        rerun_mcap_phase_a_proof_v1,
        re_mcap_locked_remote_wasm_allocator_v1
    ))]
    profile: WebPhysicalBudgetProfileV1,
    #[cfg(any(
        all(test, not(target_arch = "wasm32")),
        rerun_mcap_phase_a_proof_v1,
        re_mcap_locked_remote_wasm_allocator_v1
    ))]
    material: Option<McapCorrelationMaterialV1>,
}

impl<'a> WebPhysicalReceiptV1<'a> {
    #[cfg(any(
        all(test, not(target_arch = "wasm32")),
        rerun_mcap_phase_a_proof_v1,
        re_mcap_locked_remote_wasm_allocator_v1
    ))]
    pub(crate) fn identity_v1(
        &self,
    ) -> Result<WebPhysicalPendingIdentityV1, WebPhysicalCompletionBindErrorV1> {
        let lease = self
            .lease
            .as_ref()
            .ok_or(WebPhysicalCompletionBindErrorV1::StaleLease)?;
        let (_source_generation, _read_generation, canonical_ordinal, full_range) = lease
            .web_completion_identity_parts_v1()
            .map_err(|_err| WebPhysicalCompletionBindErrorV1::StaleLease)?;
        Ok(WebPhysicalPendingIdentityV1 {
            canonical_ordinal: u32::try_from(canonical_ordinal)
                .map_err(|_overflow| WebPhysicalCompletionBindErrorV1::IdentityOverflow)?,
            full_range_start: full_range.start,
            full_range_end_exclusive: full_range.end,
            budget_profile: self.profile,
        })
    }

    /// Consumes the receipt, validates a body for physical processing, and surfaces the
    /// non-authority MCAP correlation material for the future adapter match step.
    #[cfg(any(
        all(test, not(target_arch = "wasm32")),
        rerun_mcap_phase_a_proof_v1,
        re_mcap_locked_remote_wasm_allocator_v1
    ))]
    pub fn bind_exact_body_v1<'body>(
        mut self,
        body: &'body [u8],
    ) -> Result<
        (
            WebBorrowedPhysicalChunkBodyV1<'a, 'body>,
            McapCorrelationMaterialV1,
        ),
        WebPhysicalCompletionBindErrorV1,
    > {
        let identity = self.identity_v1()?;
        let expected_len = identity
            .full_range_end_exclusive
            .checked_sub(identity.full_range_start)
            .ok_or(WebPhysicalCompletionBindErrorV1::IdentityOverflow)?;
        let actual_len = u64::try_from(body.len())
            .map_err(|_overflow| WebPhysicalCompletionBindErrorV1::IdentityOverflow)?;
        if actual_len != expected_len {
            return Err(WebPhysicalCompletionBindErrorV1::CrossCombination);
        }
        let material = self
            .material
            .take()
            .expect("a live physical receipt retains its correlation material");
        Ok((
            WebBorrowedPhysicalChunkBodyV1 {
                receipt: self,
                body,
                identity,
            },
            material,
        ))
    }

    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(crate) fn bind_borrowed_exact_body_v1<'body>(
        self,
        body: &'body [u8],
        profile: WebPhysicalBudgetProfileV1,
    ) -> Result<WebBorrowedPhysicalChunkBodyV1<'a, 'body>, WebPhysicalCompletionBindErrorV1> {
        if profile != self.profile {
            return Err(WebPhysicalCompletionBindErrorV1::CrossCombination);
        }
        self.bind_exact_body_v1(body).map(|(body, _material)| body)
    }

    #[cfg(any(
        all(test, not(target_arch = "wasm32")),
        rerun_mcap_phase_a_proof_v1,
        re_mcap_locked_remote_wasm_allocator_v1
    ))]
    pub(crate) fn from_lease_v1(
        lease: PhysicalChunkReadLease<'a, PendingHeaderValidation>,
        permit: McapCorrelationPermitV1,
    ) -> Self {
        Self {
            lease: Some(lease),
            profile: WebPhysicalBudgetProfileV1::UnfrozenPhaseACandidate,
            material: Some(permit.into_material_v1()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    rerun_mcap_phase_a_proof_v1,
    re_mcap_locked_remote_wasm_allocator_v1
))]
pub(crate) enum WebPhysicalBudgetProfileV1 {
    UnfrozenPhaseACandidate,
    #[cfg(all(test, not(target_arch = "wasm32")))]
    TestMismatched,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WebPhysicalPendingIdentityV1 {
    canonical_ordinal: u32,
    full_range_start: u64,
    full_range_end_exclusive: u64,
    #[cfg(any(
        all(test, not(target_arch = "wasm32")),
        rerun_mcap_phase_a_proof_v1,
        re_mcap_locked_remote_wasm_allocator_v1
    ))]
    budget_profile: WebPhysicalBudgetProfileV1,
}

#[cfg(all(test, not(target_arch = "wasm32")))]
impl WebPhysicalPendingIdentityV1 {
    pub(crate) const fn canonical_ordinal_v1(self) -> u32 {
        self.canonical_ordinal
    }

    pub(crate) const fn full_range_v1(self) -> (u64, u64) {
        (self.full_range_start, self.full_range_end_exclusive)
    }

    pub(crate) const fn budget_profile_v1(self) -> WebPhysicalBudgetProfileV1 {
        self.budget_profile
    }
}

/// Opaque borrowed processing boundary created only after the pending lease validates exact length
/// and typed budget profile. The transport owner remains in the upper Viewer adapter.
pub struct WebBorrowedPhysicalChunkBodyV1<'a, 'body> {
    receipt: WebPhysicalReceiptV1<'a>,
    body: &'body [u8],
    identity: WebPhysicalPendingIdentityV1,
}

impl<'a> WebBorrowedPhysicalChunkBodyV1<'a, '_> {
    pub fn process_zero_copy_v1(
        self,
        overlap_budget: &WebPhysicalCopyOverlapBudgetV1,
        mut revalidate: impl FnMut(WebPhysicalBodySafePointV1) -> Result<(), ()>,
    ) -> Result<CompletedWebPhysicalBodyHandoffV1<'a>, WebPhysicalBodyHandoffErrorV1> {
        process_zero_copy_body_v1(self.receipt, self.body, overlap_budget, |point| {
            let _identity = self.identity;
            revalidate(point)
        })
    }

    pub fn process_explicit_copy_v1(
        self,
        overlap_budget: &WebPhysicalCopyOverlapBudgetV1,
        mut revalidate: impl FnMut(WebPhysicalBodySafePointV1) -> Result<(), ()>,
    ) -> Result<CompletedWebPhysicalBodyHandoffV1<'a>, WebPhysicalBodyHandoffErrorV1> {
        process_explicit_copy_body_v1(self.receipt, self.body, overlap_budget, |point| {
            let _identity = self.identity;
            revalidate(point)
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebPhysicalCompletionBindErrorV1 {
    StaleLease,
    IdentityOverflow,
    CrossCombination,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebPhysicalBodySafePointV1 {
    BeforeHeaderValidation,
    AfterPayloadCopy,
    AfterPayloadTransfer,
    AfterExactDecompression,
    AfterPhysicalScan,
    BeforeCachePublication,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebPhysicalBodyHandoffErrorV1 {
    RevalidationFailed(WebPhysicalBodySafePointV1),
    HeaderValidationFailed,
    ResourceLimitExceeded,
    PayloadInstallFailed,
    DecompressionFailed,
    PhysicalScanFailed,
    CacheInstallFailed,
}

pub struct WebPhysicalScanCacheEntryV1<'a> {
    _inner: PhysicalChunkScanCacheEntry<'a>,
}

pub struct CompletedWebPhysicalBodyHandoffV1<'a> {
    // Field order is normative: cache bytes and their lease drop before overlap accounting.
    cache: Option<WebPhysicalScanCacheEntryV1<'a>>,
    overlap: Option<WebPhysicalCopyOverlapReservationV1>,
}

impl<'a> CompletedWebPhysicalBodyHandoffV1<'a> {
    /// The unique Web adapter must drop its `ExactLengthRangeBody` before calling this method.
    pub fn into_cache_after_body_drop_v1(mut self) -> WebPhysicalScanCacheEntryV1<'a> {
        let cache = self
            .cache
            .take()
            .expect("a completed body handoff retains its cache");
        drop(self.overlap.take());
        cache
    }
}

impl<'a> WebPhysicalScanCacheEntryV1<'a> {
    #[cfg(any(all(test, not(target_arch = "wasm32")), rerun_mcap_phase_a_proof_v1))]
    pub(crate) fn into_message_evidence_for_phase_a_measurement_v1(
        self,
    ) -> Result<
        crate::remote_chunk_scan::PhysicalChunkMessageEvidenceV1<'a>,
        WebPhysicalBodyHandoffErrorV1,
    > {
        self._inner
            .consumer()
            .into_message_evidence_v1()
            .map_err(|_err| WebPhysicalBodyHandoffErrorV1::PhysicalScanFailed)
    }

    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(crate) fn is_current_for_test_v1(&self) -> bool {
        self._inner.consumer().is_current()
    }
}

pub(crate) fn process_zero_copy_body_v1<'a>(
    mut receipt: WebPhysicalReceiptV1<'a>,
    full_record: &[u8],
    overlap_budget: &WebPhysicalCopyOverlapBudgetV1,
    mut revalidate: impl FnMut(WebPhysicalBodySafePointV1) -> Result<(), ()>,
) -> Result<CompletedWebPhysicalBodyHandoffV1<'a>, WebPhysicalBodyHandoffErrorV1> {
    let checkpoint = |point, revalidate: &mut dyn FnMut(_) -> Result<(), ()>| {
        revalidate(point).map_err(|()| WebPhysicalBodyHandoffErrorV1::RevalidationFailed(point))
    };
    checkpoint(
        WebPhysicalBodySafePointV1::BeforeHeaderValidation,
        &mut revalidate,
    )?;
    let lease = receipt
        .lease
        .take()
        .expect("a pending Web physical read retains its lease");
    let validated =
        crate::remote_chunk_scan::validate_borrowed_physical_chunk_header_v1(lease, full_record)
            .map_err(|_err| WebPhysicalBodyHandoffErrorV1::HeaderValidationFailed)?;
    let full_record_bytes = u64::try_from(full_record.len())
        .map_err(|_overflow| WebPhysicalBodyHandoffErrorV1::ResourceLimitExceeded)?;
    let destination_bytes = validated.uncompressed_size_v1();
    let overlap = overlap_budget.reserve_v1(full_record_bytes, destination_bytes)?;
    let compressed =
        crate::remote_chunk_scan::install_borrowed_header_validated_payload_zero_copy_v1(validated)
            .map_err(|_err| WebPhysicalBodyHandoffErrorV1::PayloadInstallFailed)?;
    checkpoint(
        WebPhysicalBodySafePointV1::AfterPayloadTransfer,
        &mut revalidate,
    )?;
    let decompressed = crate::remote_decompression::decompress_exact_chunk_borrowed(compressed)
        .map_err(|_err| WebPhysicalBodyHandoffErrorV1::DecompressionFailed)?;
    checkpoint(
        WebPhysicalBodySafePointV1::AfterExactDecompression,
        &mut revalidate,
    )?;
    let scan = crate::remote_chunk_scan::scan_decompressed_physical_chunk(decompressed)
        .map_err(|_err| WebPhysicalBodyHandoffErrorV1::PhysicalScanFailed)?;
    checkpoint(
        WebPhysicalBodySafePointV1::AfterPhysicalScan,
        &mut revalidate,
    )?;
    let cache = PhysicalChunkScanCacheEntry::new(scan)
        .map_err(|_err| WebPhysicalBodyHandoffErrorV1::CacheInstallFailed)?;
    checkpoint(
        WebPhysicalBodySafePointV1::BeforeCachePublication,
        &mut revalidate,
    )?;
    Ok(CompletedWebPhysicalBodyHandoffV1 {
        cache: Some(WebPhysicalScanCacheEntryV1 { _inner: cache }),
        overlap: Some(overlap),
    })
}

pub(crate) fn process_explicit_copy_body_v1<'a>(
    mut receipt: WebPhysicalReceiptV1<'a>,
    full_record: &[u8],
    overlap_budget: &WebPhysicalCopyOverlapBudgetV1,
    mut revalidate: impl FnMut(WebPhysicalBodySafePointV1) -> Result<(), ()>,
) -> Result<CompletedWebPhysicalBodyHandoffV1<'a>, WebPhysicalBodyHandoffErrorV1> {
    let checkpoint = |point, revalidate: &mut dyn FnMut(_) -> Result<(), ()>| {
        revalidate(point).map_err(|()| WebPhysicalBodyHandoffErrorV1::RevalidationFailed(point))
    };
    checkpoint(
        WebPhysicalBodySafePointV1::BeforeHeaderValidation,
        &mut revalidate,
    )?;
    let lease = receipt
        .lease
        .take()
        .expect("a pending Web physical read retains its lease");
    let validated =
        crate::remote_chunk_scan::validate_borrowed_physical_chunk_header_v1(lease, full_record)
            .map_err(|_err| WebPhysicalBodyHandoffErrorV1::HeaderValidationFailed)?;
    let full_record_bytes = u64::try_from(full_record.len())
        .map_err(|_overflow| WebPhysicalBodyHandoffErrorV1::ResourceLimitExceeded)?;
    let destination_bytes = u64::try_from(validated.payload_len_v1())
        .map_err(|_overflow| WebPhysicalBodyHandoffErrorV1::ResourceLimitExceeded)?;
    let overlap = overlap_budget.reserve_v1(full_record_bytes, destination_bytes)?;
    let compressed =
        crate::remote_chunk_scan::install_borrowed_header_validated_payload_copy_v1(validated)
            .map_err(|_err| WebPhysicalBodyHandoffErrorV1::PayloadInstallFailed)?;
    checkpoint(
        WebPhysicalBodySafePointV1::AfterPayloadCopy,
        &mut revalidate,
    )?;
    let decompressed = crate::remote_decompression::decompress_exact_chunk(compressed)
        .map_err(|_err| WebPhysicalBodyHandoffErrorV1::DecompressionFailed)?;
    checkpoint(
        WebPhysicalBodySafePointV1::AfterExactDecompression,
        &mut revalidate,
    )?;
    let scan = crate::remote_chunk_scan::scan_decompressed_physical_chunk(decompressed)
        .map_err(|_err| WebPhysicalBodyHandoffErrorV1::PhysicalScanFailed)?;
    checkpoint(
        WebPhysicalBodySafePointV1::AfterPhysicalScan,
        &mut revalidate,
    )?;
    let cache = PhysicalChunkScanCacheEntry::new(scan)
        .map_err(|_err| WebPhysicalBodyHandoffErrorV1::CacheInstallFailed)?;
    checkpoint(
        WebPhysicalBodySafePointV1::BeforeCachePublication,
        &mut revalidate,
    )?;
    Ok(CompletedWebPhysicalBodyHandoffV1 {
        cache: Some(WebPhysicalScanCacheEntryV1 { _inner: cache }),
        overlap: Some(overlap),
    })
}
