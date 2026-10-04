import { test } from "node:test";
import assert from "node:assert/strict";

import { ItemStore, createApi } from "../src/index.js";

function walk(api, params) {
  const pages = [];
  let cursor = null;
  do {
    const page = api.listItems({ ...params, cursor });
    pages.push(page.items.map((item) => item.id));
    cursor = page.nextCursor;
  } while (cursor);
  return pages;
}

test("walks all items in updatedAt order", () => {
  const store = new ItemStore([
    { id: "c", name: "Chair", price: 40, updatedAt: "2024-01-03T09:00:00.000Z", category: "home" },
    { id: "a", name: "Lamp", price: 25, updatedAt: "2024-01-01T09:00:00.000Z", category: "home" },
    { id: "e", name: "Desk", price: 120, updatedAt: "2024-01-05T09:00:00.000Z", category: "office" },
    { id: "b", name: "Mug", price: 8, updatedAt: "2024-01-02T09:00:00.000Z", category: "kitchen" },
    { id: "d", name: "Rug", price: 60, updatedAt: "2024-01-04T09:00:00.000Z", category: "home" },
  ]);
  assert.deepEqual(walk(createApi(store), { limit: 2 }), [["a", "b"], ["c", "d"], ["e"]]);
});

test("walks items with equal prices exactly once", () => {
  const store = new ItemStore([
    { id: "p1", name: "Pen", price: 2, updatedAt: "2024-02-01T00:00:00.000Z", category: "office" },
    { id: "p2", name: "Pencil", price: 2, updatedAt: "2024-02-02T00:00:00.000Z", category: "office" },
    { id: "p3", name: "Eraser", price: 2, updatedAt: "2024-02-03T00:00:00.000Z", category: "office" },
    { id: "p4", name: "Stapler", price: 9, updatedAt: "2024-02-04T00:00:00.000Z", category: "office" },
  ]);
  assert.deepEqual(walk(createApi(store), { sort: "price", limit: 2 }), [["p1", "p2"], ["p3", "p4"]]);
});
