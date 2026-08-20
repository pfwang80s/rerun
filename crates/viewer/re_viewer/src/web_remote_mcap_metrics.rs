//! Production-disarmed remote-only MCAP metrics and redacted diagnostics boundary.
//!
//! This module owns bounded, checked resource accounting and lifecycle telemetry for the remote
//! MCAP surface only. It does not import or construct a `Viewer`, `StoreHub`, `StoreBundle`,
//! `EntityDb`, `egui` panel, command sender, real transport, or real mutation/query path. It also
//! does not read `performance.memory`; heap availability is represented only as unavailable.

#![allow(dead_code)]

use std::fmt;

use crate::web_remote_mcap_failure::{
    RemoteMcapFailureClassificationV1, RemoteMcapFailurePhaseV1, RemoteMcapFailureRetryabilityV1,
    RemoteMcapFailureV1, RemoteTerminalCauseV1,
};

const MAX_DEBUG_ENTRY_LABEL_LEN: usize = 80;

/// Bytes that have been admitted through a remote-only accountable boundary.
///
/// Raw URL/query/ETag/Topic/Schema/EntityPath/StoreId/token/payload bytes are not representable
/// here; only an explicit numeric accounting value is carried.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct RemoteAccountableBytesV1(u64);

impl RemoteAccountableBytesV1 {
    pub(crate) const fn new_v1(value: u64) -> Self {
        Self(value)
    }

    pub(crate) const fn get_v1(self) -> u64 {
        self.0
    }
}

impl fmt::Display for RemoteAccountableBytesV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} accounted remote bytes", self.0)
    }
}

/// Low-cardinality, redacted source label.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum RemoteMetricSourceLabelV1 {
    SourceA,
    SourceB,
    SourceC,
    SourceD,
}

/// Opaque session index. No source URL, store id, generation, or token is retained.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct RemoteMetricSessionIndexV1(u16);

impl RemoteMetricSessionIndexV1 {
    pub(crate) const fn new_v1(value: u16) -> Self {
        Self(value)
    }

    pub(crate) const fn get_v1(self) -> u16 {
        self.0
    }
}

impl fmt::Display for RemoteMetricSessionIndexV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "remote session #{}", self.0)
    }
}

/// Ingress routes that may attempt to reach this metrics boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteMetricsRouteV1 {
    RemoteMcap,
    LogChannel,
    GrpcMessageProxy,
    Redap,
    Native,
    Local,
    Legacy,
    Global,
}

impl RemoteMetricsRouteV1 {
    pub(crate) const fn is_remote_mcap_v1(self) -> bool {
        matches!(self, Self::RemoteMcap)
    }
}

/// The only metric keys accepted by this registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteMetricKeyV1 {
    RemoteResource,
    RemoteWorkUnit,
    RemotePageSuspension,
    RemoteInternBurn,
    RemotePresentationGc,
    RemoteFailureDistribution,
    Global,
    LogChannel,
    GrpcMessageProxy,
    Redap,
    Native,
    Local,
    Legacy,
}

impl RemoteMetricKeyV1 {
    pub(crate) const fn is_remote_mcap_v1(self) -> bool {
        matches!(
            self,
            Self::RemoteResource
                | Self::RemoteWorkUnit
                | Self::RemotePageSuspension
                | Self::RemoteInternBurn
                | Self::RemotePresentationGc
                | Self::RemoteFailureDistribution
        )
    }
}

/// Resource classes owned by the remote MCAP surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum RemoteResourceClassV1 {
    Slot,
    Opening,
    Client,
    Source,
    Operation,
    RecordingWrapper,
    CatalogCard,
    PendingStrictSniff,
    TerminalStatus,
    DeliveryPermit,
}

impl RemoteResourceClassV1 {
    pub(crate) const COUNT: usize = 10;

    pub(crate) const ALL: [Self; Self::COUNT] = [
        Self::Slot,
        Self::Opening,
        Self::Client,
        Self::Source,
        Self::Operation,
        Self::RecordingWrapper,
        Self::CatalogCard,
        Self::PendingStrictSniff,
        Self::TerminalStatus,
        Self::DeliveryPermit,
    ];

    pub(crate) const fn index_v1(self) -> usize {
        match self {
            Self::Slot => 0,
            Self::Opening => 1,
            Self::Client => 2,
            Self::Source => 3,
            Self::Operation => 4,
            Self::RecordingWrapper => 5,
            Self::CatalogCard => 6,
            Self::PendingStrictSniff => 7,
            Self::TerminalStatus => 8,
            Self::DeliveryPermit => 9,
        }
    }

    pub(crate) fn try_from_index_v1(index: u8) -> Result<Self, RemoteResourceRegistryErrorV1> {
        Self::ALL
            .get(usize::from(index))
            .copied()
            .ok_or(RemoteResourceRegistryErrorV1::UnknownClass { index })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteResourceUsageV1 {
    current_count: u64,
    current_bytes: u64,
    high_water_count: u64,
    high_water_bytes: u64,
}

impl RemoteResourceUsageV1 {
    const ZERO: Self = Self {
        current_count: 0,
        current_bytes: 0,
        high_water_count: 0,
        high_water_bytes: 0,
    };

    pub(crate) const fn current_count_v1(self) -> u64 {
        self.current_count
    }

    pub(crate) const fn current_bytes_v1(self) -> u64 {
        self.current_bytes
    }

    pub(crate) const fn high_water_count_v1(self) -> u64 {
        self.high_water_count
    }

    pub(crate) const fn high_water_bytes_v1(self) -> u64 {
        self.high_water_bytes
    }
}

/// Hard caps applied by `RemoteResourceRegistryV1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteResourceLimitsV1 {
    max_total_count: u64,
    max_total_bytes: u64,
    max_class_count: u64,
    max_class_bytes: u64,
}

impl RemoteResourceLimitsV1 {
    pub(crate) const fn new_v1(
        max_total_count: u64,
        max_total_bytes: u64,
        max_class_count: u64,
        max_class_bytes: u64,
    ) -> Self {
        Self {
            max_total_count,
            max_total_bytes,
            max_class_count,
            max_class_bytes,
        }
    }
}

/// A successful checked resource acquisition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteResourceReservationV1 {
    class: RemoteResourceClassV1,
    bytes: RemoteAccountableBytesV1,
    revision: u64,
}

impl RemoteResourceReservationV1 {
    pub(crate) const fn class_v1(self) -> RemoteResourceClassV1 {
        self.class
    }

    pub(crate) const fn bytes_v1(self) -> RemoteAccountableBytesV1 {
        self.bytes
    }

    pub(crate) const fn revision_v1(self) -> u64 {
        self.revision
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteResourceRegistryErrorV1 {
    UnknownClass {
        index: u8,
    },
    CountOverflow,
    BytesOverflow,
    TotalCountOverflow,
    TotalBytesOverflow,
    RevisionOverflow,
    ClassCountLimitExceeded {
        class: RemoteResourceClassV1,
        current: u64,
        attempted: u64,
    },
    ClassBytesLimitExceeded {
        class: RemoteResourceClassV1,
        current: u64,
        attempted: u64,
    },
    TotalCountLimitExceeded {
        current: u64,
        attempted: u64,
    },
    TotalBytesLimitExceeded {
        current: u64,
        attempted: u64,
    },
    ReleaseCountUnderflow,
    ReleaseBytesUnderflow,
    ReleaseTotalCountUnderflow,
    ReleaseTotalBytesUnderflow,
}

/// Exact and comparable remote resource ownership snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteResourceSnapshotV1 {
    revision: u64,
    total_count: u64,
    total_bytes: u64,
    classes: [RemoteResourceUsageV1; RemoteResourceClassV1::COUNT],
    live_owner_classes: [bool; RemoteResourceClassV1::COUNT],
}

impl RemoteResourceSnapshotV1 {
    pub(crate) const fn revision_v1(&self) -> u64 {
        self.revision
    }

    pub(crate) const fn total_count_v1(&self) -> u64 {
        self.total_count
    }

    pub(crate) const fn total_bytes_v1(&self) -> u64 {
        self.total_bytes
    }

    pub(crate) fn usage_v1(&self, class: RemoteResourceClassV1) -> RemoteResourceUsageV1 {
        self.classes[class.index_v1()]
    }

    pub(crate) fn live_remote_owner_classes_v1(&self) -> Vec<RemoteResourceClassV1> {
        self.live_owner_classes
            .iter()
            .enumerate()
            .filter(|(_, is_live)| **is_live)
            .map(|(index, _)| RemoteResourceClassV1::ALL[index])
            .collect()
    }
}

pub(crate) struct RemoteResourceRegistryV1 {
    revision: u64,
    total_count: u64,
    total_bytes: u64,
    classes: [RemoteResourceUsageV1; RemoteResourceClassV1::COUNT],
}

impl RemoteResourceRegistryV1 {
    pub(crate) const fn new_disarmed_v1() -> Self {
        Self {
            revision: 0,
            total_count: 0,
            total_bytes: 0,
            classes: [RemoteResourceUsageV1::ZERO; RemoteResourceClassV1::COUNT],
        }
    }

    pub(crate) const fn revision_v1(&self) -> u64 {
        self.revision
    }

    pub(crate) fn usage_v1(&self, class: RemoteResourceClassV1) -> RemoteResourceUsageV1 {
        self.classes[class.index_v1()]
    }

    pub(crate) fn acquire_v1(
        &mut self,
        class: RemoteResourceClassV1,
        bytes: RemoteAccountableBytesV1,
        limits: RemoteResourceLimitsV1,
    ) -> Result<RemoteResourceReservationV1, RemoteResourceRegistryErrorV1> {
        self.acquire_by_index_v1(class.index_v1() as u8, class, bytes, limits)
    }

    pub(crate) fn acquire_by_index_v1(
        &mut self,
        index: u8,
        expected_class: RemoteResourceClassV1,
        bytes: RemoteAccountableBytesV1,
        limits: RemoteResourceLimitsV1,
    ) -> Result<RemoteResourceReservationV1, RemoteResourceRegistryErrorV1> {
        let class = RemoteResourceClassV1::try_from_index_v1(index)?;
        if class != expected_class {
            return Err(RemoteResourceRegistryErrorV1::UnknownClass { index });
        }

        let class_index = class.index_v1();
        let usage = self.classes[class_index];
        let next_count = usage
            .current_count
            .checked_add(1)
            .ok_or(RemoteResourceRegistryErrorV1::CountOverflow)?;
        let next_bytes = usage
            .current_bytes
            .checked_add(bytes.get_v1())
            .ok_or(RemoteResourceRegistryErrorV1::BytesOverflow)?;
        let next_total_count = self
            .total_count
            .checked_add(1)
            .ok_or(RemoteResourceRegistryErrorV1::TotalCountOverflow)?;
        let next_total_bytes = self
            .total_bytes
            .checked_add(bytes.get_v1())
            .ok_or(RemoteResourceRegistryErrorV1::TotalBytesOverflow)?;

        if next_count > limits.max_class_count {
            return Err(RemoteResourceRegistryErrorV1::ClassCountLimitExceeded {
                class,
                current: usage.current_count,
                attempted: next_count,
            });
        }
        if next_bytes > limits.max_class_bytes {
            return Err(RemoteResourceRegistryErrorV1::ClassBytesLimitExceeded {
                class,
                current: usage.current_bytes,
                attempted: next_bytes,
            });
        }
        if next_total_count > limits.max_total_count {
            return Err(RemoteResourceRegistryErrorV1::TotalCountLimitExceeded {
                current: self.total_count,
                attempted: next_total_count,
            });
        }
        if next_total_bytes > limits.max_total_bytes {
            return Err(RemoteResourceRegistryErrorV1::TotalBytesLimitExceeded {
                current: self.total_bytes,
                attempted: next_total_bytes,
            });
        }

        let next_revision = self
            .revision
            .checked_add(1)
            .ok_or(RemoteResourceRegistryErrorV1::RevisionOverflow)?;
        self.classes[class_index] = RemoteResourceUsageV1 {
            current_count: next_count,
            current_bytes: next_bytes,
            high_water_count: usage.high_water_count.max(next_count),
            high_water_bytes: usage.high_water_bytes.max(next_bytes),
        };
        self.total_count = next_total_count;
        self.total_bytes = next_total_bytes;
        self.revision = next_revision;

        Ok(RemoteResourceReservationV1 {
            class,
            bytes,
            revision: next_revision,
        })
    }

    pub(crate) fn release_reservation_v1(
        &mut self,
        reservation: RemoteResourceReservationV1,
    ) -> Result<(), RemoteResourceRegistryErrorV1> {
        let class_index = reservation.class.index_v1();
        let usage = self.classes[class_index];
        let next_count = usage
            .current_count
            .checked_sub(1)
            .ok_or(RemoteResourceRegistryErrorV1::ReleaseCountUnderflow)?;
        let next_bytes = usage
            .current_bytes
            .checked_sub(reservation.bytes.get_v1())
            .ok_or(RemoteResourceRegistryErrorV1::ReleaseBytesUnderflow)?;
        let next_total_count = self
            .total_count
            .checked_sub(1)
            .ok_or(RemoteResourceRegistryErrorV1::ReleaseTotalCountUnderflow)?;
        let next_total_bytes = self
            .total_bytes
            .checked_sub(reservation.bytes.get_v1())
            .ok_or(RemoteResourceRegistryErrorV1::ReleaseTotalBytesUnderflow)?;
        let next_revision = self
            .revision
            .checked_add(1)
            .ok_or(RemoteResourceRegistryErrorV1::RevisionOverflow)?;

        self.classes[class_index] = RemoteResourceUsageV1 {
            current_count: next_count,
            current_bytes: next_bytes,
            high_water_count: usage.high_water_count,
            high_water_bytes: usage.high_water_bytes,
        };
        self.total_count = next_total_count;
        self.total_bytes = next_total_bytes;
        self.revision = next_revision;
        Ok(())
    }

    pub(crate) fn snapshot_v1(&self) -> RemoteResourceSnapshotV1 {
        let live_owner_classes = self.classes.map(|usage| usage.current_count > 0);
        RemoteResourceSnapshotV1 {
            revision: self.revision,
            total_count: self.total_count,
            total_bytes: self.total_bytes,
            classes: self.classes,
            live_owner_classes,
        }
    }
}

/// Remote work-unit classes requiring duration and accounted-byte telemetry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum RemoteWorkUnitClassV1 {
    BodyPump,
    ExactBodyTransferCopy,
    OpeningParse,
    MessageIndexRegionParse,
    ChunkValidation,
    ChunkCount,
    DispatchDecode,
    InternCommit,
    AddChunk,
    RemoteGc,
}

impl RemoteWorkUnitClassV1 {
    pub(crate) const COUNT: usize = 10;

    pub(crate) const ALL: [Self; Self::COUNT] = [
        Self::BodyPump,
        Self::ExactBodyTransferCopy,
        Self::OpeningParse,
        Self::MessageIndexRegionParse,
        Self::ChunkValidation,
        Self::ChunkCount,
        Self::DispatchDecode,
        Self::InternCommit,
        Self::AddChunk,
        Self::RemoteGc,
    ];

    pub(crate) const fn index_v1(self) -> usize {
        match self {
            Self::BodyPump => 0,
            Self::ExactBodyTransferCopy => 1,
            Self::OpeningParse => 2,
            Self::MessageIndexRegionParse => 3,
            Self::ChunkValidation => 4,
            Self::ChunkCount => 5,
            Self::DispatchDecode => 6,
            Self::InternCommit => 7,
            Self::AddChunk => 8,
            Self::RemoteGc => 9,
        }
    }

    pub(crate) fn try_from_index_v1(index: u8) -> Result<Self, RemoteWorkUnitErrorV1> {
        Self::ALL
            .get(usize::from(index))
            .copied()
            .ok_or(RemoteWorkUnitErrorV1::UnknownClass { index })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteWorkUnitUsageV1 {
    completed_count: u64,
    input_bytes: u64,
    output_bytes: u64,
    max_duration_micros: u64,
    overflowed: bool,
}

impl RemoteWorkUnitUsageV1 {
    const ZERO: Self = Self {
        completed_count: 0,
        input_bytes: 0,
        output_bytes: 0,
        max_duration_micros: 0,
        overflowed: false,
    };

    pub(crate) const fn completed_count_v1(self) -> u64 {
        self.completed_count
    }

    pub(crate) const fn input_bytes_v1(self) -> u64 {
        self.input_bytes
    }

    pub(crate) const fn output_bytes_v1(self) -> u64 {
        self.output_bytes
    }

    pub(crate) const fn max_duration_micros_v1(self) -> u64 {
        self.max_duration_micros
    }

    pub(crate) const fn overflowed_v1(self) -> bool {
        self.overflowed
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteWorkUnitObservationV1 {
    pub(crate) overflowed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteWorkUnitErrorV1 {
    UnknownClass { index: u8 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteWorkUnitSnapshotV1 {
    units: [RemoteWorkUnitUsageV1; RemoteWorkUnitClassV1::COUNT],
}

impl RemoteWorkUnitSnapshotV1 {
    pub(crate) fn usage_v1(&self, class: RemoteWorkUnitClassV1) -> RemoteWorkUnitUsageV1 {
        self.units[class.index_v1()]
    }
}

pub(crate) struct RemoteWorkUnitRegistryV1 {
    units: [RemoteWorkUnitUsageV1; RemoteWorkUnitClassV1::COUNT],
}

impl RemoteWorkUnitRegistryV1 {
    pub(crate) const fn new_disarmed_v1() -> Self {
        Self {
            units: [RemoteWorkUnitUsageV1::ZERO; RemoteWorkUnitClassV1::COUNT],
        }
    }

    pub(crate) fn usage_v1(&self, class: RemoteWorkUnitClassV1) -> RemoteWorkUnitUsageV1 {
        self.units[class.index_v1()]
    }

    pub(crate) fn record_completion_v1(
        &mut self,
        class: RemoteWorkUnitClassV1,
        input_bytes: RemoteAccountableBytesV1,
        output_bytes: RemoteAccountableBytesV1,
        duration_micros: u64,
    ) -> RemoteWorkUnitObservationV1 {
        let index = class.index_v1();
        let usage = self.units[index];
        let mut overflowed = usage.overflowed;

        let completed_count = if let Some(value) = usage.completed_count.checked_add(1) {
            value
        } else {
            overflowed = true;
            u64::MAX
        };
        let input_bytes = if let Some(value) = usage.input_bytes.checked_add(input_bytes.get_v1()) {
            value
        } else {
            overflowed = true;
            u64::MAX
        };
        let output_bytes =
            if let Some(value) = usage.output_bytes.checked_add(output_bytes.get_v1()) {
                value
            } else {
                overflowed = true;
                u64::MAX
            };

        self.units[index] = RemoteWorkUnitUsageV1 {
            completed_count,
            input_bytes,
            output_bytes,
            max_duration_micros: usage.max_duration_micros.max(duration_micros),
            overflowed,
        };
        RemoteWorkUnitObservationV1 { overflowed }
    }

    pub(crate) fn record_completion_by_index_v1(
        &mut self,
        index: u8,
        input_bytes: RemoteAccountableBytesV1,
        output_bytes: RemoteAccountableBytesV1,
        duration_micros: u64,
    ) -> Result<RemoteWorkUnitObservationV1, RemoteWorkUnitErrorV1> {
        let class = RemoteWorkUnitClassV1::try_from_index_v1(index)?;
        Ok(self.record_completion_v1(class, input_bytes, output_bytes, duration_micros))
    }

    pub(crate) fn snapshot_v1(&self) -> RemoteWorkUnitSnapshotV1 {
        RemoteWorkUnitSnapshotV1 { units: self.units }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemotePageSuspensionKindV1 {
    NonForeground,
    PageHidden,
}

/// Opaque one-shot resume identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteResumeNonceV1(u64);

impl RemoteResumeNonceV1 {
    pub(crate) const fn new_v1(value: u64) -> Self {
        Self(value)
    }

    pub(crate) const fn get_v1(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemotePageSuspensionSnapshotV1 {
    suspended_for_nonforeground_count: u64,
    pagehidden_count: u64,
    resume_nonce: Option<RemoteResumeNonceV1>,
    discarded_dt_total_micros: u64,
    discarded_dt_count: u64,
    resume_quantum_total_micros: u64,
    resume_quantum_count: u64,
    resume_quantum_max_micros: u64,
    overflowed: bool,
}

impl RemotePageSuspensionSnapshotV1 {
    pub(crate) const fn suspended_for_nonforeground_count_v1(self) -> u64 {
        self.suspended_for_nonforeground_count
    }

    pub(crate) const fn pagehidden_count_v1(self) -> u64 {
        self.pagehidden_count
    }

    pub(crate) const fn resume_nonce_v1(self) -> Option<RemoteResumeNonceV1> {
        self.resume_nonce
    }

    pub(crate) const fn discarded_dt_total_micros_v1(self) -> u64 {
        self.discarded_dt_total_micros
    }

    pub(crate) const fn resume_quantum_total_micros_v1(self) -> u64 {
        self.resume_quantum_total_micros
    }
}

pub(crate) struct RemotePageSuspensionRegistryV1 {
    suspended_for_nonforeground_count: u64,
    pagehidden_count: u64,
    resume_nonce: Option<RemoteResumeNonceV1>,
    discarded_dt_total_micros: u64,
    discarded_dt_count: u64,
    resume_quantum_total_micros: u64,
    resume_quantum_count: u64,
    resume_quantum_max_micros: u64,
    overflowed: bool,
}

impl RemotePageSuspensionRegistryV1 {
    pub(crate) const fn new_disarmed_v1() -> Self {
        Self {
            suspended_for_nonforeground_count: 0,
            pagehidden_count: 0,
            resume_nonce: None,
            discarded_dt_total_micros: 0,
            discarded_dt_count: 0,
            resume_quantum_total_micros: 0,
            resume_quantum_count: 0,
            resume_quantum_max_micros: 0,
            overflowed: false,
        }
    }

    pub(crate) fn record_suspension_v1(
        &mut self,
        kind: RemotePageSuspensionKindV1,
        resume_nonce: Option<RemoteResumeNonceV1>,
    ) -> Result<(), RemotePageSuspensionErrorV1> {
        let count = match kind {
            RemotePageSuspensionKindV1::NonForeground => {
                self.suspended_for_nonforeground_count.checked_add(1)
            }
            RemotePageSuspensionKindV1::PageHidden => self.pagehidden_count.checked_add(1),
        }
        .ok_or(RemotePageSuspensionErrorV1::CountOverflow)?;
        match kind {
            RemotePageSuspensionKindV1::NonForeground => {
                self.suspended_for_nonforeground_count = count;
            }
            RemotePageSuspensionKindV1::PageHidden => {
                self.pagehidden_count = count;
            }
        }
        if resume_nonce.is_some() {
            self.resume_nonce = resume_nonce;
        }
        Ok(())
    }

    pub(crate) fn record_discarded_dt_v1(
        &mut self,
        duration_micros: u64,
    ) -> Result<(), RemotePageSuspensionErrorV1> {
        Self::record_checked_total_v1(
            &mut self.discarded_dt_total_micros,
            &mut self.discarded_dt_count,
            duration_micros,
        )
    }

    pub(crate) fn record_resume_quantum_v1(
        &mut self,
        duration_micros: u64,
    ) -> Result<(), RemotePageSuspensionErrorV1> {
        Self::record_checked_total_v1(
            &mut self.resume_quantum_total_micros,
            &mut self.resume_quantum_count,
            duration_micros,
        )?;
        self.resume_quantum_max_micros = self.resume_quantum_max_micros.max(duration_micros);
        Ok(())
    }

    pub(crate) fn snapshot_v1(&self) -> RemotePageSuspensionSnapshotV1 {
        RemotePageSuspensionSnapshotV1 {
            suspended_for_nonforeground_count: self.suspended_for_nonforeground_count,
            pagehidden_count: self.pagehidden_count,
            resume_nonce: self.resume_nonce,
            discarded_dt_total_micros: self.discarded_dt_total_micros,
            discarded_dt_count: self.discarded_dt_count,
            resume_quantum_total_micros: self.resume_quantum_total_micros,
            resume_quantum_count: self.resume_quantum_count,
            resume_quantum_max_micros: self.resume_quantum_max_micros,
            overflowed: self.overflowed,
        }
    }

    fn record_checked_total_v1(
        total: &mut u64,
        count: &mut u64,
        duration_micros: u64,
    ) -> Result<(), RemotePageSuspensionErrorV1> {
        *total = total
            .checked_add(duration_micros)
            .ok_or(RemotePageSuspensionErrorV1::ArithmeticOverflow)?;
        *count = count
            .checked_add(1)
            .ok_or(RemotePageSuspensionErrorV1::CountOverflow)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemotePageSuspensionErrorV1 {
    CountOverflow,
    ArithmeticOverflow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteInternCandidateReservationV1 {
    bytes: RemoteAccountableBytesV1,
}

impl RemoteInternCandidateReservationV1 {
    pub(crate) const fn bytes_v1(self) -> RemoteAccountableBytesV1 {
        self.bytes
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteInternBurnErrorV1 {
    ArithmeticOverflow,
    RevisionOverflow,
    ReservationUnderflow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteInternBurnSnapshotV1 {
    borrowed_legacy_hits: u64,
    remote_side_map_string_burn_bytes: u64,
    remote_side_map_entry_burn_bytes: u64,
    remote_side_map_capacity_burn_bytes: u64,
    remote_side_map_entries: u64,
    remote_side_map_capacity_bytes: u64,
    coordination_revision: u64,
    candidate_peak_bytes: u64,
    candidate_peak_high_water_bytes: u64,
    allocation_rejections: u64,
    capacity_rejections: u64,
    zero_partial_assertions: u64,
    viewer_restarts: u64,
}

impl RemoteInternBurnSnapshotV1 {
    pub(crate) const fn borrowed_legacy_hits_v1(self) -> u64 {
        self.borrowed_legacy_hits
    }

    pub(crate) const fn remote_side_map_string_burn_bytes_v1(self) -> u64 {
        self.remote_side_map_string_burn_bytes
    }

    pub(crate) const fn coordination_revision_v1(self) -> u64 {
        self.coordination_revision
    }

    pub(crate) const fn candidate_peak_bytes_v1(self) -> u64 {
        self.candidate_peak_bytes
    }

    pub(crate) const fn candidate_peak_high_water_bytes_v1(self) -> u64 {
        self.candidate_peak_high_water_bytes
    }
}

pub(crate) struct RemoteInternBurnV1 {
    borrowed_legacy_hits: u64,
    remote_side_map_string_burn_bytes: u64,
    remote_side_map_entry_burn_bytes: u64,
    remote_side_map_capacity_burn_bytes: u64,
    remote_side_map_entries: u64,
    remote_side_map_capacity_bytes: u64,
    coordination_revision: u64,
    candidate_peak_bytes: u64,
    candidate_peak_high_water_bytes: u64,
    allocation_rejections: u64,
    capacity_rejections: u64,
    zero_partial_assertions: u64,
    viewer_restarts: u64,
}

impl RemoteInternBurnV1 {
    pub(crate) const fn new_disarmed_v1() -> Self {
        Self {
            borrowed_legacy_hits: 0,
            remote_side_map_string_burn_bytes: 0,
            remote_side_map_entry_burn_bytes: 0,
            remote_side_map_capacity_burn_bytes: 0,
            remote_side_map_entries: 0,
            remote_side_map_capacity_bytes: 0,
            coordination_revision: 0,
            candidate_peak_bytes: 0,
            candidate_peak_high_water_bytes: 0,
            allocation_rejections: 0,
            capacity_rejections: 0,
            zero_partial_assertions: 0,
            viewer_restarts: 0,
        }
    }

    pub(crate) const fn borrowed_legacy_burn_delta_v1() -> u64 {
        0
    }

    pub(crate) fn record_borrowed_legacy_hit_v1(&mut self) -> Result<(), RemoteInternBurnErrorV1> {
        self.borrowed_legacy_hits = self
            .borrowed_legacy_hits
            .checked_add(1)
            .ok_or(RemoteInternBurnErrorV1::ArithmeticOverflow)?;
        Ok(())
    }

    pub(crate) fn record_legacy_coordination_revision_v1(
        &mut self,
    ) -> Result<(), RemoteInternBurnErrorV1> {
        self.coordination_revision = self
            .coordination_revision
            .checked_add(1)
            .ok_or(RemoteInternBurnErrorV1::RevisionOverflow)?;
        Ok(())
    }

    pub(crate) fn record_remote_side_map_commit_v1(
        &mut self,
        string_bytes: RemoteAccountableBytesV1,
        entry_count: u64,
        entry_bytes: RemoteAccountableBytesV1,
        capacity_bytes: RemoteAccountableBytesV1,
    ) -> Result<(), RemoteInternBurnErrorV1> {
        let next_string = self
            .remote_side_map_string_burn_bytes
            .checked_add(string_bytes.get_v1())
            .ok_or(RemoteInternBurnErrorV1::ArithmeticOverflow)?;
        let next_entry_bytes = self
            .remote_side_map_entry_burn_bytes
            .checked_add(entry_bytes.get_v1())
            .ok_or(RemoteInternBurnErrorV1::ArithmeticOverflow)?;
        let next_capacity_bytes = self
            .remote_side_map_capacity_burn_bytes
            .checked_add(capacity_bytes.get_v1())
            .ok_or(RemoteInternBurnErrorV1::ArithmeticOverflow)?;
        let next_entries = self
            .remote_side_map_entries
            .checked_add(entry_count)
            .ok_or(RemoteInternBurnErrorV1::ArithmeticOverflow)?;
        let next_capacity = self
            .remote_side_map_capacity_bytes
            .checked_add(capacity_bytes.get_v1())
            .ok_or(RemoteInternBurnErrorV1::ArithmeticOverflow)?;
        let next_revision = self
            .coordination_revision
            .checked_add(1)
            .ok_or(RemoteInternBurnErrorV1::RevisionOverflow)?;

        self.remote_side_map_string_burn_bytes = next_string;
        self.remote_side_map_entry_burn_bytes = next_entry_bytes;
        self.remote_side_map_capacity_burn_bytes = next_capacity_bytes;
        self.remote_side_map_entries = next_entries;
        self.remote_side_map_capacity_bytes = next_capacity;
        self.coordination_revision = next_revision;
        Ok(())
    }

    pub(crate) fn reserve_candidate_peak_v1(
        &mut self,
        bytes: RemoteAccountableBytesV1,
    ) -> Result<RemoteInternCandidateReservationV1, RemoteInternBurnErrorV1> {
        let next = self
            .candidate_peak_bytes
            .checked_add(bytes.get_v1())
            .ok_or(RemoteInternBurnErrorV1::ArithmeticOverflow)?;
        self.candidate_peak_bytes = next;
        self.candidate_peak_high_water_bytes = self.candidate_peak_high_water_bytes.max(next);
        Ok(RemoteInternCandidateReservationV1 { bytes })
    }

    pub(crate) fn release_candidate_peak_v1(
        &mut self,
        reservation: RemoteInternCandidateReservationV1,
    ) -> Result<(), RemoteInternBurnErrorV1> {
        self.candidate_peak_bytes = self
            .candidate_peak_bytes
            .checked_sub(reservation.bytes.get_v1())
            .ok_or(RemoteInternBurnErrorV1::ReservationUnderflow)?;
        Ok(())
    }

    pub(crate) fn record_allocation_rejection_v1(&mut self) -> Result<(), RemoteInternBurnErrorV1> {
        self.allocation_rejections = self
            .allocation_rejections
            .checked_add(1)
            .ok_or(RemoteInternBurnErrorV1::ArithmeticOverflow)?;
        Ok(())
    }

    pub(crate) fn record_capacity_rejection_v1(&mut self) -> Result<(), RemoteInternBurnErrorV1> {
        self.capacity_rejections = self
            .capacity_rejections
            .checked_add(1)
            .ok_or(RemoteInternBurnErrorV1::ArithmeticOverflow)?;
        Ok(())
    }

    pub(crate) fn record_zero_partial_assertion_v1(
        &mut self,
    ) -> Result<(), RemoteInternBurnErrorV1> {
        self.zero_partial_assertions = self
            .zero_partial_assertions
            .checked_add(1)
            .ok_or(RemoteInternBurnErrorV1::ArithmeticOverflow)?;
        Ok(())
    }

    pub(crate) fn record_viewer_restart_v1(&mut self) -> Result<(), RemoteInternBurnErrorV1> {
        self.viewer_restarts = self
            .viewer_restarts
            .checked_add(1)
            .ok_or(RemoteInternBurnErrorV1::ArithmeticOverflow)?;
        Ok(())
    }

    pub(crate) fn snapshot_v1(&self) -> RemoteInternBurnSnapshotV1 {
        RemoteInternBurnSnapshotV1 {
            borrowed_legacy_hits: self.borrowed_legacy_hits,
            remote_side_map_string_burn_bytes: self.remote_side_map_string_burn_bytes,
            remote_side_map_entry_burn_bytes: self.remote_side_map_entry_burn_bytes,
            remote_side_map_capacity_burn_bytes: self.remote_side_map_capacity_burn_bytes,
            remote_side_map_entries: self.remote_side_map_entries,
            remote_side_map_capacity_bytes: self.remote_side_map_capacity_bytes,
            coordination_revision: self.coordination_revision,
            candidate_peak_bytes: self.candidate_peak_bytes,
            candidate_peak_high_water_bytes: self.candidate_peak_high_water_bytes,
            allocation_rejections: self.allocation_rejections,
            capacity_rejections: self.capacity_rejections,
            zero_partial_assertions: self.zero_partial_assertions,
            viewer_restarts: self.viewer_restarts,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemotePresentationGcCounterV1 {
    Insertion,
    Gc,
    CleanupTurn,
    SafePoint,
    SuspendedForNonforeground,
    PageHiddenOwnership,
    PendingReclaim,
    CloseLatency,
}

impl RemotePresentationGcCounterV1 {
    pub(crate) const COUNT: usize = 8;

    pub(crate) const ALL: [Self; Self::COUNT] = [
        Self::Insertion,
        Self::Gc,
        Self::CleanupTurn,
        Self::SafePoint,
        Self::SuspendedForNonforeground,
        Self::PageHiddenOwnership,
        Self::PendingReclaim,
        Self::CloseLatency,
    ];

    pub(crate) const fn index_v1(self) -> usize {
        match self {
            Self::Insertion => 0,
            Self::Gc => 1,
            Self::CleanupTurn => 2,
            Self::SafePoint => 3,
            Self::SuspendedForNonforeground => 4,
            Self::PageHiddenOwnership => 5,
            Self::PendingReclaim => 6,
            Self::CloseLatency => 7,
        }
    }

    pub(crate) fn try_from_index_v1(index: u8) -> Result<Self, RemotePresentationGcErrorV1> {
        Self::ALL
            .get(usize::from(index))
            .copied()
            .ok_or(RemotePresentationGcErrorV1::UnknownCounter { index })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemotePresentationGcSnapshotV1 {
    counters: [u64; RemotePresentationGcCounterV1::COUNT],
    close_latency_total_micros: u64,
    close_latency_count: u64,
    close_latency_max_micros: u64,
    overflowed: bool,
}

impl RemotePresentationGcSnapshotV1 {
    pub(crate) const fn count_v1(self, counter: RemotePresentationGcCounterV1) -> u64 {
        self.counters[counter.index_v1()]
    }

    pub(crate) const fn close_latency_total_micros_v1(self) -> u64 {
        self.close_latency_total_micros
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemotePresentationGcErrorV1 {
    UnknownCounter { index: u8 },
    CountOverflow,
    LatencyTotalOverflow,
    DurationRequiredForCloseLatency,
}

pub(crate) struct RemotePresentationGcRegistryV1 {
    counters: [u64; RemotePresentationGcCounterV1::COUNT],
    close_latency_total_micros: u64,
    close_latency_count: u64,
    close_latency_max_micros: u64,
    overflowed: bool,
}

impl RemotePresentationGcRegistryV1 {
    pub(crate) const fn new_disarmed_v1() -> Self {
        Self {
            counters: [0; RemotePresentationGcCounterV1::COUNT],
            close_latency_total_micros: 0,
            close_latency_count: 0,
            close_latency_max_micros: 0,
            overflowed: false,
        }
    }

    pub(crate) fn record_count_v1(
        &mut self,
        counter: RemotePresentationGcCounterV1,
    ) -> Result<(), RemotePresentationGcErrorV1> {
        if counter == RemotePresentationGcCounterV1::CloseLatency {
            return Err(RemotePresentationGcErrorV1::DurationRequiredForCloseLatency);
        }
        let index = counter.index_v1();
        self.counters[index] = self.counters[index]
            .checked_add(1)
            .ok_or(RemotePresentationGcErrorV1::CountOverflow)?;
        Ok(())
    }

    pub(crate) fn record_close_latency_v1(
        &mut self,
        duration_micros: u64,
    ) -> Result<(), RemotePresentationGcErrorV1> {
        let index = RemotePresentationGcCounterV1::CloseLatency.index_v1();
        self.counters[index] = self.counters[index]
            .checked_add(1)
            .ok_or(RemotePresentationGcErrorV1::CountOverflow)?;
        self.close_latency_total_micros = self
            .close_latency_total_micros
            .checked_add(duration_micros)
            .ok_or(RemotePresentationGcErrorV1::LatencyTotalOverflow)?;
        self.close_latency_count = self
            .close_latency_count
            .checked_add(1)
            .ok_or(RemotePresentationGcErrorV1::CountOverflow)?;
        self.close_latency_max_micros = self.close_latency_max_micros.max(duration_micros);
        Ok(())
    }

    pub(crate) fn snapshot_v1(&self) -> RemotePresentationGcSnapshotV1 {
        RemotePresentationGcSnapshotV1 {
            counters: self.counters,
            close_latency_total_micros: self.close_latency_total_micros,
            close_latency_count: self.close_latency_count,
            close_latency_max_micros: self.close_latency_max_micros,
            overflowed: self.overflowed,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum RemoteFailureConsistencyCapabilityV1 {
    StrongValidator,
    DeploymentAssumed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteFailureKeyV1 {
    code: RemoteMcapFailureClassificationV1,
    phase: RemoteMcapFailurePhaseV1,
    retryability: RemoteMcapFailureRetryabilityV1,
    consistency_capability: RemoteFailureConsistencyCapabilityV1,
}

impl RemoteFailureKeyV1 {
    const fn new_v1(
        failure: RemoteMcapFailureV1,
        consistency_capability: RemoteFailureConsistencyCapabilityV1,
    ) -> Self {
        Self {
            code: failure.classification,
            phase: failure.phase,
            retryability: failure.retryability,
            consistency_capability,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteFailureDistributionEntryV1 {
    key: RemoteFailureKeyV1,
    count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteFailureDistributionSnapshotV1 {
    entries: Vec<RemoteFailureDistributionEntryV1>,
    terminal_first_cause: Option<RemoteTerminalCauseV1>,
}

impl RemoteFailureDistributionSnapshotV1 {
    pub(crate) fn count_v1(&self, key: RemoteFailureKeyV1) -> u64 {
        self.entries
            .iter()
            .find(|entry| entry.key == key)
            .map_or(0, |entry| entry.count)
    }

    pub(crate) const fn terminal_first_cause_v1(&self) -> Option<RemoteTerminalCauseV1> {
        self.terminal_first_cause
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteFailureDistributionErrorV1 {
    CountOverflow,
    TerminalCauseAlreadyRecorded,
}

pub(crate) struct RemoteFailureDistributionV1 {
    entries: Vec<RemoteFailureDistributionEntryV1>,
    terminal_first_cause: Option<RemoteTerminalCauseV1>,
}

impl RemoteFailureDistributionV1 {
    pub(crate) const fn new_disarmed_v1() -> Self {
        Self {
            entries: Vec::new(),
            terminal_first_cause: None,
        }
    }

    pub(crate) fn record_failure_v1(
        &mut self,
        failure: RemoteMcapFailureV1,
        consistency_capability: RemoteFailureConsistencyCapabilityV1,
    ) -> Result<(), RemoteFailureDistributionErrorV1> {
        let key = RemoteFailureKeyV1::new_v1(failure, consistency_capability);
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.key == key) {
            entry.count = entry
                .count
                .checked_add(1)
                .ok_or(RemoteFailureDistributionErrorV1::CountOverflow)?;
        } else {
            self.entries
                .push(RemoteFailureDistributionEntryV1 { key, count: 1 });
        }
        Ok(())
    }

    pub(crate) fn record_terminal_first_cause_v1(
        &mut self,
        cause: RemoteTerminalCauseV1,
    ) -> Result<(), RemoteFailureDistributionErrorV1> {
        if self.terminal_first_cause.is_some() {
            return Err(RemoteFailureDistributionErrorV1::TerminalCauseAlreadyRecorded);
        }
        self.terminal_first_cause = Some(cause);
        Ok(())
    }

    pub(crate) fn snapshot_v1(&self) -> RemoteFailureDistributionSnapshotV1 {
        RemoteFailureDistributionSnapshotV1 {
            entries: self.entries.clone(),
            terminal_first_cause: self.terminal_first_cause,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteMemoryAvailabilityV1 {
    Unavailable,
}

impl RemoteMemoryAvailabilityV1 {
    pub(crate) const fn precise_heap_bytes_v1(self) -> Option<RemoteAccountableBytesV1> {
        let _ = self;
        None
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteMetricsSnapshotV1 {
    resource: RemoteResourceSnapshotV1,
    work_units: RemoteWorkUnitSnapshotV1,
    page_suspension: RemotePageSuspensionSnapshotV1,
    intern_burn: RemoteInternBurnSnapshotV1,
    presentation_gc: RemotePresentationGcSnapshotV1,
    failure: RemoteFailureDistributionSnapshotV1,
    memory: RemoteMemoryAvailabilityV1,
}

impl RemoteMetricsSnapshotV1 {
    pub(crate) fn resource_v1(&self) -> &RemoteResourceSnapshotV1 {
        &self.resource
    }

    pub(crate) fn work_units_v1(&self) -> &RemoteWorkUnitSnapshotV1 {
        &self.work_units
    }

    pub(crate) fn intern_burn_v1(&self) -> &RemoteInternBurnSnapshotV1 {
        &self.intern_burn
    }

    pub(crate) fn presentation_gc_v1(&self) -> &RemotePresentationGcSnapshotV1 {
        &self.presentation_gc
    }

    pub(crate) fn failure_v1(&self) -> &RemoteFailureDistributionSnapshotV1 {
        &self.failure
    }

    pub(crate) const fn memory_v1(&self) -> RemoteMemoryAvailabilityV1 {
        self.memory
    }

    pub(crate) const fn page_suspension_v1(&self) -> &RemotePageSuspensionSnapshotV1 {
        &self.page_suspension
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteMetricsDebugCapV1 {
    max_entries: usize,
    max_bytes: usize,
}

impl RemoteMetricsDebugCapV1 {
    pub(crate) const fn new_v1(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            max_entries,
            max_bytes,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteCriticalDiagnosticV1 {
    LiveRemoteOwner { class: RemoteResourceClassV1 },
    TerminalCause(RemoteTerminalCauseV1),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteDiagnosticEntryV1 {
    label: &'static str,
    value: u64,
}

impl RemoteDiagnosticEntryV1 {
    const fn new_v1(label: &'static str, value: u64) -> Self {
        Self { label, value }
    }

    fn encoded_len_v1(self) -> usize {
        let digits = self.value.to_string().len();
        self.label.len().min(MAX_DEBUG_ENTRY_LABEL_LEN) + digits + 1
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteDebugSnapshotV1 {
    critical: Vec<RemoteCriticalDiagnosticV1>,
    best_effort: Vec<RemoteDiagnosticEntryV1>,
    truncated: bool,
}

impl RemoteDebugSnapshotV1 {
    pub(crate) fn critical_v1(&self) -> &[RemoteCriticalDiagnosticV1] {
        &self.critical
    }

    pub(crate) fn best_effort_v1(&self) -> &[RemoteDiagnosticEntryV1] {
        &self.best_effort
    }

    pub(crate) const fn truncated_v1(&self) -> bool {
        self.truncated
    }

    pub(crate) const fn label_v1(&self) -> &'static str {
        let _ = self;
        "remote MCAP redacted metrics debug snapshot"
    }
}

pub(crate) struct WebRemoteMcapMetricsV1 {
    resource_registry: RemoteResourceRegistryV1,
    work_unit_registry: RemoteWorkUnitRegistryV1,
    page_suspension: RemotePageSuspensionRegistryV1,
    intern_burn: RemoteInternBurnV1,
    presentation_gc: RemotePresentationGcRegistryV1,
    failure_distribution: RemoteFailureDistributionV1,
    memory: RemoteMemoryAvailabilityV1,
}

impl WebRemoteMcapMetricsV1 {
    pub(crate) const fn new_disarmed_v1() -> Self {
        Self {
            resource_registry: RemoteResourceRegistryV1::new_disarmed_v1(),
            work_unit_registry: RemoteWorkUnitRegistryV1::new_disarmed_v1(),
            page_suspension: RemotePageSuspensionRegistryV1::new_disarmed_v1(),
            intern_burn: RemoteInternBurnV1::new_disarmed_v1(),
            presentation_gc: RemotePresentationGcRegistryV1::new_disarmed_v1(),
            failure_distribution: RemoteFailureDistributionV1::new_disarmed_v1(),
            memory: RemoteMemoryAvailabilityV1::Unavailable,
        }
    }

    pub(crate) fn acquire_resource_v1(
        &mut self,
        class: RemoteResourceClassV1,
        bytes: RemoteAccountableBytesV1,
        limits: RemoteResourceLimitsV1,
    ) -> Result<RemoteResourceReservationV1, RemoteResourceRegistryErrorV1> {
        self.resource_registry.acquire_v1(class, bytes, limits)
    }

    pub(crate) fn release_resource_v1(
        &mut self,
        reservation: RemoteResourceReservationV1,
    ) -> Result<(), RemoteResourceRegistryErrorV1> {
        self.resource_registry.release_reservation_v1(reservation)
    }

    pub(crate) fn record_work_unit_v1(
        &mut self,
        class: RemoteWorkUnitClassV1,
        input_bytes: RemoteAccountableBytesV1,
        output_bytes: RemoteAccountableBytesV1,
        duration_micros: u64,
    ) -> RemoteWorkUnitObservationV1 {
        self.work_unit_registry.record_completion_v1(
            class,
            input_bytes,
            output_bytes,
            duration_micros,
        )
    }

    pub(crate) fn record_page_suspension_v1(
        &mut self,
        kind: RemotePageSuspensionKindV1,
        resume_nonce: Option<RemoteResumeNonceV1>,
    ) -> Result<(), RemotePageSuspensionErrorV1> {
        self.page_suspension
            .record_suspension_v1(kind, resume_nonce)
    }

    pub(crate) fn record_discarded_dt_v1(
        &mut self,
        duration_micros: u64,
    ) -> Result<(), RemotePageSuspensionErrorV1> {
        self.page_suspension.record_discarded_dt_v1(duration_micros)
    }

    pub(crate) fn record_resume_quantum_v1(
        &mut self,
        duration_micros: u64,
    ) -> Result<(), RemotePageSuspensionErrorV1> {
        self.page_suspension
            .record_resume_quantum_v1(duration_micros)
    }

    pub(crate) fn record_borrowed_legacy_hit_v1(&mut self) -> Result<(), RemoteInternBurnErrorV1> {
        self.intern_burn.record_borrowed_legacy_hit_v1()
    }

    pub(crate) fn record_remote_side_map_commit_v1(
        &mut self,
        string_bytes: RemoteAccountableBytesV1,
        entry_count: u64,
        entry_bytes: RemoteAccountableBytesV1,
        capacity_bytes: RemoteAccountableBytesV1,
    ) -> Result<(), RemoteInternBurnErrorV1> {
        self.intern_burn.record_remote_side_map_commit_v1(
            string_bytes,
            entry_count,
            entry_bytes,
            capacity_bytes,
        )
    }

    pub(crate) fn reserve_candidate_peak_v1(
        &mut self,
        bytes: RemoteAccountableBytesV1,
    ) -> Result<RemoteInternCandidateReservationV1, RemoteInternBurnErrorV1> {
        self.intern_burn.reserve_candidate_peak_v1(bytes)
    }

    pub(crate) fn release_candidate_peak_v1(
        &mut self,
        reservation: RemoteInternCandidateReservationV1,
    ) -> Result<(), RemoteInternBurnErrorV1> {
        self.intern_burn.release_candidate_peak_v1(reservation)
    }

    pub(crate) fn record_presentation_gc_count_v1(
        &mut self,
        counter: RemotePresentationGcCounterV1,
    ) -> Result<(), RemotePresentationGcErrorV1> {
        self.presentation_gc.record_count_v1(counter)
    }

    pub(crate) fn record_close_latency_v1(
        &mut self,
        duration_micros: u64,
    ) -> Result<(), RemotePresentationGcErrorV1> {
        self.presentation_gc
            .record_close_latency_v1(duration_micros)
    }

    pub(crate) fn record_failure_v1(
        &mut self,
        failure: RemoteMcapFailureV1,
        consistency_capability: RemoteFailureConsistencyCapabilityV1,
    ) -> Result<(), RemoteFailureDistributionErrorV1> {
        self.failure_distribution
            .record_failure_v1(failure, consistency_capability)
    }

    pub(crate) fn record_terminal_first_cause_v1(
        &mut self,
        cause: RemoteTerminalCauseV1,
    ) -> Result<(), RemoteFailureDistributionErrorV1> {
        self.failure_distribution
            .record_terminal_first_cause_v1(cause)
    }

    pub(crate) fn snapshot_v1(&self) -> RemoteMetricsSnapshotV1 {
        RemoteMetricsSnapshotV1 {
            resource: self.resource_registry.snapshot_v1(),
            work_units: self.work_unit_registry.snapshot_v1(),
            page_suspension: self.page_suspension.snapshot_v1(),
            intern_burn: self.intern_burn.snapshot_v1(),
            presentation_gc: self.presentation_gc.snapshot_v1(),
            failure: self.failure_distribution.snapshot_v1(),
            memory: self.memory,
        }
    }

    pub(crate) fn snapshot_for_route_v1(
        &self,
        route: RemoteMetricsRouteV1,
    ) -> Option<RemoteMetricsSnapshotV1> {
        route.is_remote_mcap_v1().then(|| self.snapshot_v1())
    }

    pub(crate) const fn accepts_metric_key_v1(&self, key: RemoteMetricKeyV1) -> bool {
        let _ = self;
        key.is_remote_mcap_v1()
    }

    pub(crate) fn debug_snapshot_v1(&self, cap: RemoteMetricsDebugCapV1) -> RemoteDebugSnapshotV1 {
        let snapshot = self.snapshot_v1();
        let mut critical = snapshot
            .resource
            .live_remote_owner_classes_v1()
            .into_iter()
            .map(|class| RemoteCriticalDiagnosticV1::LiveRemoteOwner { class })
            .collect::<Vec<_>>();
        if let Some(cause) = snapshot.failure.terminal_first_cause {
            critical.push(RemoteCriticalDiagnosticV1::TerminalCause(cause));
        }

        let entries = [
            RemoteDiagnosticEntryV1::new_v1(
                "resource_total_count",
                snapshot.resource.total_count_v1(),
            ),
            RemoteDiagnosticEntryV1::new_v1(
                "resource_total_bytes",
                snapshot.resource.total_bytes_v1(),
            ),
            RemoteDiagnosticEntryV1::new_v1(
                "body_pump_completed",
                snapshot
                    .work_units
                    .usage_v1(RemoteWorkUnitClassV1::BodyPump)
                    .completed_count_v1(),
            ),
            RemoteDiagnosticEntryV1::new_v1(
                "opening_parse_max_duration_micros",
                snapshot
                    .work_units
                    .usage_v1(RemoteWorkUnitClassV1::OpeningParse)
                    .max_duration_micros_v1(),
            ),
            RemoteDiagnosticEntryV1::new_v1(
                "remote_intern_burn_bytes",
                snapshot.intern_burn.remote_side_map_string_burn_bytes_v1(),
            ),
            RemoteDiagnosticEntryV1::new_v1(
                "insertion_count",
                snapshot
                    .presentation_gc
                    .count_v1(RemotePresentationGcCounterV1::Insertion),
            ),
        ];

        let mut best_effort = Vec::new();
        let mut used_bytes = 0_usize;
        let mut truncated = false;
        for entry in entries {
            let entry_len = entry.encoded_len_v1();
            if best_effort.len() >= cap.max_entries
                || used_bytes
                    .checked_add(entry_len)
                    .is_none_or(|next| next > cap.max_bytes)
            {
                truncated = true;
                continue;
            }
            used_bytes += entry_len;
            best_effort.push(entry);
        }

        RemoteDebugSnapshotV1 {
            critical,
            best_effort,
            truncated,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure_for_test_v1() -> RemoteMcapFailureV1 {
        RemoteMcapFailureV1 {
            classification: RemoteMcapFailureClassificationV1::Network,
            scope: None,
            owner: crate::web_remote_mcap_failure::RemoteMcapFailureOwnerV1::MetadataOpening,
            phase: RemoteMcapFailurePhaseV1::MetadataOpening,
            retryability: RemoteMcapFailureRetryabilityV1::Retryable,
            label: "redacted test failure",
        }
    }

    #[test]
    fn resource_registry_rolls_back_limit_failures_without_revision_or_usage_changes() {
        let mut registry = RemoteResourceRegistryV1::new_disarmed_v1();
        let limits = RemoteResourceLimitsV1::new_v1(u64::MAX, u64::MAX, 2, 20);
        let reservation = registry
            .acquire_v1(
                RemoteResourceClassV1::Slot,
                RemoteAccountableBytesV1::new_v1(10),
                limits,
            )
            .expect("first slot acquisition should fit");

        assert_eq!(reservation.class_v1(), RemoteResourceClassV1::Slot);
        assert_eq!(registry.revision_v1(), 1);
        assert_eq!(
            registry
                .usage_v1(RemoteResourceClassV1::Slot)
                .current_count_v1(),
            1
        );
        assert_eq!(
            registry
                .usage_v1(RemoteResourceClassV1::Slot)
                .current_bytes_v1(),
            10
        );
        let before = registry.snapshot_v1();

        let rejected = registry.acquire_v1(
            RemoteResourceClassV1::Slot,
            RemoteAccountableBytesV1::new_v1(11),
            limits,
        );

        assert!(matches!(
            rejected,
            Err(RemoteResourceRegistryErrorV1::ClassBytesLimitExceeded { .. })
        ));
        assert_eq!(registry.snapshot_v1(), before);
    }

    #[test]
    fn resource_registry_release_keeps_high_water_and_checks_underflow() {
        let mut registry = RemoteResourceRegistryV1::new_disarmed_v1();
        let limits = RemoteResourceLimitsV1::new_v1(u64::MAX, u64::MAX, u64::MAX, u64::MAX);
        let reservation = registry
            .acquire_v1(
                RemoteResourceClassV1::Client,
                RemoteAccountableBytesV1::new_v1(25),
                limits,
            )
            .expect("client acquisition should fit");
        registry
            .release_reservation_v1(reservation)
            .expect("release should succeed");
        let usage = registry.usage_v1(RemoteResourceClassV1::Client);

        assert_eq!(usage.current_count_v1(), 0);
        assert_eq!(usage.current_bytes_v1(), 0);
        assert_eq!(usage.high_water_count_v1(), 1);
        assert_eq!(usage.high_water_bytes_v1(), 25);
        assert!(matches!(
            registry.release_reservation_v1(reservation),
            Err(RemoteResourceRegistryErrorV1::ReleaseCountUnderflow)
        ));
    }

    #[test]
    fn resource_registry_reports_overflow_without_panicking() {
        let mut registry = RemoteResourceRegistryV1 {
            revision: 0,
            total_count: 0,
            total_bytes: 0,
            classes: [RemoteResourceUsageV1::ZERO; RemoteResourceClassV1::COUNT],
        };
        registry.classes[RemoteResourceClassV1::Operation.index_v1()] = RemoteResourceUsageV1 {
            current_count: u64::MAX,
            current_bytes: 0,
            high_water_count: u64::MAX,
            high_water_bytes: 0,
        };

        let result = registry.acquire_v1(
            RemoteResourceClassV1::Operation,
            RemoteAccountableBytesV1::new_v1(1),
            RemoteResourceLimitsV1::new_v1(u64::MAX, u64::MAX, u64::MAX, u64::MAX),
        );

        assert!(matches!(
            result,
            Err(RemoteResourceRegistryErrorV1::CountOverflow)
        ));
        assert_eq!(registry.revision_v1(), 0);
    }

    #[test]
    fn work_units_record_all_accounted_fields_and_saturate_overflow() {
        let mut registry = RemoteWorkUnitRegistryV1::new_disarmed_v1();
        let observation = registry.record_completion_v1(
            RemoteWorkUnitClassV1::BodyPump,
            RemoteAccountableBytesV1::new_v1(120),
            RemoteAccountableBytesV1::new_v1(80),
            11,
        );
        assert!(!observation.overflowed);

        let usage = registry.usage_v1(RemoteWorkUnitClassV1::BodyPump);
        assert_eq!(usage.completed_count_v1(), 1);
        assert_eq!(usage.input_bytes_v1(), 120);
        assert_eq!(usage.output_bytes_v1(), 80);
        assert_eq!(usage.max_duration_micros_v1(), 11);

        registry.units[RemoteWorkUnitClassV1::DispatchDecode.index_v1()] = RemoteWorkUnitUsageV1 {
            completed_count: u64::MAX,
            input_bytes: u64::MAX,
            output_bytes: 0,
            max_duration_micros: 7,
            overflowed: false,
        };
        let overflowed = registry.record_completion_v1(
            RemoteWorkUnitClassV1::DispatchDecode,
            RemoteAccountableBytesV1::new_v1(1),
            RemoteAccountableBytesV1::new_v1(1),
            9,
        );
        assert!(overflowed.overflowed);
        let usage = registry.usage_v1(RemoteWorkUnitClassV1::DispatchDecode);
        assert_eq!(usage.completed_count_v1(), u64::MAX);
        assert_eq!(usage.input_bytes_v1(), u64::MAX);
        assert_eq!(usage.max_duration_micros_v1(), 9);
    }

    #[test]
    fn intern_borrowed_legacy_burn_delta_is_zero_and_side_map_burn_survives_restart() {
        let mut intern = RemoteInternBurnV1::new_disarmed_v1();
        intern
            .record_borrowed_legacy_hit_v1()
            .expect("legacy hit should be recorded");
        assert_eq!(RemoteInternBurnV1::borrowed_legacy_burn_delta_v1(), 0);

        intern
            .record_remote_side_map_commit_v1(
                RemoteAccountableBytesV1::new_v1(15),
                1,
                RemoteAccountableBytesV1::new_v1(8),
                RemoteAccountableBytesV1::new_v1(4),
            )
            .expect("side-map commit should be recorded");
        let before_restart = intern.snapshot_v1();
        intern
            .record_viewer_restart_v1()
            .expect("viewer restart should be recorded");
        let after_restart = intern.snapshot_v1();

        assert_eq!(
            after_restart.remote_side_map_string_burn_bytes_v1(),
            before_restart.remote_side_map_string_burn_bytes_v1()
        );
        assert_eq!(
            after_restart.remote_side_map_entry_burn_bytes,
            before_restart.remote_side_map_entry_burn_bytes
        );
        assert_eq!(after_restart.coordination_revision_v1(), 1);
    }

    #[test]
    fn intern_candidate_peak_is_temporary_and_never_becomes_permanent_burn() {
        let mut intern = RemoteInternBurnV1::new_disarmed_v1();
        let permanent_before = intern.snapshot_v1();
        let reservation = intern
            .reserve_candidate_peak_v1(RemoteAccountableBytesV1::new_v1(64))
            .expect("candidate reservation should fit");
        intern
            .release_candidate_peak_v1(reservation)
            .expect("candidate reservation should be released");

        let after = intern.snapshot_v1();
        assert_eq!(after.candidate_peak_bytes_v1(), 0);
        assert_eq!(after.candidate_peak_high_water_bytes_v1(), 64);
        assert_eq!(
            after.remote_side_map_string_burn_bytes_v1(),
            permanent_before.remote_side_map_string_burn_bytes_v1()
        );
    }

    #[test]
    fn failure_distribution_preserves_terminal_first_cause_and_is_low_cardinality() {
        let mut distribution = RemoteFailureDistributionV1::new_disarmed_v1();
        let failure = failure_for_test_v1();
        distribution
            .record_failure_v1(
                failure,
                RemoteFailureConsistencyCapabilityV1::StrongValidator,
            )
            .expect("failure should be recorded");
        let cause = RemoteTerminalCauseV1::SessionFatal(failure);
        distribution
            .record_terminal_first_cause_v1(cause)
            .expect("first terminal cause should be recorded");
        let snapshot = distribution.snapshot_v1();

        assert_eq!(
            snapshot.terminal_first_cause_v1(),
            Some(RemoteTerminalCauseV1::SessionFatal(failure))
        );
        assert!(matches!(
            distribution.record_terminal_first_cause_v1(cause),
            Err(RemoteFailureDistributionErrorV1::TerminalCauseAlreadyRecorded)
        ));
        assert!(format!("{snapshot:?}").len() < 512);
    }

    #[test]
    fn memory_unavailable_snapshot_has_no_precise_heap_bytes() {
        let metrics = WebRemoteMcapMetricsV1::new_disarmed_v1();
        let snapshot = metrics.snapshot_v1();
        assert_eq!(
            snapshot.memory_v1(),
            RemoteMemoryAvailabilityV1::Unavailable
        );
        assert!(snapshot.memory_v1().precise_heap_bytes_v1().is_none());
        assert!(!format!("{snapshot:?}").contains("precise_heap"));
    }

    #[test]
    fn debug_snapshot_truncates_best_effort_but_keeps_live_owner_and_terminal_cause() {
        let mut metrics = WebRemoteMcapMetricsV1::new_disarmed_v1();
        let limits = RemoteResourceLimitsV1::new_v1(u64::MAX, u64::MAX, u64::MAX, u64::MAX);
        metrics
            .acquire_resource_v1(
                RemoteResourceClassV1::Source,
                RemoteAccountableBytesV1::new_v1(1),
                limits,
            )
            .expect("resource acquisition should fit");
        metrics
            .record_terminal_first_cause_v1(RemoteTerminalCauseV1::ExplicitClose)
            .expect("terminal cause should be recorded");

        let debug = metrics.debug_snapshot_v1(RemoteMetricsDebugCapV1::new_v1(1, 64));
        assert!(debug.truncated_v1());
        assert!(
            debug
                .critical_v1()
                .iter()
                .any(|entry| matches!(entry, RemoteCriticalDiagnosticV1::LiveRemoteOwner { .. }))
        );
        assert!(debug.critical_v1().iter().any(|entry| matches!(
            entry,
            RemoteCriticalDiagnosticV1::TerminalCause(RemoteTerminalCauseV1::ExplicitClose)
        )));
        assert_eq!(debug.best_effort_v1().len(), 1);
    }

    #[test]
    fn remote_only_keys_and_routes_reject_nonremote_and_global_access() {
        let metrics = WebRemoteMcapMetricsV1::new_disarmed_v1();
        assert!(metrics.accepts_metric_key_v1(RemoteMetricKeyV1::RemoteResource));
        assert!(!metrics.accepts_metric_key_v1(RemoteMetricKeyV1::Global));
        assert!(!metrics.accepts_metric_key_v1(RemoteMetricKeyV1::Redap));
        assert!(
            metrics
                .snapshot_for_route_v1(RemoteMetricsRouteV1::RemoteMcap)
                .is_some()
        );
        assert!(
            metrics
                .snapshot_for_route_v1(RemoteMetricsRouteV1::Global)
                .is_none()
        );
        assert!(
            metrics
                .snapshot_for_route_v1(RemoteMetricsRouteV1::LogChannel)
                .is_none()
        );
    }

    #[test]
    fn debug_snapshot_labels_do_not_contain_high_cardinality_secret_tokens() {
        let metrics = WebRemoteMcapMetricsV1::new_disarmed_v1();
        let debug = metrics.debug_snapshot_v1(RemoteMetricsDebugCapV1::new_v1(64, 2048));
        let rendered = format!("{debug:?}");

        for secret in [
            "url=",
            "query=",
            "etag",
            "topic",
            "schema",
            "entity_path",
            "store_id",
            "token",
            "generation=",
            "payload",
        ] {
            assert!(
                !rendered.contains(secret),
                "leaked marker {secret:?}: {rendered}"
            );
        }
    }
}
