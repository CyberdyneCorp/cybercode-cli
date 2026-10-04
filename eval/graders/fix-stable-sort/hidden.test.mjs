import { test } from "node:test";
import assert from "node:assert/strict";

import { rankPlayers, withRanks } from "./src/leaderboard.js";

// Deterministic pseudo-random generator so the grader never flakes.
function lcg(seed) {
  let state = seed;
  return () => (state = (state * 1103515245 + 12345) % 2147483648) / 2147483648;
}

function makePlayers(count, seed) {
  const next = lcg(seed);
  return Array.from({ length: count }, (_, i) => ({ name: `p${i}`, score: Math.floor(next() * 7) * 10 }));
}

function reference(players) {
  return players
    .map((player, index) => ({ player, index }))
    .sort((a, b) => b.player.score - a.player.score || a.index - b.index)
    .map(({ player }) => player.name);
}

test("visible: sorts by score descending", () => {
  const players = [{ name: "ana", score: 10 }, { name: "bo", score: 30 }, { name: "cy", score: 20 }];
  assert.deepEqual(rankPlayers(players).map((p) => p.name), ["bo", "cy", "ana"]);
});

test("ties keep input order", () => {
  const players = [
    { name: "a", score: 5 }, { name: "b", score: 9 }, { name: "c", score: 5 },
    { name: "d", score: 9 }, { name: "e", score: 5 },
  ];
  assert.deepEqual(rankPlayers(players).map((p) => p.name), ["b", "d", "a", "c", "e"]);
});

test("large inputs with many ties are sorted stably", () => {
  for (const [count, seed] of [[11, 1], [40, 7], [300, 42], [1000, 99]]) {
    const players = makePlayers(count, seed);
    assert.deepEqual(rankPlayers(players).map((p) => p.name), reference(players), `count=${count}`);
  }
});

test("handles negative and fractional scores", () => {
  const players = [{ name: "x", score: -1 }, { name: "y", score: 0.5 }, { name: "z", score: -1 }, { name: "w", score: 2 }];
  assert.deepEqual(rankPlayers(players).map((p) => p.name), ["w", "y", "x", "z"]);
});

test("does not modify the input array", () => {
  const players = makePlayers(50, 3);
  const before = players.map((p) => p.name);
  const result = rankPlayers(players);
  assert.notEqual(result, players);
  assert.deepEqual(players.map((p) => p.name), before);
});

test("empty input", () => {
  assert.deepEqual(rankPlayers([]), []);
  assert.deepEqual(withRanks([]), []);
});

test("competition ranking shares ranks and skips", () => {
  const players = [
    { name: "a", score: 80 }, { name: "b", score: 90 }, { name: "c", score: 80 },
    { name: "d", score: 70 }, { name: "e", score: 70 }, { name: "f", score: 60 },
  ];
  assert.deepEqual(
    withRanks(players).map((p) => [p.name, p.rank]),
    [["b", 1], ["a", 2], ["c", 2], ["d", 4], ["e", 4], ["f", 6]],
  );
});

test("withRanks copies players and leaves the input untouched", () => {
  const players = [{ name: "a", score: 1 }, { name: "b", score: 2 }];
  const ranked = withRanks(players);
  assert.deepEqual(players, [{ name: "a", score: 1 }, { name: "b", score: 2 }]);
  assert.ok(ranked.every((p) => !players.includes(p)));
  assert.deepEqual(ranked[0], { name: "b", score: 2, rank: 1 });
});
