/**
 * Deterministic benchmark workload: `players` inserts followed by `ops` mixed operations.
 * Shared by bench/bench.js; the hidden benchmark uses the same shape with other seeds.
 */

/** Small seeded PRNG (mulberry32) so every run sees the same operations. */
export function rng(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** Run the workload against `board` and return a checksum of every result. */
export function runWorkload(board, { players = 100_000, ops = 200_000, seed = 1 } = {}) {
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

  for (let i = 0; i < players; i += 1) {
    names.push(`player${i}`);
    board.update(names[i], int(50_000));
  }
  for (let i = 0; i < ops; i += 1) {
    const roll = int(100);
    if (roll < 40) {
      const player = int(10) === 0 ? `late${i}` : someone();
      if (player.startsWith("late")) names.push(player);
      const current = board.score(player) ?? 0;
      fold(board.update(player, int(2) === 0 ? int(50_000) : current + int(201) - 100));
    } else if (roll < 45) {
      fold(board.remove(someone()));
    } else if (roll < 57) {
      fold(board.rank(someone()));
    } else if (roll < 67) {
      fold(board.topK(int(50) === 0 ? 2_000 : int(20)));
    } else if (roll < 79) {
      fold(board.around(someone(), int(50) === 0 ? 1_000 : int(6)));
    } else if (roll < 90) {
      const lo = int(50_000);
      fold(board.countInRange(lo, lo + int(5_000)));
    } else {
      fold(board.percentile(1 + int(100)));
    }
  }
  return checksum;
}
