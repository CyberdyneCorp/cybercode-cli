/**
 * In-memory leaderboard for one game mode. See README.md for the contract.
 *
 * Ranking order: higher score first; equal scores by player name ascending, compared by
 * UTF-16 code units (plain `<` on strings). A player's rank is 1 + the number of players with
 * a strictly greater score, so tied players share a rank.
 *
 * Entries live in a treap (randomized balanced binary search tree) ordered by the ranking
 * order, with subtree sizes for order statistics, plus a Map from player to score. Updates,
 * rank, countInRange and percentile are O(log n) expected; topK and around are
 * O(log n + entries returned).
 */
import { checkBound, checkCount, checkPercent, checkPlayer, checkScore } from "./validate.js";

/** Comparator for the ranking order. */
export function compareEntries(a, b) {
  if (a.score !== b.score) return b.score - a.score;
  if (a.player < b.player) return -1;
  if (a.player > b.player) return 1;
  return 0;
}

class Node {
  constructor(player, score) {
    this.player = player;
    this.score = score;
    this.priority = Math.random();
    this.size = 1;
    this.left = null;
    this.right = null;
  }
}

const sizeOf = (node) => (node === null ? 0 : node.size);

function resize(node) {
  node.size = 1 + sizeOf(node.left) + sizeOf(node.right);
  return node;
}

function merge(left, right) {
  if (left === null) return right;
  if (right === null) return left;
  if (left.priority > right.priority) {
    left.right = merge(left.right, right);
    return resize(left);
  }
  right.left = merge(left, right.left);
  return resize(right);
}

/** Split into [nodes ranked before `key`, the rest]. */
function split(node, key) {
  if (node === null) return [null, null];
  if (compareEntries(node, key) < 0) {
    const [left, right] = split(node.right, key);
    node.right = left;
    return [resize(node), right];
  }
  const [left, right] = split(node.left, key);
  node.left = right;
  return [left, resize(node)];
}

function removeKey(node, key) {
  const order = compareEntries(key, node);
  if (order === 0) return merge(node.left, node.right);
  if (order < 0) node.left = removeKey(node.left, key);
  else node.right = removeKey(node.right, key);
  return resize(node);
}

/**
 * Number of nodes satisfying `inPrefix`, which must hold for a prefix of the ranking order
 * (true for every node before some point, false after it).
 */
function countPrefix(node, inPrefix) {
  let count = 0;
  while (node !== null) {
    if (inPrefix(node)) {
      count += sizeOf(node.left) + 1;
      node = node.right;
    } else {
      node = node.left;
    }
  }
  return count;
}

/** Nodes in ranking order starting at 0-based position `index`. */
function* nodesFrom(root, index) {
  const stack = [];
  let node = root;
  while (node !== null) {
    const leftSize = sizeOf(node.left);
    if (index < leftSize) {
      stack.push(node);
      node = node.left;
    } else if (index === leftSize) {
      stack.push(node);
      break;
    } else {
      index -= leftSize + 1;
      node = node.right;
    }
  }
  while (stack.length > 0) {
    const current = stack.pop();
    yield current;
    for (let child = current.right; child !== null; child = child.left) stack.push(child);
  }
}

export class Leaderboard {
  #scores = new Map();
  #root = null;

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
    if (previous !== null) this.#root = removeKey(this.#root, { player, score: previous });
    const node = new Node(player, score);
    const [before, after] = split(this.#root, node);
    this.#root = merge(merge(before, node), after);
    this.#scores.set(player, score);
    return previous;
  }

  /** Remove the player. Returns true when the player was on the board. */
  remove(player) {
    checkPlayer(player);
    if (!this.#scores.has(player)) return false;
    this.#root = removeKey(this.#root, { player, score: this.#scores.get(player) });
    this.#scores.delete(player);
    return true;
  }

  clear() {
    this.#scores.clear();
    this.#root = null;
  }

  /** 1 + number of players with a strictly greater score; null when absent. */
  rank(player) {
    checkPlayer(player);
    if (!this.#scores.has(player)) return null;
    return this.#countAbove(this.#scores.get(player)) + 1;
  }

  /** The first k entries in ranking order (all of them when k >= size). */
  topK(k) {
    checkCount("k", k);
    return this.#slice(0, k);
  }

  /** The player's entry with up to n entries before and after it, in ranking order. */
  around(player, n) {
    checkPlayer(player);
    checkCount("n", n);
    if (!this.#scores.has(player)) return null;
    const key = { player, score: this.#scores.get(player) };
    const index = countPrefix(this.#root, (node) => compareEntries(node, key) < 0);
    const start = Math.max(0, index - n);
    return this.#slice(start, index + n + 1 - start);
  }

  /** Number of players with lo <= score <= hi. */
  countInRange(lo, hi) {
    checkBound("lo", lo);
    checkBound("hi", hi);
    if (lo > hi) return 0;
    const atLeastLo = countPrefix(this.#root, (node) => node.score >= lo);
    return atLeastLo - this.#countAbove(hi);
  }

  /** Nearest-rank percentile of the scores; null on an empty board. */
  percentile(p) {
    checkPercent(p);
    const n = this.size;
    if (n === 0) return null;
    const ordinal = Math.ceil((p * n) / 100);
    // The ordinal-th smallest score is at 0-based position n - ordinal in ranking order.
    return nodesFrom(this.#root, n - ordinal).next().value.score;
  }

  /** Every entry in ranking order. */
  entries() {
    return this.#slice(0, this.size);
  }

  #countAbove(score) {
    return countPrefix(this.#root, (node) => node.score > score);
  }

  /** Up to `count` entries as { player, score, rank } from 0-based position `start`. */
  #slice(start, count) {
    const entries = [];
    if (count <= 0 || start >= this.size) return entries;
    let previous = null;
    for (const node of nodesFrom(this.#root, start)) {
      if (entries.length === count) break;
      let rank;
      if (previous === null) rank = this.#countAbove(node.score) + 1;
      else if (node.score === previous.score) rank = previous.rank;
      else rank = start + entries.length + 1;
      previous = { player: node.player, score: node.score, rank };
      entries.push(previous);
    }
    return entries;
  }
}
