//! Discovers and runs Rerun web tests.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr as _;

use anyhow::{Context as _, bail};
use cargo_metadata::{DependencyKind, MetadataCommand};
use tokio::process::Command;

#[derive(argh::FromArgs)]
/// Discover and run workspace web tests.
struct Args {
    /// browser to pass to wasm-pack: firefox or chrome.
    #[argh(option, default = "Browser::Firefox")]
    browser: Browser,

    /// run tests for a single package.
    #[argh(option)]
    package: Option<String>,

    /// run wasm-bindgen-test in a visible browser.
    #[argh(switch)]
    no_headless: bool,

    /// directory containing an optimized MCAP Phase A proof artifact.
    #[argh(option)]
    phase_a_proof_dir: Option<PathBuf>,

    /// write validated MCAP Phase A evidence JSON to this path.
    #[argh(option)]
    phase_a_artifact_out: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Browser {
    Firefox,
    Chrome,
}

impl Browser {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Firefox => "firefox",
            Self::Chrome => "chrome",
        }
    }
}

impl std::fmt::Display for Browser {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for Browser {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "firefox" => Ok(Self::Firefox),
            "chrome" => Ok(Self::Chrome),
            _ => Err(format!(
                "unsupported browser {value:?}; expected firefox or chrome"
            )),
        }
    }
}

struct WebTestPackage {
    name: String,
    path: PathBuf,
    redap_server: bool,
    mcap_range_server: bool,
    phase_a_benchmark: bool,
    browsers: Vec<Browser>,
}

fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;

    for _ in 0..16 {
        let temporary = parent.join(format!(
            ".rerun-mcap-phase-a-{}-{:016x}.tmp",
            std::process::id(),
            rand::random::<u64>()
        ));
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        };
        let write_result = file.write_all(bytes).and_then(|()| file.sync_all());
        drop(file);
        if let Err(error) = write_result {
            _ = std::fs::remove_file(&temporary);
            return Err(error.into());
        }
        if let Err(error) = std::fs::rename(&temporary, path) {
            _ = std::fs::remove_file(&temporary);
            return Err(error.into());
        }
        return Ok(());
    }
    bail!("failed to allocate a temporary Phase A evidence artifact")
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> anyhow::Result<()> {
    let args: Args = argh::from_env();

    let packages = discover_packages(args.package.as_deref())?;
    if packages.is_empty() {
        bail!("found no web-test packages");
    }

    let mut executed_packages = 0_usize;
    for package in packages {
        if !should_execute_package(&package, args.browser, args.package.is_some())? {
            eprintln!(
                "Skipping {}: browser {:?} is not in {:?}",
                package.name,
                args.browser.as_str(),
                package
                    .browsers
                    .iter()
                    .map(|browser| browser.as_str())
                    .collect::<Vec<_>>()
            );
            continue;
        }
        run_package(&args, &package).await?;
        executed_packages += 1;
    }
    require_executed_packages(executed_packages, args.browser)?;
    eprintln!(
        "Completed {executed_packages} web-test package command(s) in {}",
        args.browser
    );

    Ok(())
}

fn package_supports_browser(package: &WebTestPackage, browser: Browser) -> bool {
    package.browsers.contains(&browser)
}

fn should_execute_package(
    package: &WebTestPackage,
    browser: Browser,
    explicitly_requested: bool,
) -> anyhow::Result<bool> {
    if package_supports_browser(package, browser) {
        return Ok(true);
    }
    if explicitly_requested {
        bail!(
            "package {:?} does not support browser {:?}; supported browsers: {:?}",
            package.name,
            browser.as_str(),
            package
                .browsers
                .iter()
                .map(|browser| browser.as_str())
                .collect::<Vec<_>>()
        );
    }
    Ok(false)
}

fn require_executed_packages(executed_packages: usize, browser: Browser) -> anyhow::Result<()> {
    if executed_packages == 0 {
        bail!("no web-test package commands executed for browser {browser}");
    }
    Ok(())
}

fn discover_packages(package_filter: Option<&str>) -> anyhow::Result<Vec<WebTestPackage>> {
    let metadata = MetadataCommand::new().no_deps().exec()?;

    let workspace_members = metadata.workspace_members;
    let mut packages = metadata
        .packages
        .into_iter()
        .filter(|package| workspace_members.contains(&package.id))
        .filter(|package| package_filter.is_none_or(|filter| package.name == filter))
        .filter(|package| {
            package.dependencies.iter().any(|dep| {
                dep.name == "wasm-bindgen-test" && dep.kind == DependencyKind::Development
            })
        })
        .map(|package| {
            Ok(WebTestPackage {
                name: package.name.to_string(),
                path: package
                    .manifest_path
                    .parent()
                    .context("package manifest has no parent")?
                    .to_path_buf()
                    .into_std_path_buf(),
                redap_server: metadata_bool(&package.metadata, "redap-server")?,
                mcap_range_server: metadata_bool(&package.metadata, "mcap-range-server")?,
                phase_a_benchmark: metadata_bool(&package.metadata, "phase-a-benchmark")?,
                browsers: metadata_browsers(&package.metadata)?,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    if let Some(package_filter) = package_filter
        && packages.is_empty()
    {
        bail!("package {package_filter:?} is not a discovered web-test package");
    }

    packages.sort_by(|lhs, rhs| lhs.name.cmp(&rhs.name));
    Ok(packages)
}

fn metadata_bool(metadata: &serde_json::Value, key: &str) -> anyhow::Result<bool> {
    let pointer = format!("/rerun/web-test/{key}");
    let Some(value) = metadata.pointer(&pointer) else {
        return Ok(false);
    };
    value
        .as_bool()
        .with_context(|| format!("package metadata {pointer:?} must be a boolean"))
}

fn metadata_browsers(metadata: &serde_json::Value) -> anyhow::Result<Vec<Browser>> {
    let pointer = "/rerun/web-test/browsers";
    let Some(value) = metadata.pointer(pointer) else {
        return Ok(vec![Browser::Firefox, Browser::Chrome]);
    };
    let values = value
        .as_array()
        .with_context(|| format!("package metadata {pointer:?} must be an array"))?;
    if values.is_empty() {
        bail!("package metadata {pointer:?} must not be empty");
    }
    let mut browsers = Vec::with_capacity(values.len());
    for value in values {
        let browser = value
            .as_str()
            .with_context(|| format!("package metadata {pointer:?} entries must be strings"))?;
        let browser = Browser::from_str(browser)
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("package metadata {pointer:?} contains an invalid browser"))?;
        if browsers.contains(&browser) {
            bail!("package metadata {pointer:?} contains duplicate browser {browser:?}");
        }
        browsers.push(browser);
    }
    Ok(browsers)
}

async fn run_package(args: &Args, package: &WebTestPackage) -> anyhow::Result<()> {
    if args.no_headless && (package.redap_server || package.mcap_range_server) {
        // Interactive mode parses `WASM_BINDGEN_TEST_ADDRESS` as a socket address, so it
        // cannot carry the native-server URL query parameters used by headless tests.
        bail!(
            "package {:?} requires native test origins, which are only supported in headless mode",
            package.name
        );
    }

    eprintln!("Running web tests for {}", package.name);

    // Spawn the fixture with a synchronous `Drop` fallback first.
    // If the Redap server subsequently fails to start, the fixture still closes both ports.
    let mcap_range_server = if package.phase_a_benchmark {
        let proof_dir = args
            .phase_a_proof_dir
            .as_deref()
            .context("Phase A benchmark package requires --phase-a-proof-dir")?;
        Some(
            re_web_tests::mcap_range_server::McapRangeTestServer::spawn_with_phase_a_proof_v1(
                proof_dir,
            )
            .await?,
        )
    } else if package.mcap_range_server {
        Some(re_web_tests::mcap_range_server::McapRangeTestServer::spawn().await?)
    } else {
        None
    };
    let server = if package.redap_server {
        Some(
            re_server::Args {
                host: "127.0.0.1".to_owned(),
                port: 0,
                ..Default::default()
            }
            .create_server_handle()
            .await?,
        )
    } else {
        None
    };
    let mut command = Command::new("wasm-pack");
    command.arg("test");
    if !args.no_headless {
        command.arg("--headless");
    }
    command.arg(format!("--{}", args.browser.as_str()));
    command.arg(&package.path);

    let mut address_parameters = Vec::new();
    if let Some(server) = &server {
        address_parameters.push(format!("redap_port={}", server.connect_addr().port()));
    }
    if let Some(server) = &mcap_range_server {
        // The nonce is intentionally discovered from the loopback bootstrap endpoint,
        // rather than copied into the browser test page's query string.
        address_parameters.push(format!(
            "mcap_fixture_page_port={}",
            server.page_addr().port()
        ));
        address_parameters.push(format!(
            "mcap_fixture_object_port={}",
            server.object_addr().port()
        ));
    }
    if address_parameters.is_empty() {
        command.env_remove("WASM_BINDGEN_TEST_ADDRESS");
    } else {
        command.env(
            "WASM_BINDGEN_TEST_ADDRESS",
            format!("http://127.0.0.1/?{}", address_parameters.join("&")),
        );
    }

    let status = command.status().await;

    if let Some(server) = server {
        server.shutdown_and_wait().await;
    }
    if let Some(server) = mcap_range_server {
        let artifact_result = (|| -> anyhow::Result<()> {
            if package.phase_a_benchmark
                && status.as_ref().is_ok_and(std::process::ExitStatus::success)
            {
                let evidence = server
                    .phase_a_evidence_v1()
                    .context("Chrome benchmark returned no Phase A evidence")?;
                let artifact_out = args
                    .phase_a_artifact_out
                    .as_deref()
                    .context("Phase A benchmark package requires --phase-a-artifact-out")?;
                let bytes = serde_json::to_vec_pretty(&evidence)?;
                write_atomic(artifact_out, &bytes)?;
            }
            Ok(())
        })();
        server.shutdown().await;
        artifact_result?;
    }

    let status = status.with_context(|| format!("failed to run wasm-pack for {}", package.name))?;

    if !status.success() {
        bail!("web tests failed for {}", package.name);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use argh::FromArgs as _;

    use super::*;

    fn test_package(name: &str, browsers: Vec<Browser>) -> WebTestPackage {
        WebTestPackage {
            name: name.to_owned(),
            path: PathBuf::from("fixture"),
            redap_server: false,
            mcap_range_server: false,
            phase_a_benchmark: false,
            browsers,
        }
    }

    #[test]
    fn native_server_metadata_requires_booleans() {
        let absent = serde_json::json!({});
        assert!(!metadata_bool(&absent, "mcap-range-server").expect("absent is false"));
        let enabled = serde_json::json!({
            "rerun": { "web-test": { "mcap-range-server": true } }
        });
        assert!(metadata_bool(&enabled, "mcap-range-server").expect("boolean metadata"));

        for invalid in [
            serde_json::json!(null),
            serde_json::json!("true"),
            serde_json::json!(1),
            serde_json::json!([]),
            serde_json::json!({}),
        ] {
            let metadata = serde_json::json!({
                "rerun": { "web-test": { "mcap-range-server": invalid } }
            });
            assert!(metadata_bool(&metadata, "mcap-range-server").is_err());
        }
    }

    #[test]
    fn browser_metadata_is_typed_and_bounded() {
        let default = metadata_browsers(&serde_json::json!({})).expect("browser defaults");
        assert_eq!(default, [Browser::Firefox, Browser::Chrome]);
        let chrome = serde_json::json!({
            "rerun": { "web-test": { "browsers": ["chrome"] } }
        });
        assert_eq!(
            metadata_browsers(&chrome).expect("Chrome metadata"),
            [Browser::Chrome]
        );

        for invalid in [
            serde_json::json!([]),
            serde_json::json!(["chrome", "chrome"]),
            serde_json::json!(["safari"]),
            serde_json::json!([1]),
            serde_json::json!("chrome"),
        ] {
            let metadata = serde_json::json!({
                "rerun": { "web-test": { "browsers": invalid } }
            });
            assert!(metadata_browsers(&metadata).is_err());
        }
    }

    #[test]
    fn browser_cli_and_execution_selection_never_false_green() {
        let default = Args::from_args(&["re_web_tests"], &[]).expect("default browser");
        assert_eq!(default.browser, Browser::Firefox);
        let chrome =
            Args::from_args(&["re_web_tests"], &["--browser", "chrome"]).expect("Chrome CLI value");
        assert_eq!(chrome.browser, Browser::Chrome);

        for invalid in ["safari", "chorme"] {
            assert!(
                Args::from_args(&["re_web_tests"], &["--browser", invalid]).is_err(),
                "invalid browser {invalid:?} was accepted"
            );
        }

        let chrome_only = test_package("chrome-only", vec![Browser::Chrome]);
        assert!(package_supports_browser(&chrome_only, Browser::Chrome));
        assert!(!package_supports_browser(&chrome_only, Browser::Firefox));
        assert!(
            should_execute_package(&chrome_only, Browser::Firefox, true).is_err(),
            "explicit Chrome-only package was silently filtered from Firefox"
        );
        assert!(
            !should_execute_package(&chrome_only, Browser::Firefox, false)
                .expect("implicit unsupported package should be skipped")
        );
        assert!(require_executed_packages(1, Browser::Chrome).is_ok());
        assert!(require_executed_packages(0, Browser::Chrome).is_err());
    }

    #[test]
    fn phase_a_evidence_artifact_is_replaced_atomically() {
        let directory = tempfile::tempdir().expect("artifact directory");
        let artifact = directory.path().join("evidence.json");
        write_atomic(&artifact, b"first").expect("first atomic write");
        assert_eq!(std::fs::read(&artifact).expect("first artifact"), b"first");
        write_atomic(&artifact, b"second").expect("replacement atomic write");
        assert_eq!(
            std::fs::read(&artifact).expect("replacement artifact"),
            b"second"
        );
        let entries = std::fs::read_dir(directory.path())
            .expect("artifact directory")
            .collect::<Result<Vec<_>, _>>()
            .expect("artifact entries");
        assert_eq!(entries.len(), 1);
    }
}
