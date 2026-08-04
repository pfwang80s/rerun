import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import { inspect as inspectValue } from "node:util";

import {
  CheckedResourceRegistry,
  ExactSnapshot,
  LeakDetector,
  PublicEffectProbe,
  ZeroizingTestBytes,
  assertNoInternerDelta,
  assertNoPublicEffects,
  assertRegistryUnchanged,
  boundedLabel,
  captureInternerSnapshot,
  checkZeroized,
  parseRedactionLeakCorpusV1,
  publicEffectKinds,
  sensitiveKinds,
} from "./support/resource_assertions.mjs";

const corpusUrl = new URL(
  "../../../crates/build/re_web_tests/test_data/redaction_leak_corpus_v1.tsv",
  import.meta.url,
);

async function corpus() {
  return parseRedactionLeakCorpusV1(await readFile(corpusUrl, "utf8"));
}

function assertErrorDoesNotRetain(error, sentinel) {
  const ownProperties = Reflect.ownKeys(error).map((key) => [
    String(key),
    inspectValue(error[key]),
  ]);
  const surfaces = [
    error.message,
    error.stack ?? "",
    JSON.stringify(error),
    inspectValue(error),
    inspectValue(ownProperties),
  ];
  for (const surface of surfaces) {
    assert.equal(surface.includes(sentinel), false);
  }
}

test("checked resource prepare is atomic and rollback preserves exact snapshot", () => {
  const registry = new CheckedResourceRegistry([
    { key: 1, maxCount: 2n, maxBytes: 16n },
    { key: 2, maxCount: 2n, maxBytes: 32n },
  ]);
  const before = registry.snapshot();
  assert.throws(
    () =>
      registry.prepare([
        { key: 1, count: 1n, bytes: 8n },
        { key: 2, count: 1n, bytes: 33n },
      ]),
    (error) => error.code === "resource_byte_limit_exceeded" && error.key === 2,
  );
  assertRegistryUnchanged(before, registry.snapshot());

  const aborted = registry.prepare([{ key: 1, count: 1n, bytes: 8n }]);
  assert.deepEqual(registry.snapshot().wrapperRetention, {
    prepared: 1n,
    reservations: 0n,
  });
  aborted.abort();
  aborted.abort();
  aborted.dispose();
  assertRegistryUnchanged(before, registry.snapshot());
  assert.throws(
    () =>
      registry.prepare([
        { key: 1, count: (1n << 64n) - 1n, bytes: 0n },
        { key: 1, count: 1n, bytes: 0n },
      ]),
    { code: "resource_count_overflow", key: 1 },
  );
  assertRegistryUnchanged(before, registry.snapshot());
});

test("checked resource accounting handles duplicate claims and concurrent release", () => {
  const registry = new CheckedResourceRegistry([
    { key: 1, maxCount: 4n, maxBytes: 32n },
  ]);
  const firstPrepared = registry.prepare([
    { key: 1, count: 1n, bytes: 3n },
    { key: 1, count: 1n, bytes: 5n },
  ]);
  assert.deepEqual(registry.snapshot().wrapperRetention, {
    prepared: 1n,
    reservations: 0n,
  });
  const first = firstPrepared.commit();
  assert.deepEqual(registry.snapshot().wrapperRetention, {
    prepared: 0n,
    reservations: 1n,
  });
  const second = registry
    .prepare([{ key: 1, count: 1n, bytes: 7n }])
    .commit();
  first.release();
  first.release();
  first.dispose();
  let usage = registry.snapshot().entries[0].usage;
  assert.deepEqual(
    [usage.currentCount, usage.currentBytes],
    [1n, 7n],
  );
  second.dispose();
  second.dispose();
  usage = registry.snapshot().entries[0].usage;
  assert.deepEqual(
    [usage.currentCount, usage.currentBytes],
    [0n, 0n],
  );
  assert.deepEqual(
    [usage.highWaterCount, usage.highWaterBytes],
    [3n, 15n],
  );
  assert.deepEqual(registry.snapshot().wrapperRetention, {
    prepared: 0n,
    reservations: 0n,
  });
});

test("stale commit consumes all wrapper retention without garbage collection", () => {
  const registry = new CheckedResourceRegistry([
    { key: 1, maxCount: 2n, maxBytes: 16n },
  ]);
  const first = registry.prepare([{ key: 1, count: 1n, bytes: 1n }]);
  const stale = registry.prepare([{ key: 1, count: 1n, bytes: 1n }]);
  assert.deepEqual(registry.snapshot().wrapperRetention, {
    prepared: 2n,
    reservations: 0n,
  });
  const reservation = first.commit();
  assert.throws(() => stale.commit(), { code: "prepared_revision_mismatch" });
  stale.dispose();
  assert.deepEqual(registry.snapshot().wrapperRetention, {
    prepared: 0n,
    reservations: 1n,
  });
  reservation.dispose();
  assert.deepEqual(registry.snapshot().wrapperRetention, {
    prepared: 0n,
    reservations: 0n,
  });
});

test("shared corpus detects every required leak without echoing secrets", async () => {
  const text = await readFile(corpusUrl, "utf8");
  const detector = parseRedactionLeakCorpusV1(text);
  assert.equal(detector.size, sensitiveKinds.length);
  for (const line of text.split(/\r?\n/u)) {
    if (line.length === 0 || line.startsWith("#")) {
      continue;
    }
    const [kind, value] = line.split("\t");
    assert.throws(
      () => detector.inspect(value),
      (error) => {
        assert.equal(error.code, "sensitive_value_leak");
        assert.equal(error.kind, kind);
        assert.equal(error.offset, 0);
        assert.equal(error.message.includes(value), false);
        return true;
      },
    );
  }
  detector.inspect("code=remote_open_failed stage=activation");
  detector.dispose();
  detector.dispose();
});

test("bounded labels enforce UTF-8 bytes and registered sensitive values", async () => {
  const detector = await corpus();
  assert.equal(
    boundedLabel("remote_open_failed", 32, detector),
    "remote_open_failed",
  );
  assert.throws(
    () => boundedLabel("éé", 3, detector),
    (error) =>
      error.code === "bounded_label_too_long" &&
      error.actualBytes === 4 &&
      error.maxBytes === 3,
  );
  const explicit = new LeakDetector().register("store_id", "private-store");
  assert.throws(
    () => boundedLabel("private-store", 64, explicit),
    (error) =>
      error.code === "sensitive_value_leak" && error.kind === "store_id",
  );
  explicit.dispose();
  detector.dispose();
});

test("leak detector clones own and zeroize independent sentinel storage", () => {
  const sentinel = ["detector", "private", "sentinel"].join("_");
  const detector = new LeakDetector().register("query", sentinel);
  const clone = detector.clone();
  detector.dispose();
  detector.dispose();
  assert.equal(detector.size, 0);
  assert.throws(
    () => clone.inspect(sentinel),
    (error) => {
      assert.equal(error.code, "sensitive_value_leak");
      assertErrorDoesNotRetain(error, sentinel);
      return true;
    },
  );
  clone.dispose();
  assert.equal(clone.size, 0);
});

test("zeroize probe exposes only self-clearing temporary copies", () => {
  const owner = new ZeroizingTestBytes(new TextEncoder().encode("private"));
  const observer = owner.observer();
  assert.throws(() => observer.check(), { code: "buffer_not_zeroized" });
  let retained;
  const returned = owner.inspect((temporary) => {
    retained = temporary;
    temporary.fill(7);
    return temporary;
  });
  assert.equal(returned, retained);
  assert.equal(retained.isZeroized(), true);
  assert.throws(() => observer.check(), { code: "buffer_not_zeroized" });

  let retainedFromThrow;
  assert.throws(
    () =>
      owner.inspect((temporary) => {
        retainedFromThrow = temporary;
        temporary.fill(9);
        throw new Error("inspector_failed");
      }),
    /inspector_failed/,
  );
  assert.equal(retainedFromThrow.isZeroized(), true);
  assert.throws(() => observer.check(), { code: "buffer_not_zeroized" });

  owner.dispose();
  owner.dispose();
  observer.check();
  retained.fill(5);
  retainedFromThrow.fill(6);
  observer.check();
  checkZeroized(new Uint8Array(4));
});

test("public-effect snapshots reject all malformed shapes without retaining input", () => {
  const effects = new PublicEffectProbe();
  const before = effects.snapshot();
  assertNoPublicEffects(before, effects.snapshot());
  effects.record("query");
  assert.throws(
    () => assertNoPublicEffects(before, effects.snapshot()),
    (error) =>
      error.code === "unexpected_public_effect" && error.kind === "query",
  );
  assert.deepEqual(
    before.map(({ kind }) => kind),
    publicEffectKinds,
  );

  const sentinel = ["caller", "public", "effect", "sentinel"].join("_");
  const malformed = [
    [{ kind: sentinel, count: 0n }, ...before.slice(1)],
    [{ kind: "event", count: sentinel }, ...before.slice(1)],
    [{ kind: "event", count: 0n, [sentinel]: sentinel }, ...before.slice(1)],
    [
      new Proxy(
        {},
        {
          ownKeys() {
            throw new Error(sentinel);
          },
        },
      ),
      ...before.slice(1),
    ],
    before.slice(1),
  ];
  for (const snapshot of malformed) {
    assert.throws(
      () => assertNoPublicEffects(before, snapshot),
      (error) => {
        assert.equal(error.code, "invalid_public_effect_snapshot");
        assert.equal(error.kind, "invalid");
        assertErrorDoesNotRetain(error, sentinel);
        return true;
      },
    );
  }
});

test("caller-defined exact snapshots never format retained state", () => {
  const sentinel = ["exact", "snapshot", "sentinel"].join("_");
  const state = { secret: sentinel, nested: { count: 1 } };
  const snapshot = ExactSnapshot.capture(state, {
    clone: (value) => structuredClone(value),
    equals: (expected, actual) =>
      expected.secret === actual.secret &&
      expected.nested.count === actual.nested.count,
  });
  assert.equal(snapshot.isUnchanged(state), true);
  snapshot.assertUnchanged(structuredClone(state));
  assert.throws(
    () =>
      snapshot.assertUnchanged({
        secret: sentinel,
        nested: { count: 2 },
      }),
    (error) => {
      assert.equal(error.code, "exact_snapshot_changed");
      assertErrorDoesNotRetain(error, sentinel);
      return true;
    },
  );
  snapshot.dispose();
  snapshot.dispose();
  assert.throws(
    () => snapshot.isUnchanged(state),
    (error) => {
      assert.equal(error.code, "exact_snapshot_disposed");
      assertErrorDoesNotRetain(error, sentinel);
      return true;
    },
  );

  assert.throws(
    () =>
      ExactSnapshot.capture(state, {
        clone() {
          throw new Error(sentinel);
        },
        equals: () => true,
      }),
    (error) => {
      assert.equal(error.code, "exact_snapshot_clone_failed");
      assertErrorDoesNotRetain(error, sentinel);
      return true;
    },
  );
  assert.throws(
    () =>
      ExactSnapshot.capture(
        state,
        new Proxy(
          {},
          {
            getOwnPropertyDescriptor() {
              throw new Error(sentinel);
            },
          },
        ),
      ),
    (error) => {
      assert.equal(error.code, "invalid_exact_snapshot_callbacks");
      assertErrorDoesNotRetain(error, sentinel);
      return true;
    },
  );
  const comparisonFailure = ExactSnapshot.capture(state, {
    clone: (value) => structuredClone(value),
    equals() {
      throw new Error(sentinel);
    },
  });
  assert.throws(
    () => comparisonFailure.isUnchanged(state),
    (error) => {
      assert.equal(error.code, "exact_snapshot_comparison_failed");
      assertErrorDoesNotRetain(error, sentinel);
      return true;
    },
  );
  comparisonFailure.dispose();
});

test("interner snapshots observe exact counter boundaries", () => {
  const internerBefore = captureInternerSnapshot(() => 42n);
  const internerAfter = captureInternerSnapshot(() => 42n);
  assertNoInternerDelta(internerBefore, internerAfter);
  assert.throws(
    () => assertNoInternerDelta(internerBefore, 43n),
    { code: "unexpected_interner_delta" },
  );
});
