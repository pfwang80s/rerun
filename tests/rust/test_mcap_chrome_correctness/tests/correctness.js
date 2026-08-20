// Production-disarmed Chrome correctness E2E.
//
// This suite verifies the controlled Range fixture boundary only. It does not implement or
// exercise real remote-MCAP open/Store/query/playback. Those release gates belong to MCAP-114 or
// a later real-capability installation.

const FIXTURE_PROTOCOL = "mcap-range-fixture-v1";
const MESSAGE_PROTOCOL = "mcap-range-v1";
const COMMAND_TIMEOUT_MS = 10_000;

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

function delay(milliseconds) {
  return new Promise((resolve) => window.setTimeout(resolve, milliseconds));
}

function baseScenario(overrides = {}) {
  return {
    object_length: 64,
    object_seed: 0,
    request: {
      range: { mode: "any" },
      if_match: { mode: "any" },
      head: "mirror_get",
    },
    status: 206,
    content_range: { type: "derived_from_request" },
    content_length: { type: "body_length" },
    content_encoding: "omit",
    etag: { type: "strong", value: "v1" },
    cors: {
      allow_origin: "page_origin",
      preflight: "allow_required_headers",
      expose: "required_range_headers",
    },
    redirect: "none",
    csp_connect: "self_and_object_origin",
    body: {
      type: "finite",
      chunks: [{ length: 4, delay_ms: 0, wait_for_gate: null }],
    },
    service_worker: { type: "disabled" },
    response_revisions: [],
    ...overrides,
  };
}

function fixturePort() {
  const rawPort = new URLSearchParams(window.location.search).get("mcap_fixture_page_port");
  assert(rawPort !== null && /^\d+$/.test(rawPort), "missing MCAP fixture page port");
  const port = Number(rawPort);
  assert(Number.isSafeInteger(port) && port > 0 && port <= 65_535, "invalid fixture port");
  return port;
}

async function responseJson(response, label) {
  if (!response.ok) {
    throw new Error(`${label} failed with status ${response.status}`);
  }
  return response.json();
}

async function loadIframe(url) {
  const iframe = document.createElement("iframe");
  iframe.hidden = true;
  const loaded = new Promise((resolve, reject) => {
    const timeout = window.setTimeout(
      () => reject(new Error("fixture iframe load timed out")),
      COMMAND_TIMEOUT_MS,
    );
    iframe.addEventListener(
      "load",
      () => {
        window.clearTimeout(timeout);
        resolve();
      },
      { once: true },
    );
    iframe.addEventListener(
      "error",
      () => {
        window.clearTimeout(timeout);
        reject(new Error("fixture iframe failed to load"));
      },
      { once: true },
    );
  });
  iframe.src = url;
  document.body.append(iframe);
  try {
    await loaded;
    return iframe;
  } catch (error) {
    iframe.remove();
    throw error;
  }
}

let nextRequestId = 1;

function command(bootstrap, iframe, scenarioId, commandName, extra = {}) {
  const requestId = nextRequestId;
  nextRequestId += 1;
  return new Promise((resolve, reject) => {
    const timeout = window.setTimeout(() => {
      window.removeEventListener("message", receive);
      reject(new Error(`fixture command timed out: ${commandName}`));
    }, COMMAND_TIMEOUT_MS);

    function receive(message) {
      if (
        message.source !== iframe.contentWindow ||
        message.origin !== bootstrap.page_origin ||
        message.data?.fixture !== MESSAGE_PROTOCOL ||
        message.data?.requestId !== requestId
      ) {
        return;
      }
      window.clearTimeout(timeout);
      window.removeEventListener("message", receive);
      if (message.data.ok) {
        resolve(message.data.result);
      } else {
        const failure = message.data.error;
        const error = new Error(`${failure.command}:${failure.code}@${failure.stage}`);
        error.fixtureFailure = failure;
        reject(error);
      }
    }

    window.addEventListener("message", receive);
    iframe.contentWindow.postMessage(
      {
        fixture: MESSAGE_PROTOCOL,
        scenarioId,
        requestId,
        command: commandName,
        ...extra,
      },
      bootstrap.page_origin,
    );
  });
}

async function expectReject(promise, label) {
  try {
    await promise;
  } catch (error) {
    return error;
  }
  throw new Error(`${label} unexpectedly succeeded`);
}

function browserEvent(snapshot, type) {
  return snapshot.events.some(
    (event) => event.type === "browser" && event.event.type === type,
  );
}

function fixtureEvent(snapshot, type) {
  return snapshot.events.some((event) => event.type === type);
}

function requestEvents(snapshot, method = undefined) {
  return snapshot.events.filter(
    (event) => event.type === "request" && (method === undefined || event.method === method),
  );
}

function assertNoRangeLessGet(snapshot, label) {
  const getRequests = requestEvents(snapshot, "GET");
  assert(getRequests.length > 0, `${label} produced no GET request`);
  for (const request of getRequests) {
    assert(
      request.range?.state === "ascii" && request.range.value.startsWith("bytes="),
      `${label} observed a Range-less GET`,
    );
  }
}

function assertRedactedFixtureFailure(error, label) {
  const failure = error.fixtureFailure;
  assert(failure !== undefined, `${label} was not a structured fixture failure`);
  assert(
    Object.keys(failure).sort().join(",") === "code,command,stage",
    `${label} exposed non-contract failure fields`,
  );
}

function createHarness(bootstrap) {
  const controlBase = `${bootstrap.page_origin}${bootstrap.control_root}`;

  return {
    async withScenario(spec, test) {
      const descriptor = await responseJson(
        await fetch(`${controlBase}/control/scenarios`, {
          method: "POST",
          cache: "no-store",
          headers: { "content-type": "application/json" },
          body: JSON.stringify(spec),
        }),
        "scenario registration",
      );

      let iframe;
      try {
        iframe = await loadIframe(descriptor.page_url);
        const api = {
          descriptor,
          run(commandName, extra) {
            return command(bootstrap, iframe, descriptor.id, commandName, extra);
          },
          async snapshot() {
            return responseJson(
              await fetch(`${controlBase}/control/scenarios/${descriptor.id}`, {
                cache: "no-store",
              }),
              "event snapshot",
            );
          },
          async waitFor(predicate, label) {
            for (let attempt = 0; attempt < 120; attempt += 1) {
              const snapshot = await this.snapshot();
              if (predicate(snapshot)) {
                return snapshot;
              }
              await delay(25);
            }
            throw new Error(`timed out waiting for ${label}`);
          },
        };
        await test(api);
      } finally {
        if (iframe !== undefined) {
          await command(bootstrap, iframe, descriptor.id, "unregisterServiceWorkers").catch(
            () => {},
          );
          iframe.remove();
        }
        await fetch(`${controlBase}/control/scenarios/${descriptor.id}`, {
          method: "DELETE",
          cache: "no-store",
        });
      }
    },
  };
}

async function sameOriginLegalRange(harness) {
  await harness.withScenario(
    baseScenario({
      object_seed: 5,
      body: {
        type: "finite",
        chunks: [{ length: 4, delay_ms: 0, wait_for_gate: null }],
      },
    }),
    async (page) => {
      const result = await page.run("fetchByob", {
        options: {
          headers: { Range: "bytes=0-3", "If-Match": '"v1"' },
          scratchBytes: 2,
          expectedRange: { start: 0, end: 3 },
          requiredResponseHeaders: ["content-range", "etag"],
        },
      });
      const instrumentation = await page.run("getInstrumentation");
      assert(result.status === 206, "same-origin legal Range returned the wrong status");
      assert(result.visibleBytes === 4, "same-origin legal Range returned wrong bytes");
      assert(result.chunks.flat().join(",") === "5,6,7,8", "same-origin Range bytes changed");
      assert(result.reads >= 3, "same-origin BYOB did not perform a fresh EOF read");
      assert(result.eofChecks === 1, "same-origin BYOB did not perform one EOF probe");
      assert(result.rebuiltViews === result.reads, "BYOB reused an input view");
      assert(instrumentation.arrayBufferCallCount === 0, "Range path called Response.arrayBuffer");
      assert(instrumentation.eventSinkFailureCount === 0, "browser event sink failed");

      const snapshot = await page.snapshot();
      assertNoRangeLessGet(snapshot, "same-origin legal Range");
      const eofEvent = snapshot.events
        .filter((event) => event.type === "browser" && event.event.type === "byob_read")
        .at(-1).event;
      assert(eofEvent.requested_bytes === 1, "same-origin EOF probe was not one byte");
      assert(eofEvent.returned_bytes === 0, "same-origin EOF probe returned bytes");
      assert(eofEvent.done, "same-origin EOF probe did not return done=true");
    },
  );
}

async function crossOriginLegalRange(harness) {
  await harness.withScenario(
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
    async (page) => {
      await page.run("startHeartbeat", { periodMs: 1 });
      const result = await page.run("fetchByob", {
        options: {
          crossOrigin: true,
          headers: { Range: "bytes=0-3", "If-Match": '"v1"' },
          scratchBytes: 2,
          expectedRange: { start: 0, end: 3 },
          requiredResponseHeaders: ["content-range", "etag"],
        },
      });
      const heartbeat = await page.run("stopHeartbeat");
      const instrumentation = await page.run("getInstrumentation");
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

      const snapshot = await page.snapshot();
      assert(fixtureEvent(snapshot, "preflight"), "cross-origin Range skipped required preflight");
      assertNoRangeLessGet(snapshot, "cross-origin legal Range");
      assert(browserEvent(snapshot, "heartbeat"), "cross-origin heartbeat event missing");
    },
  );
}

async function corsFailures(harness) {
  await harness.withScenario(
    baseScenario({
      cors: {
        allow_origin: "omit",
        preflight: "allow_required_headers",
        expose: "required_range_headers",
      },
    }),
    async (page) => {
      const error = await expectReject(
        page.run("fetchByob", {
          options: { crossOrigin: true, scratchBytes: 2, expectedBytes: 4 },
        }),
        "CORS omit",
      );
      assertRedactedFixtureFailure(error, "CORS omit");
      assert(error.fixtureFailure?.code === "fetch_failed", "CORS omit failure code changed");
    },
  );

  await harness.withScenario(
    baseScenario({
      cors: {
        allow_origin: "page_origin",
        preflight: "reject",
        expose: "required_range_headers",
      },
    }),
    async (page) => {
      const error = await expectReject(
        page.run("fetchByob", {
          options: {
            crossOrigin: true,
            headers: { Range: "bytes=0-3", "If-Match": '"v1"' },
            scratchBytes: 2,
            expectedRange: { start: 0, end: 3 },
          },
        }),
        "CORS preflight reject",
      );
      assertRedactedFixtureFailure(error, "CORS preflight reject");
      const snapshot = await page.snapshot();
      assert(fixtureEvent(snapshot, "preflight"), "rejected preflight was not observed");
    },
  );

  await harness.withScenario(
    baseScenario({
      cors: {
        allow_origin: "page_origin",
        preflight: "allow_required_headers",
        expose: "none",
      },
    }),
    async (page) => {
      const error = await expectReject(
        page.run("fetchByob", {
          options: {
            crossOrigin: true,
            headers: { Range: "bytes=0-3", "If-Match": '"v1"' },
            scratchBytes: 2,
            expectedRange: { start: 0, end: 3 },
            requiredResponseHeaders: ["content-range", "etag"],
          },
        }),
        "CORS expose missing",
      );
      assertRedactedFixtureFailure(error, "CORS expose missing");
      assert(
        error.fixtureFailure?.code === "response_header_unavailable",
        "unexposed CORS response headers were not rejected",
      );
    },
  );
}

async function cspBlocksObjectOrigin(harness) {
  await harness.withScenario(baseScenario({ csp_connect: "self_only" }), async (page) => {
    const error = await expectReject(
      page.run("fetchByob", {
        options: {
          crossOrigin: true,
          headers: { Range: "bytes=0-3" },
          scratchBytes: 2,
          expectedRange: { start: 0, end: 3 },
        },
      }),
      "CSP block",
    );
    assertRedactedFixtureFailure(error, "CSP block");
    assert(error.fixtureFailure?.code === "fetch_failed", "CSP block failure code changed");
    const snapshot = await page.snapshot();
    assert(!fixtureEvent(snapshot, "request"), "CSP-blocked request reached the object origin");
  });
}

async function malformedContentRange(harness) {
  await harness.withScenario(
    baseScenario({ content_range: { type: "omit" } }),
    async (page) => {
      const error = await expectReject(
        page.run("fetchByob", {
          options: {
            headers: { Range: "bytes=0-3" },
            scratchBytes: 2,
            expectedRange: { start: 0, end: 3 },
          },
        }),
        "missing Content-Range",
      );
      assertRedactedFixtureFailure(error, "missing Content-Range");
      assert(
        error.fixtureFailure?.code === "unexpected_content_range" &&
          error.fixtureFailure?.stage === "response_headers",
        "missing Content-Range was not rejected at response headers",
      );
      const snapshot = await page.snapshot();
      assertNoRangeLessGet(snapshot, "missing Content-Range");
      const instrumentation = await page.run("getInstrumentation");
      assert(
        instrumentation.arrayBufferCallCount === 0,
        "missing Content-Range path called Response.arrayBuffer",
      );
    },
  );

  await harness.withScenario(
    baseScenario({ content_range: { type: "fixed", start: 5, end: 8, total: 64 } }),
    async (page) => {
      const error = await expectReject(
        page.run("fetchByob", {
          options: {
            headers: { Range: "bytes=0-3" },
            scratchBytes: 2,
            expectedRange: { start: 0, end: 3 },
          },
        }),
        "inconsistent Content-Range",
      );
      assertRedactedFixtureFailure(error, "inconsistent Content-Range");
      assert(
        error.fixtureFailure?.code === "unexpected_content_range" &&
          error.fixtureFailure?.stage === "response_headers",
        "inconsistent Content-Range was not rejected at response headers",
      );
      const snapshot = await page.snapshot();
      assertNoRangeLessGet(snapshot, "inconsistent Content-Range");
      const instrumentation = await page.run("getInstrumentation");
      assert(
        instrumentation.arrayBufferCallCount === 0,
        "inconsistent Content-Range path called Response.arrayBuffer",
      );
    },
  );
}

async function earlyEof(harness) {
  await harness.withScenario(
    baseScenario({
      body: {
        type: "finite",
        chunks: [{ length: 2, delay_ms: 0, wait_for_gate: null }],
      },
    }),
    async (page) => {
      const error = await expectReject(
        page.run("fetchByob", {
          options: {
            headers: { Range: "bytes=0-3" },
            scratchBytes: 2,
            expectedBytes: 4,
          },
        }),
        "early EOF",
      );
      assertRedactedFixtureFailure(error, "early EOF");
      assert(
        error.fixtureFailure?.code === "unexpected_eof" &&
          error.fixtureFailure?.stage === "data_read",
        "early EOF was not rejected during data read",
      );
      const snapshot = await page.snapshot();
      assertNoRangeLessGet(snapshot, "early EOF");
      const instrumentation = await page.run("getInstrumentation");
      assert(
        instrumentation.arrayBufferCallCount === 0,
        "early EOF path called Response.arrayBuffer",
      );
      const eofEvent = snapshot.events
        .filter((event) => event.type === "browser" && event.event.type === "byob_read")
        .at(-1).event;
      assert(eofEvent.done, "early EOF did not end the reader");
    },
  );
}

async function overlongBody(harness) {
  await harness.withScenario(
    baseScenario({
      body: {
        type: "finite",
        chunks: [{ length: 5, delay_ms: 0, wait_for_gate: null }],
      },
    }),
    async (page) => {
      const error = await expectReject(
        page.run("fetchByob", {
          options: {
            headers: { Range: "bytes=0-3" },
            scratchBytes: 2,
            expectedRange: { start: 0, end: 3 },
          },
        }),
        "overlong response",
      );
      assertRedactedFixtureFailure(error, "overlong response");
      assert(
        error.fixtureFailure?.code === "overlong_response" &&
          error.fixtureFailure?.stage === "eof_check",
        "overlong response was not rejected during the isolated EOF probe",
      );
      const snapshot = await page.waitFor(
        (events) => browserEvent(events, "abort_controller"),
        "overlong response abort",
      );
      assertNoRangeLessGet(snapshot, "overlong body");
      const instrumentation = await page.run("getInstrumentation");
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
  );
}

async function boundedInfiniteBody(harness) {
  const infinite = baseScenario({
    content_length: { type: "omit" },
    body: {
      type: "infinite",
      chunk: { length: 1, delay_ms: 1, wait_for_gate: null },
    },
  });

  await harness.withScenario(infinite, async (page) => {
    const result = await page.run("fetchByob", {
      options: {
        headers: { Range: "bytes=0-63" },
        scratchBytes: 2,
        expectedBytes: 64,
        cancelAfterBytes: 2,
      },
    });
    assert(result.visibleBytes >= 2, "reader cancel happened before requested bytes");
    const snapshot = await page.waitFor(
      (events) =>
        browserEvent(events, "reader_cancelled") && fixtureEvent(events, "body_cancelled"),
      "reader cancellation",
    );
    assertNoRangeLessGet(snapshot, "reader-cancelled infinite body");
    const instrumentation = await page.run("getInstrumentation");
    assert(
      instrumentation.arrayBufferCallCount === 0,
      "reader-cancelled infinite body called Response.arrayBuffer",
    );
    assert(browserEvent(snapshot, "reader_cancelled"), "reader cancellation was not recorded");
  });

  await harness.withScenario(infinite, async (page) => {
    const result = await page.run("fetchByob", {
      options: {
        headers: { Range: "bytes=0-63" },
        scratchBytes: 2,
        expectedBytes: 64,
        abortAfterBytes: 2,
      },
    });
    assert(result.visibleBytes >= 2, "abort happened before requested bytes");
    const snapshot = await page.waitFor(
      (events) =>
        browserEvent(events, "abort_controller") && fixtureEvent(events, "body_cancelled"),
      "AbortController cancellation",
    );
    assertNoRangeLessGet(snapshot, "aborted infinite body");
    const instrumentation = await page.run("getInstrumentation");
    assert(
      instrumentation.arrayBufferCallCount === 0,
      "aborted infinite body called Response.arrayBuffer",
    );
    assert(browserEvent(snapshot, "abort_controller"), "AbortController event was not recorded");
  });
}

async function browserErrorRedaction(harness) {
  await harness.withScenario(baseScenario(), async (page) => {
    const error = await expectReject(
      page.run("injectUnsafeErrorForTest"),
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
    const nonce = new URL(page.descriptor.page_url).pathname.split("/")[3];
    assert(nonce?.length === 32, "fixture nonce was not discoverable for redaction test");
    for (const forbidden of [
      nonce,
      page.descriptor.cross_origin_object_url,
      "authorization",
      "fixture-secret",
    ]) {
      assert(!retainedFailure.includes(forbidden), "browser failure retained unsafe input");
    }
  });
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

  const bootstrap = await responseJson(
    await fetch(`http://127.0.0.1:${fixturePort()}/__mcap_range_fixture/v1/bootstrap`, {
      cache: "no-store",
    }),
    "fixture bootstrap",
  );
  assert(bootstrap.protocol === FIXTURE_PROTOCOL, "fixture protocol version mismatch");
  assert(bootstrap.page_origin !== bootstrap.object_origin, "fixture origins are not distinct");

  const harness = createHarness(bootstrap);
  await sameOriginLegalRange(harness);
  await crossOriginLegalRange(harness);
  await corsFailures(harness);
  await cspBlocksObjectOrigin(harness);
  await malformedContentRange(harness);
  await earlyEof(harness);
  await overlongBody(harness);
  await boundedInfiniteBody(harness);
  await browserErrorRedaction(harness);
}
