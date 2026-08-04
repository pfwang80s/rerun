//! Test-only support for constructing adversarial MCAP files.
//!
//! This module is compiled only by this crate's tests or when the explicit `testing` feature is
//! enabled.
//!
//! `AdversarialMcapFixtureBuilder` owns only deterministic MCAP bytes and the decoder, cardinality,
//! and partition inputs that are inseparable from interpreting those bytes.
//!
//! Browser HTTP, Range, CORS, BYOB, validator, and extensionless-probe behavior belongs to the
//! controlled origin and network fixtures in MCAP-003 and MCAP-013 through MCAP-016.
//!
//! Async ownership, frame allowances, page lifecycle, public lifecycle dispatch, startup handoff,
//! and compatibility ingress ordering belong to MCAP-004 and their implementation work items.
//!
//! Strict and compatibility open registries, URL identity, semantic reuse, legacy import and apply,
//! `LogChannel` transfer/backpressure, Redap restart/publication, recording-use state, memory pressure,
//! and client terminal registries belong to the later route-specific harnesses in M4 through M9.
//!
//! `OutputFullBeforeCodecEof` and `ZeroProgress` are explicit fake-codec scheduling inputs because no
//! static byte string can force an injected decoder implementation to return either state.

mod adversarial_fixture;

pub use adversarial_fixture::*;
