//! Admitted second-pass dispatch and terminal publication for remote MCAP chunks.
//!
//! This module deliberately owns no Viewer or Store mutation.  It consumes the sealed
//! validation plan and one-shot executable adapter and returns an all-or-nothing terminal
//! payload.  Callers must publish the returned payload only after the terminal state is complete.

#![allow(dead_code)]
#![allow(
    clippy::ignored_unit_patterns,
    clippy::map_err_ignore,
    clippy::needless_pass_by_value
)]

use crate::remote_channel_group::CanonicalSourceOrderKeyV1;
use crate::remote_chunk_scan::{PhysicalChunkMessageEvidenceV1, RemoteMessageEnvelopeV1};
use crate::remote_chunk_validation_count::{
    RemoteValidationCountErrorV1, ValidatedChunkDispatchPlanV1,
};
use crate::remote_protobuf_descriptor::{
    RemoteExecutableAdapterErrorV1, RemoteExecutableDecoderAdapterV1, RemoteNormalizedValueV1,
    RemoteTypedDecodedBatchV1,
};
use crate::remote_time::{RawMcapTime, canonicalize_raw_mcap_time};
use crate::remote_typed_output::{RemoteTypedOutputDescriptorV1, RemoteTypedScalarKindV1};
use arrow::array::{
    ArrayRef, BooleanArray, Float32Array, Float64Array, Int8Array, Int16Array, Int32Array,
    Int64Array, StructArray, UInt8Array, UInt16Array, UInt32Array, UInt64Array,
};
use arrow::datatypes::{DataType, Field, Fields};
use re_chunk::{Chunk, TimePoint, TimelineName};
use re_log_types::TimeCell;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteChunkDispatchFailureV1 {
    StaleSource,
    PlanMismatch,
    Decode(RemoteExecutableAdapterErrorV1),
    Validation(RemoteValidationCountErrorV1),
}

pub(crate) enum RemoteChunkTerminalV1 {
    Complete(RemoteTypedDecodedBatchV1),
    CompleteEmpty,
    Failed(RemoteChunkDispatchFailureV1),
}

pub(crate) struct RemoteTypedChunkHandoffV1 {
    chunk: Chunk,
    _reservation: crate::remote_chunk_validation_count::RemoteTypedOutputReservationV1,
}

fn row_id_from_envelope_v1(
    envelope: &RemoteMessageEnvelopeV1<'_>,
) -> Result<re_chunk::RowId, RemoteChunkDispatchFailureV1> {
    CanonicalSourceOrderKeyV1::new(
        envelope.top_level_record_absolute_offset,
        envelope.record_local_offset,
        0,
    )
    .map(CanonicalSourceOrderKeyV1::stable_row_id)
    .map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)
}

impl RemoteTypedChunkHandoffV1 {
    /// The publication arbiter may inspect the sealed chunk while the matching plan-root
    /// reservation is necessarily still held.
    pub(crate) const fn chunk_v1(&self) -> &Chunk {
        &self.chunk
    }
}

pub(crate) fn build_ros_scalar_chunk_v1(
    descriptor: RemoteTypedOutputDescriptorV1,
    plan: &ValidatedChunkDispatchPlanV1<'_, '_, '_, '_, '_, '_>,
    batch: RemoteTypedDecodedBatchV1,
) -> Result<RemoteTypedChunkHandoffV1, RemoteChunkDispatchFailureV1> {
    // V1 has one output per sealed single-field contract. The authority owns this ordinal;
    // callers cannot select an arbitrary root namespace.
    let output_ordinal = 0;
    descriptor
        .ensure_current_v1()
        .map_err(|_| RemoteChunkDispatchFailureV1::StaleSource)?;
    if !descriptor.matches_batch_v1(&batch) {
        return Err(RemoteChunkDispatchFailureV1::StaleSource);
    }
    let authority = plan.authority_v1();
    authority
        .ensure_current_v1()
        .map_err(|_| RemoteChunkDispatchFailureV1::StaleSource)?;
    plan.ensure_current_v1()
        .map_err(RemoteChunkDispatchFailureV1::Validation)?;
    if !descriptor.matches_binding_v1(plan.evidence_v1().source_binding_v1()) {
        return Err(RemoteChunkDispatchFailureV1::StaleSource);
    }
    let expected_channel = plan
        .channels_v1()
        .iter()
        .find(|channel| channel.channel_id_v1() == descriptor.channel_id_v1())
        .copied()
        .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?;
    if batch.rows_v1() != expected_channel.message_count_v1()
        || batch.input_payload_bytes_v1() != expected_channel.payload_bytes_v1()
    {
        return Err(RemoteChunkDispatchFailureV1::PlanMismatch);
    }
    let output_reservation = plan
        .reserve_typed_output_v1(batch.rows_v1(), &descriptor)
        .map_err(RemoteChunkDispatchFailureV1::Validation)?;
    let root = authority
        .issue_root_v1(output_ordinal)
        .map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?;
    let mut builder = Chunk::builder_with_id(root.root_chunk_id_v1(), descriptor.entity_path_v1());
    let mut messages = plan
        .evidence_v1()
        .message_envelopes_v1()
        .map_err(|_| RemoteChunkDispatchFailureV1::StaleSource)?;
    for envelope in batch.envelopes_v1() {
        let message = loop {
            let Some(message) = messages
                .next_v1()
                .map_err(|_| RemoteChunkDispatchFailureV1::StaleSource)?
            else {
                return Err(RemoteChunkDispatchFailureV1::PlanMismatch);
            };
            if message.channel_id == descriptor.channel_id_v1() {
                break message;
            }
        };
        let [field] = envelope.fields_v1() else {
            return Err(RemoteChunkDispatchFailureV1::PlanMismatch);
        };
        let expected = descriptor
            .field_kind_v1(field.tag)
            .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?;
        let scalar: ArrayRef = match (expected, field.value) {
            (RemoteTypedScalarKindV1::Int8, RemoteNormalizedValueV1::Signed(value)) => {
                std::sync::Arc::new(Int8Array::from(vec![
                    i8::try_from(value).map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?,
                ]))
            }
            (RemoteTypedScalarKindV1::Int16, RemoteNormalizedValueV1::Signed(value)) => {
                std::sync::Arc::new(Int16Array::from(vec![
                    i16::try_from(value).map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?,
                ]))
            }
            (RemoteTypedScalarKindV1::Int32, RemoteNormalizedValueV1::Signed(value)) => {
                std::sync::Arc::new(Int32Array::from(vec![
                    i32::try_from(value).map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?,
                ]))
            }
            (RemoteTypedScalarKindV1::Int64, RemoteNormalizedValueV1::Signed(value)) => {
                std::sync::Arc::new(Int64Array::from(vec![value]))
            }
            (RemoteTypedScalarKindV1::UInt8, RemoteNormalizedValueV1::Unsigned(value)) => {
                std::sync::Arc::new(UInt8Array::from(vec![
                    u8::try_from(value).map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?,
                ]))
            }
            (RemoteTypedScalarKindV1::UInt16, RemoteNormalizedValueV1::Unsigned(value)) => {
                std::sync::Arc::new(UInt16Array::from(vec![
                    u16::try_from(value).map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?,
                ]))
            }
            (RemoteTypedScalarKindV1::UInt32, RemoteNormalizedValueV1::Unsigned(value)) => {
                std::sync::Arc::new(UInt32Array::from(vec![
                    u32::try_from(value).map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?,
                ]))
            }
            (RemoteTypedScalarKindV1::UInt64, RemoteNormalizedValueV1::Unsigned(value)) => {
                std::sync::Arc::new(UInt64Array::from(vec![value]))
            }
            (RemoteTypedScalarKindV1::Float32, RemoteNormalizedValueV1::Float32(value)) => {
                std::sync::Arc::new(Float32Array::from(vec![f32::from_bits(value)]))
            }
            (RemoteTypedScalarKindV1::Float64, RemoteNormalizedValueV1::Float64(value)) => {
                std::sync::Arc::new(Float64Array::from(vec![f64::from_bits(value)]))
            }
            (RemoteTypedScalarKindV1::Bool, RemoteNormalizedValueV1::Bool(value)) => {
                std::sync::Arc::new(BooleanArray::from(vec![value]))
            }
            _ => return Err(RemoteChunkDispatchFailureV1::PlanMismatch),
        };
        let field_contract = descriptor
            .single_field_v1()
            .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?;
        let datatype = match expected {
            RemoteTypedScalarKindV1::Int8 => DataType::Int8,
            RemoteTypedScalarKindV1::Int16 => DataType::Int16,
            RemoteTypedScalarKindV1::Int32 => DataType::Int32,
            RemoteTypedScalarKindV1::Int64 => DataType::Int64,
            RemoteTypedScalarKindV1::UInt8 => DataType::UInt8,
            RemoteTypedScalarKindV1::UInt16 => DataType::UInt16,
            RemoteTypedScalarKindV1::UInt32 => DataType::UInt32,
            RemoteTypedScalarKindV1::UInt64 => DataType::UInt64,
            RemoteTypedScalarKindV1::Float32 => DataType::Float32,
            RemoteTypedScalarKindV1::Float64 => DataType::Float64,
            RemoteTypedScalarKindV1::Bool => DataType::Boolean,
        };
        let value: ArrayRef = std::sync::Arc::new(StructArray::new(
            Fields::from(vec![Field::new(field_contract.name_v1(), datatype, true)]),
            vec![scalar],
            None,
        ));
        let timepoint = TimePoint::from([
            (
                TimelineName::from("message_log_time"),
                TimeCell::new(
                    descriptor.time_type_v1().into(),
                    canonicalize_raw_mcap_time(RawMcapTime::new(message.log_time))
                        .map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?,
                ),
            ),
            (
                TimelineName::from("message_publish_time"),
                TimeCell::new(
                    descriptor.time_type_v1().into(),
                    canonicalize_raw_mcap_time(RawMcapTime::new(message.publish_time))
                        .map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?,
                ),
            ),
        ]);
        let row_id = row_id_from_envelope_v1(&message)?;
        builder = builder.with_row(
            row_id,
            timepoint,
            [(descriptor.component_v1().clone(), value)],
        );
    }
    while let Some(message) = messages
        .next_v1()
        .map_err(|_| RemoteChunkDispatchFailureV1::StaleSource)?
    {
        if message.channel_id == descriptor.channel_id_v1() {
            return Err(RemoteChunkDispatchFailureV1::PlanMismatch);
        }
    }
    descriptor
        .ensure_current_v1()
        .map_err(|_| RemoteChunkDispatchFailureV1::StaleSource)?;
    let chunk = builder
        .build()
        .map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?;
    Ok(RemoteTypedChunkHandoffV1 {
        chunk,
        _reservation: output_reservation,
    })
}

impl RemoteChunkTerminalV1 {
    pub(crate) const fn is_complete_v1(&self) -> bool {
        matches!(self, Self::Complete(_) | Self::CompleteEmpty)
    }
}

/// Executes the admitted second pass.  No output is returned on any mismatch or decode error;
/// the adapter is consumed and its reservation is dropped, preventing partial publication.
pub(crate) fn dispatch_admitted_v1(
    adapter: RemoteExecutableDecoderAdapterV1<'_>,
    plan: &ValidatedChunkDispatchPlanV1<'_, '_, '_, '_, '_, '_>,
) -> RemoteChunkTerminalV1 {
    if let Err(error) = plan.ensure_current_v1() {
        return RemoteChunkTerminalV1::Failed(RemoteChunkDispatchFailureV1::Validation(error));
    }
    let evidence: &PhysicalChunkMessageEvidenceV1<'_> = plan.evidence_v1();
    if evidence.ensure_current_v1().is_err() {
        return RemoteChunkTerminalV1::Failed(RemoteChunkDispatchFailureV1::StaleSource);
    }
    let expected_rows = plan.expected_rows_v1();
    let expected_payload_bytes = plan.expected_payload_bytes_v1();
    if expected_rows == 0 {
        return RemoteChunkTerminalV1::CompleteEmpty;
    }
    let result = adapter.execute_batch_v1(evidence);
    match result {
        Ok(batch)
            if batch.rows_v1() == expected_rows
                && batch.input_payload_bytes_v1() == expected_payload_bytes =>
        {
            RemoteChunkTerminalV1::Complete(batch)
        }
        Ok(_batch) => RemoteChunkTerminalV1::Failed(RemoteChunkDispatchFailureV1::PlanMismatch),
        Err(error) => RemoteChunkTerminalV1::Failed(RemoteChunkDispatchFailureV1::Decode(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::{RemoteChunkDispatchFailureV1, RemoteChunkTerminalV1, row_id_from_envelope_v1};

    #[test]
    fn row_ids_follow_canonical_source_order_and_reject_unrepresentable_offsets() {
        let binding =
            crate::remote_chunk_scan::PhysicalChunkSourceBindingV1::new_unscanned_for_assignment_test_v1();
        let first = crate::remote_chunk_scan::RemoteMessageEnvelopeV1::new_with_source_offsets_for_dispatch_test_v1(
            binding.clone(),
            9,
            12,
        );
        let second = crate::remote_chunk_scan::RemoteMessageEnvelopeV1::new_with_source_offsets_for_dispatch_test_v1(
            binding.clone(),
            9,
            13,
        );
        let later_record = crate::remote_chunk_scan::RemoteMessageEnvelopeV1::new_with_source_offsets_for_dispatch_test_v1(
            binding.clone(),
            10,
            0,
        );
        assert_eq!(
            row_id_from_envelope_v1(&first).unwrap().as_tuid().as_u128(),
            (9_u128 << 64) | (12_u128 << 16)
        );
        assert!(
            row_id_from_envelope_v1(&first).unwrap() < row_id_from_envelope_v1(&second).unwrap()
        );
        assert!(
            row_id_from_envelope_v1(&second).unwrap()
                < row_id_from_envelope_v1(&later_record).unwrap()
        );
        let too_large = crate::remote_chunk_scan::RemoteMessageEnvelopeV1::new_with_source_offsets_for_dispatch_test_v1(
            binding,
            10,
            1_u64 << 48,
        );
        assert_eq!(
            row_id_from_envelope_v1(&too_large),
            Err(RemoteChunkDispatchFailureV1::PlanMismatch)
        );
    }

    #[test]
    fn terminal_states_are_total_and_non_partial() {
        assert!(
            !RemoteChunkTerminalV1::Failed(super::RemoteChunkDispatchFailureV1::PlanMismatch)
                .is_complete_v1()
        );
        assert!(RemoteChunkTerminalV1::CompleteEmpty.is_complete_v1());
    }
}
