//! Attested Phase A measurement bridge.
//!
//! This module is absent from ordinary product builds. It issues the unfrozen measurement profile
//! used by MCAP-033 while executing the same move-only opening and physical-source transitions.

use std::num::NonZeroU64;

use super::*;
use crate::remote_fixed_layout::{RemoteMcapSlice, prepare_fixed_layout};
use crate::remote_summary::ambiguous_zero::{AmbiguousZeroBudget, AmbiguousZeroLimits};
use crate::remote_summary::materialization::{
    NestedPreflightCensus, SummaryMaterializationBudget, SummaryMaterializationLimits,
};
use crate::remote_summary::message_index::{MessageIndexRegionBudget, MessageIndexRegionLimits};
use crate::remote_summary::physical_regions::{PhysicalRegionBudget, PhysicalRegionLimits};

const RECORD_ENVELOPE_LEN: usize = 9;
const FOOTER_TAIL_LEN: usize = RECORD_ENVELOPE_LEN + 20 + mcap::MAGIC.len();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhaseAMeasurementOutputV1 {
    pub input_bytes: u64,
    pub output_bytes: u64,
    pub completed_count: u64,
    pub retained_high_water_bytes: u64,
}

pub struct PhaseAOpeningStageOwnerV1<'a> {
    physical: PreparedBoundRemotePhysicalSourceV1<
        &'static str,
        crate::remote_summary::physical_regions::ValidatedPhysicalRegions<'a>,
    >,
    bytes: &'a [u8],
}

pub struct PhaseAMessageIndexStageOwnerV1<'a> {
    plan: PreparedBoundRemotePhysicalSourceV1<
        &'static str,
        crate::remote_summary::ambiguous_zero::PreparedAmbiguousZeroBodyPlan<'a>,
    >,
    bytes: &'a [u8],
}

pub struct PhaseAPhysicalValidationStageOwnerV1<'a> {
    evidence: crate::remote_chunk_scan::PhysicalChunkMessageEvidenceV1<'a>,
}

fn measurement_profile_v1(object_len: u64) -> RemotePhysicalEvidenceProfileV1 {
    let summary_census_limits =
        crate::remote_summary::SummaryCensusLimits::for_phase_a_measurement_v1(object_len, 10_000);
    let summary_materialization_budget = SummaryMaterializationBudget::for_phase_a_measurement_v1(
        SummaryMaterializationLimits::for_phase_a_measurement_v1(
            10_000,
            object_len,
            object_len,
            object_len.saturating_mul(8),
            10_000,
            10_000,
        ),
        1,
        NestedPreflightCensus {
            main_summary_records: 10_000,
            owned_string_count: 100_000,
            owned_string_bytes: object_len.saturating_mul(8),
            schema_data_bytes: object_len.saturating_mul(8),
            channel_metadata_entries: 100_000,
            fixed_map_entries: 100_000,
            nested_encoded_bytes: object_len.saturating_mul(8),
            nested_retained_bytes: object_len.saturating_mul(8),
        },
    );
    let physical_region_budget = PhysicalRegionBudget::for_phase_a_measurement_v1(
        PhysicalRegionLimits::for_phase_a_measurement_v1(
            10_000,
            object_len,
            object_len,
            object_len,
            object_len,
            object_len.saturating_mul(8),
            object_len.saturating_mul(8),
        ),
        1,
        10_000,
        object_len.saturating_mul(8),
    );
    let ambiguous_zero_budget = AmbiguousZeroBudget::for_phase_a_measurement_v1(
        AmbiguousZeroLimits::for_phase_a_measurement_v1(
            [10_000, object_len, 1_000_000, 10_000, object_len],
            [
                10_000, object_len, object_len, object_len, 10_000, object_len, 1_000_000,
            ],
        ),
        1,
        1,
        1,
    );
    let message_index_budget = MessageIndexRegionBudget::for_phase_a_measurement_v1(
        MessageIndexRegionLimits::for_phase_a_measurement_v1(
            10_000, 1_000_000, object_len, object_len,
        ),
        10_000,
        object_len,
        10_000,
        1_000_000,
        1_000_000,
        object_len,
    );
    RemotePhysicalEvidenceProfileV1 {
        summary_census_limits,
        summary_materialization_budget,
        physical_region_budget,
        message_index_budget,
        ambiguous_zero_budget,
        decompression_budget:
            crate::remote_decompression::chunk_decompression_budget_for_phase_a_measurement_v1(),
        scan_budget: crate::remote_chunk_scan::PhysicalChunkScanBudget::for_phase_a_measurement_v1(
            crate::remote_chunk_scan::PhysicalChunkScanLimits::for_phase_a_measurement_v1(),
        ),
        aggregate_budget_root: AggregateResolutionBudgetRootV1::new_for_sealed_profile_v1(
            1,
            object_len.saturating_add(16 << 20),
        ),
    }
}

fn prepare_opening_v1(
    bytes: &[u8],
) -> Result<
    PreparedBoundRemotePhysicalSourceV1<
        &'static str,
        crate::remote_summary::physical_regions::ValidatedPhysicalRegions<'_>,
    >,
    PhysicalSourceResolutionErrorV1,
> {
    let object_len = u64::try_from(bytes.len())
        .map_err(|_error| PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?;
    let tail_start = bytes
        .len()
        .checked_sub(FOOTER_TAIL_LEN)
        .ok_or(PhysicalSourceResolutionErrorV1::InvalidFixedLayout)?;
    let initial_read = RemoteMcapSlice::new(
        0,
        bytes
            .get(..mcap::MAGIC.len() + RECORD_ENVELOPE_LEN)
            .ok_or(PhysicalSourceResolutionErrorV1::InvalidFixedLayout)?,
    );
    let footer_read = RemoteMcapSlice::new(
        u64::try_from(tail_start)
            .map_err(|_error| PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?,
        &bytes[tail_start..],
    );
    let fixed = prepare_fixed_layout(object_len, initial_read, footer_read)
        .map_err(|_error| PhysicalSourceResolutionErrorV1::InvalidFixedLayout)?;
    let summary_range = fixed.data_end_and_summary_range();
    let summary_start = usize::try_from(summary_range.start)
        .map_err(|_error| PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?;
    let summary_end = usize::try_from(summary_range.end)
        .map_err(|_error| PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?;
    let summary_read = RemoteMcapSlice::new(
        summary_range.start,
        bytes
            .get(summary_start..summary_end)
            .ok_or(PhysicalSourceResolutionErrorV1::InvalidFixedLayout)?,
    );
    let evidence = PreparedBoundRemotePhysicalSourceV1::<&'static str, ()>::issue_and_prepare_fixed_layout_for_phase_a_measurement_v1(
        "phase-a-measurement-validator",
        NonZeroU64::new(object_len).ok_or(PhysicalSourceResolutionErrorV1::InvalidFixedLayout)?,
        RemoteObjectConsistencyClassV1::StrongValidator,
        initial_read,
        footer_read,
        measurement_profile_v1(object_len),
    )?;
    let summary_read = evidence.issue_read_v1(summary_read)?;
    let evidence = evidence
        .validate_data_end_and_summary_v1(summary_read)
        .map_err(|_error| PhysicalSourceResolutionErrorV1::InvalidFixedLayout)?;
    let header_envelope_start = mcap::MAGIC.len();
    let header_body_start = header_envelope_start
        .checked_add(RECORD_ENVELOPE_LEN)
        .ok_or(PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?;
    if bytes.get(header_envelope_start) != Some(&mcap::records::op::HEADER) {
        return Err(PhysicalSourceResolutionErrorV1::InvalidFixedLayout);
    }
    let header_body_len = u64::from_le_bytes(
        bytes
            .get(header_envelope_start + 1..header_body_start)
            .ok_or(PhysicalSourceResolutionErrorV1::InvalidFixedLayout)?
            .try_into()
            .map_err(|_error| PhysicalSourceResolutionErrorV1::InvalidFixedLayout)?,
    );
    let header_end = u64::try_from(header_body_start)
        .ok()
        .and_then(|start| start.checked_add(header_body_len))
        .and_then(|end| usize::try_from(end).ok())
        .ok_or(PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?;
    let header_read = evidence.issue_read_v1(RemoteMcapSlice::new(
        u64::try_from(header_body_start)
            .map_err(|_error| PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?,
        bytes
            .get(header_body_start..header_end)
            .ok_or(PhysicalSourceResolutionErrorV1::InvalidFixedLayout)?,
    ))?;
    evidence
        .prepare_summary_v1(header_read)
        .map_err(|_error| PhysicalSourceResolutionErrorV1::InvalidFixedLayout)?
        .preflight_materialization_v1()
        .map_err(|_error| PhysicalSourceResolutionErrorV1::AllocationFailed)?
        .materialize_summary_v1()
        .map_err(|_error| PhysicalSourceResolutionErrorV1::AllocationFailed)?
        .validate_definitions_v1()
        .map_err(|_error| PhysicalSourceResolutionErrorV1::InvalidFixedLayout)?
        .validate_physical_regions_v1()
        .map_err(|_error| PhysicalSourceResolutionErrorV1::InvalidFixedLayout)
}

pub fn run_opening_parse_v1(
    bytes: &[u8],
) -> Result<PhaseAMeasurementOutputV1, PhysicalSourceResolutionErrorV1> {
    let input_bytes = u64::try_from(bytes.len())
        .map_err(|_error| PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?;
    let owner = prepare_opening_stage_v1(bytes)?;
    let physical = &owner.physical;
    let output_bytes = physical
        .inner
        .retained_bytes_v1()
        .map_err(|_error| PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?;
    Ok(PhaseAMeasurementOutputV1 {
        input_bytes,
        output_bytes,
        completed_count: u64::try_from(physical.inner.canonical_chunk_count())
            .map_err(|_error| PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?,
        retained_high_water_bytes: input_bytes
            .checked_add(output_bytes)
            .ok_or(PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?,
    })
}

pub fn prepare_opening_stage_v1(
    bytes: &[u8],
) -> Result<PhaseAOpeningStageOwnerV1<'_>, PhysicalSourceResolutionErrorV1> {
    Ok(PhaseAOpeningStageOwnerV1 {
        physical: prepare_opening_v1(bytes)?,
        bytes,
    })
}

impl PhaseAOpeningStageOwnerV1<'_> {
    pub fn measurement_v1(
        &self,
    ) -> Result<PhaseAMeasurementOutputV1, PhysicalSourceResolutionErrorV1> {
        let input_bytes = u64::try_from(self.bytes.len())
            .map_err(|_error| PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?;
        let output_bytes = self
            .physical
            .inner
            .retained_bytes_v1()
            .map_err(|_error| PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?;
        Ok(PhaseAMeasurementOutputV1 {
            input_bytes,
            output_bytes,
            completed_count: u64::try_from(self.physical.inner.canonical_chunk_count())
                .map_err(|_error| PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?,
            retained_high_water_bytes: 0,
        })
    }
}

fn prepare_body_plan_v1(
    bytes: &[u8],
) -> Result<
    PreparedBoundRemotePhysicalSourceV1<
        &'static str,
        crate::remote_summary::ambiguous_zero::PreparedAmbiguousZeroBodyPlan<'_>,
    >,
    PhysicalSourceResolutionErrorV1,
> {
    prepare_message_index_stage_v1(prepare_opening_stage_v1(bytes)?).map(|owner| owner.plan)
}

pub fn prepare_message_index_stage_v1(
    opening: PhaseAOpeningStageOwnerV1<'_>,
) -> Result<PhaseAMessageIndexStageOwnerV1<'_>, PhysicalSourceResolutionErrorV1> {
    let bytes = opening.bytes;
    let mut evidence = opening
        .physical
        .prepare_ambiguous_zero_v1()
        .map_err(|_error| PhysicalSourceResolutionErrorV1::InvalidFixedLayout)?;
    loop {
        match evidence
            .next_ambiguous_zero_v1()
            .map_err(|_error| PhysicalSourceResolutionErrorV1::InvalidFixedLayout)?
        {
            BoundAmbiguousZeroTurnV1::MessageIndex(prepared) => {
                let range = prepared.expected_message_index_range_v1();
                let start = usize::try_from(range.start)
                    .map_err(|_error| PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?;
                let end = usize::try_from(range.end)
                    .map_err(|_error| PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?;
                let read = prepared.issue_read_v1(RemoteMcapSlice::new(
                    range.start,
                    bytes
                        .get(start..end)
                        .ok_or(PhysicalSourceResolutionErrorV1::InvalidFixedLayout)?,
                ))?;
                evidence = prepared
                    .install_and_execute_message_index_v1(read)
                    .map_err(|_error| PhysicalSourceResolutionErrorV1::InvalidFixedLayout)?;
            }
            BoundAmbiguousZeroTurnV1::BodyPlan(plan) => {
                return Ok(PhaseAMessageIndexStageOwnerV1 { plan, bytes });
            }
        }
    }
}

impl PhaseAMessageIndexStageOwnerV1<'_> {
    pub fn measurement_v1(
        &self,
    ) -> Result<PhaseAMeasurementOutputV1, PhysicalSourceResolutionErrorV1> {
        let input_bytes = u64::try_from(self.bytes.len())
            .map_err(|_error| PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?;
        let output_bytes = self
            .plan
            .inner
            .retained_resolution_evidence_bytes_v1()
            .map_err(|_error| PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?;
        Ok(PhaseAMeasurementOutputV1 {
            input_bytes,
            output_bytes,
            completed_count: 1,
            retained_high_water_bytes: 0,
        })
    }
}

pub fn run_message_index_parse_v1(
    bytes: &[u8],
) -> Result<PhaseAMeasurementOutputV1, PhysicalSourceResolutionErrorV1> {
    let input_bytes = u64::try_from(bytes.len())
        .map_err(|_error| PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?;
    let plan = prepare_body_plan_v1(bytes)?;
    let output_bytes = plan
        .inner
        .retained_resolution_evidence_bytes_v1()
        .map_err(|_error| PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?;
    Ok(PhaseAMeasurementOutputV1 {
        input_bytes,
        output_bytes,
        completed_count: 1,
        retained_high_water_bytes: input_bytes
            .checked_add(output_bytes)
            .ok_or(PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?,
    })
}

pub struct PhaseAMeasurementResolvedSourceV1<'a> {
    owner: BoundResolvedPhysicalSourceAuthorityV1<'a, &'static str>,
    bytes: &'a [u8],
}

impl PhaseAMeasurementResolvedSourceV1<'_> {
    pub fn source_generation_v1(&self) -> u64 {
        self.owner.source_v1().source_generation_v1()
    }

    pub fn chunk_range_v1(&self) -> Result<std::ops::Range<u64>, PhysicalSourceResolutionErrorV1> {
        self.owner
            .inner
            .authority
            .physical_region_v1(0)
            .map(|region| region.chunk_range())
            .ok_or(PhysicalSourceResolutionErrorV1::InvalidPlan)
    }

    pub fn issue_pending_v1(
        &self,
    ) -> Result<
        crate::web_body_handoff::WebPendingPhysicalChunkReadV1<'_>,
        PhysicalSourceResolutionErrorV1,
    > {
        self.owner
            .inner
            .issue_lease_v1(0)
            .map(crate::web_body_handoff::WebPendingPhysicalChunkReadV1::from_lease_v1)
    }

    pub fn bytes_v1(&self) -> &[u8] {
        self.bytes
    }

    pub fn run_dispatch_decode_v1(
        &self,
        cache: crate::web_body_handoff::WebPhysicalScanCacheEntryV1<'_>,
        channel_id: u16,
    ) -> Result<PhaseAMeasurementOutputV1, PhysicalSourceResolutionErrorV1> {
        let validation = complete_physical_validation_stage_v1(cache)?;
        self.run_dispatch_from_validation_stage_v1(validation, channel_id)
            .map(|(measurement, _chunks)| measurement)
    }

    pub fn run_dispatch_from_validation_stage_v1(
        &self,
        validation: PhaseAPhysicalValidationStageOwnerV1<'_>,
        channel_id: u16,
    ) -> Result<(PhaseAMeasurementOutputV1, Vec<re_chunk::Chunk>), PhysicalSourceResolutionErrorV1>
    {
        use re_chunk::external::re_byte_size::SizeBytes as _;

        let evidence = validation.evidence;
        let input_bytes = evidence
            .retained_physical_bytes_v1()
            .map_err(PhysicalSourceResolutionErrorV1::Physical)?;
        let physical = self
            .owner
            .inner
            .authority
            .definitions_capability_for_remote_assignment_v1();
        let chunks = crate::remote_decoder_assignment::execute_group_from_finalized_source_for_phase_a_measurement_v1(
            self.owner.source_v1(),
            &physical,
            evidence,
            channel_id,
        )
        .map_err(|_error| PhysicalSourceResolutionErrorV1::InvalidPlan)?;
        let output_bytes = chunks.iter().try_fold(0_u64, |total, chunk| {
            total
                .checked_add(chunk.heap_size_bytes())
                .ok_or(PhysicalSourceResolutionErrorV1::ArithmeticOverflow)
        })?;
        let completed_count = u64::try_from(chunks.len())
            .map_err(|_overflow| PhysicalSourceResolutionErrorV1::ArithmeticOverflow)?;
        Ok((
            PhaseAMeasurementOutputV1 {
                input_bytes,
                output_bytes,
                completed_count,
                retained_high_water_bytes: 0,
            },
            chunks,
        ))
    }

    pub fn measure_physical_validation_v1(
        cache: crate::web_body_handoff::WebPhysicalScanCacheEntryV1<'_>,
    ) -> Result<PhaseAMeasurementOutputV1, PhysicalSourceResolutionErrorV1> {
        let owner = complete_physical_validation_stage_v1(cache)?;
        owner.measurement_v1()
    }
}

pub fn complete_physical_validation_stage_v1(
    cache: crate::web_body_handoff::WebPhysicalScanCacheEntryV1<'_>,
) -> Result<PhaseAPhysicalValidationStageOwnerV1<'_>, PhysicalSourceResolutionErrorV1> {
    let evidence = cache
        .into_message_evidence_for_phase_a_measurement_v1()
        .map_err(|_error| PhysicalSourceResolutionErrorV1::InvalidPlan)?;
    Ok(PhaseAPhysicalValidationStageOwnerV1 { evidence })
}

impl PhaseAPhysicalValidationStageOwnerV1<'_> {
    pub fn measurement_v1(
        &self,
    ) -> Result<PhaseAMeasurementOutputV1, PhysicalSourceResolutionErrorV1> {
        let retained = self
            .evidence
            .retained_physical_bytes_v1()
            .map_err(PhysicalSourceResolutionErrorV1::Physical)?;
        Ok(PhaseAMeasurementOutputV1 {
            input_bytes: retained,
            output_bytes: retained,
            completed_count: 1,
            retained_high_water_bytes: retained,
        })
    }
}

pub fn with_resolved_source_v1<R>(
    bytes: &[u8],
    callback: impl FnOnce(&PhaseAMeasurementResolvedSourceV1<'_>) -> R,
) -> Result<R, PhysicalSourceResolutionErrorV1> {
    let owner = prepare_body_plan_v1(bytes)?.prepare()?.finalize_v1()?;
    Ok(callback(&PhaseAMeasurementResolvedSourceV1 {
        owner,
        bytes,
    }))
}

pub fn resolve_source_v1(
    bytes: &[u8],
) -> Result<PhaseAMeasurementResolvedSourceV1<'_>, PhysicalSourceResolutionErrorV1> {
    let owner = prepare_body_plan_v1(bytes)?.prepare()?.finalize_v1()?;
    Ok(PhaseAMeasurementResolvedSourceV1 { owner, bytes })
}

pub fn resolve_message_index_stage_v1(
    stage: PhaseAMessageIndexStageOwnerV1<'_>,
) -> Result<PhaseAMeasurementResolvedSourceV1<'_>, PhysicalSourceResolutionErrorV1> {
    let owner = stage.plan.prepare()?.finalize_v1()?;
    Ok(PhaseAMeasurementResolvedSourceV1 {
        owner,
        bytes: stage.bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{
        AdversarialMcapFixture, AdversarialMcapFixtureBuilder, FixtureChannel, FixtureChunk,
        FixtureMessage, FixtureSchema, PartitionFixture,
    };

    fn fixture() -> AdversarialMcapFixture {
        AdversarialMcapFixtureBuilder::new()
            .with_chunks([FixtureChunk::new([
                FixtureMessage::new(1, 1, 10),
                FixtureMessage::new(1, 2, 20),
            ])])
            .build()
            .expect("the fixed Phase A input fixture builds")
    }

    fn ros_fixture() -> AdversarialMcapFixture {
        let mut payload = vec![0, 1, 0, 0];
        payload.extend_from_slice(&42_i32.to_le_bytes());
        AdversarialMcapFixtureBuilder::new()
            .with_schemas([FixtureSchema::new(7, "pkg/Root", "ros2msg").with_data(b"int32 value")])
            .with_channels([FixtureChannel::schema_less(1, "/phase_a").with_schema(7, "cdr")])
            .with_chunks([FixtureChunk::new([
                FixtureMessage::new(1, 1, 1).with_data(payload)
            ])])
            .with_partition_fixture(PartitionFixture::default())
            .build()
            .expect("the fixed Phase A ROS input fixture builds")
    }

    #[test]
    fn opening_and_message_index_measurements_use_the_sealed_pipeline() {
        let fixture = fixture();
        let opening = run_opening_parse_v1(&fixture.bytes).unwrap();
        let index = run_message_index_parse_v1(&fixture.bytes).unwrap();

        assert_eq!(opening.input_bytes, fixture.bytes.len() as u64);
        assert_eq!(opening.completed_count, 1);
        assert!(opening.output_bytes > 0);
        assert!(opening.retained_high_water_bytes >= opening.input_bytes);
        assert_eq!(index.input_bytes, fixture.bytes.len() as u64);
        assert_eq!(index.completed_count, 1);
        assert!(index.output_bytes > 0);
        assert!(index.retained_high_water_bytes >= index.input_bytes);
    }

    #[test]
    fn finalized_measurement_source_issues_the_exact_chunk_range() {
        let fixture = fixture();
        with_resolved_source_v1(&fixture.bytes, |source| {
            let range = source.chunk_range_v1().unwrap();
            assert_eq!(range.start as usize, fixture.layout.chunks[0].record.start);
            assert_eq!(range.end as usize, fixture.layout.chunks[0].record.end);
            let pending = source.issue_pending_v1().unwrap();
            let identity = pending.identity_v1().unwrap();
            assert_eq!(
                identity.source_generation_v1(),
                source.source_generation_v1()
            );
            assert_eq!(identity.full_range_v1(), (range.start, range.end));
        })
        .unwrap();
    }

    #[test]
    fn physical_validation_and_dispatch_use_the_sealed_025a_030_031_chain() {
        let fixture = ros_fixture();
        with_resolved_source_v1(&fixture.bytes, |source| {
            let range = source.chunk_range_v1().unwrap();
            let body = &fixture.bytes[range.start as usize..range.end as usize];
            let completed = source
                .issue_pending_v1()
                .unwrap()
                .bind_borrowed_exact_body_v1(
                    body,
                    crate::web_body_handoff::WebPhysicalBudgetProfileV1::UnfrozenPhaseACandidate,
                )
                .unwrap()
                .process_explicit_copy_v1(
                    &crate::web_body_handoff::WebPhysicalCopyOverlapBudgetV1::new_unfrozen_phase_a_v1(),
                    |_safe_point| Ok(()),
                )
                .unwrap();
            let cache = completed.into_cache_after_body_drop_v1();
            let result = source.run_dispatch_decode_v1(cache, 1).unwrap();
            assert!(result.input_bytes > 0);
            assert!(result.output_bytes > 0);
            assert_eq!(result.completed_count, 1);
            assert_eq!(result.retained_high_water_bytes, 0);
        })
        .unwrap();
    }
}
