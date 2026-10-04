import { test } from "node:test";
import assert from "node:assert/strict";

import { searchProducts } from "../src/products.js";

const catalog = [
  { sku: "a1", name: "Red Mug" },
  { sku: "a2", name: "Blue mug" },
  { sku: "a3", name: "Teapot" },
];

test("matches names case-insensitively", () => {
  assert.deepEqual(searchProducts(catalog, " MUG ").map((p) => p.sku), ["a1", "a2"]);
});

test("returns an empty list when nothing matches", () => {
  assert.deepEqual(searchProducts(catalog, "kettle"), []);
});
