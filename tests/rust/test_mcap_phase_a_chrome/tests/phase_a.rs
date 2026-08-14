#![cfg(target_arch = "wasm32")]

use js_sys::Promise;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::wasm_bindgen_test;

wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen(module = "/tests/phase_a.js")]
extern "C" {
    #[wasm_bindgen(js_name = runMcapPhaseABenchmark)]
    fn run_mcap_phase_a_benchmark() -> Promise;
}

#[wasm_bindgen_test]
async fn release_wasm_phase_a_benchmark_emits_complete_evidence() {
    JsFuture::from(run_mcap_phase_a_benchmark())
        .await
        .expect("release-Wasm Chrome Phase A benchmark failed");
}
