import { test } from "node:test";
import assert from "node:assert/strict";

import { paginate } from "../src/paginate.js";

test("returns the requested page with metadata", () => {
  const page = paginate([1, 2, 3, 4, 5], { page: 2, perPage: 2 });
  assert.deepEqual(page, { items: [3, 4], page: 2, perPage: 2, total: 5, totalPages: 3, hasNext: true, hasPrev: true });
});

test("rejects invalid page sizes", () => {
  assert.throws(() => paginate([], { perPage: 0 }), RangeError);
});
