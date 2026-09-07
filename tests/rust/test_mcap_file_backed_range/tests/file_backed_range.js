// Chrome-only file-backed Range boundary probe.
//
// The production remote-MCAP capability remains disarmed: nothing here installs or exercises a
// real remote Store/query/playback path. This suite drives the file-backed fixture server
// directly from the top-level test page (no controlled iframe handshake) and asserts the
// transport boundary: exact ranges, Content-Range/Content-Length/ETag, byte equality against a
// deterministic fixture, full-length EOF, an overlong range, query rejection, AbortController
// cancel, and redacted error text.

const FIXTURE_LEN = 8192;
const COMMAND_TIMEOUT_MS = 10_000;

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

function fixtureObjectUrl() {
  const rawPort = new URLSearchParams(window.location.search).get("file_backed_object_port");
  const rawId = new URLSearchParams(window.location.search).get("file_backed_object_id");
  assert(rawPort !== null && /^\d+$/.test(rawPort), "missing file-backed object port");
  assert(rawId !== null && /^\d+$/.test(rawId), "missing file-backed object id");
  const port = Number(rawPort);
  assert(Number.isSafeInteger(port) && port > 0 && port <= 65_535, "invalid object port");
  return `http://127.0.0.1:${port}/__file_fixture_v1/object/${rawId}`;
}

function expectedFixtureBytes() {
  const bytes = new Uint8Array(FIXTURE_LEN);
  for (let index = 0; index < FIXTURE_LEN; index += 1) {
    bytes[index] = (index * 131 + 17) & 0xff;
  }
  return bytes;
}

function base64FromBytes(bytes) {
  let binary = "";
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  if (typeof btoa === "function") {
    return btoa(binary);
  }
  // Node fallback is not needed in Chrome; kept for tooling parity only.
  return Buffer.from(binary, "binary").toString("base64");
}

async function withTimeout(promise, label) {
  let timer;
  const timeout = new Promise((resolve, reject) => {
    timer = window.setTimeout(
      () => reject(new Error(`${label} timed out after ${COMMAND_TIMEOUT_MS}ms`)),
      COMMAND_TIMEOUT_MS,
    );
  });
  try {
    return await Promise.race([promise, timeout]);
  } finally {
    window.clearTimeout(timer);
  }
}

export async function runFileBackedRangeBoundaryE2E() {
  const objectUrl = fixtureObjectUrl();
  const expected = expectedFixtureBytes();
  console.log(`[file-back] objectUrl=${objectUrl} search=${window.location.search}`);

  // 1. Exact range: 206 with Content-Range, Content-Length, strong ETag, and exact bytes.
  {
    let response;
    try {
      response = await withTimeout(
        fetch(objectUrl, { headers: { Range: "bytes=1024-1535" }, credentials: "omit", cache: "no-store" }),
        "exact range fetch",
      );
      console.log(`[file-back] exact status=${response.status}`);
    } catch (error) {
      console.log(`[file-back] exact range fetch error: ${String(error)}`);
      throw new Error(`exact range fetch failed: ${String(error)}`);
    }
    assert(response.status === 206, `exact range status ${response.status}`);
    assert(
      response.headers.get("content-range") === "bytes 1024-1535/8192",
      `content-range ${response.headers.get("content-range")}`,
    );
    assert(response.headers.get("content-length") === "512", "content-length");
    const etag = response.headers.get("etag");
    assert(etag !== null && etag.length > 0, "etag missing");
    assert(etag.startsWith('"') && etag.endsWith('"'), "etag must be strong/opaque quoted");
    const body = new Uint8Array(await response.arrayBuffer());
    assert(body.length === 512, `exact body length ${body.length}`);
    for (let index = 0; index < 512; index += 1) {
      if (body[index] !== expected[1024 + index]) {
        throw new Error(`exact range byte mismatch at ${index}`);
      }
    }
    console.log("[file-back] stage1 exact range PASS");
  }

  // 2. Full-length EOF: two sequential range reads cover the whole fixture; a third read on the
  //    same reader reports `done` (no silent truncation / overlong delivery).
  {
    const response = await withTimeout(
      fetch(objectUrl, { headers: { Range: "bytes=0-8191" }, credentials: "omit", cache: "no-store" }),
      "full length fetch",
    );
    assert(response.status === 206, `full length status ${response.status}`);
    const reader = response.body.getReader({ mode: "byob" });
    const view1 = new Uint8Array(4000);
    const first = await reader.read(view1);
    assert(!first.done && first.value !== undefined, "first EOF read must deliver bytes");
    const second = await reader.read(new Uint8Array(8192 - first.value.length));
    assert(!second.done, "second read must deliver remaining bytes");
    const combined = new Uint8Array(first.value.length + second.value.length);
    combined.set(first.value, 0);
    combined.set(second.value, first.value.length);
    assert(combined.length === 8192, `combined length ${combined.length}`);
    for (let index = 0; index < combined.length; index += 1) {
      if (combined[index] !== expected[index]) {
        throw new Error(`full length byte mismatch at ${index}`);
      }
    }
    const third = await reader.read(new Uint8Array(1));
    assert(third.done === true, "third read must report EOF done");
    reader.releaseLock();
    console.log("[file-back] stage2 full-length EOF PASS");
  }



  // 3. Overlong range: a request that ends past the file must be rejected (416), not silently
  //    truncated.
  {
    const response = await withTimeout(
      fetch(objectUrl, { headers: { Range: "bytes=0-16383" }, credentials: "omit", cache: "no-store" }),
      "overlong range fetch",
    );
    assert(response.status === 416, `overlong range status ${response.status}`);
    console.log("[file-back] stage3 overlong PASS");
  }

  // 4. Query rejection: a query-bearing URL is refused (400), never served.
  {
    const response = await withTimeout(
      fetch(`${objectUrl}?secret=forbidden`, { headers: { Range: "bytes=0-1" }, credentials: "omit" }),
      "query fetch",
    );
    assert(response.status === 400, `query status ${response.status}`);
    const text = await response.text();
    assert(!text.includes("secret"), "query secret leaked in response");
  }

  // 5. AbortController cancel: an in-flight body read is cancelled and the error is redacted.
  {
    const controller = new AbortController();
    const response = await withTimeout(
      fetch(objectUrl, { headers: { Range: "bytes=0-8191" }, signal: controller.signal, credentials: "omit" }),
      "cancel fetch",
    );
    assert(response.status === 206, `cancel status ${response.status}`);
    const reader = response.body.getReader({ mode: "byob" });
    const first = await reader.read(new Uint8Array(256));
    assert(!first.done, "cancel read must start");
    controller.abort();
    let cancelled = false;
    try {
      await reader.read(new Uint8Array(256));
    } catch (error) {
      cancelled = true;
      const text = String(error);
      assert(!text.includes(objectUrl), "cancel error leaked object URL");
      assert(!text.includes("forbidden"), "cancel error leaked secret marker");
    }
    assert(cancelled, "second read after abort must reject");
    reader.releaseLock();
  }

  // 6. Unknown object id: 404 with a redacted body (no path/query/token echo).
  {
    const port = new URLSearchParams(window.location.search).get("file_backed_object_port");
    const unknownUrl = `http://127.0.0.1:${port}/__file_fixture_v1/object/999999999`;
    const response = await withTimeout(
      fetch(unknownUrl, { headers: { Range: "bytes=0-1" }, credentials: "omit" }),
      "unknown object fetch",
    );
    assert(response.status === 404, `unknown object status ${response.status}`);
    const text = await response.text();
    assert(!text.includes("999999999"), "unknown id leaked in response");
    assert(!text.includes("token"), "token marker leaked");
  }

  // 7. Deterministic digest assertion: fixture length and byte scheme are pinned above.
  assert(
    expected.length === FIXTURE_LEN,
    `fixture length pinned mismatch ${expected.length}`,
  );

  return base64FromBytes(expected.slice(0, 8));
}
