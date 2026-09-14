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

// Extracts JS error name/message/fixtureFailure so the panic text identifies the failing stage.
fn js_error_text(value: &JsValue) -> String {
    let mut text = String::new();
    for key in ["name", "message", "stack", "code", "stage"] {
        if let Ok(member) = js_sys::Reflect::get(value, &JsValue::from_str(key)) {
            if let Some(string) = member.as_string() {
                text.push_str(&format!("[{key}] {string}\n"));
            } else if !member.is_undefined() && !member.is_null() {
                text.push_str(&format!("[{key}] {member:?}\n"));
            }
        }
    }
    if text.is_empty() {
        text = format!("{value:?}");
    }
    text
}

#[wasm_bindgen_test]
async fn controlled_range_fixture_works_in_chrome() {
    match JsFuture::from(run_chrome_smoke()).await {
        Ok(_) => {}
        Err(error) => {
            panic!(
                "Chrome MCAP Range fixture smoke failed: code=chrome_smoke_failed command=runChromeSmoke stage=test\n{}",
                js_error_text(&error)
            );
        }
    }
}
