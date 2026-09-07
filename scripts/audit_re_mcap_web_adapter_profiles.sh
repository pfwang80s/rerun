#!/usr/bin/env bash
# Profile audit for re_mcap_web_adapter.
#
# This audit is fail-closed. Cargo tree describes resolved dependencies only; source checks below
# separately classify implementation reachability and must not be read as cfg execution proof.
set -euo pipefail

profile=${1:?profile is required: consumer_contract, phase_a, locked, locked_negative, or locked_invalid}
target=${TARGET:-wasm32-unknown-unknown}
package=re_mcap_web_adapter
out_dir=${OUT_DIR:-target/refact-20260828}
lib_rs=${AUDIT_LIB_RS:-crates/store/re_mcap/src/lib.rs}
mkdir -p "$out_dir"

assert_absent() {
  local needle=$1 file=$2
  if grep -Fq -- "$needle" "$file"; then
    printf 'forbidden match: %s in %s\n' "$needle" "$file" >&2
    return 1
  fi
}

assert_present() {
  local needle=$1 file=$2
  if ! grep -Fq -- "$needle" "$file"; then
    printf 'missing required match: %s in %s\n' "$needle" "$file" >&2
    return 1
  fi
}

assert_source_present() {
  local needle=$1 file=$2
  if ! grep -Fq -- "$needle" "$file"; then
    printf 'missing required source match: %s in %s\n' "$needle" "$file" >&2
    return 1
  fi
}

# This is the single pinned profile manifest for the re_mcap locked-only declarations. The audit
# rejects both omissions from this list and declarations in lib.rs that are not classified here.
readonly LOCKED_MODULES=(
  remote_protobuf_projection_boundary
  remote_protobuf_descriptor
  remote_decoder_assignment
  remote_deterministic_insertion
  remote_channel_group
  remote_chunk_validation_count
  remote_manifest
  remote_loaded_coverage
  remote_partition_residency
  remote_window_demand
  remote_navigation
  remote_seek
  remote_partition_job
  remote_predecessor_backfill
  remote_root_reload
  remote_chunk_dispatch
  remote_typed_output
  remote_runtime_intern
  remote_ros2_reflection
)
readonly PHASE_A_SHARED_MODULES=(remote_physical_resolution)
readonly WEB_ADAPTER_SHARED_MODULES=(
  web_body_handoff
  remote_time
  remote_fixed_layout
  remote_summary
  remote_decompression
  remote_chunk_scan
  remote_physical_seam
)

is_remote_module() {
  local module=$1
  is_in_list "$module" "${LOCKED_MODULES[@]}" ||
    is_in_list "$module" "${PHASE_A_SHARED_MODULES[@]}" ||
    is_in_list "$module" "${WEB_ADAPTER_SHARED_MODULES[@]}"
}


module_declaration() {
  local module=$1
  python3 - "$lib_rs" "$module" <<'PY'
import re
import sys
from pathlib import Path

path, module = sys.argv[1:]
text = Path(path).read_text()
results = []
for match in re.finditer(r"#\[cfg\(", text):
    start = match.end()
    depth = 1
    index = start
    while index < len(text) and depth:
        if text[index] == "(":
            depth += 1
        elif text[index] == ")":
            depth -= 1
        index += 1
    if depth:
        raise SystemExit(f"unterminated cfg attribute before {module}")
    suffix = text[index:]
    if suffix.startswith("]"):
        suffix = suffix[1:]
    declaration = re.match(r"\s*(?:pub\s+)?mod\s+([A-Za-z0-9_]+)\s*;", suffix)
    if declaration and declaration.group(1) == module:
        predicate = re.sub(r"\s+", " ", text[start:index - 1]).strip()
        predicate = predicate.replace("any( ", "any(").replace(" )", ")").replace(", ", ", ")
        results.append(predicate)
if not results:
    raise SystemExit(f"missing cfg module declaration for {module}")
for predicate in results:
    print(predicate)
PY
}

assert_exact_module_predicate() {
  local module=$1 expected=$2 evidence=$3
  local predicates
  predicates=$(module_declaration "$module") || {
    printf 'fail-closed: unable to parse cfg declaration for %s\n' "$module" | tee -a "$evidence" >&2
    return 1
  }
  local matches
  matches=$(printf '%s\n' "$predicates" | grep -Fxc -- "$expected" || true)
  if [[ "$matches" != 1 ]]; then
    printf 'fail-closed: %s predicate count mismatch\nexpected exactly once: %s\nactual:\n%s\n' "$module" "$expected" "$predicates" | tee -a "$evidence" >&2
    return 1
  fi
  while IFS= read -r predicate; do
    [[ "$predicate" == "$expected" ]] && continue
    if [[ "$expected" == *re_mcap_locked_remote_wasm_allocator_v1* && "$predicate" == *re_mcap_locked_remote_wasm_allocator_v1* ]] ||
       [[ "$expected" == *rerun_mcap_phase_a_proof_v1* && "$predicate" == *rerun_mcap_phase_a_proof_v1* ]]; then
      printf 'fail-closed: %s has an additional conflicting predicate: %s\n' "$module" "$predicate" | tee -a "$evidence" >&2
      return 1
    fi
  done <<<"$predicates"
  printf '%s -> cfg(%s)\n' "$module" "$expected" >>"$evidence"
}

normalize_cfg() {
  tr '\n\t' ' ' | sed -E 's/[[:space:]]+/ /g; s/^ //; s/ $//'
}

module_names_from_lib() {
  sed -nE 's/^(pub )?mod ([a-zA-Z0-9_]+);$/\2/p' "$lib_rs"
}

is_in_list() {
  local needle=$1; shift
  local item
  for item in "$@"; do [[ "$item" == "$needle" ]] && return 0; done
  return 1
}

assert_locked_module_manifest_and_cfg() {
  local evidence="$out_dir/re_mcap-profile-manifest.txt"
  : >"$evidence"
  local module declaration cfg
  local declared_locked=()

  while read -r module; do
    # Ordinary parser, file, error, info, and recovery modules are not part of the remote profile
    # graph. Do not ask the remote-module parser to classify them as pinned remote modules.
    if ! is_remote_module "$module"; then
      printf 'not-applicable: %s is an ordinary re_mcap base module; skipped remote cfg audit\n' \
        "$module" >>"$evidence"
      continue
    fi
    if predicates=$(module_declaration "$module" 2>/dev/null) &&
       printf '%s\n' "$predicates" | grep -q 're_mcap_locked_remote_wasm_allocator_v1'; then
      declared_locked+=("$module")
    fi
  done < <(module_names_from_lib)

  for module in "${LOCKED_MODULES[@]}"; do
    if ! is_in_list "$module" "${declared_locked[@]}"; then
      printf 'fail-closed: pinned locked module %s is missing or not locked-gated\n' "$module" | tee -a "$evidence" >&2
      return 1
    fi
    assert_exact_module_predicate "$module" \
      'any(all(test, not(target_arch = "wasm32")), re_mcap_locked_remote_wasm_allocator_v1)' \
      "$evidence" || return 1
  done

  for module in "${declared_locked[@]}"; do
    if ! is_in_list "$module" "${LOCKED_MODULES[@]}" &&
       ! is_in_list "$module" "${PHASE_A_SHARED_MODULES[@]}" &&
       ! is_in_list "$module" "${WEB_ADAPTER_SHARED_MODULES[@]}"; then
      printf 'fail-closed: lib.rs locked declaration %s is absent from pinned manifest\n' "$module" | tee -a "$evidence" >&2
      return 1
    fi
  done
  printf 'manifest and exact cfg audit passed: %d locked modules\n' "${#LOCKED_MODULES[@]}" >>"$evidence"
}

assert_phase_a_cfg_manifest() {
  local evidence="$out_dir/phase-a-module-predicates.txt"
  : >"$evidence"
  local module declaration cfg
  for module in "${PHASE_A_SHARED_MODULES[@]}"; do
    assert_exact_module_predicate "$module" \
      'any(all(rerun_mcap_phase_a_proof_v1, feature = "web_adapter"), re_mcap_locked_remote_wasm_allocator_v1,)' \
      "$evidence" || return 1
  done
  printf 'phase-a allowlist and predicate audit passed\n' >>"$evidence"
}

assert_build_script_environment_tracking() {
  local build_script=crates/mcap/re_mcap_web_adapter/build.rs
  local verifier=crates/build/re_build_tools/src/remote_wasm_contract.rs
  local evidence="$out_dir/build-script-env-tracking.txt"
  : >"$evidence"
  local variables=()
  mapfile -t variables < <(sed -n '/LOCKED_ATTESTATION_ENV_V1:/,/^];/p' "$build_script" | sed -nE 's/.*"([A-Z][A-Z0-9_]+)".*/\1/p')

  if ((${#variables[@]} == 0)); then
    printf 'fail-closed: adapter build.rs authoritative attestation env manifest is missing\n' | tee -a "$evidence" >&2
    return 1
  fi
  if ! grep -q 'LOCKED_ATTESTATION_ENV_V1' "$build_script"; then
    printf 'fail-closed: build.rs has no pinned attestation environment manifest\n' | tee -a "$evidence" >&2
    return 1
  fi
  for variable in "${variables[@]}"; do
    if ! grep -q 'for variable in LOCKED_ATTESTATION_ENV_V1' "$build_script"; then
      printf 'fail-closed: build.rs does not emit rerun directives from authoritative manifest\n' | tee -a "$evidence" >&2
      return 1
    fi
  done
  # The verifier's environment contract must still be visibly routed through its tracking helper.
  if ! grep -q 'track_probe_environment_v1();' "$verifier" ||
     ! grep -q 'fn track_probe_environment_v1' "$verifier"; then
    printf 'fail-closed: verifier environment tracking helper cannot be located\n' | tee -a "$evidence" >&2
    return 1
  fi
  # Detect literal verifier inputs added to the attestation entrypoint unless the pinned manifest
  # is updated. This is a drift guard, not a claim of dynamic environment discovery.
  local verifier_literals
  verifier_literals=$(sed -n '/fn locked_remote_wasm_consumer_attestation_v1/,/fn mcap_phase_a_artifact_attestation_v1/p' "$verifier" |
    sed -nE 's/.*std::env::var(_os)?\("([A-Z][A-Z0-9_]+)"\).*/\2/p' | sort -u)
  local variable
  while read -r variable; do
    [[ -z "$variable" ]] && continue
    if ! printf '%s\n' "${variables[@]}" | grep -Fxq "$variable"; then
      printf 'fail-closed: verifier literal env input %s is absent from pinned build.rs manifest\n' "$variable" | tee -a "$evidence" >&2
      return 1
    fi
  done <<<"$verifier_literals"
  printf 'authoritative build.rs attestation env manifest (%d variables): %s\n' "${#variables[@]}" "${variables[*]}" >>"$evidence"
  printf 'verifier drift guard passed; manifest is pinned, not dynamically discovered\n' >>"$evidence"
}

assert_phase_a_reachable_graph_absent() {
  local evidence="$out_dir/phase-a-reachable-graph.txt"
  : >"$evidence"
  assert_locked_module_manifest_and_cfg
  assert_phase_a_cfg_manifest
  local module
  for module in "${LOCKED_MODULES[@]}"; do
    printf 'proven-absent-in-phase-a: %s is outside Phase-A allowlist\n' "$module" >>"$evidence"
  done
  printf 'not-applicable: ordinary re_mcap base modules are excluded from the remote Phase-A audit\n' \
    >>"$evidence"
  if rg -n --glob '*.rs' 'PhysicalChunkDispatchDecode' crates/store/re_mcap/src >"$out_dir/phase-a-reachable-dispatch-symbols.txt"; then
    printf 'not-proven: PhysicalChunkDispatchDecode exists in reachable re_mcap source graph\n' | tee -a "$evidence" >&2
    return 1
  fi
  printf 'proven-absent-in-phase-a: PhysicalChunkDispatchDecode absent from re_mcap source graph\n' >>"$evidence"
}

case "$profile" in
  predicate_negative)
    fixture=$(mktemp)
    trap 'rm -f "$fixture"' EXIT
    cat >"$fixture" <<'EOF'
#[cfg(any(all(test, not(target_arch = "wasm32")), rerun_mcap_phase_a_proof_v1, re_mcap_locked_remote_wasm_allocator_v1))]
mod remote_decoder_assignment;
EOF
    mkdir -p "$out_dir/predicate-negative"
    if AUDIT_LIB_RS="$fixture" OUT_DIR="$out_dir/predicate-negative" \
      bash -c 'source "$1"; e="$2/result.log"; assert_exact_module_predicate remote_decoder_assignment '"'"'any(all(test, not(target_arch = "wasm32")), re_mcap_locked_remote_wasm_allocator_v1)'"'"' "$e"' _ "$0" "$out_dir/predicate-negative" \
      >"$out_dir/predicate-negative/stdout.log" 2>"$out_dir/predicate-negative/stderr.log"; then
      printf 'broadened predicate unexpectedly accepted\n' >&2
      exit 1
    fi
    printf 'broadened predicate rejected fail-closed\n'
    ;;
  consumer_contract)
    cargo tree --target "$target" --no-default-features --features consumer_contract \
      -e features -p "$package" > "$out_dir/tree-consumer.txt"
    cargo metadata --no-deps --format-version 1 > "$out_dir/metadata-consumer.json"
    assert_absent 're_web v' "$out_dir/tree-consumer.txt"
    assert_absent 're_mcap v' "$out_dir/tree-consumer.txt"
    assert_absent 'feature "phase_a"' "$out_dir/tree-consumer.txt"
    assert_absent 'feature "locked"' "$out_dir/tree-consumer.txt"
    assert_present '"consumer_contract"' "$out_dir/metadata-consumer.json"
    ;;
  phase_a)
    cargo tree --target "$target" --no-default-features --features phase_a \
      -e features -p "$package" > "$out_dir/tree-phase-a.txt"
    cargo metadata --no-deps --format-version 1 > "$out_dir/metadata-phase-a.json"
    assert_present 're_web v' "$out_dir/tree-phase-a.txt"
    assert_present 're_mcap v' "$out_dir/tree-phase-a.txt"
    assert_absent 'feature "locked"' "$out_dir/tree-phase-a.txt"
    assert_present '"phase_a"' "$out_dir/metadata-phase-a.json"
    assert_source_present 'all(feature = "phase_a", target_arch = "wasm32")' \
      crates/mcap/re_mcap_web_adapter/src/lib.rs
    assert_phase_a_reachable_graph_absent
    assert_build_script_environment_tracking
    assert_absent 'PhysicalChunkDispatchDecode' crates/mcap/re_mcap_web_adapter/src/phase_a.rs
    for forbidden in decoder_assignment manifest dispatch runtime_intern PhysicalChunkDispatchDecode; do
      if rg -n --glob '*.rs' --glob '*.toml' --glob '*.lock' "$forbidden" crates/mcap/re_mcap_web_adapter/src crates/mcap/re_mcap_web_adapter/Cargo.toml > "$out_dir/phase-a-forbidden-$forbidden.txt"; then
        printf 'forbidden Phase A source/manifest match: %s\n' "$forbidden" >&2
        exit 1
      fi
    done
    printf 'phase_a forbidden graph audit: pinned declarations and reachable source checks passed\n' >> "$out_dir/phase-a-forbidden-graph.txt"
    ;;
  locked)
    if [[ "${RERUN_LOCKED_ATTESTATION_V1:-}" != "1" ]]; then
      printf 'locked profile requires the external attestation preflight marker\n' >&2
      exit 1
    fi
    locked_log="$out_dir/locked-positive-build.log"
    if ! RERUN_LOCKED_ATTESTATION_V1=1 RERUN_REMOTE_ROS2_ARTIFACT_PROBE_V1=1 \
      cargo check -p "$package" --target "$target" --no-default-features --features locked \
      >"$locked_log" 2>&1; then
      printf 'locked positive build not proven: real external attestation/build failed\n' >&2
      cat "$locked_log" >&2
      exit 1
    fi
    assert_present 'external attestation accepted' "$locked_log"
    cargo tree --target "$target" --no-default-features --features locked \
      -e features -p "$package" > "$out_dir/tree-locked.txt"
    cargo metadata --no-deps --format-version 1 > "$out_dir/metadata-locked.json"
    assert_present 're_web v' "$out_dir/tree-locked.txt"
    assert_present 're_mcap v' "$out_dir/tree-locked.txt"
    assert_present '"locked"' "$out_dir/metadata-locked.json"
    assert_source_present 're_mcap_web_adapter_locked_attested_v1' \
      crates/mcap/re_mcap_web_adapter/src/lib.rs
    # The locked profile must pull re_mcap's bounded runtime-interner Cargo feature so that the
    # locked consumer graph never compiles against a missing optional dependency.
    assert_present 're_mcap v' "$out_dir/tree-locked.txt"
    if ! grep -Fq 'feature "rerun_mcap_locked_remote_wasm_allocator_v1"' "$out_dir/tree-locked.txt"; then
      printf 'fail-closed: adapter locked profile does not enable re_mcap locked runtime-interner feature\n' >&2
      exit 1
    fi
    assert_present 'rerun_mcap_locked_remote_wasm_allocator_v1 = [' \
      crates/mcap/re_mcap_web_adapter/Cargo.toml
    assert_present 're_mcap/rerun_mcap_locked_remote_wasm_allocator_v1' \
      crates/mcap/re_mcap_web_adapter/Cargo.toml
    ;;
  locked_negative)
    if RERUN_LOCKED_ATTESTATION_V1= cargo check -p "$package" --target "$target" \
      --no-default-features --features locked > "$out_dir/locked-negative.log" 2>&1; then
      printf 'locked negative unexpectedly succeeded\n' >&2
      exit 1
    fi
    assert_present 'requires an external artifact attestation' "$out_dir/locked-negative.log"
    ;;
  locked_invalid)
    if RERUN_REMOTE_ROS2_ARTIFACT_PROBE_V1=1 CARGO_ENCODED_RUSTFLAGS='--cfg=not_canonical' \
      cargo check -p "$package" --target "$target" --no-default-features --features locked \
      > "$out_dir/locked-invalid.log" 2>&1; then
      printf 'locked invalid-attestation probe unexpectedly succeeded\n' >&2
      exit 1
    fi
    assert_present 'non-canonical Rust flags' "$out_dir/locked-invalid.log"
    ;;
  *)
    printf 'unknown profile: %s\n' "$profile" >&2
    exit 1
    ;;
esac

assert_locked_module_manifest_and_cfg
printf 'audit passed: %s\n' "$profile"
