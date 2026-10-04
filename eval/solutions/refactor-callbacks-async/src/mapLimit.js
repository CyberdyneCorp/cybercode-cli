/**
 * Map over `items` with at most `limit` iterator calls in flight. See README.md ("mapLimit").
 */

/**
 * Calls `iterator(item, index)` for each item, starting them in index order, never more than
 * `limit` at a time; a new one starts as soon as one finishes. Resolves with the results in
 * input order. On the first rejection, starts nothing new and rejects immediately with that
 * error; items still in flight are left to finish and their outcomes are ignored.
 */
export async function mapLimit(items, limit, iterator) {
  if (!Array.isArray(items)) throw new TypeError("items must be an array");
  if (!Number.isInteger(limit) || limit < 1) throw new RangeError("limit must be a positive integer");
  const results = new Array(items.length);
  let next = 0;
  let failed = false;

  const worker = async () => {
    while (!failed && next < items.length) {
      const index = next;
      next += 1;
      try {
        results[index] = await iterator(items[index], index);
      } catch (error) {
        failed = true;
        throw error;
      }
    }
  };

  const workers = Array.from({ length: Math.min(limit, items.length) }, worker);
  await Promise.all(workers);
  return results;
}
