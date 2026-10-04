/**
 * A tiny query builder over the sorted rows of the repository, shaped like the SQL it
 * replaced:  SELECT ... WHERE <keyset predicate> ORDER BY <sort> LIMIT n.
 */

import { compareKeys, sortKey } from "./sort.js";

export class Query {
  #rows;
  #predicates = [];
  #limit = Infinity;
  #fromEnd = false;

  /** `rows` must already be in sort order. */
  constructor(rows) {
    this.#rows = rows;
  }

  where(predicate) {
    this.#predicates.push(predicate);
    return this;
  }

  limit(n) {
    this.#limit = n;
    return this;
  }

  /** Take rows from the end of the ordering instead of the start (ORDER BY ... reversed). */
  fromEnd() {
    this.#fromEnd = true;
    return this;
  }

  /** Matching rows, always returned in sort order. */
  run() {
    const matching = this.#rows.filter((row) => this.#predicates.every((predicate) => predicate(row)));
    if (!this.#fromEnd) return matching.slice(0, this.#limit);
    return matching.slice(Math.max(0, matching.length - this.#limit));
  }

  exists() {
    return this.#rows.some((row) => this.#predicates.every((predicate) => predicate(row)));
  }
}

/** WHERE (sort field, id) > (:value, :id)   in the ordering of `sort` */
export function after(sort, key) {
  return (row) => compareKeys(sort, sortKey(row, sort), key) > 0;
}

/** WHERE (sort field, id) < (:value, :id)   in the ordering of `sort` */
export function before(sort, key) {
  return (row) => compareKeys(sort, sortKey(row, sort), key) < 0;
}
