use std::collections::BTreeSet;

use anyhow::{Context as _, bail};
use serde::{Deserialize, Serialize};

pub const PHASE_A_EVIDENCE_SCHEMA_V1: &str = "rerun-mcap-phase-a-evidence-v1";
pub const PHASE_A_WARMUP_ITERATIONS_V1: u32 = 2;
pub const PHASE_A_SAMPLE_ITERATIONS_V1: u32 = 5;
pub const REQUIRED_PHASE_A_STAGES_V1: [&str; 4] = [
    "byob_copy",
    "opening_parse",
    "message_index_parse",
    "physical_validation",
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhaseAEvidenceV1 {
    pub schema: String,
    pub profile_status: String,
    pub build_profile: String,
    pub wasm_optimized: bool,
    pub warmup_iterations: u32,
    pub sample_iterations: u32,
    pub provenance: PhaseAProvenanceV1,
    pub build: PhaseABuildEvidenceV1,
    pub stages: Vec<PhaseAStageEvidenceV1>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhaseAProvenanceV1 {
    pub fixture: String,
    pub transport: String,
    pub pipeline: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhaseABuildEvidenceV1 {
    pub wasm_sha256: String,
    pub js_sha256: String,
    pub fixture_sha256: String,
    pub git_commit: String,
    pub browser_family: String,
    pub chrome_version: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhaseAStageEvidenceV1 {
    pub name: String,
    pub max_duration_micros: u64,
    pub input_bytes: u64,
    pub output_bytes: u64,
    pub completed_count: u64,
    pub retained_high_water_bytes: u64,
    pub overflowed: bool,
}

impl PhaseAEvidenceV1 {
    pub fn parse_and_validate_v1(bytes: &[u8]) -> anyhow::Result<Self> {
        let evidence: Self =
            serde_json::from_slice(bytes).context("invalid Phase A evidence JSON")?;
        evidence.validate_v1()?;
        Ok(evidence)
    }

    pub fn validate_v1(&self) -> anyhow::Result<()> {
        if self.schema != PHASE_A_EVIDENCE_SCHEMA_V1 {
            bail!("unexpected Phase A evidence schema");
        }
        if self.profile_status != "unfrozen" {
            bail!("Phase A evidence must not claim a frozen profile");
        }
        if self.build_profile != "web-release" || !self.wasm_optimized {
            bail!("Phase A evidence must come from an optimized web-release artifact");
        }
        if self.warmup_iterations != PHASE_A_WARMUP_ITERATIONS_V1
            || self.sample_iterations != PHASE_A_SAMPLE_ITERATIONS_V1
            || self.provenance.fixture != "fixed-mcap-phase-a-v1"
            || self.provenance.transport != "controlled-range-byob-v1"
            || self.provenance.pipeline != "re_viewer-transport-physical-pipeline-proof-v1"
        {
            bail!("Phase A evidence provenance or sample schedule is invalid");
        }
        if !is_sha256_v1(&self.build.wasm_sha256)
            || !is_sha256_v1(&self.build.js_sha256)
            || !is_sha256_v1(&self.build.fixture_sha256)
            || !is_git_commit_v1(&self.build.git_commit)
            || self.build.browser_family != "chrome-stable"
            || !is_chrome_version_v1(&self.build.chrome_version)
        {
            bail!("Phase A build or browser evidence is invalid");
        }
        if self.stages.len() != REQUIRED_PHASE_A_STAGES_V1.len() {
            bail!("Phase A evidence must contain exactly four stages");
        }
        let mut names = BTreeSet::new();
        for (stage, expected_name) in self.stages.iter().zip(REQUIRED_PHASE_A_STAGES_V1) {
            if stage.name != expected_name || !names.insert(stage.name.as_str()) {
                bail!("Phase A evidence stages are missing, duplicated, or out of order");
            }
            if stage.max_duration_micros == 0
                || stage.completed_count != u64::from(PHASE_A_SAMPLE_ITERATIONS_V1)
                || stage.input_bytes == 0
                || stage.output_bytes == 0
                || stage.retained_high_water_bytes == 0
                || stage.overflowed
            {
                bail!("Phase A stage evidence is incomplete or overflowed");
            }
        }
        Ok(())
    }
}

fn is_sha256_v1(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_git_commit_v1(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_chrome_version_v1(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid() -> PhaseAEvidenceV1 {
        PhaseAEvidenceV1 {
            schema: PHASE_A_EVIDENCE_SCHEMA_V1.to_owned(),
            profile_status: "unfrozen".to_owned(),
            build_profile: "web-release".to_owned(),
            wasm_optimized: true,
            warmup_iterations: PHASE_A_WARMUP_ITERATIONS_V1,
            sample_iterations: PHASE_A_SAMPLE_ITERATIONS_V1,
            provenance: PhaseAProvenanceV1 {
                fixture: "fixed-mcap-phase-a-v1".to_owned(),
                transport: "controlled-range-byob-v1".to_owned(),
                pipeline: "re_viewer-transport-physical-pipeline-proof-v1".to_owned(),
            },
            build: PhaseABuildEvidenceV1 {
                wasm_sha256: "1".repeat(64),
                js_sha256: "2".repeat(64),
                fixture_sha256: "3".repeat(64),
                git_commit: "4".repeat(40),
                browser_family: "chrome-stable".to_owned(),
                chrome_version: "140.0.0.0".to_owned(),
            },
            stages: REQUIRED_PHASE_A_STAGES_V1
                .into_iter()
                .map(|name| PhaseAStageEvidenceV1 {
                    name: name.to_owned(),
                    max_duration_micros: 1,
                    input_bytes: 1,
                    output_bytes: 1,
                    completed_count: u64::from(PHASE_A_SAMPLE_ITERATIONS_V1),
                    retained_high_water_bytes: 1,
                    overflowed: false,
                })
                .collect(),
        }
    }

    #[test]
    fn strict_schema_accepts_only_complete_unfrozen_release_evidence() {
        valid().validate_v1().unwrap();

        let mut invalid = valid();
        invalid.schema = "future-schema".to_owned();
        assert!(invalid.validate_v1().is_err());

        let mut invalid = valid();
        invalid.profile_status = "frozen".to_owned();
        assert!(invalid.validate_v1().is_err());

        let mut invalid = valid();
        invalid.build_profile = "debug".to_owned();
        assert!(invalid.validate_v1().is_err());

        let mut invalid = valid();
        invalid.wasm_optimized = false;
        assert!(invalid.validate_v1().is_err());

        let mut invalid = valid();
        invalid.warmup_iterations = 0;
        assert!(invalid.validate_v1().is_err());

        let mut invalid = valid();
        invalid.sample_iterations = 0;
        assert!(invalid.validate_v1().is_err());

        let mut invalid = valid();
        invalid.provenance.pipeline = "testing-helper".to_owned();
        assert!(invalid.validate_v1().is_err());

        let mut invalid = valid();
        invalid.stages.pop();
        assert!(invalid.validate_v1().is_err());

        for mutate in [
            |stage: &mut PhaseAStageEvidenceV1| stage.completed_count = 0,
            |stage: &mut PhaseAStageEvidenceV1| stage.max_duration_micros = 0,
            |stage: &mut PhaseAStageEvidenceV1| stage.input_bytes = 0,
            |stage: &mut PhaseAStageEvidenceV1| stage.output_bytes = 0,
            |stage: &mut PhaseAStageEvidenceV1| stage.retained_high_water_bytes = 0,
            |stage: &mut PhaseAStageEvidenceV1| stage.overflowed = true,
        ] {
            let mut invalid = valid();
            mutate(&mut invalid.stages[0]);
            assert!(invalid.validate_v1().is_err());
        }
    }

    #[test]
    fn removed_dispatch_decode_stage_is_rejected() {
        let mut invalid = valid();
        invalid.stages[0].name = "dispatch_decode".to_owned();
        assert!(invalid.validate_v1().is_err());
    }

    #[test]
    fn unknown_fields_and_duplicate_stages_are_rejected() {
        let mut json = serde_json::to_value(valid()).unwrap();
        json.as_object_mut()
            .unwrap()
            .insert("extra".to_owned(), true.into());
        assert!(
            PhaseAEvidenceV1::parse_and_validate_v1(&serde_json::to_vec(&json).unwrap()).is_err()
        );

        let mut duplicate = valid();
        duplicate.stages[1].name = "byob_copy".to_owned();
        assert!(duplicate.validate_v1().is_err());
    }
}
