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
        let needle = format!("impl {type_name}");
        let Some(start) = text.find(&needle) else {
            return false;
        };
        let after_impl = &text[start + needle.len()..];
        let next_impl = after_impl.find("\nimpl ").unwrap_or(after_impl.len());
        let block = &after_impl[..next_impl];
        block
            .lines()
            .any(|line| line.trim_start().starts_with("pub fn"))
    }

    fn remote_modules_are_target_gated(&self) -> bool {
        let re_web_lib = self.text("crates/utils/re_web/src/lib.rs");
        let re_mcap_lib = self.text("crates/store/re_mcap/src/lib.rs");
        let (Ok(re_web_lib), Ok(re_mcap_lib)) = (re_web_lib, re_mcap_lib) else {
            return false;
        };
        module_has_preceding_cfg(
            &re_web_lib,
            "pub mod remote_limits;",
            "target_arch = \"wasm32\"",
        ) && module_has_preceding_cfg(
            &re_mcap_lib,
            "pub mod web_body_handoff;",
            "any(target_arch = \"wasm32\", test)",
        ) && !re_mcap_lib.contains("pub mod remote_summary;")
            && !re_mcap_lib.contains("pub mod remote_chunk_scan;")
    }

    fn legacy_intern_constructors_remain(&self) -> bool {
        let Ok(text) = self.text("crates/utils/re_string_interner/src/lib.rs") else {
            return false;
        };
        text.contains("pub fn new(string: &str) -> Self") && text.contains("fn global_intern(")
    }
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

fn module_has_preceding_cfg(text: &str, module_line: &str, expected_cfg: &str) -> bool {
    let Some(position) = text.find(module_line) else {
        return false;
    };
    let prefix = &text[..position];
    prefix.lines().rev().take(3).any(|line| {
        line.trim()
            .strip_prefix("#[cfg(")
            .and_then(|cfg| cfg.strip_suffix(")]"))
            .is_some_and(|cfg| cfg.contains(expected_cfg))
    })
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
}
