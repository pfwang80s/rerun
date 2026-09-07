#![cfg(target_arch = "wasm32")]

use js_sys::Promise;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::wasm_bindgen_test;

wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen(module = "/tests/file_backed_range.js")]
extern "C" {
    #[wasm_bindgen(js_name = runFileBackedRangeBoundaryE2E)]
    fn run_file_backed_range_boundary_e2e() -> Promise;
}

#[wasm_bindgen_test]
async fn file_backed_range_boundary_is_explicit_in_chrome() {
    let result = JsFuture::from(run_file_backed_range_boundary_e2e()).await;
    if let Err(error) = &result {
        // Propagate the JS assertion message so failures are actionable in CI logs.
        let message = error
            .as_string()
            .unwrap_or_else(|| format!("non-string JS rejection: {error:?}"));
        panic!("Chrome file-backed Range boundary failed: {message}");
    }
    assert!(
        result.is_ok(),
        "Chrome file-backed Range boundary failed: code=file_backed_range_failed stage=test"
    );
}
