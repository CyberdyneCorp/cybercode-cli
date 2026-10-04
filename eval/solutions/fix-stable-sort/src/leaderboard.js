/**
 * Sort players by score, highest first. See README.md for the full contract.
 * Array.prototype.toSorted is stable, so equal scores keep their input order.
 * @param {{name: string, score: number}[]} players
 */
export function rankPlayers(players) {
  return players.toSorted((a, b) => b.score - a.score);
}

/**
 * Add a 1-based competition rank to each player, in leaderboard order.
 * @param {{name: string, score: number}[]} players
 */
export function withRanks(players) {
  let rank = 0;
  return rankPlayers(players).map((player, index, ranked) => {
    if (index === 0 || ranked[index - 1].score !== player.score) rank = index + 1;
    return { ...player, rank };
  });
}
