// Mutant: keys() removes expired entries (with reason "expire")
/** Size- and count-bounded LRU cache with per-entry TTL. See README.md for the contract. */

function checkTtl(ttl) {
  if (typeof ttl !== "number" || !Number.isFinite(ttl) || ttl < 0) {
    throw new RangeError("ttl must be a non-negative number");
  }
}

export class LRUCache {
  /** key -> { value, size, expiresAt }, in recency order: least recently used first. */
  #entries = new Map();
  #totalSize = 0;
  #stats = { hits: 0, misses: 0, evictions: 0 };
  #maxEntries;
  #maxSize;
  #sizeOf;
  #ttl;
  #now;
  #onEvict;

  constructor({ maxEntries = Infinity, maxSize = Infinity, sizeOf = () => 1, ttl = 0, now = Date.now, onEvict = () => {} } = {}) {
    if (maxEntries !== Infinity && (!Number.isInteger(maxEntries) || maxEntries < 1)) {
      throw new RangeError("maxEntries must be a positive integer");
    }
    if (typeof maxSize !== "number" || !(maxSize > 0)) {
      throw new RangeError("maxSize must be a positive number");
    }
    checkTtl(ttl);
    for (const [name, fn] of [["sizeOf", sizeOf], ["now", now], ["onEvict", onEvict]]) {
      if (typeof fn !== "function") throw new TypeError(`${name} must be a function`);
    }
    this.#maxEntries = maxEntries;
    this.#maxSize = maxSize;
    this.#sizeOf = sizeOf;
    this.#ttl = ttl;
    this.#now = now;
    this.#onEvict = onEvict;
  }

  /** Number of stored entries, including expired entries not removed yet. */
  get size() {
    return this.#entries.size;
  }

  /** Sum of the sizes of the stored entries, including expired entries not removed yet. */
  get totalSize() {
    return this.#totalSize;
  }

  /**
   * Store `value` under `key` as the most recently used entry. Returns false (and stores
   * nothing) when the value's size exceeds maxSize.
   */
  set(key, value, { ttl = this.#ttl } = {}) {
    checkTtl(ttl);
    const size = this.#sizeOf(value, key);
    if (typeof size !== "number" || !Number.isFinite(size) || size < 0) {
      throw new TypeError("sizeOf must return a non-negative finite number");
    }
    const existing = this.#live(key);
    if (existing !== undefined) this.#remove(key, existing, "set");
    if (size > this.#maxSize) return false;
    const expiresAt = ttl > 0 ? this.#now() + ttl : Infinity;
    this.#entries.set(key, { value, size, expiresAt });
    this.#totalSize += size;
    while (this.#entries.size > this.#maxEntries || this.#totalSize > this.#maxSize) {
      const [oldestKey, oldest] = this.#entries.entries().next().value;
      this.#remove(oldestKey, oldest, "evict");
    }
    return true;
  }

  /** The value, marking the entry most recently used; undefined (a miss) when absent or expired. */
  get(key) {
    const entry = this.#live(key);
    if (entry === undefined) {
      this.#stats.misses += 1;
      return undefined;
    }
    this.#stats.hits += 1;
    this.#entries.delete(key);
    this.#entries.set(key, entry);
    return entry.value;
  }

  /** The value without touching recency or stats; undefined when absent or expired. */
  peek(key) {
    return this.#live(key)?.value;
  }

  /** Whether a live (unexpired) entry exists. Does not touch recency or stats. */
  has(key) {
    return this.#live(key) !== undefined;
  }

  /** Remove a live entry. Returns true if one was removed. */
  delete(key) {
    const entry = this.#live(key);
    if (entry === undefined) return false;
    this.#remove(key, entry, "delete");
    return true;
  }

  /** Remove every entry without calling onEvict. Stats are kept. */
  clear() {
    this.#entries.clear();
    this.#totalSize = 0;
  }

  /** Remove every expired entry, least recently used first. Returns how many were removed. */
  prune() {
    let removed = 0;
    for (const [key, entry] of [...this.#entries]) {
      if (this.#expired(entry)) {
        this.#remove(key, entry, "expire");
        removed += 1;
      }
    }
    return removed;
  }

  /** Keys of live entries, most recently used first. Does not remove expired entries. */
  keys() {
    this.prune();
    return [...this.#entries.keys()].reverse();
  }

  stats() {
    return { ...this.#stats };
  }

  #expired(entry) {
    return this.#now() >= entry.expiresAt;
  }

  /** The entry if present and live; an expired entry is removed (reason "expire"). */
  #live(key) {
    const entry = this.#entries.get(key);
    if (entry === undefined) return undefined;
    if (this.#expired(entry)) {
      this.#remove(key, entry, "expire");
      return undefined;
    }
    return entry;
  }

  #remove(key, entry, reason) {
    this.#entries.delete(key);
    this.#totalSize -= entry.size;
    if (reason === "evict") this.#stats.evictions += 1;
    this.#onEvict(key, entry.value, reason);
  }
}
