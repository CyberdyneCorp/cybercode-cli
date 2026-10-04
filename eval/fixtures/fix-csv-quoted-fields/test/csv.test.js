import { test } from "node:test";
import assert from "node:assert/strict";

import { parseCsv } from "../src/csv.js";
import { totalsByCategory } from "../src/report.js";

test("parses simple records", () => {
  assert.deepEqual(parseCsv("a,b\n1,2\n"), [["a", "b"], ["1", "2"]]);
});

test("totals per category", () => {
  const text = "category,amount\nfood,10\ntravel,5\nfood,2.5";
  assert.deepEqual(totalsByCategory(text), { food: 12.5, travel: 5 });
});
