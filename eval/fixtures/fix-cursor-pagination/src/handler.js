/**
 * GET /items as a function: listItems(params) -> { items, nextCursor, prevCursor }.
 * See README.md for the pagination contract.
 */

import { ItemRepository } from "./repository.js";
import { Query, after, before } from "./query.js";
import { sortKey } from "./sort.js";
import { queryFingerprint, encodeCursor, decodeCursor } from "./cursor.js";
import { validateParams } from "./validation.js";

function readPage(rows, sort, position, limit) {
  const query = new Query(rows);
  if (!position) return query.limit(limit).run();
  if (position.direction === "next") return query.where(after(sort, position.key)).limit(limit).run();
  return query.where(before(sort, position.key)).fromEnd().limit(limit).run();
}

export function listItems(repository, params = {}) {
  const { sort, limit, filters, cursor } = validateParams(params);
  const fingerprint = queryFingerprint(sort, filters);
  const position = cursor === null ? null : decodeCursor(cursor, { fingerprint, field: sort.field });

  const rows = repository.find(sort, filters);
  const items = readPage(rows, sort, position, limit);
  if (items.length === 0) return { items, nextCursor: null, prevCursor: null };

  const cursorAt = (direction, item) =>
    encodeCursor({ fingerprint, direction, field: sort.field, key: sortKey(item, sort) });
  const first = items[0];
  const last = items[items.length - 1];

  const hasNext = new Query(rows).where(after(sort, sortKey(last, sort))).exists();
  const hasPrev =
    position?.direction === "next" || new Query(rows).where(before(sort, sortKey(first, sort))).exists();

  return {
    items,
    nextCursor: hasNext ? cursorAt("next", last) : null,
    prevCursor: hasPrev ? cursorAt("prev", first) : null,
  };
}

/** Binds the handler to a store. The repository (and its view cache) lives as long as the API. */
export function createApi(store) {
  const repository = new ItemRepository(store);
  return { listItems: (params = {}) => listItems(repository, params) };
}
