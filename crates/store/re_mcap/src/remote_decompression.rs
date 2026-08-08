//! Exact-output decompression for one Web remote-MCAP physical Chunk.
//!
//! This module deliberately does not use the general MCAP or LZ4 frame readers: both are allowed
//! to grow internal output containers or continue into another frame.
//! The only output allocation here has the exact declared uncompressed length.

// This module is intentionally production-disarmed until the remote resource profile is sealed
// and the physical Chunk scanner consumes `ExactOutputChunk` directly.
#![allow(dead_code)]

use std::sync::Arc;

use parking_lot::Mutex;

use crate::remote_chunk_scan::{HeaderValidated, PhysicalChunkReadLease};
use crate::remote_fixed_layout::OptionalCrcValidation;

const OVERFLOW_DETECTION_SCRATCH_BYTES: u64 = 1;
const ZSTD_WINDOW_LOG_MIN: u32 = 10;
const ZSTD_WINDOW_LOG_MAX_32: u32 = 30;
const ZSTD_WINDOW_LOG_MAX_64: u32 = 31;
const LZ4_FRAME_MAGIC: u32 = 0x184D_2204;
const LZ4_LEGACY_FRAME_MAGIC: u32 = 0x184C_2102;
const LZ4_SKIPPABLE_FRAME_MAGIC_START: u32 = 0x184D_2A50;
const LZ4_SKIPPABLE_FRAME_MAGIC_END: u32 = 0x184D_2A5F;
const LZ4_FRAME_VERSION: u8 = 0b01 << 6;
const LZ4_FRAME_VERSION_MASK: u8 = 0b11 << 6;
const LZ4_BLOCK_INDEPENDENCE: u8 = 1 << 5;
const LZ4_BLOCK_CHECKSUM: u8 = 1 << 4;
const LZ4_CONTENT_SIZE: u8 = 1 << 3;
const LZ4_CONTENT_CHECKSUM: u8 = 1 << 2;
const LZ4_FLAG_RESERVED: u8 = 1 << 1;
const LZ4_DICTIONARY_ID: u8 = 1;
const LZ4_BLOCK_UNCOMPRESSED: u32 = 1 << 31;
const LZ4_DICTIONARY_BYTES: usize = 64 * 1024;

/// Fixed limits for one non-preemptible decompression work unit.
///
/// There is deliberately no production constructor while the Web remote profile is disarmed.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ChunkDecompressionLimits {
    max_compressed_bytes: u64,
    max_uncompressed_bytes: u64,
    max_decompression_ratio: u64,
    max_zstd_window_log: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ChunkDecompressionBudgetUsage {
    active_inputs: u64,
    retained_input_bytes: u64,
    active_work_units: u64,
    overflow_scratch_bytes: u64,
    zstd_working_bytes: u64,
    active_outputs: u64,
    retained_output_bytes: u64,
}

#[derive(Clone, Copy, Debug)]
struct ChunkDecompressionBudgetCapacity {
    max_active_inputs: u64,
    max_retained_input_bytes: u64,
    max_active_work_units: u64,
    max_overflow_scratch_bytes: u64,
    max_zstd_working_bytes: u64,
    max_active_outputs: u64,
    max_retained_output_bytes: u64,
}

struct ChunkDecompressionBudgetState {
    limits: ChunkDecompressionLimits,
    capacity: ChunkDecompressionBudgetCapacity,
    usage: Mutex<ChunkDecompressionBudgetUsage>,
}

/// One source/profile identity for compressed input, decoder work, and retained exact output.
///
/// Every ownership transition keeps the same `Arc` state, so a caller cannot splice an admitted
/// input into another source's decoder or output capacity.
pub(crate) struct ChunkDecompressionBudget {
    state: Arc<ChunkDecompressionBudgetState>,
}

impl std::fmt::Debug for ChunkDecompressionBudget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ChunkDecompressionBudget")
            .field("limits", &self.state.limits)
            .field("capacity", &self.state.capacity)
            .finish_non_exhaustive()
    }
}

impl ChunkDecompressionBudgetState {
    fn try_reserve_input(
        self: &Arc<Self>,
        bytes: u64,
    ) -> Result<CompressedChunkInputReservation, ChunkDecompressionError> {
        let mut usage = self.usage.lock();
        let next_active_inputs = usage
            .active_inputs
            .checked_add(1)
            .ok_or(ChunkDecompressionError::ReservationArithmeticOverflow)?;
        let next_retained_input_bytes = usage
            .retained_input_bytes
            .checked_add(bytes)
            .ok_or(ChunkDecompressionError::ReservationArithmeticOverflow)?;
        if next_active_inputs > self.capacity.max_active_inputs
            || next_retained_input_bytes > self.capacity.max_retained_input_bytes
        {
            return Err(ChunkDecompressionError::CompressedInputReservationLimitExceeded);
        }
        usage.active_inputs = next_active_inputs;
        usage.retained_input_bytes = next_retained_input_bytes;
        drop(usage);
        Ok(CompressedChunkInputReservation {
            state: Arc::clone(self),
            bytes,
        })
    }

    fn try_reserve(
        self: &Arc<Self>,
        compressed_bytes: u64,
        declared_uncompressed_bytes: u64,
        zstd_working_bytes: u64,
    ) -> Result<ChunkDecompressionWorkReservation, ChunkDecompressionError> {
        if compressed_bytes > self.limits.max_compressed_bytes {
            return Err(ChunkDecompressionError::CompressedBytesLimitExceeded);
        }
        if declared_uncompressed_bytes > self.limits.max_uncompressed_bytes {
            return Err(ChunkDecompressionError::UncompressedBytesLimitExceeded);
        }
        if !within_ratio(
            compressed_bytes,
            declared_uncompressed_bytes,
            self.limits.max_decompression_ratio,
        ) {
            return Err(ChunkDecompressionError::DecompressionRatioLimitExceeded);
        }

        // This checked sum is part of admission even though the scratch itself lives on the stack.
        // It proves that output plus the only permitted overflow probe fit the selected profile.
        declared_uncompressed_bytes
            .checked_add(OVERFLOW_DETECTION_SCRATCH_BYTES)
            .ok_or(ChunkDecompressionError::ReservationArithmeticOverflow)?;

        let mut usage = self.usage.lock();
        let next = ChunkDecompressionBudgetUsage {
            active_inputs: usage.active_inputs,
            retained_input_bytes: usage.retained_input_bytes,
            active_work_units: usage
                .active_work_units
                .checked_add(1)
                .ok_or(ChunkDecompressionError::ReservationArithmeticOverflow)?,
            overflow_scratch_bytes: usage
                .overflow_scratch_bytes
                .checked_add(OVERFLOW_DETECTION_SCRATCH_BYTES)
                .ok_or(ChunkDecompressionError::ReservationArithmeticOverflow)?,
            zstd_working_bytes: usage
                .zstd_working_bytes
                .checked_add(zstd_working_bytes)
                .ok_or(ChunkDecompressionError::ReservationArithmeticOverflow)?,
            active_outputs: usage
                .active_outputs
                .checked_add(1)
                .ok_or(ChunkDecompressionError::ReservationArithmeticOverflow)?,
            retained_output_bytes: usage
                .retained_output_bytes
                .checked_add(declared_uncompressed_bytes)
                .ok_or(ChunkDecompressionError::ReservationArithmeticOverflow)?,
        };
        if next.zstd_working_bytes > self.capacity.max_zstd_working_bytes {
            return Err(ChunkDecompressionError::ZstdWorkingReservationLimitExceeded);
        }
        if next.active_work_units > self.capacity.max_active_work_units
            || next.overflow_scratch_bytes > self.capacity.max_overflow_scratch_bytes
            || next.active_outputs > self.capacity.max_active_outputs
            || next.retained_output_bytes > self.capacity.max_retained_output_bytes
        {
            return Err(ChunkDecompressionError::ReservationLimitExceeded);
        }
        *usage = next;
        drop(usage);

        Ok(ChunkDecompressionWorkReservation {
            state: Some(Arc::clone(self)),
            output_bytes: declared_uncompressed_bytes,
            zstd_working_bytes,
        })
    }
}

/// Move-only ownership of one output reservation and its active one-byte scratch claim.
struct ChunkDecompressionWorkReservation {
    state: Option<Arc<ChunkDecompressionBudgetState>>,
    output_bytes: u64,
    zstd_working_bytes: u64,
}

impl ChunkDecompressionWorkReservation {
    fn complete(mut self) -> ChunkDecompressionOutputReservation {
        let state = self
            .state
            .take()
            .expect("live decompression work has one budget owner");
        {
            let mut usage = state.usage.lock();
            usage.active_work_units = usage
                .active_work_units
                .checked_sub(1)
                .expect("live decompression work owns one active work-unit claim");
            usage.overflow_scratch_bytes = usage
                .overflow_scratch_bytes
                .checked_sub(OVERFLOW_DETECTION_SCRATCH_BYTES)
                .expect("live decompression work owns its one-byte scratch claim");
            usage.zstd_working_bytes = usage
                .zstd_working_bytes
                .checked_sub(self.zstd_working_bytes)
                .expect("live decompression work owns its zstd working-byte claim");
        }
        ChunkDecompressionOutputReservation {
            state,
            output_bytes: self.output_bytes,
        }
    }
}

impl Drop for ChunkDecompressionWorkReservation {
    fn drop(&mut self) {
        let Some(state) = self.state.take() else {
            return;
        };
        let mut usage = state.usage.lock();
        usage.active_work_units = usage
            .active_work_units
            .checked_sub(1)
            .expect("live decompression work owns one active work-unit claim");
        usage.overflow_scratch_bytes = usage
            .overflow_scratch_bytes
            .checked_sub(OVERFLOW_DETECTION_SCRATCH_BYTES)
            .expect("live decompression work owns its one-byte scratch claim");
        usage.zstd_working_bytes = usage
            .zstd_working_bytes
            .checked_sub(self.zstd_working_bytes)
            .expect("live decompression work owns its zstd working-byte claim");
        usage.active_outputs = usage
            .active_outputs
            .checked_sub(1)
            .expect("live decompression work owns one pending output claim");
        usage.retained_output_bytes = usage
            .retained_output_bytes
            .checked_sub(self.output_bytes)
            .expect("live decompression work owns its pending output bytes");
    }
}

/// Move-only ownership of one retained exact output allocation.
struct ChunkDecompressionOutputReservation {
    state: Arc<ChunkDecompressionBudgetState>,
    output_bytes: u64,
}

impl Drop for ChunkDecompressionOutputReservation {
    fn drop(&mut self) {
        let mut usage = self.state.usage.lock();
        usage.active_outputs = usage
            .active_outputs
            .checked_sub(1)
            .expect("live decompressed output owns one output claim");
        usage.retained_output_bytes = usage
            .retained_output_bytes
            .checked_sub(self.output_bytes)
            .expect("live decompressed output owns its exact output bytes");
    }
}

/// Move-only identity established by the sealed upstream physical-Range handoff.
///
/// There is deliberately no production constructor while that adapter remains disarmed.
/// The identity cannot be copied, cloned, compared from a caller-supplied scalar, or rebound to
/// another payload after it enters this module.
pub(crate) enum PhysicalChunkReadIdentity<'a> {
    Lease(PhysicalChunkReadLease<'a, HeaderValidated>),
    #[cfg(test)]
    CodecUnit {
        opaque: u128,
    },
}

impl std::fmt::Debug for PhysicalChunkReadIdentity<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PhysicalChunkReadIdentity")
            .field("identity", &"<opaque move-only owner>")
            .finish_non_exhaustive()
    }
}

impl PhysicalChunkReadIdentity<'_> {
    fn ensure_current(&self) -> Result<(), ChunkDecompressionError> {
        match self {
            Self::Lease(lease) => lease
                .ensure_current()
                .map_err(|_error| ChunkDecompressionError::PhysicalReadNotCurrent),
            #[cfg(test)]
            Self::CodecUnit { .. } => Ok(()),
        }
    }
}

#[cfg(test)]
impl PhysicalChunkReadIdentity<'_> {
    fn codec_test_opaque(&self) -> u128 {
        match self {
            Self::CodecUnit { opaque } => *opaque,
            Self::Lease(_) => panic!("the codec-unit assertion received a physical read lease"),
        }
    }
}

/// Move-only ownership of one exact compressed backing claim.
struct CompressedChunkInputReservation {
    state: Arc<ChunkDecompressionBudgetState>,
    bytes: u64,
}

impl Drop for CompressedChunkInputReservation {
    fn drop(&mut self) {
        let mut usage = self.state.usage.lock();
        usage.active_inputs = usage
            .active_inputs
            .checked_sub(1)
            .expect("live compressed Chunk input owns one active input claim");
        usage.retained_input_bytes = usage
            .retained_input_bytes
            .checked_sub(self.bytes)
            .expect("live compressed Chunk input owns its exact byte claim");
    }
}

/// A sealed upstream physical-read handoff acquired before an exact Range backing is installed.
pub(super) struct PreparedExactCompressedChunkInput<'a> {
    // Keep the reservation before the lease-owning identity: Rust drops fields in declaration
    // order, so abandoning a prepared handoff releases accounting before its physical read claim.
    reservation: CompressedChunkInputReservation,
    identity: PhysicalChunkReadIdentity<'a>,
    codec: ChunkCompressionCodec,
    expected_compressed_bytes: u64,
    declared_uncompressed_size: u64,
    declared_uncompressed_crc: u32,
}

impl std::fmt::Debug for PreparedExactCompressedChunkInput<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedExactCompressedChunkInput")
            .field("identity", &"<opaque>")
            .field("codec", &self.codec)
            .field("expected_compressed_bytes", &self.expected_compressed_bytes)
            .field(
                "declared_uncompressed_size",
                &self.declared_uncompressed_size,
            )
            .field("declared_uncompressed_crc", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl<'a> PreparedExactCompressedChunkInput<'a> {
    /// Installs the only accepted production-shaped backing: an exact-sized `Box<[u8]>`.
    pub(super) fn install(
        self,
        compressed_payload: Box<[u8]>,
    ) -> Result<ExactCompressedChunkInput<'a>, ChunkDecompressionError> {
        self.identity.ensure_current()?;
        let actual_bytes = u64::try_from(compressed_payload.len())
            .map_err(|_overflow| ChunkDecompressionError::CompressedBytesLimitExceeded)?;
        if actual_bytes != self.expected_compressed_bytes {
            // The backing must be released before the source-scoped reservation in `self`.
            drop(compressed_payload);
            return Err(ChunkDecompressionError::UnexpectedCompressedInputLength);
        }
        Ok(ExactCompressedChunkInput {
            compressed_payload: Some(compressed_payload),
            reservation: Some(self.reservation),
            identity: Some(self.identity),
            codec: self.codec,
            declared_uncompressed_size: self.declared_uncompressed_size,
            declared_uncompressed_crc: self.declared_uncompressed_crc,
        })
    }
}

pub(super) fn prepare_header_validated_compressed_chunk_input(
    lease: PhysicalChunkReadLease<'_, HeaderValidated>,
    codec: ChunkCompressionCodec,
    expected_compressed_bytes: u64,
    declared_uncompressed_size: u64,
    declared_uncompressed_crc: u32,
) -> Result<PreparedExactCompressedChunkInput<'_>, ChunkDecompressionError> {
    let reservation = lease
        .decompression_budget()
        .state
        .try_reserve_input(expected_compressed_bytes)?;
    Ok(PreparedExactCompressedChunkInput {
        identity: PhysicalChunkReadIdentity::Lease(lease),
        codec,
        expected_compressed_bytes,
        declared_uncompressed_size,
        declared_uncompressed_crc,
        reservation,
    })
}

// This is test-only until the exact physical-Range adapter can move its own sealed response owner
// into this module. Keeping the builder private prevents crate siblings from asserting metadata or
// rebinding an arbitrary same-length backing to a trusted identity.
#[cfg(test)]
fn prepare_exact_compressed_chunk_input(
    compression: &str,
    expected_compressed_bytes: u64,
    declared_uncompressed_size: u64,
    declared_uncompressed_crc: u32,
    budget: &ChunkDecompressionBudget,
) -> Result<PreparedExactCompressedChunkInput<'static>, ChunkDecompressionError> {
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_PHYSICAL_READ_IDENTITY: AtomicU64 = AtomicU64::new(1);
    let identity = PhysicalChunkReadIdentity::CodecUnit {
        opaque: u128::from(NEXT_TEST_PHYSICAL_READ_IDENTITY.fetch_add(1, Ordering::Relaxed)),
    };
    Ok(PreparedExactCompressedChunkInput {
        identity,
        codec: ChunkCompressionCodec::classify_mcap_name(compression),
        expected_compressed_bytes,
        declared_uncompressed_size,
        declared_uncompressed_crc,
        reservation: budget.state.try_reserve_input(expected_compressed_bytes)?,
    })
}

/// Exact compressed payload coupled to its source-scoped byte reservation.
pub(crate) struct ExactCompressedChunkInput<'a> {
    // `Drop` releases this backing before its permit on every failure path.
    compressed_payload: Option<Box<[u8]>>,
    reservation: Option<CompressedChunkInputReservation>,
    identity: Option<PhysicalChunkReadIdentity<'a>>,
    codec: ChunkCompressionCodec,
    declared_uncompressed_size: u64,
    declared_uncompressed_crc: u32,
}

impl std::fmt::Debug for ExactCompressedChunkInput<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExactCompressedChunkInput")
            .field("identity", &"<opaque>")
            .field("codec", &self.codec)
            .field("compressed_payload", &"<owned exact bytes and permit>")
            .field(
                "declared_uncompressed_size",
                &self.declared_uncompressed_size,
            )
            .field("declared_uncompressed_crc", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl<'a> ExactCompressedChunkInput<'a> {
    fn ensure_current(&self) -> Result<(), ChunkDecompressionError> {
        self.identity
            .as_ref()
            .expect("live exact compressed input retains its evidence identity")
            .ensure_current()
    }

    fn compressed_payload(&self) -> &[u8] {
        self.compressed_payload
            .as_deref()
            .expect("live exact compressed input retains its backing")
    }

    fn budget_state(&self) -> Arc<ChunkDecompressionBudgetState> {
        Arc::clone(
            &self
                .reservation
                .as_ref()
                .expect("live exact compressed input retains its source profile")
                .state,
        )
    }

    fn take_identity_after_releasing_compressed(mut self) -> PhysicalChunkReadIdentity<'a> {
        drop(self.compressed_payload.take());
        drop(self.reservation.take());
        self.identity
            .take()
            .expect("live exact compressed input retains its evidence identity")
    }

    fn transition_none_to_output(mut self) -> (PhysicalChunkReadIdentity<'a>, Box<[u8]>) {
        let output = self
            .compressed_payload
            .take()
            .expect("live raw exact input retains its backing");
        // The already-acquired decompressed-output reservation now covers this same backing.
        drop(self.reservation.take());
        let identity = self
            .identity
            .take()
            .expect("live raw exact input retains its evidence identity");
        (identity, output)
    }
}

impl Drop for ExactCompressedChunkInput<'_> {
    fn drop(&mut self) {
        drop(self.compressed_payload.take());
        drop(self.reservation.take());
    }
}

/// The release-Wasm codec allowlist frozen into an exact output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChunkCompressionCodec {
    None,
    Zstd,
    Lz4,
    Unsupported,
}

impl ChunkCompressionCodec {
    pub(super) fn from_mcap_name(name: &str) -> Result<Self, ChunkDecompressionError> {
        match Self::classify_mcap_name(name) {
            Self::Unsupported => Err(ChunkDecompressionError::UnsupportedCompression),
            codec => Ok(codec),
        }
    }

    fn classify_mcap_name(name: &str) -> Self {
        match name {
            "" => Self::None,
            "zstd" => Self::Zstd,
            "lz4" => Self::Lz4,
            _ => Self::Unsupported,
        }
    }
}

/// CRC evidence retained with one exact decompressed buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ChunkCrcEvidence {
    declared: u32,
    validation: OptionalCrcValidation,
}

/// Sealed, move-only bytes that the physical Chunk scanner must consume.
///
/// The owner retains the exact upstream identity, codec, CRC evidence, and output reservation.
pub(crate) struct ExactOutputChunk<'a> {
    // These owners are optional solely to make the security-relevant release order explicit in
    // `Drop`: backing bytes, then accounting, then the physical read lease.
    bytes: Option<Box<[u8]>>,
    reservation: Option<ChunkDecompressionOutputReservation>,
    identity: Option<PhysicalChunkReadIdentity<'a>>,
    codec: ChunkCompressionCodec,
    crc: ChunkCrcEvidence,
}

impl std::fmt::Debug for ExactOutputChunk<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExactOutputChunk")
            .field("identity", &"<opaque>")
            .field("codec", &self.codec)
            .field("crc", &self.crc.validation)
            .field("bytes", &"<sealed exact bytes>")
            .finish_non_exhaustive()
    }
}

impl<'a> ExactOutputChunk<'a> {
    pub(crate) fn identity(&self) -> &PhysicalChunkReadIdentity<'a> {
        self.identity
            .as_ref()
            .expect("live exact output retains its physical read identity")
    }

    pub(crate) fn codec(&self) -> ChunkCompressionCodec {
        self.codec
    }

    pub(crate) fn crc(&self) -> ChunkCrcEvidence {
        self.crc
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        self.bytes
            .as_deref()
            .expect("live exact output retains its backing")
    }

    pub(crate) fn has_matching_lease_profile(&self) -> bool {
        let PhysicalChunkReadIdentity::Lease(lease) = self.identity() else {
            return false;
        };
        Arc::ptr_eq(
            &lease.decompression_budget().state,
            &self
                .reservation
                .as_ref()
                .expect("live exact output retains its output reservation")
                .state,
        )
    }

    pub(crate) fn crc_validation(&self) -> OptionalCrcValidation {
        self.crc.validation
    }
}

impl Drop for ExactOutputChunk<'_> {
    fn drop(&mut self) {
        drop(self.bytes.take());
        drop(self.reservation.take());
        drop(self.identity.take());
    }
}

/// A decompression failure that is safe to surface without source bytes or sizes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChunkDecompressionError {
    PhysicalReadNotCurrent,
    UnsupportedCompression,
    DeclaredOutputSizeUnsupported,
    CompressedBytesLimitExceeded,
    UncompressedBytesLimitExceeded,
    DecompressionRatioLimitExceeded,
    ReservationArithmeticOverflow,
    ReservationLimitExceeded,
    CompressedInputReservationLimitExceeded,
    UnexpectedCompressedInputLength,
    InvalidZstdWindowProfile,
    ZstdWorkingEstimateFailed,
    ZstdWorkingReservationLimitExceeded,
    InvalidCompressedData,
    CompressedFrameTruncated,
    CodecZeroProgress,
    CodecContractViolation,
    OutputTooShort,
    OutputTooLong,
    Lz4BlockOutputLimitExceeded,
    TrailingCompressedInput,
    ChunkChecksumMismatch,
}

impl std::fmt::Display for ChunkDecompressionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::PhysicalReadNotCurrent => "remote MCAP physical Chunk read is no longer current",
            Self::UnsupportedCompression => "remote MCAP Chunk compression is unsupported",
            Self::DeclaredOutputSizeUnsupported => {
                "remote MCAP Chunk output size is unsupported on this target"
            }
            Self::CompressedBytesLimitExceeded => {
                "remote MCAP Chunk compressed bytes exceed their limit"
            }
            Self::UncompressedBytesLimitExceeded => {
                "remote MCAP Chunk uncompressed bytes exceed their limit"
            }
            Self::DecompressionRatioLimitExceeded => {
                "remote MCAP Chunk decompression ratio exceeds its limit"
            }
            Self::ReservationArithmeticOverflow => {
                "remote MCAP Chunk decompression reservation overflowed"
            }
            Self::ReservationLimitExceeded => {
                "remote MCAP Chunk decompression reservation exceeds its capacity"
            }
            Self::CompressedInputReservationLimitExceeded => {
                "remote MCAP compressed input reservation exceeds its capacity"
            }
            Self::UnexpectedCompressedInputLength => {
                "remote MCAP compressed input does not match its prepared length"
            }
            Self::InvalidZstdWindowProfile => "remote MCAP zstd window profile is invalid",
            Self::ZstdWorkingEstimateFailed => "remote MCAP zstd working-memory estimate failed",
            Self::ZstdWorkingReservationLimitExceeded => {
                "remote MCAP zstd working memory exceeds its reservation capacity"
            }
            Self::InvalidCompressedData => "remote MCAP Chunk compressed data is invalid",
            Self::CompressedFrameTruncated => "remote MCAP Chunk compressed frame is truncated",
            Self::CodecZeroProgress => "remote MCAP Chunk codec made no progress",
            Self::CodecContractViolation => "remote MCAP Chunk codec violated its bounded contract",
            Self::OutputTooShort => "remote MCAP Chunk output is shorter than declared",
            Self::OutputTooLong => "remote MCAP Chunk output is longer than declared",
            Self::Lz4BlockOutputLimitExceeded => {
                "remote MCAP LZ4 block output exceeds its declared block limit"
            }
            Self::TrailingCompressedInput => {
                "remote MCAP Chunk has another frame or trailing compressed input"
            }
            Self::ChunkChecksumMismatch => "remote MCAP Chunk checksum does not match",
        })
    }
}

impl std::error::Error for ChunkDecompressionError {}

/// Decompresses one exact physical payload without publishing or scanning any record.
pub(crate) fn decompress_exact_chunk(
    input: ExactCompressedChunkInput<'_>,
) -> Result<ExactOutputChunk<'_>, ChunkDecompressionError> {
    input.ensure_current()?;
    let budget_state = input.budget_state();
    // Unsupported codecs fail before any size reservation or output allocation.
    let codec = match input.codec {
        ChunkCompressionCodec::Unsupported => {
            return Err(ChunkDecompressionError::UnsupportedCompression);
        }
        codec => codec,
    };
    let compressed_bytes = u64::try_from(input.compressed_payload().len())
        .map_err(|_overflow| ChunkDecompressionError::CompressedBytesLimitExceeded)?;
    let output_len = usize::try_from(input.declared_uncompressed_size)
        .map_err(|_overflow| ChunkDecompressionError::DeclaredOutputSizeUnsupported)?;
    let zstd_working_bytes = match codec {
        ChunkCompressionCodec::Zstd => {
            estimate_zstd_working_bytes(budget_state.limits.max_zstd_window_log)?
        }
        ChunkCompressionCodec::None | ChunkCompressionCodec::Lz4 => 0,
        ChunkCompressionCodec::Unsupported => unreachable!("unsupported codecs fail above"),
    };
    let reservation = budget_state.try_reserve(
        compressed_bytes,
        input.declared_uncompressed_size,
        zstd_working_bytes,
    )?;
    let declared_uncompressed_crc = input.declared_uncompressed_crc;

    let (identity, output) = match codec {
        ChunkCompressionCodec::None => {
            if input.compressed_payload().len() < output_len {
                return Err(ChunkDecompressionError::OutputTooShort);
            }
            if input.compressed_payload().len() > output_len {
                return Err(ChunkDecompressionError::OutputTooLong);
            }
            input.transition_none_to_output()
        }
        ChunkCompressionCodec::Zstd => {
            let mut output = allocate_exact_output(output_len);
            decompress_zstd_single_frame(
                input.compressed_payload(),
                &mut output,
                budget_state.limits.max_zstd_window_log,
            )?;
            (input.take_identity_after_releasing_compressed(), output)
        }
        ChunkCompressionCodec::Lz4 => {
            let mut output = allocate_exact_output(output_len);
            decompress_lz4_single_frame(input.compressed_payload(), &mut output)?;
            (input.take_identity_after_releasing_compressed(), output)
        }
        ChunkCompressionCodec::Unsupported => unreachable!("unsupported codecs fail above"),
    };

    let validation = if declared_uncompressed_crc == 0 {
        OptionalCrcValidation::NotProvided
    } else if crc32fast::hash(&output) == declared_uncompressed_crc {
        OptionalCrcValidation::Verified
    } else {
        drop(output);
        drop(reservation);
        drop(identity);
        return Err(ChunkDecompressionError::ChunkChecksumMismatch);
    };

    if let Err(error) = identity.ensure_current() {
        drop(output);
        drop(reservation);
        drop(identity);
        return Err(error);
    }

    Ok(ExactOutputChunk {
        bytes: Some(output),
        reservation: Some(reservation.complete()),
        identity: Some(identity),
        codec,
        crc: ChunkCrcEvidence {
            declared: declared_uncompressed_crc,
            validation,
        },
    })
}

/// Safe, allocation-free wrapper around zstd 1.5.7's documented `DStream` estimate.
///
/// The `zstd = 0.13.3` lock resolves to `zstd-safe = 7.2.4` and
/// `zstd-sys = 2.0.16+zstd.1.5.7` in `Cargo.lock`.
/// `ZSTD_estimateDStreamSize` includes the `DCtx`, input buffer, and window/output ring buffer for
/// the maximum admitted window, and this path never loads a dictionary.
#[expect(
    unsafe_code,
    reason = "zstd-safe does not wrap this scalar-only documented estimate"
)]
fn estimate_zstd_working_bytes(max_window_log: u32) -> Result<u64, ChunkDecompressionError> {
    let target_max = if usize::BITS == 32 {
        ZSTD_WINDOW_LOG_MAX_32
    } else {
        ZSTD_WINDOW_LOG_MAX_64
    };
    if !(ZSTD_WINDOW_LOG_MIN..=target_max).contains(&max_window_log) {
        return Err(ChunkDecompressionError::InvalidZstdWindowProfile);
    }
    let window_bytes = 1_usize
        .checked_shl(max_window_log)
        .ok_or(ChunkDecompressionError::InvalidZstdWindowProfile)?;
    // SAFETY: this documented estimate accepts one scalar, does not dereference caller memory,
    // and performs no allocation. The version-locked result is checked as a zstd error code.
    let estimate = unsafe { zstd::zstd_safe::zstd_sys::ZSTD_estimateDStreamSize(window_bytes) };
    // SAFETY: `ZSTD_isError` only classifies the scalar returned by the estimate above.
    if unsafe { zstd::zstd_safe::zstd_sys::ZSTD_isError(estimate) } != 0 {
        return Err(ChunkDecompressionError::ZstdWorkingEstimateFailed);
    }
    u64::try_from(estimate).map_err(|_overflow| ChunkDecompressionError::ZstdWorkingEstimateFailed)
}

fn within_ratio(compressed_bytes: u64, uncompressed_bytes: u64, max_ratio: u64) -> bool {
    if uncompressed_bytes == 0 {
        return true;
    }
    if compressed_bytes == 0 || max_ratio == 0 {
        return false;
    }
    compressed_bytes
        .checked_mul(max_ratio)
        .is_none_or(|maximum| uncompressed_bytes <= maximum)
}

fn allocate_exact_output(length: usize) -> Box<[u8]> {
    // `vec![value; length]` performs its one fixed-size allocation up front; no codec can grow it.
    vec![0_u8; length].into_boxed_slice()
}

#[derive(Clone, Copy, Debug)]
struct StreamingCodecStep {
    input_consumed: usize,
    output_written: usize,
    frame_finished: bool,
}

fn drive_exact_streaming_codec(
    input: &[u8],
    output: &mut [u8],
    mut step: impl FnMut(&[u8], &mut [u8]) -> Result<StreamingCodecStep, ChunkDecompressionError>,
) -> Result<(), ChunkDecompressionError> {
    let mut input_position = 0_usize;
    let mut output_position = 0_usize;
    let mut overflow_scratch = [0_u8; OVERFLOW_DETECTION_SCRATCH_BYTES as usize];

    loop {
        let input_remaining = input
            .get(input_position..)
            .ok_or(ChunkDecompressionError::CodecContractViolation)?;
        let probing_overflow = output_position == output.len();
        let output_remaining = if probing_overflow {
            &mut overflow_scratch[..]
        } else {
            output
                .get_mut(output_position..)
                .ok_or(ChunkDecompressionError::CodecContractViolation)?
        };
        let status = step(input_remaining, output_remaining)?;
        if status.input_consumed > input_remaining.len()
            || status.output_written > output_remaining.len()
        {
            return Err(ChunkDecompressionError::CodecContractViolation);
        }

        input_position = input_position
            .checked_add(status.input_consumed)
            .ok_or(ChunkDecompressionError::CodecContractViolation)?;
        if probing_overflow {
            if status.output_written != 0 {
                return Err(ChunkDecompressionError::OutputTooLong);
            }
        } else {
            output_position = output_position
                .checked_add(status.output_written)
                .ok_or(ChunkDecompressionError::CodecContractViolation)?;
        }

        if status.frame_finished {
            if input_position != input.len() {
                return Err(ChunkDecompressionError::TrailingCompressedInput);
            }
            if output_position != output.len() {
                return Err(ChunkDecompressionError::OutputTooShort);
            }
            return Ok(());
        }
        if status.input_consumed == 0 && status.output_written == 0 {
            return if input_position == input.len() {
                Err(ChunkDecompressionError::CompressedFrameTruncated)
            } else {
                Err(ChunkDecompressionError::CodecZeroProgress)
            };
        }
        if input_position == input.len() {
            return Err(ChunkDecompressionError::CompressedFrameTruncated);
        }
    }
}

fn decompress_zstd_single_frame(
    input: &[u8],
    output: &mut [u8],
    max_window_log: u32,
) -> Result<(), ChunkDecompressionError> {
    use zstd::stream::raw::Operation as _;

    let mut decoder = zstd::stream::raw::Decoder::new()
        .map_err(|_error| ChunkDecompressionError::InvalidCompressedData)?;
    decoder
        .set_parameter(zstd::stream::raw::DParameter::WindowLogMax(max_window_log))
        .map_err(|_error| ChunkDecompressionError::InvalidCompressedData)?;
    drive_exact_streaming_codec(input, output, |input, output| {
        let status = decoder
            .run_on_buffers(input, output)
            .map_err(|_error| ChunkDecompressionError::InvalidCompressedData)?;
        Ok(StreamingCodecStep {
            input_consumed: status.bytes_read,
            output_written: status.bytes_written,
            frame_finished: status.remaining == 0,
        })
    })
}

#[derive(Clone, Copy, Debug)]
struct Lz4FrameDescriptor {
    block_independent: bool,
    block_checksum: bool,
    content_checksum: bool,
    content_size: Option<u64>,
    max_block_size: usize,
}

fn decompress_lz4_single_frame(
    input: &[u8],
    output: &mut [u8],
) -> Result<(), ChunkDecompressionError> {
    let mut cursor = 0_usize;
    let descriptor = parse_lz4_frame_descriptor(input, &mut cursor)?;
    let output_len = u64::try_from(output.len())
        .map_err(|_overflow| ChunkDecompressionError::DeclaredOutputSizeUnsupported)?;
    if descriptor
        .content_size
        .is_some_and(|content_size| content_size != output_len)
    {
        return Err(
            if descriptor.content_size.unwrap_or_default() < output_len {
                ChunkDecompressionError::OutputTooShort
            } else {
                ChunkDecompressionError::OutputTooLong
            },
        );
    }

    let mut output_position = 0_usize;
    let mut overflow_scratch = [0_u8; OVERFLOW_DETECTION_SCRATCH_BYTES as usize];
    loop {
        let block_header = read_lz4_u32(input, &mut cursor)?;
        if block_header == 0 {
            break;
        }
        let uncompressed = block_header & LZ4_BLOCK_UNCOMPRESSED != 0;
        let block_len = usize::try_from(block_header & !LZ4_BLOCK_UNCOMPRESSED)
            .map_err(|_overflow| ChunkDecompressionError::InvalidCompressedData)?;
        if block_len == 0 || block_len > descriptor.max_block_size {
            return Err(ChunkDecompressionError::InvalidCompressedData);
        }
        let block = take_lz4_bytes(input, &mut cursor, block_len)?;

        let written = if uncompressed {
            if block.len() > output.len().saturating_sub(output_position) {
                overflow_scratch[0] = block[0];
                return Err(classify_lz4_extra_output(&overflow_scratch));
            }
            let end = output_position
                .checked_add(block.len())
                .ok_or(ChunkDecompressionError::CodecContractViolation)?;
            output[output_position..end].copy_from_slice(block);
            block.len()
        } else if output_position == output.len() {
            let written = decompress_lz4_block(
                block,
                &mut overflow_scratch,
                output,
                descriptor.block_independent,
            )
            .map_err(|error| match error {
                Lz4BlockDecodeError::OutputTooSmall => ChunkDecompressionError::OutputTooLong,
                Lz4BlockDecodeError::Invalid => ChunkDecompressionError::InvalidCompressedData,
            })?;
            if written != 0 {
                return Err(ChunkDecompressionError::OutputTooLong);
            }
            0
        } else {
            let remaining_output = output.len() - output_position;
            let destination_limit = remaining_output.min(descriptor.max_block_size);
            let (prefix, remaining) = output.split_at_mut(output_position);
            let destination = &mut remaining[..destination_limit];
            decompress_lz4_block(block, destination, prefix, descriptor.block_independent).map_err(
                |error| match error {
                    Lz4BlockDecodeError::OutputTooSmall
                        if remaining_output >= descriptor.max_block_size =>
                    {
                        ChunkDecompressionError::Lz4BlockOutputLimitExceeded
                    }
                    Lz4BlockDecodeError::OutputTooSmall => ChunkDecompressionError::OutputTooLong,
                    Lz4BlockDecodeError::Invalid => ChunkDecompressionError::InvalidCompressedData,
                },
            )?
        };
        output_position = output_position
            .checked_add(written)
            .ok_or(ChunkDecompressionError::CodecContractViolation)?;

        if descriptor.block_checksum {
            let declared = read_lz4_u32(input, &mut cursor)?;
            if xxhash_rust::xxh32::xxh32(block, 0) != declared {
                return Err(ChunkDecompressionError::InvalidCompressedData);
            }
        }
    }

    if descriptor.content_checksum {
        let declared = read_lz4_u32(input, &mut cursor)?;
        if xxhash_rust::xxh32::xxh32(output, 0) != declared {
            return Err(ChunkDecompressionError::InvalidCompressedData);
        }
    }
    if cursor != input.len() {
        return Err(ChunkDecompressionError::TrailingCompressedInput);
    }
    if output_position != output.len() {
        return Err(ChunkDecompressionError::OutputTooShort);
    }
    Ok(())
}

fn classify_lz4_extra_output(scratch: &[u8; 1]) -> ChunkDecompressionError {
    let _first_extra_output_byte = scratch[0];
    ChunkDecompressionError::OutputTooLong
}

fn parse_lz4_frame_descriptor(
    input: &[u8],
    cursor: &mut usize,
) -> Result<Lz4FrameDescriptor, ChunkDecompressionError> {
    let magic = read_lz4_u32(input, cursor)?;
    if magic == LZ4_LEGACY_FRAME_MAGIC
        || (LZ4_SKIPPABLE_FRAME_MAGIC_START..=LZ4_SKIPPABLE_FRAME_MAGIC_END).contains(&magic)
        || magic != LZ4_FRAME_MAGIC
    {
        return Err(ChunkDecompressionError::InvalidCompressedData);
    }

    let descriptor_start = *cursor;
    let flags = take_lz4_bytes(input, cursor, 1)?[0];
    let block_descriptor = take_lz4_bytes(input, cursor, 1)?[0];
    if flags & LZ4_FRAME_VERSION_MASK != LZ4_FRAME_VERSION
        || flags & LZ4_FLAG_RESERVED != 0
        || block_descriptor & 0b1000_1111 != 0
        || flags & LZ4_DICTIONARY_ID != 0
    {
        return Err(ChunkDecompressionError::InvalidCompressedData);
    }
    let max_block_size = match (block_descriptor >> 4) & 0b111 {
        4 => 64 * 1024,
        5 => 256 * 1024,
        6 => 1024 * 1024,
        7 => 4 * 1024 * 1024,
        _ => return Err(ChunkDecompressionError::InvalidCompressedData),
    };
    let content_size = if flags & LZ4_CONTENT_SIZE != 0 {
        Some(read_lz4_u64(input, cursor)?)
    } else {
        None
    };
    let descriptor_end = *cursor;
    let declared_header_checksum = take_lz4_bytes(input, cursor, 1)?[0];
    let descriptor_bytes = input
        .get(descriptor_start..descriptor_end)
        .ok_or(ChunkDecompressionError::CodecContractViolation)?;
    let actual_header_checksum = (xxhash_rust::xxh32::xxh32(descriptor_bytes, 0) >> 8) as u8;
    if declared_header_checksum != actual_header_checksum {
        return Err(ChunkDecompressionError::InvalidCompressedData);
    }

    Ok(Lz4FrameDescriptor {
        block_independent: flags & LZ4_BLOCK_INDEPENDENCE != 0,
        block_checksum: flags & LZ4_BLOCK_CHECKSUM != 0,
        content_checksum: flags & LZ4_CONTENT_CHECKSUM != 0,
        content_size,
        max_block_size,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lz4BlockDecodeError {
    OutputTooSmall,
    Invalid,
}

fn decompress_lz4_block(
    input: &[u8],
    output: &mut [u8],
    previous_output: &[u8],
    block_independent: bool,
) -> Result<usize, Lz4BlockDecodeError> {
    let result = if block_independent {
        lz4_flex::block::decompress_into(input, output)
    } else {
        let dictionary_start = previous_output.len().saturating_sub(LZ4_DICTIONARY_BYTES);
        lz4_flex::block::decompress_into_with_dict(
            input,
            output,
            &previous_output[dictionary_start..],
        )
    };
    result.map_err(|error| match error {
        lz4_flex::block::DecompressError::OutputTooSmall { .. } => {
            Lz4BlockDecodeError::OutputTooSmall
        }
        _ => Lz4BlockDecodeError::Invalid,
    })
}

fn take_lz4_bytes<'a>(
    input: &'a [u8],
    cursor: &mut usize,
    length: usize,
) -> Result<&'a [u8], ChunkDecompressionError> {
    let end = cursor
        .checked_add(length)
        .ok_or(ChunkDecompressionError::InvalidCompressedData)?;
    let bytes = input
        .get(*cursor..end)
        .ok_or(ChunkDecompressionError::CompressedFrameTruncated)?;
    *cursor = end;
    Ok(bytes)
}

fn read_lz4_u32(input: &[u8], cursor: &mut usize) -> Result<u32, ChunkDecompressionError> {
    let bytes = take_lz4_bytes(input, cursor, size_of::<u32>())?;
    Ok(u32::from_le_bytes(bytes.try_into().map_err(|_error| {
        ChunkDecompressionError::CodecContractViolation
    })?))
}

fn read_lz4_u64(input: &[u8], cursor: &mut usize) -> Result<u64, ChunkDecompressionError> {
    let bytes = take_lz4_bytes(input, cursor, size_of::<u64>())?;
    Ok(u64::from_le_bytes(bytes.try_into().map_err(|_error| {
        ChunkDecompressionError::CodecContractViolation
    })?))
}

#[cfg(test)]
pub(crate) fn chunk_decompression_budget_for_test() -> ChunkDecompressionBudget {
    let max_zstd_working_bytes =
        estimate_zstd_working_bytes(22).expect("the test zstd profile is valid");
    ChunkDecompressionBudget {
        state: Arc::new(ChunkDecompressionBudgetState {
            limits: ChunkDecompressionLimits {
                max_compressed_bytes: 16 * 1024 * 1024,
                max_uncompressed_bytes: 16 * 1024 * 1024,
                max_decompression_ratio: 1_024,
                max_zstd_window_log: 22,
            },
            capacity: ChunkDecompressionBudgetCapacity {
                max_active_inputs: 16,
                max_retained_input_bytes: 32 * 1024 * 1024,
                max_active_work_units: 4,
                max_overflow_scratch_bytes: 4,
                max_zstd_working_bytes: max_zstd_working_bytes.saturating_mul(4),
                max_active_outputs: 16,
                max_retained_output_bytes: 32 * 1024 * 1024,
            },
            usage: Mutex::new(ChunkDecompressionBudgetUsage::default()),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use lz4_flex::frame::{BlockMode, BlockSize, FrameEncoder, FrameInfo};

    use super::*;

    fn budget() -> ChunkDecompressionBudget {
        budget_with_input_capacity(4, 8 * 1024 * 1024)
    }

    fn budget_with_input_capacity(
        max_active_inputs: u64,
        max_retained_input_bytes: u64,
    ) -> ChunkDecompressionBudget {
        let max_zstd_working_bytes =
            estimate_zstd_working_bytes(22).expect("test zstd profile is valid");
        ChunkDecompressionBudget {
            state: Arc::new(ChunkDecompressionBudgetState {
                limits: ChunkDecompressionLimits {
                    max_compressed_bytes: 4 * 1024 * 1024,
                    max_uncompressed_bytes: 4 * 1024 * 1024,
                    max_decompression_ratio: 1_024,
                    max_zstd_window_log: 22,
                },
                capacity: ChunkDecompressionBudgetCapacity {
                    max_active_inputs,
                    max_retained_input_bytes,
                    max_active_work_units: 1,
                    max_overflow_scratch_bytes: 1,
                    max_zstd_working_bytes,
                    max_active_outputs: 4,
                    max_retained_output_bytes: 8 * 1024 * 1024,
                },
                usage: Mutex::new(ChunkDecompressionBudgetUsage::default()),
            }),
        }
    }

    fn input<'a>(
        budget: &ChunkDecompressionBudget,
        _test_case: u64,
        compression: &'a str,
        compressed: Vec<u8>,
        declared_size: usize,
        crc: u32,
    ) -> ExactCompressedChunkInput<'a> {
        let expected_compressed_bytes = compressed.len() as u64;
        prepare_exact_compressed_chunk_input(
            compression,
            expected_compressed_bytes,
            declared_size as u64,
            crc,
            budget,
        )
        .expect("test compressed input reservation succeeds")
        .install(compressed.into_boxed_slice())
        .expect("test compressed backing matches its prepared length")
    }

    fn zstd_compress(bytes: &[u8]) -> Vec<u8> {
        zstd::bulk::compress(bytes, 0).expect("test zstd compression succeeds")
    }

    fn zstd_compress_with_window(bytes: &[u8], window_log: u32) -> Vec<u8> {
        let mut encoder = zstd::stream::Encoder::new(Vec::new(), 0)
            .expect("test streaming zstd encoder is created");
        encoder
            .window_log(window_log)
            .expect("test zstd window is configured");
        encoder
            .include_contentsize(false)
            .expect("test zstd frame omits the single-segment content size");
        encoder
            .write_all(bytes)
            .expect("test streaming zstd compression succeeds");
        encoder.finish().expect("test zstd frame finishes")
    }

    fn lz4_compress(bytes: &[u8], frame_info: FrameInfo) -> Vec<u8> {
        let mut encoder = FrameEncoder::with_frame_info(frame_info, Vec::new());
        encoder
            .write_all(bytes)
            .expect("test LZ4 compression succeeds");
        encoder.finish().expect("test LZ4 frame finishes")
    }

    #[expect(
        clippy::fn_params_excessive_bools,
        reason = "test-only adversarial frame builder exposes each independent descriptor flag"
    )]
    fn manual_lz4_frame(
        block_independent: bool,
        block_size_code: u8,
        content_size: Option<u64>,
        block_checksums: bool,
        content_checksum: bool,
        blocks: &[(bool, Vec<u8>)],
        uncompressed_content: &[u8],
    ) -> Vec<u8> {
        let mut flags = LZ4_FRAME_VERSION;
        if block_independent {
            flags |= LZ4_BLOCK_INDEPENDENCE;
        }
        if block_checksums {
            flags |= LZ4_BLOCK_CHECKSUM;
        }
        if content_checksum {
            flags |= LZ4_CONTENT_CHECKSUM;
        }
        if content_size.is_some() {
            flags |= LZ4_CONTENT_SIZE;
        }
        let block_descriptor = block_size_code << 4;

        let mut frame = LZ4_FRAME_MAGIC.to_le_bytes().to_vec();
        let descriptor_start = frame.len();
        frame.push(flags);
        frame.push(block_descriptor);
        if let Some(content_size) = content_size {
            frame.extend_from_slice(&content_size.to_le_bytes());
        }
        let header_checksum = (xxhash_rust::xxh32::xxh32(&frame[descriptor_start..], 0) >> 8) as u8;
        frame.push(header_checksum);

        for (uncompressed, block) in blocks {
            let block_len = u32::try_from(block.len()).expect("test block length fits in u32");
            let block_header = if *uncompressed {
                block_len | LZ4_BLOCK_UNCOMPRESSED
            } else {
                block_len
            };
            frame.extend_from_slice(&block_header.to_le_bytes());
            frame.extend_from_slice(block);
            if block_checksums {
                frame.extend_from_slice(&xxhash_rust::xxh32::xxh32(block, 0).to_le_bytes());
            }
        }
        frame.extend_from_slice(&0_u32.to_le_bytes());
        if content_checksum {
            frame.extend_from_slice(
                &xxhash_rust::xxh32::xxh32(uncompressed_content, 0).to_le_bytes(),
            );
        }
        frame
    }

    fn recompute_lz4_header_checksum(frame: &mut [u8]) {
        let flags = frame[4];
        let descriptor_end = 6
            + if flags & LZ4_CONTENT_SIZE != 0 { 8 } else { 0 }
            + if flags & LZ4_DICTIONARY_ID != 0 { 4 } else { 0 };
        frame[descriptor_end] =
            (xxhash_rust::xxh32::xxh32(&frame[4..descriptor_end], 0) >> 8) as u8;
    }

    fn assert_budget_empty(budget: &ChunkDecompressionBudget) {
        assert_eq!(
            *budget.state.usage.lock(),
            ChunkDecompressionBudgetUsage::default()
        );
    }

    fn assert_input_released(budget: &ChunkDecompressionBudget) {
        let usage = *budget.state.usage.lock();
        assert_eq!(usage.active_inputs, 0);
        assert_eq!(usage.retained_input_bytes, 0);
    }

    #[test]
    fn implementation_is_sealed_disarmed_and_has_no_growing_output_adapter() {
        let source = include_str!("remote_decompression.rs");
        let production = source
            .split("#[cfg(test)]\nmod tests")
            .next()
            .expect("the production section precedes tests");
        for forbidden in [
            "FrameDecoder",
            "read_to_end",
            "Vec::with_capacity",
            "try_reserve_exact",
            ".extend_from_slice(",
        ] {
            assert!(
                !production.contains(forbidden),
                "production decompressor must not contain {forbidden}"
            );
        }
        assert!(production.contains("vec![0_u8; length].into_boxed_slice()"));
        assert!(production.contains("WindowLogMax"));
        assert!(production.contains("OVERFLOW_DETECTION_SCRATCH_BYTES: u64 = 1"));

        let codec_allowlist = production
            .find("let codec = match input.codec")
            .expect("codec allowlist is checked");
        let estimate = production
            .find("estimate_zstd_working_bytes")
            .expect("zstd working memory is estimated");
        let reservation = production
            .find("let reservation = budget_state.try_reserve(")
            .expect("output, scratch, and working-memory reservation is acquired");
        let allocation = production
            .find("let mut output = allocate_exact_output(output_len)")
            .expect("exact output allocation exists");
        assert!(codec_allowlist < reservation);
        assert!(estimate < reservation);
        assert!(reservation < allocation);
        assert!(production.contains("ZSTD_estimateDStreamSize"));
        assert!(!production.contains("pub(crate) fn prepare_exact_compressed_chunk_input"));
        assert!(!production.contains("pub(crate) fn install("));
        assert!(!production.contains("CompressedChunkEvidenceIdentity"));
        let identity_declaration = production
            .split("pub(crate) enum PhysicalChunkReadIdentity")
            .next()
            .expect("physical-read identity declaration exists");
        assert!(!identity_declaration.ends_with("#[derive(Clone, Copy, Debug)]\n"));
        assert!(production.contains("pub(crate) fn identity(&self) -> &PhysicalChunkReadIdentity"));
        let decompressor_signature = production
            .split("pub(crate) fn decompress_exact_chunk(")
            .nth(1)
            .expect("sealed decompressor entry exists")
            .split(") -> Result")
            .next()
            .expect("sealed decompressor signature ends before its result");
        assert!(!decompressor_signature.contains("budget"));
        assert!(decompressor_signature.contains("ExactCompressedChunkInput"));
        assert!(production.contains("fn install(\n        self,"));
        assert!(
            !production.contains("impl ExactCompressedChunkInput<'_> {\n    pub(crate) fn new")
        );
        let input_drop = production
            .find("impl Drop for ExactCompressedChunkInput")
            .expect("compressed input has an explicit release order");
        let input_drop = &production[input_drop..];
        let backing_drop = input_drop
            .find("drop(self.compressed_payload.take())")
            .expect("compressed backing is released explicitly");
        let permit_drop = input_drop
            .find("drop(self.reservation.take())")
            .expect("compressed permit is released explicitly");
        assert!(backing_drop < permit_drop);

        let output_drop = production
            .find("impl Drop for ExactOutputChunk")
            .expect("exact output has an explicit release order");
        let output_drop = &production[output_drop..];
        let backing_drop = output_drop
            .find("drop(self.bytes.take())")
            .expect("exact output drops its backing explicitly");
        let permit_drop = output_drop
            .find("drop(self.reservation.take())")
            .expect("exact output drops its permit explicitly");
        let identity_drop = output_drop
            .find("drop(self.identity.take())")
            .expect("exact output drops its lease-owning identity explicitly");
        assert!(backing_drop < permit_drop);
        assert!(permit_drop < identity_drop);

        let crate_root = include_str!("lib.rs");
        assert!(crate_root.contains("mod remote_decompression;"));
        assert!(!crate_root.contains("pub mod remote_decompression;"));
    }

    #[test]
    fn compressed_input_claim_is_exact_move_only_and_released_on_every_boundary_failure() {
        let input_budget = budget_with_input_capacity(1, 3);
        let prepared = prepare_exact_compressed_chunk_input("zstd", 3, 1, 0, &input_budget)
            .expect("the exact input claim fits");
        assert_eq!(
            *input_budget.state.usage.lock(),
            ChunkDecompressionBudgetUsage {
                active_inputs: 1,
                retained_input_bytes: 3,
                ..ChunkDecompressionBudgetUsage::default()
            }
        );
        assert_eq!(
            prepare_exact_compressed_chunk_input("zstd", 1, 1, 0, &input_budget,)
                .expect_err("a held input claim blocks a second claim"),
            ChunkDecompressionError::CompressedInputReservationLimitExceeded
        );
        assert_eq!(
            prepared
                .install(vec![1, 2].into_boxed_slice())
                .expect_err("the installed backing must have the prepared exact length"),
            ChunkDecompressionError::UnexpectedCompressedInputLength
        );
        assert_budget_empty(&input_budget);

        let zero_capacity = budget_with_input_capacity(0, 3);
        assert_eq!(
            prepare_exact_compressed_chunk_input("zstd", 3, 1, 0, &zero_capacity,)
                .expect_err("the input count limit is checked before installation"),
            ChunkDecompressionError::CompressedInputReservationLimitExceeded
        );
        assert_budget_empty(&zero_capacity);

        let byte_limited = budget_with_input_capacity(1, 2);
        assert_eq!(
            prepare_exact_compressed_chunk_input("zstd", 3, 1, 0, &byte_limited,)
                .expect_err("the exact retained-byte limit is checked before installation"),
            ChunkDecompressionError::CompressedInputReservationLimitExceeded
        );
        assert_budget_empty(&byte_limited);

        let exact_byte_capacity = budget_with_input_capacity(1, 3);
        let exact_claim =
            prepare_exact_compressed_chunk_input("zstd", 3, 1, 0, &exact_byte_capacity)
                .expect("the exact retained-byte boundary is accepted");
        drop(exact_claim);
        assert_budget_empty(&exact_byte_capacity);
    }

    #[test]
    fn compressed_input_permit_releases_before_result_or_after_all_decompression_failures() {
        let budget = budget();

        let raw = b"same backing becomes exact output".to_vec();
        let raw_len = raw.len();
        let raw = decompress_exact_chunk(input(&budget, 1, "", raw, raw_len, 0))
            .expect("raw exact output succeeds");
        assert_input_released(&budget);
        assert_eq!(raw.bytes().len(), raw_len);
        drop(raw);

        let payload = b"compressed input releases before output publication".repeat(32);
        let compressed = zstd_compress(&payload);
        let output =
            decompress_exact_chunk(input(&budget, 2, "zstd", compressed, payload.len(), 0))
                .expect("compressed exact output succeeds");
        assert_input_released(&budget);
        assert_eq!(output.bytes(), payload);
        drop(output);

        for (identity, compression, compressed, declared_size, crc, expected) in [
            (
                3,
                "snappy",
                vec![1],
                1,
                0,
                ChunkDecompressionError::UnsupportedCompression,
            ),
            (
                4,
                "zstd",
                vec![1],
                1,
                0,
                ChunkDecompressionError::InvalidCompressedData,
            ),
            (
                5,
                "",
                vec![1],
                1,
                1,
                ChunkDecompressionError::ChunkChecksumMismatch,
            ),
        ] {
            assert_eq!(
                decompress_exact_chunk(input(
                    &budget,
                    identity,
                    compression,
                    compressed,
                    declared_size,
                    crc,
                ))
                .expect_err("the injected decompression boundary fails"),
                expected
            );
            assert_budget_empty(&budget);
        }

        let no_output_capacity = ChunkDecompressionBudget {
            state: Arc::new(ChunkDecompressionBudgetState {
                limits: budget.state.limits,
                capacity: ChunkDecompressionBudgetCapacity {
                    max_active_inputs: 1,
                    max_retained_input_bytes: 1,
                    max_active_work_units: 0,
                    max_overflow_scratch_bytes: 0,
                    max_zstd_working_bytes: 0,
                    max_active_outputs: 0,
                    max_retained_output_bytes: 0,
                },
                usage: Mutex::new(ChunkDecompressionBudgetUsage::default()),
            }),
        };
        assert_eq!(
            decompress_exact_chunk(input(&no_output_capacity, 6, "", vec![1], 1, 0))
                .expect_err("output reservation failure releases exact input ownership"),
            ChunkDecompressionError::ReservationLimitExceeded
        );
        assert_budget_empty(&no_output_capacity);
    }

    #[test]
    fn sealed_handoff_identity_cannot_replay_rebind_or_cross_source_profiles() {
        let source_profile = budget();
        let other_profile = budget();
        let first_payload = b"first physical read".to_vec();
        let first_prepared = prepare_exact_compressed_chunk_input(
            "",
            first_payload.len() as u64,
            first_payload.len() as u64,
            0,
            &source_profile,
        )
        .expect("first physical handoff is prepared");
        let first_identity = first_prepared.identity.codec_test_opaque();
        let first_input = first_prepared
            .install(first_payload.clone().into_boxed_slice())
            .expect("the exact first backing installs once");
        let first_output = decompress_exact_chunk(first_input).expect("first handoff succeeds");
        assert_eq!(first_output.identity().codec_test_opaque(), first_identity);
        assert_eq!(first_output.bytes(), first_payload);
        assert_input_released(&source_profile);
        assert_budget_empty(&other_profile);
        assert_eq!(
            *source_profile.state.usage.lock(),
            ChunkDecompressionBudgetUsage {
                active_outputs: 1,
                retained_output_bytes: first_payload.len() as u64,
                ..ChunkDecompressionBudgetUsage::default()
            },
            "output ownership remains on the handoff's original source profile"
        );

        let second_payload = b"different physical read".to_vec();
        let second_prepared = prepare_exact_compressed_chunk_input(
            "",
            second_payload.len() as u64,
            second_payload.len() as u64,
            0,
            &source_profile,
        )
        .expect("a later physical handoff is prepared independently");
        assert_ne!(
            second_prepared.identity.codec_test_opaque(),
            first_identity,
            "no API accepts a prior identity for a different payload"
        );
        drop(second_prepared);
        drop(first_output);
        assert_budget_empty(&source_profile);

        let source = include_str!("remote_decompression.rs");
        let production = source
            .split("#[cfg(test)]\nmod tests")
            .next()
            .expect("production source precedes tests");
        assert!(!production.contains("PhysicalChunkReadIdentity::new"));
        assert!(!production.contains("identity: u128"));
        assert!(production.contains("fn install(\n        self,"));
        assert!(!production.contains("pub(crate) fn install("));
        assert!(!production.contains(
            "decompress_exact_chunk(\n    input: ExactCompressedChunkInput<'_>,\n    budget:"
        ));
    }

    #[test]
    fn codec_unit_identity_cannot_enter_the_physical_semantic_scanner() {
        let budget = budget();
        let output = decompress_exact_chunk(input(&budget, 1, "", vec![1], 1, 0)).unwrap();
        assert_eq!(
            crate::remote_chunk_scan::scan_decompressed_physical_chunk(output).unwrap_err(),
            crate::remote_chunk_scan::PhysicalChunkValidationError::MissingPhysicalReadLease
        );
        assert_budget_empty(&budget);
    }

    #[test]
    fn source_profile_state_closes_only_after_the_last_input_or_output_owner() {
        let budget = budget();
        let state = Arc::clone(&budget.state);
        let weak_state = Arc::downgrade(&state);
        let payload = b"source profile lifetime".to_vec();
        let input = input(&budget, 1, "", payload.clone(), payload.len(), 0);
        assert!(Arc::ptr_eq(
            &input
                .reservation
                .as_ref()
                .expect("installed input retains its source profile")
                .state,
            &state,
        ));

        drop(budget);
        let output = decompress_exact_chunk(input)
            .expect("the sealed input keeps its source profile alive during close");
        assert!(Arc::ptr_eq(
            &output
                .reservation
                .as_ref()
                .expect("the exact output retains its reservation")
                .state,
            &state
        ));
        assert_eq!(output.bytes(), payload);
        drop(state);
        assert!(weak_state.upgrade().is_some());
        drop(output);
        assert!(
            weak_state.upgrade().is_none(),
            "the unified profile closes after its final exact output releases"
        );
    }

    #[test]
    fn zstd_working_set_is_estimated_reserved_and_released_separately() {
        let window_log = 22;
        let estimate = estimate_zstd_working_bytes(window_log)
            .expect("the release zstd profile has a documented estimate");
        assert!(estimate > 0);

        for invalid in [0, ZSTD_WINDOW_LOG_MIN - 1] {
            assert_eq!(
                estimate_zstd_working_bytes(invalid)
                    .expect_err("an invalid zstd window profile is rejected"),
                ChunkDecompressionError::InvalidZstdWindowProfile
            );
        }
        let target_max = if usize::BITS == 32 {
            ZSTD_WINDOW_LOG_MAX_32
        } else {
            ZSTD_WINDOW_LOG_MAX_64
        };
        assert_eq!(
            estimate_zstd_working_bytes(target_max + 1)
                .expect_err("a target-inexpressible zstd window profile is rejected"),
            ChunkDecompressionError::InvalidZstdWindowProfile
        );

        let payload = b"working memory is budgeted before decoder creation".repeat(64);
        let compressed = zstd_compress(&payload);
        let exact_budget = ChunkDecompressionBudget {
            state: Arc::new(ChunkDecompressionBudgetState {
                limits: ChunkDecompressionLimits {
                    max_compressed_bytes: compressed.len() as u64,
                    max_uncompressed_bytes: payload.len() as u64,
                    max_decompression_ratio: 1_024,
                    max_zstd_window_log: window_log,
                },
                capacity: ChunkDecompressionBudgetCapacity {
                    max_active_inputs: 2,
                    max_retained_input_bytes: (compressed.len() * 2) as u64,
                    max_active_work_units: 2,
                    max_overflow_scratch_bytes: 2,
                    max_zstd_working_bytes: estimate,
                    max_active_outputs: 2,
                    max_retained_output_bytes: (payload.len() * 2) as u64,
                },
                usage: Mutex::new(ChunkDecompressionBudgetUsage::default()),
            }),
        };
        let output = decompress_exact_chunk(input(
            &exact_budget,
            7,
            "zstd",
            compressed.clone(),
            payload.len(),
            0,
        ))
        .expect("an exact zstd working-set capacity succeeds");
        assert_eq!(
            *exact_budget.state.usage.lock(),
            ChunkDecompressionBudgetUsage {
                active_outputs: 1,
                retained_output_bytes: payload.len() as u64,
                ..ChunkDecompressionBudgetUsage::default()
            },
            "zstd working bytes are released after the decoder is dropped"
        );
        drop(output);

        let held = exact_budget
            .state
            .try_reserve(compressed.len() as u64, payload.len() as u64, estimate)
            .expect("the first zstd work claim fits exactly");
        assert_eq!(
            decompress_exact_chunk(input(
                &exact_budget,
                8,
                "zstd",
                compressed.clone(),
                payload.len(),
                0,
            ))
            .expect_err("a live decoder working claim blocks a second claim"),
            ChunkDecompressionError::ZstdWorkingReservationLimitExceeded
        );
        drop(held);
        assert_budget_empty(&exact_budget);

        let underprovisioned = ChunkDecompressionBudget {
            state: Arc::new(ChunkDecompressionBudgetState {
                limits: exact_budget.state.limits,
                capacity: ChunkDecompressionBudgetCapacity {
                    max_active_inputs: 1,
                    max_retained_input_bytes: compressed.len() as u64,
                    max_active_work_units: 1,
                    max_overflow_scratch_bytes: 1,
                    max_zstd_working_bytes: estimate - 1,
                    max_active_outputs: 1,
                    max_retained_output_bytes: payload.len() as u64,
                },
                usage: Mutex::new(ChunkDecompressionBudgetUsage::default()),
            }),
        };
        assert_eq!(
            decompress_exact_chunk(input(
                &underprovisioned,
                9,
                "zstd",
                compressed.clone(),
                payload.len(),
                0,
            ))
            .expect_err("one byte below the documented estimate fails admission"),
            ChunkDecompressionError::ZstdWorkingReservationLimitExceeded
        );
        assert_budget_empty(&underprovisioned);

        assert_eq!(
            decompress_exact_chunk(input(&exact_budget, 10, "zstd", vec![1], 1, 0))
                .expect_err("decoder failure releases its working-set claim"),
            ChunkDecompressionError::InvalidCompressedData
        );
        assert_budget_empty(&exact_budget);
    }

    #[test]
    fn none_transfers_exact_input_and_preserves_identity_and_crc_evidence() {
        let budget = budget();
        let payload = b"exact raw MCAP records".to_vec();
        let crc = crc32fast::hash(&payload);
        let chunk =
            decompress_exact_chunk(input(&budget, 7, "", payload.clone(), payload.len(), crc))
                .expect("exact raw payload succeeds");

        assert_ne!(chunk.identity().codec_test_opaque(), 0);
        assert_eq!(chunk.codec(), ChunkCompressionCodec::None);
        assert_eq!(chunk.bytes(), payload);
        assert_eq!(
            chunk.crc(),
            ChunkCrcEvidence {
                declared: crc,
                validation: OptionalCrcValidation::Verified,
            }
        );
        assert_eq!(
            *budget.state.usage.lock(),
            ChunkDecompressionBudgetUsage {
                active_outputs: 1,
                retained_output_bytes: payload.len() as u64,
                ..ChunkDecompressionBudgetUsage::default()
            }
        );
        drop(chunk);
        assert_budget_empty(&budget);
    }

    #[test]
    fn zero_crc_is_not_provided_and_mismatch_releases_reservation() {
        let budget = budget();
        let payload = b"crc semantics".to_vec();
        let chunk =
            decompress_exact_chunk(input(&budget, 1, "", payload.clone(), payload.len(), 0))
                .expect("zero CRC is accepted as absent");
        assert_eq!(chunk.crc().validation, OptionalCrcValidation::NotProvided);
        drop(chunk);

        let error =
            decompress_exact_chunk(input(&budget, 2, "", payload.clone(), payload.len(), 1))
                .expect_err("nonzero mismatching CRC fails");
        assert_eq!(error, ChunkDecompressionError::ChunkChecksumMismatch);
        assert_budget_empty(&budget);
    }

    #[test]
    fn zstd_requires_one_exact_frame_and_exact_output() {
        let budget = budget();
        let payload = b"zstd exact output".repeat(128);
        let compressed = zstd_compress(&payload);
        let chunk = decompress_exact_chunk(input(
            &budget,
            3,
            "zstd",
            compressed.clone(),
            payload.len(),
            0,
        ))
        .expect("one exact zstd frame succeeds");
        assert_eq!(chunk.codec(), ChunkCompressionCodec::Zstd);
        assert_eq!(chunk.bytes(), payload);
        drop(chunk);

        let error = decompress_exact_chunk(input(
            &budget,
            3,
            "zstd",
            compressed.clone(),
            payload.len() - 1,
            0,
        ))
        .expect_err("declared output shorter than the frame fails");
        assert_eq!(error, ChunkDecompressionError::OutputTooLong);

        let error =
            decompress_exact_chunk(input(&budget, 3, "zstd", compressed, payload.len() + 1, 0))
                .expect_err("declared output longer than the frame fails");
        assert_eq!(error, ChunkDecompressionError::OutputTooShort);
        assert_budget_empty(&budget);
    }

    #[test]
    fn empty_zstd_and_lz4_frames_reach_eof_through_the_probe_path() {
        let budget = budget();
        for (identity, compression, compressed) in [
            (10, "zstd", zstd_compress(&[])),
            (11, "lz4", lz4_compress(&[], FrameInfo::new())),
        ] {
            let chunk =
                decompress_exact_chunk(input(&budget, identity, compression, compressed, 0, 0))
                    .expect("one exact empty frame succeeds");
            assert!(chunk.bytes().is_empty());
            drop(chunk);
        }
        assert_budget_empty(&budget);
    }

    #[test]
    fn zstd_rejects_concatenation_trailing_payload_and_truncation() {
        let budget = budget();
        let payload = b"single zstd frame".repeat(64);
        let canonical = zstd_compress(&payload);

        let mut concatenated = canonical.clone();
        concatenated.extend_from_slice(&zstd_compress(&[]));
        assert_eq!(
            decompress_exact_chunk(input(&budget, 4, "zstd", concatenated, payload.len(), 0))
                .expect_err("a second zstd frame fails"),
            ChunkDecompressionError::TrailingCompressedInput
        );

        let mut trailing = canonical.clone();
        trailing.extend_from_slice(b"trailing");
        assert_eq!(
            decompress_exact_chunk(input(&budget, 4, "zstd", trailing, payload.len(), 0))
                .expect_err("trailing bytes fail"),
            ChunkDecompressionError::TrailingCompressedInput
        );

        let mut truncated = canonical;
        truncated.pop();
        assert!(matches!(
            decompress_exact_chunk(input(&budget, 4, "zstd", truncated, payload.len(), 0)),
            Err(ChunkDecompressionError::CompressedFrameTruncated
                | ChunkDecompressionError::InvalidCompressedData)
        ));
        assert_budget_empty(&budget);
    }

    #[test]
    fn lz4_independent_and_linked_frames_write_only_the_exact_output() {
        let budget = budget();
        let payload = (0..200_000_u32)
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>();
        for (identity, block_mode) in [BlockMode::Independent, BlockMode::Linked]
            .into_iter()
            .enumerate()
        {
            let frame_info = FrameInfo::new()
                .block_mode(block_mode)
                .block_size(BlockSize::Max64KB)
                .content_size(Some(payload.len() as u64))
                .block_checksums(true)
                .content_checksum(true);
            let compressed = lz4_compress(&payload, frame_info);
            let chunk = decompress_exact_chunk(input(
                &budget,
                identity as u64,
                "lz4",
                compressed,
                payload.len(),
                0,
            ))
            .expect("one exact LZ4 frame succeeds");
            assert_eq!(chunk.codec(), ChunkCompressionCodec::Lz4);
            assert_eq!(chunk.bytes(), payload);
            drop(chunk);
        }
        assert_budget_empty(&budget);
    }

    #[test]
    fn lz4_rejects_short_long_concatenated_trailing_and_truncated_inputs() {
        let budget = budget();
        let payload = b"single LZ4 frame".repeat(256);
        let canonical = lz4_compress(&payload, FrameInfo::new());

        assert_eq!(
            decompress_exact_chunk(input(
                &budget,
                5,
                "lz4",
                canonical.clone(),
                payload.len() - 1,
                0,
            ))
            .expect_err("short declared output fails"),
            ChunkDecompressionError::OutputTooLong
        );
        assert_eq!(
            decompress_exact_chunk(input(
                &budget,
                5,
                "lz4",
                canonical.clone(),
                payload.len() + 1,
                0,
            ))
            .expect_err("long declared output fails"),
            ChunkDecompressionError::OutputTooShort
        );

        let mut concatenated = canonical.clone();
        concatenated.extend_from_slice(&lz4_compress(&[], FrameInfo::new()));
        assert_eq!(
            decompress_exact_chunk(input(&budget, 5, "lz4", concatenated, payload.len(), 0))
                .expect_err("concatenated LZ4 frame fails"),
            ChunkDecompressionError::TrailingCompressedInput
        );

        let mut trailing = canonical.clone();
        trailing.extend_from_slice(b"trailing");
        assert_eq!(
            decompress_exact_chunk(input(&budget, 5, "lz4", trailing, payload.len(), 0))
                .expect_err("trailing LZ4 payload fails"),
            ChunkDecompressionError::TrailingCompressedInput
        );

        let mut truncated = canonical;
        truncated.pop();
        assert_eq!(
            decompress_exact_chunk(input(&budget, 5, "lz4", truncated, payload.len(), 0))
                .expect_err("truncated LZ4 frame fails"),
            ChunkDecompressionError::CompressedFrameTruncated
        );
        assert_budget_empty(&budget);
    }

    #[test]
    fn malformed_lz4_checksums_fail_before_result_publication() {
        let budget = budget();
        let payload = b"checksummed LZ4".repeat(1_024);
        let frame_info = FrameInfo::new()
            .block_checksums(true)
            .content_checksum(true);
        let mut compressed = lz4_compress(&payload, frame_info);
        let last = compressed
            .last_mut()
            .expect("checksummed LZ4 frame is nonempty");
        *last ^= 1;
        assert_eq!(
            decompress_exact_chunk(input(&budget, 6, "lz4", compressed, payload.len(), 0))
                .expect_err("bad LZ4 checksum fails"),
            ChunkDecompressionError::InvalidCompressedData
        );
        assert_budget_empty(&budget);
    }

    #[test]
    fn lz4_block_decoder_never_receives_more_than_the_bd_maximum() {
        for block_independent in [true, false] {
            let oversized_payload = vec![0_u8; 70 * 1024];
            let compressed_block = lz4_flex::block::compress(&oversized_payload);
            assert!(compressed_block.len() < 64 * 1024);
            let frame = manual_lz4_frame(
                block_independent,
                4,
                Some(oversized_payload.len() as u64),
                false,
                false,
                &[(false, compressed_block.clone())],
                &oversized_payload,
            );
            let mut output = vec![0xA5; oversized_payload.len()];
            assert_eq!(
                decompress_lz4_single_frame(&frame, &mut output)
                    .expect_err("one block cannot expand beyond its BD maximum"),
                ChunkDecompressionError::Lz4BlockOutputLimitExceeded
            );
            assert!(
                output[64 * 1024..].iter().all(|byte| *byte == 0xA5),
                "the block decoder never receives or mutates bytes beyond the BD cap"
            );

            let frame_without_content_size = manual_lz4_frame(
                block_independent,
                4,
                None,
                false,
                false,
                &[(false, compressed_block)],
                &oversized_payload,
            );
            let mut short_output = vec![0xA5; 60 * 1024];
            assert_eq!(
                decompress_lz4_single_frame(&frame_without_content_size, &mut short_output)
                    .expect_err("an exact output shorter than the BD maximum cannot be exceeded"),
                ChunkDecompressionError::OutputTooLong
            );

            let exact_payload = vec![0_u8; 64 * 1024];
            let exact_frame = manual_lz4_frame(
                block_independent,
                4,
                Some(exact_payload.len() as u64),
                false,
                false,
                &[(false, lz4_flex::block::compress(&exact_payload))],
                &exact_payload,
            );
            let mut exact_output = vec![0xA5; exact_payload.len()];
            decompress_lz4_single_frame(&exact_frame, &mut exact_output)
                .expect("a block exactly at its BD output maximum succeeds");
            assert_eq!(exact_output, exact_payload);
        }
    }

    #[test]
    fn lz4_descriptor_rejects_reserved_legacy_skippable_and_dictionary_forms() {
        let canonical = manual_lz4_frame(true, 4, None, false, false, &[], &[]);
        for mutate in [
            |frame: &mut Vec<u8>| frame[4] = 0,
            |frame: &mut Vec<u8>| frame[4] |= LZ4_FLAG_RESERVED,
            |frame: &mut Vec<u8>| frame[5] |= 0x80,
            |frame: &mut Vec<u8>| frame[5] = 0x30,
        ] {
            let mut frame = canonical.clone();
            mutate(&mut frame);
            recompute_lz4_header_checksum(&mut frame);
            let mut cursor = 0;
            assert_eq!(
                parse_lz4_frame_descriptor(&frame, &mut cursor)
                    .expect_err("invalid LZ4 descriptor bits are rejected"),
                ChunkDecompressionError::InvalidCompressedData
            );
        }

        let mut dictionary = LZ4_FRAME_MAGIC.to_le_bytes().to_vec();
        let flags = LZ4_FRAME_VERSION | LZ4_BLOCK_INDEPENDENCE | LZ4_DICTIONARY_ID;
        dictionary.push(flags);
        dictionary.push(4 << 4);
        dictionary.extend_from_slice(&123_u32.to_le_bytes());
        let checksum = (xxhash_rust::xxh32::xxh32(&dictionary[4..], 0) >> 8) as u8;
        dictionary.push(checksum);
        dictionary.extend_from_slice(&0_u32.to_le_bytes());
        assert_eq!(
            decompress_lz4_single_frame(&dictionary, &mut [])
                .expect_err("dictionary-ID frames are outside the release allowlist"),
            ChunkDecompressionError::InvalidCompressedData
        );

        for magic in [LZ4_LEGACY_FRAME_MAGIC, LZ4_SKIPPABLE_FRAME_MAGIC_START] {
            let mut frame = canonical.clone();
            frame[..4].copy_from_slice(&magic.to_le_bytes());
            assert_eq!(
                decompress_lz4_single_frame(&frame, &mut [])
                    .expect_err("legacy and skippable frames are outside the release allowlist"),
                ChunkDecompressionError::InvalidCompressedData
            );
        }

        let mut bad_checksum = canonical;
        bad_checksum[6] ^= 1;
        assert_eq!(
            decompress_lz4_single_frame(&bad_checksum, &mut [])
                .expect_err("the descriptor checksum is mandatory"),
            ChunkDecompressionError::InvalidCompressedData
        );
    }

    #[test]
    fn lz4_frame_matrix_covers_all_bd_sizes_raw_blocks_and_truncated_checksums() {
        for block_size_code in 4..=7 {
            let frame = manual_lz4_frame(true, block_size_code, None, false, false, &[], &[]);
            let mut cursor = 0;
            let descriptor = parse_lz4_frame_descriptor(&frame, &mut cursor)
                .expect("all four standard BD size codes are accepted");
            assert_eq!(
                descriptor.max_block_size,
                match block_size_code {
                    4 => 64 * 1024,
                    5 => 256 * 1024,
                    6 => 1024 * 1024,
                    7 => 4 * 1024 * 1024,
                    _ => unreachable!(),
                }
            );
            decompress_lz4_single_frame(&frame, &mut [])
                .expect("an empty single frame for each BD code succeeds");
        }

        let payload = b"uncompressed LZ4 block".repeat(16);
        let raw_frame = manual_lz4_frame(
            true,
            4,
            Some(payload.len() as u64),
            true,
            true,
            &[(true, payload.clone())],
            &payload,
        );
        let mut output = vec![0; payload.len()];
        decompress_lz4_single_frame(&raw_frame, &mut output)
            .expect("a checksummed uncompressed block succeeds");
        assert_eq!(output, payload);

        let mut bad_block_checksum = raw_frame.clone();
        let block_checksum_offset = 4 + 2 + 8 + 1 + 4 + payload.len();
        bad_block_checksum[block_checksum_offset] ^= 1;
        assert_eq!(
            decompress_lz4_single_frame(&bad_block_checksum, &mut vec![0; payload.len()])
                .expect_err("a corrupt block checksum fails"),
            ChunkDecompressionError::InvalidCompressedData
        );

        let mut bad_content_checksum = raw_frame.clone();
        *bad_content_checksum
            .last_mut()
            .expect("checksummed frame is nonempty") ^= 1;
        assert_eq!(
            decompress_lz4_single_frame(&bad_content_checksum, &mut vec![0; payload.len()])
                .expect_err("a corrupt content checksum fails"),
            ChunkDecompressionError::InvalidCompressedData
        );

        for truncate_by in [4, 3, 1] {
            let mut truncated = raw_frame.clone();
            truncated.truncate(truncated.len() - truncate_by);
            assert_eq!(
                decompress_lz4_single_frame(&truncated, &mut vec![0; payload.len()])
                    .expect_err("missing checksum or EndMark bytes fail as truncation"),
                ChunkDecompressionError::CompressedFrameTruncated
            );
        }

        let block_checksum_frame = manual_lz4_frame(
            true,
            4,
            None,
            true,
            false,
            &[(true, payload.clone())],
            &payload,
        );
        let block_checksum_start = 7 + 4 + payload.len();
        for retained_checksum_bytes in [0, 1, 3] {
            let mut truncated = block_checksum_frame.clone();
            truncated.truncate(block_checksum_start + retained_checksum_bytes);
            assert_eq!(
                decompress_lz4_single_frame(&truncated, &mut vec![0; payload.len()])
                    .expect_err("a truncated block checksum cannot reach publication"),
                ChunkDecompressionError::CompressedFrameTruncated
            );
        }

        let no_checksums = manual_lz4_frame(
            true,
            4,
            None,
            false,
            false,
            &[(true, payload.clone())],
            &payload,
        );
        let mut missing_end_mark = no_checksums;
        missing_end_mark.truncate(missing_end_mark.len() - 4);
        assert_eq!(
            decompress_lz4_single_frame(&missing_end_mark, &mut vec![0; payload.len()])
                .expect_err("a missing EndMark fails as truncation"),
            ChunkDecompressionError::CompressedFrameTruncated
        );
    }

    #[test]
    fn lz4_content_size_must_match_the_exact_output_contract() {
        let smaller = manual_lz4_frame(true, 4, Some(1), false, false, &[], &[]);
        assert_eq!(
            decompress_lz4_single_frame(&smaller, &mut [0; 2])
                .expect_err("a smaller frame content size cannot fill the exact output"),
            ChunkDecompressionError::OutputTooShort
        );
        let larger = manual_lz4_frame(true, 4, Some(2), false, false, &[], &[]);
        assert_eq!(
            decompress_lz4_single_frame(&larger, &mut [0; 1])
                .expect_err("a larger frame content size exceeds the exact output"),
            ChunkDecompressionError::OutputTooLong
        );
    }

    #[test]
    fn unsupported_and_resource_failures_happen_before_output_ownership() {
        let budget = budget();
        assert_eq!(
            decompress_exact_chunk(input(&budget, 8, "snappy", vec![1], 1, 0))
                .expect_err("unsupported codec fails"),
            ChunkDecompressionError::UnsupportedCompression
        );
        assert_budget_empty(&budget);

        let oversized = prepare_exact_compressed_chunk_input("zstd", 1, u64::MAX, 0, &budget)
            .expect("test compressed input reservation succeeds")
            .install(vec![1].into_boxed_slice())
            .expect("test compressed backing matches its prepared length");
        assert!(matches!(
            decompress_exact_chunk(oversized),
            Err(ChunkDecompressionError::DeclaredOutputSizeUnsupported
                | ChunkDecompressionError::UncompressedBytesLimitExceeded)
        ));
        assert_budget_empty(&budget);

        let ratio_limited = ChunkDecompressionBudget {
            state: Arc::new(ChunkDecompressionBudgetState {
                limits: ChunkDecompressionLimits {
                    max_compressed_bytes: 100,
                    max_uncompressed_bytes: 100,
                    max_decompression_ratio: 2,
                    max_zstd_window_log: 10,
                },
                capacity: ChunkDecompressionBudgetCapacity {
                    max_active_inputs: 1,
                    max_retained_input_bytes: 100,
                    max_active_work_units: 1,
                    max_overflow_scratch_bytes: 1,
                    max_zstd_working_bytes: estimate_zstd_working_bytes(10)
                        .expect("test zstd profile is valid"),
                    max_active_outputs: 1,
                    max_retained_output_bytes: 100,
                },
                usage: Mutex::new(ChunkDecompressionBudgetUsage::default()),
            }),
        };
        assert_eq!(
            decompress_exact_chunk(input(&ratio_limited, 8, "zstd", vec![1], 3, 0))
                .expect_err("ratio cap fails"),
            ChunkDecompressionError::DecompressionRatioLimitExceeded
        );
        assert_budget_empty(&ratio_limited);
    }

    #[test]
    fn zstd_window_and_reservation_arithmetic_are_bounded_before_output_allocation() {
        let constrained_window = ChunkDecompressionBudget {
            state: Arc::new(ChunkDecompressionBudgetState {
                limits: ChunkDecompressionLimits {
                    max_compressed_bytes: 128 * 1024,
                    max_uncompressed_bytes: 128 * 1024,
                    max_decompression_ratio: 1_024,
                    max_zstd_window_log: 10,
                },
                capacity: ChunkDecompressionBudgetCapacity {
                    max_active_inputs: 1,
                    max_retained_input_bytes: 128 * 1024,
                    max_active_work_units: 1,
                    max_overflow_scratch_bytes: 1,
                    max_zstd_working_bytes: estimate_zstd_working_bytes(10)
                        .expect("test zstd profile is valid"),
                    max_active_outputs: 1,
                    max_retained_output_bytes: 128 * 1024,
                },
                usage: Mutex::new(ChunkDecompressionBudgetUsage::default()),
            }),
        };
        let mut state = 0x1234_5678_u32;
        let payload = (0..64 * 1024)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect::<Vec<_>>();
        assert_eq!(
            decompress_exact_chunk(input(
                &constrained_window,
                12,
                "zstd",
                zstd_compress_with_window(&payload, 16),
                payload.len(),
                0,
            ),)
            .expect_err("a frame above the admitted zstd window fails"),
            ChunkDecompressionError::InvalidCompressedData
        );
        assert_budget_empty(&constrained_window);

        let overflow_budget = ChunkDecompressionBudget {
            state: Arc::new(ChunkDecompressionBudgetState {
                limits: ChunkDecompressionLimits {
                    max_compressed_bytes: u64::MAX,
                    max_uncompressed_bytes: u64::MAX,
                    max_decompression_ratio: u64::MAX,
                    max_zstd_window_log: 22,
                },
                capacity: ChunkDecompressionBudgetCapacity {
                    max_active_inputs: u64::MAX,
                    max_retained_input_bytes: u64::MAX,
                    max_active_work_units: u64::MAX,
                    max_overflow_scratch_bytes: u64::MAX,
                    max_zstd_working_bytes: u64::MAX,
                    max_active_outputs: u64::MAX,
                    max_retained_output_bytes: u64::MAX,
                },
                usage: Mutex::new(ChunkDecompressionBudgetUsage::default()),
            }),
        };
        let overflow =
            prepare_exact_compressed_chunk_input("zstd", 1, u64::MAX, 0, &overflow_budget)
                .expect("test compressed input reservation succeeds")
                .install(vec![1].into_boxed_slice())
                .expect("test compressed backing matches its prepared length");
        assert_eq!(
            decompress_exact_chunk(overflow)
                .expect_err("output plus one-byte scratch arithmetic must not wrap"),
            ChunkDecompressionError::ReservationArithmeticOverflow
        );
        assert_budget_empty(&overflow_budget);
    }

    #[test]
    fn streaming_driver_detects_zero_progress_and_unfinished_full_output() {
        let mut output = [0_u8; 1];
        assert_eq!(
            drive_exact_streaming_codec(&[1], &mut output, |_input, _output| {
                Ok(StreamingCodecStep {
                    input_consumed: 0,
                    output_written: 0,
                    frame_finished: false,
                })
            })
            .expect_err("zero progress fails"),
            ChunkDecompressionError::CodecZeroProgress
        );

        let mut first = true;
        assert_eq!(
            drive_exact_streaming_codec(&[1], &mut output, |_input, output| {
                if first {
                    first = false;
                    output[0] = 1;
                    Ok(StreamingCodecStep {
                        input_consumed: 1,
                        output_written: 1,
                        frame_finished: false,
                    })
                } else {
                    unreachable!("exhausted input is rejected before another codec step")
                }
            })
            .expect_err("full output without frame EOF fails"),
            ChunkDecompressionError::CompressedFrameTruncated
        );
    }

    #[test]
    fn streaming_driver_uses_one_byte_probe_to_detect_extra_output() {
        let mut output = [0_u8; 1];
        let mut call = 0;
        assert_eq!(
            drive_exact_streaming_codec(&[1, 2], &mut output, |input, output| {
                call += 1;
                output[0] = input[0];
                Ok(StreamingCodecStep {
                    input_consumed: 1,
                    output_written: 1,
                    frame_finished: call == 2,
                })
            })
            .expect_err("the first byte past exact output fails"),
            ChunkDecompressionError::OutputTooLong
        );
        assert_eq!(call, 2, "the second call is the one-byte overflow probe");
    }
}
