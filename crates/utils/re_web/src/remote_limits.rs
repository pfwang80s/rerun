//! Versioned resource limits for bounded Web data ingestion.
//!
//! This module defines the schema and accounting primitives only.
//! It is deliberately not connected to any production data path until every limit in a profile
//! has been frozen with the evidence required by its definition.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::num::NonZeroU64;
use std::rc::{Rc, Weak};
#[cfg(any(test, target_arch = "wasm32"))]
use std::sync::atomic::{AtomicU64, Ordering};

/// The version of the Web remote resource-limit schema.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WebRemoteLimitsProfileVersion {
    V1,
}

/// A checked protocol identity for page-execution transitions.
///
/// This is intentionally not a resource-limit schema value: normal visibility transitions must
/// not consume a measured quota. Exhaustion is terminal for the owning Viewer instance so an old
/// identity is never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PageExecutionEpoch(u64);

impl PageExecutionEpoch {
    pub const INITIAL: Self = Self(0);

    pub const fn get(self) -> u64 {
        self.0
    }

    pub const fn checked_next(self) -> Result<Self, PageExecutionEpochExhausted> {
        match self.0.checked_add(1) {
            Some(next) => Ok(Self(next)),
            None => Err(PageExecutionEpochExhausted),
        }
    }
}

/// The page-execution identity space has been exhausted and must not wrap or be reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageExecutionEpochExhausted;

impl fmt::Display for PageExecutionEpochExhausted {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("page execution epoch exhausted")
    }
}

impl std::error::Error for PageExecutionEpochExhausted {}

/// The subsystem whose ownership is bounded by a limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WebRemoteLimitDomain {
    ScopeAccounting,
    Summary,
    NestedCollection,
    Chunk,
    Decoder,
    Planning,
    Registration,
    Transport,
    Opening,
    OpenRegistry,
    Lifecycle,
    Presentation,
    PageExecution,
    ExternalString,
    ExistingIdentifierIndex,
    RuntimeIntern,
    Cache,
    StoreMutation,
    MemoryPressure,
    Backfill,
}

/// The unit used by a limit value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WebRemoteLimitUnit {
    Bytes,
    Count,
    Utf16CodeUnits,
    Utf8Bytes,
    Rows,
    Records,
    Messages,
    RangeRequests,
    Operations,
    Frames,
    Microseconds,
    Milliseconds,
    RatioPermille,
}

/// The lifetime or ownership boundary to which a limit applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WebRemoteLimitScope {
    WasmModuleLifetime,
    AccountingScopeNode,
    ViewerInstance,
    Source,
    Session,
    Request,
    AtomicBatch,
    Operation,
    Summary,
    NestedRecord,
    MessageIndexRegion,
    Chunk,
    DecoderGroupPerChunk,
    Partition,
    Generation,
    Window,
    RangeResponse,
    Frame,
    WorkUnit,
    IngressItem,
    Channel,
    UrlFingerprintBucket,
    PageExecution,
    Store,
}

/// How a value participates in checked accounting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WebRemoteAccountingKind {
    /// A checked formula or fixed safety margin that is not runtime ownership.
    FormulaConstant,

    /// A bounded scalar or cumulative observation within the declared scope.
    ScalarObservation,

    /// Process-global ownership that is permanently burned once committed.
    ModuleLifetimeBurn,

    /// Concurrent ownership that is reclaimed by an exact generation-aware release.
    ReclaimableConcurrent,
}

/// The telemetry aggregation required when a limit is eventually connected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WebRemoteLimitTelemetry {
    MaximumObserved,
    HighWatermark,
    DeadlineOutcome,
}

macro_rules! define_normative_requirements {
    ($($variant:ident),+ $(,)?) => {
        /// One stable checklist line from `_mcap_stream_seek.md:5774-5839`, or a named
        /// additional resource boundary required by the same design.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum NormativeResourceRequirement {
            $($variant),+
        }

        impl NormativeResourceRequirement {
            pub const ALL: [Self; [$(stringify!($variant)),+].len()] = [$(Self::$variant),+];
        }
    };
}

define_normative_requirements! {
    SummaryBytes,
    SummaryRecordCount,
    SummarySchemaCount,
    SummaryChannelCount,
    SummaryChunkIndexCount,
    PhysicalRegionDescriptorCount,
    ChannelMetadataAndNestedRetention,
    ChunkIndexMessageIndexOffsets,
    StatisticsChannelMessageCounts,
    MessageIndexRecordEntries,
    MessageIndexOwningRegion,
    AmbiguousZeroMessageIndex,
    AmbiguousZeroChunk,
    ChunkCompressedBytes,
    ChunkUncompressedBytes,
    ChunkDecompressionRatio,
    ExactOutputDecompressor,
    ChunkRecordCount,
    ChunkMessageCount,
    ValidationAndDispatchCounts,
    ManifestDenseValidationPlan,
    DecoderGroupAndAggregateBounds,
    PartitionOutputBytes,
    PartitionDerivedRoots,
    SourceRecordDerivedOrdinal,
    WindowIntervalHits,
    SelectedChannelGroups,
    SelectedMembershipAndAssignment,
    PartitionRootAndOriginBounds,
    PlanningCrossProduct,
    SessionPartitionFormula,
    SessionDescriptorOriginFormula,
    RegistrationMetadataHeadroom,
    GenerationPartitions,
    GenerationTerminalRoots,
    SessionRegisteredPartitions,
    SessionCompleteEmptyEntries,
    SessionRootDescriptors,
    SessionExternalOriginBytes,
    SessionResidentPhysicalRoots,
    ConcurrentRangeRequests,
    InFlightRangeBytes,
    RangeRetry,
    RangeByobOverlap,
    RemoteValidatorByteString,
    ExtensionlessSniffer,
    OpenAdmission,
    OpenSourceIndexAndCatalog,
    TerminalStatusRegistry,
    DisarmedTerminalClaims,
    PublicLifecycle,
    PresentationQueryOwnership,
    ChromePageExecution,
    MetadataOpeningAndCpu,
    RawCache,
    OpeningValidationDispatch,
    MessageIndexParse,
    ByobPump,
    AddChunk,
    InsertionFrame,
    RemoteGc,
    MemoryPressure,
    BackfillRangeRequests,
    BackfillMessageIndexBytes,
    BackfillParsedEntries,
    BackfillDeadline,
    AccountingScopeOwnership,
    ExternalStrings,
    ExistingIdentifierIndex,
    RuntimeInternBudget,
    RemoteInternalTotal,
    AccountingSelfOwnership,
}

/// Whether a value is a runtime hard limit or a non-preemptive acceptance threshold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WebRemoteLimitEnforcement {
    HardLimit,
    TelemetryAcceptanceThreshold,
}

/// The implementation milestone that must produce a measured value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MeasurementStage {
    PhaseAReleaseWasmChrome,
    PhaseBAdversarialHarness,
}

/// The only acceptable kind of evidence for a measured value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EvidenceKind {
    ReleaseWasmChromeBenchmark,
    AdversarialResourceHarness,
}

/// A stable design reference for a statically frozen value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DesignConstant {
    pub value: NonZeroU64,
    pub reference: &'static str,
}

/// The source that must provide a limit value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebRemoteLimitRequirement {
    DesignConstant(DesignConstant),
    Measurement {
        stage: MeasurementStage,
        evidence: EvidenceKind,
    },
}

/// Static metadata for one limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WebRemoteLimitDefinition {
    pub key: WebRemoteLimitKey,
    pub stable_name: &'static str,
    pub profile_version: WebRemoteLimitsProfileVersion,
    pub domain: WebRemoteLimitDomain,
    pub unit: WebRemoteLimitUnit,
    pub scope: WebRemoteLimitScope,
    pub accounting: WebRemoteAccountingKind,
    pub telemetry: WebRemoteLimitTelemetry,
    pub enforcement: WebRemoteLimitEnforcement,
    pub design_requirement: NormativeResourceRequirement,
    pub requirement: WebRemoteLimitRequirement,
}

const fn design_constant(value: u64, reference: &'static str) -> WebRemoteLimitRequirement {
    let Some(value) = NonZeroU64::new(value) else {
        panic!("a design constant must be non-zero");
    };
    WebRemoteLimitRequirement::DesignConstant(DesignConstant { value, reference })
}

const fn phase_a() -> WebRemoteLimitRequirement {
    WebRemoteLimitRequirement::Measurement {
        stage: MeasurementStage::PhaseAReleaseWasmChrome,
        evidence: EvidenceKind::ReleaseWasmChromeBenchmark,
    }
}

const fn phase_b() -> WebRemoteLimitRequirement {
    WebRemoteLimitRequirement::Measurement {
        stage: MeasurementStage::PhaseBAdversarialHarness,
        evidence: EvidenceKind::AdversarialResourceHarness,
    }
}

macro_rules! define_web_remote_limits {
    ($(
        $variant:ident => {
            name: $name:literal,
            domain: $domain:ident,
            unit: $unit:ident,
            scope: $scope:ident,
            accounting: $accounting:ident,
            telemetry: $telemetry:ident,
            requirement: $requirement:expr
        }
    ),+ $(,)?) => {
        /// Stable keys in the V1 Web remote limits schema.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[repr(u16)]
        pub enum WebRemoteLimitKey {
            $($variant),+
        }

        const WEB_REMOTE_LIMIT_COUNT: usize = [$(stringify!($variant)),+].len();

        impl WebRemoteLimitKey {
            pub const ALL: [Self; WEB_REMOTE_LIMIT_COUNT] = [$(Self::$variant),+];

            pub const fn count() -> usize {
                WEB_REMOTE_LIMIT_COUNT
            }

            pub const fn definition(self) -> WebRemoteLimitDefinition {
                match self {
                    $(Self::$variant => WebRemoteLimitDefinition {
                        key: Self::$variant,
                        stable_name: $name,
                        profile_version: WebRemoteLimitsProfileVersion::V1,
                        domain: WebRemoteLimitDomain::$domain,
                        unit: WebRemoteLimitUnit::$unit,
                        scope: WebRemoteLimitScope::$scope,
                        accounting: WebRemoteAccountingKind::$accounting,
                        telemetry: WebRemoteLimitTelemetry::$telemetry,
                        enforcement: Self::$variant.enforcement(),
                        design_requirement: Self::$variant.design_requirement(),
                        requirement: $requirement,
                    }),+
                }
            }

            const fn index(self) -> usize {
                self as usize
            }
        }
    };
}

define_web_remote_limits! {
    AccountingScopeNodesGlobal => { name: "accounting_scope_nodes_global", domain: ScopeAccounting, unit: Count, scope: WasmModuleLifetime, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    AccountingScopeNodeBytesGlobal => { name: "accounting_scope_node_bytes_global", domain: ScopeAccounting, unit: Bytes, scope: WasmModuleLifetime, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    AccountingChildScopeNodesPerParent => { name: "accounting_child_scope_nodes_per_parent", domain: ScopeAccounting, unit: Count, scope: AccountingScopeNode, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    AccountingChildScopeBytesPerParent => { name: "accounting_child_scope_bytes_per_parent", domain: ScopeAccounting, unit: Bytes, scope: AccountingScopeNode, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    AccountingScopeNodeBytes => { name: "accounting_scope_node_bytes", domain: ScopeAccounting, unit: Bytes, scope: AccountingScopeNode, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_b() },
    AccountingPreparedReservationRecords => { name: "accounting_prepared_reservation_records", domain: ScopeAccounting, unit: Count, scope: WasmModuleLifetime, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    AccountingActiveReservationRecords => { name: "accounting_active_reservation_records", domain: ScopeAccounting, unit: Count, scope: WasmModuleLifetime, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    AccountingReservationRequestEntries => { name: "accounting_reservation_request_entries", domain: ScopeAccounting, unit: Count, scope: WasmModuleLifetime, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    AccountingUsageNodes => { name: "accounting_usage_nodes", domain: ScopeAccounting, unit: Count, scope: WasmModuleLifetime, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    AccountingPreparedReservationRetainedBytes => { name: "accounting_prepared_reservation_retained_bytes", domain: ScopeAccounting, unit: Bytes, scope: WasmModuleLifetime, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    AccountingActiveReservationRetainedBytes => { name: "accounting_active_reservation_retained_bytes", domain: ScopeAccounting, unit: Bytes, scope: WasmModuleLifetime, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    AccountingReservationRequestRetainedBytes => { name: "accounting_reservation_request_retained_bytes", domain: ScopeAccounting, unit: Bytes, scope: WasmModuleLifetime, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    AccountingUsageNodeRetainedBytes => { name: "accounting_usage_node_retained_bytes", domain: ScopeAccounting, unit: Bytes, scope: WasmModuleLifetime, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    AccountingPreparedReservationRecordBytes => { name: "accounting_prepared_reservation_record_bytes", domain: ScopeAccounting, unit: Bytes, scope: WasmModuleLifetime, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_b() },
    AccountingActiveReservationRecordBytes => { name: "accounting_active_reservation_record_bytes", domain: ScopeAccounting, unit: Bytes, scope: WasmModuleLifetime, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_b() },
    AccountingReservationRequestEntryBytes => { name: "accounting_reservation_request_entry_bytes", domain: ScopeAccounting, unit: Bytes, scope: WasmModuleLifetime, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_b() },
    AccountingUsageNodeBytes => { name: "accounting_usage_node_bytes", domain: ScopeAccounting, unit: Bytes, scope: WasmModuleLifetime, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_b() },
    AccountingReleaseScratchEntries => { name: "accounting_release_scratch_entries", domain: ScopeAccounting, unit: Count, scope: WasmModuleLifetime, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    AccountingReleaseScratchRetainedBytes => { name: "accounting_release_scratch_retained_bytes", domain: ScopeAccounting, unit: Bytes, scope: WasmModuleLifetime, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    AccountingReleaseScratchEntryBytes => { name: "accounting_release_scratch_entry_bytes", domain: ScopeAccounting, unit: Bytes, scope: WasmModuleLifetime, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_b() },

    SummaryBytes => { name: "summary_bytes", domain: Summary, unit: Bytes, scope: Summary, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_a() },
    SummaryRecordCount => { name: "summary_record_count", domain: Summary, unit: Records, scope: Summary, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    SummarySchemaCount => { name: "summary_schema_count", domain: Summary, unit: Count, scope: Summary, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    SummaryChannelCount => { name: "summary_channel_count", domain: Summary, unit: Count, scope: Summary, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    SummaryChunkIndexCount => { name: "summary_chunk_index_count", domain: Summary, unit: Count, scope: Summary, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    PhysicalRegionDescriptorCount => { name: "physical_region_descriptor_count", domain: Summary, unit: Count, scope: Summary, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },

    ChannelMetadataEntriesPerRecord => { name: "channel_metadata_entries_per_record", domain: NestedCollection, unit: Count, scope: NestedRecord, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_a() },
    ChannelMetadataKeyBytes => { name: "channel_metadata_key_bytes", domain: NestedCollection, unit: Bytes, scope: NestedRecord, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_a() },
    ChannelMetadataValueBytes => { name: "channel_metadata_value_bytes", domain: NestedCollection, unit: Bytes, scope: NestedRecord, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_a() },
    NestedRetainedBytesPerSummary => { name: "nested_retained_bytes_per_summary", domain: NestedCollection, unit: Bytes, scope: Summary, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    NestedRetainedBytesPerChunkScan => { name: "nested_retained_bytes_per_chunk_scan", domain: NestedCollection, unit: Bytes, scope: Chunk, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    ChunkIndexMessageIndexOffsets => { name: "chunk_index_message_index_offsets", domain: NestedCollection, unit: Count, scope: NestedRecord, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_a() },
    StatisticsChannelMessageCounts => { name: "statistics_channel_message_counts", domain: NestedCollection, unit: Count, scope: NestedRecord, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_a() },
    MessageIndexEntriesPerRecord => { name: "message_index_entries_per_record", domain: NestedCollection, unit: Count, scope: NestedRecord, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_a() },
    MessageIndexRegionBytes => { name: "message_index_region_bytes", domain: NestedCollection, unit: Bytes, scope: MessageIndexRegion, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_a() },
    MessageIndexRegionRecordCount => { name: "message_index_region_record_count", domain: NestedCollection, unit: Records, scope: MessageIndexRegion, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    MessageIndexRegionEntryCount => { name: "message_index_region_entry_count", domain: NestedCollection, unit: Count, scope: MessageIndexRegion, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    AmbiguousZeroMessageIndexBytes => { name: "ambiguous_zero_message_index_bytes", domain: Opening, unit: Bytes, scope: Source, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    AmbiguousZeroMessageIndexEntries => { name: "ambiguous_zero_message_index_entries", domain: Opening, unit: Count, scope: Source, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    AmbiguousZeroMessageIndexRanges => { name: "ambiguous_zero_message_index_ranges", domain: Opening, unit: RangeRequests, scope: Source, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    AmbiguousZeroChunkCompressedBytes => { name: "ambiguous_zero_chunk_compressed_bytes", domain: Opening, unit: Bytes, scope: Source, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    AmbiguousZeroChunkUncompressedBytes => { name: "ambiguous_zero_chunk_uncompressed_bytes", domain: Opening, unit: Bytes, scope: Source, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    AmbiguousZeroChunkRanges => { name: "ambiguous_zero_chunk_ranges", domain: Opening, unit: RangeRequests, scope: Source, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },

    ChunkCompressedBytes => { name: "chunk_compressed_bytes", domain: Chunk, unit: Bytes, scope: Chunk, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_a() },
    ChunkUncompressedBytes => { name: "chunk_uncompressed_bytes", domain: Chunk, unit: Bytes, scope: Chunk, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_a() },
    ChunkDecompressionRatioPermille => { name: "chunk_decompression_ratio_permille", domain: Chunk, unit: RatioPermille, scope: Chunk, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_a() },
    DecompressorOverflowScratchBytes => { name: "decompressor_overflow_scratch_bytes", domain: Chunk, unit: Bytes, scope: WorkUnit, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: design_constant(1, "section 11.2 exact-output overflow detection scratch") },
    ChunkRecordCount => { name: "chunk_record_count", domain: Chunk, unit: Records, scope: Chunk, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    ChunkMessageCount => { name: "chunk_message_count", domain: Chunk, unit: Messages, scope: Chunk, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    SelectedMessageDispatchesPerScan => { name: "selected_message_dispatches_per_scan", domain: Chunk, unit: Messages, scope: Chunk, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    ManifestDenseChannelCount => { name: "manifest_dense_channel_count", domain: Chunk, unit: Count, scope: Channel, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    ManifestDensePayloadPlanBytes => { name: "manifest_dense_payload_plan_bytes", domain: Chunk, unit: Bytes, scope: Chunk, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    ValidationPlanEntries => { name: "validation_plan_entries", domain: Chunk, unit: Count, scope: Chunk, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    ValidationPlanRetainedBytes => { name: "validation_plan_retained_bytes", domain: Chunk, unit: Bytes, scope: Chunk, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_a() },
    DecompressedChunkRetainedBytes => { name: "decompressed_chunk_retained_bytes", domain: Chunk, unit: Bytes, scope: Chunk, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_a() },
    SourceRecordDerivedOrdinal => { name: "source_record_derived_ordinal", domain: Chunk, unit: Count, scope: Chunk, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_a() },

    DecoderWorkingBytes => { name: "decoder_working_bytes", domain: Decoder, unit: Bytes, scope: DecoderGroupPerChunk, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_a() },
    DecoderBuilderBytes => { name: "decoder_builder_bytes", domain: Decoder, unit: Bytes, scope: DecoderGroupPerChunk, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_a() },
    DecoderPayloadScratchBytes => { name: "decoder_payload_scratch_bytes", domain: Decoder, unit: Bytes, scope: DecoderGroupPerChunk, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_a() },
    DecoderLensIntermediateBytes => { name: "decoder_lens_intermediate_bytes", domain: Decoder, unit: Bytes, scope: DecoderGroupPerChunk, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_a() },
    DecoderTerminalOutputBytes => { name: "decoder_terminal_output_bytes", domain: Decoder, unit: Bytes, scope: DecoderGroupPerChunk, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_a() },
    DecoderDerivedRows => { name: "decoder_derived_rows", domain: Decoder, unit: Rows, scope: DecoderGroupPerChunk, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    DecoderGenerationWorkingBytes => { name: "decoder_generation_working_bytes", domain: Decoder, unit: Bytes, scope: Generation, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_a() },
    DecoderChunkWorkingBytes => { name: "decoder_chunk_working_bytes", domain: Decoder, unit: Bytes, scope: Chunk, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_a() },
    DecoderSessionWorkingBytes => { name: "decoder_session_working_bytes", domain: Decoder, unit: Bytes, scope: Session, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_a() },
    DecoderGlobalWorkingBytes => { name: "decoder_global_working_bytes", domain: Decoder, unit: Bytes, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_a() },
    DecoderGenerationOutputRows => { name: "decoder_generation_output_rows", domain: Decoder, unit: Rows, scope: Generation, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_a() },
    DecoderChunkOutputRows => { name: "decoder_chunk_output_rows", domain: Decoder, unit: Rows, scope: Chunk, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_a() },
    DecoderSessionOutputRows => { name: "decoder_session_output_rows", domain: Decoder, unit: Rows, scope: Session, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_a() },
    DecoderGlobalOutputRows => { name: "decoder_global_output_rows", domain: Decoder, unit: Rows, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_a() },
    PartitionOutputBytes => { name: "partition_output_bytes", domain: Decoder, unit: Bytes, scope: Partition, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_a() },
    PartitionDerivedRoots => { name: "partition_derived_roots", domain: Decoder, unit: Count, scope: Partition, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },

    WindowChunkHits => { name: "window_chunk_hits", domain: Planning, unit: Count, scope: Window, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_b() },
    SelectedChannelGroups => { name: "selected_channel_groups", domain: Planning, unit: Count, scope: Window, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_b() },
    SelectedChannelGroupMemberships => { name: "selected_channel_group_memberships", domain: Planning, unit: Count, scope: Window, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_b() },
    SelectedChannelAssignmentsPerChannel => { name: "selected_channel_assignments_per_channel", domain: Planning, unit: Count, scope: Channel, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: design_constant(1, "section 8.3 selected temporal Channel has exactly one owner") },
    PlanningCrossProductOperations => { name: "planning_cross_product_operations", domain: Planning, unit: Operations, scope: Generation, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_b() },
    GenerationPartitions => { name: "generation_partitions", domain: Planning, unit: Count, scope: Generation, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_b() },
    GenerationTerminalBatches => { name: "generation_terminal_batches", domain: Planning, unit: Count, scope: Generation, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_b() },
    GenerationDerivedRoots => { name: "generation_derived_roots", domain: Planning, unit: Count, scope: Generation, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_b() },
    GenerationRootDescriptors => { name: "generation_root_descriptors", domain: Planning, unit: Count, scope: Generation, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_b() },

    RootsPerPartition => { name: "roots_per_partition", domain: Registration, unit: Count, scope: Partition, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_b() },
    ExternalOriginBytesPerPartition => { name: "external_origin_bytes_per_partition", domain: Registration, unit: Bytes, scope: Partition, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_b() },
    SessionRegisteredPartitions => { name: "session_registered_partitions", domain: Registration, unit: Count, scope: Session, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_b() },
    SessionCompleteEmptyEntries => { name: "session_complete_empty_entries", domain: Registration, unit: Count, scope: Session, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_b() },
    SessionRootDescriptors => { name: "session_root_descriptors", domain: Registration, unit: Count, scope: Session, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_b() },
    SessionExternalOriginBytes => { name: "session_external_origin_bytes", domain: Registration, unit: Bytes, scope: Session, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_b() },
    SessionResidentPhysicalRoots => { name: "session_resident_physical_roots", domain: Registration, unit: Count, scope: Session, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    SessionRegistrationMetadataBytes => { name: "session_registration_metadata_bytes", domain: Registration, unit: Bytes, scope: Session, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    RemoteInternalRetainedBytes => { name: "remote_internal_retained_bytes", domain: Registration, unit: Bytes, scope: Session, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    RemoteStagedTerminalBatchBytes => { name: "remote_staged_terminal_batch_bytes", domain: Registration, unit: Bytes, scope: Generation, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    RemotePinnedResidentBytes => { name: "remote_pinned_resident_bytes", domain: Registration, unit: Bytes, scope: Session, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    RemoteEntityDbRetainedBytes => { name: "remote_entity_db_retained_bytes", domain: Registration, unit: Bytes, scope: Store, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    RemoteStoreResidentBytes => { name: "remote_store_resident_bytes", domain: Registration, unit: Bytes, scope: Store, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    FetchWasmRawRetainedBytes => { name: "fetch_wasm_raw_retained_bytes", domain: Transport, unit: Bytes, scope: RangeResponse, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },

    ConcurrentRangeRequests => { name: "concurrent_range_requests", domain: Transport, unit: Count, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_a() },
    InFlightRangeBytes => { name: "in_flight_range_bytes", domain: Transport, unit: Bytes, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_a() },
    RangeRetryAttemptsPerOperation => { name: "range_retry_attempts_per_operation", domain: Transport, unit: Count, scope: Operation, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    RequestedRangeBytes => { name: "requested_range_bytes", domain: Transport, unit: Bytes, scope: RangeResponse, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_a() },
    ByobScratchBytes => { name: "byob_scratch_bytes", domain: Transport, unit: Bytes, scope: RangeResponse, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_a() },
    RangeJsWasmOverlapBytes => { name: "range_js_wasm_overlap_bytes", domain: Transport, unit: Bytes, scope: RangeResponse, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_a() },
    RemoteValidatorIngressValues => { name: "remote_validator_ingress_values", domain: Transport, unit: Count, scope: WorkUnit, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: design_constant(1, "MCAP-008 absent-or-single entity-tag header grammar") },
    RemoteValidatorIngressWireBytes => { name: "remote_validator_ingress_wire_bytes", domain: Transport, unit: Bytes, scope: WorkUnit, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_a() },
    RemoteValidatorIngressJsWasmOverlapBytes => { name: "remote_validator_ingress_js_wasm_overlap_bytes", domain: Transport, unit: Bytes, scope: WorkUnit, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_a() },
    RemoteValidatorIngressScratchBytes => { name: "remote_validator_ingress_scratch_bytes", domain: Transport, unit: Bytes, scope: WorkUnit, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_a() },
    RemoteValidatorRetainedBytes => { name: "remote_validator_retained_bytes", domain: Transport, unit: Bytes, scope: Session, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_a() },
    RemoteValidatorEgressValues => { name: "remote_validator_egress_values", domain: Transport, unit: Count, scope: RangeResponse, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: design_constant(1, "MCAP-008 one-shot If-Match owner per RangeResponse") },
    RemoteValidatorEgressWireBytes => { name: "remote_validator_egress_wire_bytes", domain: Transport, unit: Bytes, scope: WorkUnit, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_a() },
    RemoteValidatorEgressJsWasmOverlapBytes => { name: "remote_validator_egress_js_wasm_overlap_bytes", domain: Transport, unit: Bytes, scope: WorkUnit, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_a() },
    RemoteValidatorEgressScratchBytes => { name: "remote_validator_egress_scratch_bytes", domain: Transport, unit: Bytes, scope: WorkUnit, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_a() },
    ExtensionlessSnifferBufferBytes => { name: "extensionless_sniffer_buffer_bytes", domain: Transport, unit: Bytes, scope: Request, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: design_constant(8, "sections 5.1 and 9.1 extensionless application-visible prefix") },
    ByobPumpSliceBytes => { name: "byob_pump_slice_bytes", domain: Transport, unit: Bytes, scope: WorkUnit, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_a() },
    ByobPumpSliceDurationMicros => { name: "byob_pump_slice_duration_micros", domain: Transport, unit: Microseconds, scope: WorkUnit, accounting: ScalarObservation, telemetry: DeadlineOutcome, requirement: phase_a() },

    MetadataOpeningBytes => { name: "metadata_opening_bytes", domain: Opening, unit: Bytes, scope: Session, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_a() },
    MetadataOpeningRangeRequests => { name: "metadata_opening_range_requests", domain: Opening, unit: RangeRequests, scope: Source, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    MetadataOpeningVisibleDeadlineMillis => { name: "metadata_opening_visible_deadline_millis", domain: Opening, unit: Milliseconds, scope: Source, accounting: ScalarObservation, telemetry: DeadlineOutcome, requirement: phase_a() },
    RemoteCpuWorkUnitsPerFrame => { name: "remote_cpu_work_units_per_frame", domain: Opening, unit: Count, scope: Frame, accounting: ScalarObservation, telemetry: HighWatermark, requirement: design_constant(1, "section 11.5 remote CPU work observation") },
    OpeningParseInputBytes => { name: "opening_parse_input_bytes", domain: Opening, unit: Bytes, scope: WorkUnit, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_a() },
    OpeningParseOutputBytes => { name: "opening_parse_output_bytes", domain: Opening, unit: Bytes, scope: WorkUnit, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_a() },
    OpeningParseRecordCount => { name: "opening_parse_record_count", domain: Opening, unit: Records, scope: WorkUnit, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    OpeningParseMessageCount => { name: "opening_parse_message_count", domain: Opening, unit: Messages, scope: WorkUnit, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    OpeningParseDispatchCount => { name: "opening_parse_dispatch_count", domain: Opening, unit: Messages, scope: WorkUnit, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    OpeningParseDurationMicros => { name: "opening_parse_duration_micros", domain: Opening, unit: Microseconds, scope: WorkUnit, accounting: ScalarObservation, telemetry: DeadlineOutcome, requirement: phase_a() },
    ChunkValidationInputBytes => { name: "chunk_validation_input_bytes", domain: Opening, unit: Bytes, scope: WorkUnit, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_a() },
    ChunkValidationOutputBytes => { name: "chunk_validation_output_bytes", domain: Opening, unit: Bytes, scope: WorkUnit, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_a() },
    ChunkValidationRecordCount => { name: "chunk_validation_record_count", domain: Opening, unit: Records, scope: WorkUnit, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    ChunkValidationMessageCount => { name: "chunk_validation_message_count", domain: Opening, unit: Messages, scope: WorkUnit, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    ChunkValidationDispatchCount => { name: "chunk_validation_dispatch_count", domain: Opening, unit: Messages, scope: WorkUnit, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    ChunkValidationDurationMicros => { name: "chunk_validation_duration_micros", domain: Opening, unit: Microseconds, scope: WorkUnit, accounting: ScalarObservation, telemetry: DeadlineOutcome, requirement: phase_a() },
    ChunkDispatchInputBytes => { name: "chunk_dispatch_input_bytes", domain: Opening, unit: Bytes, scope: WorkUnit, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_a() },
    ChunkDispatchOutputBytes => { name: "chunk_dispatch_output_bytes", domain: Opening, unit: Bytes, scope: WorkUnit, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_a() },
    ChunkDispatchRecordCount => { name: "chunk_dispatch_record_count", domain: Opening, unit: Records, scope: WorkUnit, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    ChunkDispatchMessageCount => { name: "chunk_dispatch_message_count", domain: Opening, unit: Messages, scope: WorkUnit, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    ChunkDispatchActualAppendCount => { name: "chunk_dispatch_actual_append_count", domain: Opening, unit: Count, scope: WorkUnit, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    ChunkDispatchDurationMicros => { name: "chunk_dispatch_duration_micros", domain: Opening, unit: Microseconds, scope: WorkUnit, accounting: ScalarObservation, telemetry: DeadlineOutcome, requirement: phase_a() },
    MessageIndexParseDurationMicros => { name: "message_index_parse_duration_micros", domain: Opening, unit: Microseconds, scope: WorkUnit, accounting: ScalarObservation, telemetry: DeadlineOutcome, requirement: phase_a() },

    RemoteSessionSlots => { name: "remote_session_slots", domain: OpenRegistry, unit: Count, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: design_constant(1, "section 8.5 singleton RemoteMcapSlot") },
    OpenBatchUrls => { name: "open_batch_urls", domain: OpenRegistry, unit: Count, scope: AtomicBatch, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_b() },
    PendingSecretOpenBytes => { name: "pending_secret_open_bytes", domain: OpenRegistry, unit: Bytes, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    PendingOpenOptionsBytes => { name: "pending_open_options_bytes", domain: OpenRegistry, unit: Bytes, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    PendingExtensionlessPrepared => { name: "pending_extensionless_prepared", domain: OpenRegistry, unit: Count, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    PendingFormatSniffs => { name: "pending_format_sniffs", domain: OpenRegistry, unit: Count, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    DisarmedStrictSources => { name: "disarmed_strict_sources", domain: OpenRegistry, unit: Count, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    DisarmedHandoffRetainedBytes => { name: "disarmed_handoff_retained_bytes", domain: OpenRegistry, unit: Bytes, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    DisarmedHandoffDescriptors => { name: "disarmed_handoff_descriptors", domain: OpenRegistry, unit: Count, scope: AtomicBatch, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_b() },
    DisarmedHandoffOperations => { name: "disarmed_handoff_operations", domain: OpenRegistry, unit: Count, scope: AtomicBatch, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_b() },
    AtomicOpenReservationBytes => { name: "atomic_open_reservation_bytes", domain: OpenRegistry, unit: Bytes, scope: AtomicBatch, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    UrlFingerprintBuckets => { name: "url_fingerprint_buckets", domain: OpenRegistry, unit: Count, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    UrlFingerprintTokens => { name: "url_fingerprint_tokens", domain: OpenRegistry, unit: Count, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    UrlFingerprintTokensPerBucket => { name: "url_fingerprint_tokens_per_bucket", domain: OpenRegistry, unit: Count, scope: UrlFingerprintBucket, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_b() },
    UrlCanonicalMatchBytes => { name: "url_canonical_match_bytes", domain: OpenRegistry, unit: Bytes, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    UrlAliasDescriptors => { name: "url_alias_descriptors", domain: OpenRegistry, unit: Count, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    UrlFingerprintIndexRetainedBytes => { name: "url_fingerprint_index_retained_bytes", domain: OpenRegistry, unit: Bytes, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    UrlAliasDescriptorRetainedBytes => { name: "url_alias_descriptor_retained_bytes", domain: OpenRegistry, unit: Bytes, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    PreexistingRecordingAttachments => { name: "preexisting_recording_attachments", domain: OpenRegistry, unit: Count, scope: AtomicBatch, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_b() },
    PreexistingRecordingAttachmentBytes => { name: "preexisting_recording_attachment_bytes", domain: OpenRegistry, unit: Bytes, scope: AtomicBatch, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_b() },
    InstallationAcks => { name: "installation_acks", domain: OpenRegistry, unit: Count, scope: AtomicBatch, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_b() },
    InstallationAckRetainedBytes => { name: "installation_ack_retained_bytes", domain: OpenRegistry, unit: Bytes, scope: AtomicBatch, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_b() },
    CatalogEntries => { name: "catalog_entries", domain: OpenRegistry, unit: Count, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    CatalogProjectionBytes => { name: "catalog_projection_bytes", domain: OpenRegistry, unit: Bytes, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    CatalogRows => { name: "catalog_rows", domain: OpenRegistry, unit: Rows, scope: Source, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_b() },
    CatalogStringBytes => { name: "catalog_string_bytes", domain: OpenRegistry, unit: Bytes, scope: Source, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_b() },
    SemanticConfigRetainedBytes => { name: "semantic_config_retained_bytes", domain: OpenRegistry, unit: Bytes, scope: Source, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    OwnedRemoteClientEntries => { name: "owned_remote_client_entries", domain: OpenRegistry, unit: Count, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: design_constant(1, "section 20 Viewer remote client registry owned entry upper bound") },
    OpenSourceStatusSlots => { name: "open_source_status_slots", domain: OpenRegistry, unit: Count, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    OpenSourceTerminalEntries => { name: "open_source_terminal_entries", domain: OpenRegistry, unit: Count, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    OpenSourceTerminalRetainedBytes => { name: "open_source_terminal_retained_bytes", domain: OpenRegistry, unit: Bytes, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    OpenSourceStatusRetainedBytes => { name: "open_source_status_retained_bytes", domain: OpenRegistry, unit: Bytes, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    OpenSourceLiveStatusOwners => { name: "open_source_live_status_owners", domain: OpenRegistry, unit: Count, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    RemoteTerminalStatusBytes => { name: "remote_terminal_status_bytes", domain: OpenRegistry, unit: Bytes, scope: Source, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_b() },
    RemoteTerminalDiagnostics => { name: "remote_terminal_diagnostics", domain: OpenRegistry, unit: Count, scope: Source, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_b() },
    DisarmedStatusClaims => { name: "disarmed_status_claims", domain: OpenRegistry, unit: Count, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    DeferredEvictionVictims => { name: "deferred_eviction_victims", domain: OpenRegistry, unit: Count, scope: AtomicBatch, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_b() },
    DeferredEvictionDescriptorBytes => { name: "deferred_eviction_descriptor_bytes", domain: OpenRegistry, unit: Bytes, scope: AtomicBatch, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },

    OpenSourceOwners => { name: "open_source_owners", domain: Lifecycle, unit: Count, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    OpenSourceOwnerRetainedBytes => { name: "open_source_owner_retained_bytes", domain: Lifecycle, unit: Bytes, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    OpenOperations => { name: "open_operations", domain: Lifecycle, unit: Count, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    OpenOperationRetainedBytes => { name: "open_operation_retained_bytes", domain: Lifecycle, unit: Bytes, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    PublicRecordingHandles => { name: "public_recording_handles", domain: Lifecycle, unit: Count, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    PublicRecordingHandleRetainedBytes => { name: "public_recording_handle_retained_bytes", domain: Lifecycle, unit: Bytes, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    RecordingsPerSource => { name: "recordings_per_source", domain: Lifecycle, unit: Count, scope: Source, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_b() },
    OperationRecordingSubscriptions => { name: "operation_recording_subscriptions", domain: Lifecycle, unit: Count, scope: Operation, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    OperationRecordingSubscriptionRetainedBytes => { name: "operation_recording_subscription_retained_bytes", domain: Lifecycle, unit: Bytes, scope: Operation, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    LifecycleSubscribers => { name: "lifecycle_subscribers", domain: Lifecycle, unit: Count, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    LifecycleSubscriberRetainedBytes => { name: "lifecycle_subscriber_retained_bytes", domain: Lifecycle, unit: Bytes, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    LifecycleHubs => { name: "lifecycle_hubs", domain: Lifecycle, unit: Count, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    LifecycleHubRetainedBytes => { name: "lifecycle_hub_retained_bytes", domain: Lifecycle, unit: Bytes, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    LifecycleChildStates => { name: "lifecycle_child_states", domain: Lifecycle, unit: Count, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    LifecycleChildStateBytes => { name: "lifecycle_child_state_bytes", domain: Lifecycle, unit: Bytes, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    ActivationDeliveryAcks => { name: "activation_delivery_acks", domain: Lifecycle, unit: Count, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    ActivationDeliveryAckBytes => { name: "activation_delivery_ack_bytes", domain: Lifecycle, unit: Bytes, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    LatestLifecycleSnapshots => { name: "latest_lifecycle_snapshots", domain: Lifecycle, unit: Count, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    LatestLifecycleSnapshotBytes => { name: "latest_lifecycle_snapshot_bytes", domain: Lifecycle, unit: Bytes, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    LifecycleOutstandingDeliveries => { name: "lifecycle_outstanding_deliveries", domain: Lifecycle, unit: Count, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    LifecycleOutstandingDeliveryBytes => { name: "lifecycle_outstanding_delivery_bytes", domain: Lifecycle, unit: Bytes, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    LifecycleListenerErrorCredits => { name: "lifecycle_listener_error_credits", domain: Lifecycle, unit: Count, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    SingleLifecycleEventBytes => { name: "single_lifecycle_event_bytes", domain: Lifecycle, unit: Bytes, scope: IngressItem, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_b() },
    TypescriptDispatcherItems => { name: "typescript_dispatcher_items", domain: Lifecycle, unit: Count, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    TypescriptDispatcherRetainedBytes => { name: "typescript_dispatcher_retained_bytes", domain: Lifecycle, unit: Bytes, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    LifecycleScheduledTasks => { name: "lifecycle_scheduled_tasks", domain: Lifecycle, unit: Count, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: design_constant(1, "section 8.5.1 single scheduled lifecycle task") },
    LifecycleCurrentlyDispatching => { name: "lifecycle_currently_dispatching", domain: Lifecycle, unit: Count, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: design_constant(1, "section 8.5.1 single currently-dispatching item") },
    ListenerErrorNotificationsPerEvent => { name: "listener_error_notifications_per_event", domain: Lifecycle, unit: Count, scope: IngressItem, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: design_constant(1, "section 8.5.1 at most one aggregate listener-error item") },
    PromiseClosureBytes => { name: "promise_closure_bytes", domain: Lifecycle, unit: Bytes, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    PromiseClosureBytesPerOperation => { name: "promise_closure_bytes_per_operation", domain: Lifecycle, unit: Bytes, scope: Operation, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_b() },
    WrapperCacheEntries => { name: "wrapper_cache_entries", domain: Lifecycle, unit: Count, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    WrapperCacheRetainedBytes => { name: "wrapper_cache_retained_bytes", domain: Lifecycle, unit: Bytes, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    RemovedTombstoneRetainers => { name: "removed_tombstone_retainers", domain: Lifecycle, unit: Count, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    RemovedTombstoneRetainedBytes => { name: "removed_tombstone_retained_bytes", domain: Lifecycle, unit: Bytes, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },

    PresentationFacades => { name: "presentation_facades", domain: Presentation, unit: Count, scope: Source, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: design_constant(1, "one sealed presentation facade per active remote source") },
    PresentationFacadeRetainedBytes => { name: "presentation_facade_retained_bytes", domain: Presentation, unit: Bytes, scope: Source, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    PresentationSnapshots => { name: "presentation_snapshots", domain: Presentation, unit: Count, scope: Source, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: design_constant(1, "one current immutable query snapshot per active remote source") },
    PresentationSnapshotRetainedBytes => { name: "presentation_snapshot_retained_bytes", domain: Presentation, unit: Bytes, scope: Source, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    PresentationActiveLeases => { name: "presentation_active_leases", domain: Presentation, unit: Count, scope: Source, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    PresentationActiveLeaseRetainedBytes => { name: "presentation_active_lease_retained_bytes", domain: Presentation, unit: Bytes, scope: Source, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },


    PageLifecycleListeners => { name: "page_lifecycle_listeners", domain: PageExecution, unit: Count, scope: PageExecution, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    PageHiddenWakeFlags => { name: "page_hidden_wake_flags", domain: PageExecution, unit: Count, scope: PageExecution, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    PageParkedResponseOwners => { name: "page_parked_response_owners", domain: PageExecution, unit: Count, scope: PageExecution, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    PageParkedResponseOwnerRetainedBytes => { name: "page_parked_response_owner_retained_bytes", domain: PageExecution, unit: Bytes, scope: PageExecution, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    PageVisibleDeadlineBookkeepingBytes => { name: "page_visible_deadline_bookkeeping_bytes", domain: PageExecution, unit: Bytes, scope: PageExecution, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    PageResumeRevalidationWorkCount => { name: "page_resume_revalidation_work_count", domain: PageExecution, unit: Count, scope: PageExecution, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_b() },
    PageResumeRevalidationDurationMicros => { name: "page_resume_revalidation_duration_micros", domain: PageExecution, unit: Microseconds, scope: PageExecution, accounting: ScalarObservation, telemetry: DeadlineOutcome, requirement: phase_b() },
    PageTeardownCleanupItems => { name: "page_teardown_cleanup_items", domain: PageExecution, unit: Count, scope: PageExecution, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_b() },
    PageTeardownDurationMicros => { name: "page_teardown_duration_micros", domain: PageExecution, unit: Microseconds, scope: PageExecution, accounting: ScalarObservation, telemetry: DeadlineOutcome, requirement: phase_b() },

    RawCacheBytes => { name: "raw_cache_bytes", domain: Cache, unit: Bytes, scope: Session, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },

    ExternalFieldsPerCall => { name: "external_fields_per_call", domain: ExternalString, unit: Count, scope: Request, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_b() },
    ExternalUtf16CodeUnitsPerCall => { name: "external_utf16_code_units_per_call", domain: ExternalString, unit: Utf16CodeUnits, scope: Request, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_b() },
    ExternalUtf8BytesPerCall => { name: "external_utf8_bytes_per_call", domain: ExternalString, unit: Utf8Bytes, scope: Request, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_b() },
    TimelineUtf16CodeUnits => { name: "timeline_utf16_code_units", domain: ExternalString, unit: Utf16CodeUnits, scope: IngressItem, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_b() },
    TimelineUtf8Bytes => { name: "timeline_utf8_bytes", domain: ExternalString, unit: Utf8Bytes, scope: IngressItem, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_b() },
    TopicFilterUtf16CodeUnits => { name: "topic_filter_utf16_code_units", domain: ExternalString, unit: Utf16CodeUnits, scope: Request, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_b() },
    TopicFilterUtf8Bytes => { name: "topic_filter_utf8_bytes", domain: ExternalString, unit: Utf8Bytes, scope: Request, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_b() },
    DecoderSelectorUtf16CodeUnits => { name: "decoder_selector_utf16_code_units", domain: ExternalString, unit: Utf16CodeUnits, scope: Request, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_b() },
    DecoderSelectorUtf8Bytes => { name: "decoder_selector_utf8_bytes", domain: ExternalString, unit: Utf8Bytes, scope: Request, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_b() },
    RouteFieldUtf16CodeUnits => { name: "route_field_utf16_code_units", domain: ExternalString, unit: Utf16CodeUnits, scope: IngressItem, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_b() },
    RouteFieldUtf8Bytes => { name: "route_field_utf8_bytes", domain: ExternalString, unit: Utf8Bytes, scope: IngressItem, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_b() },
    RouteFragmentFields => { name: "route_fragment_fields", domain: ExternalString, unit: Count, scope: IngressItem, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_b() },
    EntityPathParts => { name: "entity_path_parts", domain: ExternalString, unit: Count, scope: IngressItem, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_b() },
    EntityPathUtf8Bytes => { name: "entity_path_utf8_bytes", domain: ExternalString, unit: Utf8Bytes, scope: IngressItem, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_b() },
    ExternalStringCopyCalls => { name: "external_string_copy_calls", domain: ExternalString, unit: Count, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    ExternalStringCopyAllocationBytes => { name: "external_string_copy_allocation_bytes", domain: ExternalString, unit: Bytes, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },


    ExistingTimelineCount => { name: "existing_timeline_count", domain: ExistingIdentifierIndex, unit: Count, scope: Store, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    ExistingEntityPathCount => { name: "existing_entity_path_count", domain: ExistingIdentifierIndex, unit: Count, scope: Store, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    ExistingComponentCount => { name: "existing_component_count", domain: ExistingIdentifierIndex, unit: Count, scope: Store, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    ExistingEntityPathKeyBytes => { name: "existing_entity_path_key_bytes", domain: ExistingIdentifierIndex, unit: Bytes, scope: Store, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    ExistingIdentifierNodeBytes => { name: "existing_identifier_node_bytes", domain: ExistingIdentifierIndex, unit: Bytes, scope: Store, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    ExistingIdentifierViewerBytes => { name: "existing_identifier_viewer_bytes", domain: ExistingIdentifierIndex, unit: Bytes, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },




    RuntimeInternStringBytes => { name: "runtime_intern_string_bytes", domain: RuntimeIntern, unit: Bytes, scope: WasmModuleLifetime, accounting: ModuleLifetimeBurn, telemetry: HighWatermark, requirement: phase_a() },
    RuntimeInternEntryAndCapacityBytes => { name: "runtime_intern_entry_and_capacity_bytes", domain: RuntimeIntern, unit: Bytes, scope: WasmModuleLifetime, accounting: ModuleLifetimeBurn, telemetry: HighWatermark, requirement: phase_a() },
    RuntimeInternCensusIdentifiers => { name: "runtime_intern_census_identifiers", domain: RuntimeIntern, unit: Count, scope: WorkUnit, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    RuntimeInternCensusRetainedBytes => { name: "runtime_intern_census_retained_bytes", domain: RuntimeIntern, unit: Bytes, scope: WorkUnit, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_a() },
    RuntimeInternCandidatePeakBytes => { name: "runtime_intern_candidate_peak_bytes", domain: RuntimeIntern, unit: Bytes, scope: WorkUnit, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_a() },

    AddChunkInputBytes => { name: "add_chunk_input_bytes", domain: StoreMutation, unit: Bytes, scope: WorkUnit, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_a() },
    AddChunkRows => { name: "add_chunk_rows", domain: StoreMutation, unit: Rows, scope: WorkUnit, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_a() },
    AddChunkComponentCount => { name: "add_chunk_component_count", domain: StoreMutation, unit: Count, scope: WorkUnit, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_a() },
    AddChunkTimelineCount => { name: "add_chunk_timeline_count", domain: StoreMutation, unit: Count, scope: WorkUnit, accounting: ScalarObservation, telemetry: MaximumObserved, requirement: phase_a() },
    AddChunkDurationMicros => { name: "add_chunk_duration_micros", domain: StoreMutation, unit: Microseconds, scope: WorkUnit, accounting: ScalarObservation, telemetry: DeadlineOutcome, requirement: phase_a() },
    PendingInsertionBytesPerFrame => { name: "pending_insertion_bytes_per_frame", domain: StoreMutation, unit: Bytes, scope: Frame, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    AddChunkCallsPerFrame => { name: "add_chunk_calls_per_frame", domain: StoreMutation, unit: Count, scope: Frame, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    SharedRemoteWorkUnitsPerFrame => { name: "shared_remote_work_units_per_frame", domain: StoreMutation, unit: Count, scope: Frame, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: design_constant(1, "sections 11.5 and 13 shared remote CPU/GC work-unit ownership") },
    RemoteGcWorkUnitsPerFrame => { name: "remote_gc_work_units_per_frame", domain: StoreMutation, unit: Count, scope: Frame, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_b() },
    RemoteCpuAllowanceMicrosPerFrame => { name: "remote_cpu_allowance_micros_per_frame", domain: StoreMutation, unit: Microseconds, scope: Frame, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    RemoteGcCpuAllowanceMicrosPerFrame => { name: "remote_gc_cpu_allowance_micros_per_frame", domain: StoreMutation, unit: Microseconds, scope: Frame, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    RemoteGcCandidateRoots => { name: "remote_gc_candidate_roots", domain: StoreMutation, unit: Count, scope: Session, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    RemoteGcDurationMicros => { name: "remote_gc_duration_micros", domain: StoreMutation, unit: Microseconds, scope: WorkUnit, accounting: ScalarObservation, telemetry: DeadlineOutcome, requirement: phase_a() },

    MemoryPressurePendingRequests => { name: "memory_pressure_pending_requests", domain: MemoryPressure, unit: Count, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: phase_b() },
    MemoryPressureAllocationPauseStates => { name: "memory_pressure_allocation_pause_states", domain: MemoryPressure, unit: Count, scope: ViewerInstance, accounting: ReclaimableConcurrent, telemetry: HighWatermark, requirement: design_constant(1, "section 20 singleton allocation-pause state per Viewer") },
    MemoryPressureCompletionDeadlineMillis => { name: "memory_pressure_completion_deadline_millis", domain: MemoryPressure, unit: Milliseconds, scope: Request, accounting: ScalarObservation, telemetry: DeadlineOutcome, requirement: phase_b() },
    ProcessSafetyHeadroomBytes => { name: "process_safety_headroom_bytes", domain: MemoryPressure, unit: Bytes, scope: ViewerInstance, accounting: FormulaConstant, telemetry: MaximumObserved, requirement: phase_b() },

    BackfillRangeRequestsPerSeek => { name: "backfill_range_requests_per_seek", domain: Backfill, unit: RangeRequests, scope: Request, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    BackfillMessageIndexBytesPerSeek => { name: "backfill_message_index_bytes_per_seek", domain: Backfill, unit: Bytes, scope: Request, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    BackfillParsedEntriesPerSeek => { name: "backfill_parsed_entries_per_seek", domain: Backfill, unit: Count, scope: Request, accounting: ScalarObservation, telemetry: HighWatermark, requirement: phase_a() },
    BackfillVisibleDeadlineMillis => { name: "backfill_visible_deadline_millis", domain: Backfill, unit: Milliseconds, scope: Request, accounting: ScalarObservation, telemetry: DeadlineOutcome, requirement: phase_a() }
}

/// The reviewed inventory size of the V1 schema.
///
/// This is intentionally independent from the generated array length so adding or removing a key
/// requires an explicit schema review and a matching metadata-fingerprint update.
pub const EXPECTED_WEB_REMOTE_LIMIT_COUNT_V1: usize = 272;

impl WebRemoteLimitKey {
    pub const fn design_requirement(self) -> NormativeResourceRequirement {
        use NormativeResourceRequirement as Requirement;
        match self {
            Self::AccountingScopeNodesGlobal
            | Self::AccountingScopeNodeBytesGlobal
            | Self::AccountingChildScopeNodesPerParent
            | Self::AccountingChildScopeBytesPerParent
            | Self::AccountingScopeNodeBytes => Requirement::AccountingScopeOwnership,
            Self::AccountingPreparedReservationRecords
            | Self::AccountingActiveReservationRecords
            | Self::AccountingReservationRequestEntries
            | Self::AccountingUsageNodes
            | Self::AccountingPreparedReservationRetainedBytes
            | Self::AccountingActiveReservationRetainedBytes
            | Self::AccountingReservationRequestRetainedBytes
            | Self::AccountingUsageNodeRetainedBytes
            | Self::AccountingPreparedReservationRecordBytes
            | Self::AccountingActiveReservationRecordBytes
            | Self::AccountingReservationRequestEntryBytes
            | Self::AccountingUsageNodeBytes
            | Self::AccountingReleaseScratchEntries
            | Self::AccountingReleaseScratchRetainedBytes
            | Self::AccountingReleaseScratchEntryBytes => Requirement::AccountingSelfOwnership,
            Self::SummaryBytes => Requirement::SummaryBytes,
            Self::SummaryRecordCount => Requirement::SummaryRecordCount,
            Self::SummarySchemaCount => Requirement::SummarySchemaCount,
            Self::SummaryChannelCount => Requirement::SummaryChannelCount,
            Self::SummaryChunkIndexCount => Requirement::SummaryChunkIndexCount,
            Self::PhysicalRegionDescriptorCount => Requirement::PhysicalRegionDescriptorCount,
            Self::ChannelMetadataEntriesPerRecord
            | Self::ChannelMetadataKeyBytes
            | Self::ChannelMetadataValueBytes
            | Self::NestedRetainedBytesPerSummary
            | Self::NestedRetainedBytesPerChunkScan => {
                Requirement::ChannelMetadataAndNestedRetention
            }
            Self::ChunkIndexMessageIndexOffsets => Requirement::ChunkIndexMessageIndexOffsets,
            Self::StatisticsChannelMessageCounts => Requirement::StatisticsChannelMessageCounts,
            Self::MessageIndexEntriesPerRecord => Requirement::MessageIndexRecordEntries,
            Self::MessageIndexRegionBytes
            | Self::MessageIndexRegionRecordCount
            | Self::MessageIndexRegionEntryCount => Requirement::MessageIndexOwningRegion,
            Self::AmbiguousZeroMessageIndexBytes
            | Self::AmbiguousZeroMessageIndexEntries
            | Self::AmbiguousZeroMessageIndexRanges => Requirement::AmbiguousZeroMessageIndex,
            Self::AmbiguousZeroChunkCompressedBytes
            | Self::AmbiguousZeroChunkUncompressedBytes
            | Self::AmbiguousZeroChunkRanges => Requirement::AmbiguousZeroChunk,
            Self::ChunkCompressedBytes => Requirement::ChunkCompressedBytes,
            Self::ChunkUncompressedBytes => Requirement::ChunkUncompressedBytes,
            Self::ChunkDecompressionRatioPermille => Requirement::ChunkDecompressionRatio,
            Self::DecompressorOverflowScratchBytes => Requirement::ExactOutputDecompressor,
            Self::ChunkRecordCount => Requirement::ChunkRecordCount,
            Self::ChunkMessageCount => Requirement::ChunkMessageCount,
            Self::SelectedMessageDispatchesPerScan => Requirement::ValidationAndDispatchCounts,
            Self::ManifestDenseChannelCount
            | Self::ManifestDensePayloadPlanBytes
            | Self::ValidationPlanEntries
            | Self::ValidationPlanRetainedBytes
            | Self::DecompressedChunkRetainedBytes => Requirement::ManifestDenseValidationPlan,
            Self::SourceRecordDerivedOrdinal => Requirement::SourceRecordDerivedOrdinal,
            Self::DecoderWorkingBytes
            | Self::DecoderBuilderBytes
            | Self::DecoderPayloadScratchBytes
            | Self::DecoderLensIntermediateBytes
            | Self::DecoderTerminalOutputBytes
            | Self::DecoderDerivedRows
            | Self::DecoderGenerationWorkingBytes
            | Self::DecoderChunkWorkingBytes
            | Self::DecoderSessionWorkingBytes
            | Self::DecoderGlobalWorkingBytes
            | Self::DecoderGenerationOutputRows
            | Self::DecoderChunkOutputRows
            | Self::DecoderSessionOutputRows
            | Self::DecoderGlobalOutputRows => Requirement::DecoderGroupAndAggregateBounds,
            Self::PartitionOutputBytes => Requirement::PartitionOutputBytes,
            Self::PartitionDerivedRoots => Requirement::PartitionDerivedRoots,
            Self::WindowChunkHits => Requirement::WindowIntervalHits,
            Self::SelectedChannelGroups => Requirement::SelectedChannelGroups,
            Self::SelectedChannelGroupMemberships | Self::SelectedChannelAssignmentsPerChannel => {
                Requirement::SelectedMembershipAndAssignment
            }
            Self::RootsPerPartition | Self::ExternalOriginBytesPerPartition => {
                Requirement::PartitionRootAndOriginBounds
            }
            Self::PlanningCrossProductOperations => Requirement::PlanningCrossProduct,
            Self::GenerationPartitions => Requirement::GenerationPartitions,
            Self::GenerationTerminalBatches
            | Self::GenerationDerivedRoots
            | Self::GenerationRootDescriptors => Requirement::GenerationTerminalRoots,
            Self::SessionRegisteredPartitions => Requirement::SessionRegisteredPartitions,
            Self::SessionCompleteEmptyEntries => Requirement::SessionCompleteEmptyEntries,
            Self::SessionRootDescriptors => Requirement::SessionRootDescriptors,
            Self::SessionExternalOriginBytes => Requirement::SessionExternalOriginBytes,
            Self::SessionResidentPhysicalRoots => Requirement::SessionResidentPhysicalRoots,
            Self::SessionRegistrationMetadataBytes => Requirement::RegistrationMetadataHeadroom,
            Self::RemoteInternalRetainedBytes
            | Self::RemoteStagedTerminalBatchBytes
            | Self::RemotePinnedResidentBytes
            | Self::RemoteEntityDbRetainedBytes
            | Self::RemoteStoreResidentBytes
            | Self::FetchWasmRawRetainedBytes => Requirement::RemoteInternalTotal,
            Self::ConcurrentRangeRequests => Requirement::ConcurrentRangeRequests,
            Self::InFlightRangeBytes => Requirement::InFlightRangeBytes,
            Self::RangeRetryAttemptsPerOperation => Requirement::RangeRetry,
            Self::RequestedRangeBytes | Self::ByobScratchBytes | Self::RangeJsWasmOverlapBytes => {
                Requirement::RangeByobOverlap
            }
            Self::RemoteValidatorIngressValues
            | Self::RemoteValidatorIngressWireBytes
            | Self::RemoteValidatorIngressJsWasmOverlapBytes
            | Self::RemoteValidatorIngressScratchBytes
            | Self::RemoteValidatorRetainedBytes
            | Self::RemoteValidatorEgressValues
            | Self::RemoteValidatorEgressWireBytes
            | Self::RemoteValidatorEgressJsWasmOverlapBytes
            | Self::RemoteValidatorEgressScratchBytes => Requirement::RemoteValidatorByteString,
            Self::ExtensionlessSnifferBufferBytes => Requirement::ExtensionlessSniffer,
            Self::ByobPumpSliceBytes | Self::ByobPumpSliceDurationMicros => Requirement::ByobPump,
            Self::MetadataOpeningBytes
            | Self::MetadataOpeningRangeRequests
            | Self::MetadataOpeningVisibleDeadlineMillis
            | Self::RemoteCpuWorkUnitsPerFrame => Requirement::MetadataOpeningAndCpu,
            Self::OpeningParseInputBytes
            | Self::OpeningParseOutputBytes
            | Self::OpeningParseRecordCount
            | Self::OpeningParseMessageCount
            | Self::OpeningParseDispatchCount
            | Self::OpeningParseDurationMicros
            | Self::ChunkValidationInputBytes
            | Self::ChunkValidationOutputBytes
            | Self::ChunkValidationRecordCount
            | Self::ChunkValidationMessageCount
            | Self::ChunkValidationDispatchCount
            | Self::ChunkValidationDurationMicros
            | Self::ChunkDispatchInputBytes
            | Self::ChunkDispatchOutputBytes
            | Self::ChunkDispatchRecordCount
            | Self::ChunkDispatchMessageCount
            | Self::ChunkDispatchActualAppendCount
            | Self::ChunkDispatchDurationMicros => Requirement::OpeningValidationDispatch,
            Self::MessageIndexParseDurationMicros => Requirement::MessageIndexParse,
            Self::RemoteSessionSlots
            | Self::OpenBatchUrls
            | Self::PendingSecretOpenBytes
            | Self::PendingOpenOptionsBytes
            | Self::PendingExtensionlessPrepared
            | Self::PendingFormatSniffs
            | Self::DisarmedStrictSources
            | Self::DisarmedHandoffRetainedBytes
            | Self::DisarmedHandoffDescriptors
            | Self::DisarmedHandoffOperations
            | Self::AtomicOpenReservationBytes => Requirement::OpenAdmission,
            Self::UrlFingerprintBuckets
            | Self::UrlFingerprintTokens
            | Self::UrlFingerprintTokensPerBucket
            | Self::UrlCanonicalMatchBytes
            | Self::UrlAliasDescriptors
            | Self::UrlFingerprintIndexRetainedBytes
            | Self::UrlAliasDescriptorRetainedBytes
            | Self::PreexistingRecordingAttachments
            | Self::PreexistingRecordingAttachmentBytes
            | Self::InstallationAcks
            | Self::InstallationAckRetainedBytes
            | Self::CatalogEntries
            | Self::CatalogProjectionBytes
            | Self::CatalogRows
            | Self::CatalogStringBytes
            | Self::SemanticConfigRetainedBytes => Requirement::OpenSourceIndexAndCatalog,
            Self::OwnedRemoteClientEntries
            | Self::OpenSourceStatusSlots
            | Self::OpenSourceTerminalEntries
            | Self::OpenSourceTerminalRetainedBytes
            | Self::OpenSourceStatusRetainedBytes
            | Self::OpenSourceLiveStatusOwners
            | Self::RemoteTerminalStatusBytes
            | Self::RemoteTerminalDiagnostics => Requirement::TerminalStatusRegistry,
            Self::DisarmedStatusClaims
            | Self::DeferredEvictionVictims
            | Self::DeferredEvictionDescriptorBytes => Requirement::DisarmedTerminalClaims,
            Self::OpenSourceOwners
            | Self::OpenSourceOwnerRetainedBytes
            | Self::OpenOperations
            | Self::OpenOperationRetainedBytes
            | Self::PublicRecordingHandles
            | Self::PublicRecordingHandleRetainedBytes
            | Self::RecordingsPerSource
            | Self::OperationRecordingSubscriptions
            | Self::OperationRecordingSubscriptionRetainedBytes
            | Self::LifecycleSubscribers
            | Self::LifecycleSubscriberRetainedBytes
            | Self::LifecycleHubs
            | Self::LifecycleHubRetainedBytes
            | Self::LifecycleChildStates
            | Self::LifecycleChildStateBytes
            | Self::ActivationDeliveryAcks
            | Self::ActivationDeliveryAckBytes
            | Self::LatestLifecycleSnapshots
            | Self::LatestLifecycleSnapshotBytes
            | Self::LifecycleOutstandingDeliveries
            | Self::LifecycleOutstandingDeliveryBytes
            | Self::LifecycleListenerErrorCredits
            | Self::SingleLifecycleEventBytes
            | Self::TypescriptDispatcherItems
            | Self::TypescriptDispatcherRetainedBytes
            | Self::LifecycleScheduledTasks
            | Self::LifecycleCurrentlyDispatching
            | Self::ListenerErrorNotificationsPerEvent
            | Self::PromiseClosureBytes
            | Self::PromiseClosureBytesPerOperation
            | Self::WrapperCacheEntries
            | Self::WrapperCacheRetainedBytes
            | Self::RemovedTombstoneRetainers
            | Self::RemovedTombstoneRetainedBytes => Requirement::PublicLifecycle,
            Self::PresentationFacades
            | Self::PresentationFacadeRetainedBytes
            | Self::PresentationSnapshots
            | Self::PresentationSnapshotRetainedBytes
            | Self::PresentationActiveLeases
            | Self::PresentationActiveLeaseRetainedBytes => Requirement::PresentationQueryOwnership,
            Self::PageLifecycleListeners
            | Self::PageHiddenWakeFlags
            | Self::PageParkedResponseOwners
            | Self::PageParkedResponseOwnerRetainedBytes
            | Self::PageVisibleDeadlineBookkeepingBytes
            | Self::PageResumeRevalidationWorkCount
            | Self::PageResumeRevalidationDurationMicros
            | Self::PageTeardownCleanupItems
            | Self::PageTeardownDurationMicros => Requirement::ChromePageExecution,
            Self::RawCacheBytes => Requirement::RawCache,
            Self::ExternalFieldsPerCall
            | Self::ExternalUtf16CodeUnitsPerCall
            | Self::ExternalUtf8BytesPerCall
            | Self::TimelineUtf16CodeUnits
            | Self::TimelineUtf8Bytes
            | Self::TopicFilterUtf16CodeUnits
            | Self::TopicFilterUtf8Bytes
            | Self::DecoderSelectorUtf16CodeUnits
            | Self::DecoderSelectorUtf8Bytes
            | Self::RouteFieldUtf16CodeUnits
            | Self::RouteFieldUtf8Bytes
            | Self::RouteFragmentFields
            | Self::EntityPathParts
            | Self::EntityPathUtf8Bytes
            | Self::ExternalStringCopyCalls
            | Self::ExternalStringCopyAllocationBytes => Requirement::ExternalStrings,
            Self::ExistingTimelineCount
            | Self::ExistingEntityPathCount
            | Self::ExistingComponentCount
            | Self::ExistingEntityPathKeyBytes
            | Self::ExistingIdentifierNodeBytes
            | Self::ExistingIdentifierViewerBytes => Requirement::ExistingIdentifierIndex,
            Self::RuntimeInternStringBytes
            | Self::RuntimeInternEntryAndCapacityBytes
            | Self::RuntimeInternCensusIdentifiers
            | Self::RuntimeInternCensusRetainedBytes
            | Self::RuntimeInternCandidatePeakBytes => Requirement::RuntimeInternBudget,
            Self::AddChunkInputBytes
            | Self::AddChunkRows
            | Self::AddChunkComponentCount
            | Self::AddChunkTimelineCount
            | Self::AddChunkDurationMicros => Requirement::AddChunk,
            Self::PendingInsertionBytesPerFrame | Self::AddChunkCallsPerFrame => {
                Requirement::InsertionFrame
            }
            Self::SharedRemoteWorkUnitsPerFrame
            | Self::RemoteGcWorkUnitsPerFrame
            | Self::RemoteCpuAllowanceMicrosPerFrame
            | Self::RemoteGcCpuAllowanceMicrosPerFrame
            | Self::RemoteGcCandidateRoots
            | Self::RemoteGcDurationMicros => Requirement::RemoteGc,
            Self::MemoryPressurePendingRequests
            | Self::MemoryPressureAllocationPauseStates
            | Self::MemoryPressureCompletionDeadlineMillis
            | Self::ProcessSafetyHeadroomBytes => Requirement::MemoryPressure,
            Self::BackfillRangeRequestsPerSeek => Requirement::BackfillRangeRequests,
            Self::BackfillMessageIndexBytesPerSeek => Requirement::BackfillMessageIndexBytes,
            Self::BackfillParsedEntriesPerSeek => Requirement::BackfillParsedEntries,
            Self::BackfillVisibleDeadlineMillis => Requirement::BackfillDeadline,
        }
    }

    /// Classifies synchronous duration observations that cannot be used as preemptive deadlines.
    pub const fn enforcement(self) -> WebRemoteLimitEnforcement {
        match self {
            Self::ByobPumpSliceDurationMicros
            | Self::OpeningParseDurationMicros
            | Self::ChunkValidationDurationMicros
            | Self::ChunkDispatchDurationMicros
            | Self::MessageIndexParseDurationMicros
            | Self::AddChunkDurationMicros
            | Self::PageResumeRevalidationDurationMicros
            | Self::PageTeardownDurationMicros
            | Self::RemoteGcDurationMicros => {
                WebRemoteLimitEnforcement::TelemetryAcceptanceThreshold
            }
            _ => WebRemoteLimitEnforcement::HardLimit,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AggregateReservationFamily {
    ScopeNode,
    AccountingSelf,
    Decoder,
    RemoteInternal,
    SharedFrameWork,
    Lifecycle,
    Presentation,
    PageExecution,
    OpenRegistry,
    ExistingIdentifier,
}

const fn aggregate_reservation_family(
    key: WebRemoteLimitKey,
) -> Option<AggregateReservationFamily> {
    match key {
        WebRemoteLimitKey::AccountingScopeNodesGlobal
        | WebRemoteLimitKey::AccountingScopeNodeBytesGlobal
        | WebRemoteLimitKey::AccountingChildScopeNodesPerParent
        | WebRemoteLimitKey::AccountingChildScopeBytesPerParent => {
            Some(AggregateReservationFamily::ScopeNode)
        }
        WebRemoteLimitKey::AccountingPreparedReservationRecords
        | WebRemoteLimitKey::AccountingActiveReservationRecords
        | WebRemoteLimitKey::AccountingReservationRequestEntries
        | WebRemoteLimitKey::AccountingUsageNodes
        | WebRemoteLimitKey::AccountingPreparedReservationRetainedBytes
        | WebRemoteLimitKey::AccountingActiveReservationRetainedBytes
        | WebRemoteLimitKey::AccountingReservationRequestRetainedBytes
        | WebRemoteLimitKey::AccountingUsageNodeRetainedBytes
        | WebRemoteLimitKey::AccountingReleaseScratchEntries
        | WebRemoteLimitKey::AccountingReleaseScratchRetainedBytes => {
            Some(AggregateReservationFamily::AccountingSelf)
        }
        WebRemoteLimitKey::DecoderWorkingBytes
        | WebRemoteLimitKey::DecoderBuilderBytes
        | WebRemoteLimitKey::DecoderPayloadScratchBytes
        | WebRemoteLimitKey::DecoderLensIntermediateBytes
        | WebRemoteLimitKey::DecoderTerminalOutputBytes
        | WebRemoteLimitKey::DecoderChunkWorkingBytes
        | WebRemoteLimitKey::DecoderGenerationWorkingBytes
        | WebRemoteLimitKey::DecoderSessionWorkingBytes
        | WebRemoteLimitKey::DecoderGlobalWorkingBytes
        | WebRemoteLimitKey::DecoderChunkOutputRows
        | WebRemoteLimitKey::DecoderGenerationOutputRows
        | WebRemoteLimitKey::DecoderSessionOutputRows
        | WebRemoteLimitKey::DecoderGlobalOutputRows => Some(AggregateReservationFamily::Decoder),
        WebRemoteLimitKey::ValidationPlanRetainedBytes
        | WebRemoteLimitKey::DecompressedChunkRetainedBytes
        | WebRemoteLimitKey::SessionRegistrationMetadataBytes
        | WebRemoteLimitKey::RemoteInternalRetainedBytes
        | WebRemoteLimitKey::ByobScratchBytes
        | WebRemoteLimitKey::RangeJsWasmOverlapBytes
        | WebRemoteLimitKey::RemoteValidatorIngressJsWasmOverlapBytes
        | WebRemoteLimitKey::RemoteValidatorIngressScratchBytes
        | WebRemoteLimitKey::RemoteValidatorRetainedBytes
        | WebRemoteLimitKey::RemoteValidatorEgressValues
        | WebRemoteLimitKey::RemoteValidatorEgressJsWasmOverlapBytes
        | WebRemoteLimitKey::RemoteValidatorEgressScratchBytes
        | WebRemoteLimitKey::MetadataOpeningBytes
        | WebRemoteLimitKey::RawCacheBytes
        | WebRemoteLimitKey::RemoteStagedTerminalBatchBytes
        | WebRemoteLimitKey::RemotePinnedResidentBytes
        | WebRemoteLimitKey::RemoteEntityDbRetainedBytes
        | WebRemoteLimitKey::RemoteStoreResidentBytes
        | WebRemoteLimitKey::FetchWasmRawRetainedBytes => {
            Some(AggregateReservationFamily::RemoteInternal)
        }
        WebRemoteLimitKey::SharedRemoteWorkUnitsPerFrame => {
            Some(AggregateReservationFamily::SharedFrameWork)
        }
        WebRemoteLimitKey::OpenSourceOwners
        | WebRemoteLimitKey::OpenSourceOwnerRetainedBytes
        | WebRemoteLimitKey::OpenOperations
        | WebRemoteLimitKey::OpenOperationRetainedBytes
        | WebRemoteLimitKey::PublicRecordingHandles
        | WebRemoteLimitKey::PublicRecordingHandleRetainedBytes
        | WebRemoteLimitKey::OperationRecordingSubscriptions
        | WebRemoteLimitKey::OperationRecordingSubscriptionRetainedBytes
        | WebRemoteLimitKey::LifecycleSubscribers
        | WebRemoteLimitKey::LifecycleSubscriberRetainedBytes
        | WebRemoteLimitKey::LifecycleHubs
        | WebRemoteLimitKey::LifecycleHubRetainedBytes
        | WebRemoteLimitKey::LifecycleChildStates
        | WebRemoteLimitKey::LifecycleChildStateBytes
        | WebRemoteLimitKey::ActivationDeliveryAcks
        | WebRemoteLimitKey::ActivationDeliveryAckBytes
        | WebRemoteLimitKey::LatestLifecycleSnapshots
        | WebRemoteLimitKey::LatestLifecycleSnapshotBytes
        | WebRemoteLimitKey::LifecycleOutstandingDeliveries
        | WebRemoteLimitKey::LifecycleOutstandingDeliveryBytes
        | WebRemoteLimitKey::LifecycleListenerErrorCredits
        | WebRemoteLimitKey::TypescriptDispatcherItems
        | WebRemoteLimitKey::TypescriptDispatcherRetainedBytes
        | WebRemoteLimitKey::LifecycleScheduledTasks
        | WebRemoteLimitKey::LifecycleCurrentlyDispatching
        | WebRemoteLimitKey::PromiseClosureBytes
        | WebRemoteLimitKey::WrapperCacheEntries
        | WebRemoteLimitKey::WrapperCacheRetainedBytes
        | WebRemoteLimitKey::RemovedTombstoneRetainers
        | WebRemoteLimitKey::RemovedTombstoneRetainedBytes => {
            Some(AggregateReservationFamily::Lifecycle)
        }
        WebRemoteLimitKey::PresentationFacades
        | WebRemoteLimitKey::PresentationFacadeRetainedBytes
        | WebRemoteLimitKey::PresentationSnapshots
        | WebRemoteLimitKey::PresentationSnapshotRetainedBytes
        | WebRemoteLimitKey::PresentationActiveLeases
        | WebRemoteLimitKey::PresentationActiveLeaseRetainedBytes => {
            Some(AggregateReservationFamily::Presentation)
        }
        WebRemoteLimitKey::PageParkedResponseOwners
        | WebRemoteLimitKey::PageParkedResponseOwnerRetainedBytes => {
            Some(AggregateReservationFamily::PageExecution)
        }
        WebRemoteLimitKey::RemoteSessionSlots
        | WebRemoteLimitKey::PendingSecretOpenBytes
        | WebRemoteLimitKey::PendingOpenOptionsBytes
        | WebRemoteLimitKey::PendingExtensionlessPrepared
        | WebRemoteLimitKey::PendingFormatSniffs
        | WebRemoteLimitKey::DisarmedStrictSources
        | WebRemoteLimitKey::DisarmedHandoffRetainedBytes
        | WebRemoteLimitKey::AtomicOpenReservationBytes
        | WebRemoteLimitKey::UrlFingerprintBuckets
        | WebRemoteLimitKey::UrlFingerprintTokens
        | WebRemoteLimitKey::UrlCanonicalMatchBytes
        | WebRemoteLimitKey::UrlAliasDescriptors
        | WebRemoteLimitKey::UrlFingerprintIndexRetainedBytes
        | WebRemoteLimitKey::UrlAliasDescriptorRetainedBytes
        | WebRemoteLimitKey::CatalogEntries
        | WebRemoteLimitKey::CatalogProjectionBytes
        | WebRemoteLimitKey::SemanticConfigRetainedBytes
        | WebRemoteLimitKey::OwnedRemoteClientEntries
        | WebRemoteLimitKey::OpenSourceStatusSlots
        | WebRemoteLimitKey::OpenSourceTerminalEntries
        | WebRemoteLimitKey::OpenSourceTerminalRetainedBytes
        | WebRemoteLimitKey::OpenSourceStatusRetainedBytes
        | WebRemoteLimitKey::OpenSourceLiveStatusOwners
        | WebRemoteLimitKey::DisarmedStatusClaims
        | WebRemoteLimitKey::DeferredEvictionDescriptorBytes => {
            Some(AggregateReservationFamily::OpenRegistry)
        }
        WebRemoteLimitKey::ExistingTimelineCount
        | WebRemoteLimitKey::ExistingEntityPathCount
        | WebRemoteLimitKey::ExistingComponentCount
        | WebRemoteLimitKey::ExistingEntityPathKeyBytes
        | WebRemoteLimitKey::ExistingIdentifierNodeBytes
        | WebRemoteLimitKey::ExistingIdentifierViewerBytes => {
            Some(AggregateReservationFamily::ExistingIdentifier)
        }
        _ => None,
    }
}

macro_rules! define_accounting_key {
    ($name:ident, $accounting:ident, $error:ident) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(WebRemoteLimitKey);

        impl $name {
            pub fn try_from_schema_key(
                key: WebRemoteLimitKey,
            ) -> Result<Self, LimitKeyClassificationError> {
                if key.enforcement() != WebRemoteLimitEnforcement::HardLimit
                    || key.definition().accounting != WebRemoteAccountingKind::$accounting
                {
                    return Err(LimitKeyClassificationError::$error);
                }
                Ok(Self(key))
            }

            pub const fn schema_key(self) -> WebRemoteLimitKey {
                self.0
            }
        }
    };
}

define_accounting_key!(
    ScalarObservationKey,
    ScalarObservation,
    NotScalarObservation
);

impl ScalarObservationKey {
    const fn from_schema_key_internal(key: WebRemoteLimitKey) -> Self {
        Self(key)
    }
}

impl FormulaConstantKey {
    const fn from_schema_key_internal(key: WebRemoteLimitKey) -> Self {
        Self(key)
    }
}
define_accounting_key!(FormulaConstantKey, FormulaConstant, NotFormulaConstant);
define_accounting_key!(
    ModuleLifetimeBurnKey,
    ModuleLifetimeBurn,
    NotModuleLifetimeBurn
);

/// A directly reservable concurrent key.
///
/// Keys that participate in a mandatory aggregate bundle have no public conversion to this type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReclaimableConcurrentKey(WebRemoteLimitKey);

impl ReclaimableConcurrentKey {
    pub fn try_from_schema_key(
        key: WebRemoteLimitKey,
    ) -> Result<Self, LimitKeyClassificationError> {
        if key.enforcement() != WebRemoteLimitEnforcement::HardLimit
            || key.definition().accounting != WebRemoteAccountingKind::ReclaimableConcurrent
        {
            return Err(LimitKeyClassificationError::NotReclaimableConcurrent);
        }
        if aggregate_reservation_family(key).is_some() {
            return Err(LimitKeyClassificationError::AggregateReservationRequired);
        }
        Ok(Self(key))
    }

    pub const fn schema_key(self) -> WebRemoteLimitKey {
        self.0
    }

    const fn from_schema_key_internal(key: WebRemoteLimitKey) -> Self {
        Self(key)
    }
}

/// A key that is valid only for telemetry and acceptance testing.
///
/// Passing this type to a runtime enforcement API is a compile-time type error.
///
/// ```compile_fail
/// use re_web::remote_limits::{
///     ScalarObservationKey, TelemetryAcceptanceThresholdKey, WebRemoteLimitKey,
/// };
///
/// fn runtime_enforcement(_: ScalarObservationKey) {}
/// let threshold = TelemetryAcceptanceThresholdKey::try_from_schema_key(
///     WebRemoteLimitKey::ChunkValidationDurationMicros,
/// )
/// .unwrap();
/// runtime_enforcement(threshold);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TelemetryAcceptanceThresholdKey(WebRemoteLimitKey);

impl TelemetryAcceptanceThresholdKey {
    pub fn try_from_schema_key(
        key: WebRemoteLimitKey,
    ) -> Result<Self, LimitKeyClassificationError> {
        if key.enforcement() != WebRemoteLimitEnforcement::TelemetryAcceptanceThreshold {
            return Err(LimitKeyClassificationError::NotTelemetryAcceptanceThreshold);
        }
        Ok(Self(key))
    }

    pub const fn schema_key(self) -> WebRemoteLimitKey {
        self.0
    }
}

/// Failure to classify a schema key for a typed API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitKeyClassificationError {
    NotScalarObservation,
    NotFormulaConstant,
    NotModuleLifetimeBurn,
    NotReclaimableConcurrent,
    AggregateReservationRequired,
    NotTelemetryAcceptanceThreshold,
}

impl fmt::Display for LimitKeyClassificationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotScalarObservation => formatter.write_str("key is not a scalar observation"),
            Self::NotFormulaConstant => formatter.write_str("key is not a formula constant"),
            Self::NotModuleLifetimeBurn => formatter.write_str("key is not a module-lifetime burn"),
            Self::NotReclaimableConcurrent => {
                formatter.write_str("key is not directly reclaimable concurrent ownership")
            }
            Self::AggregateReservationRequired => {
                formatter.write_str("key requires a typed aggregate reservation")
            }
            Self::NotTelemetryAcceptanceThreshold => {
                formatter.write_str("key is not a telemetry acceptance threshold")
            }
        }
    }
}

impl std::error::Error for LimitKeyClassificationError {}

macro_rules! define_accounting_value {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct $name(NonZeroU64);

        impl $name {
            const fn get(self) -> u64 {
                self.0.get()
            }
        }
    };
}

define_accounting_value!(ScalarObservationValue);
define_accounting_value!(FormulaConstantValue);
define_accounting_value!(ModuleLifetimeBurnValue);
define_accounting_value!(ReclaimableConcurrentValue);

/// An opaque telemetry acceptance-threshold value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TelemetryAcceptanceThresholdValue(NonZeroU64);

impl TelemetryAcceptanceThresholdValue {
    const fn get(self) -> u64 {
        self.0.get()
    }
}

/// The result of comparing telemetry with its acceptance threshold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TelemetryThresholdObservation {
    WithinThreshold,
    ExceededThreshold,
}

/// A content digest that identifies the immutable measurement artifact used to freeze a value.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MeasurementEvidenceDigest([u8; 32]);

impl MeasurementEvidenceDigest {
    pub fn new(bytes: [u8; 32]) -> Result<Self, MeasurementEvidenceError> {
        if bytes == [0; 32] {
            return Err(MeasurementEvidenceError::ZeroDigest);
        }
        Ok(Self(bytes))
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for MeasurementEvidenceDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("MeasurementEvidenceDigest")
            .field(&"<opaque>")
            .finish()
    }
}

/// Failure to construct measurement evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeasurementEvidenceError {
    ZeroDigest,
}

impl fmt::Display for MeasurementEvidenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("measurement evidence digest must be non-zero")
    }
}

impl std::error::Error for MeasurementEvidenceError {}

/// Evidence attached to one measured limit transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeasurementEvidence {
    pub stage: MeasurementStage,
    pub kind: EvidenceKind,
    pub artifact_digest: MeasurementEvidenceDigest,
}

/// The provenance of a frozen limit value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrozenLimitValueSource {
    DesignConstant { reference: &'static str },
    Measurement(MeasurementEvidence),
}

/// A non-zero frozen limit and its provenance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrozenWebRemoteLimit {
    pub value: NonZeroU64,
    pub source: FrozenLimitValueSource,
}

/// The state of one value in a draft profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebRemoteLimitState {
    Unfrozen {
        stage: MeasurementStage,
        evidence: EvidenceKind,
    },
    Frozen(FrozenWebRemoteLimit),
}

/// A V1 profile whose measured fields may still be unfrozen.
///
/// This type is metadata, not a production-use capability.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftWebRemoteLimitsV1 {
    revision: u64,
    states: [WebRemoteLimitState; WEB_REMOTE_LIMIT_COUNT],
}

impl Default for DraftWebRemoteLimitsV1 {
    fn default() -> Self {
        Self::new()
    }
}

impl DraftWebRemoteLimitsV1 {
    /// Builds the canonical V1 draft.
    ///
    /// Only values explicitly fixed by the design are frozen here.
    /// Every measured value remains [`WebRemoteLimitState::Unfrozen`].
    pub fn new() -> Self {
        let states = std::array::from_fn(|index| {
            let key = WebRemoteLimitKey::ALL[index];
            match key.definition().requirement {
                WebRemoteLimitRequirement::DesignConstant(constant) => {
                    WebRemoteLimitState::Frozen(FrozenWebRemoteLimit {
                        value: constant.value,
                        source: FrozenLimitValueSource::DesignConstant {
                            reference: constant.reference,
                        },
                    })
                }
                WebRemoteLimitRequirement::Measurement { stage, evidence } => {
                    WebRemoteLimitState::Unfrozen { stage, evidence }
                }
            }
        });
        Self {
            revision: 0,
            states,
        }
    }

    pub const fn version() -> WebRemoteLimitsProfileVersion {
        WebRemoteLimitsProfileVersion::V1
    }

    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub fn state(&self, key: WebRemoteLimitKey) -> WebRemoteLimitState {
        self.states[key.index()]
    }

    pub fn unfrozen_count(&self) -> usize {
        self.states
            .iter()
            .filter(|state| matches!(state, WebRemoteLimitState::Unfrozen { .. }))
            .count()
    }

    /// Prepares an atomic, evidence-checked transition without mutating this profile.
    pub fn prepare_measurements(
        &self,
        measurements: impl IntoIterator<Item = MeasuredLimitValue>,
    ) -> Result<PreparedLimitProfileTransitionV1, LimitProfileTransitionError> {
        let next_revision = self
            .revision
            .checked_add(1)
            .ok_or(LimitProfileTransitionError::RevisionExhausted)?;
        let mut seen = BTreeSet::new();
        let mut updates = Vec::new();

        for measurement in measurements {
            if !seen.insert(measurement.key) {
                return Err(LimitProfileTransitionError::DuplicateKey {
                    key: measurement.key,
                });
            }
            let WebRemoteLimitState::Unfrozen { stage, evidence } =
                self.states[measurement.key.index()]
            else {
                return Err(LimitProfileTransitionError::AlreadyFrozen {
                    key: measurement.key,
                });
            };
            if measurement.evidence.stage != stage || measurement.evidence.kind != evidence {
                return Err(LimitProfileTransitionError::EvidenceMismatch {
                    key: measurement.key,
                    expected_stage: stage,
                    expected_kind: evidence,
                });
            }
            updates.push((
                measurement.key,
                FrozenWebRemoteLimit {
                    value: measurement.value,
                    source: FrozenLimitValueSource::Measurement(measurement.evidence),
                },
            ));
        }

        if updates.is_empty() {
            return Err(LimitProfileTransitionError::EmptyTransition);
        }

        Ok(PreparedLimitProfileTransitionV1 {
            expected_revision: self.revision,
            committed_revision: next_revision,
            updates,
        })
    }

    /// Validates every value and returns a tooling artifact.
    ///
    /// A validated artifact is not a production-use capability.
    /// Only a future checked-in canonical artifact can be sealed inside this crate.
    pub fn validate_complete(
        &self,
    ) -> Result<ValidatedFrozenWebRemoteLimitsV1, LimitProfileCompletionError> {
        for key in WebRemoteLimitKey::ALL {
            if let WebRemoteLimitState::Unfrozen { stage, evidence } = self.states[key.index()] {
                return Err(LimitProfileCompletionError::Unfrozen {
                    key,
                    stage,
                    evidence,
                });
            }
        }

        let values = self.states.map(|state| match state {
            WebRemoteLimitState::Frozen(frozen) => frozen.value,
            WebRemoteLimitState::Unfrozen { .. } => {
                unreachable!("the complete preflight rejected every unfrozen limit")
            }
        });
        let sources = self.states.map(|state| match state {
            WebRemoteLimitState::Frozen(frozen) => frozen.source,
            WebRemoteLimitState::Unfrozen { .. } => {
                unreachable!("the complete preflight rejected every unfrozen limit")
            }
        });

        validate_complete_profile(&values)?;

        Ok(ValidatedFrozenWebRemoteLimitsV1 {
            profile_revision: self.revision,
            values,
            sources,
        })
    }
}

/// One value and its required evidence in a profile transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeasuredLimitValue {
    pub key: WebRemoteLimitKey,
    pub value: NonZeroU64,
    pub evidence: MeasurementEvidence,
}

/// A fully validated but disarmed profile transition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedLimitProfileTransitionV1 {
    expected_revision: u64,
    committed_revision: u64,
    updates: Vec<(WebRemoteLimitKey, FrozenWebRemoteLimit)>,
}

impl PreparedLimitProfileTransitionV1 {
    /// Applies the transition atomically if the profile still matches its prepared revision.
    pub fn commit(
        self,
        profile: &mut DraftWebRemoteLimitsV1,
    ) -> Result<(), LimitProfileTransitionError> {
        if profile.revision != self.expected_revision {
            return Err(LimitProfileTransitionError::RevisionMismatch);
        }
        for (key, _) in &self.updates {
            if !matches!(
                profile.states[key.index()],
                WebRemoteLimitState::Unfrozen { .. }
            ) {
                return Err(LimitProfileTransitionError::StateChanged { key: *key });
            }
        }

        for (key, value) in self.updates {
            profile.states[key.index()] = WebRemoteLimitState::Frozen(value);
        }
        profile.revision = self.committed_revision;
        Ok(())
    }
}

/// Failure to prepare or commit a profile transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitProfileTransitionError {
    EmptyTransition,
    DuplicateKey {
        key: WebRemoteLimitKey,
    },
    AlreadyFrozen {
        key: WebRemoteLimitKey,
    },
    EvidenceMismatch {
        key: WebRemoteLimitKey,
        expected_stage: MeasurementStage,
        expected_kind: EvidenceKind,
    },
    RevisionExhausted,
    RevisionMismatch,
    StateChanged {
        key: WebRemoteLimitKey,
    },
}

impl fmt::Display for LimitProfileTransitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyTransition => formatter.write_str("limit profile transition is empty"),
            Self::DuplicateKey { key } => {
                write!(
                    formatter,
                    "duplicate limit key {}",
                    key.definition().stable_name
                )
            }
            Self::AlreadyFrozen { key } => {
                write!(
                    formatter,
                    "limit {} is already frozen",
                    key.definition().stable_name
                )
            }
            Self::EvidenceMismatch { key, .. } => write!(
                formatter,
                "measurement evidence does not match limit {}",
                key.definition().stable_name
            ),
            Self::RevisionExhausted => formatter.write_str("limit profile revision exhausted"),
            Self::RevisionMismatch => {
                formatter.write_str("prepared limit profile revision mismatch")
            }
            Self::StateChanged { key } => write!(
                formatter,
                "prepared limit state changed for {}",
                key.definition().stable_name
            ),
        }
    }
}

impl std::error::Error for LimitProfileTransitionError {}

/// The cross-field rule that rejected an otherwise fully frozen profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LimitProfileConstraint {
    BatchFitsTerminalRegistry,
    TerminalRegistryFitsStatusRegistry,
    TerminalRetainedBytesCoverTerminalEntries,
    StatusComponentsFitLiveOwners,
    StatusRetainedBytesCoverOwnerSlots,
    PublishedLiveAndTerminalFitStatusSlots,
    FutureClaimsFitDisarmedClaims,
    DeferredEvictionVictimsFitTerminalRegistry,
    ReleasedLiveFutureAndTerminalFitStatusSlots,
    StatusRetainedBytesCoverFutureClaimsAndEviction,
    LifecycleWorstCaseRustDeliveryCount,
    LifecycleWorstCaseListenerErrorCredits,
    LifecycleWorstCaseRustDeliveryBytes,
    LifecycleWorstCaseTypescriptDeliveryCount,
    LifecycleWorstCaseTypescriptDeliveryBytes,
    PromiseClosureBytesCoverOperations,
    WrapperCacheCoversOperationsAndRecordings,
    RecordingHandlesFitTombstoneRetainers,
    FingerprintTokensPerBucketFitGlobal,
    RemoteGcFitsSharedCpuAllowance,
    RemoteCpuWorkFitsSharedFrameCap,
    RemoteGcWorkFitsSharedFrameCap,
    GlobalScopeNodeBytesCoverNodes,
    ParentScopeNodeBytesCoverNodes,
    OperationSubscriptionsCoverRecordingHandles,
    RuntimeInternMinimumSideMapFitsBudgets,
}

impl LimitProfileConstraint {
    pub const ALL: [Self; 26] = [
        Self::BatchFitsTerminalRegistry,
        Self::TerminalRegistryFitsStatusRegistry,
        Self::TerminalRetainedBytesCoverTerminalEntries,
        Self::StatusComponentsFitLiveOwners,
        Self::StatusRetainedBytesCoverOwnerSlots,
        Self::PublishedLiveAndTerminalFitStatusSlots,
        Self::FutureClaimsFitDisarmedClaims,
        Self::DeferredEvictionVictimsFitTerminalRegistry,
        Self::ReleasedLiveFutureAndTerminalFitStatusSlots,
        Self::StatusRetainedBytesCoverFutureClaimsAndEviction,
        Self::LifecycleWorstCaseRustDeliveryCount,
        Self::LifecycleWorstCaseListenerErrorCredits,
        Self::LifecycleWorstCaseRustDeliveryBytes,
        Self::LifecycleWorstCaseTypescriptDeliveryCount,
        Self::LifecycleWorstCaseTypescriptDeliveryBytes,
        Self::PromiseClosureBytesCoverOperations,
        Self::WrapperCacheCoversOperationsAndRecordings,
        Self::RecordingHandlesFitTombstoneRetainers,
        Self::FingerprintTokensPerBucketFitGlobal,
        Self::RemoteGcFitsSharedCpuAllowance,
        Self::RemoteCpuWorkFitsSharedFrameCap,
        Self::RemoteGcWorkFitsSharedFrameCap,
        Self::GlobalScopeNodeBytesCoverNodes,
        Self::ParentScopeNodeBytesCoverNodes,
        Self::OperationSubscriptionsCoverRecordingHandles,
        Self::RuntimeInternMinimumSideMapFitsBudgets,
    ];

    pub const fn design_requirement(self) -> NormativeResourceRequirement {
        use NormativeResourceRequirement as Requirement;
        match self {
            Self::BatchFitsTerminalRegistry
            | Self::TerminalRegistryFitsStatusRegistry
            | Self::TerminalRetainedBytesCoverTerminalEntries => {
                Requirement::TerminalStatusRegistry
            }
            Self::StatusComponentsFitLiveOwners
            | Self::StatusRetainedBytesCoverOwnerSlots
            | Self::PublishedLiveAndTerminalFitStatusSlots => Requirement::TerminalStatusRegistry,
            Self::FutureClaimsFitDisarmedClaims
            | Self::DeferredEvictionVictimsFitTerminalRegistry
            | Self::ReleasedLiveFutureAndTerminalFitStatusSlots
            | Self::StatusRetainedBytesCoverFutureClaimsAndEviction => {
                Requirement::DisarmedTerminalClaims
            }
            Self::LifecycleWorstCaseRustDeliveryCount
            | Self::LifecycleWorstCaseListenerErrorCredits
            | Self::LifecycleWorstCaseRustDeliveryBytes
            | Self::LifecycleWorstCaseTypescriptDeliveryCount
            | Self::LifecycleWorstCaseTypescriptDeliveryBytes
            | Self::PromiseClosureBytesCoverOperations
            | Self::WrapperCacheCoversOperationsAndRecordings
            | Self::RecordingHandlesFitTombstoneRetainers
            | Self::OperationSubscriptionsCoverRecordingHandles => Requirement::PublicLifecycle,
            Self::FingerprintTokensPerBucketFitGlobal => Requirement::OpenSourceIndexAndCatalog,
            Self::RemoteGcFitsSharedCpuAllowance
            | Self::RemoteCpuWorkFitsSharedFrameCap
            | Self::RemoteGcWorkFitsSharedFrameCap => Requirement::RemoteGc,
            Self::GlobalScopeNodeBytesCoverNodes | Self::ParentScopeNodeBytesCoverNodes => {
                Requirement::AccountingScopeOwnership
            }
            Self::RuntimeInternMinimumSideMapFitsBudgets => Requirement::RuntimeInternBudget,
        }
    }
}

/// A named multi-key formula or inseparable reservation bundle which closes one or more
/// normative resource requirements.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NormativeAggregateFormula {
    ValidationEstimateAndActualDispatch,
    DecoderAggregateReservation,
    PlanningCrossProductArithmetic,
    SessionPartitionArithmetic,
    SessionDescriptorOriginArithmetic,
    RegistrationMetadataHeadroomReservation,
    RangeBodyPumpReservation,
    RemoteInternalTotalGate,
    RemoteValidatorIngressReservation,
    RemoteValidatorRetainedReservation,
    RemoteValidatorEgressReservation,
    LifecycleRetentionBundle,
    PresentationActivationReservation,
    PresentationLeaseReservation,
    PageParkedResponseOwnerReservation,
    SharedFrameWorkUnitReservation,
    ScopeNodeCreditReservation,
    AccountingSelfCreditReservation,
    OpenAdmissionReservation,
    OpenStatusReservation,
    UrlIndexReservation,
    ExistingIdentifierIndexReservation,
    RuntimeInternInitializationProfile,
    MemoryHeadroomGate,
}

/// The single typed runtime entry point which enforces a normative aggregate formula.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AggregateTypedEntryPoint {
    ValidateDispatchFormula,
    PrepareDecoderReservation,
    ValidatePlanningCrossProduct,
    ValidateSessionPartitionFormula,
    ValidateSessionDescriptorFormula,
    PrepareRegistrationMetadataFormula,
    PrepareRangeBodyPumpReservation,
    PrepareRemoteInternalReservation,
    PrepareRemoteValidatorIngress,
    PrepareRemoteValidatorRetained,
    PrepareRemoteValidatorEgress,
    PrepareLifecycleReservation,
    ActivatePresentation,
    AcquirePresentationLease,
    PreparePageParkedResponseOwnerReservation,
    PrepareSharedFrameWork,
    CreateChildScopeWithNodeCredit,
    BeginReservationPlanWithAccountingCredit,
    PrepareOpenAdmissionReservation,
    PrepareOpenStatusReservation,
    PrepareUrlIndexReservation,
    PrepareExistingIdentifierIndexReservation,
    ValidateRuntimeInternInitializationProfile,
    ValidateMemoryHeadroom,
}

impl NormativeAggregateFormula {
    pub const ALL: [Self; 24] = [
        Self::ValidationEstimateAndActualDispatch,
        Self::DecoderAggregateReservation,
        Self::PlanningCrossProductArithmetic,
        Self::SessionPartitionArithmetic,
        Self::SessionDescriptorOriginArithmetic,
        Self::RegistrationMetadataHeadroomReservation,
        Self::RangeBodyPumpReservation,
        Self::RemoteInternalTotalGate,
        Self::RemoteValidatorIngressReservation,
        Self::RemoteValidatorRetainedReservation,
        Self::RemoteValidatorEgressReservation,
        Self::LifecycleRetentionBundle,
        Self::PresentationActivationReservation,
        Self::PresentationLeaseReservation,
        Self::PageParkedResponseOwnerReservation,
        Self::SharedFrameWorkUnitReservation,
        Self::ScopeNodeCreditReservation,
        Self::AccountingSelfCreditReservation,
        Self::OpenAdmissionReservation,
        Self::OpenStatusReservation,
        Self::UrlIndexReservation,
        Self::ExistingIdentifierIndexReservation,
        Self::RuntimeInternInitializationProfile,
        Self::MemoryHeadroomGate,
    ];

    pub const fn design_requirement(self) -> NormativeResourceRequirement {
        use NormativeResourceRequirement as Requirement;
        match self {
            Self::ValidationEstimateAndActualDispatch => Requirement::ValidationAndDispatchCounts,
            Self::DecoderAggregateReservation => Requirement::DecoderGroupAndAggregateBounds,
            Self::PlanningCrossProductArithmetic => Requirement::PlanningCrossProduct,
            Self::SessionPartitionArithmetic => Requirement::SessionPartitionFormula,
            Self::SessionDescriptorOriginArithmetic => Requirement::SessionDescriptorOriginFormula,
            Self::RegistrationMetadataHeadroomReservation => {
                Requirement::RegistrationMetadataHeadroom
            }
            Self::RangeBodyPumpReservation => Requirement::RangeByobOverlap,
            Self::RemoteInternalTotalGate => Requirement::RemoteInternalTotal,
            Self::RemoteValidatorIngressReservation
            | Self::RemoteValidatorRetainedReservation
            | Self::RemoteValidatorEgressReservation => Requirement::RemoteValidatorByteString,
            Self::LifecycleRetentionBundle => Requirement::PublicLifecycle,
            Self::PresentationActivationReservation | Self::PresentationLeaseReservation => {
                Requirement::PresentationQueryOwnership
            }
            Self::PageParkedResponseOwnerReservation => Requirement::ChromePageExecution,
            Self::SharedFrameWorkUnitReservation => Requirement::RemoteGc,
            Self::ScopeNodeCreditReservation => Requirement::AccountingScopeOwnership,
            Self::AccountingSelfCreditReservation => Requirement::AccountingSelfOwnership,
            Self::OpenAdmissionReservation => Requirement::OpenAdmission,
            Self::OpenStatusReservation => Requirement::TerminalStatusRegistry,
            Self::UrlIndexReservation => Requirement::OpenSourceIndexAndCatalog,
            Self::ExistingIdentifierIndexReservation => Requirement::ExistingIdentifierIndex,
            Self::RuntimeInternInitializationProfile => Requirement::RuntimeInternBudget,
            Self::MemoryHeadroomGate => Requirement::MemoryPressure,
        }
    }

    pub const fn typed_entry_point(self) -> AggregateTypedEntryPoint {
        match self {
            Self::ValidationEstimateAndActualDispatch => {
                AggregateTypedEntryPoint::ValidateDispatchFormula
            }
            Self::DecoderAggregateReservation => {
                AggregateTypedEntryPoint::PrepareDecoderReservation
            }
            Self::PlanningCrossProductArithmetic => {
                AggregateTypedEntryPoint::ValidatePlanningCrossProduct
            }
            Self::SessionPartitionArithmetic => {
                AggregateTypedEntryPoint::ValidateSessionPartitionFormula
            }
            Self::SessionDescriptorOriginArithmetic => {
                AggregateTypedEntryPoint::ValidateSessionDescriptorFormula
            }
            Self::RegistrationMetadataHeadroomReservation => {
                AggregateTypedEntryPoint::PrepareRegistrationMetadataFormula
            }
            Self::RangeBodyPumpReservation => {
                AggregateTypedEntryPoint::PrepareRangeBodyPumpReservation
            }
            Self::RemoteInternalTotalGate => {
                AggregateTypedEntryPoint::PrepareRemoteInternalReservation
            }
            Self::RemoteValidatorIngressReservation => {
                AggregateTypedEntryPoint::PrepareRemoteValidatorIngress
            }
            Self::RemoteValidatorRetainedReservation => {
                AggregateTypedEntryPoint::PrepareRemoteValidatorRetained
            }
            Self::RemoteValidatorEgressReservation => {
                AggregateTypedEntryPoint::PrepareRemoteValidatorEgress
            }
            Self::LifecycleRetentionBundle => AggregateTypedEntryPoint::PrepareLifecycleReservation,
            Self::PresentationActivationReservation => {
                AggregateTypedEntryPoint::ActivatePresentation
            }
            Self::PresentationLeaseReservation => {
                AggregateTypedEntryPoint::AcquirePresentationLease
            }
            Self::PageParkedResponseOwnerReservation => {
                AggregateTypedEntryPoint::PreparePageParkedResponseOwnerReservation
            }
            Self::SharedFrameWorkUnitReservation => {
                AggregateTypedEntryPoint::PrepareSharedFrameWork
            }
            Self::ScopeNodeCreditReservation => {
                AggregateTypedEntryPoint::CreateChildScopeWithNodeCredit
            }
            Self::AccountingSelfCreditReservation => {
                AggregateTypedEntryPoint::BeginReservationPlanWithAccountingCredit
            }
            Self::OpenAdmissionReservation => {
                AggregateTypedEntryPoint::PrepareOpenAdmissionReservation
            }
            Self::OpenStatusReservation => AggregateTypedEntryPoint::PrepareOpenStatusReservation,
            Self::UrlIndexReservation => AggregateTypedEntryPoint::PrepareUrlIndexReservation,
            Self::ExistingIdentifierIndexReservation => {
                AggregateTypedEntryPoint::PrepareExistingIdentifierIndexReservation
            }
            Self::RuntimeInternInitializationProfile => {
                AggregateTypedEntryPoint::ValidateRuntimeInternInitializationProfile
            }
            Self::MemoryHeadroomGate => AggregateTypedEntryPoint::ValidateMemoryHeadroom,
        }
    }
}

/// Failure to turn a draft into a production-use capability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitProfileCompletionError {
    Unfrozen {
        key: WebRemoteLimitKey,
        stage: MeasurementStage,
        evidence: EvidenceKind,
    },
    ArithmeticOverflow {
        constraint: LimitProfileConstraint,
    },
    ConstraintViolation {
        constraint: LimitProfileConstraint,
    },
}

impl fmt::Display for LimitProfileCompletionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unfrozen { key, .. } => write!(
                formatter,
                "limit {} is not frozen",
                key.definition().stable_name
            ),
            Self::ArithmeticOverflow { constraint } => {
                write!(
                    formatter,
                    "limit constraint arithmetic overflow: {constraint:?}"
                )
            }
            Self::ConstraintViolation { constraint } => {
                write!(formatter, "limit constraint violation: {constraint:?}")
            }
        }
    }
}

impl std::error::Error for LimitProfileCompletionError {}

/// A complete, constraint-checked tooling artifact.
///
/// This value deliberately cannot start production accounting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedFrozenWebRemoteLimitsV1 {
    profile_revision: u64,
    values: [NonZeroU64; WEB_REMOTE_LIMIT_COUNT],
    sources: [FrozenLimitValueSource; WEB_REMOTE_LIMIT_COUNT],
}

impl ValidatedFrozenWebRemoteLimitsV1 {
    pub const fn version() -> WebRemoteLimitsProfileVersion {
        WebRemoteLimitsProfileVersion::V1
    }

    pub const fn profile_revision(&self) -> u64 {
        self.profile_revision
    }

    pub fn frozen(&self, key: WebRemoteLimitKey) -> FrozenWebRemoteLimit {
        FrozenWebRemoteLimit {
            value: self.values[key.index()],
            source: self.sources[key.index()],
        }
    }
}

/// Sealed V1 production limits.
///
/// There is intentionally no public constructor or draft-to-production conversion.
/// A future checked-in canonical artifact must add a private constructor in this module.
///
/// A tooling draft or validated artifact cannot start production accounting:
///
/// ```compile_fail
/// use re_web::remote_limits::DraftWebRemoteLimitsV1;
///
/// let draft = DraftWebRemoteLimitsV1::new();
/// let _root = draft.start_accounting_root();
/// ```
///
/// A complete tooling artifact remains disarmed:
///
/// ```compile_fail
/// use re_web::remote_limits::ValidatedFrozenWebRemoteLimitsV1;
///
/// fn cannot_start(artifact: ValidatedFrozenWebRemoteLimitsV1) {
///     let _root = artifact.start_accounting_root();
/// }
/// ```
///
/// Measurement evidence is provenance and cannot be upgraded into a production capability:
///
/// ```compile_fail
/// use re_web::remote_limits::{MeasurementEvidence, ProductionWebRemoteLimitsV1};
///
/// fn start_production(_: ProductionWebRemoteLimitsV1) {}
/// fn cannot_upgrade(evidence: MeasurementEvidence) {
///     start_production(evidence.into());
/// }
/// ```
///
/// The remote-MCAP authority is not part of the native API:
///
/// ```compile_fail,ignore-wasm32
/// use re_web::remote_limits::WebRemoteMcapAccountingCapability;
/// ```
pub struct ProductionWebRemoteLimitsV1 {
    values: [NonZeroU64; WEB_REMOTE_LIMIT_COUNT],
}

/// The sealed production authority for Web remote-MCAP accounting.
///
/// It exists only in Wasm builds, cannot be constructed outside this module, and is deliberately
/// absent from native and non-MCAP Web entry points.
#[cfg(target_arch = "wasm32")]
pub struct WebRemoteMcapAccountingCapability {
    limits: ProductionWebRemoteLimitsV1,
}

#[cfg(target_arch = "wasm32")]
impl WebRemoteMcapAccountingCapability {
    /// Consumes the one-shot capability and starts the module accounting root.
    pub fn start_accounting_root(
        self,
    ) -> Result<WasmModuleLimitAccountingRoot, ScopeAccountingError> {
        self.limits.start_accounting_root_inner()
    }
}

impl fmt::Debug for ProductionWebRemoteLimitsV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProductionWebRemoteLimitsV1")
            .field("version", &WebRemoteLimitsProfileVersion::V1)
            .field("values", &"<sealed>")
            .finish()
    }
}

impl ProductionWebRemoteLimitsV1 {
    #[cfg(rerun_mcap_phase_a_proof_v1)]
    pub(crate) fn start_phase_a_measurement_root_v1()
    -> Result<WasmModuleLimitAccountingRoot, ScopeAccountingError> {
        let limit = NonZeroU64::new(1_u64 << 40).expect("the proof limit is non-zero");
        Self {
            values: [limit; WEB_REMOTE_LIMIT_COUNT],
        }
        .start_accounting_root_inner()
    }

    fn raw_limit_value(&self, key: WebRemoteLimitKey) -> NonZeroU64 {
        self.values[key.index()]
    }

    /// Projects the frozen MCAP-006 values into the additive remote interner foundation.
    ///
    /// This does not initialize the module singleton or grant a decoder transaction.
    #[cfg(any(test, target_arch = "wasm32"))]
    #[cfg_attr(
        all(target_arch = "wasm32", not(test)),
        expect(
            dead_code,
            reason = "the remote decoder consumes this sealed projection in MCAP-079/081"
        )
    )]
    pub(crate) fn remote_mcap_runtime_intern_limits(
        &self,
    ) -> re_string_interner::bounded_runtime_intern::RemoteMcapRuntimeInternLimits {
        use WebRemoteLimitKey as Key;

        re_string_interner::bounded_runtime_intern::RemoteMcapRuntimeInternLimits::from_profile_values(
            self.raw_limit_value(Key::RuntimeInternStringBytes),
            self.raw_limit_value(Key::RuntimeInternEntryAndCapacityBytes),
            self.raw_limit_value(Key::RuntimeInternCensusIdentifiers),
            self.raw_limit_value(Key::RuntimeInternCensusRetainedBytes),
            self.raw_limit_value(Key::RuntimeInternCandidatePeakBytes),
        )
    }

    fn scalar_value(&self, key: ScalarObservationKey) -> ScalarObservationValue {
        ScalarObservationValue(self.raw_limit_value(key.schema_key()))
    }

    fn formula_value(&self, key: FormulaConstantKey) -> FormulaConstantValue {
        FormulaConstantValue(self.raw_limit_value(key.schema_key()))
    }

    fn burn_value(&self, key: ModuleLifetimeBurnKey) -> ModuleLifetimeBurnValue {
        ModuleLifetimeBurnValue(self.raw_limit_value(key.schema_key()))
    }

    fn reclaimable_value(&self, key: ReclaimableConcurrentKey) -> ReclaimableConcurrentValue {
        ReclaimableConcurrentValue(self.raw_limit_value(key.schema_key()))
    }

    fn threshold_value(
        &self,
        key: TelemetryAcceptanceThresholdKey,
    ) -> TelemetryAcceptanceThresholdValue {
        TelemetryAcceptanceThresholdValue(self.values[key.schema_key().index()])
    }

    #[cfg(test)]
    fn from_validated_artifact_for_test(artifact: &ValidatedFrozenWebRemoteLimitsV1) -> Self {
        Self {
            values: artifact.values,
        }
    }

    #[cfg(test)]
    fn raw_test_value(&self, key: WebRemoteLimitKey) -> NonZeroU64 {
        self.values[key.index()]
    }

    fn validate_scalar(
        &self,
        key: ScalarObservationKey,
        actual: u64,
    ) -> Result<(), CheckedLimitArithmeticError> {
        if actual > self.scalar_value(key).get() {
            return Err(CheckedLimitArithmeticError::LimitExceeded {
                key: key.schema_key(),
            });
        }
        Ok(())
    }

    fn observe_threshold(
        &self,
        key: TelemetryAcceptanceThresholdKey,
        actual: u64,
    ) -> TelemetryThresholdObservation {
        if actual <= self.threshold_value(key).get() {
            TelemetryThresholdObservation::WithinThreshold
        } else {
            TelemetryThresholdObservation::ExceededThreshold
        }
    }

    fn checked_scalar_sum(
        &self,
        key: ScalarObservationKey,
        values: impl IntoIterator<Item = u64>,
    ) -> Result<u64, CheckedLimitArithmeticError> {
        let mut total = 0_u64;
        for value in values {
            total =
                total
                    .checked_add(value)
                    .ok_or_else(|| CheckedLimitArithmeticError::Overflow {
                        key: key.schema_key(),
                    })?;
        }
        self.validate_scalar(key, total)?;
        Ok(total)
    }

    fn checked_scalar_product(
        &self,
        key: ScalarObservationKey,
        left: u64,
        right: u64,
    ) -> Result<u64, CheckedLimitArithmeticError> {
        let product =
            left.checked_mul(right)
                .ok_or_else(|| CheckedLimitArithmeticError::Overflow {
                    key: key.schema_key(),
                })?;
        self.validate_scalar(key, product)?;
        Ok(product)
    }
}

fn validate_complete_profile(
    values: &[NonZeroU64; WEB_REMOTE_LIMIT_COUNT],
) -> Result<(), LimitProfileCompletionError> {
    for constraint in LimitProfileConstraint::ALL {
        validate_profile_constraint(values, constraint)?;
    }
    Ok(())
}

// V1 uses a target-portable upper bound for one `(u64, &'static str)` entry (24 bytes),
// four conservative buckets, an equal control/allocation allowance, and 64 fixed bytes.
// This is stricter than the wasm32 representation and matches the lower-layer V1 formula on
// 64-bit tooling hosts.
const RUNTIME_INTERN_MINIMUM_SIDE_MAP_CAPACITY_BYTES_V1: u64 = 256;
const RUNTIME_INTERN_MINIMUM_ENTRY_AND_CAPACITY_BYTES_V1: u64 = 280;

fn validate_profile_constraint(
    values: &[NonZeroU64; WEB_REMOTE_LIMIT_COUNT],
    constraint: LimitProfileConstraint,
) -> Result<(), LimitProfileCompletionError> {
    let get = |key: WebRemoteLimitKey| values[key.index()].get();
    match constraint {
        LimitProfileConstraint::BatchFitsTerminalRegistry => require_less_or_equal(
            get(WebRemoteLimitKey::OpenBatchUrls),
            get(WebRemoteLimitKey::OpenSourceTerminalEntries),
            constraint,
        ),
        LimitProfileConstraint::TerminalRegistryFitsStatusRegistry => require_less_or_equal(
            get(WebRemoteLimitKey::OpenSourceTerminalEntries),
            get(WebRemoteLimitKey::OpenSourceStatusSlots),
            constraint,
        ),
        LimitProfileConstraint::TerminalRetainedBytesCoverTerminalEntries => {
            let required = checked_mul_constraint(
                get(WebRemoteLimitKey::OpenSourceTerminalEntries),
                get(WebRemoteLimitKey::RemoteTerminalStatusBytes),
                constraint,
            )?;
            require_less_or_equal(
                required,
                get(WebRemoteLimitKey::OpenSourceTerminalRetainedBytes),
                constraint,
            )
        }
        LimitProfileConstraint::StatusComponentsFitLiveOwners => {
            let required = checked_sum_constraint(
                [
                    WebRemoteLimitKey::DisarmedStrictSources,
                    WebRemoteLimitKey::PendingFormatSniffs,
                    WebRemoteLimitKey::RemoteSessionSlots,
                ]
                .map(get),
                constraint,
            )?;
            require_less_or_equal(
                required,
                get(WebRemoteLimitKey::OpenSourceLiveStatusOwners),
                constraint,
            )
        }
        LimitProfileConstraint::StatusRetainedBytesCoverOwnerSlots => {
            let required = checked_mul_constraint(
                get(WebRemoteLimitKey::OpenSourceStatusSlots),
                get(WebRemoteLimitKey::RemoteTerminalStatusBytes),
                constraint,
            )?;
            require_less_or_equal(
                required,
                get(WebRemoteLimitKey::OpenSourceStatusRetainedBytes),
                constraint,
            )
        }
        LimitProfileConstraint::PublishedLiveAndTerminalFitStatusSlots => {
            let required = checked_add_constraint(
                get(WebRemoteLimitKey::OpenSourceLiveStatusOwners),
                get(WebRemoteLimitKey::OpenSourceTerminalEntries),
                constraint,
            )?;
            require_less_or_equal(
                required,
                get(WebRemoteLimitKey::OpenSourceStatusSlots),
                constraint,
            )
        }
        LimitProfileConstraint::FutureClaimsFitDisarmedClaims => require_less_or_equal(
            get(WebRemoteLimitKey::DisarmedStrictSources),
            get(WebRemoteLimitKey::DisarmedStatusClaims),
            constraint,
        ),
        LimitProfileConstraint::DeferredEvictionVictimsFitTerminalRegistry => {
            require_less_or_equal(
                get(WebRemoteLimitKey::DeferredEvictionVictims),
                get(WebRemoteLimitKey::OpenSourceTerminalEntries),
                constraint,
            )
        }
        LimitProfileConstraint::ReleasedLiveFutureAndTerminalFitStatusSlots => {
            let surviving = get(WebRemoteLimitKey::OpenSourceTerminalEntries)
                .checked_sub(get(WebRemoteLimitKey::DeferredEvictionVictims))
                .ok_or(LimitProfileCompletionError::ArithmeticOverflow { constraint })?;
            let required = checked_sum_constraint(
                [
                    get(WebRemoteLimitKey::OpenSourceLiveStatusOwners),
                    get(WebRemoteLimitKey::DisarmedStatusClaims),
                    surviving,
                ],
                constraint,
            )?;
            require_less_or_equal(
                required,
                get(WebRemoteLimitKey::OpenSourceStatusSlots),
                constraint,
            )
        }
        LimitProfileConstraint::StatusRetainedBytesCoverFutureClaimsAndEviction => {
            let owner_bytes = checked_mul_constraint(
                get(WebRemoteLimitKey::OpenSourceStatusSlots),
                get(WebRemoteLimitKey::RemoteTerminalStatusBytes),
                constraint,
            )?;
            let future_bytes = checked_mul_constraint(
                get(WebRemoteLimitKey::DisarmedStatusClaims),
                get(WebRemoteLimitKey::RemoteTerminalStatusBytes),
                constraint,
            )?;
            let required = checked_sum_constraint(
                [
                    owner_bytes,
                    future_bytes,
                    get(WebRemoteLimitKey::DeferredEvictionDescriptorBytes),
                ],
                constraint,
            )?;
            require_less_or_equal(
                required,
                get(WebRemoteLimitKey::OpenSourceStatusRetainedBytes),
                constraint,
            )
        }
        LimitProfileConstraint::LifecycleWorstCaseRustDeliveryCount
        | LimitProfileConstraint::LifecycleWorstCaseListenerErrorCredits
        | LimitProfileConstraint::LifecycleWorstCaseRustDeliveryBytes
        | LimitProfileConstraint::LifecycleWorstCaseTypescriptDeliveryCount
        | LimitProfileConstraint::LifecycleWorstCaseTypescriptDeliveryBytes => {
            let (listener_errors, total) = lifecycle_delivery_formula(&get, constraint)?;
            match constraint {
                LimitProfileConstraint::LifecycleWorstCaseRustDeliveryCount => {
                    require_less_or_equal(
                        total,
                        get(WebRemoteLimitKey::LifecycleOutstandingDeliveries),
                        constraint,
                    )
                }
                LimitProfileConstraint::LifecycleWorstCaseListenerErrorCredits => {
                    require_less_or_equal(
                        listener_errors,
                        get(WebRemoteLimitKey::LifecycleListenerErrorCredits),
                        constraint,
                    )
                }
                LimitProfileConstraint::LifecycleWorstCaseRustDeliveryBytes => {
                    let bytes = checked_mul_constraint(
                        total,
                        get(WebRemoteLimitKey::SingleLifecycleEventBytes),
                        constraint,
                    )?;
                    require_less_or_equal(
                        bytes,
                        get(WebRemoteLimitKey::LifecycleOutstandingDeliveryBytes),
                        constraint,
                    )
                }
                LimitProfileConstraint::LifecycleWorstCaseTypescriptDeliveryCount => {
                    require_less_or_equal(
                        total,
                        get(WebRemoteLimitKey::TypescriptDispatcherItems),
                        constraint,
                    )
                }
                LimitProfileConstraint::LifecycleWorstCaseTypescriptDeliveryBytes => {
                    let bytes = checked_mul_constraint(
                        total,
                        get(WebRemoteLimitKey::SingleLifecycleEventBytes),
                        constraint,
                    )?;
                    require_less_or_equal(
                        bytes,
                        get(WebRemoteLimitKey::TypescriptDispatcherRetainedBytes),
                        constraint,
                    )
                }
                _ => unreachable!("outer match restricts lifecycle constraints"),
            }
        }
        LimitProfileConstraint::PromiseClosureBytesCoverOperations => {
            let required = checked_mul_constraint(
                get(WebRemoteLimitKey::OpenOperations),
                get(WebRemoteLimitKey::PromiseClosureBytesPerOperation),
                constraint,
            )?;
            require_less_or_equal(
                required,
                get(WebRemoteLimitKey::PromiseClosureBytes),
                constraint,
            )
        }
        LimitProfileConstraint::WrapperCacheCoversOperationsAndRecordings => {
            let required = checked_add_constraint(
                get(WebRemoteLimitKey::OpenOperations),
                get(WebRemoteLimitKey::PublicRecordingHandles),
                constraint,
            )?;
            require_less_or_equal(
                required,
                get(WebRemoteLimitKey::WrapperCacheEntries),
                constraint,
            )
        }
        LimitProfileConstraint::RecordingHandlesFitTombstoneRetainers => require_less_or_equal(
            get(WebRemoteLimitKey::PublicRecordingHandles),
            get(WebRemoteLimitKey::RemovedTombstoneRetainers),
            constraint,
        ),
        LimitProfileConstraint::FingerprintTokensPerBucketFitGlobal => require_less_or_equal(
            get(WebRemoteLimitKey::UrlFingerprintTokensPerBucket),
            get(WebRemoteLimitKey::UrlFingerprintTokens),
            constraint,
        ),
        LimitProfileConstraint::RemoteGcFitsSharedCpuAllowance => require_less_or_equal(
            get(WebRemoteLimitKey::RemoteGcCpuAllowanceMicrosPerFrame),
            get(WebRemoteLimitKey::RemoteCpuAllowanceMicrosPerFrame),
            constraint,
        ),
        LimitProfileConstraint::RemoteCpuWorkFitsSharedFrameCap => require_less_or_equal(
            get(WebRemoteLimitKey::RemoteCpuWorkUnitsPerFrame),
            get(WebRemoteLimitKey::SharedRemoteWorkUnitsPerFrame),
            constraint,
        ),
        LimitProfileConstraint::RemoteGcWorkFitsSharedFrameCap => require_less_or_equal(
            get(WebRemoteLimitKey::RemoteGcWorkUnitsPerFrame),
            get(WebRemoteLimitKey::SharedRemoteWorkUnitsPerFrame),
            constraint,
        ),
        LimitProfileConstraint::GlobalScopeNodeBytesCoverNodes => {
            let required = checked_mul_constraint(
                get(WebRemoteLimitKey::AccountingScopeNodesGlobal),
                get(WebRemoteLimitKey::AccountingScopeNodeBytes),
                constraint,
            )?;
            require_less_or_equal(
                required,
                get(WebRemoteLimitKey::AccountingScopeNodeBytesGlobal),
                constraint,
            )
        }
        LimitProfileConstraint::ParentScopeNodeBytesCoverNodes => {
            let required = checked_mul_constraint(
                get(WebRemoteLimitKey::AccountingChildScopeNodesPerParent),
                get(WebRemoteLimitKey::AccountingScopeNodeBytes),
                constraint,
            )?;
            require_less_or_equal(
                required,
                get(WebRemoteLimitKey::AccountingChildScopeBytesPerParent),
                constraint,
            )
        }
        LimitProfileConstraint::OperationSubscriptionsCoverRecordingHandles => {
            let capacity = checked_mul_constraint(
                get(WebRemoteLimitKey::OpenOperations),
                get(WebRemoteLimitKey::OperationRecordingSubscriptions),
                constraint,
            )?;
            require_less_or_equal(
                get(WebRemoteLimitKey::PublicRecordingHandles),
                capacity,
                constraint,
            )
        }
        LimitProfileConstraint::RuntimeInternMinimumSideMapFitsBudgets => {
            require_less_or_equal(
                RUNTIME_INTERN_MINIMUM_SIDE_MAP_CAPACITY_BYTES_V1,
                get(WebRemoteLimitKey::RuntimeInternCandidatePeakBytes),
                constraint,
            )?;
            require_less_or_equal(
                RUNTIME_INTERN_MINIMUM_ENTRY_AND_CAPACITY_BYTES_V1,
                get(WebRemoteLimitKey::RuntimeInternEntryAndCapacityBytes),
                constraint,
            )
        }
    }
}

fn lifecycle_delivery_formula(
    get: &impl Fn(WebRemoteLimitKey) -> u64,
    constraint: LimitProfileConstraint,
) -> Result<(u64, u64), LimitProfileCompletionError> {
    let recording_deliveries =
        checked_mul_constraint(get(WebRemoteLimitKey::RecordingsPerSource), 3, constraint)?;
    let deliveries_per_operation = checked_add_constraint(3, recording_deliveries, constraint)?;
    let origin = checked_mul_constraint(
        get(WebRemoteLimitKey::OpenOperations),
        deliveries_per_operation,
        constraint,
    )?;
    let listener_errors = checked_mul_constraint(
        origin,
        get(WebRemoteLimitKey::ListenerErrorNotificationsPerEvent),
        constraint,
    )?;
    let total = checked_add_constraint(origin, listener_errors, constraint)?;
    Ok((listener_errors, total))
}

fn checked_add_constraint(
    left: u64,
    right: u64,
    constraint: LimitProfileConstraint,
) -> Result<u64, LimitProfileCompletionError> {
    left.checked_add(right)
        .ok_or(LimitProfileCompletionError::ArithmeticOverflow { constraint })
}

fn checked_sum_constraint(
    values: impl IntoIterator<Item = u64>,
    constraint: LimitProfileConstraint,
) -> Result<u64, LimitProfileCompletionError> {
    values.into_iter().try_fold(0_u64, |total, value| {
        checked_add_constraint(total, value, constraint)
    })
}

fn checked_mul_constraint(
    left: u64,
    right: u64,
    constraint: LimitProfileConstraint,
) -> Result<u64, LimitProfileCompletionError> {
    left.checked_mul(right)
        .ok_or(LimitProfileCompletionError::ArithmeticOverflow { constraint })
}

fn require_less_or_equal(
    left: u64,
    right: u64,
    constraint: LimitProfileConstraint,
) -> Result<(), LimitProfileCompletionError> {
    if left > right {
        return Err(LimitProfileCompletionError::ConstraintViolation { constraint });
    }
    Ok(())
}

/// Failure from checked arithmetic against a complete profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckedLimitArithmeticError {
    Overflow { key: WebRemoteLimitKey },
    LimitExceeded { key: WebRemoteLimitKey },
}

impl fmt::Display for CheckedLimitArithmeticError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Overflow { key } => write!(
                formatter,
                "resource arithmetic overflow for {}",
                key.definition().stable_name
            ),
            Self::LimitExceeded { key } => write!(
                formatter,
                "resource limit exceeded for {}",
                key.definition().stable_name
            ),
        }
    }
}

impl std::error::Error for CheckedLimitArithmeticError {}

#[cfg(any(test, target_arch = "wasm32"))]
static NEXT_ACCOUNTING_ROOT_NONCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct AccountingScopeIdentity {
    root_nonce: NonZeroU64,
    sequence: NonZeroU64,
    generation: NonZeroU64,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct ReservationIdentity {
    root_nonce: NonZeroU64,
    sequence: NonZeroU64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScopedLimitUsage {
    pub current: u64,
    pub high_watermark: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct ScopedUsageKey {
    scope: AccountingScopeIdentity,
    key: WebRemoteLimitKey,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ScopeNodeUsage {
    current_nodes: u64,
    node_high_watermark: u64,
    current_bytes: u64,
    byte_high_watermark: u64,
}

#[derive(Clone, PartialEq, Eq)]
struct AccountingScopeRecord {
    kind: WebRemoteLimitScope,
    parent: Option<AccountingScopeIdentity>,
    active: bool,
    active_children: u64,
    node_bytes: u64,
}

#[derive(Clone, PartialEq, Eq)]
struct ReservationRecord {
    totals: BTreeMap<ScopedUsageKey, u64>,
    /// Preallocated to the reservation request count before any accounting mutation.
    usage_node_claims: Vec<ScopedUsageKey>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct AccountingSelfUsage {
    prepared_records: u64,
    active_records: u64,
    request_entries: u64,
    usage_nodes: u64,
    prepared_bytes: u64,
    active_bytes: u64,
    request_bytes: u64,
    usage_node_bytes: u64,
    release_scratch_entries: u64,
    release_scratch_bytes: u64,
}

#[derive(Clone, PartialEq, Eq)]
struct ScopeAccountingState {
    revision: u64,
    next_scope_sequence: u64,
    next_scope_generation: u64,
    next_reservation_sequence: u64,
    scopes: BTreeMap<AccountingScopeIdentity, AccountingScopeRecord>,
    usage: BTreeMap<ScopedUsageKey, ScopedLimitUsage>,
    global_node_usage: ScopeNodeUsage,
    child_node_usage: BTreeMap<AccountingScopeIdentity, ScopeNodeUsage>,
    reservations: BTreeMap<ReservationIdentity, ReservationRecord>,
    accounting_self: AccountingSelfUsage,
    phase_a_byte_ledger: PhaseAByteLedgerV1,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PhaseAByteLedgerV1 {
    current_bytes: u64,
    high_water_bytes: u64,
    overflowed: bool,
}

/// An exact snapshot of scope ownership, usage, revisions, and active reservations.
#[derive(Clone, PartialEq, Eq)]
#[cfg(test)]
pub(crate) struct ScopeAccountingSnapshot(ScopeAccountingState);

#[cfg(test)]
impl ScopeAccountingSnapshot {
    #[cfg(test)]
    fn usage(
        &self,
        scope: &WebRemoteAccountingScope,
        key: WebRemoteLimitKey,
    ) -> Option<ScopedLimitUsage> {
        self.0
            .usage
            .get(&ScopedUsageKey {
                scope: scope.lease.identity,
                key,
            })
            .copied()
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn same_except_revision_and_reservation_sequence(&self, other: &Self) -> bool {
        let mut left = self.clone();
        let mut right = other.clone();
        left.0.revision = 0;
        right.0.revision = 0;
        left.0.next_reservation_sequence = 0;
        right.0.next_reservation_sequence = 0;
        left == right
    }
}

#[cfg(test)]
impl fmt::Debug for ScopeAccountingSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ScopeAccountingSnapshot")
            .field("revision", &self.0.revision)
            .field("scope_count", &self.0.scopes.len())
            .field("usage_count", &self.0.usage.len())
            .field("reservation_count", &self.0.reservations.len())
            .field(
                "active_scope_nodes",
                &self.0.global_node_usage.current_nodes,
            )
            .finish()
    }
}

/// A fixed-size, low-cardinality view of the accounting allocator itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AccountingScalarSnapshot {
    pub active_scope_nodes: u64,
    pub prepared_reservation_records: u64,
    pub active_reservation_records: u64,
    pub reservation_request_entries: u64,
    pub usage_nodes: u64,
    pub prepared_retained_bytes: u64,
    pub active_retained_bytes: u64,
    pub request_retained_bytes: u64,
    pub usage_node_retained_bytes: u64,
    pub release_scratch_entries: u64,
    pub release_scratch_retained_bytes: u64,
}

struct ScopeAccountingRootInner {
    limits: ProductionWebRemoteLimitsV1,
    state: RefCell<ScopeAccountingState>,
}

struct AccountingScopeLease {
    root: Weak<ScopeAccountingRootInner>,
    identity: AccountingScopeIdentity,
    kind: WebRemoteLimitScope,
    internal_projection_claims: Cell<u8>,
    _parent_lease: Option<Rc<Self>>,
}

impl Drop for AccountingScopeLease {
    fn drop(&mut self) {
        let Some(root) = self.root.upgrade() else {
            return;
        };
        let _close_result = close_scope_inner(&root, self.identity);
    }
}

#[derive(Clone)]
struct WebRemoteAccountingScope {
    lease: Rc<AccountingScopeLease>,
}

impl WebRemoteAccountingScope {
    fn close(&self) -> Result<(), ScopeAccountingError> {
        let root = self
            .lease
            .root
            .upgrade()
            .ok_or(ScopeAccountingError::RootStopped)?;
        close_scope_inner(&root, self.lease.identity)
    }

    fn validate_scalar(
        &self,
        key: ScalarObservationKey,
        actual: u64,
    ) -> Result<(), ScopeAccountingError> {
        let root = self
            .lease
            .root
            .upgrade()
            .ok_or(ScopeAccountingError::RootStopped)?;
        validate_scope_and_key(
            &root,
            self,
            key.schema_key(),
            WebRemoteAccountingKind::ScalarObservation,
        )?;
        root.limits
            .validate_scalar(key, actual)
            .map_err(|_limit| ScopeAccountingError::LimitExceeded)
    }

    fn checked_scalar_sum(
        &self,
        key: ScalarObservationKey,
        values: impl IntoIterator<Item = u64>,
    ) -> Result<u64, ScopeAccountingError> {
        let root = self
            .lease
            .root
            .upgrade()
            .ok_or(ScopeAccountingError::RootStopped)?;
        validate_scope_and_key(
            &root,
            self,
            key.schema_key(),
            WebRemoteAccountingKind::ScalarObservation,
        )?;
        root.limits
            .checked_scalar_sum(key, values)
            .map_err(|error| match error {
                CheckedLimitArithmeticError::Overflow { .. } => {
                    ScopeAccountingError::ArithmeticOverflow
                }
                CheckedLimitArithmeticError::LimitExceeded { .. } => {
                    ScopeAccountingError::LimitExceeded
                }
            })
    }

    fn checked_scalar_product(
        &self,
        key: ScalarObservationKey,
        left: u64,
        right: u64,
    ) -> Result<u64, ScopeAccountingError> {
        let root = self
            .lease
            .root
            .upgrade()
            .ok_or(ScopeAccountingError::RootStopped)?;
        validate_scope_and_key(
            &root,
            self,
            key.schema_key(),
            WebRemoteAccountingKind::ScalarObservation,
        )?;
        root.limits
            .checked_scalar_product(key, left, right)
            .map_err(|error| match error {
                CheckedLimitArithmeticError::Overflow { .. } => {
                    ScopeAccountingError::ArithmeticOverflow
                }
                CheckedLimitArithmeticError::LimitExceeded { .. } => {
                    ScopeAccountingError::LimitExceeded
                }
            })
    }

    fn observe_threshold(
        &self,
        key: TelemetryAcceptanceThresholdKey,
        actual: u64,
    ) -> Result<TelemetryThresholdObservation, ScopeAccountingError> {
        let root = self
            .lease
            .root
            .upgrade()
            .ok_or(ScopeAccountingError::RootStopped)?;
        validate_scope_identity(&root, self, key.schema_key().definition().scope)?;
        Ok(root.limits.observe_threshold(key, actual))
    }
}

impl fmt::Debug for WebRemoteAccountingScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WebRemoteAccountingScope")
            .field("kind", &self.lease.kind)
            .field("identity", &"<opaque>")
            .finish()
    }
}

macro_rules! define_typed_scope {
    ($name:ident, $kind:ident) => {
        /// An unforgeable generation of one typed accounting scope.
        #[derive(Clone)]
        pub struct $name(WebRemoteAccountingScope);

        impl $name {
            pub fn close(&self) -> Result<(), ScopeAccountingError> {
                self.0.close()
            }

            pub fn validate_scalar(
                &self,
                key: ScalarObservationKey,
                actual: u64,
            ) -> Result<(), ScopeAccountingError> {
                self.0.validate_scalar(key, actual)
            }

            pub fn checked_scalar_sum(
                &self,
                key: ScalarObservationKey,
                values: impl IntoIterator<Item = u64>,
            ) -> Result<u64, ScopeAccountingError> {
                self.0.checked_scalar_sum(key, values)
            }

            pub fn checked_scalar_product(
                &self,
                key: ScalarObservationKey,
                left: u64,
                right: u64,
            ) -> Result<u64, ScopeAccountingError> {
                self.0.checked_scalar_product(key, left, right)
            }

            pub fn observe_threshold(
                &self,
                key: TelemetryAcceptanceThresholdKey,
                actual: u64,
            ) -> Result<TelemetryThresholdObservation, ScopeAccountingError> {
                self.0.observe_threshold(key, actual)
            }

            pub fn reservation_request(
                &self,
                key: ReclaimableConcurrentKey,
                amount: NonZeroU64,
            ) -> ScopedReservationRequest<'_> {
                ScopedReservationRequest {
                    key,
                    amount,
                    scope: &self.0,
                }
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .debug_struct(stringify!($name))
                    .field("kind", &WebRemoteLimitScope::$kind)
                    .field("identity", &"<opaque>")
                    .finish()
            }
        }
    };
}

define_typed_scope!(ViewerAccountingScope, ViewerInstance);
define_typed_scope!(SourceAccountingScope, Source);
define_typed_scope!(SessionAccountingScope, Session);
define_typed_scope!(RequestAccountingScope, Request);
define_typed_scope!(AtomicBatchAccountingScope, AtomicBatch);
define_typed_scope!(OperationAccountingScope, Operation);
define_typed_scope!(SummaryAccountingScope, Summary);
define_typed_scope!(NestedRecordAccountingScope, NestedRecord);
define_typed_scope!(MessageIndexRegionAccountingScope, MessageIndexRegion);
define_typed_scope!(ChunkAccountingScope, Chunk);
define_typed_scope!(DecoderGroupAccountingScope, DecoderGroupPerChunk);
define_typed_scope!(PartitionAccountingScope, Partition);
define_typed_scope!(GenerationAccountingScope, Generation);
define_typed_scope!(WindowAccountingScope, Window);
define_typed_scope!(RangeResponseAccountingScope, RangeResponse);
define_typed_scope!(FrameAccountingScope, Frame);
define_typed_scope!(WorkUnitAccountingScope, WorkUnit);
define_typed_scope!(IngressItemAccountingScope, IngressItem);
define_typed_scope!(ChannelAccountingScope, Channel);
define_typed_scope!(UrlFingerprintBucketAccountingScope, UrlFingerprintBucket);
define_typed_scope!(PageExecutionAccountingScope, PageExecution);
define_typed_scope!(StoreAccountingScope, Store);

macro_rules! define_child_constructor {
    ($parent:ident, $method:ident, $child:ident, $kind:ident) => {
        impl $parent {
            pub fn $method(&self) -> Result<$child, ScopeAccountingError> {
                let root = self
                    .0
                    .lease
                    .root
                    .upgrade()
                    .ok_or(ScopeAccountingError::RootStopped)?;
                create_child_scope(&root, &self.0.lease, WebRemoteLimitScope::$kind).map($child)
            }
        }
    };
}

define_child_constructor!(
    ViewerAccountingScope,
    create_source_scope,
    SourceAccountingScope,
    Source
);
define_child_constructor!(
    ViewerAccountingScope,
    create_request_scope,
    RequestAccountingScope,
    Request
);
define_child_constructor!(
    ViewerAccountingScope,
    create_atomic_batch_scope,
    AtomicBatchAccountingScope,
    AtomicBatch
);
define_child_constructor!(
    ViewerAccountingScope,
    create_frame_scope,
    FrameAccountingScope,
    Frame
);
define_child_constructor!(
    ViewerAccountingScope,
    create_ingress_item_scope,
    IngressItemAccountingScope,
    IngressItem
);
define_child_constructor!(
    ViewerAccountingScope,
    create_channel_scope,
    ChannelAccountingScope,
    Channel
);
define_child_constructor!(
    ViewerAccountingScope,
    create_url_fingerprint_bucket_scope,
    UrlFingerprintBucketAccountingScope,
    UrlFingerprintBucket
);
define_child_constructor!(
    ViewerAccountingScope,
    create_page_execution_scope,
    PageExecutionAccountingScope,
    PageExecution
);
define_child_constructor!(
    ViewerAccountingScope,
    create_store_scope,
    StoreAccountingScope,
    Store
);

define_child_constructor!(
    SourceAccountingScope,
    create_session_scope,
    SessionAccountingScope,
    Session
);
define_child_constructor!(
    SourceAccountingScope,
    create_operation_scope,
    OperationAccountingScope,
    Operation
);
define_child_constructor!(
    SourceAccountingScope,
    create_summary_scope,
    SummaryAccountingScope,
    Summary
);
define_child_constructor!(
    SummaryAccountingScope,
    create_nested_record_scope,
    NestedRecordAccountingScope,
    NestedRecord
);

define_child_constructor!(
    SessionAccountingScope,
    create_generation_scope,
    GenerationAccountingScope,
    Generation
);
define_child_constructor!(
    SessionAccountingScope,
    create_partition_scope,
    PartitionAccountingScope,
    Partition
);
define_child_constructor!(
    SessionAccountingScope,
    create_window_scope,
    WindowAccountingScope,
    Window
);
define_child_constructor!(
    SessionAccountingScope,
    create_range_response_scope,
    RangeResponseAccountingScope,
    RangeResponse
);
define_child_constructor!(
    SessionAccountingScope,
    create_work_unit_scope,
    WorkUnitAccountingScope,
    WorkUnit
);
define_child_constructor!(
    SessionAccountingScope,
    create_message_index_region_scope,
    MessageIndexRegionAccountingScope,
    MessageIndexRegion
);
define_child_constructor!(
    SessionAccountingScope,
    create_store_scope,
    StoreAccountingScope,
    Store
);

define_child_constructor!(
    GenerationAccountingScope,
    create_chunk_scope,
    ChunkAccountingScope,
    Chunk
);
define_child_constructor!(
    GenerationAccountingScope,
    create_partition_scope,
    PartitionAccountingScope,
    Partition
);
define_child_constructor!(
    GenerationAccountingScope,
    create_work_unit_scope,
    WorkUnitAccountingScope,
    WorkUnit
);
define_child_constructor!(
    ChunkAccountingScope,
    create_decoder_group_scope,
    DecoderGroupAccountingScope,
    DecoderGroupPerChunk
);
define_child_constructor!(
    ChunkAccountingScope,
    create_nested_record_scope,
    NestedRecordAccountingScope,
    NestedRecord
);
define_child_constructor!(
    ChunkAccountingScope,
    create_work_unit_scope,
    WorkUnitAccountingScope,
    WorkUnit
);
define_child_constructor!(
    ChunkAccountingScope,
    create_channel_scope,
    ChannelAccountingScope,
    Channel
);
define_child_constructor!(
    RequestAccountingScope,
    create_range_response_scope,
    RangeResponseAccountingScope,
    RangeResponse
);
define_child_constructor!(
    RequestAccountingScope,
    create_work_unit_scope,
    WorkUnitAccountingScope,
    WorkUnit
);
define_child_constructor!(
    AtomicBatchAccountingScope,
    create_operation_scope,
    OperationAccountingScope,
    Operation
);
define_child_constructor!(
    FrameAccountingScope,
    create_work_unit_scope,
    WorkUnitAccountingScope,
    WorkUnit
);
define_child_constructor!(
    IngressItemAccountingScope,
    create_work_unit_scope,
    WorkUnitAccountingScope,
    WorkUnit
);
define_child_constructor!(
    PageExecutionAccountingScope,
    create_work_unit_scope,
    WorkUnitAccountingScope,
    WorkUnit
);
define_child_constructor!(
    RangeResponseAccountingScope,
    create_work_unit_scope,
    WorkUnitAccountingScope,
    WorkUnit
);
define_child_constructor!(
    MessageIndexRegionAccountingScope,
    create_work_unit_scope,
    WorkUnitAccountingScope,
    WorkUnit
);
define_child_constructor!(
    PartitionAccountingScope,
    create_work_unit_scope,
    WorkUnitAccountingScope,
    WorkUnit
);

/// The unique shared root for one Wasm module instance.
///
/// The typed parent graph has no enum-driven public escape hatch:
///
/// ```compile_fail
/// use re_web::remote_limits::{ViewerAccountingScope, WebRemoteLimitScope};
///
/// fn arbitrary_child(viewer: &ViewerAccountingScope) {
///     let _ = viewer.create_child(WebRemoteLimitScope::Source);
/// }
/// ```
///
/// Accounting kinds cannot cross production APIs:
///
/// ```compile_fail
/// use re_web::remote_limits::{FormulaConstantKey, ViewerAccountingScope};
///
/// fn formula_is_not_an_observation(viewer: &ViewerAccountingScope, key: FormulaConstantKey) {
///     let _ = viewer.validate_scalar(key, 1);
/// }
/// ```
///
/// ```compile_fail
/// use std::num::NonZeroU64;
/// use re_web::remote_limits::{ModuleLifetimeBurnKey, ViewerAccountingScope};
///
/// fn burn_is_not_reclaimable(viewer: &ViewerAccountingScope, key: ModuleLifetimeBurnKey) {
///     let _ = viewer.reservation_request(key, NonZeroU64::MIN);
/// }
/// ```
///
/// ```compile_fail
/// use std::num::NonZeroU64;
/// use re_web::remote_limits::{ReclaimableConcurrentKey, WasmModuleLimitAccountingRoot};
///
/// fn reclaimable_is_not_a_burn(
///     root: &WasmModuleLimitAccountingRoot,
///     key: ReclaimableConcurrentKey,
/// ) {
///     let _ = root.burn_module_lifetime(key, NonZeroU64::MIN);
/// }
/// ```
///
/// Module-lifetime burns deliberately have no refund operation:
///
/// ```compile_fail
/// use std::num::NonZeroU64;
/// use re_web::remote_limits::{ModuleLifetimeBurnKey, WasmModuleLimitAccountingRoot};
///
/// fn no_refund(root: &WasmModuleLimitAccountingRoot, key: ModuleLifetimeBurnKey) {
///     let _ = root.refund_module_lifetime(key, NonZeroU64::MIN);
/// }
/// ```
///
/// Full ownership snapshots are test-only and cannot retain production maps:
///
/// ```compile_fail
/// use re_web::remote_limits::WasmModuleLimitAccountingRoot;
///
/// fn no_full_snapshot(root: &WasmModuleLimitAccountingRoot) {
///     let _ = root.snapshot();
/// }
/// ```
///
/// Callers must acquire bounded plan credit before they can submit reservation entries:
///
/// ```compile_fail
/// use re_web::remote_limits::{ScopedReservationRequest, WasmModuleLimitAccountingRoot};
///
/// fn no_uncredited_iterator<'a>(
///     root: &WasmModuleLimitAccountingRoot,
///     requests: impl IntoIterator<Item = ScopedReservationRequest<'a>>,
/// ) {
///     let _ = root.prepare_reservations(requests);
/// }
/// ```
#[derive(Clone)]
pub struct WasmModuleLimitAccountingRoot {
    inner: Rc<ScopeAccountingRootInner>,
    module_scope: WebRemoteAccountingScope,
}

impl fmt::Debug for WasmModuleLimitAccountingRoot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WasmModuleLimitAccountingRoot")
            .field("identity", &"<opaque>")
            .finish()
    }
}

#[cfg(any(test, target_arch = "wasm32"))]
impl ProductionWebRemoteLimitsV1 {
    fn start_accounting_root_inner(
        self,
    ) -> Result<WasmModuleLimitAccountingRoot, ScopeAccountingError> {
        let root_nonce = NonZeroU64::new(
            NEXT_ACCOUNTING_ROOT_NONCE
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                    current.checked_add(1)
                })
                .map_err(|_identity_exhausted| ScopeAccountingError::IdentityExhausted)?,
        )
        .ok_or(ScopeAccountingError::IdentityExhausted)?;
        let module_identity = AccountingScopeIdentity {
            root_nonce,
            sequence: NonZeroU64::MIN,
            generation: NonZeroU64::MIN,
        };
        let mut scopes = BTreeMap::new();
        scopes.insert(
            module_identity,
            AccountingScopeRecord {
                kind: WebRemoteLimitScope::WasmModuleLifetime,
                parent: None,
                active: true,
                active_children: 0,
                node_bytes: 0,
            },
        );
        let inner = Rc::new(ScopeAccountingRootInner {
            limits: self,
            state: RefCell::new(ScopeAccountingState {
                revision: 0,
                next_scope_sequence: 2,
                next_scope_generation: 2,
                next_reservation_sequence: 1,
                scopes,
                usage: BTreeMap::new(),
                global_node_usage: ScopeNodeUsage::default(),
                child_node_usage: BTreeMap::new(),
                reservations: BTreeMap::new(),
                accounting_self: AccountingSelfUsage::default(),
                phase_a_byte_ledger: PhaseAByteLedgerV1::default(),
            }),
        });
        let module_scope = WebRemoteAccountingScope {
            lease: Rc::new(AccountingScopeLease {
                root: Rc::downgrade(&inner),
                identity: module_identity,
                kind: WebRemoteLimitScope::WasmModuleLifetime,
                internal_projection_claims: Cell::new(0),
                _parent_lease: None,
            }),
        };
        Ok(WasmModuleLimitAccountingRoot {
            inner,
            module_scope,
        })
    }

    #[cfg(test)]
    pub(crate) fn start_accounting_root(
        self,
    ) -> Result<WasmModuleLimitAccountingRoot, ScopeAccountingError> {
        self.start_accounting_root_inner()
    }
}

impl WasmModuleLimitAccountingRoot {
    #[cfg(rerun_mcap_phase_a_proof_v1)]
    pub(crate) fn phase_a_byte_ledger_v1(&self) -> (u64, bool) {
        let ledger = self.inner.state.borrow().phase_a_byte_ledger;
        (ledger.high_water_bytes, ledger.overflowed)
    }

    pub fn create_viewer_scope(&self) -> Result<ViewerAccountingScope, ScopeAccountingError> {
        create_child_scope(
            &self.inner,
            &self.module_scope.lease,
            WebRemoteLimitScope::ViewerInstance,
        )
        .map(ViewerAccountingScope)
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> ScopeAccountingSnapshot {
        ScopeAccountingSnapshot(self.inner.state.borrow().clone())
    }

    #[cfg(test)]
    fn prepare_reservations<'a>(
        &self,
        requests: impl IntoIterator<Item = ScopedReservationRequest<'a>>,
    ) -> Result<PreparedScopedReservations, ScopeAccountingError> {
        let requests = requests.into_iter().collect::<Vec<_>>();
        let request_count = u64::try_from(requests.len())
            .map_err(|_overflow| ScopeAccountingError::ArithmeticOverflow)?;
        let Some(request_capacity) = NonZeroU64::new(request_count) else {
            return Err(ScopeAccountingError::EmptyReservation);
        };
        let mut plan = self.begin_reservation_plan(request_capacity)?;
        for request in requests {
            plan.push(request)?;
        }
        plan.prepare()
    }

    pub fn accounting_scalar_snapshot(&self) -> AccountingScalarSnapshot {
        let state = self.inner.state.borrow();
        let accounting = state.accounting_self;
        AccountingScalarSnapshot {
            active_scope_nodes: state.global_node_usage.current_nodes,
            prepared_reservation_records: accounting.prepared_records,
            active_reservation_records: accounting.active_records,
            reservation_request_entries: accounting.request_entries,
            usage_nodes: accounting.usage_nodes,
            prepared_retained_bytes: accounting.prepared_bytes,
            active_retained_bytes: accounting.active_bytes,
            request_retained_bytes: accounting.request_bytes,
            usage_node_retained_bytes: accounting.usage_node_bytes,
            release_scratch_entries: accounting.release_scratch_entries,
            release_scratch_retained_bytes: accounting.release_scratch_bytes,
        }
    }

    /// Permanently burns module-lifetime ownership.
    pub fn burn_module_lifetime(
        &self,
        key: ModuleLifetimeBurnKey,
        amount: NonZeroU64,
    ) -> Result<(), ScopeAccountingError> {
        validate_scope_and_key(
            &self.inner,
            &self.module_scope,
            key.schema_key(),
            WebRemoteAccountingKind::ModuleLifetimeBurn,
        )?;
        let usage_key = ScopedUsageKey {
            scope: self.module_scope.lease.identity,
            key: key.schema_key(),
        };
        let mut state = self.inner.state.borrow_mut();
        let installs_usage_node = !state.usage.contains_key(&usage_key);
        let usage_node_bytes = self
            .inner
            .limits
            .raw_limit_value(WebRemoteLimitKey::AccountingUsageNodeBytes)
            .get();
        if installs_usage_node {
            let next_nodes = checked_accounting_add(state.accounting_self.usage_nodes, 1)?;
            let next_bytes =
                checked_accounting_add(state.accounting_self.usage_node_bytes, usage_node_bytes)?;
            if next_nodes
                > self
                    .inner
                    .limits
                    .raw_limit_value(WebRemoteLimitKey::AccountingUsageNodes)
                    .get()
                || next_bytes
                    > self
                        .inner
                        .limits
                        .raw_limit_value(WebRemoteLimitKey::AccountingUsageNodeRetainedBytes)
                        .get()
            {
                return Err(ScopeAccountingError::LimitExceeded);
            }
        }
        let current = state.usage.get(&usage_key).copied().unwrap_or_default();
        let next = current
            .current
            .checked_add(amount.get())
            .ok_or(ScopeAccountingError::ArithmeticOverflow)?;
        if next > self.inner.limits.burn_value(key).get() {
            return Err(ScopeAccountingError::LimitExceeded);
        }
        let next_revision = state
            .revision
            .checked_add(1)
            .ok_or(ScopeAccountingError::RevisionExhausted)?;
        state.usage.insert(
            usage_key,
            ScopedLimitUsage {
                current: next,
                high_watermark: next,
            },
        );
        if installs_usage_node {
            state.accounting_self.usage_nodes += 1;
            state.accounting_self.usage_node_bytes += usage_node_bytes;
        }
        state.revision = next_revision;
        Ok(())
    }

    pub fn begin_reservation_plan<'a>(
        &self,
        request_capacity: NonZeroU64,
    ) -> Result<ScopedReservationPlan<'a>, ScopeAccountingError> {
        let capacity = usize::try_from(request_capacity.get())
            .map_err(|_conversion_error| ScopeAccountingError::ArithmeticOverflow)?;
        let credit = acquire_prepared_accounting_credit(&self.inner, request_capacity)?;
        let mut requests = Vec::new();
        if requests.try_reserve_exact(capacity).is_err() {
            release_prepared_accounting_credit(&self.inner, credit);
            return Err(ScopeAccountingError::AllocationFailed);
        }
        Ok(ScopedReservationPlan {
            root: self.clone(),
            requests,
            capacity,
            credit: Some(credit),
        })
    }

    fn begin_internal_reservation_plan<'a>(
        &self,
        request_capacity: NonZeroU64,
    ) -> Result<InternalReservationPlan<'a>, ScopeAccountingError> {
        let capacity = usize::try_from(request_capacity.get())
            .map_err(|_conversion_error| ScopeAccountingError::ArithmeticOverflow)?;
        let credit = acquire_prepared_accounting_credit(&self.inner, request_capacity)?;
        let mut requests = Vec::new();
        if requests.try_reserve_exact(capacity).is_err() {
            release_prepared_accounting_credit(&self.inner, credit);
            return Err(ScopeAccountingError::AllocationFailed);
        }
        Ok(InternalReservationPlan {
            root: self.clone(),
            requests,
            capacity,
            credit: Some(credit),
        })
    }

    fn finish_reservation_requests(
        &self,
        requests: Vec<InternalReservationRequest<'_>>,
        allow_aggregate: bool,
        mut accounting_credit: ReservationAccountingCredit,
    ) -> Result<PreparedScopedReservations, ScopeAccountingError> {
        let mut totals = BTreeMap::<ScopedUsageKey, u64>::new();
        let mut scope_leases = BTreeMap::<AccountingScopeIdentity, Rc<AccountingScopeLease>>::new();

        for request in requests {
            if !allow_aggregate && aggregate_reservation_family(request.key).is_some() {
                return Err(ScopeAccountingError::AggregateReservationRequired);
            }
            validate_scope_and_key(
                &self.inner,
                request.scope,
                request.key,
                WebRemoteAccountingKind::ReclaimableConcurrent,
            )?;
            let usage_key = ScopedUsageKey {
                scope: request.scope.lease.identity,
                key: request.key,
            };
            let total = totals.entry(usage_key).or_default();
            *total = total
                .checked_add(request.amount.get())
                .ok_or(ScopeAccountingError::ArithmeticOverflow)?;
            scope_leases
                .entry(request.scope.lease.identity)
                .or_insert_with(|| Rc::clone(&request.scope.lease));
        }
        if totals.is_empty() {
            return Err(ScopeAccountingError::EmptyReservation);
        }

        let state = self.inner.state.borrow();
        let committed_revision = state
            .revision
            .checked_add(1)
            .ok_or(ScopeAccountingError::RevisionExhausted)?;
        let reservation_sequence = NonZeroU64::new(state.next_reservation_sequence)
            .ok_or(ScopeAccountingError::IdentityExhausted)?;
        let next_reservation_sequence = state
            .next_reservation_sequence
            .checked_add(1)
            .ok_or(ScopeAccountingError::IdentityExhausted)?;
        let reservation_identity = ReservationIdentity {
            root_nonce: self.module_scope.lease.identity.root_nonce,
            sequence: reservation_sequence,
        };

        for (usage_key, amount) in &totals {
            let current = state.usage.get(usage_key).copied().unwrap_or_default();
            let next = current
                .current
                .checked_add(*amount)
                .ok_or(ScopeAccountingError::ArithmeticOverflow)?;
            let key = ReclaimableConcurrentKey::from_schema_key_internal(usage_key.key);
            if next > self.inner.limits.reclaimable_value(key).get() {
                return Err(ScopeAccountingError::LimitExceeded);
            }
        }

        drop(state);
        let (usage_node_claims, release_scratch) =
            claim_prepared_usage_nodes(&self.inner, &totals, &mut accounting_credit)?;
        Ok(PreparedScopedReservations {
            root: Rc::clone(&self.inner),
            expected_revision: self.inner.state.borrow().revision,
            committed_revision,
            next_reservation_sequence,
            reservation_identity,
            totals,
            scope_leases: scope_leases.into_values().collect(),
            usage_node_claims,
            release_scratch,
            accounting_credit: Some(accounting_credit),
        })
    }

    fn prepare_decoder_reservation(
        &self,
        decoder_group: &WebRemoteAccountingScope,
        spec: DecoderReservationSpec,
    ) -> Result<PreparedScopedReservations, ScopeAccountingError> {
        decoder_group.validate_scalar(
            ScalarObservationKey::from_schema_key_internal(WebRemoteLimitKey::DecoderDerivedRows),
            spec.derived_rows,
        )?;
        let chunk = ancestor_scope(decoder_group, WebRemoteLimitScope::Chunk)?;
        let generation = ancestor_scope(decoder_group, WebRemoteLimitScope::Generation)?;
        let session = ancestor_scope(decoder_group, WebRemoteLimitScope::Session)?;
        let viewer = ancestor_scope(decoder_group, WebRemoteLimitScope::ViewerInstance)?;
        let working_total = [
            spec.parser_working_bytes,
            spec.builder_bytes,
            spec.payload_scratch_bytes,
            spec.lens_intermediate_bytes,
            spec.terminal_output_bytes,
        ]
        .into_iter()
        .try_fold(0_u64, u64::checked_add)
        .ok_or(ScopeAccountingError::ArithmeticOverflow)?;
        let mut plan = self.begin_internal_reservation_plan(reservation_capacity(14))?;
        plan.push(
            WebRemoteLimitKey::DecoderWorkingBytes,
            spec.parser_working_bytes,
            decoder_group,
        )?;
        plan.push(
            WebRemoteLimitKey::DecoderBuilderBytes,
            spec.builder_bytes,
            decoder_group,
        )?;
        plan.push(
            WebRemoteLimitKey::DecoderPayloadScratchBytes,
            spec.payload_scratch_bytes,
            decoder_group,
        )?;
        plan.push(
            WebRemoteLimitKey::DecoderLensIntermediateBytes,
            spec.lens_intermediate_bytes,
            decoder_group,
        )?;
        plan.push(
            WebRemoteLimitKey::DecoderTerminalOutputBytes,
            spec.terminal_output_bytes,
            decoder_group,
        )?;
        for (key, scope) in [
            (WebRemoteLimitKey::DecoderChunkWorkingBytes, &chunk),
            (
                WebRemoteLimitKey::DecoderGenerationWorkingBytes,
                &generation,
            ),
            (WebRemoteLimitKey::DecoderSessionWorkingBytes, &session),
            (WebRemoteLimitKey::DecoderGlobalWorkingBytes, &viewer),
            (WebRemoteLimitKey::RemoteInternalRetainedBytes, &session),
        ] {
            plan.push(key, working_total, scope)?;
        }
        for (key, scope) in [
            (WebRemoteLimitKey::DecoderChunkOutputRows, &chunk),
            (WebRemoteLimitKey::DecoderGenerationOutputRows, &generation),
            (WebRemoteLimitKey::DecoderSessionOutputRows, &session),
            (WebRemoteLimitKey::DecoderGlobalOutputRows, &viewer),
        ] {
            plan.push(key, spec.derived_rows, scope)?;
        }
        plan.prepare()
    }

    fn prepare_remote_internal_reservation(
        &self,
        owner: &WebRemoteAccountingScope,
        key: WebRemoteLimitKey,
        amount: NonZeroU64,
    ) -> Result<PreparedScopedReservations, ScopeAccountingError> {
        let session = ancestor_scope(owner, WebRemoteLimitScope::Session)?;
        let mut plan = self.begin_internal_reservation_plan(reservation_capacity(2))?;
        plan.push(key, amount.get(), owner)?;
        plan.push(
            WebRemoteLimitKey::RemoteInternalRetainedBytes,
            amount.get(),
            &session,
        )?;
        plan.prepare()
    }

    fn prepare_remote_validator_temporary_reservation(
        &self,
        work_unit: &WebRemoteAccountingScope,
        overlap_key: WebRemoteLimitKey,
        overlap_bytes: NonZeroU64,
        scratch_key: WebRemoteLimitKey,
        scratch_bytes: NonZeroU64,
    ) -> Result<PreparedScopedReservations, ScopeAccountingError> {
        let session = ancestor_scope(work_unit, WebRemoteLimitScope::Session)?;
        let total = overlap_bytes
            .get()
            .checked_add(scratch_bytes.get())
            .and_then(NonZeroU64::new)
            .ok_or(ScopeAccountingError::ArithmeticOverflow)?;
        let mut plan = self.begin_internal_reservation_plan(reservation_capacity(3))?;
        plan.push(overlap_key, overlap_bytes.get(), work_unit)?;
        plan.push(scratch_key, scratch_bytes.get(), work_unit)?;
        plan.push(
            WebRemoteLimitKey::RemoteInternalRetainedBytes,
            total.get(),
            &session,
        )?;
        plan.prepare()
    }

    fn prepare_remote_validator_retained_reservation(
        &self,
        work_unit: &WebRemoteAccountingScope,
        retained_bytes: NonZeroU64,
    ) -> Result<PreparedScopedReservations, ScopeAccountingError> {
        let session = ancestor_scope(work_unit, WebRemoteLimitScope::Session)?;
        let mut plan = self.begin_internal_reservation_plan(reservation_capacity(2))?;
        plan.push(
            WebRemoteLimitKey::RemoteValidatorRetainedBytes,
            retained_bytes.get(),
            &session,
        )?;
        plan.push(
            WebRemoteLimitKey::RemoteInternalRetainedBytes,
            retained_bytes.get(),
            &session,
        )?;
        plan.prepare()
    }

    fn prepare_remote_validator_egress_reservation(
        &self,
        work_unit: &WebRemoteAccountingScope,
        overlap_bytes: NonZeroU64,
        scratch_bytes: NonZeroU64,
    ) -> Result<PreparedScopedReservations, ScopeAccountingError> {
        let session = ancestor_scope(work_unit, WebRemoteLimitScope::Session)?;
        let range = ancestor_scope(work_unit, WebRemoteLimitScope::RangeResponse)?;
        let total = overlap_bytes
            .get()
            .checked_add(scratch_bytes.get())
            .and_then(NonZeroU64::new)
            .ok_or(ScopeAccountingError::ArithmeticOverflow)?;
        let mut plan = self.begin_internal_reservation_plan(reservation_capacity(4))?;
        plan.push(WebRemoteLimitKey::RemoteValidatorEgressValues, 1, &range)?;
        plan.push(
            WebRemoteLimitKey::RemoteValidatorEgressJsWasmOverlapBytes,
            overlap_bytes.get(),
            work_unit,
        )?;
        plan.push(
            WebRemoteLimitKey::RemoteValidatorEgressScratchBytes,
            scratch_bytes.get(),
            work_unit,
        )?;
        plan.push(
            WebRemoteLimitKey::RemoteInternalRetainedBytes,
            total.get(),
            &session,
        )?;
        plan.prepare()
    }

    fn prepare_range_body_pump_reservation(
        &self,
        range_response: &WebRemoteAccountingScope,
        work_unit: &WebRemoteAccountingScope,
        output_bytes: NonZeroU64,
        scratch_bytes: NonZeroU64,
    ) -> Result<PreparedScopedReservations, ScopeAccountingError> {
        let work_range = ancestor_scope(work_unit, WebRemoteLimitScope::RangeResponse)?;
        if work_range.lease.identity != range_response.lease.identity {
            return Err(ScopeAccountingError::AggregateAncestryMismatch);
        }
        let session = ancestor_scope(range_response, WebRemoteLimitScope::Session)?;
        let overlap_bytes = output_bytes
            .get()
            .checked_add(scratch_bytes.get())
            .ok_or(ScopeAccountingError::ArithmeticOverflow)?;
        let mut plan = self.begin_internal_reservation_plan(reservation_capacity(4))?;
        plan.push(
            WebRemoteLimitKey::FetchWasmRawRetainedBytes,
            output_bytes.get(),
            range_response,
        )?;
        plan.push(
            WebRemoteLimitKey::ByobScratchBytes,
            scratch_bytes.get(),
            range_response,
        )?;
        plan.push(
            WebRemoteLimitKey::RangeJsWasmOverlapBytes,
            overlap_bytes,
            range_response,
        )?;
        plan.push(
            WebRemoteLimitKey::RemoteInternalRetainedBytes,
            overlap_bytes,
            &session,
        )?;
        plan.prepare()
    }

    fn prepare_shared_frame_work(
        &self,
        frame: &WebRemoteAccountingScope,
    ) -> Result<PreparedScopedReservations, ScopeAccountingError> {
        let mut plan = self.begin_internal_reservation_plan(NonZeroU64::MIN)?;
        plan.push(WebRemoteLimitKey::SharedRemoteWorkUnitsPerFrame, 1, frame)?;
        plan.prepare()
    }

    /// Checks `committed + unmaterialized + safety_headroom <= process_limit`.
    pub fn validate_memory_headroom(
        &self,
        process_limit: u64,
        committed: u64,
        unmaterialized: u64,
    ) -> Result<u64, MemoryHeadroomError> {
        let headroom_key = FormulaConstantKey::from_schema_key_internal(
            WebRemoteLimitKey::ProcessSafetyHeadroomBytes,
        );
        let required = committed
            .checked_add(unmaterialized)
            .and_then(|value| {
                value.checked_add(self.inner.limits.formula_value(headroom_key).get())
            })
            .ok_or(MemoryHeadroomError::ArithmeticOverflow)?;
        process_limit
            .checked_sub(required)
            .ok_or(MemoryHeadroomError::InsufficientHeadroom)
    }
}

fn ancestor_scope(
    scope: &WebRemoteAccountingScope,
    kind: WebRemoteLimitScope,
) -> Result<WebRemoteAccountingScope, ScopeAccountingError> {
    let mut lease = Some(Rc::clone(&scope.lease));
    while let Some(current) = lease {
        if current.kind == kind {
            return Ok(WebRemoteAccountingScope { lease: current });
        }
        lease = current._parent_lease.as_ref().map(Rc::clone);
    }
    Err(ScopeAccountingError::AggregateAncestryMismatch)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DecoderReservationSpec {
    pub parser_working_bytes: u64,
    pub builder_bytes: u64,
    pub payload_scratch_bytes: u64,
    pub lens_intermediate_bytes: u64,
    pub terminal_output_bytes: u64,
    pub derived_rows: u64,
}

impl DecoderGroupAccountingScope {
    pub fn prepare_decoder_reservation(
        &self,
        root: &WasmModuleLimitAccountingRoot,
        spec: DecoderReservationSpec,
    ) -> Result<PreparedScopedReservations, ScopeAccountingError> {
        root.prepare_decoder_reservation(&self.0, spec)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValidationDispatchSpec {
    pub estimated_selected_messages: u64,
    pub validation_dispatches: u64,
    pub actual_appends: u64,
}

impl WorkUnitAccountingScope {
    pub fn validate_dispatch_formula(
        &self,
        spec: ValidationDispatchSpec,
    ) -> Result<(), ScopeAccountingError> {
        let chunk = ancestor_scope(&self.0, WebRemoteLimitScope::Chunk)?;
        chunk.validate_scalar(
            ScalarObservationKey::from_schema_key_internal(
                WebRemoteLimitKey::SelectedMessageDispatchesPerScan,
            ),
            spec.estimated_selected_messages,
        )?;
        self.validate_scalar(
            ScalarObservationKey::from_schema_key_internal(
                WebRemoteLimitKey::ChunkValidationDispatchCount,
            ),
            spec.validation_dispatches,
        )?;
        self.validate_scalar(
            ScalarObservationKey::from_schema_key_internal(
                WebRemoteLimitKey::ChunkDispatchActualAppendCount,
            ),
            spec.actual_appends,
        )?;
        if spec.validation_dispatches != spec.estimated_selected_messages
            || spec.actual_appends != spec.estimated_selected_messages
        {
            return Err(ScopeAccountingError::FormulaViolation);
        }
        Ok(())
    }
}

/// The exact bytes reserved for one bounded Chrome body pump.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RangeBodyPumpReservationSpec {
    pub output_bytes: NonZeroU64,
    pub scratch_bytes: NonZeroU64,
}

/// A checked but disarmed reservation for one exact-length Chrome body pump.
pub struct PreparedRangeBodyPumpReservation {
    inner: PreparedScopedReservations,
    spec: RangeBodyPumpReservationSpec,
}

impl fmt::Debug for PreparedRangeBodyPumpReservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedRangeBodyPumpReservation")
            .field("spec", &self.spec)
            .finish_non_exhaustive()
    }
}

impl PreparedRangeBodyPumpReservation {
    pub fn commit(self) -> Result<ActiveRangeBodyPumpReservation, ScopeAccountingError> {
        Ok(ActiveRangeBodyPumpReservation {
            inner: Some(self.inner.commit()?),
            spec: self.spec,
        })
    }
}

/// Exact non-cloneable ownership of one active Chrome body-pump reservation.
pub struct ActiveRangeBodyPumpReservation {
    inner: Option<ActiveScopedReservations>,
    spec: RangeBodyPumpReservationSpec,
}

/// Opaque proof of the exact source/session/range/work ancestry charged by one body pump.
///
/// The identity is minted only after every scope is proven live, rooted in the same accounting
/// allocator, and the work unit is proven to be a child of the exact Range-response scope.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct RangeAttemptAccountingBinding {
    source: AccountingScopeIdentity,
    session: AccountingScopeIdentity,
    range_response: AccountingScopeIdentity,
    work_unit: AccountingScopeIdentity,
}

impl fmt::Debug for RangeAttemptAccountingBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RangeAttemptAccountingBinding(<opaque>)")
    }
}

impl fmt::Debug for ActiveRangeBodyPumpReservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ActiveRangeBodyPumpReservation")
            .field("spec", &self.spec)
            .field("active", &self.inner.is_some())
            .finish()
    }
}

impl ActiveRangeBodyPumpReservation {
    pub const fn spec(&self) -> RangeBodyPumpReservationSpec {
        self.spec
    }

    pub fn release(mut self) -> Result<(), ScopeAccountingError> {
        self.inner
            .take()
            .expect("active typed reservation owns its inner reservation")
            .release()
    }
}

impl RangeResponseAccountingScope {
    pub(crate) fn range_attempt_accounting_binding(
        &self,
        work_unit: &WorkUnitAccountingScope,
    ) -> Result<RangeAttemptAccountingBinding, ScopeAccountingError> {
        let root = self
            .0
            .lease
            .root
            .upgrade()
            .ok_or(ScopeAccountingError::RootStopped)?;
        validate_scope_identity(&root, &self.0, WebRemoteLimitScope::RangeResponse)?;
        validate_scope_identity(&root, &work_unit.0, WebRemoteLimitScope::WorkUnit)?;

        let range_session = ancestor_scope(&self.0, WebRemoteLimitScope::Session)?;
        validate_scope_identity(&root, &range_session, WebRemoteLimitScope::Session)?;
        let session_source = ancestor_scope(&range_session, WebRemoteLimitScope::Source)?;
        validate_scope_identity(&root, &session_source, WebRemoteLimitScope::Source)?;
        let work_range = ancestor_scope(&work_unit.0, WebRemoteLimitScope::RangeResponse)?;
        if work_range.lease.identity != self.0.lease.identity {
            return Err(ScopeAccountingError::AggregateAncestryMismatch);
        }

        Ok(RangeAttemptAccountingBinding {
            source: session_source.lease.identity,
            session: range_session.lease.identity,
            range_response: self.0.lease.identity,
            work_unit: work_unit.0.lease.identity,
        })
    }

    /// Validates and atomically reserves the Wasm output, fixed scratch, peak overlap and session
    /// total before the first body-pump allocation.
    pub fn prepare_body_pump_reservation(
        &self,
        root: &WasmModuleLimitAccountingRoot,
        work_unit: &WorkUnitAccountingScope,
        spec: RangeBodyPumpReservationSpec,
    ) -> Result<PreparedRangeBodyPumpReservation, ScopeAccountingError> {
        self.validate_scalar(
            ScalarObservationKey::from_schema_key_internal(WebRemoteLimitKey::RequestedRangeBytes),
            spec.output_bytes.get(),
        )?;
        work_unit.validate_scalar(
            ScalarObservationKey::from_schema_key_internal(WebRemoteLimitKey::ByobPumpSliceBytes),
            spec.scratch_bytes.get(),
        )?;
        let inner = root.prepare_range_body_pump_reservation(
            &self.0,
            &work_unit.0,
            spec.output_bytes,
            spec.scratch_bytes,
        )?;
        Ok(PreparedRangeBodyPumpReservation { inner, spec })
    }
}

/// Failure to prepare one irreversible physical Range-attempt burn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RangeAttemptBurnError {
    AttemptLimitExhausted,
    MetadataOpeningRangeLimitExhausted,
    ArithmeticOverflow,
    AccountingUnavailable(ScopeAccountingError),
}

impl fmt::Display for RangeAttemptBurnError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AttemptLimitExhausted => {
                formatter.write_str("remote Range retry attempt limit exhausted")
            }
            Self::MetadataOpeningRangeLimitExhausted => {
                formatter.write_str("remote metadata-opening Range limit exhausted")
            }
            Self::ArithmeticOverflow => {
                formatter.write_str("remote Range attempt accounting overflow")
            }
            Self::AccountingUnavailable(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for RangeAttemptBurnError {}

/// Sealed, non-cloneable attempt budget for one exact Range operation.
///
/// The value can only be projected from a production limits capability (or its test-only
/// equivalent) through an [`OperationAccountingScope`].
pub(crate) struct RangeRetryAttemptBudget {
    operation: OperationAccountingScope,
    limit: NonZeroU64,
    burned: u64,
}

impl fmt::Debug for RangeRetryAttemptBudget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RangeRetryAttemptBudget")
            .field("burned", &self.burned)
            .field("limit", &"<sealed>")
            .finish_non_exhaustive()
    }
}

impl RangeRetryAttemptBudget {
    pub const fn burned(&self) -> u64 {
        self.burned
    }

    /// Proves that the transport scopes belong to the same source as this operation and its
    /// source-owned metadata-opening budget before any prepared owner or burn is accepted.
    pub(crate) fn bind_metadata_opening_transport(
        &self,
        range_response: &RangeResponseAccountingScope,
        work_unit: &WorkUnitAccountingScope,
    ) -> Result<RangeAttemptAccountingBinding, RangeAttemptBurnError> {
        let operation_source = ancestor_scope(&self.operation.0, WebRemoteLimitScope::Source)
            .map_err(RangeAttemptBurnError::AccountingUnavailable)?;
        let binding = range_response
            .range_attempt_accounting_binding(work_unit)
            .map_err(RangeAttemptBurnError::AccountingUnavailable)?;
        if binding.source != operation_source.lease.identity {
            return Err(RangeAttemptBurnError::AccountingUnavailable(
                ScopeAccountingError::AggregateAncestryMismatch,
            ));
        }
        Ok(binding)
    }

    /// Checks both cumulative counters and returns a zero-burn composite guard.
    pub fn prepare_metadata_opening_attempt<'a>(
        &'a mut self,
        phase: &'a mut MetadataOpeningRangeBudgetOwner,
    ) -> Result<PreparedRangeAttemptBurn<'a>, RangeAttemptBurnError> {
        let operation_source = ancestor_scope(&self.operation.0, WebRemoteLimitScope::Source)
            .map_err(RangeAttemptBurnError::AccountingUnavailable)?;
        if operation_source.lease.identity != phase.source.0.lease.identity {
            return Err(RangeAttemptBurnError::AccountingUnavailable(
                ScopeAccountingError::AggregateAncestryMismatch,
            ));
        }

        let next_attempt = self
            .burned
            .checked_add(1)
            .ok_or(RangeAttemptBurnError::ArithmeticOverflow)?;
        if next_attempt > self.limit.get() {
            return Err(RangeAttemptBurnError::AttemptLimitExhausted);
        }
        let next_phase_range = phase
            .burned
            .checked_add(1)
            .ok_or(RangeAttemptBurnError::ArithmeticOverflow)?;
        if next_phase_range > phase.limit.get() {
            return Err(RangeAttemptBurnError::MetadataOpeningRangeLimitExhausted);
        }

        let next_attempt = NonZeroU64::new(next_attempt)
            .expect("zero plus one cannot produce a zero attempt ordinal");
        self.operation
            .validate_scalar(
                ScalarObservationKey::from_schema_key_internal(
                    WebRemoteLimitKey::RangeRetryAttemptsPerOperation,
                ),
                next_attempt.get(),
            )
            .map_err(RangeAttemptBurnError::AccountingUnavailable)?;
        phase
            .source
            .validate_scalar(
                ScalarObservationKey::from_schema_key_internal(
                    WebRemoteLimitKey::MetadataOpeningRangeRequests,
                ),
                next_phase_range,
            )
            .map_err(RangeAttemptBurnError::AccountingUnavailable)?;

        Ok(PreparedRangeAttemptBurn {
            attempt: self,
            phase,
            next_attempt,
            next_phase_range,
        })
    }
}

/// Non-cloneable cumulative physical-Range owner for one metadata-opening source.
pub(crate) struct MetadataOpeningRangeBudgetOwner {
    source: SourceAccountingScope,
    limit: NonZeroU64,
    burned: u64,
}

impl fmt::Debug for MetadataOpeningRangeBudgetOwner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MetadataOpeningRangeBudgetOwner")
            .field("burned", &self.burned)
            .field("limit", &"<sealed>")
            .finish_non_exhaustive()
    }
}

impl MetadataOpeningRangeBudgetOwner {
    pub const fn burned(&self) -> u64 {
        self.burned
    }
}

/// State returned after charging visible-only metadata-opening time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MetadataOpeningDeadlineState {
    Active,
    Expired,
}

/// Non-cloneable active-visible deadline owner for one metadata-opening source.
///
/// This owner intentionally knows nothing about `document.visibilityState` or browser execution
/// epochs. The page-execution coordinator added later supplies only visible elapsed time.
pub(crate) struct MetadataOpeningActiveVisibleDeadlineOwner {
    source: SourceAccountingScope,
    limit_millis: NonZeroU64,
    elapsed_millis: u64,
}

impl fmt::Debug for MetadataOpeningActiveVisibleDeadlineOwner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MetadataOpeningActiveVisibleDeadlineOwner")
            .field("elapsed_millis", &self.elapsed_millis)
            .field("limit_millis", &"<sealed>")
            .finish_non_exhaustive()
    }
}

impl MetadataOpeningActiveVisibleDeadlineOwner {
    #[cfg(test)]
    pub const fn elapsed_millis(&self) -> u64 {
        self.elapsed_millis
    }

    pub const fn state(&self) -> MetadataOpeningDeadlineState {
        if self.elapsed_millis >= self.limit_millis.get() {
            MetadataOpeningDeadlineState::Expired
        } else {
            MetadataOpeningDeadlineState::Active
        }
    }

    /// Charges only time that the caller has already established was active and visible.
    pub fn advance_visible(
        &mut self,
        elapsed_millis: u64,
    ) -> Result<MetadataOpeningDeadlineState, ScopeAccountingError> {
        let next = self
            .elapsed_millis
            .checked_add(elapsed_millis)
            .ok_or(ScopeAccountingError::ArithmeticOverflow)?
            .min(self.limit_millis.get());
        self.source.validate_scalar(
            ScalarObservationKey::from_schema_key_internal(
                WebRemoteLimitKey::MetadataOpeningVisibleDeadlineMillis,
            ),
            next,
        )?;
        self.elapsed_millis = next;
        Ok(self.state())
    }
}

/// A checked, zero-burn coupling of an operation attempt and its caller phase Range count.
///
/// Dropping this guard preserves both owners bit-for-bit. [`Self::commit`] has no fallible work
/// after either counter is changed.
pub(crate) struct PreparedRangeAttemptBurn<'a> {
    attempt: &'a mut RangeRetryAttemptBudget,
    phase: &'a mut MetadataOpeningRangeBudgetOwner,
    next_attempt: NonZeroU64,
    next_phase_range: u64,
}

impl fmt::Debug for PreparedRangeAttemptBurn<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedRangeAttemptBurn")
            .field("next_attempt", &self.next_attempt)
            .field("next_phase_range", &self.next_phase_range)
            .finish_non_exhaustive()
    }
}

impl PreparedRangeAttemptBurn<'_> {
    pub const fn next_attempt(&self) -> NonZeroU64 {
        self.next_attempt
    }

    /// Atomically and irreversibly consumes both scalar counters.
    pub fn commit(self) -> CommittedRangeAttemptBurn {
        self.attempt.burned = self.next_attempt.get();
        self.phase.burned = self.next_phase_range;
        CommittedRangeAttemptBurn {
            attempt: self.next_attempt,
            metadata_opening_range: self.next_phase_range,
        }
    }
}

/// Receipt for one irreversible physical Fetch attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CommittedRangeAttemptBurn {
    attempt: NonZeroU64,
    metadata_opening_range: u64,
}

impl CommittedRangeAttemptBurn {
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the disarmed retry receipt is consumed by the future production frame driver"
        )
    )]
    pub const fn attempt(self) -> NonZeroU64 {
        self.attempt
    }

    #[cfg(test)]
    pub const fn metadata_opening_range(self) -> u64 {
        self.metadata_opening_range
    }
}

const RANGE_RETRY_ATTEMPT_PROJECTION: u8 = 1 << 0;
const METADATA_OPENING_PROJECTION: u8 = 1 << 1;

fn claim_internal_projection(
    lease: &AccountingScopeLease,
    claim: u8,
) -> Result<(), ScopeAccountingError> {
    let current = lease.internal_projection_claims.get();
    if current & claim != 0 {
        return Err(ScopeAccountingError::ScopeBusy);
    }
    lease.internal_projection_claims.set(current | claim);
    Ok(())
}

impl OperationAccountingScope {
    /// Projects the sealed per-operation retry limit into one non-cloneable owner.
    pub(crate) fn range_retry_attempt_budget(
        &self,
    ) -> Result<RangeRetryAttemptBudget, ScopeAccountingError> {
        let root = self
            .0
            .lease
            .root
            .upgrade()
            .ok_or(ScopeAccountingError::RootStopped)?;
        validate_scope_and_key(
            &root,
            &self.0,
            WebRemoteLimitKey::RangeRetryAttemptsPerOperation,
            WebRemoteAccountingKind::ScalarObservation,
        )?;
        claim_internal_projection(&self.0.lease, RANGE_RETRY_ATTEMPT_PROJECTION)?;
        Ok(RangeRetryAttemptBudget {
            operation: self.clone(),
            limit: root
                .limits
                .raw_limit_value(WebRemoteLimitKey::RangeRetryAttemptsPerOperation),
            burned: 0,
        })
    }
}

impl SourceAccountingScope {
    /// Projects the two sealed metadata-opening limits into source-owned, non-cloneable owners.
    pub(crate) fn metadata_opening_range_and_deadline_owners(
        &self,
    ) -> Result<
        (
            MetadataOpeningRangeBudgetOwner,
            MetadataOpeningActiveVisibleDeadlineOwner,
        ),
        ScopeAccountingError,
    > {
        let root = self
            .0
            .lease
            .root
            .upgrade()
            .ok_or(ScopeAccountingError::RootStopped)?;
        for key in [
            WebRemoteLimitKey::MetadataOpeningRangeRequests,
            WebRemoteLimitKey::MetadataOpeningVisibleDeadlineMillis,
        ] {
            validate_scope_and_key(
                &root,
                &self.0,
                key,
                WebRemoteAccountingKind::ScalarObservation,
            )?;
        }
        claim_internal_projection(&self.0.lease, METADATA_OPENING_PROJECTION)?;
        Ok((
            MetadataOpeningRangeBudgetOwner {
                source: self.clone(),
                limit: root
                    .limits
                    .raw_limit_value(WebRemoteLimitKey::MetadataOpeningRangeRequests),
                burned: 0,
            },
            MetadataOpeningActiveVisibleDeadlineOwner {
                source: self.clone(),
                limit_millis: root
                    .limits
                    .raw_limit_value(WebRemoteLimitKey::MetadataOpeningVisibleDeadlineMillis),
                elapsed_millis: 0,
            },
        ))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlanningCrossProductSpec {
    pub hit_chunks: u64,
    pub selected_groups: u64,
}

impl GenerationAccountingScope {
    pub fn validate_planning_cross_product(
        &self,
        window: &WindowAccountingScope,
        spec: PlanningCrossProductSpec,
    ) -> Result<u64, ScopeAccountingError> {
        let generation_session = ancestor_scope(&self.0, WebRemoteLimitScope::Session)?;
        let window_session = ancestor_scope(&window.0, WebRemoteLimitScope::Session)?;
        if generation_session.lease.identity != window_session.lease.identity {
            return Err(ScopeAccountingError::AggregateAncestryMismatch);
        }
        window.validate_scalar(
            ScalarObservationKey::from_schema_key_internal(WebRemoteLimitKey::WindowChunkHits),
            spec.hit_chunks,
        )?;
        window.validate_scalar(
            ScalarObservationKey::from_schema_key_internal(
                WebRemoteLimitKey::SelectedChannelGroups,
            ),
            spec.selected_groups,
        )?;
        self.checked_scalar_product(
            ScalarObservationKey::from_schema_key_internal(
                WebRemoteLimitKey::PlanningCrossProductOperations,
            ),
            spec.hit_chunks,
            spec.selected_groups,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionPartitionFormulaSpec {
    pub nonempty_source_chunks: u64,
    pub selected_groups: u64,
    pub opening_static_partitions: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionDescriptorFormulaSpec {
    pub nonempty_source_chunks: u64,
    pub roots_per_nonempty_chunk: u64,
    pub origin_bytes_per_nonempty_chunk: u64,
    pub opening_static_roots: u64,
    pub opening_static_origin_bytes: u64,
}

impl SessionAccountingScope {
    pub fn validate_partition_formula(
        &self,
        spec: SessionPartitionFormulaSpec,
    ) -> Result<u64, ScopeAccountingError> {
        let temporal = spec
            .nonempty_source_chunks
            .checked_mul(spec.selected_groups)
            .ok_or(ScopeAccountingError::ArithmeticOverflow)?;
        let total = temporal
            .checked_add(spec.opening_static_partitions)
            .ok_or(ScopeAccountingError::ArithmeticOverflow)?;
        self.validate_scalar(
            ScalarObservationKey::from_schema_key_internal(
                WebRemoteLimitKey::SessionRegisteredPartitions,
            ),
            total,
        )?;
        Ok(total)
    }

    pub fn validate_descriptor_formula(
        &self,
        spec: SessionDescriptorFormulaSpec,
    ) -> Result<(u64, u64), ScopeAccountingError> {
        let roots = spec
            .nonempty_source_chunks
            .checked_mul(spec.roots_per_nonempty_chunk)
            .and_then(|value| value.checked_add(spec.opening_static_roots))
            .ok_or(ScopeAccountingError::ArithmeticOverflow)?;
        let origin_bytes = spec
            .nonempty_source_chunks
            .checked_mul(spec.origin_bytes_per_nonempty_chunk)
            .and_then(|value| value.checked_add(spec.opening_static_origin_bytes))
            .ok_or(ScopeAccountingError::ArithmeticOverflow)?;
        self.validate_scalar(
            ScalarObservationKey::from_schema_key_internal(
                WebRemoteLimitKey::SessionRootDescriptors,
            ),
            roots,
        )?;
        self.validate_scalar(
            ScalarObservationKey::from_schema_key_internal(
                WebRemoteLimitKey::SessionExternalOriginBytes,
            ),
            origin_bytes,
        )?;
        Ok((roots, origin_bytes))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegistrationMetadataReservationSpec {
    pub partition_entry_bytes: u64,
    pub complete_empty_entry_bytes: u64,
    pub root_descriptor_bytes: u64,
    pub external_origin_bytes: u64,
    pub fixed_overhead_bytes: u64,
}

impl SessionAccountingScope {
    pub fn prepare_registration_metadata_formula(
        &self,
        root: &WasmModuleLimitAccountingRoot,
        spec: RegistrationMetadataReservationSpec,
    ) -> Result<PreparedScopedReservations, ScopeAccountingError> {
        let retained = [
            spec.partition_entry_bytes,
            spec.complete_empty_entry_bytes,
            spec.root_descriptor_bytes,
            spec.external_origin_bytes,
            spec.fixed_overhead_bytes,
        ]
        .into_iter()
        .try_fold(0_u64, u64::checked_add)
        .ok_or(ScopeAccountingError::ArithmeticOverflow)?;
        let retained = NonZeroU64::new(retained).ok_or(ScopeAccountingError::EmptyReservation)?;
        self.prepare_registration_metadata_reservation(root, retained)
    }
}

macro_rules! define_remote_validator_reservation_owner {
    ($prepared:ident, $active:ident) => {
        /// A checked but disarmed validator `ByteString` reservation.
        pub struct $prepared {
            inner: PreparedScopedReservations,
            accounted_bytes: NonZeroU64,
        }

        impl fmt::Debug for $prepared {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .debug_struct(stringify!($prepared))
                    .field("accounted_bytes", &self.accounted_bytes)
                    .finish_non_exhaustive()
            }
        }

        impl $prepared {
            pub const fn accounted_bytes(&self) -> NonZeroU64 {
                self.accounted_bytes
            }

            pub fn commit(self) -> Result<$active, ScopeAccountingError> {
                Ok($active {
                    inner: Some(self.inner.commit()?),
                    accounted_bytes: self.accounted_bytes,
                })
            }
        }

        /// Exact non-cloneable ownership of one active validator `ByteString` reservation.
        pub struct $active {
            inner: Option<ActiveScopedReservations>,
            accounted_bytes: NonZeroU64,
        }

        impl fmt::Debug for $active {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .debug_struct(stringify!($active))
                    .field("accounted_bytes", &self.accounted_bytes)
                    .field("active", &self.inner.is_some())
                    .finish()
            }
        }

        impl $active {
            pub const fn accounted_bytes(&self) -> NonZeroU64 {
                self.accounted_bytes
            }

            pub fn release(mut self) -> Result<(), ScopeAccountingError> {
                self.inner
                    .take()
                    .expect("active typed reservation owns its inner reservation")
                    .release()
            }
        }
    };
}

define_remote_validator_reservation_owner!(
    PreparedRemoteValidatorIngressReservation,
    ActiveRemoteValidatorIngressReservation
);
define_remote_validator_reservation_owner!(
    PreparedRemoteValidatorRetainedReservation,
    ActiveRemoteValidatorRetainedReservation
);
define_remote_validator_reservation_owner!(
    PreparedRemoteValidatorEgressReservation,
    ActiveRemoteValidatorEgressReservation
);

impl WorkUnitAccountingScope {
    /// Checks the observable shape before any validator `ByteString` copy or code-unit scan.
    pub fn validate_remote_validator_ingress_shape(
        &self,
        visible_values: u64,
        wire_bytes: u64,
    ) -> Result<(), ScopeAccountingError> {
        self.validate_scalar(
            ScalarObservationKey::from_schema_key_internal(
                WebRemoteLimitKey::RemoteValidatorIngressValues,
            ),
            visible_values,
        )?;
        self.validate_scalar(
            ScalarObservationKey::from_schema_key_internal(
                WebRemoteLimitKey::RemoteValidatorIngressWireBytes,
            ),
            wire_bytes,
        )
    }

    /// Checks and atomically reserves the JavaScript-retained and Wasm scratch sides of one
    /// `ByteString` ingress copy.
    pub fn prepare_remote_validator_ingress(
        &self,
        root: &WasmModuleLimitAccountingRoot,
        visible_values: NonZeroU64,
        wire_bytes: NonZeroU64,
    ) -> Result<PreparedRemoteValidatorIngressReservation, ScopeAccountingError> {
        self.validate_remote_validator_ingress_shape(visible_values.get(), wire_bytes.get())?;
        let js_utf16_bytes = wire_bytes
            .get()
            .checked_mul(size_of::<u16>() as u64)
            .and_then(NonZeroU64::new)
            .ok_or(ScopeAccountingError::ArithmeticOverflow)?;
        let inner = root.prepare_remote_validator_temporary_reservation(
            &self.0,
            WebRemoteLimitKey::RemoteValidatorIngressJsWasmOverlapBytes,
            js_utf16_bytes,
            WebRemoteLimitKey::RemoteValidatorIngressScratchBytes,
            wire_bytes,
        )?;
        Ok(PreparedRemoteValidatorIngressReservation {
            inner,
            accounted_bytes: wire_bytes,
        })
    }

    /// Atomically reserves the final shared wire-byte owner and the session-wide remote total.
    pub fn prepare_remote_validator_retained(
        &self,
        root: &WasmModuleLimitAccountingRoot,
        retained_bytes: NonZeroU64,
    ) -> Result<PreparedRemoteValidatorRetainedReservation, ScopeAccountingError> {
        let inner = root.prepare_remote_validator_retained_reservation(&self.0, retained_bytes)?;
        Ok(PreparedRemoteValidatorRetainedReservation {
            inner,
            accounted_bytes: retained_bytes,
        })
    }

    /// Checks and atomically reserves the `JavaScript` `ByteString` plus `u16` construction scratch
    /// used by one strong `If-Match` egress value.
    pub(crate) fn prepare_remote_validator_egress(
        &self,
        root: &WasmModuleLimitAccountingRoot,
        wire_bytes: NonZeroU64,
    ) -> Result<PreparedRemoteValidatorEgressReservation, ScopeAccountingError> {
        self.validate_scalar(
            ScalarObservationKey::from_schema_key_internal(
                WebRemoteLimitKey::RemoteValidatorEgressWireBytes,
            ),
            wire_bytes.get(),
        )?;
        let utf16_bytes = wire_bytes
            .get()
            .checked_mul(size_of::<u16>() as u64)
            .and_then(NonZeroU64::new)
            .ok_or(ScopeAccountingError::ArithmeticOverflow)?;
        let inner =
            root.prepare_remote_validator_egress_reservation(&self.0, utf16_bytes, utf16_bytes)?;
        Ok(PreparedRemoteValidatorEgressReservation {
            inner,
            accounted_bytes: wire_bytes,
        })
    }
}

macro_rules! define_remote_internal_method {
    ($scope:ident, $method:ident, $key:ident) => {
        impl $scope {
            pub fn $method(
                &self,
                root: &WasmModuleLimitAccountingRoot,
                amount: NonZeroU64,
            ) -> Result<PreparedScopedReservations, ScopeAccountingError> {
                root.prepare_remote_internal_reservation(&self.0, WebRemoteLimitKey::$key, amount)
            }
        }
    };
}

define_remote_internal_method!(
    ChunkAccountingScope,
    prepare_validation_plan_reservation,
    ValidationPlanRetainedBytes
);
define_remote_internal_method!(
    ChunkAccountingScope,
    prepare_decompressed_chunk_reservation,
    DecompressedChunkRetainedBytes
);
define_remote_internal_method!(
    SessionAccountingScope,
    prepare_registration_metadata_reservation,
    SessionRegistrationMetadataBytes
);
define_remote_internal_method!(
    SessionAccountingScope,
    prepare_metadata_opening_reservation,
    MetadataOpeningBytes
);
define_remote_internal_method!(
    SessionAccountingScope,
    prepare_raw_cache_reservation,
    RawCacheBytes
);
define_remote_internal_method!(
    GenerationAccountingScope,
    prepare_staged_terminal_batch_reservation,
    RemoteStagedTerminalBatchBytes
);
define_remote_internal_method!(
    SessionAccountingScope,
    prepare_pinned_resident_reservation,
    RemotePinnedResidentBytes
);
define_remote_internal_method!(
    StoreAccountingScope,
    prepare_entity_db_reservation,
    RemoteEntityDbRetainedBytes
);
define_remote_internal_method!(
    StoreAccountingScope,
    prepare_store_residency_reservation,
    RemoteStoreResidentBytes
);

impl FrameAccountingScope {
    pub fn prepare_remote_cpu_work(
        &self,
        root: &WasmModuleLimitAccountingRoot,
    ) -> Result<PreparedScopedReservations, ScopeAccountingError> {
        root.prepare_shared_frame_work(&self.0)
    }

    pub fn prepare_remote_gc_work(
        &self,
        root: &WasmModuleLimitAccountingRoot,
    ) -> Result<PreparedScopedReservations, ScopeAccountingError> {
        root.prepare_shared_frame_work(&self.0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LifecycleReservationSpec {
    pub source_owners: u64,
    pub source_owner_bytes: u64,
    pub operations: u64,
    pub operation_bytes: u64,
    pub recording_handles: u64,
    pub recording_handle_bytes: u64,
    pub operation_recording_subscriptions: u64,
    pub operation_recording_subscription_bytes: u64,
    pub subscribers: u64,
    pub subscriber_bytes: u64,
    pub hubs: u64,
    pub hub_bytes: u64,
    pub child_states: u64,
    pub child_state_bytes: u64,
    pub activation_acks: u64,
    pub activation_ack_bytes: u64,
    pub latest_snapshots: u64,
    pub latest_snapshot_bytes: u64,
    pub outstanding_deliveries: u64,
    pub outstanding_delivery_bytes: u64,
    pub listener_error_credits: u64,
    pub typescript_dispatcher_items: u64,
    pub typescript_dispatcher_bytes: u64,
    pub scheduled_tasks: u64,
    pub currently_dispatching: u64,
    pub promise_closure_bytes: u64,
    pub wrapper_entries: u64,
    pub wrapper_bytes: u64,
    pub tombstone_retainers: u64,
    pub tombstone_bytes: u64,
}

impl OperationAccountingScope {
    pub fn prepare_lifecycle_reservation(
        &self,
        root: &WasmModuleLimitAccountingRoot,
        spec: LifecycleReservationSpec,
    ) -> Result<PreparedScopedReservations, ScopeAccountingError> {
        let viewer = ancestor_scope(&self.0, WebRemoteLimitScope::ViewerInstance)?;
        let mut plan = root.begin_internal_reservation_plan(reservation_capacity(30))?;
        for (key, amount) in [
            (WebRemoteLimitKey::OpenSourceOwners, spec.source_owners),
            (
                WebRemoteLimitKey::OpenSourceOwnerRetainedBytes,
                spec.source_owner_bytes,
            ),
            (WebRemoteLimitKey::OpenOperations, spec.operations),
            (
                WebRemoteLimitKey::OpenOperationRetainedBytes,
                spec.operation_bytes,
            ),
            (
                WebRemoteLimitKey::PublicRecordingHandles,
                spec.recording_handles,
            ),
            (
                WebRemoteLimitKey::PublicRecordingHandleRetainedBytes,
                spec.recording_handle_bytes,
            ),
            (WebRemoteLimitKey::LifecycleSubscribers, spec.subscribers),
            (
                WebRemoteLimitKey::LifecycleSubscriberRetainedBytes,
                spec.subscriber_bytes,
            ),
            (WebRemoteLimitKey::LifecycleHubs, spec.hubs),
            (WebRemoteLimitKey::LifecycleHubRetainedBytes, spec.hub_bytes),
            (WebRemoteLimitKey::LifecycleChildStates, spec.child_states),
            (
                WebRemoteLimitKey::LifecycleChildStateBytes,
                spec.child_state_bytes,
            ),
            (
                WebRemoteLimitKey::ActivationDeliveryAcks,
                spec.activation_acks,
            ),
            (
                WebRemoteLimitKey::ActivationDeliveryAckBytes,
                spec.activation_ack_bytes,
            ),
            (
                WebRemoteLimitKey::LatestLifecycleSnapshots,
                spec.latest_snapshots,
            ),
            (
                WebRemoteLimitKey::LatestLifecycleSnapshotBytes,
                spec.latest_snapshot_bytes,
            ),
            (
                WebRemoteLimitKey::LifecycleOutstandingDeliveries,
                spec.outstanding_deliveries,
            ),
            (
                WebRemoteLimitKey::LifecycleOutstandingDeliveryBytes,
                spec.outstanding_delivery_bytes,
            ),
            (
                WebRemoteLimitKey::LifecycleListenerErrorCredits,
                spec.listener_error_credits,
            ),
            (
                WebRemoteLimitKey::TypescriptDispatcherItems,
                spec.typescript_dispatcher_items,
            ),
            (
                WebRemoteLimitKey::TypescriptDispatcherRetainedBytes,
                spec.typescript_dispatcher_bytes,
            ),
            (
                WebRemoteLimitKey::LifecycleScheduledTasks,
                spec.scheduled_tasks,
            ),
            (
                WebRemoteLimitKey::LifecycleCurrentlyDispatching,
                spec.currently_dispatching,
            ),
            (
                WebRemoteLimitKey::PromiseClosureBytes,
                spec.promise_closure_bytes,
            ),
            (WebRemoteLimitKey::WrapperCacheEntries, spec.wrapper_entries),
            (
                WebRemoteLimitKey::WrapperCacheRetainedBytes,
                spec.wrapper_bytes,
            ),
            (
                WebRemoteLimitKey::RemovedTombstoneRetainers,
                spec.tombstone_retainers,
            ),
            (
                WebRemoteLimitKey::RemovedTombstoneRetainedBytes,
                spec.tombstone_bytes,
            ),
        ] {
            plan.push(key, amount, &viewer)?;
        }
        plan.push(
            WebRemoteLimitKey::OperationRecordingSubscriptions,
            spec.operation_recording_subscriptions,
            &self.0,
        )?;
        plan.push(
            WebRemoteLimitKey::OperationRecordingSubscriptionRetainedBytes,
            spec.operation_recording_subscription_bytes,
            &self.0,
        )?;
        plan.prepare()
    }
}

/// Exact application-owned bytes installed by one presentation activation.
///
/// `facade_shell_bytes` owns the facade object itself.
/// `owner_state_shell_bytes` owns the non-cloneable RAII owner, its shared state, and the lease
/// counter shell.
/// Their checked sum is charged once to `presentation_facade_retained_bytes`.
/// `current_snapshot_bytes` is charged only to `presentation_snapshot_retained_bytes`.
/// None of these classes may include lease/guard shells, live-set nodes, or lease snapshot copies.
///
/// Omitting either half is a compile-time error:
///
/// ```compile_fail
/// use std::num::NonZeroU64;
/// use re_web::remote_limits::PresentationActivationBytes;
///
/// let _incomplete = PresentationActivationBytes {
///     facade_shell_bytes: NonZeroU64::MIN,
/// };
/// ```
///
/// There is no generic partial presentation reservation API:
///
/// ```compile_fail
/// use re_web::remote_limits::PresentationOwnershipReservationSpec;
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresentationActivationBytes {
    pub facade_shell_bytes: NonZeroU64,
    pub owner_state_shell_bytes: NonZeroU64,
    pub current_snapshot_bytes: NonZeroU64,
}

/// Exact application-owned bytes installed by one query-lease acquisition.
///
/// `lease_guard_shell_bytes` owns the non-cloneable lease wrapper and guard state.
/// `live_set_node_bytes` owns only the facade's live-set entry.
/// `snapshot_copy_bytes` owns only the immutable snapshot retained by the lease.
/// Their checked sum is charged once to `presentation_active_lease_retained_bytes`; none of these
/// classes may also be included in facade or current-snapshot accounting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresentationLeaseBytes {
    pub lease_guard_shell_bytes: NonZeroU64,
    pub live_set_node_bytes: NonZeroU64,
    pub snapshot_copy_bytes: NonZeroU64,
}

/// A failed sealed-presentation state transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresentationTransitionError {
    Accounting(ScopeAccountingError),
    Busy,
    WrongOwner,
    StaleLease,
    ArithmeticOverflow,
}

impl fmt::Display for PresentationTransitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Accounting(error) => write!(formatter, "presentation accounting failed: {error}"),
            Self::Busy => formatter.write_str("presentation owner still has live query leases"),
            Self::WrongOwner => {
                formatter.write_str("query lease belongs to another presentation owner")
            }
            Self::StaleLease => formatter.write_str("query lease identity is stale"),
            Self::ArithmeticOverflow => {
                formatter.write_str("presentation ownership arithmetic overflow")
            }
        }
    }
}

impl std::error::Error for PresentationTransitionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Accounting(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ScopeAccountingError> for PresentationTransitionError {
    fn from(error: ScopeAccountingError) -> Self {
        Self::Accounting(error)
    }
}

struct PresentationOwnerState {
    root: WasmModuleLimitAccountingRoot,
    source: SourceAccountingScope,
    activation: ActiveScopedReservations,
    live_leases: u64,
    next_lease_sequence: u64,
}

/// Non-cloneable ownership of one source's sealed presentation facade and current snapshot.
///
/// Dropping the public owner cannot ungate a Store while leases survive: each lease retains the
/// shared state, and the activation reservation is therefore released only after the last lease.
///
/// Successful cleanup consumes the owner, so no released wrapper can survive:
///
/// ```compile_fail
/// use re_web::remote_limits::PresentationOwner;
///
/// fn cannot_reuse(owner: PresentationOwner) {
///     let _ = owner.cleanup();
///     let _ = owner.cleanup();
/// }
/// ```
pub struct PresentationOwner {
    state: Rc<RefCell<PresentationOwnerState>>,
}

impl fmt::Debug for PresentationOwner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.state.borrow();
        formatter
            .debug_struct("PresentationOwner")
            .field("has_live_leases", &(state.live_leases != 0))
            .field("identity", &"<opaque>")
            .finish_non_exhaustive()
    }
}

impl PresentationOwner {
    /// Atomically acquires one lease/guard shell, one live-set entry, and one snapshot copy.
    pub fn acquire_lease(
        &self,
        bytes: PresentationLeaseBytes,
    ) -> Result<PresentationLease, PresentationTransitionError> {
        let retained_bytes = bytes
            .lease_guard_shell_bytes
            .get()
            .checked_add(bytes.live_set_node_bytes.get())
            .and_then(|subtotal| subtotal.checked_add(bytes.snapshot_copy_bytes.get()))
            .ok_or(PresentationTransitionError::ArithmeticOverflow)?;
        let mut state = self.state.borrow_mut();
        let sequence = NonZeroU64::new(state.next_lease_sequence)
            .ok_or(PresentationTransitionError::ArithmeticOverflow)?;
        let next_lease_sequence = state
            .next_lease_sequence
            .checked_add(1)
            .ok_or(PresentationTransitionError::ArithmeticOverflow)?;
        let next_live_leases = state
            .live_leases
            .checked_add(1)
            .ok_or(PresentationTransitionError::ArithmeticOverflow)?;
        let mut plan = state
            .root
            .begin_internal_reservation_plan(reservation_capacity(2))?;
        plan.push(
            WebRemoteLimitKey::PresentationActiveLeases,
            1,
            &state.source.0,
        )?;
        plan.push(
            WebRemoteLimitKey::PresentationActiveLeaseRetainedBytes,
            retained_bytes,
            &state.source.0,
        )?;
        let reservation = plan.prepare()?.commit()?;
        state.next_lease_sequence = next_lease_sequence;
        state.live_leases = next_live_leases;
        drop(state);
        Ok(PresentationLease {
            inner: Box::new(PresentationLeaseInner {
                state: Rc::clone(&self.state),
                sequence,
                reservation,
            }),
        })
    }

    /// Consumes and destroys the facade owner after releasing its facade and current snapshot.
    ///
    /// Busy or accounting failures return the unchanged owner for an explicit retry.
    pub fn cleanup(self) -> Result<(), PresentationOwnerCleanupFailure> {
        if self.state.borrow().live_leases != 0 {
            return Err(PresentationOwnerCleanupFailure {
                error: PresentationTransitionError::Busy,
                owner: self,
            });
        }
        let result = self.state.borrow_mut().activation.release_inner();
        match result {
            Ok(()) => Ok(()),
            Err(error) => Err(PresentationOwnerCleanupFailure {
                error: PresentationTransitionError::Accounting(error),
                owner: self,
            }),
        }
    }
}

/// A cleanup failure which returns the still-owning presentation owner for retry.
pub struct PresentationOwnerCleanupFailure {
    pub error: PresentationTransitionError,
    pub owner: PresentationOwner,
}

impl fmt::Debug for PresentationOwnerCleanupFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PresentationOwnerCleanupFailure")
            .field("error", &self.error)
            .field("owner", &"<owning>")
            .finish()
    }
}

/// Non-cloneable ownership of one facade live-set entry and its immutable snapshot copy.
///
/// Successful release consumes the lease, so no released guard can survive:
///
/// ```compile_fail
/// use re_web::remote_limits::{PresentationLease, PresentationOwner};
///
/// fn cannot_reuse(lease: PresentationLease, owner: &PresentationOwner) {
///     let _ = lease.release(owner);
///     let _ = lease.release(owner);
/// }
/// ```
pub struct PresentationLease {
    inner: Box<PresentationLeaseInner>,
}

struct PresentationLeaseInner {
    state: Rc<RefCell<PresentationOwnerState>>,
    sequence: NonZeroU64,
    reservation: ActiveScopedReservations,
}

impl fmt::Debug for PresentationLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PresentationLease")
            .field("identity", &"<opaque>")
            .finish_non_exhaustive()
    }
}

impl PresentationLease {
    /// Consumes and destroys this lease through its matching facade owner.
    ///
    /// Wrong-owner, stale-identity, or accounting failures return the unchanged lease for retry.
    pub fn release(
        mut self,
        owner: &PresentationOwner,
    ) -> Result<(), PresentationLeaseReleaseFailure> {
        if !Rc::ptr_eq(&self.inner.state, &owner.state) {
            return Err(PresentationLeaseReleaseFailure {
                error: PresentationTransitionError::WrongOwner,
                lease: self,
            });
        }
        match self.release_inner() {
            Ok(()) => Ok(()),
            Err(error) => Err(PresentationLeaseReleaseFailure { error, lease: self }),
        }
    }

    fn release_inner(&mut self) -> Result<(), PresentationTransitionError> {
        let mut state = self.inner.state.borrow_mut();
        if self.inner.sequence.get() >= state.next_lease_sequence {
            return Err(PresentationTransitionError::StaleLease);
        }
        let next_live_leases = state
            .live_leases
            .checked_sub(1)
            .ok_or(PresentationTransitionError::StaleLease)?;
        self.inner.reservation.release_inner()?;
        state.live_leases = next_live_leases;
        Ok(())
    }
}

/// A release failure which returns the still-owning presentation lease for retry.
pub struct PresentationLeaseReleaseFailure {
    pub error: PresentationTransitionError,
    pub lease: PresentationLease,
}

impl fmt::Debug for PresentationLeaseReleaseFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PresentationLeaseReleaseFailure")
            .field("error", &self.error)
            .field("lease", &"<owning>")
            .finish()
    }
}

impl Drop for PresentationLease {
    fn drop(&mut self) {
        if self.inner.reservation.active {
            let _result = self.release_inner();
        }
    }
}

impl SourceAccountingScope {
    /// Atomically activates exactly one facade and one current immutable snapshot.
    pub fn activate_presentation(
        &self,
        root: &WasmModuleLimitAccountingRoot,
        bytes: PresentationActivationBytes,
    ) -> Result<PresentationOwner, PresentationTransitionError> {
        let facade_retained_bytes = bytes
            .facade_shell_bytes
            .get()
            .checked_add(bytes.owner_state_shell_bytes.get())
            .ok_or(PresentationTransitionError::ArithmeticOverflow)?;
        let mut plan = root.begin_internal_reservation_plan(reservation_capacity(4))?;
        for (key, amount) in [
            (WebRemoteLimitKey::PresentationFacades, 1),
            (
                WebRemoteLimitKey::PresentationFacadeRetainedBytes,
                facade_retained_bytes,
            ),
            (WebRemoteLimitKey::PresentationSnapshots, 1),
            (
                WebRemoteLimitKey::PresentationSnapshotRetainedBytes,
                bytes.current_snapshot_bytes.get(),
            ),
        ] {
            plan.push(key, amount, &self.0)?;
        }
        let activation = plan.prepare()?.commit()?;
        Ok(PresentationOwner {
            state: Rc::new(RefCell::new(PresentationOwnerState {
                root: root.clone(),
                source: self.clone(),
                activation,
                live_leases: 0,
                next_lease_sequence: 1,
            })),
        })
    }
}

/// Ownership for parked browser responses and their body readers.
///
/// `owners` counts one indivisible `{Response, body reader}` pair per parked Range operation.
/// `application_retained_bytes` charges only precisely measured application-owned Rust state,
/// Wasm allocations, JS wrappers/descriptors, and visible response buffers.
/// It never estimates Chrome, network-stack, HTTP cache, or transport-internal buffers; those are
/// constrained indirectly by owner count, requested/in-flight byte limits, and stopping reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PageParkedResponseOwnerReservationSpec {
    pub owners: u64,
    pub application_retained_bytes: u64,
}

impl PageExecutionAccountingScope {
    pub fn prepare_parked_response_owner_reservation(
        &self,
        root: &WasmModuleLimitAccountingRoot,
        spec: PageParkedResponseOwnerReservationSpec,
    ) -> Result<PreparedScopedReservations, ScopeAccountingError> {
        if (spec.owners == 0) != (spec.application_retained_bytes == 0) {
            return Err(ScopeAccountingError::FormulaViolation);
        }
        let mut plan = root.begin_internal_reservation_plan(reservation_capacity(2))?;
        plan.push(
            WebRemoteLimitKey::PageParkedResponseOwners,
            spec.owners,
            &self.0,
        )?;
        plan.push(
            WebRemoteLimitKey::PageParkedResponseOwnerRetainedBytes,
            spec.application_retained_bytes,
            &self.0,
        )?;
        plan.prepare()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenAdmissionReservationSpec {
    pub batch_urls: u64,
    pub remote_session_slots: u64,
    pub pending_secret_bytes: u64,
    pub pending_options_bytes: u64,
    pub extensionless_prepared: u64,
    pub pending_sniffs: u64,
    pub disarmed_sources: u64,
    pub disarmed_retained_bytes: u64,
    pub atomic_reservation_bytes: u64,
    pub descriptors: u64,
    pub operations: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenStatusReservationSpec {
    pub owned_client_entries: u64,
    pub status_slots: u64,
    pub live_status_owners: u64,
    pub terminal_entries: u64,
    pub terminal_bytes: u64,
    pub status_retained_bytes: u64,
    pub disarmed_status_claims: u64,
    pub terminal_status_bytes: u64,
    pub terminal_diagnostics: u64,
    pub deferred_eviction_victims: u64,
    pub deferred_eviction_descriptor_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StrictOpenWireBatchLimits {
    pub open_batch_urls: NonZeroU64,
    pub preexisting_recording_attachments: NonZeroU64,
    pub installation_acks: NonZeroU64,
}

impl AtomicBatchAccountingScope {
    pub(crate) fn strict_open_wire_batch_limits(
        &self,
    ) -> Result<StrictOpenWireBatchLimits, ScopeAccountingError> {
        let root = self
            .0
            .lease
            .root
            .upgrade()
            .ok_or(ScopeAccountingError::RootStopped)?;
        Ok(StrictOpenWireBatchLimits {
            open_batch_urls: root
                .limits
                .raw_limit_value(WebRemoteLimitKey::OpenBatchUrls),
            preexisting_recording_attachments: root
                .limits
                .raw_limit_value(WebRemoteLimitKey::PreexistingRecordingAttachments),
            installation_acks: root
                .limits
                .raw_limit_value(WebRemoteLimitKey::InstallationAcks),
        })
    }

    pub fn prepare_open_admission_reservation(
        &self,
        root: &WasmModuleLimitAccountingRoot,
        request: &RequestAccountingScope,
        spec: OpenAdmissionReservationSpec,
    ) -> Result<PreparedScopedReservations, ScopeAccountingError> {
        let viewer = ancestor_scope(&self.0, WebRemoteLimitScope::ViewerInstance)?;
        let request_viewer = ancestor_scope(&request.0, WebRemoteLimitScope::ViewerInstance)?;
        if viewer.lease.identity != request_viewer.lease.identity {
            return Err(ScopeAccountingError::AggregateAncestryMismatch);
        }
        self.validate_scalar(
            ScalarObservationKey::from_schema_key_internal(WebRemoteLimitKey::OpenBatchUrls),
            spec.batch_urls,
        )?;
        self.validate_scalar(
            ScalarObservationKey::from_schema_key_internal(
                WebRemoteLimitKey::DisarmedHandoffDescriptors,
            ),
            spec.descriptors,
        )?;
        self.validate_scalar(
            ScalarObservationKey::from_schema_key_internal(
                WebRemoteLimitKey::DisarmedHandoffOperations,
            ),
            spec.operations,
        )?;
        let mut plan = root.begin_internal_reservation_plan(reservation_capacity(8))?;
        for (key, amount, scope) in [
            (
                WebRemoteLimitKey::RemoteSessionSlots,
                spec.remote_session_slots,
                &viewer,
            ),
            (
                WebRemoteLimitKey::PendingSecretOpenBytes,
                spec.pending_secret_bytes,
                &viewer,
            ),
            (
                WebRemoteLimitKey::PendingOpenOptionsBytes,
                spec.pending_options_bytes,
                &viewer,
            ),
            (
                WebRemoteLimitKey::PendingExtensionlessPrepared,
                spec.extensionless_prepared,
                &viewer,
            ),
            (
                WebRemoteLimitKey::PendingFormatSniffs,
                spec.pending_sniffs,
                &viewer,
            ),
            (
                WebRemoteLimitKey::DisarmedStrictSources,
                spec.disarmed_sources,
                &viewer,
            ),
            (
                WebRemoteLimitKey::DisarmedHandoffRetainedBytes,
                spec.disarmed_retained_bytes,
                &viewer,
            ),
            (
                WebRemoteLimitKey::AtomicOpenReservationBytes,
                spec.atomic_reservation_bytes,
                &self.0,
            ),
        ] {
            plan.push(key, amount, scope)?;
        }
        plan.prepare()
    }

    pub fn prepare_open_status_reservation(
        &self,
        root: &WasmModuleLimitAccountingRoot,
        source: &SourceAccountingScope,
        spec: OpenStatusReservationSpec,
    ) -> Result<PreparedScopedReservations, ScopeAccountingError> {
        let viewer = ancestor_scope(&self.0, WebRemoteLimitScope::ViewerInstance)?;
        let source_viewer = ancestor_scope(&source.0, WebRemoteLimitScope::ViewerInstance)?;
        if viewer.lease.identity != source_viewer.lease.identity {
            return Err(ScopeAccountingError::AggregateAncestryMismatch);
        }
        source.validate_scalar(
            ScalarObservationKey::from_schema_key_internal(
                WebRemoteLimitKey::RemoteTerminalStatusBytes,
            ),
            spec.terminal_status_bytes,
        )?;
        source.validate_scalar(
            ScalarObservationKey::from_schema_key_internal(
                WebRemoteLimitKey::RemoteTerminalDiagnostics,
            ),
            spec.terminal_diagnostics,
        )?;
        self.validate_scalar(
            ScalarObservationKey::from_schema_key_internal(
                WebRemoteLimitKey::DeferredEvictionVictims,
            ),
            spec.deferred_eviction_victims,
        )?;
        let mut plan = root.begin_internal_reservation_plan(reservation_capacity(8))?;
        for (key, amount, scope) in [
            (
                WebRemoteLimitKey::OwnedRemoteClientEntries,
                spec.owned_client_entries,
                &viewer,
            ),
            (
                WebRemoteLimitKey::OpenSourceStatusSlots,
                spec.status_slots,
                &viewer,
            ),
            (
                WebRemoteLimitKey::OpenSourceLiveStatusOwners,
                spec.live_status_owners,
                &viewer,
            ),
            (
                WebRemoteLimitKey::OpenSourceTerminalEntries,
                spec.terminal_entries,
                &viewer,
            ),
            (
                WebRemoteLimitKey::OpenSourceTerminalRetainedBytes,
                spec.terminal_bytes,
                &viewer,
            ),
            (
                WebRemoteLimitKey::OpenSourceStatusRetainedBytes,
                spec.status_retained_bytes,
                &viewer,
            ),
            (
                WebRemoteLimitKey::DisarmedStatusClaims,
                spec.disarmed_status_claims,
                &viewer,
            ),
            (
                WebRemoteLimitKey::DeferredEvictionDescriptorBytes,
                spec.deferred_eviction_descriptor_bytes,
                &self.0,
            ),
        ] {
            plan.push(key, amount, scope)?;
        }
        plan.prepare()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UrlIndexReservationSpec {
    pub bucket_tokens: u64,
    pub canonical_match_bytes: u64,
    pub alias_descriptors: u64,
    pub alias_descriptor_bytes: u64,
    pub index_retained_bytes: u64,
    pub preexisting_recording_attachments: u64,
    pub preexisting_recording_attachment_bytes: u64,
    pub installation_acks: u64,
    pub installation_ack_bytes: u64,
    pub catalog_entries: u64,
    pub catalog_projection_bytes: u64,
    pub catalog_rows: u64,
    pub catalog_string_bytes: u64,
}

impl UrlFingerprintBucketAccountingScope {
    pub fn prepare_url_index_reservation(
        &self,
        root: &WasmModuleLimitAccountingRoot,
        batch: &AtomicBatchAccountingScope,
        source: &SourceAccountingScope,
        spec: UrlIndexReservationSpec,
    ) -> Result<PreparedScopedReservations, ScopeAccountingError> {
        self.validate_scalar(
            ScalarObservationKey::from_schema_key_internal(
                WebRemoteLimitKey::UrlFingerprintTokensPerBucket,
            ),
            spec.bucket_tokens,
        )?;
        let viewer = ancestor_scope(&self.0, WebRemoteLimitScope::ViewerInstance)?;
        let batch_viewer = ancestor_scope(&batch.0, WebRemoteLimitScope::ViewerInstance)?;
        let source_viewer = ancestor_scope(&source.0, WebRemoteLimitScope::ViewerInstance)?;
        if viewer.lease.identity != batch_viewer.lease.identity
            || viewer.lease.identity != source_viewer.lease.identity
        {
            return Err(ScopeAccountingError::AggregateAncestryMismatch);
        }
        for (key, amount) in [
            (
                WebRemoteLimitKey::PreexistingRecordingAttachments,
                spec.preexisting_recording_attachments,
            ),
            (
                WebRemoteLimitKey::PreexistingRecordingAttachmentBytes,
                spec.preexisting_recording_attachment_bytes,
            ),
            (WebRemoteLimitKey::InstallationAcks, spec.installation_acks),
            (
                WebRemoteLimitKey::InstallationAckRetainedBytes,
                spec.installation_ack_bytes,
            ),
        ] {
            batch.validate_scalar(ScalarObservationKey::from_schema_key_internal(key), amount)?;
        }
        source.validate_scalar(
            ScalarObservationKey::from_schema_key_internal(WebRemoteLimitKey::CatalogRows),
            spec.catalog_rows,
        )?;
        source.validate_scalar(
            ScalarObservationKey::from_schema_key_internal(WebRemoteLimitKey::CatalogStringBytes),
            spec.catalog_string_bytes,
        )?;
        let mut plan = root.begin_internal_reservation_plan(reservation_capacity(9))?;
        for (key, amount) in [
            (WebRemoteLimitKey::UrlFingerprintBuckets, 1),
            (WebRemoteLimitKey::UrlFingerprintTokens, spec.bucket_tokens),
            (
                WebRemoteLimitKey::UrlCanonicalMatchBytes,
                spec.canonical_match_bytes,
            ),
            (
                WebRemoteLimitKey::UrlAliasDescriptors,
                spec.alias_descriptors,
            ),
            (
                WebRemoteLimitKey::UrlAliasDescriptorRetainedBytes,
                spec.alias_descriptor_bytes,
            ),
            (
                WebRemoteLimitKey::UrlFingerprintIndexRetainedBytes,
                spec.index_retained_bytes,
            ),
            (WebRemoteLimitKey::CatalogEntries, spec.catalog_entries),
            (
                WebRemoteLimitKey::CatalogProjectionBytes,
                spec.catalog_projection_bytes,
            ),
        ] {
            plan.push(key, amount, &viewer)?;
        }
        plan.prepare()
    }
}

impl SourceAccountingScope {
    pub fn prepare_semantic_config_reservation(
        &self,
        root: &WasmModuleLimitAccountingRoot,
        bytes: NonZeroU64,
    ) -> Result<PreparedScopedReservations, ScopeAccountingError> {
        let mut plan = root.begin_internal_reservation_plan(NonZeroU64::MIN)?;
        plan.push(
            WebRemoteLimitKey::SemanticConfigRetainedBytes,
            bytes.get(),
            &self.0,
        )?;
        plan.prepare()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExistingIdentifierIndexReservationSpec {
    pub timelines: u64,
    pub entity_paths: u64,
    pub components: u64,
    pub entity_path_key_bytes: u64,
    pub node_bytes: u64,
    pub viewer_bytes: u64,
}

impl StoreAccountingScope {
    pub fn prepare_existing_identifier_index_reservation(
        &self,
        root: &WasmModuleLimitAccountingRoot,
        spec: ExistingIdentifierIndexReservationSpec,
    ) -> Result<PreparedScopedReservations, ScopeAccountingError> {
        let viewer = ancestor_scope(&self.0, WebRemoteLimitScope::ViewerInstance)?;
        let mut plan = root.begin_internal_reservation_plan(reservation_capacity(6))?;
        for (key, amount) in [
            (WebRemoteLimitKey::ExistingTimelineCount, spec.timelines),
            (
                WebRemoteLimitKey::ExistingEntityPathCount,
                spec.entity_paths,
            ),
            (WebRemoteLimitKey::ExistingComponentCount, spec.components),
            (
                WebRemoteLimitKey::ExistingEntityPathKeyBytes,
                spec.entity_path_key_bytes,
            ),
            (
                WebRemoteLimitKey::ExistingIdentifierNodeBytes,
                spec.node_bytes,
            ),
        ] {
            plan.push(key, amount, &self.0)?;
        }
        plan.push(
            WebRemoteLimitKey::ExistingIdentifierViewerBytes,
            spec.viewer_bytes,
            &viewer,
        )?;
        plan.prepare()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryHeadroomError {
    ArithmeticOverflow,
    InsufficientHeadroom,
}

impl fmt::Display for MemoryHeadroomError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ArithmeticOverflow => formatter.write_str("memory headroom arithmetic overflow"),
            Self::InsufficientHeadroom => formatter.write_str("insufficient memory headroom"),
        }
    }
}

impl std::error::Error for MemoryHeadroomError {}

#[derive(Clone, Copy)]
struct InternalReservationRequest<'a> {
    key: WebRemoteLimitKey,
    amount: NonZeroU64,
    scope: &'a WebRemoteAccountingScope,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ReservationAccountingCredit {
    request_entries: u64,
    request_bytes: u64,
    prepared_bytes: u64,
    active_bytes: u64,
    usage_nodes: u64,
    usage_node_bytes: u64,
    release_scratch_entries: u64,
    release_scratch_bytes: u64,
}

type ReleaseSuccessorTransfer = (ScopedUsageKey, ReservationIdentity);

fn checked_accounting_add(left: u64, right: u64) -> Result<u64, ScopeAccountingError> {
    left.checked_add(right)
        .ok_or(ScopeAccountingError::ArithmeticOverflow)
}

fn checked_accounting_mul(left: u64, right: u64) -> Result<u64, ScopeAccountingError> {
    left.checked_mul(right)
        .ok_or(ScopeAccountingError::ArithmeticOverflow)
}

const fn reservation_capacity(value: u64) -> NonZeroU64 {
    match NonZeroU64::new(value) {
        Some(value) => value,
        None => panic!("reservation plan capacity must be non-zero"),
    }
}

fn acquire_prepared_accounting_credit(
    root: &Rc<ScopeAccountingRootInner>,
    request_entries: NonZeroU64,
) -> Result<ReservationAccountingCredit, ScopeAccountingError> {
    let prepared_bytes = root
        .limits
        .raw_limit_value(WebRemoteLimitKey::AccountingPreparedReservationRecordBytes)
        .get();
    let active_bytes = root
        .limits
        .raw_limit_value(WebRemoteLimitKey::AccountingActiveReservationRecordBytes)
        .get();
    let request_bytes = checked_accounting_mul(
        request_entries.get(),
        root.limits
            .raw_limit_value(WebRemoteLimitKey::AccountingReservationRequestEntryBytes)
            .get(),
    )?;
    let release_scratch_entries = request_entries.get();
    let release_scratch_bytes = checked_accounting_mul(
        release_scratch_entries,
        root.limits
            .raw_limit_value(WebRemoteLimitKey::AccountingReleaseScratchEntryBytes)
            .get(),
    )?;
    let mut state = root.state.borrow_mut();
    let current = state.accounting_self;
    let next_prepared = checked_accounting_add(current.prepared_records, 1)?;
    let next_entries = checked_accounting_add(current.request_entries, request_entries.get())?;
    let next_prepared_bytes = checked_accounting_add(current.prepared_bytes, prepared_bytes)?;
    let next_request_bytes = checked_accounting_add(current.request_bytes, request_bytes)?;
    let next_release_scratch_entries =
        checked_accounting_add(current.release_scratch_entries, release_scratch_entries)?;
    let next_release_scratch_bytes =
        checked_accounting_add(current.release_scratch_bytes, release_scratch_bytes)?;
    for (actual, key) in [
        (
            next_prepared,
            WebRemoteLimitKey::AccountingPreparedReservationRecords,
        ),
        (
            next_entries,
            WebRemoteLimitKey::AccountingReservationRequestEntries,
        ),
        (
            next_prepared_bytes,
            WebRemoteLimitKey::AccountingPreparedReservationRetainedBytes,
        ),
        (
            next_request_bytes,
            WebRemoteLimitKey::AccountingReservationRequestRetainedBytes,
        ),
        (
            next_release_scratch_entries,
            WebRemoteLimitKey::AccountingReleaseScratchEntries,
        ),
        (
            next_release_scratch_bytes,
            WebRemoteLimitKey::AccountingReleaseScratchRetainedBytes,
        ),
    ] {
        if actual > root.limits.raw_limit_value(key).get() {
            return Err(ScopeAccountingError::LimitExceeded);
        }
    }
    state.accounting_self.prepared_records = next_prepared;
    state.accounting_self.request_entries = next_entries;
    state.accounting_self.prepared_bytes = next_prepared_bytes;
    state.accounting_self.request_bytes = next_request_bytes;
    state.accounting_self.release_scratch_entries = next_release_scratch_entries;
    state.accounting_self.release_scratch_bytes = next_release_scratch_bytes;
    Ok(ReservationAccountingCredit {
        request_entries: request_entries.get(),
        request_bytes,
        prepared_bytes,
        active_bytes,
        usage_nodes: 0,
        usage_node_bytes: 0,
        release_scratch_entries,
        release_scratch_bytes,
    })
}

fn claim_prepared_usage_nodes(
    root: &Rc<ScopeAccountingRootInner>,
    totals: &BTreeMap<ScopedUsageKey, u64>,
    credit: &mut ReservationAccountingCredit,
) -> Result<(Vec<ScopedUsageKey>, Vec<ReleaseSuccessorTransfer>), ScopeAccountingError> {
    let capacity = usize::try_from(credit.release_scratch_entries)
        .map_err(|_conversion_error| ScopeAccountingError::ArithmeticOverflow)?;
    let mut claims = Vec::new();
    claims
        .try_reserve_exact(capacity)
        .map_err(|_allocation_error| ScopeAccountingError::AllocationFailed)?;
    let mut release_scratch = Vec::new();
    release_scratch
        .try_reserve_exact(capacity)
        .map_err(|_allocation_error| ScopeAccountingError::AllocationFailed)?;
    let mut state = root.state.borrow_mut();
    claims.extend(
        totals
            .keys()
            .filter(|key| !state.usage.contains_key(key))
            .copied(),
    );
    let node_count = u64::try_from(claims.len())
        .map_err(|_conversion_error| ScopeAccountingError::ArithmeticOverflow)?;
    let node_bytes = checked_accounting_mul(
        node_count,
        root.limits
            .raw_limit_value(WebRemoteLimitKey::AccountingUsageNodeBytes)
            .get(),
    )?;
    let next_nodes = checked_accounting_add(state.accounting_self.usage_nodes, node_count)?;
    let next_bytes = checked_accounting_add(state.accounting_self.usage_node_bytes, node_bytes)?;
    if next_nodes
        > root
            .limits
            .raw_limit_value(WebRemoteLimitKey::AccountingUsageNodes)
            .get()
        || next_bytes
            > root
                .limits
                .raw_limit_value(WebRemoteLimitKey::AccountingUsageNodeRetainedBytes)
                .get()
    {
        return Err(ScopeAccountingError::LimitExceeded);
    }
    state.accounting_self.usage_nodes = next_nodes;
    state.accounting_self.usage_node_bytes = next_bytes;
    credit.usage_nodes = node_count;
    credit.usage_node_bytes = node_bytes;
    Ok((claims, release_scratch))
}

fn release_prepared_accounting_credit(
    root: &Rc<ScopeAccountingRootInner>,
    credit: ReservationAccountingCredit,
) {
    let mut state = root.state.borrow_mut();
    state.accounting_self.prepared_records -= 1;
    state.accounting_self.request_entries -= credit.request_entries;
    state.accounting_self.prepared_bytes -= credit.prepared_bytes;
    state.accounting_self.request_bytes -= credit.request_bytes;
    state.accounting_self.usage_nodes -= credit.usage_nodes;
    state.accounting_self.usage_node_bytes -= credit.usage_node_bytes;
    state.accounting_self.release_scratch_entries -= credit.release_scratch_entries;
    state.accounting_self.release_scratch_bytes -= credit.release_scratch_bytes;
}

fn move_prepared_to_active_accounting_credit(
    root: &Rc<ScopeAccountingRootInner>,
    credit: ReservationAccountingCredit,
) -> Result<(), ScopeAccountingError> {
    let mut state = root.state.borrow_mut();
    let next_prepared = state
        .accounting_self
        .prepared_records
        .checked_sub(1)
        .ok_or(ScopeAccountingError::OwnershipUnderflow)?;
    let next_prepared_bytes = state
        .accounting_self
        .prepared_bytes
        .checked_sub(credit.prepared_bytes)
        .ok_or(ScopeAccountingError::OwnershipUnderflow)?;
    let next_active = checked_accounting_add(state.accounting_self.active_records, 1)?;
    let next_active_bytes =
        checked_accounting_add(state.accounting_self.active_bytes, credit.active_bytes)?;
    if next_active
        > root
            .limits
            .raw_limit_value(WebRemoteLimitKey::AccountingActiveReservationRecords)
            .get()
        || next_active_bytes
            > root
                .limits
                .raw_limit_value(WebRemoteLimitKey::AccountingActiveReservationRetainedBytes)
                .get()
    {
        return Err(ScopeAccountingError::LimitExceeded);
    }
    state.accounting_self.prepared_records = next_prepared;
    state.accounting_self.prepared_bytes = next_prepared_bytes;
    state.accounting_self.active_records = next_active;
    state.accounting_self.active_bytes = next_active_bytes;
    Ok(())
}

/// A checked direct charge tied to an unforgeable typed scope handle.
#[derive(Clone, Copy, Debug)]
pub struct ScopedReservationRequest<'a> {
    key: ReclaimableConcurrentKey,
    amount: NonZeroU64,
    scope: &'a WebRemoteAccountingScope,
}

/// A bounded reservation plan which owns accounting credit before accepting requests.
pub struct ScopedReservationPlan<'a> {
    root: WasmModuleLimitAccountingRoot,
    requests: Vec<InternalReservationRequest<'a>>,
    capacity: usize,
    credit: Option<ReservationAccountingCredit>,
}

impl fmt::Debug for ScopedReservationPlan<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ScopedReservationPlan")
            .field("request_count", &self.requests.len())
            .field("capacity", &self.capacity)
            .finish_non_exhaustive()
    }
}

impl<'a> ScopedReservationPlan<'a> {
    pub fn push(
        &mut self,
        request: ScopedReservationRequest<'a>,
    ) -> Result<(), ScopeAccountingError> {
        if self.requests.len() == self.capacity {
            return Err(ScopeAccountingError::ReservationPlanFull);
        }
        self.requests.push(InternalReservationRequest {
            key: request.key.schema_key(),
            amount: request.amount,
            scope: request.scope,
        });
        Ok(())
    }

    pub fn prepare(mut self) -> Result<PreparedScopedReservations, ScopeAccountingError> {
        let credit = self
            .credit
            .take()
            .expect("active plan owns accounting credit");
        let requests = std::mem::take(&mut self.requests);
        let result = self
            .root
            .finish_reservation_requests(requests, false, credit);
        if result.is_err() {
            release_prepared_accounting_credit(&self.root.inner, credit);
        }
        result
    }
}

impl Drop for ScopedReservationPlan<'_> {
    fn drop(&mut self) {
        if let Some(credit) = self.credit.take() {
            release_prepared_accounting_credit(&self.root.inner, credit);
        }
    }
}

struct InternalReservationPlan<'a> {
    root: WasmModuleLimitAccountingRoot,
    requests: Vec<InternalReservationRequest<'a>>,
    capacity: usize,
    credit: Option<ReservationAccountingCredit>,
}

impl<'a> InternalReservationPlan<'a> {
    fn push(
        &mut self,
        key: WebRemoteLimitKey,
        amount: u64,
        scope: &'a WebRemoteAccountingScope,
    ) -> Result<(), ScopeAccountingError> {
        let Some(amount) = NonZeroU64::new(amount) else {
            return Ok(());
        };
        if self.requests.len() == self.capacity {
            return Err(ScopeAccountingError::ReservationPlanFull);
        }
        self.requests
            .push(InternalReservationRequest { key, amount, scope });
        Ok(())
    }

    fn prepare(mut self) -> Result<PreparedScopedReservations, ScopeAccountingError> {
        let credit = self
            .credit
            .take()
            .expect("active plan owns accounting credit");
        let requests = std::mem::take(&mut self.requests);
        let result = self
            .root
            .finish_reservation_requests(requests, true, credit);
        if result.is_err() {
            release_prepared_accounting_credit(&self.root.inner, credit);
        }
        result
    }
}

impl Drop for InternalReservationPlan<'_> {
    fn drop(&mut self) {
        if let Some(credit) = self.credit.take() {
            release_prepared_accounting_credit(&self.root.inner, credit);
        }
    }
}

/// A fully checked but disarmed scoped reservation.
pub struct PreparedScopedReservations {
    root: Rc<ScopeAccountingRootInner>,
    expected_revision: u64,
    committed_revision: u64,
    next_reservation_sequence: u64,
    reservation_identity: ReservationIdentity,
    totals: BTreeMap<ScopedUsageKey, u64>,
    scope_leases: Vec<Rc<AccountingScopeLease>>,
    usage_node_claims: Vec<ScopedUsageKey>,
    release_scratch: Vec<ReleaseSuccessorTransfer>,
    accounting_credit: Option<ReservationAccountingCredit>,
}

impl fmt::Debug for PreparedScopedReservations {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedScopedReservations")
            .field("charge_count", &self.totals.len())
            .field("identity", &"<opaque>")
            .finish_non_exhaustive()
    }
}

impl PreparedScopedReservations {
    pub fn commit(mut self) -> Result<ActiveScopedReservations, ScopeAccountingError> {
        let state = self.root.state.borrow();
        if state.revision != self.expected_revision {
            return Err(ScopeAccountingError::RevisionMismatch);
        }
        for scope in &self.scope_leases {
            let Some(record) = state.scopes.get(&scope.identity) else {
                return Err(ScopeAccountingError::StaleScope);
            };
            if !record.active || record.kind != scope.kind {
                return Err(ScopeAccountingError::StaleScope);
            }
        }
        if state.reservations.contains_key(&self.reservation_identity) {
            return Err(ScopeAccountingError::ReservationIdentityReused);
        }
        drop(state);

        let credit = self
            .accounting_credit
            .take()
            .expect("prepared reservation owns accounting credit");
        if let Err(err) = move_prepared_to_active_accounting_credit(&self.root, credit) {
            self.accounting_credit = Some(credit);
            return Err(err);
        }

        let mut state = self.root.state.borrow_mut();
        for (usage_key, amount) in &self.totals {
            let usage = state.usage.entry(*usage_key).or_default();
            usage.current = usage
                .current
                .checked_add(*amount)
                .expect("prepared reservation remains in range");
            usage.high_watermark = usage.high_watermark.max(usage.current);
        }
        let usage_node_claims = std::mem::take(&mut self.usage_node_claims);
        state.reservations.insert(
            self.reservation_identity,
            ReservationRecord {
                totals: self.totals.clone(),
                usage_node_claims,
            },
        );
        state.next_reservation_sequence = self.next_reservation_sequence;
        state.revision = self.committed_revision;
        refresh_phase_a_byte_ledger_v1(&mut state);
        drop(state);

        let totals = std::mem::take(&mut self.totals);
        let scope_leases = std::mem::take(&mut self.scope_leases);
        let release_scratch = std::mem::take(&mut self.release_scratch);
        Ok(ActiveScopedReservations {
            root: Rc::clone(&self.root),
            reservation_identity: self.reservation_identity,
            totals,
            scope_leases,
            accounting_credit: credit,
            release_scratch,
            active: true,
        })
    }
}

impl Drop for PreparedScopedReservations {
    fn drop(&mut self) {
        if let Some(credit) = self.accounting_credit.take() {
            release_prepared_accounting_credit(&self.root, credit);
        }
    }
}

/// Exact non-cloneable ownership of a committed scoped reservation.
pub struct ActiveScopedReservations {
    root: Rc<ScopeAccountingRootInner>,
    reservation_identity: ReservationIdentity,
    totals: BTreeMap<ScopedUsageKey, u64>,
    scope_leases: Vec<Rc<AccountingScopeLease>>,
    accounting_credit: ReservationAccountingCredit,
    release_scratch: Vec<ReleaseSuccessorTransfer>,
    active: bool,
}

impl fmt::Debug for ActiveScopedReservations {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ActiveScopedReservations")
            .field("charge_count", &self.totals.len())
            .field("scope_count", &self.scope_leases.len())
            .field("identity", &"<opaque>")
            .finish_non_exhaustive()
    }
}

impl ActiveScopedReservations {
    pub fn release(mut self) -> Result<(), ScopeAccountingError> {
        self.release_inner()
    }

    fn release_inner(&mut self) -> Result<(), ScopeAccountingError> {
        if !self.active {
            return Err(ScopeAccountingError::ReservationAlreadyReleased);
        }
        release_reservation_inner(
            &self.root,
            self.reservation_identity,
            &self.totals,
            self.accounting_credit,
            &mut self.release_scratch,
        )?;
        self.active = false;
        Ok(())
    }
}

impl Drop for ActiveScopedReservations {
    fn drop(&mut self) {
        if self.active {
            // All release scratch was acquired before this active owner was published.
            // A stale or deliberately forged state must not turn cleanup into a panic.
            let _result = self.release_inner();
        }
    }
}

fn create_child_scope(
    root: &Rc<ScopeAccountingRootInner>,
    parent: &Rc<AccountingScopeLease>,
    kind: WebRemoteLimitScope,
) -> Result<WebRemoteAccountingScope, ScopeAccountingError> {
    if !valid_child_scope(parent.kind, kind) {
        return Err(ScopeAccountingError::InvalidParentScope);
    }
    let Some(parent_root) = parent.root.upgrade() else {
        return Err(ScopeAccountingError::RootStopped);
    };
    if !Rc::ptr_eq(root, &parent_root) {
        return Err(ScopeAccountingError::RootMismatch);
    }
    let mut state = root.state.borrow_mut();
    let next_revision = state
        .revision
        .checked_add(1)
        .ok_or(ScopeAccountingError::RevisionExhausted)?;
    let sequence = NonZeroU64::new(state.next_scope_sequence)
        .ok_or(ScopeAccountingError::IdentityExhausted)?;
    let generation = NonZeroU64::new(state.next_scope_generation)
        .ok_or(ScopeAccountingError::IdentityExhausted)?;
    let next_sequence = state
        .next_scope_sequence
        .checked_add(1)
        .ok_or(ScopeAccountingError::IdentityExhausted)?;
    let next_generation = state
        .next_scope_generation
        .checked_add(1)
        .ok_or(ScopeAccountingError::IdentityExhausted)?;
    let Some(parent_record) = state.scopes.get(&parent.identity) else {
        return Err(ScopeAccountingError::StaleScope);
    };
    if !parent_record.active || parent_record.kind != parent.kind {
        return Err(ScopeAccountingError::StaleScope);
    }
    let next_children = parent_record
        .active_children
        .checked_add(1)
        .ok_or(ScopeAccountingError::ArithmeticOverflow)?;
    let node_bytes = root
        .limits
        .scalar_value(ScalarObservationKey::from_schema_key_internal(
            WebRemoteLimitKey::AccountingScopeNodeBytes,
        ))
        .get();
    let global_node_usage = state.global_node_usage;
    let next_global_nodes = global_node_usage
        .current_nodes
        .checked_add(1)
        .ok_or(ScopeAccountingError::ArithmeticOverflow)?;
    let next_global_bytes = global_node_usage
        .current_bytes
        .checked_add(node_bytes)
        .ok_or(ScopeAccountingError::ArithmeticOverflow)?;
    if next_global_nodes
        > root
            .limits
            .reclaimable_value(ReclaimableConcurrentKey::from_schema_key_internal(
                WebRemoteLimitKey::AccountingScopeNodesGlobal,
            ))
            .get()
        || next_global_bytes
            > root
                .limits
                .reclaimable_value(ReclaimableConcurrentKey::from_schema_key_internal(
                    WebRemoteLimitKey::AccountingScopeNodeBytesGlobal,
                ))
                .get()
    {
        return Err(ScopeAccountingError::LimitExceeded);
    }
    let parent_node_usage = state
        .child_node_usage
        .get(&parent.identity)
        .copied()
        .unwrap_or_default();
    let next_parent_nodes = parent_node_usage
        .current_nodes
        .checked_add(1)
        .ok_or(ScopeAccountingError::ArithmeticOverflow)?;
    let next_parent_bytes = parent_node_usage
        .current_bytes
        .checked_add(node_bytes)
        .ok_or(ScopeAccountingError::ArithmeticOverflow)?;
    if next_parent_nodes
        > root
            .limits
            .reclaimable_value(ReclaimableConcurrentKey::from_schema_key_internal(
                WebRemoteLimitKey::AccountingChildScopeNodesPerParent,
            ))
            .get()
        || next_parent_bytes
            > root
                .limits
                .reclaimable_value(ReclaimableConcurrentKey::from_schema_key_internal(
                    WebRemoteLimitKey::AccountingChildScopeBytesPerParent,
                ))
                .get()
    {
        return Err(ScopeAccountingError::LimitExceeded);
    }
    let identity = AccountingScopeIdentity {
        root_nonce: parent.identity.root_nonce,
        sequence,
        generation,
    };
    if state.scopes.contains_key(&identity) {
        return Err(ScopeAccountingError::ScopeIdentityReused);
    }

    state
        .scopes
        .get_mut(&parent.identity)
        .expect("validated parent remains installed")
        .active_children = next_children;
    state.scopes.insert(
        identity,
        AccountingScopeRecord {
            kind,
            parent: Some(parent.identity),
            active: true,
            active_children: 0,
            node_bytes,
        },
    );
    state.global_node_usage = ScopeNodeUsage {
        current_nodes: next_global_nodes,
        node_high_watermark: global_node_usage.node_high_watermark.max(next_global_nodes),
        current_bytes: next_global_bytes,
        byte_high_watermark: global_node_usage.byte_high_watermark.max(next_global_bytes),
    };
    state.child_node_usage.insert(
        parent.identity,
        ScopeNodeUsage {
            current_nodes: next_parent_nodes,
            node_high_watermark: parent_node_usage.node_high_watermark.max(next_parent_nodes),
            current_bytes: next_parent_bytes,
            byte_high_watermark: parent_node_usage.byte_high_watermark.max(next_parent_bytes),
        },
    );
    state.next_scope_sequence = next_sequence;
    state.next_scope_generation = next_generation;
    state.revision = next_revision;
    refresh_phase_a_byte_ledger_v1(&mut state);
    drop(state);

    Ok(WebRemoteAccountingScope {
        lease: Rc::new(AccountingScopeLease {
            root: Rc::downgrade(root),
            identity,
            kind,
            internal_projection_claims: Cell::new(0),
            _parent_lease: Some(Rc::clone(parent)),
        }),
    })
}

fn valid_child_scope(parent: WebRemoteLimitScope, child: WebRemoteLimitScope) -> bool {
    matches!(
        (parent, child),
        (
            WebRemoteLimitScope::WasmModuleLifetime,
            WebRemoteLimitScope::ViewerInstance
        ) | (
            WebRemoteLimitScope::ViewerInstance,
            WebRemoteLimitScope::Source
                | WebRemoteLimitScope::Request
                | WebRemoteLimitScope::AtomicBatch
                | WebRemoteLimitScope::Frame
                | WebRemoteLimitScope::IngressItem
                | WebRemoteLimitScope::Channel
                | WebRemoteLimitScope::UrlFingerprintBucket
                | WebRemoteLimitScope::PageExecution
                | WebRemoteLimitScope::Store
        ) | (
            WebRemoteLimitScope::Source,
            WebRemoteLimitScope::Session
                | WebRemoteLimitScope::Operation
                | WebRemoteLimitScope::Summary
        ) | (
            WebRemoteLimitScope::Summary,
            WebRemoteLimitScope::NestedRecord
        ) | (
            WebRemoteLimitScope::Session,
            WebRemoteLimitScope::Generation
                | WebRemoteLimitScope::Partition
                | WebRemoteLimitScope::Window
                | WebRemoteLimitScope::RangeResponse
                | WebRemoteLimitScope::WorkUnit
                | WebRemoteLimitScope::MessageIndexRegion
                | WebRemoteLimitScope::Store
        ) | (
            WebRemoteLimitScope::Generation,
            WebRemoteLimitScope::Chunk
                | WebRemoteLimitScope::Partition
                | WebRemoteLimitScope::WorkUnit
        ) | (
            WebRemoteLimitScope::Chunk,
            WebRemoteLimitScope::DecoderGroupPerChunk
                | WebRemoteLimitScope::NestedRecord
                | WebRemoteLimitScope::WorkUnit
                | WebRemoteLimitScope::Channel
        ) | (
            WebRemoteLimitScope::Request,
            WebRemoteLimitScope::RangeResponse | WebRemoteLimitScope::WorkUnit
        ) | (
            WebRemoteLimitScope::AtomicBatch,
            WebRemoteLimitScope::Operation
        ) | (
            WebRemoteLimitScope::Frame
                | WebRemoteLimitScope::IngressItem
                | WebRemoteLimitScope::PageExecution
                | WebRemoteLimitScope::RangeResponse
                | WebRemoteLimitScope::MessageIndexRegion
                | WebRemoteLimitScope::Partition,
            WebRemoteLimitScope::WorkUnit
        )
    )
}

fn close_scope_inner(
    root: &Rc<ScopeAccountingRootInner>,
    identity: AccountingScopeIdentity,
) -> Result<(), ScopeAccountingError> {
    let mut state = root.state.borrow_mut();
    let Some(record) = state.scopes.get(&identity) else {
        return Err(ScopeAccountingError::StaleScope);
    };
    if !record.active {
        return Err(ScopeAccountingError::StaleScope);
    }
    if record.parent.is_none() {
        return Err(ScopeAccountingError::ModuleScopeCannotClose);
    }
    if record.active_children != 0
        || state
            .reservations
            .values()
            .any(|reservation| reservation.totals.keys().any(|key| key.scope == identity))
        || state.usage.keys().any(|key| key.scope == identity)
    {
        return Err(ScopeAccountingError::ScopeBusy);
    }
    let next_revision = state
        .revision
        .checked_add(1)
        .ok_or(ScopeAccountingError::RevisionExhausted)?;
    let parent = record.parent.expect("non-module scope has a parent");
    let node_bytes = record.node_bytes;
    let Some(parent_record) = state.scopes.get(&parent) else {
        return Err(ScopeAccountingError::StaleScope);
    };
    let next_parent_children = parent_record
        .active_children
        .checked_sub(1)
        .ok_or(ScopeAccountingError::OwnershipUnderflow)?;
    let global_node_usage = state.global_node_usage;
    let next_global_nodes = global_node_usage
        .current_nodes
        .checked_sub(1)
        .ok_or(ScopeAccountingError::OwnershipUnderflow)?;
    let next_global_bytes = global_node_usage
        .current_bytes
        .checked_sub(node_bytes)
        .ok_or(ScopeAccountingError::OwnershipUnderflow)?;
    let parent_node_usage = state
        .child_node_usage
        .get(&parent)
        .copied()
        .ok_or(ScopeAccountingError::OwnershipUnderflow)?;
    let next_parent_nodes = parent_node_usage
        .current_nodes
        .checked_sub(1)
        .ok_or(ScopeAccountingError::OwnershipUnderflow)?;
    let next_parent_bytes = parent_node_usage
        .current_bytes
        .checked_sub(node_bytes)
        .ok_or(ScopeAccountingError::OwnershipUnderflow)?;

    state.scopes.remove(&identity);
    state
        .scopes
        .get_mut(&parent)
        .expect("validated parent remains installed")
        .active_children = next_parent_children;
    state.global_node_usage.current_nodes = next_global_nodes;
    state.global_node_usage.current_bytes = next_global_bytes;
    let parent_usage = state
        .child_node_usage
        .get_mut(&parent)
        .expect("validated parent node credit remains installed");
    parent_usage.current_nodes = next_parent_nodes;
    parent_usage.current_bytes = next_parent_bytes;
    state.child_node_usage.remove(&identity);
    state.revision = next_revision;
    refresh_phase_a_byte_ledger_v1(&mut state);
    Ok(())
}

fn validate_scope_identity(
    root: &Rc<ScopeAccountingRootInner>,
    scope: &WebRemoteAccountingScope,
    expected_scope: WebRemoteLimitScope,
) -> Result<(), ScopeAccountingError> {
    let Some(scope_root) = scope.lease.root.upgrade() else {
        return Err(ScopeAccountingError::RootStopped);
    };
    if !Rc::ptr_eq(root, &scope_root) {
        return Err(ScopeAccountingError::RootMismatch);
    }
    if scope.lease.kind != expected_scope {
        return Err(ScopeAccountingError::ScopeKindMismatch {
            expected: expected_scope,
            actual: scope.lease.kind,
        });
    }
    let state = root.state.borrow();
    let Some(record) = state.scopes.get(&scope.lease.identity) else {
        return Err(ScopeAccountingError::StaleScope);
    };
    if !record.active || record.kind != scope.lease.kind {
        return Err(ScopeAccountingError::StaleScope);
    }
    Ok(())
}

fn validate_scope_and_key(
    root: &Rc<ScopeAccountingRootInner>,
    scope: &WebRemoteAccountingScope,
    key: WebRemoteLimitKey,
    expected_accounting: WebRemoteAccountingKind,
) -> Result<(), ScopeAccountingError> {
    let definition = key.definition();
    if definition.accounting != expected_accounting {
        return Err(ScopeAccountingError::AccountingKindMismatch);
    }
    validate_scope_identity(root, scope, definition.scope)
}

fn release_reservation_inner(
    root: &Rc<ScopeAccountingRootInner>,
    identity: ReservationIdentity,
    expected_totals: &BTreeMap<ScopedUsageKey, u64>,
    accounting_credit: ReservationAccountingCredit,
    successor_transfers: &mut Vec<ReleaseSuccessorTransfer>,
) -> Result<(), ScopeAccountingError> {
    let mut state = root.state.borrow_mut();
    let Some(record) = state.reservations.get(&identity) else {
        return Err(ScopeAccountingError::ReservationAlreadyReleased);
    };
    if &record.totals != expected_totals {
        return Err(ScopeAccountingError::ReservationIdentityMismatch);
    }
    let usage_node_claims = &record.usage_node_claims;
    let next_revision = state
        .revision
        .checked_add(1)
        .ok_or(ScopeAccountingError::RevisionExhausted)?;
    for (usage_key, amount) in expected_totals {
        let Some(usage) = state.usage.get(usage_key) else {
            return Err(ScopeAccountingError::OwnershipUnderflow);
        };
        if usage.current < *amount {
            return Err(ScopeAccountingError::OwnershipUnderflow);
        }
    }
    successor_transfers.clear();
    if successor_transfers.capacity() < usage_node_claims.len() {
        return Err(ScopeAccountingError::ReleaseScratchInvariant);
    }
    let mut released_usage_nodes = 0_u64;
    for usage_key in usage_node_claims {
        let current = state
            .usage
            .get(usage_key)
            .expect("claimed usage node remains installed")
            .current;
        let released = expected_totals
            .get(usage_key)
            .copied()
            .ok_or(ScopeAccountingError::ReservationIdentityMismatch)?;
        let remaining = current
            .checked_sub(released)
            .ok_or(ScopeAccountingError::OwnershipUnderflow)?;
        if remaining != 0 {
            let successor = state
                .reservations
                .iter()
                .find(|(candidate_identity, reservation)| {
                    **candidate_identity != identity && reservation.totals.contains_key(usage_key)
                })
                .map(|(candidate_identity, _reservation)| *candidate_identity)
                .ok_or(ScopeAccountingError::OwnershipUnderflow)?;
            let successor_record = state
                .reservations
                .get(&successor)
                .expect("located successor remains installed");
            if successor_record.usage_node_claims.contains(usage_key)
                || successor_record.usage_node_claims.len()
                    == successor_record.usage_node_claims.capacity()
            {
                return Err(ScopeAccountingError::ReleaseScratchInvariant);
            }
            successor_transfers.push((*usage_key, successor));
        } else {
            released_usage_nodes = released_usage_nodes
                .checked_add(1)
                .ok_or(ScopeAccountingError::ArithmeticOverflow)?;
        }
    }
    let released_usage_node_bytes = checked_accounting_mul(
        released_usage_nodes,
        root.limits
            .raw_limit_value(WebRemoteLimitKey::AccountingUsageNodeBytes)
            .get(),
    )?;
    let next_active_records = state
        .accounting_self
        .active_records
        .checked_sub(1)
        .ok_or(ScopeAccountingError::OwnershipUnderflow)?;
    let next_request_entries = state
        .accounting_self
        .request_entries
        .checked_sub(accounting_credit.request_entries)
        .ok_or(ScopeAccountingError::OwnershipUnderflow)?;
    let next_active_bytes = state
        .accounting_self
        .active_bytes
        .checked_sub(accounting_credit.active_bytes)
        .ok_or(ScopeAccountingError::OwnershipUnderflow)?;
    let next_request_bytes = state
        .accounting_self
        .request_bytes
        .checked_sub(accounting_credit.request_bytes)
        .ok_or(ScopeAccountingError::OwnershipUnderflow)?;
    let next_usage_nodes = state
        .accounting_self
        .usage_nodes
        .checked_sub(released_usage_nodes)
        .ok_or(ScopeAccountingError::OwnershipUnderflow)?;
    let next_usage_node_bytes = state
        .accounting_self
        .usage_node_bytes
        .checked_sub(released_usage_node_bytes)
        .ok_or(ScopeAccountingError::OwnershipUnderflow)?;
    let next_release_scratch_entries = state
        .accounting_self
        .release_scratch_entries
        .checked_sub(accounting_credit.release_scratch_entries)
        .ok_or(ScopeAccountingError::OwnershipUnderflow)?;
    let next_release_scratch_bytes = state
        .accounting_self
        .release_scratch_bytes
        .checked_sub(accounting_credit.release_scratch_bytes)
        .ok_or(ScopeAccountingError::OwnershipUnderflow)?;

    let released_record = state
        .reservations
        .remove(&identity)
        .expect("validated reservation remains installed");
    for (usage_key, amount) in expected_totals {
        state
            .usage
            .get_mut(usage_key)
            .expect("validated usage remains installed")
            .current -= amount;
    }
    for (usage_key, successor_identity) in successor_transfers.iter().copied() {
        state
            .reservations
            .get_mut(&successor_identity)
            .expect("validated successor remains installed")
            .usage_node_claims
            .push(usage_key);
    }
    for usage_key in &released_record.usage_node_claims {
        if state
            .usage
            .get(usage_key)
            .is_some_and(|usage| usage.current == 0)
        {
            state.usage.remove(usage_key);
        }
    }
    state.accounting_self.active_records = next_active_records;
    state.accounting_self.request_entries = next_request_entries;
    state.accounting_self.active_bytes = next_active_bytes;
    state.accounting_self.request_bytes = next_request_bytes;
    state.accounting_self.usage_nodes = next_usage_nodes;
    state.accounting_self.usage_node_bytes = next_usage_node_bytes;
    state.accounting_self.release_scratch_entries = next_release_scratch_entries;
    state.accounting_self.release_scratch_bytes = next_release_scratch_bytes;
    state.revision = next_revision;
    refresh_phase_a_byte_ledger_v1(&mut state);
    Ok(())
}

fn refresh_phase_a_byte_ledger_v1(state: &mut ScopeAccountingState) {
    let mut total = Some(0_u64);
    let mut add = |bytes: u64| {
        total = total.and_then(|current| current.checked_add(bytes));
    };
    add(state.global_node_usage.current_bytes);
    add(state.accounting_self.prepared_bytes);
    add(state.accounting_self.active_bytes);
    add(state.accounting_self.request_bytes);
    add(state.accounting_self.usage_node_bytes);
    add(state.accounting_self.release_scratch_bytes);
    for (key, usage) in &state.usage {
        if matches!(
            key.key.definition().unit,
            WebRemoteLimitUnit::Bytes | WebRemoteLimitUnit::Utf8Bytes
        ) {
            add(usage.current);
        }
    }
    if let Some(current_bytes) = total {
        state.phase_a_byte_ledger.current_bytes = current_bytes;
        state.phase_a_byte_ledger.high_water_bytes = state
            .phase_a_byte_ledger
            .high_water_bytes
            .max(current_bytes);
    } else {
        state.phase_a_byte_ledger.current_bytes = u64::MAX;
        state.phase_a_byte_ledger.high_water_bytes = u64::MAX;
        state.phase_a_byte_ledger.overflowed = true;
    }
}

/// A low-cardinality failure from generation-aware scope accounting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeAccountingError {
    RootStopped,
    RootMismatch,
    InvalidParentScope,
    ScopeKindMismatch {
        expected: WebRemoteLimitScope,
        actual: WebRemoteLimitScope,
    },
    StaleScope,
    ScopeBusy,
    ModuleScopeCannotClose,
    AccountingKindMismatch,
    AggregateReservationRequired,
    AggregateAncestryMismatch,
    FormulaViolation,
    EmptyReservation,
    ReservationPlanFull,
    AllocationFailed,
    ReleaseScratchInvariant,
    ArithmeticOverflow,
    OwnershipUnderflow,
    LimitExceeded,
    RevisionExhausted,
    RevisionMismatch,
    IdentityExhausted,
    ScopeIdentityReused,
    ReservationIdentityReused,
    ReservationIdentityMismatch,
    ReservationAlreadyReleased,
}

impl fmt::Display for ScopeAccountingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RootStopped => formatter.write_str("accounting root stopped"),
            Self::RootMismatch => formatter.write_str("accounting scope belongs to another root"),
            Self::InvalidParentScope => formatter.write_str("invalid accounting parent scope"),
            Self::ScopeKindMismatch { .. } => formatter.write_str("accounting scope kind mismatch"),
            Self::StaleScope => formatter.write_str("accounting scope is stale"),
            Self::ScopeBusy => formatter.write_str("accounting scope still owns resources"),
            Self::ModuleScopeCannotClose => {
                formatter.write_str("module accounting scope cannot close independently")
            }
            Self::AccountingKindMismatch => formatter.write_str("accounting kind mismatch"),
            Self::AggregateReservationRequired => {
                formatter.write_str("typed aggregate reservation required")
            }
            Self::AggregateAncestryMismatch => {
                formatter.write_str("aggregate reservation ancestry mismatch")
            }
            Self::FormulaViolation => formatter.write_str("aggregate formula violation"),
            Self::EmptyReservation => formatter.write_str("resource reservation is empty"),
            Self::ReservationPlanFull => formatter.write_str("resource reservation plan is full"),
            Self::AllocationFailed => formatter.write_str("resource reservation allocation failed"),
            Self::ReleaseScratchInvariant => {
                formatter.write_str("release scratch ownership invariant failed")
            }
            Self::ArithmeticOverflow => formatter.write_str("resource arithmetic overflow"),
            Self::OwnershipUnderflow => formatter.write_str("resource ownership underflow"),
            Self::LimitExceeded => formatter.write_str("resource limit exceeded"),
            Self::RevisionExhausted => formatter.write_str("accounting revision exhausted"),
            Self::RevisionMismatch => formatter.write_str("prepared accounting revision mismatch"),
            Self::IdentityExhausted => formatter.write_str("accounting identity exhausted"),
            Self::ScopeIdentityReused => formatter.write_str("accounting scope identity reused"),
            Self::ReservationIdentityReused => {
                formatter.write_str("accounting reservation identity reused")
            }
            Self::ReservationIdentityMismatch => {
                formatter.write_str("accounting reservation identity mismatch")
            }
            Self::ReservationAlreadyReleased => {
                formatter.write_str("accounting reservation already released")
            }
        }
    }
}

impl std::error::Error for ScopeAccountingError {}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn digest_for(key: WebRemoteLimitKey) -> MeasurementEvidenceDigest {
        let mut bytes = [0_u8; 32];
        let encoded = (key.index() as u64 + 1).to_le_bytes();
        bytes[..encoded.len()].copy_from_slice(&encoded);
        MeasurementEvidenceDigest::new(bytes).expect("non-zero key digest")
    }

    fn nz(value: u64) -> NonZeroU64 {
        NonZeroU64::new(value).expect("test value is non-zero")
    }

    fn scalar(key: WebRemoteLimitKey) -> ScalarObservationKey {
        ScalarObservationKey::try_from_schema_key(key).expect("test key is scalar")
    }

    fn reclaimable(key: WebRemoteLimitKey) -> ReclaimableConcurrentKey {
        ReclaimableConcurrentKey::try_from_schema_key(key)
            .expect("test key is directly reclaimable")
    }

    fn burn(key: WebRemoteLimitKey) -> ModuleLifetimeBurnKey {
        ModuleLifetimeBurnKey::try_from_schema_key(key).expect("test key is module lifetime burn")
    }

    fn threshold(key: WebRemoteLimitKey) -> TelemetryAcceptanceThresholdKey {
        TelemetryAcceptanceThresholdKey::try_from_schema_key(key)
            .expect("test key is a telemetry threshold")
    }

    fn measured_value(key: WebRemoteLimitKey) -> NonZeroU64 {
        let value = match key {
            WebRemoteLimitKey::OpenSourceStatusSlots
            | WebRemoteLimitKey::RuntimeInternEntryAndCapacityBytes
            | WebRemoteLimitKey::RuntimeInternCandidatePeakBytes => 512,
            WebRemoteLimitKey::OpenSourceLiveStatusOwners
            | WebRemoteLimitKey::AccountingPreparedReservationRecords
            | WebRemoteLimitKey::AccountingActiveReservationRecords => 256,
            WebRemoteLimitKey::OpenSourceTerminalEntries
            | WebRemoteLimitKey::WrapperCacheEntries => 128,
            WebRemoteLimitKey::OpenSourceStatusRetainedBytes
            | WebRemoteLimitKey::LifecycleOutstandingDeliveryBytes
            | WebRemoteLimitKey::TypescriptDispatcherRetainedBytes => 131_072,
            WebRemoteLimitKey::OpenSourceTerminalRetainedBytes => 65_536,
            WebRemoteLimitKey::RecordingsPerSource | WebRemoteLimitKey::OpenOperations => 16,
            WebRemoteLimitKey::LifecycleOutstandingDeliveries
            | WebRemoteLimitKey::TypescriptDispatcherItems => 2_048,
            WebRemoteLimitKey::LifecycleListenerErrorCredits
            | WebRemoteLimitKey::PromiseClosureBytes => 1_024,
            WebRemoteLimitKey::AccountingReleaseScratchEntries
            | WebRemoteLimitKey::AccountingScopeNodeBytesGlobal
            | WebRemoteLimitKey::AccountingChildScopeBytesPerParent
            | WebRemoteLimitKey::AccountingReservationRequestEntries
            | WebRemoteLimitKey::AccountingUsageNodes => 4_096,
            WebRemoteLimitKey::AccountingPreparedReservationRetainedBytes
            | WebRemoteLimitKey::AccountingActiveReservationRetainedBytes
            | WebRemoteLimitKey::AccountingReservationRequestRetainedBytes
            | WebRemoteLimitKey::AccountingUsageNodeRetainedBytes
            | WebRemoteLimitKey::AccountingReleaseScratchRetainedBytes => 262_144,
            WebRemoteLimitKey::RemoteGcWorkUnitsPerFrame => 1,
            _ => 64,
        };
        nz(value)
    }

    fn complete_test_artifact() -> ValidatedFrozenWebRemoteLimitsV1 {
        let mut draft = DraftWebRemoteLimitsV1::new();
        let measurements = WebRemoteLimitKey::ALL
            .into_iter()
            .filter_map(|key| {
                let WebRemoteLimitState::Unfrozen { stage, evidence } = draft.state(key) else {
                    return None;
                };
                Some(MeasuredLimitValue {
                    key,
                    value: measured_value(key),
                    evidence: MeasurementEvidence {
                        stage,
                        kind: evidence,
                        artifact_digest: digest_for(key),
                    },
                })
            })
            .collect::<Vec<_>>();
        draft
            .prepare_measurements(measurements)
            .expect("prepare complete test evidence")
            .commit(&mut draft)
            .expect("commit complete test evidence");
        draft.validate_complete().expect("complete test profile")
    }

    pub(crate) fn complete_test_profile() -> ProductionWebRemoteLimitsV1 {
        ProductionWebRemoteLimitsV1::from_validated_artifact_for_test(&complete_test_artifact())
    }

    fn complete_test_values() -> [NonZeroU64; WEB_REMOTE_LIMIT_COUNT] {
        complete_test_artifact().values
    }

    pub(crate) fn test_profile_with(
        overrides: &[(WebRemoteLimitKey, u64)],
    ) -> ProductionWebRemoteLimitsV1 {
        let mut values = complete_test_values();
        for (key, value) in overrides {
            values[key.index()] = nz(*value);
        }
        validate_complete_profile(&values).expect("test overrides preserve profile constraints");
        ProductionWebRemoteLimitsV1 { values }
    }

    /// Byte capacity that lets one prepared transport attempt fit inside the fabricated profile.
    ///
    /// The fabricated profile keeps a deliberately tiny fallback for unlisted byte keys so that
    /// tests can exercise aggregate limits, so transport scenarios must raise the capacities their
    /// own reservations consume: one output buffer, its BYOB scratch, their overlap, and one
    /// parsed entity-tag.
    pub(crate) const TRANSPORT_TEST_BYTE_CAP: u64 = 65_536;

    /// [`test_profile_with`] extended with the byte capacities a transport scenario needs.
    ///
    /// Explicit `overrides` win over the transport capacities so a scenario can still force one
    /// specific limit failure.
    pub(crate) fn transport_test_profile_with(
        overrides: &[(WebRemoteLimitKey, u64)],
    ) -> ProductionWebRemoteLimitsV1 {
        let mut values = complete_test_values();
        for key in [
            WebRemoteLimitKey::RequestedRangeBytes,
            WebRemoteLimitKey::ByobPumpSliceBytes,
            WebRemoteLimitKey::FetchWasmRawRetainedBytes,
            WebRemoteLimitKey::ByobScratchBytes,
            WebRemoteLimitKey::RangeJsWasmOverlapBytes,
            WebRemoteLimitKey::RemoteInternalRetainedBytes,
            WebRemoteLimitKey::RemoteValidatorRetainedBytes,
        ] {
            values[key.index()] = nz(TRANSPORT_TEST_BYTE_CAP);
        }
        for (key, value) in overrides {
            values[key.index()] = nz(*value);
        }
        validate_complete_profile(&values).expect("test overrides preserve profile constraints");
        ProductionWebRemoteLimitsV1 { values }
    }

    fn assert_constraint_error(
        overrides: &[(WebRemoteLimitKey, u64)],
        expected: LimitProfileCompletionError,
    ) {
        let mut values = complete_test_values();
        for (key, value) in overrides {
            values[key.index()] = nz(*value);
        }
        assert_eq!(validate_complete_profile(&values), Err(expected));
    }

    fn metadata_fingerprint() -> u64 {
        const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

        let mut hash = FNV_OFFSET;
        for key in WebRemoteLimitKey::ALL {
            let definition = key.definition();
            let row = format!(
                "{}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
                definition.stable_name,
                definition.profile_version,
                definition.domain,
                definition.unit,
                definition.scope,
                definition.accounting,
                definition.telemetry,
                definition.enforcement,
                definition.design_requirement,
            );
            for byte in row
                .bytes()
                .chain(format!("|{:?}\n", definition.requirement).bytes())
            {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(FNV_PRIME);
            }
        }
        for constraint in LimitProfileConstraint::ALL {
            for byte in format!(
                "constraint|{constraint:?}|{:?}\n",
                constraint.design_requirement()
            )
            .bytes()
            {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(FNV_PRIME);
            }
        }
        for formula in NormativeAggregateFormula::ALL {
            for byte in format!(
                "formula|{formula:?}|{:?}|{:?}\n",
                formula.design_requirement(),
                formula.typed_entry_point(),
            )
            .bytes()
            {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(FNV_PRIME);
            }
        }
        hash
    }

    /// Independently reviewed Web-only remote-MCAP schema allowlist.
    ///
    /// This deliberately duplicates the key inventory instead of deriving it from the schema
    /// macro, so adding any unrelated transport family fails review even if counts and hashes are
    /// updated mechanically.
    const REMOTE_MCAP_V1_KEY_ALLOWLIST: &[WebRemoteLimitKey] = &[
        WebRemoteLimitKey::AccountingScopeNodesGlobal,
        WebRemoteLimitKey::AccountingScopeNodeBytesGlobal,
        WebRemoteLimitKey::AccountingChildScopeNodesPerParent,
        WebRemoteLimitKey::AccountingChildScopeBytesPerParent,
        WebRemoteLimitKey::AccountingScopeNodeBytes,
        WebRemoteLimitKey::AccountingPreparedReservationRecords,
        WebRemoteLimitKey::AccountingActiveReservationRecords,
        WebRemoteLimitKey::AccountingReservationRequestEntries,
        WebRemoteLimitKey::AccountingUsageNodes,
        WebRemoteLimitKey::AccountingPreparedReservationRetainedBytes,
        WebRemoteLimitKey::AccountingActiveReservationRetainedBytes,
        WebRemoteLimitKey::AccountingReservationRequestRetainedBytes,
        WebRemoteLimitKey::AccountingUsageNodeRetainedBytes,
        WebRemoteLimitKey::AccountingPreparedReservationRecordBytes,
        WebRemoteLimitKey::AccountingActiveReservationRecordBytes,
        WebRemoteLimitKey::AccountingReservationRequestEntryBytes,
        WebRemoteLimitKey::AccountingUsageNodeBytes,
        WebRemoteLimitKey::AccountingReleaseScratchEntries,
        WebRemoteLimitKey::AccountingReleaseScratchRetainedBytes,
        WebRemoteLimitKey::AccountingReleaseScratchEntryBytes,
        WebRemoteLimitKey::SummaryBytes,
        WebRemoteLimitKey::SummaryRecordCount,
        WebRemoteLimitKey::SummarySchemaCount,
        WebRemoteLimitKey::SummaryChannelCount,
        WebRemoteLimitKey::SummaryChunkIndexCount,
        WebRemoteLimitKey::PhysicalRegionDescriptorCount,
        WebRemoteLimitKey::ChannelMetadataEntriesPerRecord,
        WebRemoteLimitKey::ChannelMetadataKeyBytes,
        WebRemoteLimitKey::ChannelMetadataValueBytes,
        WebRemoteLimitKey::NestedRetainedBytesPerSummary,
        WebRemoteLimitKey::NestedRetainedBytesPerChunkScan,
        WebRemoteLimitKey::ChunkIndexMessageIndexOffsets,
        WebRemoteLimitKey::StatisticsChannelMessageCounts,
        WebRemoteLimitKey::MessageIndexEntriesPerRecord,
        WebRemoteLimitKey::MessageIndexRegionBytes,
        WebRemoteLimitKey::MessageIndexRegionRecordCount,
        WebRemoteLimitKey::MessageIndexRegionEntryCount,
        WebRemoteLimitKey::AmbiguousZeroMessageIndexBytes,
        WebRemoteLimitKey::AmbiguousZeroMessageIndexEntries,
        WebRemoteLimitKey::AmbiguousZeroMessageIndexRanges,
        WebRemoteLimitKey::AmbiguousZeroChunkCompressedBytes,
        WebRemoteLimitKey::AmbiguousZeroChunkUncompressedBytes,
        WebRemoteLimitKey::AmbiguousZeroChunkRanges,
        WebRemoteLimitKey::ChunkCompressedBytes,
        WebRemoteLimitKey::ChunkUncompressedBytes,
        WebRemoteLimitKey::ChunkDecompressionRatioPermille,
        WebRemoteLimitKey::DecompressorOverflowScratchBytes,
        WebRemoteLimitKey::ChunkRecordCount,
        WebRemoteLimitKey::ChunkMessageCount,
        WebRemoteLimitKey::SelectedMessageDispatchesPerScan,
        WebRemoteLimitKey::ManifestDenseChannelCount,
        WebRemoteLimitKey::ManifestDensePayloadPlanBytes,
        WebRemoteLimitKey::ValidationPlanEntries,
        WebRemoteLimitKey::ValidationPlanRetainedBytes,
        WebRemoteLimitKey::DecompressedChunkRetainedBytes,
        WebRemoteLimitKey::SourceRecordDerivedOrdinal,
        WebRemoteLimitKey::DecoderWorkingBytes,
        WebRemoteLimitKey::DecoderBuilderBytes,
        WebRemoteLimitKey::DecoderPayloadScratchBytes,
        WebRemoteLimitKey::DecoderLensIntermediateBytes,
        WebRemoteLimitKey::DecoderTerminalOutputBytes,
        WebRemoteLimitKey::DecoderDerivedRows,
        WebRemoteLimitKey::DecoderGenerationWorkingBytes,
        WebRemoteLimitKey::DecoderChunkWorkingBytes,
        WebRemoteLimitKey::DecoderSessionWorkingBytes,
        WebRemoteLimitKey::DecoderGlobalWorkingBytes,
        WebRemoteLimitKey::DecoderGenerationOutputRows,
        WebRemoteLimitKey::DecoderChunkOutputRows,
        WebRemoteLimitKey::DecoderSessionOutputRows,
        WebRemoteLimitKey::DecoderGlobalOutputRows,
        WebRemoteLimitKey::PartitionOutputBytes,
        WebRemoteLimitKey::PartitionDerivedRoots,
        WebRemoteLimitKey::WindowChunkHits,
        WebRemoteLimitKey::SelectedChannelGroups,
        WebRemoteLimitKey::SelectedChannelGroupMemberships,
        WebRemoteLimitKey::SelectedChannelAssignmentsPerChannel,
        WebRemoteLimitKey::PlanningCrossProductOperations,
        WebRemoteLimitKey::GenerationPartitions,
        WebRemoteLimitKey::GenerationTerminalBatches,
        WebRemoteLimitKey::GenerationDerivedRoots,
        WebRemoteLimitKey::GenerationRootDescriptors,
        WebRemoteLimitKey::RootsPerPartition,
        WebRemoteLimitKey::ExternalOriginBytesPerPartition,
        WebRemoteLimitKey::SessionRegisteredPartitions,
        WebRemoteLimitKey::SessionCompleteEmptyEntries,
        WebRemoteLimitKey::SessionRootDescriptors,
        WebRemoteLimitKey::SessionExternalOriginBytes,
        WebRemoteLimitKey::SessionResidentPhysicalRoots,
        WebRemoteLimitKey::SessionRegistrationMetadataBytes,
        WebRemoteLimitKey::RemoteInternalRetainedBytes,
        WebRemoteLimitKey::RemoteStagedTerminalBatchBytes,
        WebRemoteLimitKey::RemotePinnedResidentBytes,
        WebRemoteLimitKey::RemoteEntityDbRetainedBytes,
        WebRemoteLimitKey::RemoteStoreResidentBytes,
        WebRemoteLimitKey::FetchWasmRawRetainedBytes,
        WebRemoteLimitKey::ConcurrentRangeRequests,
        WebRemoteLimitKey::InFlightRangeBytes,
        WebRemoteLimitKey::RangeRetryAttemptsPerOperation,
        WebRemoteLimitKey::RequestedRangeBytes,
        WebRemoteLimitKey::ByobScratchBytes,
        WebRemoteLimitKey::RangeJsWasmOverlapBytes,
        WebRemoteLimitKey::RemoteValidatorIngressValues,
        WebRemoteLimitKey::RemoteValidatorIngressWireBytes,
        WebRemoteLimitKey::RemoteValidatorIngressJsWasmOverlapBytes,
        WebRemoteLimitKey::RemoteValidatorIngressScratchBytes,
        WebRemoteLimitKey::RemoteValidatorRetainedBytes,
        WebRemoteLimitKey::RemoteValidatorEgressValues,
        WebRemoteLimitKey::RemoteValidatorEgressWireBytes,
        WebRemoteLimitKey::RemoteValidatorEgressJsWasmOverlapBytes,
        WebRemoteLimitKey::RemoteValidatorEgressScratchBytes,
        WebRemoteLimitKey::ExtensionlessSnifferBufferBytes,
        WebRemoteLimitKey::ByobPumpSliceBytes,
        WebRemoteLimitKey::ByobPumpSliceDurationMicros,
        WebRemoteLimitKey::MetadataOpeningBytes,
        WebRemoteLimitKey::MetadataOpeningRangeRequests,
        WebRemoteLimitKey::MetadataOpeningVisibleDeadlineMillis,
        WebRemoteLimitKey::RemoteCpuWorkUnitsPerFrame,
        WebRemoteLimitKey::OpeningParseInputBytes,
        WebRemoteLimitKey::OpeningParseOutputBytes,
        WebRemoteLimitKey::OpeningParseRecordCount,
        WebRemoteLimitKey::OpeningParseMessageCount,
        WebRemoteLimitKey::OpeningParseDispatchCount,
        WebRemoteLimitKey::OpeningParseDurationMicros,
        WebRemoteLimitKey::ChunkValidationInputBytes,
        WebRemoteLimitKey::ChunkValidationOutputBytes,
        WebRemoteLimitKey::ChunkValidationRecordCount,
        WebRemoteLimitKey::ChunkValidationMessageCount,
        WebRemoteLimitKey::ChunkValidationDispatchCount,
        WebRemoteLimitKey::ChunkValidationDurationMicros,
        WebRemoteLimitKey::ChunkDispatchInputBytes,
        WebRemoteLimitKey::ChunkDispatchOutputBytes,
        WebRemoteLimitKey::ChunkDispatchRecordCount,
        WebRemoteLimitKey::ChunkDispatchMessageCount,
        WebRemoteLimitKey::ChunkDispatchActualAppendCount,
        WebRemoteLimitKey::ChunkDispatchDurationMicros,
        WebRemoteLimitKey::MessageIndexParseDurationMicros,
        WebRemoteLimitKey::RemoteSessionSlots,
        WebRemoteLimitKey::OpenBatchUrls,
        WebRemoteLimitKey::PendingSecretOpenBytes,
        WebRemoteLimitKey::PendingOpenOptionsBytes,
        WebRemoteLimitKey::PendingExtensionlessPrepared,
        WebRemoteLimitKey::PendingFormatSniffs,
        WebRemoteLimitKey::DisarmedStrictSources,
        WebRemoteLimitKey::DisarmedHandoffRetainedBytes,
        WebRemoteLimitKey::DisarmedHandoffDescriptors,
        WebRemoteLimitKey::DisarmedHandoffOperations,
        WebRemoteLimitKey::AtomicOpenReservationBytes,
        WebRemoteLimitKey::UrlFingerprintBuckets,
        WebRemoteLimitKey::UrlFingerprintTokens,
        WebRemoteLimitKey::UrlFingerprintTokensPerBucket,
        WebRemoteLimitKey::UrlCanonicalMatchBytes,
        WebRemoteLimitKey::UrlAliasDescriptors,
        WebRemoteLimitKey::UrlFingerprintIndexRetainedBytes,
        WebRemoteLimitKey::UrlAliasDescriptorRetainedBytes,
        WebRemoteLimitKey::PreexistingRecordingAttachments,
        WebRemoteLimitKey::PreexistingRecordingAttachmentBytes,
        WebRemoteLimitKey::InstallationAcks,
        WebRemoteLimitKey::InstallationAckRetainedBytes,
        WebRemoteLimitKey::CatalogEntries,
        WebRemoteLimitKey::CatalogProjectionBytes,
        WebRemoteLimitKey::CatalogRows,
        WebRemoteLimitKey::CatalogStringBytes,
        WebRemoteLimitKey::SemanticConfigRetainedBytes,
        WebRemoteLimitKey::OwnedRemoteClientEntries,
        WebRemoteLimitKey::OpenSourceStatusSlots,
        WebRemoteLimitKey::OpenSourceTerminalEntries,
        WebRemoteLimitKey::OpenSourceTerminalRetainedBytes,
        WebRemoteLimitKey::OpenSourceStatusRetainedBytes,
        WebRemoteLimitKey::OpenSourceLiveStatusOwners,
        WebRemoteLimitKey::RemoteTerminalStatusBytes,
        WebRemoteLimitKey::RemoteTerminalDiagnostics,
        WebRemoteLimitKey::DisarmedStatusClaims,
        WebRemoteLimitKey::DeferredEvictionVictims,
        WebRemoteLimitKey::DeferredEvictionDescriptorBytes,
        WebRemoteLimitKey::OpenSourceOwners,
        WebRemoteLimitKey::OpenSourceOwnerRetainedBytes,
        WebRemoteLimitKey::OpenOperations,
        WebRemoteLimitKey::OpenOperationRetainedBytes,
        WebRemoteLimitKey::PublicRecordingHandles,
        WebRemoteLimitKey::PublicRecordingHandleRetainedBytes,
        WebRemoteLimitKey::RecordingsPerSource,
        WebRemoteLimitKey::OperationRecordingSubscriptions,
        WebRemoteLimitKey::OperationRecordingSubscriptionRetainedBytes,
        WebRemoteLimitKey::LifecycleSubscribers,
        WebRemoteLimitKey::LifecycleSubscriberRetainedBytes,
        WebRemoteLimitKey::LifecycleHubs,
        WebRemoteLimitKey::LifecycleHubRetainedBytes,
        WebRemoteLimitKey::LifecycleChildStates,
        WebRemoteLimitKey::LifecycleChildStateBytes,
        WebRemoteLimitKey::ActivationDeliveryAcks,
        WebRemoteLimitKey::ActivationDeliveryAckBytes,
        WebRemoteLimitKey::LatestLifecycleSnapshots,
        WebRemoteLimitKey::LatestLifecycleSnapshotBytes,
        WebRemoteLimitKey::LifecycleOutstandingDeliveries,
        WebRemoteLimitKey::LifecycleOutstandingDeliveryBytes,
        WebRemoteLimitKey::LifecycleListenerErrorCredits,
        WebRemoteLimitKey::SingleLifecycleEventBytes,
        WebRemoteLimitKey::TypescriptDispatcherItems,
        WebRemoteLimitKey::TypescriptDispatcherRetainedBytes,
        WebRemoteLimitKey::LifecycleScheduledTasks,
        WebRemoteLimitKey::LifecycleCurrentlyDispatching,
        WebRemoteLimitKey::ListenerErrorNotificationsPerEvent,
        WebRemoteLimitKey::PromiseClosureBytes,
        WebRemoteLimitKey::PromiseClosureBytesPerOperation,
        WebRemoteLimitKey::WrapperCacheEntries,
        WebRemoteLimitKey::WrapperCacheRetainedBytes,
        WebRemoteLimitKey::RemovedTombstoneRetainers,
        WebRemoteLimitKey::RemovedTombstoneRetainedBytes,
        WebRemoteLimitKey::PresentationFacades,
        WebRemoteLimitKey::PresentationFacadeRetainedBytes,
        WebRemoteLimitKey::PresentationSnapshots,
        WebRemoteLimitKey::PresentationSnapshotRetainedBytes,
        WebRemoteLimitKey::PresentationActiveLeases,
        WebRemoteLimitKey::PresentationActiveLeaseRetainedBytes,
        WebRemoteLimitKey::PageLifecycleListeners,
        WebRemoteLimitKey::PageHiddenWakeFlags,
        WebRemoteLimitKey::PageParkedResponseOwners,
        WebRemoteLimitKey::PageParkedResponseOwnerRetainedBytes,
        WebRemoteLimitKey::PageVisibleDeadlineBookkeepingBytes,
        WebRemoteLimitKey::PageResumeRevalidationWorkCount,
        WebRemoteLimitKey::PageResumeRevalidationDurationMicros,
        WebRemoteLimitKey::PageTeardownCleanupItems,
        WebRemoteLimitKey::PageTeardownDurationMicros,
        WebRemoteLimitKey::RawCacheBytes,
        WebRemoteLimitKey::ExternalFieldsPerCall,
        WebRemoteLimitKey::ExternalUtf16CodeUnitsPerCall,
        WebRemoteLimitKey::ExternalUtf8BytesPerCall,
        WebRemoteLimitKey::TimelineUtf16CodeUnits,
        WebRemoteLimitKey::TimelineUtf8Bytes,
        WebRemoteLimitKey::TopicFilterUtf16CodeUnits,
        WebRemoteLimitKey::TopicFilterUtf8Bytes,
        WebRemoteLimitKey::DecoderSelectorUtf16CodeUnits,
        WebRemoteLimitKey::DecoderSelectorUtf8Bytes,
        WebRemoteLimitKey::RouteFieldUtf16CodeUnits,
        WebRemoteLimitKey::RouteFieldUtf8Bytes,
        WebRemoteLimitKey::RouteFragmentFields,
        WebRemoteLimitKey::EntityPathParts,
        WebRemoteLimitKey::EntityPathUtf8Bytes,
        WebRemoteLimitKey::ExternalStringCopyCalls,
        WebRemoteLimitKey::ExternalStringCopyAllocationBytes,
        WebRemoteLimitKey::ExistingTimelineCount,
        WebRemoteLimitKey::ExistingEntityPathCount,
        WebRemoteLimitKey::ExistingComponentCount,
        WebRemoteLimitKey::ExistingEntityPathKeyBytes,
        WebRemoteLimitKey::ExistingIdentifierNodeBytes,
        WebRemoteLimitKey::ExistingIdentifierViewerBytes,
        WebRemoteLimitKey::RuntimeInternStringBytes,
        WebRemoteLimitKey::RuntimeInternEntryAndCapacityBytes,
        WebRemoteLimitKey::RuntimeInternCensusIdentifiers,
        WebRemoteLimitKey::RuntimeInternCensusRetainedBytes,
        WebRemoteLimitKey::RuntimeInternCandidatePeakBytes,
        WebRemoteLimitKey::AddChunkInputBytes,
        WebRemoteLimitKey::AddChunkRows,
        WebRemoteLimitKey::AddChunkComponentCount,
        WebRemoteLimitKey::AddChunkTimelineCount,
        WebRemoteLimitKey::AddChunkDurationMicros,
        WebRemoteLimitKey::PendingInsertionBytesPerFrame,
        WebRemoteLimitKey::AddChunkCallsPerFrame,
        WebRemoteLimitKey::SharedRemoteWorkUnitsPerFrame,
        WebRemoteLimitKey::RemoteGcWorkUnitsPerFrame,
        WebRemoteLimitKey::RemoteCpuAllowanceMicrosPerFrame,
        WebRemoteLimitKey::RemoteGcCpuAllowanceMicrosPerFrame,
        WebRemoteLimitKey::RemoteGcCandidateRoots,
        WebRemoteLimitKey::RemoteGcDurationMicros,
        WebRemoteLimitKey::MemoryPressurePendingRequests,
        WebRemoteLimitKey::MemoryPressureAllocationPauseStates,
        WebRemoteLimitKey::MemoryPressureCompletionDeadlineMillis,
        WebRemoteLimitKey::ProcessSafetyHeadroomBytes,
        WebRemoteLimitKey::BackfillRangeRequestsPerSeek,
        WebRemoteLimitKey::BackfillMessageIndexBytesPerSeek,
        WebRemoteLimitKey::BackfillParsedEntriesPerSeek,
        WebRemoteLimitKey::BackfillVisibleDeadlineMillis,
    ];

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct RequirementCoverageGolden {
        requirement: NormativeResourceRequirement,
        keys: usize,
        constraints: usize,
        formulas: usize,
    }

    fn actual_requirement_coverage() -> Vec<RequirementCoverageGolden> {
        NormativeResourceRequirement::ALL
            .into_iter()
            .map(|requirement| RequirementCoverageGolden {
                requirement,
                keys: WebRemoteLimitKey::ALL
                    .into_iter()
                    .filter(|key| key.design_requirement() == requirement)
                    .count(),
                constraints: LimitProfileConstraint::ALL
                    .into_iter()
                    .filter(|constraint| constraint.design_requirement() == requirement)
                    .count(),
                formulas: NormativeAggregateFormula::ALL
                    .into_iter()
                    .filter(|formula| formula.design_requirement() == requirement)
                    .count(),
            })
            .collect()
    }

    #[test]
    fn schema_inventory_has_unique_names_complete_metadata_and_golden_fingerprint() {
        let mut names = BTreeSet::new();
        let mut covered_requirements = BTreeSet::new();
        let draft = DraftWebRemoteLimitsV1::new();
        assert!(draft.unfrozen_count() > 0);
        assert_eq!(
            WebRemoteLimitKey::count(),
            EXPECTED_WEB_REMOTE_LIMIT_COUNT_V1
        );
        assert_eq!(
            WebRemoteLimitKey::ALL.as_slice(),
            REMOTE_MCAP_V1_KEY_ALLOWLIST
        );
        let WebRemoteLimitState::Frozen(ingress_values) =
            draft.state(WebRemoteLimitKey::RemoteValidatorIngressValues)
        else {
            panic!("the absent-or-single entity-tag grammar must be frozen");
        };
        assert_eq!(ingress_values.value, NonZeroU64::MIN);
        assert!(matches!(
            ingress_values.source,
            FrozenLimitValueSource::DesignConstant { .. }
        ));

        for key in WebRemoteLimitKey::ALL {
            let definition = key.definition();
            assert_eq!(definition.key, key);
            assert_eq!(
                definition.profile_version,
                WebRemoteLimitsProfileVersion::V1
            );
            assert_eq!(definition.enforcement, key.enforcement());
            if definition.enforcement == WebRemoteLimitEnforcement::TelemetryAcceptanceThreshold {
                assert_eq!(definition.unit, WebRemoteLimitUnit::Microseconds);
                assert_eq!(
                    definition.accounting,
                    WebRemoteAccountingKind::ScalarObservation
                );
            }
            assert!(names.insert(definition.stable_name));
            covered_requirements.insert(definition.design_requirement);
            match (definition.requirement, draft.state(key)) {
                (
                    WebRemoteLimitRequirement::DesignConstant(constant),
                    WebRemoteLimitState::Frozen(frozen),
                ) => {
                    assert_eq!(constant.value, frozen.value);
                    assert_ne!(frozen.value.get(), u64::MAX);
                }
                (
                    WebRemoteLimitRequirement::Measurement { stage, evidence },
                    WebRemoteLimitState::Unfrozen {
                        stage: actual_stage,
                        evidence: actual_evidence,
                    },
                ) => {
                    assert_eq!(stage, actual_stage);
                    assert_eq!(evidence, actual_evidence);
                }
                other => panic!("definition/state mismatch: {other:?}"),
            }
        }
        covered_requirements.extend(
            LimitProfileConstraint::ALL
                .into_iter()
                .map(LimitProfileConstraint::design_requirement),
        );
        covered_requirements.extend(
            NormativeAggregateFormula::ALL
                .into_iter()
                .map(NormativeAggregateFormula::design_requirement),
        );
        assert_eq!(
            covered_requirements,
            NormativeResourceRequirement::ALL.into_iter().collect()
        );
        assert_eq!(metadata_fingerprint(), 0xc975_59dc_064b_9d7c);
    }

    #[test]
    fn remote_runtime_intern_limits_are_projected_only_from_the_sealed_profile() {
        for key in [
            WebRemoteLimitKey::RuntimeInternStringBytes,
            WebRemoteLimitKey::RuntimeInternEntryAndCapacityBytes,
            WebRemoteLimitKey::RuntimeInternCensusIdentifiers,
            WebRemoteLimitKey::RuntimeInternCensusRetainedBytes,
            WebRemoteLimitKey::RuntimeInternCandidatePeakBytes,
        ] {
            assert!(matches!(
                key.definition().requirement,
                WebRemoteLimitRequirement::Measurement {
                    stage: MeasurementStage::PhaseAReleaseWasmChrome,
                    evidence: EvidenceKind::ReleaseWasmChromeBenchmark,
                }
            ));
        }
        let limits = complete_test_profile().remote_mcap_runtime_intern_limits();
        re_string_interner::bounded_runtime_intern::validate_remote_mcap_runtime_intern_profile(
            limits,
        )
        .expect("the complete production profile can prepare the fixed side-map");
        assert_eq!(
            format!("{limits:?}"),
            "RemoteMcapRuntimeInternLimits { values: \"<sealed>\" }"
        );
    }

    #[test]
    fn runtime_intern_cross_key_minimums_fail_during_profile_validation() {
        for (key, value) in [
            (
                WebRemoteLimitKey::RuntimeInternCandidatePeakBytes,
                RUNTIME_INTERN_MINIMUM_SIDE_MAP_CAPACITY_BYTES_V1 - 1,
            ),
            (
                WebRemoteLimitKey::RuntimeInternEntryAndCapacityBytes,
                RUNTIME_INTERN_MINIMUM_ENTRY_AND_CAPACITY_BYTES_V1 - 1,
            ),
        ] {
            assert_constraint_error(
                &[(key, value)],
                LimitProfileCompletionError::ConstraintViolation {
                    constraint: LimitProfileConstraint::RuntimeInternMinimumSideMapFitsBudgets,
                },
            );
        }
    }

    #[test]
    fn every_normative_requirement_has_independently_frozen_key_or_formula_coverage() {
        use NormativeResourceRequirement as R;

        macro_rules! golden {
            ($($requirement:ident => ($keys:expr, $constraints:expr, $formulas:expr)),+ $(,)?) => {
                &[$(RequirementCoverageGolden {
                    requirement: R::$requirement,
                    keys: $keys,
                    constraints: $constraints,
                    formulas: $formulas,
                }),+]
            };
        }

        const GOLDEN: &[RequirementCoverageGolden] = golden! {
            SummaryBytes => (1, 0, 0),
            SummaryRecordCount => (1, 0, 0),
            SummarySchemaCount => (1, 0, 0),
            SummaryChannelCount => (1, 0, 0),
            SummaryChunkIndexCount => (1, 0, 0),
            PhysicalRegionDescriptorCount => (1, 0, 0),
            ChannelMetadataAndNestedRetention => (5, 0, 0),
            ChunkIndexMessageIndexOffsets => (1, 0, 0),
            StatisticsChannelMessageCounts => (1, 0, 0),
            MessageIndexRecordEntries => (1, 0, 0),
            MessageIndexOwningRegion => (3, 0, 0),
            AmbiguousZeroMessageIndex => (3, 0, 0),
            AmbiguousZeroChunk => (3, 0, 0),
            ChunkCompressedBytes => (1, 0, 0),
            ChunkUncompressedBytes => (1, 0, 0),
            ChunkDecompressionRatio => (1, 0, 0),
            ExactOutputDecompressor => (1, 0, 0),
            ChunkRecordCount => (1, 0, 0),
            ChunkMessageCount => (1, 0, 0),
            ValidationAndDispatchCounts => (1, 0, 1),
            ManifestDenseValidationPlan => (5, 0, 0),
            DecoderGroupAndAggregateBounds => (14, 0, 1),
            PartitionOutputBytes => (1, 0, 0),
            PartitionDerivedRoots => (1, 0, 0),
            SourceRecordDerivedOrdinal => (1, 0, 0),
            WindowIntervalHits => (1, 0, 0),
            SelectedChannelGroups => (1, 0, 0),
            SelectedMembershipAndAssignment => (2, 0, 0),
            PartitionRootAndOriginBounds => (2, 0, 0),
            PlanningCrossProduct => (1, 0, 1),
            SessionPartitionFormula => (0, 0, 1),
            SessionDescriptorOriginFormula => (0, 0, 1),
            RegistrationMetadataHeadroom => (1, 0, 1),
            GenerationPartitions => (1, 0, 0),
            GenerationTerminalRoots => (3, 0, 0),
            SessionRegisteredPartitions => (1, 0, 0),
            SessionCompleteEmptyEntries => (1, 0, 0),
            SessionRootDescriptors => (1, 0, 0),
            SessionExternalOriginBytes => (1, 0, 0),
            SessionResidentPhysicalRoots => (1, 0, 0),
            ConcurrentRangeRequests => (1, 0, 0),
            InFlightRangeBytes => (1, 0, 0),
            RangeRetry => (1, 0, 0),
            RangeByobOverlap => (3, 0, 1),
            RemoteValidatorByteString => (9, 0, 3),
            ExtensionlessSniffer => (1, 0, 0),
            OpenAdmission => (11, 0, 1),
            OpenSourceIndexAndCatalog => (16, 1, 1),
            TerminalStatusRegistry => (8, 6, 1),
            DisarmedTerminalClaims => (3, 4, 0),
            PublicLifecycle => (34, 9, 1),
            PresentationQueryOwnership => (6, 0, 2),
            ChromePageExecution => (9, 0, 1),
            MetadataOpeningAndCpu => (4, 0, 0),
            RawCache => (1, 0, 0),
            OpeningValidationDispatch => (18, 0, 0),
            MessageIndexParse => (1, 0, 0),
            ByobPump => (2, 0, 0),
            AddChunk => (5, 0, 0),
            InsertionFrame => (2, 0, 0),
            RemoteGc => (6, 3, 1),
            MemoryPressure => (4, 0, 1),
            BackfillRangeRequests => (1, 0, 0),
            BackfillMessageIndexBytes => (1, 0, 0),
            BackfillParsedEntries => (1, 0, 0),
            BackfillDeadline => (1, 0, 0),
            AccountingScopeOwnership => (5, 2, 1),
            ExternalStrings => (16, 0, 0),
            ExistingIdentifierIndex => (6, 0, 1),
            RuntimeInternBudget => (5, 1, 1),
            RemoteInternalTotal => (6, 0, 1),
            AccountingSelfOwnership => (15, 0, 1),
        };
        let actual = actual_requirement_coverage();
        assert!(
            actual
                .iter()
                .all(|entry| entry.keys + entry.constraints + entry.formulas > 0)
        );
        assert_eq!(actual, GOLDEN);
    }

    #[derive(Clone, Copy, Debug)]
    struct OwnerClosureGolden {
        requirement: NormativeResourceRequirement,
        keys: &'static [WebRemoteLimitKey],
        constraints: &'static [LimitProfileConstraint],
        formulas: &'static [(NormativeAggregateFormula, AggregateTypedEntryPoint)],
    }

    #[test]
    fn design_owner_closure_inventory_freezes_exact_composite_members() {
        use AggregateTypedEntryPoint as Entry;
        use LimitProfileConstraint as Constraint;
        use NormativeAggregateFormula as Formula;
        use NormativeResourceRequirement as Requirement;
        use WebRemoteLimitKey as Key;

        const INVENTORY: &[OwnerClosureGolden] = &[
            OwnerClosureGolden {
                requirement: Requirement::ValidationAndDispatchCounts,
                keys: &[Key::SelectedMessageDispatchesPerScan],
                constraints: &[],
                formulas: &[(
                    Formula::ValidationEstimateAndActualDispatch,
                    Entry::ValidateDispatchFormula,
                )],
            },
            OwnerClosureGolden {
                requirement: Requirement::DecoderGroupAndAggregateBounds,
                keys: &[
                    Key::DecoderWorkingBytes,
                    Key::DecoderBuilderBytes,
                    Key::DecoderPayloadScratchBytes,
                    Key::DecoderLensIntermediateBytes,
                    Key::DecoderTerminalOutputBytes,
                    Key::DecoderDerivedRows,
                    Key::DecoderGenerationWorkingBytes,
                    Key::DecoderChunkWorkingBytes,
                    Key::DecoderSessionWorkingBytes,
                    Key::DecoderGlobalWorkingBytes,
                    Key::DecoderGenerationOutputRows,
                    Key::DecoderChunkOutputRows,
                    Key::DecoderSessionOutputRows,
                    Key::DecoderGlobalOutputRows,
                ],
                constraints: &[],
                formulas: &[(
                    Formula::DecoderAggregateReservation,
                    Entry::PrepareDecoderReservation,
                )],
            },
            OwnerClosureGolden {
                requirement: Requirement::PlanningCrossProduct,
                keys: &[Key::PlanningCrossProductOperations],
                constraints: &[],
                formulas: &[(
                    Formula::PlanningCrossProductArithmetic,
                    Entry::ValidatePlanningCrossProduct,
                )],
            },
            OwnerClosureGolden {
                requirement: Requirement::SessionPartitionFormula,
                keys: &[],
                constraints: &[],
                formulas: &[(
                    Formula::SessionPartitionArithmetic,
                    Entry::ValidateSessionPartitionFormula,
                )],
            },
            OwnerClosureGolden {
                requirement: Requirement::SessionDescriptorOriginFormula,
                keys: &[],
                constraints: &[],
                formulas: &[(
                    Formula::SessionDescriptorOriginArithmetic,
                    Entry::ValidateSessionDescriptorFormula,
                )],
            },
            OwnerClosureGolden {
                requirement: Requirement::RegistrationMetadataHeadroom,
                keys: &[Key::SessionRegistrationMetadataBytes],
                constraints: &[],
                formulas: &[(
                    Formula::RegistrationMetadataHeadroomReservation,
                    Entry::PrepareRegistrationMetadataFormula,
                )],
            },
            OwnerClosureGolden {
                requirement: Requirement::RangeByobOverlap,
                keys: &[
                    Key::RequestedRangeBytes,
                    Key::ByobScratchBytes,
                    Key::RangeJsWasmOverlapBytes,
                ],
                constraints: &[],
                formulas: &[(
                    Formula::RangeBodyPumpReservation,
                    Entry::PrepareRangeBodyPumpReservation,
                )],
            },
            OwnerClosureGolden {
                requirement: Requirement::RemoteInternalTotal,
                keys: &[
                    Key::RemoteInternalRetainedBytes,
                    Key::RemoteStagedTerminalBatchBytes,
                    Key::RemotePinnedResidentBytes,
                    Key::RemoteEntityDbRetainedBytes,
                    Key::RemoteStoreResidentBytes,
                    Key::FetchWasmRawRetainedBytes,
                ],
                constraints: &[],
                formulas: &[(
                    Formula::RemoteInternalTotalGate,
                    Entry::PrepareRemoteInternalReservation,
                )],
            },
            OwnerClosureGolden {
                requirement: Requirement::RemoteValidatorByteString,
                keys: &[
                    Key::RemoteValidatorIngressValues,
                    Key::RemoteValidatorIngressWireBytes,
                    Key::RemoteValidatorIngressJsWasmOverlapBytes,
                    Key::RemoteValidatorIngressScratchBytes,
                    Key::RemoteValidatorRetainedBytes,
                    Key::RemoteValidatorEgressValues,
                    Key::RemoteValidatorEgressWireBytes,
                    Key::RemoteValidatorEgressJsWasmOverlapBytes,
                    Key::RemoteValidatorEgressScratchBytes,
                ],
                constraints: &[],
                formulas: &[
                    (
                        Formula::RemoteValidatorIngressReservation,
                        Entry::PrepareRemoteValidatorIngress,
                    ),
                    (
                        Formula::RemoteValidatorRetainedReservation,
                        Entry::PrepareRemoteValidatorRetained,
                    ),
                    (
                        Formula::RemoteValidatorEgressReservation,
                        Entry::PrepareRemoteValidatorEgress,
                    ),
                ],
            },
            OwnerClosureGolden {
                requirement: Requirement::OpenAdmission,
                keys: &[
                    Key::RemoteSessionSlots,
                    Key::OpenBatchUrls,
                    Key::PendingSecretOpenBytes,
                    Key::PendingOpenOptionsBytes,
                    Key::PendingExtensionlessPrepared,
                    Key::PendingFormatSniffs,
                    Key::DisarmedStrictSources,
                    Key::DisarmedHandoffRetainedBytes,
                    Key::DisarmedHandoffDescriptors,
                    Key::DisarmedHandoffOperations,
                    Key::AtomicOpenReservationBytes,
                ],
                constraints: &[],
                formulas: &[(
                    Formula::OpenAdmissionReservation,
                    Entry::PrepareOpenAdmissionReservation,
                )],
            },
            OwnerClosureGolden {
                requirement: Requirement::OpenSourceIndexAndCatalog,
                keys: &[
                    Key::UrlFingerprintBuckets,
                    Key::UrlFingerprintTokens,
                    Key::UrlFingerprintTokensPerBucket,
                    Key::UrlCanonicalMatchBytes,
                    Key::UrlAliasDescriptors,
                    Key::UrlFingerprintIndexRetainedBytes,
                    Key::UrlAliasDescriptorRetainedBytes,
                    Key::PreexistingRecordingAttachments,
                    Key::PreexistingRecordingAttachmentBytes,
                    Key::InstallationAcks,
                    Key::InstallationAckRetainedBytes,
                    Key::CatalogEntries,
                    Key::CatalogProjectionBytes,
                    Key::CatalogRows,
                    Key::CatalogStringBytes,
                    Key::SemanticConfigRetainedBytes,
                ],
                constraints: &[Constraint::FingerprintTokensPerBucketFitGlobal],
                formulas: &[(
                    Formula::UrlIndexReservation,
                    Entry::PrepareUrlIndexReservation,
                )],
            },
            OwnerClosureGolden {
                requirement: Requirement::TerminalStatusRegistry,
                keys: &[
                    Key::OwnedRemoteClientEntries,
                    Key::OpenSourceStatusSlots,
                    Key::OpenSourceTerminalEntries,
                    Key::OpenSourceTerminalRetainedBytes,
                    Key::OpenSourceStatusRetainedBytes,
                    Key::OpenSourceLiveStatusOwners,
                    Key::RemoteTerminalStatusBytes,
                    Key::RemoteTerminalDiagnostics,
                ],
                constraints: &[
                    Constraint::BatchFitsTerminalRegistry,
                    Constraint::TerminalRegistryFitsStatusRegistry,
                    Constraint::TerminalRetainedBytesCoverTerminalEntries,
                    Constraint::StatusComponentsFitLiveOwners,
                    Constraint::StatusRetainedBytesCoverOwnerSlots,
                    Constraint::PublishedLiveAndTerminalFitStatusSlots,
                ],
                formulas: &[(
                    Formula::OpenStatusReservation,
                    Entry::PrepareOpenStatusReservation,
                )],
            },
            OwnerClosureGolden {
                requirement: Requirement::PublicLifecycle,
                keys: &[
                    Key::OpenSourceOwners,
                    Key::OpenSourceOwnerRetainedBytes,
                    Key::OpenOperations,
                    Key::OpenOperationRetainedBytes,
                    Key::PublicRecordingHandles,
                    Key::PublicRecordingHandleRetainedBytes,
                    Key::RecordingsPerSource,
                    Key::OperationRecordingSubscriptions,
                    Key::OperationRecordingSubscriptionRetainedBytes,
                    Key::LifecycleSubscribers,
                    Key::LifecycleSubscriberRetainedBytes,
                    Key::LifecycleHubs,
                    Key::LifecycleHubRetainedBytes,
                    Key::LifecycleChildStates,
                    Key::LifecycleChildStateBytes,
                    Key::ActivationDeliveryAcks,
                    Key::ActivationDeliveryAckBytes,
                    Key::LatestLifecycleSnapshots,
                    Key::LatestLifecycleSnapshotBytes,
                    Key::LifecycleOutstandingDeliveries,
                    Key::LifecycleOutstandingDeliveryBytes,
                    Key::LifecycleListenerErrorCredits,
                    Key::SingleLifecycleEventBytes,
                    Key::TypescriptDispatcherItems,
                    Key::TypescriptDispatcherRetainedBytes,
                    Key::LifecycleScheduledTasks,
                    Key::LifecycleCurrentlyDispatching,
                    Key::ListenerErrorNotificationsPerEvent,
                    Key::PromiseClosureBytes,
                    Key::PromiseClosureBytesPerOperation,
                    Key::WrapperCacheEntries,
                    Key::WrapperCacheRetainedBytes,
                    Key::RemovedTombstoneRetainers,
                    Key::RemovedTombstoneRetainedBytes,
                ],
                constraints: &[
                    Constraint::LifecycleWorstCaseRustDeliveryCount,
                    Constraint::LifecycleWorstCaseListenerErrorCredits,
                    Constraint::LifecycleWorstCaseRustDeliveryBytes,
                    Constraint::LifecycleWorstCaseTypescriptDeliveryCount,
                    Constraint::LifecycleWorstCaseTypescriptDeliveryBytes,
                    Constraint::PromiseClosureBytesCoverOperations,
                    Constraint::WrapperCacheCoversOperationsAndRecordings,
                    Constraint::RecordingHandlesFitTombstoneRetainers,
                    Constraint::OperationSubscriptionsCoverRecordingHandles,
                ],
                formulas: &[(
                    Formula::LifecycleRetentionBundle,
                    Entry::PrepareLifecycleReservation,
                )],
            },
            OwnerClosureGolden {
                requirement: Requirement::PresentationQueryOwnership,
                keys: &[
                    Key::PresentationFacades,
                    Key::PresentationFacadeRetainedBytes,
                    Key::PresentationSnapshots,
                    Key::PresentationSnapshotRetainedBytes,
                    Key::PresentationActiveLeases,
                    Key::PresentationActiveLeaseRetainedBytes,
                ],
                constraints: &[],
                formulas: &[
                    (
                        Formula::PresentationActivationReservation,
                        Entry::ActivatePresentation,
                    ),
                    (
                        Formula::PresentationLeaseReservation,
                        Entry::AcquirePresentationLease,
                    ),
                ],
            },
            OwnerClosureGolden {
                requirement: Requirement::ChromePageExecution,
                keys: &[
                    Key::PageLifecycleListeners,
                    Key::PageHiddenWakeFlags,
                    Key::PageParkedResponseOwners,
                    Key::PageParkedResponseOwnerRetainedBytes,
                    Key::PageVisibleDeadlineBookkeepingBytes,
                    Key::PageResumeRevalidationWorkCount,
                    Key::PageResumeRevalidationDurationMicros,
                    Key::PageTeardownCleanupItems,
                    Key::PageTeardownDurationMicros,
                ],
                constraints: &[],
                formulas: &[(
                    Formula::PageParkedResponseOwnerReservation,
                    Entry::PreparePageParkedResponseOwnerReservation,
                )],
            },
            OwnerClosureGolden {
                requirement: Requirement::RemoteGc,
                keys: &[
                    Key::SharedRemoteWorkUnitsPerFrame,
                    Key::RemoteGcWorkUnitsPerFrame,
                    Key::RemoteCpuAllowanceMicrosPerFrame,
                    Key::RemoteGcCpuAllowanceMicrosPerFrame,
                    Key::RemoteGcCandidateRoots,
                    Key::RemoteGcDurationMicros,
                ],
                constraints: &[
                    Constraint::RemoteGcFitsSharedCpuAllowance,
                    Constraint::RemoteCpuWorkFitsSharedFrameCap,
                    Constraint::RemoteGcWorkFitsSharedFrameCap,
                ],
                formulas: &[(
                    Formula::SharedFrameWorkUnitReservation,
                    Entry::PrepareSharedFrameWork,
                )],
            },
            OwnerClosureGolden {
                requirement: Requirement::MemoryPressure,
                keys: &[
                    Key::MemoryPressurePendingRequests,
                    Key::MemoryPressureAllocationPauseStates,
                    Key::MemoryPressureCompletionDeadlineMillis,
                    Key::ProcessSafetyHeadroomBytes,
                ],
                constraints: &[],
                formulas: &[(Formula::MemoryHeadroomGate, Entry::ValidateMemoryHeadroom)],
            },
            OwnerClosureGolden {
                requirement: Requirement::AccountingScopeOwnership,
                keys: &[
                    Key::AccountingScopeNodesGlobal,
                    Key::AccountingScopeNodeBytesGlobal,
                    Key::AccountingChildScopeNodesPerParent,
                    Key::AccountingChildScopeBytesPerParent,
                    Key::AccountingScopeNodeBytes,
                ],
                constraints: &[
                    Constraint::GlobalScopeNodeBytesCoverNodes,
                    Constraint::ParentScopeNodeBytesCoverNodes,
                ],
                formulas: &[(
                    Formula::ScopeNodeCreditReservation,
                    Entry::CreateChildScopeWithNodeCredit,
                )],
            },
            OwnerClosureGolden {
                requirement: Requirement::AccountingSelfOwnership,
                keys: &[
                    Key::AccountingPreparedReservationRecords,
                    Key::AccountingActiveReservationRecords,
                    Key::AccountingReservationRequestEntries,
                    Key::AccountingUsageNodes,
                    Key::AccountingPreparedReservationRetainedBytes,
                    Key::AccountingActiveReservationRetainedBytes,
                    Key::AccountingReservationRequestRetainedBytes,
                    Key::AccountingUsageNodeRetainedBytes,
                    Key::AccountingPreparedReservationRecordBytes,
                    Key::AccountingActiveReservationRecordBytes,
                    Key::AccountingReservationRequestEntryBytes,
                    Key::AccountingUsageNodeBytes,
                    Key::AccountingReleaseScratchEntries,
                    Key::AccountingReleaseScratchRetainedBytes,
                    Key::AccountingReleaseScratchEntryBytes,
                ],
                constraints: &[],
                formulas: &[(
                    Formula::AccountingSelfCreditReservation,
                    Entry::BeginReservationPlanWithAccountingCredit,
                )],
            },
            OwnerClosureGolden {
                requirement: Requirement::ExistingIdentifierIndex,
                keys: &[
                    Key::ExistingTimelineCount,
                    Key::ExistingEntityPathCount,
                    Key::ExistingComponentCount,
                    Key::ExistingEntityPathKeyBytes,
                    Key::ExistingIdentifierNodeBytes,
                    Key::ExistingIdentifierViewerBytes,
                ],
                constraints: &[],
                formulas: &[(
                    Formula::ExistingIdentifierIndexReservation,
                    Entry::PrepareExistingIdentifierIndexReservation,
                )],
            },
            OwnerClosureGolden {
                requirement: Requirement::RuntimeInternBudget,
                keys: &[
                    Key::RuntimeInternStringBytes,
                    Key::RuntimeInternEntryAndCapacityBytes,
                    Key::RuntimeInternCensusIdentifiers,
                    Key::RuntimeInternCensusRetainedBytes,
                    Key::RuntimeInternCandidatePeakBytes,
                ],
                constraints: &[Constraint::RuntimeInternMinimumSideMapFitsBudgets],
                formulas: &[(
                    Formula::RuntimeInternInitializationProfile,
                    Entry::ValidateRuntimeInternInitializationProfile,
                )],
            },
        ];

        let mut requirements = BTreeSet::new();
        for closure in INVENTORY {
            assert!(requirements.insert(closure.requirement));
            let actual_keys = WebRemoteLimitKey::ALL
                .into_iter()
                .filter(|key| key.design_requirement() == closure.requirement)
                .collect::<Vec<_>>();
            assert_eq!(actual_keys, closure.keys);
            for key in closure.keys {
                let definition = key.definition();
                assert_eq!(definition.design_requirement, closure.requirement);
                assert!(!definition.stable_name.is_empty());
                match definition.requirement {
                    WebRemoteLimitRequirement::DesignConstant(constant) => {
                        assert_ne!(constant.value.get(), 0);
                    }
                    WebRemoteLimitRequirement::Measurement { stage, evidence } => {
                        assert!(matches!(
                            stage,
                            MeasurementStage::PhaseAReleaseWasmChrome
                                | MeasurementStage::PhaseBAdversarialHarness
                        ));
                        assert!(matches!(
                            evidence,
                            EvidenceKind::ReleaseWasmChromeBenchmark
                                | EvidenceKind::AdversarialResourceHarness
                        ));
                    }
                }
                let _reviewed_unit = definition.unit;
                let _reviewed_scope = definition.scope;
                let _reviewed_accounting = definition.accounting;
            }
            let actual_constraints = LimitProfileConstraint::ALL
                .into_iter()
                .filter(|constraint| constraint.design_requirement() == closure.requirement)
                .collect::<Vec<_>>();
            assert_eq!(actual_constraints, closure.constraints);
            let actual_formulas = NormativeAggregateFormula::ALL
                .into_iter()
                .filter(|formula| formula.design_requirement() == closure.requirement)
                .map(|formula| (formula, formula.typed_entry_point()))
                .collect::<Vec<_>>();
            let expected_formulas = closure
                .formulas
                .iter()
                .map(|(formula, entry)| (*formula, *entry))
                .collect::<Vec<_>>();
            assert_eq!(actual_formulas, expected_formulas);
        }
        assert_eq!(
            requirements,
            NormativeAggregateFormula::ALL
                .into_iter()
                .map(NormativeAggregateFormula::design_requirement)
                .collect()
        );
    }

    #[test]
    fn accounting_metadata_has_reviewed_ownership_boundaries() {
        for key in WebRemoteLimitKey::ALL {
            let definition = key.definition();
            match definition.accounting {
                WebRemoteAccountingKind::FormulaConstant => {
                    assert_eq!(key, WebRemoteLimitKey::ProcessSafetyHeadroomBytes);
                }
                WebRemoteAccountingKind::ModuleLifetimeBurn => {
                    assert_eq!(definition.scope, WebRemoteLimitScope::WasmModuleLifetime);
                }
                WebRemoteAccountingKind::ScalarObservation
                | WebRemoteAccountingKind::ReclaimableConcurrent => {}
            }
        }
        assert_eq!(
            WebRemoteLimitKey::UrlFingerprintTokensPerBucket
                .definition()
                .scope,
            WebRemoteLimitScope::UrlFingerprintBucket
        );
        assert_eq!(
            WebRemoteLimitKey::SelectedChannelAssignmentsPerChannel
                .definition()
                .scope,
            WebRemoteLimitScope::Channel
        );
    }

    #[test]
    fn incomplete_profile_has_no_production_capability() {
        assert!(matches!(
            DraftWebRemoteLimitsV1::new().validate_complete(),
            Err(LimitProfileCompletionError::Unfrozen { .. })
        ));
    }

    #[test]
    fn evidence_mismatch_and_dropped_prepare_are_exact_rollbacks() {
        let mut draft = DraftWebRemoteLimitsV1::new();
        let before = draft.clone();
        let key = WebRemoteLimitKey::SummaryBytes;
        let WebRemoteLimitState::Unfrozen { stage, evidence } = draft.state(key) else {
            panic!("summary bytes is measured");
        };
        let wrong_stage = match stage {
            MeasurementStage::PhaseAReleaseWasmChrome => MeasurementStage::PhaseBAdversarialHarness,
            MeasurementStage::PhaseBAdversarialHarness => MeasurementStage::PhaseAReleaseWasmChrome,
        };
        let result = draft.prepare_measurements([MeasuredLimitValue {
            key,
            value: reservation_capacity(1),
            evidence: MeasurementEvidence {
                stage: wrong_stage,
                kind: evidence,
                artifact_digest: digest_for(key),
            },
        }]);
        assert!(matches!(
            result,
            Err(LimitProfileTransitionError::EvidenceMismatch { .. })
        ));
        assert_eq!(draft, before);

        let prepared = draft
            .prepare_measurements([MeasuredLimitValue {
                key,
                value: reservation_capacity(1),
                evidence: MeasurementEvidence {
                    stage,
                    kind: evidence,
                    artifact_digest: digest_for(key),
                },
            }])
            .expect("valid prepare");
        drop(prepared);
        assert_eq!(draft, before);

        let prepared = draft
            .prepare_measurements([MeasuredLimitValue {
                key,
                value: reservation_capacity(1),
                evidence: MeasurementEvidence {
                    stage,
                    kind: evidence,
                    artifact_digest: digest_for(key),
                },
            }])
            .expect("valid prepare");
        prepared.commit(&mut draft).expect("valid commit");
        assert_eq!(draft.revision(), 1);
        assert!(matches!(draft.state(key), WebRemoteLimitState::Frozen(_)));
    }

    #[test]
    fn stale_profile_transition_is_an_exact_rollback() {
        let mut draft = DraftWebRemoteLimitsV1::new();
        let prepare = |key: WebRemoteLimitKey, draft: &DraftWebRemoteLimitsV1| {
            let WebRemoteLimitState::Unfrozen { stage, evidence } = draft.state(key) else {
                panic!("test key is measured");
            };
            draft
                .prepare_measurements([MeasuredLimitValue {
                    key,
                    value: reservation_capacity(1),
                    evidence: MeasurementEvidence {
                        stage,
                        kind: evidence,
                        artifact_digest: digest_for(key),
                    },
                }])
                .unwrap()
        };

        let first = prepare(WebRemoteLimitKey::SummaryBytes, &draft);
        let stale = prepare(WebRemoteLimitKey::SummaryRecordCount, &draft);
        first.commit(&mut draft).unwrap();
        let before_stale_commit = draft.clone();
        assert_eq!(
            stale.commit(&mut draft),
            Err(LimitProfileTransitionError::RevisionMismatch)
        );
        assert_eq!(draft, before_stale_commit);
    }

    #[test]
    fn complete_tooling_artifact_retains_evidence() {
        let artifact = complete_test_artifact();
        assert_eq!(
            ValidatedFrozenWebRemoteLimitsV1::version(),
            WebRemoteLimitsProfileVersion::V1
        );
        assert_eq!(artifact.profile_revision(), 1);
        assert!(matches!(
            artifact.frozen(WebRemoteLimitKey::SummaryBytes).source,
            FrozenLimitValueSource::Measurement(MeasurementEvidence {
                stage: MeasurementStage::PhaseAReleaseWasmChrome,
                kind: EvidenceKind::ReleaseWasmChromeBenchmark,
                ..
            })
        ));
    }

    #[test]
    fn accounting_kinds_arithmetic_and_threshold_observation_are_separate() {
        let root = complete_test_profile().start_accounting_root().unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let source = viewer.create_source_scope().unwrap();
        let summary = source.create_summary_scope().unwrap();
        let session = source.create_session_scope().unwrap();
        let work = session.create_work_unit_scope().unwrap();
        assert_eq!(
            summary
                .checked_scalar_sum(scalar(WebRemoteLimitKey::SummaryBytes), [32, 32])
                .unwrap(),
            64
        );
        assert!(matches!(
            summary.checked_scalar_sum(scalar(WebRemoteLimitKey::SummaryBytes), [u64::MAX, 1]),
            Err(ScopeAccountingError::ArithmeticOverflow)
        ));
        assert!(matches!(
            summary.checked_scalar_product(scalar(WebRemoteLimitKey::SummaryBytes), 65, 1),
            Err(ScopeAccountingError::LimitExceeded)
        ));
        assert_eq!(
            work.observe_threshold(
                threshold(WebRemoteLimitKey::ChunkValidationDurationMicros),
                64
            ),
            Ok(TelemetryThresholdObservation::WithinThreshold)
        );
        assert_eq!(
            work.observe_threshold(
                threshold(WebRemoteLimitKey::ChunkValidationDurationMicros),
                65
            ),
            Ok(TelemetryThresholdObservation::ExceededThreshold)
        );
        assert_eq!(
            ScalarObservationKey::try_from_schema_key(
                WebRemoteLimitKey::ChunkValidationDurationMicros
            ),
            Err(LimitKeyClassificationError::NotScalarObservation)
        );
        assert_eq!(
            ReclaimableConcurrentKey::try_from_schema_key(
                WebRemoteLimitKey::RemoteInternalRetainedBytes
            ),
            Err(LimitKeyClassificationError::AggregateReservationRequired)
        );
    }

    #[test]
    fn every_complete_profile_constraint_has_a_violation_test() {
        let cases: &[(&[(WebRemoteLimitKey, u64)], LimitProfileConstraint)] = &[
            (
                &[(WebRemoteLimitKey::OpenBatchUrls, 129)],
                LimitProfileConstraint::BatchFitsTerminalRegistry,
            ),
            (
                &[(WebRemoteLimitKey::OpenSourceTerminalEntries, 513)],
                LimitProfileConstraint::TerminalRegistryFitsStatusRegistry,
            ),
            (
                &[(WebRemoteLimitKey::OpenSourceTerminalRetainedBytes, 8_191)],
                LimitProfileConstraint::TerminalRetainedBytesCoverTerminalEntries,
            ),
            (
                &[(WebRemoteLimitKey::DisarmedStrictSources, 400)],
                LimitProfileConstraint::StatusComponentsFitLiveOwners,
            ),
            (
                &[(WebRemoteLimitKey::OpenSourceStatusRetainedBytes, 32_767)],
                LimitProfileConstraint::StatusRetainedBytesCoverOwnerSlots,
            ),
            (
                &[(WebRemoteLimitKey::OpenSourceLiveStatusOwners, 385)],
                LimitProfileConstraint::PublishedLiveAndTerminalFitStatusSlots,
            ),
            (
                &[(WebRemoteLimitKey::DisarmedStrictSources, 65)],
                LimitProfileConstraint::FutureClaimsFitDisarmedClaims,
            ),
            (
                &[(WebRemoteLimitKey::DeferredEvictionVictims, 129)],
                LimitProfileConstraint::DeferredEvictionVictimsFitTerminalRegistry,
            ),
            (
                &[(WebRemoteLimitKey::DisarmedStatusClaims, 193)],
                LimitProfileConstraint::ReleasedLiveFutureAndTerminalFitStatusSlots,
            ),
            (
                &[(WebRemoteLimitKey::OpenSourceStatusRetainedBytes, 36_927)],
                LimitProfileConstraint::StatusRetainedBytesCoverFutureClaimsAndEviction,
            ),
            (
                &[(WebRemoteLimitKey::LifecycleOutstandingDeliveries, 1_631)],
                LimitProfileConstraint::LifecycleWorstCaseRustDeliveryCount,
            ),
            (
                &[(WebRemoteLimitKey::LifecycleListenerErrorCredits, 815)],
                LimitProfileConstraint::LifecycleWorstCaseListenerErrorCredits,
            ),
            (
                &[(
                    WebRemoteLimitKey::LifecycleOutstandingDeliveryBytes,
                    104_447,
                )],
                LimitProfileConstraint::LifecycleWorstCaseRustDeliveryBytes,
            ),
            (
                &[(WebRemoteLimitKey::TypescriptDispatcherItems, 1_631)],
                LimitProfileConstraint::LifecycleWorstCaseTypescriptDeliveryCount,
            ),
            (
                &[(
                    WebRemoteLimitKey::TypescriptDispatcherRetainedBytes,
                    104_447,
                )],
                LimitProfileConstraint::LifecycleWorstCaseTypescriptDeliveryBytes,
            ),
            (
                &[(WebRemoteLimitKey::PromiseClosureBytes, 1_023)],
                LimitProfileConstraint::PromiseClosureBytesCoverOperations,
            ),
            (
                &[(WebRemoteLimitKey::WrapperCacheEntries, 79)],
                LimitProfileConstraint::WrapperCacheCoversOperationsAndRecordings,
            ),
            (
                &[(WebRemoteLimitKey::RemovedTombstoneRetainers, 63)],
                LimitProfileConstraint::RecordingHandlesFitTombstoneRetainers,
            ),
            (
                &[(WebRemoteLimitKey::UrlFingerprintTokensPerBucket, 65)],
                LimitProfileConstraint::FingerprintTokensPerBucketFitGlobal,
            ),
            (
                &[(WebRemoteLimitKey::RemoteGcCpuAllowanceMicrosPerFrame, 65)],
                LimitProfileConstraint::RemoteGcFitsSharedCpuAllowance,
            ),
            (
                &[(WebRemoteLimitKey::RemoteCpuWorkUnitsPerFrame, 2)],
                LimitProfileConstraint::RemoteCpuWorkFitsSharedFrameCap,
            ),
            (
                &[(WebRemoteLimitKey::RemoteGcWorkUnitsPerFrame, 2)],
                LimitProfileConstraint::RemoteGcWorkFitsSharedFrameCap,
            ),
            (
                &[(WebRemoteLimitKey::AccountingScopeNodeBytesGlobal, 4_095)],
                LimitProfileConstraint::GlobalScopeNodeBytesCoverNodes,
            ),
            (
                &[(WebRemoteLimitKey::AccountingChildScopeBytesPerParent, 4_095)],
                LimitProfileConstraint::ParentScopeNodeBytesCoverNodes,
            ),
            (
                &[
                    (WebRemoteLimitKey::OpenOperations, 1),
                    (WebRemoteLimitKey::PublicRecordingHandles, 2),
                    (WebRemoteLimitKey::OperationRecordingSubscriptions, 1),
                ],
                LimitProfileConstraint::OperationSubscriptionsCoverRecordingHandles,
            ),
            (
                &[(
                    WebRemoteLimitKey::RuntimeInternEntryAndCapacityBytes,
                    RUNTIME_INTERN_MINIMUM_ENTRY_AND_CAPACITY_BYTES_V1 - 1,
                )],
                LimitProfileConstraint::RuntimeInternMinimumSideMapFitsBudgets,
            ),
        ];
        let mut covered = BTreeSet::new();
        for (overrides, constraint) in cases {
            assert!(covered.insert(*constraint));
            assert_constraint_error(
                overrides,
                LimitProfileCompletionError::ConstraintViolation {
                    constraint: *constraint,
                },
            );
        }
        assert_eq!(covered, LimitProfileConstraint::ALL.into_iter().collect());
    }

    #[test]
    fn every_checked_formula_operator_reports_its_constraint_on_overflow() {
        for constraint in LimitProfileConstraint::ALL {
            assert_eq!(
                checked_add_constraint(u64::MAX, 1, constraint),
                Err(LimitProfileCompletionError::ArithmeticOverflow { constraint })
            );
            assert_eq!(
                checked_mul_constraint(u64::MAX, 2, constraint),
                Err(LimitProfileCompletionError::ArithmeticOverflow { constraint })
            );
            assert_eq!(
                checked_sum_constraint([u64::MAX, 1], constraint),
                Err(LimitProfileCompletionError::ArithmeticOverflow { constraint })
            );
        }
    }

    #[test]
    fn every_composed_constraint_formula_is_independently_overflow_checked() {
        let values = [nz(u64::MAX); WEB_REMOTE_LIMIT_COUNT];
        let arithmetic_constraints = [
            LimitProfileConstraint::TerminalRetainedBytesCoverTerminalEntries,
            LimitProfileConstraint::StatusComponentsFitLiveOwners,
            LimitProfileConstraint::StatusRetainedBytesCoverOwnerSlots,
            LimitProfileConstraint::PublishedLiveAndTerminalFitStatusSlots,
            LimitProfileConstraint::ReleasedLiveFutureAndTerminalFitStatusSlots,
            LimitProfileConstraint::StatusRetainedBytesCoverFutureClaimsAndEviction,
            LimitProfileConstraint::LifecycleWorstCaseRustDeliveryCount,
            LimitProfileConstraint::LifecycleWorstCaseListenerErrorCredits,
            LimitProfileConstraint::LifecycleWorstCaseRustDeliveryBytes,
            LimitProfileConstraint::LifecycleWorstCaseTypescriptDeliveryCount,
            LimitProfileConstraint::LifecycleWorstCaseTypescriptDeliveryBytes,
            LimitProfileConstraint::PromiseClosureBytesCoverOperations,
            LimitProfileConstraint::WrapperCacheCoversOperationsAndRecordings,
            LimitProfileConstraint::GlobalScopeNodeBytesCoverNodes,
            LimitProfileConstraint::ParentScopeNodeBytesCoverNodes,
        ];
        for constraint in arithmetic_constraints {
            assert_eq!(
                validate_profile_constraint(&values, constraint),
                Err(LimitProfileCompletionError::ArithmeticOverflow { constraint })
            );
        }
    }

    #[test]
    fn accounting_key_classification_covers_all_keys_and_seals_aggregate_families() {
        for key in WebRemoteLimitKey::ALL {
            if key.enforcement() == WebRemoteLimitEnforcement::TelemetryAcceptanceThreshold {
                assert!(TelemetryAcceptanceThresholdKey::try_from_schema_key(key).is_ok());
                assert_eq!(
                    ScalarObservationKey::try_from_schema_key(key),
                    Err(LimitKeyClassificationError::NotScalarObservation)
                );
                continue;
            }

            match key.definition().accounting {
                WebRemoteAccountingKind::FormulaConstant => {
                    assert!(FormulaConstantKey::try_from_schema_key(key).is_ok());
                }
                WebRemoteAccountingKind::ScalarObservation => {
                    assert!(ScalarObservationKey::try_from_schema_key(key).is_ok());
                }
                WebRemoteAccountingKind::ModuleLifetimeBurn => {
                    assert!(ModuleLifetimeBurnKey::try_from_schema_key(key).is_ok());
                }
                WebRemoteAccountingKind::ReclaimableConcurrent => {
                    let classification = ReclaimableConcurrentKey::try_from_schema_key(key);
                    if aggregate_reservation_family(key).is_some() {
                        assert_eq!(
                            classification,
                            Err(LimitKeyClassificationError::AggregateReservationRequired),
                            "aggregate key {} obtained a direct reservation capability",
                            key.definition().stable_name
                        );
                    } else {
                        assert!(classification.is_ok());
                    }
                }
            }
        }
    }

    #[test]
    fn utf16_and_utf8_limits_are_independent_intersection_checks() {
        let mut values = complete_test_values();
        values[WebRemoteLimitKey::TimelineUtf16CodeUnits.index()] = nz(u64::MAX);
        values[WebRemoteLimitKey::TimelineUtf8Bytes.index()] = nz(1);
        validate_complete_profile(&values).expect("there is no unsound UTF conversion formula");
    }

    #[test]
    fn typed_scope_parent_graph_matches_the_frozen_parent_matrix() {
        use WebRemoteLimitScope as S;

        const ALL_SCOPES: &[S] = &[
            S::WasmModuleLifetime,
            S::AccountingScopeNode,
            S::ViewerInstance,
            S::Source,
            S::Session,
            S::Request,
            S::AtomicBatch,
            S::Operation,
            S::Summary,
            S::NestedRecord,
            S::MessageIndexRegion,
            S::Chunk,
            S::DecoderGroupPerChunk,
            S::Partition,
            S::Generation,
            S::Window,
            S::RangeResponse,
            S::Frame,
            S::WorkUnit,
            S::IngressItem,
            S::Channel,
            S::UrlFingerprintBucket,
            S::PageExecution,
            S::Store,
        ];
        const LEGAL_EDGES: &[(S, S)] = &[
            (S::WasmModuleLifetime, S::ViewerInstance),
            (S::ViewerInstance, S::Source),
            (S::ViewerInstance, S::Request),
            (S::ViewerInstance, S::AtomicBatch),
            (S::ViewerInstance, S::Frame),
            (S::ViewerInstance, S::IngressItem),
            (S::ViewerInstance, S::Channel),
            (S::ViewerInstance, S::UrlFingerprintBucket),
            (S::ViewerInstance, S::PageExecution),
            (S::ViewerInstance, S::Store),
            (S::Source, S::Session),
            (S::Source, S::Operation),
            (S::Source, S::Summary),
            (S::Summary, S::NestedRecord),
            (S::Session, S::Generation),
            (S::Session, S::Partition),
            (S::Session, S::Window),
            (S::Session, S::RangeResponse),
            (S::Session, S::WorkUnit),
            (S::Session, S::MessageIndexRegion),
            (S::Session, S::Store),
            (S::Generation, S::Chunk),
            (S::Generation, S::Partition),
            (S::Generation, S::WorkUnit),
            (S::Chunk, S::DecoderGroupPerChunk),
            (S::Chunk, S::NestedRecord),
            (S::Chunk, S::WorkUnit),
            (S::Chunk, S::Channel),
            (S::Request, S::RangeResponse),
            (S::Request, S::WorkUnit),
            (S::AtomicBatch, S::Operation),
            (S::Frame, S::WorkUnit),
            (S::IngressItem, S::WorkUnit),
            (S::PageExecution, S::WorkUnit),
            (S::RangeResponse, S::WorkUnit),
            (S::MessageIndexRegion, S::WorkUnit),
            (S::Partition, S::WorkUnit),
        ];

        for parent in ALL_SCOPES {
            for child in ALL_SCOPES {
                assert_eq!(
                    valid_child_scope(*parent, *child),
                    LEGAL_EDGES.contains(&(*parent, *child)),
                    "unexpected parent edge {parent:?} -> {child:?}"
                );
            }
        }
        assert_eq!(
            LEGAL_EDGES.iter().copied().collect::<BTreeSet<_>>().len(),
            LEGAL_EDGES.len()
        );
    }

    #[test]
    fn scope_node_flood_is_bounded_and_close_refunds_exact_credit() {
        let root = complete_test_profile().start_accounting_root().unwrap();
        let mut viewers = (0..64)
            .map(|_| root.create_viewer_scope().unwrap())
            .collect::<Vec<_>>();
        let saturated = root.snapshot();
        assert_eq!(saturated.0.global_node_usage.current_nodes, 64);
        assert_eq!(saturated.0.global_node_usage.current_bytes, 4_096);
        assert_eq!(
            root.create_viewer_scope().unwrap_err(),
            ScopeAccountingError::LimitExceeded
        );
        assert_eq!(root.snapshot(), saturated);

        drop(viewers.pop());
        assert_eq!(root.snapshot().0.global_node_usage.current_nodes, 63);
        let replacement = root.create_viewer_scope().unwrap();
        assert!(
            replacement.0.lease.identity != viewers[0].0.lease.identity,
            "scope generations are never reused"
        );
        drop(replacement);
        drop(viewers);
        let drained = root.snapshot();
        assert_eq!(drained.0.global_node_usage.current_nodes, 0);
        assert_eq!(drained.0.global_node_usage.current_bytes, 0);
        assert_eq!(drained.0.scopes.len(), 1);
        assert_eq!(
            drained.0.child_node_usage[&root.module_scope.lease.identity].current_nodes,
            0
        );
    }

    #[test]
    fn global_and_per_parent_scope_node_caps_fail_without_partial_ownership() {
        let parent_limited = test_profile_with(&[
            (WebRemoteLimitKey::AccountingChildScopeNodesPerParent, 2),
            (WebRemoteLimitKey::AccountingChildScopeBytesPerParent, 128),
        ])
        .start_accounting_root()
        .unwrap();
        let _viewer_a = parent_limited.create_viewer_scope().unwrap();
        let _viewer_b = parent_limited.create_viewer_scope().unwrap();
        let before_parent_failure = parent_limited.snapshot();
        assert_eq!(
            parent_limited.create_viewer_scope().unwrap_err(),
            ScopeAccountingError::LimitExceeded
        );
        assert_eq!(parent_limited.snapshot(), before_parent_failure);

        let globally_limited = test_profile_with(&[
            (WebRemoteLimitKey::AccountingScopeNodesGlobal, 3),
            (WebRemoteLimitKey::AccountingScopeNodeBytesGlobal, 192),
        ])
        .start_accounting_root()
        .unwrap();
        let viewer = globally_limited.create_viewer_scope().unwrap();
        let source = viewer.create_source_scope().unwrap();
        let _session = source.create_session_scope().unwrap();
        let before_global_failure = globally_limited.snapshot();
        assert_eq!(
            viewer.create_request_scope().unwrap_err(),
            ScopeAccountingError::LimitExceeded
        );
        assert_eq!(globally_limited.snapshot(), before_global_failure);
    }

    #[test]
    fn nested_scope_drop_closes_child_before_its_retained_parent_lease() {
        let root = complete_test_profile().start_accounting_root().unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let source = viewer.create_source_scope().unwrap();
        let session = source.create_session_scope().unwrap();
        drop((viewer, source));
        assert_eq!(root.snapshot().0.scopes.len(), 4);
        drop(session);
        let drained = root.snapshot();
        assert_eq!(drained.0.scopes.len(), 1);
        assert_eq!(drained.0.global_node_usage.current_nodes, 0);
        assert_eq!(drained.0.global_node_usage.current_bytes, 0);
    }

    #[test]
    fn decoder_bundle_charges_every_ancestor_tier_and_rolls_back_atomically() {
        let root = complete_test_profile().start_accounting_root().unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let source = viewer.create_source_scope().unwrap();
        let session = source.create_session_scope().unwrap();
        let generation = session.create_generation_scope().unwrap();
        let chunk = generation.create_chunk_scope().unwrap();
        let decoder = chunk.create_decoder_group_scope().unwrap();
        let active = decoder
            .prepare_decoder_reservation(
                &root,
                DecoderReservationSpec {
                    parser_working_bytes: 1,
                    builder_bytes: 2,
                    payload_scratch_bytes: 3,
                    lens_intermediate_bytes: 4,
                    terminal_output_bytes: 5,
                    derived_rows: 7,
                },
            )
            .unwrap()
            .commit()
            .unwrap();
        let snapshot = root.snapshot();
        for (scope, key, expected) in [
            (&decoder.0, WebRemoteLimitKey::DecoderWorkingBytes, 1),
            (&decoder.0, WebRemoteLimitKey::DecoderBuilderBytes, 2),
            (&decoder.0, WebRemoteLimitKey::DecoderPayloadScratchBytes, 3),
            (
                &decoder.0,
                WebRemoteLimitKey::DecoderLensIntermediateBytes,
                4,
            ),
            (&decoder.0, WebRemoteLimitKey::DecoderTerminalOutputBytes, 5),
            (&chunk.0, WebRemoteLimitKey::DecoderChunkWorkingBytes, 15),
            (
                &generation.0,
                WebRemoteLimitKey::DecoderGenerationWorkingBytes,
                15,
            ),
            (
                &session.0,
                WebRemoteLimitKey::DecoderSessionWorkingBytes,
                15,
            ),
            (&viewer.0, WebRemoteLimitKey::DecoderGlobalWorkingBytes, 15),
            (
                &session.0,
                WebRemoteLimitKey::RemoteInternalRetainedBytes,
                15,
            ),
            (&chunk.0, WebRemoteLimitKey::DecoderChunkOutputRows, 7),
            (
                &generation.0,
                WebRemoteLimitKey::DecoderGenerationOutputRows,
                7,
            ),
            (&session.0, WebRemoteLimitKey::DecoderSessionOutputRows, 7),
            (&viewer.0, WebRemoteLimitKey::DecoderGlobalOutputRows, 7),
        ] {
            assert_eq!(snapshot.usage(scope, key).unwrap().current, expected);
        }
        active.release().unwrap();
        assert!(
            root.snapshot()
                .0
                .usage
                .values()
                .all(|usage| usage.current == 0)
        );

        let limited = test_profile_with(&[(WebRemoteLimitKey::DecoderSessionWorkingBytes, 10)])
            .start_accounting_root()
            .unwrap();
        let viewer = limited.create_viewer_scope().unwrap();
        let source = viewer.create_source_scope().unwrap();
        let session = source.create_session_scope().unwrap();
        let generation = session.create_generation_scope().unwrap();
        let chunk = generation.create_chunk_scope().unwrap();
        let decoder = chunk.create_decoder_group_scope().unwrap();
        let before = limited.snapshot();
        assert_eq!(
            decoder
                .prepare_decoder_reservation(
                    &limited,
                    DecoderReservationSpec {
                        terminal_output_bytes: 11,
                        ..Default::default()
                    }
                )
                .unwrap_err(),
            ScopeAccountingError::LimitExceeded
        );
        assert_eq!(limited.snapshot(), before);
    }

    #[test]
    fn decoder_global_tier_is_shared_across_sources() {
        let root = complete_test_profile().start_accounting_root().unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let make_decoder = || {
            let source = viewer.create_source_scope().unwrap();
            let session = source.create_session_scope().unwrap();
            let generation = session.create_generation_scope().unwrap();
            let chunk = generation.create_chunk_scope().unwrap();
            chunk.create_decoder_group_scope().unwrap()
        };
        let decoder_a = make_decoder();
        let decoder_b = make_decoder();
        let active = decoder_a
            .prepare_decoder_reservation(
                &root,
                DecoderReservationSpec {
                    terminal_output_bytes: 40,
                    ..Default::default()
                },
            )
            .unwrap()
            .commit()
            .unwrap();
        let before = root.snapshot();
        assert_eq!(
            decoder_b
                .prepare_decoder_reservation(
                    &root,
                    DecoderReservationSpec {
                        terminal_output_bytes: 30,
                        ..Default::default()
                    },
                )
                .unwrap_err(),
            ScopeAccountingError::LimitExceeded
        );
        assert_eq!(root.snapshot(), before);
        drop(active);
    }

    #[test]
    fn decoder_scalar_row_cap_precedes_all_aggregate_charges() {
        let root = test_profile_with(&[(WebRemoteLimitKey::DecoderDerivedRows, 4)])
            .start_accounting_root()
            .unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let source = viewer.create_source_scope().unwrap();
        let session = source.create_session_scope().unwrap();
        let generation = session.create_generation_scope().unwrap();
        let chunk = generation.create_chunk_scope().unwrap();
        let decoder = chunk.create_decoder_group_scope().unwrap();
        let before = root.snapshot();

        assert_eq!(
            decoder
                .prepare_decoder_reservation(
                    &root,
                    DecoderReservationSpec {
                        derived_rows: 5,
                        ..Default::default()
                    },
                )
                .unwrap_err(),
            ScopeAccountingError::LimitExceeded
        );
        assert_eq!(root.snapshot(), before);

        let active = decoder
            .prepare_decoder_reservation(
                &root,
                DecoderReservationSpec {
                    derived_rows: 4,
                    ..Default::default()
                },
            )
            .unwrap()
            .commit()
            .unwrap();
        let snapshot = root.snapshot();
        for (scope, key) in [
            (&chunk.0, WebRemoteLimitKey::DecoderChunkOutputRows),
            (
                &generation.0,
                WebRemoteLimitKey::DecoderGenerationOutputRows,
            ),
            (&session.0, WebRemoteLimitKey::DecoderSessionOutputRows),
            (&viewer.0, WebRemoteLimitKey::DecoderGlobalOutputRows),
        ] {
            assert_eq!(snapshot.usage(scope, key).unwrap().current, 4);
        }
        active.release().unwrap();
    }

    #[test]
    fn remote_internal_aggregate_bundles_cannot_omit_session_total() {
        let root = complete_test_profile().start_accounting_root().unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let source = viewer.create_source_scope().unwrap();
        let session = source.create_session_scope().unwrap();
        let generation = session.create_generation_scope().unwrap();
        let chunk = generation.create_chunk_scope().unwrap();
        let validation = chunk
            .prepare_validation_plan_reservation(&root, nz(40))
            .unwrap()
            .commit()
            .unwrap();
        let before_total_failure = root.snapshot();
        assert_eq!(
            session
                .prepare_raw_cache_reservation(&root, nz(30))
                .unwrap_err(),
            ScopeAccountingError::LimitExceeded
        );
        assert_eq!(root.snapshot(), before_total_failure);
        drop(validation);
    }

    #[test]
    fn remote_validator_reservations_are_atomic_typed_and_exactly_released() {
        let root = test_profile_with(&[
            (WebRemoteLimitKey::RemoteInternalRetainedBytes, 1_024),
            (WebRemoteLimitKey::RemoteValidatorRetainedBytes, 512),
        ])
        .start_accounting_root()
        .unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let source = viewer.create_source_scope().unwrap();
        let session = source.create_session_scope().unwrap();
        let range = session.create_range_response_scope().unwrap();
        let work = range.create_work_unit_scope().unwrap();
        let other_range = session.create_range_response_scope().unwrap();
        let other_work = other_range.create_work_unit_scope().unwrap();
        let initial = root.snapshot();

        let prepared = work
            .prepare_remote_validator_ingress(&root, nz(1), nz(8))
            .unwrap();
        drop(prepared);
        assert_eq!(root.snapshot(), initial);

        let ingress = work
            .prepare_remote_validator_ingress(&root, nz(1), nz(8))
            .unwrap()
            .commit()
            .unwrap();
        let active_ingress = root.snapshot();
        assert_eq!(ingress.accounted_bytes(), nz(8));
        assert_eq!(
            active_ingress
                .usage(
                    &work.0,
                    WebRemoteLimitKey::RemoteValidatorIngressJsWasmOverlapBytes,
                )
                .unwrap()
                .current,
            16
        );
        assert_eq!(
            active_ingress
                .usage(
                    &work.0,
                    WebRemoteLimitKey::RemoteValidatorIngressScratchBytes,
                )
                .unwrap()
                .current,
            8
        );
        assert_eq!(
            active_ingress
                .usage(&session.0, WebRemoteLimitKey::RemoteInternalRetainedBytes)
                .unwrap()
                .current,
            24
        );

        let retained = work
            .prepare_remote_validator_retained(&root, nz(80))
            .unwrap()
            .commit()
            .unwrap();
        assert_eq!(retained.accounted_bytes(), nz(80));
        assert_eq!(
            root.snapshot()
                .usage(&session.0, WebRemoteLimitKey::RemoteValidatorRetainedBytes,)
                .unwrap()
                .current,
            80
        );

        let egress = work
            .prepare_remote_validator_egress(&root, nz(8))
            .unwrap()
            .commit()
            .unwrap();
        let all_active = root.snapshot();
        assert_eq!(egress.accounted_bytes(), nz(8));
        assert_eq!(
            all_active
                .usage(&range.0, WebRemoteLimitKey::RemoteValidatorEgressValues)
                .unwrap()
                .current,
            1
        );
        assert_eq!(
            all_active
                .usage(
                    &work.0,
                    WebRemoteLimitKey::RemoteValidatorEgressJsWasmOverlapBytes,
                )
                .unwrap()
                .current,
            16
        );
        assert_eq!(
            all_active
                .usage(
                    &work.0,
                    WebRemoteLimitKey::RemoteValidatorEgressScratchBytes,
                )
                .unwrap()
                .current,
            16
        );
        assert_eq!(
            all_active
                .usage(&session.0, WebRemoteLimitKey::RemoteInternalRetainedBytes)
                .unwrap()
                .current,
            136
        );

        let before_same_range_failure = root.snapshot();
        assert_eq!(
            work.prepare_remote_validator_egress(&root, nz(8))
                .unwrap_err(),
            ScopeAccountingError::LimitExceeded
        );
        assert_eq!(root.snapshot(), before_same_range_failure);

        let other_egress = other_work
            .prepare_remote_validator_egress(&root, nz(8))
            .unwrap()
            .commit()
            .unwrap();
        assert_eq!(
            root.snapshot()
                .usage(
                    &other_range.0,
                    WebRemoteLimitKey::RemoteValidatorEgressValues,
                )
                .unwrap()
                .current,
            1
        );
        assert_eq!(
            root.snapshot()
                .usage(&session.0, WebRemoteLimitKey::RemoteInternalRetainedBytes)
                .unwrap()
                .current,
            168
        );

        other_egress.release().unwrap();
        egress.release().unwrap();
        retained.release().unwrap();
        ingress.release().unwrap();
        let drained = root.snapshot();
        let mut expected_drained = initial;
        expected_drained.0.revision = drained.0.revision;
        expected_drained.0.next_reservation_sequence = drained.0.next_reservation_sequence;
        expected_drained.0.phase_a_byte_ledger.high_water_bytes =
            drained.0.phase_a_byte_ledger.high_water_bytes;
        expected_drained.0.phase_a_byte_ledger.overflowed =
            drained.0.phase_a_byte_ledger.overflowed;
        assert_eq!(drained, expected_drained);
    }

    #[test]
    fn range_body_pump_reservation_is_atomic_typed_and_exactly_released() {
        let root = complete_test_profile().start_accounting_root().unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let source = viewer.create_source_scope().unwrap();
        let session = source.create_session_scope().unwrap();
        let range = session.create_range_response_scope().unwrap();
        let work = range.create_work_unit_scope().unwrap();
        let initial = root.snapshot();
        let spec = RangeBodyPumpReservationSpec {
            output_bytes: nz(8),
            scratch_bytes: nz(4),
        };

        let prepared = range
            .prepare_body_pump_reservation(&root, &work, spec)
            .unwrap();
        drop(prepared);
        assert_eq!(root.snapshot(), initial);

        let active = range
            .prepare_body_pump_reservation(&root, &work, spec)
            .unwrap()
            .commit()
            .unwrap();
        assert_eq!(active.spec(), spec);
        let snapshot = root.snapshot();
        for (scope, key, expected) in [
            (&range.0, WebRemoteLimitKey::FetchWasmRawRetainedBytes, 8),
            (&range.0, WebRemoteLimitKey::ByobScratchBytes, 4),
            (&range.0, WebRemoteLimitKey::RangeJsWasmOverlapBytes, 12),
            (
                &session.0,
                WebRemoteLimitKey::RemoteInternalRetainedBytes,
                12,
            ),
        ] {
            assert_eq!(snapshot.usage(scope, key).unwrap().current, expected);
        }

        active.release().unwrap();
        let drained = root.snapshot();
        let mut expected_drained = initial;
        expected_drained.0.revision = drained.0.revision;
        expected_drained.0.next_reservation_sequence = drained.0.next_reservation_sequence;
        expected_drained.0.phase_a_byte_ledger.high_water_bytes =
            drained.0.phase_a_byte_ledger.high_water_bytes;
        expected_drained.0.phase_a_byte_ledger.overflowed =
            drained.0.phase_a_byte_ledger.overflowed;
        assert_eq!(drained, expected_drained);
    }

    #[test]
    fn range_body_pump_limit_and_ancestry_failures_are_exact_rollbacks() {
        let root = complete_test_profile().start_accounting_root().unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let source = viewer.create_source_scope().unwrap();
        let session = source.create_session_scope().unwrap();
        let range = session.create_range_response_scope().unwrap();
        let work = range.create_work_unit_scope().unwrap();
        let other_range = session.create_range_response_scope().unwrap();
        let other_work = other_range.create_work_unit_scope().unwrap();

        let before_limit = root.snapshot();
        assert!(matches!(
            range.prepare_body_pump_reservation(
                &root,
                &work,
                RangeBodyPumpReservationSpec {
                    output_bytes: nz(65),
                    scratch_bytes: nz(1),
                },
            ),
            Err(ScopeAccountingError::LimitExceeded)
        ));
        assert_eq!(root.snapshot(), before_limit);

        let before_ancestry = root.snapshot();
        assert_eq!(
            range
                .prepare_body_pump_reservation(
                    &root,
                    &other_work,
                    RangeBodyPumpReservationSpec {
                        output_bytes: nz(8),
                        scratch_bytes: nz(4),
                    },
                )
                .unwrap_err(),
            ScopeAccountingError::AggregateAncestryMismatch
        );
        assert_eq!(root.snapshot(), before_ancestry);
    }

    #[test]
    fn metadata_range_attempt_burn_is_atomic_irreversible_and_source_scoped() {
        let root = test_profile_with(&[
            (WebRemoteLimitKey::RangeRetryAttemptsPerOperation, 2),
            (WebRemoteLimitKey::MetadataOpeningRangeRequests, 3),
        ])
        .start_accounting_root()
        .unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let source = viewer.create_source_scope().unwrap();
        let operation = source.create_operation_scope().unwrap();
        let mut attempts = operation.range_retry_attempt_budget().unwrap();
        let (mut ranges, _deadline) = source.metadata_opening_range_and_deadline_owners().unwrap();

        let before_prepare = root.snapshot();
        {
            let _prepared = attempts
                .prepare_metadata_opening_attempt(&mut ranges)
                .unwrap();
        }
        assert_eq!(attempts.burned(), 0);
        assert_eq!(ranges.burned(), 0);
        assert_eq!(root.snapshot(), before_prepare);

        let first = attempts
            .prepare_metadata_opening_attempt(&mut ranges)
            .unwrap()
            .commit();
        assert_eq!(first.attempt().get(), 1);
        assert_eq!(first.metadata_opening_range(), 1);
        let second = attempts
            .prepare_metadata_opening_attempt(&mut ranges)
            .unwrap()
            .commit();
        assert_eq!(second.attempt().get(), 2);
        assert_eq!(second.metadata_opening_range(), 2);
        assert_eq!(attempts.burned(), 2);
        assert_eq!(ranges.burned(), 2);

        assert_eq!(
            attempts
                .prepare_metadata_opening_attempt(&mut ranges)
                .unwrap_err(),
            RangeAttemptBurnError::AttemptLimitExhausted
        );
        assert_eq!(attempts.burned(), 2);
        assert_eq!(ranges.burned(), 2);

        let other_source = viewer.create_source_scope().unwrap();
        let (mut other_ranges, _other_deadline) = other_source
            .metadata_opening_range_and_deadline_owners()
            .unwrap();
        assert_eq!(
            attempts
                .prepare_metadata_opening_attempt(&mut other_ranges)
                .unwrap_err(),
            RangeAttemptBurnError::AccountingUnavailable(
                ScopeAccountingError::AggregateAncestryMismatch
            )
        );
        assert_eq!(other_ranges.burned(), 0);
    }

    #[test]
    fn metadata_range_limit_failure_does_not_partially_burn_attempt() {
        let root = test_profile_with(&[
            (WebRemoteLimitKey::RangeRetryAttemptsPerOperation, 3),
            (WebRemoteLimitKey::MetadataOpeningRangeRequests, 1),
        ])
        .start_accounting_root()
        .unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let source = viewer.create_source_scope().unwrap();
        let first_operation = source.create_operation_scope().unwrap();
        let second_operation = source.create_operation_scope().unwrap();
        let mut first_attempts = first_operation.range_retry_attempt_budget().unwrap();
        let mut second_attempts = second_operation.range_retry_attempt_budget().unwrap();
        let (mut ranges, _deadline) = source.metadata_opening_range_and_deadline_owners().unwrap();

        first_attempts
            .prepare_metadata_opening_attempt(&mut ranges)
            .unwrap()
            .commit();
        assert_eq!(
            second_attempts
                .prepare_metadata_opening_attempt(&mut ranges)
                .unwrap_err(),
            RangeAttemptBurnError::MetadataOpeningRangeLimitExhausted
        );
        assert_eq!(second_attempts.burned(), 0);
        assert_eq!(ranges.burned(), 1);
    }

    #[test]
    fn retry_and_metadata_owners_are_projected_once_across_scope_clones() {
        let root = test_profile_with(&[]).start_accounting_root().unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let source = viewer.create_source_scope().unwrap();
        let source_clone = source.clone();
        let operation = source.create_operation_scope().unwrap();
        let operation_clone = operation.clone();

        let attempts = operation.range_retry_attempt_budget().unwrap();
        assert_eq!(
            operation_clone.range_retry_attempt_budget().unwrap_err(),
            ScopeAccountingError::ScopeBusy
        );
        drop(attempts);
        assert_eq!(
            operation.range_retry_attempt_budget().unwrap_err(),
            ScopeAccountingError::ScopeBusy
        );

        let owners = source.metadata_opening_range_and_deadline_owners().unwrap();
        assert_eq!(
            source_clone
                .metadata_opening_range_and_deadline_owners()
                .unwrap_err(),
            ScopeAccountingError::ScopeBusy
        );
        drop(owners);
        assert_eq!(
            source
                .metadata_opening_range_and_deadline_owners()
                .unwrap_err(),
            ScopeAccountingError::ScopeBusy
        );
    }

    #[test]
    fn metadata_active_visible_deadline_counts_only_explicit_visible_time() {
        let root =
            test_profile_with(&[(WebRemoteLimitKey::MetadataOpeningVisibleDeadlineMillis, 10)])
                .start_accounting_root()
                .unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let source = viewer.create_source_scope().unwrap();
        let (_ranges, mut deadline) = source.metadata_opening_range_and_deadline_owners().unwrap();

        assert_eq!(deadline.state(), MetadataOpeningDeadlineState::Active);
        assert_eq!(
            deadline.advance_visible(4).unwrap(),
            MetadataOpeningDeadlineState::Active
        );
        // Hidden wall time is represented by not calling `advance_visible`.
        assert_eq!(deadline.elapsed_millis(), 4);
        assert_eq!(
            deadline.advance_visible(6).unwrap(),
            MetadataOpeningDeadlineState::Expired
        );
        assert_eq!(deadline.elapsed_millis(), 10);
        assert_eq!(
            deadline.advance_visible(u64::MAX).unwrap_err(),
            ScopeAccountingError::ArithmeticOverflow
        );
        assert_eq!(deadline.elapsed_millis(), 10);
    }

    #[test]
    fn remote_validator_credit_failure_overflow_and_wrong_ancestry_are_rollbacks() {
        let limited = test_profile_with(&[
            (
                WebRemoteLimitKey::RemoteValidatorIngressJsWasmOverlapBytes,
                7,
            ),
            (WebRemoteLimitKey::RemoteInternalRetainedBytes, 1_024),
        ])
        .start_accounting_root()
        .unwrap();
        let viewer = limited.create_viewer_scope().unwrap();
        let source = viewer.create_source_scope().unwrap();
        let session = source.create_session_scope().unwrap();
        let work = session.create_work_unit_scope().unwrap();
        let before_limit = limited.snapshot();
        assert_eq!(
            work.prepare_remote_validator_ingress(&limited, nz(1), nz(4))
                .unwrap_err(),
            ScopeAccountingError::LimitExceeded
        );
        assert_eq!(limited.snapshot(), before_limit);

        let overflow = test_profile_with(&[
            (
                WebRemoteLimitKey::RemoteValidatorIngressWireBytes,
                u64::MAX - 1,
            ),
            (
                WebRemoteLimitKey::RemoteValidatorIngressJsWasmOverlapBytes,
                u64::MAX - 1,
            ),
            (
                WebRemoteLimitKey::RemoteValidatorIngressScratchBytes,
                u64::MAX - 1,
            ),
            (WebRemoteLimitKey::RemoteInternalRetainedBytes, u64::MAX - 1),
        ])
        .start_accounting_root()
        .unwrap();
        let viewer = overflow.create_viewer_scope().unwrap();
        let source = viewer.create_source_scope().unwrap();
        let session = source.create_session_scope().unwrap();
        let work = session.create_work_unit_scope().unwrap();
        let before_overflow = overflow.snapshot();
        assert_eq!(
            work.prepare_remote_validator_ingress(&overflow, nz(1), nz(u64::MAX / 2 + 1),)
                .unwrap_err(),
            ScopeAccountingError::ArithmeticOverflow
        );
        assert_eq!(overflow.snapshot(), before_overflow);

        let root = complete_test_profile().start_accounting_root().unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let frame = viewer.create_frame_scope().unwrap();
        let work = frame.create_work_unit_scope().unwrap();
        let before_ancestry = root.snapshot();
        assert_eq!(
            work.prepare_remote_validator_egress(&root, nz(4))
                .unwrap_err(),
            ScopeAccountingError::AggregateAncestryMismatch
        );
        assert_eq!(root.snapshot(), before_ancestry);
    }

    #[test]
    fn cpu_and_gc_compete_for_the_same_fixed_frame_work_credit() {
        let root = complete_test_profile().start_accounting_root().unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let frame = viewer.create_frame_scope().unwrap();
        let cpu = frame
            .prepare_remote_cpu_work(&root)
            .unwrap()
            .commit()
            .unwrap();
        let before = root.snapshot();
        assert_eq!(
            frame.prepare_remote_gc_work(&root).unwrap_err(),
            ScopeAccountingError::LimitExceeded
        );
        assert_eq!(root.snapshot(), before);
        cpu.release().unwrap();
        frame
            .prepare_remote_gc_work(&root)
            .unwrap()
            .commit()
            .unwrap()
            .release()
            .unwrap();
    }

    #[test]
    fn memory_headroom_formula_is_checked_as_one_root_owned_formula() {
        let root = complete_test_profile().start_accounting_root().unwrap();
        assert_eq!(root.validate_memory_headroom(164, 40, 60), Ok(0));
        assert_eq!(
            root.validate_memory_headroom(163, 40, 60),
            Err(MemoryHeadroomError::InsufficientHeadroom)
        );
        assert_eq!(
            root.validate_memory_headroom(u64::MAX, u64::MAX, 1),
            Err(MemoryHeadroomError::ArithmeticOverflow)
        );
    }

    #[test]
    fn scoped_prepare_abort_limit_failure_and_release_preserve_exact_ownership() {
        let root = complete_test_profile().start_accounting_root().unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let key = reclaimable(WebRemoteLimitKey::InFlightRangeBytes);
        let before = root.snapshot();

        let prepared = root
            .prepare_reservations([viewer.reservation_request(key, nz(40))])
            .expect("prepare reservation");
        assert_eq!(
            root.accounting_scalar_snapshot()
                .prepared_reservation_records,
            1
        );
        drop(prepared);
        assert_eq!(root.snapshot(), before);

        assert_eq!(
            root.prepare_reservations([viewer.reservation_request(key, nz(65))])
                .unwrap_err(),
            ScopeAccountingError::LimitExceeded
        );
        assert_eq!(root.snapshot(), before);

        let active = root
            .prepare_reservations([
                viewer.reservation_request(key, nz(32)),
                viewer.reservation_request(key, nz(8)),
            ])
            .expect("prepare combined reservation")
            .commit()
            .expect("commit reservation");
        assert_eq!(
            root.snapshot().usage(&viewer.0, key.schema_key()),
            Some(ScopedLimitUsage {
                current: 40,
                high_watermark: 40,
            })
        );
        active.release().expect("exact release");
        assert_eq!(root.snapshot().usage(&viewer.0, key.schema_key()), None);
    }

    #[test]
    fn one_viewer_shares_global_caps_while_sources_have_independent_caps() {
        let root = complete_test_profile().start_accounting_root().unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let source_a = viewer.create_source_scope().unwrap();
        let source_b = viewer.create_source_scope().unwrap();
        let global_key = reclaimable(WebRemoteLimitKey::InFlightRangeBytes);

        let global = root
            .prepare_reservations([viewer.reservation_request(global_key, nz(40))])
            .unwrap()
            .commit()
            .unwrap();
        let before_failed_global = root.snapshot();
        assert_eq!(
            root.prepare_reservations([viewer.reservation_request(global_key, nz(25))])
                .unwrap_err(),
            ScopeAccountingError::LimitExceeded
        );
        assert_eq!(root.snapshot(), before_failed_global);

        let source_a_reservation = source_a
            .prepare_semantic_config_reservation(&root, nz(60))
            .unwrap()
            .commit()
            .unwrap();
        let source_b_reservation = source_b
            .prepare_semantic_config_reservation(&root, nz(60))
            .unwrap()
            .commit()
            .unwrap();
        assert_eq!(
            root.snapshot()
                .usage(&source_a.0, WebRemoteLimitKey::SemanticConfigRetainedBytes),
            Some(ScopedLimitUsage {
                current: 60,
                high_watermark: 60,
            })
        );
        assert_eq!(
            root.snapshot()
                .usage(&source_b.0, WebRemoteLimitKey::SemanticConfigRetainedBytes),
            Some(ScopedLimitUsage {
                current: 60,
                high_watermark: 60,
            })
        );

        drop((global, source_a_reservation, source_b_reservation));
    }

    #[test]
    fn scope_clone_shares_identity_and_does_not_copy_budget() {
        let root = complete_test_profile().start_accounting_root().unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let alias = viewer.clone();
        let key = reclaimable(WebRemoteLimitKey::InFlightRangeBytes);
        let active = root
            .prepare_reservations([viewer.reservation_request(key, nz(40))])
            .unwrap()
            .commit()
            .unwrap();
        let before = root.snapshot();
        assert_eq!(
            root.prepare_reservations([alias.reservation_request(key, nz(25))])
                .unwrap_err(),
            ScopeAccountingError::LimitExceeded
        );
        assert_eq!(root.snapshot(), before);
        drop(active);
    }

    #[test]
    fn cross_root_wrong_scope_and_stale_scope_requests_are_exact_rollbacks() {
        let root_a = complete_test_profile().start_accounting_root().unwrap();
        let root_b = complete_test_profile().start_accounting_root().unwrap();
        let viewer_a = root_a.create_viewer_scope().unwrap();
        let viewer_b = root_b.create_viewer_scope().unwrap();
        let source_a = viewer_a.create_source_scope().unwrap();
        let global_key = reclaimable(WebRemoteLimitKey::InFlightRangeBytes);

        let before_a = root_a.snapshot();
        let before_b = root_b.snapshot();
        assert_eq!(
            root_a
                .prepare_reservations([viewer_b.reservation_request(global_key, nz(1))])
                .unwrap_err(),
            ScopeAccountingError::RootMismatch
        );
        assert_eq!(root_a.snapshot(), before_a);
        assert_eq!(root_b.snapshot(), before_b);

        assert!(matches!(
            root_a.prepare_reservations([source_a.reservation_request(global_key, nz(1))]),
            Err(ScopeAccountingError::ScopeKindMismatch { .. })
        ));
        assert_eq!(root_a.snapshot(), before_a);

        let stale = viewer_a.clone();
        drop(source_a);
        viewer_a.close().expect("close active viewer generation");
        let before_stale = root_a.snapshot();
        assert_eq!(before_stale.0.scopes.len(), 1);
        assert_eq!(
            stale.create_source_scope().unwrap_err(),
            ScopeAccountingError::StaleScope
        );
        assert_eq!(root_a.snapshot(), before_stale);
    }

    #[test]
    fn busy_scope_and_stale_prepared_commit_are_exact_rollbacks() {
        let root = complete_test_profile().start_accounting_root().unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let key = reclaimable(WebRemoteLimitKey::InFlightRangeBytes);
        let prepared = root
            .prepare_reservations([viewer.reservation_request(key, nz(1))])
            .unwrap();
        let prepared_credit = prepared.accounting_credit.unwrap();
        let source = viewer.create_source_scope().unwrap();
        let before_stale_commit = root.snapshot();
        assert_eq!(
            prepared.commit().unwrap_err(),
            ScopeAccountingError::RevisionMismatch
        );
        let mut expected_after_abort = before_stale_commit.clone();
        expected_after_abort.0.accounting_self.prepared_records -= 1;
        expected_after_abort.0.accounting_self.request_entries -= prepared_credit.request_entries;
        expected_after_abort.0.accounting_self.prepared_bytes -= prepared_credit.prepared_bytes;
        expected_after_abort.0.accounting_self.request_bytes -= prepared_credit.request_bytes;
        expected_after_abort.0.accounting_self.usage_nodes -= prepared_credit.usage_nodes;
        expected_after_abort.0.accounting_self.usage_node_bytes -= prepared_credit.usage_node_bytes;
        expected_after_abort
            .0
            .accounting_self
            .release_scratch_entries -= prepared_credit.release_scratch_entries;
        expected_after_abort.0.accounting_self.release_scratch_bytes -=
            prepared_credit.release_scratch_bytes;
        assert_eq!(root.snapshot(), expected_after_abort);
        assert_eq!(viewer.close(), Err(ScopeAccountingError::ScopeBusy));
        drop(source);

        let active = root
            .prepare_reservations([viewer.reservation_request(key, nz(1))])
            .unwrap()
            .commit()
            .unwrap();
        let before_busy = root.snapshot();
        assert_eq!(viewer.close(), Err(ScopeAccountingError::ScopeBusy));
        assert_eq!(root.snapshot(), before_busy);
        drop(active);
        viewer.close().expect("released viewer can close");
    }

    #[test]
    fn module_lifetime_burn_is_shared_across_viewers_and_never_refunded() {
        let root = complete_test_profile().start_accounting_root().unwrap();
        let viewer_a = root.create_viewer_scope().unwrap();
        let viewer_b = root.create_viewer_scope().unwrap();
        let key = burn(WebRemoteLimitKey::RuntimeInternStringBytes);
        root.burn_module_lifetime(key, nz(40)).unwrap();
        drop((viewer_a, viewer_b));
        let before_failure = root.snapshot();
        assert_eq!(
            root.burn_module_lifetime(key, nz(25)),
            Err(ScopeAccountingError::LimitExceeded)
        );
        assert_eq!(root.snapshot(), before_failure);
        assert_eq!(
            root.snapshot().usage(&root.module_scope, key.schema_key()),
            Some(ScopedLimitUsage {
                current: 40,
                high_watermark: 40,
            })
        );
    }

    #[test]
    fn forged_release_totals_and_cross_scope_release_are_exact_rollbacks() {
        let root = complete_test_profile().start_accounting_root().unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let other_viewer = root.create_viewer_scope().unwrap();
        let key = reclaimable(WebRemoteLimitKey::InFlightRangeBytes);
        let mut active = root
            .prepare_reservations([viewer.reservation_request(key, nz(8))])
            .unwrap()
            .commit()
            .unwrap();

        let before = root.snapshot();
        let mut reused = root
            .prepare_reservations([viewer.reservation_request(key, nz(1))])
            .unwrap();
        reused.reservation_identity = active.reservation_identity;
        assert_eq!(
            reused.commit().unwrap_err(),
            ScopeAccountingError::ReservationIdentityReused
        );
        assert_eq!(root.snapshot(), before);

        let mut wrong_amount = active.totals.clone();
        wrong_amount.values_mut().next().unwrap().clone_from(&9);
        let active_root = Rc::clone(&active.root);
        assert_eq!(
            release_reservation_inner(
                &active_root,
                active.reservation_identity,
                &wrong_amount,
                active.accounting_credit,
                &mut active.release_scratch,
            ),
            Err(ScopeAccountingError::ReservationIdentityMismatch)
        );
        assert_eq!(root.snapshot(), before);

        let cross_scope = BTreeMap::from([(
            ScopedUsageKey {
                scope: other_viewer.0.lease.identity,
                key: key.schema_key(),
            },
            8,
        )]);
        assert_eq!(
            release_reservation_inner(
                &active_root,
                active.reservation_identity,
                &cross_scope,
                active.accounting_credit,
                &mut active.release_scratch,
            ),
            Err(ScopeAccountingError::ReservationIdentityMismatch)
        );
        assert_eq!(root.snapshot(), before);
        let released_identity = active.reservation_identity;
        let released_totals = active.totals.clone();
        let released_root = Rc::clone(&active.root);
        let released_credit = active.accounting_credit;
        let mut released_scratch = Vec::new();
        active
            .release()
            .expect("original ownership remains releasable");
        let after_release = root.snapshot();
        assert_eq!(
            release_reservation_inner(
                &released_root,
                released_identity,
                &released_totals,
                released_credit,
                &mut released_scratch,
            ),
            Err(ScopeAccountingError::ReservationAlreadyReleased)
        );
        assert_eq!(root.snapshot(), after_release);
    }

    #[test]
    fn accounting_self_credit_bounds_no_yield_prepared_flood_and_refunds_exactly() {
        let root = test_profile_with(&[
            (WebRemoteLimitKey::AccountingPreparedReservationRecords, 2),
            (
                WebRemoteLimitKey::AccountingPreparedReservationRetainedBytes,
                128,
            ),
            (WebRemoteLimitKey::AccountingReservationRequestEntries, 2),
            (
                WebRemoteLimitKey::AccountingReservationRequestRetainedBytes,
                128,
            ),
        ])
        .start_accounting_root()
        .unwrap();
        let before = root.snapshot();
        let first = root.begin_reservation_plan(NonZeroU64::MIN).unwrap();
        let second = root.begin_reservation_plan(NonZeroU64::MIN).unwrap();
        assert_eq!(
            root.accounting_scalar_snapshot(),
            AccountingScalarSnapshot {
                prepared_reservation_records: 2,
                reservation_request_entries: 2,
                prepared_retained_bytes: 128,
                request_retained_bytes: 128,
                release_scratch_entries: 2,
                release_scratch_retained_bytes: 128,
                ..Default::default()
            }
        );
        assert_eq!(
            root.begin_reservation_plan(NonZeroU64::MIN).unwrap_err(),
            ScopeAccountingError::LimitExceeded
        );
        drop(first);
        let replacement = root.begin_reservation_plan(NonZeroU64::MIN).unwrap();
        drop((replacement, second));
        assert_eq!(root.snapshot(), before);
    }

    #[test]
    fn release_scratch_transfers_usage_claims_without_allocation_or_partial_mutation() {
        let root = complete_test_profile().start_accounting_root().unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let key = reclaimable(WebRemoteLimitKey::InFlightRangeBytes);
        let first = root
            .prepare_reservations([viewer.reservation_request(key, nz(1))])
            .unwrap()
            .commit()
            .unwrap();
        let mut successor = root
            .prepare_reservations([viewer.reservation_request(key, nz(1))])
            .unwrap()
            .commit()
            .unwrap();
        let before_missing_scratch = root.snapshot();
        let mut missing_scratch = Vec::new();

        assert_eq!(
            release_reservation_inner(
                &first.root,
                first.reservation_identity,
                &first.totals,
                first.accounting_credit,
                &mut missing_scratch,
            ),
            Err(ScopeAccountingError::ReleaseScratchInvariant)
        );
        assert_eq!(root.snapshot(), before_missing_scratch);

        first.release().unwrap();
        let transferred = root.snapshot();
        assert_eq!(transferred.0.accounting_self.usage_nodes, 1);
        let transferred_claims =
            &transferred.0.reservations[&successor.reservation_identity].usage_node_claims;
        assert_eq!(transferred_claims.len(), 1);
        assert!(
            transferred_claims[0]
                == ScopedUsageKey {
                    scope: viewer.0.lease.identity,
                    key: key.schema_key(),
                }
        );

        let retry_root = Rc::clone(&successor.root);
        let retry_identity = successor.reservation_identity;
        let retry_totals = successor.totals.clone();
        let retry_credit = successor.accounting_credit;
        successor.release_scratch.clear();
        successor.release_scratch.shrink_to_fit();
        let before_drop_retry = root.snapshot();
        // Any panic here fails the test; production uses `panic = "abort"`, so no unwind helper.
        drop(successor);
        assert_eq!(root.snapshot(), before_drop_retry);

        let mut retry_scratch = Vec::new();
        retry_scratch
            .try_reserve_exact(usize::try_from(retry_credit.release_scratch_entries).unwrap())
            .unwrap();
        release_reservation_inner(
            &retry_root,
            retry_identity,
            &retry_totals,
            retry_credit,
            &mut retry_scratch,
        )
        .unwrap();
        let drained = root.snapshot();
        assert_eq!(drained.0.accounting_self.usage_nodes, 0);
        assert_eq!(drained.0.accounting_self.release_scratch_entries, 0);
        assert_eq!(drained.0.accounting_self.release_scratch_bytes, 0);
    }

    #[test]
    fn accounting_self_credit_bounds_one_byte_active_splits() {
        let root = test_profile_with(&[
            (WebRemoteLimitKey::AccountingActiveReservationRecords, 2),
            (
                WebRemoteLimitKey::AccountingActiveReservationRetainedBytes,
                128,
            ),
        ])
        .start_accounting_root()
        .unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let key = reclaimable(WebRemoteLimitKey::InFlightRangeBytes);
        let first = root
            .prepare_reservations([viewer.reservation_request(key, nz(1))])
            .unwrap()
            .commit()
            .unwrap();
        let second = root
            .prepare_reservations([viewer.reservation_request(key, nz(1))])
            .unwrap()
            .commit()
            .unwrap();
        let before_failure = root.snapshot();
        assert_eq!(
            root.prepare_reservations([viewer.reservation_request(key, nz(1))])
                .unwrap()
                .commit()
                .unwrap_err(),
            ScopeAccountingError::LimitExceeded
        );
        assert_eq!(root.snapshot(), before_failure);
        drop((first, second));
        assert_eq!(
            root.accounting_scalar_snapshot(),
            AccountingScalarSnapshot {
                active_scope_nodes: 1,
                ..Default::default()
            }
        );
    }

    #[test]
    fn accounting_self_each_layer_failure_is_an_exact_rollback() {
        for (key, value) in [
            (WebRemoteLimitKey::AccountingPreparedReservationRecords, 1),
            (
                WebRemoteLimitKey::AccountingPreparedReservationRetainedBytes,
                63,
            ),
            (WebRemoteLimitKey::AccountingReservationRequestEntries, 1),
            (
                WebRemoteLimitKey::AccountingReservationRequestRetainedBytes,
                63,
            ),
        ] {
            let root = test_profile_with(&[(key, value)])
                .start_accounting_root()
                .unwrap();
            let before = root.snapshot();
            let result = if key == WebRemoteLimitKey::AccountingPreparedReservationRecords {
                let held = root.begin_reservation_plan(NonZeroU64::MIN).unwrap();
                let before_failure = root.snapshot();
                let result = root.begin_reservation_plan(NonZeroU64::MIN);
                assert_eq!(root.snapshot(), before_failure);
                drop(held);
                result
            } else if key == WebRemoteLimitKey::AccountingReservationRequestEntries {
                root.begin_reservation_plan(nz(2))
            } else {
                root.begin_reservation_plan(NonZeroU64::MIN)
            };
            assert_eq!(result.unwrap_err(), ScopeAccountingError::LimitExceeded);
            assert_eq!(root.snapshot(), before);
        }

        for (key, value) in [
            (WebRemoteLimitKey::AccountingUsageNodes, 1),
            (WebRemoteLimitKey::AccountingUsageNodeRetainedBytes, 63),
        ] {
            let root = test_profile_with(&[(key, value)])
                .start_accounting_root()
                .unwrap();
            let viewer = root.create_viewer_scope().unwrap();
            let before = root.snapshot();
            let mut plan = root.begin_reservation_plan(nz(2)).unwrap();
            plan.push(
                viewer
                    .reservation_request(reclaimable(WebRemoteLimitKey::InFlightRangeBytes), nz(1)),
            )
            .unwrap();
            if key == WebRemoteLimitKey::AccountingUsageNodes {
                plan.push(viewer.reservation_request(
                    reclaimable(WebRemoteLimitKey::MemoryPressurePendingRequests),
                    nz(1),
                ))
                .unwrap();
            }
            assert_eq!(
                plan.prepare().unwrap_err(),
                ScopeAccountingError::LimitExceeded
            );
            assert_eq!(root.snapshot(), before);
        }

        let root = test_profile_with(&[(
            WebRemoteLimitKey::AccountingActiveReservationRetainedBytes,
            63,
        )])
        .start_accounting_root()
        .unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let before = root.snapshot();
        assert_eq!(
            root.prepare_reservations([viewer
                .reservation_request(reclaimable(WebRemoteLimitKey::InFlightRangeBytes), nz(1),)])
                .unwrap()
                .commit()
                .unwrap_err(),
            ScopeAccountingError::LimitExceeded
        );
        assert_eq!(root.snapshot(), before);
    }

    #[test]
    fn lifecycle_alias_removal_and_dispose_refund_tombstones_incrementally() {
        let root = complete_test_profile().start_accounting_root().unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let source = viewer.create_source_scope().unwrap();
        let operation = source.create_operation_scope().unwrap();
        let owner = operation
            .prepare_lifecycle_reservation(
                &root,
                LifecycleReservationSpec {
                    source_owners: 1,
                    operations: 1,
                    ..Default::default()
                },
            )
            .unwrap()
            .commit()
            .unwrap();
        let mut aliases = Vec::new();
        aliases.try_reserve_exact(64).unwrap();
        for _ in 0..64 {
            aliases.push(
                operation
                    .prepare_lifecycle_reservation(
                        &root,
                        LifecycleReservationSpec {
                            recording_handles: 1,
                            recording_handle_bytes: 1,
                            operation_recording_subscriptions: 1,
                            operation_recording_subscription_bytes: 1,
                            wrapper_entries: 1,
                            wrapper_bytes: 1,
                            tombstone_retainers: 1,
                            tombstone_bytes: 1,
                            ..Default::default()
                        },
                    )
                    .unwrap()
                    .commit()
                    .unwrap(),
            );
        }
        for expected in (0..64).rev() {
            aliases.pop().unwrap().release().unwrap();
            let snapshot = root.snapshot();
            for key in [
                WebRemoteLimitKey::PublicRecordingHandles,
                WebRemoteLimitKey::WrapperCacheEntries,
                WebRemoteLimitKey::RemovedTombstoneRetainers,
            ] {
                assert_eq!(
                    snapshot.usage(&viewer.0, key).map(|usage| usage.current),
                    NonZeroU64::new(expected).map(NonZeroU64::get),
                );
            }
            assert_eq!(
                snapshot
                    .usage(
                        &operation.0,
                        WebRemoteLimitKey::OperationRecordingSubscriptions,
                    )
                    .map(|usage| usage.current),
                NonZeroU64::new(expected).map(NonZeroU64::get),
            );
        }
        owner.release().unwrap();
    }

    #[test]
    fn presentation_transitions_charge_exclusive_bytes_and_cleanup_waits_for_last_lease() {
        let root = test_profile_with(&[
            (WebRemoteLimitKey::PresentationActiveLeases, 4),
            (WebRemoteLimitKey::PresentationActiveLeaseRetainedBytes, 16),
        ])
        .start_accounting_root()
        .unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let source = viewer.create_source_scope().unwrap();
        let accounting_baseline = root.accounting_scalar_snapshot();
        let mut owner = source
            .activate_presentation(
                &root,
                PresentationActivationBytes {
                    facade_shell_bytes: nz(2),
                    owner_state_shell_bytes: nz(2),
                    current_snapshot_bytes: nz(3),
                },
            )
            .unwrap();
        let mut leases = (0..4)
            .map(|_| {
                owner
                    .acquire_lease(PresentationLeaseBytes {
                        lease_guard_shell_bytes: nz(1),
                        live_set_node_bytes: nz(1),
                        snapshot_copy_bytes: nz(2),
                    })
                    .unwrap()
            })
            .collect::<Vec<_>>();

        let snapshot = root.snapshot();
        for (key, expected) in [
            (WebRemoteLimitKey::PresentationFacades, 1),
            (WebRemoteLimitKey::PresentationFacadeRetainedBytes, 4),
            (WebRemoteLimitKey::PresentationSnapshots, 1),
            (WebRemoteLimitKey::PresentationSnapshotRetainedBytes, 3),
            (WebRemoteLimitKey::PresentationActiveLeases, 4),
            (WebRemoteLimitKey::PresentationActiveLeaseRetainedBytes, 16),
        ] {
            assert_eq!(snapshot.usage(&source.0, key).unwrap().current, expected);
        }

        let before_early_cleanup = root.snapshot();
        let cleanup_failure = owner.cleanup().unwrap_err();
        assert_eq!(cleanup_failure.error, PresentationTransitionError::Busy);
        owner = cleanup_failure.owner;
        assert_eq!(root.snapshot(), before_early_cleanup);

        let before_flood_failure = root.snapshot();
        assert_eq!(
            owner
                .acquire_lease(PresentationLeaseBytes {
                    lease_guard_shell_bytes: nz(1),
                    live_set_node_bytes: nz(1),
                    snapshot_copy_bytes: nz(2),
                })
                .unwrap_err(),
            PresentationTransitionError::Accounting(ScopeAccountingError::LimitExceeded)
        );
        assert_eq!(root.snapshot(), before_flood_failure);

        while leases.len() > 1 {
            leases.pop().unwrap().release(&owner).unwrap();
            let snapshot = root.snapshot();
            assert_eq!(
                snapshot
                    .usage(&source.0, WebRemoteLimitKey::PresentationFacades)
                    .unwrap()
                    .current,
                1
            );
            assert_eq!(
                snapshot
                    .usage(&source.0, WebRemoteLimitKey::PresentationSnapshots)
                    .unwrap()
                    .current,
                1
            );
            let before_cleanup = root.snapshot();
            let cleanup_failure = owner.cleanup().unwrap_err();
            assert_eq!(cleanup_failure.error, PresentationTransitionError::Busy);
            owner = cleanup_failure.owner;
            assert_eq!(root.snapshot(), before_cleanup);
        }
        leases.pop().unwrap().release(&owner).unwrap();
        owner.cleanup().unwrap();

        let drained = root.snapshot();
        for key in [
            WebRemoteLimitKey::PresentationFacades,
            WebRemoteLimitKey::PresentationFacadeRetainedBytes,
            WebRemoteLimitKey::PresentationSnapshots,
            WebRemoteLimitKey::PresentationSnapshotRetainedBytes,
            WebRemoteLimitKey::PresentationActiveLeases,
            WebRemoteLimitKey::PresentationActiveLeaseRetainedBytes,
        ] {
            assert_eq!(drained.usage(&source.0, key), None);
        }
        assert_eq!(root.accounting_scalar_snapshot(), accounting_baseline);
    }

    #[test]
    fn presentation_transition_failures_wrong_owner_stale_owner_and_duplicate_release_are_exact() {
        for (failed_key, bytes) in [
            (
                WebRemoteLimitKey::PresentationFacadeRetainedBytes,
                PresentationActivationBytes {
                    facade_shell_bytes: nz(2),
                    owner_state_shell_bytes: nz(1),
                    current_snapshot_bytes: nz(1),
                },
            ),
            (
                WebRemoteLimitKey::PresentationSnapshotRetainedBytes,
                PresentationActivationBytes {
                    facade_shell_bytes: nz(1),
                    owner_state_shell_bytes: nz(1),
                    current_snapshot_bytes: nz(2),
                },
            ),
        ] {
            let root = test_profile_with(&[(failed_key, 1)])
                .start_accounting_root()
                .unwrap();
            let viewer = root.create_viewer_scope().unwrap();
            let source = viewer.create_source_scope().unwrap();
            let before = root.snapshot();
            assert_eq!(
                source.activate_presentation(&root, bytes).unwrap_err(),
                PresentationTransitionError::Accounting(ScopeAccountingError::LimitExceeded),
                "failed key: {}",
                failed_key.definition().stable_name
            );
            assert_eq!(root.snapshot(), before);
        }

        let root = complete_test_profile().start_accounting_root().unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let source_a = viewer.create_source_scope().unwrap();
        let source_b = viewer.create_source_scope().unwrap();
        let bytes = PresentationActivationBytes {
            facade_shell_bytes: nz(1),
            owner_state_shell_bytes: nz(1),
            current_snapshot_bytes: nz(1),
        };
        let owner_a = source_a.activate_presentation(&root, bytes).unwrap();
        let owner_b = source_b.activate_presentation(&root, bytes).unwrap();

        let before_duplicate_activation = root.snapshot();
        assert_eq!(
            source_a.activate_presentation(&root, bytes).unwrap_err(),
            PresentationTransitionError::Accounting(ScopeAccountingError::LimitExceeded)
        );
        assert_eq!(root.snapshot(), before_duplicate_activation);

        let lease = owner_a
            .acquire_lease(PresentationLeaseBytes {
                lease_guard_shell_bytes: nz(1),
                live_set_node_bytes: nz(1),
                snapshot_copy_bytes: nz(1),
            })
            .unwrap();
        let before_wrong_owner = root.snapshot();
        let wrong_owner = lease.release(&owner_b).unwrap_err();
        assert_eq!(wrong_owner.error, PresentationTransitionError::WrongOwner);
        let mut lease = wrong_owner.lease;
        assert_eq!(root.snapshot(), before_wrong_owner);

        let saved_totals = lease.inner.reservation.totals.clone();
        lease.inner.reservation.totals.clear();
        let before_accounting_error = root.snapshot();
        let accounting_error = lease.release(&owner_a).unwrap_err();
        assert_eq!(
            accounting_error.error,
            PresentationTransitionError::Accounting(
                ScopeAccountingError::ReservationIdentityMismatch
            )
        );
        assert_eq!(root.snapshot(), before_accounting_error);
        let mut lease = accounting_error.lease;
        lease.inner.reservation.totals = saved_totals;
        lease.release(&owner_a).unwrap();

        let stale_lease = owner_b
            .acquire_lease(PresentationLeaseBytes {
                lease_guard_shell_bytes: nz(1),
                live_set_node_bytes: nz(1),
                snapshot_copy_bytes: nz(1),
            })
            .unwrap();
        let valid_sequence = stale_lease.inner.sequence;
        let mut stale_lease = stale_lease;
        stale_lease.inner.sequence = nz(u64::MAX);
        let before_stale_release = root.snapshot();
        let stale = stale_lease.release(&owner_b).unwrap_err();
        assert_eq!(stale.error, PresentationTransitionError::StaleLease);
        assert_eq!(root.snapshot(), before_stale_release);
        let mut stale_lease = stale.lease;
        stale_lease.inner.sequence = valid_sequence;
        stale_lease.release(&owner_b).unwrap();

        owner_a.cleanup().unwrap();

        let saved_totals = owner_b.state.borrow().activation.totals.clone();
        owner_b.state.borrow_mut().activation.totals.clear();
        let before_cleanup_error = root.snapshot();
        let cleanup_error = owner_b.cleanup().unwrap_err();
        assert_eq!(
            cleanup_error.error,
            PresentationTransitionError::Accounting(
                ScopeAccountingError::ReservationIdentityMismatch
            )
        );
        assert_eq!(root.snapshot(), before_cleanup_error);
        let owner_b = cleanup_error.owner;
        owner_b.state.borrow_mut().activation.totals = saved_totals;
        owner_b.cleanup().unwrap();
    }

    #[test]
    fn presentation_success_consumes_wrappers_and_drop_churn_refunds_every_dimension() {
        let root = complete_test_profile().start_accounting_root().unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let source = viewer.create_source_scope().unwrap();
        let accounting_baseline = root.accounting_scalar_snapshot();
        let activation_bytes = PresentationActivationBytes {
            facade_shell_bytes: nz(1),
            owner_state_shell_bytes: nz(1),
            current_snapshot_bytes: nz(1),
        };
        let lease_bytes = PresentationLeaseBytes {
            lease_guard_shell_bytes: nz(1),
            live_set_node_bytes: nz(1),
            snapshot_copy_bytes: nz(1),
        };
        let mut release_completions = Vec::<()>::new();
        let mut cleanup_completions = Vec::<()>::new();

        for _ in 0..128 {
            let owner = source
                .activate_presentation(&root, activation_bytes)
                .unwrap();
            let lease = owner.acquire_lease(lease_bytes).unwrap();
            lease.release(&owner).unwrap();
            release_completions.push(());
            owner.cleanup().unwrap();
            cleanup_completions.push(());
            let snapshot = root.snapshot();
            for key in [
                WebRemoteLimitKey::PresentationFacades,
                WebRemoteLimitKey::PresentationFacadeRetainedBytes,
                WebRemoteLimitKey::PresentationSnapshots,
                WebRemoteLimitKey::PresentationSnapshotRetainedBytes,
                WebRemoteLimitKey::PresentationActiveLeases,
                WebRemoteLimitKey::PresentationActiveLeaseRetainedBytes,
            ] {
                assert_eq!(snapshot.usage(&source.0, key), None);
            }
            assert_eq!(root.accounting_scalar_snapshot(), accounting_baseline);
        }
        assert_eq!(release_completions, vec![(); 128]);
        assert_eq!(cleanup_completions, vec![(); 128]);

        let owner = source
            .activate_presentation(&root, activation_bytes)
            .unwrap();
        let lease = owner.acquire_lease(lease_bytes).unwrap();
        drop(owner);
        let retained_by_lease = root.snapshot();
        assert_eq!(
            retained_by_lease
                .usage(&source.0, WebRemoteLimitKey::PresentationFacades)
                .unwrap()
                .current,
            1
        );
        assert_eq!(
            retained_by_lease
                .usage(&source.0, WebRemoteLimitKey::PresentationActiveLeases)
                .unwrap()
                .current,
            1
        );
        drop(lease);
        let dropped = root.snapshot();
        for key in [
            WebRemoteLimitKey::PresentationFacades,
            WebRemoteLimitKey::PresentationFacadeRetainedBytes,
            WebRemoteLimitKey::PresentationSnapshots,
            WebRemoteLimitKey::PresentationSnapshotRetainedBytes,
            WebRemoteLimitKey::PresentationActiveLeases,
            WebRemoteLimitKey::PresentationActiveLeaseRetainedBytes,
        ] {
            assert_eq!(dropped.usage(&source.0, key), None);
        }
        assert_eq!(root.accounting_scalar_snapshot(), accounting_baseline);
    }

    #[test]
    fn parked_response_and_body_reader_have_one_atomic_bounded_owner() {
        let root = test_profile_with(&[
            (WebRemoteLimitKey::PageParkedResponseOwners, 2),
            (WebRemoteLimitKey::PageParkedResponseOwnerRetainedBytes, 3),
        ])
        .start_accounting_root()
        .unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let page = viewer.create_page_execution_scope().unwrap();
        let accounting_baseline = root.accounting_scalar_snapshot();

        for spec in [
            PageParkedResponseOwnerReservationSpec {
                owners: 1,
                application_retained_bytes: 0,
            },
            PageParkedResponseOwnerReservationSpec {
                owners: 0,
                application_retained_bytes: 1,
            },
        ] {
            assert_eq!(
                page.prepare_parked_response_owner_reservation(&root, spec)
                    .unwrap_err(),
                ScopeAccountingError::FormulaViolation
            );
        }

        let before_prepared_drop = root.snapshot();
        let prepared = page
            .prepare_parked_response_owner_reservation(
                &root,
                PageParkedResponseOwnerReservationSpec {
                    owners: 1,
                    application_retained_bytes: 3,
                },
            )
            .unwrap();
        drop(prepared);
        assert_eq!(root.snapshot(), before_prepared_drop);

        let stopped = page
            .prepare_parked_response_owner_reservation(
                &root,
                PageParkedResponseOwnerReservationSpec {
                    owners: 1,
                    application_retained_bytes: 1,
                },
            )
            .unwrap()
            .commit()
            .unwrap();
        // Abort/stop owns one exact release of both application-side dimensions.
        stopped.release().unwrap();
        let stopped_snapshot = root.snapshot();
        for key in [
            WebRemoteLimitKey::PageParkedResponseOwners,
            WebRemoteLimitKey::PageParkedResponseOwnerRetainedBytes,
        ] {
            assert_eq!(stopped_snapshot.usage(&page.0, key), None);
        }
        assert_eq!(root.accounting_scalar_snapshot(), accounting_baseline);

        let parked = page
            .prepare_parked_response_owner_reservation(
                &root,
                PageParkedResponseOwnerReservationSpec {
                    owners: 1,
                    application_retained_bytes: 3,
                },
            )
            .unwrap()
            .commit()
            .unwrap();
        let snapshot = root.snapshot();
        assert_eq!(
            snapshot
                .usage(&page.0, WebRemoteLimitKey::PageParkedResponseOwners)
                .unwrap()
                .current,
            1
        );
        assert_eq!(
            snapshot
                .usage(
                    &page.0,
                    WebRemoteLimitKey::PageParkedResponseOwnerRetainedBytes,
                )
                .unwrap()
                .current,
            3
        );

        let before_bytes_failure = root.snapshot();
        assert_eq!(
            page.prepare_parked_response_owner_reservation(
                &root,
                PageParkedResponseOwnerReservationSpec {
                    owners: 1,
                    application_retained_bytes: 1,
                },
            )
            .unwrap_err(),
            ScopeAccountingError::LimitExceeded
        );
        assert_eq!(root.snapshot(), before_bytes_failure);

        // Resume owns the one tokenized release of both the response and its reader.
        parked.release().unwrap();
        let resumed = root.snapshot();
        assert_eq!(
            resumed.usage(&page.0, WebRemoteLimitKey::PageParkedResponseOwners),
            None
        );
        assert_eq!(
            resumed.usage(
                &page.0,
                WebRemoteLimitKey::PageParkedResponseOwnerRetainedBytes,
            ),
            None
        );
        assert_eq!(root.accounting_scalar_snapshot(), accounting_baseline);
    }

    #[test]
    fn page_execution_epoch_is_checked_protocol_identity_not_a_resource_cap() {
        assert_eq!(PageExecutionEpoch::INITIAL.get(), 0);
        let near_exhaustion = PageExecutionEpoch(u64::MAX - 1);
        let last = near_exhaustion.checked_next().unwrap();
        assert_eq!(last.get(), u64::MAX);
        assert_eq!(last.checked_next(), Err(PageExecutionEpochExhausted));
        assert_eq!(last.get(), u64::MAX, "exhaustion must not wrap identity");
    }

    #[test]
    fn remote_open_and_identifier_helpers_charge_complete_cross_scope_bundles() {
        let root = complete_test_profile().start_accounting_root().unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let request = viewer.create_request_scope().unwrap();
        let batch = viewer.create_atomic_batch_scope().unwrap();
        let source = viewer.create_source_scope().unwrap();
        let bucket = viewer.create_url_fingerprint_bucket_scope().unwrap();
        let store = viewer.create_store_scope().unwrap();

        let open = batch
            .prepare_open_admission_reservation(
                &root,
                &request,
                OpenAdmissionReservationSpec {
                    batch_urls: 1,
                    remote_session_slots: 1,
                    pending_secret_bytes: 1,
                    pending_options_bytes: 1,
                    extensionless_prepared: 1,
                    pending_sniffs: 1,
                    disarmed_sources: 1,
                    disarmed_retained_bytes: 1,
                    atomic_reservation_bytes: 1,
                    descriptors: 1,
                    operations: 1,
                },
            )
            .unwrap()
            .commit()
            .unwrap();
        let status = batch
            .prepare_open_status_reservation(
                &root,
                &source,
                OpenStatusReservationSpec {
                    owned_client_entries: 1,
                    status_slots: 1,
                    live_status_owners: 1,
                    terminal_entries: 1,
                    terminal_bytes: 1,
                    status_retained_bytes: 1,
                    disarmed_status_claims: 1,
                    terminal_status_bytes: 1,
                    terminal_diagnostics: 1,
                    deferred_eviction_victims: 1,
                    deferred_eviction_descriptor_bytes: 1,
                },
            )
            .unwrap()
            .commit()
            .unwrap();
        let url = bucket
            .prepare_url_index_reservation(
                &root,
                &batch,
                &source,
                UrlIndexReservationSpec {
                    bucket_tokens: 1,
                    canonical_match_bytes: 1,
                    alias_descriptors: 1,
                    alias_descriptor_bytes: 1,
                    index_retained_bytes: 1,
                    preexisting_recording_attachments: 1,
                    preexisting_recording_attachment_bytes: 1,
                    installation_acks: 1,
                    installation_ack_bytes: 1,
                    catalog_entries: 1,
                    catalog_projection_bytes: 1,
                    catalog_rows: 1,
                    catalog_string_bytes: 1,
                },
            )
            .unwrap()
            .commit()
            .unwrap();
        let semantic = source
            .prepare_semantic_config_reservation(&root, NonZeroU64::MIN)
            .unwrap()
            .commit()
            .unwrap();
        let existing = store
            .prepare_existing_identifier_index_reservation(
                &root,
                ExistingIdentifierIndexReservationSpec {
                    timelines: 1,
                    entity_paths: 1,
                    components: 1,
                    entity_path_key_bytes: 1,
                    node_bytes: 1,
                    viewer_bytes: 1,
                },
            )
            .unwrap()
            .commit()
            .unwrap();
        drop((existing, semantic, url, status, open));
    }

    #[test]
    fn remote_open_aggregate_cross_root_failure_is_an_exact_rollback() {
        let root_a = complete_test_profile().start_accounting_root().unwrap();
        let root_b = complete_test_profile().start_accounting_root().unwrap();
        let viewer_a = root_a.create_viewer_scope().unwrap();
        let viewer_b = root_b.create_viewer_scope().unwrap();
        let bucket = viewer_a.create_url_fingerprint_bucket_scope().unwrap();
        let source = viewer_a.create_source_scope().unwrap();
        let wrong_batch = viewer_b.create_atomic_batch_scope().unwrap();
        let before_a = root_a.snapshot();
        let before_b = root_b.snapshot();
        assert_eq!(
            bucket
                .prepare_url_index_reservation(
                    &root_a,
                    &wrong_batch,
                    &source,
                    UrlIndexReservationSpec {
                        bucket_tokens: 1,
                        ..Default::default()
                    },
                )
                .unwrap_err(),
            ScopeAccountingError::AggregateAncestryMismatch
        );
        assert_eq!(root_a.snapshot(), before_a);
        assert_eq!(root_b.snapshot(), before_b);
    }

    #[test]
    fn accounting_debug_and_errors_redact_internal_identities_and_values() {
        let limits = complete_test_profile();
        let raw_value = limits.raw_test_value(WebRemoteLimitKey::SummaryBytes).get();
        let debug = format!("{limits:?}");
        assert!(debug.contains("<sealed>"));
        assert!(!debug.contains(&raw_value.to_string()));

        let root = limits.start_accounting_root().unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let scope_debug = format!("{viewer:?}");
        assert!(scope_debug.contains("<opaque>"));
        assert!(!scope_debug.contains("root_nonce"));
        assert!(
            !ScopeAccountingError::RootMismatch
                .to_string()
                .contains("nonce")
        );
    }

    #[test]
    fn zero_evidence_digest_is_rejected() {
        assert_eq!(
            MeasurementEvidenceDigest::new([0; 32]),
            Err(MeasurementEvidenceError::ZeroDigest)
        );
    }
}
