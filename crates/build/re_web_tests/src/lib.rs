//! Shared support for Rerun's browser-based tests.

pub mod resource_assertions;

#[cfg(not(target_arch = "wasm32"))]
pub mod deterministic_async;
#[cfg(not(target_arch = "wasm32"))]
pub mod mcap_range_server;
