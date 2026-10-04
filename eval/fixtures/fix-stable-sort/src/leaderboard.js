/**
 * Sort players by score, highest first. See README.md for the full contract.
 * @param {{name: string, score: number}[]} players
 */
export function rankPlayers(players) {
  return players.sort((a, b) => a.score < b.score);
}

/**
 * Add a 1-based competition rank to each player, in leaderboard order.
 * @param {{name: string, score: number}[]} players
 */
export function withRanks(players) {
  return rankPlayers(players).map((player, index) => ({ ...player, rank: index + 1 }));
}
