fn main() {
    println!("cargo::rustc-check-cfg=cfg(re_mcap_locked_remote_wasm_allocator_v1)");
    println!("cargo::rustc-check-cfg=cfg(rerun_mcap_phase_a_proof_v1)");
    if re_build_tools::remote_wasm_contract::remote_ros2_artifact_attestation_v1()
        .unwrap_or_else(|error| panic!("Phase A proof attestation failed: {error:#}"))
        .is_some()
    {
        println!("cargo::rustc-cfg=re_mcap_locked_remote_wasm_allocator_v1");
    }
    if re_build_tools::remote_wasm_contract::mcap_phase_a_artifact_attestation_v1()
        .unwrap_or_else(|error| panic!("Phase A proof attestation failed: {error:#}"))
        .is_some()
    {
        println!("cargo::rustc-cfg=rerun_mcap_phase_a_proof_v1");
    }
}
