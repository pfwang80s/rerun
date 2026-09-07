//! Production-disarmed MCAP-114 release-gate audit.
//!
//! This module proves only what can be proven locally from repository shape,
//! Cargo metadata, and selected source boundaries. It intentionally does not
//! run Chrome, `pixi`, `wasm-pack`, `wasm-opt`, a Wasm target, or any real
//! remote-MCAP open/Store/query path.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use cargo_metadata::MetadataCommand;
use serde::{Deserialize, Serialize};

pub const RELEASE_GATE_AUDIT_SCHEMA_V1: &str = "rerun-mcap114-release-gate-audit-v1";
pub const RELEASE_GATE_PROFILE_STATUS_V1: &str = "unfrozen";
pub const RELEASE_GATE_CAPABILITY_STATUS_V1: &str = "production_disarmed";

const REQUIRED_EXTERNAL_GATES_V1: [&str; 6] = [
    "rerun-build-web",
    "web-publication-two-phase-abi-smoke",
    "cargo-nextest",
    "chrome-stable-e2e",
    "release-wasm-symbol-dependency-api-audit",
    "lint",
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseGateAuditCheckStatusV1 {
    Passed,
    Failed,
    NotProven,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalReleaseGateStatusV1 {
    Passed,
    SkippedUnavailable,
    NotProven,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseGateAuditReportV1 {
    pub schema: String,
    pub profile_status: String,
    pub capability_status: String,
    pub workspace_packages: usize,
    pub rust_source_files: usize,
    pub checks: Vec<ReleaseGateAuditCheckV1>,
    pub external_gates: Vec<ExternalReleaseGateV1>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseGateAuditCheckV1 {
    pub id: String,
    pub status: ReleaseGateAuditCheckStatusV1,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalReleaseGateV1 {
    pub id: String,
    pub status: ExternalReleaseGateStatusV1,
    pub tool: String,
    pub reason: String,
}

impl ReleaseGateAuditReportV1 {
    pub fn validate_release_acceptance_v1(&self) -> anyhow::Result<()> {
        self.validate_v1()?;
        if self
            .external_gates
            .iter()
            .any(|gate| gate.status != ExternalReleaseGateStatusV1::Passed)
        {
            bail!("MCAP-114 release acceptance requires every external gate to be passed");
        }
        Ok(())
    }

    pub fn validate_v1(&self) -> anyhow::Result<()> {
        if self.schema != RELEASE_GATE_AUDIT_SCHEMA_V1 {
            bail!("unexpected MCAP-114 release-gate audit schema");
        }
        if self.profile_status != RELEASE_GATE_PROFILE_STATUS_V1 {
            bail!("MCAP-114 release-gate audit must not claim a frozen profile");
        }
        if self.capability_status != RELEASE_GATE_CAPABILITY_STATUS_V1 {
            bail!("MCAP-114 release-gate audit must not claim an armed capability");
        }
        if self.workspace_packages == 0 || self.rust_source_files == 0 || self.checks.is_empty() {
            bail!("MCAP-114 release-gate audit evidence is incomplete");
        }
        if self
            .checks
            .iter()
            .any(|check| check.status == ReleaseGateAuditCheckStatusV1::Failed)
        {
            bail!("MCAP-114 release-gate audit contains a failed local check");
        }

        let mut external_ids = self
            .external_gates
            .iter()
            .map(|gate| gate.id.as_str())
            .collect::<Vec<_>>();
        external_ids.sort_unstable();
        let mut expected_ids = REQUIRED_EXTERNAL_GATES_V1.to_vec();
        expected_ids.sort_unstable();
        if external_ids != expected_ids {
            bail!("MCAP-114 release-gate audit external gates are missing or duplicated");
        }

        let json = serde_json::to_value(self).context("failed to serialize MCAP-114 audit")?;
        if json_contains_secret_material(&json) {
            bail!("MCAP-114 release-gate audit contains unredacted secret material");
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct WorkspaceReleaseGateAudit {
    workspace_root: PathBuf,
    rust_source_files: Vec<PathBuf>,
}

pub fn run_release_gate_audit_v1() -> anyhow::Result<ReleaseGateAuditReportV1> {
    let metadata = MetadataCommand::new()
        .no_deps()
        .exec()
        .context("failed to load workspace metadata")?;
    let workspace_root = metadata.workspace_root.as_std_path().to_path_buf();
    let rust_source_files = rust_source_files(&workspace_root);
    let audit = WorkspaceReleaseGateAudit {
        workspace_root: workspace_root.clone(),
        rust_source_files,
    };

    let checks = vec![
        WorkspaceReleaseGateAudit::check(
            "strict-startup-gate-disarmed",
            ReleaseGateAuditCheckStatusV1::Passed,
            audit.strict_startup_gate_is_disarmed(),
            "strict startup returns the fixed Disarmed gate",
        ),
        WorkspaceReleaseGateAudit::check(
            "re_mcap-no-re_web-dependency",
            ReleaseGateAuditCheckStatusV1::Passed,
            !package_depends_on(&metadata, "re_mcap", "re_web"),
            "re_mcap has no re_web dependency",
        ),
        WorkspaceReleaseGateAudit::check(
            "re_web-no-re_mcap-dependency",
            ReleaseGateAuditCheckStatusV1::Passed,
            !package_depends_on(&metadata, "re_web", "re_mcap"),
            "re_web has no re_mcap dependency",
        ),
        WorkspaceReleaseGateAudit::check(
            "only-approved-cross-bridge",
            ReleaseGateAuditCheckStatusV1::Passed,
            only_allowed_cross_crate_bridge(&metadata),
            "only re_viewer, the re_mcap_web_adapter producer, and the Phase A artifact are approved cross bridges",
        ),
        WorkspaceReleaseGateAudit::check(
            "remote-side-map-literal-pattern-scan",
            ReleaseGateAuditCheckStatusV1::Passed,
            audit.remote_side_map_growth_uses_point_lookups_only(),
            "literal-pattern scan found no known legacy-map iteration in selected remote growth functions; this is not release-artifact proof",
        ),
        WorkspaceReleaseGateAudit::check(
            "intern-constructors-remain",
            ReleaseGateAuditCheckStatusV1::Passed,
            audit.intern_constructors_remain(),
            "existing InternedString, newtype, and serde constructors remain present",
        ),
        WorkspaceReleaseGateAudit::check(
            "nonremote-routes-have-no-remote-owner",
            ReleaseGateAuditCheckStatusV1::Passed,
            audit.nonremote_routes_have_no_remote_owner(),
            "compatibility nonremote routes fall through before remote dispatch or slot claim",
        ),
    ];

    let report = ReleaseGateAuditReportV1 {
        schema: RELEASE_GATE_AUDIT_SCHEMA_V1.to_owned(),
        profile_status: RELEASE_GATE_PROFILE_STATUS_V1.to_owned(),
        capability_status: RELEASE_GATE_CAPABILITY_STATUS_V1.to_owned(),
        workspace_packages: metadata.packages.len(),
        rust_source_files: audit.rust_source_files.len(),
        checks,
        external_gates: external_release_gates(),
    };
    report.validate_v1()?;
    Ok(report)
}

impl WorkspaceReleaseGateAudit {
    fn check(
        id: &str,
        expected_status: ReleaseGateAuditCheckStatusV1,
        passed: bool,
        detail: &str,
    ) -> ReleaseGateAuditCheckV1 {
        ReleaseGateAuditCheckV1 {
            id: id.to_owned(),
            status: if passed {
                expected_status
            } else {
                ReleaseGateAuditCheckStatusV1::Failed
            },
            detail: detail.to_owned(),
        }
    }

    fn file(&self, relative: &str) -> anyhow::Result<PathBuf> {
        let path = self.workspace_root.join(relative);
        anyhow::ensure!(path.is_file(), "missing audit source file {relative}");
        Ok(path)
    }

    fn text(&self, relative: &str) -> anyhow::Result<String> {
        let path = self.file(relative)?;
        std::fs::read_to_string(path).with_context(|| format!("failed to read {relative}"))
    }

    fn strict_startup_gate_is_disarmed(&self) -> bool {
        let Ok(text) = self.text("crates/viewer/re_viewer/src/web_strict_startup.rs") else {
            return false;
        };
        let Some(body) = function_body(
            &text,
            "fn strict_startup_capability_gate_v1() -> StrictStartupCapabilityGateV1",
        ) else {
            return false;
        };
        body.contains("StrictStartupCapabilityGateV1::Disarmed") && !body.contains("Armed")
    }

    fn remote_side_map_growth_uses_point_lookups_only(&self) -> bool {
        let Ok(text) = self.text("crates/utils/re_string_interner/src/bounded_runtime_intern.rs")
        else {
            return false;
        };
        const FORBIDDEN_LEGACY_SCAN_PATTERNS: &[&str] = &[
            "self.legacy.map.iter()",
            "self.legacy.map.values()",
            "self.legacy.map.keys()",
            "self.legacy.map.drain(",
            "self.legacy.map.clone()",
            "self.legacy.map.into_iter()",
            "self.legacy.bytes_used()",
        ];
        for signature in [
            "fn checked_lookup(",
            "fn plan_census(",
            "fn prepare_remote(",
            "fn commit_remote(",
        ] {
            let Some(body) = function_body(&text, signature) else {
                return false;
            };
            if FORBIDDEN_LEGACY_SCAN_PATTERNS
                .iter()
                .any(|pattern| body.contains(pattern))
                || body.lines().any(legacy_iteration_line)
            {
                return false;
            }
        }
        true
    }

    fn intern_constructors_remain(&self) -> bool {
        let Ok(text) = self.text("crates/utils/re_string_interner/src/lib.rs") else {
            return false;
        };
        text.contains("pub fn new(string: &str) -> Self")
            && text.contains("impl serde::Serialize for InternedString")
            && text.contains("impl<'de> serde::Deserialize<'de> for InternedString")
            && text.contains("macro_rules! declare_new_type")
            && text.contains("macro_rules! declare_new_type_nonempty")
            && text.contains("pub fn try_new(")
            && text.contains("pub fn try_from_interned(")
            && text.contains("fn global_intern(")
    }

    fn nonremote_routes_have_no_remote_owner(&self) -> bool {
        let Ok(web_startup) = self.text("crates/viewer/re_viewer/src/web_startup.rs") else {
            return false;
        };
        let Some(dispatch_body) =
            function_body(&web_startup, "pub(crate) fn dispatch_compatibility_url_v1(")
        else {
            return false;
        };
        let nonremote_branch = dispatch_body
            .find("if !is_explicit_remote_mcap")
            .is_some_and(|nonremote_pos| {
                dispatch_body
                    .find("remote_dispatch()")
                    .is_some_and(|remote_pos| nonremote_pos < remote_pos)
            });
        let remote_calls = dispatch_body.matches("remote_dispatch()").count();
        let nonremote_branch_returns_existing = dispatch_body.contains("open_existing(parsed);")
            && dispatch_body.contains("CompatibilityUrlDispatchOutcomeV1::ExistingDispatcher");
        if !nonremote_branch || remote_calls != 1 || !nonremote_branch_returns_existing {
            return false;
        }

        let Ok(compatibility_open) = self.text("crates/utils/re_web/src/compatibility_open.rs")
        else {
            return false;
        };
        let Some(new_disarmed_body) =
            function_body(&compatibility_open, "pub fn new_disarmed_v1() -> Self")
        else {
            return false;
        };
        let Some(dispatch_v1_body) = function_body(
            &compatibility_open,
            "pub fn dispatch_v1(&mut self) -> CompatibilityRemoteMcapDispatchV1",
        ) else {
            return false;
        };
        new_disarmed_body.contains("capability_installed: false")
            && dispatch_v1_body.contains("if !self.capability_installed")
            && dispatch_v1_body
                .contains("return CompatibilityRemoteMcapDispatchV1::ExistingDispatcher;")
    }
}

fn legacy_iteration_line(line: &str) -> bool {
    line.contains("for ") && line.contains("legacy")
}

fn function_body<'a>(text: &'a str, signature: &str) -> Option<&'a str> {
    let signature_pos = text.find(signature)?;
    let after_signature = &text[signature_pos + signature.len()..];
    let open_brace = after_signature.find('{')?;
    let body_start = signature_pos + signature.len() + open_brace + 1;
    let mut depth = 1_u32;
    for (offset, character) in text[body_start..].char_indices() {
        match character {
            '{' => depth = depth.checked_add(1)?,
            '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(&text[body_start..body_start + offset]);
                }
            }
            _ => {}
        }
    }
    None
}

fn package_depends_on(
    metadata: &cargo_metadata::Metadata,
    package_name: &str,
    dependency: &str,
) -> bool {
    metadata
        .packages
        .iter()
        .find(|package| package.name == package_name)
        .is_some_and(|package| {
            package
                .dependencies
                .iter()
                .any(|dep| dep.name == dependency)
        })
}

fn only_allowed_cross_crate_bridge(metadata: &cargo_metadata::Metadata) -> bool {
    let Some(viewer) = metadata
        .packages
        .iter()
        .find(|package| package.name == "re_viewer")
    else {
        return false;
    };
    let viewer_has_both_wasm_deps = ["re_mcap", "re_web"].iter().all(|dependency| {
        viewer.dependencies.iter().any(|dep| {
            dep.name == *dependency
                && dep
                    .target
                    .as_ref()
                    .is_some_and(|target| target.to_string() == "cfg(target_arch = \"wasm32\")")
        })
    });
    // Approved cross-layer producers that are allowed to depend on both owning crates:
    // `re_viewer` (existing product consumer), `re_mcap_web_adapter` (the sole trusted
    // cross-layer producer created by MCAP-114 W03), and the existing Phase A artifact.
    let approved_bridges = ["re_viewer", "re_mcap_web_adapter", "re_mcap_phase_a_chrome"];
    viewer_has_both_wasm_deps
        && metadata
            .packages
            .iter()
            .filter(|package| !approved_bridges.contains(&package.name.as_str()))
            .all(|package| {
                let has_re_mcap = package.dependencies.iter().any(|dep| dep.name == "re_mcap");
                let has_re_web = package.dependencies.iter().any(|dep| dep.name == "re_web");
                !(has_re_mcap && has_re_web)
            })
}

fn rust_source_files(workspace_root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for root in [
        workspace_root.join("crates"),
        workspace_root.join("tests/rust"),
    ] {
        collect_rust_source_files(&root, &mut files);
    }
    files.sort();
    files
}

fn collect_rust_source_files(root: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_rust_source_files(&path, files);
        } else if file_type.is_file() && path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
}

fn command_is_available(command: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|directory| directory.join(command).is_file())
}

fn external_release_gates() -> Vec<ExternalReleaseGateV1> {
    let pixi_available = command_is_available("pixi");
    let wasm_pack_available = command_is_available("wasm-pack");
    let wasm_opt_available = command_is_available("wasm-opt");
    let chrome_browser_available = [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
    ]
    .iter()
    .any(|command| command_is_available(command));
    let chromedriver_available = command_is_available("chromedriver");

    vec![
        external_gate(
            "rerun-build-web",
            &["pixi"],
            pixi_available,
            "native audit does not execute web release builds",
        ),
        external_gate(
            "web-publication-two-phase-abi-smoke",
            &["pixi", "wasm-pack"],
            pixi_available && wasm_pack_available,
            "native audit does not execute Web publication or two-phase ABI smoke",
        ),
        external_gate(
            "cargo-nextest",
            &["cargo-nextest"],
            command_is_available("cargo-nextest"),
            "native audit does not run workspace nextest",
        ),
        external_gate(
            "chrome-stable-e2e",
            &[
                "google-chrome-stable|google-chrome|chromium|chromium-browser",
                "chromedriver",
            ],
            chrome_browser_available && chromedriver_available,
            "native audit does not launch a browser",
        ),
        external_gate(
            "release-wasm-symbol-dependency-api-audit",
            &["wasm-pack", "wasm-opt"],
            wasm_pack_available && wasm_opt_available,
            "source audit cannot prove final release-Wasm symbols, dependencies, or exported API",
        ),
        external_gate(
            "lint",
            &["pixi"],
            pixi_available,
            "native audit does not run workspace lint",
        ),
    ]
}

fn external_gate(
    id: &str,
    tools: &[&str],
    tools_available: bool,
    not_proven_reason: &str,
) -> ExternalReleaseGateV1 {
    ExternalReleaseGateV1 {
        id: id.to_owned(),
        status: if tools_available {
            ExternalReleaseGateStatusV1::NotProven
        } else {
            ExternalReleaseGateStatusV1::SkippedUnavailable
        },
        tool: tools.join("|"),
        reason: if tools_available {
            not_proven_reason.to_owned()
        } else {
            "required tool or runner is not available locally".to_owned()
        },
    }
}

fn json_contains_secret_material(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {
            false
        }
        serde_json::Value::String(value) => {
            let lower = value.to_ascii_lowercase();
            [
                "http://",
                "https://",
                ".mcap?",
                "etag:",
                "topic=",
                "topic:",
                "entity path:",
                "entity_path:",
                "store id:",
                "store_id:",
                "token=",
                "internal_token:",
                "fixture nonce:",
            ]
            .iter()
            .any(|needle| lower.contains(needle))
        }
        serde_json::Value::Array(values) => values.iter().any(json_contains_secret_material),
        serde_json::Value::Object(fields) => fields.values().any(json_contains_secret_material),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_report() -> ReleaseGateAuditReportV1 {
        ReleaseGateAuditReportV1 {
            schema: RELEASE_GATE_AUDIT_SCHEMA_V1.to_owned(),
            profile_status: RELEASE_GATE_PROFILE_STATUS_V1.to_owned(),
            capability_status: RELEASE_GATE_CAPABILITY_STATUS_V1.to_owned(),
            workspace_packages: 1,
            rust_source_files: 1,
            checks: vec![ReleaseGateAuditCheckV1 {
                id: "strict-startup-gate-disarmed".to_owned(),
                status: ReleaseGateAuditCheckStatusV1::Passed,
                detail: "strict startup returns the fixed Disarmed gate".to_owned(),
            }],
            external_gates: REQUIRED_EXTERNAL_GATES_V1
                .into_iter()
                .map(|id| ExternalReleaseGateV1 {
                    id: id.to_owned(),
                    status: ExternalReleaseGateStatusV1::NotProven,
                    tool: "tool".to_owned(),
                    reason: "not run by the synthetic report".to_owned(),
                })
                .collect(),
        }
    }

    #[test]
    fn fixed_schema_accepts_disarmed_report_but_release_rejects_unproven_gates() {
        valid_report().validate_v1().unwrap();
        assert!(valid_report().validate_release_acceptance_v1().is_err());

        let mut invalid = valid_report();
        invalid.schema = "future-schema".to_owned();
        assert!(invalid.validate_v1().is_err());

        let mut invalid = valid_report();
        invalid.capability_status = "armed".to_owned();
        assert!(invalid.validate_v1().is_err());

        let mut invalid = valid_report();
        invalid.profile_status = "frozen".to_owned();
        assert!(invalid.validate_v1().is_err());

        let mut invalid = valid_report();
        invalid.checks[0].status = ReleaseGateAuditCheckStatusV1::Failed;
        assert!(invalid.validate_v1().is_err());

        let mut invalid = valid_report();
        invalid.external_gates.pop();
        assert!(invalid.validate_v1().is_err());
    }

    #[test]
    fn unknown_fields_and_secret_material_are_rejected() {
        let mut json = serde_json::to_value(valid_report()).unwrap();
        json.as_object_mut()
            .unwrap()
            .insert("extra".to_owned(), true.into());
        assert!(serde_json::from_value::<ReleaseGateAuditReportV1>(json).is_err());

        let secret = serde_json::json!({
            "url": "https://example.test/data.mcap?token=secret"
        });
        assert!(json_contains_secret_material(&secret));
        assert!(json_contains_secret_material(&serde_json::json!({
            "value": "HTTP://example.test/data.mcap?token=secret"
        })));
        assert!(json_contains_secret_material(&serde_json::json!(
            "etag: opaque"
        )));
        assert!(json_contains_secret_material(&serde_json::json!(
            "topic=opaque"
        )));
        assert!(json_contains_secret_material(&serde_json::json!(
            "entity_path: /secret"
        )));
        assert!(json_contains_secret_material(&serde_json::json!(
            "store_id: opaque"
        )));
        assert!(json_contains_secret_material(&serde_json::json!(
            "internal_token: opaque"
        )));
        assert!(!json_contains_secret_material(&serde_json::json!(
            "redacted aggregate"
        )));
    }

    #[test]
    fn runner_report_is_disarmed_and_has_no_failed_checks() {
        let report = run_release_gate_audit_v1().expect("native MCAP-114 audit should run");
        report.validate_v1().unwrap();
        assert_eq!(report.capability_status, RELEASE_GATE_CAPABILITY_STATUS_V1);
        assert!(
            report
                .checks
                .iter()
                .all(|check| check.status != ReleaseGateAuditCheckStatusV1::Failed)
        );
        assert!(
            report
                .external_gates
                .iter()
                .all(|gate| gate.status != ExternalReleaseGateStatusV1::Passed)
        );
    }

    #[test]
    fn adapter_is_an_approved_cross_bridge_in_workspace_metadata() {
        // The MCAP-114 architecture approves `re_viewer`, `re_mcap_web_adapter`, and
        // `re_mcap_phase_a_chrome` as the only crates that may depend on both `re_mcap` and
        // `re_web`. The adapter is the sole trusted cross-layer producer, so the bridge check
        // must pass on the real workspace metadata.
        let metadata = MetadataCommand::new().no_deps().exec().unwrap();
        assert!(only_allowed_cross_crate_bridge(&metadata));
        let adapter = metadata
            .packages
            .iter()
            .find(|package| package.name == "re_mcap_web_adapter")
            .expect("re_mcap_web_adapter is a workspace member");
        let has_re_mcap = adapter.dependencies.iter().any(|dep| dep.name == "re_mcap");
        let has_re_web = adapter.dependencies.iter().any(|dep| dep.name == "re_web");
        assert!(
            has_re_mcap && has_re_web,
            "adapter must bridge both owning crates"
        );
    }
}
