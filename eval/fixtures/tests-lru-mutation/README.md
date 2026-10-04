# lru-cache

`src/lruCache.js` (Node 22, ES modules, no dependencies) exports `LRUCache`, a least-recently-used
cache bounded by entry count and by total size, with per-entry time-to-live. The code is correct
but untested. Tests go under `test/` as `*.test.js` files using `node:test` and `node:assert`,
run with `node --test`.

## Contract

### Construction

`new LRUCache({ maxEntries = Infinity, maxSize = Infinity, sizeOf = () => 1, ttl = 0, now = Date.now, onEvict = () => {} } = {})`

- `maxEntries`: a positive integer or `Infinity`, else `RangeError("maxEntries must be a positive integer")`.
- `maxSize`: a number `> 0` (`Infinity` allowed), else `RangeError("maxSize must be a positive number")`.
- `ttl`: default time-to-live in ms, a finite number `>= 0`; `0` means entries never expire. Else
  `RangeError("ttl must be a non-negative number")`.
- `sizeOf(value, key)`, `now()` and `onEvict(key, value, reason)` must be functions, else
  `TypeError("<name> must be a function")`. `now()` is the clock (ms) used for all expiry.

### Recency

Entries are kept in recency order. **Only `get` (a hit) and `set` make an entry the most
recently used.** `peek`, `has`, `keys`, `delete` and `prune` never change the order.

### Expiry

An entry stored with a ttl `t > 0` at time `now() = s` expires at `s + t`: it is **expired when
`now() >= s + t`**. An entry with ttl `0` never expires. Reading an entry (`get`, `peek`, `has`)
never extends its expiry; only `set` gives it a new one.

Expired entries are removed lazily: `get`, `peek`, `has`, `delete` and `set` remove an expired
entry for the key they are called with (reason `"expire"`), and `prune()` removes all of them.
Until removed, an expired entry still counts in `size` and `totalSize` (and in eviction, below),
but it is never returned and never listed by `keys()`.

### Methods

- `set(key, value, { ttl } = {})`: stores the value as the most recently used entry with size
  `sizeOf(value, key)` and the given ttl (default: the constructor's `ttl`; an explicit `0` means
  never expires, even when the default is not 0; an invalid ttl throws the RangeError above).
  `sizeOf` must return a finite number `>= 0`, else `TypeError("sizeOf must return a non-negative finite number")`.
  Steps, in order:
  1. If the key has an entry: an expired one is removed with reason `"expire"`; a live one is
     removed with reason `"set"` (also when the new value is identical).
  2. If the new size is greater than `maxSize`, nothing is stored and `set` returns `false`
     (so a previous entry for the key is gone). A size exactly equal to `maxSize` is accepted.
  3. Otherwise the entry is stored (its ttl starts now), then least recently used entries are
     evicted (reason `"evict"`), one at a time, until `size <= maxEntries` and
     `totalSize <= maxSize` both hold; the entry just stored is never evicted. Returns `true`.
- `get(key)`: the value of a live entry, making it the most recently used (a **hit**); otherwise
  `undefined` (a **miss**; an expired entry is removed).
- `peek(key)`: the value of a live entry or `undefined`; no recency change, not counted in stats.
- `has(key)`: `true` for a live entry, `false` otherwise (also for an expired one); no recency
  change, not counted in stats.
- `delete(key)`: removes a live entry with reason `"delete"` and returns `true`; returns `false`
  when there is none (an expired entry is removed with reason `"expire"`, and `false` is returned).
- `clear()`: removes every entry **without** calling `onEvict`. Stats are not reset.
- `prune()`: removes every expired entry (reason `"expire"`, least recently used first) and
  returns how many it removed.
- `keys()`: array of the keys of live entries, most recently used first. It does not remove
  expired entries.
- `size` / `totalSize` (getters): number of stored entries / sum of their sizes.
- `stats()`: a new object `{ hits, misses, evictions }`. `hits` and `misses` count `get` calls only;
  `evictions` counts only removals with reason `"evict"` (not expirations, deletes or replacements).

### onEvict

`onEvict(key, value, reason)` is called once for each entry removed for one of the reasons
`"evict"` (capacity), `"expire"`, `"delete"` (`delete()`) or `"set"` (replaced or removed by `set`
for the same key), with the removed entry's key and value. It is called **after** the entry has
been removed (inside the callback, `has(key)` is false and `size`/`totalSize` no longer include
it). It is not called by `clear()`.
