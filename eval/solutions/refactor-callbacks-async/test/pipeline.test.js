import { test } from "node:test";
import assert from "node:assert/strict";

import { mapLimit, openStore, retry, runPipeline } from "../src/index.js";

/** A scheduler that runs timers immediately on the next macrotask. */
const instant = { setTimeout: (fn) => setImmediate(fn) };
const later = (value, ms) => new Promise((resolve) => setTimeout(() => resolve(value), ms));

test("pipeline transforms every record and writes a summary", async () => {
  const files = new Map([
    ["in/a.jsonl", '{"n":1}\n{"n":2}\n'],
    ["in/b.jsonl", '{"n":3}\nnot json\n'],
  ]);
  const transform = async (record) => (record.n === 2 ? null : { n: record.n * 10 });
  const summary = await runPipeline({ store: { files, scheduler: instant }, transform, retry: { scheduler: instant } });
  assert.deepEqual(summary, { files: 2, processed: 2, skipped: 0, missing: 0, records: 2, dropped: 1, malformed: 1 });
  assert.equal(files.get("out/a.jsonl"), '{"n":10}\n');
  assert.equal(files.get("out/b.jsonl"), '{"n":30}\n');
  assert.equal(files.get("errors.log"), "in/b.jsonl:2: not json\n");
});

test("store reports missing files", async () => {
  const store = await openStore({ scheduler: instant });
  await assert.rejects(store.read("nope"), { code: "ENOENT" });
  await store.close();
});

test("mapLimit keeps input order", async () => {
  const results = await mapLimit([1, 2, 3, 4], 2, (item) => later(item * 2, 10 - item));
  assert.deepEqual(results, [2, 4, 6, 8]);
});

test("retry gives up after the configured attempts", async () => {
  let calls = 0;
  const task = async (attempt) => {
    calls += 1;
    throw new Error(`fail ${attempt}`);
  };
  await assert.rejects(retry(task, { attempts: 3, scheduler: instant }), { message: "fail 3", attempts: 3 });
  assert.equal(calls, 3);
});

test("pipeline closes the store once when a transform fails", async () => {
  const files = new Map([["in/a.jsonl", '{"n":1}\n']]);
  const closes = [];
  const latency = (op) => {
    if (op === "close") closes.push(op);
    return 0;
  };
  const transform = async () => {
    throw new Error("bad record");
  };
  await assert.rejects(runPipeline({ store: { files, scheduler: instant, latency }, transform }), { message: "bad record" });
  assert.deepEqual(closes, ["close"]);
});
