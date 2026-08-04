const FIXTURE_PREFIX = "/__mcap_range_fixture/v1/";
const pathParts = window.location.pathname.split("/").filter(Boolean);
const prefixIndex = pathParts.indexOf("v1");
const fixtureNonce = pathParts[prefixIndex + 1];
const scenarioId = Number(pathParts[pathParts.length - 1]);
const fixtureBase = `${FIXTURE_PREFIX}${fixtureNonce}`;
const sameOriginObjectUrl = `${window.location.origin}${fixtureBase}/object/${scenarioId}`;
const crossOriginObjectUrl = document.body.dataset.crossOriginObjectUrl;
const MAX_EXPECTED_BYTES = 16 * 1024 * 1024;
const SAFE_COMMANDS = new Set([
  "fetchByob",
  "callArrayBuffer",
  "getInstrumentation",
  "registerServiceWorker",
  "unregisterServiceWorkers",
  "startHeartbeat",
  "stopHeartbeat",
  "injectUnsafeErrorForTest",
]);

let heartbeatTimer = undefined;
let heartbeatCount = 0;
let arrayBufferCallCount = 0;
let eventSinkFailureCount = 0;

class FixtureCommandError extends Error {
  constructor(code, stage) {
    super(`${code}@${stage}`);
    this.code = code;
    this.stage = stage;
  }
}

function commandFailure(error, command) {
  if (error instanceof FixtureCommandError) {
    return { code: error.code, command, stage: error.stage };
  }
  return { code: "browser_api_failed", command, stage: "execute" };
}

function invalidOptions() {
  return new FixtureCommandError("invalid_options", "validate");
}

async function postEvent(event) {
  const response = await fetch(`${fixtureBase}/control/scenarios/${scenarioId}/browser-events`, {
    method: "POST",
    cache: "no-store",
    credentials: "same-origin",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(event),
  });
  if (!response.ok) {
    throw new FixtureCommandError("event_sink_rejected", "event_sink");
  }
}

const originalArrayBuffer = Response.prototype.arrayBuffer;
Response.prototype.arrayBuffer = function (...args) {
  arrayBufferCallCount += 1;
  void postEvent({ type: "array_buffer_called" }).catch(() => {
    eventSinkFailureCount += 1;
  });
  return originalArrayBuffer.apply(this, args);
};

async function registerServiceWorker() {
  if (!("serviceWorker" in navigator)) {
    throw new Error("service workers are unavailable");
  }

  const registration = await navigator.serviceWorker.register(
    `${fixtureBase}/service-worker.js`,
    { scope: `${fixtureBase}/` },
  );
  await navigator.serviceWorker.ready;
  if (!navigator.serviceWorker.controller) {
    await new Promise((resolve) => {
      navigator.serviceWorker.addEventListener("controllerchange", resolve, { once: true });
    });
  }
  return registration;
}

async function unregisterServiceWorkers() {
  if (!("serviceWorker" in navigator)) {
    return 0;
  }
  const registrations = await navigator.serviceWorker.getRegistrations();
  let removed = 0;
  for (const registration of registrations) {
    if (registration.scope.includes(fixtureBase) && (await registration.unregister())) {
      removed += 1;
    }
  }
  return removed;
}

function startHeartbeat(periodMs = 1) {
  if (heartbeatTimer !== undefined) {
    throw new Error("heartbeat already running");
  }
  heartbeatCount = 0;
  heartbeatTimer = window.setInterval(() => {
    heartbeatCount += 1;
  }, periodMs);
}

async function stopHeartbeat() {
  if (heartbeatTimer !== undefined) {
    window.clearInterval(heartbeatTimer);
    heartbeatTimer = undefined;
  }
  await postEvent({ type: "heartbeat", count: heartbeatCount });
  return heartbeatCount;
}

async function fetchByob(options = {}) {
  const {
    crossOrigin = false,
    headers = {},
    scratchBytes = 16,
    expectedBytes,
    expectedRange,
    cancelAfterBytes,
    abortAfterBytes,
    redirectMode = "error",
    timeoutMs,
    requiredResponseHeaders = [],
  } = options;
  if (!Number.isSafeInteger(scratchBytes) || scratchBytes < 1 || scratchBytes > 65536) {
    throw invalidOptions();
  }
  if (redirectMode !== "error" && redirectMode !== "manual") {
    throw invalidOptions();
  }
  if (
    timeoutMs !== undefined &&
    (!Number.isSafeInteger(timeoutMs) || timeoutMs < 1 || timeoutMs > 10000)
  ) {
    throw invalidOptions();
  }
  if (
    !Array.isArray(requiredResponseHeaders) ||
    requiredResponseHeaders.some(
      (name) => name !== "content-range" && name !== "content-length" && name !== "etag",
    )
  ) {
    throw invalidOptions();
  }
  let rangeBytes;
  if (expectedRange !== undefined) {
    if (
      expectedRange === null ||
      !Number.isSafeInteger(expectedRange.start) ||
      !Number.isSafeInteger(expectedRange.end) ||
      expectedRange.start < 0 ||
      expectedRange.end < expectedRange.start
    ) {
      throw invalidOptions();
    }
    rangeBytes = expectedRange.end - expectedRange.start + 1;
    if (!Number.isSafeInteger(rangeBytes) || rangeBytes > MAX_EXPECTED_BYTES) {
      throw invalidOptions();
    }
  }
  if (
    expectedBytes !== undefined &&
    (!Number.isSafeInteger(expectedBytes) || expectedBytes < 1 || expectedBytes > MAX_EXPECTED_BYTES)
  ) {
    throw invalidOptions();
  }
  if (expectedBytes === undefined && rangeBytes === undefined) {
    throw invalidOptions();
  }
  if (expectedBytes !== undefined && rangeBytes !== undefined && expectedBytes !== rangeBytes) {
    throw invalidOptions();
  }
  const boundedExpectedBytes = expectedBytes ?? rangeBytes;

  const controller = new AbortController();
  let response;
  try {
    response = await fetch(crossOrigin ? crossOriginObjectUrl : sameOriginObjectUrl, {
      method: "GET",
      cache: "no-store",
      credentials: "omit",
      redirect: redirectMode,
      referrerPolicy: "no-referrer",
      headers,
      signal: controller.signal,
    });
  } catch (_error) {
    throw new FixtureCommandError("fetch_failed", "fetch");
  }
  if (!response.body) {
    throw new FixtureCommandError("body_unavailable", "response");
  }
  for (const name of requiredResponseHeaders) {
    if (response.headers.get(name) === null) {
      throw new FixtureCommandError("response_header_unavailable", "response_headers");
    }
  }
  if (expectedRange !== undefined) {
    const contentRange = response.headers.get("content-range");
    const match = contentRange?.match(/^bytes (\d+)-(\d+)\/(\d+|\*)$/);
    if (
      match === null ||
      match === undefined ||
      Number(match[1]) !== expectedRange.start ||
      Number(match[2]) !== expectedRange.end
    ) {
      throw new FixtureCommandError("unexpected_content_range", "response_headers");
    }
  }

  let reader;
  try {
    reader = response.body.getReader({ mode: "byob" });
  } catch (_error) {
    throw new FixtureCommandError("byob_unavailable", "reader");
  }
  let visibleBytes = 0;
  const chunks = [];
  let reads = 0;
  let eofChecks = 0;
  let rebuiltViews = 0;
  let previousInputBuffer = undefined;
  let stoppedEarly = false;

  async function readBounded(requestedBytes) {
    // Chrome can detach the supplied buffer after each BYOB read.
    // Allocate the next exact scratch view instead of reusing an old view.
    const inputBuffer = new ArrayBuffer(requestedBytes);
    const view = new Uint8Array(inputBuffer);
    const rebuiltView = inputBuffer !== previousInputBuffer;
    previousInputBuffer = inputBuffer;
    rebuiltViews += rebuiltView ? 1 : 0;
    reads += 1;
    let timeoutHandle;
    let readResult;
    try {
      readResult = await Promise.race([
        reader.read(view),
        ...(timeoutMs === undefined
          ? []
          : [
              new Promise((_, reject) => {
                timeoutHandle = window.setTimeout(() => {
                  controller.abort();
                  reject(new FixtureCommandError("read_timeout", "read"));
                }, timeoutMs);
              }),
            ]),
      ]);
    } catch (error) {
      if (controller.signal.aborted) {
        await postEvent({ type: "abort_controller", visible_bytes: visibleBytes });
      }
      if (error instanceof FixtureCommandError) {
        throw error;
      }
      throw new FixtureCommandError("read_failed", "read");
    } finally {
      if (timeoutHandle !== undefined) {
        window.clearTimeout(timeoutHandle);
      }
    }
    const { done, value } = readResult;
    await postEvent({
      type: "byob_read",
      requested_bytes: requestedBytes,
      returned_bytes: value?.byteLength ?? 0,
      done,
      rebuilt_view: rebuiltView,
    });
    return { done, value, rebuiltView };
  }

  async function cancelOverlongResponse() {
    try {
      await reader.cancel("fixture rejected overlong response");
    } catch (_error) {
      // The abort below is the final bounded cleanup authority.
    }
    controller.abort();
    await postEvent({ type: "abort_controller", visible_bytes: visibleBytes });
  }

  while (visibleBytes < boundedExpectedBytes) {
    const remainingBytes = boundedExpectedBytes - visibleBytes;
    const requestedBytes = Math.min(scratchBytes, remainingBytes);
    const { done, value } = await readBounded(requestedBytes);
    if (done) {
      throw new FixtureCommandError("unexpected_eof", "data_read");
    }
    const returnedBytes = value?.byteLength ?? 0;
    if (returnedBytes === 0) {
      throw new FixtureCommandError("zero_progress", "data_read");
    }
    if (returnedBytes > remainingBytes) {
      await cancelOverlongResponse();
      throw new FixtureCommandError("overlong_response", "data_read");
    }
    visibleBytes += returnedBytes;
    chunks.push(Array.from(value));
    await postEvent({ type: "application_bytes", count: returnedBytes, total: visibleBytes });

    if (cancelAfterBytes !== undefined && visibleBytes >= cancelAfterBytes) {
      await reader.cancel("fixture requested cancellation");
      await postEvent({ type: "reader_cancelled", visible_bytes: visibleBytes });
      stoppedEarly = true;
      break;
    }
    if (abortAfterBytes !== undefined && visibleBytes >= abortAfterBytes) {
      controller.abort();
      await postEvent({ type: "abort_controller", visible_bytes: visibleBytes });
      stoppedEarly = true;
      break;
    }

    // A macrotask yield makes heartbeat starvation observable even when a
    // service worker has all response bytes ready synchronously.
    await new Promise((resolve) => window.setTimeout(resolve, 0));
  }

  if (!stoppedEarly) {
    const eof = await readBounded(1);
    eofChecks = 1;
    if (!eof.done || (eof.value?.byteLength ?? 0) !== 0) {
      await cancelOverlongResponse();
      throw new FixtureCommandError("overlong_response", "eof_check");
    }
  }

  return {
    status: response.status,
    visibleBytes,
    chunks,
    reads,
    eofChecks,
    rebuiltViews,
    contentLength: response.headers.get("content-length"),
    contentRange: response.headers.get("content-range"),
    etag: response.headers.get("etag"),
  };
}

async function callArrayBuffer(options = {}) {
  const response = await fetch(
    options.crossOrigin ? crossOriginObjectUrl : sameOriginObjectUrl,
    {
      method: "GET",
      cache: "no-store",
      credentials: "omit",
      redirect: "error",
      referrerPolicy: "no-referrer",
      headers: options.headers ?? {},
    },
  );
  const bytes = await response.arrayBuffer();
  return { byteLength: bytes.byteLength, arrayBufferCallCount };
}

function getInstrumentation() {
  return { arrayBufferCallCount, heartbeatCount, eventSinkFailureCount };
}

window.mcapRangeFixture = Object.freeze({
  fetchByob,
  callArrayBuffer,
  getInstrumentation,
  registerServiceWorker,
  unregisterServiceWorkers,
  sameOriginObjectUrl,
  crossOriginObjectUrl,
  startHeartbeat,
  stopHeartbeat,
});

window.addEventListener("message", async (message) => {
  if (message.data?.fixture !== "mcap-range-v1" || message.data?.scenarioId !== scenarioId) {
    return;
  }

  try {
    const requestedCommand = message.data.command;
    const command = SAFE_COMMANDS.has(requestedCommand) ? requestedCommand : "unknown";
    let result;
    if (command === "fetchByob") {
      result = await fetchByob(message.data.options);
    } else if (command === "callArrayBuffer") {
      result = await callArrayBuffer(message.data.options);
    } else if (command === "getInstrumentation") {
      result = getInstrumentation();
    } else if (command === "registerServiceWorker") {
      await registerServiceWorker();
      result = { registered: true };
    } else if (command === "unregisterServiceWorkers") {
      result = { removed: await unregisterServiceWorkers() };
    } else if (command === "startHeartbeat") {
      startHeartbeat(message.data.periodMs);
      result = { started: true };
    } else if (command === "stopHeartbeat") {
      result = { count: await stopHeartbeat() };
    } else if (command === "injectUnsafeErrorForTest") {
      throw new DOMException(
        `unsafe ${fixtureNonce} ${crossOriginObjectUrl} authorization: Bearer fixture-secret`,
        "NetworkError",
      );
    } else {
      throw new FixtureCommandError("unknown_command", "dispatch");
    }
    message.source?.postMessage(
      { fixture: "mcap-range-v1", requestId: message.data.requestId, ok: true, result },
      message.origin,
    );
  } catch (error) {
    const requestedCommand = message.data.command;
    const command = SAFE_COMMANDS.has(requestedCommand) ? requestedCommand : "unknown";
    message.source?.postMessage(
      {
        fixture: "mcap-range-v1",
        requestId: message.data.requestId,
        ok: false,
        error: commandFailure(error, command),
      },
      message.origin,
    );
  }
});
