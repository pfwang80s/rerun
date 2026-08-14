mod lib;

use argh::FromArgs;
use cargo_metadata::camino::Utf8PathBuf;
use lib::{Profile, Target, build, build_mcap_phase_a_proof_v1, default_build_dir};

/// Build the web-viewer.
#[derive(FromArgs)]
#[argh(subcommand, name = "build-web-viewer")]
pub struct Args {
    /// compile for release and run wasm-opt.
    ///
    /// Mutually exclusive with `--debug`.
    /// NOTE: --release also removes debug symbols which are otherwise useful for in-browser profiling.
    #[argh(switch)]
    release: bool,

    /// compile for debug and don't run wasm-opt.
    ///
    /// Mutually exclusive with `--release`.
    #[argh(switch)]
    debug: bool,

    /// keep debug symbols, even in release builds.
    /// This gives better callstacks on panics, and also allows for in-browser profiling of the Wasm.
    #[argh(switch, short = 'g')]
    debug_symbols: bool,

    /// target to build for.
    #[argh(option, short = 't', long = "target", default = "Target::Browser")]
    target: Target,

    /// set the output directory. This is a path relative to the cargo workspace root.
    #[argh(option, short = 'o', long = "out")]
    build_dir: Option<Utf8PathBuf>,

    /// comma-separated list of features to pass on to `re_viewer`
    #[argh(option, short = 'F', long = "features", default = "default_features()")]
    features: String,

    /// whether to exclude default features from `re_viewer` wasm build
    #[argh(switch, long = "no-default-features")]
    no_default_features: bool,

    /// generate a cargo build timings report in `<target-dir>/cargo-timings/`.
    #[argh(switch)]
    timings: bool,

    /// build only the private MCAP Phase A release-Wasm proof artifact into this directory.
    #[argh(option, long = "mcap-phase-a-proof-out")]
    mcap_phase_a_proof_out: Option<Utf8PathBuf>,
}

fn default_features() -> String {
    "analytics".to_owned()
}

pub fn main(args: Args) -> anyhow::Result<()> {
    let profile = if args.release && !args.debug {
        Profile::WebRelease
    } else if !args.release && args.debug {
        Profile::Debug
    } else {
        return Err(anyhow::anyhow!(
            "Exactly one of --release or --debug must be set"
        ));
    };

    if let Some(proof_out) = args.mcap_phase_a_proof_out {
        anyhow::ensure!(
            profile == Profile::WebRelease,
            "MCAP Phase A proof artifacts require --release"
        );
        return build_mcap_phase_a_proof_v1(&proof_out);
    }

    let build_dir = args.build_dir.unwrap_or_else(default_build_dir);

    build(
        profile,
        args.debug_symbols,
        args.target,
        &build_dir,
        args.no_default_features,
        &args.features,
        args.timings,
    )
}
