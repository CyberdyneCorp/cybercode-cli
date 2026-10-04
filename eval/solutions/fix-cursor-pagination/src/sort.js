/**
 * Sort specifications and the item ordering.
 *
 * A sort spec is a field name, optionally prefixed with "-" for descending order:
 * "name", "-price", "updatedAt", ... Every ordering ends with `id` as the unique tie-breaker.
 */

export const SORT_FIELDS = ["name", "price", "updatedAt"];
export const DEFAULT_SORT = "updatedAt";

export function parseSort(spec) {
  const descending = spec.startsWith("-");
  const field = descending ? spec.slice(1) : spec;
  if (!SORT_FIELDS.includes(field)) return null;
  return { spec, field, direction: descending ? "desc" : "asc" };
}

/** UTF-16 code unit order, the order of `<` on strings. */
export function compareStrings(a, b) {
  if (a < b) return -1;
  return a > b ? 1 : 0;
}

export const compareIds = compareStrings;

// Ascending comparison of two non-null values of each field. Timestamps all have the
// fixed-width `toISOString` format, so code unit order is chronological order.
const compareByField = {
  name: compareStrings,
  price: (a, b) => a - b,
  updatedAt: compareStrings,
};

/** The sort key of an item: [value of the sort field, id]. */
export function sortKey(item, sort) {
  return [item[sort.field] ?? null, item.id];
}

/**
 * Compares two sort keys in the order of `sort`. Null values sort last in both directions;
 * the direction applies to the value and to the id tie-breaker.
 */
export function compareKeys(sort, [valueA, idA], [valueB, idB]) {
  const nullA = valueA === null;
  const nullB = valueB === null;
  if (nullA !== nullB) return nullA ? 1 : -1;
  const ascending = (nullA ? 0 : compareByField[sort.field](valueA, valueB)) || compareIds(idA, idB);
  return sort.direction === "desc" ? -ascending : ascending;
}

export function compareItems(sort, a, b) {
  return compareKeys(sort, sortKey(a, sort), sortKey(b, sort));
}
