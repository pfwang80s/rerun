const FIXTURE_PROTOCOL = "mcap-range-fixture-v1";
const MESSAGE_PROTOCOL = "mcap-range-v1";
const COMMAND_TIMEOUT_MS = 5_000;

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
    const timeout = window.setTimeout(() => reject(new Error("fixture iframe load timed out")), COMMAND_TIMEOUT_MS);
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
        const error = new Error(
          `${failure.command}:${failure.code}@${failure.stage}`,
        );
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
          registerServiceWorker() {
            return command(
              bootstrap,
              iframe,
              descriptor.id,
              "registerServiceWorker",
            );
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
            for (let attempt = 0; attempt < 80; attempt += 1) {
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
          await command(
            bootstrap,
            iframe,
            descriptor.id,
            "unregisterServiceWorkers",
          ).catch(() => {});
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

async function legalCrossOrigin(harness) {
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
  await harness.withScenario(spec, async (page) => {
    await page.run("startHeartbeat", { periodMs: 1 });
    const result = await page.run("fetchByob", {
      options: {
        crossOrigin: true,
        headers: { Range: "bytes=0-3", "If-Match": "\"v1\"" },
        scratchBytes: 2,
        expectedRange: { start: 0, end: 3 },
        requiredResponseHeaders: ["content-range", "etag"],
      },
    });
    const heartbeat = await page.run("stopHeartbeat");
    const instrumentation = await page.run("getInstrumentation");
    assert(result.status === 206, "legal Range returned the wrong status");
    assert(result.visibleBytes === 4, "legal Range returned the wrong byte count");
    assert(result.chunks.flat().join(",") === "10,11,12,13", "legal Range bytes changed");
    assert(result.reads >= 3, "BYOB did not perform multiple slices and an EOF read");
    assert(result.eofChecks === 1, "BYOB did not perform exactly one extra EOF read");
    assert(result.rebuiltViews === result.reads, "BYOB reused a detached input view");
    assert(heartbeat.count > 0, "browser heartbeat was starved");
    assert(instrumentation.arrayBufferCallCount === 0, "BYOB path called Response.arrayBuffer");
    assert(instrumentation.eventSinkFailureCount === 0, "browser event sink failed");
    const snapshot = await page.snapshot();
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

async function boundedEofRejection(harness) {
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
      assert(error.fixtureFailure?.code === "overlong_response", "overlong code changed");
      assert(error.fixtureFailure?.stage === "eof_check", "overlong stage changed");
      const snapshot = await page.waitFor(
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

async function corsAndCspFailures(harness) {
  await harness.withScenario(
    baseScenario({
      content_range: { type: "fixed", start: 0, end: 3, total: 64 },
      cors: {
        allow_origin: "omit",
        preflight: "allow_required_headers",
        expose: "required_range_headers",
      },
    }),
    async (page) => {
      await expectReject(
        page.run("fetchByob", {
          options: { crossOrigin: true, scratchBytes: 2, expectedBytes: 4 },
        }),
        "CORS rejection",
      );
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
      await expectReject(
        page.run("fetchByob", {
          options: {
            crossOrigin: true,
            headers: { Range: "bytes=0-3", "If-Match": "\"v1\"" },
            scratchBytes: 2,
            expectedRange: { start: 0, end: 3 },
          },
        }),
        "preflight rejection",
      );
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
      await expectReject(
        page.run("fetchByob", {
          options: {
            crossOrigin: true,
            headers: { Range: "bytes=0-3", "If-Match": "\"v1\"" },
            expectedRange: { start: 0, end: 3 },
            requiredResponseHeaders: ["content-range", "etag"],
          },
        }),
        "unexposed response headers",
      );
    },
  );

  await harness.withScenario(
    baseScenario({ csp_connect: "self_only" }),
    async (page) => {
      await expectReject(
        page.run("fetchByob", {
          options: {
            crossOrigin: true,
            headers: { Range: "bytes=0-3" },
            expectedRange: { start: 0, end: 3 },
          },
        }),
        "CSP block",
      );
      const snapshot = await page.snapshot();
      assert(!fixtureEvent(snapshot, "request"), "CSP-blocked request reached the object origin");
    },
  );
}

async function serviceWorkerPaths(harness) {
  await harness.withScenario(
    baseScenario({ service_worker: { type: "forward_no_store" } }),
    async (page) => {
      await page.registerServiceWorker();
      const result = await page.run("fetchByob", {
        options: {
          headers: { Range: "bytes=0-3" },
          scratchBytes: 2,
          expectedRange: { start: 0, end: 3 },
        },
      });
      assert(result.visibleBytes === 4, "forwarding service worker changed response bytes");
      const snapshot = await page.waitFor(
        (events) => browserEvent(events, "service_worker_forwarded"),
        "service worker forward event",
      );
      assert(browserEvent(snapshot, "service_worker_forwarded"), "service worker did not forward");
    },
  );

  await harness.withScenario(
    baseScenario({
      service_worker: {
        type: "synthetic",
        status: 206,
        headers: {
          "content-range": "bytes 0-63/64",
          "content-length": "64",
          etag: "\"sw\"",
        },
        body: {
          type: "finite",
          chunks: [{ length: 64, fill: 7, delay_ms: 0 }],
        },
      },
    }),
    async (page) => {
      await page.registerServiceWorker();
      await page.run("startHeartbeat", { periodMs: 1 });
      const result = await page.run("fetchByob", {
        options: { scratchBytes: 1, expectedBytes: 64 },
      });
      const heartbeat = await page.run("stopHeartbeat");
      assert(result.visibleBytes === 64, "ready SW bytes were not fully consumed");
      assert(result.reads === 65, "ready SW stream did not use 64 reads plus one EOF probe");
      assert(
        result.chunks.every((chunk) => chunk.length === 1 && chunk[0] === 7),
        "ready SW stream changed application bytes",
      );
      assert(heartbeat.count > 0, "explicit BYOB macrotask yields did not service heartbeat");
      const snapshot = await page.snapshot();
      assert(browserEvent(snapshot, "heartbeat"), "ready SW heartbeat event was not recorded");
      assert(
        browserEvent(snapshot, "service_worker_synthetic"),
        "ready SW synthetic route was not recorded",
      );
    },
  );

  await harness.withScenario(
    baseScenario({
      service_worker: {
        type: "synthetic",
        status: 206,
        headers: {},
        body: { type: "zero_progress_then_stall", pulls: 2, delay_ms: 1 },
      },
    }),
    async (page) => {
      await page.registerServiceWorker();
      await expectReject(
        page.run("fetchByob", {
          options: { timeoutMs: 250, scratchBytes: 2, expectedBytes: 1 },
        }),
        "zero-progress synthetic stream",
      );
      const snapshot = await page.waitFor(
        (events) =>
          browserEvent(events, "service_worker_synthetic") &&
          browserEvent(events, "service_worker_zero_progress") &&
          browserEvent(events, "abort_controller"),
        "synthetic zero-progress abort",
      );
      assert(browserEvent(snapshot, "service_worker_synthetic"), "synthetic SW was not used");
    },
  );
}

async function cancellationAndInstrumentation(harness) {
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
    assert(result.visibleBytes >= 2, "reader cancel happened before the requested bytes");
    const snapshot = await page.waitFor(
      (events) => browserEvent(events, "reader_cancelled") && fixtureEvent(events, "body_cancelled"),
      "reader cancellation",
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
    assert(result.visibleBytes >= 2, "abort happened before the requested bytes");
    const snapshot = await page.waitFor(
      (events) => browserEvent(events, "abort_controller") && fixtureEvent(events, "body_cancelled"),
      "AbortController cancellation",
    );
    assert(browserEvent(snapshot, "abort_controller"), "AbortController event was not recorded");
  });

  await harness.withScenario(baseScenario(), async (page) => {
    const result = await page.run("callArrayBuffer", {
      options: { headers: { Range: "bytes=0-3" } },
    });
    assert(result.byteLength === 4, "arrayBuffer probe returned the wrong length");
    assert(result.arrayBufferCallCount === 1, "arrayBuffer counter did not increment exactly once");
    const instrumentation = await page.run("getInstrumentation");
    assert(instrumentation.arrayBufferCallCount === 1, "arrayBuffer instrumentation drifted");
    assert(instrumentation.eventSinkFailureCount === 0, "arrayBuffer event sink failed");
    const snapshot = await page.waitFor(
      (events) => browserEvent(events, "array_buffer_called"),
      "arrayBuffer event",
    );
    assert(browserEvent(snapshot, "array_buffer_called"), "arrayBuffer event was not recorded");
  });
}

async function browserErrorRedaction(harness) {
  await harness.withScenario(baseScenario(), async (page) => {
    const error = await expectReject(
      page.run("injectUnsafeErrorForTest"),
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

export async function runChromeSmoke() {
  const bootstrap = await responseJson(
    await fetch(
      `http://127.0.0.1:${fixturePort()}/__mcap_range_fixture/v1/bootstrap`,
      { cache: "no-store" },
    ),
    "fixture bootstrap",
  );
  assert(bootstrap.protocol === FIXTURE_PROTOCOL, "fixture protocol version mismatch");
  assert(bootstrap.page_origin !== bootstrap.object_origin, "fixture origins are not distinct");
  const harness = createHarness(bootstrap);
  await legalCrossOrigin(harness);
  await boundedEofRejection(harness);
  await corsAndCspFailures(harness);
  await serviceWorkerPaths(harness);
  await cancellationAndInstrumentation(harness);
  await browserErrorRedaction(harness);
}
