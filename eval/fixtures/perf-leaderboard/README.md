# arena-leaderboard

Leaderboard for the arena game backend (Node 22, ES modules, no dependencies).

- `src/leaderboard.js` - the `Leaderboard` class (contract below).
- `src/service.js` - `LeaderboardService`: personal-best submissions, admin corrections, bans,
  seasons. Uses only the public `Leaderboard` API.
- `src/cli.js` - replays a command script against a fresh service (`node src/cli.js script.txt`).
- `src/format.js`, `src/validate.js` - CLI rendering and argument checks.
- `bench/` - `node bench/bench.js [players] [ops]` times the benchmark workload.

Run the tests with `node --test`.

## Leaderboard contract

Players are non-empty strings; scores are safe integers (`Number.isSafeInteger`), may be
negative, and may go down as well as up.

**Ranking order**: higher score first; equal scores are ordered by player name ascending,
comparing UTF-16 code units (JavaScript's `<` on strings, not `localeCompare`). So `"B"` comes
before `"a"`, and `"￿"` before `"😀"`.

**Rank**: 1 + the number of players with a strictly greater score. Tied players share a rank
(scores 30, 20, 20, 10 have ranks 1, 2, 2, 4).

An **entry** is a plain object `{ player, score, rank }`. Every method that returns entries
returns a new array of new objects in ranking order; mutating them does not affect the board.

| Member | Behaviour |
|---|---|
| `size` (getter) | number of players on the board |
| `has(player)` | whether the player is on the board |
| `score(player)` | the player's score, or `null` when absent |
| `update(player, score)` | **sets** the player's score (adds the player if new). Returns the previous score, or `null` for a new player |
| `remove(player)` | removes the player; returns `true` if the player was on the board, else `false` |
| `clear()` | removes every player |
| `rank(player)` | the player's rank, or `null` when absent |
| `topK(k)` | the first `k` entries (all entries when `k >= size`; `[]` for `k = 0`) |
| `around(player, n)` | the player's own entry with up to `n` entries before it and up to `n` after it, in ranking order (fewer at the ends of the board); `null` when the player is absent |
| `countInRange(lo, hi)` | number of players with `lo <= score <= hi` (both inclusive); `0` when `lo > hi`. Bounds may be any non-NaN numbers, including fractions and `±Infinity` |
| `percentile(p)` | nearest-rank percentile: with `n` players, the score at 1-based position `Math.ceil((p * n) / 100)` (this exact expression) of the scores sorted ascending; `null` when the board is empty |
| `entries()` | every entry |

Argument errors, checked before anything else (so `around("nobody", -1)` throws):

- `update`, `remove`, `rank`, `around`: a player that is not a non-empty string throws
  `TypeError("player must be a non-empty string")`. (`has` and `score` do not validate.)
- `update`: a score that is not a safe integer throws `TypeError("score must be a safe integer")`.
- `topK(k)` / `around(player, n)`: `k` / `n` not a non-negative safe integer throws
  `RangeError("k must be a non-negative integer")` / `RangeError("n must be a non-negative integer")`.
  For `around` the player is checked first, then `n`.
- `countInRange`: a bound that is not a number or is NaN throws `TypeError("lo must be a number")`
  / `TypeError("hi must be a number")`, `lo` checked first.
- `percentile(p)`: `p` not a number with `0 < p <= 100` throws `RangeError("p must be a number in (0, 100]")`
  (also on an empty board).

## Performance

The board must handle 100,000 players and 200,000 mixed operations (the shape of
`bench/workload.js`: about 40% updates, 5% removes, and rank, topK, around, countInRange and
percentile queries, with occasional `topK(2000)` and `around(player, 1000)`) in **under 4
seconds**, including the 100,000 initial inserts. This must also hold when scores are heavily
tied (every score in a range of 100 values) and when players are inserted with strictly
increasing scores. Every operation should be O(log n) (expected or amortized); `topK(k)` and
`around(player, n)` should cost O(log n + entries returned); `entries()` O(n).

## Service

`new LeaderboardService({ board, podiumSize = 3 })`:

- `submit(player, score)`: personal best only. Banned players get `{ accepted: false, reason: "banned" }`;
  a score not above the current one gets `{ accepted: false, reason: "not a personal best" }`;
  otherwise the score is set and `{ accepted: true, previous, rank }` is returned.
- `correct(player, score)`: admin override, sets the score even if lower; returns the previous score.
- `ban(player)` removes the player and blocks future submissions; `unban(player)` lifts it.
- `card(player, radius = 2)`: `{ player, score, rank, total, neighbours }` (`neighbours` is
  `around(player, radius)`), or `null` when absent.
- `bracket(lo, hi)` is `countInRange`; `cutoff(p)` is `percentile`.
- `closeSeason()` stores the `topK(podiumSize)` podium in the season history, clears the board
  and returns the podium; `seasons()` lists `{ season, podium }`. Bans carry over.
