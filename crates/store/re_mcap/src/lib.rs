#![allow(clippy::iter_over_hash_type)]

//! Library providing utilities to load MCAP files with Rerun.
//!
//! Remote raw-time canonicalization is intentionally absent from the native production API:
//!
//! ```compile_fail,ignore-wasm32
//! use re_mcap::remote_time;
//! ```
//!
//! Remote fixed-layout validation is likewise absent from the native production API:
//!
//! ```compile_fail,ignore-wasm32
//! use re_mcap::remote_fixed_layout;
//! ```
//!
//! Remote physical handoff contracts are absent from the native production API:
//!
//! ```compile_fail,ignore-wasm32
//! use re_mcap::remote_physical_contract::RemotePhysicalOperationV1;
//! ```
//!
//! On Web targets, downstream callers still cannot construct operations or results from scalar
//! identities or empty values:
//!
//! ```compile_fail
//! use re_mcap::remote_physical_contract::RemotePhysicalOperationV1;
//! let _ = RemotePhysicalOperationV1::from_scalars(1, 2, 3, 0..4);
//! ```
//!
//! ```compile_fail
//! use re_mcap::remote_physical_contract::RemotePhysicalOperationResultV1;
//! let _ = RemotePhysicalOperationResultV1 {};
//! ```
//!
//! Remote Summary preparation remains sealed inside `re_mcap` on every target:
//!
//! ```compile_fail
//! use re_mcap::remote_summary;
//! ```
//!
//! Exact-output remote Chunk decompression also remains sealed inside `re_mcap`:
//!
//! ```compile_fail
//! use re_mcap::remote_decompression;
//! ```
//!
//! Bounded remote ROS 2 initialization, its raw-schema entry points, and its source/policy
//! evidence remain sealed as well:
//!
//! ```compile_fail
//! use re_mcap::remote_ros2_reflection;
//! ```
//!
//! The neutral protobuf projection EOF boundary is crate-private too:
//!
//! ```compile_fail
//! use re_mcap::remote_protobuf_projection_boundary;
//! ```
//!
//! The combined bounded protobuf graph and its source-bound recognition rows cannot be named,
//! copied, or rebound by downstream crates:
//!
//! ```compile_fail
//! use re_mcap::remote_protobuf_descriptor;
//! ```
//!
//! The source-bound remote decoder assignment result and its reservation are sealed too:
//!
//! ```compile_fail
//! use re_mcap::remote_decoder_assignment;
//! ```
//!
//! Immutable remote Channel groups, partition keys, and source-row identities are sealed too:
//!
//! ```compile_fail
//! use re_mcap::remote_channel_group;
//! ```
//!
//! Raw Summary/schema bytes, scalar policy claims, source generations, and reservations therefore
//! cannot be used by a downstream crate to construct or rebind the sealed capability:
//!
//! ```compile_fail
//! let raw_summary: mcap::Summary = todo!("compile-fail raw summary placeholder");
//! let raw_schema: &[u8] = b"int32 value";
//! let policy_claim = true;
//! let generation = 1_u64;
//! re_mcap::remote_ros2_reflection::initialize(
//!     raw_summary,
//!     raw_schema,
//!     policy_claim,
//!     generation,
//! );
//! ```

/// Every MCAP record is framed by a fixed header containing a one-byte opcode followed by an
/// eight-byte little-endian `u64` body length.
const RECORD_HEADER_LEN: usize = 1 + std::mem::size_of::<u64>();

#[cfg(any(target_arch = "wasm32", test))]
pub mod web_body_handoff;

#[cfg(any(test, all(target_arch = "wasm32", rerun_mcap_phase_a_proof_v1)))]
pub use remote_physical_resolution::phase_a_measurement;

#[cfg(target_arch = "wasm32")]
pub mod remote_physical_contract;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod remote_physical_contract;

pub mod decoders;
#[cfg(test)]
mod remote_physical_core;

mod error;
mod file;
mod info;
mod recover;

/// Checked raw-time types for the Web remote-MCAP path.
#[cfg(target_arch = "wasm32")]
pub mod remote_time;

// Host builds compile this Web-only module only for its unit tests.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod remote_time;

/// Checked fixed-layout and checksum validation for Web remote-MCAP inputs.
#[cfg(target_arch = "wasm32")]
pub mod remote_fixed_layout;

// Host builds compile this Web-only module only for its unit tests.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod remote_fixed_layout;

// The first remote Summary parsing stage remains crate-private and production-disarmed.
#[cfg(any(target_arch = "wasm32", test))]
mod remote_summary;

// Exact-output decompression remains crate-private and production-disarmed until the remote
// resource profile and physical Chunk scanner are sealed.
#[cfg(any(target_arch = "wasm32", test))]
mod remote_decompression;

// Physical Chunk authority, header validation, and semantic scanning remain crate-private and
// production-disarmed until the Chrome body-owner adapter and remote resource profile are sealed.
#[cfg(any(target_arch = "wasm32", test))]
mod remote_chunk_scan;

// Exactly-once MCAP-023/025 ownership coordinator. The public Wasm surface exposes only opaque,
// sealed owner types needed by the future Viewer adapter; constructors remain artifact-gated.
#[cfg(target_arch = "wasm32")]
pub mod remote_physical_resolution;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod remote_physical_resolution;

// Neutral compile-time boundary between the ROS 2 initializer and the future protobuf initializer.
// It can own only the opaque post-EOF authority and is not a descendant of either implementation.
#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    re_mcap_locked_remote_wasm_allocator_v1
))]
mod remote_protobuf_projection_boundary;

// Bounded protobuf descriptor initialization is a sibling of the neutral post-EOF boundary.
// The boundary exposes only opaque operations, so the initializer can retain, but never detach,
// the exact ROS/source/policy authority established by MCAP-026.
#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    re_mcap_locked_remote_wasm_allocator_v1
))]
mod remote_protobuf_descriptor;

// Deterministic assignment consumes only the sealed combined initializer owner and remains
// production-disarmed until the immutable manifest and executable decoder contracts are sealed.
#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    re_mcap_locked_remote_wasm_allocator_v1
))]
mod remote_decoder_assignment;

#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    re_mcap_locked_remote_wasm_allocator_v1
))]
mod remote_deterministic_insertion;

// Immutable Channel groups consume the complete assignment owner and remain production-disarmed
// until the full manifest and remote resource profile are sealed.
#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    re_mcap_locked_remote_wasm_allocator_v1
))]
mod remote_channel_group;

// Exact validation/count plans consume a physical scan and immutable Channel-group owner while
// remaining production-disarmed until decoder resource admission is implemented.
#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    re_mcap_locked_remote_wasm_allocator_v1
))]
mod remote_chunk_validation_count;

// Immutable manifest and partition/root identity authority. Kept disarmed until the Web
// production adapter is enabled; native and ordinary local MCAP paths never compile this module.
#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    re_mcap_locked_remote_wasm_allocator_v1
))]
mod remote_manifest;

#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    re_mcap_locked_remote_wasm_allocator_v1
))]
mod remote_loaded_coverage;

// Partition/root registration consumes only sealed manifest identities and the capability-gated
// Web remote-MCAP Store origin API. It remains absent from native production builds.
#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    re_mcap_locked_remote_wasm_allocator_v1
))]
mod remote_partition_residency;

// Bounded three-layer window planner and production-disarmed demand ownership. It remains
// independent of metadata-opening retry owners and is absent from native/local MCAP builds.
#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    re_mcap_locked_remote_wasm_allocator_v1
))]
mod remote_window_demand;

// Requested/committed navigation state and candidate-clock hold semantics. It remains a pure,
// production-disarmed `re_mcap` state layer and owns no Viewer command routing or Store mutation.
#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    re_mcap_locked_remote_wasm_allocator_v1
))]
mod remote_navigation;

// Supersedable/CommitLocked seek ownership, stale-result classification, and the three explicit
// failure paths for Web remote-MCAP navigation. It remains production-disarmed and owns no Viewer,
// network transport, or Store mutation.
#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    re_mcap_locked_remote_wasm_allocator_v1
))]
mod remote_seek;

// Generation/job-driven staging and Store insertion consume the bounded window-demand planner,
// atomic registration, and deterministic insertion contract. It remains absent from native/local
// MCAP builds and owns no HTTP client or retry transport.
#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    re_mcap_locked_remote_wasm_allocator_v1
))]
mod remote_partition_job;

#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    re_mcap_locked_remote_wasm_allocator_v1
))]
mod remote_predecessor_backfill;

#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    re_mcap_locked_remote_wasm_allocator_v1
))]
mod remote_root_reload;

// Admitted second-pass dispatch remains crate-private and production-disarmed until the Web
// Store mutation arbiter is wired to the complete terminal contract.
#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    re_mcap_locked_remote_wasm_allocator_v1
))]
mod remote_chunk_dispatch;

// Sealed typed output contracts bridge bounded initializer state to the remote Chunk builder.
// They remain absent from native/local MCAP APIs and ordinary Viewer code.
#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    re_mcap_locked_remote_wasm_allocator_v1
))]
mod remote_typed_output;

// Summary and Chunk decoding must redeem this MCAP-012 proof before domain construction.
#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    re_mcap_locked_remote_wasm_allocator_v1
))]
mod remote_runtime_intern;

// Bounded ROS 2 reflection initialization remains crate-private and production-disarmed until the
// remote decoder policy and resource profile are sealed.
#[cfg(any(
    all(test, not(target_arch = "wasm32")),
    re_mcap_locked_remote_wasm_allocator_v1
))]
mod remote_ros2_reflection;

pub(crate) mod parsers;
pub(crate) mod util;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use decoders::{
    Decoder, DecoderIdentifier, DecoderRegistry, MessageDecoder, SelectedDecoders, TopicFilter,
};

pub use error::Error;
pub use file::McapFile;
pub use info::{
    McapChannelInfo, McapChunkInfo, McapCompressionInfo, McapInfo, McapSchemaInfo,
    McapSummarySource,
};
pub use mcap::Summary;
pub use parsers::ros2msg::sensor_msgs::{
    ImageEncoding, decode_image_encoding, decode_image_format,
};
pub use parsers::{MessageParser, ParserContext, cdr};
pub use recover::{ScanResult, build_chunk_index, read_or_reconstruct_summary};
pub use util::read_summary;
