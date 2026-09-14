// Production-disarmed Chrome smoke for the controlled MCAP Range fixture.
//
// Top-level direct driver (no iframe): the wasm test page talks to the fixture server directly
// via `range_driver.js`. Service-worker scenarios are intentionally not covered here: SW
// registration requires a page whose own origin owns the object, which this top-level harness
// cannot provide. See the WI-3/WI-4 handoff for the not-covered registry and unblock condition.
//
// This suite verifies the controlled Range fixture boundary only. It does not implement or
// exercise real remote-MCAP open/Store/query/playback.
//
// NOTE: `range_driver.js` is duplicated verbatim in the `re_mcap_chrome_correctness` package
// because wasm-bindgen resolves JS module paths relative to each crate root.

import {
  assertNoRangeLessGet,
  assertSrcdocCspBlocksObjectOrigin,
  baseScenario,
  browserEvent,
  expectReject,
  fetchBootstrap,
  fixtureEvent,
  withScenario,
} from "/tests/range_driver.js";

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

async function legalCrossOrigin(bootstrap) {
  const spec = baseScenario({
    object_seed: 10,
    request: {
      range: { mode: "exact", value: "bytes=0-3" },
      if_match: { mode: "exact", value: "\"v1\"" },
      head: "mirror_get",
    },
    body: {
      type: "finite",
      chunks: [0, 1, 2, 3].map(() => ({ length: 1, delay_ms: 5, wait_for_gate: null })),
    },
  });
  await withScenario(bootstrap, spec, async (session) => {
    session.startHeartbeat(1);
    const result = await session.fetchByob({
      crossOrigin: true,
      headers: { Range: "bytes=0-3", "If-Match": "\"v1\"" },
      scratchBytes: 2,
      expectedRange: { start: 0, end: 3 },
      requiredResponseHeaders: ["content-range", "etag"],
    });
    const heartbeat = await session.stopHeartbeat();
    const instrumentation = session.getInstrumentation();
    assert(result.status === 206, "legal Range returned the wrong status");
    assert(result.visibleBytes === 4, "legal Range returned the wrong byte count");
    assert(result.chunks.flat().join(",") === "10,11,12,13", "legal Range bytes changed");
    assert(result.reads >= 3, "BYOB did not perform multiple slices and an EOF read");
    assert(result.eofChecks === 1, "BYOB did not perform exactly one extra EOF read");
    assert(result.rebuiltViews === result.reads, "BYOB reused a detached input view");
    assert(heartbeat.count > 0, "browser heartbeat was starved");
    assert(instrumentation.arrayBufferCallCount === 0, "BYOB path called Response.arrayBuffer");
    assert(instrumentation.eventSinkFailureCount === 0, "browser event sink failed");
    const snapshot = await session.snapshot();
    const readEvents = snapshot.events.filter(
      (event) => event.type === "browser" && event.event.type === "byob_read",
    );
    assert(readEvents.length === result.reads, "BYOB read instrumentation is incomplete");
    assert(
      readEvents.every((event) => event.event.rebuilt_view),
      "a BYOB read reused an input view or backing buffer",
    );
    const eofEvent = readEvents.at(-1).event;
    assert(eofEvent.requested_bytes === 1, "EOF probe did not use a one-byte view");
    assert(eofEvent.returned_bytes === 0, "EOF probe exposed overlong bytes");
    assert(eofEvent.done, "EOF probe did not return done=true");
    assert(fixtureEvent(snapshot, "preflight"), "legal cross-origin request skipped preflight");
    assert(browserEvent(snapshot, "heartbeat"), "heartbeat event was not recorded");
  });
}

async function boundedEofRejection(bootstrap) {
  await withScenario(
    bootstrap,
    baseScenario({
      body: {
        type: "finite",
        chunks: [{ length: 5, delay_ms: 0, wait_for_gate: null }],
      },
    }),
    async (session) => {
      const error = await expectReject(
        session.fetchByob({
          headers: { Range: "bytes=0-3" },
          scratchBytes: 2,
          expectedRange: { start: 0, end: 3 },
        }),
        "overlong response",
      );
      assert(error.fixtureFailure?.code === "overlong_response", "overlong code changed");
      assert(error.fixtureFailure?.stage === "eof_check", "overlong stage changed");
      const snapshot = await session.waitFor(
        (events) => browserEvent(events, "abort_controller"),
        "overlong response abort",
      );
      const applicationEvents = snapshot.events.filter(
        (event) => event.type === "browser" && event.event.type === "application_bytes",
      );
      assert(
        applicationEvents.at(-1).event.total === 4,
        "overlong byte entered application-visible retention",
      );
      const eofEvent = snapshot.events
        .filter((event) => event.type === "browser" && event.event.type === "byob_read")
        .at(-1).event;
      assert(
        eofEvent.requested_bytes === 1 && eofEvent.returned_bytes === 1,
        "overlong EOF probe was not isolated to one byte",
      );
    },
  );
}

async function corsAndCspFailures(bootstrap) {
  await withScenario(
    bootstrap,
    baseScenario({
      content_range: { type: "fixed", start: 0, end: 3, total: 64 },
      cors: {
        allow_origin: "omit",
        preflight: "allow_required_headers",
        expose: "required_range_headers",
      },
    }),
    async (session) => {
      await expectReject(
        session.fetchByob({ crossOrigin: true, scratchBytes: 2, expectedBytes: 4 }),
        "CORS rejection",
      );
    },
  );

  await withScenario(
    bootstrap,
    baseScenario({
      cors: {
        allow_origin: "request_origin",
        preflight: "reject",
        expose: "required_range_headers",
      },
    }),
    async (session) => {
      await expectReject(
        session.fetchByob({
          crossOrigin: true,
          headers: { Range: "bytes=0-3", "If-Match": "\"v1\"" },
          scratchBytes: 2,
          expectedRange: { start: 0, end: 3 },
        }),
        "preflight rejection",
      );
      const snapshot = await session.snapshot();
      assert(fixtureEvent(snapshot, "preflight"), "rejected preflight was not observed");
    },
  );

  await withScenario(
    bootstrap,
    baseScenario({
      cors: {
        allow_origin: "request_origin",
        preflight: "allow_required_headers",
        expose: "none",
      },
    }),
    async (session) => {
      await expectReject(
        session.fetchByob({
          crossOrigin: true,
          headers: { Range: "bytes=0-3", "If-Match": "\"v1\"" },
          expectedRange: { start: 0, end: 3 },
          requiredResponseHeaders: ["content-range", "etag"],
        }),
        "unexposed response headers",
      );
    },
  );

  await withScenario(bootstrap, baseScenario({ csp_connect: "self_only" }), async (session) => {
    await expectReject(
      assertSrcdocCspBlocksObjectOrigin(session, { headers: { Range: "bytes=0-3" } }),
      "CSP block",
    );
    const snapshot = await session.snapshot();
    assert(!fixtureEvent(snapshot, "request"), "CSP-blocked request reached the object origin");
  });
}

async function cancellationAndInstrumentation(bootstrap) {
  const infinite = baseScenario({
    content_length: { type: "omit" },
    body: {
      type: "infinite",
      chunk: { length: 1, delay_ms: 1, wait_for_gate: null },
    },
  });
  await withScenario(bootstrap, infinite, async (session) => {
    const result = await session.fetchByob({
      headers: { Range: "bytes=0-63" },
      scratchBytes: 2,
      expectedBytes: 64,
      cancelAfterBytes: 2,
    });
    assert(result.visibleBytes >= 2, "reader cancel happened before the requested bytes");
    const snapshot = await session.waitFor(
      (events) => browserEvent(events, "reader_cancelled") && fixtureEvent(events, "body_cancelled"),
      "reader cancellation",
    );
    assert(browserEvent(snapshot, "reader_cancelled"), "reader cancellation was not recorded");
  });

  await withScenario(bootstrap, infinite, async (session) => {
    const result = await session.fetchByob({
      headers: { Range: "bytes=0-63" },
      scratchBytes: 2,
      expectedBytes: 64,
      abortAfterBytes: 2,
    });
    assert(result.visibleBytes >= 2, "abort happened before the requested bytes");
    const snapshot = await session.waitFor(
      (events) => browserEvent(events, "abort_controller") && fixtureEvent(events, "body_cancelled"),
      "AbortController cancellation",
    );
    assert(browserEvent(snapshot, "abort_controller"), "AbortController event was not recorded");
  });

  await withScenario(bootstrap, baseScenario(), async (session) => {
    const result = await session.callArrayBuffer({ headers: { Range: "bytes=0-3" } });
    assert(result.byteLength === 4, "arrayBuffer probe returned the wrong length");
    assert(result.arrayBufferCallCount === 1, "arrayBuffer counter did not increment exactly once");
    const instrumentation = session.getInstrumentation();
    assert(instrumentation.arrayBufferCallCount === 1, "arrayBuffer instrumentation drifted");
    assert(instrumentation.eventSinkFailureCount === 0, "arrayBuffer event sink failed");
    const snapshot = await session.waitFor(
      (events) => browserEvent(events, "array_buffer_called"),
      "arrayBuffer event",
    );
    assert(browserEvent(snapshot, "array_buffer_called"), "arrayBuffer event was not recorded");
  });
}

async function browserErrorRedaction(bootstrap) {
  await withScenario(bootstrap, baseScenario(), async (session) => {
    const error = await expectReject(
      Promise.resolve().then(() => session.injectUnsafeErrorForTest()),
      "unsafe browser error injection",
    );
    const failure = error.fixtureFailure;
    assert(failure !== undefined, "browser failure was not structured");
    assert(
      Object.keys(failure).sort().join(",") === "code,command,stage",
      "browser failure exposed non-contract fields",
    );
    assert(failure.code === "browser_api_failed", "browser failure code was not normalized");
    assert(
      failure.command === "injectUnsafeErrorForTest" && failure.stage === "execute",
      "browser failure command or stage was not normalized",
    );
    const retainedFailure = `${error.message} ${JSON.stringify(failure)}`;
    const nonce = new URL(session.descriptor.page_url).pathname.split("/")[3];
    assert(nonce?.length === 32, "fixture nonce was not discoverable for redaction test");
    for (const forbidden of [
      nonce,
      session.descriptor.cross_origin_object_url,
      "authorization",
      "fixture-secret",
    ]) {
      assert(!retainedFailure.includes(forbidden), "browser failure retained unsafe input");
    }
  });
}

export async function runChromeSmoke() {
  const bootstrap = await fetchBootstrap();
  assert(bootstrap.page_origin !== bootstrap.object_origin, "fixture origins are not distinct");
  await legalCrossOrigin(bootstrap);
  await boundedEofRejection(bootstrap);
  await corsAndCspFailures(bootstrap);
  // `serviceWorkerPaths` is not covered by this top-level harness: SW registration and scope
  // require a page whose own origin owns the object. See the WI-3/WI-4 handoff.
  await cancellationAndInstrumentation(bootstrap);
  await browserErrorRedaction(bootstrap);
}
