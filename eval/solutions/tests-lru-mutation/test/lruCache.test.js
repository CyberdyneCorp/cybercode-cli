import { test } from "node:test";
import assert from "node:assert/strict";

import { LRUCache } from "../src/lruCache.js";

/** A manual clock and an onEvict recorder. */
function setup(options = {}) {
  const clock = { t: 1000 };
  const evicted = [];
  const cache = new LRUCache({
    now: () => clock.t,
    onEvict: (key, value, reason) => evicted.push([key, value, reason]),
    ...options,
  });
  return { cache, clock, evicted };
}

const fill = (cache, ...keys) => keys.forEach((key) => cache.set(key, key.toUpperCase()));

test("constructor validates its options", () => {
  const bad = [
    [{ maxEntries: 0 }, RangeError, "maxEntries must be a positive integer"],
    [{ maxEntries: -1 }, RangeError, "maxEntries must be a positive integer"],
    [{ maxEntries: 1.5 }, RangeError, "maxEntries must be a positive integer"],
    [{ maxSize: 0 }, RangeError, "maxSize must be a positive number"],
    [{ maxSize: -3 }, RangeError, "maxSize must be a positive number"],
    [{ maxSize: "5" }, RangeError, "maxSize must be a positive number"],
    [{ ttl: -1 }, RangeError, "ttl must be a non-negative number"],
    [{ ttl: Infinity }, RangeError, "ttl must be a non-negative number"],
    [{ sizeOf: 3 }, TypeError, "sizeOf must be a function"],
    [{ now: null }, TypeError, "now must be a function"],
    [{ onEvict: "x" }, TypeError, "onEvict must be a function"],
  ];
  for (const [options, type, message] of bad) {
    assert.throws(() => new LRUCache(options), (error) => error instanceof type && error.message === message, JSON.stringify(options));
  }
  for (const options of [{}, { maxEntries: 1 }, { maxEntries: Infinity }, { maxSize: 0.5 }, { maxSize: Infinity }, { ttl: 0 }]) {
    assert.doesNotThrow(() => new LRUCache(options));
  }
  assert.doesNotThrow(() => new LRUCache());
});

test("get returns values and makes the entry most recently used", () => {
  const { cache, evicted } = setup({ maxEntries: 3 });
  fill(cache, "a", "b", "c");
  assert.equal(cache.get("a"), "A");
  assert.deepEqual(cache.keys(), ["a", "c", "b"]);
  cache.set("d", "D");
  assert.deepEqual(evicted, [["b", "B", "evict"]]);
  assert.deepEqual(cache.keys(), ["d", "a", "c"]);
  assert.equal(cache.get("zzz"), undefined);
});

test("peek, has and keys do not change recency", () => {
  const { cache, evicted } = setup({ maxEntries: 3 });
  fill(cache, "a", "b", "c");
  assert.equal(cache.peek("a"), "A");
  assert.equal(cache.has("a"), true);
  assert.equal(cache.has("nope"), false);
  assert.equal(cache.peek("nope"), undefined);
  cache.keys();
  assert.deepEqual(cache.keys(), ["c", "b", "a"]);
  cache.set("d", "D");
  assert.deepEqual(evicted, [["a", "A", "evict"]]);
});

test("delete and prune do not change recency", () => {
  const { cache, clock, evicted } = setup({ maxEntries: 3 });
  cache.set("a", "A");
  cache.set("t", "T", { ttl: 10 });
  cache.set("b", "B");
  clock.t += 10;
  assert.equal(cache.prune(), 1);
  cache.set("c", "C");
  assert.equal(cache.delete("c"), true);
  cache.set("d", "D");
  cache.set("e", "E");
  assert.deepEqual(evicted, [["t", "T", "expire"], ["c", "C", "delete"], ["a", "A", "evict"]]);
  assert.deepEqual(cache.keys(), ["e", "d", "b"]);
});

test("eviction removes least recently used entries in order", () => {
  const { cache, evicted } = setup({ maxEntries: 2 });
  fill(cache, "a", "b", "c", "d");
  assert.deepEqual(evicted, [["a", "A", "evict"], ["b", "B", "evict"]]);
  assert.deepEqual(cache.keys(), ["d", "c"]);
  assert.equal(cache.size, 2);
});

test("set evicts as many entries as needed to fit maxSize", () => {
  const { cache, evicted } = setup({ maxSize: 10, sizeOf: (value) => value.length });
  cache.set("a", "xxx");
  cache.set("b", "xxx");
  cache.set("c", "xxx");
  assert.equal(cache.totalSize, 9);
  cache.set("d", "xxxxxxxx");
  assert.deepEqual(evicted.map((e) => e[0]), ["a", "b", "c"]);
  assert.equal(cache.totalSize, 8);
  assert.deepEqual(cache.keys(), ["d"]);
  assert.equal(cache.stats().evictions, 3);
});

test("a cache filled exactly to maxSize keeps every entry", () => {
  const { cache, evicted } = setup({ maxSize: 10, sizeOf: (value) => value.length });
  cache.set("a", "xxxxx");
  cache.set("b", "xxxxx");
  assert.equal(cache.totalSize, 10);
  assert.deepEqual(evicted, []);
  assert.deepEqual(cache.keys(), ["b", "a"]);
});

test("both limits are enforced together", () => {
  const { cache, evicted } = setup({ maxEntries: 3, maxSize: 10, sizeOf: (value) => value });
  cache.set("a", 1);
  cache.set("b", 1);
  cache.set("c", 1);
  cache.set("d", 1);
  assert.deepEqual(evicted.map((e) => e[0]), ["a"]);
  cache.set("e", 9);
  assert.deepEqual(evicted.map((e) => e[0]), ["a", "b", "c"]);
  assert.deepEqual(cache.keys(), ["e", "d"]);
  assert.equal(cache.totalSize, 10);
});

test("sizeOf receives the value and the key; its result is validated", () => {
  const calls = [];
  const { cache } = setup({ sizeOf: (value, key) => { calls.push([value, key]); return 1; } });
  cache.set("k", "v");
  assert.deepEqual(calls, [["v", "k"]]);
  for (const result of [-1, Number.NaN, Infinity, "3", undefined]) {
    const { cache: other } = setup({ sizeOf: () => result });
    assert.throws(() => other.set("k", "v"), (error) => error instanceof TypeError && error.message === "sizeOf must return a non-negative finite number", String(result));
  }
  const { cache: zero } = setup({ sizeOf: () => 0 });
  assert.equal(zero.set("k", "v"), true);
});

test("values larger than maxSize are rejected and remove the old entry", () => {
  const { cache, evicted } = setup({ maxSize: 5, sizeOf: (value) => value.length });
  assert.equal(cache.set("a", "abc"), true);
  assert.equal(cache.set("b", "toolong"), false);
  assert.equal(cache.has("b"), false);
  assert.equal(cache.set("a", "toolong"), false);
  assert.equal(cache.has("a"), false);
  assert.equal(cache.size, 0);
  assert.equal(cache.totalSize, 0);
  assert.deepEqual(evicted, [["a", "abc", "set"]]);
  assert.equal(cache.set("c", "12345"), true);
  assert.equal(cache.get("c"), "12345");
});

test("set on an existing key replaces value, size, ttl and recency", () => {
  const { cache, clock, evicted } = setup({ maxEntries: 3, sizeOf: (value) => value.length, ttl: 100 });
  cache.set("a", "1");
  cache.set("b", "22");
  cache.set("c", "333");
  assert.equal(cache.totalSize, 6);
  clock.t += 60;
  cache.set("a", "4444");
  assert.deepEqual(evicted, [["a", "1", "set"]]);
  assert.equal(cache.totalSize, 9);
  assert.equal(cache.size, 3);
  assert.deepEqual(cache.keys(), ["a", "c", "b"]);
  clock.t += 50;
  assert.equal(cache.get("a"), "4444");
  assert.equal(cache.get("b"), undefined);
  assert.deepEqual(evicted.at(-1), ["b", "22", "expire"]);
  cache.set("d", "x");
  cache.set("e", "y");
  assert.deepEqual(evicted.at(-1), ["c", "333", "evict"]);
  assert.equal(cache.stats().evictions, 1);
});

test("replacing with the identical value still reports reason set", () => {
  const { cache, evicted } = setup();
  const value = { id: 1 };
  cache.set("k", value);
  cache.set("k", value);
  assert.deepEqual(evicted, [["k", value, "set"]]);
  assert.equal(cache.stats().evictions, 0);
});

test("entries expire exactly at their ttl", () => {
  const { cache, clock, evicted } = setup({ ttl: 100 });
  cache.set("a", "A");
  clock.t += 99;
  assert.equal(cache.get("a"), "A");
  clock.t += 1;
  assert.equal(cache.get("a"), undefined);
  assert.deepEqual(evicted, [["a", "A", "expire"]]);
  assert.equal(cache.size, 0);
});

test("per-entry ttl overrides the default, and ttl 0 never expires", () => {
  const { cache, clock } = setup({ ttl: 100 });
  cache.set("short", 1, { ttl: 10 });
  cache.set("forever", 2, { ttl: 0 });
  cache.set("default", 3);
  cache.set("undef", 4, { ttl: undefined });
  clock.t += 10;
  assert.equal(cache.peek("short"), undefined);
  clock.t += 1_000_000;
  assert.equal(cache.peek("forever"), 2);
  assert.equal(cache.peek("default"), undefined);
  assert.equal(cache.peek("undef"), undefined);
  const { cache: noDefault, clock: c2 } = setup();
  noDefault.set("x", 1);
  c2.t += 1e12;
  assert.equal(noDefault.get("x"), 1);
});

test("set rejects an invalid ttl option", () => {
  const { cache } = setup();
  for (const ttl of [-1, Number.NaN, Infinity, "10"]) {
    assert.throws(() => cache.set("k", 1, { ttl }), (error) => error instanceof RangeError && error.message === "ttl must be a non-negative number", String(ttl));
  }
  assert.equal(cache.size, 0);
});

test("reading never extends the expiry", () => {
  const { cache, clock } = setup({ ttl: 100 });
  cache.set("a", "A");
  clock.t += 50;
  assert.equal(cache.get("a"), "A");
  assert.equal(cache.peek("a"), "A");
  assert.equal(cache.has("a"), true);
  clock.t += 50;
  assert.equal(cache.get("a"), undefined);
});

test("get on an expired entry is a miss and removes it", () => {
  const { cache, clock, evicted } = setup({ ttl: 10 });
  cache.set("a", "A");
  clock.t += 10;
  assert.equal(cache.get("a"), undefined);
  assert.deepEqual(cache.stats(), { hits: 0, misses: 1, evictions: 0 });
  assert.deepEqual(evicted, [["a", "A", "expire"]]);
});

test("peek and has remove expired entries", () => {
  for (const method of ["peek", "has"]) {
    const { cache, clock, evicted } = setup({ ttl: 10, sizeOf: () => 4 });
    cache.set("a", "A");
    clock.t += 10;
    assert.equal(cache.size, 1);
    assert.equal(cache[method]("a"), method === "has" ? false : undefined);
    assert.deepEqual(evicted, [["a", "A", "expire"]], method);
    assert.equal(cache.size, 0);
    assert.equal(cache.totalSize, 0);
  }
});

test("expired entries count until removed and keys() skips them without removing", () => {
  const { cache, clock, evicted } = setup({ sizeOf: () => 2 });
  cache.set("a", "A", { ttl: 5 });
  cache.set("b", "B");
  clock.t += 5;
  assert.deepEqual(cache.keys(), ["b"]);
  assert.equal(cache.size, 2);
  assert.equal(cache.totalSize, 4);
  assert.deepEqual(evicted, []);
});

test("prune removes expired entries least recently used first", () => {
  const { cache, clock, evicted } = setup({ sizeOf: (value) => value });
  cache.set("a", 1, { ttl: 5 });
  cache.set("b", 2);
  cache.set("c", 3, { ttl: 5 });
  cache.set("d", 4, { ttl: 50 });
  cache.get("a");
  clock.t += 5;
  assert.equal(cache.prune(), 2);
  assert.deepEqual(evicted, [["c", 3, "expire"], ["a", 1, "expire"]]);
  assert.equal(cache.size, 2);
  assert.equal(cache.totalSize, 6);
  assert.equal(cache.prune(), 0);
  assert.equal(cache.stats().evictions, 0);
});

test("expired entries still count for eviction", () => {
  const { cache, clock, evicted } = setup({ maxEntries: 2 });
  cache.set("a", "A", { ttl: 5 });
  cache.set("b", "B");
  clock.t += 5;
  cache.set("c", "C");
  assert.deepEqual(evicted, [["a", "A", "evict"]]);
  assert.equal(cache.stats().evictions, 1);
});

test("set over an expired entry reports expire, not set", () => {
  const { cache, clock, evicted } = setup();
  cache.set("a", "old", { ttl: 5 });
  clock.t += 5;
  assert.equal(cache.set("a", "new"), true);
  assert.deepEqual(evicted, [["a", "old", "expire"]]);
  assert.equal(cache.get("a"), "new");
});

test("delete removes live entries and reports expired ones", () => {
  const { cache, clock, evicted } = setup({ sizeOf: () => 3 });
  cache.set("a", "A");
  cache.set("b", "B", { ttl: 5 });
  assert.equal(cache.delete("a"), true);
  assert.equal(cache.delete("a"), false);
  assert.equal(cache.delete("zzz"), false);
  clock.t += 5;
  assert.equal(cache.delete("b"), false);
  assert.deepEqual(evicted, [["a", "A", "delete"], ["b", "B", "expire"]]);
  assert.equal(cache.size, 0);
  assert.equal(cache.totalSize, 0);
  assert.equal(cache.stats().evictions, 0);
});

test("clear removes everything silently and keeps stats", () => {
  const { cache, evicted } = setup({ sizeOf: () => 2 });
  fill(cache, "a", "b");
  cache.get("a");
  cache.get("x");
  cache.clear();
  assert.deepEqual(evicted, []);
  assert.equal(cache.size, 0);
  assert.equal(cache.totalSize, 0);
  assert.deepEqual(cache.keys(), []);
  assert.deepEqual(cache.stats(), { hits: 1, misses: 1, evictions: 0 });
});

test("stats count get hits and misses only, and evictions", () => {
  const { cache } = setup({ maxEntries: 1 });
  cache.set("a", 1);
  cache.peek("a");
  cache.has("a");
  cache.get("a");
  cache.get("b");
  cache.set("b", 2);
  const stats = cache.stats();
  assert.deepEqual(stats, { hits: 1, misses: 1, evictions: 1 });
  stats.hits = 99;
  assert.equal(cache.stats().hits, 1);
});

test("onEvict runs after the entry is removed", () => {
  const seen = [];
  const clock = { t: 0 };
  const cache = new LRUCache({
    maxEntries: 2,
    sizeOf: () => 1,
    now: () => clock.t,
    onEvict: (key, value, reason) => seen.push([key, reason, cache.has(key), cache.size, cache.totalSize]),
  });
  cache.set("a", 1);
  cache.set("b", 2);
  cache.set("c", 3);
  cache.set("b", 4);
  cache.delete("c");
  assert.deepEqual(seen, [
    ["a", "evict", false, 2, 2],
    ["b", "set", false, 1, 1],
    ["c", "delete", false, 1, 1],
  ]);
});

test("cache works with non-string keys", () => {
  const { cache } = setup({ maxEntries: 2 });
  const key = { id: 1 };
  cache.set(key, "obj");
  cache.set(1, "one");
  assert.equal(cache.get(key), "obj");
  cache.set("1", "string one");
  assert.equal(cache.has(1), false);
  assert.deepEqual(cache.keys(), ["1", key]);
});
