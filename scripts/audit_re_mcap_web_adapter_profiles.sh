#!/usr/bin/env bash
# Profile audit for re_mcap_web_adapter.
#
# This is the only plan-approved entrypoint for the adapter profile audit. It is fail-closed:
# a forbidden dependency/feature match or a missing attestation precondition exits non-zero.
#
# The environment variable RERUN_LOCKED_ATTESTATION_V1 is only a fail-closed preflight marker,
# NOT an authentic attestation. Authentic locked acceptance comes from the external
# attestation workflow in re_mcap/build.rs / re_build_tools, which this wrapper does not and
# cannot perform. The locked branch here only performs the no-attestation negative rejection
# and the declared-feature check.
set -euo pipefail

profile=${1:?profile is required: consumer_contract, phase_a, or locked}
target=${TARGET:-wasm32-unknown-unknown}
package=re_mcap_web_adapter
out_dir=${OUT_DIR:-target/refact-20260828}
mkdir -p "$out_dir"

assert_absent() {
  local needle=$1
  local file=$2
  if grep -Fq -- "$needle" "$file"; then
    printf 'forbidden match: %s in %s\n' "$needle" "$file" >&2
    return 1
  fi
}

assert_present() {
  local needle=$1
  local file=$2
  if ! grep -Fq -- "$needle" "$file"; then
    printf 'missing required match: %s in %s\n' "$needle" "$file" >&2
    return 1
  fi
}

case "$profile" in
  consumer_contract)
    cargo tree --target "$target" --no-default-features --features consumer_contract \
      -e features -p "$package" > "$out_dir/tree-consumer.txt"
    cargo metadata --no-deps --format-version 1 > "$out_dir/metadata-consumer.json"
    # The consumer profile must not enable either producer or the producer features.
    assert_absent 're_web v' "$out_dir/tree-consumer.txt"
    assert_absent 're_mcap v' "$out_dir/tree-consumer.txt"
    assert_absent 'feature "phase_a"' "$out_dir/tree-consumer.txt"
    assert_absent 'feature "locked"' "$out_dir/tree-consumer.txt"
    # Positive consumer_contract assertion: the feature must be declared (and the empty
    # feature may not appear as a `cargo tree -e features` edge, hence the metadata check).
    grep -Fq '"consumer_contract"' "$out_dir/metadata-consumer.json" || {
      printf 'missing required feature: consumer_contract in %s\n' "$out_dir/metadata-consumer.json" >&2
      exit 1
    }
    ;;
  phase_a)
    cargo tree --target "$target" --no-default-features --features phase_a \
      -e features -p "$package" > "$out_dir/tree-phase-a.txt"
    cargo metadata --no-deps --format-version 1 > "$out_dir/metadata-phase-a.json"
    # phase_a enablement is proven by both producers being pulled in (the consumer profile
    # pulls neither). The root package's own feature is not emitted as a tree edge.
    assert_present 're_web v' "$out_dir/tree-phase-a.txt"
    assert_present 're_mcap v' "$out_dir/tree-phase-a.txt"
    assert_absent 'feature "locked"' "$out_dir/tree-phase-a.txt"
    grep -Fq '"phase_a"' "$out_dir/metadata-phase-a.json" || {
      printf 'missing required feature: phase_a in %s\n' "$out_dir/metadata-phase-a.json" >&2
      exit 1
    }
    ;;
  locked)
    # Negative rejection: locked may never run without the external-attestation preflight
    # marker. Authentic attestation is enforced by the build workflow, not this marker.
    if [[ "${RERUN_LOCKED_ATTESTATION_V1:-}" != "1" ]]; then
      printf 'locked profile requires the external attestation preflight marker\n' >&2
      exit 1
    fi
    cargo tree --target "$target" --no-default-features --features locked \
      -e features -p "$package" > "$out_dir/tree-locked.txt"
    cargo metadata --no-deps --format-version 1 > "$out_dir/metadata-locked.json"
    assert_present 're_web v' "$out_dir/tree-locked.txt"
    assert_present 're_mcap v' "$out_dir/tree-locked.txt"
    grep -Fq '"locked"' "$out_dir/metadata-locked.json" || {
      printf 'missing required feature: locked in %s\n' "$out_dir/metadata-locked.json" >&2
      exit 1
    }
    # Positive locked implementation assertions are deferred to the locked substage; the
    # locked producer is not implemented here.
    ;;
  *)
    printf 'unknown profile: %s\n' "$profile" >&2
    exit 1
    ;;
esac

# Locked-only modules must remain cfg-gated behind the locked allocator proof. This is a
# source-shape check, not a `cargo tree` needle: module names never appear in tree output.
lock_gate_evidence="$out_dir/re_mcap-locked-module-gates.txt"
# Record the gate + module lines (no line numbers) as evidence, then verify adjacency directly.
grep -E '^#\[cfg\(any\(test, re_mcap_locked_remote_wasm_allocator_v1\)\)\]|^mod (remote_decoder_assignment|remote_manifest|remote_chunk_dispatch|remote_runtime_intern);' \
  crates/store/re_mcap/src/lib.rs > "$lock_gate_evidence" || true
for module in remote_decoder_assignment remote_manifest remote_chunk_dispatch remote_runtime_intern; do
  if ! grep -B1 "^mod $module;" crates/store/re_mcap/src/lib.rs | grep -Fq 're_mcap_locked_remote_wasm_allocator_v1'; then
    printf 'locked-only module %s is not cfg-gated behind re_mcap_locked_remote_wasm_allocator_v1\n' "$module" >&2
    exit 1
  fi
done

printf 'audit passed: %s\n' "$profile"
