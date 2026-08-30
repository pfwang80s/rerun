#![cfg(all(target_arch = "wasm32", rerun_mcap_phase_a_proof_v1))]

use serde::Serialize;
use wasm_bindgen::prelude::*;

#[global_allocator]
static PHASE_A_ALLOCATOR_V1: re_memory::AccountingAllocator<std::alloc::System> =
    re_memory::AccountingAllocator::new(std::alloc::System);

#[path = "../../../../crates/viewer/re_viewer/src/web_remote_mcap_cpu.rs"]
mod production_remote_mcap_cpu;

const MAX_PHASE_A_OUTPUT_BYTES_V1: u64 = 64 * 1024 * 1024;

#[derive(Serialize)]
struct CpuStageResultV1 {
    input_bytes: u64,
    output_bytes: u64,
    completed_count: u64,
    retained_high_water_bytes: u64,
    max_duration_micros: u64,
    overflowed: bool,
}

fn js_error(context: &'static str, error: impl std::fmt::Debug) -> JsValue {
    JsValue::from_str(&format!("{context}: {error:?}"))
}

async fn fetch_exact(
    fixture_url: &str,
    range: std::ops::Range<u64>,
) -> Result<(re_web::chrome_byob::ExactLengthRangeBody, u64, bool), JsValue> {
    re_web::chrome_range::fetch_exact_phase_a_measurement_v1(fixture_url, range)
        .await
        .map(re_web::chrome_range::PhaseAExactRangeMeasurementV1::into_parts_v1)
        .map_err(|error| js_error("controlled Phase A Range/BYOB failed", error))
}

fn serialize_driver(
    result: production_remote_mcap_cpu::PhaseADriverMeasurementV1,
    retained_high_water_bytes: u64,
) -> Result<JsValue, JsValue> {
    let pipeline = result.pipeline;
    serde_wasm_bindgen::to_value(&CpuStageResultV1 {
        input_bytes: pipeline.input_bytes,
        output_bytes: pipeline.output_bytes,
        completed_count: pipeline.completed_count,
        retained_high_water_bytes,
        max_duration_micros: result.max_duration_micros,
        overflowed: result.overflowed,
    })
    .map_err(|error| js_error("Phase A result serialization failed", error))
}

#[wasm_bindgen]
pub async fn run_phase_a_sample_v1(
    stage: &str,
    fixture_url: &str,
    fixture_length: u64,
) -> Result<JsValue, JsValue> {
    let allocation_ledger = re_memory::InstantaneousAllocationByteLedgerV1::begin()
        .ok_or_else(|| JsValue::from_str("a Phase A allocation ledger is already active"))?;
    if fixture_length == 0 {
        return Err(JsValue::from_str("the Phase A fixture length is zero"));
    }
    if stage == "byob_copy" {
        let (body, _retained_high_water_bytes, overflowed) =
            fetch_exact(fixture_url, 0..fixture_length).await?;
        let bytes = u64::try_from(body.len())
            .map_err(|error| js_error("Phase A BYOB length overflow", error))?;
        drop(body);
        let retained_high_water_bytes = u64::try_from(allocation_ledger.high_water_bytes())
            .map_err(|error| js_error("Phase A allocation high-water overflow", error))?;
        return serde_wasm_bindgen::to_value(&CpuStageResultV1 {
            input_bytes: bytes,
            output_bytes: bytes,
            completed_count: 1,
            retained_high_water_bytes,
            max_duration_micros: 1,
            overflowed,
        })
        .map_err(|error| js_error("Phase A BYOB result serialization failed", error));
    }

    let (full_body, _full_fetch_high_water_bytes, full_fetch_overflowed) =
        fetch_exact(fixture_url, 0..fixture_length).await?;
    if full_fetch_overflowed {
        return Err(JsValue::from_str(
            "Phase A full-body byte ledger overflowed",
        ));
    }
    let kind = match stage {
        "opening_parse" => production_remote_mcap_cpu::RemoteCpuWorkKindV1::OpeningParse,
        "message_index_parse" => production_remote_mcap_cpu::RemoteCpuWorkKindV1::MessageIndexParse,
        "physical_validation" => {
            production_remote_mcap_cpu::RemoteCpuWorkKindV1::PhysicalChunkValidation
        }
        _ => return Err(JsValue::from_str("unknown Phase A CPU stage")),
    };

    let input_bytes = u64::try_from(full_body.len())
        .map_err(|error| js_error("Phase A input length overflow", error))?;
    if kind == production_remote_mcap_cpu::RemoteCpuWorkKindV1::OpeningParse {
        let result = production_remote_mcap_cpu::drive_phase_a_measurement_work_v1(
            kind,
            input_bytes,
            MAX_PHASE_A_OUTPUT_BYTES_V1,
            || {
                let owner =
                    re_mcap::phase_a_measurement::prepare_opening_stage_v1(full_body.as_slice())
                        .map_err(|_error| {
                            production_remote_mcap_cpu::RemoteCpuExecutionErrorV1::BoundViolation
                        })?;
                let measurement = owner.measurement_v1().map_err(|_error| {
                    production_remote_mcap_cpu::RemoteCpuExecutionErrorV1::BoundViolation
                })?;
                Ok((measurement, owner))
            },
        )
        .map_err(|error| js_error("Phase A CPU driver failed", error))?;
        let retained_high_water_bytes = u64::try_from(allocation_ledger.high_water_bytes())
            .map_err(|error| js_error("Phase A allocation high-water overflow", error))?;
        return serialize_driver(result, retained_high_water_bytes);
    }

    if kind == production_remote_mcap_cpu::RemoteCpuWorkKindV1::MessageIndexParse {
        let opening = re_mcap::phase_a_measurement::prepare_opening_stage_v1(full_body.as_slice())
            .map_err(|error| js_error("Phase A opening preparation failed", error))?;
        let result = production_remote_mcap_cpu::drive_phase_a_measurement_work_v1(
            kind,
            input_bytes,
            MAX_PHASE_A_OUTPUT_BYTES_V1,
            move || {
                let owner = re_mcap::phase_a_measurement::prepare_message_index_stage_v1(opening)
                    .map_err(|_error| {
                    production_remote_mcap_cpu::RemoteCpuExecutionErrorV1::BoundViolation
                })?;
                let measurement = owner.measurement_v1().map_err(|_error| {
                    production_remote_mcap_cpu::RemoteCpuExecutionErrorV1::BoundViolation
                })?;
                Ok((measurement, owner))
            },
        )
        .map_err(|error| js_error("Phase A CPU driver failed", error))?;
        let retained_high_water_bytes = u64::try_from(allocation_ledger.high_water_bytes())
            .map_err(|error| js_error("Phase A allocation high-water overflow", error))?;
        return serialize_driver(result, retained_high_water_bytes);
    }

    let opening = re_mcap::phase_a_measurement::prepare_opening_stage_v1(full_body.as_slice())
        .map_err(|error| js_error("Phase A opening preparation failed", error))?;
    let message_index = re_mcap::phase_a_measurement::prepare_message_index_stage_v1(opening)
        .map_err(|error| js_error("Phase A MessageIndex preparation failed", error))?;
    let source = re_mcap::phase_a_measurement::resolve_message_index_stage_v1(message_index)
        .map_err(|error| js_error("Phase A physical source resolution failed", error))?;
    let chunk_range = source
        .chunk_range_v1()
        .map_err(|error| js_error("Phase A physical range lookup failed", error))?;
    let (chunk_body, _chunk_fetch_high_water_bytes, chunk_fetch_overflowed) =
        fetch_exact(fixture_url, chunk_range).await?;
    if chunk_fetch_overflowed {
        return Err(JsValue::from_str("Phase A Chunk byte ledger overflowed"));
    }
    let retained_input_bytes = u64::try_from(full_body.len())
        .ok()
        .and_then(|full| {
            u64::try_from(chunk_body.len())
                .ok()
                .and_then(|chunk| full.checked_add(chunk))
        })
        .ok_or_else(|| JsValue::from_str("Phase A physical input length overflow"))?;
    let (_pair, web_permit, mcap_permit) =
        re_mcap_web_contract::CorrelationFactoryV1::new_operation_v1();
    let transport_receipt =
        re_web::transport_receipt::issue_transport_receipt_v1(chunk_body, web_permit);
    let physical_receipt = source
        .resolved_authority_v1()
        .issue_physical_receipt_v1(0, mcap_permit)
        .map_err(|error| js_error("Phase A physical receipt issue failed", error))?;
    let result = production_remote_mcap_cpu::drive_phase_a_measurement_work_v1(
        kind,
        retained_input_bytes,
        MAX_PHASE_A_OUTPUT_BYTES_V1,
        move || {
            let cache = re_mcap_web_adapter::phase_a::initiate_operation_v1(
                transport_receipt,
                physical_receipt,
            )
            .run_v1()
            .map_err(|_error| {
                production_remote_mcap_cpu::RemoteCpuExecutionErrorV1::BoundViolation
            })?
            .into_cache_v1();
            let owner = re_mcap::phase_a_measurement::complete_physical_validation_stage_v1(cache)
                .map_err(|_error| {
                    production_remote_mcap_cpu::RemoteCpuExecutionErrorV1::BoundViolation
                })?;
            let measurement = owner.measurement_v1().map_err(|_error| {
                production_remote_mcap_cpu::RemoteCpuExecutionErrorV1::BoundViolation
            })?;
            Ok((measurement, owner))
        },
    )
    .map_err(|error| js_error("Phase A physical CPU driver failed", error))?;
    let retained_high_water_bytes = u64::try_from(allocation_ledger.high_water_bytes())
        .map_err(|error| js_error("Phase A allocation high-water overflow", error))?;
    serialize_driver(result, retained_high_water_bytes)
}
