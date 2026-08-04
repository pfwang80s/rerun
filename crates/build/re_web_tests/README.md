# re_web_tests

Discovers and runs browser-based Wasm tests in the Rerun workspace.

## Deterministic asynchronous tests

The [`deterministic_async`](src/deterministic_async.rs) module is a native, single-threaded test scheduler for local futures.
It polls real [`Future`](https://doc.rust-lang.org/std/future/trait.Future.html) and [`Waker`](https://doc.rust-lang.org/std/task/struct.Waker.html) implementations while tests explicitly choose network, microtask, macrotask, animation-frame, Viewer-frame or Store-callback lanes.
Its browser lanes are a deterministic model for unit tests, not evidence of Chrome event-loop behavior.
The Chrome-only `re_mcap_chrome_test` package separately exercises a minimal adapter with real `queueMicrotask`, `setTimeout`, `requestAnimationFrame` and repaint callbacks.

The harness provides generation-aware task slots, completion cells, named checkpoint gates, bounded ack-retaining queues, lease drain futures, manual monotonic and wall clocks, page-signal delivery, repaint/frame allowances and real `AbortHandle` cancellation.
Queue deliveries carry a generation token and source-local sequence, and acknowledgement remains retryable so tests can prove wrong, duplicate and stale acknowledgements do not release replacement-generation ownership.
Every retained task, waiter, owner, queue item, delivery, timer and lease is represented by RAII accounting with count and byte high-water marks.
The scheduler has a mandatory step budget, a bounded enum-only diagnostic trace and stale-wakeup filtering for slot reuse.
Tests must call `stop`, deliver any intended late callbacks, drop external handles and assert a zero-live-resource snapshot before returning.

This module does not implement remote-open, lifecycle, legacy-import or Store-mutation state machines.
Those production components remain responsible for their own transitions and use these primitives only to expose and control their real callback boundaries in tests.

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
