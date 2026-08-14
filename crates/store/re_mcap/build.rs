fn main() {
    println!("cargo::rustc-check-cfg=cfg(re_mcap_locked_remote_wasm_allocator_v1)");
    println!("cargo::rustc-check-cfg=cfg(rerun_mcap_phase_a_proof_v1)");
    let attestation = re_build_tools::remote_wasm_contract::remote_ros2_artifact_attestation_v1()
        .unwrap_or_else(|err| panic!("Remote MCAP artifact attestation failed: {err:#}"));
    let out_dir = std::env::var_os("OUT_DIR").expect("Cargo did not provide OUT_DIR");
    re_build_tools::remote_wasm_contract::write_remote_ros2_generated_capability_v1(
        attestation.as_ref(),
        std::path::Path::new(&out_dir),
        re_build_tools::remote_wasm_contract::RemoteRos2ArtifactConsumerV1::Mcap,
    )
    .unwrap_or_else(|err| panic!("Failed to generate the remote MCAP capability: {err:#}"));
    if let Some(_attestation) = attestation {
        println!("cargo::rustc-cfg=re_mcap_locked_remote_wasm_allocator_v1");
    }
    if re_build_tools::remote_wasm_contract::mcap_phase_a_artifact_attestation_v1()
        .unwrap_or_else(|err| panic!("MCAP Phase A artifact attestation failed: {err:#}"))
        .is_some()
    {
        println!("cargo::rustc-cfg=rerun_mcap_phase_a_proof_v1");
    }
}
