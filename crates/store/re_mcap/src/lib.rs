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
//! Raw Summary/schema bytes, scalar policy claims, source generations, and reservations therefore
//! cannot be used by a downstream crate to construct or rebind the sealed capability:
//!
//! ```compile_fail
//! let raw_summary: mcap::Summary = todo!();
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

pub mod decoders;
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

// Neutral compile-time boundary between the ROS 2 initializer and the future protobuf initializer.
// It can own only the opaque post-EOF authority and is not a descendant of either implementation.
#[cfg(any(test, re_mcap_locked_remote_wasm_allocator_v1))]
mod remote_protobuf_projection_boundary;

// Bounded ROS 2 reflection initialization remains crate-private and production-disarmed until the
// remote decoder policy and resource profile are sealed.
#[cfg(any(test, re_mcap_locked_remote_wasm_allocator_v1))]
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
