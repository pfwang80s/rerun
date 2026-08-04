# Web Viewer public-contract characterization

These tests freeze the compatibility behavior that predates strict remote MCAP opening.

The JavaScript suite compiles and executes the production `index.ts` and replaces only the generated Wasm module and browser DOM.
The fake `WebHandle` records boundary calls and can inject an ABI exception without duplicating `WebViewer` logic.

The suite freezes the following current behavior:

- `start` and `open` dispatch URL arrays item by item in caller order, including HTTP file, Rerun proxy, and Redap dataset strings.
- The hidden `options.url` value is passed to Rust startup independently, before direct `start(rrd)` calls `open` after the Viewer becomes ready.
- An ABI exception on a later array item leaves the earlier call attempted, stops the entire wrapper, invalidates the accepted prefix, and rethrows.
- A malformed URL is not an ABI exception because Rust `add_receiver` logs the parse error and returns normally; the Rust URL test freezes the malformed input boundary.
- `recording_open` events preserve arrival order, allow multiple Stores from one source, and carry no request correlation or Store identity.
- Notebook and Gradio raw events run synchronously inside the Wasm callback, while parsed public events use a later macrotask.
- `close(url)` removes the matching receiver and recording by URI without stopping the Viewer.
- `LogChannel` send and close calls are synchronous while ready and become silent no-ops after channel close or Viewer stop.
- Raw recording-ID controls forward the caller's string and retain their existing missing-recording fallback or no-op behavior.
- Public methods reject a stopped wrapper synchronously, while repeated `stop()` calls are no-ops.

The Rust component suite in `crates/viewer/re_viewer/tests/web_public_contracts.rs` complements the wrapper tests with the production `App`, `viewer_harness`, `LogReceiver`, RRD encoder/decoder, and `StoreHub` paths.
It freezes reverse completion selection, one receiver installing and selecting multiple Stores, real `HttpStream` panel-close cleanup, malformed-item continuation, startup URL dispatch before the initial stable App state, and decoded RRD delivery through a real JS-channel source.
The RRD component test uses native `DecoderApp`; it does not claim to cover wasm `web_decode` yielding or `WebHandle::send_rrd_to_channel` scheduling.
The `ViewerOpenUrl` unit suite separately verifies the concrete `SystemCommand` or `LogDataSource` emitted for every compatibility URL family in the public route matrix.
The shared recording-ID resolver test freezes duplicate-ID first-match behavior, missing-ID fallback, and the absence of StoreHub mutation.
The hidden-startup unit suite invokes the exact `dispatch_hidden_startup_urls` helper used by wasm `create_app`, with the production `StringOrStringArray`, `ViewerOpenUrl::from_str`, loader commands, malformed warning, and continue-in-order behavior.

The following browser-owned effects remain for the two-phase WebRunner work in MCAP-104 and the desktop Chrome E2E suite in MCAP-111, and are not simulated by the Node replacement module or native `viewer_harness`:

- JavaScript deserialization into `StringOrStringArray` and the browser timing between wasm `create_app` dispatch and the first real eframe update.
- `WebHandle::destroy/free` cleanup of real ResizeObserver, request-animation-frame, DOM callback, Fetch, and Redap connection ownership.

Those boundaries cannot be instantiated by the native harness because `web.rs` is compiled only for `wasm32` and owns an `eframe::WebRunner`.
They remain mandatory compatibility assertions in the later Chrome suite; the native startup test deliberately claims only the closest production App contract.

The runtime event characterization includes Rust's actual `segment_id: null` wire field.
The public TypeScript `ViewerEventBase` currently declares `partition_id?` instead, which is a pre-existing type/wire mismatch recorded here rather than changed by this compatibility-only item.

## Compatibility disposition

The design's section "当前代码约束与集成边界" keeps compatibility `open/start` per-item dispatch, non-throwing malformed URL handling, all existing URL route families, programmatic selection on every completed Store, source-level close, and stopped-wrapper fatal ABI handling.
The currently declared accepted safety changes include notebook and Gradio raw delivery becoming asynchronous, hidden or uncredited legacy synchronous LogChannel send rejecting before copying, pagehide and freeze tearing down the Viewer, and non-resumable live Web transports terminalizing on hidden rather than reconnecting unsafely.
Section 8.5.1 additionally bounds suspended compatibility commands by their algebraic meaning: absolute setters use latest-wins, toggles fold by effective state or parity, relative step/move commands use bounded accumulation or explicit rejection, atomic timeline-plus-seek bundles stay atomic, and commands with external side effects are rejected instead of silently overwritten.
The Web Redap detached-publication contract rejects unavailable, empty, or invalid manifests before any StoreHub mutation instead of falling back to unsafe direct streaming.
This list records the safety changes relevant to these characterization tests and is not an exhaustive migration guide.
The additive strict HTTP API and exact recording handles do not redefine these compatibility assertions.
