#![cfg(target_arch = "wasm32")]

use js_sys::Promise;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::wasm_bindgen_test;

wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen(module = "/tests/release_gate.js")]
extern "C" {
    #[wasm_bindgen(js_name = runRemoteMcapDisarmedPreflightE2E)]
    fn run_remote_mcap_disarmed_preflight_e2e() -> Promise;
}

#[wasm_bindgen_test]
async fn production_disarmed_mcap114_fixture_preflight_is_explicit() {
    assert!(
        JsFuture::from(run_remote_mcap_disarmed_preflight_e2e())
            .await
            .is_ok(),
        "code=remote_mcap_release_gate_failed stage=test"
    );
}
