/**
 * In-memory item store. Items are frozen copies; missing optional fields are stored as null.
 *
 * Item shape: { id: string, name: string|null, price: number|null,
 *               updatedAt: string|null, category: string }
 */

const ISO_MS = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/;

function normalize(item) {
  if (typeof item?.id !== "string" || item.id === "") throw new TypeError("item.id must be a non-empty string");
  const normalized = {
    id: item.id,
    name: item.name ?? null,
    price: item.price ?? null,
    updatedAt: item.updatedAt ?? null,
    category: item.category ?? "",
  };
  if (normalized.name !== null && typeof normalized.name !== "string") {
    throw new TypeError(`item ${item.id}: name must be a string or null`);
  }
  if (normalized.price !== null && !(Number.isFinite(normalized.price) && normalized.price >= 0)) {
    throw new TypeError(`item ${item.id}: price must be a finite number >= 0 or null`);
  }
  if (normalized.updatedAt !== null && !ISO_MS.test(normalized.updatedAt)) {
    throw new TypeError(`item ${item.id}: updatedAt must be an ISO-8601 UTC timestamp with milliseconds`);
  }
  if (typeof normalized.category !== "string") throw new TypeError(`item ${item.id}: category must be a string`);
  return Object.freeze(normalized);
}

export class ItemStore {
  #items = new Map();

  constructor(items = []) {
    for (const item of items) this.insert(item);
  }

  get size() {
    return this.#items.size;
  }

  insert(item) {
    const normalized = normalize(item);
    if (this.#items.has(normalized.id)) throw new Error(`duplicate id: ${normalized.id}`);
    this.#items.set(normalized.id, normalized);
    return normalized;
  }

  delete(id) {
    return this.#items.delete(id);
  }

  get(id) {
    return this.#items.get(id) ?? null;
  }

  all() {
    return [...this.#items.values()];
  }
}
