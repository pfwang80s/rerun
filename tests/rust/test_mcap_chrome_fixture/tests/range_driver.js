// Top-level (no-iframe) driver for the controlled MCAP Range fixture.
//
// The wasm test page runs on the wasm-bindgen origin and talks to the fixture server directly:
// bootstrap -> register scenario -> direct cross-origin BYOB fetch -> browser-events -> snapshot.
// This replaces the former controlled-iframe + postMessage command protocol, which never
// executed on any tested browser (local headless and CI both timed out before the iframe
// document was parsed).
//
// Failure contract: every driver failure is a `FixtureCommandError` exposing exactly
// `{code, command, stage}` through `fixtureFailure`, matching the former command protocol so
// the existing redaction assertions keep their meaning.

const FIXTURE_PROTOCOL = "mcap-range-fixture-v1";

/** Opaque, contract-shaped fixture failure. */
export class FixtureCommandError extends Error {
  constructor(code, stage, command) {
    super(`${command}:${code}@${stage}`);
    this.code = code;
    this.command = command;
    this.stage = stage;
    // Exactly `code,command,stage`: no unsafe input, no extra fields.
    this.fixtureFailure = { code, command, stage };
  }
}

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

async function responseJson(response, label) {
  if (!response.ok) {
    throw new Error(`${label} failed with status ${response.status}`);
  }
  return response.json();
}

/** Per-scenario top-level driver. */
class ScenarioSession {
  constructor(bootstrap, descriptor, maxPollAttempts) {
    this.bootstrap = bootstrap;
    this.descriptor = descriptor;
    this.controlBase = `${bootstrap.page_origin}${bootstrap.control_root}`;
    this.maxPollAttempts = maxPollAttempts;
    this.instrumentation = {
      arrayBufferCallCount: 0,
      heartbeatCount: 0,
      eventSinkFailureCount: 0,
    };
    this.heartbeatTimer = undefined;
    this.originalArrayBuffer = undefined;
    this.installArrayBufferProbe();
  }

  /** Counts `Response.arrayBuffer` use and reports it, mirroring the controlled-page probe. */
  installArrayBufferProbe() {
    const original = Response.prototype.arrayBuffer;
    this.originalArrayBuffer = original;
    const session = this;
    Response.prototype.arrayBuffer = function (...args) {
      session.instrumentation.arrayBufferCallCount += 1;
      void session.postEvent({ type: "array_buffer_called" }).catch(() => {
        session.instrumentation.eventSinkFailureCount += 1;
      });
      return original.apply(this, args);
    };
  }

  restoreArrayBufferProbe() {
    if (this.originalArrayBuffer !== undefined) {
      Response.prototype.arrayBuffer = this.originalArrayBuffer;
      this.originalArrayBuffer = undefined;
    }
  }

  async postEvent(event) {
    const response = await fetch(
      `${this.controlBase}/control/scenarios/${this.descriptor.id}/browser-events`,
      {
        method: "POST",
        cache: "no-store",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(event),
      },
    );
    if (!response.ok) {
      throw new FixtureCommandError("event_sink_rejected", "event_sink", "postEvent");
    }
  }

  async snapshot() {
    return responseJson(
      await fetch(`${this.controlBase}/control/scenarios/${this.descriptor.id}`, {
        cache: "no-store",
      }),
      "event snapshot",
    );
  }

  async waitFor(predicate, label) {
    for (let attempt = 0; attempt < this.maxPollAttempts; attempt += 1) {
      const snapshot = await this.snapshot();
      if (predicate(snapshot)) {
        return snapshot;
      }
      await new Promise((resolve) => window.setTimeout(resolve, 25));
    }
    throw new Error(`timed out waiting for ${label}`);
  }

  objectUrl(crossOrigin) {
    return crossOrigin
      ? this.descriptor.cross_origin_object_url
      : this.descriptor.same_origin_object_url;
  }

  startHeartbeat(periodMs = 1) {
    if (this.heartbeatTimer !== undefined) {
      throw new Error("heartbeat already running");
    }
    this.instrumentation.heartbeatCount = 0;
    this.heartbeatTimer = window.setInterval(() => {
      this.instrumentation.heartbeatCount += 1;
    }, periodMs);
  }

  async stopHeartbeat() {
    if (this.heartbeatTimer !== undefined) {
      window.clearInterval(this.heartbeatTimer);
      this.heartbeatTimer = undefined;
    }
    await this.postEvent({
      type: "heartbeat",
      count: this.instrumentation.heartbeatCount,
    });
    return { count: this.instrumentation.heartbeatCount };
  }

  getInstrumentation() {
    return {
      arrayBufferCallCount: this.instrumentation.arrayBufferCallCount,
      heartbeatCount: this.instrumentation.heartbeatCount,
      eventSinkFailureCount: this.instrumentation.eventSinkFailureCount,
    };
  }

  /**
   * Reads the object with a plain (non-BYOB) `Response.arrayBuffer`.
   *
   * Mirrors the former controlled-page `callArrayBuffer` probe.
   */
  async callArrayBuffer(options = {}) {
    const { crossOrigin = false, headers = {} } = options;
    const response = await fetch(this.objectUrl(crossOrigin), {
      method: "GET",
      cache: "no-store",
      credentials: "omit",
      redirect: "error",
      referrerPolicy: "no-referrer",
      headers,
    });
    const bytes = await response.arrayBuffer();
    return {
      byteLength: bytes.byteLength,
      arrayBufferCallCount: this.instrumentation.arrayBufferCallCount,
    };
  }

  /**
   * Throws an unsafe, secret-bearing browser error and normalizes it to the opaque contract.
   *
   * Mirrors the former controlled-page `injectUnsafeErrorForTest`: the thrown `DOMException`
   * carries the fixture nonce, the cross-origin object URL and a bearer secret, and the failure
   * surfaced to the caller retains none of it.
   */
  injectUnsafeErrorForTest() {
    try {
      const nonce = new URL(this.descriptor.page_url).pathname.split("/")[3];
      throw new DOMException(
        `unsafe ${nonce} ${this.descriptor.cross_origin_object_url} authorization: Bearer fixture-secret`,
        "NetworkError",
      );
    } catch {
      throw new FixtureCommandError(
        "browser_api_failed",
        "execute",
        "injectUnsafeErrorForTest",
      );
    }
  }

  /**
   * Reads the object with a BYOB reader at the top level.
   *
   * Mirrors the former controlled-page `fetchByob`: exactly one EOF probe after the expected
   * bytes, fresh scratch views per read, error classification by stage, and bounded cleanup.
   */
  async fetchByob(options = {}) {
    const {
      crossOrigin = false,
      headers = {},
      scratchBytes = 16,
      expectedBytes,
      expectedRange,
      cancelAfterBytes,
      abortAfterBytes,
      requiredResponseHeaders = [],
    } = options;

    if (!Number.isSafeInteger(scratchBytes) || scratchBytes < 1 || scratchBytes > 65536) {
      throw new FixtureCommandError("invalid_options", "validate", "fetchByob");
    }
    let boundBytes;
    if (expectedRange !== undefined) {
      if (
        expectedRange === null ||
        !Number.isSafeInteger(expectedRange.start) ||
        !Number.isSafeInteger(expectedRange.end) ||
        expectedRange.start < 0 ||
        expectedRange.end < expectedRange.start
      ) {
        throw new FixtureCommandError("invalid_options", "validate", "fetchByob");
      }
      boundBytes = expectedRange.end - expectedRange.start + 1;
    } else if (expectedBytes !== undefined) {
      if (!Number.isSafeInteger(expectedBytes) || expectedBytes < 1) {
        throw new FixtureCommandError("invalid_options", "validate", "fetchByob");
      }
      boundBytes = expectedBytes;
    } else {
      throw new FixtureCommandError("invalid_options", "validate", "fetchByob");
    }

    const controller = new AbortController();
    let response;
    try {
      response = await fetch(this.objectUrl(crossOrigin), {
        method: "GET",
        cache: "no-store",
        credentials: "omit",
        redirect: "error",
        referrerPolicy: "no-referrer",
        headers,
        signal: controller.signal,
      });
    } catch {
      throw new FixtureCommandError("fetch_failed", "fetch", "fetchByob");
    }
    if (!response.body) {
      throw new FixtureCommandError("body_unavailable", "response", "fetchByob");
    }
    for (const name of requiredResponseHeaders) {
      if (response.headers.get(name) === null) {
        throw new FixtureCommandError(
          "response_header_unavailable",
          "response_headers",
          "fetchByob",
        );
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
        throw new FixtureCommandError(
          "unexpected_content_range",
          "response_headers",
          "fetchByob",
        );
      }
    }

    const reader = response.body.getReader({ mode: "byob" });
    let visibleBytes = 0;
    const chunks = [];
    let reads = 0;
    let eofChecks = 0;
    let rebuiltViews = 0;
    let previousInputBuffer = undefined;
    let stoppedEarly = false;

    const readBounded = async (requestedBytes) => {
      const inputBuffer = new ArrayBuffer(requestedBytes);
      const view = new Uint8Array(inputBuffer);
      const rebuiltView = inputBuffer !== previousInputBuffer;
      previousInputBuffer = inputBuffer;
      rebuiltViews += rebuiltView ? 1 : 0;
      reads += 1;
      const { done, value } = await reader.read(view);
      await this.postEvent({
        type: "byob_read",
        requested_bytes: requestedBytes,
        returned_bytes: value?.byteLength ?? 0,
        done,
        rebuilt_view: rebuiltView,
      });
      return { done, value, rebuiltView };
    };

    while (visibleBytes < boundBytes) {
      const remaining = boundBytes - visibleBytes;
      const requestedBytes = Math.min(scratchBytes, remaining);
      const { done, value } = await readBounded(requestedBytes);
      if (done) {
        throw new FixtureCommandError("unexpected_eof", "data_read", "fetchByob");
      }
      const returnedBytes = value?.byteLength ?? 0;
      if (returnedBytes === 0) {
        throw new FixtureCommandError("zero_progress", "data_read", "fetchByob");
      }
      if (returnedBytes > remaining) {
        await reader.cancel("fixture rejected overlong response").catch(() => {});
        controller.abort();
        await this.postEvent({ type: "abort_controller", visible_bytes: visibleBytes });
        throw new FixtureCommandError("overlong_response", "data_read", "fetchByob");
      }
      visibleBytes += returnedBytes;
      chunks.push(Array.from(value));
      await this.postEvent({
        type: "application_bytes",
        count: returnedBytes,
        total: visibleBytes,
      });

      if (cancelAfterBytes !== undefined && visibleBytes >= cancelAfterBytes) {
        await reader.cancel("fixture requested cancellation");
        await this.postEvent({ type: "reader_cancelled", visible_bytes: visibleBytes });
        stoppedEarly = true;
        break;
      }
      if (abortAfterBytes !== undefined && visibleBytes >= abortAfterBytes) {
        controller.abort();
        await this.postEvent({ type: "abort_controller", visible_bytes: visibleBytes });
        stoppedEarly = true;
        break;
      }
      await new Promise((resolve) => window.setTimeout(resolve, 0));
    }

    if (!stoppedEarly) {
      const eof = await readBounded(1);
      eofChecks = 1;
      if (!eof.done || (eof.value?.byteLength ?? 0) !== 0) {
        await reader.cancel("fixture rejected overlong response").catch(() => {});
        controller.abort();
        await this.postEvent({ type: "abort_controller", visible_bytes: visibleBytes });
        throw new FixtureCommandError("overlong_response", "eof_check", "fetchByob");
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
}

/**
 * Registers one scenario, runs `test`, then always removes the scenario.
 *
 * `attempts` preserves each suite's original polling bound (80 for smoke, 120 for correctness).
 */
export async function withScenario(bootstrap, spec, test, { attempts = 80 } = {}) {
  assert(bootstrap.protocol === FIXTURE_PROTOCOL, "fixture protocol version mismatch");
  const controlBase = `${bootstrap.page_origin}${bootstrap.control_root}`;
  const descriptor = await responseJson(
    await fetch(`${controlBase}/control/scenarios`, {
      method: "POST",
      cache: "no-store",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(spec),
    }),
    "scenario registration",
  );
  const session = new ScenarioSession(bootstrap, descriptor, attempts);
  try {
    return await test(session);
  } finally {
    session.restoreArrayBufferProbe();
    await fetch(`${controlBase}/control/scenarios/${descriptor.id}`, {
      method: "DELETE",
      cache: "no-store",
    }).catch(() => {});
  }
}

/**
 * Proves that a document whose CSP forbids the object origin cannot reach it.
 *
 * A same-origin `srcdoc` iframe (script execution verified on this runner, unlike navigated
 * cross-origin iframes) carries a `<meta http-equiv="Content-Security-Policy">` with
 * `connect-src 'none'` and attempts the cross-origin fetch from inside that document. The
 * server must see no request, and the failure is surfaced with the same contract shape as a
 * rejected `fetchByob`.
 */
export async function assertSrcdocCspBlocksObjectOrigin(session, { headers = {} } = {}) {
  const objectUrl = session.descriptor.cross_origin_object_url;
  const srcdoc = [
    "<!doctype html><meta http-equiv=\"Content-Security-Policy\"",
    " content=\"script-src 'unsafe-inline'; connect-src 'none'\">",
    "<script>",
    `fetch(${JSON.stringify(objectUrl)}, { cache: "no-store", headers: ${JSON.stringify(headers)} })`,
    '  .then(() => window.parent.postMessage({ cspProbe: "allowed" }, "*"))',
    '  .catch(() => window.parent.postMessage({ cspProbe: "blocked" }, "*"));',
    "</scr" + "ipt>",
  ].join("");

  const iframe = document.createElement("iframe");
  iframe.hidden = true;
  iframe.srcdoc = srcdoc;
  const outcome = await new Promise((resolve) => {
    const timer = window.setTimeout(() => resolve("timeout"), 5_000);
    const listener = (message) => {
      if (message.source !== iframe.contentWindow || message.data?.cspProbe === undefined) {
        return;
      }
      finish(message.data.cspProbe);
    };
    const finish = (value) => {
      window.clearTimeout(timer);
      window.removeEventListener("message", listener);
      resolve(value);
    };
    window.addEventListener("message", listener);
    document.body.append(iframe);
  });
  iframe.remove();
  if (outcome !== "blocked") {
    throw new Error(`srcdoc CSP probe did not block the object origin: ${outcome}`);
  }
  // The synthesized code/stage keeps the pre-migration error contract shape; the real proof of
  // the CSP block is the `outcome === "blocked"` check above.
  throw new FixtureCommandError("fetch_failed", "fetch", "fetchByob");
}

export function baseScenario(overrides = {}) {
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
      allow_origin: "request_origin",
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

export function browserEvent(snapshot, type) {
  return snapshot.events.some(
    (event) => event.type === "browser" && event.event.type === type,
  );
}

export function fixtureEvent(snapshot, type) {
  return snapshot.events.some((event) => event.type === type);
}

export function requestEvents(snapshot, method = undefined) {
  return snapshot.events.filter(
    (event) => event.type === "request" && (method === undefined || event.method === method),
  );
}

export function assertNoRangeLessGet(snapshot, label) {
  const getRequests = requestEvents(snapshot, "GET");
  assert(getRequests.length > 0, `${label} produced no GET request`);
  for (const request of getRequests) {
    assert(
      request.range?.state === "ascii" && request.range.value.startsWith("bytes="),
      `${label} observed a Range-less GET`,
    );
  }
}

export function assertRedactedFixtureFailure(error, label) {
  const failure = error.fixtureFailure;
  assert(failure !== undefined, `${label} was not a structured fixture failure`);
  assert(
    Object.keys(failure).sort().join(",") === "code,command,stage",
    `${label} exposed non-contract failure fields`,
  );
}

export async function expectReject(promise, label) {
  try {
    await promise;
  } catch (error) {
    return error;
  }
  throw new Error(`${label} unexpectedly succeeded`);
}

export function fixturePort() {
  const rawPort = new URLSearchParams(window.location.search).get("mcap_fixture_page_port");
  assert(rawPort !== null && /^\d+$/.test(rawPort), "missing MCAP fixture page port");
  const port = Number(rawPort);
  assert(Number.isSafeInteger(port) && port > 0 && port <= 65_535, "invalid fixture port");
  return port;
}

export async function fetchBootstrap() {
  const bootstrap = await responseJson(
    await fetch(`http://127.0.0.1:${fixturePort()}/__mcap_range_fixture/v1/bootstrap`, {
      cache: "no-store",
    }),
    "fixture bootstrap",
  );
  assert(bootstrap.protocol === FIXTURE_PROTOCOL, "fixture protocol version mismatch");
  return bootstrap;
}
