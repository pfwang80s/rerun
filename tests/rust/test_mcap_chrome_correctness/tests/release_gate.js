// Production-disarmed MCAP-114 release-gate boundary harness.
//
// This suite exercises only the current disarmed classification and extensionless HTTP Range
// boundary. It does not install or exercise real remote-MCAP open, Store, query, playback, seek,
// GC, or interner exhaustion.

const FIXTURE_PROTOCOL = "mcap-range-fixture-v1";
const MESSAGE_PROTOCOL = "mcap-range-v1";
const COMMAND_TIMEOUT_MS = 10_000;

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
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

function assertRedactedFixtureFailure(error, label) {
  const failure = error.fixtureFailure;
  assert(failure !== undefined, `${label} was not a structured fixture failure`);
  assert(
    Object.keys(failure).sort().join(",") === "code,command,stage",
    `${label} exposed non-contract failure fields`,
  );
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

function assertNoRemoteOwnerSignals(snapshot, label) {
  assert(!snapshot.overflowed, `${label} fixture event log overflowed`);
  for (const event of snapshot.events) {
    if (event.type === "request") {
      assert(!event.query_present, `${label} observed query-bearing remote traffic`);
      assert(!event.redirected_target, `${label} observed redirected remote traffic`);
    }
    if (event.type === "browser") {
      assert(
        ![
          "remote_owner_claimed",
          "remote_probe_started",
          "remote_slot_claimed",
        ].includes(event.event.type),
        `${label} observed a remote owner or probe event`,
      );
    }
  }
}

function assertNoForbiddenDiagnosticMaterial(value, label) {
  const lower = value.toLowerCase();
  for (const forbidden of [
    "http://",
    "https://",
    "?",
    "etag",
    "topic",
    "entity_path",
    "entity path",
    "store_id",
    "store id",
    "token",
    "fixture nonce",
  ]) {
    assert(!lower.includes(forbidden), `${label} retained forbidden diagnostic material`);
  }
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

async function extensionlessRange(harness) {
  await harness.withScenario(
    baseScenario({
      object_seed: 9,
      body: {
        type: "finite",
        chunks: [{ length: 4, delay_ms: 0, wait_for_gate: null }],
      },
    }),
    async (page) => {
      const result = await page.run("fetchByob", {
        options: {
          headers: { Range: "bytes=0-3", "If-Match": '"v1"' },
          scratchBytes: 4,
          expectedRange: { start: 0, end: 3 },
          requiredResponseHeaders: ["content-range", "etag"],
        },
      });
      const instrumentation = await page.run("getInstrumentation");
      assert(result.status === 206, "extensionless Range returned the wrong status");
      assert(result.visibleBytes === 4, "extensionless Range returned wrong bytes");
      assert(result.chunks.flat().join(",") === "9,10,11,12", "extensionless Range bytes changed");
      assert(
        instrumentation.arrayBufferCallCount === 0,
        "extensionless Range path called Response.arrayBuffer",
      );

      const snapshot = await page.snapshot();
      assertNoRangeLessGet(snapshot, "extensionless Range");
      assertNoRemoteOwnerSignals(snapshot, "extensionless Range");
    },
  );
}

async function extensionlessRedactedFailure(harness) {
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
            scratchBytes: 4,
            expectedBytes: 4,
          },
        }),
        "extensionless bounded EOF",
      );
      assertRedactedFixtureFailure(error, "extensionless bounded EOF");
      assert(error.fixtureFailure?.code === "unexpected_eof", "extensionless failure code changed");
      assert(error.fixtureFailure?.stage === "data_read", "extensionless failure stage changed");
      const retainedFailure = `${error.message} ${JSON.stringify(error.fixtureFailure)}`;
      assertNoForbiddenDiagnosticMaterial(retainedFailure, "extensionless bounded EOF");
    },
  );
}

export async function runRemoteMcapDisarmedPreflightE2E() {

  const bootstrap = await responseJson(
    await fetch(`http://127.0.0.1:${fixturePort()}/__mcap_range_fixture/v1/bootstrap`, {
      cache: "no-store",
    }),
    "fixture bootstrap",
  );
  assert(bootstrap.protocol === FIXTURE_PROTOCOL, "fixture protocol version mismatch");
  assert(bootstrap.page_origin !== bootstrap.object_origin, "fixture origins are not distinct");

  const harness = createHarness(bootstrap);
  await extensionlessRange(harness);
  await extensionlessRedactedFailure(harness);
}
