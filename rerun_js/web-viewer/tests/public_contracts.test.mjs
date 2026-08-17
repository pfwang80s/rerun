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
const { ChromePageExecutionController, LogChannel, StrictOpenError, WebViewer } = await import("../index.js");

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

test("remote page execution lifecycle is epoch-checked and idempotent", () => {
  const listeners = new Map();
  const target = {
    addEventListener(name, listener) { listeners.set(name, listener); },
    removeEventListener(name, listener) { if (listeners.get(name) === listener) listeners.delete(name); },
  };
  const page = { visibilityState: "visible" };
  const states = [];
  const controller = new ChromePageExecutionController({ target, document: page, on_state: (state) => states.push(state) });
  const owner = controller.acquire_owner();
  page.visibilityState = "hidden";
  listeners.get("visibilitychange")();
  listeners.get("visibilitychange")();
  assert.equal(controller.state.kind, "HiddenSuspended");
  assert.equal(owner.is_current(), false);
  page.visibilityState = "visible";
  listeners.get("pageshow")();
  assert.equal(controller.state.kind, "VisibleRunning");
  listeners.get("freeze")();
  listeners.get("pagehide")();
  assert.equal(controller.state.kind, "RemoteTerminating");
  assert.equal(states.filter((state) => state.kind === "HiddenSuspended").length, 1);
  controller.dispose();
  assert.equal(listeners.size, 0);
});

test("remote owner teardown and reentrant resume are guarded", () => {
  const listeners = new Map();
  const target = { addEventListener: (n, f) => listeners.set(n, f), removeEventListener: () => {} };
  const page = { visibilityState: "hidden" };
  let terminated = 0;
  let resumed = 0;
  let controller;
  controller = new ChromePageExecutionController({ target, document: page, on_state: (state) => {
    if (state.kind === "VisibleRevalidating") listeners.get("pagehide")();
  }});
  assert.equal(controller.state.kind, "HiddenSuspended");
  const owner = controller.register_remote_owner({ on_resume: () => resumed++, on_terminate: () => terminated++ });
  page.visibilityState = "visible";
  listeners.get("pageshow")();
  assert.equal(resumed, 0);
  listeners.get("freeze")();
  assert.equal(terminated, 1); // retained suspended owner is cancelled by reentrant pagehide
  owner();
  controller.dispose();
});

test("remote page callbacks isolate throws and hidden deadlines stay cleared", () => {
  const listeners = new Map();
  const target = { addEventListener: (n, f) => listeners.set(n, f), removeEventListener: () => {} };
  const page = { visibilityState: "visible" };
  const controller = new ChromePageExecutionController({ target, document: page });
  let later = 0;
  controller.register_remote_owner({ on_hidden: () => { throw new Error("hidden"); }, on_terminate: () => { throw new Error("terminate"); } });
  controller.register_remote_owner({ on_hidden: () => later++, on_terminate: () => later++ });
  controller.set_visible_deadline(10);
  page.visibilityState = "hidden";
  listeners.get("visibilitychange")();
  assert.equal(controller.visible_deadline, null);
  controller.set_visible_deadline(20);
  assert.equal(controller.visible_deadline, null);
  listeners.get("pagehide")();
  assert.equal(later, 2);
});

function callsNamed(name) {
  return globalThis.__rerun_web_viewer_test_state.calls.filter(
    (call) => call[0] === name,
  );
}

function strictCache(viewer) {
  const cache = globalThis.__rerun_web_viewer_test_state.strict_open_caches.get(viewer);
  return new Proxy(cache, {
    get(target, property) {
      const value = Reflect.get(target, property, target);
      if (typeof value !== "function") return value;
      if (property === "register_remote_owner") return value.bind(target);
      return (...args) => {
        const epoch = target.instance_epoch;
        if (property === "install_operation") while (args.length < 4) args.push(args.length === 1 ? null : args.length === 2 ? "accepted" : "open_and_select");
        if (["attach_preexisting_recording", "replay_active_recording", "replay_completed_recording"].includes(property)) while (args.length < 4) args.push(args.length === 2 ? "test-generation" : null);
        return value.apply(target, [...args, epoch]);
      };
    },
  });
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
  assert.equal(strictCache(viewer).operation_count, 0);
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

  assert.equal(strictCache(viewer).operation_count, 0);
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
  assert.equal(strictCache(viewer).operation_count, 0);
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
  assert.equal(strictCache(viewer).operation_count, 0);
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
  assert.equal(strictCache(viewer).operation_count, 0);
  viewer.stop();
});

test("strict wrapper cache reuses recording wrappers and cached snapshots", () => {
  const viewer = new WebViewer();
  const cache = strictCache(viewer);

  assert.equal(cache.operation_count, 0);

  const operation = cache.install_operation("operation-1");
  assert.equal(cache.operation_count, 1);
  assert.equal(cache.recording_count("operation-1"), 0);
  assert.equal(cache.wrapper_count("operation-1"), 0);

  const first = cache.attach_preexisting_recording("operation-1", "recording-a");
  const second = cache.attach_preexisting_recording("operation-1", "recording-b");
  const duplicate = cache.attach_preexisting_recording("operation-1", "recording-a");

  assert.strictEqual(first, duplicate);
  assert.strictEqual(operation.recordings, operation.recordings);
  assert.strictEqual(operation.recordings[0], first);
  assert.strictEqual(operation.recordings[1], second);
  assert.equal(cache.recording_count("operation-1"), 2);
  assert.equal(cache.wrapper_count("operation-1"), 2);
  assert.equal(first.phase, "preexisting");

  cache.replay_active_recording("operation-1", "recording-a");
  cache.replay_completed_recording("operation-1", "recording-b");
  assert.strictEqual(operation.recordings[0], first);
  assert.strictEqual(operation.recordings[1], second);
  assert.equal(first.phase, "active");
  assert.equal(second.phase, "completed");
  assert.equal(cache.recording_count("operation-1"), 2);

  cache.complete_installation_ack("operation-1");
  cache.arm_internal_activation_bridge("operation-1");
  assert.equal(cache.installation_ack_complete("operation-1"), true);
  assert.equal(cache.activation_bridge_armed("operation-1"), true);
});

test("strict wrapper internals are absent from the ordinary runtime object graph", () => {
  const viewer = new WebViewer();
  const cache = strictCache(viewer);
  const operation = cache.install_operation("operation-opaque");
  const recording = cache.attach_preexisting_recording(
    "operation-opaque",
    "recording-opaque",
  );
  const forbidden = [
    "attach_preexisting_recording",
    "replay_active_recording",
    "replay_completed_recording",
    "complete_installation_ack",
    "arm_internal_activation_bridge",
    "mark_active",
    "mark_completed",
    "recording_count",
    "wrapper_count",
    "installation_ack_complete",
    "activation_bridge_armed",
  ];

  assert.equal("_strict_open_cache" in viewer, false);
  assert.deepEqual(Reflect.ownKeys(operation), []);
  assert.deepEqual(Reflect.ownKeys(recording), []);
  assert.deepEqual(
    Object.getOwnPropertySymbols(Object.getPrototypeOf(operation)),
    [],
  );
  assert.deepEqual(
    Object.getOwnPropertySymbols(Object.getPrototypeOf(recording)),
    [],
  );
  for (const name of forbidden) {
    assert.equal(Object.getOwnPropertyNames(Object.getPrototypeOf(operation)).includes(name), false);
    assert.equal(Object.getOwnPropertyNames(Object.getPrototypeOf(recording)).includes(name), false);
  }

  const oldTestFlag = process.env.RERUN_WEB_VIEWER_TEST;
  const oldProcess = globalThis.process;
  delete process.env.RERUN_WEB_VIEWER_TEST;
  try {
    const ordinaryViewer = new WebViewer();
    assert.equal(globalThis.__rerun_web_viewer_test_state.strict_open_caches.has(ordinaryViewer), false);
    globalThis.process = {
      release: { name: "node" },
      env: { RERUN_WEB_VIEWER_TEST: "1" },
    };
    const forgedHostViewer = new WebViewer();
    assert.equal(
      globalThis.__rerun_web_viewer_test_state.strict_open_caches.has(forgedHostViewer),
      false,
    );
    globalThis.process = {
      release: { name: "node" },
      getBuiltinModule() { return this; },
      env: { RERUN_WEB_VIEWER_TEST: "1" },
    };
    const postImportForgedViewer = new WebViewer();
    assert.equal(
      globalThis.__rerun_web_viewer_test_state.strict_open_caches.has(postImportForgedViewer),
      false,
    );
  } finally {
    globalThis.process = oldProcess;
    process.env.RERUN_WEB_VIEWER_TEST = oldTestFlag;
  }
  operation.dispose();
});

test("strict wrapper cache aborts before disposing temporary operations", () => {
  const viewer = new WebViewer();
  const cache = strictCache(viewer);
  const order = [];

  assert.throws(
    () =>
      cache.install_operation_with_abort(
        "operation-throw",
        (operation) => {
          order.push(["build", cache.recording_count("operation-throw")]);
          cache.attach_preexisting_recording("operation-throw", "recording-a");
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

test("strict exact identity distinguishes same raw recording across Store generations", () => {
  const viewer = new WebViewer();
  const cache = strictCache(viewer);
  const operation = cache.install_operation("operation-generations");
  const calls = [];
  const adapter = (label) => ({
    select(identity) {
      calls.push([label, "select", identity]);
      return { command: "select", code: "accepted", accepted: true };
    },
    seek(identity) {
      calls.push([label, "seek", identity]);
      return { command: "seek", code: "accepted", accepted: true };
    },
    play(identity) {
      calls.push([label, "play", identity]);
      return { command: "play", code: "accepted", accepted: true };
    },
    close(identity) {
      calls.push([label, "close", identity]);
      return { command: "close", code: "accepted", accepted: true };
    },
    dispose() {},
  });

  const first = cache.attach_preexisting_recording(
    "operation-generations",
    "same-raw-recording",
    "generation-a",
    adapter("first"),
  );
  const second = cache.attach_preexisting_recording(
    "operation-generations",
    "same-raw-recording",
    "generation-b",
    adapter("second"),
  );
  assert.notStrictEqual(first, second);
  assert.equal(cache.recording_count("operation-generations"), 2);
  assert.equal(first.select().code, "accepted");
  assert.equal(second.select().code, "accepted");
  assert.deepEqual(calls.map((call) => call.slice(0, 2)), [
    ["first", "select"],
    ["second", "select"],
  ]);
  assert.notStrictEqual(calls[0][2], calls[1][2]);

  first.dispose();
  assert.equal(cache.recording_count("operation-generations"), 1);
  second.dispose();
  operation.dispose();
  assert.equal(cache.operation_count, 0);
});

test("strict operation close gates children, late attach, and lifecycle callbacks", () => {
  const viewer = new WebViewer();
  const cache = strictCache(viewer);
  let sourceCloseCount = 0;
  let childSelectCount = 0;
  const operation = cache.install_operation(
    "operation-close",
    {
      close() {
        sourceCloseCount += 1;
        return { command: "close", code: "accepted", accepted: true };
      },
      dispose() {},
    },
    "activated",
    "background",
  );
  const recording = cache.attach_preexisting_recording(
    "operation-close",
    "recording-close",
    "generation-close",
    {
      select() {
        childSelectCount += 1;
        return { command: "select", code: "accepted", accepted: true };
      },
      seek() {
        return { command: "seek", code: "accepted", accepted: true };
      },
      play() {
        return { command: "play", code: "accepted", accepted: true };
      },
      close() {
        return { command: "close", code: "accepted", accepted: true };
      },
      dispose() {},
    },
  );
  const phases = [];
  operation.on("terminal", (handle) => phases.push(handle.phase));
  assert.equal(operation.phase, "activated");
  assert.equal(operation.recordingOpenBehavior, "background");
  assert.equal(cache.transition("operation-close", "behavior_ready"), true);
  assert.equal(cache.transition("operation-close", "presentation_ready"), true);
  assert.equal(cache.transition("operation-close", "terminal"), true);
  assert.deepEqual(phases, []);
  viewer._strict_dispatcher.drain_now();
  assert.deepEqual(phases, ["terminal"]);

  assert.deepEqual(recording.select(), {
    command: "select",
    code: "operation_closed",
    accepted: false,
  });
  assert.equal(childSelectCount, 0);
  assert.equal(cache.replay_active_recording(
    "operation-close",
    "late-recording",
    "late-generation",
  ), null);
  assert.deepEqual(operation.close(), {
    command: "close",
    code: "operation_closed",
    accepted: false,
  });
  assert.equal(sourceCloseCount, 0);

  assert.equal(cache.transition("operation-close", "removed"), true);
  assert.deepEqual(recording.select(), {
    command: "select",
    code: "recording_removed",
    accepted: false,
  });
  assert.equal(cache.transition("operation-close", "terminal"), false);

  operation.dispose();
  assert.equal(cache.operation_count, 0);

  const liveOperation = cache.install_operation(
    "operation-close-accepted",
    {
      close() {
        sourceCloseCount += 1;
        return { command: "close", code: "accepted", accepted: true };
      },
      dispose() {},
    },
  );
  const liveRecording = cache.attach_preexisting_recording(
    "operation-close-accepted",
    "recording-close-accepted",
    "generation-close-accepted",
  );
  assert.deepEqual(liveOperation.close(), {
    command: "close",
    code: "accepted",
    accepted: true,
  });
  assert.equal(sourceCloseCount, 1);
  assert.deepEqual(liveRecording.select(), {
    command: "select",
    code: "operation_closed",
    accepted: false,
  });
  assert.equal(cache.replay_active_recording(
    "operation-close-accepted",
    "late-recording",
  ), null);
  liveOperation.dispose();
  assert.equal(cache.operation_count, 0);
});

test("strict operation close propagates recording_removed to exact children", () => {
  const viewer = new WebViewer();
  const cache = strictCache(viewer);
  const operation = cache.install_operation("operation-close-removed", {
    close: () => ({ command: "close", code: "recording_removed", accepted: false }),
    dispose() {},
  });
  const recording = cache.attach_preexisting_recording(
    "operation-close-removed", "recording-close-removed",
  );
  assert.deepEqual(operation.close(), {
    command: "close", code: "recording_removed", accepted: false,
  });
  assert.deepEqual(recording.select(), {
    command: "select", code: "recording_removed", accepted: false,
  });
  operation.dispose();
});

test("strict lifecycle transitions are contiguous and drop stale queued listeners", () => {
  const viewer = new WebViewer();
  const cache = strictCache(viewer);
  const operation = cache.install_operation("operation-transition-order");
  const events = [];
  operation.on("activated", () => events.push("activated"));
  operation.on("terminal", () => events.push("terminal"));
  operation.on("removed", () => events.push("removed"));

  assert.equal(cache.transition("operation-transition-order", "activated"), true);
  assert.equal(cache.transition("operation-transition-order", "accepted"), false);
  assert.equal(cache.transition("operation-transition-order", "activated"), false);
  assert.equal(cache.transition("operation-transition-order", "terminal"), false);
  assert.equal(cache.transition("operation-transition-order", "behavior_ready"), true);
  assert.equal(cache.transition("operation-transition-order", "presentation_ready"), true);
  assert.equal(cache.transition("operation-transition-order", "terminal"), true);
  assert.equal(cache.transition("operation-transition-order", "removed"), true);
  assert.equal(operation.phase, "removed");

  viewer._strict_dispatcher.drain_now();
  assert.deepEqual(events, ["removed"]);
  assert.equal(cache.transition("operation-transition-order", "removed"), false);
  assert.equal(cache.transition("operation-transition-order", "presentation_ready"), false);
  assert.equal(cache.transition("operation-transition-order", "invalid"), false);

  operation.dispose();
  assert.equal(cache.operation_count, 0);
});

test("strict lifecycle callbacks queued before viewer stop are discarded", async () => {
  const viewer = new WebViewer();
  const cache = strictCache(viewer);
  const operation = cache.install_operation("operation-stop-stale");
  let callbacks = 0;
  operation.on("activated", () => { callbacks += 1; });
  await viewer.start(null, document.body, null);
  assert.equal(cache.transition("operation-stop-stale", "activated"), true);
  viewer.stop();
  viewer._strict_dispatcher.drain_now();
  assert.equal(callbacks, 0);
  operation.dispose();
});

test("strict operation close discards queued lifecycle callbacks", async () => {
  const viewer = new WebViewer();
  const cache = strictCache(viewer);
  const operation = cache.install_operation("operation-close-stale", {
    close: () => ({ command: "close", code: "accepted", accepted: true }),
    dispose() {},
  });
  let callbacks = 0;
  operation.on("activated", () => { callbacks += 1; });
  await viewer.start(null, document.body, null);
  assert.equal(cache.transition("operation-close-stale", "activated"), true);
  assert.deepEqual(operation.close(), { command: "close", code: "accepted", accepted: true });
  viewer._strict_dispatcher.drain_now();
  assert.equal(callbacks, 0);
  operation.dispose();
});

test("strict lifecycle transition does not partially commit when dispatcher is full", () => {
  const viewer = new WebViewer();
  const cache = strictCache(viewer);
  const operation = cache.install_operation("operation-backpressure");
  operation.on("activated", () => {});
  for (let index = 0; index < viewer._strict_dispatcher.max_queue_length; index++) {
    assert.equal(viewer._strict_dispatcher.enqueue(() => {}), true);
  }
  assert.equal(cache.transition("operation-backpressure", "activated"), false);
  assert.equal(operation.phase, "accepted");
  viewer._strict_dispatcher.cancel();
  assert.equal(cache.transition("operation-backpressure", "activated"), false);
  operation.dispose();
});

test("strict abort install never replaces an existing operation identity", () => {
  const viewer = new WebViewer();
  const cache = strictCache(viewer);
  const existing = cache.install_operation("operation-abort-existing");
  let built = false;
  assert.strictEqual(
    cache.install_operation_with_abort(
      "operation-abort-existing",
      () => { built = true; },
      () => { throw new Error("abort callback must not run"); },
    ),
    existing,
  );
  assert.equal(built, false);
  assert.strictEqual(cache.get_operation("operation-abort-existing"), existing);
  existing.dispose();
});

test("strict duplicate identity rejects adapter conflicts and upgrades null adapters once", () => {
  const viewer = new WebViewer();
  const cache = strictCache(viewer);
  cache.install_operation("operation-adapter");
  let disposeCount = 0;
  const adapter = {
    select: () => ({ command: "select", code: "accepted", accepted: true }),
    seek: () => ({ command: "seek", code: "accepted", accepted: true }),
    play: () => ({ command: "play", code: "accepted", accepted: true }),
    close: () => ({ command: "close", code: "accepted", accepted: true }),
    dispose() { disposeCount += 1; },
  };
  const first = cache.attach_preexisting_recording("operation-adapter", "recording-adapter");
  assert.deepEqual(first?.select(), {
    command: "select", code: "capability_unavailable", accepted: false,
  });
  assert.strictEqual(cache.attach_preexisting_recording(
    "operation-adapter", "recording-adapter", "test-generation", adapter,
  ), first);
  assert.deepEqual(first?.select(), { command: "select", code: "accepted", accepted: true });
  assert.equal(cache.attach_preexisting_recording(
    "operation-adapter", "recording-adapter", "test-generation", { ...adapter },
  ), null);
  first?.dispose();
  assert.equal(disposeCount, 1);
  cache.dispose_operation("operation-adapter");
});

test("strict duplicate operation identity upgrades null adapter and rejects conflicts", () => {
  const viewer = new WebViewer();
  const cache = strictCache(viewer);
  const first = cache.install_operation("operation-adapter-upgrade");
  let disposeCount = 0;
  const adapter = {
    close: () => ({ command: "close", code: "accepted", accepted: true }),
    dispose: () => { disposeCount += 1; },
  };
  assert.strictEqual(
    cache.install_operation("operation-adapter-upgrade", adapter),
    first,
  );
  assert.equal(cache.install_operation(
    "operation-adapter-upgrade",
    { ...adapter },
  ), null);
  first.dispose();
  assert.equal(disposeCount, 1);
});

test("strict exact controls validate sealed targets and keep close separate from dispose", () => {
  const viewer = new WebViewer();
  const cache = strictCache(viewer);
  const operation = cache.install_operation("operation-exact");
  const recording = cache.attach_preexisting_recording(
    "operation-exact",
    "same-recording-id",
  );

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
  assert.equal(cache.recording_count("operation-exact"), 0);
  operation.dispose();
  assert.equal(operation.disposed, true);
  assert.equal(cache.recording_count("operation-exact"), 0);
});

test("strict explicit dispose still releases adapters after weak finalizer hardening", () => {
  const viewer = new WebViewer();
  const cache = strictCache(viewer);
  let operationDisposals = 0;
  let recordingDisposals = 0;
  const operation = cache.install_operation("operation-dispose-adapter", {
    close: () => ({ command: "close", code: "accepted", accepted: true }),
    dispose: () => { operationDisposals += 1; },
  });
  const recording = cache.attach_preexisting_recording(
    "operation-dispose-adapter",
    "recording-dispose-adapter",
    "generation-dispose-adapter",
    {
      select: () => ({ command: "select", code: "accepted", accepted: true }),
      seek: () => ({ command: "seek", code: "accepted", accepted: true }),
      play: () => ({ command: "play", code: "accepted", accepted: true }),
      close: () => ({ command: "close", code: "accepted", accepted: true }),
      dispose: () => { recordingDisposals += 1; },
    },
  );
  recording.dispose();
  operation.dispose();
  assert.equal(recordingDisposals, 1);
  assert.equal(operationDisposals, 1);
});

test("strict exact handles become terminal when the viewer stops", async () => {
  const viewer = await startViewer();
  const cache = strictCache(viewer);
  const operation = cache.install_operation("operation-stopped");
  const recording = cache.attach_preexisting_recording(
    "operation-stopped",
    "recording-stopped",
  );
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

test("viewer stop synchronously tears down remote owners and drops stale work", async () => {
  const viewer = await startViewer();
  const cache = strictCache(viewer);
  const old_epoch = cache.instance_epoch;
  const calls = [];
  const operation = cache.install_operation("operation-teardown", {
    close: () => ({ command: "close", code: "accepted", accepted: true }),
    dispose: () => calls.push("operation"),
  });
  const recording = cache.attach_preexisting_recording(
    "operation-teardown",
    "recording-teardown",
    "generation-teardown",
    {
      select: () => ({ command: "select", code: "accepted", accepted: true }),
      seek: () => ({ command: "seek", code: "accepted", accepted: true }),
      play: () => ({ command: "play", code: "accepted", accepted: true }),
      close: () => ({ command: "close", code: "accepted", accepted: true }),
      dispose: () => calls.push("recording"),
    },
  );
  let listener_calls = 0;
  operation.on("activated", () => listener_calls++);
  viewer._strict_dispatcher.enqueue(() => calls.push("stale"));
  let owner_cancels = 0;
  assert.equal(cache.register_remote_owner({ cancel: () => owner_cancels++ }), true);

  viewer.stop();

  assert.deepEqual(calls, ["recording", "operation"]);
  assert.equal(owner_cancels, 1);
  assert.equal(listener_calls, 0);
  assert.equal(viewer._strict_dispatcher.stopped, true);
  assert.deepEqual(recording.select(), {
    command: "select",
    code: "viewer_stopped",
    accepted: false,
  });

  // A restarted Viewer instance gets a fresh dispatcher epoch; old queued work cannot run.
  await viewer.start(null, document.body, null);
  assert.equal(viewer._strict_dispatcher.stopped, false);
  assert.equal(cache.install_operation("late-stale", null, "accepted", "open_and_select", old_epoch), null);
  assert.equal(cache.complete_installation_ack("operation-teardown", old_epoch), false);
  assert.equal(cache.arm_internal_activation_bridge("operation-teardown", old_epoch), false);
  assert.equal(cache.dispose_operation("operation-teardown", old_epoch), false);
  assert.equal(cache.recording_count("operation-teardown", old_epoch), 0);
  const fresh_operation = cache.install_operation("operation-teardown");
  assert.notStrictEqual(fresh_operation, operation);
  assert.equal(fresh_operation.phase, "accepted");
  assert.equal(viewer._strict_dispatcher.enqueue(() => calls.push("fresh")), true);
  viewer._strict_dispatcher.drain_now();
  assert.deepEqual(calls, ["recording", "operation", "fresh"]);
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(calls, ["recording", "operation", "fresh"]);
  fresh_operation.dispose();
  viewer.stop();
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
