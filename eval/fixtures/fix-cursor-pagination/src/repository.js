/**
 * Read side of the store: sorted views, cached per sort spec because sorting the whole
 * catalog on every page request is the expensive part of a request.
 */

import { compareItems } from "./sort.js";
import { matchesFilters } from "./filters.js";

export class ItemRepository {
  #store;
  #views = new Map();

  constructor(store) {
    this.#store = store;
  }

  /** All items in `sort` order. */
  sorted(sort) {
    const cached = this.#views.get(sort.spec);
    if (cached && cached.size === this.#store.size) return cached.items;
    const items = this.#store.all().sort((a, b) => compareItems(sort, a, b));
    this.#views.set(sort.spec, { size: this.#store.size, items });
    return items;
  }

  /** Items matching `filters`, in `sort` order. */
  find(sort, filters) {
    return this.sorted(sort).filter((item) => matchesFilters(item, filters));
  }
}
