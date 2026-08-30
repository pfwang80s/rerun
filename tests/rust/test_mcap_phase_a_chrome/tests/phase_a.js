const SCHEMA = "rerun-mcap-phase-a-evidence-v1";
const WARMUP_ITERATIONS = 2;
const SAMPLE_ITERATIONS = 5;
const STAGES = ["byob_copy", "opening_parse", "message_index_parse", "physical_validation"];

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function fixturePagePort() {
  const raw = new URLSearchParams(window.location.search).get("mcap_fixture_page_port");
  assert(raw !== null && /^\d+$/.test(raw), "missing fixture page port");
  return Number(raw);
}

async function bootstrap() {
  const response = await fetch(`http://127.0.0.1:${fixturePagePort()}/__mcap_range_fixture/v1/bootstrap`);
  assert(response.ok, "Phase A bootstrap failed");
  return response.json();
}

function safePositive(raw, label) {
  const value = typeof raw === "bigint" ? Number(raw) : raw;
  assert(Number.isSafeInteger(value) && value > 0, `invalid ${label}`);
  return value;
}

async function sample(proof, name, fixtureUrl, fixtureLength) {
  const started = performance.now();
  const raw = await proof.run_phase_a_sample_v1(name, fixtureUrl, BigInt(fixtureLength));
  const elapsedMicros = Math.max(1, Math.ceil((performance.now() - started) * 1000));
  return {
    input_bytes: safePositive(raw.input_bytes, `${name}.input_bytes`),
    output_bytes: safePositive(raw.output_bytes, `${name}.output_bytes`),
    completed_count: safePositive(raw.completed_count, `${name}.completed_count`),
    retained_high_water_bytes: safePositive(raw.retained_high_water_bytes, `${name}.retained_high_water_bytes`),
    max_duration_micros: name === "byob_copy"
      ? elapsedMicros
      : Math.max(1, safePositive(raw.max_duration_micros, `${name}.max_duration_micros`)),
    overflowed: raw.overflowed === true,
  };
}

async function measureStage(proof, name, fixtureUrl, fixtureLength) {
  for (let iteration = 0; iteration < WARMUP_ITERATIONS; iteration += 1) {
    await sample(proof, name, fixtureUrl, fixtureLength);
  }
  const aggregate = {
    name,
    max_duration_micros: 0,
    input_bytes: 0,
    output_bytes: 0,
    completed_count: 0,
    retained_high_water_bytes: 0,
    overflowed: false,
  };
  for (let iteration = 0; iteration < SAMPLE_ITERATIONS; iteration += 1) {
    const current = await sample(proof, name, fixtureUrl, fixtureLength);
    aggregate.max_duration_micros = Math.max(aggregate.max_duration_micros, current.max_duration_micros);
    aggregate.input_bytes += current.input_bytes;
    aggregate.output_bytes += current.output_bytes;
    aggregate.completed_count += current.completed_count;
    aggregate.retained_high_water_bytes = Math.max(
      aggregate.retained_high_water_bytes,
      current.retained_high_water_bytes,
    );
    aggregate.overflowed ||= current.overflowed;
    for (const field of ["input_bytes", "output_bytes", "completed_count"]) {
      assert(Number.isSafeInteger(aggregate[field]), `${name}.${field} overflowed JS safe integer`);
    }
  }
  assert(aggregate.completed_count === SAMPLE_ITERATIONS, `${name} completed count mismatch`);
  assert(!aggregate.overflowed, `${name} telemetry overflowed`);
  return aggregate;
}

export async function runMcapPhaseABenchmark() {
  const config = await bootstrap();
  assert(typeof config.phase_a_proof_module_url === "string", "missing release proof module URL");
  assert(typeof config.phase_a_result_url === "string", "missing evidence result URL");
  assert(typeof config.phase_a_fixture_url === "string", "missing controlled fixture URL");
  assert(Number.isSafeInteger(config.phase_a_fixture_length) && config.phase_a_fixture_length > 0, "invalid controlled fixture length");
  assert(config.phase_a_build !== null && typeof config.phase_a_build === "object", "missing proof build provenance");
  const chromeMatch = navigator.userAgent.match(/(?:HeadlessChrome|Chrome)\/([0-9.]+)/);
  assert(chromeMatch !== null, "Phase A benchmark did not run in Chrome");
  const proof = await import(config.phase_a_proof_module_url);
  await proof.default(config.phase_a_proof_wasm_url);

  const stages = [];
  for (const name of STAGES) {
    stages.push(await measureStage(
      proof,
      name,
      config.phase_a_fixture_url,
      config.phase_a_fixture_length,
    ));
  }

  const evidence = {
    schema: SCHEMA,
    profile_status: "unfrozen",
    build_profile: "web-release",
    wasm_optimized: true,
    warmup_iterations: WARMUP_ITERATIONS,
    sample_iterations: SAMPLE_ITERATIONS,
    provenance: {
      fixture: "fixed-mcap-phase-a-v1",
      transport: "controlled-range-byob-v1",
      pipeline: "re_viewer-transport-physical-pipeline-proof-v1",
    },
    build: {
      wasm_sha256: config.phase_a_build.wasm_sha256,
      js_sha256: config.phase_a_build.js_sha256,
      fixture_sha256: config.phase_a_build.fixture_sha256,
      git_commit: config.phase_a_build.git_commit,
      browser_family: config.phase_a_build.browser_family,
      chrome_version: chromeMatch[1],
    },
    stages,
  };
  const response = await fetch(config.phase_a_result_url, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(evidence),
  });
  assert(response.ok, `evidence upload failed: ${response.status}`);
}
