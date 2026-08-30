//! Opaque consumer contract for the Web remote-MCAP adapter.
//!
//! This module exports only move-only, private-field operation handles. It has no `re_web`,
//! `re_mcap`, HTTP, URL/query, `Response`, body, lease, cache, reservation, or scalar-identity
//! dependency in the neutral `consumer_contract` profile, and exposes no
//! `Clone`/`Copy`/`Debug`/`Hash`/serialization on the handles.

/// Move-only opaque operation handle issued by the adapter.
///
/// The ordinary disarmed profile never produces a runnable operation. When the
/// `phase_a`/`locked` producer profile is compiled, the producer constructs a real handle in
/// a separate module path (`crate::phase_a::AdapterOperationV1`); this neutral handle exposes
/// no identity, body, lease, cache, reservation, or scalar.
pub struct AdapterOperationV1 {
    inner: AdapterOperationInnerV1,
}

enum AdapterOperationInnerV1 {
    /// Production-disarmed: no producer is compiled, so the operation cannot run.
    Disarmed,
}

/// Move-only opaque terminal result returned by a successful operation.
///
/// In the neutral disarmed profile this is a type-only marker (never constructed). The
/// `phase_a`/`locked` wasm32 module shadows this name with its own lifetime-carrying
/// `AdapterOperationResultV1<'a>` that holds the real physical scan cache.
pub struct AdapterOperationResultV1 {
    _private: (),
}

/// Opaque operation status observable by the consumer scheduler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdapterOperationStatusV1 {
    /// The adapter is production-disarmed and will never run a real operation.
    Disarmed,
}

/// Opaque operation failure.
///
/// The error is `#[non_exhaustive]` with a private field, so downstream consumers cannot
/// construct it and cannot observe any body, URL, `ETag`, identity, or scalar detail. The
/// `phase_a` producer maps physical bind/handoff/correlation failures into this single
/// opaque marker; the detail is intentionally redacted.
#[non_exhaustive]
pub struct AdapterOperationErrorV1 {
    _private: (),
}

impl AdapterOperationErrorV1 {
    /// Crate-internal construction for the `phase_a` producer and the disarmed path.
    pub(crate) fn new_v1() -> Self {
        Self { _private: () }
    }

    #[cfg(all(feature = "phase_a", target_arch = "wasm32"))]
    pub(crate) fn correlation_mismatch_v1() -> Self {
        Self::new_v1()
    }

    #[cfg(all(feature = "phase_a", target_arch = "wasm32"))]
    pub(crate) fn physical_bind_v1() -> Self {
        Self::new_v1()
    }

    #[cfg(all(feature = "phase_a", target_arch = "wasm32"))]
    pub(crate) fn physical_handoff_v1() -> Self {
        Self::new_v1()
    }
}

/// Initiates an opaque operation.
///
/// In the disarmed profile this returns a non-runnable handle; the `phase_a`/`locked`
/// producer profiles provide the real producer-side initiation in `crate::phase_a`.
#[must_use]
pub fn initiate_operation_v1() -> AdapterOperationV1 {
    AdapterOperationV1 {
        inner: AdapterOperationInnerV1::Disarmed,
    }
}

impl AdapterOperationV1 {
    /// Runs the operation exactly once, consuming it.
    ///
    /// In the disarmed profile this always fails closed; a real producer is only compiled in
    /// the `phase_a`/`locked` wasm32 profiles.
    pub fn run_v1(self) -> Result<AdapterOperationResultV1, AdapterOperationErrorV1> {
        match self.inner {
            AdapterOperationInnerV1::Disarmed => Err(AdapterOperationErrorV1::new_v1()),
        }
    }

    /// Observes the current operation status without consuming it.
    pub fn status_v1(&self) -> AdapterOperationStatusV1 {
        match self.inner {
            AdapterOperationInnerV1::Disarmed => AdapterOperationStatusV1::Disarmed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use static_assertions::assert_not_impl_any;

    assert_not_impl_any!(AdapterOperationV1: Clone, Copy, core::fmt::Debug, std::hash::Hash);
    assert_not_impl_any!(AdapterOperationResultV1: Clone, Copy, core::fmt::Debug, std::hash::Hash);
    assert_not_impl_any!(AdapterOperationErrorV1: Clone, Copy, core::fmt::Debug, std::hash::Hash);

    #[test]
    fn disarmed_operation_fails_closed_and_reports_disarmed_status() {
        let operation = initiate_operation_v1();
        assert_eq!(operation.status_v1(), AdapterOperationStatusV1::Disarmed);
        assert!(matches!(
            operation.run_v1(),
            Err(AdapterOperationErrorV1 { .. })
        ));
    }
}
