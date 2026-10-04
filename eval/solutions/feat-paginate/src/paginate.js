export const MAX_PER_PAGE = 100;

/**
 * Return one page of `items` plus the metadata the HTTP layer needs.
 * @template T
 * @param {T[]} items
 * @param {{page?: number, perPage?: number}} [options]
 */
export function paginate(items, { page = 1, perPage = 20 } = {}) {
  if (!Array.isArray(items)) throw new TypeError("items must be an array");
  if (!Number.isInteger(page) || page < 1) throw new RangeError("page must be an integer >= 1");
  if (!Number.isInteger(perPage) || perPage < 1 || perPage > MAX_PER_PAGE) {
    throw new RangeError(`perPage must be an integer between 1 and ${MAX_PER_PAGE}`);
  }
  const total = items.length;
  const totalPages = Math.ceil(total / perPage);
  const start = (page - 1) * perPage;
  return {
    items: items.slice(start, start + perPage),
    page,
    perPage,
    total,
    totalPages,
    hasNext: page < totalPages,
    hasPrev: page > 1,
  };
}
