import { test } from "node:test";
import assert from "node:assert/strict";

import { paginate } from "./src/paginate.js";
import { searchProducts } from "./src/products.js";

const range = (n) => Array.from({ length: n }, (_, i) => i + 1);

test("visible: search still works", () => {
  const catalog = [{ sku: "a1", name: "Red Mug" }, { sku: "a2", name: "Blue mug" }, { sku: "a3", name: "Teapot" }];
  assert.deepEqual(searchProducts(catalog, " MUG ").map((p) => p.sku), ["a1", "a2"]);
});

test("defaults to page 1 of 20", () => {
  assert.deepEqual(paginate(range(45)), {
    items: range(20), page: 1, perPage: 20, total: 45, totalPages: 3, hasNext: true, hasPrev: false,
  });
  assert.deepEqual(paginate(range(45), {}).items, range(20));
  assert.equal(paginate(range(45), { page: 2 }).perPage, 20);
  assert.equal(paginate(range(45), { perPage: 5 }).page, 1);
});

test("middle and last pages", () => {
  assert.deepEqual(paginate(range(45), { page: 2, perPage: 20 }), {
    items: range(40).slice(20), page: 2, perPage: 20, total: 45, totalPages: 3, hasNext: true, hasPrev: true,
  });
  assert.deepEqual(paginate(range(45), { page: 3, perPage: 20 }), {
    items: [41, 42, 43, 44, 45], page: 3, perPage: 20, total: 45, totalPages: 3, hasNext: false, hasPrev: true,
  });
});

test("exact multiple of perPage", () => {
  const last = paginate(range(10), { page: 2, perPage: 5 });
  assert.deepEqual(last.items, [6, 7, 8, 9, 10]);
  assert.equal(last.totalPages, 2);
  assert.equal(last.hasNext, false);
});

test("empty input has zero pages", () => {
  assert.deepEqual(paginate([]), {
    items: [], page: 1, perPage: 20, total: 0, totalPages: 0, hasNext: false, hasPrev: false,
  });
});

test("page past the end returns no items", () => {
  assert.deepEqual(paginate(range(5), { page: 4, perPage: 2 }), {
    items: [], page: 4, perPage: 2, total: 5, totalPages: 3, hasNext: false, hasPrev: true,
  });
});

test("perPage bounds", () => {
  assert.equal(paginate(range(150), { perPage: 100 }).items.length, 100);
  assert.equal(paginate(range(3), { perPage: 1, page: 3 }).items[0], 3);
  for (const perPage of [0, -1, 101, 2.5, NaN, "10", null]) {
    assert.throws(() => paginate(range(3), { perPage }), RangeError, `perPage=${perPage}`);
  }
});

test("page validation", () => {
  for (const page of [0, -2, 1.5, NaN, Infinity, "2", null]) {
    assert.throws(() => paginate(range(3), { page }), RangeError, `page=${page}`);
  }
});

test("non-array items is a TypeError", () => {
  for (const items of [null, undefined, "abc", { length: 2 }]) {
    assert.throws(() => paginate(items), TypeError);
  }
});

test("returns a new array and leaves the input untouched", () => {
  const items = range(4);
  const page = paginate(items, { perPage: 10 });
  assert.notEqual(page.items, items);
  page.items.push(99);
  assert.deepEqual(items, [1, 2, 3, 4]);
});

test("works on search results", () => {
  const catalog = range(30).map((i) => ({ sku: `s${i}`, name: i % 2 ? `Mug ${i}` : `Cup ${i}` }));
  const page = paginate(searchProducts(catalog, "mug"), { page: 2, perPage: 10 });
  assert.deepEqual(page.items.map((p) => p.sku), ["s21", "s23", "s25", "s27", "s29"]);
  assert.equal(page.total, 15);
});
