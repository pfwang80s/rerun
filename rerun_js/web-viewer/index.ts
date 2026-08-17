// @ts-ignore
import type { WebHandle, wasm_bindgen } from "./re_viewer";

let get_wasm_bindgen: (() => typeof wasm_bindgen) | null = null;
let _wasm_module: WebAssembly.Module | null = null;

/**
 * Feature-detect WebAssembly SIMD (`simd128`).
 *
 * The viewer .wasm is compiled with `-Ctarget-feature=+simd128`, so a browser
 * without SIMD support will fail to instantiate the module with a cryptic
 * `CompileError`. We probe up-front and surface a clear error instead.
 *
 * The probe is a minimal module that uses the `v128.any_true` instruction.
 * Supported in: Chrome 91+, Firefox 89+, Safari 16.4+.
 */
function has_wasm_simd(): boolean {
  try {
    return WebAssembly.validate(new Uint8Array([
      0, 97, 115, 109, 1, 0, 0, 0, 1, 5, 1, 96, 0, 1, 123, 3, 2, 1, 0,
      10, 10, 1, 8, 0, 65, 0, 253, 15, 253, 98, 11,
    ]));
  } catch {
    return false;
  }
}

const UNSUPPORTED_BROWSER_MESSAGE =
  "Your browser is too old to run the Rerun Viewer. " +
  "The Viewer requires WebAssembly SIMD support, available in " +
  "Chrome 91+, Firefox 89+, Safari 16.4+, or any modern Chromium-based browser. " +
  "Please update your browser and try again.";

async function fetch_viewer_js(base_url?: string): Promise<(() => typeof wasm_bindgen)> {
  // @ts-ignore
  return (await import("./re_viewer")).default;
}

async function fetch_viewer_wasm(
  base_url?: string,
  on_progress?: (received: number, total: number | null) => void,
): Promise<Response> {
  //!<INLINE-MARKER-OPEN>
  const url = base_url
    ? new URL("./re_viewer_bg.wasm", base_url)
    : new URL("./re_viewer_bg.wasm", import.meta.url);
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(
      `Failed to fetch viewer Wasm: ${response.status} ${response.statusText}`,
    );
  }
  return wrap_fetch_with_progress(response, on_progress);
  //!<INLINE-MARKER-CLOSE>
}

/**
 * Estimates total uncompressed bytes for progress display.
 * This is a rough estimate — do NOT use for truncation detection.
 */
function estimate_total_bytes(response: Response): number | null {
  // When served with `rerun-final-length`, use that (set by `re_web_viewer_server`).
  const final_length = response.headers.get("rerun-final-length");
  if (final_length != null) return parseInt(final_length, 10);

  // When gzip-compressed, try the GCS uncompressed-size header.
  if (response.headers.get("content-encoding") === "gzip") {
    const uncompressed = response.headers.get("x-goog-meta-uncompressed-size");
    if (uncompressed != null) return parseInt(uncompressed, 10);

    // Fall back to content-length * 3 (good empirical approximation for gzip'd wasm).
    const cl = response.headers.get("content-length");
    if (cl != null) return parseInt(cl, 10) * 3;
  }

  // Uncompressed: content-length is the exact size.
  const cl = response.headers.get("content-length");
  if (cl != null) return parseInt(cl, 10);

  return null;
}

/**
 * Wraps a fetch response to track download progress.
 */
function wrap_fetch_with_progress(
  response: Response,
  on_progress?: (received: number, total: number | null) => void,
): Response {
  const total_bytes = estimate_total_bytes(response);

  if (!response.body) return response;

  let received = 0;
  const body = response.body;
  const tracked = new ReadableStream({
    async start(controller) {
      const reader = body.getReader();
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        received += value.byteLength;
        on_progress?.(received, total_bytes);
        controller.enqueue(value);
      }
      controller.close();
    },
  });

  return new Response(tracked, {
    status: response.status,
    statusText: response.statusText,
    headers: response.headers,
  });
}

function format_mib(bytes: number): string {
  return (bytes / (1024 * 1024)).toFixed(1) + " MiB";
}

async function load(
  base_url?: string,
  on_progress?: (received: number, total: number | null) => void,
): Promise<typeof wasm_bindgen.WebHandle> {
  if (!has_wasm_simd()) {
    throw new Error(UNSUPPORTED_BROWSER_MESSAGE);
  }

  // instantiate wbg globals+module for every invocation of `load`,
  // but don't load the JS/Wasm source every time
  if (!get_wasm_bindgen || !_wasm_module) {
    [get_wasm_bindgen, _wasm_module] = await Promise.all([
      fetch_viewer_js(base_url),
      WebAssembly.compileStreaming(fetch_viewer_wasm(base_url, on_progress)),
    ]);
  }
  let bindgen = get_wasm_bindgen();
  await bindgen({ module_or_path: _wasm_module });
  return class extends bindgen.WebHandle {
    free() {
      super.free();
      // @ts-ignore
      bindgen.deinit();
    }
  };
}

let _minimize_current_fullscreen_viewer: (() => void) | null = null;

function randomId(): string {
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  return Array.from(bytes)
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
}

export type Panel = "top" | "blueprint" | "selection" | "time";
export type PanelState = "hidden" | "collapsed" | "expanded";
export type Backend = "webgpu" | "webgl";
export type VideoDecoder = "auto" | "prefer_software" | "prefer_hardware";

export interface LoginOptions {
  /** URL to redirect to after successful OAuth login (e.g. "/signed-in" or "https://example.com/signed-in"). */
  signed_in_url: string;
  /** URL to redirect to after logout (e.g. "/signed-out" or "https://example.com/signed-out"). */
  signed_out_url: string;
}

// NOTE: When changing these options, consider how it affects the `web-viewer-react` package:
//       - Should this option be exposed?
//       - Should changing this option result in the viewer being restarted?
export interface WebViewerOptions {
  /** Url to the example manifest. Unused if `hide_welcome_screen` is set to `true`. */
  manifest_url?: string;

  /** The render backend used by the viewer. Either "webgl" or "webgpu". Prefers "webgpu". */
  render_backend?: Backend;

  /** Video decoder config used by the viewer. Either "auto", "prefer_software" or "prefer_hardware". */
  video_decoder?: VideoDecoder;

  /** If set to `true`, hides the welcome screen, which contains our examples. Defaults to `false`. */
  hide_welcome_screen?: boolean;

  /**
   * Allow the viewer to handle fullscreen mode.
   * This option sets canvas style so is not recommended if you are doing anything custom,
   * or are embedding the viewer in an iframe.
   *
   * Defaults to `false`.
   */
  allow_fullscreen?: boolean;

  /**
   * Enable the history feature of the viewer.
   *
   * This is only relevant when `hide_welcome_screen` is `false`,
   * as it's currently only used to allow going between the welcome screen and examples.
   *
   * Defaults to `false`.
   */
  enable_history?: boolean;

  /** The CSS width of the canvas. */
  width?: string;

  /** The CSS height of the canvas. */
  height?: string;

  /** The fallback token to use, if any.
   *
   * The fallback token behaves similarly to the `REDAP_TOKEN` env variable. If set in the
   * enclosing notebook environment, it should be used to set the fallback token.
   */
  fallback_token?: string;

  /**
   * The color theme to use.
   *
   * If not set, the viewer uses the previously persisted theme preference or defaults to "system".
   */
  theme?: "dark" | "light" | "system";

  /**
   * Enable OAuth login in the viewer.
   *
   * When set, the viewer shows login UI and uses the provided URLs for OAuth redirects.
   *
   * To use this:
   * 1. Host the `signed-in.html` and `signed-out.html` pages alongside your viewer.
   *    Templates can be found at:
   *    - https://github.com/rerun-io/rerun/blob/main/crates/viewer/re_web_viewer_server/web_viewer/signed-in.html
   *    - https://github.com/rerun-io/rerun/blob/main/crates/viewer/re_web_viewer_server/web_viewer/signed-out.html
   * 2. Set the URLs to those pages here.
   * 3. Contact your Rerun representative to have the redirect URLs
   *    and origin whitelisted in the OAuth configuration.
   *
   * When not set (default), login UI is hidden. Token-based auth still works.
   */
  login?: LoginOptions;
}

// `AppOptions` and `WebViewerOptions` must be compatible
// otherwise we need to restructure how we pass options to the viewer

/**
 * The public interface is @see {WebViewerOptions}. This adds a few additional, internal options.
 *
 * @private
 */
export interface AppOptions extends WebViewerOptions {
  /** The url that's used when sharing web viewer urls
   *
   * If not set, the viewer will use the url of the page it is embedded in.
   */
  viewer_base_url?: string;

  /** Whether the viewer is running in a notebook. */
  notebook?: boolean;

  url?: string;
  panel_state_overrides?: Partial<{
    [K in Panel]: PanelState;
  }>;
  on_viewer_event?: (event_json: string) => void;
  fullscreen?: FullscreenOptions;
}

// Types are based on `crates/viewer/re_viewer/src/event.rs`.
// Important: The event names defined here are `snake_case` versions
// of their `PascalCase` counterparts on the Rust side.
/** An event produced in the Viewer. */
export type ViewerEvent =
  | PlayEvent
  | PauseEvent
  | TimeUpdateEvent
  | TimelineChangeEvent
  | SelectionChangeEvent
  | RecordingOpenEvent;

/**
 * Properties available on all {@link ViewerEvent} types.
 */
export type ViewerEventBase = {
  application_id: string;
  recording_id: string;
  partition_id?: string;
}

/**
 * Fired when the timeline starts playing.
 */
export type PlayEvent = ViewerEventBase & {
  type: "play";
};

/**
 * Fired when the timeline stops playing.
 */
export type PauseEvent = ViewerEventBase & {
  type: "pause";
}

/**
 * Fired when the timepoint changes.
 */
export type TimeUpdateEvent = ViewerEventBase & {
  type: "time_update";
  time: number;
}

/**
 * Fired when a different timeline is selected.
 */
export type TimelineChangeEvent = ViewerEventBase & {
  type: "timeline_change";
  timeline: string;
  time: number;
}

/**
 * Fired when the selection changes.
 *
 * This event is fired each time any part of the event payload changes,
 * this includes for example clicking on different parts of the same
 * entity in a 2D or 3D view.
 */
export type SelectionChangeEvent = ViewerEventBase & {
  type: "selection_change";
  items: SelectionChangeItem[];
}

/**
 * Fired when a new recording is opened in the Viewer.
 *
 * For `rrd` file or stream, a recording is considered "open" after
 * enough information about the recording, such as its ID and source,
 * is received.
 *
 * Contains some basic information about the origin of the recording.
 */
export type RecordingOpenEvent = ViewerEventBase & {
  type: "recording_open";

  /**
   * Where the recording came from.
   *
   * The value should be considered unstable, which is why we don't
   * list the possible values here.
   */
  source: string;

  /**
   * Version of the SDK used to create this recording.
   *
   * Uses semver format.
   */
  version?: string;
}

// A bit of TypeScript metaprogramming to automatically produce a
// mapping of event names to event payloads given the above type
// definitions.

// Yield the event with type `K`.
type _GetViewerEvent<K> =
  Extract<ViewerEvent, { type: K }>;

// `ViewerEvent` is a union of all events, so its `type` field
// is a union of all `type` fields.
type _ViewerEventNames = ViewerEvent["type"];

// For every event, get its payload type.
type ViewerEventMap = {
  [K in _ViewerEventNames]: _GetViewerEvent<K>
}

/**
 * Selected an entity, or an instance of an entity.
 *
 * If the entity was selected within a view, then this also
 * includes the view's name.
 *
 * If the entity was selected within a 2D or 3D space view,
 * then this also includes the position.
 */
export type EntityItem = {
  type: "entity";

  entity_path: string;
  instance_id?: number;
  view_name?: string;
  position?: [number, number, number];
};

/** Selected a view. */
export type ViewItem = { type: "view"; view_id: string; view_name: string };

/** Selected a container. */
export type ContainerItem = {
  type: "container";
  container_id: string;
  container_name: string;
};

/** A single item in a selection. */
export type SelectionChangeItem = EntityItem | ViewItem | ContainerItem;

interface FullscreenOptions {
  get_state: () => boolean;
  on_toggle: () => void;
}

export interface WebViewerEvents extends ViewerEventMap {
  fullscreen: boolean;
  ready: void;
}

// This abomination is a mapped type with key filtering, and is used to split the events
// into those which take no value in their callback, and those which do.
// https://www.typescriptlang.org/docs/handbook/2/mapped-types.html#key-remapping-via-as
export type EventsWithValue = {
  [K in keyof WebViewerEvents as WebViewerEvents[K] extends void
  ? never
  : K]: WebViewerEvents[K] extends any[]
  ? WebViewerEvents[K]
  : [WebViewerEvents[K]];
};

export type EventsWithoutValue = {
  [K in keyof WebViewerEvents as WebViewerEvents[K] extends void
  ? K
  : never]: WebViewerEvents[K];
};

function delay(ms: number) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/** Resolve a potentially relative URL against the current origin. */
function resolveAbsoluteUrl(url: string): string {
  return new URL(url, window.location.href).toString();
}

/** The timeline used when decoding a remote MCAP recording. */
export type RemoteMcapTimeType = "timestamp_ns" | "duration_ns";

/** The validator policy used when admitting a remote MCAP object. */
export type RemoteMcapRepresentationConsistency =
  | "require_strong_validator"
  | "allow_deployment_assumed";

/** The behavior applied when the recording becomes ready. */
export type RemoteMcapOpenBehavior = "open" | "open_and_select" | "background";

/** Typed options for a strict HTTP remote-MCAP request. */
export interface HttpOpenRequestOptions {
  /** Canonical topic filters. */
  topic_filter?: string | readonly string[];
  /** Version of the bounded decoder allowlist, encoded as a decimal string. */
  decoder_allowlist_version?: string;
  /** Version of the bounded assignment policy, encoded as a decimal string. */
  assignment_policy_version?: string;
  /** MCAP timeline type. Defaults to `timestamp_ns`; sequence timelines are unsupported. */
  mcap_time_type?: RemoteMcapTimeType;
  /** Representation validator policy. Defaults to `require_strong_validator`. */
  representation_consistency?: RemoteMcapRepresentationConsistency;
  /** Recording behavior. Defaults to `open_and_select`. */
  recording_open_behavior?: RemoteMcapOpenBehavior;
  /** Explicitly opt in to the bounded 8-byte extensionless format sniff. */
  allow_extensionless_sniff?: boolean;
}

/** One strict HTTP remote-MCAP request. */
export interface HttpOpenRequestSpec {
  /** HTTP(S) URL for a remote MCAP object. Query bytes are never retained by the public handle. */
  url: string;
  /** Frozen semantic and opening options. */
  options?: HttpOpenRequestOptions;
}

export type StrictOpenErrorCode =
  | "ViewerStopped"
  | "InvalidRequestShape"
  | "InvalidUrl"
  | "UnsupportedStrictOpenRoute"
  | "UnsupportedFormat"
  | "ExistingSourceOptionsConflict"
  | "BatchTooLarge"
  | "ResourceLimitExceeded"
  | "CapabilityUnavailable"
  | "HandoffCancelled"
  | "HandoffStateChanged"
  | "ProtocolViolation";

export type StrictOpenErrorPhase = "admission" | "handoff" | "opening" | "lifecycle";

const strict_open_error_messages: Record<StrictOpenErrorCode, string> = {
  ViewerStopped: "Viewer is stopped",
  InvalidRequestShape: "strict open request shape is invalid",
  InvalidUrl: "strict open URL is invalid",
  UnsupportedStrictOpenRoute: "strict open route is unsupported",
  UnsupportedFormat: "strict open format is unsupported",
  ExistingSourceOptionsConflict: "strict open options conflict with an existing source",
  BatchTooLarge: "strict open batch exceeds its item limit",
  ResourceLimitExceeded: "strict open resource limit was exceeded",
  CapabilityUnavailable: "strict remote-MCAP capability is unavailable",
  HandoffCancelled: "strict open handoff was cancelled",
  HandoffStateChanged: "strict open handoff state changed",
  ProtocolViolation: "strict open handoff protocol violation",
};

/** A redacted, request-local strict-open failure. */
export class StrictOpenError extends Error {
  readonly code: StrictOpenErrorCode;
  readonly phase: StrictOpenErrorPhase;
  readonly retryable: boolean;
  readonly index: number | null;

  constructor(
    code: StrictOpenErrorCode,
    phase: StrictOpenErrorPhase,
    options: { retryable?: boolean; index?: number | null } = {},
  ) {
    super(strict_open_error_messages[code]);
    this.name = "StrictOpenError";
    this.code = code;
    this.phase = phase;
    this.retryable = options.retryable ?? false;
    this.index = options.index ?? null;
  }
}

const STRICT_OPEN_MAX_BATCH_ITEMS = 64;
const STRICT_OPEN_MAX_TOPIC_FILTERS = 128;
const STRICT_OPEN_MAX_FIELD_BYTES = 65_536;

type NormalizedStrictOpenSpec = {
  route: "explicit_mcap" | "extensionless_sniff";
};

function strict_utf8_byte_length(value: string): number {
  // TextEncoder is available in every supported browser and avoids treating UTF-16 code units
  // as wire bytes.  The fallback keeps contract tests usable in minimal JS hosts.
  return typeof TextEncoder === "function" ? new TextEncoder().encode(value).byteLength : value.length;
}

function strict_decimal_option(value: unknown, index: number): string {
  if (typeof value !== "string" || !/^(?:0|[1-9][0-9]{0,19})$/.test(value)) {
    throw new StrictOpenError("InvalidRequestShape", "admission", { index });
  }
  // Avoid a JavaScript Number conversion: canonical decimal strings are the wire contract.
  if (value.length === 20 && value > "18446744073709551615") {
    throw new StrictOpenError("ResourceLimitExceeded", "admission", { index });
  }
  return value;
}

function strict_option_value<T>(
  value: unknown,
  allowed: readonly T[],
  index: number,
): T {
  if (!allowed.includes(value as T)) {
    throw new StrictOpenError("InvalidRequestShape", "admission", { index });
  }
  return value as T;
}

function normalize_strict_open_spec(
  spec: unknown,
  index: number,
): NormalizedStrictOpenSpec {
  try {
    if (spec === null || typeof spec !== "object" || Array.isArray(spec)) {
      throw new StrictOpenError("InvalidRequestShape", "admission", { index });
    }
    const request = spec as Record<string, unknown>;
    if (Object.getPrototypeOf(request) !== Object.prototype && Object.getPrototypeOf(request) !== null) {
      throw new StrictOpenError("InvalidRequestShape", "admission", { index });
    }
    const request_keys = Reflect.ownKeys(request);
    if (request_keys.some((key) => typeof key !== "string" || !["url", "options"].includes(key))) {
      throw new StrictOpenError("InvalidRequestShape", "admission", { index });
    }
    const url_descriptor = Object.getOwnPropertyDescriptor(request, "url");
    if (!url_descriptor || !("value" in url_descriptor)
      || typeof url_descriptor.value !== "string" || strict_utf8_byte_length(url_descriptor.value) === 0) {
      throw new StrictOpenError("InvalidRequestShape", "admission", { index });
    }
    const raw_url = url_descriptor.value;
    if (strict_utf8_byte_length(raw_url) > STRICT_OPEN_MAX_FIELD_BYTES) {
      throw new StrictOpenError("ResourceLimitExceeded", "admission", { index });
    }

    let parsed: URL;
    try {
      parsed = new URL(raw_url);
    } catch {
      throw new StrictOpenError("InvalidUrl", "admission", { index });
    }
    if ((parsed.protocol !== "http:" && parsed.protocol !== "https:") || parsed.username || parsed.password) {
      throw new StrictOpenError("UnsupportedStrictOpenRoute", "admission", { index });
    }

    const options_descriptor = Object.getOwnPropertyDescriptor(request, "options");
    const options_value = options_descriptor?.value;
    if (options_descriptor && !("value" in options_descriptor)) {
      throw new StrictOpenError("InvalidRequestShape", "admission", { index });
    }
    if (options_value !== undefined &&
        (options_value === null || typeof options_value !== "object" || Array.isArray(options_value))) {
      throw new StrictOpenError("InvalidRequestShape", "admission", { index });
    }
    const options = (options_value ?? {}) as Record<string, unknown>;
    if (Object.getPrototypeOf(options) !== Object.prototype && Object.getPrototypeOf(options) !== null) {
      throw new StrictOpenError("InvalidRequestShape", "admission", { index });
    }
    const known_options = new Set([
      "topic_filter",
      "decoder_allowlist_version",
      "assignment_policy_version",
      "mcap_time_type",
      "representation_consistency",
      "recording_open_behavior",
      "allow_extensionless_sniff",
    ]);
    if (Reflect.ownKeys(options).some((key) => typeof key !== "string" || !known_options.has(key))) {
      throw new StrictOpenError("InvalidRequestShape", "admission", { index });
    }
    const option = (name: string, fallback: unknown) => {
      const descriptor = Object.getOwnPropertyDescriptor(options, name);
      if (descriptor && !("value" in descriptor)) {
        throw new StrictOpenError("InvalidRequestShape", "admission", { index });
      }
      return descriptor?.value ?? fallback;
    };

    const topic_value = option("topic_filter", []);
    if (typeof topic_value === "string") {
      if (strict_utf8_byte_length(topic_value) > 4_096) {
        throw new StrictOpenError("InvalidRequestShape", "admission", { index });
      }
    } else if (Array.isArray(topic_value)) {
      if (topic_value.length > STRICT_OPEN_MAX_TOPIC_FILTERS) {
        throw new StrictOpenError("InvalidRequestShape", "admission", { index });
      }
      let topic_filter_bytes = 0;
      for (let topic_index = 0; topic_index < topic_value.length; topic_index++) {
        const topic_descriptor = Object.getOwnPropertyDescriptor(topic_value, String(topic_index));
        if (!topic_descriptor || !("value" in topic_descriptor)
          || typeof topic_descriptor.value !== "string"
          || strict_utf8_byte_length(topic_descriptor.value) > 4_096) {
          throw new StrictOpenError("InvalidRequestShape", "admission", { index });
        }
        const topic_bytes = strict_utf8_byte_length(topic_descriptor.value);
        if (topic_bytes > STRICT_OPEN_MAX_FIELD_BYTES - topic_filter_bytes) {
          throw new StrictOpenError("ResourceLimitExceeded", "admission", { index });
        }
        topic_filter_bytes += topic_bytes;
      }
    } else {
      throw new StrictOpenError("InvalidRequestShape", "admission", { index });
    }

    strict_decimal_option(
      option("decoder_allowlist_version", "1"),
      index,
    );
    strict_decimal_option(
      option("assignment_policy_version", "1"),
      index,
    );
    strict_option_value(
      option("mcap_time_type", "timestamp_ns"),
      ["timestamp_ns", "duration_ns"] as const,
      index,
    );
    strict_option_value(
      option("representation_consistency", "require_strong_validator"),
      ["require_strong_validator", "allow_deployment_assumed"] as const,
      index,
    );
    strict_option_value(
      option("recording_open_behavior", "open_and_select"),
      ["open", "open_and_select", "background"] as const,
      index,
    );
    const allow_extensionless = option("allow_extensionless_sniff", false);
    if (typeof allow_extensionless !== "boolean") {
      throw new StrictOpenError("InvalidRequestShape", "admission", { index });
    }

    const explicit_mcap = parsed.pathname.toLowerCase().endsWith(".mcap");
    const last_segment = parsed.pathname.slice(parsed.pathname.lastIndexOf("/") + 1);
    if (!explicit_mcap && last_segment.includes(".")) {
      throw new StrictOpenError("UnsupportedFormat", "admission", { index });
    }
    if (!explicit_mcap && !allow_extensionless) {
      throw new StrictOpenError("UnsupportedFormat", "admission", { index });
    }
    return { route: explicit_mcap ? "explicit_mcap" : "extensionless_sniff" };
  } catch (error) {
    if (error instanceof StrictOpenError) throw error;
    throw new StrictOpenError("InvalidRequestShape", "admission", { index });
  }
}

/**
 * Rerun Web Viewer
 *
 * ```ts
 * const viewer = new WebViewer();
 * await viewer.start();
 * ```
 *
 * Data may be provided to the Viewer as:
 * - An HTTP file URL, e.g. `viewer.start("https://app.rerun.io/version/0.35.0/examples/dna.rrd")`
 * - A Rerun gRPC URL, e.g. `viewer.start("rerun+http://127.0.0.1:9876/proxy")`
 * - A stream of log messages, via {@link WebViewer.open_channel}.
 *
 * Callbacks may be attached for various events using {@link WebViewer.on}:
 *
 * ```ts
 * viewer.on("time_update", (time) => console.log(`current time: {time}`));
 * ```
 *
 * For the full list of available events, see {@link ViewerEvent}.
 */
export class WebViewer {
  #id = randomId();
  // NOTE: Using the handle requires wrapping all calls to its methods in try/catch.
  //       On failure, call `this.stop` to prevent a memory leak, then re-throw the error.
  #handle: WebHandle | null = null;
  private _strict_dispatcher = new BoundedSingleTaskDispatcher();
  #strict_open_cache = new StrictOpenWrapperCache(
    (task) => this._strict_dispatcher.enqueue(task),
  );
  #canvas: HTMLCanvasElement | null = null;
  #loader: HTMLDivElement | null = null;
  #state: "ready" | "starting" | "stopped" = "stopped";
  #fullscreen = false;
  #allow_fullscreen = false;

  constructor() {
    injectStyle();
    setupGlobalEventListeners();
    const node_process = (globalThis as any).process;
    const test_state = (globalThis as any).__rerun_web_viewer_test_state;
    if (
      node_process?.release?.name === "node"
      && node_process?.env?.RERUN_WEB_VIEWER_TEST === "1"
      && test_state
      && typeof test_state === "object"
    ) {
      const caches = test_state.strict_open_caches ?? new WeakMap();
      test_state.strict_open_caches = caches;
      caches.set(this, this.#strict_open_cache);
    }
  }

  /**
   * Start the viewer.
   *
   * @param rrd URLs to `.rrd` files or gRPC connections to our SDK.
   * Compatibility startup URLs are dispatched in input order; a failed item emits a warning and
   * does not stop the viewer or prevent later items from being attempted.
   * @param parent The element to attach the canvas onto.
   * @param options Web Viewer configuration.
   */
  async start(
    rrd: string | string[] | null,
    parent: HTMLElement | null,
    options: WebViewerOptions | null,
  ): Promise<void> {
    parent ??= document.body;
    options ??= {};
    options = options ? { ...options } : options;

    this.#allow_fullscreen = options.allow_fullscreen || false;

    if (this.#state !== "stopped") return;
    this.#state = "starting";
    this.#clearLoader();

    this.#canvas = document.createElement("canvas");
    this.#canvas.style.width = options.width ?? "640px";
    this.#canvas.style.height = options.height ?? "360px";
    parent.append(this.#canvas);

    // Show loading progress bar
    this.#loader = document.createElement("div");
    this.#loader.innerHTML = `
      <div style="display: flex; flex-direction: column; align-items: center; justify-content: center; height: 100%; background-color: #1c1c1c; font-family: sans-serif; color: white;">
        <div style="margin-bottom: 16px;">Loading Rerun\u2026</div>
        <div style="width: 200px;">
          <div style="background: #333; border-radius: 4px; height: 6px; overflow: hidden;">
            <div class="rerun-progress-bar" style="background: white; height: 100%; width: 0%; transition: width 0.2s;"></div>
          </div>
          <div class="rerun-progress-text" style="margin-top: 6px; font-size: 12px; color: #999;"></div>
        </div>
      </div>
    `;
    this.#loader.style.position = "absolute";
    this.#loader.style.inset = "0";
    parent.style.position = "relative";
    parent.append(this.#loader);

    const progress_bar = this.#loader.querySelector(".rerun-progress-bar") as HTMLElement;
    const progress_text = this.#loader.querySelector(".rerun-progress-text") as HTMLElement;

    const on_progress = (received: number, total: number | null) => {
      if (total != null && total > 0) {
        const pct = Math.min((received / total) * 100, 100);
        progress_bar.style.width = pct.toFixed(1) + "%";
        progress_text.textContent = `${Math.round(pct)}%`;
      } else {
        progress_text.textContent = format_mib(received);
      }
    };

    // This yield appears to be necessary to ensure that the canvas is attached to the DOM
    // and visible. Without it we get occasionally get a panic about a failure to find a canvas
    // element with the given ID.
    await delay(0);

    let base_url: string | undefined = (options as any)?.base_url;
    if (base_url) {
      delete (options as any).base_url;
    }

    let WebHandle_class: typeof wasm_bindgen.WebHandle;
    try {
      WebHandle_class = await load(base_url, on_progress);
    } catch (e) {
      this.#clearLoader();
      this.#fail("Failed to load rerun", String(e));
      throw e;
    }
    if (this.#state !== "starting") {
      this.#clearLoader();
      return;
    }

    const fullscreen = this.#allow_fullscreen
      ? {
        get_state: () => this.#fullscreen,
        on_toggle: () => this.toggle_fullscreen(),
      }
      : undefined;

    const on_viewer_event = (event_json: string) => {
      // for notebooks/gradio, we can avoid a whole layer
      // of serde by sending over the raw json directly,
      // which will be deserialized in Python instead
      this.#dispatch_raw_event(event_json);

      // for JS users, we dispatch the parsed event
      let event: ViewerEvent = JSON.parse(event_json);
      this.#dispatch_event(
        event.type as any,
        event,
      );
    }

    const login = options.login
      ? {
          signed_in_url: resolveAbsoluteUrl(options.login.signed_in_url),
          signed_out_url: resolveAbsoluteUrl(options.login.signed_out_url),
        }
      : undefined;

    this.#handle = new WebHandle_class({
      ...options,
      login,
      fullscreen,
      on_viewer_event,
    });
    try {
      await this.#handle.start(this.#canvas);
    } catch (e) {
      this.#clearLoader();
      this.#fail("Failed to start", String(e));
      throw e;
    }
    if (this.#state !== "starting") {
      this.#clearLoader();
      return;
    }

    this.#clearLoader();
    this.#state = "ready";
    this.#dispatch_event("ready");

    if (rrd) {
      this.open(rrd);
    }

    let self = this;

    function check_for_panic() {
      if (self.#handle?.has_panicked()) {
        self.#fail("Rerun has crashed.", self.#handle?.panic_message());
      } else {
        let delay_ms = 1000;
        setTimeout(check_for_panic, delay_ms);
      }
    }

    check_for_panic();

    return;
  }

  #raw_events: Set<(event_json: string) => void> = new Set();
  #dispatch_raw_event(event_json: string) {
    for (const callback of this.#raw_events) {
      callback(event_json);
    }
  }

  /** Internal interface */
  // NOTE: Callbacks passed to this function must NOT invoke any viewer methods!
  //       The `setTimeout` is omitted to avoid the 1-tick delay, as it is unnecessary,
  //       because this is only meant to be used for sending events to Jupyter/Gradio.
  //
  // Do not change this without searching for grepping for usage!
  private _on_raw_event(callback: (event: string) => void): () => void {
    this.#raw_events.add(callback);
    return () => this.#raw_events.delete(callback);
  }

  #event_map: Map<
    keyof WebViewerEvents,
    Map<(...args: any[]) => void, { once: boolean }>
  > = new Map();

  #dispatch_event<E extends keyof EventsWithValue>(
    event: E,
    ...args: EventsWithValue[E]
  ): void;
  #dispatch_event<E extends keyof EventsWithoutValue>(event: E): void;
  #dispatch_event(event: any, ...args: any[]): void {
    // Dispatch events on next tick.
    // This is necessary because we may have been called somewhere deep within the viewer's call stack,
    // which means that `app` may be locked. The event will not actually be dispatched until the
    // full call stack has returned or the current task has yielded to the event loop. It does not
    // guarantee that we will be able to acquire the lock here, but it makes it a lot more likely.
    setTimeout(() => {
      const callbacks = this.#event_map.get(event);
      if (callbacks) {
        for (const [callback, { once }] of [...callbacks.entries()]) {
          callback(...args);
          if (once) callbacks.delete(callback);
        }
      }
    }, 0);
  }

  /**
   * Register an event listener.
   *
   * Returns a function which removes the listener when called.
   *
   * See {@link ViewerEvent} for a full list of available events.
   */
  on<E extends keyof EventsWithValue>(
    event: E,
    callback: (...args: EventsWithValue[E]) => void,
  ): () => void;
  on<E extends keyof EventsWithoutValue>(
    event: E,
    callback: () => void,
  ): () => void;
  on(event: any, callback: any): () => void {
    const callbacks = this.#event_map.get(event) ?? new Map();
    callbacks.set(callback, { once: false });
    this.#event_map.set(event, callbacks);
    return () => callbacks.delete(callback);
  }

  /**
   * Register an event listener which runs only once.
   *
   * Returns a function which removes the listener when called.
   *
   * See {@link ViewerEvent} for a full list of available events.
   */
  once<E extends keyof EventsWithValue>(
    event: E,
    callback: (value: EventsWithValue[E]) => void,
  ): () => void;
  once<E extends keyof EventsWithoutValue>(
    event: E,
    callback: () => void,
  ): () => void;
  once(event: any, callback: any): () => void {
    const callbacks = this.#event_map.get(event) ?? new Map();
    callbacks.set(callback, { once: true });
    this.#event_map.set(event, callbacks);
    return () => callbacks.delete(callback);
  }

  /**
   * Unregister an event listener.
   *
   * The event emitter relies on referential equality to store callbacks.
   * The `callback` passed in must be the exact same _instance_ of the function passed in to `on` or `once`.
   *
   * See {@link ViewerEvent} for a full list of available events.
   */
  off<E extends keyof EventsWithValue>(
    event: E,
    callback: (value: EventsWithValue[E]) => void,
  ): void;
  off<E extends keyof EventsWithoutValue>(event: E, callback: () => void): void;
  off(event: any, callback: any): void {
    const callbacks = this.#event_map.get(event);
    if (callbacks) {
      callbacks.delete(callback);
    } else {
      console.warn(
        "Attempted to call `WebViewer.off` with an unregistered callback. Are you passing in the same function instance?",
      );
    }
  }

  /**
   * The underlying canvas element.
   */
  get canvas() {
    return this.#canvas;
  }

  /**
   * Returns `true` if the viewer is ready to connect to data sources.
   */
  get ready() {
    return this.#state === "ready";
  }

  /**
   * Open a recording.
   *
   * The viewer must have been started via {@link WebViewer.start}.
   *
   * @param rrd URLs to `.rrd` files or gRPC connections to our SDK.
   * Each compatibility URL is attempted independently in input order.  Item failures emit a
   * warning and do not throw, stop the viewer, or roll back earlier or later items.
   */
  open(rrd: string | string[]) {
    if (!this.#handle) {
      throw new Error(`attempted to open \`${rrd}\` in a stopped viewer`);
    }

    const urls = Array.isArray(rrd) ? rrd : [rrd];
    for (const url of urls) {
      try {
        this.#handle.add_receiver(url);
      } catch (e) {
        // Compatibility open/start are deliberately per-item and non-throwing.  A failed item
        // reports a warning and leaves preceding and following inputs untouched.
        console.warn("Failed to open recording; continuing with the next item", e);
      }
    }
  }

  /**
   * Open one strict HTTP remote-MCAP request.
   *
   * This additive API validates the strict request and currently rejects with
   * `CapabilityUnavailable` until the measured Rust remote-MCAP capability is installed.
   * It never creates a wrapper, schedules work, enters the compatibility dispatcher, calls
   * {@link WebViewer.stop}, or invokes the instance failure UI for a request-local error.
   */
  async openRequest(spec: HttpOpenRequestSpec): Promise<OpenRequestHandle> {
    this._validate_strict_batch([spec]);
    throw new StrictOpenError("CapabilityUnavailable", "admission");
  }

  /**
   * Atomically prepare and release a batch of strict HTTP remote-MCAP requests.
   *
   * Every input is validated before the capability gate.  Until that gate is installed, the
   * operation rejects with a redacted {@link StrictOpenError} and leaves the cache, dispatcher,
   * and compatibility routes untouched.
   */
  async openBatch(specs: readonly HttpOpenRequestSpec[]): Promise<readonly OpenRequestHandle[]> {
    this._validate_strict_batch(specs);
    throw new StrictOpenError("CapabilityUnavailable", "admission");
  }

  private _validate_strict_batch(specs: unknown): void {
    if (this.#state !== "ready" || !this.#handle) {
      throw new StrictOpenError("ViewerStopped", "admission");
    }
    try {
      if (!Array.isArray(specs) || specs.length === 0) {
        throw new StrictOpenError("InvalidRequestShape", "admission");
      }
      if (specs.length > STRICT_OPEN_MAX_BATCH_ITEMS) {
        throw new StrictOpenError("BatchTooLarge", "admission");
      }
      for (let index = 0; index < specs.length; index++) {
        if (!Object.prototype.hasOwnProperty.call(specs, index)) {
          throw new StrictOpenError("InvalidRequestShape", "admission", { index });
        }
        normalize_strict_open_spec(specs[index], index);
      }
    } catch (error) {
      if (error instanceof StrictOpenError) throw error;
      throw new StrictOpenError("InvalidRequestShape", "admission");
    }
  }

  /**
   * Close a recording.
   *
   * The viewer must have been started via {@link WebViewer.start}.
   *
   * @param rrd URLs to `.rrd` files or gRPC connections to our SDK.
   */
  close(rrd: string | string[]) {
    if (!this.#handle) {
      throw new Error(`attempted to close \`${rrd}\` in a stopped viewer`);
    }

    const urls = Array.isArray(rrd) ? rrd : [rrd];
    for (const url of urls) {
      try {
        this.#handle.remove_receiver(url);
      } catch (e) {
        this.#fail("Failed to close recording", String(e));
        throw e;
      }
    }
  }

  /**
   * Stop the viewer, freeing all associated memory.
   *
   * The same viewer instance may be started multiple times.
   */
  stop() {
    if (this.#state === "stopped") return;
    if (this.#allow_fullscreen && this.#canvas && this.#fullscreen) {
      this.#minimize();
    }

    this.#state = "stopped";
    // Strict remote handles retain their opaque identity across a Viewer restart, but all
    // controls become terminally stopped and can never redirect to a later publication.
    this.#strict_open_cache.mark_viewer_stopped();

    this.#canvas?.remove();
    this.#clearLoader();

    try {
      this.#handle?.destroy();
      this.#handle?.free();
    } catch (e) {
      this.#handle = null;
      throw e;
    }

    this.#canvas = null;
    this.#handle = null;
    this.#loader = null;
    this.#fullscreen = false;
    this.#allow_fullscreen = false;
  }

  #fail(message: string, error_message?: string) {
    console.error("WebViewer failure:", message, error_message);
    if (this.canvas?.parentElement) {
      const parent = this.canvas.parentElement;
      parent.innerHTML = `
        <div style="display: flex; flex-direction: column; align-items: center; justify-content: center; height: 100%; color: white; font-family: sans-serif; background-color: #1c1c1c;">
          <h1 class="rerun-fail-message"></h1>
          <pre class="rerun-fail-error" style="text-align: left; white-space: pre-wrap; word-break: break-word; max-width: 90vw;"></pre>
          <button class="rerun-fail-clear-cache">Clear caches and reload</button>
        </div>
      `;

      parent.querySelector(".rerun-fail-message")!.textContent = message;

      const errorEl = parent.querySelector(".rerun-fail-error")!;
      if (error_message) {
        errorEl.textContent = error_message;
      } else {
        errorEl.remove();
      }

      parent.querySelector(".rerun-fail-clear-cache")!.addEventListener("click", async () => {
        if ("caches" in window) {
          const keys = await caches.keys();
          await Promise.all(keys.map((key) => caches.delete(key)));
        }
        window.location.reload();
      });
    }

    this.stop();
  }

  #clearLoader() {
    this.#loader?.remove();
    this.#loader = null;
  }

  /**
   * Opens a new channel for sending log messages.
   *
   * The channel can be used to incrementally push `rrd` chunks into the viewer.
   *
   * @param channel_name used to identify the channel.
   */
  open_channel(channel_name: string = "rerun-io/web-viewer"): LogChannel {
    if (!this.#handle) {
      throw new Error(
        `attempted to open channel \"${channel_name}\" in a stopped web viewer`,
      );
    }

    const id = crypto.randomUUID();

    try {
      this.#handle.open_channel(id, channel_name);
    } catch (e) {
      this.#fail("Failed to open channel", String(e));
      throw e;
    }

    const on_send = (/** @type {Uint8Array} */ data: Uint8Array) => {
      if (!this.#handle) {
        throw new Error(
          `attempted to send data through channel \"${channel_name}\" to a stopped web viewer`,
        );
      }

      try {
        this.#handle.send_rrd_to_channel(id, data);
      } catch (e) {
        this.#fail("Failed to send data", String(e));
        throw e;
      }
    };

    const on_send_table = (/** @type {Uint8Array} */ data: Uint8Array) => {
      if (!this.#handle) {
        throw new Error(
          `attempted to send data through channel \"${channel_name}\" to a stopped web viewer`,
        );
      }

      try {
        this.#handle.send_table_to_channel(id, data);
      } catch (e) {
        this.#fail("Failed to send table", String(e));
        throw e;
      }
    }

    const on_close = () => {
      if (!this.#handle) {
        throw new Error(
          `attempted to send data through channel \"${channel_name}\" to a stopped web viewer`,
        );
      }

      try {
        this.#handle.close_channel(id);
      } catch (e) {
        this.#fail("Failed to close channel", String(e));
        throw e;
      }
    };

    const get_state = () => this.#state;

    return new LogChannel(on_send, on_send_table, on_close, get_state);
  }

  /**
   * Force a panel to a specific state.
   *
   * @param panel which panel to configure
   * @param state which state to force the panel into
   */
  override_panel_state(panel: Panel, state: PanelState | undefined | null) {
    if (!this.#handle) {
      throw new Error(
        `attempted to set ${panel} panel to ${state} in a stopped web viewer`,
      );
    }

    try {
      this.#handle.override_panel_state(panel, state);
    } catch (e) {
      this.#fail("Failed to override panel state", String(e));
      throw e;
    }
  }

  /**
   * Toggle panel overrides set via `override_panel_state`.
   *
   * @param value set to a specific value. Toggles the previous value if not provided.
   */
  toggle_panel_overrides(value?: boolean | null) {
    if (!this.#handle) {
      throw new Error(
        `attempted to toggle panel overrides in a stopped web viewer`,
      );
    }

    try {
      this.#handle.toggle_panel_overrides(value as boolean | undefined);
    } catch (e) {
      this.#fail("Failed to toggle panel overrides", String(e));
      throw e;
    }
  }

  /**
   * Get the active recording id.
   */
  get_active_recording_id(): string | null {
    if (!this.#handle) {
      throw new Error(
        `attempted to get active recording id in a stopped web viewer`,
      );
    }

    return this.#handle.get_active_recording_id() ?? null;
  }

  /**
   * Set the active recording id.
   *
   * This is the same as clicking on the recording in the Viewer's left panel.
   */
  set_active_recording_id(value: string) {
    if (!this.#handle) {
      throw new Error(
        `attempted to set active recording id to ${value} in a stopped web viewer`,
      );
    }

    this.#handle.set_active_recording_id(value);
  }

  /**
   * Get the play state.
   *
   * This always returns `false` if the recording can't be found.
   */
  get_playing(recording_id: string): boolean {
    if (!this.#handle) {
      throw new Error(`attempted to get play state in a stopped web viewer`);
    }

    return this.#handle.get_playing(recording_id) || false;
  }

  /**
   * Set the play state.
   *
   * This does nothing if the recording can't be found.
   */
  set_playing(recording_id: string, value: boolean) {
    if (!this.#handle) {
      throw new Error(
        `attempted to set play state to ${value ? "playing" : "paused"
        } in a stopped web viewer`,
      );
    }

    this.#handle.set_playing(recording_id, value);
  }

  /**
   * Get the current time.
   *
   * The interpretation of time depends on what kind of timeline it is:
   *
   * - For time timelines, this is the time in nanoseconds.
   * - For sequence timelines, this is the sequence number.
   *
   * This always returns `0` if the recording or timeline can't be found.
   */
  get_current_time(recording_id: string, timeline: string): number {
    if (!this.#handle) {
      throw new Error(`attempted to get current time in a stopped web viewer`);
    }

    return this.#handle.get_time_for_timeline(recording_id, timeline) || 0;
  }

  /**
   * Set the current time.
   *
   * Equivalent to clicking on the timeline in the time panel at the specified `time`.
   * The interpretation of `time` depends on what kind of timeline it is:
   *
   * - For time timelines, this is the time in nanoseconds.
   * - For sequence timelines, this is the sequence number.
   *
   * This does nothing if the recording or timeline can't be found.
   */
  set_current_time(recording_id: string, timeline: string, time: number) {
    if (!this.#handle) {
      throw new Error(
        `attempted to set current time to ${time} in a stopped web viewer`,
      );
    }

    this.#handle.set_time_for_timeline(recording_id, timeline, time);
  }

  /**
   * Get the active timeline.
   *
   * This always returns `null` if the recording can't be found.
   */
  get_active_timeline(recording_id: string): string | null {
    if (!this.#handle) {
      throw new Error(
        `attempted to get active timeline in a stopped web viewer`,
      );
    }

    return this.#handle.get_active_timeline(recording_id) ?? null;
  }

  /**
   * Set the active timeline.
   *
   * This does nothing if the recording or timeline can't be found.
   */
  set_active_timeline(recording_id: string, timeline: string) {
    if (!this.#handle) {
      throw new Error(
        `attempted to set active timeline to ${timeline} in a stopped web viewer`,
      );
    }

    this.#handle.set_active_timeline(recording_id, timeline);
  }

  /**
   * Get the time range for a timeline.
   *
   * This always returns `null` if the recording or timeline can't be found.
   */
  get_time_range(
    recording_id: string,
    timeline: string,
  ): { min: number; max: number } | null {
    if (!this.#handle) {
      throw new Error(`attempted to get time range in a stopped web viewer`);
    }

    return this.#handle.get_timeline_time_range(recording_id, timeline);
  }

  /**
   * Toggle fullscreen mode.
   *
   * This does nothing if `allow_fullscreen` was not set to `true` when starting the viewer.
   *
   * Fullscreen mode works by updating the underlying `<canvas>` element's `style`:
   * - `position` to `fixed`
   * - width/height/top/left to cover the entire viewport
   *
   * When fullscreen mode is toggled off, the style is restored to its previous values.
   *
   * When fullscreen mode is toggled on, any other instance of the viewer on the page
   * which is already in fullscreen mode is toggled off. This means that it doesn't
   * have to be tracked manually.
   *
   * This functionality can also be directly accessed in the viewer:
   * - The maximize/minimize top panel button
   * - The `Toggle fullscreen` UI command (accessible via the command palette, CTRL+P)
   */
  toggle_fullscreen() {
    if (!this.#allow_fullscreen) return;

    if (!this.#handle || !this.#canvas) {
      throw new Error(
        `attempted to toggle fullscreen mode in a stopped web viewer`,
      );
    }

    if (this.#fullscreen) {
      this.#minimize();
    } else {
      this.#maximize();
    }
  }

  set_credentials(access_token: string, email: string) {
    if (!this.#handle) {
      throw new Error(
        `attempted to set credentials in a stopped web viewer`,
      );
    }
    this.#handle.set_credentials(access_token, email);
  }



  #minimize = () => { };

  #maximize = () => {
    _minimize_current_fullscreen_viewer?.();

    const canvas = this.#canvas!;
    const rect = canvas.getBoundingClientRect();

    const sync_style_to_rect = () => {
      canvas.style.left = rect.left + "px";
      canvas.style.top = rect.top + "px";
      canvas.style.width = rect.width + "px";
      canvas.style.height = rect.height + "px";
    };
    const undo_style = () => canvas.removeAttribute("style");
    const transition = (callback: () => void) =>
      setTimeout(() => requestAnimationFrame(callback), transition_delay_ms);

    canvas.classList.add(classes.fullscreen_base, classes.fullscreen_rect);
    sync_style_to_rect();
    requestAnimationFrame(() => {
      if (!this.#fullscreen) return;
      canvas.classList.add(classes.transition);
      transition(() => {
        if (!this.#fullscreen) return;
        undo_style();

        document.body.classList.add(classes.hide_scrollbars);
        document.documentElement.classList.add(classes.hide_scrollbars);
        this.#dispatch_event("fullscreen", true);
      });
    });

    this.#minimize = () => {
      document.body.classList.remove(classes.hide_scrollbars);
      document.documentElement.classList.remove(classes.hide_scrollbars);

      sync_style_to_rect();
      canvas.classList.remove(classes.fullscreen_rect);
      transition(() => {
        if (this.#fullscreen) return;

        undo_style();
        canvas.classList.remove(classes.fullscreen_base, classes.transition);
      });

      _minimize_current_fullscreen_viewer = null;
      this.#fullscreen = false;
      this.#dispatch_event("fullscreen", false);
    };

    _minimize_current_fullscreen_viewer = () => this.#minimize();
    this.#fullscreen = true;
  };
}

export class LogChannel {
  #on_send;
  #on_send_table;
  #on_close;
  #get_state;
  #closed = false;

  /**
   * @param on_send
   * @param on_close
   * @param get_state
   */
  constructor(
    on_send: (data: Uint8Array) => void,
    on_send_table: (data: Uint8Array) => void,
    on_close: () => void,
    get_state: () => "ready" | "starting" | "stopped",
  ) {
    this.#on_send = on_send;
    this.#on_send_table = on_send_table;
    this.#on_close = on_close;
    this.#get_state = get_state;
  }

  get ready() {
    return !this.#closed && this.#get_state() === "ready";
  }

  /**
   * Send an `rrd` containing log messages to the viewer.
   *
   * Does nothing if `!this.ready`.
   *
   * @param rrd_bytes Is an rrd file stored in a byte array, received via some other side channel.
   */
  send_rrd(rrd_bytes: Uint8Array) {
    if (!this.ready) return;
    this.#on_send(rrd_bytes);
  }

  send_table(table_bytes: Uint8Array) {
    if (!this.ready) return;
    this.#on_send_table(table_bytes)
  }

  /**
   * Close the channel.
   *
   * Does nothing if `!this.ready`.
   */
  close() {
    if (!this.ready) return;
    this.#on_close();
    this.#closed = true;
  }
}

type StrictOpenRecordingPhase = "preexisting" | "active" | "completed";

type StrictRecordingControlAdapter = {
  select: (identity: StrictPublicRecordingHandleId) => StrictRecordingControlResult;
  seek: (identity: StrictPublicRecordingHandleId, target: StrictRecordingSeekTarget) => StrictRecordingControlResult;
  play: (identity: StrictPublicRecordingHandleId, value: "paused" | "playing") => StrictRecordingControlResult;
  close: (identity: StrictPublicRecordingHandleId) => StrictRecordingControlResult;
  dispose: () => void;
};

type StrictOperationControlAdapter = {
  close: () => StrictRecordingControlResult;
  dispose: () => void;
};

type StrictFinalizationToken = {
  run: () => void;
};

// Internal lifecycle transitions are symbol-keyed so an opaque public handle does not expose
// string-named constructors, identity mutators, or Viewer-stop hooks.
const strict_internal_viewer_stopped = Symbol("strict_internal_viewer_stopped");
const strict_internal_attach_recording = Symbol("strict_internal_attach_recording");
const strict_internal_recording_phase = Symbol("strict_internal_recording_phase");
const strict_internal_install_ack = Symbol("strict_internal_install_ack");
const strict_internal_arm_bridge = Symbol("strict_internal_arm_bridge");
const strict_internal_operation_closed = Symbol("strict_internal_operation_closed");
const strict_internal_recording_removed = Symbol("strict_internal_recording_removed");
const strict_internal_operation_state = Symbol("strict_internal_operation_state");
const strict_internal_transition = Symbol("strict_internal_transition");

const strict_recording_identity_keys = new WeakMap<StrictPublicRecordingHandleId, string>();

class StrictStorePublicationIdentity {
  #generation: string;

  constructor(generation: string) {
    this.#generation = generation;
  }
}

class StrictPublicRecordingHandleId {
  #recording_key: string;
  #publication: StrictStorePublicationIdentity;

  constructor(recording_key: string, generation: string) {
    this.#recording_key = recording_key;
    this.#publication = new StrictStorePublicationIdentity(generation);
    strict_recording_identity_keys.set(
      this,
      `${generation.length}:${generation}${recording_key.length}:${recording_key}`,
    );
  }
}

function strict_recording_identity_key(identity: StrictPublicRecordingHandleId): string {
  return strict_recording_identity_keys.get(identity)!;
}

const strict_open_finalization_registry: FinalizationRegistry<StrictFinalizationToken> | null =
  typeof FinalizationRegistry === "function"
    ? new FinalizationRegistry((token) => token.run())
    : null;

function strict_control_result(
  command: StrictRecordingControlCommand,
  code: StrictRecordingControlCode,
): StrictRecordingControlResult {
  return Object.freeze({
    command,
    code,
    accepted: code === "accepted",
  });
}

function strict_unavailable_result(
  command: StrictRecordingControlCommand,
): StrictRecordingControlResult {
  return strict_control_result(command, "capability_unavailable");
}

const STRICT_REMOTE_TIME_MAX = "9223372036854775807";

function is_canonical_remote_time_value(value: string): boolean {
  const negative = value.startsWith("-");
  const magnitude = negative ? value.slice(1) : value;
  if (!/^(?:0|[1-9][0-9]{0,18})$/.test(magnitude)) return false;
  if (negative && magnitude === "0") return false;
  return magnitude.length < STRICT_REMOTE_TIME_MAX.length
    || (magnitude.length === STRICT_REMOTE_TIME_MAX.length && magnitude <= STRICT_REMOTE_TIME_MAX);
}

function normalize_strict_seek_target(target: unknown): StrictRecordingSeekTarget | null {
  try {
    if (target === null || typeof target !== "object" || Array.isArray(target)) return null;
    const record = target as Record<string, unknown>;
    const prototype = Object.getPrototypeOf(record);
    if (prototype !== Object.prototype && prototype !== null) return null;
    const keys = Reflect.ownKeys(record);
    if (keys.length !== 2 || keys.some((key) =>
      typeof key !== "string" || (key !== "time_type" && key !== "value"))) {
      return null;
    }
    const type_descriptor = Object.getOwnPropertyDescriptor(record, "time_type");
    const value_descriptor = Object.getOwnPropertyDescriptor(record, "value");
    if (!type_descriptor || !("value" in type_descriptor)
      || !value_descriptor || !("value" in value_descriptor)) return null;
    const time_type = type_descriptor.value;
    const value = value_descriptor.value;
    if (time_type !== "timestamp_ns" && time_type !== "duration_ns") return null;
    if (typeof value !== "string" || !is_canonical_remote_time_value(value)) return null;
    return Object.freeze({ time_type, value });
  } catch {
    return null;
  }
}

class StrictOpenRecordingWrapper {
  #identity: StrictPublicRecordingHandleId;
  #phase: StrictOpenRecordingPhase;
  #disposed = false;
  #closed = false;
  #operation_closed = false;
  #recording_removed = false;
  #viewer_stopped = false;
  #adapter: StrictRecordingControlAdapter | null;
  #finalization_token: StrictFinalizationToken;

  constructor(
    identity: StrictPublicRecordingHandleId,
    alias_kind: StrictOpenRecordingPhase,
    adapter: StrictRecordingControlAdapter | null = null,
    on_dispose: (() => void) | null = null,
  ) {
    this.#identity = identity;
    this.#phase = alias_kind;
    this.#adapter = adapter;
    const cleanup_state = {
      disposed: false,
      adapter,
      on_dispose,
    };
    const cleanup = () => {
      if (cleanup_state.disposed) return;
      cleanup_state.disposed = true;
      const cleanup_adapter = cleanup_state.adapter;
      const cleanup_parent = cleanup_state.on_dispose;
      cleanup_state.adapter = null;
      cleanup_state.on_dispose = null;
      try {
        cleanup_adapter?.dispose();
      } finally {
        cleanup_parent?.();
      }
    };
    this.#finalization_token = { run: cleanup };
    strict_open_finalization_registry?.register(this, this.#finalization_token, this.#finalization_token);
  }

  get phase() {
    return this.#phase;
  }

  get disposed() {
    return this.#disposed;
  }

  [strict_internal_viewer_stopped]() {
    this.#viewer_stopped = true;
  }

  [strict_internal_operation_closed]() {
    this.#operation_closed = true;
  }

  [strict_internal_recording_removed]() {
    this.#recording_removed = true;
  }

  [strict_internal_recording_phase](phase: StrictOpenRecordingPhase) {
    const next = phase;
    if (next === "active") {
      this.#phase = StrictOpenRecordingWrapper.#merge_phase(this.#phase, "active");
    } else if (next === "completed") {
      this.#phase = StrictOpenRecordingWrapper.#merge_phase(this.#phase, "completed");
    }
  }

  select(): StrictRecordingControlResult {
    if (this.#disposed) return strict_control_result("select", "disposed");
    if (this.#recording_removed) return strict_control_result("select", "recording_removed");
    if (this.#operation_closed) return strict_control_result("select", "operation_closed");
    if (this.#viewer_stopped) return strict_control_result("select", "viewer_stopped");
    if (this.#closed) return strict_control_result("select", "recording_closed");
    return this.#adapter?.select(this.#identity) ?? strict_unavailable_result("select");
  }

  seek(target: StrictRecordingSeekTarget): StrictRecordingControlResult {
    if (this.#disposed) return strict_control_result("seek", "disposed");
    if (this.#recording_removed) return strict_control_result("seek", "recording_removed");
    if (this.#operation_closed) return strict_control_result("seek", "operation_closed");
    if (this.#viewer_stopped) return strict_control_result("seek", "viewer_stopped");
    if (this.#closed) return strict_control_result("seek", "recording_closed");
    const normalized = normalize_strict_seek_target(target);
    if (!normalized) return strict_control_result("seek", "invalid_request");
    return this.#adapter?.seek(this.#identity, normalized) ?? strict_unavailable_result("seek");
  }

  play(value: "paused" | "playing"): StrictRecordingControlResult {
    if (this.#disposed) return strict_control_result("play", "disposed");
    if (this.#recording_removed) return strict_control_result("play", "recording_removed");
    if (this.#operation_closed) return strict_control_result("play", "operation_closed");
    if (this.#viewer_stopped) return strict_control_result("play", "viewer_stopped");
    if (this.#closed) return strict_control_result("play", "recording_closed");
    if (value !== "paused" && value !== "playing") {
      return strict_control_result("play", "invalid_request");
    }
    return this.#adapter?.play(this.#identity, value) ?? strict_unavailable_result("play");
  }

  close(): StrictRecordingControlResult {
    if (this.#disposed) return strict_control_result("close", "disposed");
    if (this.#recording_removed) return strict_control_result("close", "recording_removed");
    if (this.#operation_closed) return strict_control_result("close", "operation_closed");
    if (this.#viewer_stopped) return strict_control_result("close", "viewer_stopped");
    if (this.#closed) return strict_control_result("close", "recording_closed");
    const result = this.#adapter?.close(this.#identity) ?? strict_unavailable_result("close");
    if (result.accepted || result.code === "recording_removed") this.#closed = true;
    return result;
  }

  dispose() {
    if (this.#disposed) return;
    this.#disposed = true;
    this.#adapter = null;
    strict_open_finalization_registry?.unregister(this.#finalization_token);
    this.#finalization_token.run();
  }

  static #merge_phase(
    current: StrictOpenRecordingPhase,
    next: StrictOpenRecordingPhase,
  ): StrictOpenRecordingPhase {
    if (current === "completed" || current === next) {
      return current;
    }
    if (current === "active") {
      return next === "completed" ? "completed" : current;
    }
    return next;
  }
}

type StrictOpenRecordingCacheEntry = {
  readonly wrapper: StrictOpenRecordingWrapper;
  readonly token: object;
};

class StrictOpenOperationWrapper {
  #recordings = new Map<string, StrictOpenRecordingCacheEntry>();
  #recordings_cache: readonly StrictOpenRecordingWrapper[] | null = null;
  #installation_ack_complete = false;
  #activation_bridge_armed = false;
  #phase: StrictOpenLifecycleEvent = "accepted";
  #recording_open_behavior: RemoteMcapOpenBehavior = "open_and_select";
  #disposed = false;
  #closed = false;
  #viewer_stopped = false;
  #adapter: StrictOperationControlAdapter | null;
  #listeners = new Map<StrictOpenLifecycleEvent, Set<StrictOpenLifecycleListener>>();
  #finalization_token: StrictFinalizationToken;
  #schedule: (task: StrictDispatcherTask) => boolean;

  constructor(
    _operation_key: string,
    adapter: StrictOperationControlAdapter | null = null,
    phase: StrictOpenLifecycleEvent = "accepted",
    recording_open_behavior: RemoteMcapOpenBehavior = "open_and_select",
    on_dispose: (() => void) | null = null,
    schedule: (task: StrictDispatcherTask) => boolean = () => false,
  ) {
    this.#adapter = adapter;
    this.#phase = phase;
    this.#closed = phase === "terminal" || phase === "removed";
    this.#recording_open_behavior = recording_open_behavior;
    this.#schedule = schedule;
    const cleanup_state = {
      disposed: false,
      adapter,
      on_dispose,
    };
    const cleanup = () => {
      if (cleanup_state.disposed) return;
      cleanup_state.disposed = true;
      const cleanup_adapter = cleanup_state.adapter;
      const cleanup_parent = cleanup_state.on_dispose;
      cleanup_state.adapter = null;
      cleanup_state.on_dispose = null;
      try {
        cleanup_adapter?.dispose();
      } finally {
        cleanup_parent?.();
      }
    };
    this.#finalization_token = { run: cleanup };
    strict_open_finalization_registry?.register(this, this.#finalization_token, this.#finalization_token);
  }

  get phase() {
    return this.#phase;
  }

  get recordingOpenBehavior() {
    return this.#recording_open_behavior;
  }

  get disposed() {
    return this.#disposed;
  }

  [strict_internal_viewer_stopped]() {
    this.#viewer_stopped = true;
    for (const entry of this.#recordings.values()) {
      entry.wrapper[strict_internal_viewer_stopped]();
    }
  }

  get recordings() {
    if (!this.#recordings_cache) {
      this.#recordings_cache = Object.freeze(
        [...this.#recordings.values()].map((entry) => entry.wrapper),
      );
    }
    return this.#recordings_cache;
  }

  [strict_internal_attach_recording](
    identity: StrictPublicRecordingHandleId,
    alias_kind: StrictOpenRecordingPhase,
    adapter: StrictRecordingControlAdapter | null,
  ) {
    if (this.#disposed || this.#closed || this.#viewer_stopped) return null;
    return this.#attach_recording(identity, alias_kind, adapter);
  }

  [strict_internal_install_ack]() {
    if (this.#disposed || this.#closed || this.#viewer_stopped) return false;
    this.#installation_ack_complete = true;
    return true;
  }

  [strict_internal_arm_bridge]() {
    if (this.#disposed || this.#closed || this.#viewer_stopped) return false;
    this.#activation_bridge_armed = true;
    return true;
  }

  [strict_internal_operation_state]() {
    return {
      installation_ack_complete: this.#installation_ack_complete,
      activation_bridge_armed: this.#activation_bridge_armed,
    };
  }

  [strict_internal_transition](phase: StrictOpenLifecycleEvent) {
    if (
      this.#disposed
      || this.#viewer_stopped
      || this.#phase === "removed"
      || (this.#closed && phase !== "removed")
    ) return false;
    if (phase === "terminal" || phase === "removed") {
      this.#closed = true;
      for (const entry of this.#recordings.values()) {
        if (phase === "removed") {
          entry.wrapper[strict_internal_recording_removed]();
        } else {
          entry.wrapper[strict_internal_operation_closed]();
        }
      }
    }
    this.#phase = phase;
    const listeners = [...(this.#listeners.get(phase) ?? [])];
    if (listeners.length > 0) {
      const handle = this as unknown as OpenRequestHandle;
      return this.#schedule(() => {
        if (this.#disposed) return;
        for (const listener of listeners) listener(handle);
      });
    }
    return true;
  }

  on(event: StrictOpenLifecycleEvent, listener: StrictOpenLifecycleListener): () => void {
    if (this.#disposed) return () => undefined;
    const listeners = this.#listeners.get(event) ?? new Set();
    listeners.add(listener);
    this.#listeners.set(event, listeners);
    return () => listeners.delete(listener);
  }

  close(): StrictRecordingControlResult {
    if (this.#disposed) return strict_control_result("close", "disposed");
    if (this.#closed) return strict_control_result("close", "operation_closed");
    if (this.#viewer_stopped) return strict_control_result("close", "viewer_stopped");
    const result = this.#adapter?.close() ?? strict_unavailable_result("close");
    if (result.accepted || result.code === "operation_closed" || result.code === "recording_removed") {
      this.#closed = true;
      for (const entry of this.#recordings.values()) {
        entry.wrapper[strict_internal_operation_closed]();
      }
    }
    return result;
  }

  dispose() {
    if (this.#disposed) return;
    this.#disposed = true;
    this.#listeners.clear();
    for (const entry of this.#recordings.values()) entry.wrapper.dispose();
    this.#recordings.clear();
    this.#recordings_cache = null;
    this.#installation_ack_complete = false;
    this.#activation_bridge_armed = false;
    this.#adapter = null;
    strict_open_finalization_registry?.unregister(this.#finalization_token);
    this.#finalization_token.run();
  }

  #attach_recording(
    identity: StrictPublicRecordingHandleId,
    alias_kind: StrictOpenRecordingPhase,
    adapter: StrictRecordingControlAdapter | null,
  ) {
    const identity_key = strict_recording_identity_key(identity);
    const existing_entry = this.#recordings.get(identity_key);
    const existing = existing_entry?.wrapper;
    if (existing) {
      if (alias_kind === "active") {
        existing[strict_internal_recording_phase]("active");
      } else if (alias_kind === "completed") {
        existing[strict_internal_recording_phase]("completed");
      }
      return existing;
    }

    const cache_token = {};
    const operation_ref = new WeakRef(this);
    const wrapper = new StrictOpenRecordingWrapper(
      identity,
      alias_kind,
      adapter,
      () => {
        const operation = operation_ref.deref();
        if (operation) operation.#delete_recording(identity_key, cache_token);
      },
    );
    this.#recordings.set(identity_key, {
      wrapper,
      token: cache_token,
    });
    this.#recordings_cache = null;
    return wrapper;
  }

  #delete_recording(identity_key: string, token: object) {
    if (this.#recordings.get(identity_key)?.token === token) {
      this.#recordings.delete(identity_key);
      this.#recordings_cache = null;
    }
  }
}

type StrictOpenOperationCacheEntry = {
  readonly reference: WeakRef<StrictOpenOperationWrapper>;
  readonly token: object;
};

class StrictOpenWrapperCache {
  #operations = new Map<string, StrictOpenOperationCacheEntry>();
  #schedule: (task: StrictDispatcherTask) => boolean;

  constructor(schedule: (task: StrictDispatcherTask) => boolean = () => false) {
    this.#schedule = schedule;
  }

  get operation_count() {
    this.#drop_dead_operations();
    return this.#operations.size;
  }

  get_operation(operation_id: string) {
    return this.#get_operation(operation_id);
  }

  recording_count(operation_id: string) {
    return this.#get_operation(operation_id)?.recordings.length ?? 0;
  }

  wrapper_count(operation_id: string) {
    return this.recording_count(operation_id);
  }

  installation_ack_complete(operation_id: string) {
    return this.#get_operation(operation_id)?.[strict_internal_operation_state]()
      .installation_ack_complete ?? false;
  }

  activation_bridge_armed(operation_id: string) {
    return this.#get_operation(operation_id)?.[strict_internal_operation_state]()
      .activation_bridge_armed ?? false;
  }

  install_operation(
    operation_id: string,
    adapter: StrictOperationControlAdapter | null = null,
    phase: StrictOpenLifecycleEvent = "accepted",
    recording_open_behavior: RemoteMcapOpenBehavior = "open_and_select",
  ) {
    const existing = this.#get_operation(operation_id);
    if (existing) {
      return existing;
    }

    const cache_token = {};
    const operation = new StrictOpenOperationWrapper(
      operation_id,
      adapter,
      phase,
      recording_open_behavior,
      () => this.#delete_operation(operation_id, cache_token),
      this.#schedule,
    );
    this.#operations.set(operation_id, {
      reference: new WeakRef(operation),
      token: cache_token,
    });
    return operation;
  }

  attach_preexisting_recording(
    operation_id: string,
    recording_key: string,
    generation = "test-generation",
    adapter: StrictRecordingControlAdapter | null = null,
  ) {
    const operation = this.#get_operation(operation_id);
    return operation?.[strict_internal_attach_recording](
      new StrictPublicRecordingHandleId(recording_key, generation),
      "preexisting",
      adapter,
    ) ?? null;
  }

  replay_active_recording(
    operation_id: string,
    recording_key: string,
    generation = "test-generation",
    adapter: StrictRecordingControlAdapter | null = null,
  ) {
    const operation = this.#get_operation(operation_id);
    return operation?.[strict_internal_attach_recording](
      new StrictPublicRecordingHandleId(recording_key, generation),
      "active",
      adapter,
    ) ?? null;
  }

  replay_completed_recording(
    operation_id: string,
    recording_key: string,
    generation = "test-generation",
    adapter: StrictRecordingControlAdapter | null = null,
  ) {
    const operation = this.#get_operation(operation_id);
    return operation?.[strict_internal_attach_recording](
      new StrictPublicRecordingHandleId(recording_key, generation),
      "completed",
      adapter,
    ) ?? null;
  }

  complete_installation_ack(operation_id: string) {
    this.#get_operation(operation_id)?.[strict_internal_install_ack]();
  }

  arm_internal_activation_bridge(operation_id: string) {
    this.#get_operation(operation_id)?.[strict_internal_arm_bridge]();
  }

  transition(operation_id: string, phase: StrictOpenLifecycleEvent) {
    return this.#get_operation(operation_id)?.[strict_internal_transition](phase) ?? false;
  }

  install_operation_with_abort(
    operation_id: string,
    build: (operation: StrictOpenOperationWrapper) => void,
    on_abort: () => void,
  ) {
    const cache_token = {};
    const operation = new StrictOpenOperationWrapper(
      operation_id,
      null,
      "accepted",
      "open_and_select",
      () => this.#delete_operation(operation_id, cache_token),
      this.#schedule,
    );
    this.#operations.set(operation_id, {
      reference: new WeakRef(operation),
      token: cache_token,
    });
    try {
      build(operation);
      return operation;
    } catch (error) {
      try {
        on_abort();
      } finally {
        operation.dispose();
        this.#delete_operation(operation_id, cache_token);
      }
      throw error;
    }
  }

  dispose_operation(operation_id: string) {
    const operation = this.#get_operation(operation_id);
    if (!operation) {
      return false;
    }

    operation.dispose();
    return true;
  }

  mark_viewer_stopped() {
    for (const entry of this.#operations.values()) {
      entry.reference.deref()?.[strict_internal_viewer_stopped]();
    }
    this.#drop_dead_operations();
  }

  #get_operation(operation_id: string) {
    const entry = this.#operations.get(operation_id);
    const operation = entry?.reference.deref();
    if (entry && !operation) this.#operations.delete(operation_id);
    return operation ?? null;
  }

  #delete_operation(operation_id: string, token: object) {
    if (this.#operations.get(operation_id)?.token === token) {
      this.#operations.delete(operation_id);
    }
  }

  #drop_dead_operations() {
    for (const [operation_id, entry] of this.#operations) {
      if (!entry.reference.deref()) this.#operations.delete(operation_id);
    }
  }
}

export type StrictOpenLifecycleEvent =
  | "accepted"
  | "activated"
  | "behavior_ready"
  | "presentation_ready"
  | "terminal"
  | "removed";

export type StrictOpenLifecycleListener = (handle: OpenRequestHandle) => void;

/** Commands that target one exact remote-MCAP recording publication. */
export type StrictRecordingControlCommand = "select" | "seek" | "play" | "close";

/** Redacted result codes returned by exact remote recording controls. */
export type StrictRecordingControlCode =
  | "accepted"
  | "capability_unavailable"
  | "invalid_request"
  | "recording_closed"
  | "recording_removed"
  | "operation_closed"
  | "disposed"
  | "viewer_stopped";

/**
 * Structured result for an exact remote recording command.
 *
 * The result deliberately contains no recording, Store, source, or token identity.
 */
export interface StrictRecordingControlResult {
  readonly command: StrictRecordingControlCommand;
  readonly code: StrictRecordingControlCode;
  readonly accepted: boolean;
}

/** Canonical target on the remote recording's frozen navigation timeline. */
export interface StrictRecordingSeekTarget {
  readonly time_type: RemoteMcapTimeType;
  readonly value: string;
}

declare const strict_recording_handle_brand: unique symbol;

/** Opaque recording handle type reserved for the installed remote-MCAP capability. */
export interface RecordingHandle {
  readonly [strict_recording_handle_brand]: true;
  readonly phase: "preexisting" | "active" | "completed";
  readonly disposed: boolean;
  /** Select this exact remote recording publication. */
  select(): StrictRecordingControlResult;
  /** Seek this exact publication on its canonical remote timeline. */
  seek(target: StrictRecordingSeekTarget): StrictRecordingControlResult;
  /** Set the paused/playing state for this exact publication. */
  play(value: "paused" | "playing"): StrictRecordingControlResult;
  /** Close only this exact remote recording publication. */
  close(): StrictRecordingControlResult;
  /** Release subscriptions and tombstone retention without closing the publication. */
  dispose(): void;
}

declare const strict_open_request_handle_brand: unique symbol;

/** Opaque operation handle type reserved for the installed remote-MCAP capability. */
export interface OpenRequestHandle {
  readonly [strict_open_request_handle_brand]: true;
  readonly recordings: readonly RecordingHandle[];
  readonly phase: StrictOpenLifecycleEvent;
  readonly disposed: boolean;
  readonly recordingOpenBehavior: RemoteMcapOpenBehavior;
  on(event: StrictOpenLifecycleEvent, listener: StrictOpenLifecycleListener): () => void;
  /** Close the shared source owned by this operation. */
  close(): StrictRecordingControlResult;
  /** Release this operation's subscriptions and tombstone retention. */
  dispose(): void;
}

type StrictDispatcherTask = () => void;

class BoundedSingleTaskDispatcher {
  #queue: StrictDispatcherTask[] = [];
  #scheduled = false;
  #draining = false;
  #stopped = false;
  #schedule_count = 0;
  #delivered_count = 0;
  #error_notification_count = 0;
  #report_error_count = 0;
  #error_hook: ((error: unknown) => void) | null = null;
  readonly max_queue_length: number;

  constructor(max_queue_length = 64) {
    this.max_queue_length = max_queue_length;
  }

  get queued_count() {
    return this.#queue.length;
  }

  get scheduled_task_count() {
    return this.#schedule_count;
  }

  get delivered_count() {
    return this.#delivered_count;
  }

  get error_notification_count() {
    return this.#error_notification_count;
  }

  get report_error_count() {
    return this.#report_error_count;
  }

  get stopped() {
    return this.#stopped;
  }

  set_error_hook(hook: ((error: unknown) => void) | null) {
    this.#error_hook = hook;
  }

  enqueue(task: StrictDispatcherTask) {
    if (this.#stopped || this.#queue.length >= this.max_queue_length) {
      return false;
    }

    this.#queue.push(task);
    if (!this.#scheduled && !this.#draining) {
      this.#scheduled = true;
      this.#schedule_count += 1;
      setTimeout(() => this.#drain(), 0);
    }

    return true;
  }

  drain_now() {
    this.#drain();
  }

  cancel() {
    this.#stopped = true;
    this.#scheduled = false;
    this.#queue.length = 0;
  }

  #drain() {
    if (this.#stopped) {
      this.#scheduled = false;
      this.#queue.length = 0;
      return;
    }

    this.#scheduled = false;
    this.#draining = true;
    try {
      while (this.#queue.length > 0) {
        if (this.#stopped) {
          this.#queue.length = 0;
          return;
        }

        const task = this.#queue.shift();
        if (!task) {
          continue;
        }

        try {
          task();
          this.#delivered_count += 1;
        } catch (error) {
          this.#report_task_error(error);
        }
      }
    } finally {
      this.#draining = false;
    }
  }

  #report_task_error(error: unknown) {
    this.#error_notification_count += 1;
    if (!this.#error_hook) {
      this.#report_sink(error);
      return;
    }

    try {
      this.#error_hook(error);
    } catch (hook_error) {
      this.#report_error_count += 1;
      this.#report_sink(hook_error);
    }
  }

  #report_sink(error: unknown) {
    if (typeof globalThis.reportError === "function") {
      try {
        globalThis.reportError(error);
        return;
      } catch {
        // Fall through to console.error.
      }
    }
    console.error(error);
  }
}

const classes = {
  hide_scrollbars: "rerun-viewer-hide-scrollbars",
  fullscreen_base: "rerun-viewer-fullscreen-base",
  fullscreen_rect: "rerun-viewer-fullscreen-rect",
  transition: "rerun-viewer-transition",
};

const transition_delay_ms = 100;

const css = `
  html.${classes.hide_scrollbars},
  body.${classes.hide_scrollbars} {
    scrollbar-gutter: auto !important;
    overflow: hidden !important;
  }

  .${classes.fullscreen_base} {
    position: fixed;
    z-index: 99999;
  }

  .${classes.transition} {
    transition: all ${transition_delay_ms / 1000}s linear;
  }

  .${classes.fullscreen_rect} {
    left: 0;
    top: 0;
    width: 100%;
    height: 100%;
  }
`;

function injectStyle() {
  const ID = "__rerun_viewer_style";

  if (document.getElementById(ID)) {
    // already injected
    return;
  }

  const style = document.createElement("style");
  style.id = ID;
  style.appendChild(document.createTextNode(css));
  document.head.appendChild(style);
}

function setupGlobalEventListeners() {
  window.addEventListener("keyup", (e) => {
    if (e.code === "Escape") {
      _minimize_current_fullscreen_viewer?.();
    }
  });
}
