/**
 * In-memory leaderboard for one game mode. See README.md for the contract.
 *
 * Ranking order: higher score first; equal scores by player name ascending, compared by
 * UTF-16 code units (plain `<` on strings). A player's rank is 1 + the number of players with
 * a strictly greater score, so tied players share a rank.
 */
import { checkBound, checkCount, checkPercent, checkPlayer, checkScore } from "./validate.js";

/** Comparator for the ranking order. */
export function compareEntries(a, b) {
  if (a.score !== b.score) return b.score - a.score;
  if (a.player < b.player) return -1;
  if (a.player > b.player) return 1;
  return 0;
}

export class Leaderboard {
  #scores = new Map();

  /** Number of players on the board. */
  get size() {
    return this.#scores.size;
  }

  has(player) {
    return this.#scores.has(player);
  }

  /** The player's score, or null when the player is not on the board. */
  score(player) {
    return this.#scores.has(player) ? this.#scores.get(player) : null;
  }

  /** Set (not add) the player's score. Returns the previous score, or null for a new player. */
  update(player, score) {
    checkPlayer(player);
    checkScore(score);
    const previous = this.score(player);
    this.#scores.set(player, score);
    return previous;
  }

  /** Remove the player. Returns true when the player was on the board. */
  remove(player) {
    checkPlayer(player);
    return this.#scores.delete(player);
  }

  clear() {
    this.#scores.clear();
  }

  /** 1 + number of players with a strictly greater score; null when absent. */
  rank(player) {
    checkPlayer(player);
    if (!this.#scores.has(player)) return null;
    const mine = this.#scores.get(player);
    let greater = 0;
    for (const score of this.#scores.values()) {
      if (score > mine) greater += 1;
    }
    return greater + 1;
  }

  /** The first k entries in ranking order (all of them when k >= size). */
  topK(k) {
    checkCount("k", k);
    return this.#ranked().slice(0, k);
  }

  /** The player's entry with up to n entries before and after it, in ranking order. */
  around(player, n) {
    checkPlayer(player);
    checkCount("n", n);
    if (!this.#scores.has(player)) return null;
    const ranked = this.#ranked();
    const index = ranked.findIndex((entry) => entry.player === player);
    return ranked.slice(Math.max(0, index - n), index + n + 1);
  }

  /** Number of players with lo <= score <= hi. */
  countInRange(lo, hi) {
    checkBound("lo", lo);
    checkBound("hi", hi);
    let count = 0;
    for (const score of this.#scores.values()) {
      if (score >= lo && score <= hi) count += 1;
    }
    return count;
  }

  /** Nearest-rank percentile of the scores; null on an empty board. */
  percentile(p) {
    checkPercent(p);
    const n = this.#scores.size;
    if (n === 0) return null;
    const ascending = [...this.#scores.values()].sort((a, b) => a - b);
    const ordinal = Math.ceil((p * n) / 100);
    return ascending[ordinal - 1];
  }

  /** Every entry in ranking order. */
  entries() {
    return this.#ranked();
  }

  /** All entries as { player, score, rank }, sorted in ranking order. */
  #ranked() {
    const sorted = [...this.#scores].map(([player, score]) => ({ player, score })).sort(compareEntries);
    let rank = 0;
    sorted.forEach((entry, index) => {
      if (index === 0 || entry.score !== sorted[index - 1].score) rank = index + 1;
      entry.rank = rank;
    });
    return sorted;
  }
}
