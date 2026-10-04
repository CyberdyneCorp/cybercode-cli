/**
 * A tiny query builder over the sorted rows of the repository, shaped like the SQL it
 * replaced:  SELECT ... WHERE <keyset predicate> ORDER BY <sort> LIMIT n.
 */

import { compareValues } from "./sort.js";

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

function directed(sort, comparison) {
  return sort.direction === "desc" ? -comparison : comparison;
}

/** WHERE <sort field> > :value   (in the direction of the sort) */
export function after(sort, key) {
  const [value] = key;
  return (row) => directed(sort, compareValues(sort.field, row[sort.field], value)) > 0;
}

/** WHERE <sort field> < :value   (in the direction of the sort) */
export function before(sort, key) {
  const [value] = key;
  return (row) => directed(sort, compareValues(sort.field, row[sort.field], value)) <= 0;
}
