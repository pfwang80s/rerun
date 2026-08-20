#![cfg(target_arch = "wasm32")]

use js_sys::Promise;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::wasm_bindgen_test;

wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen(module = "/tests/correctness.js")]
extern "C" {
    #[wasm_bindgen(js_name = runRemoteMcapCorrectnessE2E)]
    fn run_remote_mcap_correctness_e2e() -> Promise;
}

#[wasm_bindgen_test]
async fn production_disarmed_chrome_correctness_boundaries_are_explicit() {
    assert!(
        JsFuture::from(run_remote_mcap_correctness_e2e())
            .await
            .is_ok(),
        "code=remote_mcap_correctness_failed stage=test"
    );
}
