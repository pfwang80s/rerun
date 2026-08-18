//! Local Phase A exit-condition audit for the disarmed remote-MCAP boundary.
//!
//! This module intentionally checks repository shape and source/dependency invariants. It does not
//! build or run Chrome/Wasm, and it emits only redacted aggregate evidence.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use cargo_metadata::MetadataCommand;
use serde::{Deserialize, Serialize};

pub const PHASE_A_AUDIT_SCHEMA_V1: &str = "rerun-mcap-phase-a-audit-v1";
const PHASE_A_PROFILE_STATUS_V1: &str = "unfrozen";
const PHASE_A_BODY_TRANSFER_STRATEGY_V1: &str = "zero-copy-v1";

const RE_MCAP_REMOTE_MODULES: &[&str] = &[
    "web_body_handoff",
    "remote_time",
    "remote_fixed_layout",
    "remote_summary",
    "remote_decompression",
    "remote_chunk_scan",
    "remote_physical_resolution",
    "remote_protobuf_projection_boundary",
    "remote_protobuf_descriptor",
    "remote_decoder_assignment",
    "remote_deterministic_insertion",
    "remote_channel_group",
    "remote_chunk_validation_count",
    "remote_manifest",
    "remote_loaded_coverage",
    "remote_partition_residency",
    "remote_chunk_dispatch",
    "remote_typed_output",
    "remote_runtime_intern",
    "remote_ros2_reflection",
];

const RE_WEB_REMOTE_MODULES: &[&str] = &[
    "chrome_byob",
    "chrome_range",
    "compatibility_open",
    "external_string_ingress",
    "format_sniffer",
    "open_lifecycle_delivery",
    "open_lifecycle_registry",
    "open_lifecycle_sequencer",
    "open_source_client_registry",
    "open_source_terminal",
    "range_retry",
    "remote_limits",
    "remote_mutation_suspension",
    "remote_page_teardown",
    "remote_resume_revalidation",
    "remote_validator",
    "secret_url",
    "source_reuse",
    "store_publication",
    "strict_open_batch",
    "strict_open_handoff",
    "strict_open_wire",
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhaseAAuditReportV1 {
    pub schema: String,
    pub profile_status: String,
    pub body_transfer_strategy: String,
    pub workspace_packages: usize,
    pub source_files: usize,
    pub checks: Vec<PhaseAAuditCheckV1>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhaseAAuditCheckV1 {
    pub id: String,
    pub passed: bool,
    pub detail: String,
}

impl PhaseAAuditReportV1 {
    pub fn validate_v1(&self) -> anyhow::Result<()> {
        if self.schema != PHASE_A_AUDIT_SCHEMA_V1 {
            bail!("unexpected Phase A audit schema");
        }
        if self.profile_status != PHASE_A_PROFILE_STATUS_V1 {
            bail!("Phase A audit must not claim a frozen profile");
        }
        if self.body_transfer_strategy != PHASE_A_BODY_TRANSFER_STRATEGY_V1 {
            bail!("Phase A body transfer strategy is not the disarmed zero-copy strategy");
        }
        if self.workspace_packages == 0 || self.source_files == 0 || self.checks.is_empty() {
            bail!("Phase A audit evidence is incomplete");
        }
        if self.checks.iter().any(|check| !check.passed) {
            bail!("Phase A audit contains a failed check");
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct WorkspaceAudit {
    workspace_root: PathBuf,
    source_files: Vec<PathBuf>,
}

pub fn run_phase_a_audit_v1() -> anyhow::Result<PhaseAAuditReportV1> {
    let metadata = MetadataCommand::new()
        .no_deps()
        .exec()
        .context("failed to load workspace metadata")?;
    let workspace_root = metadata.workspace_root.as_std_path().to_path_buf();
    let source_files = source_files(&workspace_root);
    let audit = WorkspaceAudit {
        workspace_root: workspace_root.clone(),
        source_files,
    };

    let checks = vec![
        WorkspaceAudit::check(
            "re_mcap-no-re_web-dependency",
            !package_depends_on(&metadata, "re_mcap", "re_web"),
            "re_mcap has no re_web dependency",
        ),
        WorkspaceAudit::check(
            "re_web-no-re_mcap-dependency",
            !package_depends_on(&metadata, "re_web", "re_mcap"),
            "re_web has no re_mcap dependency",
        ),
        WorkspaceAudit::check(
            "only-viewer-artifact-bridge",
            only_allowed_cross_crate_bridge(&metadata),
            "re_viewer is the only production bridge for both Wasm crates",
        ),
        WorkspaceAudit::check(
            "re_mcap-source-no-re_web",
            audit
                .find_files_containing("crates/store/re_mcap/src", &["re_web::"])
                .is_empty(),
            "re_mcap production source has no re_web callsites",
        ),
        WorkspaceAudit::check(
            "re_web-source-no-re_mcap",
            audit
                .find_files_containing("crates/utils/re_web/src", &["re_mcap::"])
                .is_empty(),
            "re_web production source has no re_mcap callsites",
        ),
        WorkspaceAudit::check(
            "cross-crate-production-callsites-confined",
            audit.cross_crate_callsites_are_confined(),
            "cross-crate remote callsites are confined to the Viewer adapter and Phase A artifact",
        ),
        WorkspaceAudit::check(
            "body-transfer-unfrozen-zero-copy",
            audit.body_transfer_is_unfrozen_zero_copy(),
            "physical body transfer is zero-copy and the profile remains unfrozen",
        ),
        WorkspaceAudit::check(
            "lower-object-binding-no-public-constructor",
            !audit.impl_block_has_public_fn(
                "crates/store/re_mcap/src/remote_physical_resolution.rs",
                "RemotePhysicalObjectBindingV1",
            ),
            "lower object binding has no public constructor",
        ),
        WorkspaceAudit::check(
            "production-limits-no-public-constructor",
            !audit.impl_block_has_public_fn(
                "crates/utils/re_web/src/remote_limits.rs",
                "ProductionWebRemoteLimitsV1",
            ),
            "production Web remote limits have no public constructor",
        ),
        WorkspaceAudit::check(
            "remote-modules-are-target-gated",
            audit.remote_modules_are_target_gated(),
            "remote-MCAP modules are absent from the native production module tree",
        ),
        WorkspaceAudit::check(
            "legacy-intern-constructors-remain",
            audit.legacy_intern_constructors_remain(),
            "existing InternedString/global_intern constructors remain available",
        ),
    ];

    let report = PhaseAAuditReportV1 {
        schema: PHASE_A_AUDIT_SCHEMA_V1.to_owned(),
        profile_status: PHASE_A_PROFILE_STATUS_V1.to_owned(),
        body_transfer_strategy: PHASE_A_BODY_TRANSFER_STRATEGY_V1.to_owned(),
        workspace_packages: metadata.packages.len(),
        source_files: audit.source_files.len(),
        checks,
    };
    report.validate_v1()?;
    Ok(report)
}

impl WorkspaceAudit {
    fn check(id: &str, passed: bool, detail: &str) -> PhaseAAuditCheckV1 {
        PhaseAAuditCheckV1 {
            id: id.to_owned(),
            passed,
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

    fn find_files_containing(&self, relative_root: &str, needles: &[&str]) -> Vec<PathBuf> {
        let root = self.workspace_root.join(relative_root);
        self.source_files
            .iter()
            .filter(|path| path.starts_with(&root))
            .filter(|path| {
                std::fs::read_to_string(path)
                    .ok()
                    .is_some_and(|content| needles.iter().any(|needle| content.contains(needle)))
            })
            .cloned()
            .collect()
    }

    fn cross_crate_callsites_are_confined(&self) -> bool {
        let needles = [
            "re_mcap::web_body_handoff",
            "re_mcap::phase_a_measurement",
            "re_web::chrome_byob",
            "re_web::chrome_range",
        ];
        needles.iter().all(|needle| {
            self.source_files
                .iter()
                .filter(|path| {
                    path.strip_prefix(&self.workspace_root)
                        .is_ok_and(|relative| !relative.ends_with("phase_a_audit.rs"))
                })
                .filter(|path| {
                    std::fs::read_to_string(path).ok().is_some_and(|content| {
                        content
                            .lines()
                            .map(code_prefix_before_line_comment)
                            .any(|line| line.contains(needle))
                    })
                })
                .all(|path| {
                    is_allowed_cross_crate_callsite(
                        path.strip_prefix(&self.workspace_root).unwrap_or(path),
                    )
                })
        })
    }

    fn body_transfer_is_unfrozen_zero_copy(&self) -> bool {
        let Ok(text) = self.text("crates/store/re_mcap/src/web_body_handoff.rs") else {
            return false;
        };
        text.contains("ZeroCopyV1")
            && text.contains("strategy: WebPhysicalBodyStrategyV1::ZeroCopyV1")
            && text.contains("Unfrozen")
            && text.contains("process_zero_copy_body_v1")
            && !text.contains("strategy: WebPhysicalBodyStrategyV1::ExplicitCopyV1")
    }

    fn impl_block_has_public_fn(&self, relative: &str, type_name: &str) -> bool {
        let Ok(text) = self.text(relative) else {
            return true;
        };
        impl_block_has_public_fn_static(&text, type_name)
    }

    fn remote_modules_are_target_gated(&self) -> bool {
        let re_web_lib = self.text("crates/utils/re_web/src/lib.rs");
        let re_mcap_lib = self.text("crates/store/re_mcap/src/lib.rs");
        let (Ok(re_web_lib), Ok(re_mcap_lib)) = (re_web_lib, re_mcap_lib) else {
            return false;
        };
        module_declarations_are_target_gated(&re_mcap_lib, RE_MCAP_REMOTE_MODULES)
            && module_declarations_are_target_gated(&re_web_lib, RE_WEB_REMOTE_MODULES)
    }

    fn legacy_intern_constructors_remain(&self) -> bool {
        let Ok(text) = self.text("crates/utils/re_string_interner/src/lib.rs") else {
            return false;
        };
        text.contains("pub fn new(string: &str) -> Self") && text.contains("fn global_intern(")
    }
}

fn impl_block_has_public_fn_static(text: &str, type_name: &str) -> bool {
    let mut in_target_impl = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("impl ") && trimmed.contains('{') {
            in_target_impl = direct_impl_targets(trimmed, type_name);
        }
        if in_target_impl && is_public_constructor_line(trimmed) {
            return true;
        }
    }
    false
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
    viewer_has_both_wasm_deps
        && metadata
            .packages
            .iter()
            .filter(|package| {
                package.name != "re_viewer" && package.name != "re_mcap_phase_a_chrome"
            })
            .all(|package| {
                let has_re_mcap = package.dependencies.iter().any(|dep| dep.name == "re_mcap");
                let has_re_web = package.dependencies.iter().any(|dep| dep.name == "re_web");
                !(has_re_mcap && has_re_web)
            })
}

fn source_files(workspace_root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for root in [
        workspace_root.join("crates"),
        workspace_root.join("tests/rust"),
    ] {
        collect_source_files(&root, &mut files);
    }
    files.sort();
    files
}

fn collect_source_files(root: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_source_files(&path, files);
        } else if file_type.is_file() && path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
}

fn is_allowed_cross_crate_callsite(path: &Path) -> bool {
    path.starts_with("tests/rust/test_mcap_phase_a_chrome")
        || path.ends_with("crates/viewer/re_viewer/src/web_remote_mcap_cpu.rs")
}

#[cfg(test)]
fn module_has_preceding_cfg(text: &str, module_line: &str, expected_cfg: &str) -> bool {
    let Some(position) = text.find(module_line) else {
        return false;
    };
    module_preceding_cfg(&text[..position]).is_some_and(|cfg| cfg.contains(expected_cfg))
}

#[derive(Clone, Copy)]
struct ModuleDeclaration<'a> {
    public_external: bool,
    cfg: Option<&'a str>,
}

fn module_declarations<'a>(text: &'a str, module_name: &str) -> Vec<ModuleDeclaration<'a>> {
    let needle = format!("mod {module_name};");
    let mut declarations = Vec::new();
    let mut remaining = text;

    while let Some(position) = remaining.find(&needle) {
        let line_start = remaining[..position]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        let declaration_line = &remaining[line_start..position + needle.len()];
        declarations.push(ModuleDeclaration {
            public_external: declaration_line.trim_start().starts_with("pub mod "),
            cfg: module_preceding_cfg(&remaining[..position]),
        });
        remaining = &remaining[position + needle.len()..];
    }

    declarations
}

fn module_declarations_are_target_gated(text: &str, remote_modules: &[&str]) -> bool {
    remote_modules.iter().all(|module_name| {
        let declarations = module_declarations(text, module_name);
        !declarations.is_empty()
            && declarations.iter().all(|declaration| {
                !declaration.public_external
                    || declaration.cfg.and_then(cfg_expr_native_production_value) == Some(false)
            })
    })
}

fn module_preceding_cfg(text_before_module: &str) -> Option<&str> {
    for line in text_before_module.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }
        if let Some(cfg) = trim_cfg_attribute(trimmed) {
            return Some(cfg);
        }
        if is_rust_item_start(trimmed) {
            break;
        }
    }
    None
}

fn trim_cfg_attribute(line: &str) -> Option<&str> {
    line.strip_prefix("#[cfg(")
        .and_then(|cfg| cfg.strip_suffix(")]"))
}

fn is_rust_item_start(line: &str) -> bool {
    [
        "pub mod ",
        "pub(crate) mod ",
        "pub(super) mod ",
        "pub(in ",
        "mod ",
        "pub use ",
        "use ",
        "pub struct ",
        "struct ",
        "pub enum ",
        "enum ",
        "pub trait ",
        "trait ",
        "impl ",
        "pub fn ",
        "pub const fn ",
        "pub async fn ",
        "fn ",
        "const ",
        "static ",
        "type ",
    ]
    .iter()
    .any(|prefix| line.starts_with(prefix))
}

fn direct_impl_targets(line: &str, type_name: &str) -> bool {
    let after_impl = line
        .strip_prefix("impl ")
        .and_then(|line| line.strip_suffix(" {"))
        .or_else(|| line.strip_prefix("impl "));
    after_impl.is_some_and(|target| target.split_whitespace().next() == Some(type_name))
}

fn is_public_constructor_line(line: &str) -> bool {
    line.starts_with("pub const fn")
        || line.starts_with("pub async fn")
        || line.starts_with("pub fn")
}

fn cfg_expr_native_production_value(expr: &str) -> Option<bool> {
    let expr = expr.trim();
    match expr {
        "test" | "target_arch = \"wasm32\"" | "target_arch = \"wasm64\"" => Some(false),
        _ if expr.starts_with("all(") && expr.ends_with(')') => {
            let parts = split_top_level_cfg(&expr["all(".len()..expr.len() - 1]);
            parts
                .iter()
                .map(|part| cfg_expr_native_production_value(part))
                .collect::<Option<Vec<_>>>()
                .map(|values| values.iter().all(|value| *value))
        }
        _ if expr.starts_with("any(") && expr.ends_with(')') => {
            let parts = split_top_level_cfg(&expr["any(".len()..expr.len() - 1]);
            parts
                .iter()
                .map(|part| cfg_expr_native_production_value(part))
                .collect::<Option<Vec<_>>>()
                .map(|values| values.iter().any(|value| *value))
        }
        _ if expr.starts_with("not(") && expr.ends_with(')') => {
            cfg_expr_native_production_value(&expr["not(".len()..expr.len() - 1])
                .map(|value| !value)
        }
        _ => None,
    }
}

fn split_top_level_cfg(expr: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0_usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut start = 0;

    for (index, character) in expr.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }

        match character {
            '"' => in_string = true,
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(expr[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    if start < expr.len() {
        parts.push(expr[start..].trim());
    }
    parts
}

fn code_prefix_before_line_comment(line: &str) -> &str {
    line.split_once("//").map_or(line, |(code, _comment)| code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_repository_passes_the_disarmed_phase_a_exit_gate() {
        let report = run_phase_a_audit_v1().expect("Phase A audit runs on the current workspace");
        report
            .validate_v1()
            .expect("Phase A audit report is complete");
        assert_eq!(report.profile_status, PHASE_A_PROFILE_STATUS_V1);
        assert_eq!(
            report.body_transfer_strategy,
            PHASE_A_BODY_TRANSFER_STRATEGY_V1
        );
        assert!(report.workspace_packages > 100);
        assert!(report.source_files > 1_000);
    }

    #[test]
    fn audit_report_rejects_frozen_or_failed_evidence() {
        let mut report = PhaseAAuditReportV1 {
            schema: PHASE_A_AUDIT_SCHEMA_V1.to_owned(),
            profile_status: PHASE_A_PROFILE_STATUS_V1.to_owned(),
            body_transfer_strategy: PHASE_A_BODY_TRANSFER_STRATEGY_V1.to_owned(),
            workspace_packages: 1,
            source_files: 1,
            checks: vec![PhaseAAuditCheckV1 {
                id: "pass".to_owned(),
                passed: true,
                detail: "redacted".to_owned(),
            }],
        };
        report.validate_v1().unwrap();

        report.profile_status = "frozen".to_owned();
        assert!(report.validate_v1().is_err());
        report.profile_status = PHASE_A_PROFILE_STATUS_V1.to_owned();

        report.body_transfer_strategy = "explicit-copy-v1".to_owned();
        assert!(report.validate_v1().is_err());
        report.body_transfer_strategy = PHASE_A_BODY_TRANSFER_STRATEGY_V1.to_owned();

        report.checks[0].passed = false;
        assert!(report.validate_v1().is_err());
    }

    #[test]
    fn module_cfg_parser_only_accepts_the_immediate_cfg_block() {
        let text = "\n#[cfg(any(target_arch = \"wasm32\", test))]\npub mod web_body_handoff;";
        assert!(module_has_preceding_cfg(
            text,
            "pub mod web_body_handoff;",
            "any(target_arch = \"wasm32\", test)"
        ));
        assert!(!module_has_preceding_cfg(
            text,
            "pub mod web_body_handoff;",
            "native"
        ));
        assert!(!module_has_preceding_cfg(
            "pub mod web_body_handoff;",
            "pub mod web_body_handoff;",
            "any(target_arch = \"wasm32\", test)"
        ));
    }

    #[test]
    fn remote_module_audit_rejects_native_public_declarations() {
        let safe = "\
#[cfg(target_arch = \"wasm32\")]
pub mod remote_time;
#[cfg(any(target_arch = \"wasm32\", test))]
mod remote_decompression;
";
        assert!(module_declarations_are_target_gated(
            safe,
            &["remote_time", "remote_decompression"]
        ));

        let unconfigured_public = "\
pub mod remote_decompression;
";
        assert!(!module_declarations_are_target_gated(
            unconfigured_public,
            &["remote_decompression"]
        ));

        let native_public = "\
#[cfg(not(target_arch = \"wasm32\"))]
pub mod remote_decompression;
";
        assert!(!module_declarations_are_target_gated(
            native_public,
            &["remote_decompression"]
        ));
    }

    #[test]
    fn public_constructor_scan_covers_all_target_impl_forms() {
        let text = "\
impl ProductionWebRemoteLimitsV1 {
    fn private_one() {}
}

impl fmt::Debug for ProductionWebRemoteLimitsV1 {
    pub fn ignored_trait_method() {}
}

impl ProductionWebRemoteLimitsV1 {
    pub const fn new() -> Self {
        Self
    }
}
";
        assert!(impl_block_has_public_fn_static(
            text,
            "ProductionWebRemoteLimitsV1"
        ));

        let async_text = "\
impl ProductionWebRemoteLimitsV1 {
    pub async fn new() -> Self {
        Self
    }
}
";
        assert!(impl_block_has_public_fn_static(
            async_text,
            "ProductionWebRemoteLimitsV1"
        ));

        let plain_text = "\
impl ProductionWebRemoteLimitsV1 {
    pub fn new() -> Self {
        Self
    }
}
";
        assert!(impl_block_has_public_fn_static(
            plain_text,
            "ProductionWebRemoteLimitsV1"
        ));
    }
}
