import assert from "node:assert/strict";
import { after, beforeEach, test } from "node:test";

const original = {
  compileStreaming: WebAssembly.compileStreaming,
  fetch: globalThis.fetch,
  requestAnimationFrame: globalThis.requestAnimationFrame,
  setTimeout: globalThis.setTimeout,
};

class FakeClassList {
  add() {}
  remove() {}
}

class FakeElement {
  constructor(tagName) {
    this.tagName = tagName;
    this.style = {};
    this.classList = new FakeClassList();
    this.children = [];
    this.parentElement = null;
    this.textContent = "";
  }

  append(child) {
    child.parentElement = this;
    this.children.push(child);
  }

  appendChild(child) {
    this.append(child);
  }

  remove() {
    if (this.parentElement) {
      this.parentElement.children = this.parentElement.children.filter(
        (child) => child !== this,
      );
      this.parentElement = null;
    }
  }

  removeAttribute() {}
  addEventListener() {}

  querySelector() {
    return new FakeElement("query-result");
  }

  getBoundingClientRect() {
    return { left: 0, top: 0, width: 640, height: 360 };
  }
}

function makeDocument() {
  const body = new FakeElement("body");
  const head = new FakeElement("head");

  return {
    body,
    head,
    documentElement: new FakeElement("html"),
    createElement: (tagName) => new FakeElement(tagName),
    createTextNode: (text) => ({ text }),
    getElementById: (id) =>
      head.children.find((child) => child.id === id) ?? null,
  };
}

function resetState() {
  const state = globalThis.__rerun_web_viewer_test_state ?? {};
  for (const key of Object.keys(state)) {
    delete state[key];
  }
  Object.assign(state, {
    calls: [],
    handles: [],
    addReceiverCalls: 0,
    addReceiverErrorAt: -1,
    startError: null,
    activeRecordingId: null,
    activeTimeline: null,
    currentTime: null,
    playing: null,
    timelineTimeRange: null,
  });
  globalThis.__rerun_web_viewer_test_state = state;
}

globalThis.document = makeDocument();
globalThis.window = {
  addEventListener() {},
  location: {
    href: "https://viewer.example.test/",
    reload() {},
  },
};
globalThis.requestAnimationFrame = (callback) => callback(0);
globalThis.fetch = async () => new Response(new Uint8Array());
WebAssembly.compileStreaming = async () => ({ fake: true });
globalThis.setTimeout = (callback, delay, ...args) => {
  if (delay >= 1000) {
    return 0;
  }
  return original.setTimeout(callback, 0, ...args);
};

resetState();
const { LogChannel, StrictOpenError, WebViewer } = await import("../index.js");

beforeEach(() => {
  resetState();
  globalThis.document = makeDocument();
});

after(() => {
  WebAssembly.compileStreaming = original.compileStreaming;
  globalThis.fetch = original.fetch;
  globalThis.requestAnimationFrame = original.requestAnimationFrame;
  globalThis.setTimeout = original.setTimeout;
});

function callsNamed(name) {
  return globalThis.__rerun_web_viewer_test_state.calls.filter(
    (call) => call[0] === name,
  );
}

async function startViewer(rrd = null, options = null) {
  const viewer = new WebViewer();
  await viewer.start(rrd, document.body, options);
  assert.equal(viewer.ready, true);
  return viewer;
}

test("start and open preserve per-item order across existing route families", async () => {
  const startupUrls = [
    "https://example.test/first.rrd",
    "rerun+http://127.0.0.1:9876/proxy",
    "rerun://127.0.0.1:1234/dataset/1830B33B45B963E7774455beb91701ae?segment_id=pid",
  ];
  const viewer = await startViewer(startupUrls);

  assert.deepEqual(
    callsNamed("add_receiver").map((call) => call[1]),
    startupUrls,
  );

  viewer.open("https://example.test/later.rrd");
  assert.deepEqual(
    callsNamed("add_receiver").map((call) => call[1]),
    [...startupUrls, "https://example.test/later.rrd"],
  );
  viewer.stop();
});

test("hidden startup URL remains a Wasm option and direct startup URLs open afterwards", async () => {
  const hiddenUrls = [
    "https://example.test/hidden-a.rrd",
    "rerun+http://127.0.0.1:9876/proxy",
  ];
  const viewer = await startViewer("https://example.test/direct.rrd", {
    url: hiddenUrls,
  });

  const construct = callsNamed("construct")[0];
  assert.deepEqual(construct[1].url, hiddenUrls);
  assert.deepEqual(
    callsNamed("add_receiver").map((call) => call[1]),
    ["https://example.test/direct.rrd"],
  );
  viewer.stop();
});

test("malformed array items do not stop compatibility dispatch", async () => {
  const viewer = await startViewer();
  const urls = [
    "https://example.test/accepted.rrd",
    "not a URL",
    "rerun+http://127.0.0.1:9876/proxy",
  ];

  assert.doesNotThrow(() => viewer.open(urls));
  assert.deepEqual(
    callsNamed("add_receiver").map((call) => call[1]),
    urls,
  );
  assert.equal(viewer.ready, true);
  viewer.stop();
});

test("an exception in the second array item warns and continues without stopping the wrapper", async () => {
  const viewer = await startViewer();
  const state = globalThis.__rerun_web_viewer_test_state;
  state.addReceiverErrorAt = 1;

  const originalConsoleWarn = console.warn;
  const warnings = [];
  console.warn = (...args) => warnings.push(args);
  try {
    assert.doesNotThrow(() =>
      viewer.open([
        "https://example.test/accepted.rrd",
        "https://example.test/fails.rrd",
        "https://example.test/not-attempted.rrd",
      ]));
  } finally {
    console.warn = originalConsoleWarn;
  }
  assert.deepEqual(
    callsNamed("add_receiver").map((call) => call[1]),
    [
      "https://example.test/accepted.rrd",
      "https://example.test/fails.rrd",
      "https://example.test/not-attempted.rrd",
    ],
  );
  assert.equal(viewer.ready, true);
  assert.equal(callsNamed("destroy").length, 0);
  assert.equal(callsNamed("free").length, 0);
  assert.equal(warnings.length, 1);
  assert.match(String(warnings[0][0]), /continuing with the next item/);
  viewer.stop();
});

test("compatibility explicit mcap and extensionless inputs remain ordered dispatcher items", async () => {
  const viewer = await startViewer();
  const urls = [
    "https://example.test/first.mcap?token=opaque",
    "https://example.test/no-extension?token=opaque",
    "https://example.test/last.rrd",
  ];

  assert.doesNotThrow(() => viewer.open(urls));
  assert.deepEqual(
    callsNamed("add_receiver").map((call) => call[1]),
    urls,
  );
  assert.equal(viewer.ready, true);
  viewer.stop();
});

test("recording_open follows completion order and raw delivery is synchronous", async () => {
  const viewer = await startViewer();
  const delivery = [];
  viewer._on_raw_event((event) => delivery.push(["raw", JSON.parse(event).source]));
  viewer.on("recording_open", (event) => delivery.push(["parsed", event.source]));

  const handle = globalThis.__rerun_web_viewer_test_state.handles[0];
  handle.emit({
    type: "recording_open",
    application_id: "app",
    recording_id: "second",
    segment_id: null,
    source: "second-completed",
  });
  delivery.push(["after-emit", "second-completed"]);
  handle.emit({
    type: "recording_open",
    application_id: "app",
    recording_id: "first",
    segment_id: null,
    source: "first-completed",
  });

  assert.deepEqual(delivery, [
    ["raw", "second-completed"],
    ["after-emit", "second-completed"],
    ["raw", "first-completed"],
  ]);
  await new Promise((resolve) => original.setTimeout(resolve, 0));
  assert.deepEqual(delivery.slice(3), [
    ["parsed", "second-completed"],
    ["parsed", "first-completed"],
  ]);
  viewer.stop();
});

test("one source may publish multiple recording_open events", async () => {
  const viewer = await startViewer();
  const opened = [];
  viewer.on("recording_open", (event) => opened.push(event));

  const handle = globalThis.__rerun_web_viewer_test_state.handles[0];
  handle.emit({
    type: "recording_open",
    application_id: "app",
    recording_id: "store-a",
    segment_id: null,
    source: "same-source",
    version: "1.0.0",
  });
  handle.emit({
    type: "recording_open",
    application_id: "app",
    recording_id: "store-b",
    segment_id: null,
    source: "same-source",
    version: "2.0.0",
  });
  await new Promise((resolve) => original.setTimeout(resolve, 0));

  assert.deepEqual(opened, [
    {
      type: "recording_open",
      application_id: "app",
      recording_id: "store-a",
      segment_id: null,
      source: "same-source",
      version: "1.0.0",
    },
    {
      type: "recording_open",
      application_id: "app",
      recording_id: "store-b",
      segment_id: null,
      source: "same-source",
      version: "2.0.0",
    },
  ]);
  viewer.stop();
});

test("close removes a source without stopping the viewer", async () => {
  const viewer = await startViewer();
  viewer.close([
    "https://example.test/source.rrd",
    "rerun+http://127.0.0.1:9876/proxy",
  ]);

  assert.deepEqual(
    callsNamed("remove_receiver").map((call) => call[1]),
    [
      "https://example.test/source.rrd",
      "rerun+http://127.0.0.1:9876/proxy",
    ],
  );
  assert.equal(viewer.ready, true);
  viewer.stop();
});

test("LogChannel is synchronous while ready and becomes inert after close or stop", async () => {
  const viewer = await startViewer();
  const channel = viewer.open_channel("characterization");
  const rrd = new Uint8Array([1, 2, 3]);
  const table = new Uint8Array([4, 5]);

  assert.equal(channel.ready, true);
  channel.send_rrd(rrd);
  channel.send_table(table);
  channel.close();
  channel.send_rrd(new Uint8Array([6]));

  assert.equal(callsNamed("open_channel").length, 1);
  assert.strictEqual(callsNamed("send_rrd_to_channel")[0][2], rrd);
  assert.strictEqual(callsNamed("send_table_to_channel")[0][2], table);
  assert.equal(callsNamed("close_channel").length, 1);
  assert.equal(callsNamed("send_rrd_to_channel").length, 1);
  assert.equal(channel.ready, false);

  const stoppedChannel = viewer.open_channel("stopped");
  viewer.stop();
  assert.equal(stoppedChannel.ready, false);
  stoppedChannel.send_rrd(rrd);
  stoppedChannel.close();
  assert.equal(callsNamed("send_rrd_to_channel").length, 1);
  assert.equal(callsNamed("close_channel").length, 1);
});

test("strict valid singleton and batch reject before every public effect", async () => {
  const viewer = await startViewer();

  for (const call of [
    () => viewer.openRequest({ url: "https://example.test/first.mcap?token=secret" }),
    () => viewer.openBatch([
      { url: "https://example.test/second.mcap" },
      { url: "https://example.test/third.mcap" },
    ]),
  ]) {
    await assert.rejects(call(), (error) => {
      assert.ok(error instanceof StrictOpenError);
      assert.equal(error.code, "CapabilityUnavailable");
      assert.equal(error.index, null);
      assert.doesNotMatch(String(error), /example|token|secret/);
      return true;
    });
  }
  assert.equal(viewer._strict_open_cache.operation_count, 0);
  assert.equal(viewer._strict_dispatcher.queued_count, 0);
  assert.equal(callsNamed("add_receiver").length, 0);
  assert.equal(viewer.ready, true);
  viewer.stop();
});

test("strict route matrix rejects non-HTTP and requires opt-in extensionless sniff", async () => {
  const viewer = await startViewer();

  for (const [spec, code] of [
    [{ url: "rerun+http://127.0.0.1:9876/proxy" }, "UnsupportedStrictOpenRoute"],
    [{ url: "rerun://host/dataset/opaque" }, "UnsupportedStrictOpenRoute"],
    [{ url: "https://example.test/no-extension" }, "UnsupportedFormat"],
  ]) {
    await assert.rejects(viewer.openRequest(spec), (error) => {
      assert.ok(error instanceof StrictOpenError);
      assert.equal(error.code, code);
      assert.equal(error.index, 0);
      return true;
    });
  }

  assert.equal(viewer._strict_open_cache.operation_count, 0);
  assert.equal(callsNamed("add_receiver").length, 0);
  viewer.stop();
});

test("strict request shape errors are request-local and never stop the Viewer", async () => {
  const stopped = new WebViewer();
  await assert.rejects(stopped.openRequest({ url: "https://example.test/secret.mcap" }), (error) => {
    assert.ok(error instanceof StrictOpenError);
    assert.equal(error.code, "ViewerStopped");
    assert.doesNotMatch(String(error), /secret|example\.test/);
    return true;
  });

  const viewer = await startViewer();
  for (const invalid of [
    null,
    [],
    [{ url: "https://example.test/shape.mcap", options: { unknown: true } }],
  ]) {
    await assert.rejects(viewer.openBatch(invalid), (error) => {
      assert.ok(error instanceof StrictOpenError);
      assert.equal(error.code, "InvalidRequestShape");
      return true;
    });
  }
  assert.equal(viewer.ready, true);
  assert.equal(viewer._strict_open_cache.operation_count, 0);
  viewer.stop();
});

test("strict request preflight seals fields, indexes sparse batches, and rejects sequence time", async () => {
  const viewer = await startViewer();
  await assert.rejects(viewer.openRequest({
    url: "https://example.test/sequence.mcap",
    options: { mcap_time_type: "sequence" },
  }), (error) => {
    assert.ok(error instanceof StrictOpenError);
    assert.equal(error.code, "InvalidRequestShape");
    assert.equal(error.index, 0);
    return true;
  });

  await assert.rejects(viewer.openRequest({
    url: "https://example.test/topics.mcap",
    options: { topic_filter: Array.from({ length: 17 }, () => "x".repeat(4_096)) },
  }), (error) => {
    assert.ok(error instanceof StrictOpenError);
    assert.equal(error.code, "ResourceLimitExceeded");
    assert.equal(error.index, 0);
    return true;
  });

  await assert.rejects(viewer.openRequest({
    url: "https://example.test/unknown.mcap",
    extra: "field",
  }), (error) => {
    assert.ok(error instanceof StrictOpenError);
    assert.equal(error.code, "InvalidRequestShape");
    assert.equal(error.index, 0);
    return true;
  });

  const sparse = [];
  sparse.length = 2;
  sparse[1] = { url: "https://example.test/second.mcap" };
  await assert.rejects(viewer.openBatch(sparse), (error) => {
    assert.ok(error instanceof StrictOpenError);
    assert.equal(error.code, "InvalidRequestShape");
    assert.equal(error.index, 0);
    return true;
  });

  await assert.rejects(viewer.openBatch([new Proxy({ url: "https://example.test/proxy.mcap" }, {
    getOwnPropertyDescriptor() {
      throw new Error("untrusted getter");
    },
  })]), (error) => {
    assert.ok(error instanceof StrictOpenError);
    assert.equal(error.code, "InvalidRequestShape");
    assert.equal(error.index, 0);
    return true;
  });
  assert.equal(viewer._strict_open_cache.operation_count, 0);
  assert.equal(viewer.ready, true);
  viewer.stop();
});

test("strict handle classes and deferred startup API are not public before capability install", async () => {
  const module = await import("../index.js");
  assert.equal("OpenRequestHandle" in module, false);
  assert.equal("RecordingHandle" in module, false);
  const viewer = await startViewer();
  assert.equal("startWithRequests" in viewer, false);
  viewer.stop();
});

test("strict preflight does not fold raw query or duplicate semantic inputs", async () => {
  const viewer = await startViewer();
  await assert.rejects(viewer.openBatch([
    { url: "https://example.test/a.mcap" },
    { url: "https://example.test/a.mcap?" },
  ]), (error) => {
    assert.ok(error instanceof StrictOpenError);
    assert.equal(error.code, "CapabilityUnavailable");
    return true;
  });
  assert.equal(viewer._strict_open_cache.operation_count, 0);
  viewer.stop();
});

test("strict wrapper cache reuses recording wrappers and cached snapshots", () => {
  const viewer = new WebViewer();
  const cache = viewer._strict_open_cache;

  assert.equal(cache.operation_count, 0);

  const operation = cache.install_operation("operation-1");
  assert.equal(cache.operation_count, 1);
  assert.equal(operation.recording_count, 0);
  assert.equal(operation.wrapper_count, 0);

  const first = operation.attach_preexisting_recording("recording-a");
  const second = operation.attach_preexisting_recording("recording-b");
  const duplicate = operation.attach_preexisting_recording("recording-a");

  assert.strictEqual(first, duplicate);
  assert.strictEqual(operation.recordings, operation.recordings);
  assert.strictEqual(operation.recordings[0], first);
  assert.strictEqual(operation.recordings[1], second);
  assert.equal(operation.recording_count, 2);
  assert.equal(operation.wrapper_count, 2);
  assert.equal(first.phase, "preexisting");

  operation.replay_active_recording("recording-a");
  operation.replay_completed_recording("recording-b");
  assert.strictEqual(operation.recordings[0], first);
  assert.strictEqual(operation.recordings[1], second);
  assert.equal(first.phase, "active");
  assert.equal(second.phase, "completed");
  assert.equal(operation.recording_count, 2);

  operation.complete_installation_ack();
  operation.arm_internal_activation_bridge();
  assert.equal(operation.installation_ack_complete, true);
  assert.equal(operation.activation_bridge_armed, true);
});

test("strict wrapper cache aborts before disposing temporary operations", () => {
  const viewer = new WebViewer();
  const cache = viewer._strict_open_cache;
  const order = [];

  assert.throws(
    () =>
      cache.install_operation_with_abort(
        "operation-throw",
        (operation) => {
          order.push(["build", operation.recording_count]);
          operation.attach_preexisting_recording("recording-a");
          throw new Error("constructor failed");
        },
        () => order.push(["abort"]),
      ),
    /constructor failed/,
  );

  order.push(["post"]);
  assert.deepEqual(order, [
    ["build", 0],
    ["abort"],
    ["post"],
  ]);
  assert.equal(cache.get_operation("operation-throw"), null);
  assert.equal(cache.operation_count, 0);
});

test("strict exact controls validate sealed targets and keep close separate from dispose", () => {
  const viewer = new WebViewer();
  const operation = viewer._strict_open_cache.install_operation("operation-exact");
  const recording = operation.attach_preexisting_recording("same-recording-id");

  assert.equal("operation_id" in operation, false);
  assert.equal("recording_id" in recording, false);
  assert.deepEqual(recording.select(), {
    command: "select",
    code: "capability_unavailable",
    accepted: false,
  });
  assert.deepEqual(recording.seek({ time_type: "timestamp_ns", value: "42" }), {
    command: "seek",
    code: "capability_unavailable",
    accepted: false,
  });
  assert.deepEqual(recording.seek({ time_type: "duration_ns", value: "-42" }), {
    command: "seek",
    code: "capability_unavailable",
    accepted: false,
  });
  assert.deepEqual(recording.play("playing"), {
    command: "play",
    code: "capability_unavailable",
    accepted: false,
  });
  assert.deepEqual(recording.seek({ time_type: "sequence", value: "42" }), {
    command: "seek",
    code: "invalid_request",
    accepted: false,
  });
  assert.deepEqual(recording.seek({ time_type: "timestamp_ns", value: "01" }), {
    command: "seek",
    code: "invalid_request",
    accepted: false,
  });
  assert.deepEqual(recording.seek({ time_type: "timestamp_ns", value: "-0" }), {
    command: "seek",
    code: "invalid_request",
    accepted: false,
  });
  assert.deepEqual(recording.seek({ time_type: "timestamp_ns", value: "-9223372036854775808" }), {
    command: "seek",
    code: "invalid_request",
    accepted: false,
  });
  assert.deepEqual(recording.play("following"), {
    command: "play",
    code: "invalid_request",
    accepted: false,
  });
  assert.deepEqual(recording.close(), {
    command: "close",
    code: "capability_unavailable",
    accepted: false,
  });
  assert.deepEqual(operation.close(), {
    command: "close",
    code: "capability_unavailable",
    accepted: false,
  });

  // Dispose only drops the exact wrapper/subscription; it does not turn into a close.
  recording.dispose();
  assert.equal(recording.disposed, true);
  assert.deepEqual(recording.select(), {
    command: "select",
    code: "disposed",
    accepted: false,
  });
  assert.deepEqual(recording.close(), {
    command: "close",
    code: "disposed",
    accepted: false,
  });
  assert.equal(operation.recording_count, 1);
  operation.dispose();
  assert.equal(operation.disposed, true);
  assert.equal(operation.recording_count, 0);
});

test("strict exact handles become terminal when the viewer stops", async () => {
  const viewer = await startViewer();
  const operation = viewer._strict_open_cache.install_operation("operation-stopped");
  const recording = operation.attach_preexisting_recording("recording-stopped");
  viewer.stop();

  assert.deepEqual(recording.select(), {
    command: "select",
    code: "viewer_stopped",
    accepted: false,
  });
  assert.deepEqual(operation.close(), {
    command: "close",
    code: "viewer_stopped",
    accepted: false,
  });
  operation.dispose();
});

test("strict dispatcher batches into one task, preserves FIFO, and cancels cleanly", () => {
  const viewer = new WebViewer();
  const dispatcher = viewer._strict_dispatcher;
  const seen = [];

  assert.equal(dispatcher.enqueue(() => seen.push("a")), true);
  assert.equal(dispatcher.enqueue(() => seen.push("b")), true);
  assert.equal(dispatcher.enqueue(() => {
    seen.push("c");
    assert.equal(dispatcher.enqueue(() => seen.push("d")), true);
  }), true);

  assert.equal(dispatcher.scheduled_task_count, 1);
  assert.equal(dispatcher.queued_count, 3);

  dispatcher.drain_now();

  assert.deepEqual(seen, ["a", "b", "c", "d"]);
  assert.equal(dispatcher.queued_count, 0);
  assert.equal(dispatcher.delivered_count, 4);
  assert.equal(dispatcher.scheduled_task_count, 1);

  dispatcher.cancel();
  assert.equal(dispatcher.stopped, true);
  assert.equal(dispatcher.enqueue(() => seen.push("e")), false);
});

test("strict dispatcher reports task errors once and does not recurse through the hook", () => {
  const viewer = new WebViewer();
  const dispatcher = viewer._strict_dispatcher;
  const hookErrors = [];
  const reportedErrors = [];
  const originalReportError = globalThis.reportError;

  globalThis.reportError = (error) => {
    reportedErrors.push(String(error));
  };

  try {
    dispatcher.set_error_hook((error) => {
      hookErrors.push(String(error));
      throw new Error("hook failed");
    });

    dispatcher.enqueue(() => {
      throw new Error("task failed");
    });
    dispatcher.drain_now();
  } finally {
    globalThis.reportError = originalReportError;
  }

  assert.deepEqual(hookErrors, ["Error: task failed"]);
  assert.equal(dispatcher.error_notification_count, 1);
  assert.equal(dispatcher.report_error_count, 1);
  assert.deepEqual(reportedErrors, ["Error: hook failed"]);
});

test("raw recording-ID controls forward exact IDs and retain fallback values", async () => {
  const viewer = await startViewer();
  const state = globalThis.__rerun_web_viewer_test_state;

  viewer.set_active_recording_id("ambiguous-id");
  viewer.set_playing("ambiguous-id", true);
  viewer.set_active_timeline("ambiguous-id", "frame");
  viewer.set_current_time("ambiguous-id", "frame", 42);

  assert.deepEqual(callsNamed("set_active_recording_id")[0], [
    "set_active_recording_id",
    "ambiguous-id",
  ]);
  assert.deepEqual(callsNamed("set_playing")[0], [
    "set_playing",
    "ambiguous-id",
    true,
  ]);
  assert.deepEqual(callsNamed("set_active_timeline")[0], [
    "set_active_timeline",
    "ambiguous-id",
    "frame",
  ]);
  assert.deepEqual(callsNamed("set_time_for_timeline")[0], [
    "set_time_for_timeline",
    "ambiguous-id",
    "frame",
    42,
  ]);

  assert.equal(viewer.get_active_recording_id(), null);
  assert.equal(viewer.get_playing("missing"), false);
  assert.equal(viewer.get_active_timeline("missing"), null);
  assert.equal(viewer.get_current_time("missing", "frame"), 0);

  state.activeRecordingId = "present";
  state.playing = true;
  state.activeTimeline = "log_time";
  state.currentTime = 12;
  assert.equal(viewer.get_active_recording_id(), "present");
  assert.equal(viewer.get_playing("present"), true);
  assert.equal(viewer.get_active_timeline("present"), "log_time");
  assert.equal(viewer.get_current_time("present", "log_time"), 12);
  viewer.stop();
});

test("stopped-wrapper methods throw while stop remains idempotent", () => {
  const viewer = new WebViewer();

  assert.throws(() => viewer.open("https://example.test/data.rrd"), /stopped viewer/);
  assert.throws(() => viewer.close("https://example.test/data.rrd"), /stopped viewer/);
  assert.throws(() => viewer.open_channel("stopped"), /stopped web viewer/);
  assert.throws(() => viewer.get_active_recording_id(), /stopped web viewer/);
  assert.throws(
    () => viewer.set_active_recording_id("recording"),
    /stopped web viewer/,
  );
  assert.throws(() => viewer.set_playing("recording", true), /stopped web viewer/);
  assert.throws(
    () => viewer.set_current_time("recording", "frame", 1),
    /stopped web viewer/,
  );

  viewer.stop();
  viewer.stop();
  assert.equal(callsNamed("destroy").length, 0);
  assert.equal(callsNamed("free").length, 0);
});

test("LogChannel standalone contract is no-op unless ready", () => {
  const calls = [];
  let state = "starting";
  const channel = new LogChannel(
    (data) => calls.push(["rrd", data]),
    (data) => calls.push(["table", data]),
    () => calls.push(["close"]),
    () => state,
  );

  channel.send_rrd(new Uint8Array([1]));
  channel.send_table(new Uint8Array([2]));
  channel.close();
  assert.deepEqual(calls, []);

  state = "ready";
  channel.send_rrd(new Uint8Array([3]));
  channel.close();
  channel.close();
  assert.deepEqual(calls.map((call) => call[0]), ["rrd", "close"]);
});
