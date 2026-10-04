import { test } from "node:test";
import assert from "node:assert/strict";

import { Leaderboard } from "../src/leaderboard.js";

function board(scores) {
  const lb = new Leaderboard();
  for (const [player, score] of Object.entries(scores)) lb.update(player, score);
  return lb;
}

test("tied players share a rank", () => {
  const lb = board({ ann: 30, bob: 20, cat: 20, dan: 10 });
  assert.deepEqual(["ann", "bob", "cat", "dan"].map((p) => lb.rank(p)), [1, 2, 2, 4]);
  assert.equal(lb.rank("eve"), null);
});

test("update sets the score and returns the previous one", () => {
  const lb = board({ ann: 30 });
  assert.equal(lb.update("ann", 5), 30);
  assert.equal(lb.update("bob", 7), null);
  assert.equal(lb.score("ann"), 5);
  assert.equal(lb.rank("ann"), 2);
});

test("topK orders ties by name", () => {
  const lb = board({ cat: 20, ann: 30, bob: 20 });
  assert.deepEqual(lb.topK(3), [
    { player: "ann", score: 30, rank: 1 },
    { player: "bob", score: 20, rank: 2 },
    { player: "cat", score: 20, rank: 2 },
  ]);
});

test("around, countInRange and percentile", () => {
  const lb = board({ a: 50, b: 40, c: 30, d: 20, e: 10 });
  assert.deepEqual(lb.around("d", 1).map((e) => e.player), ["c", "d", "e"]);
  assert.equal(lb.countInRange(20, 40), 3);
  assert.equal(lb.percentile(50), 30);
  assert.equal(lb.percentile(100), 50);
});
