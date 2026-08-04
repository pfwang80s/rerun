const U64_MAX = (1n << 64n) - 1n;
const RESOURCE_KEY_MAX = (1 << 16) - 1;
const textEncoder = new TextEncoder();
const exactSnapshotConstructorToken = Symbol("exact_snapshot_constructor");
const leakDetectorFinalizer =
  typeof FinalizationRegistry === "function"
    ? new FinalizationRegistry((values) => {
        for (const sensitive of values) {
          sensitive.bytes.fill(0);
        }
        values.length = 0;
      })
    : null;

export const sensitiveKinds = Object.freeze([
  "url",
  "query",
  "etag",
  "topic",
  "entity_path",
  "store_id",
  "internal_token",
]);

export const publicEffectKinds = Object.freeze([
  "event",
  "store",
  "route",
  "selection",
  "panel",
  "subscriber",
  "cache",
  "callback",
  "fetch",
  "connection",
  "mutation",
  "query",
  "animation_frame",
  "dom_handler",
  "observer",
]);

function failure(code, fields = {}) {
  const error = new Error(code);
  error.name = "ResourceAssertionError";
  error.code = code;
  Object.assign(error, fields);
  return error;
}

function checkedResourceKey(value) {
  if (!Number.isInteger(value) || value < 0 || value > RESOURCE_KEY_MAX) {
    throw failure("invalid_resource_key");
  }
  return value;
}

function checkedU64(value, code, key = undefined) {
  let integer;
  if (typeof value === "bigint") {
    integer = value;
  } else if (typeof value === "number" && Number.isSafeInteger(value)) {
    integer = BigInt(value);
  } else {
    throw failure(code, key === undefined ? {} : { key });
  }
  if (integer < 0n || integer > U64_MAX) {
    throw failure(code, key === undefined ? {} : { key });
  }
  return integer;
}

function checkedAdd(left, right, code, key) {
  const result = left + right;
  if (result > U64_MAX) {
    throw failure(code, { key });
  }
  return result;
}

function cloneUsage(usage) {
  return Object.freeze({
    currentCount: usage.currentCount,
    currentBytes: usage.currentBytes,
    highWaterCount: usage.highWaterCount,
    highWaterBytes: usage.highWaterBytes,
  });
}

function snapshotEntry(key, bucket) {
  return Object.freeze({
    key,
    maxCount: bucket.maxCount,
    maxBytes: bucket.maxBytes,
    usage: cloneUsage(bucket.usage),
  });
}

export class CheckedResourceRegistry {
  #revision = 0n;
  #buckets = new Map();
  #retainedPreparedWrappers = 0n;
  #retainedReservationWrappers = 0n;

  constructor(resources) {
    for (const resource of resources) {
      const key = checkedResourceKey(resource.key);
      if (this.#buckets.has(key)) {
        throw failure("duplicate_resource_definition", { key });
      }
      this.#buckets.set(key, {
        maxCount: checkedU64(resource.maxCount, "invalid_resource_count_limit", key),
        maxBytes: checkedU64(resource.maxBytes, "invalid_resource_byte_limit", key),
        usage: {
          currentCount: 0n,
          currentBytes: 0n,
          highWaterCount: 0n,
          highWaterBytes: 0n,
        },
      });
    }
  }

  snapshot() {
    return Object.freeze({
      revision: this.#revision,
      wrapperRetention: Object.freeze({
        prepared: this.#retainedPreparedWrappers,
        reservations: this.#retainedReservationWrappers,
      }),
      entries: Object.freeze(
        [...this.#buckets.entries()]
          .sort(([left], [right]) => left - right)
          .map(([key, bucket]) => snapshotEntry(key, bucket)),
      ),
    });
  }

  prepare(requests) {
    if (this.#revision === U64_MAX) {
      throw failure("registry_revision_exhausted");
    }
    const totals = new Map();
    for (const request of requests) {
      const key = checkedResourceKey(request.key);
      if (!this.#buckets.has(key)) {
        throw failure("unknown_resource", { key });
      }
      const count = checkedU64(request.count, "invalid_resource_count", key);
      const bytes = checkedU64(request.bytes, "invalid_resource_bytes", key);
      const previous = totals.get(key) ?? { count: 0n, bytes: 0n };
      totals.set(key, {
        count: checkedAdd(previous.count, count, "resource_count_overflow", key),
        bytes: checkedAdd(previous.bytes, bytes, "resource_byte_overflow", key),
      });
    }
    for (const [key, total] of totals) {
      const bucket = this.#buckets.get(key);
      const nextCount = checkedAdd(
        bucket.usage.currentCount,
        total.count,
        "resource_count_overflow",
        key,
      );
      const nextBytes = checkedAdd(
        bucket.usage.currentBytes,
        total.bytes,
        "resource_byte_overflow",
        key,
      );
      if (nextCount > bucket.maxCount) {
        throw failure("resource_count_limit_exceeded", { key });
      }
      if (nextBytes > bucket.maxBytes) {
        throw failure("resource_byte_limit_exceeded", { key });
      }
    }
    const expectedRevision = this.#revision;
    this.#retainedPreparedWrappers = checkedAdd(
      this.#retainedPreparedWrappers,
      1n,
      "prepared_wrapper_count_overflow",
      0,
    );
    return new PreparedReservations(
      () => this.#commit(expectedRevision, totals),
      () => {
        this.#retainedPreparedWrappers -= 1n;
      },
    );
  }

  #commit(expectedRevision, totals) {
    if (this.#revision !== expectedRevision) {
      throw failure("prepared_revision_mismatch");
    }
    const retainedReservations = checkedAdd(
      this.#retainedReservationWrappers,
      1n,
      "reservation_wrapper_count_overflow",
      0,
    );
    for (const [key, total] of totals) {
      const usage = this.#buckets.get(key).usage;
      usage.currentCount += total.count;
      usage.currentBytes += total.bytes;
      usage.highWaterCount =
        usage.highWaterCount > usage.currentCount
          ? usage.highWaterCount
          : usage.currentCount;
      usage.highWaterBytes =
        usage.highWaterBytes > usage.currentBytes
          ? usage.highWaterBytes
          : usage.currentBytes;
    }
    this.#revision += 1n;
    this.#retainedReservationWrappers = retainedReservations;
    return new ResourceReservation(
      () => this.#release(totals),
      () => {
        this.#retainedReservationWrappers -= 1n;
      },
    );
  }

  #release(totals) {
    for (const [key, total] of totals) {
      const usage = this.#buckets.get(key).usage;
      if (
        usage.currentCount < total.count ||
        usage.currentBytes < total.bytes
      ) {
        throw failure("resource_ownership_unbalanced", { key });
      }
    }
    for (const [key, total] of totals) {
      const usage = this.#buckets.get(key).usage;
      usage.currentCount -= total.count;
      usage.currentBytes -= total.bytes;
    }
  }
}

class PreparedReservations {
  #commitAction;
  #releaseRetention;
  #active = true;

  constructor(commitAction, releaseRetention) {
    this.#commitAction = commitAction;
    this.#releaseRetention = releaseRetention;
  }

  commit() {
    if (!this.#active) {
      throw failure("prepared_reservation_consumed");
    }
    const commitAction = this.#commitAction;
    const releaseRetention = this.#releaseRetention;
    this.#commitAction = null;
    this.#releaseRetention = null;
    this.#active = false;
    releaseRetention();
    return commitAction();
  }

  abort() {
    if (!this.#active) {
      return;
    }
    const releaseRetention = this.#releaseRetention;
    this.#commitAction = null;
    this.#releaseRetention = null;
    this.#active = false;
    releaseRetention();
  }

  dispose() {
    this.abort();
  }
}

class ResourceReservation {
  #releaseAction;
  #releaseRetention;
  #active = true;

  constructor(releaseAction, releaseRetention) {
    this.#releaseAction = releaseAction;
    this.#releaseRetention = releaseRetention;
  }

  release() {
    if (!this.#active) {
      return;
    }
    const releaseAction = this.#releaseAction;
    const releaseRetention = this.#releaseRetention;
    this.#releaseAction = null;
    this.#releaseRetention = null;
    this.#active = false;
    try {
      releaseAction();
    } finally {
      releaseRetention();
    }
  }

  dispose() {
    this.release();
  }
}

function sameUsage(left, right) {
  return (
    left.currentCount === right.currentCount &&
    left.currentBytes === right.currentBytes &&
    left.highWaterCount === right.highWaterCount &&
    left.highWaterBytes === right.highWaterBytes
  );
}

export function registrySnapshotsEqual(left, right) {
  if (
    left.revision !== right.revision ||
    left.wrapperRetention.prepared !== right.wrapperRetention.prepared ||
    left.wrapperRetention.reservations !==
      right.wrapperRetention.reservations ||
    left.entries.length !== right.entries.length
  ) {
    return false;
  }
  return left.entries.every((entry, index) => {
    const other = right.entries[index];
    return (
      entry.key === other.key &&
      entry.maxCount === other.maxCount &&
      entry.maxBytes === other.maxBytes &&
      sameUsage(entry.usage, other.usage)
    );
  });
}

export function assertRegistryUnchanged(before, after) {
  if (!registrySnapshotsEqual(before, after)) {
    throw failure("registry_changed_across_rollback");
  }
}

export class ExactSnapshot {
  #value;
  #equals;
  #active = true;

  static capture(value, callbacks) {
    let clone;
    let equals;
    try {
      if (callbacks === null || typeof callbacks !== "object") {
        throw new Error();
      }
      const cloneDescriptor = Object.getOwnPropertyDescriptor(callbacks, "clone");
      const equalsDescriptor = Object.getOwnPropertyDescriptor(
        callbacks,
        "equals",
      );
      if (
        cloneDescriptor === undefined ||
        equalsDescriptor === undefined ||
        !("value" in cloneDescriptor) ||
        !("value" in equalsDescriptor) ||
        typeof cloneDescriptor.value !== "function" ||
        typeof equalsDescriptor.value !== "function"
      ) {
        throw new Error();
      }
      clone = cloneDescriptor.value;
      equals = equalsDescriptor.value;
    } catch {
      throw failure("invalid_exact_snapshot_callbacks");
    }
    let snapshot;
    try {
      snapshot = clone(value);
    } catch {
      throw failure("exact_snapshot_clone_failed");
    }
    return new ExactSnapshot(exactSnapshotConstructorToken, snapshot, equals);
  }

  constructor(token, value, equals) {
    if (token !== exactSnapshotConstructorToken) {
      throw failure("invalid_exact_snapshot_constructor");
    }
    this.#value = value;
    this.#equals = equals;
  }

  isUnchanged(actual) {
    if (!this.#active) {
      throw failure("exact_snapshot_disposed");
    }
    let unchanged;
    try {
      unchanged = this.#equals(this.#value, actual);
    } catch {
      throw failure("exact_snapshot_comparison_failed");
    }
    if (typeof unchanged !== "boolean") {
      throw failure("exact_snapshot_comparison_failed");
    }
    return unchanged;
  }

  assertUnchanged(actual) {
    if (!this.isUnchanged(actual)) {
      throw failure("exact_snapshot_changed");
    }
  }

  dispose() {
    this.#value = undefined;
    this.#equals = null;
    this.#active = false;
  }
}

function toBytes(value) {
  if (typeof value === "string") {
    return textEncoder.encode(value);
  }
  if (value instanceof Uint8Array) {
    return value;
  }
  if (value instanceof ArrayBuffer) {
    return new Uint8Array(value);
  }
  throw failure("invalid_byte_observable");
}

function findBytes(haystack, needle) {
  if (needle.byteLength > haystack.byteLength) {
    return -1;
  }
  const last = haystack.byteLength - needle.byteLength;
  for (let offset = 0; offset <= last; offset += 1) {
    let matches = true;
    for (let index = 0; index < needle.byteLength; index += 1) {
      if (haystack[offset + index] !== needle[index]) {
        matches = false;
        break;
      }
    }
    if (matches) {
      return offset;
    }
  }
  return -1;
}

export class LeakDetector {
  #values;
  #active = true;

  constructor() {
    this.#values = [];
    leakDetectorFinalizer?.register(this, this.#values, this);
  }

  register(kind, value) {
    if (!this.#active) {
      throw failure("leak_detector_disposed");
    }
    if (!sensitiveKinds.includes(kind)) {
      throw failure("unknown_sensitive_kind");
    }
    const bytes = toBytes(value);
    if (bytes.byteLength === 0) {
      throw failure("empty_sensitive_value", { kind });
    }
    this.#values.push({ kind, bytes: bytes.slice() });
    return this;
  }

  inspect(observable) {
    if (!this.#active) {
      throw failure("leak_detector_disposed");
    }
    const bytes = toBytes(observable);
    for (const sensitive of this.#values) {
      const offset = findBytes(bytes, sensitive.bytes);
      if (offset !== -1) {
        throw failure("sensitive_value_leak", {
          kind: sensitive.kind,
          offset,
        });
      }
    }
  }

  clone() {
    if (!this.#active) {
      throw failure("leak_detector_disposed");
    }
    const clone = new LeakDetector();
    for (const sensitive of this.#values) {
      clone.register(sensitive.kind, sensitive.bytes);
    }
    return clone;
  }

  dispose() {
    if (!this.#active) {
      return;
    }
    leakDetectorFinalizer?.unregister(this);
    for (const sensitive of this.#values) {
      sensitive.bytes.fill(0);
    }
    this.#values.length = 0;
    this.#active = false;
  }

  get size() {
    return this.#values.length;
  }
}

export function parseRedactionLeakCorpusV1(text) {
  if (typeof text !== "string") {
    throw failure("invalid_redaction_corpus");
  }
  const detector = new LeakDetector();
  try {
    const seen = new Set();
    const lines = text.split(/\r?\n/u);
    for (let index = 0; index < lines.length; index += 1) {
      const line = lines[index];
      if (line.length === 0 || line.startsWith("#")) {
        continue;
      }
      const separator = line.indexOf("\t");
      if (separator <= 0 || separator === line.length - 1) {
        throw failure("invalid_redaction_corpus_line", { line: index + 1 });
      }
      const kind = line.slice(0, separator);
      if (!sensitiveKinds.includes(kind)) {
        throw failure("unknown_redaction_corpus_kind", { line: index + 1 });
      }
      if (seen.has(kind)) {
        throw failure("duplicate_redaction_corpus_kind", { kind });
      }
      seen.add(kind);
      detector.register(kind, line.slice(separator + 1));
    }
    if (seen.size !== sensitiveKinds.length) {
      throw failure("incomplete_redaction_corpus");
    }
    return detector;
  } catch (error) {
    detector.dispose();
    throw error;
  }
}

export function boundedLabel(value, maxBytes, detector) {
  if (typeof value !== "string") {
    throw failure("invalid_bounded_label");
  }
  if (!Number.isSafeInteger(maxBytes) || maxBytes < 0) {
    throw failure("invalid_bounded_label_limit");
  }
  const actualBytes = textEncoder.encode(value).byteLength;
  if (actualBytes > maxBytes) {
    throw failure("bounded_label_too_long", { actualBytes, maxBytes });
  }
  detector.inspect(value);
  return value;
}

export function checkZeroized(value) {
  const bytes = toBytes(value);
  const offset = bytes.findIndex((byte) => byte !== 0);
  if (offset !== -1) {
    throw failure("buffer_not_zeroized", { offset });
  }
}

class TemporaryByteInspection {
  #bytes;

  constructor(bytes) {
    this.#bytes = bytes;
  }

  get byteLength() {
    return this.#bytes.byteLength;
  }

  at(index) {
    if (!Number.isSafeInteger(index) || index < 0 || index >= this.#bytes.length) {
      throw failure("invalid_zeroize_inspection_index");
    }
    return this.#bytes[index];
  }

  set(index, value) {
    if (!Number.isSafeInteger(index) || index < 0 || index >= this.#bytes.length) {
      throw failure("invalid_zeroize_inspection_index");
    }
    if (!Number.isSafeInteger(value) || value < 0 || value > 255) {
      throw failure("invalid_zeroize_inspection_byte");
    }
    this.#bytes[index] = value;
  }

  fill(value) {
    if (!Number.isSafeInteger(value) || value < 0 || value > 255) {
      throw failure("invalid_zeroize_inspection_byte");
    }
    this.#bytes.fill(value);
  }

  isZeroized() {
    return this.#bytes.every((byte) => byte === 0);
  }
}

export class ZeroizingTestBytes {
  #bytes;
  #active = true;

  constructor(value) {
    this.#bytes = toBytes(value).slice();
  }

  inspect(callback) {
    if (typeof callback !== "function") {
      throw failure("invalid_zeroize_inspector");
    }
    const temporary = this.#bytes.slice();
    const inspection = new TemporaryByteInspection(temporary);
    try {
      return callback(inspection);
    } finally {
      temporary.fill(0);
    }
  }

  dispose() {
    if (this.#active) {
      this.#bytes.fill(0);
      this.#active = false;
    }
  }

  observer() {
    return Object.freeze({
      check: () => checkZeroized(this.#bytes),
    });
  }
}

export class PublicEffectProbe {
  #counts = new Map(publicEffectKinds.map((kind) => [kind, 0n]));

  record(kind) {
    if (!this.#counts.has(kind)) {
      throw failure("unknown_public_effect_kind");
    }
    const count = this.#counts.get(kind);
    if (count === U64_MAX) {
      throw failure("public_effect_counter_overflow", { kind });
    }
    this.#counts.set(kind, count + 1n);
  }

  snapshot() {
    return Object.freeze(
      publicEffectKinds.map((kind) =>
        Object.freeze({ kind, count: this.#counts.get(kind) }),
      ),
    );
  }
}

function validatedPublicEffectSnapshot(snapshot) {
  try {
    if (
      !Array.isArray(snapshot) ||
      snapshot.length !== publicEffectKinds.length
    ) {
      throw new Error();
    }
    const counts = [];
    for (let index = 0; index < publicEffectKinds.length; index += 1) {
      const entry = snapshot[index];
      if (entry === null || typeof entry !== "object") {
        throw new Error();
      }
      const keys = Reflect.ownKeys(entry);
      if (
        keys.length !== 2 ||
        !keys.includes("kind") ||
        !keys.includes("count")
      ) {
        throw new Error();
      }
      const kind = Object.getOwnPropertyDescriptor(entry, "kind");
      const count = Object.getOwnPropertyDescriptor(entry, "count");
      if (
        kind === undefined ||
        count === undefined ||
        !("value" in kind) ||
        !("value" in count) ||
        kind.value !== publicEffectKinds[index] ||
        typeof count.value !== "bigint" ||
        count.value < 0n ||
        count.value > U64_MAX
      ) {
        throw new Error();
      }
      counts.push(count.value);
    }
    return counts;
  } catch {
    throw failure("invalid_public_effect_snapshot", { kind: "invalid" });
  }
}

export function assertNoPublicEffects(before, after) {
  const beforeCounts = validatedPublicEffectSnapshot(before);
  const afterCounts = validatedPublicEffectSnapshot(after);
  for (let index = 0; index < publicEffectKinds.length; index += 1) {
    if (beforeCounts[index] !== afterCounts[index]) {
      throw failure("unexpected_public_effect", {
        kind: publicEffectKinds[index],
      });
    }
  }
}

export function captureInternerSnapshot(readBytesUsed) {
  return checkedU64(readBytesUsed(), "invalid_interner_counter");
}

export function assertNoInternerDelta(before, after) {
  if (before !== after) {
    throw failure("unexpected_interner_delta");
  }
}
