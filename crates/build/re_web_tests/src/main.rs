//! Discovers and runs Rerun web tests.

use std::path::PathBuf;
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
    browsers: Vec<Browser>,
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
    let mcap_range_server = if package.mcap_range_server {
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
        server.shutdown().await;
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
}
