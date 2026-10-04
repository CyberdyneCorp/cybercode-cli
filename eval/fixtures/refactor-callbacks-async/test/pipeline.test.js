import { test } from "node:test";
import assert from "node:assert/strict";

import { mapLimit, openStore, retry, runPipeline } from "../src/index.js";

/** A scheduler that runs timers immediately on the next macrotask. */
const instant = { setTimeout: (fn) => setImmediate(fn) };

test("pipeline transforms every record and writes a summary", (t, end) => {
  const files = new Map([
    ["in/a.jsonl", '{"n":1}\n{"n":2}\n'],
    ["in/b.jsonl", '{"n":3}\nnot json\n'],
  ]);
  const transform = (record, cb) => cb(null, record.n === 2 ? null : { n: record.n * 10 });
  runPipeline({ store: { files, scheduler: instant }, transform, retry: { scheduler: instant } }, (error, summary) => {
    assert.equal(error, null);
    assert.deepEqual(summary, { files: 2, processed: 2, skipped: 0, missing: 0, records: 2, dropped: 1, malformed: 1 });
    assert.equal(files.get("out/a.jsonl"), '{"n":10}\n');
    assert.equal(files.get("out/b.jsonl"), '{"n":30}\n');
    assert.equal(files.get("errors.log"), "in/b.jsonl:2: not json\n");
    end();
  });
});

test("store reports missing files", (t, end) => {
  openStore({ scheduler: instant }, (error, store) => {
    assert.equal(error, null);
    store.read("nope", (readError) => {
      assert.equal(readError.code, "ENOENT");
      store.close(end);
    });
  });
});

test("mapLimit keeps input order", (t, end) => {
  const iterator = (item, index, cb) => setTimeout(() => cb(null, item * 2), 10 - item);
  mapLimit([1, 2, 3, 4], 2, iterator, (error, results) => {
    assert.equal(error, null);
    assert.deepEqual(results, [2, 4, 6, 8]);
    end();
  });
});

test("retry gives up after the configured attempts", (t, end) => {
  let calls = 0;
  const task = (attempt, cb) => {
    calls += 1;
    cb(new Error(`fail ${attempt}`));
  };
  retry(task, { attempts: 3, scheduler: instant }, (error) => {
    assert.equal(error.message, "fail 3");
    assert.equal(error.attempts, 3);
    assert.equal(calls, 3);
    end();
  });
});
