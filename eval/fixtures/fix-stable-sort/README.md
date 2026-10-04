# leaderboard

Weekly leaderboard helpers (`src/leaderboard.js`, ES modules, no dependencies).

Contract:

- `rankPlayers(players)` returns a **new** array sorted by `score`, highest first. Players with
  equal scores keep their input order (earlier sign-up wins). The input array is not modified.
- `withRanks(players)` returns the same order with a 1-based `rank` added to each player (a
  shallow copy; input objects are not modified). Tied scores share a rank and the next rank skips
  ("1224" competition ranking): scores 90, 80, 80, 70 get ranks 1, 2, 2, 4.

Users report that tied players come out in random order, that the leaderboard sometimes is not
sorted at all, and that ties get different ranks.

Run the tests with `node --test`.
