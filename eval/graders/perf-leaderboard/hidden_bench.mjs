// Hidden benchmark for perf-leaderboard. For each workload it runs the grader's reference
// Leaderboard (untimed) for the expected checksum, then times the workspace's Leaderboard and
// writes one JSON line: { name, seconds, checksum, expected }.
import { writeSync } from "node:fs";

import { Leaderboard } from "./src/leaderboard.js";
import { Leaderboard as Reference } from "./grader_reference/leaderboard.js";

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

/** Same shape as the fixture's bench/workload.js, plus tied and ascending score variants. */
function runWorkload(board, { players, ops, seed, scoreRange, ascending }) {
  const random = rng(seed);
  const int = (n) => Math.floor(random() * n);
  const names = [];
  let checksum = 0;
  const mix = (n) => {
    checksum = (Math.imul(checksum, 31) + n) | 0;
  };
  const fold = (value) => {
    if (Array.isArray(value)) {
      mix(value.length);
      for (const entry of value) {
        mix(entry.rank);
        mix(entry.score);
        mix(entry.player.length * 65_536 + entry.player.charCodeAt(entry.player.length - 1));
      }
    } else {
      mix(value === null ? -1 : Number(value));
    }
  };
  const someone = () => names[int(names.length)];
  const newScore = (current, i) => {
    if (ascending) return players + i;
    return int(2) === 0 ? int(scoreRange) : current + int(201) - 100;
  };

  for (let i = 0; i < players; i += 1) {
    names.push(`player${i}`);
    board.update(names[i], ascending ? i : int(scoreRange));
  }
  for (let i = 0; i < ops; i += 1) {
    const roll = int(100);
    if (roll < 40) {
      const player = int(10) === 0 ? `late${i}` : someone();
      if (player.startsWith("late")) names.push(player);
      fold(board.update(player, newScore(board.score(player) ?? 0, i)));
    } else if (roll < 45) {
      fold(board.remove(someone()));
    } else if (roll < 57) {
      fold(board.rank(someone()));
    } else if (roll < 67) {
      fold(board.topK(int(50) === 0 ? 2_000 : int(20)));
    } else if (roll < 79) {
      fold(board.around(someone(), int(50) === 0 ? 1_000 : int(6)));
    } else if (roll < 90) {
      const lo = int(scoreRange);
      fold(board.countInRange(lo, lo + int(Math.max(1, scoreRange / 10))));
    } else {
      fold(board.percentile(1 + int(100)));
    }
  }
  return checksum;
}

const WORKLOADS = [
  { name: "mixed", players: 100_000, ops: 200_000, seed: 20261, scoreRange: 50_000, ascending: false },
  { name: "heavy ties", players: 100_000, ops: 200_000, seed: 20262, scoreRange: 100, ascending: false },
  { name: "increasing scores", players: 100_000, ops: 200_000, seed: 20263, scoreRange: 300_000, ascending: true },
];

for (const workload of WORKLOADS) {
  const expected = runWorkload(new Reference(), workload);
  const started = performance.now();
  const checksum = runWorkload(new Leaderboard(), workload);
  const seconds = (performance.now() - started) / 1000;
  writeSync(1, `${JSON.stringify({ name: workload.name, seconds, checksum, expected })}\n`);
}
