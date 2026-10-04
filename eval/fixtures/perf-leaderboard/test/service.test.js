import { test } from "node:test";
import assert from "node:assert/strict";

import { LeaderboardService } from "../src/service.js";

test("submit keeps the personal best", () => {
  const svc = new LeaderboardService();
  assert.deepEqual(svc.submit("ann", 10), { accepted: true, previous: null, rank: 1 });
  assert.deepEqual(svc.submit("ann", 8), { accepted: false, reason: "not a personal best" });
  assert.equal(svc.board.score("ann"), 10);
});

test("closeSeason stores the podium and clears the board", () => {
  const svc = new LeaderboardService({ podiumSize: 2 });
  svc.submit("ann", 10);
  svc.submit("bob", 20);
  svc.submit("cat", 5);
  assert.deepEqual(svc.closeSeason().map((e) => e.player), ["bob", "ann"]);
  assert.equal(svc.board.size, 0);
  assert.equal(svc.seasons().length, 1);
});
