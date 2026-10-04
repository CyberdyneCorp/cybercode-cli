/**
 * Map over `items` with at most `limit` iterator calls in flight. See README.md ("mapLimit").
 */
import { once } from "./once.js";

/**
 * Calls `iterator(item, index, cb)` for each item, starting them in index order, never more
 * than `limit` at a time; a new one starts as soon as one finishes. Calls `cb(null, results)`
 * with results in input order. On the first error, starts nothing new and calls `cb(error)`
 * immediately; items still in flight are left to finish and their outcomes are ignored.
 */
export function mapLimit(items, limit, iterator, cb) {
  if (!Array.isArray(items)) {
    process.nextTick(cb, new TypeError("items must be an array"));
    return;
  }
  if (!Number.isInteger(limit) || limit < 1) {
    process.nextTick(cb, new RangeError("limit must be a positive integer"));
    return;
  }
  const results = new Array(items.length);
  let started = 0;
  let finished = 0;
  let failed = false;

  if (items.length === 0) {
    process.nextTick(cb, null, results);
    return;
  }

  const launch = () => {
    const index = started;
    started += 1;
    iterator(items[index], index, once((error, value) => {
      if (failed) return;
      if (error) {
        failed = true;
        cb(error);
        return;
      }
      results[index] = value;
      finished += 1;
      if (finished === items.length) cb(null, results);
      else if (started < items.length) launch();
    }));
  };

  while (started < Math.min(limit, items.length)) launch();
}
