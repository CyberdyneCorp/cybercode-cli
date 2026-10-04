import { test } from "node:test";
import assert from "node:assert/strict";

import { fetchAll } from "./src/fetchAll.js";
import { retry } from "./src/retry.js";

const later = (value, ms) => new Promise((resolve) => setTimeout(() => resolve(value), ms));
const failLater = (error, ms) => new Promise((_, reject) => setTimeout(() => reject(error), ms));

const unhandled = [];
process.on("unhandledRejection", (reason) => unhandled.push(reason));

/** A fake fetcher that records how many calls are in flight. */
function trackingFetcher(delays) {
  const stats = { inFlight: 0, maxInFlight: 0, calls: [] };
  const fetcher = async (url) => {
    stats.calls.push(url);
    stats.inFlight += 1;
    stats.maxInFlight = Math.max(stats.maxInFlight, stats.inFlight);
    try {
      return await later(`got:${url}`, delays[url] ?? 5);
    } finally {
      stats.inFlight -= 1;
    }
  };
  return { fetcher, stats };
}

test("visible: fetches every url and retry returns success", async () => {
  assert.deepEqual(await fetchAll(["a", "b"], (url) => later(url.toUpperCase(), 5)), ["A", "B"]);
  assert.equal(await retry(async () => "ok"), "ok");
});

test("retry passes 1-based attempt numbers and resolves on a later attempt", async () => {
  const seen = [];
  const value = await retry(async (attempt) => {
    seen.push(attempt);
    if (attempt < 3) throw new Error(`boom ${attempt}`);
    return "third time";
  }, { attempts: 5, delayMs: 1 });
  assert.equal(value, "third time");
  assert.deepEqual(seen, [1, 2, 3]);
});

test("retry rejects with the last attempt's error", async () => {
  let calls = 0;
  const errors = [new Error("first"), new Error("second"), new Error("third")];
  await assert.rejects(
    retry(() => failLater(errors[calls++], 1), { attempts: 3 }),
    (error) => error === errors[2],
  );
  assert.equal(calls, 3);
});

test("retry handles synchronous throws and default attempts", async () => {
  let calls = 0;
  const last = new Error("sync");
  await assert.rejects(retry(() => { calls += 1; throw last; }), (error) => error === last);
  assert.equal(calls, 3);
});

test("retry with one attempt does not retry", async () => {
  let calls = 0;
  await assert.rejects(retry(async () => { calls += 1; throw new Error("x"); }, { attempts: 1 }));
  assert.equal(calls, 1);
});

test("retry waits between attempts", async () => {
  const times = [];
  await assert.rejects(retry(async () => { times.push(Date.now()); throw new Error("x"); }, { attempts: 3, delayMs: 30 }));
  assert.equal(times.length, 3);
  assert.ok(times[1] - times[0] >= 25 && times[2] - times[1] >= 25, `gaps ${times}`);
});

test("results keep url order even when later urls finish first", async () => {
  const urls = ["u0", "u1", "u2", "u3", "u4", "u5"];
  const delays = { u0: 40, u1: 5, u2: 25, u3: 1, u4: 15, u5: 2 };
  const { fetcher } = trackingFetcher(delays);
  assert.deepEqual(await fetchAll(urls, fetcher, { concurrency: 3 }), urls.map((u) => `got:${u}`));
});

test("concurrency limit is reached but never exceeded", async () => {
  for (const concurrency of [1, 2, 3, 5]) {
    const urls = Array.from({ length: 12 }, (_, i) => `u${i}`);
    const delays = Object.fromEntries(urls.map((u, i) => [u, 2 + ((i * 7) % 5) * 3]));
    const { fetcher, stats } = trackingFetcher(delays);
    const results = await fetchAll(urls, fetcher, { concurrency });
    assert.deepEqual(results, urls.map((u) => `got:${u}`));
    assert.equal(stats.maxInFlight, concurrency, `concurrency=${concurrency}`);
    assert.equal(stats.calls.length, urls.length);
  }
});

test("default concurrency is 4", async () => {
  const urls = Array.from({ length: 10 }, (_, i) => `u${i}`);
  const { fetcher, stats } = trackingFetcher({});
  await fetchAll(urls, fetcher);
  assert.equal(stats.maxInFlight, 4);
});

test("a new call starts as soon as one settles", async () => {
  // u0 is slow; with concurrency 2 the other urls must run through the second slot meanwhile
  // instead of waiting for u0 as a fixed batch would.
  const urls = ["u0", "u1", "u2", "u3", "u4"];
  const { fetcher } = trackingFetcher({ u0: 300, u1: 5, u2: 5, u3: 5, u4: 5 });
  let u0Settled = false;
  const startedAfterU0 = {};
  const wrapped = (url) => {
    startedAfterU0[url] = u0Settled;
    const promise = fetcher(url);
    if (url === "u0") promise.then(() => { u0Settled = true; });
    return promise;
  };
  await fetchAll(urls, wrapped, { concurrency: 2 });
  assert.equal(startedAfterU0.u4, false, "u4 should start while u0 is still running, not wait for it");
});

test("flaky urls are retried per url", async () => {
  const failuresLeft = { a: 2, b: 0, c: 1 };
  const calls = { a: 0, b: 0, c: 0 };
  const fetcher = async (url) => {
    calls[url] += 1;
    await later(null, 2);
    if (failuresLeft[url]-- > 0) throw new Error(`flaky ${url}`);
    return url;
  };
  assert.deepEqual(await fetchAll(["a", "b", "c"], fetcher, { attempts: 3 }), ["a", "b", "c"]);
  assert.deepEqual(calls, { a: 3, b: 1, c: 2 });
});

test("a url that keeps failing rejects fetchAll with its last error", async () => {
  let attempt = 0;
  const finalError = new Error("down for good");
  const fetcher = async (url) => {
    if (url !== "bad") return later(url, 2);
    attempt += 1;
    await later(null, 2);
    throw attempt === 2 ? finalError : new Error(`try ${attempt}`);
  };
  await assert.rejects(
    fetchAll(["ok1", "bad", "ok2"], fetcher, { attempts: 2, concurrency: 2 }),
    (error) => error === finalError,
  );
  assert.equal(attempt, 2);
});

test("empty input resolves to an empty array", async () => {
  assert.deepEqual(await fetchAll([], async () => { throw new Error("unused"); }), []);
});

test("no unhandled rejections", async () => {
  await later(null, 50);
  assert.deepEqual(unhandled, []);
});
