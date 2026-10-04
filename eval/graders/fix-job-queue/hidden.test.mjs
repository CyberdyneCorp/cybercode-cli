// Hidden tests for fix-job-queue: targeted tests for every README clause plus randomized
// scenarios on a deterministic fake clock that check the contract's invariants at every step.
import { after, test as nodeTest } from "node:test";
import assert from "node:assert/strict";

import { CancelSource, CancelledError, JobQueue } from "./src/index.js";

const flush = () => new Promise((resolve) => setImmediate(resolve));

// Keep the event loop alive so that a test whose promises never settle fails by its own
// timeout instead of cancelling every test after it.
const keepAlive = setInterval(() => {}, 1000);
after(() => clearInterval(keepAlive));
const test = (name, fn) => nodeTest(name, { timeout: 2000 }, fn);

/** Deterministic fake clock. `queueTimers` is the view handed to a JobQueue (its calls are logged). */
class FakeClock {
  now = 0;
  #seq = 0;
  #pending = new Map();
  queueCalls = [];

  setTimeout(fn, ms) {
    const handle = { id: ++this.#seq };
    this.#pending.set(handle, { at: this.now + ms, seq: handle.id, fn });
    return handle;
  }

  clearTimeout(handle) {
    this.#pending.delete(handle);
  }

  get queueTimers() {
    return {
      setTimeout: (fn, ms) => {
        const handle = this.setTimeout(() => {
          this.onQueueTimer?.();
          fn();
        }, ms);
        this.queueCalls.push(["setTimeout", this.now, ms, handle.id]);
        return handle;
      },
      clearTimeout: (handle) => {
        this.queueCalls.push(["clearTimeout", this.now, handle?.id]);
        this.clearTimeout(handle);
      },
    };
  }

  sleep(ms) {
    return new Promise((resolve) => this.setTimeout(resolve, ms));
  }

  /** Run the timers due up to `until` one at a time, settling promise work after each. */
  async runUntil(until, afterEach = () => {}) {
    for (;;) {
      await flush();
      afterEach();
      let next = null;
      for (const [handle, timer] of this.#pending) {
        if (timer.at <= until && (next === null || timer.at < next[1].at || (timer.at === next[1].at && timer.seq < next[1].seq))) {
          next = [handle, timer];
        }
      }
      if (next === null) break;
      this.#pending.delete(next[0]);
      this.now = Math.max(this.now, next[1].at);
      next[1].fn();
    }
    this.now = Math.max(this.now, until);
  }

  advance(ms, afterEach) {
    return this.runUntil(this.now + ms, afterEach);
  }

  get idle() {
    return this.#pending.size === 0;
  }
}

/** Record every event as [time, event, id, attempt, detail]. */
function recordEvents(queue, clock) {
  const events = [];
  for (const name of ["start", "retry", "success", "failure", "cancel"]) {
    queue.on(name, (job, payload, delay) => {
      const detail = name === "retry" ? [payload?.message, delay] : name === "start" ? [] : [payload instanceof Error ? payload.message : payload];
      events.push([clock.now, name, job.id, job.attempt, ...detail]);
    });
  }
  return events;
}

/** Observe a promise's outcome without leaving rejections unhandled. */
function observe(promise) {
  const state = { settled: false };
  promise.then(
    (value) => Object.assign(state, { settled: true, value }),
    (error) => Object.assign(state, { settled: true, error }),
  );
  return state;
}

const isCancelled = (error, reason) => error instanceof CancelledError && error.name === "CancelledError" && error.message === "job cancelled" && error.reason === reason;

// ---- targeted tests -------------------------------------------------------------------------

test("a synchronous throw releases the slot and is retried like a rejection", async () => {
  const clock = new FakeClock();
  const queue = new JobQueue({ concurrency: 1, retries: 1, baseDelay: 100, timers: clock.queueTimers });
  const events = recordEvents(queue, clock);
  const a = observe(queue.add(({ attempt }) => {
    if (attempt === 1) throw new Error("sync boom");
    return "a ok";
  }));
  const b = observe(queue.add(() => clock.sleep(30).then(() => "b ok")));
  await clock.advance(0);
  assert.deepEqual(queue.stats, { queued: 0, running: 1, retrying: 1, paused: false });
  await clock.advance(200);
  assert.equal(a.value, "a ok");
  assert.equal(b.value, "b ok");
  assert.deepEqual(events, [
    [0, "start", 1, 1],
    [0, "retry", 1, 1, "sync boom", 100],
    [0, "start", 2, 1],
    [30, "success", 2, 1, "b ok"],
    [100, "start", 1, 2],
    [100, "success", 1, 2, "a ok"],
  ]);
});

test("synchronous throws never leak slots", async () => {
  const queue = new JobQueue({ concurrency: 2 });
  const outcomes = [];
  for (let i = 0; i < 10; i += 1) {
    outcomes.push(observe(queue.add(() => {
      throw new Error(`boom ${i}`);
    })));
  }
  outcomes.push(observe(queue.add(async () => "last")));
  await queue.drain();
  await flush();
  assert.equal(outcomes.filter((o) => o.error?.message?.startsWith("boom")).length, 10);
  assert.equal(outcomes.at(-1).value, "last");
  assert.deepEqual(queue.stats, { queued: 0, running: 0, retrying: 0, paused: false });
});

test("equal priorities run in the order they were added", async () => {
  const queue = new JobQueue({ concurrency: 1 });
  const started = [];
  queue.pause();
  const jobs = [];
  for (let i = 0; i < 40; i += 1) {
    const priority = [0, 2, 1, 2, 0][i % 5];
    jobs.push(queue.add(() => started.push(i), { priority, id: `j${i}` }));
  }
  queue.resume();
  await Promise.all(jobs);
  const expected = [...Array(40).keys()].sort((x, y) => [0, 2, 1, 2, 0][y % 5] - [0, 2, 1, 2, 0][x % 5] || x - y);
  assert.deepEqual(started, expected);
});

test("FIFO holds while jobs keep arriving", async () => {
  const clock = new FakeClock();
  const queue = new JobQueue({ concurrency: 2, timers: clock.queueTimers });
  const started = [];
  const jobs = [];
  for (let i = 0; i < 30; i += 1) {
    jobs.push(queue.add(async () => {
      started.push(i);
      await clock.sleep(1 + (i % 3));
    }, { priority: i % 2 }));
  }
  await clock.advance(100);
  await Promise.all(jobs);
  // The first two start at once; the rest by priority, FIFO within a priority.
  const rest = [...Array(30).keys()].slice(2).sort((x, y) => (y % 2) - (x % 2) || x - y);
  assert.deepEqual(started, [0, 1, ...rest]);
});

test("a retried job re-enters behind equal-priority jobs already waiting", async () => {
  const clock = new FakeClock();
  const queue = new JobQueue({ concurrency: 1, baseDelay: 10, timers: clock.queueTimers });
  const events = recordEvents(queue, clock);
  queue.add(async ({ attempt }) => {
    if (attempt === 1) throw new Error("first");
    return "A";
  }, { id: "A", priority: 1, retries: 1 });
  queue.add(() => clock.sleep(50), { id: "B", priority: 1 });
  await clock.advance(5);
  queue.add(() => "C", { id: "C", priority: 1 });
  await clock.advance(100);
  assert.deepEqual(events.filter((e) => e[1] === "start").map((e) => [e[0], e[2]]), [[0, "A"], [0, "B"], [50, "C"], [50, "A"]]);
});

test("backoff delays double and are capped at maxDelay", async () => {
  const clock = new FakeClock();
  const queue = new JobQueue({ retries: 5, baseDelay: 100, maxDelay: 300, timers: clock.queueTimers });
  const events = recordEvents(queue, clock);
  const outcome = observe(queue.add(async ({ attempt }) => {
    throw new Error(`fail ${attempt}`);
  }));
  await clock.advance(5000);
  assert.equal(outcome.error.message, "fail 6");
  assert.deepEqual(events.filter((e) => e[1] === "retry").map((e) => e[5]), [100, 200, 300, 300, 300]);
  assert.deepEqual(clock.queueCalls.map((c) => [c[0], c[2]]), [100, 200, 300, 300, 300].map((ms) => ["setTimeout", ms]));
  assert.deepEqual(events.filter((e) => e[1] === "start").map((e) => e[0]), [0, 100, 300, 600, 900, 1200]);
  assert.deepEqual(events.at(-1), [1200, "failure", 1, 6, "fail 6"]);
});

test("a job is attempted at most retries + 1 times, per-job retries override", async () => {
  const clock = new FakeClock();
  const queue = new JobQueue({ retries: 2, baseDelay: 1, timers: clock.queueTimers });
  const counts = { a: 0, b: 0, c: 0 };
  const failing = (name) => async () => {
    counts[name] += 1;
    throw new Error(name);
  };
  const outcomes = [observe(queue.add(failing("a"))), observe(queue.add(failing("b"), { retries: 0 })), observe(queue.add(failing("c"), { retries: 4 }))];
  await clock.advance(1000);
  assert.deepEqual(counts, { a: 3, b: 1, c: 5 });
  assert.deepEqual(outcomes.map((o) => o.error.message), ["a", "b", "c"]);
});

test("drain waits for jobs that are backing off", async () => {
  const clock = new FakeClock();
  const queue = new JobQueue({ retries: 1, baseDelay: 100, timers: clock.queueTimers });
  const job = observe(queue.add(async ({ attempt }) => {
    if (attempt === 1) throw new Error("again");
    await clock.sleep(10);
    return "ok";
  }));
  const drained = observe(queue.drain());
  await clock.advance(50);
  assert.equal(drained.settled, false, "drain resolved while a job was waiting for its retry");
  assert.deepEqual(queue.stats, { queued: 0, running: 0, retrying: 1, paused: false });
  await clock.advance(55);
  assert.equal(drained.settled, false, "drain resolved while the retry was running");
  await clock.advance(10);
  assert.equal(job.value, "ok");
  assert.equal(drained.settled, true);
  assert.equal(drained.value, undefined);
});

test("drain resolves when idle, after failures, and not while paused with queued jobs", async () => {
  const clock = new FakeClock();
  const queue = new JobQueue({ timers: clock.queueTimers });
  const idle = observe(queue.drain());
  await flush();
  assert.equal(idle.settled, true);
  observe(queue.add(async () => {
    throw new Error("x");
  }));
  const afterFailure = observe(queue.drain());
  await clock.advance(1);
  assert.equal(afterFailure.settled, true);
  assert.equal(afterFailure.error, undefined);
  queue.pause();
  queue.add(() => "later");
  const paused = observe(queue.drain());
  await clock.advance(100);
  assert.equal(paused.settled, false);
  queue.resume();
  await clock.advance(1);
  assert.equal(paused.settled, true);
});

test("cancelling during backoff prevents further attempts", async () => {
  const clock = new FakeClock();
  const queue = new JobQueue({ retries: 3, baseDelay: 100, timers: clock.queueTimers });
  const events = recordEvents(queue, clock);
  const source = new CancelSource();
  let attempts = 0;
  const job = observe(queue.add(async () => {
    attempts += 1;
    throw new Error("flaky");
  }, { token: source.token }));
  const drained = observe(queue.drain());
  await clock.advance(50);
  source.cancel("shutdown");
  assert.deepEqual(queue.stats, { queued: 0, running: 0, retrying: 0, paused: false });
  await clock.advance(0);
  assert.ok(isCancelled(job.error, "shutdown"), "rejects with CancelledError and the reason");
  assert.equal(drained.settled, true, "drain resolves once the cancelled job is gone");
  const handle = clock.queueCalls[0][3];
  assert.deepEqual(clock.queueCalls.slice(1), [["clearTimeout", 50, handle]]);
  await clock.advance(10_000);
  assert.equal(attempts, 1);
  assert.deepEqual(events, [[0, "start", 1, 1], [0, "retry", 1, 1, "flaky", 100], [50, "cancel", 1, 1, "shutdown"]]);
});

test("cancelling a queued job removes it", async () => {
  const clock = new FakeClock();
  const queue = new JobQueue({ concurrency: 1, timers: clock.queueTimers });
  const events = recordEvents(queue, clock);
  const source = new CancelSource();
  queue.add(() => clock.sleep(10), { id: "busy" });
  let ran = false;
  const job = observe(queue.add(() => { ran = true; }, { id: "victim", token: source.token }));
  const other = observe(queue.add(() => "other", { id: "other" }));
  await clock.advance(1);
  source.cancel();
  assert.deepEqual(queue.stats, { queued: 1, running: 1, retrying: 0, paused: false });
  await clock.advance(50);
  assert.equal(ran, false);
  assert.ok(isCancelled(job.error, "cancelled"), "default reason is \"cancelled\"");
  assert.equal(other.value, "other");
  assert.deepEqual(events.filter((e) => e[2] === "victim"), [[1, "cancel", "victim", 0, "cancelled"]]);
});

test("adding with an already-cancelled token never starts the job", async () => {
  const queue = new JobQueue();
  const events = recordEvents(queue, { now: 0 });
  const source = new CancelSource();
  source.cancel("too late");
  let ran = false;
  const job = observe(queue.add(() => { ran = true; }, { token: source.token }));
  assert.deepEqual(events, [[0, "cancel", 1, 0, "too late"]]);
  await queue.drain();
  await flush();
  assert.equal(ran, false);
  assert.ok(isCancelled(job.error, "too late"));
});

test("cancelling a running attempt lets it finish", async () => {
  const clock = new FakeClock();
  const queue = new JobQueue({ retries: 3, baseDelay: 10, timers: clock.queueTimers });
  const events = recordEvents(queue, clock);
  const ok = new CancelSource();
  const bad = new CancelSource();
  let badAttempts = 0;
  const succeeds = observe(queue.add(async ({ token }) => {
    await clock.sleep(20);
    return token.cancelled ? "saw cancel" : "no cancel";
  }, { id: "ok", token: ok.token }));
  const fails = observe(queue.add(async () => {
    badAttempts += 1;
    await clock.sleep(20);
    throw new Error("broken");
  }, { id: "bad", token: bad.token, priority: -1 }));
  await clock.advance(5);
  ok.cancel("stop ok");
  await clock.advance(30);
  assert.equal(succeeds.value, "saw cancel");
  await clock.advance(30);
  bad.cancel("stop bad");
  await clock.advance(100);
  assert.equal(badAttempts, 2);
  assert.ok(isCancelled(fails.error, "stop bad"));
  assert.deepEqual(events.filter((e) => e[2] === "bad"), [
    [20, "start", "bad", 1],
    [40, "retry", "bad", 1, "broken", 10],
    [50, "start", "bad", 2],
    [70, "cancel", "bad", 2, "stop bad"],
  ]);
  ok.cancel();
  bad.cancel();
  await clock.advance(10);
  assert.equal(events.length, 6);
});

test("resume never starts more than concurrency jobs", async () => {
  const clock = new FakeClock();
  const queue = new JobQueue({ concurrency: 2, timers: clock.queueTimers });
  let inFlight = 0;
  let highWater = 0;
  const task = (ms) => async () => {
    inFlight += 1;
    highWater = Math.max(highWater, inFlight);
    await clock.sleep(ms);
    inFlight -= 1;
  };
  queue.add(task(100));
  queue.add(task(100));
  await clock.advance(10);
  queue.pause();
  for (let i = 0; i < 5; i += 1) queue.add(task(10));
  queue.resume();
  assert.deepEqual(queue.stats, { queued: 5, running: 2, retrying: 0, paused: false });
  queue.pause();
  queue.pause();
  await clock.advance(95);
  assert.equal(queue.stats.running, 0, "pause lets running jobs finish");
  assert.equal(queue.stats.queued, 5);
  queue.resume();
  queue.resume();
  assert.deepEqual(queue.stats, { queued: 3, running: 2, retrying: 0, paused: false });
  await clock.advance(1000);
  assert.equal(highWater, 2);
});

test("a backoff that ends while paused waits in the queue", async () => {
  const clock = new FakeClock();
  const queue = new JobQueue({ retries: 1, baseDelay: 10, timers: clock.queueTimers });
  const events = recordEvents(queue, clock);
  queue.add(async ({ attempt }) => {
    if (attempt === 1) throw new Error("once");
    return "fine";
  });
  await clock.advance(1);
  queue.pause();
  await clock.advance(50);
  assert.deepEqual(queue.stats, { queued: 1, running: 0, retrying: 0, paused: true });
  queue.resume();
  await clock.advance(1);
  assert.deepEqual(events.map((e) => [e[0], e[1]]), [[0, "start"], [0, "retry"], [51, "start"], [51, "success"]]);
});

test("job objects, ids and event payloads", async () => {
  const clock = new FakeClock();
  const queue = new JobQueue({ retries: 1, baseDelay: 5, timers: clock.queueTimers });
  const seen = [];
  queue.on("start", (job) => seen.push(["start", job, job.attempt]));
  queue.on("retry", (job, error, delay) => seen.push(["retry", job, error.message, delay]));
  queue.on("success", (job, value) => seen.push(["success", job, value]));
  queue.on("failure", (job, error) => seen.push(["failure", job, error.message]));
  const first = observe(queue.add(async ({ attempt }) => {
    if (attempt === 1) throw new Error("nope");
    return 42;
  }, { priority: 3 }));
  const second = observe(queue.add(async () => {
    throw new Error("always");
  }, { id: "custom", retries: 0 }));
  const third = observe(queue.add(() => "sync value"));
  await clock.advance(100);
  assert.equal(first.value, 42);
  assert.equal(second.error.message, "always");
  assert.equal(third.value, "sync value");
  const jobOne = seen[0][1];
  assert.deepEqual(jobOne, { id: 1, priority: 3, attempt: 2 });
  assert.ok(seen.filter((s) => s[1].id === 1).every((s) => s[1] === jobOne), "the same job object is passed to every event");
  assert.deepEqual(seen.map((s) => [s[0], s[1].id, ...s.slice(2)]), [
    ["start", 1, 1], ["retry", 1, "nope", 5], ["start", "custom", 1], ["failure", "custom", "always"],
    ["start", 2, 1], ["success", 2, "sync value"], ["start", 1, 2], ["success", 1, 42],
  ]);
});

test("the job promise settles after the final event's listeners ran", async () => {
  const queue = new JobQueue();
  const order = [];
  queue.on("success", () => order.push("event"));
  queue.on("failure", () => order.push("failure event"));
  const ok = queue.add(async () => "v").then(() => order.push("resolved"));
  const bad = queue.add(async () => {
    throw new Error("e");
  }).catch(() => order.push("rejected"));
  await Promise.all([ok, bad]);
  assert.deepEqual(order, ["event", "resolved", "failure event", "rejected"]);
});

test("argument validation", () => {
  for (const options of [{ concurrency: 0 }, { concurrency: 1.5 }, { retries: -1 }, { retries: 0.5 }, { baseDelay: -1 }, { maxDelay: Number.NaN }]) {
    assert.throws(() => new JobQueue(options), RangeError, JSON.stringify(options));
  }
  const queue = new JobQueue();
  assert.throws(() => queue.add("not a function"), TypeError);
  assert.throws(() => queue.add(() => 1, { priority: Infinity }), TypeError);
  assert.throws(() => queue.add(() => 1, { priority: "1" }), TypeError);
  assert.throws(() => queue.add(() => 1, { retries: -2 }), RangeError);
  assert.deepEqual(queue.stats, { queued: 0, running: 0, retrying: 0, paused: false });
});

// ---- randomized invariant checks ------------------------------------------------------------

function rng(seed) {
  let a = seed >>> 0;
  const next = () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
  const int = (n) => Math.floor(next() * n);
  return { int, chance: (p) => next() < p, pick: (list) => list[int(list.length)] };
}

async function randomScenario(seed) {
  const random = rng(seed);
  const clock = new FakeClock();
  const concurrency = 1 + random.int(3);
  const baseDelay = random.pick([0, 1, 5, 20]);
  const maxDelay = random.pick([5, 40, 1000]);
  const queue = new JobQueue({ concurrency, retries: random.int(3), baseDelay, maxDelay, timers: clock.queueTimers });
  const where = (message) => `seed ${seed}, t=${clock.now}: ${message}`;
  const jobs = new Map();
  let inFlight = 0;
  let addOrder = 0;
  const drains = [];
  let paused = false;
  let retrySeq = 0;

  const check = () => {
    const stats = queue.stats;
    assert.ok(inFlight <= concurrency, where(`${inFlight} attempts running, concurrency ${concurrency}`));
    assert.equal(stats.running, inFlight, where("stats.running does not match the attempts in flight"));
    if (!paused) assert.ok(stats.queued === 0 || stats.running === concurrency, where(`a slot is free while ${stats.queued} jobs wait`));
    const queued = [...jobs.values()].filter((j) => j.state === "queued").length;
    const backoff = [...jobs.values()].filter((j) => j.state === "backoff").length;
    assert.equal(stats.queued, queued, where("stats.queued is wrong"));
    assert.equal(stats.retrying, backoff, where("stats.retrying is wrong"));
    const idle = stats.queued === 0 && stats.running === 0 && stats.retrying === 0;
    for (const drain of drains) {
      if (drain.settled && !drain.checked) {
        assert.ok(idle, where("drain() resolved while jobs were queued, running or retrying"));
        drain.checked = true;
      }
      if (idle) assert.ok(drain.settled, where("drain() did not resolve although the queue is idle"));
    }
  };

  queue.on("start", (job) => {
    const me = jobs.get(job.id);
    assert.equal(me.state, "queued", where(`job ${job.id} started from state ${me.state}`));
    for (const other of jobs.values()) {
      if (other === me || other.state !== "queued") continue;
      const earlier = other.since < me.since || (other.since === me.since && other.viaAdd && me.viaAdd && other.order < me.order);
      if (other.since < clock.now || (other.viaAdd && me.viaAdd && other.since === me.since)) {
        assert.ok(other.priority <= me.priority, where(`job ${job.id} (priority ${me.priority}) started before waiting job ${other.id} (priority ${other.priority})`));
        if (other.priority === me.priority) assert.ok(!earlier, where(`job ${job.id} started before job ${other.id}, which entered the queue first with the same priority`));
      }
    }
    me.state = "running";
    me.starts += 1;
    assert.ok(me.starts <= me.retries + 1, where(`job ${job.id} attempted ${me.starts} times with retries ${me.retries}`));
    assert.equal(job.attempt, me.starts, where("job.attempt is wrong"));
    me.events.push("start");
  });
  queue.on("retry", (job, error, delay) => {
    const me = jobs.get(job.id);
    assert.equal(delay, Math.min(maxDelay, baseDelay * 2 ** (job.attempt - 1)), where("wrong backoff delay"));
    me.state = "backoff";
    me.reenter = clock.now + delay;
    me.retrySeq = retrySeq++;
    me.events.push("retry");
  });
  for (const name of ["success", "failure", "cancel"]) {
    queue.on(name, (job, payload) => {
      const me = jobs.get(job.id);
      me.state = "done";
      me.events.push(name);
      me.final = [name, payload];
    });
  }

  const addJob = () => {
    const id = `j${addOrder}`;
    const source = random.chance(0.35) ? new CancelSource() : null;
    const me = {
      id, priority: random.int(3), retries: random.int(3), source, state: "queued", since: clock.now, viaAdd: true,
      order: addOrder++, starts: 0, events: [], plan: Array.from({ length: 4 }, () => random.pick(["ok", "ok", "reject", "throw", "syncok"])),
      durations: Array.from({ length: 4 }, () => random.int(8)),
    };
    jobs.set(id, me);
    me.promise = observe(queue.add(({ attempt }) => {
      const step = me.plan[attempt - 1];
      if (step === "throw") throw new Error(`${id} threw ${attempt}`);
      if (step === "syncok") return `${id} sync`;
      inFlight += 1;
      return clock.sleep(me.durations[attempt - 1]).then(() => {
        inFlight -= 1;
        if (step === "reject") throw new Error(`${id} rejected ${attempt}`);
        return `${id} ok`;
      });
    }, { id, priority: me.priority, retries: me.retries, token: source?.token }));
  };

  // The queue's only timers are backoff waits: when one fires, the job whose delay ended first
  // (earliest retry among equal times) re-enters the queue.
  clock.onQueueTimer = () => {
    const due = [...jobs.values()].filter((j) => j.state === "backoff" && j.reenter <= clock.now);
    assert.ok(due.length > 0, where("a queue timer fired but no job's backoff delay has ended"));
    const me = due.reduce((a, b) => (a.reenter < b.reenter || (a.reenter === b.reenter && a.retrySeq < b.retrySeq) ? a : b));
    me.state = "queued";
    me.since = clock.now;
    me.viaAdd = false;
  };
  const afterTimer = check;

  for (let step = 0; step < 60; step += 1) {
    await clock.advance(random.int(6), afterTimer);
    const roll = random.int(20);
    if (roll < 11) {
      addJob();
    } else if (roll < 14) {
      const cancellable = [...jobs.values()].filter((j) => j.source && j.state !== "done");
      if (cancellable.length > 0) {
        const victim = random.pick(cancellable);
        const before = victim.state;
        victim.source.cancel(`stop ${victim.id}`);
        if (before === "queued" || before === "backoff") {
          assert.equal(victim.state, "done", where(`cancelling ${before} job ${victim.id} did not end it`));
          assert.deepEqual(victim.final, ["cancel", `stop ${victim.id}`], where(`cancelled job ${victim.id} did not emit cancel`));
        }
      }
    } else if (roll < 16) {
      queue.pause();
      paused = true;
    } else if (roll < 18) {
      queue.resume();
      paused = false;
    } else {
      drains.push(observe(queue.drain()));
    }
    await flush();
    check();
  }
  queue.resume();
  paused = false;
  await clock.advance(100_000, afterTimer);
  assert.ok(clock.idle, where("timers still pending at the end"));
  for (const me of jobs.values()) {
    assert.equal(me.state, "done", where(`job ${me.id} never finished (${me.events.join(" ")})`));
    assert.match(me.events.join(" "), /^(start( retry start)*( retry)? (success|failure|cancel)|cancel)$/, where(`job ${me.id} events`));
    assert.ok(me.promise.settled, where(`job ${me.id} promise never settled`));
    const [kind, payload] = me.final;
    if (kind === "success") assert.equal(me.promise.value, payload, where(`job ${me.id} resolved with another value`));
    if (kind === "failure") assert.equal(me.promise.error, payload, where(`job ${me.id} rejected with another error`));
    if (kind === "cancel") assert.ok(isCancelled(me.promise.error, payload), where(`job ${me.id} cancel rejection`));
    if (kind === "failure") assert.equal(me.starts, me.retries + 1, where(`job ${me.id} failed before using its retries`));
  }
  for (const drain of drains) assert.ok(drain.settled, where("a drain() never resolved"));
  assert.deepEqual(queue.stats, { queued: 0, running: 0, retrying: 0, paused: false }, where("final stats"));
}

nodeTest("randomized scenarios keep every invariant", { timeout: 30_000 }, async () => {
  for (let seed = 1; seed <= 400; seed += 1) await randomScenario(seed);
});
