fn main() {
    println!("cargo::rustc-check-cfg=cfg(rerun_remote_ros2_artifact_probe_v1)");
    let attestation = re_build_tools::remote_wasm_contract::remote_ros2_artifact_attestation_v1()
        .unwrap_or_else(|err| panic!("Remote Viewer artifact attestation failed: {err:#}"));
    let out_dir = std::env::var_os("OUT_DIR").expect("Cargo did not provide OUT_DIR");
    re_build_tools::remote_wasm_contract::write_remote_ros2_generated_capability_v1(
        attestation.as_ref(),
        std::path::Path::new(&out_dir),
        re_build_tools::remote_wasm_contract::RemoteRos2ArtifactConsumerV1::Viewer,
    )
    .unwrap_or_else(|err| panic!("Failed to generate the remote Viewer capability: {err:#}"));
    if let Some(_attestation) = attestation {
        println!("cargo::rustc-cfg=rerun_remote_ros2_artifact_probe_v1");
    }
    re_build_tools::export_build_info_vars_for_crate("re_viewer");
}
