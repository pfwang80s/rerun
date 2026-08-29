//! Dependency-neutral, non-authority correlation material for remote-MCAP Web operations.
//!
//! This crate intentionally does **not** issue producer-authentic receipts and does **not**
//! authorize any Web, MCAP, cache, store, lease, reservation, or operation capability.
//! Its only job is to mint opaque, move-only correlation material and to prove that two
//! material halves came from the same fresh pair.
//!
//! Producer authenticity is the exclusive responsibility of the owning crates
//! (`re_web` for transport and `re_mcap` for physical reads), which must attach their own
//! private-owner authority before any receipt is issued.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Process-wide source of fresh, opaque correlation nonces.
///
/// The nonce is an implementation detail. It is never exposed and never usable as an
/// authority or identity by a downstream caller.
static NEXT_CORRELATION_NONCE: AtomicU64 = AtomicU64::new(1);

/// Opaque correlation marker shared by a single fresh pair and its material halves.
///
/// This is private on purpose: callers cannot forge a marker, project its value, or compare
/// two markers outside the crate. It is shared via [`Arc`] so each holder owns a real
/// allocation rather than a `Copy` scalar.
#[derive(PartialEq)]
struct CorrelationMarker(Arc<u64>);

/// Creates fresh, opaque correlation material.
///
/// The factory has no associated authority. Any caller may create an unused opaque pair,
/// but doing so grants no Web, MCAP, cache, store, lease, reservation, or operation
/// capability.
pub struct CorrelationFactoryV1;

/// Opaque correlation anchor owned by the operation owner.
///
/// The pair is move-only and has no public projection or split capability. Its private
/// marker field exists solely to make the pair non-constructible by downstream callers and
/// to keep each fresh issue distinct; the marker is intentionally not readable.
pub struct CorrelationPairV1 {
    _marker: CorrelationMarker,
}

/// One-shot correlation input reserved for the Web transport producer.
///
/// The permit is move-only and non-authority. The owning transport crate consumes it
/// together with its authentic private owner when issuing its own receipt.
pub struct WebCorrelationPermitV1 {
    marker: CorrelationMarker,
}

/// One-shot correlation input reserved for the MCAP physical producer.
///
/// The permit is move-only and non-authority. The owning physical crate consumes it
/// together with its authentic private owner when issuing its own receipt.
pub struct McapCorrelationPermitV1 {
    marker: CorrelationMarker,
}

/// Opaque correlation half produced by consuming the Web permit.
///
/// This material is non-authority: it proves pair membership only, never operation
/// permission. It has no public constructor or projection.
pub struct WebCorrelationMaterialV1 {
    marker: CorrelationMarker,
}

/// Opaque correlation half produced by consuming the MCAP permit.
///
/// This material is non-authority: it proves pair membership only, never operation
/// permission. It has no public constructor or projection.
pub struct McapCorrelationMaterialV1 {
    marker: CorrelationMarker,
}

/// Proof that two material halves came from the same fresh pair.
///
/// This result is non-authority and cannot independently authorize a Web, MCAP, cache,
/// store, lease, reservation, or operation action. It is move-only and has no public
/// projection or construction path: the private marker field binds it to the matched pair
/// and prevents downstream callers from forging a match result.
pub struct MatchedCorrelationV1 {
    _marker: CorrelationMarker,
}

/// Error returned when two material halves do not come from the same fresh pair.
///
/// The error is intentionally opaque: it exposes no nonce, pair identity, or reason detail.
/// Downstream layers must treat a mismatch as a fail-closed rejection.
#[non_exhaustive]
pub struct CorrelationMismatchV1;

impl CorrelationFactoryV1 {
    /// Creates a fresh correlation pair and one move-only permit per producer.
    ///
    /// The returned material has no authority and cannot be used to construct a producer
    /// receipt. Permits are consumed exactly once by the owning producers.
    #[must_use]
    pub fn new_operation_v1() -> (
        CorrelationPairV1,
        WebCorrelationPermitV1,
        McapCorrelationPermitV1,
    ) {
        let nonce = NEXT_CORRELATION_NONCE.fetch_add(1, Ordering::Relaxed);
        let marker = CorrelationMarker(Arc::new(nonce));

        (
            CorrelationPairV1 {
                _marker: CorrelationMarker(Arc::clone(&marker.0)),
            },
            WebCorrelationPermitV1 {
                marker: CorrelationMarker(Arc::clone(&marker.0)),
            },
            McapCorrelationPermitV1 { marker },
        )
    }
}

impl WebCorrelationPermitV1 {
    /// Consumes this permit and returns the opaque Web correlation material half.
    ///
    /// The resulting material is non-authority and is used only by the owning producer and
    /// the adapter for pair matching.
    #[must_use]
    pub fn into_material_v1(self) -> WebCorrelationMaterialV1 {
        WebCorrelationMaterialV1 {
            marker: self.marker,
        }
    }
}

impl McapCorrelationPermitV1 {
    /// Consumes this permit and returns the opaque MCAP correlation material half.
    ///
    /// The resulting material is non-authority and is used only by the owning producer and
    /// the adapter for pair matching.
    #[must_use]
    pub fn into_material_v1(self) -> McapCorrelationMaterialV1 {
        McapCorrelationMaterialV1 {
            marker: self.marker,
        }
    }
}

/// Consumes two material halves and proves they came from the same fresh pair.
///
/// A mismatch, replay, or duplicate consumption is rejected because the halves are
/// move-only and consumed exactly once. The returned [`MatchedCorrelationV1`] is
/// non-authority and must never be used by itself to authorize an operation.
pub fn match_correlation_v1(
    web: WebCorrelationMaterialV1,
    mcap: McapCorrelationMaterialV1,
) -> Result<MatchedCorrelationV1, CorrelationMismatchV1> {
    let WebCorrelationMaterialV1 { marker: web_marker } = web;
    let McapCorrelationMaterialV1 {
        marker: mcap_marker,
    } = mcap;

    if web_marker == mcap_marker {
        Ok(MatchedCorrelationV1 {
            _marker: web_marker,
        })
    } else {
        Err(CorrelationMismatchV1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    assert_impl_all!(CorrelationPairV1: Send, Sync);
    assert_impl_all!(WebCorrelationPermitV1: Send, Sync);
    assert_impl_all!(McapCorrelationPermitV1: Send, Sync);
    assert_impl_all!(WebCorrelationMaterialV1: Send, Sync);
    assert_impl_all!(McapCorrelationMaterialV1: Send, Sync);
    assert_impl_all!(MatchedCorrelationV1: Send, Sync);
    assert_impl_all!(CorrelationMismatchV1: Send, Sync);

    assert_not_impl_any!(CorrelationPairV1: Clone, Copy, core::fmt::Debug, std::hash::Hash);
    assert_not_impl_any!(WebCorrelationPermitV1: Clone, Copy, core::fmt::Debug, std::hash::Hash);
    assert_not_impl_any!(McapCorrelationPermitV1: Clone, Copy, core::fmt::Debug, std::hash::Hash);
    assert_not_impl_any!(WebCorrelationMaterialV1: Clone, Copy, core::fmt::Debug, std::hash::Hash);
    assert_not_impl_any!(McapCorrelationMaterialV1: Clone, Copy, core::fmt::Debug, std::hash::Hash);
    assert_not_impl_any!(MatchedCorrelationV1: Clone, Copy, core::fmt::Debug, std::hash::Hash);
    assert_not_impl_any!(CorrelationMismatchV1: Clone, Copy, core::fmt::Debug, std::hash::Hash);

    #[test]
    fn factory_creates_fresh_move_only_material() {
        let (_pair, web, mcap) = CorrelationFactoryV1::new_operation_v1();
        let web_material = web.into_material_v1();
        let mcap_material = mcap.into_material_v1();

        let matched = match_correlation_v1(web_material, mcap_material);
        assert!(matched.is_ok());
    }

    #[test]
    fn pair_is_an_anchor_without_split_capability() {
        // The pair must not provide a second split path. Its presence in the factory tuple
        // is an operation anchor; the one-shot permits are issued by the factory itself.
        let (_pair, web, mcap) = CorrelationFactoryV1::new_operation_v1();
        let web_material = web.into_material_v1();
        let mcap_material = mcap.into_material_v1();
        assert!(match_correlation_v1(web_material, mcap_material).is_ok());
    }

    #[test]
    fn mismatched_material_is_rejected() {
        let (_pair_a, web_a, _mcap_a) = CorrelationFactoryV1::new_operation_v1();
        let (_pair_b, _web_b, mcap_b) = CorrelationFactoryV1::new_operation_v1();

        let web_material = web_a.into_material_v1();
        let mcap_material = mcap_b.into_material_v1();
        assert!(matches!(
            match_correlation_v1(web_material, mcap_material),
            Err(CorrelationMismatchV1)
        ));
    }

    #[test]
    fn each_factory_call_is_fresh() {
        let (_pair_a, web_a, mcap_a) = CorrelationFactoryV1::new_operation_v1();
        let (_pair_b, web_b, mcap_b) = CorrelationFactoryV1::new_operation_v1();

        assert!(matches!(
            match_correlation_v1(web_a.into_material_v1(), mcap_b.into_material_v1()),
            Err(CorrelationMismatchV1)
        ));
        assert!(matches!(
            match_correlation_v1(web_b.into_material_v1(), mcap_a.into_material_v1()),
            Err(CorrelationMismatchV1)
        ));
    }

    #[test]
    fn material_has_no_public_issuance_or_scalar_projection() {
        let source = include_str!("lib.rs");

        // Build needles from fragments so the test source does not itself contain
        // the exact forbidden signatures it is scanning for.
        let pub_fn = ["pub", "fn"].join(" ");
        let forbidden = [
            format!("{pub_fn} issue"),
            format!("{pub_fn} from_"),
            format!("{pub_fn} as_"),
            format!("{pub_fn} identity"),
            format!("{pub_fn} token"),
        ];

        for needle in forbidden {
            assert!(
                !source.contains(&needle),
                "forbidden source pattern present: {needle}"
            );
        }

        assert!(!source.contains(&format!("{}de", "ser")));
        assert!(!source.contains(&format!("{}erialize", "Ser")));
        assert!(!source.contains(&format!("{}erialize", "Des")));
    }
}
