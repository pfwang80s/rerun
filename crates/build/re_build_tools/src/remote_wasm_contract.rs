//! Locked build contract for the remote ROS 2 Web artifact verifier.
//!
//! The environment capability only requests the verifier build. It is not an attestation: every
//! consuming build script must call [`remote_ros2_artifact_attestation_v1`] and can only enable its
//! private code after receiving the sealed result.

use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context as _;
use sha2::{Digest as _, Sha256};

/// Build-scoped request for the remote ROS 2 verifier artifact.
pub const REMOTE_ROS2_ARTIFACT_PROBE_ENV_V1: &str = "RERUN_REMOTE_ROS2_ARTIFACT_PROBE_V1";

/// Additional build-scoped request for the MCAP Phase A measurement artifact.
///
/// This never enables measurement code by itself: every final consumer must also validate the
/// locked remote-Wasm attestation before emitting its private proof cfg.
pub const MCAP_PHASE_A_PROOF_ENV_V1: &str = "RERUN_MCAP_PHASE_A_PROOF_V1";

const CANONICAL_WEB_RUSTFLAGS_V1: [&str; 3] = [
    "--cfg=web_sys_unstable_apis",
    "--cfg=getrandom_backend=\"wasm_js\"",
    "-Ctarget-feature=+simd128,+bulk-memory,+nontrapping-fptoint,+multivalue",
];
const REMOTE_ROS2_STACK_EXPORT_RUSTFLAG_V1: &str = "-Clink-arg=--export=__stack_pointer";
const LOCKED_RUST_RELEASE_V1: &str = "1.95.0";
const LOCKED_RUST_COMMIT_V1: &str = "59807616e1fa2540724bfbac14d7976d7e4a3860";
const LOCKED_RUST_HOST_V1: &str = "x86_64-unknown-linux-gnu";
const LOCKED_RUSTC_DIGEST_V1: &str =
    "bff349e72704ff70bc08a234a3847338e797065bbedde5e556808bc87b7bf7c6";
const LOCKED_CARGO_DIGEST_V1: &str =
    "841072d1d92f9e841d9ba5b0814182a0adf064acf4527cd120967b7bc49dcb66";
const LOCKED_RUST_LLD_DIGEST_V1: &str =
    "60f305b55da767671e895b231e0c78e87f4ccbf790d7181842039c53bb60281c";
const LOCKED_SYSROOT_FILES_V1: [(&str, &str); 5] = [
    (
        "lib/rustlib/manifest-rust-std-wasm32-unknown-unknown",
        "a8ff3d81e2ec92509e6d3db338e2ebfec1492a71e483f8715779625af05e4ba7",
    ),
    (
        "lib/rustlib/wasm32-unknown-unknown/lib/libstd-2c19544a9276d693.rlib",
        "f20c9cacffeae7852c761769587ccf5b24c3e3efc034fac7c8501f271d268b03",
    ),
    (
        "lib/rustlib/wasm32-unknown-unknown/lib/liballoc-497b1066d2ceb8c1.rlib",
        "ff388e477d4598af1113f4cf79ccb9db6871eb89a7c70782e908ddc449e22cf0",
    ),
    (
        "lib/rustlib/wasm32-unknown-unknown/lib/libdlmalloc-fd0a31467a15ee47.rlib",
        "b843df7e5fa86af802740e73579cdc2da473f8b1fa74cc2e653cab295784ef8a",
    ),
    (
        "lib/rustlib/x86_64-unknown-linux-gnu/bin/rust-lld",
        LOCKED_RUST_LLD_DIGEST_V1,
    ),
];

const FORBIDDEN_VERIFIER_ENV_V1: [&str; 9] = [
    "RUSTFLAGS",
    "RUSTC_BOOTSTRAP",
    "CARGO_UNSTABLE_BUILD_STD",
    "CARGO_BUILD_RUSTC",
    "CARGO_BUILD_RUSTC_WRAPPER",
    "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
    "CARGO_BUILD_RUSTFLAGS",
    "CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS",
    "CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTC",
];

// Cargo exposes an explicitly disabled wrapper to build scripts as a present, empty environment
// value. That is distinct from every non-empty value, including whitespace, which would name an
// executable if it reached a compiler invocation.
const CARGO_DISABLED_RUSTC_WRAPPER_ENV_V1: [&str; 2] = ["RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"];

const MCAP_GENERATED_CAPABILITY_FILE_V1: &str = "re_mcap_remote_ros2_generated_capability_v1.rs";
const VIEWER_GENERATED_CAPABILITY_FILE_V1: &str =
    "re_viewer_remote_ros2_generated_capability_v1.rs";
const DISABLE_RUSTC_WRAPPER_CONFIG_V1: &str = "build.rustc-wrapper=\"\"";
const DISABLE_RUSTC_WORKSPACE_WRAPPER_CONFIG_V1: &str = "build.rustc-workspace-wrapper=\"\"";

/// A sealed proof that the actual compiler and sysroot match the locked V1 artifact contract.
pub struct RemoteWasmToolchainAttestationV1 {
    canonical_rustc: PathBuf,
    canonical_cargo: PathBuf,
    canonical_rust_lld: PathBuf,
}

impl RemoteWasmToolchainAttestationV1 {
    /// Exact compiler which must perform every verifier compilation.
    pub fn canonical_rustc_v1(&self) -> &Path {
        &self.canonical_rustc
    }

    /// Exact Cargo binary used to launch the nested verifier build.
    pub fn canonical_cargo_v1(&self) -> &Path {
        &self.canonical_cargo
    }

    /// Exact `rust-lld` injected for the verifier's Wasm target.
    pub fn canonical_rust_lld_v1(&self) -> &Path {
        &self.canonical_rust_lld
    }

    fn canonical_toolchain_bin_v1(&self) -> &Path {
        self.canonical_rustc
            .parent()
            .expect("the attested compiler always has a sysroot bin parent")
    }
}

/// A sealed proof created independently by each consuming build script.
pub struct RemoteRos2ArtifactAttestationV1 {
    _toolchain: RemoteWasmToolchainAttestationV1,
}

/// A sealed proof that the independently requested MCAP Phase A artifact uses the locked
/// remote-Wasm toolchain contract.
pub struct McapPhaseAArtifactAttestationV1 {
    _sealed: RemoteRos2ArtifactAttestationV1,
}

/// Final Cargo consumer which receives a generated, attestation-bound source capability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteRos2ArtifactConsumerV1 {
    /// The bounded remote ROS 2 initializer in `re_mcap`.
    Mcap,

    /// The final Web allocator and initializer probes in `re_viewer`.
    Viewer,
}

impl RemoteRos2ArtifactConsumerV1 {
    fn generated_file_name(self) -> &'static str {
        match self {
            Self::Mcap => MCAP_GENERATED_CAPABILITY_FILE_V1,
            Self::Viewer => VIEWER_GENERATED_CAPABILITY_FILE_V1,
        }
    }

    fn generated_source(self, attested: bool) -> &'static str {
        if !attested {
            return "compile_error!(\"remote ROS 2 artifact cfg lacks its attested generated capability\");\n";
        }
        match self {
            Self::Mcap => {
                "#[derive(Clone, Copy)]\n\
                 pub(crate) struct ReMcapRemoteRos2GeneratedCapabilityV1;\n\
                 pub(crate) const RE_MCAP_REMOTE_ROS2_GENERATED_CAPABILITY_V1: ReMcapRemoteRos2GeneratedCapabilityV1 = ReMcapRemoteRos2GeneratedCapabilityV1;\n"
            }
            Self::Viewer => {
                "#[derive(Clone, Copy)]\n\
                 pub(crate) struct ReViewerRemoteRos2GeneratedCapabilityV1;\n\
                 pub(crate) const RE_VIEWER_REMOTE_ROS2_GENERATED_CAPABILITY_V1: ReViewerRemoteRos2GeneratedCapabilityV1 = ReViewerRemoteRos2GeneratedCapabilityV1;\n"
            }
        }
    }
}

/// Whether this build-tool host owns the canonical V1 executable proof artifact.
///
/// Existing release workflows also invoke the Web builder on Windows, macOS ARM, and Linux ARM.
/// Those builds continue producing and ABI-checking the production artifact, but must not pretend
/// that the Linux-x64 compiler-body attestation applies to a different host executable.
pub const fn is_canonical_remote_ros2_probe_host_v1() -> bool {
    cfg!(all(target_os = "linux", target_arch = "x86_64"))
}

/// Returns the exact encoded flags for an ordinary product or verifier build.
pub fn canonical_web_rustflags_v1(probe: bool) -> String {
    let mut flags = CANONICAL_WEB_RUSTFLAGS_V1.to_vec();
    if probe {
        flags.push(REMOTE_ROS2_STACK_EXPORT_RUSTFLAG_V1);
    }
    flags.join("\u{1f}")
}

/// Installs the three existing product Wasm flags without changing Cargo/toolchain integration.
///
/// Product builds deliberately preserve their caller's Cargo home, registry authentication,
/// compiler wrappers, linker, compiler, and `PATH`. Only conflicting flag encodings and the
/// private verifier request are removed.
pub fn configure_product_wasm_command_v1(command: &mut Command) {
    for variable in [
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        REMOTE_ROS2_ARTIFACT_PROBE_ENV_V1,
        MCAP_PHASE_A_PROOF_ENV_V1,
    ] {
        command.env_remove(variable);
    }
    command.env("CARGO_ENCODED_RUSTFLAGS", canonical_web_rustflags_v1(false));
}

/// Hardens the private verifier child around the exact attested compiler and linker.
pub fn configure_remote_ros2_verifier_command_v1(
    command: &mut Command,
    toolchain: &RemoteWasmToolchainAttestationV1,
) -> anyhow::Result<()> {
    configure_remote_ros2_verifier_command_with_path_v1(
        command,
        toolchain,
        std::env::var_os("PATH"),
    )
}

/// Hardens the private MCAP Phase A child and adds its independent measurement request.
pub fn configure_mcap_phase_a_proof_command_v1(
    command: &mut Command,
    toolchain: &RemoteWasmToolchainAttestationV1,
) -> anyhow::Result<()> {
    configure_remote_ros2_verifier_command_with_path_v1(
        command,
        toolchain,
        std::env::var_os("PATH"),
    )?;
    configure_mcap_phase_a_proof_env_v1(command, toolchain)
}

/// Installs the hardened private MCAP Phase A environment without Cargo arguments.
///
/// A caller which launches a tool that forwards its own Cargo invocation (`wasm-pack test`, for
/// example) cannot hand that tool Cargo's `--config` arguments. Such a caller puts the attested
/// environment in place with this function and lets the tool build the child command.
pub fn configure_mcap_phase_a_proof_env_v1(
    command: &mut Command,
    toolchain: &RemoteWasmToolchainAttestationV1,
) -> anyhow::Result<()> {
    configure_remote_ros2_verifier_env_with_path_v1(command, toolchain, std::env::var_os("PATH"))?;
    command.env_remove(REMOTE_ROS2_ARTIFACT_PROBE_ENV_V1);
    command.env(MCAP_PHASE_A_PROOF_ENV_V1, "1");
    Ok(())
}

/// Disables Cargo's compiler wrappers for a command that runs Cargo itself.
fn configure_disabled_rustc_wrappers_v1(command: &mut Command) {
    command.args([
        "--config",
        DISABLE_RUSTC_WRAPPER_CONFIG_V1,
        "--config",
        DISABLE_RUSTC_WORKSPACE_WRAPPER_CONFIG_V1,
    ]);
}

fn configure_remote_ros2_verifier_command_with_path_v1(
    command: &mut Command,
    toolchain: &RemoteWasmToolchainAttestationV1,
    ambient_path: Option<OsString>,
) -> anyhow::Result<()> {
    configure_remote_ros2_verifier_env_with_path_v1(command, toolchain, ambient_path)?;
    configure_disabled_rustc_wrappers_v1(command);
    Ok(())
}

fn configure_remote_ros2_verifier_env_with_path_v1(
    command: &mut Command,
    toolchain: &RemoteWasmToolchainAttestationV1,
    ambient_path: Option<OsString>,
) -> anyhow::Result<()> {
    for variable in FORBIDDEN_VERIFIER_ENV_V1
        .into_iter()
        .chain(CARGO_DISABLED_RUSTC_WRAPPER_ENV_V1)
        .chain([
            "CARGO_ENCODED_RUSTFLAGS",
            "RUSTC",
            "RUSTC_LINKER",
            "CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_LINKER",
            REMOTE_ROS2_ARTIFACT_PROBE_ENV_V1,
            MCAP_PHASE_A_PROOF_ENV_V1,
        ])
    {
        command.env_remove(variable);
    }
    command.env("CARGO_ENCODED_RUSTFLAGS", canonical_web_rustflags_v1(true));
    command.env(REMOTE_ROS2_ARTIFACT_PROBE_ENV_V1, "1");
    command.env("RUSTC", toolchain.canonical_rustc_v1());
    command.env(
        "CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_LINKER",
        toolchain.canonical_rust_lld_v1(),
    );

    let mut path = vec![toolchain.canonical_toolchain_bin_v1().to_owned()];
    if let Some(ambient) = ambient_path {
        path.extend(std::env::split_paths(&ambient));
    }
    let path = std::env::join_paths(path)
        .context("Failed to construct the canonical verifier tool PATH")?;
    command.env("PATH", path);
    Ok(())
}

/// Writes the fixed source capability consumed by one final Cargo boundary.
///
/// The file is always written. Ordinary builds therefore install a fail-closed source file, so a
/// caller cannot enable the private code by injecting its plain cfg after the build script ran.
pub fn write_remote_ros2_generated_capability_v1(
    attestation: Option<&RemoteRos2ArtifactAttestationV1>,
    out_dir: &Path,
    consumer: RemoteRos2ArtifactConsumerV1,
) -> anyhow::Result<PathBuf> {
    let path = out_dir.join(consumer.generated_file_name());
    std::fs::create_dir_all(out_dir).with_context(|| {
        format!(
            "Failed to create the remote ROS 2 capability directory\nFile path: {}",
            out_dir.display()
        )
    })?;
    std::fs::write(&path, consumer.generated_source(attestation.is_some())).with_context(|| {
        format!(
            "Failed to write the remote ROS 2 generated capability\nFile path: {}",
            path.display()
        )
    })?;
    Ok(path)
}

/// Verifies the compiler that the canonical Web builder is about to use.
///
/// Consuming build scripts perform the same verification again after Cargo has resolved all of
/// its config and environment. This first check therefore cannot be used to bypass the consumer
/// boundary.
pub fn locked_remote_wasm_toolchain_attestation_v1(
    rustc_discovery: &OsStr,
) -> anyhow::Result<RemoteWasmToolchainAttestationV1> {
    let discovery = resolve_executable(rustc_discovery)
        .context("Failed to resolve the Rust sysroot discovery command")?;
    // A rustup proxy is used only to discover a candidate sysroot. Its version output and compile
    // behavior are never trusted and it is never installed in the nested verifier command.
    let sysroot = Command::new(&discovery)
        .args(["--print", "sysroot"])
        .output()
        .context("Failed to discover the locked Rust sysroot")?;
    anyhow::ensure!(
        sysroot.status.success(),
        "Failed to discover the locked Rust sysroot"
    );
    let sysroot =
        String::from_utf8(sysroot.stdout).context("Rust sysroot path was not valid UTF-8")?;
    let canonical_sysroot = std::fs::canonicalize(sysroot.trim())
        .context("Failed to canonicalize the locked Rust sysroot")?;
    let canonical_rustc = std::fs::canonicalize(
        canonical_sysroot
            .join("bin")
            .join(format!("rustc{}", std::env::consts::EXE_SUFFIX)),
    )
    .context("Failed to canonicalize the locked sysroot compiler")?;
    let canonical_cargo = std::fs::canonicalize(
        canonical_sysroot
            .join("bin")
            .join(format!("cargo{}", std::env::consts::EXE_SUFFIX)),
    )
    .context("Failed to canonicalize the locked sysroot Cargo")?;
    let canonical_rust_lld = std::fs::canonicalize(
        canonical_sysroot
            .join("lib")
            .join("rustlib")
            .join(LOCKED_RUST_HOST_V1)
            .join("bin")
            .join(format!("rust-lld{}", std::env::consts::EXE_SUFFIX)),
    )
    .context("Failed to canonicalize the locked sysroot rust-lld")?;

    verify_file_digest(&canonical_rustc, LOCKED_RUSTC_DIGEST_V1)?;
    verify_file_digest(&canonical_cargo, LOCKED_CARGO_DIGEST_V1)?;
    verify_file_digest(&canonical_rust_lld, LOCKED_RUST_LLD_DIGEST_V1)?;
    verify_locked_sysroot_files_v1(&canonical_sysroot, &LOCKED_SYSROOT_FILES_V1)?;
    verify_canonical_rustc_identity_v1(&canonical_rustc, &canonical_sysroot)?;

    Ok(RemoteWasmToolchainAttestationV1 {
        canonical_rustc,
        canonical_cargo,
        canonical_rust_lld,
    })
}

fn verify_canonical_rustc_identity_v1(rustc: &Path, sysroot: &Path) -> anyhow::Result<()> {
    let verbose = Command::new(rustc)
        .arg("-vV")
        .output()
        .context("Failed to inspect the canonical Rust compiler")?;
    anyhow::ensure!(
        verbose.status.success(),
        "Failed to inspect the canonical Rust compiler"
    );
    let verbose = String::from_utf8(verbose.stdout)
        .context("Rust compiler version output was not valid UTF-8")?;
    anyhow::ensure!(
        verbose
            .lines()
            .any(|line| line == format!("release: {LOCKED_RUST_RELEASE_V1}"))
            && verbose
                .lines()
                .any(|line| line == format!("commit-hash: {LOCKED_RUST_COMMIT_V1}"))
            && verbose
                .lines()
                .any(|line| line == format!("host: {LOCKED_RUST_HOST_V1}")),
        "Remote MCAP allocator accounting requires the locked Rust {LOCKED_RUST_RELEASE_V1} compiler artifact"
    );
    let reported_sysroot = Command::new(rustc)
        .args(["--print", "sysroot"])
        .output()
        .context("Failed to inspect the canonical Rust sysroot")?;
    anyhow::ensure!(
        reported_sysroot.status.success(),
        "Failed to inspect the canonical Rust sysroot"
    );
    let reported_sysroot = String::from_utf8(reported_sysroot.stdout)
        .context("Canonical Rust sysroot path was not valid UTF-8")?;
    let reported_sysroot = std::fs::canonicalize(reported_sysroot.trim())
        .context("Failed to canonicalize the compiler-reported Rust sysroot")?;
    anyhow::ensure!(
        reported_sysroot == sysroot,
        "The canonical Rust compiler reported a different sysroot"
    );
    Ok(())
}

/// Validates the private probe request at the final Cargo consumer boundary.
///
/// `Ok(None)` is the ordinary build path. If the probe request is present, all compiler inputs are
/// verified before the returned sealed value can be used to emit a private cfg.
pub fn remote_ros2_artifact_attestation_v1()
-> anyhow::Result<Option<RemoteRos2ArtifactAttestationV1>> {
    track_probe_environment_v1();
    if std::env::var(REMOTE_ROS2_ARTIFACT_PROBE_ENV_V1).as_deref() != Ok("1") {
        return Ok(None);
    }
    locked_remote_wasm_consumer_attestation_v1().map(Some)
}

fn locked_remote_wasm_consumer_attestation_v1() -> anyhow::Result<RemoteRos2ArtifactAttestationV1> {
    anyhow::ensure!(
        std::env::var("TARGET").as_deref() == Ok("wasm32-unknown-unknown"),
        "The locked remote-Wasm artifact is only valid for wasm32-unknown-unknown"
    );
    let rustc = std::env::var_os("RUSTC")
        .context("Cargo did not provide the Rust compiler at the consumer boundary")?;
    let toolchain = locked_remote_wasm_toolchain_attestation_v1(&rustc)?;
    let actual_rustc = std::fs::canonicalize(resolve_executable(&rustc)?)
        .context("Failed to canonicalize the consumer Rust compiler")?;
    anyhow::ensure!(
        actual_rustc == toolchain.canonical_rustc,
        "The verifier consumer must execute the canonical sysroot compiler directly"
    );
    validate_probe_environment_v1(
        |name| std::env::var_os(name),
        toolchain.canonical_rust_lld_v1(),
    )?;
    Ok(RemoteRos2ArtifactAttestationV1 {
        _toolchain: toolchain,
    })
}

/// Validates the independent MCAP Phase A request in addition to the locked remote-Wasm
/// attestation.
pub fn mcap_phase_a_artifact_attestation_v1()
-> anyhow::Result<Option<McapPhaseAArtifactAttestationV1>> {
    println!("cargo::rerun-if-env-changed={MCAP_PHASE_A_PROOF_ENV_V1}");
    if std::env::var(MCAP_PHASE_A_PROOF_ENV_V1).as_deref() != Ok("1") {
        return Ok(None);
    }
    track_probe_environment_v1();
    let sealed = locked_remote_wasm_consumer_attestation_v1()?;
    Ok(Some(McapPhaseAArtifactAttestationV1 { _sealed: sealed }))
}

fn track_probe_environment_v1() {
    for variable in FORBIDDEN_VERIFIER_ENV_V1
        .into_iter()
        .chain(CARGO_DISABLED_RUSTC_WRAPPER_ENV_V1)
        .chain([
            "TARGET",
            "RUSTC",
            "RUSTC_LINKER",
            "CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_LINKER",
            "CARGO_ENCODED_RUSTFLAGS",
            REMOTE_ROS2_ARTIFACT_PROBE_ENV_V1,
        ])
    {
        println!("cargo::rerun-if-env-changed={variable}");
    }
}

fn validate_probe_environment_v1(
    mut get: impl FnMut(&str) -> Option<OsString>,
    canonical_rust_lld: &Path,
) -> anyhow::Result<()> {
    for variable in FORBIDDEN_VERIFIER_ENV_V1 {
        anyhow::ensure!(
            get(variable).is_none(),
            "Remote MCAP allocator accounting rejects compiler override {variable}"
        );
    }
    for variable in CARGO_DISABLED_RUSTC_WRAPPER_ENV_V1 {
        anyhow::ensure!(
            get(variable).is_none_or(|value| value.is_empty()),
            "Remote MCAP allocator accounting rejects non-empty compiler wrapper {variable}"
        );
    }
    let encoded = get("CARGO_ENCODED_RUSTFLAGS")
        .context("Remote MCAP allocator accounting requires exact encoded Rust flags")?;
    anyhow::ensure!(
        encoded == OsString::from(canonical_web_rustflags_v1(true)),
        "Remote MCAP allocator accounting rejects non-canonical Rust flags"
    );
    let expected_linker = canonical_rust_lld.as_os_str();
    for variable in ["RUSTC_LINKER", "CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_LINKER"] {
        anyhow::ensure!(
            get(variable).as_deref() == Some(expected_linker),
            "Remote MCAP allocator accounting requires the canonical rust-lld in {variable}"
        );
    }
    Ok(())
}

fn resolve_executable(executable: &OsStr) -> anyhow::Result<PathBuf> {
    let path = Path::new(executable);
    if path.components().count() > 1 || path.is_absolute() {
        let absolute = if path.is_absolute() {
            path.to_owned()
        } else {
            std::env::current_dir()
                .context("Failed to locate the current directory")?
                .join(path)
        };
        anyhow::ensure!(
            absolute.is_file(),
            "Failed to find the Rust compiler\nFile path: {}",
            absolute.display()
        );
        return Ok(absolute);
    }
    let path_environment = std::env::var_os("PATH").context("PATH is not set")?;
    for directory in std::env::split_paths(&path_environment) {
        let candidate = directory.join(path);
        if candidate.is_file() {
            return if candidate.is_absolute() {
                Ok(candidate)
            } else {
                Ok(std::env::current_dir()
                    .context("Failed to locate the current directory")?
                    .join(candidate))
            };
        }
    }
    anyhow::bail!("Failed to find the Rust compiler on PATH")
}

fn verify_locked_sysroot_files_v1(root: &Path, expected: &[(&str, &str)]) -> anyhow::Result<()> {
    for (relative_path, expected_digest) in expected {
        verify_file_digest(&root.join(relative_path), expected_digest)?;
    }
    Ok(())
}

fn verify_file_digest(path: &Path, expected: &str) -> anyhow::Result<()> {
    let mut file = std::fs::File::open(path).with_context(|| {
        format!(
            "Failed to verify the locked remote MCAP artifact\nFile path: {}",
            path.display()
        )
    })?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = file.read(&mut buffer).with_context(|| {
            format!(
                "Failed to verify the locked remote MCAP artifact\nFile path: {}",
                path.display()
            )
        })?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    let digest = digest.finalize();
    let mut actual = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut actual, "{byte:02x}").expect("writing to String cannot fail");
    }
    anyhow::ensure!(
        actual == expected,
        "Remote MCAP allocator accounting requires the official locked artifact\nFile path: {}",
        path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn fake_toolchain(root: &Path) -> RemoteWasmToolchainAttestationV1 {
        RemoteWasmToolchainAttestationV1 {
            canonical_rustc: root.join("bin/rustc"),
            canonical_cargo: root.join("bin/cargo"),
            canonical_rust_lld: root.join("lib/rustlib/host/bin/rust-lld"),
        }
    }

    fn canonical_environment(linker: &Path) -> BTreeMap<&'static str, OsString> {
        BTreeMap::from([
            (
                "CARGO_ENCODED_RUSTFLAGS",
                OsString::from(canonical_web_rustflags_v1(true)),
            ),
            ("RUSTC_LINKER", linker.as_os_str().to_owned()),
            (
                "CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_LINKER",
                linker.as_os_str().to_owned(),
            ),
        ])
    }

    #[test]
    fn consumer_boundary_rejects_every_compiler_spoof_surface() {
        let linker = Path::new("/locked/rust-lld");
        for variable in FORBIDDEN_VERIFIER_ENV_V1 {
            let mut environment = canonical_environment(linker);
            environment.insert(variable, OsString::from("spoof"));
            let error =
                validate_probe_environment_v1(|name| environment.get(name).cloned(), linker)
                    .expect_err(variable);
            assert!(error.to_string().contains(variable));
        }
        for variable in ["RUSTC_LINKER", "CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_LINKER"] {
            let mut environment = canonical_environment(linker);
            environment.insert(variable, OsString::from("spoof"));
            let error =
                validate_probe_environment_v1(|name| environment.get(name).cloned(), linker)
                    .expect_err(variable);
            assert!(error.to_string().contains(variable));
        }
    }

    #[test]
    fn consumer_boundary_accepts_only_absent_or_exactly_empty_rustc_wrappers() {
        let linker = Path::new("/locked/rust-lld");

        let absent = canonical_environment(linker);
        validate_probe_environment_v1(|name| absent.get(name).cloned(), linker).unwrap();

        let mut empty = canonical_environment(linker);
        for variable in CARGO_DISABLED_RUSTC_WRAPPER_ENV_V1 {
            empty.insert(variable, OsString::new());
        }
        validate_probe_environment_v1(|name| empty.get(name).cloned(), linker).unwrap();

        for variable in CARGO_DISABLED_RUSTC_WRAPPER_ENV_V1 {
            for rejected in [OsString::from("spoof"), OsString::from(" \t")] {
                let mut environment = canonical_environment(linker);
                environment.insert(variable, rejected);
                let error =
                    validate_probe_environment_v1(|name| environment.get(name).cloned(), linker)
                        .expect_err(variable);
                assert!(error.to_string().contains(variable));
            }
        }
    }

    #[test]
    fn consumer_boundary_requires_exact_encoded_flags() {
        let linker = Path::new("/locked/rust-lld");
        let mut environment = canonical_environment(linker);
        validate_probe_environment_v1(|name| environment.get(name).cloned(), linker).unwrap();

        environment.remove("CARGO_ENCODED_RUSTFLAGS");
        assert!(
            validate_probe_environment_v1(|name| environment.get(name).cloned(), linker).is_err()
        );
        environment.insert(
            "CARGO_ENCODED_RUSTFLAGS",
            OsString::from(format!(
                "{}\u{1f}-Copt-level=0",
                canonical_web_rustflags_v1(true)
            )),
        );
        assert!(
            validate_probe_environment_v1(|name| environment.get(name).cloned(), linker).is_err()
        );
    }

    #[test]
    fn locked_sysroot_verifier_is_exact() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("artifact");
        std::fs::write(&path, b"locked").unwrap();
        let digest = {
            let mut digest = Sha256::new();
            digest.update(b"locked");
            let mut encoded = String::new();
            for byte in digest.finalize() {
                write!(&mut encoded, "{byte:02x}").unwrap();
            }
            encoded
        };
        verify_locked_sysroot_files_v1(directory.path(), &[("artifact", &digest)]).unwrap();
        std::fs::write(path, b"spoof").unwrap();
        assert!(
            verify_locked_sysroot_files_v1(directory.path(), &[("artifact", &digest)]).is_err()
        );
    }

    #[test]
    fn verifier_command_removes_overrides_and_installs_canonical_compiler_and_linker() {
        let toolchain = fake_toolchain(Path::new("/locked/sysroot"));
        let mut command = Command::new("cargo");
        for variable in FORBIDDEN_VERIFIER_ENV_V1
            .into_iter()
            .chain(CARGO_DISABLED_RUSTC_WRAPPER_ENV_V1)
            .chain([
                "CARGO_HOME",
                "CARGO_ENCODED_RUSTFLAGS",
                "RUSTC",
                "RUSTC_LINKER",
                "CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_LINKER",
            ])
        {
            command.env(variable, "spoof");
        }
        configure_remote_ros2_verifier_command_v1(&mut command, &toolchain).unwrap();
        let environment = command
            .get_envs()
            .map(|(name, value)| (name.to_owned(), value.map(OsStr::to_owned)))
            .collect::<BTreeMap<_, _>>();
        for variable in FORBIDDEN_VERIFIER_ENV_V1
            .into_iter()
            .chain(CARGO_DISABLED_RUSTC_WRAPPER_ENV_V1)
            .chain(["RUSTC_LINKER"])
        {
            assert_eq!(environment.get(OsStr::new(variable)), Some(&None));
        }
        assert_eq!(
            environment.get(OsStr::new("CARGO_HOME")),
            Some(&Some(OsString::from("spoof")))
        );
        assert_eq!(
            environment.get(OsStr::new("CARGO_ENCODED_RUSTFLAGS")),
            Some(&Some(OsString::from(canonical_web_rustflags_v1(true))))
        );
        assert_eq!(
            environment.get(OsStr::new(REMOTE_ROS2_ARTIFACT_PROBE_ENV_V1)),
            Some(&Some(OsString::from("1")))
        );
        assert_eq!(
            environment.get(OsStr::new("RUSTC")),
            Some(&Some(toolchain.canonical_rustc_v1().as_os_str().to_owned()))
        );
        assert_eq!(
            environment.get(OsStr::new("CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_LINKER")),
            Some(&Some(
                toolchain.canonical_rust_lld_v1().as_os_str().to_owned()
            ))
        );
        let path = environment
            .get(OsStr::new("PATH"))
            .and_then(Option::as_ref)
            .unwrap();
        assert_eq!(
            std::env::split_paths(path).next().as_deref(),
            Some(toolchain.canonical_toolchain_bin_v1())
        );
        let arguments = command.get_args().collect::<Vec<_>>();
        assert_eq!(
            arguments,
            [
                OsStr::new("--config"),
                OsStr::new(DISABLE_RUSTC_WRAPPER_CONFIG_V1),
                OsStr::new("--config"),
                OsStr::new(DISABLE_RUSTC_WORKSPACE_WRAPPER_CONFIG_V1),
            ]
        );
    }

    #[test]
    fn phase_a_proof_request_is_independent_from_the_ros2_probe_request() {
        let toolchain = fake_toolchain(Path::new("/locked/sysroot"));
        let mut command = Command::new("cargo");
        command.env(REMOTE_ROS2_ARTIFACT_PROBE_ENV_V1, "spoof");
        configure_mcap_phase_a_proof_command_v1(&mut command, &toolchain).unwrap();
        let environment = command
            .get_envs()
            .map(|(name, value)| (name.to_owned(), value.map(OsStr::to_owned)))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            environment.get(OsStr::new(REMOTE_ROS2_ARTIFACT_PROBE_ENV_V1)),
            Some(&None)
        );
        assert_eq!(
            environment.get(OsStr::new(MCAP_PHASE_A_PROOF_ENV_V1)),
            Some(&Some(OsString::from("1")))
        );
    }

    /// The environment-only variant exists for callers that run a tool forwarding its own Cargo
    /// invocation, so it has to install the attested environment without adding arguments of its
    /// own.
    #[test]
    fn phase_a_proof_env_only_variant_installs_the_environment_without_arguments() {
        let toolchain = fake_toolchain(Path::new("/locked/sysroot"));
        let mut command = Command::new("wasm-pack");
        command.env(REMOTE_ROS2_ARTIFACT_PROBE_ENV_V1, "spoof");
        command.env("RUSTFLAGS", "spoof");
        configure_mcap_phase_a_proof_env_v1(&mut command, &toolchain).unwrap();

        assert_eq!(
            command.get_args().count(),
            0,
            "the environment-only variant must not add Cargo arguments"
        );
        let environment = command
            .get_envs()
            .map(|(name, value)| (name.to_owned(), value.map(OsStr::to_owned)))
            .collect::<BTreeMap<_, _>>();
        for (name, value) in canonical_environment(toolchain.canonical_rust_lld_v1()) {
            // Cargo derives `RUSTC_LINKER` for build scripts from the target linker, so this variant
            // removes it and installs the target linker only.
            if name == "RUSTC_LINKER" {
                assert_eq!(environment.get(OsStr::new(name)), Some(&None));
                continue;
            }
            assert_eq!(environment.get(OsStr::new(name)), Some(&Some(value)));
        }
        assert_eq!(
            environment.get(OsStr::new("RUSTC")),
            Some(&Some(toolchain.canonical_rustc_v1().as_os_str().to_owned()))
        );
        assert_eq!(environment.get(OsStr::new("RUSTFLAGS")), Some(&None));
        assert_eq!(
            environment.get(OsStr::new(REMOTE_ROS2_ARTIFACT_PROBE_ENV_V1)),
            Some(&None)
        );
        assert_eq!(
            environment.get(OsStr::new(MCAP_PHASE_A_PROOF_ENV_V1)),
            Some(&Some(OsString::from("1")))
        );
        let path = environment
            .get(OsStr::new("PATH"))
            .and_then(Option::as_ref)
            .expect("the canonical toolchain directory leads PATH");
        assert_eq!(
            std::env::split_paths(path).next().as_deref(),
            Some(toolchain.canonical_toolchain_bin_v1())
        );
    }

    #[test]
    fn product_command_preserves_cargo_home_wrappers_compiler_and_linker() {
        let mut command = Command::new("cargo");
        let passthrough = [
            ("CARGO_HOME", "/custom/cargo-home"),
            ("RUSTC", "/custom/rustc"),
            ("RUSTC_WRAPPER", "/custom/wrapper"),
            ("RUSTC_WORKSPACE_WRAPPER", "/custom/workspace-wrapper"),
            ("RUSTC_LINKER", "/custom/linker"),
            (
                "CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_LINKER",
                "/custom/target-linker",
            ),
            ("PATH", "/custom/bin"),
        ];
        for (name, value) in passthrough {
            command.env(name, value);
        }
        command.env("RUSTFLAGS", "spoof");
        configure_product_wasm_command_v1(&mut command);
        let environment = command
            .get_envs()
            .map(|(name, value)| (name.to_owned(), value.map(OsStr::to_owned)))
            .collect::<BTreeMap<_, _>>();
        for (name, value) in passthrough {
            assert_eq!(
                environment.get(OsStr::new(name)),
                Some(&Some(OsString::from(value)))
            );
        }
        assert_eq!(environment.get(OsStr::new("RUSTFLAGS")), Some(&None));
        assert_eq!(
            environment.get(OsStr::new("CARGO_ENCODED_RUSTFLAGS")),
            Some(&Some(OsString::from(canonical_web_rustflags_v1(false))))
        );
        assert_eq!(
            environment.get(OsStr::new(REMOTE_ROS2_ARTIFACT_PROBE_ENV_V1)),
            Some(&None)
        );
    }

    #[cfg(unix)]
    #[test]
    fn executable_resolution_preserves_rustup_proxy_argv_zero() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let rustup = directory.path().join("rustup");
        std::fs::write(&rustup, b"proxy").unwrap();
        let proxy = directory.path().join("rustc");
        symlink(&rustup, &proxy).unwrap();

        let resolved = resolve_executable(proxy.as_os_str()).unwrap();
        assert_eq!(resolved, proxy);
        assert_ne!(resolved, std::fs::canonicalize(&rustup).unwrap());
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    fn installed_locked_rustc() -> Option<PathBuf> {
        if let Some(rustc) = std::env::var_os("RUSTC").map(PathBuf::from)
            && Command::new(&rustc)
                .arg("-vV")
                .output()
                .ok()
                .is_some_and(|output| {
                    String::from_utf8_lossy(&output.stdout)
                        .lines()
                        .any(|line| line == format!("release: {LOCKED_RUST_RELEASE_V1}"))
                })
        {
            return Some(rustc);
        }
        let rustup_home = std::env::var_os("RUSTUP_HOME").map_or_else(
            || std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".rustup")),
            |home| Some(PathBuf::from(home)),
        )?;
        let rustc = rustup_home
            .join("toolchains")
            .join(format!("{LOCKED_RUST_RELEASE_V1}-{LOCKED_RUST_HOST_V1}"))
            .join("bin")
            .join(format!("rustc{}", std::env::consts::EXE_SUFFIX));
        rustc.is_file().then_some(rustc)
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn installed_locked_rustup_proxy_is_attested_when_available() {
        let cargo_home = std::env::var_os("CARGO_HOME").map_or_else(
            || std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")),
            |home| Some(PathBuf::from(home)),
        );
        let Some(cargo_home) = cargo_home else {
            return;
        };
        let proxy = cargo_home
            .join("bin")
            .join(format!("rustc{}", std::env::consts::EXE_SUFFIX));
        if !proxy.is_file() {
            return;
        }
        let Ok(verbose) = Command::new(&proxy).arg("-vV").output() else {
            return;
        };
        if !String::from_utf8_lossy(&verbose.stdout)
            .lines()
            .any(|line| line == format!("release: {LOCKED_RUST_RELEASE_V1}"))
        {
            return;
        }
        let toolchain = locked_remote_wasm_toolchain_attestation_v1(proxy.as_os_str()).unwrap();
        assert_eq!(
            toolchain.canonical_rustc_v1(),
            std::fs::canonicalize(installed_locked_rustc().unwrap()).unwrap()
        );
        assert_ne!(
            std::fs::canonicalize(proxy).unwrap(),
            toolchain.canonical_rustc_v1()
        );
    }

    #[cfg(all(unix, target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn lying_discovery_proxy_cannot_compile_or_replace_the_canonical_child() {
        use std::os::unix::fs::PermissionsExt as _;

        let Some(locked_rustc) = installed_locked_rustc() else {
            return;
        };
        let sysroot = Command::new(&locked_rustc)
            .args(["--print", "sysroot"])
            .output()
            .unwrap();
        assert!(sysroot.status.success());
        let sysroot = String::from_utf8(sysroot.stdout).unwrap();
        let sysroot = sysroot.trim();

        let directory = tempfile::tempdir().unwrap();
        let proxy = directory.path().join("rustc");
        let log = directory.path().join("proxy.log");
        let tamper = directory.path().join("compile-was-tampered");
        std::fs::write(
            &proxy,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{}'\n\
                 if [ \"$1\" = '--print' ] && [ \"$2\" = 'sysroot' ]; then printf '%s\\n' '{}'; exit 0; fi\n\
                 if [ \"$1\" = '-vV' ]; then printf '%s\\n' 'release: {}' 'commit-hash: {}'; exit 0; fi\n\
                 : > '{}'\nexit 99\n",
                log.display(),
                sysroot,
                LOCKED_RUST_RELEASE_V1,
                LOCKED_RUST_COMMIT_V1,
                tamper.display(),
            ),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&proxy).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&proxy, permissions).unwrap();

        let toolchain = locked_remote_wasm_toolchain_attestation_v1(proxy.as_os_str()).unwrap();
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "--print\nsysroot\n");
        assert!(!tamper.exists());
        assert_ne!(
            std::fs::canonicalize(&proxy).unwrap(),
            toolchain.canonical_rustc_v1()
        );
        verify_file_digest(toolchain.canonical_rust_lld_v1(), LOCKED_RUST_LLD_DIGEST_V1).unwrap();
        let replaced_lld = directory.path().join("replaced-rust-lld");
        std::fs::copy(toolchain.canonical_rust_lld_v1(), &replaced_lld).unwrap();
        use std::io::Write as _;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&replaced_lld)
            .unwrap()
            .write_all(b"replacement")
            .unwrap();
        assert!(verify_file_digest(&replaced_lld, LOCKED_RUST_LLD_DIGEST_V1).is_err());

        let project = directory.path().join("project");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::write(
            project.join("Cargo.toml"),
            "[package]\nname='canonical-child-fixture'\nversion='0.0.0'\nedition='2024'\n",
        )
        .unwrap();
        std::fs::write(
            project.join("build.rs"),
            format!(
                "fn main() {{\n\
                 let rustc = std::fs::canonicalize(std::env::var_os(\"RUSTC\").unwrap()).unwrap();\n\
                 assert_eq!(rustc, std::path::Path::new({:?}));\n\
                 assert_eq!(std::env::var_os(\"RUSTC_LINKER\").as_deref(), Some(std::ffi::OsStr::new({:?})));\n\
                 assert_eq!(std::env::var_os(\"CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_LINKER\").as_deref(), Some(std::ffi::OsStr::new({:?})));\n\
                 assert_eq!(std::env::var_os(\"CARGO_ENCODED_RUSTFLAGS\").unwrap(), std::ffi::OsString::from({:?}));\n\
                 assert_eq!(std::env::var(\"{}\").as_deref(), Ok(\"1\"));\n\
                 }}\n",
                toolchain.canonical_rustc_v1().as_os_str(),
                toolchain.canonical_rust_lld_v1().as_os_str(),
                toolchain.canonical_rust_lld_v1().as_os_str(),
                canonical_web_rustflags_v1(true),
                REMOTE_ROS2_ARTIFACT_PROBE_ENV_V1,
            ),
        )
        .unwrap();
        std::fs::write(
            project.join("src/lib.rs"),
            "pub fn answer() -> u32 { 42 }\n",
        )
        .unwrap();
        let mut command = Command::new(toolchain.canonical_cargo_v1());
        command.args([
            "check",
            "--target=wasm32-unknown-unknown",
            "--offline",
            "-vv",
        ]);
        let mut adversarial_path = vec![directory.path().to_owned()];
        if let Some(ambient) = std::env::var_os("PATH") {
            adversarial_path.extend(std::env::split_paths(&ambient));
        }
        configure_remote_ros2_verifier_command_with_path_v1(
            &mut command,
            &toolchain,
            Some(std::env::join_paths(adversarial_path).unwrap()),
        )
        .unwrap();
        let output = command.current_dir(&project).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let trace = String::from_utf8_lossy(&output.stderr);
        assert!(trace.contains(&toolchain.canonical_rustc_v1().display().to_string()));
        assert!(trace.contains(&toolchain.canonical_rust_lld_v1().display().to_string()));
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "--print\nsysroot\n");
        assert!(!tamper.exists());
    }

    #[cfg(all(unix, target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn verifier_cargo_never_executes_home_workspace_or_environment_wrappers() {
        use std::os::unix::fs::PermissionsExt as _;

        let Some(locked_rustc) = installed_locked_rustc() else {
            return;
        };
        let toolchain = locked_remote_wasm_toolchain_attestation_v1(locked_rustc.as_os_str())
            .expect("the installed locked toolchain is attested");
        let directory = tempfile::tempdir().unwrap();
        let trace = directory.path().join("wrapper.trace");
        let write_wrapper = |name: &str| {
            let path = directory.path().join(name);
            std::fs::write(
                &path,
                format!(
                    "#!/bin/sh\nprintf '%s\\n' '{}' >> '{}'\nexec \"$@\"\n",
                    name,
                    trace.display()
                ),
            )
            .unwrap();
            let mut permissions = std::fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&path, permissions).unwrap();
            path
        };
        let home_wrapper = write_wrapper("home-wrapper");
        let workspace_wrapper = write_wrapper("workspace-wrapper");
        let environment_wrapper = write_wrapper("environment-wrapper");

        let vendor = directory.path().join("vendor");
        let vendored_package = vendor.join("home-config-fixture-1.0.0");
        std::fs::create_dir_all(vendored_package.join("src")).unwrap();
        let vendored_manifest =
            b"[package]\nname='home-config-fixture'\nversion='1.0.0'\nedition='2024'\n";
        let vendored_source = b"pub fn answer() -> u32 { 42 }\n";
        std::fs::write(vendored_package.join("Cargo.toml"), vendored_manifest).unwrap();
        std::fs::write(vendored_package.join("src/lib.rs"), vendored_source).unwrap();
        let digest = |bytes: &[u8]| {
            let mut encoded = String::new();
            for byte in Sha256::digest(bytes) {
                write!(&mut encoded, "{byte:02x}").unwrap();
            }
            encoded
        };
        std::fs::write(
            vendored_package.join(".cargo-checksum.json"),
            format!(
                "{{\"files\":{{\"Cargo.toml\":\"{}\",\"src/lib.rs\":\"{}\"}},\"package\":null}}",
                digest(vendored_manifest),
                digest(vendored_source)
            ),
        )
        .unwrap();

        let fake_home = directory.path().join("home");
        std::fs::create_dir_all(fake_home.join(".cargo")).unwrap();
        let home_config = fake_home.join(".cargo/config.toml");
        let home_config_contents = format!(
            "[build]\nrustc-wrapper = {:?}\n\
             [env]\nRERUN_VERIFIER_HOME_CONFIG_VISIBLE = \"yes\"\n\
             [source.crates-io]\nreplace-with = \"fixture-vendor\"\n\
             [source.fixture-vendor]\ndirectory = {:?}\n",
            home_wrapper.display().to_string(),
            vendor.display().to_string()
        );
        std::fs::write(&home_config, &home_config_contents).unwrap();
        let credentials = fake_home.join(".cargo/credentials.toml");
        let credentials_contents = "[registries.fixture-private]\ntoken = \"credential-marker\"\n";
        std::fs::write(&credentials, credentials_contents).unwrap();

        let project = directory.path().join("project");
        std::fs::create_dir_all(project.join(".cargo")).unwrap();
        std::fs::write(
            project.join(".cargo/config.toml"),
            format!(
                "[build]\nrustc-workspace-wrapper = {:?}\n",
                workspace_wrapper.display().to_string()
            ),
        )
        .unwrap();
        std::fs::write(
            project.join("Cargo.toml"),
            "[workspace]\nmembers=['re-mcap-consumer', 're-viewer-consumer']\nresolver='2'\n",
        )
        .unwrap();
        for (member, package) in [
            ("re-mcap-consumer", "re-mcap-consumer-fixture"),
            ("re-viewer-consumer", "re-viewer-consumer-fixture"),
        ] {
            let member = project.join(member);
            std::fs::create_dir_all(member.join("src")).unwrap();
            std::fs::write(
                member.join("Cargo.toml"),
                format!(
                    "[package]\nname={package:?}\nversion='0.0.0'\nedition='2024'\n\
                     [dependencies]\nhome-config-fixture='1.0.0'\n"
                ),
            )
            .unwrap();
            std::fs::write(
                member.join("build.rs"),
                format!(
                    "fn main() {{\n\
                     for variable in [\"RUSTC_WRAPPER\", \"RUSTC_WORKSPACE_WRAPPER\"] {{\n\
                     assert_eq!(std::env::var_os(variable), Some(std::ffi::OsString::new()), \"{{variable}} was not disabled exactly\");\n\
                     }}\n\
                     println!(\"cargo::warning={package} observed the disabled-wrapper consumer contract\");\n\
                     }}\n"
                ),
            )
            .unwrap();
            std::fs::write(
                member.join("src/lib.rs"),
                "pub const HOME_CONFIG: &str = env!(\"RERUN_VERIFIER_HOME_CONFIG_VISIBLE\");\n\
                 pub fn answer() -> u32 { home_config_fixture::answer() }\n",
            )
            .unwrap();
        }

        let mut command = Command::new(toolchain.canonical_cargo_v1());
        command.args([
            "check",
            "--target=wasm32-unknown-unknown",
            "--offline",
            "-vv",
            "--config=.cargo/config.toml",
        ]);
        command.env("HOME", &fake_home);
        command.env("CARGO_HOME", fake_home.join(".cargo"));
        command.env("CARGO_NET_OFFLINE", "true");
        command.env("RUSTC_WRAPPER", &environment_wrapper);
        command.env("RUSTC_WORKSPACE_WRAPPER", &environment_wrapper);
        configure_remote_ros2_verifier_command_v1(&mut command, &toolchain).unwrap();
        let output = command.current_dir(&project).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains(&toolchain.canonical_rustc_v1().display().to_string())
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        for package in ["re-mcap-consumer-fixture", "re-viewer-consumer-fixture"] {
            assert!(
                stderr.contains(&format!(
                    "{package} observed the disabled-wrapper consumer contract"
                )),
                "{package} build script did not observe the consumer environment: {stderr}"
            );
        }
        assert!(
            !trace.exists(),
            "a wrapper ran before consumer attestation: {}",
            std::fs::read_to_string(&trace).unwrap_or_default()
        );
        assert_eq!(
            std::fs::read_to_string(&home_config).unwrap(),
            home_config_contents
        );
        assert_eq!(
            std::fs::read_to_string(&credentials).unwrap(),
            credentials_contents
        );
    }

    #[cfg(all(unix, target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn verifier_preserves_a_warm_git_cache_for_offline_rebuilds() {
        let Some(locked_rustc) = installed_locked_rustc() else {
            return;
        };
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let toolchain = locked_remote_wasm_toolchain_attestation_v1(locked_rustc.as_os_str())
            .expect("the installed locked toolchain is attested");
        let directory = tempfile::tempdir().unwrap();
        let dependency = directory.path().join("warm-cache-dependency");
        std::fs::create_dir_all(dependency.join("src")).unwrap();
        std::fs::write(
            dependency.join("Cargo.toml"),
            "[package]\nname='warm-cache-fixture'\nversion='1.0.0'\nedition='2024'\n",
        )
        .unwrap();
        std::fs::write(
            dependency.join("src/lib.rs"),
            "pub fn answer() -> u32 { 42 }\n",
        )
        .unwrap();
        for arguments in [
            vec!["init", "--quiet"],
            vec!["config", "user.email", "fixture@rerun.invalid"],
            vec!["config", "user.name", "Rerun Fixture"],
            vec!["add", "."],
            vec!["commit", "--quiet", "-m", "fixture"],
        ] {
            let output = Command::new("git")
                .arg("-C")
                .arg(&dependency)
                .args(arguments)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let project = directory.path().join("project");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::write(
            project.join("Cargo.toml"),
            format!(
                "[package]\nname='warm-cache-consumer'\nversion='0.0.0'\nedition='2024'\n\
                 [dependencies]\nwarm-cache-fixture={{git={:?}}}\n",
                format!("file://{}", dependency.display())
            ),
        )
        .unwrap();
        std::fs::write(
            project.join("src/lib.rs"),
            "pub fn answer() -> u32 { warm_cache_fixture::answer() }\n",
        )
        .unwrap();

        let cargo_home = directory.path().join("caller-cargo-home");
        std::fs::create_dir_all(&cargo_home).unwrap();
        let mut warm = Command::new(toolchain.canonical_cargo_v1());
        warm.args(["check", "--target=wasm32-unknown-unknown"]);
        warm.env("CARGO_HOME", &cargo_home);
        warm.env("CARGO_TARGET_DIR", directory.path().join("warm-target"));
        configure_remote_ros2_verifier_command_v1(&mut warm, &toolchain).unwrap();
        let output = warm.current_dir(&project).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(cargo_home.join("git/checkouts").is_dir());

        std::fs::rename(
            &dependency,
            directory.path().join("unavailable-original-dependency"),
        )
        .unwrap();
        let mut offline = Command::new(toolchain.canonical_cargo_v1());
        offline.args([
            "check",
            "--target=wasm32-unknown-unknown",
            "--locked",
            "--offline",
        ]);
        offline.env("CARGO_HOME", &cargo_home);
        offline.env("CARGO_NET_OFFLINE", "true");
        offline.env("CARGO_TARGET_DIR", directory.path().join("offline-target"));
        configure_remote_ros2_verifier_command_v1(&mut offline, &toolchain).unwrap();
        let output = offline.current_dir(&project).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn generated_capability_is_a_real_fail_closed_rust_source() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("fixture.rs");
        let output = directory.path().join("fixture.rmeta");
        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| OsString::from("rustc"));
        let attestation = RemoteRos2ArtifactAttestationV1 {
            _toolchain: fake_toolchain(directory.path()),
        };
        for consumer in [
            RemoteRos2ArtifactConsumerV1::Mcap,
            RemoteRos2ArtifactConsumerV1::Viewer,
        ] {
            let generated =
                write_remote_ros2_generated_capability_v1(None, directory.path(), consumer)
                    .unwrap();
            std::fs::write(&source, format!("include!({:?});", generated.as_os_str())).unwrap();
            let rejected = Command::new(&rustc)
                .arg("--crate-name=direct_cfg_injection")
                .args(["--crate-type=lib", "--emit=metadata"])
                .arg(&source)
                .arg("-o")
                .arg(&output)
                .output()
                .unwrap();
            assert!(!rejected.status.success(), "{consumer:?}");
            assert!(
                String::from_utf8_lossy(&rejected.stderr)
                    .contains("cfg lacks its attested generated capability")
            );

            write_remote_ros2_generated_capability_v1(
                Some(&attestation),
                directory.path(),
                consumer,
            )
            .unwrap();
            let accepted = Command::new(&rustc)
                .arg("--crate-name=attested_cfg")
                .args(["--crate-type=lib", "--emit=metadata"])
                .arg(&source)
                .arg("-o")
                .arg(&output)
                .output()
                .unwrap();
            assert!(
                accepted.status.success(),
                "{consumer:?}: {}",
                String::from_utf8_lossy(&accepted.stderr)
            );
        }
    }
}
