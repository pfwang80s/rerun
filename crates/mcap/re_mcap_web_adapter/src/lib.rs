//! Sole trusted cross-layer producer for Web remote-MCAP operations.
//!
//! This crate is the only layer allowed to depend on both `re_web` and `re_mcap`. The
//! ordinary product Viewer builds only the `consumer_contract` profile, which exports opaque
//! move-only operation handles with no transport, physical, or scalar-identity surface.
//!
//! The `phase_a`/`locked` wasm32 profile additionally exports the real cross-layer
//! composition producer. Production capability remains disarmed: the disarmed consumer
//! contract never executes a real operation.

#[cfg(feature = "consumer_contract")]
pub mod consumer_contract;

#[cfg(all(feature = "phase_a", target_arch = "wasm32"))]
pub mod phase_a;
