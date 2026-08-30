//! `phase_a` producer: the real cross-layer composition of a transport receipt and a
//! physical receipt.
//!
//! This module is compiled only in the `phase_a`/`locked` wasm32 profile. It owns only the
//! composition step: it consumes the transport receipt in a synchronous non-escaping
//! callback, inside the callback binds and processes the physical receipt, surfaces both
//! non-authority correlation materials, matches them, drops the transport body before
//! extracting the cache, and returns the owned opaque result.
//!
//! It performs no scalar identity comparison; source/read-generation/ordinal/profile/body/
//! live-slot revalidation stays in the owning producers (`re_web` and `re_mcap`).

use re_mcap_web_contract::match_correlation_v1;

use crate::consumer_contract::AdapterOperationErrorV1;

/// Move-only, cross-layer operation handle owned by the adapter.
pub struct AdapterOperationV1<'a> {
    transport: re_web::transport_receipt::RemoteTransportReceiptV1<
        re_web::chrome_byob::ExactLengthRangeBody,
    >,
    physical: re_mcap::web_body_handoff::WebPhysicalReceiptV1<'a>,
    overlap_budget: re_mcap::web_body_handoff::WebPhysicalCopyOverlapBudgetV1,
}

/// Move-only opaque terminal result carrying the owned physical scan cache.
///
/// This shadows the neutral `consumer_contract::AdapterOperationResultV1` with a
/// lifetime-carrying form that holds the real cache produced by the composition. The cache is
/// extracted by the Phase-A consumer via [`AdapterOperationResultV1::into_cache_v1`].
pub struct AdapterOperationResultV1<'a> {
    cache: re_mcap::web_body_handoff::WebPhysicalScanCacheEntryV1<'a>,
}

impl<'a> AdapterOperationResultV1<'a> {
    /// Extracts the owned physical scan cache from a completed operation.
    #[must_use]
    pub fn into_cache_v1(self) -> re_mcap::web_body_handoff::WebPhysicalScanCacheEntryV1<'a> {
        self.cache
    }
}

/// Initiates a real cross-layer operation from two producer-issued receipts.
///
/// The Viewer has already minted the correlation pair, run the strict fetch, and called the
/// public issue seams to obtain the two receipts. The adapter only composes them. No URL,
/// range, ordinal, scalar identity, lease, cache, or reservation is accepted or exposed.
#[must_use]
pub fn initiate_operation_v1<'a>(
    transport: re_web::transport_receipt::RemoteTransportReceiptV1<
        re_web::chrome_byob::ExactLengthRangeBody,
    >,
    physical: re_mcap::web_body_handoff::WebPhysicalReceiptV1<'a>,
) -> AdapterOperationV1<'a> {
    AdapterOperationV1 {
        transport,
        physical,
        overlap_budget:
            re_mcap::web_body_handoff::WebPhysicalCopyOverlapBudgetV1::new_unfrozen_phase_a_v1(),
    }
}

impl<'a> AdapterOperationV1<'a> {
    /// Runs the composition exactly once, consuming the operation.
    ///
    /// The transport receipt is consumed in a synchronous `FnOnce` callback. Inside the
    /// callback the physical receipt is bound and processed zero-copy, surfacing the MCAP
    /// correlation material; the Web correlation material is returned by the consume step.
    /// The two materials are matched, the transport body is dropped before the cache is
    /// extracted, and the owned opaque result is returned.
    ///
    /// The operation-level cancellation decision is made by the Viewer scheduler before this
    /// atomic synchronous run; the owning producers still revalidate their own
    /// source/read-generation/ordinal/profile/body/live-slot authority at issue, bind, and
    /// every safe point.
    pub fn run_v1(self) -> Result<AdapterOperationResultV1<'a>, AdapterOperationErrorV1> {
        let Self {
            transport,
            physical,
            overlap_budget,
        } = self;

        let (completed, web_material) = transport
            .consume_transport_v1(|body| {
                let (borrowed, mcap_material) = physical
                    .bind_exact_body_v1(body)
                    .map_err(|_err| AdapterOperationErrorV1::physical_bind_v1())?;
                let completed = borrowed
                    .process_zero_copy_v1(&overlap_budget, |_safe_point| Ok(()))
                    .map_err(|_err| AdapterOperationErrorV1::physical_handoff_v1())?;
                Ok::<_, AdapterOperationErrorV1>((completed, mcap_material))
            })
            .map_err(|err| err)?;

        let (completed, mcap_material) = completed;
        let _matched = match_correlation_v1(web_material, mcap_material)
            .map_err(|_err| AdapterOperationErrorV1::correlation_mismatch_v1())?;

        let cache = completed.into_cache_after_body_drop_v1();
        Ok(AdapterOperationResultV1 { cache })
    }
}
