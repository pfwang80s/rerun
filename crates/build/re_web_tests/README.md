# re_web_tests

Discovers and runs browser-based Wasm tests in the Rerun workspace.

## Native test servers

A package can request the existing Redap test server with the following metadata.

```toml
[package.metadata.rerun.web-test]
redap-server = true
```

A package can request the controlled MCAP Range fixture with the following metadata.

```toml
[package.metadata.rerun.web-test]
mcap-range-server = true
```

The runner starts two random `127.0.0.1` ports for the Range fixture.
The first port serves the controlled page and a same-origin object route, while the second port serves a genuinely cross-origin object route.
Only the non-secret port numbers are added to `WASM_BINDGEN_TEST_ADDRESS` as `mcap_fixture_page_port` and `mcap_fixture_object_port`.
Browser tests discover the versioned control root and random nonce from `http://127.0.0.1:<mcap_fixture_page_port>/__mcap_range_fixture/v1/bootstrap`.

The control root accepts bounded `ScenarioSpec` JSON at `POST <control-root>/control/scenarios`.
Each successful response provides a controlled-page URL plus same-origin and cross-origin object URLs for an isolated scenario.
Scenarios can script Range and `If-Match` expectations, `HEAD`, CORS and preflight, exposed headers, CSP, redirect, status, `Content-Range`, `Content-Length`, `Content-Encoding`, ETag shape, response revisions, finite, stalled or infinite bodies, deterministic gates, and service-worker forwarding or synthesis.
The controlled page exposes `window.mcapRangeFixture` and a `postMessage` interface for BYOB reads, heartbeat observation, reader cancellation and `AbortController` tests.
It also reports application-visible bytes and calls to `Response.arrayBuffer()` to the bounded event log.
Every BYOB command supplies `expectedBytes`, `expectedRange`, or both.
The helper bounds each data view by the remaining expected bytes and then performs exactly one fresh one-byte EOF read, rejecting and cancelling an overlong response before retaining its extra byte.
Command failures cross the browser boundary only as bounded `{code, command, stage}` values; native browser messages, URLs, headers, scopes and fixture nonces are never forwarded.

Fetch `GET <control-root>/control/scenarios/<id>` to inspect the sanitized event snapshot.
Use `POST <control-root>/control/scenarios/<id>/gates/<gate>` to release a body gate and `DELETE <control-root>/control/scenarios/<id>` to cancel and remove a scenario.
Event records retain only selected bounded headers and a boolean indicating whether a query was present; URL paths, queries, referrers and other headers are not logged.
Explicit server shutdown cancels every streaming body before closing both listeners, and the server's `Drop` implementation is a final non-blocking cleanup fallback.

Packages can restrict browser discovery with a non-empty `browsers = ["firefox"]` or `browsers = ["chrome"]` metadata list.
The default is both supported browsers.
The reusable web-test workflow runs the existing suite in Firefox and runs `re_mcap_chrome_test` in a dedicated Chrome stable lane with a matching ChromeDriver.
The Chrome probe exercises the fixture and browser primitives only; it does not implement or test the production Range reader.
