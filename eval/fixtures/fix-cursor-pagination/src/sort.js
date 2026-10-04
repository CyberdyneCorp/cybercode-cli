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

const isNull = (value) => !value;

const compareByField = {
  name: (a, b) => a.localeCompare(b),
  price: (a, b) => a - b,
  updatedAt: (a, b) => Date.parse(a) - Date.parse(b),
};

export function compareIds(a, b) {
  if (a < b) return -1;
  return a > b ? 1 : 0;
}

/** Ascending comparison of two field values; nulls sort after every non-null value. */
export function compareValues(field, a, b) {
  if (isNull(a) || isNull(b)) {
    if (isNull(a) && isNull(b)) return 0;
    return isNull(a) ? 1 : -1;
  }
  return compareByField[field](a, b);
}

/** The sort key of an item: [value of the sort field, id]. */
export function sortKey(item, sort) {
  return [item[sort.field] ?? null, item.id];
}

/** Compares two sort keys in the order of `sort` (direction included). */
export function compareKeys(sort, [valueA, idA], [valueB, idB]) {
  const ascending = compareValues(sort.field, valueA, valueB) || compareIds(idA, idB);
  return sort.direction === "desc" ? -ascending : ascending;
}

export function compareItems(sort, a, b) {
  return compareKeys(sort, sortKey(a, sort), sortKey(b, sort));
}
