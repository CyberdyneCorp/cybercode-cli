import { test } from "node:test";
import assert from "node:assert/strict";

import { CancelSource, CancelledError, JobQueue } from "../src/index.js";

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

test("equal priorities run in the order they were added", async () => {
  const queue = new JobQueue({ concurrency: 1 });
  const order = [];
  queue.pause();
  const jobs = [];
  for (let i = 0; i < 20; i += 1) jobs.push(queue.add(() => order.push(i), { priority: i % 2 }));
  queue.resume();
  await Promise.all(jobs);
  const odd = [...Array(20).keys()].filter((i) => i % 2 === 1);
  const even = [...Array(20).keys()].filter((i) => i % 2 === 0);
  assert.deepEqual(order, [...odd, ...even]);
});

test("drain waits for jobs waiting to be retried", async () => {
  const timers = fakeTimers();
  const queue = new JobQueue({ retries: 1, baseDelay: 100, timers });
  let drained = false;
  queue.add(async ({ attempt }) => {
    if (attempt === 1) throw new Error("again");
  });
  queue.drain().then(() => {
    drained = true;
  });
  await timers.advance(50);
  assert.equal(drained, false);
  assert.equal(queue.stats.retrying, 1);
  await timers.advance(100);
  assert.equal(drained, true);
});

test("cancelling a job during its backoff stops it", async () => {
  const timers = fakeTimers();
  const queue = new JobQueue({ retries: 3, baseDelay: 100, timers });
  const source = new CancelSource();
  let attempts = 0;
  const job = queue.add(async () => {
    attempts += 1;
    throw new Error("flaky");
  }, { token: source.token });
  const rejected = assert.rejects(job, (error) => error instanceof CancelledError && error.reason === "stop");
  await timers.advance(50);
  source.cancel("stop");
  assert.deepEqual(queue.stats, { queued: 0, running: 0, retrying: 0, paused: false });
  await timers.advance(1000);
  await rejected;
  assert.equal(attempts, 1);
});

test("resume respects the concurrency limit", async () => {
  const timers = fakeTimers();
  const queue = new JobQueue({ concurrency: 2, timers });
  const slow = () => new Promise((resolve) => timers.setTimeout(resolve, 100));
  queue.add(slow);
  queue.add(slow);
  queue.pause();
  for (let i = 0; i < 4; i += 1) queue.add(slow);
  queue.resume();
  assert.equal(queue.stats.running, 2);
  assert.equal(queue.stats.queued, 4);
  await timers.advance(1000);
  await queue.drain();
});
