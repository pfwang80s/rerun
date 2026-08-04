#![cfg(target_arch = "wasm32")]

use js_sys::Promise;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::wasm_bindgen_test;

wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen(module = "/tests/chrome_smoke.js")]
extern "C" {
    #[wasm_bindgen(js_name = runChromeSmoke)]
    fn run_chrome_smoke() -> Promise;
}

#[wasm_bindgen_test]
async fn controlled_range_fixture_works_in_chrome() {
    assert!(
        JsFuture::from(run_chrome_smoke()).await.is_ok(),
        "Chrome MCAP Range fixture smoke failed: code=chrome_smoke_failed command=runChromeSmoke stage=test"
    );
}
