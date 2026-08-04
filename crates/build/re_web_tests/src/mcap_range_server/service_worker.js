const FIXTURE_PREFIX = "/__mcap_range_fixture/v1/";

self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (event) => event.waitUntil(self.clients.claim()));

function scenarioIdFromUrl(url) {
  const parts = new URL(url).pathname.split("/").filter(Boolean);
  const objectIndex = parts.indexOf("object");
  return objectIndex === -1 ? undefined : Number(parts[objectIndex + 1]);
}

function fixtureBaseFromUrl(url) {
  const parts = new URL(url).pathname.split("/").filter(Boolean);
  const v1Index = parts.indexOf("v1");
  return `${FIXTURE_PREFIX}${parts[v1Index + 1]}`;
}

async function postEvent(base, scenarioId, event) {
  await fetch(`${base}/control/scenarios/${scenarioId}/browser-events`, {
    method: "POST",
    cache: "no-store",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(event),
  });
}

async function browserConfig(base, scenarioId) {
  const response = await fetch(`${base}/control/scenarios/${scenarioId}/browser-config`, {
    cache: "no-store",
  });
  if (!response.ok) {
    throw new Error(`fixture browser config failed with status ${response.status}`);
  }
  return response.json();
}

function syntheticResponse(base, scenarioId, config) {
  const plan = config.service_worker;
  let chunkIndex = 0;
  let zeroProgressPulls = 0;
  let cancelled = false;

  const stream = new ReadableStream({
    type: "bytes",
    async pull(controller) {
      if (plan.body.type === "zero_progress_then_stall") {
        if (zeroProgressPulls < plan.body.pulls) {
          zeroProgressPulls += 1;
          await new Promise((resolve) => setTimeout(resolve, plan.body.delay_ms));
          await postEvent(base, scenarioId, {
            type: "service_worker_zero_progress",
            count: zeroProgressPulls,
          });
          return;
        }
        return new Promise(() => {});
      }

      if (plan.body.type === "infinite") {
        await new Promise((resolve) => setTimeout(resolve, plan.body.delay_ms));
        const value = new Uint8Array(plan.body.chunk_bytes);
        value.fill(plan.body.fill);
        controller.enqueue(value);
        return;
      }

      if (chunkIndex >= plan.body.chunks.length) {
        controller.close();
        return;
      }
      const chunk = plan.body.chunks[chunkIndex];
      chunkIndex += 1;
      await new Promise((resolve) => setTimeout(resolve, chunk.delay_ms));
      const value = new Uint8Array(chunk.length);
      value.fill(chunk.fill);
      controller.enqueue(value);
    },
    async cancel() {
      if (!cancelled) {
        cancelled = true;
        await postEvent(base, scenarioId, { type: "service_worker_cancelled" });
      }
    },
  });

  return new Response(stream, {
    status: plan.status,
    headers: plan.headers,
  });
}

self.addEventListener("fetch", (event) => {
  const scenarioId = scenarioIdFromUrl(event.request.url);
  if (scenarioId === undefined) {
    return;
  }

  const base = fixtureBaseFromUrl(event.request.url);
  event.respondWith(
    (async () => {
      const config = await browserConfig(base, scenarioId);
      if (config.service_worker.type === "disabled") {
        return fetch(event.request);
      }
      if (config.service_worker.type === "forward_no_store") {
        await postEvent(base, scenarioId, { type: "service_worker_forwarded" });
        return fetch(event.request, { cache: "no-store" });
      }
      await postEvent(base, scenarioId, { type: "service_worker_synthetic" });
      return syntheticResponse(base, scenarioId, config);
    })(),
  );
});
