#![cfg(target_arch = "wasm32")]

use js_sys::Promise;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::wasm_bindgen_test;

wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen(module = "/tests/browser_scheduler.js")]
extern "C" {
    #[wasm_bindgen(js_name = runBrowserSchedulerSmoke)]
    fn run_browser_scheduler_smoke() -> Promise;
}

#[wasm_bindgen_test]
async fn real_chrome_scheduler_boundaries_are_explicitly_gated() {
    assert!(
        JsFuture::from(run_browser_scheduler_smoke()).await.is_ok(),
        "Chrome scheduler smoke failed: code=browser_scheduler_failed stage=test"
    );
}
