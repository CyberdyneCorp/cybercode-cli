// Hidden differential tests for perf-leaderboard: the workspace's Leaderboard, service and CLI
// must behave exactly like the frozen original (grader_original/) on randomized operation
// sequences, argument errors included.
import { test } from "node:test";
import assert from "node:assert/strict";

import { Leaderboard } from "./src/leaderboard.js";
import { LeaderboardService } from "./src/service.js";
import { run } from "./src/cli.js";
import { Leaderboard as Original } from "./grader_original/leaderboard.js";
import { run as originalRun } from "./grader_original/cli.js";

function rng(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** Return value or thrown error (constructor name and message) of a call. */
function outcome(fn) {
  try {
    return { value: fn() };
  } catch (error) {
    return { error: `${error?.constructor?.name}: ${error?.message}` };
  }
}

const NAMES = ["a", "b", "B", "Z", "ab", "a b", "é", "é", "😀", "￿", "0", "10", "9", "p1", "p2", "p3", "p4", "p5", "p6", "p7"];
const PERCENTS = [0.5, 1, 7, 10, 25, 33.3, 50, 66.7, 75, 90, 99, 99.9, 100];
const BIG = Number.MAX_SAFE_INTEGER;

/** A random operation as [description, (board) => result]. */
function randomOp(random, names, { scoreSpread, maxK }) {
  const int = (n) => Math.floor(random() * n);
  const pick = (list) => list[int(list.length)];
  const player = () => (int(30) === 0 ? `ghost${int(3)}` : pick(names));
  const score = () => {
    const roll = int(20);
    if (roll === 0) return pick([BIG, -BIG, 0]);
    return int(2 * scoreSpread + 1) - scoreSpread;
  };
  const roll = int(100);
  if (roll < 38) {
    const p = player();
    const s = score();
    return [`update(${JSON.stringify(p)}, ${s})`, (b) => b.update(p, s)];
  }
  if (roll < 45) {
    const p = player();
    return [`remove(${JSON.stringify(p)})`, (b) => b.remove(p)];
  }
  if (roll < 53) {
    const p = player();
    return [`rank(${JSON.stringify(p)})`, (b) => b.rank(p)];
  }
  if (roll < 57) {
    const p = player();
    return [`score/has/size(${JSON.stringify(p)})`, (b) => [b.score(p), b.has(p), b.size]];
  }
  if (roll < 66) {
    const k = int(maxK + 1);
    return [`topK(${k})`, (b) => b.topK(k)];
  }
  if (roll < 77) {
    const p = player();
    const n = int(5) === 0 ? int(maxK + 1) : int(4);
    return [`around(${JSON.stringify(p)}, ${n})`, (b) => b.around(p, n)];
  }
  if (roll < 87) {
    const choices = [-Infinity, Infinity, -0.5, 1.5, BIG, -BIG];
    const bound = () => (int(6) === 0 ? pick(choices) : int(2 * scoreSpread + 3) - scoreSpread - 1);
    const lo = bound();
    const hi = bound();
    return [`countInRange(${lo}, ${hi})`, (b) => b.countInRange(lo, hi)];
  }
  if (roll < 96) {
    const p = int(3) === 0 ? 1 + int(100) : pick(PERCENTS);
    return [`percentile(${p})`, (b) => b.percentile(p)];
  }
  if (roll < 99) return ["entries()", (b) => b.entries()];
  return ["clear()", (b) => b.clear()];
}

function differential({ seeds, ops, names, scoreSpread, maxK, prefill = 0 }) {
  for (let seed = 1; seed <= seeds; seed += 1) {
    const random = rng(seed * 7919 + ops);
    const mine = new Leaderboard();
    const original = new Original();
    const history = [];
    for (let i = 0; i < prefill; i += 1) {
      const p = names[Math.floor(random() * names.length)];
      const s = Math.floor(random() * (2 * scoreSpread + 1)) - scoreSpread;
      mine.update(p, s);
      original.update(p, s);
    }
    for (let i = 0; i < ops; i += 1) {
      const [description, call] = randomOp(random, names, { scoreSpread, maxK });
      history.push(description);
      const expected = outcome(() => call(original));
      const actual = outcome(() => call(mine));
      assert.deepStrictEqual(actual, expected, `seed ${seed}, op ${i}: ${description}\nlast ops: ${history.slice(-8).join("; ")}`);
    }
  }
}

test("differential: small boards with ties, decreases and code-unit name order", () => {
  differential({ seeds: 400, ops: 150, names: NAMES, scoreSpread: 3, maxK: 12 });
});

test("differential: larger boards with large k and n", () => {
  const names = Array.from({ length: 1500 }, (_, i) => `u${i}`).concat(NAMES);
  differential({ seeds: 25, ops: 400, names, scoreSpread: 40, maxK: 1700, prefill: 1200 });
});

test("differential: wide score range", () => {
  const names = Array.from({ length: 200 }, (_, i) => `w${i}`);
  differential({ seeds: 40, ops: 300, names, scoreSpread: 1_000_000, maxK: 250, prefill: 150 });
});

test("argument errors match the contract", () => {
  const calls = [
    ["update('', 1)", (b) => b.update("", 1)],
    ["update(5, 1)", (b) => b.update(5, 1)],
    ["update(null, 1)", (b) => b.update(null, 1)],
    ["update('a', 1.5)", (b) => b.update("a", 1.5)],
    ["update('a', NaN)", (b) => b.update("a", Number.NaN)],
    ["update('a', 2**53)", (b) => b.update("a", 2 ** 53)],
    ["update('a', '5')", (b) => b.update("a", "5")],
    ["update(1, 1.5)", (b) => b.update(1, 1.5)],
    ["remove('')", (b) => b.remove("")],
    ["remove(7)", (b) => b.remove(7)],
    ["rank(3)", (b) => b.rank(3)],
    ["rank('')", (b) => b.rank("")],
    ["has(3)", (b) => b.has(3)],
    ["score('')", (b) => b.score("")],
    ["topK(-1)", (b) => b.topK(-1)],
    ["topK(1.5)", (b) => b.topK(1.5)],
    ["topK('3')", (b) => b.topK("3")],
    ["topK(Infinity)", (b) => b.topK(Infinity)],
    ["topK()", (b) => b.topK()],
    ["around('nobody', -1)", (b) => b.around("nobody", -1)],
    ["around('', -1)", (b) => b.around("", -1)],
    ["around('a', 1.2)", (b) => b.around("a", 1.2)],
    ["around('nobody', 2)", (b) => b.around("nobody", 2)],
    ["countInRange('1', 2)", (b) => b.countInRange("1", 2)],
    ["countInRange(NaN, 'x')", (b) => b.countInRange(Number.NaN, "x")],
    ["countInRange(1, NaN)", (b) => b.countInRange(1, Number.NaN)],
    ["countInRange(5, 1)", (b) => b.countInRange(5, 1)],
    ["percentile(0)", (b) => b.percentile(0)],
    ["percentile(101)", (b) => b.percentile(101)],
    ["percentile(NaN)", (b) => b.percentile(Number.NaN)],
    ["percentile('50')", (b) => b.percentile("50")],
    ["percentile(-5)", (b) => b.percentile(-5)],
  ];
  for (const filled of [false, true]) {
    const mine = new Leaderboard();
    const original = new Original();
    if (filled) for (const [p, s] of [["a", 3], ["b", 1], ["c", 3]]) [mine, original].forEach((b) => b.update(p, s));
    for (const [description, call] of calls) {
      assert.deepStrictEqual(outcome(() => call(mine)), outcome(() => call(original)), `${description} (board ${filled ? "filled" : "empty"})`);
    }
    assert.deepStrictEqual(mine.entries(), original.entries(), "failed calls must not change the board");
  }
});

test("returned entries are fresh copies", () => {
  const lb = new Leaderboard();
  for (const [p, s] of [["a", 5], ["b", 5], ["c", 9], ["d", 1]]) lb.update(p, s);
  for (const result of [lb.topK(4), lb.around("b", 3), lb.entries()]) {
    for (const entry of result) {
      entry.score = -1;
      entry.rank = 99;
      entry.player = "x";
    }
    result.length = 0;
  }
  assert.deepStrictEqual(lb.entries(), [
    { player: "c", score: 9, rank: 1 },
    { player: "a", score: 5, rank: 2 },
    { player: "b", score: 5, rank: 2 },
    { player: "d", score: 1, rank: 4 },
  ]);
  assert.equal(lb.rank("d"), 4);
});

test("service and CLI behave as before", () => {
  assert.equal(typeof LeaderboardService, "function");
  const verbs = ["submit", "submit", "submit", "correct", "ban", "unban", "rank", "score", "top", "around", "card", "range", "pct", "size", "close", "seasons", "bogus"];
  for (let seed = 1; seed <= 60; seed += 1) {
    const random = rng(seed);
    const int = (n) => Math.floor(random() * n);
    const lines = [];
    for (let i = 0; i < 250; i += 1) {
      const verb = verbs[int(verbs.length)];
      const p = `pl${int(25)}`;
      const s = int(41) - 20;
      const args = {
        submit: [p, s], correct: [p, s], ban: [p], unban: [p], rank: [p], score: [p],
        top: [int(8) - 1], around: [p, int(4)], card: [p], range: [int(41) - 20, int(41) - 20],
        pct: [[0, 1, 50, 99.5, 100, 120][int(6)]], size: [], close: [], seasons: [], bogus: [],
      }[verb];
      if (verb === "close" && int(4) !== 0) continue;
      lines.push([verb, ...args].join(" "));
    }
    const script = lines.join("\n");
    assert.equal(run(script), originalRun(script), `CLI script seed ${seed}`);
  }
});
