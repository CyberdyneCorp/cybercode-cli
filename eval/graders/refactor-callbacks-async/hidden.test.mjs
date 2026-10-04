// Hidden differential tests for refactor-callbacks-async.
//
// Every scenario is built twice from the same seed and run once against the workspace's
// promise API (./src/index.js) and once against the frozen callback original
// (./grader_original/index.js, adapted with a promisify wrapper). A fake scheduler runs one
// timer at a time and lets all promise work settle before the next one, and records every
// side effect: scheduler calls (with ms), store hook calls (latency at the start of an
// operation, fault at its completion), iterator/task/transform calls, with virtual times.
// The logs, outcomes, settle times and final file contents must be identical.
import { test } from "node:test";
import assert from "node:assert/strict";

import * as mine from "./src/index.js";
import * as orig from "./grader_original/index.js";

const unhandled = [];
process.on("unhandledRejection", (reason) => unhandled.push(reason));

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
  return { next, int, pick: (list) => list[int(list.length)], chance: (p) => next() < p };
}

const flush = () => new Promise((resolve) => setImmediate(resolve));

/** Virtual clock: runs the earliest timer (ties by creation order), one at a time. */
class FakeScheduler {
  constructor(log) {
    this.log = log;
    this.now = 0;
    this.seq = 0;
    this.timers = [];
  }

  setTimeout(fn, ms) {
    this.log.push(["setTimeout", this.now, ms]);
    this.timers.push({ at: this.now + ms, seq: this.seq++, fn });
  }

  /** Grader-side wait (used by iterators, tasks and transforms). */
  wait(ms) {
    return new Promise((resolve) => {
      this.log.push(["wait", this.now, ms]);
      this.timers.push({ at: this.now + ms, seq: this.seq++, fn: resolve });
    });
  }

  async drain() {
    for (let steps = 0; steps < 100_000; steps += 1) {
      await flush();
      if (this.timers.length === 0) return;
      let best = 0;
      for (let i = 1; i < this.timers.length; i += 1) {
        const t = this.timers[i];
        const b = this.timers[best];
        if (t.at < b.at || (t.at === b.at && t.seq < b.seq)) best = i;
      }
      const [timer] = this.timers.splice(best, 1);
      this.now = Math.max(this.now, timer.at);
      timer.fn();
    }
    throw new Error("fake scheduler: too many timers");
  }
}

/** Describe an outcome so that the two implementations can be compared. */
function describeError(error) {
  if (!(error instanceof Error)) return { thrown: String(error) };
  const described = { name: error.constructor.name, message: error.message };
  for (const key of ["code", "attempts", "tag"]) if (key in error) described[key] = error[key];
  return described;
}

/** Track a promise: records { value | error, at } when it settles. */
function track(promise, clock) {
  const result = { settled: false };
  if (!promise || typeof promise.then !== "function") {
    result.notAPromise = true;
    return result;
  }
  promise.then(
    (value) => Object.assign(result, { settled: true, value, at: clock.now }),
    (error) => Object.assign(result, { settled: true, error: describeError(error), at: clock.now }),
  );
  return result;
}

function taggedError(tag, extra = {}) {
  return Object.assign(new Error(`failure ${tag}`), { tag }, extra);
}

// ---- adapters: the original callback API behind promises ---------------------------------

const promisify = (fn) => (...args) => new Promise((resolve, reject) => {
  fn(...args, (error, value) => (error ? reject(error) : resolve(value)));
});

/** Turn an async function into a node-style one for the original. */
const callbackify = (fn) => (...args) => {
  const cb = args.pop();
  fn(...args).then((value) => cb(null, value), (error) => cb(error));
};

function adaptStore(store) {
  const methods = ["read", "write", "append", "exists", "remove", "list", "close"];
  const adapted = Object.fromEntries(methods.map((m) => [m, promisify(store[m].bind(store))]));
  Object.defineProperty(adapted, "closed", { get: () => store.closed });
  adapted.raw = store;
  return adapted;
}

const ORIGINAL = {
  sleep: promisify(orig.sleep),
  openStore: async (options) => adaptStore(await promisify(orig.openStore)(options)),
  mapLimit: (items, limit, iterator) => promisify(orig.mapLimit)(items, limit, callbackify(iterator)),
  retry: (task, options) => promisify(orig.retry)(callbackify(task), options),
  copyFile: (store, ...args) => promisify(orig.copyFile)(store.raw, ...args),
  moveFile: (store, ...args) => promisify(orig.moveFile)(store.raw, ...args),
  readJson: (store, ...args) => promisify(orig.readJson)(store.raw, ...args),
  writeJson: (store, ...args) => promisify(orig.writeJson)(store.raw, ...args),
  runPipeline: (options) => promisify(orig.runPipeline)({ ...options, transform: callbackify(options.transform) }),
};

const MINE = {
  sleep: mine.sleep,
  openStore: mine.openStore,
  mapLimit: mine.mapLimit,
  retry: mine.retry,
  copyFile: mine.copyFile,
  moveFile: mine.moveFile,
  readJson: mine.readJson,
  writeJson: mine.writeJson,
  runPipeline: mine.runPipeline,
};

/** Path and values of the first difference between two recorded results (for messages). */
function firstDifference(actual, expected, path = "") {
  if (Object.is(actual, expected)) return null;
  const bothObjects = actual && expected && typeof actual === "object" && typeof expected === "object";
  const flat = (value) => Array.isArray(value) && value.every((item) => item === null || typeof item !== "object");
  if (!bothObjects || (flat(actual) && flat(expected))) {
    if (bothObjects && JSON.stringify(actual) === JSON.stringify(expected)) return null;
    return `${path || "value"}: got ${JSON.stringify(actual)}, expected ${JSON.stringify(expected)}`;
  }
  for (const key of new Set([...Object.keys(expected), ...Object.keys(actual)])) {
    const found = firstDifference(actual[key], expected[key], `${path}${Array.isArray(expected) ? `[${key}]` : `.${key}`}`);
    if (found) return found;
  }
  return null;
}

/** Run `scenario(api, seed)` against both implementations and compare everything recorded. */
async function differential(name, seeds, scenario) {
  for (let seed = 1; seed <= seeds; seed += 1) {
    const expected = await scenario(ORIGINAL, seed);
    const actual = await scenario(MINE, seed);
    const difference = firstDifference(actual, expected);
    assert.ok(difference === null, `${name}: seed ${seed}: ${difference}`);
    assert.deepStrictEqual(unhandled.map(describeError), [], `${name}: seed ${seed}: unhandled promise rejection`);
  }
}

/** Store hooks driven by the scenario's RNG, logging every call. */
function storeHooks(random, clock, log, { faultRate = 0.15, codes = ["EIO"], openFaults = true } = {}) {
  let faults = 0;
  return {
    scheduler: clock,
    latency: (op, path) => {
      const ms = random.int(4);
      log.push(["latency", clock.now, op, path, ms]);
      return ms;
    },
    fault: (op, path) => {
      log.push(["fault?", clock.now, op, path]);
      if (op === "open" && !openFaults) return null;
      if (!random.chance(faultRate)) return null;
      faults += 1;
      const code = op === "read" || op === "remove" ? random.pick(codes) : "EIO";
      return taggedError(`${op}-${faults}`, { code });
    },
  };
}

// ---- scenarios ----------------------------------------------------------------------------

async function storeScenario(api, seed) {
  const random = rng(seed);
  const log = [];
  const clock = new FakeScheduler(log);
  const files = new Map([["a", "A"], ["b", "B"], ["dir/c", "C"], ["dir/d", "{\"x\":1}\n"], ["bad.json", "{"]]);
  const hooks = storeHooks(random, clock, log, { faultRate: random.pick([0, 0.1, 0.3]), codes: ["EIO", "ENOENT"] });
  const opened = track(api.openStore({ files, ...hooks }), clock);
  await clock.drain();
  const results = { opened: opened.error ?? "ok" };
  if (opened.error) return { log, results, files: [...files] };
  const store = opened.value;
  const paths = ["a", "b", "dir/c", "dir/d", "bad.json", "new", "dir/new", "", 7];
  let id = 0;
  for (let batch = 0; batch < 6; batch += 1) {
    const count = 1 + random.int(4);
    for (let i = 0; i < count; i += 1) {
      const path = random.chance(0.06) ? random.pick(["", 7, null]) : random.pick(paths.slice(0, 7));
      const data = random.chance(0.05) ? 42 : `v${id}`;
      const roll = random.int(13);
      let call;
      if (roll === 0) call = ["read", () => store.read(path)];
      else if (roll === 1) call = ["write", () => store.write(path, data)];
      else if (roll === 2) call = ["append", () => store.append(path, data)];
      else if (roll === 3) call = ["exists", () => store.exists(path)];
      else if (roll === 4) call = ["remove", () => store.remove(path)];
      else if (roll === 5) call = ["list", () => store.list(random.pick(["", "dir/", "d", 5]))];
      else if (roll === 6) call = ["copyFile", () => api.copyFile(store, path, random.pick(paths.slice(0, 7)))];
      else if (roll === 7) call = ["moveFile", () => api.moveFile(store, path, random.pick(paths.slice(0, 7)))];
      else if (roll === 8) call = ["readJson", () => api.readJson(store, random.pick(["dir/d", "bad.json", "a", "nope"]))];
      else if (roll === 9) call = ["writeJson", () => api.writeJson(store, path, { id, list: [1, "two"] })];
      else if (roll === 10 && random.chance(0.3)) call = ["close", () => store.close()];
      else if (roll === 11) call = ["read", () => store.read(path)];
      else call = ["write", () => store.write(path, data)];
      results[`${id} ${call[0]}`] = track(call[1](), clock);
      results[`${id} closed`] = store.closed;
      id += 1;
    }
    await clock.drain();
  }
  if (!store.closed) results.finalClose = track(store.close(), clock);
  results.closeAgain = track(store.close(), clock);
  await clock.drain();
  return { log, results, files: [...files] };
}

async function mapLimitScenario(api, seed) {
  const random = rng(seed);
  const log = [];
  const clock = new FakeScheduler(log);
  const n = random.int(11);
  const items = Array.from({ length: n }, (_, i) => i * 3 + random.int(3));
  const limit = random.chance(0.05) ? random.pick([0, -1, 1.5, "2"]) : 1 + random.int(4);
  const failAt = new Set(Array.from({ length: n }, (_, i) => i).filter(() => random.chance(0.12)));
  const delays = items.map(() => random.int(6));
  const state = { inFlight: 0, highWater: 0 };
  const iterator = async (item, index) => {
    log.push(["start", clock.now, item, index]);
    state.inFlight += 1;
    state.highWater = Math.max(state.highWater, state.inFlight);
    await clock.wait(delays[index]);
    state.inFlight -= 1;
    log.push(["end", clock.now, index]);
    if (failAt.has(index)) throw taggedError(`item-${index}`);
    return { item, doubled: item * 2 };
  };
  const target = random.chance(0.04) ? "not an array" : items;
  const outcome = track(api.mapLimit(target, limit, iterator), clock);
  await clock.drain();
  return { log, outcome, state };
}

async function retryScenario(api, seed) {
  const random = rng(seed);
  const log = [];
  const clock = new FakeScheduler(log);
  const options = {
    attempts: random.chance(0.05) ? random.pick([0, 2.5, -3]) : 1 + random.int(5),
    baseDelay: random.pick([0, 1, 10, 100]),
    factor: random.pick([1, 2, 3]),
    maxDelay: random.pick([5, 50, 1000, 10_000]),
    scheduler: clock,
  };
  if (random.chance(0.5)) {
    options.shouldRetry = (error, attempt) => {
      log.push(["shouldRetry", clock.now, error.tag, attempt]);
      return !error.fatal;
    };
  }
  for (const key of ["baseDelay", "factor", "maxDelay"]) if (random.chance(0.15)) delete options[key];
  const plan = Array.from({ length: 6 }, (_, i) => (random.chance(0.6) ? (random.chance(0.2) ? "fatal" : "fail") : "ok"));
  const task = async (attempt) => {
    log.push(["attempt", clock.now, attempt]);
    await clock.wait(random.int(3));
    const step = plan[attempt - 1] ?? "ok";
    if (step === "ok") return `value ${attempt}`;
    throw taggedError(`attempt-${attempt}`, step === "fatal" ? { fatal: true } : {});
  };
  const outcome = track(api.retry(task, random.chance(0.03) ? undefined : options), clock);
  await clock.drain();
  return { log, outcome };
}

function makeInputs(random) {
  const files = new Map();
  const inputs = 1 + random.int(6);
  let id = 0;
  for (let i = 0; i < inputs; i += 1) {
    const lines = [];
    const count = random.int(6);
    for (let j = 0; j < count; j += 1) {
      const roll = random.int(20);
      if (roll === 0) lines.push("not json");
      else if (roll === 1) lines.push("[1,2]");
      else if (roll === 2) lines.push("   ");
      else if (roll === 3) lines.push('{"id":' + id++ + ',"v":1,"drop":true}');
      else if (roll === 4 && random.chance(0.3)) lines.push('{"id":' + id++ + ',"v":2,"poison":true}');
      else lines.push(JSON.stringify({ id: id++, v: random.int(100) }));
    }
    files.set(`in/f${i}.jsonl`, lines.join("\n") + (random.chance(0.7) ? "\n" : ""));
    if (random.chance(0.15)) files.set(`out/f${i}.jsonl`, "already done\n");
  }
  if (random.chance(0.4)) files.set("in/readme.txt", "not an input");
  if (random.chance(0.3)) files.set("other/f9.jsonl", '{"id":999}\n');
  if (random.chance(0.2)) files.set("errors.log", "old\n");
  return files;
}

async function pipelineScenario(api, seed) {
  const random = rng(seed);
  const log = [];
  const clock = new FakeScheduler(log);
  const files = makeInputs(random);
  const faultRate = random.pick([0, 0, 0.05, 0.15, 0.3]);
  const hooks = storeHooks(random, clock, log, { faultRate, codes: ["EIO", "EIO", "ENOENT"], openFaults: random.chance(0.3) });
  const transform = async (record) => {
    log.push(["transform", clock.now, record.id]);
    if (record.id % 3 === 0) await clock.wait(random.int(3));
    if (record.poison) throw taggedError(`poison-${record.id}`);
    if (record.drop) return null;
    return { id: record.id, v: record.v * 2 };
  };
  const options = {
    store: { files, ...hooks },
    transform,
    retry: { attempts: 1 + random.int(4), baseDelay: random.pick([0, 1, 5]), maxDelay: 20, scheduler: clock },
  };
  if (random.chance(0.7)) options.concurrency = 1 + random.int(3);
  if (random.chance(0.4)) options.archive = "archive/";
  if (random.chance(0.2)) options.output = "results/";
  if (random.chance(0.1)) options.errorLog = "log/bad.txt";
  const outcome = track(api.runPipeline(options), clock);
  await clock.drain();
  return { log, outcome, files: [...files].sort(([a], [b]) => (a < b ? -1 : 1)) };
}

// ---- tests ----------------------------------------------------------------------------------

test("index.js still exports every public name", () => {
  for (const name of Object.keys(orig)) assert.ok(name in mine, `missing export ${name}`);
});

test("differential: sleep", async () => {
  await differential("sleep", 1, async (api) => {
    const log = [];
    const clock = new FakeScheduler(log);
    const outcomes = [5, 0, -1, Number.NaN, "3", 2.5].map((ms) => track(api.sleep(clock, ms), clock));
    await clock.drain();
    return { log, outcomes };
  });
});

test("differential: file store operations and helpers", async () => {
  await differential("store", 300, storeScenario);
});

test("differential: openStore argument errors", async () => {
  await differential("openStore", 1, async (api) => {
    const log = [];
    const clock = new FakeScheduler(log);
    const hooks = storeHooks(rng(1), clock, log, { faultRate: 0 });
    const outcomes = [track(api.openStore({ files: {}, ...hooks }), clock), track(api.openStore({ files: [["a", "b"]], ...hooks }), clock)];
    await clock.drain();
    return { log, outcomes };
  });
});

test("differential: mapLimit ordering, concurrency and first-error semantics", async () => {
  await differential("mapLimit", 400, mapLimitScenario);
});

test("differential: retry attempts, delays and errors", async () => {
  await differential("retry", 400, retryScenario);
});

test("differential: runPipeline", async () => {
  await differential("runPipeline", 500, pipelineScenario);
});

test("differential: runPipeline argument errors", async () => {
  await differential("runPipeline args", 1, async (api) => {
    const log = [];
    const clock = new FakeScheduler(log);
    const hooks = storeHooks(rng(2), clock, log, { faultRate: 0 });
    const outcomes = [
      track(api.runPipeline({ store: hooks, transform: async (r) => r, concurrency: 0 }), clock),
    ];
    await clock.drain();
    return { log, outcomes };
  });
  const clock = new FakeScheduler([]);
  const outcome = track(mine.runPipeline({ store: { scheduler: clock } }), clock);
  await clock.drain();
  assert.deepStrictEqual(outcome.error, { name: "TypeError", message: "transform must be a function" });
  assert.deepStrictEqual(clock.log, [], "transform is checked before the store is opened");
});

test("no public function takes a callback any more", async () => {
  const log = [];
  const clock = new FakeScheduler(log);
  const calls = [];
  const spy = (...args) => calls.push(args);
  const files = new Map([["in/a.jsonl", '{"id":1}\n'], ["x", "{}"]]);
  const pending = [];
  const call = (name, fn) => {
    const result = fn();
    assert.ok(result instanceof Promise, `${name} must return a Promise`);
    pending.push(result.catch(() => {}));
  };
  call("sleep", () => mine.sleep(clock, 1, spy));
  call("mapLimit", () => mine.mapLimit([1, 2], 1, async (x) => x, spy));
  call("retry", () => mine.retry(async () => 1, { scheduler: clock }, spy));
  call("runPipeline", () => mine.runPipeline({ store: { files: new Map(files), scheduler: clock }, transform: async (r) => r, retry: { scheduler: clock } }, spy));
  const opened = mine.openStore({ files, scheduler: clock }, spy);
  assert.ok(opened instanceof Promise, "openStore must return a Promise");
  await clock.drain();
  const store = await opened;
  call("store.read", () => store.read("x", spy));
  call("store.write", () => store.write("y", "data", spy));
  call("store.append", () => store.append("y", "more", spy));
  call("store.exists", () => store.exists("x", spy));
  call("store.list", () => store.list("", spy));
  call("readJson", () => mine.readJson(store, "x", spy));
  call("writeJson", () => mine.writeJson(store, "z", {}, spy));
  call("copyFile", () => mine.copyFile(store, "x", "x2", spy));
  await clock.drain();
  call("moveFile", () => mine.moveFile(store, "x2", "x3", spy));
  await clock.drain();
  call("store.remove", () => store.remove("x3", spy));
  await clock.drain();
  call("store.close", () => store.close(spy));
  await clock.drain();
  await Promise.all(pending);
  assert.deepStrictEqual(calls, [], "a trailing callback argument must be ignored");
  assert.deepStrictEqual(unhandled.map(describeError), []);
});
