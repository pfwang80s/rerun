# Rerun web viewer

Embed the Rerun web viewer within your app.

<p align="center">
  <picture>
    <img src="https://static.rerun.io/opf_screenshot/bee51040cba93c0bae62ef6c57fa703704012a41/full.png" alt="">
    <source media="(max-width: 480px)" srcset="https://static.rerun.io/opf_screenshot/bee51040cba93c0bae62ef6c57fa703704012a41/480w.png">
    <source media="(max-width: 768px)" srcset="https://static.rerun.io/opf_screenshot/bee51040cba93c0bae62ef6c57fa703704012a41/768w.png">
    <source media="(max-width: 1024px)" srcset="https://static.rerun.io/opf_screenshot/bee51040cba93c0bae62ef6c57fa703704012a41/1024w.png">
    <source media="(max-width: 1200px)" srcset="https://static.rerun.io/opf_screenshot/bee51040cba93c0bae62ef6c57fa703704012a41/1200w.png">
  </picture>
</p>

This package is framework-agnostic. A React wrapper is available at <https://www.npmjs.com/package/@rerun-io/web-viewer-react>.

## Install

```sh
npm i @rerun-io/web-viewer
```

ℹ️ Note:
The package version is equal to the supported Rerun SDK version, and [RRD files are only partially stable across different versions](https://rerun.io/blog/release-0.23).
This means that:
- `@rerun-io/web-viewer@0.10.0` can only connect to a data source (`.rrd` file, gRPC connection, etc.) that originates from a Rerun SDK with version `0.10.0`!
- For versions after `@rerun-io/web-viewer@0.23.0`, the Viewer can load data from the previous _minor_ version of Rerun, e.g. `0.24` can load `0.23` files.

## Usage

The entrypoint for this packages is the [`WebViewer`](https://ref.rerun.io/docs/js/0.35.0/web-viewer/classes/WebViewer.html) class.
The web viewer is an object which manages a canvas element:

```js
import { WebViewer } from "@rerun-io/web-viewer";

const rrd = "…";
const parentElement = document.body;

const viewer = new WebViewer();
await viewer.start(rrd, parentElement, { width: "800px", height: "600px" });
// …
viewer.stop();
```

## Remote-MCAP contracts

Remote-MCAP page execution is tracked independently from the Viewer frame driver.
`ChromePageExecutionController` is a remote-MCAP-only page manager.
It exposes typed visible, hidden, revalidating, and terminating states for integrations that need to coordinate remote-MCAP work with browser lifecycle signals.
Hidden state suspends only remote-MCAP owners and work.
It does not pause the Viewer frame driver, canvas, or non-MCAP stores and receivers.
`pagehide` and `freeze` synchronously tear down remote-MCAP owners while keeping the Viewer, canvas, and non-MCAP stores and receivers alive.
Returning from BFCache requires an explicit reopen, and old remote tokens and generations are not revived.

### Compatibility lane

Compatibility URLs passed to `start` or `open` are attempted independently in input order.
An item failure emits a warning and does not throw, stop the Viewer, or roll back earlier or later items.
Only an explicit HTTP(S) URL whose path ends in `.mcap` enters the remote-MCAP lane.
Extensionless compatibility URLs and every non-MCAP route continue through the existing dispatcher.
They create zero new remote-MCAP probe or slot ownership.

### Strict lane

`openRequest`, `openBatch`, and `startWithRequests` are additive strict APIs.
They accept only HTTP(S) remote-MCAP sources.
Explicit `.mcap` URLs are admitted directly.
Extensionless URLs require `allow_extensionless_sniff: true` and use a bounded 8-byte format sniff.
`openRequest` is a singleton transaction.
`openBatch` is all-or-nothing.
`startWithRequests` resolves only after the Viewer and remote operation handoff are safely published.
It does not resolve when a recording becomes presentation-ready.
Strict failures are redacted `StrictOpenError` values with fixed codes and phases.
`StrictOpenErrorCode` is `ViewerStopped`, `InvalidRequestShape`, `InvalidUrl`, `UnsupportedStrictOpenRoute`, `UnsupportedFormat`, `ExistingSourceOptionsConflict`, `BatchTooLarge`, `ResourceLimitExceeded`, `CapabilityUnavailable`, `HandoffCancelled`, `HandoffStateChanged`, or `ProtocolViolation`.
`StrictOpenErrorPhase` is `admission`, `handoff`, `opening`, or `lifecycle`.
Valid strict requests reject with `CapabilityUnavailable` while the release-Wasm remote-MCAP capability is not installed.

### Handles and lifecycle

Each `OpenRequestHandle` owns one strict operation, and each `RecordingHandle` targets one exact remote Store publication.
`OpenRequestHandle.close()` closes the shared source owned by the operation.
`RecordingHandle.close()` closes only one exact recording publication.
`dispose()` on either handle releases subscriptions, pending promises, wrapper retention, and removed tombstones without closing the source or publication.
The `FinalizationRegistry` is only a best-effort fallback.
It invokes the same internal cleanup token as explicit `dispose()`.
Lifecycle events are the exported `StrictOpenLifecycleEvent` values `accepted`, `activated`, `behavior_ready`, `presentation_ready`, `terminal`, and `removed`, in that order.
Lifecycle transitions are contiguous and non-reversing: each transition moves to the immediately following event.
An already-active alias replays the current presentation snapshot independently.
Public handles never expose a Store ID, recording ID, URL, lifecycle token, or secret.

### Semantic reuse and behavior

The same canonical URL may share a source only when the complete frozen `RemoteMcapSemanticConfig` is exact-equal and actual consistency satisfies the requested policy.
Each operation keeps independent `RecordingOpenBehavior`, selection intent, ready state, and disposal.
The three behavior values are `open`, `open_and_select`, and `background`, corresponding to the `Open`, `OpenAndSelect`, and `Background` contract modes.
`background` creates a separate discoverable and closeable metadata catalog card.
It is not equivalent to an existing StoreHub preview.
Behavior effects are monotonic.
A strict atomic batch uses only its last `open_and_select` item for batch-local initial selection authority.

### Limits and redaction

Remote URL and option strings are bounded and redacted.
Strict public handles, `StrictOpenError`, strict lifecycle events, remote-MCAP metrics, and remote-MCAP-owned debug output do not expose a raw URL, query, ETag, topic, entity path, Store ID, generation, or internal token.
The compatibility `.mcap` fallback can still emit a raw URL in debug output.
`seek` accepts only `timestamp_ns` or `duration_ns` plus a canonical decimal value.
The public API does not provide `timestamp_offset_ns`.
It does not guess Unix epoch or boot-relative semantics from time values.

### Unchanged routes

RRD, legacy HTTP/RRD, `rerun+http(s)` gRPC/message proxy, Redap, raw-event, native Viewer, local MCAP, drag-and-drop MCAP, and other non-remote Web routes remain unchanged.

```ts
const viewer = new WebViewer();
await viewer.start(null, document.body, null);

const handles = await viewer.openBatch([
  { url: "https://example.test/first.mcap" },
  {
    url: "https://example.test/extensionless",
    options: { allow_extensionless_sniff: true },
  },
]);
```

The existing `open` and `start` APIs remain the compatibility lane.
In particular, extensionless compatibility URLs still use the existing dispatcher and do not perform the strict format sniff.

The `rrd` in the snippet above should be a URL pointing to either:
- A hosted `.rrd` file, such as <https://app.rerun.io/version/0.35.0/examples/dna.rrd>
- A gRPC connection to the SDK opened via the [`serve`](https://www.rerun.io/docs/reference/sdk/operating-modes#serve) API

If `rrd` is not set, the Viewer will display the same welcome screen as <https://app.rerun.io>.
This can be disabled by setting `hide_welcome_screen` to `true` in the options object of `viewer.start`.

⚠ It's important to set the viewer's width and height, as without it the viewer may not display correctly.
Setting the values to empty strings is valid, as long as you style the canvas through other means.

For a full example, see https://github.com/rerun-io/web-viewer-example.
You can open the example via CodeSandbox: https://codesandbox.io/s/github/rerun-io/web-viewer-example

ℹ️ Note:
This package only targets recent versions of browsers.
If your target browser does not support Wasm imports or top-level await, you may need to install additional plugins for your bundler.

For more information about using the package, visit:
- [Integration docs](https://rerun.io/docs/howto/integrations/embed-web#using-the-javascript-package).
- [Package docs](https://ref.rerun.io/docs/js/0.26.0/web-viewer/index.html).
