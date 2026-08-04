//! Shared support for Rerun's browser-based tests.

#![cfg(not(target_arch = "wasm32"))]

pub mod deterministic_async;
pub mod mcap_range_server;
