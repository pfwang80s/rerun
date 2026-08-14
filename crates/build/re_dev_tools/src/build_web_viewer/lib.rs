#![expect(clippy::unwrap_used)]

//! Build the Rerun web-viewer .wasm and generate the .js bindings for it.

use std::time::Instant;

use anyhow::Context as _;
use cargo_metadata::camino::{Utf8Path, Utf8PathBuf};
use re_build_tools::remote_wasm_contract::{
    RemoteWasmToolchainAttestationV1, configure_mcap_phase_a_proof_command_v1,
    configure_product_wasm_command_v1, configure_remote_ros2_verifier_command_v1,
    is_canonical_remote_ros2_probe_host_v1, locked_remote_wasm_toolchain_attestation_v1,
};
use sha2::{Digest as _, Sha256};

const REMOTE_ROS2_ALLOCATOR_CONTRACT_V1: &[u8] =
    b"AccountingAllocator<System>;tracking=admission-v1";
const REMOTE_ROS2_ALLOCATOR_SECTION_V1: &str = "rerun_remote_ros2_allocator_contract_v1";
const REMOTE_ROS2_INITIALIZER_PROBE_V1: &str = "rerun_remote_ros2_initializer_artifact_probe_v1";
const REMOTE_PROTOBUF_INITIALIZER_PROBE_V1: &str =
    "rerun_remote_protobuf_initializer_artifact_probe_v1";
const REMOTE_ROS2_ACCOUNTING_ALLOCATOR_PROBE_V1: &str =
    "rerun_remote_ros2_accounting_allocator_artifact_probe_v1";
const REMOTE_ROS2_STACK_POINTER_V1: &str = "__stack_pointer";
const REMOTE_ROS2_REQUIRED_STAGES_V1: [&str; 5] = [
    "rerun_remote_ros2_signature_stage_v1",
    "rerun_remote_ros2_census_stage_v1",
    "rerun_remote_ros2_name_stage_v1",
    "rerun_remote_ros2_complex_stage_v1",
    "rerun_remote_ros2_peak_stage_v1",
];
const REMOTE_ROS2_REQUIRED_STAGE_IDENTITIES_V1: [i32; 5] = [0x2601, 0x2602, 0x2603, 0x2604, 0x2605];
const REMOTE_PROTOBUF_REQUIRED_STAGES_V1: [&str; 6] = [
    "rerun_remote_protobuf_wire_stage_v1",
    "rerun_remote_protobuf_graph_stage_v1",
    "rerun_remote_protobuf_peak_stage_v1",
    "rerun_remote_protobuf_arena_stage_v1",
    "rerun_remote_protobuf_recognition_stage_v1",
    "rerun_remote_decoder_assignment_stage_v1",
];
const REMOTE_PROTOBUF_REQUIRED_STAGE_IDENTITIES_V1: [i32; 6] =
    [0x2701, 0x2702, 0x2703, 0x2704, 0x2705, 0x2801];
const REMOTE_ROS2_INTERNAL_EXPORTS_V1: [&str; 17] = [
    REMOTE_ROS2_INITIALIZER_PROBE_V1,
    REMOTE_PROTOBUF_INITIALIZER_PROBE_V1,
    REMOTE_ROS2_ACCOUNTING_ALLOCATOR_PROBE_V1,
    "rerun_remote_ros2_initializer_artifact_probe_v1_impl",
    "rerun_remote_protobuf_initializer_artifact_probe_v1_impl",
    REMOTE_ROS2_STACK_POINTER_V1,
    REMOTE_ROS2_REQUIRED_STAGES_V1[0],
    REMOTE_ROS2_REQUIRED_STAGES_V1[1],
    REMOTE_ROS2_REQUIRED_STAGES_V1[2],
    REMOTE_ROS2_REQUIRED_STAGES_V1[3],
    REMOTE_ROS2_REQUIRED_STAGES_V1[4],
    REMOTE_PROTOBUF_REQUIRED_STAGES_V1[0],
    REMOTE_PROTOBUF_REQUIRED_STAGES_V1[1],
    REMOTE_PROTOBUF_REQUIRED_STAGES_V1[2],
    REMOTE_PROTOBUF_REQUIRED_STAGES_V1[3],
    REMOTE_PROTOBUF_REQUIRED_STAGES_V1[4],
    REMOTE_PROTOBUF_REQUIRED_STAGES_V1[5],
];
const REMOTE_ROS2_MIN_PROBE_STACK_V1: u64 = 512;
const REMOTE_ROS2_MAX_PROBE_STACK_V1: u64 = 16 * 1024;

pub fn workspace_root() -> Utf8PathBuf {
    cargo_metadata::MetadataCommand::new()
        .manifest_path(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .features(cargo_metadata::CargoOpt::NoDefaultFeatures)
        .no_deps()
        .exec()
        .unwrap()
        .workspace_root
}

pub fn default_build_dir() -> Utf8PathBuf {
    // crates/viewer/re_web_viewer_server/web_viewer
    workspace_root()
        .join("crates")
        .join("viewer")
        .join("re_web_viewer_server")
        .join("web_viewer")
}

fn target_directory() -> Utf8PathBuf {
    let metadata = cargo_metadata::MetadataCommand::new()
        .manifest_path(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .features(cargo_metadata::CargoOpt::NoDefaultFeatures)
        .exec()
        .unwrap();
    metadata.target_directory
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    WebRelease,
    Debug,
}

impl Profile {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::WebRelease => "web-release",
            Self::Debug => "debug",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Browser,
    Module,

    /// Custom target meant for post-processing inside `rerun_js`.
    NoModulesBase,
}

impl argh::FromArgValue for Target {
    fn from_arg_value(value: &str) -> Result<Self, String> {
        match value {
            "browser" => Ok(Self::Browser),
            "module" => Ok(Self::Module),
            "no-modules-base" => Ok(Self::NoModulesBase),
            _ => Err(format!("Unknown target: {value}")),
        }
    }
}

fn verify_remote_ros2_probe_toolchain() -> anyhow::Result<RemoteWasmToolchainAttestationV1> {
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    locked_remote_wasm_toolchain_attestation_v1(&rustc)
}

/// Builds the private MCAP Phase A proof with the exact release profile and optimizer used by the
/// Web Viewer product artifact.
pub fn build_mcap_phase_a_proof_v1(build_dir: &Utf8Path) -> anyhow::Result<()> {
    std::env::set_current_dir(workspace_root())?;
    std::fs::create_dir_all(build_dir)?;
    let root_dir = workspace_root();
    let target_dir = Utf8PathBuf::from(format!("{}_mcap_phase_a_proof", target_directory()));
    let toolchain = verify_remote_ros2_probe_toolchain()?;
    let mut command = std::process::Command::new(toolchain.canonical_cargo_v1());
    command.args([
        "build",
        "--package=re_mcap_phase_a_chrome",
        "--lib",
        "--target=wasm32-unknown-unknown",
        &format!("--target-dir={target_dir}"),
        "--profile=web-release",
        "--config=.cargo/config.toml",
    ]);
    configure_mcap_phase_a_proof_command_v1(&mut command, &toolchain)?;
    eprintln!("{root_dir}> {command:?}");
    let status = command
        .current_dir(&root_dir)
        .status()
        .context("Failed to build the MCAP Phase A proof artifact")?;
    anyhow::ensure!(
        status.success(),
        "Failed to build the MCAP Phase A proof artifact"
    );

    let input_wasm = target_dir
        .join("wasm32-unknown-unknown")
        .join("web-release")
        .join("re_mcap_phase_a_chrome.wasm");
    let mut bindgen = wasm_bindgen_cli_support::Bindgen::new();
    bindgen
        .input_path(input_wasm.as_str())
        .out_name("re_mcap_phase_a_proof")
        .web(true)?
        .typescript(false);
    bindgen
        .generate(build_dir.as_str())
        .context("Failed to generate MCAP Phase A proof bindings")?;
    let optimized_wasm = build_dir.join("re_mcap_phase_a_proof_bg.wasm");
    optimize_wasm(&root_dir, &optimized_wasm, false)?;
    let mut payload = vec![0, 1, 0, 0];
    payload.extend_from_slice(&42_i32.to_le_bytes());
    let fixture = re_mcap::testing::AdversarialMcapFixtureBuilder::new()
        .with_schemas([
            re_mcap::testing::FixtureSchema::new(7, "pkg/Root", "ros2msg")
                .with_data(b"int32 value"),
        ])
        .with_channels([
            re_mcap::testing::FixtureChannel::schema_less(1, "/phase_a").with_schema(7, "cdr")
        ])
        .with_chunks([re_mcap::testing::FixtureChunk::new([
            re_mcap::testing::FixtureMessage::new(1, 1, 1).with_data(payload),
        ])])
        .with_partition_fixture(re_mcap::testing::PartitionFixture::default())
        .build()
        .map_err(|error| anyhow::anyhow!("Failed to build the fixed Phase A fixture: {error}"))?;
    let fixture_name = "phase-a-fixture.mcap";
    std::fs::write(build_dir.join(fixture_name), &fixture.bytes)?;
    let module_name = "re_mcap_phase_a_proof.js";
    let wasm_name = "re_mcap_phase_a_proof_bg.wasm";
    let module_sha256 = sha256_hex_v1(&std::fs::read(build_dir.join(module_name))?);
    let wasm_sha256 = sha256_hex_v1(&std::fs::read(build_dir.join(wasm_name))?);
    let fixture_sha256 = sha256_hex_v1(&fixture.bytes);
    let git_commit = re_build_tools::git_commit_hash()?;
    std::fs::write(
        build_dir.join("proof-manifest-v1.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": "rerun-mcap-phase-a-proof-build-v1",
            "profile": "web-release",
            "wasm_optimized": true,
            "module": module_name,
            "wasm": wasm_name,
            "fixture": fixture_name,
            "fixture_length": fixture.bytes.len(),
            "module_sha256": module_sha256,
            "wasm_sha256": wasm_sha256,
            "fixture_sha256": fixture_sha256,
            "git_commit": git_commit,
            "browser_family": "chrome-stable"
        }))?,
    )?;
    Ok(())
}

fn sha256_hex_v1(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Build `re_viewer` as Wasm, generate .js bindings for it, and place it all into the `build_dir` folder.
///
/// If `debug_symbols` is set, debug symbols are kept even in release builds,
/// allowing for better callstacks on panics, as well as in-browser profiling of the wasm.
#[expect(clippy::fn_params_excessive_bools)] // TODO(emilk): remove bool parameters
pub fn build(
    profile: Profile,
    debug_symbols: bool,
    target: Target,
    build_dir: &Utf8Path,
    no_default_features: bool,
    features: &String,
    timings: bool,
) -> anyhow::Result<()> {
    std::env::set_current_dir(workspace_root())?;

    eprintln!("Building web viewer…\n");

    let crate_name = "re_viewer";

    // Where we tell cargo to build to.
    // We want this to be different from the default target folder
    // in order to support recursive cargo builds (calling `cargo` from within a `build.rs`).
    let target_wasm_dir = Utf8PathBuf::from(format!("{}_wasm", target_directory()));

    // Workspace root
    let root_dir = workspace_root();

    // Where we will place the final .wasm and .js artifacts.
    assert!(
        build_dir.exists(),
        "Failed to find dir {build_dir}. CWD: {:?}, CARGO_MANIFEST_DIR: {:?}",
        std::env::current_dir(),
        std::env!("CARGO_MANIFEST_DIR")
    );

    // The two files we are building:
    let wasm_path = build_dir.join(format!("{crate_name}_bg.wasm"));
    let js_path = build_dir.join(format!("{crate_name}.js"));

    // Clean old versions:
    std::fs::remove_file(wasm_path.clone()).ok();
    std::fs::remove_file(js_path).ok();

    // The proof artifact is deliberately separate from the product artifact. It uses the same
    // locked compiler, dependency graph, release profile, and post-link optimizer, but alone owns
    // the private probe capability and stack-pointer export. The product build below never links
    // the probe call graph, so no post-hoc export stripping or dead-code cleanup is required.
    if profile == Profile::WebRelease && is_canonical_remote_ros2_probe_host_v1() {
        let toolchain = verify_remote_ros2_probe_toolchain()?;
        let verifier_target_dir =
            Utf8PathBuf::from(format!("{}_remote_ros2_probe", target_wasm_dir.as_str()));
        let mut command = std::process::Command::new(toolchain.canonical_cargo_v1());
        command.args([
            "build",
            &format!("--package={crate_name}"),
            "--lib",
            "--target=wasm32-unknown-unknown",
            &format!("--target-dir={}", verifier_target_dir.as_str()),
            "--profile=web-release",
            "--config=.cargo/config.toml",
        ]);
        if no_default_features {
            command.arg("--no-default-features");
        }
        if !features.is_empty() {
            command.arg(format!("--features={features}"));
        }
        if timings {
            command.arg("--timings");
        }
        configure_remote_ros2_verifier_command_v1(&mut command, &toolchain)?;
        eprintln!("{root_dir}> {command:?}");
        let status = command
            .current_dir(&root_dir)
            .status()
            .context("Failed to build the remote ROS 2 verifier artifact")?;
        anyhow::ensure!(
            status.success(),
            "Failed to build the remote ROS 2 verifier artifact"
        );
        let verifier_wasm_path = verifier_target_dir
            .join("wasm32-unknown-unknown")
            .join(profile.as_str())
            .join(format!("{crate_name}.wasm"));
        optimize_wasm(&root_dir, &verifier_wasm_path, debug_symbols)?;
        verify_remote_ros2_allocator_contract(&verifier_wasm_path)?;
    }

    {
        eprintln!("Compiling Rust to wasm in {target_wasm_dir}…");
        let start_time = Instant::now();

        let mut cmd = std::process::Command::new("cargo");
        cmd.args([
            "build",
            &format!("--package={crate_name}"),
            "--lib",
            "--target=wasm32-unknown-unknown",
            &format!("--target-dir={}", target_wasm_dir.as_str()),
        ]);
        if no_default_features {
            cmd.arg("--no-default-features");
        }
        if !features.is_empty() {
            cmd.arg(format!("--features={features}"));
        }
        if profile == Profile::WebRelease {
            cmd.arg("--profile=web-release");
        }
        if timings {
            cmd.arg("--timings");
        }

        // Note that we can't use RUSTFLAGS here directly since having more than one flag on set via
        // `cmd.env("RUSTFLAGS", rustflags)` completely messes up the rustc invocation no matter how we quote it.
        cmd.arg("--config=.cargo/config.toml");

        // When executing this script from a Rust build script, do _not_, under any circumstances,
        // allow pre-encoded `RUSTFLAGS` to leak into the current environment.
        // These pre-encoded flags are generally generated by Cargo itself when loading its
        // configuration from e.g. `$CARGO_HOME/config.toml`; which means they will contain
        // values that only make sense for the native target host, not for a wasm build.
        configure_product_wasm_command_v1(&mut cmd);

        eprintln!("{root_dir}> {cmd:?}");
        let status = cmd
            .current_dir(&root_dir)
            .status()
            .context("Failed to build Wasm")?;

        anyhow::ensure!(status.success(), "Failed to build Wasm");

        let target_wasm_path = target_wasm_dir
            .join("wasm32-unknown-unknown")
            .join(profile.as_str())
            .join(format!("{crate_name}.wasm"));
        verify_remote_ros2_product_artifact(&target_wasm_path)?;

        eprintln!(
            "Web viewer .wasm built in {:.1}s\n",
            start_time.elapsed().as_secs_f32()
        );
    }

    {
        eprintln!("Generating JS bindings for wasm…");
        let start_time = Instant::now();

        let build = profile.as_str();

        let target_wasm_path = target_wasm_dir
            .join("wasm32-unknown-unknown")
            .join(build)
            .join(format!("{crate_name}.wasm"));

        let mut bindgen_cmd = wasm_bindgen_cli_support::Bindgen::new();
        bindgen_cmd
            .input_path(target_wasm_path.as_str())
            .out_name(crate_name);
        match target {
            Target::Browser => bindgen_cmd.no_modules(true)?.typescript(false),
            Target::Module => bindgen_cmd.no_modules(false)?.typescript(true),
            Target::NoModulesBase => bindgen_cmd.no_modules(true)?.typescript(true),
        };
        if let Err(err) = bindgen_cmd.generate(build_dir.as_str()) {
            if err
                .to_string()
                .starts_with("cannot import from modules (`env`")
            {
                // Very common error: "cannot import from modules (`env`) with `--no-modules`"
                anyhow::bail!(
                "Failed to run wasm-bindgen: {err}. This is often because some dependency is calling `std::time::Instant::now()` or similar. You can try diagnosing this with:\n\
                wasm2wat {target_wasm_path} | rg '\"env\"'\n\
                wasm2wat {target_wasm_path} | rg 'call .now\\b' -B 20\n\
                \n\
                You can also try `twiggy paths`:
                https://github.com/AlexEne/twiggy/blob/945e6241bf7b6d918fba17082d0b12eae1c56349/guide/src/usage/command-line-interface/paths.md
                "
            );
            } else {
                return Err(err.context("Failed to run wasm-bindgen"));
            }
        }

        eprintln!(
            "Generated JS bindings in {:.1}s\n",
            start_time.elapsed().as_secs_f32()
        );
    }

    if profile == Profile::WebRelease {
        optimize_wasm(&root_dir, &wasm_path, debug_symbols)?;
    }

    verify_remote_ros2_product_artifact(&wasm_path)?;
    verify_remote_ros2_public_bindings(build_dir, crate_name)?;

    // --------------------------------------------------------------------------------

    eprintln!("Finished {wasm_path}");

    Ok(())
}

fn optimize_wasm(
    root_dir: &Utf8Path,
    wasm_path: &Utf8Path,
    debug_symbols: bool,
) -> anyhow::Result<()> {
    eprintln!("Optimizing wasm with wasm-opt…");
    let start_time = Instant::now();
    let mut command = std::process::Command::new("wasm-opt");
    let mut args = vec![
        wasm_path.as_str(),
        "-O2",
        "--output",
        wasm_path.as_str(),
        "--enable-reference-types",
        "--enable-simd",
        "--enable-bulk-memory",
        "--enable-nontrapping-float-to-int",
        "--enable-multivalue",
        "--vacuum",
    ];
    if debug_symbols {
        args.push("-g");
    } else {
        args.push("--strip-debug");
    }
    command.args(args);
    eprintln!("{root_dir}> {command:?}");
    let output = command
        .current_dir(root_dir)
        .output()
        .context("Failed to run wasm-opt, it may not be installed")?;
    anyhow::ensure!(
        output.status.success(),
        "Failed to run wasm-opt:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
    eprintln!(
        "Optimized wasm in {:.1}s\n",
        start_time.elapsed().as_secs_f32()
    );
    Ok(())
}

fn verify_remote_ros2_allocator_contract(wasm_path: &Utf8Path) -> anyhow::Result<()> {
    let wasm = std::fs::read(wasm_path).with_context(|| {
        format!("Failed to read Web allocator contract\nFile path: {wasm_path}")
    })?;
    verify_remote_ros2_allocator_contract_bytes(&wasm).with_context(|| {
        format!("Web allocator contract verification failed\nFile path: {wasm_path}")
    })
}

fn verify_remote_ros2_product_artifact(wasm_path: &Utf8Path) -> anyhow::Result<()> {
    let wasm = std::fs::read(wasm_path)
        .with_context(|| format!("Failed to read Web product artifact\nFile path: {wasm_path}"))?;
    verify_remote_ros2_product_artifact_bytes(&wasm)
        .with_context(|| format!("Web product ABI verification failed\nFile path: {wasm_path}"))
}

fn verify_remote_ros2_product_artifact_bytes(wasm: &[u8]) -> anyhow::Result<()> {
    use wasmparser::{Parser, Payload};

    for payload in Parser::new(0).parse_all(wasm) {
        match payload? {
            Payload::ExportSection(reader) => {
                for export in reader {
                    anyhow::ensure!(
                        !REMOTE_ROS2_INTERNAL_EXPORTS_V1.contains(&export?.name),
                        "Product artifact exposes an internal remote ROS 2 verifier ABI"
                    );
                }
            }
            Payload::CustomSection(section)
                if section.name() == REMOTE_ROS2_ALLOCATOR_SECTION_V1 =>
            {
                anyhow::bail!("Product artifact retains the remote ROS 2 verifier section");
            }
            _ => {}
        }
    }
    for internal in REMOTE_ROS2_INTERNAL_EXPORTS_V1
        .iter()
        .copied()
        .filter(|name| *name != REMOTE_ROS2_STACK_POINTER_V1)
        .chain(std::iter::once(REMOTE_ROS2_ALLOCATOR_SECTION_V1))
    {
        anyhow::ensure!(
            !wasm
                .windows(internal.len())
                .any(|window| window == internal.as_bytes()),
            "Product artifact retains internal remote ROS 2 verifier code or metadata"
        );
    }
    Ok(())
}

fn verify_remote_ros2_public_bindings(
    build_dir: &Utf8Path,
    crate_name: &str,
) -> anyhow::Result<()> {
    for extension in ["js", "d.ts"] {
        let path = build_dir.join(format!("{crate_name}.{extension}"));
        let Ok(bindings) = std::fs::read_to_string(&path) else {
            continue;
        };
        for internal in REMOTE_ROS2_INTERNAL_EXPORTS_V1 {
            anyhow::ensure!(
                !bindings.contains(internal),
                "Published JS/TypeScript ABI contains an internal verifier symbol\nFile path: {path}"
            );
        }
    }
    Ok(())
}

#[derive(Default)]
struct RemoteRos2WasmFunctionFacts {
    own_stack_bytes: u64,
    calls: Vec<u32>,
    stack_writes: Vec<(u32, Vec<StackWriteOperator>)>,
    has_memory_grow: bool,
    has_unresolved_indirect_call: bool,
    i32_constants: Vec<i32>,
}

#[derive(Clone, Copy)]
enum StackWriteOperator {
    GlobalGet(u32),
    I32Const(i32),
    I32Sub,
    I32Add,
    LocalGet(u32),
    LocalTee(u32),
    Other,
}

fn verify_remote_ros2_allocator_contract_bytes(wasm: &[u8]) -> anyhow::Result<()> {
    use std::collections::{BTreeMap, BTreeSet};

    use wasmparser::{ExternalKind, Operator, Parser, Payload, TypeRef};

    let mut marker_count = 0_usize;
    let mut imported_functions = 0_u32;
    let mut probe_function = None;
    let mut protobuf_probe_function = None;
    let mut accounting_allocator_probe_function = None;
    let mut stack_pointer_global = None;
    let mut required_stage_functions = BTreeMap::<&'static str, u32>::new();
    let mut protobuf_stage_functions = BTreeMap::<&'static str, u32>::new();
    let mut next_defined_function = 0_u32;
    let mut functions = BTreeMap::<u32, RemoteRos2WasmFunctionFacts>::new();

    for payload in Parser::new(0).parse_all(wasm) {
        match payload? {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    if matches!(import?.ty, TypeRef::Func(_) | TypeRef::FuncExact(_)) {
                        imported_functions = imported_functions
                            .checked_add(1)
                            .context("Web function-index arithmetic overflowed")?;
                    }
                }
            }
            Payload::ExportSection(reader) => {
                for export in reader {
                    let export = export?;
                    if export.name == REMOTE_ROS2_INITIALIZER_PROBE_V1
                        && export.kind == ExternalKind::Func
                    {
                        anyhow::ensure!(
                            probe_function.replace(export.index).is_none(),
                            "Web artifact exports the remote ROS 2 initializer probe more than once"
                        );
                    }
                    if export.name == REMOTE_PROTOBUF_INITIALIZER_PROBE_V1
                        && export.kind == ExternalKind::Func
                    {
                        anyhow::ensure!(
                            protobuf_probe_function.replace(export.index).is_none(),
                            "Web artifact exports the remote protobuf initializer probe more than once"
                        );
                    }
                    if export.name == REMOTE_ROS2_ACCOUNTING_ALLOCATOR_PROBE_V1
                        && export.kind == ExternalKind::Func
                    {
                        anyhow::ensure!(
                            accounting_allocator_probe_function
                                .replace(export.index)
                                .is_none(),
                            "Web artifact exports the remote ROS 2 accounting allocator probe more than once"
                        );
                    }
                    if export.name == REMOTE_ROS2_STACK_POINTER_V1
                        && export.kind == ExternalKind::Global
                    {
                        anyhow::ensure!(
                            stack_pointer_global.replace(export.index).is_none(),
                            "Web artifact exports the Wasm stack pointer more than once"
                        );
                    }
                    if let Some(stage) = REMOTE_ROS2_REQUIRED_STAGES_V1
                        .iter()
                        .copied()
                        .find(|stage| *stage == export.name)
                    {
                        anyhow::ensure!(
                            export.kind == ExternalKind::Func
                                && required_stage_functions
                                    .insert(stage, export.index)
                                    .is_none(),
                            "Web artifact has a malformed or duplicate remote ROS 2 stage anchor"
                        );
                    }
                    if let Some(stage) = REMOTE_PROTOBUF_REQUIRED_STAGES_V1
                        .iter()
                        .copied()
                        .find(|stage| *stage == export.name)
                    {
                        anyhow::ensure!(
                            export.kind == ExternalKind::Func
                                && protobuf_stage_functions
                                    .insert(stage, export.index)
                                    .is_none(),
                            "Web artifact has a malformed or duplicate remote protobuf stage anchor"
                        );
                    }
                }
            }
            Payload::CustomSection(section)
                if section.name() == REMOTE_ROS2_ALLOCATOR_SECTION_V1 =>
            {
                anyhow::ensure!(
                    section.data() == REMOTE_ROS2_ALLOCATOR_CONTRACT_V1,
                    "Web artifact contains a malformed remote ROS 2 allocator contract section"
                );
                marker_count = marker_count
                    .checked_add(1)
                    .context("Web custom-section count overflowed")?;
            }
            Payload::CodeSectionEntry(body) => {
                let function_index = imported_functions
                    .checked_add(next_defined_function)
                    .context("Web function-index arithmetic overflowed")?;
                next_defined_function = next_defined_function
                    .checked_add(1)
                    .context("Web function-index arithmetic overflowed")?;
                let mut facts = RemoteRos2WasmFunctionFacts::default();
                let mut stack_writes = Vec::new();
                let mut recent = std::collections::VecDeque::with_capacity(3);
                let mut operators = body.get_operators_reader()?;
                while !operators.eof() {
                    let operator = operators.read()?;
                    match operator {
                        Operator::Call { function_index }
                        | Operator::ReturnCall { function_index } => {
                            facts.calls.push(function_index);
                        }
                        Operator::CallIndirect { .. }
                        | Operator::ReturnCallIndirect { .. }
                        | Operator::CallRef { .. }
                        | Operator::ReturnCallRef { .. } => {
                            facts.has_unresolved_indirect_call = true;
                        }
                        Operator::MemoryGrow { .. } => facts.has_memory_grow = true,
                        Operator::I32Const { value } => facts.i32_constants.push(value),
                        Operator::GlobalSet { global_index } => {
                            stack_writes.push((global_index, recent.iter().copied().collect()));
                        }
                        _ => {}
                    }
                    let recent_operator = match operator {
                        Operator::GlobalGet { global_index } => {
                            StackWriteOperator::GlobalGet(global_index)
                        }
                        Operator::I32Const { value } => StackWriteOperator::I32Const(value),
                        Operator::I32Sub => StackWriteOperator::I32Sub,
                        Operator::I32Add => StackWriteOperator::I32Add,
                        Operator::LocalGet { local_index } => {
                            StackWriteOperator::LocalGet(local_index)
                        }
                        Operator::LocalTee { local_index } => {
                            StackWriteOperator::LocalTee(local_index)
                        }
                        _ => StackWriteOperator::Other,
                    };
                    if recent.len() == 4 {
                        recent.pop_front();
                    }
                    recent.push_back(recent_operator);
                }
                // Stack writes are interpreted only after the unique linker-exported stack global
                // is known. Keep the raw writes for the second pass below.
                facts.stack_writes = stack_writes;
                functions.insert(function_index, facts);
            }
            _ => {}
        }
    }

    anyhow::ensure!(
        marker_count == 1,
        "Web artifact does not contain exactly one locked remote ROS 2 allocator contract (found {marker_count})"
    );
    let probe_function = probe_function
        .context("Web artifact does not export the remote ROS 2 initializer probe")?;
    anyhow::ensure!(
        probe_function >= imported_functions,
        "Web artifact resolves the remote ROS 2 initializer probe to an imported function"
    );
    let protobuf_probe_function = protobuf_probe_function
        .context("Web artifact does not export the remote protobuf initializer probe")?;
    anyhow::ensure!(
        protobuf_probe_function >= imported_functions,
        "Web artifact resolves the remote protobuf initializer probe to an imported function"
    );
    let accounting_allocator_probe_function = accounting_allocator_probe_function
        .context("Web artifact does not export the remote ROS 2 accounting allocator probe")?;
    anyhow::ensure!(
        accounting_allocator_probe_function >= imported_functions,
        "Web artifact resolves the remote ROS 2 accounting allocator probe to an imported function"
    );
    let stack_pointer_global = stack_pointer_global
        .context("Web artifact does not export the linker-defined Wasm stack pointer")?;
    anyhow::ensure!(
        required_stage_functions.len() == REMOTE_ROS2_REQUIRED_STAGES_V1.len(),
        "Web artifact omits a required remote ROS 2 production-stage anchor"
    );
    anyhow::ensure!(
        protobuf_stage_functions.len() == REMOTE_PROTOBUF_REQUIRED_STAGES_V1.len(),
        "Web artifact omits a required remote protobuf production-stage anchor"
    );
    let distinct_stage_functions = required_stage_functions
        .values()
        .copied()
        .collect::<BTreeSet<_>>();
    anyhow::ensure!(
        distinct_stage_functions.len() == REMOTE_ROS2_REQUIRED_STAGES_V1.len(),
        "Web artifact aliases distinct remote ROS 2 production-stage anchors"
    );
    for (index, stage) in REMOTE_ROS2_REQUIRED_STAGES_V1.iter().enumerate() {
        let function = required_stage_functions[stage];
        let expected = REMOTE_ROS2_REQUIRED_STAGE_IDENTITIES_V1[index];
        anyhow::ensure!(
            functions
                .get(&function)
                .is_some_and(|facts| facts.i32_constants.contains(&expected)),
            "Remote ROS 2 production stage {stage} does not return its stable identity"
        );
    }
    let distinct_protobuf_stage_functions = protobuf_stage_functions
        .values()
        .copied()
        .collect::<BTreeSet<_>>();
    anyhow::ensure!(
        distinct_protobuf_stage_functions.len() == REMOTE_PROTOBUF_REQUIRED_STAGES_V1.len(),
        "Web artifact aliases distinct remote protobuf production-stage anchors"
    );
    for (index, stage) in REMOTE_PROTOBUF_REQUIRED_STAGES_V1.iter().enumerate() {
        let function = protobuf_stage_functions[stage];
        let expected = REMOTE_PROTOBUF_REQUIRED_STAGE_IDENTITIES_V1[index];
        anyhow::ensure!(
            functions
                .get(&function)
                .is_some_and(|facts| facts.i32_constants.contains(&expected)),
            "Remote protobuf production stage {stage} does not return its stable identity"
        );
    }
    let probe_facts = functions
        .get(&probe_function)
        .context("Missing Web code body for the remote ROS 2 initializer probe")?;
    anyhow::ensure!(
        probe_facts
            .calls
            .contains(&accounting_allocator_probe_function),
        "Remote ROS 2 initializer probe does not directly call the exact accounting allocator probe"
    );
    let protobuf_probe_facts = functions
        .get(&protobuf_probe_function)
        .context("Missing Web code body for the remote protobuf initializer probe")?;
    anyhow::ensure!(
        protobuf_probe_facts
            .calls
            .contains(&accounting_allocator_probe_function),
        "Remote protobuf initializer probe does not directly call the exact accounting allocator probe"
    );
    let mut visiting = BTreeSet::new();
    let mut completed = BTreeMap::new();
    let _reachability = remote_ros2_stack_and_call_proof(
        probe_function,
        imported_functions,
        &functions,
        &mut visiting,
        &mut completed,
    )?;
    let reachable: BTreeSet<_> = completed.keys().copied().collect();
    for (function_index, facts) in &mut functions {
        if reachable.contains(function_index) {
            facts.own_stack_bytes =
                verified_stack_frame_peak(&facts.stack_writes, stack_pointer_global)?;
        }
    }
    visiting.clear();
    completed.clear();
    let proof = remote_ros2_stack_and_call_proof(
        probe_function,
        imported_functions,
        &functions,
        &mut visiting,
        &mut completed,
    )?;
    for (stage, function) in &required_stage_functions {
        anyhow::ensure!(
            completed.contains_key(function),
            "Remote ROS 2 initializer probe does not cover production stage {stage}"
        );
    }
    anyhow::ensure!(
        proof.max_stack_bytes >= REMOTE_ROS2_MIN_PROBE_STACK_V1,
        "Remote ROS 2 initializer probe did not retain its sealed fixed scratch frame"
    );
    anyhow::ensure!(
        proof.max_stack_bytes <= REMOTE_ROS2_MAX_PROBE_STACK_V1,
        "Remote ROS 2 initializer probe stack/spill peak {} exceeds the locked {}-byte ceiling",
        proof.max_stack_bytes,
        REMOTE_ROS2_MAX_PROBE_STACK_V1
    );
    let allocator_proof = completed
        .get(&accounting_allocator_probe_function)
        .context(
            "Remote ROS 2 accounting allocator probe is absent from the verified call graph",
        )?;
    anyhow::ensure!(
        allocator_proof.has_memory_grow,
        "Remote ROS 2 accounting allocator probe has no direct path to the locked Wasm allocator"
    );
    anyhow::ensure!(
        !proof.has_unresolved_indirect_call,
        "Remote ROS 2 initializer probe contains an unresolved indirect call path"
    );

    visiting.clear();
    completed.clear();
    let _protobuf_reachability = remote_ros2_stack_and_call_proof(
        protobuf_probe_function,
        imported_functions,
        &functions,
        &mut visiting,
        &mut completed,
    )?;
    let protobuf_reachable: BTreeSet<_> = completed.keys().copied().collect();
    for (function_index, facts) in &mut functions {
        if protobuf_reachable.contains(function_index) {
            facts.own_stack_bytes =
                verified_stack_frame_peak(&facts.stack_writes, stack_pointer_global)?;
        }
    }
    visiting.clear();
    completed.clear();
    let protobuf_proof = remote_ros2_stack_and_call_proof(
        protobuf_probe_function,
        imported_functions,
        &functions,
        &mut visiting,
        &mut completed,
    )?;
    for (stage, function) in protobuf_stage_functions {
        anyhow::ensure!(
            completed.contains_key(&function),
            "Remote protobuf initializer probe does not cover production stage {stage}"
        );
    }
    for (stage, function) in required_stage_functions {
        anyhow::ensure!(
            completed.contains_key(&function),
            "Remote protobuf initializer probe does not preserve the ROS 2 stage {stage} in its sealed transition chain"
        );
    }
    anyhow::ensure!(
        protobuf_proof.max_stack_bytes <= REMOTE_ROS2_MAX_PROBE_STACK_V1,
        "Remote protobuf initializer probe stack/spill peak {} exceeds the locked {}-byte ceiling",
        protobuf_proof.max_stack_bytes,
        REMOTE_ROS2_MAX_PROBE_STACK_V1
    );
    anyhow::ensure!(
        !protobuf_proof.has_unresolved_indirect_call,
        "Remote protobuf initializer probe contains an unresolved indirect call path"
    );
    Ok(())
}

fn verified_stack_frame_peak(
    writes: &[(u32, Vec<StackWriteOperator>)],
    stack_pointer_global: u32,
) -> anyhow::Result<u64> {
    let mut active_frames = Vec::<(Option<u32>, u64)>::new();
    let mut current = 0_u64;
    let mut peak = 0_u64;
    for (written_global, recent) in writes {
        if *written_global != stack_pointer_global {
            continue;
        }
        let suffix = |length: usize| recent.get(recent.len().saturating_sub(length)..);
        let lowered = match suffix(4) {
            Some(
                [
                    StackWriteOperator::GlobalGet(source),
                    StackWriteOperator::I32Const(frame),
                    StackWriteOperator::I32Sub,
                    StackWriteOperator::LocalTee(local),
                ],
            ) if *source == stack_pointer_global => u64::try_from(*frame)
                .ok()
                .filter(|frame| *frame > 0)
                .map(|frame| (frame, Some(*local))),
            _ => match suffix(3) {
                Some(
                    [
                        StackWriteOperator::GlobalGet(source),
                        StackWriteOperator::I32Const(frame),
                        StackWriteOperator::I32Sub,
                    ],
                ) if *source == stack_pointer_global => u64::try_from(*frame)
                    .ok()
                    .filter(|frame| *frame > 0)
                    .map(|frame| (frame, None)),
                _ => None,
            },
        };
        if let Some((frame, local)) = lowered {
            current = current
                .checked_add(frame)
                .context("Remote ROS 2 stack-depth proof overflowed")?;
            peak = peak.max(current);
            active_frames.push((local, frame));
            continue;
        }
        let restored = match (active_frames.last(), suffix(3)) {
            (
                Some((Some(expected_local), expected_frame)),
                Some(
                    [
                        StackWriteOperator::LocalGet(local),
                        StackWriteOperator::I32Const(frame),
                        StackWriteOperator::I32Add,
                    ],
                ),
            ) => *expected_local == *local && i32::try_from(*expected_frame).ok() == Some(*frame),
            _ => false,
        };
        anyhow::ensure!(
            restored,
            "Remote ROS 2 call graph contains an unrecognized write to the real Wasm stack pointer"
        );
        let (_local, frame) = active_frames
            .pop()
            .context("Remote ROS 2 stack restore has no matching lowering")?;
        current = current
            .checked_sub(frame)
            .context("Remote ROS 2 stack-depth proof underflowed")?;
    }
    anyhow::ensure!(
        active_frames.is_empty() && current == 0,
        "Remote ROS 2 call graph leaves the real Wasm stack pointer unbalanced"
    );
    Ok(peak)
}

#[derive(Clone, Copy)]
struct RemoteRos2StackAndCallProof {
    max_stack_bytes: u64,
    has_memory_grow: bool,
    has_unresolved_indirect_call: bool,
}

fn remote_ros2_stack_and_call_proof(
    function_index: u32,
    imported_functions: u32,
    functions: &std::collections::BTreeMap<u32, RemoteRos2WasmFunctionFacts>,
    visiting: &mut std::collections::BTreeSet<u32>,
    completed: &mut std::collections::BTreeMap<u32, RemoteRos2StackAndCallProof>,
) -> anyhow::Result<RemoteRos2StackAndCallProof> {
    if function_index < imported_functions {
        return Ok(RemoteRos2StackAndCallProof {
            max_stack_bytes: 0,
            has_memory_grow: false,
            has_unresolved_indirect_call: false,
        });
    }
    if let Some(proof) = completed.get(&function_index) {
        return Ok(*proof);
    }
    anyhow::ensure!(
        visiting.insert(function_index),
        "Remote ROS 2 initializer probe has a recursive direct-call cycle"
    );
    let facts = functions
        .get(&function_index)
        .with_context(|| format!("Missing Web code body for function {function_index}"))?;
    let mut max_child_stack = 0_u64;
    let mut has_memory_grow = facts.has_memory_grow;
    let mut has_unresolved_indirect_call = facts.has_unresolved_indirect_call;
    for called in &facts.calls {
        let child = remote_ros2_stack_and_call_proof(
            *called,
            imported_functions,
            functions,
            visiting,
            completed,
        )?;
        max_child_stack = max_child_stack.max(child.max_stack_bytes);
        has_memory_grow |= child.has_memory_grow;
        has_unresolved_indirect_call |= child.has_unresolved_indirect_call;
    }
    visiting.remove(&function_index);
    let proof = RemoteRos2StackAndCallProof {
        max_stack_bytes: facts
            .own_stack_bytes
            .checked_add(max_child_stack)
            .context("Remote ROS 2 stack proof overflowed")?,
        has_memory_grow,
        has_unresolved_indirect_call,
    };
    completed.insert(function_index, proof);
    Ok(proof)
}

#[cfg(test)]
mod remote_ros2_allocator_contract_tests {
    use super::*;

    #[test]
    fn verifies_synthetic_call_path_stack_peak_and_fail_closed_variants() {
        verify_remote_ros2_allocator_contract_bytes(&artifact_fixture(
            1_024,
            ArtifactFixtureOptions::valid(),
        ))
        .unwrap();
        let mut cases = Vec::new();
        cases.push((20_000, ArtifactFixtureOptions::valid()));
        let mut no_allocator = ArtifactFixtureOptions::valid();
        no_allocator.allocator_memory_grow = false;
        cases.push((1_024, no_allocator));
        let mut decoy_only = no_allocator;
        decoy_only.decoy_memory_grow = true;
        cases.push((1_024, decoy_only));
        let mut no_marker = ArtifactFixtureOptions::valid();
        no_marker.marker = false;
        cases.push((1_024, no_marker));
        let mut unknown_prologue = ArtifactFixtureOptions::valid();
        unknown_prologue.unknown_prologue = true;
        cases.push((1_024, unknown_prologue));
        let mut wrong_global = ArtifactFixtureOptions::valid();
        wrong_global.stack_pointer_export_global = 1;
        cases.push((1_024, wrong_global));
        let mut aliased_stage = ArtifactFixtureOptions::valid();
        aliased_stage.alias_second_stage = true;
        cases.push((1_024, aliased_stage));
        let mut wrong_stage_identity = ArtifactFixtureOptions::valid();
        wrong_stage_identity.wrong_stage_identity = true;
        cases.push((1_024, wrong_stage_identity));
        let mut nested_stack = ArtifactFixtureOptions::valid();
        nested_stack.nested_frame_bytes = Some(10 * 1024);
        cases.push((10 * 1024, nested_stack));
        let mut detached_protobuf_chain = ArtifactFixtureOptions::valid();
        detached_protobuf_chain.protobuf_omits_ros_stages = true;
        cases.push((1_024, detached_protobuf_chain));
        for omitted_stage in 0..REMOTE_ROS2_REQUIRED_STAGES_V1.len() {
            let mut omitted = ArtifactFixtureOptions::valid();
            omitted.omitted_stage = Some(omitted_stage);
            cases.push((1_024, omitted));
        }
        for (frame, options) in cases {
            assert!(
                verify_remote_ros2_allocator_contract_bytes(&artifact_fixture(frame, options))
                    .is_err()
            );
        }
    }

    #[derive(Clone, Copy)]
    struct ArtifactFixtureOptions {
        allocator_memory_grow: bool,
        decoy_memory_grow: bool,
        marker: bool,
        stack_pointer_export_global: u32,
        unknown_prologue: bool,
        omitted_stage: Option<usize>,
        alias_second_stage: bool,
        wrong_stage_identity: bool,
        nested_frame_bytes: Option<i32>,
        protobuf_omits_ros_stages: bool,
    }

    impl ArtifactFixtureOptions {
        const fn valid() -> Self {
            Self {
                allocator_memory_grow: true,
                decoy_memory_grow: false,
                marker: true,
                stack_pointer_export_global: 0,
                unknown_prologue: false,
                omitted_stage: None,
                alias_second_stage: false,
                wrong_stage_identity: false,
                nested_frame_bytes: None,
                protobuf_omits_ros_stages: false,
            }
        }
    }

    fn artifact_fixture(frame_bytes: i32, options: ArtifactFixtureOptions) -> Vec<u8> {
        use std::borrow::Cow;

        use wasm_encoder::{
            CodeSection, ConstExpr, CustomSection, ExportKind, ExportSection, Function,
            FunctionSection, GlobalSection, GlobalType, Instruction, MemorySection, MemoryType,
            Module, TypeSection, ValType,
        };

        let mut module = Module::new();
        let mut types = TypeSection::new();
        types.ty().function([], [ValType::I32]);
        types.ty().function([], []);
        module.section(&types);
        let mut functions = FunctionSection::new();
        for _index in 0..4 {
            functions.function(0);
        }
        for _stage in &REMOTE_ROS2_REQUIRED_STAGES_V1 {
            functions.function(0);
        }
        for _stage in &REMOTE_PROTOBUF_REQUIRED_STAGES_V1 {
            functions.function(0);
        }
        module.section(&functions);
        let mut memory = MemorySection::new();
        memory.memory(MemoryType {
            minimum: 1,
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        });
        module.section(&memory);
        let mut globals = GlobalSection::new();
        globals.global(
            GlobalType {
                val_type: ValType::I32,
                mutable: true,
                shared: false,
            },
            &ConstExpr::i32_const(65_536),
        );
        globals.global(
            GlobalType {
                val_type: ValType::I32,
                mutable: true,
                shared: false,
            },
            &ConstExpr::i32_const(65_536),
        );
        module.section(&globals);
        let mut exports = ExportSection::new();
        exports.export(REMOTE_ROS2_INITIALIZER_PROBE_V1, ExportKind::Func, 0);
        exports.export(REMOTE_PROTOBUF_INITIALIZER_PROBE_V1, ExportKind::Func, 3);
        exports.export(
            REMOTE_ROS2_ACCOUNTING_ALLOCATOR_PROBE_V1,
            ExportKind::Func,
            1,
        );
        exports.export(
            REMOTE_ROS2_STACK_POINTER_V1,
            ExportKind::Global,
            options.stack_pointer_export_global,
        );
        for (stage_index, stage) in REMOTE_ROS2_REQUIRED_STAGES_V1.iter().enumerate() {
            if options.omitted_stage != Some(stage_index) {
                let function = if options.alias_second_stage && stage_index == 1 {
                    4
                } else {
                    4 + stage_index as u32
                };
                exports.export(stage, ExportKind::Func, function);
            }
        }
        for (stage_index, stage) in REMOTE_PROTOBUF_REQUIRED_STAGES_V1.iter().enumerate() {
            exports.export(
                stage,
                ExportKind::Func,
                4 + REMOTE_ROS2_REQUIRED_STAGES_V1.len() as u32 + stage_index as u32,
            );
        }
        module.section(&exports);
        let mut code = CodeSection::new();
        let mut probe = Function::new([(2, ValType::I32)]);
        probe.instruction(&Instruction::GlobalGet(0));
        probe.instruction(&Instruction::I32Const(frame_bytes));
        probe.instruction(&Instruction::I32Sub);
        probe.instruction(&Instruction::LocalTee(0));
        if options.unknown_prologue {
            probe.instruction(&Instruction::I32Const(0));
            probe.instruction(&Instruction::Drop);
        }
        probe.instruction(&Instruction::GlobalSet(0));
        if let Some(nested_frame_bytes) = options.nested_frame_bytes {
            probe.instruction(&Instruction::GlobalGet(0));
            probe.instruction(&Instruction::I32Const(nested_frame_bytes));
            probe.instruction(&Instruction::I32Sub);
            probe.instruction(&Instruction::LocalTee(1));
            probe.instruction(&Instruction::GlobalSet(0));
        }
        probe.instruction(&Instruction::Call(1));
        probe.instruction(&Instruction::Drop);
        probe.instruction(&Instruction::Call(2));
        for stage_index in 0..REMOTE_ROS2_REQUIRED_STAGES_V1.len() {
            if options.omitted_stage != Some(stage_index) {
                probe.instruction(&Instruction::Call(4 + stage_index as u32));
                probe.instruction(&Instruction::Drop);
            }
        }
        if let Some(nested_frame_bytes) = options.nested_frame_bytes {
            probe.instruction(&Instruction::LocalGet(1));
            probe.instruction(&Instruction::I32Const(nested_frame_bytes));
            probe.instruction(&Instruction::I32Add);
            probe.instruction(&Instruction::GlobalSet(0));
        }
        probe.instruction(&Instruction::LocalGet(0));
        probe.instruction(&Instruction::I32Const(frame_bytes));
        probe.instruction(&Instruction::I32Add);
        probe.instruction(&Instruction::GlobalSet(0));
        probe.instruction(&Instruction::End);
        code.function(&probe);
        let mut allocator = Function::new([]);
        if options.allocator_memory_grow {
            allocator.instruction(&Instruction::I32Const(1));
            allocator.instruction(&Instruction::MemoryGrow(0));
            allocator.instruction(&Instruction::Drop);
        }
        allocator.instruction(&Instruction::I32Const(0));
        allocator.instruction(&Instruction::End);
        code.function(&allocator);
        let mut decoy = Function::new([]);
        if options.decoy_memory_grow {
            decoy.instruction(&Instruction::I32Const(1));
            decoy.instruction(&Instruction::MemoryGrow(0));
            decoy.instruction(&Instruction::Drop);
        }
        decoy.instruction(&Instruction::I32Const(0));
        decoy.instruction(&Instruction::End);
        code.function(&decoy);
        let mut protobuf_probe = Function::new([]);
        protobuf_probe.instruction(&Instruction::Call(1));
        protobuf_probe.instruction(&Instruction::Drop);
        for stage_index in 0..REMOTE_ROS2_REQUIRED_STAGES_V1.len() {
            if !options.protobuf_omits_ros_stages && options.omitted_stage != Some(stage_index) {
                protobuf_probe.instruction(&Instruction::Call(4 + stage_index as u32));
                protobuf_probe.instruction(&Instruction::Drop);
            }
        }
        for stage_index in 0..REMOTE_PROTOBUF_REQUIRED_STAGES_V1.len() {
            protobuf_probe.instruction(&Instruction::Call(
                4 + REMOTE_ROS2_REQUIRED_STAGES_V1.len() as u32 + stage_index as u32,
            ));
            protobuf_probe.instruction(&Instruction::Drop);
        }
        protobuf_probe.instruction(&Instruction::I32Const(0));
        protobuf_probe.instruction(&Instruction::End);
        code.function(&protobuf_probe);
        for (stage_index, _stage) in REMOTE_ROS2_REQUIRED_STAGES_V1.iter().enumerate() {
            let mut stage = Function::new([]);
            let identity = if options.wrong_stage_identity && stage_index == 0 {
                0x7777
            } else {
                REMOTE_ROS2_REQUIRED_STAGE_IDENTITIES_V1[stage_index]
            };
            stage.instruction(&Instruction::I32Const(identity));
            stage.instruction(&Instruction::End);
            code.function(&stage);
        }
        for (stage_index, _stage) in REMOTE_PROTOBUF_REQUIRED_STAGES_V1.iter().enumerate() {
            let mut stage = Function::new([]);
            stage.instruction(&Instruction::I32Const(
                REMOTE_PROTOBUF_REQUIRED_STAGE_IDENTITIES_V1[stage_index],
            ));
            stage.instruction(&Instruction::End);
            code.function(&stage);
        }
        module.section(&code);
        if options.marker {
            module.section(&CustomSection {
                name: Cow::Borrowed(REMOTE_ROS2_ALLOCATOR_SECTION_V1),
                data: Cow::Borrowed(REMOTE_ROS2_ALLOCATOR_CONTRACT_V1),
            });
        }
        module.finish()
    }

    #[test]
    fn product_artifact_and_public_bindings_exclude_verifier_graph() {
        use wasm_encoder::Module;

        let product = Module::new().finish();
        let verifier = artifact_fixture(1_024, ArtifactFixtureOptions::valid());
        verify_remote_ros2_product_artifact_bytes(&product).unwrap();
        assert!(verify_remote_ros2_product_artifact_bytes(&verifier).is_err());
        assert!(product.len() < verifier.len());

        let directory = tempfile::tempdir().unwrap();
        let directory = Utf8Path::from_path(directory.path()).unwrap();
        std::fs::write(directory.join("re_viewer.js"), "export const ready = true;").unwrap();
        std::fs::write(
            directory.join("re_viewer.d.ts"),
            "export interface InitOutput {}",
        )
        .unwrap();
        verify_remote_ros2_public_bindings(directory, "re_viewer").unwrap();
        std::fs::write(
            directory.join("re_viewer.d.ts"),
            format!(
                "export const {}: () => void;",
                REMOTE_ROS2_REQUIRED_STAGES_V1[0]
            ),
        )
        .unwrap();
        assert!(verify_remote_ros2_public_bindings(directory, "re_viewer").is_err());
    }

    #[test]
    fn ordinary_wasm_flags_do_not_enable_probe_or_stack_export() {
        let product = re_build_tools::remote_wasm_contract::canonical_web_rustflags_v1(false);
        assert_eq!(
            product.split('\u{1f}').collect::<Vec<_>>(),
            [
                "--cfg=web_sys_unstable_apis",
                "--cfg=getrandom_backend=\"wasm_js\"",
                "-Ctarget-feature=+simd128,+bulk-memory,+nontrapping-fptoint,+multivalue",
            ]
        );
        assert!(!product.contains("--export=__stack_pointer"));
        let probe = re_build_tools::remote_wasm_contract::canonical_web_rustflags_v1(true);
        assert_eq!(probe.split('\u{1f}').count(), 4);
        assert!(probe.contains("-Clink-arg=--export=__stack_pointer"));

        let root = workspace_root();
        let workspace_config = std::fs::read_to_string(root.join(".cargo/config.toml")).unwrap();
        assert!(!workspace_config.contains("--export=__stack_pointer"));
        let mcap_manifest =
            std::fs::read_to_string(root.join("crates/store/re_mcap/Cargo.toml")).unwrap();
        assert!(!mcap_manifest.contains("re_memory.workspace"));
        let mcap_root =
            std::fs::read_to_string(root.join("crates/store/re_mcap/src/lib.rs")).unwrap();
        assert!(mcap_root.contains("#[cfg(any(test, re_mcap_locked_remote_wasm_allocator_v1))]"));
        assert!(mcap_root.contains("mod remote_protobuf_projection_boundary;"));
        let reflection = std::fs::read_to_string(
            root.join("crates/store/re_mcap/src/remote_ros2_reflection.rs"),
        )
        .unwrap();
        assert!(reflection.contains("re_mcap_remote_ros2_generated_capability_v1.rs"));
        assert!(!reflection.contains("#[path = \"remote_protobuf_projection_boundary.rs\"]"));
        assert!(!reflection.contains("mod remote_protobuf_projection_boundary;"));
        assert!(!reflection.contains("complete_with_protobuf_result_v1"));
        assert!(!reflection.contains("RemoteRos2ProtobufCombinedV1"));
        let protobuf_boundary = std::fs::read_to_string(
            root.join("crates/store/re_mcap/src/remote_protobuf_projection_boundary.rs"),
        )
        .unwrap();
        assert!(protobuf_boundary.contains("RemoteProtobufProjectionEofContinuationV1"));
        assert!(protobuf_boundary.contains("seal_remote_ros2_projection_eof_v1"));
        assert!(protobuf_boundary.contains("#[cfg(test)]\nimpl"));
        assert!(!protobuf_boundary.contains("RemoteProtobufCombinedOwnerV1"));
        assert!(!protobuf_boundary.contains("<P>"));
        assert!(!protobuf_boundary.contains("into_parts_v1"));
        assert!(!protobuf_boundary.contains("protobuf_result_v1"));
        for forbidden_raw_capability in [
            "ValidatedSummaryDefinitions",
            "FixedArena",
            "RemoteRos2ResultReservation",
            "RemoteRos2StepOwnershipV1",
        ] {
            assert!(!protobuf_boundary.contains(forbidden_raw_capability));
        }

        let viewer_web =
            std::fs::read_to_string(root.join("crates/viewer/re_viewer/src/web.rs")).unwrap();
        assert!(viewer_web.contains("re_viewer_remote_ros2_generated_capability_v1.rs"));

        let web_builder = std::fs::read_to_string(
            root.join("crates/build/re_dev_tools/src/build_web_viewer/lib.rs"),
        )
        .unwrap();
        assert!(web_builder.contains("configure_product_wasm_command_v1(&mut cmd)"));
        assert!(web_builder.contains("configure_remote_ros2_verifier_command_v1("));
        assert!(web_builder.contains("Command::new(toolchain.canonical_cargo_v1())"));

        assert_eq!(
            is_canonical_remote_ros2_probe_host_v1(),
            cfg!(all(target_os = "linux", target_arch = "x86_64"))
        );
        assert!(web_builder.contains(
            "profile == Profile::WebRelease && is_canonical_remote_ros2_probe_host_v1()"
        ));
    }

    #[test]
    fn verifier_consumers_require_the_shared_sealed_attestation() {
        let root = workspace_root();
        for relative_path in [
            "crates/store/re_mcap/build.rs",
            "crates/viewer/re_viewer/build.rs",
        ] {
            let build_script = std::fs::read_to_string(root.join(relative_path)).unwrap();
            assert!(
                build_script.contains("remote_ros2_artifact_attestation_v1()"),
                "{relative_path} must verify the artifact contract at its own Cargo boundary"
            );
            assert!(
                build_script.contains("write_remote_ros2_generated_capability_v1("),
                "{relative_path} must generate its attestation-bound source capability"
            );
            assert!(
                !build_script.contains("std::env::var("),
                "{relative_path} must not turn the public environment request directly into cfg"
            );
        }
    }
}
