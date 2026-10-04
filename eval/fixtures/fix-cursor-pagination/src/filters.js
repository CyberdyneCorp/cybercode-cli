/** Filters: category (exact, case-sensitive) and an inclusive price range. */

export const FILTER_NAMES = ["category", "minPrice", "maxPrice"];

/** Builds the normalized filter object from already-validated params (absent filters omitted). */
export function buildFilters(params) {
  const filters = {};
  for (const name of FILTER_NAMES) {
    if (params[name] !== undefined) filters[name] = params[name];
  }
  return filters;
}

export function matchesFilters(item, filters) {
  if (filters.category !== undefined && item.category !== filters.category) return false;
  const priceFiltered = filters.minPrice !== undefined || filters.maxPrice !== undefined;
  if (priceFiltered && item.price === null) return false;
  if (filters.minPrice !== undefined && item.price < filters.minPrice) return false;
  if (filters.maxPrice !== undefined && item.price > filters.maxPrice) return false;
  return true;
}
