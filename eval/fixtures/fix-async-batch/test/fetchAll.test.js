import { test } from "node:test";
import assert from "node:assert/strict";

import { fetchAll } from "../src/fetchAll.js";
import { retry } from "../src/retry.js";

const later = (value, ms) => new Promise((resolve) => setTimeout(() => resolve(value), ms));

test("fetches every url", async () => {
  const results = await fetchAll(["a", "b"], (url) => later(url.toUpperCase(), 5));
  assert.deepEqual(results, ["A", "B"]);
});

test("retry returns the first success", async () => {
  assert.equal(await retry(async () => "ok"), "ok");
});
