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

Remote-MCAP page execution is tracked independently from the Viewer frame driver.
`ChromePageExecutionController` exposes typed visible, hidden, revalidating, and terminating states for integrations that need to coordinate remote-MCAP work with browser lifecycle signals.

```js
import { WebViewer } from "@rerun-io/web-viewer";

const rrd = "…";
const parentElement = document.body;

const viewer = new WebViewer();
await viewer.start(rrd, parentElement, { width: "800px", height: "600px" });
// …
viewer.stop();
```

Compatibility URLs passed to `start` or `open` are attempted independently in input order.
If one item fails, the Viewer emits a warning and continues with later items without throwing, stopping the Viewer, or rolling back earlier successful items.

The additive `openRequest` and `openBatch` APIs provide strict HTTP(S) remote-MCAP admission, typed options, structured request-local errors, and stable opaque operation handles.
`openBatch` is all-or-nothing and preserves input order.
Explicit `.mcap` URLs are accepted directly, while extensionless URLs require `allow_extensionless_sniff: true` and use a bounded format sniff when the remote transport capability is installed.
Non-HTTP routes, gRPC/message proxy URLs, Redap URLs, RRD files, and other formats are rejected by the strict APIs and are never handed to the compatibility importer.
Until the measured release-Wasm remote-MCAP capability is installed, valid strict requests reject with `StrictOpenError` code `CapabilityUnavailable` before creating handles or scheduling work.

When the capability is installed, each opaque `RecordingHandle` targets one exact remote Store publication.
Its `select()`, `seek({time_type, value})`, `play("paused" | "playing")`, and `close()` methods return a synchronous redacted `StrictRecordingControlResult`.
`seek` accepts only `timestamp_ns` or `duration_ns` and a canonical decimal value.
`OpenRequestHandle.close()` closes the operation's shared source, while `RecordingHandle.close()` closes only its exact publication.
`dispose()` on either handle releases subscriptions, pending promises, wrapper retention, and removed tombstones without closing the source or publication.
The `FinalizationRegistry` is only a best-effort fallback that invokes the same internal cleanup token as explicit `dispose()`.
Neither handle exposes a Store ID, recording ID, URL, or lifecycle token.

```ts
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
