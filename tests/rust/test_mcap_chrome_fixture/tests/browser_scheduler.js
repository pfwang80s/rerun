const TIMEOUT_MILLISECONDS = 5_000;

function assert(condition, code) {
  if (!condition) {
    throw new Error(code);
  }
}

function deferred() {
  let resolve;
  const promise = new Promise((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

async function waitFor(predicate, code) {
  const deadline = window.performance.now() + TIMEOUT_MILLISECONDS;
  while (!predicate()) {
    if (window.performance.now() >= deadline) {
      throw new Error(code);
    }
    await new Promise((resolve) => window.setTimeout(resolve, 0));
  }
}

export async function runBrowserSchedulerSmoke() {
  const released = {
    microtask: deferred(),
    macrotask: deferred(),
    animationFrame: deferred(),
    repaint: deferred(),
  };
  const entered = {
    microtask: 0,
    macrotask: 0,
    animationFrame: 0,
    repaint: 0,
  };
  const completed = [];

  window.queueMicrotask(async () => {
    entered.microtask += 1;
    await released.microtask.promise;
    completed.push(0);
  });
  window.setTimeout(async () => {
    entered.macrotask += 1;
    await released.macrotask.promise;
    completed.push(1);
  }, 0);
  window.requestAnimationFrame(async () => {
    entered.animationFrame += 1;
    await released.animationFrame.promise;
    completed.push(2);
  });

  let repaintPending = false;
  function requestRepaint() {
    if (repaintPending) {
      return;
    }
    repaintPending = true;
    window.requestAnimationFrame(async () => {
      await released.repaint.promise;
      repaintPending = false;
      entered.repaint += 1;
      completed.push(3);
    });
  }
  requestRepaint();
  requestRepaint();

  await waitFor(
    () => entered.microtask === 1 && entered.macrotask === 1 && entered.animationFrame === 1,
    "browser_scheduler_enter_timeout",
  );
  assert(completed.length === 0, "browser_scheduler_gate_bypass");

  released.microtask.resolve();
  await waitFor(() => completed.length === 1, "browser_scheduler_microtask_timeout");
  assert(completed[0] === 0, "browser_scheduler_microtask_order");

  released.macrotask.resolve();
  await waitFor(() => completed.length === 2, "browser_scheduler_macrotask_timeout");
  assert(completed[1] === 1, "browser_scheduler_macrotask_order");

  released.animationFrame.resolve();
  await waitFor(() => completed.length === 3, "browser_scheduler_animation_frame_timeout");
  assert(completed[2] === 2, "browser_scheduler_animation_frame_order");

  released.repaint.resolve();
  await waitFor(() => completed.length === 4, "browser_scheduler_repaint_timeout");
  assert(completed[3] === 3, "browser_scheduler_repaint_order");
  assert(entered.repaint === 1, "browser_scheduler_repaint_not_latched");
  assert(!repaintPending, "browser_scheduler_repaint_still_pending");
}
