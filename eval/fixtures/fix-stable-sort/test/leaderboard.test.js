import { test } from "node:test";
import assert from "node:assert/strict";

import { rankPlayers, withRanks } from "../src/leaderboard.js";

test("sorts by score descending", () => {
  const players = [
    { name: "ana", score: 10 },
    { name: "bo", score: 30 },
    { name: "cy", score: 20 },
  ];
  assert.deepEqual(rankPlayers(players).map((p) => p.name), ["bo", "cy", "ana"]);
});

test("ranks the first player 1", () => {
  const [first] = withRanks([{ name: "ana", score: 5 }]);
  assert.equal(first.rank, 1);
});
