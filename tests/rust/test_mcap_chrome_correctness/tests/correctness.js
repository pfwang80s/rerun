// Production-disarmed Chrome correctness E2E.
//
// Top-level direct driver (no iframe) via `range_driver.js`. `sameOriginLegalRange` is not
// covered here: the wasm test page origin can never own the fixture object origin, so the
// same-origin no-CORS/preflight path is covered by the native `mcap_range_server` unit tests.
// See the WI-3/WI-4 handoff for the not-covered registry and unblock condition.
//
// This suite verifies the controlled Range fixture boundary only. It does not implement or
// exercise real remote-MCAP open/Store/query/playback. Those release gates belong to MCAP-114 or
// a later real-capability installation.

import {
  assertNoRangeLessGet,
  assertRedactedFixtureFailure,
  assertSrcdocCspBlocksObjectOrigin,
  baseScenario,
  browserEvent,
  expectReject,
  fetchBootstrap,
  fixtureEvent,
  withScenario,
} from "/tests/range_driver.js";

const POLL_ATTEMPTS = 120;

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

async function crossOriginLegalRange(bootstrap) {
  await withScenario(
    bootstrap,
    baseScenario({
      object_seed: 10,
      request: {
        range: { mode: "exact", value: "bytes=0-3" },
        if_match: { mode: "exact", value: '"v1"' },
        head: "mirror_get",
      },
      body: {
        type: "finite",
        chunks: [0, 1, 2, 3].map(() => ({ length: 1, delay_ms: 0, wait_for_gate: null })),
      },
    }),
    async (session) => {
      session.startHeartbeat(1);
      const result = await session.fetchByob({
        crossOrigin: true,
        headers: { Range: "bytes=0-3", "If-Match": '"v1"' },
        scratchBytes: 2,
        expectedRange: { start: 0, end: 3 },
        requiredResponseHeaders: ["content-range", "etag"],
      });
      const heartbeat = await session.stopHeartbeat();
      const instrumentation = session.getInstrumentation();
      assert(result.status === 206, "cross-origin legal Range returned wrong status");
      assert(result.visibleBytes === 4, "cross-origin legal Range returned wrong bytes");
      assert(result.chunks.flat().join(",") === "10,11,12,13", "cross-origin Range bytes changed");
      assert(result.reads >= 3, "cross-origin BYOB did not perform fresh reads");
      assert(result.eofChecks === 1, "cross-origin BYOB did not perform one EOF probe");
      assert(result.rebuiltViews === result.reads, "cross-origin BYOB reused a view");
      assert(heartbeat.count > 0, "cross-origin BYOB starved the browser heartbeat");
      assert(
        instrumentation.arrayBufferCallCount === 0,
        "cross-origin Range path called Response.arrayBuffer",
      );
      assert(instrumentation.eventSinkFailureCount === 0, "cross-origin event sink failed");

      const snapshot = await session.snapshot();
      assert(fixtureEvent(snapshot, "preflight"), "cross-origin Range skipped required preflight");
      assertNoRangeLessGet(snapshot, "cross-origin legal Range");
      assert(browserEvent(snapshot, "heartbeat"), "cross-origin heartbeat event missing");
    },
    { attempts: POLL_ATTEMPTS },
  );
}

async function corsFailures(bootstrap) {
  await withScenario(
    bootstrap,
    baseScenario({
      cors: {
        allow_origin: "omit",
        preflight: "allow_required_headers",
        expose: "required_range_headers",
      },
    }),
    async (session) => {
      const error = await expectReject(
        session.fetchByob({ crossOrigin: true, scratchBytes: 2, expectedBytes: 4 }),
        "CORS omit",
      );
      assertRedactedFixtureFailure(error, "CORS omit");
      assert(error.fixtureFailure?.code === "fetch_failed", "CORS omit failure code changed");
    },
    { attempts: POLL_ATTEMPTS },
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
      const error = await expectReject(
        session.fetchByob({
          crossOrigin: true,
          headers: { Range: "bytes=0-3", "If-Match": '"v1"' },
          scratchBytes: 2,
          expectedRange: { start: 0, end: 3 },
        }),
        "CORS preflight reject",
      );
      assertRedactedFixtureFailure(error, "CORS preflight reject");
      const snapshot = await session.snapshot();
      assert(fixtureEvent(snapshot, "preflight"), "rejected preflight was not observed");
    },
    { attempts: POLL_ATTEMPTS },
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
      const error = await expectReject(
        session.fetchByob({
          crossOrigin: true,
          headers: { Range: "bytes=0-3", "If-Match": '"v1"' },
          scratchBytes: 2,
          expectedRange: { start: 0, end: 3 },
          requiredResponseHeaders: ["content-range", "etag"],
        }),
        "CORS expose missing",
      );
      assertRedactedFixtureFailure(error, "CORS expose missing");
      assert(
        error.fixtureFailure?.code === "response_header_unavailable",
        "unexposed CORS response headers were not rejected",
      );
    },
    { attempts: POLL_ATTEMPTS },
  );
}

async function cspBlocksObjectOrigin(bootstrap) {
  await withScenario(
    bootstrap,
    baseScenario({ csp_connect: "self_only" }),
    async (session) => {
      const error = await expectReject(
        assertSrcdocCspBlocksObjectOrigin(session, { headers: { Range: "bytes=0-3" } }),
        "CSP block",
      );
      assertRedactedFixtureFailure(error, "CSP block");
      assert(error.fixtureFailure?.code === "fetch_failed", "CSP block failure code changed");
      const snapshot = await session.snapshot();
      assert(!fixtureEvent(snapshot, "request"), "CSP-blocked request reached the object origin");
    },
    { attempts: POLL_ATTEMPTS },
  );
}

async function malformedContentRange(bootstrap) {
  await withScenario(
    bootstrap,
    baseScenario({ content_range: { type: "omit" } }),
    async (session) => {
      const error = await expectReject(
        session.fetchByob({
          headers: { Range: "bytes=0-3" },
          scratchBytes: 2,
          expectedRange: { start: 0, end: 3 },
        }),
        "missing Content-Range",
      );
      assertRedactedFixtureFailure(error, "missing Content-Range");
      assert(
        error.fixtureFailure?.code === "unexpected_content_range" &&
          error.fixtureFailure?.stage === "response_headers",
        "missing Content-Range was not rejected at response headers",
      );
      const snapshot = await session.snapshot();
      assertNoRangeLessGet(snapshot, "missing Content-Range");
      const instrumentation = session.getInstrumentation();
      assert(
        instrumentation.arrayBufferCallCount === 0,
        "missing Content-Range path called Response.arrayBuffer",
      );
    },
    { attempts: POLL_ATTEMPTS },
  );

  await withScenario(
    bootstrap,
    baseScenario({ content_range: { type: "fixed", start: 5, end: 8, total: 64 } }),
    async (session) => {
      const error = await expectReject(
        session.fetchByob({
          headers: { Range: "bytes=0-3" },
          scratchBytes: 2,
          expectedRange: { start: 0, end: 3 },
        }),
        "inconsistent Content-Range",
      );
      assertRedactedFixtureFailure(error, "inconsistent Content-Range");
      assert(
        error.fixtureFailure?.code === "unexpected_content_range" &&
          error.fixtureFailure?.stage === "response_headers",
        "inconsistent Content-Range was not rejected at response headers",
      );
      const snapshot = await session.snapshot();
      assertNoRangeLessGet(snapshot, "inconsistent Content-Range");
      const instrumentation = session.getInstrumentation();
      assert(
        instrumentation.arrayBufferCallCount === 0,
        "inconsistent Content-Range path called Response.arrayBuffer",
      );
    },
    { attempts: POLL_ATTEMPTS },
  );
}

async function earlyEof(bootstrap) {
  await withScenario(
    bootstrap,
    baseScenario({
      body: {
        type: "finite",
        chunks: [{ length: 2, delay_ms: 0, wait_for_gate: null }],
      },
    }),
    async (session) => {
      const error = await expectReject(
        session.fetchByob({
          headers: { Range: "bytes=0-3" },
          scratchBytes: 2,
          expectedBytes: 4,
        }),
        "early EOF",
      );
      assertRedactedFixtureFailure(error, "early EOF");
      assert(
        error.fixtureFailure?.code === "unexpected_eof" &&
          error.fixtureFailure?.stage === "data_read",
        "early EOF was not rejected during data read",
      );
      const snapshot = await session.snapshot();
      assertNoRangeLessGet(snapshot, "early EOF");
      const instrumentation = session.getInstrumentation();
      assert(
        instrumentation.arrayBufferCallCount === 0,
        "early EOF path called Response.arrayBuffer",
      );
      const eofEvent = snapshot.events
        .filter((event) => event.type === "browser" && event.event.type === "byob_read")
        .at(-1).event;
      assert(eofEvent.done, "early EOF did not end the reader");
    },
    { attempts: POLL_ATTEMPTS },
  );
}

async function overlongBody(bootstrap) {
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
      assertRedactedFixtureFailure(error, "overlong response");
      assert(
        error.fixtureFailure?.code === "overlong_response" &&
          error.fixtureFailure?.stage === "eof_check",
        "overlong response was not rejected during the isolated EOF probe",
      );
      const snapshot = await session.waitFor(
        (events) => browserEvent(events, "abort_controller"),
        "overlong response abort",
      );
      assertNoRangeLessGet(snapshot, "overlong body");
      const instrumentation = session.getInstrumentation();
      assert(
        instrumentation.arrayBufferCallCount === 0,
        "overlong body path called Response.arrayBuffer",
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
    { attempts: POLL_ATTEMPTS },
  );
}

async function boundedInfiniteBody(bootstrap) {
  const infinite = baseScenario({
    content_length: { type: "omit" },
    body: {
      type: "infinite",
      chunk: { length: 1, delay_ms: 1, wait_for_gate: null },
    },
  });

  await withScenario(
    bootstrap,
    infinite,
    async (session) => {
      const result = await session.fetchByob({
        headers: { Range: "bytes=0-63" },
        scratchBytes: 2,
        expectedBytes: 64,
        cancelAfterBytes: 2,
      });
      assert(result.visibleBytes >= 2, "reader cancel happened before requested bytes");
      const snapshot = await session.waitFor(
        (events) =>
          browserEvent(events, "reader_cancelled") && fixtureEvent(events, "body_cancelled"),
        "reader cancellation",
      );
      assertNoRangeLessGet(snapshot, "reader-cancelled infinite body");
      const instrumentation = session.getInstrumentation();
      assert(
        instrumentation.arrayBufferCallCount === 0,
        "reader-cancelled infinite body called Response.arrayBuffer",
      );
      assert(browserEvent(snapshot, "reader_cancelled"), "reader cancellation was not recorded");
    },
    { attempts: POLL_ATTEMPTS },
  );

  await withScenario(
    bootstrap,
    infinite,
    async (session) => {
      const result = await session.fetchByob({
        headers: { Range: "bytes=0-63" },
        scratchBytes: 2,
        expectedBytes: 64,
        abortAfterBytes: 2,
      });
      assert(result.visibleBytes >= 2, "abort happened before requested bytes");
      const snapshot = await session.waitFor(
        (events) =>
          browserEvent(events, "abort_controller") && fixtureEvent(events, "body_cancelled"),
        "AbortController cancellation",
      );
      assertNoRangeLessGet(snapshot, "aborted infinite body");
      const instrumentation = session.getInstrumentation();
      assert(
        instrumentation.arrayBufferCallCount === 0,
        "aborted infinite body called Response.arrayBuffer",
      );
      assert(browserEvent(snapshot, "abort_controller"), "AbortController event was not recorded");
    },
    { attempts: POLL_ATTEMPTS },
  );
}

async function browserErrorRedaction(bootstrap) {
  await withScenario(
    bootstrap,
    baseScenario(),
    async (session) => {
      const error = await expectReject(
        Promise.resolve().then(() => session.injectUnsafeErrorForTest()),
        "unsafe browser error injection",
      );
      assertRedactedFixtureFailure(error, "browser error redaction");
      const failure = error.fixtureFailure;
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
    },
    { attempts: POLL_ATTEMPTS },
  );
}

function classifyStrictOpenCandidate(rawUrl) {
  if (!/^http:\/\//i.test(rawUrl) && !/^https:\/\//i.test(rawUrl)) {
    return "unsupported_route";
  }

  const pathEnd = (() => {
    const query = rawUrl.indexOf("?");
    const fragment = rawUrl.indexOf("#");
    const candidates = [rawUrl.length];
    if (query >= 0) {
      candidates.push(query);
    }
    if (fragment >= 0) {
      candidates.push(fragment);
    }
    return Math.min(...candidates);
  })();
  const schemeEnd = rawUrl.indexOf("://");
  if (schemeEnd < 0) {
    return "invalid_url";
  }

  const authorityStart = schemeEnd + 3;
  const authority = rawUrl.slice(authorityStart, pathEnd);
  const slash = authority.indexOf("/");
  const pathStart = slash < 0 ? pathEnd : authorityStart + slash;
  const path = rawUrl.slice(pathStart, pathEnd);

  if (path.toLowerCase().endsWith(".mcap")) {
    return "known_remote_mcap";
  }

  const lastSlash = path.lastIndexOf("/");
  const lastSegment = lastSlash < 0 ? path : path.slice(lastSlash + 1);
  if (lastSegment.includes(".")) {
    return "unsupported_route";
  }
  return "extensionless_sniff";
}

function assertDisarmedStrictClassification() {
  assert(
    classifyStrictOpenCandidate("https://example.test/data.mcap?token=secret#fragment") ===
      "known_remote_mcap",
    "explicit .mcap URL was not classified as known remote MCAP",
  );
  assert(
    classifyStrictOpenCandidate("HTTP://example.test/Data.MCAP") === "known_remote_mcap",
    "case-insensitive .mcap URL was not classified as known remote MCAP",
  );
  assert(
    classifyStrictOpenCandidate("https://example.test/no-extension?x=1") ===
      "extensionless_sniff",
    "extensionless URL was not classified for bounded strict sniffing",
  );
  assert(
    classifyStrictOpenCandidate("https://example.test/not-mcap.rrd") === "unsupported_route",
    "dotted non-MCAP URL was not rejected",
  );

  assert(
    classifyStrictOpenCandidate("https://example.test/a/../data.mcap") === "known_remote_mcap",
    "raw dot-segment path changed strict .mcap classification",
  );
  assert(
    classifyStrictOpenCandidate("https://example.test/a.mcap/..") === "unsupported_route",
    "trailing dot-segment was not rejected by the raw strict classifier",
  );
  assert(
    classifyStrictOpenCandidate("https://example.test\\data.mcap") === "extensionless_sniff",
    "raw backslash path did not match the current strict classifier boundary",
  );
}

export async function runRemoteMcapCorrectnessE2E() {
  assertDisarmedStrictClassification();

  const bootstrap = await fetchBootstrap();
  assert(bootstrap.page_origin !== bootstrap.object_origin, "fixture origins are not distinct");

  // `sameOriginLegalRange` is not covered by this top-level harness: the wasm test page origin
  // can never own the fixture object origin. See the WI-3/WI-4 handoff.
  await crossOriginLegalRange(bootstrap);
  await corsFailures(bootstrap);
  await cspBlocksObjectOrigin(bootstrap);
  await malformedContentRange(bootstrap);
  await earlyEof(bootstrap);
  await overlongBody(bootstrap);
  await boundedInfiniteBody(bootstrap);
  await browserErrorRedaction(bootstrap);
}
