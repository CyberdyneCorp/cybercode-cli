import { test } from "node:test";
import assert from "node:assert/strict";

import { JobQueue } from "../src/index.js";

/** Fake timers: `advance(ms)` runs due timers in order. */
function fakeTimers() {
  let now = 0;
  let seq = 0;
  const pending = new Map();
  return {
    setTimeout(fn, ms) {
      const handle = ++seq;
      pending.set(handle, { at: now + ms, fn });
      return handle;
    },
    clearTimeout(handle) {
      pending.delete(handle);
    },
    async advance(ms) {
      const end = now + ms;
      for (;;) {
        await new Promise((resolve) => setImmediate(resolve));
        const due = [...pending].filter(([, t]) => t.at <= end).sort((a, b) => a[1].at - b[1].at || a[0] - b[0])[0];
        if (!due) break;
        pending.delete(due[0]);
        now = due[1].at;
        due[1].fn();
      }
      now = end;
      await new Promise((resolve) => setImmediate(resolve));
    },
  };
}

test("runs jobs by priority", async () => {
  const queue = new JobQueue({ concurrency: 1 });
  const order = [];
  queue.pause();
  const jobs = [1, 5, 3].map((priority) => queue.add(() => order.push(priority), { priority }));
  queue.resume();
  await Promise.all(jobs);
  assert.deepEqual(order, [5, 3, 1]);
});

test("retries with exponential backoff", async () => {
  const timers = fakeTimers();
  const queue = new JobQueue({ retries: 2, baseDelay: 100, timers });
  const delays = [];
  queue.on("retry", (job, error, delay) => delays.push(delay));
  const result = queue.add(async ({ attempt }) => {
    if (attempt < 3) throw new Error(`attempt ${attempt}`);
    return "done";
  });
  await timers.advance(1000);
  assert.equal(await result, "done");
  assert.deepEqual(delays, [100, 200]);
});

test("drain resolves when the queue is empty", async () => {
  const queue = new JobQueue({ concurrency: 2 });
  const results = [];
  for (let i = 0; i < 4; i += 1) queue.add(async () => results.push(i));
  await queue.drain();
  assert.equal(results.length, 4);
});

test("a task that throws synchronously frees its slot", async () => {
  const queue = new JobQueue({ concurrency: 1 });
  const failed = queue.add(() => {
    throw new Error("boom");
  });
  await assert.rejects(failed, /boom/);
  assert.equal(await queue.add(() => "next"), "next");
});
