// Production-disarmed MCAP-114 release-gate boundary harness.
//
// Top-level direct driver (no iframe) via `range_driver.js`.
//
// This suite exercises only the current disarmed classification and extensionless HTTP Range
// boundary. It does not install or exercise real remote-MCAP open, Store, query, playback, seek,
// GC, or interner exhaustion.

import {
  assertNoRangeLessGet,
  assertRedactedFixtureFailure,
  baseScenario,
  expectReject,
  fetchBootstrap,
  requestEvents,
  withScenario,
} from "/tests/range_driver.js";

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
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

async function extensionlessRange(bootstrap) {
  await withScenario(
    bootstrap,
    baseScenario({
      object_seed: 9,
      body: {
        type: "finite",
        chunks: [{ length: 4, delay_ms: 0, wait_for_gate: null }],
      },
    }),
    async (session) => {
      const result = await session.fetchByob({
        headers: { Range: "bytes=0-3", "If-Match": '"v1"' },
        scratchBytes: 4,
        expectedRange: { start: 0, end: 3 },
        requiredResponseHeaders: ["content-range", "etag"],
      });
      const instrumentation = session.getInstrumentation();
      assert(result.status === 206, "extensionless Range returned the wrong status");
      assert(result.visibleBytes === 4, "extensionless Range returned wrong bytes");
      assert(result.chunks.flat().join(",") === "9,10,11,12", "extensionless Range bytes changed");
      assert(
        instrumentation.arrayBufferCallCount === 0,
        "extensionless Range path called Response.arrayBuffer",
      );

      const snapshot = await session.snapshot();
      assertNoRangeLessGet(snapshot, "extensionless Range");
      assertNoRemoteOwnerSignals(snapshot, "extensionless Range");
    },
  );
}

async function extensionlessRedactedFailure(bootstrap) {
  await withScenario(
    bootstrap,
    baseScenario({
      body: {
        type: "finite",
        chunks: [{ length: 2, delay_ms: 0, wait_for_gate: null }],
      },
    }),
    async (session) => {
      const error = await expectReject(
        session.fetchByob({
          headers: { Range: "bytes=0-3" },
          scratchBytes: 4,
          expectedBytes: 4,
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
  const bootstrap = await fetchBootstrap();
  assert(bootstrap.page_origin !== bootstrap.object_origin, "fixture origins are not distinct");

  await extensionlessRange(bootstrap);
  await extensionlessRedactedFailure(bootstrap);
}
