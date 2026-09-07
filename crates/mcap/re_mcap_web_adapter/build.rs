const LOCKED_ATTESTATION_ENV_V1: &[&str] = &[
    "RUSTFLAGS",
    "RUSTC_BOOTSTRAP",
    "CARGO_UNSTABLE_BUILD_STD",
    "CARGO_BUILD_RUSTC",
    "CARGO_BUILD_RUSTC_WRAPPER",
    "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
    "CARGO_BUILD_RUSTFLAGS",
    "CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS",
    "CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTC",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "TARGET",
    "RUSTC",
    "RUSTC_LINKER",
    "CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_LINKER",
    "CARGO_ENCODED_RUSTFLAGS",
    "RERUN_REMOTE_ROS2_ARTIFACT_PROBE_V1",
    "PATH",
    "RUSTUP_HOME",
    "HOME",
    "CARGO_HOME",
];

fn main() {
    println!("cargo::rustc-check-cfg=cfg(re_mcap_web_adapter_locked_attested_v1)");

    // This pinned manifest mirrors the verifier contract inputs and is audited as one source of
    // truth. It is intentionally explicit rather than dynamically discovered by a shell script.
    for variable in LOCKED_ATTESTATION_ENV_V1 {
        println!("cargo::rerun-if-env-changed={variable}");
    }

    let locked_feature_enabled = std::env::var_os("CARGO_FEATURE_LOCKED").is_some();
    let target_is_wasm32 = std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32");

    if locked_feature_enabled && target_is_wasm32 {
        let attestation =
            re_build_tools::remote_wasm_contract::remote_ros2_artifact_attestation_v1()
                .unwrap_or_else(|err| {
                    panic!("Remote MCAP adapter locked attestation failed: {err:#}")
                });
        assert!(
            attestation.is_some(),
            "Remote MCAP adapter locked profile requires an external artifact attestation"
        );
        println!("cargo::rustc-cfg=re_mcap_web_adapter_locked_attested_v1");
        println!(
            "cargo:warning=external attestation accepted for re_mcap_web_adapter locked profile"
        );
    }
}
