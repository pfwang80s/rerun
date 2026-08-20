---
title: Migrating from 0.35 to 0.36
order: 974
---

## Web Viewer compatibility `open` and `start`

Compatibility URL arrays passed to `WebViewer.start` or `WebViewer.open` are now attempted independently in input order.
An item failure emits a warning and does not throw, stop the Viewer, or roll back earlier or later items.
Callers that relied on catching an individual URL dispatch exception should instead treat the warning as request-local and let later items continue.
This does not change errors for starting the Viewer itself or calling `open` after the Viewer has stopped.

```js
// 0.35 behavior: a caller may expect the whole array to fail when one item fails.
try {
  await viewer.start(["https://example.test/bad", "https://example.test/good"], parent, options);
} catch {
  // ...
}

// 0.36 behavior: the bad item warns, and the good item is still attempted.
await viewer.start(["https://example.test/bad", "https://example.test/good"], parent, options);
```

### Strict remote-MCAP open APIs

`WebViewer.openRequest`, `openBatch`, and `startWithRequests` are additive strict APIs.
They accept HTTP(S) remote-MCAP sources only and expose typed options, opaque handles, and redacted request-local errors.

```ts
// Strict singleton open.
const operation = await viewer.openRequest({
  url: "https://example.test/recording.mcap",
  options: {
    mcap_time_type: "timestamp_ns",
    recording_open_behavior: "open",
  },
});

// Strict atomic batch open.
const operations = await viewer.openBatch([
  { url: "https://example.test/first.mcap" },
  {
    url: "https://example.test/extensionless",
    options: { allow_extensionless_sniff: true },
  },
]);

// Strict startup uses the same atomic request transaction.
const startupOperations = await viewer.startWithRequests(
  [{ url: "https://example.test/first.mcap" }],
  parent,
  options,
);
```

`openRequest` is a singleton transaction.
`openBatch` is all-or-nothing.
`startWithRequests` resolves only after the Viewer and remote operation handoff are safely published.
It does not resolve when a recording is presentation-ready.

### Extensionless compatibility routing

Only an explicit HTTP(S) compatibility URL whose path ends in `.mcap` enters the remote-MCAP lane.
Extensionless compatibility URLs continue through the existing dispatcher and do not perform the new strict 8-byte format sniff.

```ts
// Compatibility extensionless routing is unchanged.
await viewer.start("https://example.test/recording", parent, options);

// Strict extensionless routing requires an explicit opt-in.
await viewer.openRequest({
  url: "https://example.test/recording",
  options: { allow_extensionless_sniff: true },
});
```

### Handle close and dispose semantics

`OpenRequestHandle.close()` closes the shared source owned by the operation.
`RecordingHandle.close()` closes only one exact recording publication.
`dispose()` on either handle releases subscriptions, pending promises, wrapper retention, and removed tombstones without closing the source or publication.

```ts
operation.close(); // Close the shared source and every exact publication it owns.
recording.close(); // Close only this exact publication.
operation.dispose(); // Release operation subscriptions without closing the source.
recording.dispose(); // Release recording subscriptions without closing the publication.
```

The `FinalizationRegistry` is only a best-effort fallback.
It invokes the same internal cleanup token as explicit `dispose()`.
An already-active alias replays the current presentation snapshot independently.
Public handles never expose a Store ID, recording ID, URL, lifecycle token, or secret.

### Remote hidden suspension and page teardown

`ChromePageExecutionController` is a remote-MCAP-only page manager.
Hidden state suspends only remote-MCAP owners and work.
It does not pause the Viewer frame driver, canvas, or non-MCAP stores and receivers.
`pagehide` and `freeze` synchronously tear down remote-MCAP owners while keeping the Viewer and non-MCAP state alive.
Returning from BFCache requires an explicit reopen.
Old remote tokens and generations are not revived.

### Capability-unavailable strict requests

Until the measured release-Wasm remote-MCAP capability is installed, valid strict requests reject with `StrictOpenError` code `CapabilityUnavailable`.
This rejection happens before handles or work are created.

```ts
try {
  await viewer.openRequest({ url: "https://example.test/recording.mcap" });
} catch (error) {
  if (error instanceof StrictOpenError) {
    console.log(error.code); // "CapabilityUnavailable"
  }
}
```

## `ParquetReader` loading options moved to `stream()`

The experimental `ParquetReader`'s constructor now takes only the file path.
All loading options (`entity_path_prefix`, `column_grouping`, `delimiter`, `prefixes`, `use_structs`, `static_columns`, `index_columns`) moved to `stream()`:

```python
# 0.35
ParquetReader(path, column_grouping="individual", index_columns=[IndexColumn.sequence("frame")]).stream()

# 0.36
ParquetReader(path).stream(column_grouping="individual", index_columns=[IndexColumn.sequence("frame")])
```

The reader is now a lightweight handle over the file, and each `stream()` call is independent — one reader can drive several differently-configured streams over the same file.

## `rerun mcap info` output changed

The `rerun mcap info` CLI command has been rewritten to output richer and more detailed file-level information instead of just diagnostic checks.
The diagnostic checks are now in a dedicated `rerun mcap check` subcommand instead.
