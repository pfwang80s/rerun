//! Production-disarmed Chrome correctness probes for the controlled MCAP Range fixture.
//!
//! This crate intentionally does not install or exercise real remote-MCAP open, Store, query, or
//! playback capability. It only verifies the bounded browser network and URL-classification
//! boundaries required before MCAP-114 or a later real-capability release gate.
