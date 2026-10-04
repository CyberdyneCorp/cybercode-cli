/**
 * Game-backend service around a Leaderboard: personal-best submissions, admin corrections,
 * bans and seasons. See README.md ("Service").
 */
import { Leaderboard } from "./leaderboard.js";
import { checkPlayer, checkScore } from "./validate.js";

export class LeaderboardService {
  #board;
  #podiumSize;
  #banned = new Set();
  #seasons = [];

  constructor({ board = new Leaderboard(), podiumSize = 3 } = {}) {
    this.#board = board;
    this.#podiumSize = podiumSize;
  }

  get board() {
    return this.#board;
  }

  /** Record a run. Only a player's best score counts; banned players are ignored. */
  submit(player, score) {
    checkPlayer(player);
    checkScore(score);
    if (this.#banned.has(player)) return { accepted: false, reason: "banned" };
    const previous = this.#board.score(player);
    if (previous !== null && score <= previous) return { accepted: false, reason: "not a personal best" };
    this.#board.update(player, score);
    return { accepted: true, previous, rank: this.#board.rank(player) };
  }

  /** Admin override: set the score even when it is lower than the current one. */
  correct(player, score) {
    return this.#board.update(player, score);
  }

  ban(player) {
    checkPlayer(player);
    this.#banned.add(player);
    return this.#board.remove(player);
  }

  unban(player) {
    return this.#banned.delete(player);
  }

  /** Profile card: the player's standing and the neighbours around them. */
  card(player, radius = 2) {
    const rank = this.#board.rank(player);
    if (rank === null) return null;
    return {
      player,
      score: this.#board.score(player),
      rank,
      total: this.#board.size,
      neighbours: this.#board.around(player, radius),
    };
  }

  /** Number of players whose score is in [lo, hi]. */
  bracket(lo, hi) {
    return this.#board.countInRange(lo, hi);
  }

  /** Score needed to be in the top (100 - p) percent, by the nearest-rank percentile. */
  cutoff(p) {
    return this.#board.percentile(p);
  }

  /** Close the season: store the podium, clear the board. Bans carry over. */
  closeSeason() {
    const podium = this.#board.topK(this.#podiumSize);
    this.#seasons.push({ season: this.#seasons.length + 1, podium });
    this.#board.clear();
    return podium;
  }

  seasons() {
    return this.#seasons.map((season) => ({ ...season, podium: [...season.podium] }));
  }
}
