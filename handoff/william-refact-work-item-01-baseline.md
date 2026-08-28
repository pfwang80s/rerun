# Work Item 01 baseline evidence

## Result

Work Item 01 baseline evidence was regenerated from the current worktree on 2026-08-28 UTC.
This item records failures as failures and does not claim any profile gate passed.
No production code, tests, design documents, or `/data/demo` files were modified.
No files were staged, committed, or pushed.
Production capability remains `production_disarmed`.

## Evidence manifest

All current baseline evidence is under `/data/tools/refact-20260828-work-item-01/`:

- `git-boundary.log` — `git status --short`, `git diff --name-only`, `git diff --stat`, `git diff --cached --quiet` and no-staged-files result.
- `toolchain.log` — UTC timestamp, rustc, cargo, Pixi, wasm-pack, wasm-opt, Chrome, ChromeDriver, cargo-nextest and cargo-shear availability.
- `profile-graph.log` — ordinary Wasm, Phase A, locked verifier and host-test target/cfg/package/module graph, physical/decoder/manifest/dispatch/runtime-intern inclusion and exclusion, and suppression locations.
- `dead-code.log` — complete affected `#![allow(dead_code)]` inventory, including 36 source files.
- `commands.tsv` — regenerated current command table with exit code, warning/error line counts and raw log name.
- `ordinary-wasm.log`, `phase-a.log`, `locked-release.log`, `host-clippy.log`, `host-check.log`, `scope-s-lint.log`, `rustfmt.log`, `diff-check.log` — current rerun raw logs.

No prior log is promoted as current evidence.
The raw logs and command table are from this rerun.

## Profile baseline

| Profile/check | Target/cfg | Exit | Warning lines | Error lines | Classification |
|---|---|---:|---:|---:|---|
| ordinary product Wasm | wasm32, no Phase-A/locked cfg | 0 | 78 | 0 | Compile succeeded, but warning-free gate failed. |
| Phase-A proof | wasm32 + `rerun_mcap_phase_a_proof_v1` | 0 | 30 | 0 | Compile succeeded, but warning-free gate failed. |
| locked release-Web | wasm32 + attested locked verifier path | 1 | 6 | 4 | Failed baseline: unresolved `re_string_interner` in locked probe path. |
| host affected clippy | native test graph + `-D warnings` | 101 | 1 | 21 | Failed baseline: dormant graph dead-code warnings became errors. |
| host affected check | native test graph | 0 | 21 | 0 | Compile succeeded, but warning-free gate failed. |
| scope-S custom lint | bounded MCAP-114 target paths | 0 | 0 | 0 | Current scoped custom lint passed. |
| targeted rustfmt | bounded changed Rust files | 0 | 0 | 0 | Passed. |
| `git diff --check` | current worktree | 0 | 0 | 0 | Passed. |

Counts are raw `warning:`/`error` line counts from the corresponding logs, not unique diagnostic identities.

## Profile graph

- Ordinary product Wasm uses `re_viewer` Web entrypoints and currently includes the concrete Web remote physical graph because Viewer CPU still references `re_mcap::web_body_handoff` types.
- Phase A uses `re_mcap_phase_a_chrome` and exactly four stages: `byob_copy`, `opening_parse`, `message_index_parse`, and `physical_validation`.
- Phase A excludes `dispatch_decode`, decoder assignment, manifest dispatch and runtime-intern execution graph.
- Locked release uses the attested `re_mcap_locked_remote_wasm_allocator_v1` path for the full verifier graph and verifier anchors/probes.
- Host tests use `cfg(test)` and retain the full test graph and test-only fault injection.
- The current graph is not yet a warning-free four-profile split; this is the purpose of later work items.

## Git boundary and exclusions

The current Git boundary is recorded verbatim in `git-boundary.log`.
The worktree contains pre-existing and in-scope tracked changes plus unrelated/untracked plans, handoffs, temporary files, generated target directories and other artifacts.
No cleanup, reset, clean or stash was performed.
No staged files were present: `git diff --cached --quiet` exited 0.
Future commits must use an explicit allowlist and must exclude `/data/demo`, generated Wasm, target directories, `/data/tools` logs, temporary/session artifacts and unrelated dirty files.

## Plan status

Work Item 01 is baseline-complete but review-required.
The baseline gates failed exactly as recorded above.
Work Items 02–16 remain pending and are not implied complete by this evidence item.

## Residual risks

The following remain open for later work items:

- warning-free ordinary Wasm, Phase A and locked verifier profiles;
- removal of module-level dead-code suppression through real profile boundaries;
- locked release feature/dependency failure;
- concrete `web_body_handoff`/Viewer CPU extraction;
- Chrome runner, Range smoke and correctness gates;
- full release artifact/API/dependency/ABI, stress, memory/intern and differential gates.

## Validation

- Required evidence artifacts regenerated and present.
- `git diff --check`: passed.
- No source implementation changes were made for this baseline evidence fix.
- No old evidence was promoted to a pass.

```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "Only Work Item 01 evidence/handoff/plan records were updated; no production code, tests, design, /data/demo, staging, commit, or push was performed."
    },
    {
      "id": "criterion-2",
      "status": "satisfied",
      "evidence": "The current baseline directory contains git-boundary.log, toolchain.log, profile-graph.log, dead-code.log, regenerated commands.tsv, current raw profile logs, and a structured handoff table with exact failed classifications and exclusions."
    }
  ],
  "changedFiles": [
    "handoff/william-refact-work-item-01-baseline.md",
    "_items_refact_20260828.md",
    "/data/tools/refact-20260828-work-item-01/git-boundary.log",
    "/data/tools/refact-20260828-work-item-01/toolchain.log",
    "/data/tools/refact-20260828-work-item-01/profile-graph.log",
    "/data/tools/refact-20260828-work-item-01/dead-code.log",
    "/data/tools/refact-20260828-work-item-01/commands.tsv",
    "/data/tools/refact-20260828-work-item-01/ordinary-wasm.log",
    "/data/tools/refact-20260828-work-item-01/phase-a.log",
    "/data/tools/refact-20260828-work-item-01/locked-release.log",
    "/data/tools/refact-20260828-work-item-01/host-clippy.log",
    "/data/tools/refact-20260828-work-item-01/host-check.log",
    "/data/tools/refact-20260828-work-item-01/scope-s-lint.log",
    "/data/tools/refact-20260828-work-item-01/rustfmt.log",
    "/data/tools/refact-20260828-work-item-01/diff-check.log"
  ],
  "testsAddedOrUpdated": [],
  "commandsRun": [
    {
      "command": "git status --short; git diff --name-only; git diff --stat; git diff --cached --quiet",
      "result": "passed",
      "summary": "Current Git boundary and no-staged-files result saved."
    },
    {
      "command": "rustc/cargo/pixi/wasm-pack/wasm-opt/Chrome toolchain capture",
      "result": "passed",
      "summary": "Current toolchain and unavailable cargo-nextest/cargo-shear recorded."
    },
    {
      "command": "ordinary Wasm, Phase-A, locked release, host check/clippy, scoped lint and targeted rustfmt",
      "result": "failed",
      "summary": "Commands were rerun and failures/warnings were retained as baseline evidence in commands.tsv and raw logs."
    },
    {
      "command": "git diff --check",
      "result": "passed",
      "summary": "No whitespace errors."
    }
  ],
  "validationOutput": [
    "All required Work Item 01 evidence files are present and regenerated.",
    "Baseline failures are explicitly classified: ordinary/Phase A compile-but-not-warning-free, locked release failure, host clippy failure, and custom lint baseline result.",
    "Work Items 02–16 remain pending."
  ],
  "residualRisks": [
    "The four profile graph is not warning-free and remains coupled through concrete web_body_handoff types.",
    "Locked verifier baseline fails on unresolved re_string_interner.",
    "The complete MCAP-114 Chrome/release/stress/memory/intern/differential gates remain open."
  ],
  "noStagedFiles": true,
  "diffSummary": "Regenerated complete Work Item 01 baseline evidence and corrected its status classification without source implementation changes.",
  "reviewFindings": [
    "resolved: missing baseline evidence manifest and handoff",
    "resolved: failed baseline gates are explicitly recorded as failed",
    "no implementation scope expansion"
  ],
  "manualNotes": "Dafee review remains required before a baseline-only commit."
}
```